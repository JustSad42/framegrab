//! Flattens annotations onto the image for saving/copying (tiny-skia + ab_glyph).

use super::{Kind, Shape};
use crate::capture::Image;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub fn annot_font() -> Option<Vec<u8>> {
    ["segoeuib.ttf", "seguisb.ttf", "segoeui.ttf", "arialbd.ttf"].iter().find_map(|f| std::fs::read(std::path::Path::new("C:\\Windows\\Fonts").join(f)).ok())
}

/// Pixelation block size for a blur area.
pub fn block(w: f32, h: f32) -> u32 {
    ((w.min(h) / 10.0) as u32).clamp(6, 18)
}

/// Pixelates a rectangle of `img` in place.
pub fn pixelate(img: &mut Image, x0: i32, y0: i32, x1: i32, y1: i32) {
    let (x0, x1) = (x0.clamp(0, img.w as i32) as u32, x1.clamp(0, img.w as i32) as u32);
    let (y0, y1) = (y0.clamp(0, img.h as i32) as u32, y1.clamp(0, img.h as i32) as u32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let b = block((x1 - x0) as f32, (y1 - y0) as f32);
    let mut by = y0;
    while by < y1 {
        let ey = (by + b).min(y1);
        let mut bx = x0;
        while bx < x1 {
            let ex = (bx + b).min(x1);
            let mut acc = [0u32; 3];
            let mut n = 0;
            for y in by..ey {
                for x in bx..ex {
                    let i = ((y * img.w + x) * 4) as usize;
                    acc[0] += img.px[i] as u32;
                    acc[1] += img.px[i + 1] as u32;
                    acc[2] += img.px[i + 2] as u32;
                    n += 1;
                }
            }
            let c = [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8];
            for y in by..ey {
                for x in bx..ex {
                    let i = ((y * img.w + x) * 4) as usize;
                    img.px[i..i + 3].copy_from_slice(&c);
                }
            }
            bx = ex;
        }
        by = ey;
    }
}

fn paint(c: egui::Color32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.r(), c.g(), c.b(), c.a());
    p.anti_alias = true;
    p
}

fn stroke(w: f32) -> Stroke {
    Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

/// Arrow geometry shared with the on-screen preview: shaft end and head triangle.
pub fn arrow_geom(a: egui::Pos2, b: egui::Pos2, w: f32) -> (egui::Pos2, [egui::Pos2; 3]) {
    let d = b - a;
    let len = d.length().max(0.001);
    let dir = d / len;
    let head = (w * 3.2 + 8.0).min(len * 0.6);
    let half = head * 0.55;
    let base = b - dir * head;
    let n = egui::vec2(-dir.y, dir.x);
    (base + dir * (head * 0.3), [b, base + n * half, base - n * half])
}

fn draw_text(pm: &mut Pixmap, font: &FontRef, text: &str, x: f32, y: f32, size: f32, c: egui::Color32, center: bool) {
    let sf = font.as_scaled(PxScale::from(size));
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for (li, line) in text.split('\n').enumerate() {
        let width: f32 = line.chars().map(|ch| sf.h_advance(font.glyph_id(ch))).sum();
        let mut pen = if center { x - width / 2.0 } else { x };
        let base = if center { y + (sf.ascent() + sf.descent()) / 2.0 } else { y + sf.ascent() + li as f32 * line_h };
        let mut prev = None;
        for ch in line.chars() {
            let id = font.glyph_id(ch);
            if let Some(p) = prev {
                pen += sf.kern(p, id);
            }
            let g = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, base));
            if let Some(o) = font.outline_glyph(g) {
                let bb = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let px = bb.min.x as i32 + gx as i32;
                    let py = bb.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= w || py >= h {
                        return;
                    }
                    let a = cov * c.a() as f32 / 255.0;
                    let i = ((py * w + px) * 4) as usize;
                    for (k, v) in [c.r(), c.g(), c.b()].iter().enumerate() {
                        data[i + k] = (data[i + k] as f32 * (1.0 - a) + *v as f32 * a) as u8;
                    }
                    data[i + 3] = 255;
                });
            }
            pen += sf.h_advance(id);
            prev = Some(id);
        }
    }
}

/// Text block size in image pixels (for hit-testing and layout).
pub fn text_size(font: &FontRef, text: &str, size: f32) -> (f32, f32) {
    let sf = font.as_scaled(PxScale::from(size));
    let line_h = sf.ascent() - sf.descent() + sf.line_gap();
    let w = text.split('\n').map(|l| l.chars().map(|c| sf.h_advance(font.glyph_id(c))).sum::<f32>()).fold(0.0, f32::max);
    (w, line_h * text.split('\n').count() as f32)
}

