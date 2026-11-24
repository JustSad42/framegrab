//! Settings window (egui). Everything here is set once; captures then apply it automatically.
//! Changes save immediately and the running copy reloads them.

use crate::config::{Config, Hotkey, Hotkeys, ImgFormat, NameInfo, RecFormat, TextLayout, UploadProvider};
use crate::ui::{self, ACCENT, BORDER, CARD, FAINT, MUTED, TEXT};
use egui::{Color32, CornerRadius, RichText, Stroke};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

struct Settings {
    cfg: Config,
    saved: Config,
    langs: Vec<(String, String)>,
    snip_key: Option<bool>,
    update: Option<String>,
    frames: u32,
    paused: bool,
    /// Which hotkey row is waiting for a key combination.
    recording: Option<usize>,
}

static SHOT: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>> = std::sync::LazyLock::new(Default::default);

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Framegrab Settings").with_inner_size([660.0, 820.0]).with_min_inner_size([560.0, 480.0]).with_icon(ui::app_icon()),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Framegrab Settings",
        options,
        Box::new(|cc| {
            ui::install(&cc.egui_ctx);
            let cfg = Config::load();
            let paused = std::fs::read_to_string(crate::config::data_dir().join("status")).is_ok_and(|s| s.trim() == "paused");
            Ok(Box::new(Settings { saved: cfg.clone(), cfg, langs: crate::ocr::languages(), snip_key: snip_key_setting(), update: None, frames: 0, paused, recording: None }))
        }),
    );
    recorder::stop();
}

/// Windows' own "Print Screen opens Snipping Tool" switch (None = not set).
fn snip_key_setting() -> Option<bool> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg").args(["query", r"HKCU\Control Panel\Keyboard", "/v", "PrintScreenKeyForSnippingEnabled"]).creation_flags(0x0800_0000).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return None;
    }
    Some(s.contains("0x1"))
}

/// Records a key combination with its own low-level hook, so Print Screen and Win combos
/// reach Settings instead of Windows or the running Framegrab.
mod recorder {
    use super::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    pub static ACTIVE: AtomicBool = AtomicBool::new(false);
    static THREAD: AtomicU32 = AtomicU32::new(0);
    /// Some(Some(k)) = recorded, Some(None) = cleared, None = nothing yet.
    pub static RESULT: Mutex<Option<Option<Hotkey>>> = Mutex::new(None);
    static CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

    fn down(vk: VIRTUAL_KEY) -> bool {
        unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
    }

    fn is_modifier(vk: u32) -> bool {
        matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
    }

    unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            if code == HC_ACTION as i32 && ACTIVE.load(Relaxed) {
                let kb = &*(lp.0 as *const KBDLLHOOKSTRUCT);
                let vk = kb.vkCode;
                if !is_modifier(vk) {
                    let is_down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
                    if is_down {
                        let win = down(VK_LWIN) || down(VK_RWIN);
                        let k = Hotkey { vk, ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU), win };
                        let plain = !k.ctrl && !k.shift && !k.alt && !k.win;
                        // Esc keeps the old combo (vk MAX marker); Backspace/Delete clears it.
                        let res = match vk {
                            0x1B if plain => Some(Hotkey { vk: u32::MAX, ..Default::default() }),
                            0x08 | 0x2E if plain => None,
                            _ => Some(k),
                        };
                        *RESULT.lock().unwrap() = Some(res);
                        if win {
                            // Stop the Win key-up from opening Start.
                            let inp = [
                                INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), ..Default::default() } } },
                                INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), dwFlags: KEYEVENTF_KEYUP, ..Default::default() } } },
                            ];
                            SendInput(&inp, std::mem::size_of::<INPUT>() as i32);
                        }
                        ACTIVE.store(false, Relaxed);
                        if let Some(c) = CTX.lock().unwrap().as_ref() {
                            c.request_repaint();
                        }
                    }
                    // Swallow it so no capture starts and no app sees it.
                    return LRESULT(1);
                }
            }
            CallNextHookEx(None, code, wp, lp)
        }
    }

    pub fn start(ctx: &egui::Context) {
        *CTX.lock().unwrap() = Some(ctx.clone());
        *RESULT.lock().unwrap() = None;
        ACTIVE.store(true, Relaxed);
        if THREAD.load(Relaxed) != 0 {
            return;
        }
        std::thread::spawn(|| unsafe {
            THREAD.store(GetCurrentThreadId(), Relaxed);
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), None, 0);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            if let Ok(h) = hook {
                let _ = UnhookWindowsHookEx(h);
            }
            THREAD.store(0, Relaxed);
        });
    }

    pub fn cancel() {
        ACTIVE.store(false, Relaxed);
    }

    pub fn stop() {
        ACTIVE.store(false, Relaxed);
        let t = THREAD.load(Relaxed);
        if t != 0 {
            unsafe {
                let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).font(ui::bold(14.0)).color(TEXT));
    ui.add_space(2.0);
    egui::Frame::new().fill(CARD).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
    ui.add_space(12.0);
}

