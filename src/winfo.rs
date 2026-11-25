//! Window details for file names (app + title) and full-content window capture.

use crate::capture::{Image, Rect};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{PCWSTR, PWSTR};

pub fn title(h: isize) -> String {
    let mut t = [0u16; 256];
    let n = unsafe { GetWindowTextW(HWND(h as _), &mut t) };
    String::from_utf16_lossy(&t[..n.max(0) as usize])
}

fn exe_path(h: isize) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(HWND(h as _), Some(&mut pid));
        let ph = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(ph, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(ph);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The app's display name from its version info ("Google Chrome"), else the exe name.
fn describe(path: &str) -> Option<String> {
    unsafe {
        let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, data.as_mut_ptr() as _).ok()?;
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut n = 0u32;
        let q: Vec<u16> = "\\VarFileInfo\\Translation\0".encode_utf16().collect();
        let mut langs = vec![(0x0409u16, 0x04B0u16)];
        if VerQueryValueW(data.as_ptr() as _, PCWSTR(q.as_ptr()), &mut p, &mut n).as_bool() && n >= 4 {
            let s = std::slice::from_raw_parts(p as *const u16, (n / 2) as usize);
            langs.insert(0, (s[0], s[1]));
        }
        for (l, cp) in langs {
            let key: Vec<u16> = format!("\\StringFileInfo\\{l:04x}{cp:04x}\\FileDescription\0").encode_utf16().collect();
            if VerQueryValueW(data.as_ptr() as _, PCWSTR(key.as_ptr()), &mut p, &mut n).as_bool() && n > 1 {
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, n as usize - 1));
                let s = s.trim().to_string();
                if !s.is_empty() && s.len() <= 40 {
                    return Some(s);
                }
            }
        }
        None
    }
}

pub fn app_name(h: isize) -> String {
    let Some(path) = exe_path(h) else { return String::new() };
    if let Some(d) = describe(&path) {
        return d;
    }
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut c = stem.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn is_maximized(h: isize) -> bool {
    unsafe { IsZoomed(HWND(h as _)).as_bool() }
}

/// Windows stretches apps that aren't DPI-aware (many games, e.g. League of Legends) to the
/// monitor's scale; PrintWindow then returns their small unstretched picture in one corner with
/// the rest black. Returns that stretch factor (monitor DPI / window DPI) for such windows.
pub fn dpi_stretch(hwnd: HWND) -> Option<f32> {
    use windows::Win32::UI::HiDpi::*;
    unsafe {
        let aware = GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd));
        if aware == DPI_AWARENESS_PER_MONITOR_AWARE {
            return None;
        }
        let wdpi = GetDpiForWindow(hwnd).max(96);
        let hm = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).ok()?;
        (dx != wdpi).then(|| dx as f32 / wdpi as f32)
    }
}

