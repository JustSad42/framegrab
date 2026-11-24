//! The small card shown after a capture: thumbnail + Edit / Text / Copy / Folder / Delete.

use crate::capture::{Image, Rect};
use crate::gfx::{self, Align, Canvas, col, icon};
use crate::win;
use std::cell::RefCell;
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Edit = 1,
    Text = 2,
    Copy = 3,
    Folder = 4,
    Delete = 5,
    Close = 6,
    Upload = 7,
}

impl Action {
    pub fn from(v: usize) -> Option<Action> {
        Some(match v {
            1 => Action::Edit,
            2 => Action::Text,
            3 => Action::Copy,
            4 => Action::Folder,
            5 => Action::Delete,
            6 => Action::Close,
            7 => Action::Upload,
            _ => return None,
        })
    }
}

pub enum Content {
    Image { thumb: Image, size: (u32, u32), status: String, upload: bool },
    Text { text: String, status: String },
    Video { thumb: Option<Image>, status: String, detail: String },
}

struct Toast {
    hwnd: HWND,
    host: HWND,
    canvas: Option<Canvas>,
    bmp: Option<ID2D1Bitmap>,
    content: Content,
    scale: f32,
    w: i32,
    h: i32,
    hover: Option<Action>,
    inside: bool,
    alpha: u8,
    fading_out: bool,
    life_ms: u32,
    left_ms: i64,
    pub path: Option<PathBuf>,
    buttons: Vec<(Action, Rect)>,
}

