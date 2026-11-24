//! Annotation editor (egui): arrows, boxes, ellipses, lines, pen, highlighter, text,
//! numbered steps, pixelate and crop, with undo/redo. Saves over the capture and copies it.

mod render;

use crate::capture::Image;
use crate::ui::{self, ACCENT, BORDER, CARD, FAINT, HI, MUTED, PANEL, TEXT};
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Marker,
    Text,
    Step,
    Blur,
    Spotlight,
    Callout,
    Curve,
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub id: u64,
    pub kind: Kind,
    pub a: Pos2,
    pub b: Pos2,
    pub pts: Vec<Pos2>,
    pub text: String,
    pub color: Color32,
    pub width: f32,
    pub n: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Draw(Kind),
    Crop,
    Pick,
}

const TOOLS: [(Tool, &str, &str, egui::Key); 15] = [
    (Tool::Select, "\u{E1DC}", "Select & move  (V)", egui::Key::V),
    (Tool::Draw(Kind::Arrow), "\u{E092}", "Arrow  (A)", egui::Key::A),
    (Tool::Draw(Kind::Curve), "curve", "Curved arrow  (U)", egui::Key::U),
    (Tool::Draw(Kind::Rect), "\u{E45E}", "Rectangle  (R)", egui::Key::R),
    (Tool::Draw(Kind::Ellipse), "\u{E18A}", "Ellipse  (E)", egui::Key::E),
    (Tool::Draw(Kind::Line), "\u{E32A}", "Line  (L)", egui::Key::L),
    (Tool::Draw(Kind::Pen), "\u{E3B4}", "Pen  (P)", egui::Key::P),
    (Tool::Draw(Kind::Marker), "\u{E6F0}", "Highlighter  (H)", egui::Key::H),
    (Tool::Draw(Kind::Text), "\u{E48A}", "Text  (T)", egui::Key::T),
    (Tool::Draw(Kind::Callout), "\u{E160}", "Callout bubble  (K) - drag from the point to where the bubble goes", egui::Key::K),
    (Tool::Draw(Kind::Step), "\u{E2F6}", "Numbered step  (N)", egui::Key::N),
    (Tool::Draw(Kind::Spotlight), "spot", "Spotlight  (S) - dims everything outside the boxes", egui::Key::S),
    (Tool::Draw(Kind::Blur), "\u{E296}", "Pixelate  (B)", egui::Key::B),
    (Tool::Crop, "\u{E1D4}", "Crop  (C)", egui::Key::C),
    (Tool::Pick, "\u{EAC4}", "Pick a colour from the image  (I)", egui::Key::I),
];

const COLORS: [Color32; 8] = [
    Color32::from_rgb(0xFF, 0x3B, 0x4E),
    Color32::from_rgb(0xFF, 0x8A, 0x1F),
    Color32::from_rgb(0xFF, 0xD2, 0x1F),
    Color32::from_rgb(0x2E, 0xCC, 0x71),
    Color32::from_rgb(0x3D, 0x8B, 0xFF),
    Color32::from_rgb(0xA8, 0x5C, 0xFF),
    Color32::from_rgb(0xFF, 0xFF, 0xFF),
    Color32::from_rgb(0x11, 0x11, 0x11),
];

const WIDTHS: [f32; 3] = [3.0, 5.0, 9.0];

pub fn text_px(w: f32) -> f32 {
    if w <= 3.0 {
        20.0
    } else if w <= 5.0 {
        28.0
    } else {
        44.0
    }
}

pub fn step_radius(w: f32) -> f32 {
    if w <= 3.0 {
        13.0
    } else if w <= 5.0 {
        17.0
    } else {
        24.0
    }
}

/// Black or white, whichever reads better on `c`.
pub fn contrast(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 160.0 { Color32::from_rgb(0x11, 0x11, 0x11) } else { Color32::WHITE }
}

fn marker_color(c: Color32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 110)
}

type Snapshot = (Vec<Shape>, Option<Rect>);

static DEMO_SHOT: std::sync::LazyLock<std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>> = std::sync::LazyLock::new(Default::default);

struct Editor {
    path: PathBuf,
    /// Where Save writes (format from its extension); defaults to `path`.
    out: PathBuf,
    cfg: crate::config::Config,
    beautify: bool,
    custom: Option<Color32>,
    base: Image,
    tiles: Vec<(Rect, egui::TextureHandle)>,
    shapes: Vec<Shape>,
    crop: Option<Rect>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    tool: Tool,
    color: Color32,
    width: f32,
    zoom: f32,
    offset: Vec2,
    fitted: bool,
    drawing: Option<Shape>,
    crop_drag: Option<Pos2>,
    selected: Option<usize>,
    moving: Option<Pos2>,
    moved: bool,
    editing: Option<usize>,
    focus_edit: bool,
    blur_tex: HashMap<u64, ([i32; 4], egui::TextureHandle)>,
    next_id: u64,
    dirty: bool,
    status: Option<(String, f64)>,
    font: Option<Vec<u8>>,
    ask_close: bool,
    closing: bool,
    /// Test hook: FRAMEGRAB_EDITOR_DEMO=<dir> adds sample shapes, screenshots the window, exits.
    demo: Option<(PathBuf, u32)>,
}

pub fn run(path: PathBuf, out: Option<PathBuf>) {
    let base = match crate::output::read_png(&path) {
        Ok(i) => i,
        Err(e) => {
            rfd::MessageDialog::new().set_title("Framegrab").set_description(format!("Couldn't open {}:\n{e}", path.display())).show();
            return;
        }
    };
    // Size the window around the image on the monitor with the cursor.
    let mons = crate::capture::monitors();
    let (cx, cy) = crate::capture::cursor_pos();
    let m = mons.iter().find(|m| m.rect.contains(cx, cy)).or(mons.first()).cloned();
    let (ww, wh) = match &m {
        Some(m) => {
            let s = m.scale();
            let maxw = m.work.w as f32 / s * 0.9;
            let maxh = m.work.h as f32 / s * 0.9;
            ((base.w as f32 / s + 48.0).max(1340.0).min(maxw.max(1340.0)), (base.h as f32 / s + 120.0).clamp(560.0, maxh.max(560.0)))
        }
        None => (1340.0, 800.0),
    };
    let title = format!("{} — Framegrab", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(title).with_inner_size([ww, wh]).with_min_inner_size([1300.0, 480.0]).with_icon(ui::app_icon()),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Framegrab Editor",
        options,
        Box::new(move |cc| {
            ui::install(&cc.egui_ctx);
            Ok(Box::new(Editor::new(path, out, base)))
        }),
    );
}