fn switch(ui: &mut egui::Ui, v: &mut bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(38.0, 22.0), egui::Sense::click());
    if resp.clicked() {
        *v = !*v;
    }
    let t = ui.ctx().animate_bool(resp.id, *v);
    let bg = if *v { ACCENT } else { Color32::from_rgb(0x3a, 0x3a, 0x44) };
    ui.painter().rect_filled(r, CornerRadius::same(11), bg);
    let x = egui::lerp(r.left() + 11.0..=r.right() - 11.0, t);
    ui.painter().circle_filled(egui::pos2(x, r.center().y), 8.0, Color32::WHITE);
    resp
}

fn toggle(ui: &mut egui::Ui, v: &mut bool, label: &str, hint: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).color(TEXT));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).small().color(FAINT));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = switch(ui, v).clicked();
        });
    });
    changed
}

/// A label on the left, controls on the right.
fn row(ui: &mut egui::Ui, label: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
}

fn slider<N: egui::emath::Numeric>(ui: &mut egui::Ui, v: &mut N, range: std::ops::RangeInclusive<N>, suffix: &str) {
    ui.allocate_ui_with_layout(egui::vec2(300.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().slider_width = 200.0;
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x44);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        ui.visuals_mut().selection.bg_fill = ACCENT;
        ui.add(egui::Slider::new(v, range).suffix(suffix).trailing_fill(true));
    });
}

fn hint(ui: &mut egui::Ui, s: &str) {
    ui.label(RichText::new(s).small().color(FAINT));
}

fn folder_row(ui: &mut egui::Ui, label: &str, value: &mut String, default: std::path::PathBuf) {
    ui.label(RichText::new(label).color(TEXT));
    ui.horizontal(|ui| {
        let shown = if value.trim().is_empty() { default.display().to_string() } else { value.clone() };
        ui.add(egui::Label::new(RichText::new(shown).color(MUTED)).truncate()).on_hover_text("Click Change to pick another folder");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Open").clicked() {
                let p = if value.trim().is_empty() { default.clone() } else { std::path::PathBuf::from(value.as_str()) };
                let _ = std::process::Command::new("explorer").arg(p).spawn();
            }
            if ui.button("Change…").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_directory(&default).pick_folder() {
                    *value = p.display().to_string();
                }
            }
            if !value.trim().is_empty() && ui.button("Default").clicked() {
                value.clear();
            }
        });
    });
}

