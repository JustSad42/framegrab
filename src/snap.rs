//! Edge snapping from the pixels themselves (works in any app, including games): selection
//! edges stick to panel/button borders near the cursor, and arrow tips land on the nearest edge.

/// Brightness difference that counts as an edge between two neighbouring pixels.
const EDGE: i32 = 22;

/// Finds a vertical edge (a boundary between columns x-1 and x) near `x` that runs along the
/// rows `y0..y1`. Returns the column with the strongest, longest edge, if it's convincing.
fn edge_x(lum: &dyn Fn(i32, i32) -> Option<i32>, x: i32, y0: i32, y1: i32, r: i32) -> Option<i32> {
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    let n = (y1 - y0).max(1);
    let step = (n / 160).max(1);
    let mut best: Option<(f32, i32)> = None;
    for cx in x - r..=x + r {
        let (mut hits, mut total) = (0, 0);
        let mut y = y0;
        while y <= y1 {
            if let (Some(a), Some(b)) = (lum(cx - 1, y), lum(cx, y)) {
                total += 1;
                if (a - b).abs() > EDGE {
                    hits += 1;
                }
            }
            y += step;
        }
        if total < 4 {
            continue;
        }
        let score = hits as f32 / total as f32;
        let better = match best {
            None => true,
            Some((bs, bx)) => score > bs + 0.05 || ((score - bs).abs() <= 0.05 && (cx - x).abs() < (bx - x).abs()),
        };
        if better {
            best = Some((score, cx));
        }
    }
    best.filter(|b| b.0 >= 0.6).map(|b| b.1)
}

/// Same for horizontal edges (rows), using the transposed lookup.
fn edge_y(lum: &dyn Fn(i32, i32) -> Option<i32>, y: i32, x0: i32, x1: i32, r: i32) -> Option<i32> {
    edge_x(&|a, b| lum(b, a), y, x0, x1, r)
}

/// Snaps the dragged corner `p` of a selection that started at `anchor`. Each axis snaps on its
/// own, using the edge's run along the selection (or ±24 px when the selection is thin).
pub fn snap_corner(lum: &dyn Fn(i32, i32) -> Option<i32>, anchor: (i32, i32), p: (i32, i32), r: i32) -> (i32, i32) {
    let span = |a: i32, b: i32| if (a - b).abs() < 16 { (b - 24, b + 24) } else { (a, b) };
    let (y0, y1) = span(anchor.1, p.1);
    let (x0, x1) = span(anchor.0, p.0);
    let x = edge_x(lum, p.0, y0, y1, r).unwrap_or(p.0);
    let y = edge_y(lum, p.1, x0, x1, r).unwrap_or(p.1);
    (x, y)
}

/// Snaps a single point (where a drag starts) to nearby edges, ±24 px of run each way.
pub fn snap_point(lum: &dyn Fn(i32, i32) -> Option<i32>, p: (i32, i32), r: i32) -> (i32, i32) {
    // At a corner the edge runs on one side of the point only, so try each half.
    let x = edge_x(lum, p.0, p.1, p.1 + 24, r).or_else(|| edge_x(lum, p.0, p.1 - 24, p.1, r)).unwrap_or(p.0);
    let y = edge_y(lum, p.1, p.0, p.0 + 24, r).or_else(|| edge_y(lum, p.1, p.0 - 24, p.0, r)).unwrap_or(p.1);
    (x, y)
}

/// Moves an arrow tip onto the nearest strong edge within `r` px (e.g. a button's border).
pub fn snap_tip(lum: &dyn Fn(i32, i32) -> Option<i32>, p: (i32, i32), r: i32) -> Option<(i32, i32)> {
    let mut best: Option<(i32, (i32, i32))> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            if d2 > r * r {
                continue;
            }
            let (x, y) = (p.0 + dx, p.1 + dy);
            let (Some(c), Some(l), Some(u)) = (lum(x, y), lum(x - 1, y), lum(x, y - 1)) else { continue };
            let g = (c - l).abs().max((c - u).abs());
            if g > EDGE * 2 && best.is_none_or(|b| d2 < b.0) {
                best = Some((d2, (x, y)));
            }
        }
    }
    best.map(|b| b.1)
}

/// Brightness (0–255) of a BGRA image pixel.
pub fn luma(img: &crate::capture::Image, x: i32, y: i32) -> Option<i32> {
    if x < 0 || y < 0 || x >= img.w as i32 || y >= img.h as i32 {
        return None;
    }
    let i = ((y as u32 * img.w + x as u32) * 4) as usize;
    Some((img.px[i] as i32 * 29 + img.px[i + 1] as i32 * 150 + img.px[i + 2] as i32 * 77) >> 8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Image;

    /// A white image with a dark box from (40,30) to (120,90) (exclusive).
    fn boxed() -> Image {
        let mut img = Image::new(200, 150);
        for y in 0..150 {
            for x in 0..200 {
                let v = if (40..120).contains(&x) && (30..90).contains(&y) { 40 } else { 240 };
                let i = ((y * 200 + x) * 4) as usize;
                img.px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        img
    }

    #[test]
    fn corner_snaps_to_box() {
        let img = boxed();
        let lum = |x, y| luma(&img, x, y);
        // Dragging from near the top-left corner to near the bottom-right one.
        assert_eq!(snap_point(&lum, (43, 27), 8), (40, 30));
        assert_eq!(snap_corner(&lum, (40, 30), (116, 94), 8), (120, 90));
        // Far from any edge: unchanged.
        assert_eq!(snap_corner(&lum, (150, 100), (180, 140), 8), (180, 140));
    }

    #[test]
    fn tip_lands_on_edge() {
        let img = boxed();
        let lum = |x, y| luma(&img, x, y);
        assert_eq!(snap_tip(&lum, (60, 24), 10), Some((60, 30)));
        assert_eq!(snap_tip(&lum, (170, 120), 10), None);
    }
}