impl Editor {
    fn new(path: PathBuf, out: Option<PathBuf>, base: Image) -> Self {
        let cfg = crate::config::Config::load();
        Self {
            out: out.unwrap_or_else(|| path.clone()),
            beautify: cfg.beautify,
            cfg,
            custom: None,
            path,
            base,
            tiles: Vec::new(),
            shapes: Vec::new(),
            crop: None,
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Draw(Kind::Arrow),
            color: COLORS[0],
            width: WIDTHS[1],
            zoom: 1.0,
            offset: Vec2::ZERO,
            fitted: false,
            drawing: None,
            crop_drag: None,
            selected: None,
            moving: None,
            moved: false,
            editing: None,
            focus_edit: false,
            blur_tex: HashMap::new(),
            next_id: 1,
            dirty: false,
            status: None,
            font: render::annot_font(),
            ask_close: false,
            closing: false,
            demo: std::env::var_os("FRAMEGRAB_EDITOR_DEMO").map(|d| (PathBuf::from(d), 0)),
        }
    }

    fn demo_shapes(&mut self) {
        let (w, h) = (self.base.w as f32, self.base.h as f32);
        let mk = |e: &mut Self, k: Kind, a: Pos2, b: Pos2, c: Color32| {
            let mut s = e.new_shape(k, a);
            s.b = b;
            s.color = c;
            s
        };
        let s1 = mk(self, Kind::Rect, pos2(w * 0.1, h * 0.1), pos2(w * 0.45, h * 0.4), COLORS[0]);
        let s2 = mk(self, Kind::Arrow, pos2(w * 0.7, h * 0.7), pos2(w * 0.47, h * 0.42), COLORS[0]);
        let s3 = mk(self, Kind::Blur, pos2(w * 0.55, h * 0.1), pos2(w * 0.9, h * 0.3), COLORS[0]);
        let mut s4 = mk(self, Kind::Text, pos2(w * 0.1, h * 0.75), pos2(w * 0.1, h * 0.75), COLORS[2]);
        s4.text = "Framegrab editor".into();
        let s5 = mk(self, Kind::Step, pos2(w * 0.08, h * 0.08), pos2(0.0, 0.0), COLORS[4]);
        let mut s6 = mk(self, Kind::Marker, pos2(w * 0.5, h * 0.55), pos2(w * 0.85, h * 0.55), COLORS[2]);
        s6.pts = vec![pos2(w * 0.5, h * 0.55), pos2(w * 0.85, h * 0.55)];
        let s7 = mk(self, Kind::Ellipse, pos2(w * 0.6, h * 0.62), pos2(w * 0.8, h * 0.9), COLORS[3]);
        let s8 = mk(self, Kind::Curve, pos2(w * 0.15, h * 0.55), pos2(w * 0.4, h * 0.7), COLORS[1]);
        let mut s9 = mk(self, Kind::Callout, pos2(w * 0.3, h * 0.25), pos2(w * 0.32, h * 0.52), COLORS[4]);
        s9.text = "Look here".into();
        let s10 = mk(self, Kind::Spotlight, pos2(w * 0.08, h * 0.06), pos2(w * 0.5, h * 0.95), COLORS[0]);
        self.shapes = vec![s10, s1, s2, s3, s4, s5, s6, s7, s8, s9];
        self.selected = Some(7);
    }

