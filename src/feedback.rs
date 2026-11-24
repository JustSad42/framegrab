//! Optional capture feedback: a short shutter sound and a quick white flash over the area.

use crate::capture::Rect;
use crate::win;
use std::sync::OnceLock;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, FillRect, GetStockObject, HBRUSH, PAINTSTRUCT, WHITE_BRUSH};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

/// A synthesized two-part "click-clack" shutter (16-bit mono WAV, ~90 ms).
fn shutter_wav() -> &'static [u8] {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| {
        let rate = 44100u32;
        let n = (rate as f32 * 0.09) as usize;
        let mut seed = 0x1234_5678u32;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / rate as f32;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            // Two decaying bursts (shutter open/close) with a little body.
            let e1 = (-t * 140.0).exp();
            let e2 = if t > 0.045 { (-(t - 0.045) * 160.0).exp() * 0.8 } else { 0.0 };
            let tone = (t * 2.0 * std::f32::consts::PI * 1800.0).sin() * 0.25;
            let v = (noise * 0.75 + tone) * (e1 + e2) * 0.55;
            samples.push((v.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let data_len = (samples.len() * 2) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    })
}

pub fn sound() {
    let wav = shutter_wav();
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as _), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

const CLASS: &str = "FramegrabFlash";
static FLASH: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static ALPHA: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn init() {
    win::register_class(CLASS, Some(proc_));
}

/// A white flash over `r` that fades out in ~150 ms. Never appears in captures.
pub fn flash(r: Rect) {
    if std::env::var_os("FRAMEGRAB_HEADLESS").is_some() || r.is_empty() {
        return;
    }
    let old = FLASH.swap(0, std::sync::atomic::Ordering::Relaxed);
    if old != 0 {
        unsafe {
            let _ = DestroyWindow(HWND(old as _));
        }
    }
    let h = win::create(CLASS, "", WS_POPUP, WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT, r.x, r.y, r.w, r.h, None);
    win::exclude_from_capture(h);
    ALPHA.store(110, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 110, LWA_ALPHA);
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        SetTimer(Some(h), 1, 15, None);
    }
    FLASH.store(h.0 as isize, std::sync::atomic::Ordering::Relaxed);
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                FillRect(dc, &ps.rcPaint, HBRUSH(GetStockObject(WHITE_BRUSH).0));
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_TIMER => {
                let a = ALPHA.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(14);
                ALPHA.store(a, std::sync::atomic::Ordering::Relaxed);
                if a == 0 {
                    let _ = KillTimer(Some(h), 1);
                    let _ = DestroyWindow(h);
                    let _ = FLASH.compare_exchange(h.0 as isize, 0, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed);
                } else {
                    let _ = SetLayeredWindowAttributes(h, COLORREF(0), a as u8, LWA_ALPHA);
                }
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(-1),
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}
