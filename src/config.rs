//! Settings (config.json in the data dir) and well-known folders.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum RecFormat {
    #[default]
    Mp4,
    Gif,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum ImgFormat {
    #[default]
    Png,
    Jpg,
    Webp,
    Avif,
}

impl ImgFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImgFormat::Png => "png",
            ImgFormat::Jpg => "jpg",
            ImgFormat::Webp => "webp",
            ImgFormat::Avif => "avif",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum TextLayout {
    /// One output line per line on screen.
    #[default]
    Lines,
    /// Lines of the same paragraph joined with spaces.
    Paragraphs,
    /// Everything on one line (codes, IDs).
    SingleLine,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum UploadProvider {
    #[default]
    Off,
    Imgur,
    Custom,
    ImgBB,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Upload {
    pub provider: UploadProvider,
    /// Upload every screenshot automatically (otherwise only via the card's Upload button).
    pub auto: bool,
    /// After uploading, put the link on the clipboard (otherwise the image stays there).
    pub copy_link: bool,
    pub imgur_client_id: String,
    /// imgbb.com API key (api.imgbb.com).
    pub imgbb_key: String,
    pub custom_url: String,
    pub custom_field: String,
    pub custom_header: String,
    /// Dot path to the link in a JSON response (e.g. `data.link`); empty = the response is the link.
    pub custom_response_path: String,
}

impl Default for Upload {
    fn default() -> Self {
        Self {
            provider: UploadProvider::Off,
            auto: false,
            copy_link: true,
            imgur_client_id: String::new(),
            imgbb_key: String::new(),
            custom_url: String::new(),
            custom_field: "file".into(),
            custom_header: String::new(),
            custom_response_path: String::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
pub enum TranslateProvider {
    #[default]
    Off,
    Libre,
    DeepL,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Translate {
    pub provider: TranslateProvider,
    /// Two-letter target language; empty = Windows' display language.
    pub target: String,
    /// LibreTranslate server (e.g. https://libretranslate.com).
    pub url: String,
    /// LibreTranslate or DeepL API key.
    pub key: String,
}

/// A key combination. `vk` 0 = unbound.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(default)]
pub struct Hotkey {
    pub vk: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
}

pub const VK_SNAPSHOT: u32 = 0x2C;

impl Hotkey {
    pub const fn prtsc(ctrl: bool, shift: bool, alt: bool, win: bool) -> Self {
        Self { vk: VK_SNAPSHOT, ctrl, shift, alt, win }
    }
    pub fn is_set(&self) -> bool {
        self.vk != 0
    }
    pub fn label(&self) -> String {
        if self.vk == 0 {
            return "Not set".into();
        }
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl+";
        }
        if self.alt {
            s += "Alt+";
        }
        if self.shift {
            s += "Shift+";
        }
        if self.win {
            s += "Win+";
        }
        s + &key_name(self.vk)
    }
}

pub fn key_name(vk: u32) -> String {
    match vk {
        0x2C => "Print Screen".into(),
        0x13 => "Pause".into(),
        0x91 => "Scroll Lock".into(),
        0x2D => "Insert".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x2E => "Delete".into(),
        0x20 => "Space".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0xC0 => "`".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xDC => "\\".into(),
        _ => format!("Key {vk:#04X}"),
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct Hotkeys {
    pub snip: Hotkey,
    pub screen: Hotkey,
    pub window: Hotkey,
    pub all: Hotkey,
    pub text: Hotkey,
    pub scroll: Hotkey,
    pub record: Hotkey,
    pub menu: Hotkey,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            snip: Hotkey::prtsc(false, false, false, false),
            screen: Hotkey::prtsc(false, true, false, false),
            window: Hotkey::prtsc(false, false, true, false),
            all: Hotkey::prtsc(false, false, false, true),
            text: Hotkey::prtsc(true, false, false, false),
            scroll: Hotkey::prtsc(true, false, true, false),
            record: Hotkey::prtsc(true, true, false, false),
            menu: Hotkey::default(),
        }
    }
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
    /// Legacy master switch (kept so old config files load); hotkeys live in `hotkeys`.
    pub hook_prtsc: bool,
    pub hook_win_shift_s: bool,
    pub hotkeys: Hotkeys,
    pub pause_in_fullscreen: bool,
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
    pub text_layout: TextLayout,
    pub detect_tables: bool,
    pub read_qr: bool,
    pub auto_update: bool,
    pub start_with_windows: bool,
    pub sound: bool,
    pub flash: bool,
    /// File name template: {date} {time} {app} {title} {mode} {w} {h}.
    pub name_template: String,
    pub app_subfolders: bool,
    pub format: ImgFormat,
    pub quality: u8,
    /// Clicking a window captures it with PrintWindow, so covered parts are included.
    pub window_full_content: bool,
    /// Window captures get transparent rounded corners like on screen.
    pub window_transparent: bool,
    pub beautify: bool,
    pub beautify_bg: u8,
    pub beautify_padding: u32,
    pub beautify_radius: u32,
    pub beautify_shadow: bool,
    pub upload: Upload,
    /// When the grabbed text is just a link, code or email address, copy only that.
    pub smart_copy: bool,
    /// Selection edges stick to edges in the screenshot (hold Alt to drag freely).
    pub snap_edges: bool,
    /// Editor arrow tips land on the nearest edge (hold Alt to place freely).
    pub smart_arrows: bool,
    pub translate: Translate,
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
            hotkeys: Hotkeys::default(),
            pause_in_fullscreen: true,
            magnifier: false,
            crosshair: true,
            capture_cursor: false,
            rec_fps: 60,
            rec_format: RecFormat::Mp4,
            rec_system_audio: true,
            rec_mic: false,
            rec_cursor: true,
            gif_fps: 15,
            ocr_language: String::new(),
            text_layout: TextLayout::Lines,
            detect_tables: true,
            read_qr: true,
            auto_update: true,
            start_with_windows: true,
            sound: false,
            flash: true,
            name_template: "Screenshot {date} {time}".into(),
            app_subfolders: false,
            format: ImgFormat::Png,
            quality: 90,
            window_full_content: true,
            window_transparent: true,
            beautify: false,
            beautify_bg: 0,
            beautify_padding: 56,
            beautify_radius: 12,
            beautify_shadow: true,
            upload: Upload::default(),
            smart_copy: true,
            snap_edges: true,
            smart_arrows: true,
            translate: Translate::default(),
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

/// Values for the file-name template.
#[derive(Default, Clone)]
pub struct NameInfo {
    pub app: String,
    pub title: String,
    pub mode: String,
    pub w: u32,
    pub h: u32,
}

fn sanitize(s: &str, max: usize) -> String {
    let cleaned: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { ' ' } else { c }).collect();
    let t: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    t.chars().take(max).collect::<String>().trim_end_matches('.').trim().to_string()
}

/// Expands the template and returns a free path (`dir[\app]\name.ext`).
pub fn template_path(dir: &std::path::Path, template: &str, info: &NameInfo, subfolder: bool, ext: &str) -> PathBuf {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let date = format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay);
    let time = format!("{:02}{:02}{:02}", t.wHour, t.wMinute, t.wSecond);
    let app = if info.app.is_empty() { "Desktop".to_string() } else { sanitize(&info.app, 40) };
    let tpl = if template.trim().is_empty() { "Screenshot {date} {time}" } else { template };
    let name = tpl
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{app}", &app)
        .replace("{title}", &sanitize(&info.title, 60))
        .replace("{mode}", &info.mode)
        .replace("{w}", &info.w.to_string())
        .replace("{h}", &info.h.to_string());
    let mut name = sanitize(&name, 150);
    if name.is_empty() {
        name = format!("Screenshot {date} {time}");
    }
    let dir = if subfolder { dir.join(&app) } else { dir.to_path_buf() };
    let _ = std::fs::create_dir_all(&dir);
    let mut p = dir.join(format!("{name}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{name} ({n}).{ext}"));
        n += 1;
    }
    p
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


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_expands_and_sanitizes() {
        let dir = std::env::temp_dir().join("framegrab-tpl-test");
        let info = NameInfo { app: "Google Chrome".into(), title: "a/b: c?".into(), mode: "Snip".into(), w: 10, h: 20 };
        let p = template_path(&dir, "{app} - {title} {w}x{h}", &info, true, "png");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "Google Chrome - a b c 10x20.png");
        assert!(p.parent().unwrap().ends_with("Google Chrome"));
    }
    #[test]
    fn old_config_loads_with_new_defaults() {
        let c: Config = serde_json::from_str(r#"{"rec_fps":30,"sound":true}"#).unwrap();
        assert_eq!(c.rec_fps, 30);
        assert!(c.sound);
        assert_eq!(c.hotkeys.snip.label(), "Print Screen");
        assert_eq!(c.hotkeys.record.label(), "Ctrl+Shift+Print Screen");
    }
}