/// Segmented control. Options are given left-to-right; call inside a right-to-left layout is fine.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, v: &mut T, opts: &[(T, &str)]) {
    // Inside a right-aligned row items are laid out right to left; keep the reading order.
    let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
    let order: Vec<&(T, &str)> = if rtl { opts.iter().rev().collect() } else { opts.iter().collect() };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (o, l) in order {
            let on = *v == *o;
            let b = egui::Button::new(RichText::new(*l).color(if on { Color32::WHITE } else { TEXT })).fill(if on { ACCENT } else { Color32::from_rgb(0x2c, 0x2c, 0x34) });
            if ui.add(b).clicked() {
                *v = *o;
            }
        }
    });
}

fn text_field(ui: &mut egui::Ui, label: &str, v: &mut String, hint_text: &str, password: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::TextEdit::singleline(v).desired_width(300.0).password(password).hint_text(RichText::new(hint_text).color(FAINT)));
        });
    });
}

const HOTKEY_ROWS: [(&str, &str); 8] = [
    ("Snip", "Drag an area or click a window"),
    ("Full screen", "The monitor under the mouse"),
    ("Active window", ""),
    ("All monitors", ""),
    ("Grab text", "Copies what it reads"),
    ("Scrolling capture", ""),
    ("Record the screen", "Press again to stop"),
    ("Capture menu", "Choose what to capture each time"),
];

fn hotkey_mut(h: &mut Hotkeys, i: usize) -> &mut Hotkey {
    match i {
        0 => &mut h.snip,
        1 => &mut h.screen,
        2 => &mut h.window,
        3 => &mut h.all,
        4 => &mut h.text,
        5 => &mut h.scroll,
        6 => &mut h.record,
        _ => &mut h.menu,
    }
}

const TOKENS: [(&str, &str); 7] = [("{date}", "2026-11-24"), ("{time}", "153012"), ("{app}", "app name"), ("{title}", "window title"), ("{mode}", "Snip, Window…"), ("{w}", "width"), ("{h}", "height")];

