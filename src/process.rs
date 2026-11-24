//! Post-processing applied automatically per the settings: the "beautify" frame
//! (gradient background, padding, rounded corners, soft shadow).

use crate::capture::Image;
use tiny_skia::{Color, GradientStop, LinearGradient, Paint, Pixmap, Point, Rect as SkRect, SpreadMode, Transform};

/// Background presets: (top-left, bottom-right) colours.
pub const BACKGROUNDS: [(&str, u32, u32); 6] = [
    ("Indigo", 0x7A96FF, 0x5B4DE8),
    ("Sunset", 0xFF9A62, 0xE8467C),
    ("Ocean", 0x38BDF8, 0x2563EB),
    ("Mint", 0x5EEAD4, 0x059669),
    ("Slate", 0x475569, 0x1E293B),
    ("Paper", 0xF8FAFC, 0xE2E8F0),
];

fn rgb(c: u32) -> Color {
    Color::from_rgba8((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

/// Three box-blur passes ≈ Gaussian, on a single-channel mask.
fn blur(mask: &mut [f32], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0f32; mask.len()];
    for _ in 0..3 {
        // Horizontal.
        for y in 0..h {
            let row = &mask[y * w..(y + 1) * w];
            let out = &mut tmp[y * w..(y + 1) * w];
            let mut acc = 0.0;
            let n = (2 * r + 1) as f32;
            for x in 0..=r.min(w - 1) {
                acc += row[x];
            }
            for x in 0..w {
                out[x] = acc / n;
                if x + r + 1 < w {
                    acc += row[x + r + 1];
                }
                if x >= r {
                    acc -= row[x - r];
                }
            }
        }
        // Vertical.
        for x in 0..w {
            let mut acc = 0.0;
            let n = (2 * r + 1) as f32;
            for y in 0..=r.min(h - 1) {
                acc += tmp[y * w + x];
            }
            for y in 0..h {
                mask[y * w + x] = acc / n;
                if y + r + 1 < h {
                    acc += tmp[(y + r + 1) * w + x];
                }
                if y >= r {
                    acc -= tmp[(y - r) * w + x];
                }
            }
        }
    }
}

pub struct Beautify {
    pub bg: u8,
    pub padding: u32,
    pub radius: u32,
    pub shadow: bool,
}

impl Beautify {
    pub fn from(cfg: &crate::config::Config) -> Self {
        Self { bg: cfg.beautify_bg, padding: cfg.beautify_padding, radius: cfg.beautify_radius, shadow: cfg.beautify_shadow }
    }
    /// Scale for padding/radius so the look is similar on small and 4K captures.
    pub fn scale(w: u32, h: u32) -> f32 {
        (w.max(h) as f32 / 1600.0).clamp(0.6, 2.5)
    }
}

pub fn beautify(img: &Image, b: &Beautify) -> Image {
    let s = Beautify::scale(img.w, img.h);
    let pad = (b.padding as f32 * s).round() as u32;
    let radius = b.radius as f32 * s;
    let (w, h) = (img.w + pad * 2, img.h + pad * 2);
    let Some(mut pm) = Pixmap::new(w, h) else { return img.clone() };
    let (_, c0, c1) = BACKGROUNDS[(b.bg as usize).min(BACKGROUNDS.len() - 1)];
    let mut paint = Paint::default();
    paint.shader = LinearGradient::new(Point::from_xy(0.0, 0.0), Point::from_xy(w as f32, h as f32), vec![GradientStop::new(0.0, rgb(c0)), GradientStop::new(1.0, rgb(c1))], SpreadMode::Pad, Transform::identity()).unwrap_or(tiny_skia::Shader::SolidColor(rgb(c0)));
    pm.fill_rect(SkRect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap(), &paint, Transform::identity(), None);
    // Pixmap is RGBA premultiplied; background is opaque so it's plain RGBA.
    let mut out = Image::from_rgba(w, h, pm.data());

    let mut fg = img.clone();
    crate::winfo::round_corners(&mut fg, radius);

    if b.shadow {
        let (sw, sh) = (w as usize, h as usize);
        let mut mask = vec![0f32; sw * sh];
        let off = (pad as f32 * 0.22) as usize;
        for y in 0..fg.h as usize {
            let my = y + pad as usize + off;
            if my >= sh {
                break;
            }
            for x in 0..fg.w as usize {
                mask[my * sw + x + pad as usize] = fg.px[(y * fg.w as usize + x) * 4 + 3] as f32 / 255.0;
            }
        }
        blur(&mut mask, sw, sh, ((pad as f32 * 0.3) as usize).max(1));
        for (i, p) in out.px.chunks_exact_mut(4).enumerate() {
            let a = mask[i] * 0.42;
            if a > 0.0 {
                for c in &mut p[..3] {
                    *c = (*c as f32 * (1.0 - a)) as u8;
                }
            }
        }
    }
    // Composite the screenshot.
    for y in 0..fg.h {
        for x in 0..fg.w {
            let si = ((y * fg.w + x) * 4) as usize;
            let a = fg.px[si + 3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let di = (((y + pad) * w + x + pad) * 4) as usize;
            for c in 0..3 {
                out.px[di + c] = (fg.px[si + c] as f32 * a + out.px[di + c] as f32 * (1.0 - a)) as u8;
            }
        }
    }
    for p in out.px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn beautify_pads_and_keeps_content() {
        let mut img = Image::new(200, 100);
        for p in img.px.chunks_exact_mut(4) {
            p.copy_from_slice(&[10, 200, 30, 255]);
        }
        let b = Beautify { bg: 0, padding: 50, radius: 10, shadow: true };
        let out = beautify(&img, &b);
        let pad = (50.0 * Beautify::scale(200, 100)).round() as u32;
        assert_eq!(out.w, 200 + pad * 2);
        let c = (((pad + 50) * out.w + pad + 100) * 4) as usize;
        assert_eq!(&out.px[c..c + 3], &[10, 200, 30]);
    }
}
