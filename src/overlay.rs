//! The full-screen selection overlay: one borderless window per monitor showing the frozen screen.
//! Windows are created once and kept hidden, so showing the overlay is just grab + upload + uncloak.

use crate::capture::{Rect, Shot, WinRect};
use crate::gfx::{self, Align, Canvas, col, icon};
use crate::win;
use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_U;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT, ValidateRect};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Region,
    Screen,
    Text,
    Scroll,
    Record,
}

pub const MODES: [(Mode, &str, &str); 5] = [
    (Mode::Region, icon::SELECTION, "Snip"),
    (Mode::Screen, icon::MONITOR, "Screen"),
    (Mode::Text, icon::TEXT_T, "Text"),
    (Mode::Scroll, icon::SCROLL, "Scroll"),
    (Mode::Record, icon::RECORD, "Record"),
];

impl Mode {
    pub fn hint(self) -> &'static str {
        match self {
            Mode::Region => "Drag to select  ·  click a window  ·  C zoom & colour  ·  Esc to cancel",
            Mode::Screen => "Click a monitor  ·  Enter captures all monitors",
            Mode::Text => "Drag over text to copy it  ·  click a window to read all of it",
            Mode::Scroll => "Select the area that scrolls  ·  Framegrab scrolls it for you",
            Mode::Record => "Select an area to record  ·  click a window or monitor",
        }
    }
}

pub struct Done {
    pub mode: Mode,
    pub rect: Option<Rect>,
    pub shot: Shot,
    /// Set when a window was clicked (not dragged): capture that window itself.
    pub window: Option<isize>,
    /// Window under the middle of the selection (for the app name in file names).
    pub under: Option<isize>,
}

struct OWin {
    hwnd: HWND,
    mon: Rect,
    scale: f32,
    canvas: Option<Canvas>,
    bmp: Option<ID2D1Bitmap>,
    bmp_size: (u32, u32),
}

struct Session {
    mode: Mode,
    shot: Shot,
    snaps: Vec<WinRect>,
    cursor: (i32, i32),
    drag: Option<(i32, i32)>,
    dragging: bool,
    hover_btn: Option<usize>,
    magnifier: bool,
    crosshair: bool,
}

pub struct Overlay {
    host: HWND,
    wins: Vec<OWin>,
    s: Option<Session>,
    pub headless: bool,
    finished: Option<(Mode, Option<Rect>, Option<isize>)>,
}