impl Settings {
    fn shortcuts(&mut self, ui: &mut egui::Ui) {
        // Pick up a finished recording.
        if let Some(i) = self.recording {
            if !recorder::ACTIVE.load(Relaxed) {
                if let Some(res) = recorder::RESULT.lock().unwrap().take() {
                    match res {
                        Some(k) if k.vk == u32::MAX => {} // Esc: keep the old one
                        Some(k) => {
                            // A combo can only do one thing: unbind it elsewhere.
                            for j in 0..HOTKEY_ROWS.len() {
                                if j != i && *hotkey_mut(&mut self.cfg.hotkeys, j) == k {
                                    *hotkey_mut(&mut self.cfg.hotkeys, j) = Hotkey::default();
                                }
                            }
                            *hotkey_mut(&mut self.cfg.hotkeys, i) = k;
                        }
                        None => *hotkey_mut(&mut self.cfg.hotkeys, i) = Hotkey::default(),
                    }
                }
                self.recording = None;
            }
        }
        let ctx = ui.ctx().clone();
        for (i, (name, desc)) in HOTKEY_ROWS.iter().enumerate() {
            let k = *hotkey_mut(&mut self.cfg.hotkeys, i);
            let def = *hotkey_mut(&mut Hotkeys::default(), i);
            let waiting = self.recording == Some(i);
            let mut click = false;
            let mut reset = false;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(*name).color(TEXT));
                    if !desc.is_empty() {
                        ui.label(RichText::new(*desc).small().color(FAINT));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Fixed-width slot so the buttons line up whether or not Reset shows.
                    let (slot, _) = ui.allocate_exact_size(egui::vec2(48.0, 24.0), egui::Sense::hover());
                    if k != def {
                        let r = ui.put(slot, egui::Button::new(RichText::new("Reset").small()));
                        reset = r.on_hover_text(format!("Back to {}", def.label())).clicked();
                    }
                    let label = if waiting { "Press a key combination…".to_string() } else { k.label() };
                    let b = egui::Button::new(RichText::new(label).font(ui::bold(13.0)).color(if waiting { Color32::WHITE } else if k.is_set() { TEXT } else { FAINT }))
                        .fill(if waiting { ACCENT } else { Color32::from_rgb(0x2c, 0x2c, 0x34) })
                        .min_size(egui::vec2(230.0, 28.0));
                    click = ui.add(b).on_hover_text("Click, then press the keys. Esc cancels, Backspace clears.").clicked();
                });
            });
            if click {
                if waiting {
                    recorder::cancel();
                    self.recording = None;
                } else {
                    self.recording = Some(i);
                    recorder::start(&ctx);
                }
            }
            if reset {
                for j in 0..HOTKEY_ROWS.len() {
                    if j != i && def.is_set() && *hotkey_mut(&mut self.cfg.hotkeys, j) == def {
                        *hotkey_mut(&mut self.cfg.hotkeys, j) = Hotkey::default();
                    }
                }
                *hotkey_mut(&mut self.cfg.hotkeys, i) = def;
            }
        }
        ui.add_space(4.0);
        // Print Screen as a menu: a one-time choice instead of a key per capture type.
        let prtsc = Hotkey::prtsc(false, false, false, false);
        let mut menu_on = self.cfg.hotkeys.menu == prtsc;
        if toggle(ui, &mut menu_on, "Print Screen opens the capture menu", "Pick snip, window, text, recording… from a menu each time") {
            if menu_on {
                if self.cfg.hotkeys.snip == prtsc {
                    self.cfg.hotkeys.snip = Hotkey::default();
                }
                self.cfg.hotkeys.menu = prtsc;
            } else {
                self.cfg.hotkeys.menu = Hotkey::default();
                self.cfg.hotkeys.snip = prtsc;
            }
        }
        toggle(ui, &mut self.cfg.hook_win_shift_s, "Win+Shift+S snips too", "Replaces the Snipping Tool shortcut");
        hint(ui, "In the overlay: 1–5 or Tab switch mode · arrows nudge the cursor · Enter takes what's highlighted · Esc cancels");
        if self.snip_key == Some(true) {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Windows' own \"Print Screen opens Snipping Tool\" setting is on. Framegrab still gets the key first; you can turn the Windows one off if you like.").small().color(MUTED));
                if ui.small_button("Open Windows setting").clicked() {
                    let _ = std::process::Command::new("cmd").args(["/c", "start", "ms-settings:easeofaccess-keyboard"]).spawn();
                }
            });
        }
        ui.separator();
        toggle(ui, &mut self.cfg.pause_in_fullscreen, "Pause in full-screen games", "Hotkeys pass through to exclusive full-screen apps");
        let mut p = self.paused;
        if toggle(ui, &mut p, "Pause hotkeys now", "Until you switch it back or Framegrab restarts") {
            self.paused = p;
            crate::app::send_to_running(if p { "pause on" } else { "pause off" });
        }
    }

    fn after_capture(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        toggle(ui, &mut c.copy_to_clipboard, "Copy to clipboard", "");
        toggle(ui, &mut c.save_to_disk, "Save a file", "Off = clipboard only");
        if c.save_to_disk {
            row(ui, "Format", |ui| segmented(ui, &mut c.format, &[(ImgFormat::Png, "PNG"), (ImgFormat::Jpg, "JPG"), (ImgFormat::Webp, "WebP"), (ImgFormat::Avif, "AVIF")]));
            match c.format {
                ImgFormat::Jpg | ImgFormat::Avif => {
                    row(ui, "Quality", |ui| {
                        slider(ui, &mut c.quality, 40..=100, "%");
                    });
                    hint(ui, if c.format == ImgFormat::Avif { "AVIF: smallest files; takes a moment longer to save. Transparency is kept." } else { "JPG has no transparency; window corners become white." });
                }
                ImgFormat::Webp => hint(ui, "WebP is saved lossless (smaller than PNG, pixel-exact)."),
                ImgFormat::Png => hint(ui, "PNG: lossless, works everywhere."),
            }
            ui.add_space(4.0);
            text_field(ui, "File name", &mut c.name_template, "Screenshot {date} {time}", false);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (t, tip) in TOKENS {
                    if ui.small_button(t).on_hover_text(format!("Insert {t} ({tip})")).clicked() {
                        if !c.name_template.ends_with(' ') && !c.name_template.is_empty() {
                            c.name_template.push(' ');
                        }
                        c.name_template += t;
                    }
                }
            });
            toggle(ui, &mut c.app_subfolders, "Sort into a folder per app", "e.g. Screenshots\\Google Chrome\\…");
            let info = NameInfo { app: "Google Chrome".into(), title: "GitHub".into(), mode: "Snip".into(), w: 1920, h: 1080 };
            let ex = crate::config::template_path(std::path::Path::new(""), &c.name_template, &info, c.app_subfolders, c.format.ext());
            hint(ui, &format!("Example: {}", ex.display()));
            ui.separator();
            let d = Config { save_dir: String::new(), ..c.clone() }.shots_dir();
            folder_row(ui, "Screenshots folder", &mut c.save_dir, d);
            ui.separator();
        }
        toggle(ui, &mut c.show_toolbar, "Show the toolbar card", "Edit, read text, copy, drag out, upload, show in folder or delete");
        if c.show_toolbar {
            row(ui, "Hide the card after", |ui| segmented(ui, &mut c.toolbar_seconds, &[(15, "15 s"), (10, "10 s"), (6, "6 s"), (3, "3 s")]));
        }
        toggle(ui, &mut c.open_editor, "Open the editor right away", "Skip the card and annotate every capture");
        toggle(ui, &mut c.flash, "Screen flash", "A quick white flash where the capture was taken");
        if toggle(ui, &mut c.sound, "Shutter sound", "") && c.sound {
            crate::feedback::sound();
        }
    }

    fn windows_and_beautify(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        toggle(ui, &mut c.window_full_content, "Capture the whole window, even if covered", "Clicking a window grabs its own pixels, not what's on top of it");
        toggle(ui, &mut c.window_transparent, "Transparent rounded corners", "Saves the real window shape (PNG, WebP and AVIF keep transparency)");
        ui.separator();
        toggle(ui, &mut c.beautify, "Beautify every screenshot", "Adds a background, padding, rounded corners and a shadow. The editor has an on/off button too.");
        ui.add_space(4.0);
        row(ui, "Background", |ui| {
            for (i, (name, a, b)) in crate::process::BACKGROUNDS.iter().enumerate().rev() {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(30.0, 24.0), egui::Sense::click());
                let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
                let mut mesh = egui::Mesh::default();
                let r2 = r.shrink(2.0);
                mesh.colored_vertex(r2.left_top(), rgb(*a));
                mesh.colored_vertex(r2.right_top(), rgb(*a).lerp_to_gamma(rgb(*b), 0.5));
                mesh.colored_vertex(r2.right_bottom(), rgb(*b));
                mesh.colored_vertex(r2.left_bottom(), rgb(*a).lerp_to_gamma(rgb(*b), 0.5));
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                ui.painter().add(mesh);
                if c.beautify_bg as usize == i {
                    ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(2.0, TEXT), egui::StrokeKind::Inside);
                }
                if resp.on_hover_text(*name).clicked() {
                    c.beautify_bg = i as u8;
                }
            }
        });
        row(ui, "Padding", |ui| {
            slider(ui, &mut c.beautify_padding, 16..=160, " px");
        });
        row(ui, "Corner radius", |ui| {
            slider(ui, &mut c.beautify_radius, 0..=32, " px");
        });
        toggle(ui, &mut c.beautify_shadow, "Shadow", "");
        hint(ui, "Sizes are for a 1600 px capture and scale with bigger ones.");
    }

    fn text_grab(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        row(ui, "Language", |ui| {
            let cur = if c.ocr_language.is_empty() { "Windows default".to_string() } else { self.langs.iter().find(|l| l.0 == c.ocr_language).map(|l| l.1.clone()).unwrap_or(c.ocr_language.clone()) };
            egui::ComboBox::from_id_salt("lang").selected_text(cur).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut c.ocr_language, String::new(), "Windows default");
                for (tag, name) in &self.langs {
                    ui.selectable_value(&mut c.ocr_language, tag.clone(), name);
                }
            });
        });
        if self.langs.is_empty() {
            ui.label(RichText::new("No OCR languages installed. Add a language with text recognition in Windows Settings › Time & language.").small().color(MUTED));
        }
        row(ui, "Line breaks", |ui| segmented(ui, &mut c.text_layout, &[(TextLayout::Lines, "As on screen"), (TextLayout::Paragraphs, "Join paragraphs"), (TextLayout::SingleLine, "One line")]));
        toggle(ui, &mut c.detect_tables, "Detect tables", "Columns are copied with tabs, so they paste as cells in Excel or Sheets");
        toggle(ui, &mut c.read_qr, "Read QR codes", "A QR code in the area is copied as its text or link");
    }

    fn uploads(&mut self, ui: &mut egui::Ui) {
        let u = &mut self.cfg.upload;
        row(ui, "Upload to", |ui| segmented(ui, &mut u.provider, &[(UploadProvider::Off, "Off"), (UploadProvider::Imgur, "Imgur"), (UploadProvider::Custom, "Custom server")]));
        match u.provider {
            UploadProvider::Off => hint(ui, "Nothing leaves your PC. Turn on to get an Upload button on the card."),
            UploadProvider::Imgur => {
                text_field(ui, "Client ID", &mut u.imgur_client_id, "from api.imgur.com/oauth2/addclient", true);
                hint(ui, "Register an app at imgur.com (\"anonymous usage\") to get a Client ID. Uploads are public to anyone with the link.");
            }
            UploadProvider::Custom => {
                text_field(ui, "Upload URL", &mut u.custom_url, "https://example.com/upload", false);
                text_field(ui, "File field", &mut u.custom_field, "file", false);
                text_field(ui, "Header", &mut u.custom_header, "Authorization: Bearer …  (optional)", true);
                text_field(ui, "Link in response", &mut u.custom_response_path, "e.g. data.link — empty if the reply is the link", false);
                hint(ui, "Works with ShareX-style endpoints that accept a multipart file upload.");
            }
        }
        if u.provider != UploadProvider::Off {
            toggle(ui, &mut u.auto, "Upload every screenshot", "Otherwise only when you press Upload on the card");
            toggle(ui, &mut u.copy_link, "Copy the link when done", "Replaces the image on the clipboard with the URL");
        }
    }
}

