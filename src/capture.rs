//! Screen grabbing (GDI BitBlt per monitor), monitor list and window rectangles for snapping.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::BOOL;

/// BGRA pixels, top-down, alpha = 255.
#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Image {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h, px: vec![0; (w * h * 4) as usize] }
    }

    /// Copies a sub-rectangle (clamped).
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> Image {
        let x0 = x.clamp(0, self.w as i32);
        let y0 = y.clamp(0, self.h as i32);
        let x1 = (x + w).clamp(0, self.w as i32);
        let y1 = (y + h).clamp(0, self.h as i32);
        let (cw, ch) = ((x1 - x0).max(0) as u32, (y1 - y0).max(0) as u32);
        let mut out = Image::new(cw, ch);
        for row in 0..ch {
            let s = (((y0 as u32 + row) * self.w + x0 as u32) * 4) as usize;
            let d = (row * cw * 4) as usize;
            out.px[d..d + (cw * 4) as usize].copy_from_slice(&self.px[s..s + (cw * 4) as usize]);
        }
        out
    }

    /// Copies `src` into self at (x, y), clipped.
    pub fn blit(&mut self, src: &Image, x: i32, y: i32) {
        for row in 0..src.h as i32 {
            let dy = y + row;
            if dy < 0 || dy >= self.h as i32 {
                continue;
            }
            let sx0 = (-x).max(0);
            let sx1 = (src.w as i32).min(self.w as i32 - x);
            if sx1 <= sx0 {
                continue;
            }
            let s = ((row * src.w as i32 + sx0) * 4) as usize;
            let d = ((dy * self.w as i32 + x + sx0) * 4) as usize;
            let n = ((sx1 - sx0) * 4) as usize;
            self.px[d..d + n].copy_from_slice(&src.px[s..s + n]);
        }
    }

    /// Area-averaged downscale to fit inside `mw`×`mh` (never upscales).
    pub fn fit(&self, mw: u32, mh: u32) -> Image {
        let s = (mw as f64 / self.w as f64).min(mh as f64 / self.h as f64).min(1.0);
        let (w, h) = (((self.w as f64 * s).round() as u32).max(1), ((self.h as f64 * s).round() as u32).max(1));
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut out = Image::new(w, h);
        for y in 0..h {
            let sy0 = (y as u64 * self.h as u64 / h as u64) as u32;
            let sy1 = (((y + 1) as u64 * self.h as u64 / h as u64) as u32).max(sy0 + 1).min(self.h);
            for x in 0..w {
                let sx0 = (x as u64 * self.w as u64 / w as u64) as u32;
                let sx1 = (((x + 1) as u64 * self.w as u64 / w as u64) as u32).max(sx0 + 1).min(self.w);
                let mut acc = [0u32; 3];
                let mut n = 0u32;
                for yy in sy0..sy1 {
                    let row = (yy * self.w) as usize * 4;
                    for xx in sx0..sx1 {
                        let i = row + xx as usize * 4;
                        acc[0] += self.px[i] as u32;
                        acc[1] += self.px[i + 1] as u32;
                        acc[2] += self.px[i + 2] as u32;
                        n += 1;
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                out.px[o..o + 4].copy_from_slice(&[(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, 255]);
            }
        }
        out
    }

    pub fn to_rgba(&self) -> Vec<u8> {
        let mut v = self.px.clone();
        for p in v.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        v
    }

    pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Image {
        let mut px = rgba.to_vec();
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        Image { w, h, px }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_points(a: (i32, i32), b: (i32, i32)) -> Self {
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Self { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 }
    }
    pub fn from_win(r: RECT) -> Self {
        Self { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
    pub fn area(&self) -> i64 {
        self.w as i64 * self.h as i64
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub rect: Rect,
    pub work: Rect,
    pub dpi: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        unsafe {
            let v = &mut *(lp.0 as *mut Vec<Monitor>);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hm, &mut mi).as_bool() {
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                v.push(Monitor {
                    rect: Rect::from_win(mi.rcMonitor),
                    work: Rect::from_win(mi.rcWork),
                    dpi: dx.max(96),
                    primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
            true.into()
        }
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// Grabs one rectangle of the desktop (virtual-screen coordinates).
pub fn grab(r: Rect, cursor: bool) -> Image {
    let mut img = Image::new(r.w.max(1) as u32, r.h.max(1) as u32);
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: img.w as i32,
                biHeight: -(img.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        if let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, dib.into());
            let _ = BitBlt(mem, 0, 0, img.w as i32, img.h as i32, Some(screen), r.x, r.y, SRCCOPY);
            if cursor {
                draw_cursor(mem, r.x, r.y);
            }
            let _ = GdiFlush();
            std::ptr::copy_nonoverlapping(bits as *const u8, img.px.as_mut_ptr(), img.px.len());
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
    for p in img.px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    img
}

pub unsafe fn draw_cursor(dc: HDC, ox: i32, oy: i32) {
    unsafe {
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_ok() && ci.flags.0 & CURSOR_SHOWING.0 != 0 {
            let mut ii = ICONINFO::default();
            if GetIconInfo(ci.hCursor.into(), &mut ii).is_ok() {
                let x = ci.ptScreenPos.x - ii.xHotspot as i32 - ox;
                let y = ci.ptScreenPos.y - ii.yHotspot as i32 - oy;
                let _ = DrawIconEx(dc, x, y, ci.hCursor.into(), 0, 0, 0, None, DI_NORMAL);
                if !ii.hbmMask.is_invalid() {
                    let _ = DeleteObject(ii.hbmMask.into());
                }
                if !ii.hbmColor.is_invalid() {
                    let _ = DeleteObject(ii.hbmColor.into());
                }
            }
        }
    }
}

/// Bounding box of every monitor.
pub fn virtual_rect(mons: &[Monitor]) -> Rect {
    let x0 = mons.iter().map(|m| m.rect.x).min().unwrap_or(0);
    let y0 = mons.iter().map(|m| m.rect.y).min().unwrap_or(0);
    let x1 = mons.iter().map(|m| m.rect.right()).max().unwrap_or(1);
    let y1 = mons.iter().map(|m| m.rect.bottom()).max().unwrap_or(1);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// A frozen copy of every monitor.
pub struct Shot {
    pub mons: Vec<Monitor>,
    pub imgs: Vec<Image>,
}

impl Shot {
    pub fn take(cursor: bool) -> Shot {
        let mons = monitors();
        // One thread per monitor: BitBlt of several 4K screens is the slowest part of a capture.
        let imgs = std::thread::scope(|s| {
            let hs: Vec<_> = mons.iter().map(|m| { let r = m.rect; s.spawn(move || grab(r, cursor)) }).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Image::new(1, 1))).collect()
        });
        Shot { mons, imgs }
    }

    /// Composes any virtual-screen rectangle from the per-monitor copies (gaps stay black).
    pub fn crop(&self, r: Rect) -> Image {
        let mut out = Image::new(r.w.max(1) as u32, r.h.max(1) as u32);
        for p in out.px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        for (m, img) in self.mons.iter().zip(&self.imgs) {
            if let Some(i) = m.rect.intersect(&r) {
                let part = img.crop(i.x - m.rect.x, i.y - m.rect.y, i.w, i.h);
                out.blit(&part, i.x - r.x, i.y - r.y);
            }
        }
        out
    }

    /// The pixel at a virtual-screen point.
    pub fn pixel(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        for (m, img) in self.mons.iter().zip(&self.imgs) {
            if m.rect.contains(x, y) {
                let i = (((y - m.rect.y) as u32 * img.w + (x - m.rect.x) as u32) * 4) as usize;
                return Some([img.px[i + 2], img.px[i + 1], img.px[i]]);
            }
        }
        None
    }

    pub fn monitor_at(&self, x: i32, y: i32) -> Option<usize> {
        self.mons.iter().position(|m| m.rect.contains(x, y))
    }
}

#[derive(Clone, Debug)]
pub struct WinRect {
    pub hwnd: isize,
    pub rect: Rect,
    pub title: String,
}

fn is_cloaked(h: HWND) -> bool {
    let mut c: u32 = 0;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut c as *mut _ as _, 4).is_ok() && c != 0 }
}

pub fn frame_rect(h: HWND) -> Rect {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as _, std::mem::size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(h, &mut r);
        }
    }
    Rect::from_win(r)
}

/// Visible top-level windows, topmost first, for click-to-capture snapping.
pub fn windows_list(own_pid: u32) -> Vec<WinRect> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let (v, pid) = &mut *(lp.0 as *mut (Vec<WinRect>, u32));
            if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || is_cloaked(h) {
                return true.into();
            }
            let mut wpid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut wpid));
            if wpid == *pid {
                return true.into();
            }
            let mut cls = [0u16; 64];
            let n = GetClassNameW(h, &mut cls);
            let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if cls == "Progman" || cls == "WorkerW" {
                return true.into();
            }
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT.0 != 0 && ex & WS_EX_LAYERED.0 != 0 {
                return true.into();
            }
            let r = frame_rect(h);
            if r.w < 8 || r.h < 8 {
                return true.into();
            }
            let mut t = [0u16; 256];
            let tn = GetWindowTextW(h, &mut t);
            v.push(WinRect { hwnd: h.0 as isize, rect: r, title: String::from_utf16_lossy(&t[..tn.max(0) as usize]) });
            true.into()
        }
    }
    let mut st: (Vec<WinRect>, u32) = (Vec::new(), own_pid);
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut st as *mut _ as isize));
    }
    st.0
}

pub fn foreground_rect() -> Option<Rect> {
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return None;
        }
        let r = frame_rect(h);
        (!r.is_empty()).then_some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rect_math() {
        let r = Rect::from_points((10, 20), (5, 2));
        assert_eq!(r, Rect::new(5, 2, 6, 19));
        assert_eq!(Rect::new(0, 0, 10, 10).intersect(&Rect::new(5, 5, 10, 10)), Some(Rect::new(5, 5, 5, 5)));
        assert_eq!(Rect::new(0, 0, 10, 10).intersect(&Rect::new(10, 0, 5, 5)), None);
    }
    #[test]
    fn crop_blit() {
        let mut a = Image::new(4, 4);
        for (i, p) in a.px.chunks_exact_mut(4).enumerate() {
            p[0] = i as u8;
        }
        let c = a.crop(1, 1, 2, 2);
        assert_eq!(c.px[0], 5);
        assert_eq!(c.px[4 * 3], 10);
        let mut b = Image::new(3, 3);
        b.blit(&c, 2, 2);
        assert_eq!(b.px[(2 * 3 + 2) * 4], 5);
    }
}
