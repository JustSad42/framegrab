//! Settings (config.json in the data dir) and well-known folders.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum RecFormat {
    #[default]
    Mp4,
    Gif,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(default)]
pub struct Config {
    /// Where screenshots are saved. Empty = Pictures\Screenshots.
    pub save_dir: String,
    /// Where recordings are saved. Empty = Videos\Screen Recordings.
    pub rec_dir: String,
    pub save_to_disk: bool,
    pub copy_to_clipboard: bool,
    pub show_toolbar: bool,
    pub toolbar_seconds: u32,
    pub open_editor: bool,
    pub hook_prtsc: bool,
    pub hook_win_shift_s: bool,
    pub magnifier: bool,
    pub crosshair: bool,
    pub capture_cursor: bool,
    pub rec_fps: u32,
    pub rec_format: RecFormat,
    pub rec_system_audio: bool,
    pub rec_mic: bool,
    pub rec_cursor: bool,
    pub gif_fps: u32,
    pub ocr_language: String,
    pub auto_update: bool,
    pub start_with_windows: bool,
    pub sound: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            save_dir: String::new(),
            rec_dir: String::new(),
            save_to_disk: true,
            copy_to_clipboard: true,
            show_toolbar: true,
            toolbar_seconds: 6,
            open_editor: false,
            hook_prtsc: true,
            hook_win_shift_s: true,
            magnifier: true,
            crosshair: true,
            capture_cursor: false,
            rec_fps: 30,
            rec_format: RecFormat::Mp4,
            rec_system_audio: true,
            rec_mic: false,
            rec_cursor: true,
            gif_fps: 15,
            ocr_language: String::new(),
            auto_update: true,
            start_with_windows: true,
            sound: false,
        }
    }
}

pub fn test_mode() -> bool {
    std::env::var_os("FRAMEGRAB_TEST").is_some()
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let d = base.join(if test_mode() { "Framegrab-test" } else { "Framegrab" });
    let _ = std::fs::create_dir_all(&d);
    d
}

fn known(id: &windows_core::GUID) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{KF_FLAG_CREATE, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_CREATE, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

impl Config {
    pub fn path() -> PathBuf {
        data_dir().join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    pub fn shots_dir(&self) -> PathBuf {
        if test_mode() {
            if let Some(d) = std::env::var_os("FRAMEGRAB_OUT") {
                return PathBuf::from(d);
            }
        }
        let d = if !self.save_dir.trim().is_empty() {
            PathBuf::from(self.save_dir.trim())
        } else {
            known(&windows::Win32::UI::Shell::FOLDERID_Screenshots)
                .or_else(|| known(&windows::Win32::UI::Shell::FOLDERID_Pictures).map(|p| p.join("Screenshots")))
                .unwrap_or_else(|| PathBuf::from("Screenshots"))
        };
        let _ = std::fs::create_dir_all(&d);
        d
    }

    pub fn recordings_dir(&self) -> PathBuf {
        if test_mode() {
            if let Some(d) = std::env::var_os("FRAMEGRAB_OUT") {
                return PathBuf::from(d);
            }
        }
        let d = if !self.rec_dir.trim().is_empty() {
            PathBuf::from(self.rec_dir.trim())
        } else {
            known(&windows::Win32::UI::Shell::FOLDERID_Videos).map(|p| p.join("Screen Recordings")).unwrap_or_else(|| PathBuf::from("Screen Recordings"))
        };
        let _ = std::fs::create_dir_all(&d);
        d
    }
}

/// Local time stamp like the Windows Snipping Tool uses: `2026-11-24 153012`.
pub fn stamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// `dir\<prefix> <stamp>.<ext>`, adding ` (2)` etc. when the name is taken.
pub fn unique_path(dir: &std::path::Path, prefix: &str, ext: &str) -> PathBuf {
    let base = format!("{prefix} {}", stamp());
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}
