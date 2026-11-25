//! Small Win32 helpers shared by the native windows.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

pub const WM_CMD: u32 = WM_APP + 1;
pub const WM_OVERLAY_DONE: u32 = WM_APP + 2;
pub const WM_JOB: u32 = WM_APP + 4;
pub const WM_KEYROUTE: u32 = WM_APP + 5;
pub const WM_TOAST: u32 = WM_APP + 6;
pub const WM_REC: u32 = WM_APP + 7;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|h| HINSTANCE(h.0)).unwrap_or_default() }
}

pub fn register_class(name: &str, proc_: WNDPROC) {
    let n = wide(name);
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: proc_,
            hInstance: hinstance(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(n.as_ptr()),
            hIcon: LoadIconW(Some(hinstance()), PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

pub fn create(class: &str, title: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE, x: i32, y: i32, w: i32, h: i32, parent: Option<HWND>) -> HWND {
    let c = wide(class);
    let t = wide(title);
    unsafe { CreateWindowExW(ex, PCWSTR(c.as_ptr()), PCWSTR(t.as_ptr()), style, x, y, w, h, parent, None, Some(hinstance()), None).unwrap_or_default() }
}

pub fn cloak(h: HWND, on: bool) {
    let v: windows_core::BOOL = on.into();
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &v as *const _ as _, 4);
    }
}

/// Dark title bar / rounded corners for small popups.
pub fn round_corners(h: HWND, small: bool) {
    let pref = if small { DWMWCP_ROUNDSMALL } else { DWMWCP_ROUND };
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as _, 4);
        let dark: windows_core::BOOL = true.into();
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
    }
}

/// Keeps a window out of screenshots and recordings.
pub fn exclude_from_capture(h: HWND) {
    if crate::config::test_mode() && std::env::var_os("FRAMEGRAB_ALLOW_CAPTURE").is_some() {
        return;
    }
    unsafe {
        let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
    }
}

pub fn post(h: HWND, msg: u32, w: usize, l: isize) {
    unsafe {
        let _ = PostMessageW(Some(h), msg, WPARAM(w), LPARAM(l));
    }
}

pub fn lparam_xy(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32)
}

/// Brings our window to the front even when another app has focus.
pub fn force_foreground(h: HWND) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    unsafe {
        if SetForegroundWindow(h).as_bool() {
            return;
        }
        // An injected Alt press gives this process the right to take the foreground.
        let mut inp = [INPUT::default(); 2];
        inp[0].r#type = INPUT_KEYBOARD;
        inp[0].Anonymous.ki = KEYBDINPUT { wVk: VK_MENU, ..Default::default() };
        inp[1].r#type = INPUT_KEYBOARD;
        inp[1].Anonymous.ki = KEYBDINPUT { wVk: VK_MENU, dwFlags: KEYEVENTF_KEYUP, ..Default::default() };
        SendInput(&inp, std::mem::size_of::<INPUT>() as i32);
        let _ = SetForegroundWindow(h);
    }
}

pub fn invalidate(h: HWND) {
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(h), None, false);
    }
}

pub fn client_size(h: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
}