thread_local! {
    static T: RefCell<Option<Toast>> = const { RefCell::new(None) };
    static DRAGGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const CLASS: &str = "FramegrabToast";
const TIMER_TICK: usize = 1;
const TICK_MS: u32 = 16;

pub fn init() {
    win::register_class(CLASS, Some(wndproc));
}

fn with<R>(f: impl FnOnce(&mut Toast) -> R) -> Option<R> {
    T.with(|t| t.try_borrow_mut().ok().and_then(|mut t| t.as_mut().map(f)))
}

/// Replaces the status line (e.g. "Uploading…", "Link copied") and keeps the card up a bit longer.
pub fn set_status(s: &str) {
    with(|t| {
        match &mut t.content {
            Content::Image { status, .. } | Content::Text { status, .. } | Content::Video { status, .. } => *status = s.to_string(),
        }
        t.left_ms = t.left_ms.max(3000);
        t.fading_out = false;
        win::invalidate(t.hwnd);
    });
}

pub fn path() -> Option<PathBuf> {
    with(|t| t.path.clone()).flatten()
}

pub fn hide() {
    let h = T.with(|t| t.borrow_mut().take().map(|t| t.hwnd));
    if let Some(h) = h {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Fade the card out (used after an action).
pub fn dismiss() {
    with(|t| {
        t.fading_out = true;
    });
}

pub fn show(host: HWND, content: Content, path: Option<PathBuf>, seconds: u32) {
    hide();
    // Bottom-right of the work area on the monitor with the cursor.
    let mons = crate::capture::monitors();
    let (cx, cy) = crate::capture::cursor_pos();
    let Some(m) = mons.iter().find(|m| m.rect.contains(cx, cy)).or_else(|| mons.iter().find(|m| m.primary)).or(mons.first()).cloned() else { return };
    let s = m.scale();
    let wide_card = matches!(&content, Content::Image { upload: true, .. });
    let cw = ((if wide_card { 384.0 } else { 340.0 }) * s) as i32;
    let pad = (12.0 * s) as i32;
    let body_h = match &content {
        Content::Image { thumb, .. } => {
            let tw = cw - pad * 2;
            ((thumb.h as f32 * tw as f32 / thumb.w.max(1) as f32) as i32).clamp((60.0 * s) as i32, (190.0 * s) as i32)
        }
        Content::Text { text, .. } => {
            let lines = text.lines().filter(|l| !l.trim().is_empty()).take(4).count().max(1) as f32;
            (lines * 19.0 * s) as i32 + (8.0 * s) as i32
        }
        Content::Video { thumb, .. } => {
            if thumb.is_some() {
                (150.0 * s) as i32
            } else {
                (10.0 * s) as i32
            }
        }
    };
    let ch = pad + body_h + (40.0 * s) as i32 + (44.0 * s) as i32;
    let x = m.work.right() - cw - (16.0 * s) as i32;
    let y = m.work.bottom() - ch - (16.0 * s) as i32;
    let hwnd = win::create(CLASS, "Framegrab", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED, x, y, cw, ch, None);
    win::round_corners(hwnd, false);
    win::exclude_from_capture(hwnd);
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
    }
    let headless = std::env::var_os("FRAMEGRAB_HEADLESS").is_some();
    let canvas = if headless { Canvas::offscreen(cw as u32, ch as u32) } else { Canvas::new(hwnd, cw as u32, ch as u32) };
    let bmp = match (&content, &canvas) {
        (Content::Image { thumb, .. }, Some(c)) | (Content::Video { thumb: Some(thumb), .. }, Some(c)) => {
            let tw = (cw - pad * 2) as u32;
            let t = thumb.fit(tw, body_h as u32);
            c.bitmap(t.w, t.h, &t.px).map(|b| (b, t.w, t.h))
        }
        _ => None,
    };
    let toast = Toast {
        hwnd,
        host,
        canvas,
        bmp: bmp.map(|b| b.0),
        content,
        scale: s,
        w: cw,
        h: ch,
        hover: None,
        inside: false,
        alpha: 0,
        fading_out: false,
        life_ms: seconds.max(2) * 1000,
        left_ms: (seconds.max(2) * 1000) as i64,
        path,
        buttons: Vec::new(),
    };
    T.with(|t| *t.borrow_mut() = Some(toast));
    if headless {
        // Tests: render once to <FRAMEGRAB_OUT>\toast.png instead of showing a window.
        with(|t| {
            paint(t);
            if let (Some(c), Some(dir)) = (&t.canvas, std::env::var_os("FRAMEGRAB_OUT")) {
                if let Some(px) = c.pixels() {
                    let _ = crate::output::write_png(&std::path::Path::new(&dir).join("toast.png"), &px, true);
                }
            }
        });
        hide();
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_TICK, TICK_MS, None);
    }
    win::invalidate(hwnd);
}

fn layout(t: &Toast) -> Vec<(Action, Rect)> {
    let s = t.scale;
    let pad = (12.0 * s) as i32;
    let bh = (34.0 * s) as i32;
    let y = t.h - pad - bh;
    let gap = (6.0 * s) as i32;
    let mut out = Vec::new();
    let acts: &[Action] = match t.content {
        Content::Image { upload: true, .. } => &[Action::Edit, Action::Text, Action::Copy, Action::Upload, Action::Folder, Action::Delete],
        Content::Image { .. } => &[Action::Edit, Action::Text, Action::Copy, Action::Folder, Action::Delete],
        Content::Text { .. } => &[Action::Copy],
        Content::Video { .. } => &[Action::Folder, Action::Delete],
    };
    let wide = if matches!(t.content, Content::Text { .. }) { (120.0 * s) as i32 } else if matches!(t.content, Content::Video { .. }) { (140.0 * s) as i32 } else { (88.0 * s) as i32 };
    let mut x = pad;
    for (i, a) in acts.iter().enumerate() {
        let w = if i == 0 { wide } else if matches!(a, Action::Text) { (74.0 * s) as i32 } else { bh + (6.0 * s) as i32 };
        out.push((*a, Rect::new(x, y, w, bh)));
        x += w + gap;
    }
    let cs = (26.0 * s) as i32;
    out.push((Action::Close, Rect::new(t.w - pad - cs + (4.0 * s) as i32, (8.0 * s) as i32, cs, cs)));
    out
}

fn paint(t: &mut Toast) {
    t.buttons = layout(t);
    let Some(c) = &t.canvas else { return };
    let s = t.scale;
    let (w, h) = (t.w as f32, t.h as f32);
    let pad = 12.0 * s;
    c.begin();
    c.clear(0xFF1C1C21);
    c.stroke(0.5, 0.5, w - 1.0, h - 1.0, col::BORDER, 1.0);
    let mut y = pad;
    // Body.
    match &t.content {
        Content::Image { .. } | Content::Video { thumb: Some(_), .. } => {
            if let Some(b) = &t.bmp {
                let sz = unsafe { b.GetSize() };
                let bw = w - pad * 2.0;
                let bh = sz.height.min(190.0 * s).max(1.0);
                let dx = pad + (bw - sz.width) / 2.0;
                c.round(pad, y, bw, bh, 8.0 * s, 0xFF111114);
                if c.push_round_clip(pad, y, bw, bh, 8.0 * s) {
                    c.draw_bitmap(b, gfx::rf(dx, y + (bh - sz.height) / 2.0, sz.width, sz.height), gfx::rf(0.0, 0.0, sz.width, sz.height), 1.0, false);
                    c.pop_layer();
                }
                y += bh;
            }
        }
        Content::Text { text, .. } => {
            let mut ly = y + 4.0 * s;
            for line in text.lines().filter(|l| !l.trim().is_empty()).take(4) {
                let l: String = line.chars().take(60).collect();
                c.text(&l, pad, ly, w - pad * 2.0 - 30.0 * s, 19.0 * s, 13.0 * s, 400, col::TEXT, Align::Left);
                ly += 19.0 * s;
            }
            y = ly + 4.0 * s;
        }
        Content::Video { .. } => {
            y += 10.0 * s;
        }
    }
    // Status line.
    let (status, detail) = match &t.content {
        Content::Image { size, status, .. } => (status.clone(), format!("{} × {}", size.0, size.1)),
        Content::Text { status, .. } => (status.clone(), String::new()),
        Content::Video { status, detail, .. } => (status.clone(), detail.clone()),
    };
    let sy = y + 6.0 * s;
    c.icon(icon::CHECK, pad - 2.0 * s, sy, 20.0 * s, 28.0 * s, 15.0 * s, 0xFF5BD68A);
    c.text(&status, pad + 20.0 * s, sy, w - pad * 2.0 - 110.0 * s, 28.0 * s, 13.0 * s, 600, col::TEXT, Align::Left);
    c.text(&detail, w - pad - 120.0 * s, sy, 120.0 * s, 28.0 * s, 12.0 * s, 400, col::MUTED, Align::Right);
    // Buttons.
    for (a, r) in &t.buttons {
        let (x, y, bw, bh) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let hover = t.hover == Some(*a);
        let primary = *a == t.buttons[0].0 && *a != Action::Close;
        let (ic, label) = match a {
            Action::Edit => (icon::PENCIL, "Edit"),
            Action::Text => (icon::TEXT_T, "Text"),
            Action::Copy => (icon::COPY, if matches!(t.content, Content::Text { .. }) { "Copy again" } else { "" }),
            Action::Folder => (icon::FOLDER_OPEN, if matches!(t.content, Content::Video { .. }) { "Show in folder" } else { "" }),
            Action::Delete => (icon::TRASH, ""),
            Action::Upload => (icon::UPLOAD, ""),
            Action::Close => (icon::X, ""),
        };
        if *a == Action::Close {
            if hover {
                c.circle(x + bw / 2.0, y + bh / 2.0, bw / 2.0, 0xCC2A2A31);
            } else if matches!(t.content, Content::Image { .. }) {
                c.circle(x + bw / 2.0, y + bh / 2.0, bw / 2.0, 0x99000000);
            }
            c.icon(ic, x, y, bw, bh, 14.0 * s, col::TEXT);
            continue;
        }
        let bg = if primary {
            if hover { 0xFF8199FF } else { col::ACCENT }
        } else if hover {
            0xFF34343C
        } else {
            col::PANEL_HI
        };
        c.round(x, y, bw, bh, 7.0 * s, bg);
        let fg = if *a == Action::Delete && hover { col::REC } else { col::TEXT };
        if label.is_empty() {
            c.icon(ic, x, y, bw, bh, 17.0 * s, fg);
        } else {
            let tw = gfx::fact().measure(label, 13.0 * s, 600);
            let total = 17.0 * s + 6.0 * s + tw;
            let ix = x + (bw - total) / 2.0;
            c.icon(ic, ix, y, 17.0 * s, bh, 16.0 * s, fg);
            c.text(label, ix + 23.0 * s, y, tw + 4.0, bh, 13.0 * s, 600, fg, Align::Left);
        }
    }
    // Countdown hairline.
    if !t.inside && !t.fading_out {
        let frac = (t.left_ms as f32 / t.life_ms as f32).clamp(0.0, 1.0);
        c.fill(0.0, h - 2.0 * s, w * frac, 2.0 * s, 0x806E8BFF);
    }
    c.end();
}

fn hit(t: &Toast, x: i32, y: i32) -> Option<Action> {
    t.buttons.iter().find(|(_, r)| r.contains(x, y)).map(|b| b.0)
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(h, &mut ps);
                with(paint);
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_TIMER => {
                let mut kill = false;
                with(|t| {
                    if t.fading_out {
                        t.alpha = t.alpha.saturating_sub(40);
                        if t.alpha == 0 {
                            kill = true;
                        }
                    } else {
                        t.alpha = t.alpha.saturating_add(40);
                        if !t.inside {
                            t.left_ms -= TICK_MS as i64;
                            if t.left_ms <= 0 {
                                t.fading_out = true;
                            }
                        }
                    }
                    let _ = SetLayeredWindowAttributes(t.hwnd, COLORREF(0), t.alpha, LWA_ALPHA);
                    if !t.inside {
                        win::invalidate(t.hwnd);
                    }
                });
                if kill {
                    hide();
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = win::lparam_xy(lp);
                with(|t| {
                    if !t.inside {
                        t.inside = true;
                        t.left_ms = t.life_ms as i64;
                        t.fading_out = false;
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                    }
                    let hv = hit(t, x, y);
                    if hv != t.hover {
                        t.hover = hv;
                    }
                    win::invalidate(h);
                });
                LRESULT(0)
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                with(|t| {
                    t.inside = false;
                    t.hover = None;
                    win::invalidate(h);
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let over = with(|t| t.hover.is_some()).unwrap_or(false);
                SetCursor(LoadCursorW(None, if over { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = win::lparam_xy(lp);
                // Dragging the thumbnail drags the file out (Discord, browser, Explorer…).
                let on_thumb = with(|t| matches!(t.content, Content::Image { .. }) && hit(t, x, y).is_none() && y < t.h - (56.0 * t.scale) as i32).unwrap_or(false);
                if on_thumb {
                    let mut pt = POINT { x, y };
                    let _ = windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut pt);
                    if windows::Win32::UI::Input::KeyboardAndMouse::DragDetect(h, pt).as_bool() {
                        with(|t| {
                            t.inside = true;
                            DRAGGED.with(|d| d.set(true));
                        });
                        if let Some(f) = crate::app::last_file() {
                            if crate::dragout::drag_file(&f) {
                                set_status("Dropped");
                            }
                        }
                        with(|t| t.inside = false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if DRAGGED.with(|d| d.replace(false)) {
                    return LRESULT(0);
                }
                let (x, y) = win::lparam_xy(lp);
                let r = with(|t| {
                    let a = hit(t, x, y).or_else(|| (matches!(t.content, Content::Image { .. }) && y < t.h - (56.0 * t.scale) as i32).then_some(Action::Edit));
                    (a, t.host)
                });
                if let Some((Some(a), host)) = r {
                    win::post(host, win::WM_TOAST, a as usize, 0);
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                dismiss();
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
