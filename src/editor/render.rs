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
    spotlight(&mut img, shapes);
    let rgba = img.to_rgba();
    let Some(mut pm) = Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(img.w, img.h).unwrap()) else { return img };
    let font = font_bytes.and_then(|b| FontRef::try_from_slice(b).ok());
    for s in shapes {
        let p = paint(if s.kind == Kind::Marker { super::marker_color(s.color) } else { s.color });
        match s.kind {
            Kind::Blur | Kind::Spotlight => {}
            Kind::Curve | Kind::Callout => draw_extra(&mut pm, s, &p, font.as_ref()),
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

/// Spotlight dimming (alpha of the black overlay outside the boxes).
pub const SPOT_DIM: u8 = 150;

/// Grid cells of `full` that lie outside every rect in `spots` (their union is left bright).
pub fn outside_cells(full: egui::Rect, spots: &[egui::Rect]) -> Vec<egui::Rect> {
    let mut xs = vec![full.min.x, full.max.x];
    let mut ys = vec![full.min.y, full.max.y];
    for r in spots {
        xs.extend([r.min.x.clamp(full.min.x, full.max.x), r.max.x.clamp(full.min.x, full.max.x)]);
        ys.extend([r.min.y.clamp(full.min.y, full.max.y), r.max.y.clamp(full.min.y, full.max.y)]);
    }
    for v in [&mut xs, &mut ys] {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    }
    let mut out = Vec::new();
    for yw in ys.windows(2) {
        for xw in xs.windows(2) {
            let cell = egui::Rect::from_min_max(egui::pos2(xw[0], yw[0]), egui::pos2(xw[1], yw[1]));
            if !spots.iter().any(|s| s.contains(cell.center())) {
                out.push(cell);
            }
        }
    }
    out
}

/// Bend point for a curved arrow: a third of its length to the left of travel.
pub fn curve_ctrl(a: egui::Pos2, b: egui::Pos2) -> egui::Pos2 {
    let d = b - a;
    let n = egui::vec2(d.y, -d.x);
    a + d * 0.5 + n * 0.3
}

pub fn curve_pts_to(a: egui::Pos2, c: egui::Pos2, e: egui::Pos2, n: usize) -> Vec<egui::Pos2> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            egui::pos2(u * u * a.x + 2.0 * u * t * c.x + t * t * e.x, u * u * a.y + 2.0 * u * t * c.y + t * t * e.y)
        })
        .collect()
}

pub fn curve_pts(a: egui::Pos2, b: egui::Pos2, n: usize) -> Vec<egui::Pos2> {
    curve_pts_to(a, curve_ctrl(a, b), b, n)
}

pub fn callout_px(w: f32) -> f32 {
    (super::text_px(w) * 0.8).max(17.0)
}
pub fn callout_pad(px: f32) -> f32 {
    px * 0.55
}
pub fn callout_radius(px: f32) -> f32 {
    px * 0.6
}

/// Bubble rect (centred on `s.b`) and tail triangle (tip at `s.a`). `text` is the text block size.
pub fn callout_geom(s: &Shape, text: (f32, f32)) -> (egui::Rect, [egui::Pos2; 3]) {
    let px = callout_px(s.width);
    let pad = callout_pad(px);
    let size = egui::vec2((text.0 + pad * 2.0).max(px * 3.0), text.1 + pad * 2.0);
    let r = egui::Rect::from_center_size(s.b, size);
    let d = s.a - r.center();
    let len = d.length().max(0.001);
    let n = egui::vec2(-d.y, d.x) / len;
    let hw = size.x.min(size.y) * 0.24;
    (r, [s.a, r.center() + n * hw, r.center() - n * hw])
}

fn fill_poly(pm: &mut Pixmap, pts: &[egui::Pos2], p: &Paint) {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    for q in &pts[1..] {
        pb.line_to(q.x, q.y);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, p, FillRule::Winding, Transform::identity(), None);
    }
}

/// Darkens everything outside the spotlight boxes.
fn spotlight(img: &mut Image, shapes: &[Shape]) {
    let spots: Vec<egui::Rect> = shapes.iter().filter(|s| s.kind == Kind::Spotlight).map(|s| egui::Rect::from_two_pos(s.a, s.b)).collect();
    if spots.is_empty() {
        return;
    }
    let keep = 255 - SPOT_DIM as u32;
    let full = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(img.w as f32, img.h as f32));
    for c in outside_cells(full, &spots) {
        let (x0, x1) = (c.min.x.round() as u32, (c.max.x.round() as u32).min(img.w));
        let (y0, y1) = (c.min.y.round() as u32, (c.max.y.round() as u32).min(img.h));
        for y in y0..y1 {
            let row = &mut img.px[((y * img.w + x0) * 4) as usize..((y * img.w + x1) * 4) as usize];
            for p in row.chunks_exact_mut(4) {
                for v in &mut p[..3] {
                    *v = ((*v as u32 * keep) / 255) as u8;
                }
            }
        }
    }
}

fn draw_extra(pm: &mut Pixmap, s: &Shape, p: &Paint, font: Option<&FontRef>) {
    match s.kind {
        Kind::Curve => {
            let c = curve_ctrl(s.a, s.b);
            let (end, tri) = arrow_geom(c, s.b, s.width);
            let mut pb = PathBuilder::new();
            pb.move_to(s.a.x, s.a.y);
            pb.quad_to(c.x, c.y, end.x, end.y);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, p, &stroke(s.width), Transform::identity(), None);
            }
            fill_poly(pm, &tri, p);
            let mut pb = PathBuilder::new();
            pb.move_to(tri[0].x, tri[0].y);
            pb.line_to(tri[1].x, tri[1].y);
            pb.line_to(tri[2].x, tri[2].y);
            pb.close();
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, p, &stroke(s.width * 0.5), Transform::identity(), None);
            }
        }
        Kind::Callout => {
            let px = callout_px(s.width);
            let dims = font.map(|f| text_size(f, if s.text.is_empty() { " " } else { &s.text }, px)).unwrap_or((px * 4.0, px * 1.3));
            let (r, tri) = callout_geom(s, dims);
            fill_poly(pm, &tri, p);
            if let Some(rr) = tiny_skia::Rect::from_ltrb(r.min.x, r.min.y, r.max.x, r.max.y) {
                if let Some(path) = round_rect(rr, callout_radius(px)) {
                    pm.fill_path(&path, p, FillRule::Winding, Transform::identity(), None);
                }
            }
            if let Some(f) = font {
                let pad = callout_pad(px);
                draw_text(pm, f, &s.text, r.min.x + pad, r.min.y + pad, px, super::contrast(s.color), false);
            }
        }
        _ => {}
    }
}

fn round_rect(r: tiny_skia::Rect, rad: f32) -> Option<tiny_skia::Path> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = rad * 0.4477;
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - k, t, rt, t + k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - k, rt - k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + k, b, l, b - k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + k, l + k, t, l + rad, t);
    pb.close();
    pb.finish()
}