/// Captures a window's own pixels (works when it's covered). `frame` is its visible bounds.
pub fn print_window(h: isize, frame: Rect) -> Option<Image> {
    unsafe {
        let hwnd = HWND(h as _);
        let mut wr = RECT::default();
        GetWindowRect(hwnd, &mut wr).ok()?;
        let (ww, wh) = (wr.right - wr.left, wr.bottom - wr.top);
        if ww <= 0 || wh <= 0 || ww > 16384 || wh > 16384 {
            return None;
        }
        // DPI-stretched window: when it's in front, the screen already has it at full resolution
        // (sharper than upscaling its own small picture), so keep the screen pixels.
        let stretch = dpi_stretch(hwnd);
        if stretch.is_some() && GetForegroundWindow() == hwnd {
            return None;
        }
        // Otherwise grab its own (smaller) picture and scale it up to the size on screen.
        let (ww_full, wh_full) = (ww, wh);
        let (ww, wh) = match stretch {
            Some(k) => (((ww as f32 / k).round() as i32).max(1), ((wh as f32 / k).round() as i32).max(1)),
            None => (ww, wh),
        };
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: ww, biHeight: -wh, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut out = None;
        if let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, dib.into());
            // PW_RENDERFULLCONTENT (2) also captures DirectX / browser content.
            if PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(2)).as_bool() {
                let _ = GdiFlush();
                let mut full = Image::new(ww as u32, wh as u32);
                std::ptr::copy_nonoverlapping(bits as *const u8, full.px.as_mut_ptr(), full.px.len());
                if (ww, wh) != (ww_full, wh_full) {
                    full = full.resize(ww_full as u32, wh_full as u32);
                }
                let img = full.crop(frame.x - wr.left, frame.y - wr.top, frame.w, frame.h);
                // A fully black result means the app didn't draw for PrintWindow.
                let lit = img.px.chunks_exact(4).step_by(97).any(|p| p[0] | p[1] | p[2] != 0);
                if lit && img.w > 0 && img.h > 0 {
                    let mut img = img;
                    for p in img.px.chunks_exact_mut(4) {
                        p[3] = 255;
                    }
                    out = Some(img);
                }
            }
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// Makes the corners transparent with an anti-aliased radius (Windows 11 window shape).
pub fn round_corners(img: &mut Image, r: f32) {
    let (w, h) = (img.w as i32, img.h as i32);
    let ri = r.ceil() as i32;
    if ri <= 0 || w < ri * 2 || h < ri * 2 {
        return;
    }
    for cy in 0..ri {
        for cx in 0..ri {
            // Distance from the corner circle's centre, sampled at the pixel centre.
            let dx = r - (cx as f32 + 0.5);
            let dy = r - (cy as f32 + 0.5);
            let d = (dx * dx + dy * dy).sqrt();
            let a = (r - d + 0.5).clamp(0.0, 1.0);
            if a >= 1.0 {
                continue;
            }
            for (x, y) in [(cx, cy), (w - 1 - cx, cy), (cx, h - 1 - cy), (w - 1 - cx, h - 1 - cy)] {
                let i = ((y * w + x) * 4) as usize;
                img.px[i + 3] = (img.px[i + 3] as f32 * a) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn corners_become_transparent() {
        let mut img = crate::capture::Image::new(40, 30);
        for p in img.px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        super::round_corners(&mut img, 8.0);
        assert_eq!(img.px[3], 0);
        assert_eq!(img.px[((15 * 40 + 20) * 4 + 3) as usize], 255);
        assert_eq!(img.px[((29 * 40 + 39) * 4 + 3) as usize], 0);
    }
}

#[cfg(test)]
mod live {
    /// FRAMEGRAB_DPI_TITLE=<window title> cargo test --release dpi_stretch_live -- --ignored --nocapture
    /// Grabs a non-DPI-aware window (not in front) and checks the picture fills the whole frame.
    #[test]
    #[ignore]
    fn dpi_stretch_live() {
        use super::*;
        use windows::Win32::UI::HiDpi::*;
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let title = std::env::var("FRAMEGRAB_DPI_TITLE").unwrap();
        let w = windows_core::HSTRING::from(title.as_str());
        let h = unsafe { FindWindowW(None, &w) }.expect("window");
        let k = super::dpi_stretch(h);
        let mut r = RECT::default();
        unsafe { GetWindowRect(h, &mut r).unwrap() };
        let frame = Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
        let fg = unsafe { GetForegroundWindow() } == h;
        println!("stretch {k:?} foreground {fg} frame {frame:?}");
        let Some(img) = super::print_window(h.0 as isize, frame) else {
            assert!(fg && k.is_some(), "no grab although not in front");
            println!("in front: kept screen pixels (as intended)");
            return;
        };
        // Fraction of non-black pixels in the bottom-right quarter (black there = the old bug).
        let (mut lit, mut n) = (0, 0);
        for y in img.h * 3 / 4..img.h {
            for x in img.w * 3 / 4..img.w {
                let i = ((y * img.w + x) * 4) as usize;
                n += 1;
                if img.px[i] as u32 + img.px[i + 1] as u32 + img.px[i + 2] as u32 > 30 {
                    lit += 1;
                }
            }
        }
        println!("stretch {k:?}, {}x{}, bottom-right lit {:.0}%", img.w, img.h, lit as f32 * 100.0 / n as f32);
        let _ = crate::output::write_png(&std::env::temp_dir().join("framegrab-dpi.png"), &img, true);
        assert!(k.is_some(), "window should be DPI-stretched");
        assert!(lit * 2 > n, "bottom-right should not be black");
    }
}
