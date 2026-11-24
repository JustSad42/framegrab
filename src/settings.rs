//! Settings window (egui). Changes save immediately and the running copy reloads them.

use crate::config::{Config, RecFormat};
use crate::ui::{self, ACCENT, BORDER, CARD, FAINT, MUTED, TEXT};
use egui::{Color32, CornerRadius, RichText, Stroke};

struct Settings {
    cfg: Config,
    saved: Config,
    langs: Vec<(String, String)>,
    snip_key: Option<bool>,
    update: Option<String>,
    frames: u32,
}

static SHOT: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>> = std::sync::LazyLock::new(Default::default);

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Framegrab Settings").with_inner_size([620.0, 760.0]).with_min_inner_size([520.0, 480.0]).with_icon(ui::app_icon()),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Framegrab Settings",
        options,
        Box::new(|cc| {
            ui::install(&cc.egui_ctx);
            let cfg = Config::load();
            Ok(Box::new(Settings { saved: cfg.clone(), cfg, langs: crate::ocr::languages(), snip_key: snip_key_setting(), update: None, frames: 0 }))
        }),
    );
}

/// Windows' own "Print Screen opens Snipping Tool" switch (None = not set).
fn snip_key_setting() -> Option<bool> {
    let out = std::process::Command::new("reg").args(["query", r"HKCU\Control Panel\Keyboard", "/v", "PrintScreenKeyForSnippingEnabled"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return None;
    }
    Some(s.contains("0x1"))
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

fn toggle(ui: &mut egui::Ui, v: &mut bool, label: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).color(TEXT));
            if !hint.is_empty() {
                ui.label(RichText::new(hint).small().color(FAINT));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (r, resp) = ui.allocate_exact_size(egui::vec2(38.0, 22.0), egui::Sense::click());
            if resp.clicked() {
                *v = !*v;
            }
            let t = ui.ctx().animate_bool(resp.id, *v);
            let bg = if *v { ACCENT } else { Color32::from_rgb(0x3a, 0x3a, 0x44) };
            ui.painter().rect_filled(r, CornerRadius::same(11), bg);
            let x = egui::lerp(r.left() + 11.0..=r.right() - 11.0, t);
            ui.painter().circle_filled(egui::pos2(x, r.center().y), 8.0, Color32::WHITE);
        });
    });
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

fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, v: &mut T, opts: &[(T, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (o, l) in opts {
            let on = *v == *o;
            let b = egui::Button::new(RichText::new(*l).color(if on { Color32::WHITE } else { TEXT })).fill(if on { ACCENT } else { Color32::from_rgb(0x2c, 0x2c, 0x34) });
            if ui.add(b).clicked() {
                *v = *o;
            }
        }
    });
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
                let _ = crate::output::write_png(&std::path::Path::new(&dir).join("settings.png"), &im, true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ui::BG).inner_margin(egui::Margin { left: 22, right: 22, top: 18, bottom: 10 })).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Framegrab").heading());
                    ui.label(RichText::new(format!("v{}", crate::updater::VERSION)).color(FAINT));
                });
                ui.add_space(10.0);
                let c = &mut self.cfg;

                card(ui, "Shortcuts", |ui| {
                    let rows = [
                        ("Print Screen  ·  Win+Shift+S", "Snip: drag an area or click a window"),
                        ("Shift+Print Screen", "Capture the monitor under the mouse"),
                        ("Alt+Print Screen", "Capture the active window"),
                        ("Win+Print Screen", "Capture all monitors"),
                        ("Ctrl+Print Screen", "Grab text (copies what it reads)"),
                        ("Ctrl+Alt+Print Screen", "Scrolling capture"),
                        ("Ctrl+Shift+Print Screen", "Record the screen (press again to stop)"),
                    ];
                    egui::Grid::new("keys").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
                        for (k, d) in rows {
                            ui.label(RichText::new(k).font(ui::bold(13.0)));
                            ui.label(RichText::new(d).color(MUTED));
                            ui.end_row();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("In the overlay: 1–5 or Tab switch mode · arrows nudge the cursor · Enter takes what's highlighted · Esc cancels").small().color(FAINT));
                    ui.separator();
                    toggle(ui, &mut c.hook_prtsc, "Use Print Screen", "Framegrab handles the Print Screen key instead of Windows");
                    toggle(ui, &mut c.hook_win_shift_s, "Use Win+Shift+S", "Replaces the Snipping Tool shortcut");
                    if self.snip_key == Some(true) {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Windows' own \"Print Screen opens Snipping Tool\" setting is on. Framegrab still gets the key first; you can turn the Windows one off if you like.").small().color(MUTED));
                            if ui.small_button("Open Windows setting").clicked() {
                                let _ = std::process::Command::new("cmd").args(["/c", "start", "ms-settings:easeofaccess-keyboard"]).spawn();
                            }
                        });
                    }
                });

                card(ui, "After a capture", |ui| {
                    toggle(ui, &mut c.copy_to_clipboard, "Copy to clipboard", "");
                    toggle(ui, &mut c.save_to_disk, "Save automatically", "PNG named like Windows does: Screenshot 2026-11-24 153012.png");
                    toggle(ui, &mut c.show_toolbar, "Show the toolbar card", "Edit, read text, copy, show in folder or delete");
                    if c.show_toolbar {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Hide the card after").color(MUTED));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| segmented(ui, &mut c.toolbar_seconds, &[(15, "15 s"), (10, "10 s"), (6, "6 s"), (3, "3 s")]));
                        });
                    }
                    toggle(ui, &mut c.open_editor, "Open the editor right away", "Skip the card and annotate every capture");
                    ui.separator();
                    let d = Config { save_dir: String::new(), ..c.clone() }.shots_dir();
                    folder_row(ui, "Screenshots folder", &mut c.save_dir, d);
                });

                card(ui, "Selection overlay", |ui| {
                    toggle(ui, &mut c.magnifier, "Magnifier", "Pixel zoom with colour and position next to the cursor");
                    toggle(ui, &mut c.crosshair, "Crosshair", "Faint guide lines through the cursor");
                    toggle(ui, &mut c.capture_cursor, "Include the mouse pointer in screenshots", "");
                });

                card(ui, "Screen recording", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Format").color(TEXT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| segmented(ui, &mut c.rec_format, &[(RecFormat::Gif, "GIF"), (RecFormat::Mp4, "MP4")]));
                    });
                    if c.rec_format == RecFormat::Mp4 {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Frame rate").color(TEXT));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| segmented(ui, &mut c.rec_fps, &[(60, "60"), (30, "30"), (15, "15")]));
                        });
                        toggle(ui, &mut c.rec_system_audio, "Record system sound", "Can also be switched on the recording bar");
                        toggle(ui, &mut c.rec_mic, "Record microphone", "");
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("GIF frame rate").color(TEXT));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| segmented(ui, &mut c.gif_fps, &[(20, "20"), (15, "15"), (10, "10")]));
                        });
                        ui.label(RichText::new("GIFs are scaled to at most 960 px wide and have no sound.").small().color(FAINT));
                    }
                    toggle(ui, &mut c.rec_cursor, "Show the mouse pointer", "");
                    ui.separator();
                    let d = Config { rec_dir: String::new(), ..c.clone() }.recordings_dir();
                    folder_row(ui, "Recordings folder", &mut c.rec_dir, d);
                });

                card(ui, "Text grab", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Language").color(TEXT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let cur = if c.ocr_language.is_empty() {
                                "Windows default".to_string()
                            } else {
                                self.langs.iter().find(|l| l.0 == c.ocr_language).map(|l| l.1.clone()).unwrap_or(c.ocr_language.clone())
                            };
                            egui::ComboBox::from_id_salt("lang").selected_text(cur).width(220.0).show_ui(ui, |ui| {
                                ui.selectable_value(&mut c.ocr_language, String::new(), "Windows default");
                                for (tag, name) in &self.langs {
                                    ui.selectable_value(&mut c.ocr_language, tag.clone(), name);
                                }
                            });
                        });
                    });
                    if self.langs.is_empty() {
                        ui.label(RichText::new("No OCR languages installed. Add a language with text recognition in Windows Settings › Time & language.").small().color(MUTED));
                    }
                });

                card(ui, "General", |ui| {
                    toggle(ui, &mut c.start_with_windows, "Start with Windows", "Runs quietly in the tray");
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
                        if ui.button("Quit Framegrab").clicked() {
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