thread_local! {
    static OV: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

const CLASS: &str = "FramegrabOverlay";

pub fn init(host: HWND) {
    win::register_class(CLASS, Some(wndproc));
    let headless = std::env::var_os("FRAMEGRAB_HEADLESS").is_some();
    OV.with(|o| *o.borrow_mut() = Some(Overlay { host, wins: Vec::new(), s: None, headless, finished: None }));
    // Create the windows and GPU targets now so the first capture is as fast as later ones.
    let mons = crate::capture::monitors();
    with(|o| o.ensure_windows(&Shot { mons, imgs: Vec::new() }));
}

/// Rebuilds the overlay windows after a display change.
pub fn displays_changed() {
    let mons = crate::capture::monitors();
    with(|o| {
        if o.s.is_none() {
            o.ensure_windows(&Shot { mons, imgs: Vec::new() });
        }
    });
}

pub fn active() -> bool {
    OV.with(|o| o.try_borrow().map(|o| o.as_ref().is_some_and(|o| o.s.is_some())).unwrap_or(true))
}

fn with<R>(f: impl FnOnce(&mut Overlay) -> R) -> Option<R> {
    OV.with(|o| o.try_borrow_mut().ok().and_then(|mut o| o.as_mut().map(f)))
}

/// Freezes the screen and shows the overlay in `mode`.
pub fn show(mode: Mode, cfg: &crate::config::Config) {
    let t = std::time::Instant::now();
    let shot = Shot::take(cfg.capture_cursor);
    if crate::config::test_mode() {
        crate::app::log(&format!("  grab {} ms", t.elapsed().as_millis()));
    }
    show_with(mode, shot, cfg);
}

pub fn show_with(mode: Mode, shot: Shot, cfg: &crate::config::Config) {
    let own = std::process::id();
    let snaps = crate::capture::windows_list(own);
    let cursor = crate::capture::cursor_pos();
    let t0 = std::time::Instant::now();
    let to_show = with(|o| {
        o.ensure_windows(&shot);
        if std::env::var_os("FRAMEGRAB_NO_UPLOAD").is_none() {
            for (w, img) in o.wins.iter_mut().zip(&shot.imgs) {
                w.upload(img);
            }
        }
        o.s = Some(Session { mode, shot, snaps, cursor, drag: None, dragging: false, hover_btn: None, magnifier: cfg.magnifier, crosshair: cfg.crosshair });
        o.finished = None;
        (o.wins.iter().map(|w| w.hwnd).collect::<Vec<_>>(), o.headless)
    });
    if crate::config::test_mode() {
        crate::app::log(&format!("  upload {} ms", t0.elapsed().as_millis()));
    }
    let Some((hwnds, headless)) = to_show else { return };
    if headless {
        return;
    }
    unsafe {
        for &h in &hwnds {
            win::cloak(h, true);
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        }
        with(|o| o.paint_all());
        for &h in &hwnds {
            win::cloak(h, false);
        }
        if let Some(&first) = hwnds.first() {
            let cur = crate::capture::cursor_pos();
            let target = with(|o| o.wins.iter().find(|w| w.mon.contains(cur.0, cur.1)).map(|w| w.hwnd)).flatten().unwrap_or(first);
            win::force_foreground(target);
            let _ = SetFocus(Some(target));
        }
    }
}

/// Takes the finished result (after WM_OVERLAY_DONE) and hides the windows.
pub fn take_result() -> Option<Done> {
    let (res, hwnds) = with(|o| {
        let s = o.s.take()?;
        let (mode, rect, window) = o.finished.take().unwrap_or((s.mode, None, None));
        let under = window.or_else(|| rect.and_then(|r| s.snaps.iter().find(|w| w.rect.contains(r.x + r.w / 2, r.y + r.h / 2)).map(|w| w.hwnd)));
        for w in &mut o.wins {
            w.bmp = None;
            w.bmp_size = (0, 0);
        }
        Some((Done { mode, rect, shot: s.shot, window, under }, o.wins.iter().map(|w| w.hwnd).collect::<Vec<_>>()))
    })??;
    unsafe {
        for h in hwnds {
            let _ = ShowWindow(h, SW_HIDE);
        }
        let _ = ReleaseCapture();
    }
    Some(res)
}

/// Frees GPU bitmaps (kept briefly after a capture) and the DXGI devices.
pub fn release_memory() {
    with(|o| {
        if o.s.is_none() {
            for w in &mut o.wins {
                w.bmp = None;
                w.bmp_size = (0, 0);
            }
        }
    });
    crate::dxgi::release_devices();
}

pub fn cancel() {
    with(|o| o.finish(None));
}

/// Keys routed from the keyboard hook while the overlay is up (works even without focus).
pub fn key(vk: u32) {
    with(|o| o.on_key(vk));
}

/// Test hooks: move the virtual cursor / drag / click without real input.
pub fn sim(cmd: &str) {
    let parts: Vec<i32> = cmd.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    with(|o| {
        let Some(s) = o.s.as_mut() else { return };
        match cmd.split_whitespace().next().unwrap_or("") {
            "move" if parts.len() == 2 => {
                s.cursor = (parts[0], parts[1]);
            }
            "down" if parts.len() == 2 => {
                s.cursor = (parts[0], parts[1]);
                s.drag = Some(s.cursor);
            }
            "drag" if parts.len() == 2 => {
                s.cursor = (parts[0], parts[1]);
                s.dragging = true;
            }
            "up" => {
                o.on_up();
                return;
            }
            "mode" if parts.len() == 1 => {
                s.mode = MODES[(parts[0] as usize).min(4)].0;
            }
            _ => {}
        }
        o.update_hover();
        o.invalidate();
    });
}

/// Test hook: renders each monitor's overlay off-screen and saves PNGs (scaled by `div`).
pub fn dump(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    with(|o| {
        let Some(s) = &o.s else { return };
        for (i, w) in o.wins.iter().enumerate() {
            let (cw, ch) = (w.mon.w as u32, w.mon.h as u32);
            let Some(c) = Canvas::offscreen(cw, ch) else { continue };
            let img = &s.shot.imgs[i];
            let bmp = c.bitmap(img.w, img.h, &img.px);
            c.begin();
            render(&c, bmp.as_ref(), w, s);
            c.end();
            if let Some(px) = c.pixels() {
                let p = dir.join(format!("overlay-{i}.png"));
                if crate::output::write_png(&p, &px, true).is_ok() {
                    out.push(p);
                }
            }
        }
    });
    out
}

impl OWin {
    fn upload(&mut self, img: &crate::capture::Image) {
        let Some(c) = &self.canvas else { return };
        if self.bmp.is_some() && self.bmp_size == (img.w, img.h) {
            let r = D2D_RECT_U { left: 0, top: 0, right: img.w, bottom: img.h };
            unsafe {
                if self.bmp.as_ref().unwrap().CopyFromMemory(Some(&r), img.px.as_ptr() as _, img.w * 4).is_ok() {
                    return;
                }
            }
        }
        self.bmp = c.bitmap(img.w, img.h, &img.px);
        self.bmp_size = (img.w, img.h);
    }
}

impl Overlay {
    fn ensure_windows(&mut self, shot: &Shot) {
        let same = self.wins.len() == shot.mons.len() && self.wins.iter().zip(&shot.mons).all(|(w, m)| w.mon == m.rect && (w.scale - m.scale()).abs() < 0.01);
        if same && self.wins.iter().all(|w| w.canvas.is_some()) {
            return;
        }
        for w in self.wins.drain(..) {
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
        for m in &shot.mons {
            let r = m.rect;
            let h = win::create(CLASS, "Framegrab", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW, r.x, r.y, r.w, r.h, None);
            win::exclude_from_capture(h);
            let canvas = Canvas::new(h, r.w as u32, r.h as u32);
            self.wins.push(OWin { hwnd: h, mon: r, scale: m.scale(), canvas, bmp: None, bmp_size: (0, 0) });
        }
    }

    fn invalidate(&self) {
        for w in &self.wins {
            win::invalidate(w.hwnd);
        }
    }

    fn paint_all(&mut self) {
        for i in 0..self.wins.len() {
            self.paint(i);
        }
    }

    fn paint(&mut self, i: usize) {
        let Some(s) = &self.s else { return };
        let w = &self.wins[i];
        let Some(c) = &w.canvas else { return };
        c.begin();
        render(c, w.bmp.as_ref(), w, s);
        if !c.end() {
            // Device lost: rebuild the target and re-upload on the next paint.
            let w = &mut self.wins[i];
            w.canvas = Canvas::new(w.hwnd, w.mon.w as u32, w.mon.h as u32);
            w.bmp = None;
            if let Some(s) = &self.s {
                let img = s.shot.imgs[i].clone();
                w.upload(&img);
            }
            win::invalidate(w.hwnd);
        }
    }

    fn finish(&mut self, rect: Option<Rect>) {
        self.finish_win(rect, None);
    }

    fn finish_win(&mut self, rect: Option<Rect>, window: Option<isize>) {
        if self.s.is_none() || self.finished.is_some() {
            return;
        }
        let mode = self.s.as_ref().map(|s| s.mode).unwrap_or(Mode::Region);
        if crate::config::test_mode() {
            let c = self.s.as_ref().map(|s| (s.cursor, s.hover_btn, s.shot.mons.iter().map(|m| m.rect).collect::<Vec<_>>(), s.snaps.len()));
            crate::app::log(&format!("finish {mode:?} rect={rect:?} window={window:?} cursor/hover={c:?}"));
        }
        self.finished = Some((mode, rect, window));
        win::post(self.host, win::WM_OVERLAY_DONE, 0, 0);
    }

    fn update_hover(&mut self) {
        let Some(s) = self.s.as_mut() else { return };
        s.hover_btn = None;
        if s.drag.is_none() {
            if let Some(m) = s.shot.mons.iter().find(|m| m.rect.contains(s.cursor.0, s.cursor.1)) {
                for (i, r) in bar_layout(m.rect, m.scale()).iter().enumerate() {
                    if r.contains(s.cursor.0, s.cursor.1) {
                        s.hover_btn = Some(i);
                    }
                }
            }
        }
    }

    fn on_move(&mut self) {
        let Some(s) = self.s.as_mut() else { return };
        s.cursor = crate::capture::cursor_pos();
        if let Some(d) = s.drag {
            if (s.cursor.0 - d.0).abs() > 3 || (s.cursor.1 - d.1).abs() > 3 {
                s.dragging = true;
            }
        }
        self.update_hover();
        self.invalidate();
    }

    fn on_down(&mut self, hwnd: HWND) {
        let Some(s) = self.s.as_mut() else { return };
        s.cursor = crate::capture::cursor_pos();
        self.update_hover();
        let s = self.s.as_mut().unwrap();
        if let Some(b) = s.hover_btn {
            if b < MODES.len() {
                s.mode = MODES[b].0;
            } else {
                self.finish(None);
                return;
            }
            self.invalidate();
            return;
        }
        if s.mode != Mode::Screen {
            s.drag = Some(s.cursor);
            s.dragging = false;
            unsafe {
                SetCapture(hwnd);
            }
        }
    }

    fn on_up(&mut self) {
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some(s) = self.s.as_mut() else { return };
        if s.hover_btn.is_some() && s.drag.is_none() {
            return;
        }
        if s.mode == Mode::Screen {
            let r = s.shot.mons.iter().find(|m| m.rect.contains(s.cursor.0, s.cursor.1)).map(|m| m.rect);
            self.finish(r);
            return;
        }
        let Some(d) = s.drag.take() else { return };
        if s.dragging {
            s.dragging = false;
            let r = Rect::from_points(d, s.cursor);
            if r.w >= 4 && r.h >= 4 {
                self.finish(Some(r));
            } else {
                self.invalidate();
            }
        } else {
            let (r, w) = target(s);
            self.finish_win(r, w);
        }
    }

    fn on_key(&mut self, vk: u32) {
        let Some(s) = self.s.as_mut() else { return };
        let vk = VIRTUAL_KEY(vk as u16);
        let shift = unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } < 0;
        let step = if shift { 10 } else { 1 };
        match vk {
            VK_ESCAPE => {
                if s.drag.is_some() {
                    s.drag = None;
                    s.dragging = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.invalidate();
                } else {
                    self.finish(None);
                }
            }
            VK_RETURN | VK_SPACE => {
                if s.mode == Mode::Screen {
                    let all = crate::capture::virtual_rect(&s.shot.mons);
                    self.finish(Some(all));
                } else {
                    let (r, w) = target(s);
                    self.finish_win(r, w);
                }
            }
            VK_TAB => {
                let i = MODES.iter().position(|m| m.0 == s.mode).unwrap_or(0);
                s.mode = MODES[(i + if shift { MODES.len() - 1 } else { 1 }) % MODES.len()].0;
                self.invalidate();
            }
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                let (dx, dy) = match vk {
                    VK_LEFT => (-step, 0),
                    VK_RIGHT => (step, 0),
                    VK_UP => (0, -step),
                    _ => (0, step),
                };
                unsafe {
                    let _ = SetCursorPos(s.cursor.0 + dx, s.cursor.1 + dy);
                }
                self.on_move();
            }
            _ => {
                let k = vk.0 as u32;
                if (0x31..=0x35).contains(&k) {
                    s.mode = MODES[(k - 0x31) as usize].0;
                    self.invalidate();
                } else if k == 'C' as u32 {
                    // Zoom + colour readout only when asked for (or always, if set in Settings).
                    s.magnifier = !s.magnifier;
                    self.invalidate();
                } else if k == 'F' as u32 {
                    let r = s.shot.mons.iter().find(|m| m.rect.contains(s.cursor.0, s.cursor.1)).map(|m| m.rect);
                    self.finish(r);
                }
            }
        }
    }
}

