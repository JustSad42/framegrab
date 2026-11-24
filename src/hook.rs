//! Low-level keyboard hook: takes over PrtSc (with modifiers) and Win+Shift+S, and routes
//! overlay keys while a capture is in progress so they work even if another app has focus.

use crate::win;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use windows::Win32::Foundation::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Tray "Pause hotkeys" switch, and set while a full-screen game/app is in front (if enabled).
pub static PAUSED: AtomicBool = AtomicBool::new(false);
pub static GAME: AtomicBool = AtomicBool::new(false);
static BINDINGS: std::sync::RwLock<Vec<(crate::config::Hotkey, Cmd)>> = std::sync::RwLock::new(Vec::new());
static SWALLOW_VK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Installs the key table from the settings.
pub fn set_bindings(cfg: &crate::config::Config) {
    let h = &cfg.hotkeys;
    let mut v = vec![(h.snip, Cmd::Region), (h.screen, Cmd::Screen), (h.window, Cmd::Window), (h.all, Cmd::AllScreens), (h.text, Cmd::Text), (h.scroll, Cmd::Scroll), (h.record, Cmd::Record), (h.menu, Cmd::Menu)];
    if cfg.hook_win_shift_s {
        v.push((crate::config::Hotkey { vk: 'S' as u32, ctrl: false, shift: true, alt: false, win: true }, Cmd::Region));
    }
    v.retain(|b| b.0.is_set());
    *BINDINGS.write().unwrap() = v;
}
/// While true, overlay keys (Esc, Enter, arrows, digits...) are swallowed and sent to the host.
pub static ROUTE: AtomicBool = AtomicBool::new(false);
static HOST: AtomicIsize = AtomicIsize::new(0);


const MARK: usize = 0x46475242; // "FGRB": our own injected keys

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Region = 1,
    Screen = 2,
    AllScreens = 3,
    Window = 4,
    Text = 5,
    Scroll = 6,
    Record = 7,
    Menu = 8,
}

impl Cmd {
    pub fn from(v: u32) -> Option<Cmd> {
        Some(match v {
            1 => Cmd::Region,
            2 => Cmd::Screen,
            3 => Cmd::AllScreens,
            4 => Cmd::Window,
            5 => Cmd::Text,
            6 => Cmd::Scroll,
            7 => Cmd::Record,
            8 => Cmd::Menu,
            _ => return None,
        })
    }
    pub fn parse(s: &str) -> Option<Cmd> {
        Some(match s {
            "region" => Cmd::Region,
            "screen" => Cmd::Screen,
            "all" => Cmd::AllScreens,
            "window" => Cmd::Window,
            "text" => Cmd::Text,
            "scroll" => Cmd::Scroll,
            "record" => Cmd::Record,
            "menu" => Cmd::Menu,
            _ => return None,
        })
    }
}

fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

/// An unassigned key press so the shell doesn't open Start when Win is released.
fn mask_win() {
    unsafe {
        let mut inp = [INPUT::default(); 2];
        for (i, up) in [(0, false), (1, true)] {
            inp[i].r#type = INPUT_KEYBOARD;
            inp[i].Anonymous.ki = KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, dwExtraInfo: MARK, ..Default::default() };
        }
        SendInput(&inp, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 {
            let kb = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            if kb.dwExtraInfo != MARK {
                let host = HWND(HOST.load(Relaxed) as _);
                let vk = VIRTUAL_KEY(kb.vkCode as u16);
                let is_down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
                let is_up = wp.0 as u32 == WM_KEYUP || wp.0 as u32 == WM_SYSKEYUP;
                let winkey = down(VK_LWIN) || down(VK_RWIN);

                if is_up && SWALLOW_VK.load(Relaxed) == vk.0 as u32 {
                    SWALLOW_VK.store(0, Relaxed);
                    return LRESULT(1);
                }
                if is_down && !PAUSED.load(Relaxed) && !GAME.load(Relaxed) && !matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL | VK_SHIFT | VK_LSHIFT | VK_RSHIFT | VK_MENU | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN) {
                    let (ctrl, shift, alt) = (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU));
                    let hit = BINDINGS.read().ok().and_then(|b| b.iter().find(|(h, _)| h.vk == vk.0 as u32 && h.ctrl == ctrl && h.shift == shift && h.alt == alt && h.win == winkey).map(|b| b.1));
                    if let Some(cmd) = hit {
                        if winkey {
                            mask_win();
                        }
                        SWALLOW_VK.store(vk.0 as u32, Relaxed);
                        win::post(host, win::WM_CMD, cmd as usize, 0);
                        return LRESULT(1);
                    }
                }                if ROUTE.load(Relaxed) {
                    let routed = matches!(vk, VK_ESCAPE | VK_RETURN | VK_SPACE | VK_TAB | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN)
                        || (0x31..=0x35).contains(&vk.0)
                        || vk.0 == 'F' as u16
                        || vk.0 == 'C' as u16;
                    if routed && !winkey && !down(VK_CONTROL) && !down(VK_MENU) {
                        if is_down {
                            win::post(host, win::WM_KEYROUTE, vk.0 as usize, 0);
                        }
                        return LRESULT(1);
                    }
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

/// Starts the hook on its own thread (hooks need a message loop and must answer fast).
pub fn start(host: HWND) {
    HOST.store(host.0 as isize, Relaxed);
    std::thread::Builder::new()
        .name("kbhook".into())
        .spawn(|| unsafe {
            let _ = windows::Win32::System::Threading::SetThreadPriority(
                windows::Win32::System::Threading::GetCurrentThread(),
                windows::Win32::System::Threading::THREAD_PRIORITY_TIME_CRITICAL,
            );
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), Some(win::hinstance()), 0);
            if hook.is_err() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        })
        .ok();
}

