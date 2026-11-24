//! Scrolling capture: scrolls the window under the selected area with the mouse wheel,
//! grabs a frame after each step and stitches the frames by matching overlapping rows.

use crate::capture::{Image, Rect};

fn row_hashes(img: &Image, x0: u32, x1: u32) -> (Vec<u64>, Vec<bool>) {
    let mut hs = Vec::with_capacity(img.h as usize);
    let mut info = Vec::with_capacity(img.h as usize);
    for y in 0..img.h {
        let row = &img.px[((y * img.w + x0) * 4) as usize..((y * img.w + x1) * 4) as usize];
        // FNV-1a over the row; "informative" = not one flat colour.
        let mut h: u64 = 0xcbf29ce484222325;
        let first = u32::from_le_bytes([row[0], row[1], row[2], 0]);
        let mut varied = false;
        for p in row.chunks_exact(4) {
            let v = u32::from_le_bytes([p[0], p[1], p[2], 0]);
            if v != first {
                varied = true;
            }
            h ^= v as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        hs.push(h);
        info.push(varied);
    }
    (hs, info)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Rows the content moved up by.
    pub shift: u32,
    /// Identical rows pinned at the top (sticky header) and bottom (footer).
    pub head: u32,
    pub foot: u32,
}

/// Finds how far `cur` scrolled relative to `prev` (same size). None = no confident match.
pub fn find_shift(prev: &Image, cur: &Image) -> Option<Match> {
    let h = prev.h as usize;
    if h < 24 || prev.w != cur.w || prev.h != cur.h {
        return None;
    }
    // Ignore a strip on the right where scrollbars move.
    let margin = (prev.w / 30).clamp(0, 28);
    let x1 = (prev.w - margin).max(1);
    let (a, ai) = row_hashes(prev, 0, x1);
    let (b, _) = row_hashes(cur, 0, x1);
    if a == b {
        return None;
    }
    let mut head = 0;
    while head < h && a[head] == b[head] {
        head += 1;
    }
    let mut foot = 0;
    while foot < h - head && a[h - 1 - foot] == b[h - 1 - foot] {
        foot += 1;
    }
    // Keep at most a third of the frame as fixed header/footer.
    let head = head.min(h / 3);
    let foot = foot.min(h / 3);
    let body = h - head - foot;
    let mut best: Option<(f32, usize, usize)> = None;
    for s in 1..body.saturating_sub(8) {
        let len = body - s;
        let mut hit = 0usize;
        let mut total = 0usize;
        for y in head..head + len {
            if ai[y + s] {
                total += 1;
                if a[y + s] == b[y] {
                    hit += 1;
                }
            }
        }
        if total < 6 {
            continue;
        }
        let score = hit as f32 / total as f32;
        if score > 0.92 && best.is_none_or(|(bs, _, bt)| score > bs + 0.001 || ((score - bs).abs() <= 0.001 && total > bt)) {
            best = Some((score, s, total));
        }
    }
    best.map(|(_, s, _)| Match { shift: s as u32, head: head as u32, foot: foot as u32 })
}

/// Grows a tall image frame by frame.
pub struct Stitcher {
    pub out: Image,
    last: Image,
    foot: Option<u32>,
    pub frames: u32,
    pub stalls: u32,
}

impl Stitcher {
    pub fn new(first: Image) -> Self {
        Self { out: first.clone(), last: first, foot: None, frames: 1, stalls: 0 }
    }

    /// Adds a frame; returns false when nothing moved (the end was reached or matching failed).
    pub fn push(&mut self, cur: Image) -> bool {
        let Some(m) = find_shift(&self.last, &cur) else {
            self.stalls += 1;
            return false;
        };
        let foot = *self.foot.get_or_insert(m.foot);
        // Drop the old footer, append the newly revealed rows, keep the footer for later.
        let h = cur.h;
        let keep = self.out.h - foot.min(self.out.h);
        let new_rows = m.shift.min(h - foot);
        let from = h - foot - new_rows;
        let mut out = Image::new(cur.w, keep + new_rows);
        let n = (keep * cur.w * 4) as usize;
        out.px[..n].copy_from_slice(&self.out.px[..n]);
        let src = &cur.px[(from * cur.w * 4) as usize..((from + new_rows) * cur.w * 4) as usize];
        out.px[n..].copy_from_slice(src);
        self.out = out;
        self.last = cur;
        self.frames += 1;
        self.stalls = 0;
        true
    }

    /// The finished image with the footer from the last frame put back.
    pub fn finish(mut self) -> Image {
        let foot = self.foot.unwrap_or(0).min(self.last.h);
        if foot > 0 {
            let foot_rows = self.last.crop(0, (self.last.h - foot) as i32, self.last.w as i32, foot as i32);
            let mut out = Image::new(self.out.w, self.out.h + foot);
            out.px[..self.out.px.len()].copy_from_slice(&self.out.px);
            out.px[self.out.px.len()..].copy_from_slice(&foot_rows.px);
            self.out = out;
        }
        self.out
    }
}

/// Sends wheel notches (negative = scroll down) at the area's centre.
pub fn wheel(area: Rect, notches: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(area.x + area.w / 2, area.y + area.h / 2);
        let inp = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: (notches * 120) as u32, dwFlags: MOUSEEVENTF_WHEEL, ..Default::default() } },
        };
        SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
    }
}

pub const MAX_HEIGHT: u32 = 40000;

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall synthetic "page" with distinct rows, plus a sticky header.
    fn page(h: u32) -> Image {
        let mut img = Image::new(64, h);
        for y in 0..h {
            for x in 0..64u32 {
                let i = ((y * 64 + x) * 4) as usize;
                let v = ((y * 7919 + x * 31) % 251) as u8;
                img.px[i..i + 4].copy_from_slice(&[v, (y % 256) as u8, (x * 3) as u8, 255]);
            }
        }
        img
    }

    fn view(p: &Image, top: u32, h: u32, header: u32) -> Image {
        let mut v = p.crop(0, top as i32, 64, h as i32);
        let hd = p.crop(0, 0, 64, header as i32);
        v.blit(&hd, 0, 0);
        v
    }

    #[test]
    fn stitches_with_header() {
        let p = page(600);
        let mut st = Stitcher::new(view(&p, 0, 200, 20));
        let mut top = 0;
        while top + 200 < 600 {
            top = (top + 70).min(400);
            assert!(st.push(view(&p, top, 200, 20)));
        }
        assert!(!st.push(view(&p, 400, 200, 20)));
        let out = st.finish();
        assert_eq!(out.h, 600);
        assert_eq!(out.px, view(&p, 0, 600, 20).px);
    }

    #[test]
    fn identical_is_none() {
        let p = page(100);
        assert_eq!(find_shift(&p, &p), None);
    }
}