    fn snapshot(&mut self) {
        self.undo.push((self.shapes.clone(), self.crop));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn do_undo(&mut self) {
        self.finish_edit();
        if let Some(s) = self.undo.pop() {
            self.redo.push((std::mem::take(&mut self.shapes), self.crop));
            (self.shapes, self.crop) = s;
            self.selected = None;
            self.dirty = true;
        }
    }

    fn do_redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push((std::mem::take(&mut self.shapes), self.crop));
            (self.shapes, self.crop) = s;
            self.selected = None;
            self.dirty = true;
        }
    }

    fn new_shape(&mut self, kind: Kind, p: Pos2) -> Shape {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.shapes.iter().filter(|s| s.kind == Kind::Step).count() as u32 + 1;
        Shape { id, kind, a: p, b: p, pts: vec![p], text: String::new(), color: self.color, width: self.width, n }
    }

    fn bounds(&self, s: &Shape) -> Rect {
        match s.kind {
            Kind::Pen | Kind::Marker => {
                let mut r = Rect::NOTHING;
                for p in &s.pts {
                    r.extend_with(*p);
                }
                r.expand(if s.kind == Kind::Marker { s.width * 1.8 } else { s.width })
            }
            Kind::Step => Rect::from_center_size(s.a, Vec2::splat(step_radius(s.width) * 2.0)),
            Kind::Text => {
                let (w, h) = self.text_dims(&s.text, text_px(s.width));
                Rect::from_min_size(s.a, vec2(w.max(8.0), h))
            }
            Kind::Callout => {
                let (bubble, _) = render::callout_geom(s, self.text_dims(&s.text, render::callout_px(s.width)));
                let mut r = bubble;
                r.extend_with(s.a);
                r
            }
            Kind::Curve => {
                let mut r = Rect::from_two_pos(s.a, s.b);
                r.extend_with(render::curve_ctrl(s.a, s.b));
                r.expand(s.width * 3.0)
            }
            _ => Rect::from_two_pos(s.a, s.b).expand(s.width),
        }
    }

    fn text_dims(&self, text: &str, px: f32) -> (f32, f32) {
        self.font
            .as_ref()
            .and_then(|b| ab_glyph::FontRef::try_from_slice(b).ok())
            .map(|f| render::text_size(&f, if text.is_empty() { " " } else { text }, px))
            .unwrap_or((px * text.len().max(1) as f32 * 0.5, px * 1.3))
    }

    fn hit(&self, p: Pos2, slop: f32) -> Option<usize> {
        (0..self.shapes.len()).rev().find(|&i| {
            let s = &self.shapes[i];
            let b = self.bounds(s).expand(slop);
            if !b.contains(p) {
                return false;
            }
            match s.kind {
                Kind::Line | Kind::Arrow => dist_seg(p, s.a, s.b) <= s.width + slop,
                Kind::Curve => render::curve_pts(s.a, s.b, 24).windows(2).any(|w| dist_seg(p, w[0], w[1]) <= s.width + slop),
                Kind::Callout => {
                    let (bubble, tri) = render::callout_geom(s, self.text_dims(&s.text, render::callout_px(s.width)));
                    bubble.contains(p) || dist_seg(p, tri[0], tri[1]) <= slop * 2.0 || dist_seg(p, tri[0], tri[2]) <= slop * 2.0
                }
                Kind::Pen | Kind::Marker => s.pts.windows(2).any(|w| dist_seg(p, w[0], w[1]) <= s.width * 2.0 + slop),
                Kind::Rect | Kind::Ellipse | Kind::Spotlight => {
                    let r = Rect::from_two_pos(s.a, s.b);
                    let inner = r.shrink(s.width + slop);
                    !inner.contains(p) || r.width() < 3.0 * slop || r.height() < 3.0 * slop
                }
                _ => true,
            }
        })
    }

    fn flatten(&self) -> Image {
        render::flatten(&self.base, &self.shapes, self.crop, self.font.as_deref())
    }

    /// What Save/Copy produce: the flattened image, beautified if that's on.
    fn export(&self) -> Image {
        let img = self.flatten();
        if self.beautify { crate::process::beautify(&img, &crate::process::Beautify::from(&self.cfg)) } else { img }
    }

    fn write(&self, path: &std::path::Path, img: &Image) -> Result<(), String> {
        crate::output::write_image(path, img, format_of(path), self.cfg.quality).map(|_| ())
    }

    fn copy(&mut self, now: f64) {
        self.finish_edit();
        let img = self.export();
        crate::output::copy_image(&img);
        self.status = Some(("Copied to clipboard".into(), now));
    }

    fn save(&mut self, now: f64) -> bool {
        self.finish_edit();
        let img = self.export();
        let out = self.out.clone();
        match self.write(&out, &img) {
            Ok(()) => {
                crate::output::copy_image(&img);
                self.dirty = false;
                self.status = Some(("Saved and copied".into(), now));
                true
            }
            Err(e) => {
                self.status = Some((format!("Save failed: {e}"), now));
                false
            }
        }
    }

    fn save_as(&mut self, now: f64) {
        self.finish_edit();
        let dir = self.out.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let name = self.out.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "Screenshot.png".into());
        let ext = self.out.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
        let mut dlg = rfd::FileDialog::new().set_directory(dir).set_file_name(name);
        // The current format first so it's the default filter.
        let mut filters = vec![("PNG image", "png"), ("JPEG image", "jpg"), ("WebP image", "webp"), ("AVIF image", "avif")];
        if let Some(i) = filters.iter().position(|f| f.1 == ext) {
            let f = filters.remove(i);
            filters.insert(0, f);
        }
        for (n, e) in &filters {
            dlg = dlg.add_filter(*n, &[*e]);
        }
        if let Some(p) = dlg.save_file() {
            let p = if p.extension().is_none() { p.with_extension(&ext) } else { p };
            let img = self.export();
            match self.write(&p, &img) {
                Ok(()) => {
                    self.out = p;
                    self.dirty = false;
                    self.status = Some(("Saved".into(), now));
                }
                Err(e) => self.status = Some((format!("Save failed: {e}"), now)),
            }
        }
    }

    /// Ends text editing; removes the text if it is empty.
    fn finish_edit(&mut self) {
        if let Some(i) = self.editing.take() {
            if i < self.shapes.len() && self.shapes[i].text.trim().is_empty() {
                self.shapes.remove(i);
                self.selected = None;
            }
        }
    }

    fn upload(&mut self, ctx: &egui::Context) {
        if !self.tiles.is_empty() {
            return;
        }
        let opts = egui::TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, mipmap_mode: Some(egui::TextureFilter::Linear), ..Default::default() };
        let t = 4096u32;
        let mut y = 0;
        while y < self.base.h {
            let mut x = 0;
            let th = t.min(self.base.h - y);
            while x < self.base.w {
                let tw = t.min(self.base.w - x);
                let part = self.base.crop(x as i32, y as i32, tw as i32, th as i32);
                let ci = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &part.to_rgba());
                let tex = ctx.load_texture(format!("tile{x}_{y}"), ci, opts);
                self.tiles.push((Rect::from_min_size(pos2(x as f32, y as f32), vec2(tw as f32, th as f32)), tex));
                x += t;
            }
            y += t;
        }
    }

    fn blur_texture(&mut self, ctx: &egui::Context, s: &Shape) -> Option<egui::TextureId> {
        let r = Rect::from_two_pos(s.a, s.b);
        let key = [r.min.x as i32, r.min.y as i32, r.max.x as i32, r.max.y as i32];
        let (x0, y0) = (key[0].clamp(0, self.base.w as i32), key[1].clamp(0, self.base.h as i32));
        let (x1, y1) = (key[2].clamp(0, self.base.w as i32), key[3].clamp(0, self.base.h as i32));
        if x1 - x0 < 1 || y1 - y0 < 1 {
            return None;
        }
        if let Some((k, t)) = self.blur_tex.get(&s.id) {
            if *k == key {
                return Some(t.id());
            }
        }
        // Pixelate in base coordinates so the preview matches the export block grid.
        let mut tmp = self.base.crop(key[0].min(x0), key[1].min(y0), x1 - x0, y1 - y0);
        let (tw, th) = (tmp.w as i32, tmp.h as i32);
        render::pixelate(&mut tmp, 0, 0, tw, th);
        let ci = egui::ColorImage::from_rgba_unmultiplied([tmp.w as usize, tmp.h as usize], &tmp.to_rgba());
        let tex = ctx.load_texture(format!("blur{}", s.id), ci, egui::TextureOptions::NEAREST);
        let id = tex.id();
        self.blur_tex.insert(s.id, (key, tex));
        Some(id)
    }

    fn fit(&mut self, area: Rect) {
        let z = ((area.width() - 40.0) / self.base.w as f32).min((area.height() - 40.0) / self.base.h as f32).min(1.0).max(0.02);
        self.zoom = z;
        self.offset = (area.size() - vec2(self.base.w as f32, self.base.h as f32) * z) / 2.0;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (t, ic, tip, _) in TOOLS {
                if icon_button(ui, ic, self.tool == t, tip).clicked() {
                    self.finish_edit();
                    self.tool = t;
                    self.selected = None;
                }
            }
            separator(ui);
            for c in COLORS {
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 30.0), Sense::click());
                let active = self.color == c;
                let center = r.center();
                if active {
                    ui.painter().circle_stroke(center, 10.5, Stroke::new(2.0, TEXT));
                } else if resp.hovered() {
                    ui.painter().circle_stroke(center, 10.5, Stroke::new(1.5, MUTED));
                }
                ui.painter().circle_filled(center, 7.5, c);
                if c == COLORS[7] {
                    ui.painter().circle_stroke(center, 7.5, Stroke::new(1.0, Color32::from_gray(90)));
                }
                if resp.clicked() {
                    self.set_color(c);
                }
            }
            if let Some(c) = self.custom {
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 30.0), Sense::click());
                let center = r.center();
                if self.color == c {
                    ui.painter().circle_stroke(center, 10.5, Stroke::new(2.0, TEXT));
                }
                ui.painter().circle_filled(center, 7.5, c);
                ui.painter().circle_stroke(center, 7.5, Stroke::new(1.0, Color32::from_gray(90)));
                if resp.on_hover_text(format!("Picked colour #{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())).clicked() {
                    self.set_color(c);
                }
            }
            separator(ui);
            for (k, w) in WIDTHS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(28.0, 30.0), Sense::click());
                let active = self.width == *w;
                if active || resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(7), if active { HI } else { CARD });
                }
                ui.painter().circle_filled(r.center(), 2.5 + k as f32 * 2.0, if active { TEXT } else { MUTED });
                let resp = resp.on_hover_text(["Thin", "Medium", "Thick"][k]);
                if resp.clicked() {
                    self.width = *w;
                    if let Some(i) = self.selected.or(self.editing) {
                        if i < self.shapes.len() && self.shapes[i].width != *w {
                            self.snapshot();
                            self.shapes[i].width = *w;
                        }
                    }
                }
            }
            separator(ui);
            if icon_button(ui, "\u{E038}", false, "Undo  (Ctrl+Z)").clicked() {
                self.do_undo();
            }
            if icon_button(ui, "\u{E036}", false, "Redo  (Ctrl+Y)").clicked() {
                self.do_redo();
            }
            separator(ui);
            let tip = if self.beautify { "Beautify is on: background, padding, rounded corners and shadow are added when saving  (F)\nStyle and default: Settings" } else { "Beautify: add a background, padding, rounded corners and shadow  (F)" };
            if icon_button(ui, "\u{E6A2}", self.beautify, tip).clicked() {
                self.beautify = !self.beautify;
                self.dirty = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if primary_button(ui, "\u{E182}", "Done").on_hover_text("Save, copy and close  (Enter)").clicked() && self.save(now) {
                    self.closing = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if text_button(ui, "\u{E248}", "Save").on_hover_text("Save over the capture and copy  (Ctrl+S)\nRight-click: Save as…").clicked() {
                    self.save(now);
                }
                if text_button(ui, "\u{E1CA}", "Copy").on_hover_text("Copy to clipboard  (Ctrl+C)").clicked() {
                    self.copy(now);
                }
            });
        });
    }

    fn set_color(&mut self, c: Color32) {
        self.color = c;
        if let Some(i) = self.selected.or(self.editing) {
            if i < self.shapes.len() && self.shapes[i].color != c {
                self.snapshot();
                self.shapes[i].color = c;
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context, now: f64) {
        let typing = self.editing.is_some() || ctx.memory(|m| m.focused().is_some());
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if cmd && pressed(egui::Key::Z) {
            if shift { self.do_redo() } else { self.do_undo() }
        }
        if cmd && pressed(egui::Key::Y) {
            self.do_redo();
        }
        if cmd && pressed(egui::Key::S) {
            if shift { self.save_as(now) } else { self.save(now); }
        }
        if typing {
            return;
        }
        if cmd && pressed(egui::Key::C) {
            self.copy(now);
        }
        // egui turns Ctrl+C into a Copy event rather than a key press.
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            self.copy(now);
        }
        if !cmd {
            for (t, _, _, k) in TOOLS {
                if pressed(k) {
                    self.tool = t;
                    self.selected = None;
                }
            }
        }
        if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            if let Some(i) = self.selected.take() {
                if i < self.shapes.len() {
                    self.snapshot();
                    self.shapes.remove(i);
                }
            }
        }
        if pressed(egui::Key::Escape) {
            self.selected = None;
            self.drawing = None;
        }
        if pressed(egui::Key::Enter) && self.save(now) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if pressed(egui::Key::Num0) {
            self.fitted = false;
        }
        if !cmd && pressed(egui::Key::F) {
            self.beautify = !self.beautify;
            self.dirty = true;
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let area = ui.available_rect_before_wrap();
        if !self.fitted && area.width() > 50.0 {
            self.fit(area);
            self.fitted = true;
        }
        self.upload(&ctx);
        let resp = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgb(0x10, 0x10, 0x13));

        // Zoom around the pointer (wheel or pinch / Ctrl+wheel), pan with middle or Space+drag.
        if let Some(hp) = resp.hover_pos() {
            let (scroll, zd) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = if (zd - 1.0).abs() > 0.0001 { zd } else { (scroll * 0.0018).exp() };
            if (factor - 1.0).abs() > 0.0001 {
                let nz = (self.zoom * factor).clamp(0.02, 32.0);
                let f = nz / self.zoom;
                let origin = area.min + self.offset;
                let new_origin = hp - (hp - origin) * f;
                self.offset = new_origin - area.min;
                self.zoom = nz;
            }
        }
        let space = ui.input(|i| i.key_down(egui::Key::Space)) && self.editing.is_none();
        if resp.dragged_by(egui::PointerButton::Middle) || (space && resp.dragged_by(egui::PointerButton::Primary)) {
            self.offset += resp.drag_delta();
        }
        let z = self.zoom;
        let origin = area.min + self.offset;
        let to_s = |p: Pos2| origin + p.to_vec2() * z;
        let to_i = |p: Pos2| ((p - origin) / z).to_pos2();
        let img_rect = Rect::from_min_size(origin, vec2(self.base.w as f32, self.base.h as f32) * z);

        // Beautify preview: the gradient frame around the (cropped) image.
        if self.beautify {
            let c = self.crop.unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(self.base.w as f32, self.base.h as f32)));
            let pad = self.cfg.beautify_padding as f32 * crate::process::Beautify::scale(c.width() as u32, c.height() as u32);
            let fr = Rect::from_min_max(to_s(c.min - Vec2::splat(pad)), to_s(c.max + Vec2::splat(pad)));
            let (_, c0, c1) = crate::process::BACKGROUNDS[(self.cfg.beautify_bg as usize).min(crate::process::BACKGROUNDS.len() - 1)];
            let rgb = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
            let (a, b) = (rgb(c0), rgb(c1));
            let mid = a.lerp_to_gamma(b, 0.5);
            let mut mesh = egui::Mesh::default();
            mesh.colored_vertex(fr.left_top(), a);
            mesh.colored_vertex(fr.right_top(), mid);
            mesh.colored_vertex(fr.right_bottom(), b);
            mesh.colored_vertex(fr.left_bottom(), mid);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(mesh);
        }
        // Image + soft shadow.
        painter.rect_filled(img_rect.expand(1.0), CornerRadius::ZERO, Color32::from_black_alpha(140));
        for (r, t) in &self.tiles {
            painter.image(t.id(), Rect::from_min_max(to_s(r.min), to_s(r.max)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // Input → shapes.
        let shift = ui.input(|i| i.modifiers.shift);
        if !space {
            self.interact(&resp, &to_i, shift, &ctx);
        }

        // Spotlight: dim everything outside the union of the spotlight boxes.
        let mut spots: Vec<Rect> = self.shapes.iter().filter(|s| s.kind == Kind::Spotlight).map(|s| Rect::from_two_pos(s.a, s.b)).collect();
        if let Some(d) = self.drawing.as_ref().filter(|d| d.kind == Kind::Spotlight) {
            spots.push(Rect::from_two_pos(d.a, d.b));
        }
        if !spots.is_empty() {
            let full = Rect::from_min_size(Pos2::ZERO, vec2(self.base.w as f32, self.base.h as f32));
            for r in render::outside_cells(full, &spots) {
                painter.rect_filled(Rect::from_min_max(to_s(r.min), to_s(r.max)), CornerRadius::ZERO, Color32::from_black_alpha(render::SPOT_DIM));
            }
        }

        // Shapes.
        let shapes = self.shapes.clone();
        for (i, s) in shapes.iter().enumerate() {
            if self.editing == Some(i) && s.kind == Kind::Text {
                continue;
            }
            self.draw_shape(&painter, s, &to_s, z, &ctx);
        }
        if let Some(s) = self.drawing.clone() {
            self.draw_shape(&painter, &s, &to_s, z, &ctx);
        }
        if let Some(i) = self.selected {
            if let Some(s) = self.shapes.get(i) {
                let b = self.bounds(s);
                let r = Rect::from_min_max(to_s(b.min), to_s(b.max)).expand(4.0);
                painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.5, ACCENT), StrokeKind::Outside);
            }
        }

        // Crop overlay.
        let crop = match (self.crop_drag, resp.interact_pointer_pos()) {
            (Some(a), Some(p)) if self.tool == Tool::Crop => Some(Rect::from_two_pos(a, clamp_to(to_i(p), &self.base))),
            _ => self.crop,
        };
        if let Some(c) = crop {
            let cr = Rect::from_min_max(to_s(c.min), to_s(c.max));
            let dim = Color32::from_black_alpha(165);
            for r in [
                Rect::from_min_max(img_rect.min, pos2(img_rect.max.x, cr.min.y)),
                Rect::from_min_max(pos2(img_rect.min.x, cr.max.y), img_rect.max),
                Rect::from_min_max(pos2(img_rect.min.x, cr.min.y), pos2(cr.min.x, cr.max.y)),
                Rect::from_min_max(pos2(cr.max.x, cr.min.y), pos2(img_rect.max.x, cr.max.y)),
            ] {
                if r.width() > 0.0 && r.height() > 0.0 {
                    painter.rect_filled(r, CornerRadius::ZERO, dim);
                }
            }
            painter.rect_stroke(cr, CornerRadius::ZERO, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
            let label = format!("{} × {}", c.width().round(), c.height().round());
            painter.text(cr.left_bottom() + vec2(0.0, 8.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(12.0), MUTED);
        }

        // Text editing box.
        if let Some(i) = self.editing {
            if i < self.shapes.len() {
                let s = self.shapes[i].clone();
                let callout = s.kind == Kind::Callout;
                let (pos, size, color) = if callout {
                    let px = render::callout_px(s.width);
                    let (bubble, _) = render::callout_geom(&s, self.text_dims(&s.text, px));
                    (to_s(bubble.min + Vec2::splat(render::callout_pad(px))), px * z, contrast(s.color))
                } else {
                    (to_s(s.a), text_px(s.width) * z, s.color)
                };
                let mut text = s.text.clone();
                let mut done = false;
                egui::Area::new(egui::Id::new(("textedit", s.id))).fixed_pos(pos - vec2(4.0, 2.0)).order(egui::Order::Foreground).show(&ctx, |ui| {
                    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = true;
                    }
                    egui::Frame::new().stroke(Stroke::new(1.0, if callout { Color32::TRANSPARENT } else { ACCENT.gamma_multiply(0.8) })).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(3, 1)).show(ui, |ui| {
                        let te = egui::TextEdit::multiline(&mut text)
                            .font(egui::FontId::new(size.max(6.0), egui::FontFamily::Name("annot".into())))
                            .text_color(color)
                            .frame(egui::Frame::NONE)
                            .desired_rows(1)
                            .desired_width(if callout { (self.text_dims(&s.text, render::callout_px(s.width)).0 * z + size).max(size * 3.0) } else { (self.bounds(&s).width() * z + size).max(size * 3.0) })
                            .hint_text(egui::RichText::new("Type…").color(FAINT));
                        let r = ui.add(te);
                        if self.focus_edit {
                            r.request_focus();
                            self.focus_edit = false;
                        }
                    });
                });
                if text != self.shapes[i].text {
                    self.shapes[i].text = text;
                    self.dirty = true;
                }
                if done {
                    self.finish_edit();
                }
            }
        }

        // Cursor.
        if resp.hovered() {
            let icon = if space || resp.dragged_by(egui::PointerButton::Middle) {
                egui::CursorIcon::Grabbing
            } else {
                match self.tool {
                    Tool::Select => {
                        if resp.hover_pos().and_then(|p| self.hit(to_i(p), 4.0 / z)).is_some() { egui::CursorIcon::Move } else { egui::CursorIcon::Default }
                    }
                    Tool::Draw(Kind::Text) => egui::CursorIcon::Text,
                    Tool::Pick => egui::CursorIcon::Cell,
                    _ => egui::CursorIcon::Crosshair,
                }
            };
            ctx.set_cursor_icon(icon);
        }
    }

    fn interact(&mut self, resp: &egui::Response, to_i: &dyn Fn(Pos2) -> Pos2, shift: bool, _ctx: &egui::Context) {
        let slop = 5.0 / self.zoom;
        let pp = resp.interact_pointer_pos().map(to_i);
        if resp.drag_started_by(egui::PointerButton::Primary) {
            // Drag start is reported a few pixels late; use the press origin.
            let start = resp.ctx.input(|i| i.pointer.press_origin()).map(to_i).or(pp);
            if let Some(p) = start {
                self.finish_edit();
                match self.tool {
                    Tool::Select => {
                        self.selected = self.hit(p, slop);
                        if self.selected.is_some() {
                            self.snapshot();
                            self.moving = Some(p);
                            self.moved = false;
                        }
                    }
                    Tool::Crop => self.crop_drag = Some(clamp_to(p, &self.base)),
                    Tool::Pick => self.pick(p),
                    Tool::Draw(Kind::Text) | Tool::Draw(Kind::Step) => {}
                    Tool::Draw(k) => {
                        let s = self.new_shape(k, p);
                        self.drawing = Some(s);
                    }
                }
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            if let (Some(p), Tool::Pick) = (pp, self.tool) {
                self.pick(p);
            }
            if let Some(p) = pp {
                if let (Some(last), Some(i)) = (self.moving, self.selected) {
                    let d = p - last;
                    if let Some(s) = self.shapes.get_mut(i) {
                        s.a += d;
                        s.b += d;
                        for q in &mut s.pts {
                            *q += d;
                        }
                    }
                    self.moving = Some(p);
                    self.moved = true;
                }
                if let Some(s) = &mut self.drawing {
                    match s.kind {
                        Kind::Pen | Kind::Marker => {
                            let mut q = p;
                            // Highlighter with Shift draws straight.
                            if shift && s.kind == Kind::Marker {
                                q = pos2(p.x, s.pts[0].y);
                                s.pts.truncate(1);
                            }
                            if s.pts.last().is_none_or(|l| l.distance(q) > 0.75 / self.zoom.max(0.1)) {
                                s.pts.push(q);
                            }
                            s.b = q;
                        }
                        Kind::Line | Kind::Arrow | Kind::Curve if shift => {
                            let d = p - s.a;
                            let ang = (d.y.atan2(d.x) / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
                            s.b = s.a + vec2(ang.cos(), ang.sin()) * d.length();
                        }
                        Kind::Rect | Kind::Ellipse | Kind::Blur | Kind::Spotlight if shift => {
                            let d = p - s.a;
                            let m = d.x.abs().max(d.y.abs());
                            s.b = s.a + vec2(m * d.x.signum(), m * d.y.signum());
                        }
                        _ => s.b = p,
                    }
                }
            }
        }
        if resp.drag_stopped() {
            if self.moving.take().is_some() && !self.moved {
                self.undo.pop();
            }
            if let Some(a) = self.crop_drag.take() {
                if let Some(p) = pp {
                    let r = Rect::from_two_pos(a, clamp_to(p, &self.base));
                    self.snapshot();
                    self.crop = (r.width() > 4.0 && r.height() > 4.0).then_some(r);
                }
            }
            if let Some(s) = self.drawing.take() {
                let big = match s.kind {
                    Kind::Pen | Kind::Marker => s.pts.len() >= 2,
                    _ => s.a.distance(s.b) >= 3.0 / self.zoom.max(0.1),
                };
                if big {
                    self.snapshot();
                    let callout = s.kind == Kind::Callout;
                    self.shapes.push(s);
                    if callout {
                        self.editing = Some(self.shapes.len() - 1);
                        self.focus_edit = true;
                    }
                }
            }
        }
        if resp.clicked_by(egui::PointerButton::Primary) {
            let Some(p) = pp else { return };
            let was_editing = self.editing.is_some();
            self.finish_edit();
            match self.tool {
                Tool::Select => self.selected = self.hit(p, slop),
                Tool::Pick => self.pick(p),
                Tool::Crop => {
                    if self.crop.is_some() {
                        self.snapshot();
                        self.crop = None;
                    }
                }
                Tool::Draw(Kind::Step) => {
                    let s = self.new_shape(Kind::Step, p);
                    self.snapshot();
                    self.shapes.push(s);
                }
                Tool::Draw(Kind::Text) => {
                    if let Some(i) = self.hit(p, slop).filter(|&i| self.shapes[i].kind == Kind::Text) {
                        self.editing = Some(i);
                        self.focus_edit = true;
                    } else if !was_editing {
                        let size = text_px(self.width);
                        let s = self.new_shape(Kind::Text, p - vec2(0.0, size * 0.6));
                        self.snapshot();
                        self.shapes.push(s);
                        self.editing = Some(self.shapes.len() - 1);
                        self.focus_edit = true;
                    }
                }
                Tool::Draw(Kind::Callout) => {
                    if let Some(i) = self.hit(p, slop).filter(|&i| self.shapes[i].kind == Kind::Callout) {
                        self.editing = Some(i);
                        self.focus_edit = true;
                    } else if !was_editing {
                        // A click drops a bubble up and to the right, its tail pointing at the click.
                        let mut s = self.new_shape(Kind::Callout, p);
                        s.b = p + vec2(60.0, -70.0);
                        self.snapshot();
                        self.shapes.push(s);
                        self.editing = Some(self.shapes.len() - 1);
                        self.focus_edit = true;
                    }
                }
                Tool::Draw(_) => {
                    // A click on an existing shape selects it for recolouring/deleting.
                    self.selected = self.hit(p, slop);
                }
            }
        }
        if resp.double_clicked() {
            if let Some(p) = pp {
                if let Some(i) = self.hit(p, slop).filter(|&i| matches!(self.shapes[i].kind, Kind::Text | Kind::Callout)) {
                    self.editing = Some(i);
                    self.focus_edit = true;
                }
            }
        }
    }

    /// Eyedropper: takes the colour under `p` from the capture.
    fn pick(&mut self, p: Pos2) {
        let (x, y) = (p.x.floor() as i32, p.y.floor() as i32);
        if x < 0 || y < 0 || x >= self.base.w as i32 || y >= self.base.h as i32 {
            return;
        }
        let i = ((y as u32 * self.base.w + x as u32) * 4) as usize;
        let c = Color32::from_rgb(self.base.px[i + 2], self.base.px[i + 1], self.base.px[i]);
        self.custom = Some(c);
        self.set_color(c);
    }

    fn draw_shape(&mut self, p: &egui::Painter, s: &Shape, to_s: &dyn Fn(Pos2) -> Pos2, z: f32, ctx: &egui::Context) {
        let w = s.width * z;
        let st = Stroke::new(w, s.color);
        match s.kind {
            Kind::Rect => {
                p.rect_stroke(Rect::from_two_pos(to_s(s.a), to_s(s.b)), CornerRadius::ZERO, st, StrokeKind::Middle);
            }
            Kind::Ellipse => {
                let r = Rect::from_two_pos(to_s(s.a), to_s(s.b));
                p.add(egui::epaint::EllipseShape::stroke(r.center(), r.size() / 2.0, st));
            }
            Kind::Line => {
                p.line_segment([to_s(s.a), to_s(s.b)], st);
                p.circle_filled(to_s(s.a), w / 2.0, s.color);
                p.circle_filled(to_s(s.b), w / 2.0, s.color);
            }
            Kind::Arrow => {
                let (end, tri) = render::arrow_geom(s.a, s.b, s.width);
                p.line_segment([to_s(s.a), to_s(end)], st);
                p.circle_filled(to_s(s.a), w / 2.0, s.color);
                p.add(egui::Shape::convex_polygon(tri.iter().map(|q| to_s(*q)).collect(), s.color, Stroke::new(w * 0.5, s.color)));
            }
            Kind::Pen | Kind::Marker => {
                let (c, ww) = if s.kind == Kind::Marker { (marker_color(s.color), w * 3.5) } else { (s.color, w) };
                let pts: Vec<Pos2> = s.pts.iter().map(|q| to_s(*q)).collect();
                if pts.len() == 1 {
                    p.circle_filled(pts[0], ww / 2.0, c);
                } else {
                    p.add(egui::Shape::line(pts, Stroke::new(ww, c)));
                }
            }
            Kind::Step => {
                let r = step_radius(s.width) * z;
                p.circle_filled(to_s(s.a), r, s.color);
                p.text(to_s(s.a), egui::Align2::CENTER_CENTER, s.n.to_string(), egui::FontId::new(r * 1.15, egui::FontFamily::Name("annot".into())), contrast(s.color));
            }
            Kind::Text => {
                p.text(to_s(s.a), egui::Align2::LEFT_TOP, &s.text, egui::FontId::new((text_px(s.width) * z).max(1.0), egui::FontFamily::Name("annot".into())), s.color);
            }
            Kind::Spotlight => {
                if self.drawing.as_ref().is_some_and(|d| d.id == s.id) {
                    p.rect_stroke(Rect::from_two_pos(to_s(s.a), to_s(s.b)), CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(160)), StrokeKind::Outside);
                }
            }
            Kind::Curve => {
                let c = render::curve_ctrl(s.a, s.b);
                let (end, tri) = render::arrow_geom(c, s.b, s.width);
                let pts: Vec<Pos2> = render::curve_pts_to(s.a, c, end, 32).into_iter().map(|q| to_s(q)).collect();
                p.add(egui::Shape::line(pts, st));
                p.circle_filled(to_s(s.a), w / 2.0, s.color);
                p.add(egui::Shape::convex_polygon(tri.iter().map(|q| to_s(*q)).collect(), s.color, Stroke::new(w * 0.5, s.color)));
            }
            Kind::Callout => {
                let px = render::callout_px(s.width);
                let (bubble, tri) = render::callout_geom(s, self.text_dims(&s.text, px));
                let br = Rect::from_min_max(to_s(bubble.min), to_s(bubble.max));
                p.add(egui::Shape::convex_polygon(tri.iter().map(|q| to_s(*q)).collect(), s.color, Stroke::NONE));
                p.rect_filled(br, CornerRadius::same((render::callout_radius(px) * z).min(120.0) as u8), s.color);
                let editing_this = self.editing.and_then(|i| self.shapes.get(i)).is_some_and(|e| e.id == s.id);
                if !editing_this {
                    let pad = render::callout_pad(px);
                    p.text(to_s(bubble.min + Vec2::splat(pad)), egui::Align2::LEFT_TOP, &s.text, egui::FontId::new((px * z).max(1.0), egui::FontFamily::Name("annot".into())), contrast(s.color));
                }
            }
            Kind::Blur => {
                let r = Rect::from_two_pos(s.a, s.b);
                let sr = Rect::from_min_max(to_s(r.min), to_s(r.max));
                if let Some(t) = self.blur_texture(ctx, s) {
                    let clip = Rect::from_min_max(to_s(pos2(r.min.x.max(0.0), r.min.y.max(0.0))), to_s(pos2(r.max.x.min(self.base.w as f32), r.max.y.min(self.base.h as f32))));
                    p.image(t, clip, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                if self.drawing.as_ref().is_some_and(|d| d.id == s.id) {
                    p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_white_alpha(160)), StrokeKind::Outside);
                }
            }
        }
    }
}

