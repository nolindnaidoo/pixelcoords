//! CPU rasterizer: shapes, text, and label placement into a `u32` pixel
//! buffer (`0x00RRGGBB`, softbuffer's format), plus the alpha mask applied
//! to circle crops. Everything clips to the buffer — drawing partially or
//! fully off-buffer is safe and silent.

use crate::font;
use crate::geometry::{Point, Rect, Shape, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
    };

    pub const fn to_0rgb(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }
}

pub struct Canvas<'a> {
    px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
}

impl<'a> Canvas<'a> {
    /// Wrap a pixel buffer. `px.len()` must equal `w * h`.
    pub fn new(px: &'a mut [u32], w: i32, h: i32) -> Self {
        assert!(w > 0 && h > 0, "canvas dimensions must be positive");
        assert_eq!(
            px.len(),
            (w as usize) * (h as usize),
            "buffer size mismatch"
        );
        Self { px, w, h }
    }

    fn set(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y as usize) * (self.w as usize) + (x as usize)] = color;
        }
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let c = color.to_0rgb();
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.w);
        let y1 = (rect.y + rect.h).min(self.h);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        for y in y0..y1 {
            let row = (y as usize) * (self.w as usize);
            self.px[row + x0 as usize..row + x1 as usize].fill(c);
        }
    }

    /// Darken already-drawn pixels inside `rect` to `strength`/256 of
    /// their brightness — a backdrop panel without an alpha channel. 0
    /// blacks out, 256 leaves the pixels untouched.
    pub fn dim_rect(&mut self, rect: Rect, strength: u32) {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.w);
        let y1 = (rect.y + rect.h).min(self.h);
        for y in y0..y1 {
            let row = (y as usize) * (self.w as usize);
            for px in &mut self.px[row + x0 as usize..row + x1.max(x0) as usize] {
                let r = (((*px >> 16) & 0xFF) * strength) >> 8;
                let g = (((*px >> 8) & 0xFF) * strength) >> 8;
                let b = ((*px & 0xFF) * strength) >> 8;
                *px = (r << 16) | (g << 8) | b;
            }
        }
    }

    pub fn draw_rect_outline(&mut self, rect: Rect, color: Color, thickness: i32) {
        if thickness <= 0 {
            return;
        }
        let t = thickness.min(rect.w).min(rect.h);
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, t), color);
        self.fill_rect(Rect::new(rect.x, rect.y + rect.h - t, rect.w, t), color);
        self.fill_rect(Rect::new(rect.x, rect.y, t, rect.h), color);
        self.fill_rect(Rect::new(rect.x + rect.w - t, rect.y, t, rect.h), color);
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color) {
        self.circle_band(cx, cy, 0, r, color);
    }

    pub fn draw_circle_outline(&mut self, cx: i32, cy: i32, r: i32, color: Color, thickness: i32) {
        if thickness <= 0 {
            return;
        }
        self.circle_band(cx, cy, (r - thickness).max(0), r, color);
    }

    /// Fill pixels whose distance from the center lies in (`inner`, `outer`]
    /// — `inner = 0` fills the disc.
    fn circle_band(&mut self, cx: i32, cy: i32, inner: i32, outer: i32, color: Color) {
        if outer <= 0 {
            return;
        }
        let c = color.to_0rgb();
        let inner2 = i64::from(inner) * i64::from(inner);
        let outer2 = i64::from(outer) * i64::from(outer);
        for y in (cy - outer).max(0)..=(cy + outer).min(self.h - 1) {
            for x in (cx - outer).max(0)..=(cx + outer).min(self.w - 1) {
                let dx = i64::from(x - cx);
                let dy = i64::from(y - cy);
                let d2 = dx * dx + dy * dy;
                if d2 <= outer2 && (inner == 0 || d2 > inner2) {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Draw `text` with the embedded font, top-left of the line box at
    /// (`x`, `y`). `scale` multiplies the base text size — pass the
    /// monitor's DPI scale so text is the same visual size on any display.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color, scale: i32) {
        let advance = font::advance(scale);
        let baseline = y + font::ascent(scale);
        let mut pen_x = x;
        for ch in text.chars() {
            self.blend_glyph(pen_x, baseline, ch, color, scale);
            pen_x += advance;
        }
    }

    /// One antialiased glyph, pen at (`pen_x`, `baseline`).
    fn blend_glyph(&mut self, pen_x: i32, baseline: i32, ch: char, color: Color, scale: i32) {
        let (metrics, coverage) = font::rasterize(ch, scale);
        let x0 = pen_x + metrics.xmin;
        let y0 = baseline - metrics.height as i32 - metrics.ymin;
        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let alpha = coverage[row * metrics.width + col];
                if alpha != 0 {
                    self.blend(x0 + col as i32, y0 + row as i32, color, alpha);
                }
            }
        }
    }

    /// Source-over blend of `color` at coverage `alpha` onto one pixel.
    fn blend(&mut self, col: i32, row: i32, color: Color, alpha: u8) {
        if col < 0 || row < 0 || col >= self.w || row >= self.h {
            return;
        }
        let index = (row as usize) * (self.w as usize) + (col as usize);
        let dst = self.px[index];
        let cover = u32::from(alpha);
        let keep = 255 - cover;
        let red = (u32::from(color.r) * cover + ((dst >> 16) & 0xFF) * keep) / 255;
        let green = (u32::from(color.g) * cover + ((dst >> 8) & 0xFF) * keep) / 255;
        let blue = (u32::from(color.b) * cover + (dst & 0xFF) * keep) / 255;
        self.px[index] = (red << 16) | (green << 8) | blue;
    }

    /// Whether offset (`dx`, `dy`) lies inside the origin-centered
    /// ellipse with radii (`rx`, `ry`); non-positive radii cover nothing.
    fn draw_ellipse_rotated(&mut self, shape: &Shape, deg: i32, color: Color, band: Option<i32>) {
        let Shape::Ellipse { cx, cy, rx, ry } = *shape else {
            return;
        };
        let (thickness, fill) = band.map_or((0, true), |t| (t, false));
        let bb = shape.rotated_bbox(deg);
        let c = color.to_0rgb();
        let (irx, iry) = if fill {
            (-1, -1)
        } else {
            ((rx - thickness).max(0), (ry - thickness).max(0))
        };
        for y in bb.y.max(0)..=bb.y.saturating_add(bb.h).min(self.h - 1) {
            for x in bb.x.max(0)..=bb.x.saturating_add(bb.w).min(self.w - 1) {
                let local =
                    crate::geometry::rotate_point_about(Point::new(x, y), Point::new(cx, cy), -deg);
                let (dx, dy) = (local.x - cx, local.y - cy);
                if ellipse_covers(dx, dy, rx, ry) && !ellipse_covers(dx, dy, irx, iry) {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Draw a shape rotated `deg` about its bbox center. Unrotated shapes
    /// and circles take the fast paths; triangles rotate their vertices and
    /// reuse the triangle raster; rotated rects raster by inverse-rotating
    /// each candidate pixel into the rect's local space.
    pub fn draw_shape_rotated(
        &mut self,
        shape: &Shape,
        deg: i32,
        color: Color,
        thickness: i32,
        fill: bool,
    ) {
        let deg = crate::geometry::normalize_deg(deg);
        if deg == 0 || matches!(shape, Shape::Circle { .. }) {
            return self.draw_shape(shape, color, thickness, fill);
        }
        if matches!(shape, Shape::Triangle { .. } | Shape::Poly { .. }) {
            return self.draw_shape(&shape.with_rotation_baked(deg), color, thickness, fill);
        }
        if matches!(shape, Shape::Ellipse { .. }) {
            let band = (!fill).then_some(thickness);
            return self.draw_ellipse_rotated(shape, deg, color, band);
        }
        let Shape::Rect(rect) = *shape else { return };
        if !fill && thickness <= 0 {
            return;
        }
        let c = color.to_0rgb();
        let bb = shape.rotated_bbox(deg);
        let band = f64::from(thickness);
        let (x0, y0) = (f64::from(rect.x), f64::from(rect.y));
        let (x1, y1) = (f64::from(rect.x + rect.w), f64::from(rect.y + rect.h));
        // Sample pixel CENTERS against half-open local edges, pivoting on
        // the exact f64 box center: a 180-degree rect then covers the same
        // pixels as an unrotated one. Saturating loop bounds tolerate
        // absurd deserialized coordinates.
        let pivot_x = f64::from(rect.x) + f64::from(rect.w) / 2.0;
        let pivot_y = f64::from(rect.y) + f64::from(rect.h) / 2.0;
        let rad = f64::from(-deg).to_radians();
        let (sin, cos) = rad.sin_cos();
        for y in bb.y.max(0)..=bb.y.saturating_add(bb.h).min(self.h - 1) {
            for x in bb.x.max(0)..=bb.x.saturating_add(bb.w).min(self.w - 1) {
                let dx = f64::from(x) + 0.5 - pivot_x;
                let dy = f64::from(y) + 0.5 - pivot_y;
                let lx = pivot_x + dx * cos - dy * sin;
                let ly = pivot_y + dx * sin + dy * cos;
                if lx < x0 || lx >= x1 || ly < y0 || ly >= y1 {
                    continue;
                }
                if fill {
                    self.set(x, y, c);
                    continue;
                }
                let edge_dist = (lx - x0).min(x1 - lx).min(ly - y0).min(y1 - ly);
                if edge_dist < band {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Draw a selection shape as outline or fill.
    pub fn draw_shape(&mut self, shape: &Shape, color: Color, thickness: i32, fill: bool) {
        match *shape {
            Shape::Rect(r) if fill => self.fill_rect(r, color),
            Shape::Rect(r) => self.draw_rect_outline(r, color, thickness),
            Shape::Circle { cx, cy, r } if fill => self.fill_circle(cx, cy, r, color),
            Shape::Circle { cx, cy, r } => self.draw_circle_outline(cx, cy, r, color, thickness),
            Shape::Ellipse { cx, cy, rx, ry } if fill => {
                self.ellipse_band(cx, cy, rx, ry, 0, color);
            }
            Shape::Ellipse { cx, cy, rx, ry } => {
                self.ellipse_band(cx, cy, rx, ry, thickness, color);
            }
            Shape::Triangle { .. } => self.draw_triangle(shape, color, thickness, fill),
            Shape::Poly { ref points } if fill => self.fill_poly(points, color),
            Shape::Poly { ref points } => self.draw_poly_outline(points, color, thickness),
        }
    }

    /// Even-odd scanline fill — correct for concave and self-touching
    /// freehand outlines, not just convex N-gons.
    fn fill_poly(&mut self, points: &[Point], color: Color) {
        if points.len() < 3 {
            return;
        }
        let c = color.to_0rgb();
        let y0 = points.iter().map(|p| p.y).min().unwrap_or(0).max(0);
        let y1 = points
            .iter()
            .map(|p| p.y)
            .max()
            .unwrap_or(0)
            .min(self.h - 1);
        for y in y0..=y1 {
            let spans = scanline_spans(points, y);
            for span in spans.chunks(2) {
                let [l, r] = span else { continue };
                let xa = (l.ceil() as i32).max(0);
                let xb = (r.floor() as i32).min(self.w - 1);
                for x in xa..=xb {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Outline as stamped edge segments: cheap enough to redraw per mouse
    /// move mid-drag, unlike a per-pixel distance test over the bbox.
    fn draw_poly_outline(&mut self, points: &[Point], color: Color, thickness: i32) {
        if points.len() < 2 || thickness <= 0 {
            return;
        }
        let n = points.len();
        for i in 0..n {
            self.stamp_segment(points[i], points[(i + 1) % n], color, thickness);
        }
    }

    /// A thick line as a run of small filled squares along the segment.
    fn stamp_segment(&mut self, a: Point, b: Point, color: Color, thickness: i32) {
        let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).max(1);
        let half = thickness / 2;
        for i in 0..=steps {
            let x = a.x + ((b.x - a.x) * i) / steps;
            let y = a.y + ((b.y - a.y) * i) / steps;
            self.fill_rect(
                Rect::new(x - half, y - half, thickness.max(1), thickness.max(1)),
                color,
            );
        }
    }

    /// Fill the region between the ellipse and, for `thickness > 0`, the
    /// concentric ellipse `thickness` smaller on each radius — thickness 0
    /// fills the whole interior. Normalized-distance test per pixel.
    fn ellipse_band(&mut self, cx: i32, cy: i32, rx: i32, ry: i32, thickness: i32, color: Color) {
        if rx <= 0 || ry <= 0 {
            return;
        }
        let c = color.to_0rgb();
        let (irx, iry) = if thickness <= 0 {
            (-1, -1)
        } else {
            ((rx - thickness).max(0), (ry - thickness).max(0))
        };
        for y in (cy - ry).max(0)..=(cy + ry).min(self.h - 1) {
            for x in (cx - rx).max(0)..=(cx + rx).min(self.w - 1) {
                if ellipse_covers(x - cx, y - cy, rx, ry)
                    && !ellipse_covers(x - cx, y - cy, irx, iry)
                {
                    self.set(x, y, c);
                }
            }
        }
    }

    /// Triangle raster: fill is coverage; outline is the inward band of
    /// covered pixels within `thickness` of the nearest edge.
    fn draw_triangle(&mut self, shape: &Shape, color: Color, thickness: i32, fill: bool) {
        let Shape::Triangle {
            ax,
            ay,
            bx,
            by,
            cx,
            cy,
        } = *shape
        else {
            return;
        };
        if !fill && thickness <= 0 {
            return;
        }
        let c = color.to_0rgb();
        let bb = shape.bbox();
        let band = f64::from(thickness);
        for y in bb.y.max(0)..=bb.y.saturating_add(bb.h).min(self.h - 1) {
            for x in bb.x.max(0)..=bb.x.saturating_add(bb.w).min(self.w - 1) {
                if !shape.covers(x, y) {
                    continue;
                }
                if fill {
                    self.set(x, y, c);
                    continue;
                }
                let d = seg_dist(x, y, ax, ay, bx, by)
                    .min(seg_dist(x, y, bx, by, cx, cy))
                    .min(seg_dist(x, y, cx, cy, ax, ay));
                if d < band {
                    self.set(x, y, c);
                }
            }
        }
    }
}

/// Squared-distance-free point-to-segment distance, for outline bands.
fn seg_dist(px: i32, py: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> f64 {
    let (px, py) = (f64::from(px), f64::from(py));
    let (x1, y1) = (f64::from(x1), f64::from(y1));
    let (x2, y2) = (f64::from(x2), f64::from(y2));
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0)
    };
    (px - (x1 + t * dx)).hypot(py - (y1 + t * dy))
}

/// The coordinate caption for a shape, e.g. `(120, 448) 300x88` or
/// `(900, 300) r=64`. Triangles caption their bounding box.
pub fn coord_text(shape: &Shape) -> String {
    match *shape {
        Shape::Rect(ref r) => format!("({}, {}) {}x{}", r.x, r.y, r.w, r.h),
        Shape::Circle { cx, cy, r } => format!("({cx}, {cy}) r={r}"),
        Shape::Ellipse { cx, cy, rx, ry } => format!("({cx}, {cy}) {rx}x{ry}"),
        Shape::Triangle { .. } | Shape::Poly { .. } => {
            let b = shape.bbox();
            format!("({}, {}) {}x{}", b.x, b.y, b.w, b.h)
        }
    }
}

/// Where to place a caption of `text_len` glyphs (drawn at `scale`) near
/// `bbox` so it stays readable at screen edges: above by default, flipping
/// left/right/below as needed. Ported from the predecessor's smart
/// placement.
pub fn smart_text_position(bbox: Rect, bounds: Size, text_len: usize, scale: i32) -> Point {
    let scale = scale.max(1);
    let padding = 4 * scale;
    let text_w = font::text_width(text_len, scale);
    let text_h = font::line_height(scale);
    let mut x = bbox.x;
    let mut y = bbox.y - text_h - padding;
    if x + text_w > bounds.w {
        x = bbox.x - text_w - padding;
    }
    if x < 0 {
        x = bbox.x + bbox.w + padding;
    }
    if y < 0 {
        y = bbox.y + bbox.h + padding;
    }
    if y + text_h > bounds.h {
        y = bbox.y - text_h - padding;
        if y < 0 {
            y = bbox.y + padding;
        }
    }
    x = x.max(0);
    y = y.max(0);
    if x + text_w > bounds.w {
        x = bounds.w - text_w;
    }
    if y + text_h > bounds.h {
        y = bounds.h - text_h;
    }
    Point::new(x, y)
}

/// The sorted x positions where the polygon's edges cross the horizontal
/// line through the center of pixel row `y`. Sampling mid-pixel sidesteps
/// the vertex-exactly-on-scanline double-count.
fn scanline_spans(points: &[Point], row: i32) -> Vec<f64> {
    let mid = f64::from(row) + 0.5;
    let count = points.len();
    let mut crossings = Vec::new();
    for i in 0..count {
        let from = points[i];
        let to = points[(i + 1) % count];
        let (from_y, to_y) = (f64::from(from.y), f64::from(to.y));
        if (from_y < mid) == (to_y < mid) {
            continue;
        }
        let t = (mid - from_y) / (to_y - from_y);
        crossings.push(f64::from(from.x) + t * f64::from(to.x - from.x));
    }
    crossings.sort_by(f64::total_cmp);
    crossings
}

/// Whether offset (`dx`, `dy`) lies inside the origin-centered ellipse
/// with radii (`rx`, `ry`); non-positive radii cover nothing.
fn ellipse_covers(dx: i32, dy: i32, rx: i32, ry: i32) -> bool {
    if rx <= 0 || ry <= 0 {
        return false;
    }
    let (dx, dy) = (i128::from(dx), i128::from(dy));
    let (rx, ry) = (i128::from(rx), i128::from(ry));
    dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
}

/// Zero the alpha of every RGBA pixel that no shape covers — the primary
/// cutout: selections stay visible in place, everything else goes
/// transparent. Each `(shape, deg)` pairs a shape with its rotation.
/// Coverage is rasterized per shape over its rotated bbox, so cost scales
/// with selection area rather than selections × frame.
pub fn apply_cutout_mask(rgba: &mut [u8], w: i32, h: i32, shapes: &[(Shape, i32)]) {
    let covered = coverage(rgba.len(), w, h, shapes);
    for (i, inside) in covered.iter().enumerate() {
        if !inside {
            rgba[i * 4 + 3] = 0;
        }
    }
}

/// The exact complement: zero the alpha of every pixel a shape covers —
/// the inverse cutout punches the selections out and keeps the rest, so
/// the pair reassembles the frame with no pixel in both.
pub fn apply_inverse_cutout_mask(rgba: &mut [u8], w: i32, h: i32, shapes: &[(Shape, i32)]) {
    let covered = coverage(rgba.len(), w, h, shapes);
    for (i, inside) in covered.iter().enumerate() {
        if *inside {
            rgba[i * 4 + 3] = 0;
        }
    }
}

/// Which pixels any shape covers, rasterized per shape over its rotated
/// bbox. `rgba_len` is asserted against the dimensions once, here, for
/// both cutout appliers.
fn coverage(rgba_len: usize, w: i32, h: i32, shapes: &[(Shape, i32)]) -> Vec<bool> {
    assert_eq!(
        rgba_len,
        (w as usize) * (h as usize) * 4,
        "RGBA buffer size mismatch"
    );
    let mut covered = vec![false; (w as usize) * (h as usize)];
    for (shape, deg) in shapes {
        mark_covered(&mut covered, w, h, shape, *deg);
    }
    covered
}

/// Mark the pixels `shape` (rotated `deg`) covers, clipped to the canvas.
fn mark_covered(covered: &mut [bool], w: i32, h: i32, shape: &Shape, deg: i32) {
    let bbox = shape.rotated_bbox(deg);
    let x0 = bbox.x.max(0);
    let y0 = bbox.y.max(0);
    let x1 = bbox.x.saturating_add(bbox.w).min(w);
    let y1 = bbox.y.saturating_add(bbox.h).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            if shape.hit_test_rotated(deg, crate::geometry::Point::new(x, y)) {
                covered[(y as usize) * (w as usize) + (x as usize)] = true;
            }
        }
    }
}

/// Zero the alpha of every RGBA pixel the shape (rotated `deg` about its
/// bbox center) does not cover. Used on non-rect crops so the outside of
/// the shape is transparent; `shape` is in the crop image's own coordinate
/// space.
pub fn apply_alpha_mask_outside(rgba: &mut [u8], w: i32, h: i32, shape: &Shape, deg: i32) {
    assert_eq!(
        rgba.len(),
        (w as usize) * (h as usize) * 4,
        "RGBA buffer size mismatch"
    );
    for y in 0..h {
        for x in 0..w {
            if !shape.hit_test_rotated(deg, crate::geometry::Point::new(x, y)) {
                rgba[((y as usize) * (w as usize) + (x as usize)) * 4 + 3] = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 100;
    const H: i32 = 60;
    const RED: Color = Color { r: 255, g: 0, b: 0 };

    fn canvas_buf() -> Vec<u32> {
        vec![0u32; (W * H) as usize]
    }

    fn px(buf: &[u32], x: i32, y: i32) -> u32 {
        buf[(y * W + x) as usize]
    }

    #[test]
    fn rect_outline_sets_border_not_interior() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_rect_outline(Rect::new(10, 10, 20, 20), RED, 2);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 10, 10), red);
        assert_eq!(px(&buf, 29, 29), red);
        assert_eq!(px(&buf, 11, 15), red); // inside 2px border
        assert_eq!(px(&buf, 15, 15), 0); // interior untouched
        assert_eq!(px(&buf, 9, 10), 0); // outside untouched
    }

    #[test]
    fn fill_rect_clips_to_canvas() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.fill_rect(Rect::new(-10, -10, 30, 30), RED);
        assert_eq!(px(&buf, 0, 0), RED.to_0rgb());
        assert_eq!(px(&buf, 19, 19), RED.to_0rgb());
        assert_eq!(px(&buf, 20, 20), 0);
    }

    #[test]
    fn fill_rect_fully_off_canvas_is_a_noop() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.fill_rect(Rect::new(W + 10, 10, 20, 20), RED);
        c.fill_rect(Rect::new(-50, -50, 20, 20), RED);
        c.fill_rect(Rect::new(10, H + 5, 20, 20), RED);
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn zero_thickness_draws_nothing() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_rect_outline(Rect::new(10, 10, 20, 20), RED, 0);
        c.draw_circle_outline(50, 30, 10, RED, 0);
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn ellipse_outline_is_a_band_and_fill_covers_the_interior() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        let e = Shape::Ellipse {
            cx: 50,
            cy: 30,
            rx: 30,
            ry: 15,
        };
        c.draw_shape(&e, RED, 3, false);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 79, 30), red, "on the rim");
        assert_eq!(px(&buf, 50, 30), 0, "outline leaves the center empty");
        assert_eq!(px(&buf, 79, 15), 0, "bbox corner outside");

        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_shape(&e, RED, 3, true);
        assert_eq!(px(&buf, 50, 30), red, "fill covers the center");
    }

    #[test]
    fn rotated_ellipse_raster_follows_the_turn() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        let e = Shape::Ellipse {
            cx: 50,
            cy: 30,
            rx: 25,
            ry: 6,
        };
        c.draw_shape_rotated(&e, 90, RED, 2, true);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 50, 50), red, "stands tall after the turn");
        assert_eq!(px(&buf, 70, 30), 0, "no longer lies flat");
    }

    #[test]
    fn circle_outline_is_an_annulus() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_circle_outline(50, 30, 10, RED, 2);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 60, 30), red); // on the radius
        assert_eq!(px(&buf, 59, 30), red); // within the band
        assert_eq!(px(&buf, 50, 30), 0); // center empty
        assert_eq!(px(&buf, 62, 30), 0); // outside
    }

    #[test]
    fn fill_circle_covers_center_and_clips() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.fill_circle(0, 0, 10, RED); // 3/4 off-canvas
        assert_eq!(px(&buf, 0, 0), RED.to_0rgb());
        assert_eq!(px(&buf, 7, 7), RED.to_0rgb());
        assert_eq!(px(&buf, 8, 8), 0);
    }

    #[test]
    fn triangle_fill_covers_interior_not_bbox_corners() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        let tri = Shape::Triangle {
            ax: 50,
            ay: 10,
            bx: 10,
            by: 50,
            cx: 90,
            cy: 50,
        };
        c.draw_shape(&tri, RED, 2, true);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 50, 40), red); // interior
        assert_eq!(px(&buf, 50, 11), red); // just below apex
        assert_eq!(px(&buf, 12, 12), 0); // bbox top-left, outside triangle
        assert_eq!(px(&buf, 88, 12), 0); // bbox top-right, outside triangle
    }

    #[test]
    fn triangle_outline_is_a_band_not_a_fill() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        let tri = Shape::Triangle {
            ax: 50,
            ay: 10,
            bx: 10,
            by: 50,
            cx: 90,
            cy: 50,
        };
        c.draw_shape(&tri, RED, 2, false);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 50, 49), red); // on the base edge
        assert_eq!(px(&buf, 50, 35), 0); // interior stays empty
        assert_eq!(px(&buf, 12, 12), 0); // outside stays empty
    }

    #[test]
    fn text_inks_inside_its_line_box_and_nowhere_else() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_text(10, 10, "I", RED, 1);
        // Ink lands somewhere inside the one-glyph line box; nothing
        // changes outside it (one pixel of slack for hinting offsets).
        let box_x = 10 - 1..10 + font::advance(1) + 1;
        let box_y = 10 - 1..10 + font::line_height(1) + 1;
        let mut inked = false;
        for y in 0..H {
            for x in 0..W {
                let p = px(&buf, x, y);
                if box_x.contains(&x) && box_y.contains(&y) {
                    inked |= p != 0;
                    continue;
                }
                assert_eq!(p, 0, "stray ink at ({x},{y})");
            }
        }
        assert!(inked, "the glyph drew nothing");
    }

    #[test]
    fn text_off_canvas_is_safe() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_text(-5, -5, "EDGE", RED, 1);
        c.draw_text(W - 3, H - 3, "EDGE", RED, 1);
    }

    #[test]
    fn smart_position_defaults_above() {
        let p = smart_text_position(Rect::new(200, 200, 100, 50), Size::new(1920, 1080), 10, 1);
        assert_eq!(p, Point::new(200, 200 - font::line_height(1) - 4));
    }

    #[test]
    fn smart_position_flips_below_at_top_edge() {
        let p = smart_text_position(Rect::new(200, 2, 100, 50), Size::new(1920, 1080), 10, 1);
        assert_eq!(p, Point::new(200, 2 + 50 + 4));
    }

    #[test]
    fn smart_position_flips_left_at_right_edge() {
        let bbox = Rect::new(1900, 200, 15, 50);
        let p = smart_text_position(bbox, Size::new(1920, 1080), 10, 1);
        let text_w = font::text_width(10, 1);
        assert_eq!(p.x, 1900 - text_w - 4);
    }

    #[test]
    fn smart_position_stays_on_screen_in_corners() {
        let bounds = Size::new(1920, 1080);
        let text_len = 20;
        for bbox in [
            Rect::new(0, 0, 50, 50),
            Rect::new(1870, 0, 50, 50),
            Rect::new(0, 1030, 50, 50),
            Rect::new(1870, 1030, 50, 50),
        ] {
            let p = smart_text_position(bbox, bounds, text_len, 1);
            assert!(p.x >= 0 && p.y >= 0, "{bbox:?} gave {p:?}");
            assert!(
                p.x + font::text_width(text_len, 1) <= bounds.w
                    && p.y + font::line_height(1) <= bounds.h,
                "{bbox:?} gave {p:?}"
            );
        }
    }

    #[test]
    fn scaled_text_is_larger_and_reaches_full_ink() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_text(10, 10, "M", RED, 2);
        // At scale 2 a dense glyph has fully-covered interior pixels, so
        // the exact color appears; everything stays inside the line box.
        let red = RED.to_0rgb();
        let box_x = 10 - 1..10 + font::advance(2) + 1;
        let box_y = 10 - 1..10 + font::line_height(2) + 1;
        let mut solid = false;
        for y in 0..H {
            for x in 0..W {
                let p = px(&buf, x, y);
                if box_x.contains(&x) && box_y.contains(&y) {
                    solid |= p == red;
                    continue;
                }
                assert_eq!(p, 0, "stray ink at ({x},{y})");
            }
        }
        assert!(solid, "no fully-covered pixel in 'M' at scale 2");
    }

    #[test]
    fn scaled_smart_position_scales_offsets() {
        let p = smart_text_position(Rect::new(200, 200, 100, 50), Size::new(1920, 1080), 10, 2);
        assert_eq!(p, Point::new(200, 200 - font::line_height(2) - 8));
    }

    #[test]
    fn coord_text_formats() {
        assert_eq!(
            coord_text(&Shape::Rect(Rect::new(1, 2, 3, 4))),
            "(1, 2) 3x4"
        );
        assert_eq!(
            coord_text(&Shape::Circle { cx: 9, cy: 8, r: 7 }),
            "(9, 8) r=7"
        );
    }

    #[test]
    fn rotated_rect_raster_follows_the_turn() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        // Wide flat rect centered at (50, 30), turned 90 deg: it becomes
        // tall and thin.
        let s = Shape::Rect(Rect::new(20, 25, 60, 10));
        c.draw_shape_rotated(&s, 90, RED, 2, true);
        let red = RED.to_0rgb();
        assert_eq!(px(&buf, 50, 5), red); // above center: inside turned
        assert_eq!(px(&buf, 50, 55), red); // below center: inside turned
        assert_eq!(px(&buf, 75, 30), 0); // right of center: outside now
        // Rotation 0 matches plain drawing.
        let mut buf2 = canvas_buf();
        let mut c2 = Canvas::new(&mut buf2, W, H);
        c2.draw_shape_rotated(&s, 0, RED, 2, true);
        let mut buf3 = canvas_buf();
        let mut c3 = Canvas::new(&mut buf3, W, H);
        c3.draw_shape(&s, RED, 2, true);
        assert_eq!(buf2, buf3);
    }

    #[test]
    fn rect_covers_same_pixel_count_at_0_and_180_degrees() {
        let s = Shape::Rect(Rect::new(20, 20, 11, 7)); // odd dims on purpose
        let mut plain = canvas_buf();
        Canvas::new(&mut plain, W, H).draw_shape(&s, RED, 2, true);
        let mut turned = canvas_buf();
        Canvas::new(&mut turned, W, H).draw_shape_rotated(&s, 180, RED, 2, true);
        let count = |buf: &[u32]| buf.iter().filter(|&&p| p != 0).count();
        assert_eq!(count(&plain), 77);
        assert_eq!(count(&turned), 77, "180-degree raster must match 0-degree");
    }

    #[test]
    fn dim_rect_scales_brightness_and_clips_to_the_canvas() {
        let mut buf = vec![0x00FF_8040u32; (W * H) as usize];
        let mut c = Canvas::new(&mut buf, W, H);
        // Half brightness inside; a rect hanging off the canvas clips.
        c.dim_rect(Rect::new(-10, -10, 20, 20), 128);
        assert_eq!(buf[0], 0x007F_4020, "channels each halve");
        assert_eq!(
            buf[(10 * W + 10) as usize],
            0x00FF_8040,
            "outside the rect untouched"
        );

        let mut buf = vec![0x00FF_FFFFu32; (W * H) as usize];
        Canvas::new(&mut buf, W, H).dim_rect(Rect::new(0, 0, 2, 1), 0);
        assert_eq!(buf[0], 0, "strength 0 blacks out");

        let mut buf = vec![0x0012_3456u32; (W * H) as usize];
        Canvas::new(&mut buf, W, H).dim_rect(Rect::new(0, 0, 1, 1), 256);
        assert_eq!(buf[0], 0x0012_3456, "strength 256 leaves pixels alone");
        // A fully off-canvas rect is a no-op, not a panic.
        Canvas::new(&mut buf, W, H).dim_rect(Rect::new(-50, -50, 10, 10), 64);
    }

    #[test]
    fn cutout_keeps_every_shape_in_place_and_clears_the_rest() {
        let w = 60;
        let h = 40;
        let mut rgba = vec![200u8; (w * h * 4) as usize];
        let shapes = [
            (Shape::Rect(Rect::new(5, 5, 10, 10)), 0),
            (
                Shape::Circle {
                    cx: 40,
                    cy: 20,
                    r: 6,
                },
                0,
            ),
        ];
        apply_cutout_mask(&mut rgba, w, h, &shapes);
        let pixel = |x: i32, y: i32| {
            let i = ((y * w + x) * 4) as usize;
            (rgba[i], rgba[i + 3])
        };
        // Inside either shape: color and alpha untouched.
        assert_eq!(pixel(10, 10), (200, 200));
        assert_eq!(pixel(40, 20), (200, 200));
        // Between and outside: transparent, color bytes untouched.
        assert_eq!(pixel(25, 10), (200, 0));
        assert_eq!(pixel(0, 39), (200, 0));
        // The circle's bbox corner is outside the circle itself.
        assert_eq!(pixel(35, 15), (200, 0));
    }

    #[test]
    fn cutout_of_a_rotated_rect_follows_the_turn() {
        let w = 30;
        let h = 30;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        // Flat rect centered at (15, 15), turned 90: vertical strip kept.
        apply_cutout_mask(
            &mut rgba,
            w,
            h,
            &[(Shape::Rect(Rect::new(3, 12, 24, 6)), 90)],
        );
        let alpha = |x: i32, y: i32| rgba[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(15, 5), 255);
        assert_eq!(alpha(5, 15), 0);
    }

    #[test]
    fn cutout_clips_offscreen_shapes_instead_of_panicking() {
        let w = 20;
        let h = 20;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        // Hangs off the top-left; only the on-canvas part is kept.
        apply_cutout_mask(
            &mut rgba,
            w,
            h,
            &[(Shape::Rect(Rect::new(-10, -10, 15, 15)), 0)],
        );
        let alpha = |x: i32, y: i32| rgba[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(2, 2), 255);
        assert_eq!(alpha(10, 10), 0);
    }

    #[test]
    fn cutout_with_no_shapes_clears_everything() {
        let mut rgba = vec![255u8; 4 * 4];
        apply_cutout_mask(&mut rgba, 2, 2, &[]);
        assert!(rgba.chunks(4).all(|p| p[3] == 0));
    }

    #[test]
    fn inverse_cutout_is_the_exact_complement() {
        let w = 60;
        let h = 40;
        let shapes = [
            (Shape::Rect(Rect::new(5, 5, 10, 10)), 0),
            (
                Shape::Circle {
                    cx: 40,
                    cy: 20,
                    r: 6,
                },
                45,
            ),
        ];
        let mut primary = vec![255u8; (w * h * 4) as usize];
        let mut inverse = vec![255u8; (w * h * 4) as usize];
        apply_cutout_mask(&mut primary, w, h, &shapes);
        apply_inverse_cutout_mask(&mut inverse, w, h, &shapes);
        // Every pixel is transparent in exactly one of the pair — together
        // they reassemble the frame.
        for i in 0..(w * h) as usize {
            let (p, v) = (primary[i * 4 + 3], inverse[i * 4 + 3]);
            assert_eq!(p ^ v, 255, "pixel {i}: primary {p}, inverse {v}");
        }
        // Spot-check orientation: inside a shape the inverse is the
        // transparent one.
        let idx = ((10 * w + 10) * 4 + 3) as usize;
        assert_eq!(primary[idx], 255);
        assert_eq!(inverse[idx], 0);
    }

    #[test]
    fn rotated_alpha_mask_follows_the_turn() {
        let w = 30;
        let h = 30;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        // Flat rect centered at (15,15), turned 90: vertical strip opaque.
        let s = Shape::Rect(Rect::new(3, 12, 24, 6));
        apply_alpha_mask_outside(&mut rgba, w, h, &s, 90);
        let alpha = |x: i32, y: i32| rgba[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(15, 5), 255); // vertical strip kept
        assert_eq!(alpha(5, 15), 0); // horizontal extent cleared
    }

    #[test]
    fn alpha_mask_zeroes_outside_circle_only() {
        let w = 20;
        let h = 20;
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        apply_alpha_mask_outside(
            &mut rgba,
            w,
            h,
            &Shape::Circle {
                cx: 10,
                cy: 10,
                r: 8,
            },
            0,
        );
        let alpha = |x: i32, y: i32| rgba[((y * w + x) * 4 + 3) as usize];
        assert_eq!(alpha(10, 10), 255); // center kept
        assert_eq!(alpha(10, 2), 255); // on the radius kept
        assert_eq!(alpha(0, 0), 0); // corner cleared
        assert_eq!(rgba[0], 255); // color channels untouched
    }

    #[test]
    fn a_rotated_rect_fills_its_interior() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_shape_rotated(&Shape::Rect(Rect::new(30, 15, 40, 30)), 30, RED, 2, true);
        // The centre of a rotated rect is inside it at any angle.
        assert_eq!(px(&buf, 50, 30), RED.to_0rgb());
    }

    #[test]
    fn a_rotated_rect_outline_is_a_band_not_a_fill() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        c.draw_shape_rotated(&Shape::Rect(Rect::new(30, 15, 40, 30)), 30, RED, 2, false);
        assert_eq!(px(&buf, 50, 30), 0, "centre stays empty for an outline");
        assert!(
            buf.iter().any(|&p| p == RED.to_0rgb()),
            "something was drawn"
        );
    }

    #[test]
    fn a_rotated_triangle_bakes_its_rotation() {
        let mut buf = canvas_buf();
        let mut c = Canvas::new(&mut buf, W, H);
        let tri = Shape::Triangle {
            ax: 50,
            ay: 10,
            bx: 20,
            by: 50,
            cx: 80,
            cy: 50,
        };
        c.draw_shape_rotated(&tri, 90, RED, 1, true);
        assert!(buf.iter().any(|&p| p == RED.to_0rgb()));
    }

    #[test]
    fn draw_shape_fills_or_outlines_each_kind() {
        for fill in [true, false] {
            for shape in [
                Shape::Rect(Rect::new(10, 10, 20, 20)),
                Shape::Circle {
                    cx: 50,
                    cy: 30,
                    r: 12,
                },
                Shape::Triangle {
                    ax: 60,
                    ay: 10,
                    bx: 45,
                    by: 40,
                    cx: 75,
                    cy: 40,
                },
            ] {
                let mut buf = canvas_buf();
                let mut c = Canvas::new(&mut buf, W, H);
                c.draw_shape(&shape, RED, 2, fill);
                assert!(
                    buf.iter().any(|&p| p == RED.to_0rgb()),
                    "{shape:?} fill={fill} drew nothing"
                );
            }
        }
    }

    #[test]
    fn a_caption_flips_inside_the_canvas_at_every_edge() {
        let bounds = Size::new(W, H);
        let len = 6;
        // Each corner drives a different branch of the placement cascade;
        // whichever it picks must land inside the canvas.
        for bbox in [
            Rect::new(0, 0, 20, 20),         // no room above
            Rect::new(W - 10, 0, 20, 20),    // no room right
            Rect::new(0, H - 10, 20, 20),    // no room below
            Rect::new(W - 5, H - 5, 20, 20), // no room anywhere
        ] {
            let p = smart_text_position(bbox, bounds, len, 1);
            assert!(p.x >= 0 && p.y >= 0, "{bbox:?} -> {p:?}");
            assert!(p.x + font::text_width(len, 1) <= W, "{bbox:?} -> {p:?}");
            assert!(p.y + font::line_height(1) <= H, "{bbox:?} -> {p:?}");
        }
    }

    #[test]
    fn a_caption_wider_than_the_canvas_starts_off_the_left_edge() {
        // Documented, not desired: nothing can fit, so the placement pins
        // the right edge and the head of the text clips away. Drawing
        // clips per pixel, so this is safe rather than fatal.
        let len = 40;
        assert!(font::text_width(len, 1) > W);
        let p = smart_text_position(Rect::new(0, 0, 20, 20), Size::new(W, H), len, 1);
        assert!(p.x < 0, "{p:?}");
    }
}
