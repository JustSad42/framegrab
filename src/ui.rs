//! Shared egui look for the editor and settings windows.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, vec2};
use std::sync::Arc;

pub const BG: Color32 = Color32::from_rgb(0x16, 0x16, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x21);
pub const CARD: Color32 = Color32::from_rgb(0x22, 0x22, 0x28);
pub const HI: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x34);
pub const BORDER: Color32 = Color32::from_rgb(0x2f, 0x2f, 0x37);
pub const TEXT: Color32 = Color32::from_rgb(0xf3, 0xf3, 0xf6);
pub const MUTED: Color32 = Color32::from_rgb(0xa3, 0xa3, 0xae);
pub const FAINT: Color32 = Color32::from_rgb(0x6e, 0x6e, 0x7a);
pub const ACCENT: Color32 = Color32::from_rgb(0x6e, 0x8b, 0xff);

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let win = std::path::Path::new("C:\\Windows\\Fonts");
    fonts.font_data.insert("phosphor".into(), Arc::new(egui::FontData::from_static(crate::gfx::ICON_FONT)));
    if let Ok(b) = std::fs::read(win.join("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(b)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    let bold_src = ["seguisb.ttf", "segoeuib.ttf"].iter().find_map(|f| std::fs::read(win.join(f)).ok());
    let mut bold = Vec::new();
    if let Some(b) = bold_src {
        fonts.font_data.insert("segoe-sb".into(), Arc::new(egui::FontData::from_owned(b)));
        bold.push("segoe-sb".to_string());
    }
    if let Ok(b) = std::fs::read(win.join("segoeuib.ttf")) {
        fonts.font_data.insert("segoe-b".into(), Arc::new(egui::FontData::from_owned(b)));
        fonts.families.insert(FontFamily::Name("annot".into()), vec!["segoe-b".into()]);
    } else {
        let p = fonts.families[&FontFamily::Proportional].clone();
        fonts.families.insert(FontFamily::Name("annot".into()), p);
    }
    bold.extend(fonts.families[&FontFamily::Proportional].clone());
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["phosphor".into()]);
    fonts.families.entry(FontFamily::Proportional).or_default().push("phosphor".into());
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        use egui::TextStyle::*;
        s.text_styles = [
            (Heading, FontId::new(20.0, FontFamily::Name("bold".into()))),
            (Body, FontId::new(13.5, FontFamily::Proportional)),
            (Button, FontId::new(13.5, FontFamily::Proportional)),
            (Small, FontId::new(11.5, FontFamily::Proportional)),
            (Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.interact_size.y = 30.0;
        s.interaction.tooltip_delay = 0.3;
        s.animation_time = 0.1;
        let v = &mut s.visuals;
        *v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.extreme_bg_color = Color32::from_rgb(0x12, 0x12, 0x15);
        v.faint_bg_color = CARD;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.window_stroke = Stroke::new(1.0, BORDER);
        let r = CornerRadius::same(7);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        v.widgets.inactive.weak_bg_fill = CARD;
        v.widgets.inactive.bg_fill = CARD;
        v.widgets.hovered.weak_bg_fill = HI;
        v.widgets.hovered.bg_fill = HI;
        v.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.5);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.hovered.bg_stroke = Stroke::NONE;
    });
}

pub fn app_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    let dec = png::Decoder::new(std::io::Cursor::new(&bytes[..]));
    if let Ok(mut r) = dec.read_info() {
        if let Some(n) = r.output_buffer_size() {
            let mut buf = vec![0; n];
            if let Ok(info) = r.next_frame(&mut buf) {
                if info.color_type == png::ColorType::Rgba {
                    buf.truncate((info.width * info.height * 4) as usize);
                    return egui::IconData { rgba: buf, width: info.width, height: info.height };
                }
            }
        }
    }
    egui::IconData::default()
}
#[cfg(test)]
mod icon_gen {
    use tiny_skia::*;

    fn draw(n: u32) -> Pixmap {
        let mut pm = Pixmap::new(n, n).unwrap();
        let s = n as f32 / 256.0;
        let r = 56.0 * s;
        let m = 8.0 * s;
        let mut pb = PathBuilder::new();
        let (x0, y0, x1, y1) = (m, m, n as f32 - m, n as f32 - m);
        pb.move_to(x0 + r, y0);
        pb.line_to(x1 - r, y0);
        pb.quad_to(x1, y0, x1, y0 + r);
        pb.line_to(x1, y1 - r);
        pb.quad_to(x1, y1, x1 - r, y1);
        pb.line_to(x0 + r, y1);
        pb.quad_to(x0, y1, x0, y1 - r);
        pb.line_to(x0, y0 + r);
        pb.quad_to(x0, y0, x0 + r, y0);
        pb.close();
        let bg = pb.finish().unwrap();
        let mut p = Paint::default();
        p.anti_alias = true;
        p.shader = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(n as f32, n as f32),
            vec![GradientStop::new(0.0, Color::from_rgba8(0x7A, 0x96, 0xFF, 255)), GradientStop::new(1.0, Color::from_rgba8(0x5B, 0x4D, 0xE8, 255))],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap();
        pm.fill_path(&bg, &p, FillRule::Winding, Transform::identity(), None);
        // Capture corners.
        let mut w = Paint::default();
        w.anti_alias = true;
        w.set_color_rgba8(255, 255, 255, 255);
        let st = Stroke { width: (22.0 * s).max(1.6), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
        let (a, b, l) = (66.0 * s, 190.0 * s, 44.0 * s);
        for (cx, cy, dx, dy) in [(a, a, 1.0, 1.0), (b, a, -1.0, 1.0), (a, b, 1.0, -1.0), (b, b, -1.0, -1.0)] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy + dy * l);
            pb.line_to(cx, cy);
            pb.line_to(cx + dx * l, cy);
            pm.stroke_path(&pb.finish().unwrap(), &w, &st, Transform::identity(), None);
        }
        // Centre dot.
        let mut d = Paint::default();
        d.anti_alias = true;
        d.set_color_rgba8(0xFF, 0x5A, 0x6A, 255);
        pm.fill_path(&PathBuilder::from_circle(128.0 * s, 128.0 * s, (24.0 * s).max(1.5)).unwrap(), &d, FillRule::Winding, Transform::identity(), None);
        pm
    }

    /// `cargo test make_icon -- --ignored` writes assets/icon.ico and assets/icon.png.
    #[test]
    #[ignore]
    fn make_icon() {
        let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
        let mut pngs = Vec::new();
        for &n in &sizes {
            let pm = draw(n);
            pngs.push(pm.encode_png().unwrap());
            if n == 256 {
                pm.save_png("assets/icon.png").unwrap();
            }
        }
        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0, 1, 0]);
        ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
        let mut off = 6 + 16 * sizes.len() as u32;
        for (i, &n) in sizes.iter().enumerate() {
            let b = if n >= 256 { 0 } else { n as u8 };
            ico.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
            ico.extend_from_slice(&(pngs[i].len() as u32).to_le_bytes());
            ico.extend_from_slice(&off.to_le_bytes());
            off += pngs[i].len() as u32;
        }
        for p in &pngs {
            ico.extend_from_slice(p);
        }
        std::fs::write("assets/icon.ico", ico).unwrap();
    }
}