fn format_of(path: &std::path::Path) -> crate::config::ImgFormat {
    use crate::config::ImgFormat;
    match path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => ImgFormat::Jpg,
        Some("webp") => ImgFormat::Webp,
        Some("avif") => ImgFormat::Avif,
        _ => ImgFormat::Png,
    }
}

fn clamp_to(p: Pos2, img: &Image) -> Pos2 {
    pos2(p.x.clamp(0.0, img.w as f32), p.y.clamp(0.0, img.h as f32))
}

fn dist_seg(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

fn separator(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(13.0, 30.0), Sense::hover());
    ui.painter().line_segment([pos2(r.center().x, r.top() + 6.0), pos2(r.center().x, r.bottom() - 6.0)], Stroke::new(1.0, BORDER));
}

fn icon_button(ui: &mut egui::Ui, icon: &str, active: bool, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
    let bg = if active {
        Some(ACCENT.gamma_multiply(0.28))
    } else if resp.hovered() {
        Some(HI)
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(r, CornerRadius::same(8), bg);
    }
    let fg = if active { Color32::from_rgb(0xC3, 0xCF, 0xFF) } else if resp.hovered() { TEXT } else { MUTED };
    let c = r.center();
    match icon {
        "curve" => {
            let (a, b) = (c + vec2(-7.0, 6.0), c + vec2(7.0, -5.0));
            let ctl = render::curve_ctrl(a, b);
            let (end, tri) = render::arrow_geom(ctl, b, 1.6);
            ui.painter().add(egui::Shape::line(render::curve_pts_to(a, ctl, end, 16), Stroke::new(1.6, fg)));
            ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), fg, Stroke::NONE));
        }
        "spot" => {
            ui.painter().rect_filled(Rect::from_center_size(c, vec2(18.0, 16.0)), CornerRadius::same(3), fg.gamma_multiply(0.35));
            ui.painter().rect_filled(Rect::from_center_size(c, vec2(9.0, 8.0)), CornerRadius::same(2), fg);
        }
        _ => {
            ui.painter().text(c, egui::Align2::CENTER_CENTER, icon, ui::icon_font(18.0), fg);
        }
    }
    resp.on_hover_text(tip)
}