/// What a click would capture: (rect, window handle when it's a window).
fn target(s: &Session) -> (Option<Rect>, Option<isize>) {
    let (x, y) = s.cursor;
    if s.mode != Mode::Screen {
        let vr = crate::capture::virtual_rect(&s.shot.mons);
        if let Some(w) = s.snaps.iter().find(|w| w.rect.contains(x, y)) {
            if let Some(r) = w.rect.intersect(&vr) {
                return (Some(r), Some(w.hwnd));
            }
        }
    }
    (s.shot.mons.iter().find(|m| m.rect.contains(x, y)).map(|m| m.rect), None)
}

/// What a click would capture: the window under the cursor, else its monitor.
fn target_rect(s: &Session) -> Option<Rect> {
    let (x, y) = s.cursor;
    let mon = s.shot.mons.iter().find(|m| m.rect.contains(x, y)).map(|m| m.rect);
    if s.mode == Mode::Screen {
        return mon;
    }
    let vr = crate::capture::virtual_rect(&s.shot.mons);
    s.snaps.iter().find(|w| w.rect.contains(x, y)).and_then(|w| w.rect.intersect(&vr)).or(mon)
}

/// Mode-bar buttons (five modes + close) in virtual-screen coordinates for a monitor.
fn bar_layout(m: Rect, s: f32) -> Vec<Rect> {
    let f = gfx::fact();
    let h = (40.0 * s) as i32;
    let pad = (14.0 * s) as i32;
    let gap = (4.0 * s) as i32;
    let mut widths: Vec<i32> = MODES.iter().map(|(_, _, l)| pad * 2 + (18.0 * s) as i32 + (7.0 * s) as i32 + f.measure(l, 13.0 * s, 500) as i32).collect();
    widths.push(h);
    let total: i32 = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1) + (12.0 * s) as i32;
    let mut x = m.x + (m.w - total) / 2 + (6.0 * s) as i32;
    let y = m.y + (22.0 * s) as i32 + (4.0 * s) as i32;
    widths
        .iter()
        .map(|&w| {
            let r = Rect::new(x, y, w, h - (8.0 * s) as i32);
            x += w + gap;
            r
        })
        .collect()
}

