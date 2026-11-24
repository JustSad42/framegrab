//! A floating control bar plus a frame around the active area, used by scrolling capture and
//! recording. Both are excluded from screen capture so they never show up in the output.

use crate::capture::Rect;
use crate::gfx::{self, Align, Canvas, col};
use crate::win;
use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{BeginPaint, CreateSolidBrush, EndPaint, FillRect, PAINTSTRUCT};
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone)]
pub struct Item {
    pub id: u32,
    pub icon: &'static str,
    pub label: String,
    pub style: Style,
    pub tip: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Normal,
    On,
    Primary,
    Danger,
    /// Plain text (not clickable).
    Label,
}

struct Bar {
    hwnd: HWND,
    host: HWND,
    canvas: Option<Canvas>,
    scale: f32,
    items: Vec<Item>,
    rects: Vec<Rect>,
    hover: Option<usize>,
    frame: Vec<HWND>,
    w: i32,
    h: i32,
    dot: bool,
}

thread_local! {
    static B: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

const CLASS: &str = "FramegrabBar";
const FRAME_CLASS: &str = "FramegrabFrame";

pub fn init() {
    win::register_class(CLASS, Some(wndproc));
    win::register_class(FRAME_CLASS, Some(frame_proc));
}

fn with<R>(f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    B.with(|b| b.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(f)))
}

fn headless() -> bool {
    std::env::var_os("FRAMEGRAB_HEADLESS").is_some()
}

pub fn visible() -> bool {
    B.with(|b| b.try_borrow().map(|b| b.is_some()).unwrap_or(true))
}

pub fn hide() {
    let hs = B.with(|b| b.borrow_mut().take().map(|b| (b.hwnd, b.frame)));
    if let Some((h, frame)) = hs {
        unsafe {
            let _ = DestroyWindow(h);
            for f in frame {
                let _ = DestroyWindow(f);
            }
        }
    }
}

fn item_width(it: &Item, s: f32) -> i32 {
    let f = gfx::fact();
    let h = 34.0 * s;
    match (it.icon.is_empty(), it.label.is_empty()) {
        (_, true) => h as i32,
        (true, false) => (f.measure(&it.label, 13.0 * s, 600) + 20.0 * s) as i32,
        (false, false) => (f.measure(&it.label, 13.0 * s, 600) + 18.0 * s + 6.0 * s + 24.0 * s) as i32,
    }
}

/// Shows (or replaces) the bar next to `area`, with a coloured frame around it.
pub fn show(host: HWND, area: Rect, frame_color: u32, items: Vec<Item>) {
    hide();
    let mons = crate::capture::monitors();
    let cx = area.x + area.w / 2;
    let cy = area.y + area.h / 2;
    let m = mons.iter().find(|m| m.rect.contains(cx, cy)).or(mons.first()).cloned();
    let Some(m) = m else { return };
    let s = m.scale();
    let pad = (6.0 * s) as i32;
    let gap = (4.0 * s) as i32;
    let bh = (34.0 * s) as i32 + pad * 2;
    let bw: i32 = items.iter().map(|i| item_width(i, s)).sum::<i32>() + gap * (items.len() as i32 - 1) + pad * 2;
    let mut x = (area.x + area.w / 2 - bw / 2).clamp(m.rect.x + 8, m.rect.right() - bw - 8);
    let below = area.bottom() + (10.0 * s) as i32;
    let above = area.y - bh - (10.0 * s) as i32;
    let y = if below + bh <= m.work.bottom() - 4 {
        below
    } else if above >= m.rect.y + 4 {
        above
    } else {
        area.bottom() - bh - (16.0 * s) as i32
    };
    if x < m.rect.x {
        x = m.rect.x;
    }
    let hwnd = win::create(CLASS, "Framegrab", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, x, y, bw, bh, None);
    win::round_corners(hwnd, false);
    win::exclude_from_capture(hwnd);
    let canvas = Canvas::new(hwnd, bw as u32, bh as u32);

    // Frame: four thin windows just outside the area.
    let t = (2.0 * s).round().max(2.0) as i32;
    let rects = [
        Rect::new(area.x - t, area.y - t, area.w + 2 * t, t),
        Rect::new(area.x - t, area.bottom(), area.w + 2 * t, t),
        Rect::new(area.x - t, area.y, t, area.h),
        Rect::new(area.right(), area.y, t, area.h),
    ];
    let mut frame = Vec::new();
    FRAME_COLOR.with(|c| *c.borrow_mut() = frame_color);
    for r in rects {
        let f = win::create(FRAME_CLASS, "", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED, r.x, r.y, r.w, r.h, None);
        unsafe {
            let _ = SetLayeredWindowAttributes(f, COLORREF(0), 230, LWA_ALPHA);
        }
        win::exclude_from_capture(f);
        if !headless() {
            unsafe {
                let _ = ShowWindow(f, SW_SHOWNOACTIVATE);
            }
        }
        frame.push(f);
    }
    B.with(|b| *b.borrow_mut() = Some(Bar { hwnd, host, canvas, scale: s, items, rects: Vec::new(), hover: None, frame, w: bw, h: bh, dot: true }));
    unsafe {
        if !headless() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        SetTimer(Some(hwnd), 1, 500, None);
    }
    win::invalidate(hwnd);
}