impl eframe::App for Settings {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Test hook: FRAMEGRAB_SETTINGS_SHOT=<dir> screenshots the window and exits.
        if let Some(dir) = std::env::var_os("FRAMEGRAB_SETTINGS_SHOT") {
            self.frames += 1;
            let ctx = ui.ctx().clone();
            if self.frames == 8 {
                let slot = SHOT.clone();
                ctx.request_screenshot(move |img| *slot.lock().unwrap() = Some(img));
            }
            if let Some(img) = SHOT.lock().unwrap().take() {
                let im = crate::capture::Image::from_rgba(img.size[0] as u32, img.size[1] as u32, img.as_raw());
                let _ = crate::output::write_png(&std::path::Path::new(&dir).join(format!("settings-{}.png", std::env::var("FRAMEGRAB_SETTINGS_SCROLL").unwrap_or_default())), &im, true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ui::BG).inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 10 })).show(ui, |ui| {
            let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(y) = std::env::var("FRAMEGRAB_SETTINGS_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
                sa = sa.vertical_scroll_offset(y);
            }
            sa.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Framegrab").heading());
                    ui.label(RichText::new(format!("v{}", crate::updater::VERSION)).color(FAINT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Runs in the background · open Framegrab again to get here").small().color(FAINT));
                    });
                });
                ui.add_space(10.0);
                card(ui, "Shortcuts", |ui| self.shortcuts(ui));
                card(ui, "After a capture", |ui| self.after_capture(ui));
                card(ui, "Windows & beautify", |ui| self.windows_and_beautify(ui));
                card(ui, "Selection overlay", |ui| {
                    let c = &mut self.cfg;
                    toggle(ui, &mut c.magnifier, "Always show the magnifier", "Pixel zoom and colour picker next to the cursor. When off, press C while snipping to show it.");
                    toggle(ui, &mut c.crosshair, "Crosshair", "Faint guide lines through the cursor");
                    toggle(ui, &mut c.capture_cursor, "Include the mouse pointer in screenshots", "");
                });
                card(ui, "Screen recording", |ui| {
                    let c = &mut self.cfg;
                    row(ui, "Format", |ui| segmented(ui, &mut c.rec_format, &[(RecFormat::Mp4, "MP4"), (RecFormat::Gif, "GIF")]));
                    if c.rec_format == RecFormat::Mp4 {
                        row(ui, "Frame rate", |ui| segmented(ui, &mut c.rec_fps, &[(15, "15"), (30, "30"), (60, "60")]));
                        toggle(ui, &mut c.rec_system_audio, "Record system sound", "Remembered from the recording bar too");
                        toggle(ui, &mut c.rec_mic, "Record microphone", "");
                    } else {
                        row(ui, "GIF frame rate", |ui| segmented(ui, &mut c.gif_fps, &[(10, "10"), (15, "15"), (20, "20")]));
                        hint(ui, "GIFs are scaled to at most 960 px wide and have no sound.");
                    }
                    toggle(ui, &mut c.rec_cursor, "Show the mouse pointer", "");
                    ui.separator();
                    let d = Config { rec_dir: String::new(), ..c.clone() }.recordings_dir();
                    folder_row(ui, "Recordings folder", &mut c.rec_dir, d);
                });
                card(ui, "Text grab", |ui| self.text_grab(ui));
                card(ui, "Uploads", |ui| self.uploads(ui));
                card(ui, "General", |ui| {
                    let c = &mut self.cfg;
                    toggle(ui, &mut c.start_with_windows, "Start with Windows", "Runs quietly in the background — no tray icon");
                    toggle(ui, &mut c.auto_update, "Update automatically", "Checks GitHub releases; updates install while idle and are checksum-verified");
                    ui.horizontal(|ui| {
                        if ui.button("Check for updates").clicked() {
                            let st = crate::updater::check();
                            self.update = Some(match st {
                                crate::updater::State::UpToDate => "You're on the latest version.".into(),
                                crate::updater::State::Ready { version, .. } => format!("Version {version} downloaded; it installs the next time Framegrab is idle."),
                                crate::updater::State::Error(e) => e,
                                _ => String::new(),
                            });
                        }
                        if let Some(u) = &self.update {
                            ui.label(RichText::new(u).small().color(MUTED));
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Edit an image…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png"]).pick_file() {
                                crate::app::spawn_self(&["--edit", &p.to_string_lossy()]);
                            }
                        }
                        if ui.button("Quit Framegrab").on_hover_text("Stops the background app until you open it again or sign in").clicked() {
                            crate::app::send_to_running("quit");
                        }
                        ui.label(RichText::new("Settings save automatically.").small().color(FAINT));
                    });
                });
            });
        });

        if self.cfg.toolbar_seconds == 0 {
            self.cfg.toolbar_seconds = 6;
        }
        if serde_json::to_string(&self.cfg).ok() != serde_json::to_string(&self.saved).ok() {
            self.cfg.save();
            self.saved = self.cfg.clone();
            crate::app::send_to_running("reload");
        }
    }
}