pub fn flatten(base: &Image, shapes: &[Shape], crop: Option<egui::Rect>, font_bytes: Option<&[u8]>) -> Image {
    let mut img = base.clone();
    for s in shapes.iter().filter(|s| s.kind == Kind::Blur) {
        let r = egui::Rect::from_two_pos(s.a, s.b);
        pixelate(&mut img, r.min.x as i32, r.min.y as i32, r.max.x as i32, r.max.y as i32);
    }
    let rgba = img.to_rgba();
    let Some(mut pm) = Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(img.w, img.h).unwrap()) else { return img };
    let font = font_bytes.and_then(|b| FontRef::try_from_slice(b).ok());
    for s in shapes {
        let p = paint(if s.kind == Kind::Marker { super::marker_color(s.color) } else { s.color });
        match s.kind {
            Kind::Blur => {}
            Kind::Rect => {
                let r = egui::Rect::from_two_pos(s.a, s.b);
                if let Some(rr) = tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y) {
                    let path = PathBuilder::from_rect(rr);
                    let st = Stroke { width: s.width, line_join: LineJoin::Miter, ..Default::default() };
                    pm.stroke_path(&path, &p, &st, Transform::identity(), None);
                }
            }
            Kind::Ellipse => {
                let r = egui::Rect::from_two_pos(s.a, s.b);
                if let Some(rr) = tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y) {
                    if let Some(path) = PathBuilder::from_oval(rr) {
                        pm.stroke_path(&path, &p, &stroke(s.width), Transform::identity(), None);
                    }
                }
            }
            Kind::Line | Kind::Arrow => {
                let end = if s.kind == Kind::Arrow { arrow_geom(s.a, s.b, s.width).0 } else { s.b };
                let mut pb = PathBuilder::new();
                pb.move_to(s.a.x, s.a.y);
                pb.line_to(end.x, end.y);
                if let Some(path) = pb.finish() {
                    pm.stroke_path(&path, &p, &stroke(s.width), Transform::identity(), None);
                }
                if s.kind == Kind::Arrow {
                    let (_, tri) = arrow_geom(s.a, s.b, s.width);
                    let mut pb = PathBuilder::new();
                    pb.move_to(tri[0].x, tri[0].y);
                    pb.line_to(tri[1].x, tri[1].y);
                    pb.line_to(tri[2].x, tri[2].y);
                    pb.close();
                    if let Some(path) = pb.finish() {
                        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
                        pm.stroke_path(&path, &p, &stroke(s.width * 0.5), Transform::identity(), None);
                    }
                }
            }
            Kind::Pen | Kind::Marker => {
                if s.pts.len() < 2 {
                    continue;
                }
                let mut pb = PathBuilder::new();
                pb.move_to(s.pts[0].x, s.pts[0].y);
                for q in &s.pts[1..] {
                    pb.line_to(q.x, q.y);
                }
                if let Some(path) = pb.finish() {
                    let w = if s.kind == Kind::Marker { s.width * 3.5 } else { s.width };
                    let mut st = stroke(w);
                    if s.kind == Kind::Marker {
                        st.line_cap = LineCap::Square;
                    }
                    pm.stroke_path(&path, &p, &st, Transform::identity(), None);
                }
            }
            Kind::Step => {
                let r = super::step_radius(s.width);
                if let Some(path) = PathBuilder::from_circle(s.a.x, s.a.y, r) {
                    pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
                }
                if let Some(f) = &font {
                    let fg = super::contrast(s.color);
                    draw_text(&mut pm, f, &s.n.to_string(), s.a.x, s.a.y, r * 1.15, fg, true);
                }
            }
            Kind::Text => {
                if let Some(f) = &font {
                    draw_text(&mut pm, f, &s.text, s.a.x, s.a.y, super::text_px(s.width), s.color, false);
                }
            }
        }
    }
    let out = Image::from_rgba(img.w, img.h, pm.data());
    match crop {
        Some(c) => {
            let c = c.intersect(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(out.w as f32, out.h as f32)));
            if c.width() >= 1.0 && c.height() >= 1.0 {
                out.crop(c.min.x.round() as i32, c.min.y.round() as i32, c.width().round() as i32, c.height().round() as i32)
            } else {
                out
            }
        }
        None => out,
    }
}
