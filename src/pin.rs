//! Pin to screen: an always-on-top copy of a capture for reference while you work.
//! Drag to move, wheel to zoom, double-click / middle-click / Esc to close, right-click for more.
//! Lives in the resident process (Win32 + the shared GPU device), so a pin costs only its image.

use crate::capture::{Image, Rect};
use crate::gfx::{self, Canvas, col};
use crate::win;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

const CLASS: &str = "FramegrabPin";

struct Pin {
    img: Image,
    canvas: Option<Canvas>,
    bmp: Option<ID2D1Bitmap>,
    zoom: f32,
}

thread_local! {
    static PINS: RefCell<HashMap<isize, Pin>> = RefCell::new(HashMap::new());
}

fn with<R>(h: HWND, f: impl FnOnce(&mut Pin) -> R) -> Option<R> {
    PINS.with(|p| p.borrow_mut().get_mut(&(h.0 as isize)).map(f))
}

/// Pins `img`. `at` is where it was captured (it appears right there); otherwise it's centred
/// on the cursor. Large images start scaled to fit the monitor.
pub fn pin(img: Image, at: Option<Rect>) {
    win::register_class(CLASS, Some(wndproc));
    let mons = crate::capture::monitors();
    let (cx, cy) = crate::capture::cursor_pos();
    let m = mons.iter().find(|m| m.rect.contains(cx, cy)).or(mons.first()).map(|m| m.work).unwrap_or(Rect::new(0, 0, 1920, 1080));
    let zoom = (m.w as f32 * 0.9 / img.w as f32).min(m.h as f32 * 0.9 / img.h as f32).min(1.0);
    let (w, h) = (((img.w as f32 * zoom) as i32).max(24), ((img.h as f32 * zoom) as i32).max(24));
    let (x, y) = match at.filter(|_| zoom >= 1.0) {
        Some(r) => (r.x, r.y),
        None => ((cx - w / 2).clamp(m.x, (m.right() - w).max(m.x)), (cy - h / 2).clamp(m.y, (m.bottom() - h).max(m.y))),
    };
    // Headless test copies park pins off-screen (read back with `--trigger pin-dump <dir>`).
    let (x, y) = if headless() { (-20000, -20000) } else { (x, y) };
    let hwnd = win::create(CLASS, "Framegrab pin", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW, x, y, w, h, None);
    win::round_corners(hwnd, true);
    let canvas = Canvas::new(hwnd, w as u32, h as u32);
    let bmp = canvas.as_ref().and_then(|c| c.bitmap(img.w, img.h, &img.px));
    PINS.with(|p| p.borrow_mut().insert(hwnd.0 as isize, Pin { img, canvas, bmp, zoom }));
    unsafe {
        if headless() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

fn headless() -> bool {
    crate::config::test_mode() && std::env::var_os("FRAMEGRAB_HEADLESS").is_some()
}

/// Test hook: zooms each pin by `factor` (around its centre) and saves what it shows.
pub fn dump(dir: &std::path::Path, factor: f32) -> String {
    let hs: Vec<isize> = PINS.with(|p| p.borrow().keys().copied().collect());
    let mut rep = String::new();
    for (i, h) in hs.iter().enumerate() {
        let hw = HWND(*h as _);
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(hw, &mut r);
        }
        if (factor - 1.0).abs() > 0.001 {
            zoom(hw, factor, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
            unsafe {
                let _ = GetWindowRect(hw, &mut r);
            }
        }
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hw);
        }
        paint(hw);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let rect = Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
        let img = crate::winfo::print_window(*h, rect);
        rep += &format!("pin {i}: {}x{} zoom={:.2} readback={}
", rect.w, rect.h, with(hw, |p| p.zoom).unwrap_or(0.0), img.is_some());
        if let Some(img) = img {
            let _ = crate::output::write_png(&dir.join(format!("pin-{i}.png")), &img, true);
        }
    }
    rep
}

pub fn close_all() {
    let hs: Vec<isize> = PINS.with(|p| p.borrow().keys().copied().collect());
    for h in hs {
        unsafe {
            let _ = DestroyWindow(HWND(h as _));
        }
    }
}

pub fn count() -> usize {
    PINS.with(|p| p.borrow().len())
}

fn paint(h: HWND) {
    let (w, hh) = win::client_size(h);
    with(h, |p| {
        let Some(c) = &p.canvas else { return };
        let active = unsafe { GetForegroundWindow() } == h;
        c.begin();
        c.clear(0xFF101013);
        if let Some(b) = &p.bmp {
            let nearest = p.zoom >= 2.0;
            c.draw_bitmap(b, gfx::rf(0.0, 0.0, w as f32, hh as f32), gfx::rf(0.0, 0.0, p.img.w as f32, p.img.h as f32), 1.0, nearest);
        }
        c.stroke(0.5, 0.5, w as f32 - 1.0, hh as f32 - 1.0, if active { col::ACCENT } else { 0x40FFFFFF }, 1.0);
        if !c.end() {
            // Device lost: rebuild on the next paint.
            p.canvas = Canvas::new(h, w, hh);
            p.bmp = p.canvas.as_ref().and_then(|c| c.bitmap(p.img.w, p.img.h, &p.img.px));
            win::invalidate(h);
        }
    });
}

/// Zooms by `factor` keeping the screen point (`ax`,`ay`) where it is.
fn zoom(h: HWND, factor: f32, ax: i32, ay: i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    let Some((iw, ih, z)) = with(h, |p| (p.img.w, p.img.h, p.zoom)) else { return };
    let nz = (z * factor).clamp(0.1, 8.0);
    let (nw, nh) = (((iw as f32 * nz) as i32).max(24), ((ih as f32 * nz) as i32).max(24));
    let fx = (ax - r.left) as f32 / (r.right - r.left).max(1) as f32;
    let fy = (ay - r.top) as f32 / (r.bottom - r.top).max(1) as f32;
    let (nx, ny) = (ax - (fx * nw as f32) as i32, ay - (fy * nh as f32) as i32);
    with(h, |p| p.zoom = nz);
    unsafe {
        let _ = SetWindowPos(h, None, nx, ny, nw, nh, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn menu(h: HWND) {
    unsafe {
        let m = CreatePopupMenu().unwrap_or_default();
        let add = |id: usize, s: &str| {
            let w = win::wide(s);
            let _ = AppendMenuW(m, MF_STRING, id, PCWSTR(w.as_ptr()));
        };
        add(1, "Copy\tCtrl+C");
        add(2, "Save as…");
        add(3, "Edit");
        add(4, "Actual size\t0");
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        add(5, "Close\tEsc");
        if count() > 1 {
            add(6, "Close all pins");
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(h);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, h, None);
        let _ = DestroyMenu(m);
        match id.0 {
            1 => copy(h),
            2 => {
                let Some(img) = with(h, |p| p.img.clone()) else { return };
                let dir = crate::config::Config::load().shots_dir();
                if let Some(p) = rfd::FileDialog::new().set_directory(dir).set_file_name(format!("Pin {}.png", crate::config::stamp())).add_filter("PNG image", &["png"]).save_file() {
                    let _ = crate::output::write_png(&p, &img, false);
                }
            }
            3 => {
                if let Some(img) = with(h, |p| p.img.clone()) {
                    let p = std::env::temp_dir().join(format!("Framegrab pin {}.png", crate::config::stamp()));
                    if crate::output::write_png(&p, &img, true).is_ok() {
                        crate::app::spawn_self(&["--edit", &p.to_string_lossy()]);
                    }
                }
            }
            4 => {
                let mut r = RECT::default();
                let _ = GetWindowRect(h, &mut r);
                let z = with(h, |p| p.zoom).unwrap_or(1.0);
                zoom(h, 1.0 / z, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
            }
            5 => {
                let _ = DestroyWindow(h);
            }
            6 => close_all(),
            _ => {}
        }
    }
}

fn copy(h: HWND) {
    if let Some(img) = with(h, |p| p.img.clone()) {
        crate::output::copy_image(&img);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // The whole pin is a drag handle.
            WM_NCHITTEST => LRESULT(HTCAPTION as isize),
            WM_NCLBUTTONDBLCLK | WM_NCMBUTTONUP => {
                let _ = DestroyWindow(h);
                LRESULT(0)
            }
            WM_NCRBUTTONUP | WM_CONTEXTMENU => {
                menu(h);
                LRESULT(0)
            }
            // No system menu / maximize from the caption hit-test.
            WM_NCRBUTTONDOWN => LRESULT(0),
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                let (x, y) = win::lparam_xy(lp);
                zoom(h, if delta > 0 { 1.1 } else { 1.0 / 1.1 }, x, y);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                match wp.0 as u32 {
                    0x1B => {
                        let _ = DestroyWindow(h);
                    }
                    0x43 if ctrl => copy(h),
                    0x30 | 0x60 => {
                        let mut r = RECT::default();
                        let _ = GetWindowRect(h, &mut r);
                        let z = with(h, |p| p.zoom).unwrap_or(1.0);
                        zoom(h, 1.0 / z, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SIZE => {
                let (w, hh) = ((lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                with(h, |p| {
                    if let Some(c) = &p.canvas {
                        c.resize(w, hh);
                    }
                });
                win::invalidate(h);
                LRESULT(0)
            }
            WM_ACTIVATE => {
                win::invalidate(h);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(h, &mut ps);
                paint(h);
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_DESTROY => {
                PINS.with(|p| p.borrow_mut().remove(&(h.0 as isize)));
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