fn render(c: &Canvas, bmp: Option<&ID2D1Bitmap>, w: &OWin, s: &Session) {
    let m = w.mon;
    let sc = w.scale;
    let (ox, oy) = (m.x as f32, m.y as f32);
    let (mw, mh) = (m.w as f32, m.h as f32);
    c.clear(col::BLACK);
    if let Some(b) = bmp {
        c.draw_bitmap(b, gfx::rf(0.0, 0.0, mw, mh), gfx::rf(0.0, 0.0, mw, mh), 1.0, true);
    }
    let cursor_here = m.contains(s.cursor.0, s.cursor.1);
    let on_bar = s.hover_btn.is_some();

    // Highlight: the drag rectangle, else what a click would take.
    let hl = if s.dragging {
        s.drag.map(|d| Rect::from_points(d, s.cursor))
    } else if on_bar {
        None
    } else {
        target_rect(s)
    };
    let local = hl.and_then(|h| h.intersect(&m)).map(|h| (h.x as f32 - ox, h.y as f32 - oy, h.w as f32, h.h as f32));
    match local {
        Some((x, y, hw, hh)) => {
            c.fill(0.0, 0.0, mw, y, col::DIM);
            c.fill(0.0, y + hh, mw, mh - y - hh, col::DIM);
            c.fill(0.0, y, x, hh, col::DIM);
            c.fill(x + hw, y, mw - x - hw, hh, col::DIM);
            let bw = (2.0 * sc).round().max(1.0);
            let color = if s.mode == Mode::Record { col::REC } else { col::ACCENT };
            c.stroke(x + bw / 2.0, y + bw / 2.0, hw - bw, hh - bw, color, bw);
        }
        None => c.fill(0.0, 0.0, mw, mh, col::DIM),
    }

    // Size pill under the highlight (only on the monitor holding its bottom-left corner).
    if let Some(h) = hl {
        let anchor = (h.x.max(m.x), h.bottom());
        if m.contains(anchor.0, anchor.1.min(m.bottom() - 1)) && local.is_some() {
            let label = format!("{} × {}", h.w, h.h);
            let fs = 12.5 * sc;
            let tw = gfx::fact().measure(&label, fs, 600) + 16.0 * sc;
            let ph = 24.0 * sc;
            let mut px = anchor.0 as f32 - ox;
            let mut py = h.bottom() as f32 - oy + 8.0 * sc;
            if py + ph > mh - 4.0 * sc {
                py = (h.bottom() as f32 - oy - ph - 8.0 * sc).max(4.0 * sc);
                px += 8.0 * sc;
            }
            px = px.min(mw - tw - 4.0 * sc).max(4.0 * sc);
            c.round(px, py, tw, ph, 6.0 * sc, col::PANEL);
            c.text(&label, px, py, tw, ph, fs, 600, col::TEXT, Align::Center);
        }
    }

    // Faint crosshair through the cursor.
    if cursor_here && s.crosshair && !s.dragging && !on_bar && s.mode != Mode::Screen {
        let (cx, cy) = (s.cursor.0 as f32 - ox + 0.5, s.cursor.1 as f32 - oy + 0.5);
        c.line(cx, 0.0, cx, mh, 0x50FFFFFF, 1.0, true);
        c.line(0.0, cy, mw, cy, 0x50FFFFFF, 1.0, true);
    }

    // Magnifier.
    if cursor_here && s.magnifier && !on_bar && s.mode != Mode::Screen {
        if let Some(b) = bmp {
            magnifier(c, b, w, s);
        }
    }

    // Mode bar + hint on the monitor with the cursor.
    if cursor_here {
        let btns = bar_layout(m, sc);
        let first = btns[0];
        let last = *btns.last().unwrap();
        let (bx, by) = (first.x as f32 - ox - 6.0 * sc, first.y as f32 - oy - 4.0 * sc);
        let bw = (last.right() - first.x) as f32 + 12.0 * sc;
        let bh = first.h as f32 + 8.0 * sc;
        c.round(bx - 1.0, by - 1.0, bw + 2.0, bh + 2.0, 11.0 * sc, col::BORDER);
        c.round(bx, by, bw, bh, 10.0 * sc, col::PANEL);
        for (i, r) in btns.iter().enumerate() {
            let (x, y, ww, hh) = (r.x as f32 - ox, r.y as f32 - oy, r.w as f32, r.h as f32);
            let active = i < MODES.len() && MODES[i].0 == s.mode;
            let hover = s.hover_btn == Some(i);
            if active {
                c.round(x, y, ww, hh, 7.0 * sc, if s.mode == Mode::Record { 0x40FF4D5E } else { col::ACCENT_SOFT });
            } else if hover {
                c.round(x, y, ww, hh, 7.0 * sc, 0x18FFFFFF);
            }
            let fg = if active { col::TEXT } else if hover { col::TEXT } else { col::MUTED };
            if i < MODES.len() {
                let pad = 14.0 * sc;
                let isz = 18.0 * sc;
                let icol = if active { if s.mode == Mode::Record { col::REC } else { col::ACCENT } } else { fg };
                c.icon(MODES[i].1, x + pad - 2.0 * sc, y, isz + 4.0 * sc, hh, isz, icol);
                c.text(MODES[i].2, x + pad + isz + 7.0 * sc, y, ww - pad - isz - 7.0 * sc, hh, 13.0 * sc, 500, fg, Align::Left);
            } else {
                c.icon(icon::X, x, y, ww, hh, 16.0 * sc, fg);
            }
        }
        let hint = s.mode.hint();
        let fs = 12.5 * sc;
        let tw = gfx::fact().measure(hint, fs, 400) + 24.0 * sc;
        let th = 28.0 * sc;
        let hx = (mw - tw) / 2.0;
        let hy = by + bh + 10.0 * sc;
        c.round(hx - 1.0, hy - 1.0, tw + 2.0, th + 2.0, 9.0 * sc, col::BORDER);
        c.round(hx, hy, tw, th, 8.0 * sc, 0xFA1C1C21);
        c.text(hint, hx, hy, tw, th, fs, 400, col::MUTED, Align::Center);
    }
}