fn text_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(13.5);
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(tw + 46.0, 34.0), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(8), if resp.hovered() { HI } else { CARD });
    ui.painter().text(pos2(r.left() + 20.0, r.center().y), egui::Align2::CENTER_CENTER, icon, ui::icon_font(16.0), TEXT);
    ui.painter().text(pos2(r.left() + 34.0, r.center().y), egui::Align2::LEFT_CENTER, label, font, TEXT);
    resp
}

fn primary_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let font = ui::bold(13.5);
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(tw + 50.0, 34.0), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(8), if resp.hovered() { Color32::from_rgb(0x81, 0x99, 0xFF) } else { ACCENT });
    ui.painter().text(pos2(r.left() + 21.0, r.center().y), egui::Align2::CENTER_CENTER, icon, ui::icon_font(16.0), Color32::WHITE);
    ui.painter().text(pos2(r.left() + 36.0, r.center().y), egui::Align2::LEFT_CENTER, label, font, Color32::WHITE);
    resp
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if let Some((dir, n)) = self.demo.clone() {
            if n == 0 {
                self.demo_shapes();
            }
            if n == 8 {
                let slot = DEMO_SHOT.clone();
                ctx.request_screenshot(move |img| *slot.lock().unwrap() = Some(img));
            }
            let shot = DEMO_SHOT.lock().unwrap().take();
            if let Some(img) = shot {
                let im = Image::from_rgba(img.size[0] as u32, img.size[1] as u32, img.as_raw());
                let _ = crate::output::write_png(&dir.join("editor-ui.png"), &im, true);
                let _ = crate::output::write_png(&dir.join("editor-export.png"), &self.export(), true);
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            self.demo = Some((dir, n + 1));
            ctx.request_repaint();
        }
        self.keys(&ctx, now);

        // Unsaved changes prompt on close.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.ask_close = true;
        }

        egui::Panel::top("tools").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(10, 8)).stroke(Stroke::new(1.0, BORDER))).show(ui, |ui| {
            self.toolbar(ui, now);
        });
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let size = match self.crop {
                    Some(c) => format!("{} × {} (cropped from {} × {})", c.width().round(), c.height().round(), self.base.w, self.base.h),
                    None => format!("{} × {}", self.base.w, self.base.h),
                };
                ui.label(egui::RichText::new(size).small().color(MUTED));
                ui.label(egui::RichText::new(format!("{:.0}%", self.zoom * 100.0)).small().color(FAINT));
                if let Some((msg, t)) = &self.status {
                    if now - t < 3.0 {
                        ui.label(egui::RichText::new(format!("\u{E182}  {msg}")).small().color(Color32::from_rgb(0x5B, 0xD6, 0x8A)));
                        ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("Wheel to zoom · Space+drag to pan · 0 to fit · Shift for straight lines / squares · F beautify").small().color(FAINT));
                });
            });
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(0x10, 0x10, 0x13))).show(ui, |ui| {
            self.canvas(ui);
        });

        if self.ask_close {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("close")).show(&ctx, |ui| {
                ui.set_width(320.0);
                ui.label(egui::RichText::new("Save your changes?").heading());
                ui.label(egui::RichText::new("The annotations will be saved over the original capture.").color(MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Save").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Don't save").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => {
                    self.ask_close = false;
                    if self.save(now) {
                        self.closing = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Some(1) => {
                    self.ask_close = false;
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some(2) => self.ask_close = false,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flatten_draws_and_crops() {
        let mut base = Image::new(100, 80);
        for p in base.px.chunks_exact_mut(4) {
            p.copy_from_slice(&[255, 255, 255, 255]);
        }
        let s = Shape { id: 1, kind: Kind::Rect, a: pos2(10.0, 10.0), b: pos2(50.0, 40.0), pts: vec![], text: String::new(), color: Color32::from_rgb(255, 0, 0), width: 4.0, n: 1 };
        let out = render::flatten(&base, &[s], Some(Rect::from_min_max(pos2(5.0, 5.0), pos2(65.0, 55.0))), None);
        assert_eq!((out.w, out.h), (60, 50));
        // Top edge of the rectangle at image (30,10) → cropped (25,5): red (BGRA).
        let i = ((5 * 60 + 25) * 4) as usize;
        assert_eq!(&out.px[i..i + 3], &[0, 0, 255]);
        // Inside stays white.
        let j = ((20 * 60 + 25) * 4) as usize;
        assert_eq!(&out.px[j..j + 3], &[255, 255, 255]);
    }

    #[test]
    fn pixelate_blocks() {
        let mut img = Image::new(20, 20);
        for (i, p) in img.px.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i % 256) as u8, 0, 0, 255]);
        }
        render::pixelate(&mut img, 0, 0, 20, 20);
        assert_eq!(img.px[0], img.px[4]);
    }
}