/// Updates the items in place (same count keeps the layout stable).
pub fn update(items: Vec<Item>) {
    with(|b| {
        b.items = items;
        win::invalidate(b.hwnd);
    });
}

thread_local! {
    static FRAME_COLOR: RefCell<u32> = const { RefCell::new(0xFF6E8BFF) };
}

fn layout(b: &Bar) -> Vec<Rect> {
    let s = b.scale;
    let pad = (6.0 * s) as i32;
    let gap = (4.0 * s) as i32;
    let mut x = pad;
    b.items
        .iter()
        .map(|it| {
            let w = item_width(it, s);
            let r = Rect::new(x, pad, w, (34.0 * s) as i32);
            x += w + gap;
            r
        })
        .collect()
}

fn paint(b: &mut Bar) {
    b.rects = layout(b);
    let Some(c) = &b.canvas else { return };
    let s = b.scale;
    c.begin();
    c.clear(0xFF1C1C21);
    c.stroke(0.5, 0.5, b.w as f32 - 1.0, b.h as f32 - 1.0, col::BORDER, 1.0);
    for (i, (it, r)) in b.items.iter().zip(&b.rects).enumerate() {
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let hover = b.hover == Some(i) && it.style != Style::Label;
        let bg = match it.style {
            Style::Primary => Some(if hover { 0xFFFF6676 } else { col::REC }),
            Style::On => Some(if hover { 0xFF3A4370 } else { 0xFF313A63 }),
            Style::Danger | Style::Normal => hover.then_some(0xFF34343C),
            Style::Label => None,
        };
        if let Some(bg) = bg {
            c.round(x, y, w, h, 7.0 * s, bg);
        }
        let fg = match it.style {
            Style::On => 0xFFB9C6FF,
            Style::Label => col::TEXT,
            Style::Danger if hover => col::REC,
            _ => col::TEXT,
        };
        if it.icon.is_empty() {
            if it.style == Style::Label && it.label.starts_with('●') {
                // Blinking recording dot + timer.
                let dot = if b.dot { col::REC } else { 0x55FF4D5E };
                c.circle(x + 12.0 * s, y + h / 2.0, 4.5 * s, dot);
                c.text(it.label.trim_start_matches('●').trim(), x + 22.0 * s, y, w - 22.0 * s, h, 13.0 * s, 600, fg, Align::Left);
            } else {
                c.text(&it.label, x, y, w, h, 13.0 * s, 600, fg, Align::Center);
            }
        } else if it.label.is_empty() {
            c.icon(it.icon, x, y, w, h, 17.0 * s, fg);
        } else {
            c.icon(it.icon, x + 8.0 * s, y, 22.0 * s, h, 16.0 * s, fg);
            c.text(&it.label, x + 32.0 * s, y, w - 36.0 * s, h, 13.0 * s, 600, fg, Align::Left);
        }
    }
    // Tooltip-ish hint for the hovered icon button.
    c.end();
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
                with(|b| {
                    b.dot = !b.dot;
                    win::invalidate(b.hwnd);
                });
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = win::lparam_xy(lp);
                with(|b| {
                    let hv = b.rects.iter().position(|r| r.contains(x, y));
                    if hv != b.hover {
                        b.hover = hv;
                        win::invalidate(h);
                        let mut tme = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
                            dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                            hwndTrack: h,
                            dwHoverTime: 0,
                        };
                        let _ = windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut tme);
                    }
                });
                LRESULT(0)
            }
            0x02A3 => {
                with(|b| {
                    b.hover = None;
                    win::invalidate(h);
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let over = with(|b| b.hover.is_some_and(|i| b.items.get(i).is_some_and(|it| it.style != Style::Label))).unwrap_or(false);
                SetCursor(LoadCursorW(None, if over { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_LBUTTONUP => {
                let (x, y) = win::lparam_xy(lp);
                let r = with(|b| {
                    let id = b.rects.iter().position(|r| r.contains(x, y)).and_then(|i| b.items.get(i)).filter(|it| it.style != Style::Label).map(|it| it.id);
                    (id, b.host)
                });
                if let Some((Some(id), host)) = r {
                    win::post(host, win::WM_REC, id as usize, 0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn frame_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let c = FRAME_COLOR.with(|c| *c.borrow());
                let br = CreateSolidBrush(COLORREF(((c >> 16) & 255) | (c & 0xFF00) | ((c & 255) << 16)));
                FillRect(dc, &ps.rcPaint, br);
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(br.into());
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(-1),
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