fn magnifier(c: &Canvas, b: &ID2D1Bitmap, w: &OWin, s: &Session) {
    let m = w.mon;
    let sc = w.scale;
    let cells = 19i32;
    let cell = (8.0 * sc).round();
    let size = cell * cells as f32;
    let (lx, ly) = (s.cursor.0 - m.x, s.cursor.1 - m.y);
    let gap = 26.0 * sc;
    let info_h = 26.0 * sc;
    let mut x = lx as f32 + gap;
    let mut y = ly as f32 + gap;
    if x + size > m.w as f32 - 4.0 {
        x = lx as f32 - gap - size;
    }
    if y + size + info_h > m.h as f32 - 4.0 {
        y = ly as f32 - gap - size - info_h;
    }
    let r = 10.0 * sc;
    c.round(x - 1.0, y - 1.0, size + 2.0, size + info_h + 2.0, r + 1.0, col::BORDER);
    c.round(x, y, size, size + info_h, r, col::PANEL);
    if c.push_round_clip(x, y, size, size, r) {
        c.fill(x, y, size, size, col::BLACK);
        // Source square clamped to the monitor; destination shifted to match.
        let half = cells / 2;
        let (sx0, sy0) = (lx - half, ly - half);
        let (cx0, cy0) = (sx0.max(0), sy0.max(0));
        let (cx1, cy1) = ((sx0 + cells).min(m.w), (sy0 + cells).min(m.h));
        if cx1 > cx0 && cy1 > cy0 {
            let dx = x + (cx0 - sx0) as f32 * cell;
            let dy = y + (cy0 - sy0) as f32 * cell;
            c.draw_bitmap(
                b,
                gfx::rf(dx, dy, (cx1 - cx0) as f32 * cell, (cy1 - cy0) as f32 * cell),
                gfx::rf(cx0 as f32, cy0 as f32, (cx1 - cx0) as f32, (cy1 - cy0) as f32),
                1.0,
                true,
            );
        }
        let cxp = x + half as f32 * cell;
        let cyp = y + half as f32 * cell;
        c.stroke(cxp, cyp, cell, cell, 0xFF000000, 2.0);
        c.stroke(cxp, cyp, cell, cell, col::WHITE, 1.0);
        c.pop_layer();
    }
    let px = s.shot.pixel(s.cursor.0, s.cursor.1).unwrap_or([0, 0, 0]);
    let hex = format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2]);
    let info = format!("{}, {}", lx, ly);
    let iy = y + size;
    let sw = 10.0 * sc;
    c.round(x + 10.0 * sc, iy + (info_h - sw) / 2.0, sw, sw, 3.0 * sc, 0xFF000000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32);
    c.text(&hex, x + 26.0 * sc, iy, size * 0.5, info_h, 11.5 * sc, 600, col::TEXT, Align::Left);
    c.text(&info, x + size * 0.5, iy, size * 0.5 - 10.0 * sc, info_h, 11.5 * sc, 400, col::MUTED, Align::Right);
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SETCURSOR => {
                let over_bar = with(|o| o.s.as_ref().is_some_and(|s| s.hover_btn.is_some())).unwrap_or(false);
                SetCursor(LoadCursorW(None, if over_bar { IDC_HAND } else { IDC_CROSS }).ok());
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                with(|o| o.on_move());
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                with(|o| o.on_down(h));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                with(|o| o.on_up());
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                with(|o| o.on_key(VK_ESCAPE.0 as u32));
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                with(|o| o.on_key(wp.0 as u32));
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(h, &mut ps);
                with(|o| {
                    if let Some(i) = o.wins.iter().position(|w| w.hwnd == h) {
                        o.paint(i);
                    }
                });
                let _ = EndPaint(h, &ps);
                let _ = ValidateRect(Some(h), None);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_ACTIVATE as isize),
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
