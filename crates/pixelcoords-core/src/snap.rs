//! Edge snapping: pull a point onto the UI edges already in the frozen
//! image.
//!
//! A frozen screen is the ideal substrate for this — the image cannot
//! change under the detector, so a snap is reproducible and a test can
//! assert exactly where it lands. Detection is pixels only: no
//! accessibility tree, no UI toolkit introspection, which is what keeps
//! it platform-free and equally honest on a native app, a game, and a
//! screenshot of either.
//!
//! The two axes are independent. `x` snaps to **vertical** edges (a
//! horizontal luma gradient) and `y` to **horizontal** ones, so dragging
//! a rect corner onto a button corner is one gesture that happens to
//! satisfy two separate searches.

use crate::geometry::Point;
use crate::locate::GrayImage;

/// Gradient strength below which nothing is an edge, on the 0–255 scale
/// `EdgeMap` quantizes to. Roughly a 3% luma step across two pixels: it
/// keeps compression noise and subtle background gradients from
/// capturing the cursor, while every real UI border clears it easily.
///
/// This is a floor under the adaptive threshold, not the threshold —
/// see [`EdgeMap::threshold`].
pub const MIN_GRADIENT: u8 = 20;

/// The percentage of the frame an adaptive threshold sits above. UI
/// screenshots are mostly flat, so the interesting gradients live in the
/// last couple of percent — and on a busy frame, where far more than 2%
/// of pixels carry *some* gradient, this is what keeps texture and
/// anti-aliasing from being offered as edges.
///
/// A whole percent rather than a fraction so the percentile is integer
/// arithmetic: the counts are exact, and a float round-trip through
/// millions of samples would only add a way to be off by one.
const EDGE_PERCENT: u64 = 98;

/// Rows sampled either side of the query row when scoring a column (and
/// columns either side when scoring a row). A real edge runs through all
/// of them; a lone speckle is averaged away.
const SCORE_HALF_SPAN: i32 = 2;

/// How far the reported edge extent is traced before giving up. The span
/// exists so the overlay can show *what* was snapped to; tracing a
/// full-height window border to both screen edges would be honest but
/// useless as feedback, and unbounded work per mouse move.
const MAX_SPAN: i32 = 160;

/// A snap that happened: where the point moved to, and how far the edge
/// it landed on runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapHit {
    /// The snapped coordinate on the queried axis.
    pub at: i32,
    /// Inclusive extent of the edge along the *other* axis, for drawing
    /// feedback. Always contains the query point's other coordinate.
    pub span: (i32, i32),
}

/// The result of asking where a point wants to go. Each axis answers on
/// its own: a corner snaps both, a vertical border snaps only `x`, and
/// open space snaps neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snap {
    pub x: Option<SnapHit>,
    pub y: Option<SnapHit>,
}

impl Snap {
    /// `p` with each axis moved to its snapped value, leaving axes that
    /// found nothing alone.
    #[must_use]
    pub fn apply(self, p: Point) -> Point {
        Point::new(
            self.x.map_or(p.x, |hit| hit.at),
            self.y.map_or(p.y, |hit| hit.at),
        )
    }

    #[must_use]
    pub const fn is_hit(self) -> bool {
        self.x.is_some() || self.y.is_some()
    }
}

/// Per-pixel edge strength for one frozen frame, precomputed once.
///
/// Two maps, one per axis, quantized to `u8` — a frame's worth of `f32`
/// pairs is tens of megabytes per monitor, and the extra precision buys
/// nothing when the answer is an integer pixel column.
#[derive(Debug, Clone)]
pub struct EdgeMap {
    w: usize,
    h: usize,
    /// Horizontal gradient: high on **vertical** edges, so this is what
    /// an `x` snap searches.
    gx: Vec<u8>,
    /// Vertical gradient: high on **horizontal** edges, searched by `y`.
    gy: Vec<u8>,
    threshold: u8,
}

impl EdgeMap {
    /// Detect edges in a frozen frame.
    ///
    /// The operator is a **forward** difference — `I(x) - I(x-1)` —
    /// weighted 3:10:3 across the neighbouring rows in the Scharr manner.
    /// The weighting rejects single-pixel noise without smearing the
    /// edge's position; the forward difference is what makes the position
    /// unambiguous. A centered difference peaks equally on both pixels of
    /// a one-pixel step, and a snap that lands on whichever of the two the
    /// cursor happened to approach from is not an answer a user can rely
    /// on. Position is the whole product here: an edge detected one pixel
    /// off is worse than no edge at all, because the user trusted it.
    ///
    /// The convention this fixes is that a boundary sits on the **first
    /// pixel of the new region**. Snapping both sides of a 40px-wide
    /// button therefore gives 20 and 60, and the rect drawn between them
    /// is 40 wide — the button's true width, not one pixel short.
    #[must_use]
    pub fn new(gray: &GrayImage) -> Self {
        let (w, h) = (gray.w, gray.h);
        let mut gx = vec![0u8; w * h];
        let mut gy = vec![0u8; w * h];
        // The border ring keeps its zero: a 3x3 operator has no answer
        // there, and the screen edge is not a UI edge worth snapping to.
        for y in 1..h.saturating_sub(1) {
            for x in 1..w.saturating_sub(1) {
                let at = |dx: usize, dy: usize| gray.px[(y + dy - 1) * w + (x + dx - 1)];
                let (tl, tc, tr) = (at(0, 0), at(1, 0), at(2, 0));
                let (ml, mc, mr) = (at(0, 1), at(1, 1), at(2, 1));
                let (bl, bc) = (at(0, 2), at(1, 2));
                let hx = 3.0f32.mul_add(tc - tl, 10.0f32.mul_add(mc - ml, 3.0 * (bc - bl)));
                let hy = 3.0f32.mul_add(ml - tl, 10.0f32.mul_add(mc - tc, 3.0 * (mr - tr)));
                gx[y * w + x] = quantize(hx);
                gy[y * w + x] = quantize(hy);
            }
        }
        let threshold = adaptive_threshold(&gx, &gy);
        Self {
            w,
            h,
            gx,
            gy,
            threshold,
        }
    }

    /// The strength a gradient must reach to count as an edge: the
    /// [`EDGE_PERCENT`] percentile of this frame's own gradients, floored
    /// at [`MIN_GRADIENT`].
    ///
    /// Relative, because an absolute cut that works on a light theme
    /// finds nothing on a dark one. Floored, because a nearly blank
    /// screen's 98th percentile is noise, and snapping to noise is worse
    /// than not snapping.
    #[must_use]
    pub const fn threshold(&self) -> u8 {
        self.threshold
    }

    /// Where `p` wants to go, searching `radius` pixels either way on
    /// each axis independently.
    ///
    /// A non-positive radius disables snapping outright rather than
    /// searching a degenerate window.
    #[must_use]
    pub fn snap(&self, p: Point, radius: i32) -> Snap {
        if radius <= 0 {
            return Snap::default();
        }
        Snap {
            x: self.snap_x(p, radius),
            y: self.snap_y(p, radius),
        }
    }

    /// Just the vertical-edge search, for callers that move one axis at
    /// a time — sliding a shape sideways onto an alignment, say.
    #[must_use]
    pub fn snap_x(&self, p: Point, radius: i32) -> Option<SnapHit> {
        (radius > 0).then(|| self.snap_axis(p, radius, Axis::X))?
    }

    /// Just the horizontal-edge search. See [`Self::snap_x`].
    #[must_use]
    pub fn snap_y(&self, p: Point, radius: i32) -> Option<SnapHit> {
        (radius > 0).then(|| self.snap_axis(p, radius, Axis::Y))?
    }

    fn snap_axis(&self, p: Point, radius: i32, axis: Axis) -> Option<SnapHit> {
        let (along, across) = match axis {
            Axis::X => (p.x, p.y),
            Axis::Y => (p.y, p.x),
        };
        let limit = match axis {
            Axis::X => self.w,
            Axis::Y => self.h,
        };
        let limit = i32::try_from(limit).unwrap_or(i32::MAX);
        // The scan runs one past the radius on each side so a candidate
        // at exactly the radius can still be compared against its outer
        // neighbour and recognized as a local maximum.
        let lo = (along - radius - 1).max(0);
        let hi = (along + radius + 1).min(limit - 1);
        let scores: Vec<u16> = (lo..=hi)
            .map(|v| self.score(v, across, radius, axis))
            .collect::<Vec<_>>();
        let mut best: Option<(i32, u16, i32)> = None;
        for (i, &score) in scores.iter().enumerate() {
            let v = lo + i32::try_from(i).unwrap_or(0);
            if (v - along).abs() > radius || u16::from(self.threshold) > score {
                continue;
            }
            // A wide anti-aliased edge scores highly across two or three
            // columns; without the local-maximum test the snap would
            // land on whichever of them the cursor happened to be nearer,
            // which is not a repeatable answer.
            let left = i.checked_sub(1).map_or(0, |j| scores[j]);
            let right = scores.get(i + 1).copied().unwrap_or(0);
            if score < left || score < right {
                continue;
            }
            let distance = (v - along).abs();
            let better = best.is_none_or(|(_, best_score, best_distance)| {
                // Nearest wins; a tie in distance breaks toward the
                // stronger edge, so a corner does not wobble between two
                // equidistant borders run to run. Two edges equally near
                // *and* equally strong are a real tie, and the scan runs
                // low to high, so the lower coordinate keeps it. That is
                // arbitrary but deterministic, which is the property that
                // matters — note it is also orientation-bearing: mirror
                // the image and the mirror's lower coordinate wins, which
                // is the reflection of the *other* edge.
                distance < best_distance || (distance == best_distance && score > best_score)
            });
            if better {
                best = Some((v, score, distance));
            }
        }
        let (at, _, _) = best?;
        Some(SnapHit {
            at,
            span: self.trace_span(at, across, axis),
        })
    }

    /// A column's (or row's) edge score near the query point.
    ///
    /// Two nested windows, and both are load-bearing. The inner one
    /// averages [`SCORE_HALF_SPAN`] pixels either side so an edge that
    /// survives a few pixels outscores an isolated bright one. The outer
    /// takes the **best** such average anywhere within the snap radius,
    /// which is what makes a corner reachable: approach one diagonally
    /// from outside and neither edge passes through the query's own row
    /// or column, so a score sampled only there would find nothing and
    /// the corner — the single most valuable thing to snap to — would be
    /// the one place snapping failed.
    fn score(&self, along: i32, across: i32, radius: i32, axis: Axis) -> u16 {
        let mut best = 0u16;
        for offset in -radius..=radius {
            let center = across + offset;
            let mut total = 0u32;
            let mut count = 0u32;
            for d in -SCORE_HALF_SPAN..=SCORE_HALF_SPAN {
                let Some(g) = self.gradient(along, center + d, axis) else {
                    continue;
                };
                total += u32::from(g);
                count += 1;
            }
            if count == 0 {
                continue;
            }
            best = best.max(u16::try_from(total / count).unwrap_or(u16::MAX));
        }
        best
    }

    /// How far the edge at `along` runs either side of `across`, stopping
    /// where the gradient falls below half the threshold. Half, not the
    /// threshold itself: an edge fades at its ends, and cutting at the
    /// full threshold would draw feedback visibly shorter than what the
    /// eye reads as the edge.
    fn trace_span(&self, along: i32, across: i32, axis: Axis) -> (i32, i32) {
        let floor = u16::from(self.threshold) / 2;
        let mut lo = across;
        let mut hi = across;
        for step in 1..=MAX_SPAN {
            if lo == across - step + 1
                && self
                    .gradient(along, across - step, axis)
                    .is_some_and(|g| u16::from(g) >= floor)
            {
                lo = across - step;
            }
            if hi == across + step - 1
                && self
                    .gradient(along, across + step, axis)
                    .is_some_and(|g| u16::from(g) >= floor)
            {
                hi = across + step;
            }
        }
        (lo, hi)
    }

    fn gradient(&self, along: i32, across: i32, axis: Axis) -> Option<u8> {
        let (x, y) = match axis {
            Axis::X => (along, across),
            Axis::Y => (across, along),
        };
        let x = usize::try_from(x).ok()?;
        let y = usize::try_from(y).ok()?;
        if x >= self.w || y >= self.h {
            return None;
        }
        let index = y * self.w + x;
        Some(match axis {
            Axis::X => self.gx[index],
            Axis::Y => self.gy[index],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

fn quantize(gradient: f32) -> u8 {
    // The Scharr kernel's weights sum to 16 on each side, so a full
    // black-to-white step saturates at 16.0 in luma units of [0, 1].
    let normalized = (gradient.abs() / 16.0).clamp(0.0, 1.0);
    (normalized * 255.0).round() as u8
}

/// The [`EDGE_PERCENT`] percentile of the whole frame's gradients,
/// floored at [`MIN_GRADIENT`].
///
/// Over *every* pixel, flat ones included. Excluding them would make the
/// percentile a statistic about edge strengths, and on a frame whose
/// edges are all roughly equal that lands on the strongest one and
/// rejects the rest — including the slightly-diluted score a corner
/// produces, which is precisely the case snapping exists for. Including
/// them makes it a statistic about the frame: sparse frames fall through
/// to the floor, and busy ones get a genuinely selective cut.
///
/// Histogrammed rather than sorted: the values are already `u8`, so 256
/// buckets give the exact percentile in one pass instead of sorting
/// millions of samples per frame.
fn adaptive_threshold(gx: &[u8], gy: &[u8]) -> u8 {
    let mut histogram = [0u64; 256];
    let mut total = 0u64;
    for &g in gx.iter().chain(gy) {
        histogram[g as usize] += 1;
        total += 1;
    }
    if total == 0 {
        return MIN_GRADIENT;
    }
    let target = total * EDGE_PERCENT / 100;
    let mut seen = 0u64;
    for (value, &count) in histogram.iter().enumerate() {
        seen += count;
        if seen >= target {
            return u8::try_from(value).unwrap_or(u8::MAX).max(MIN_GRADIENT);
        }
    }
    MIN_GRADIENT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark frame with a light rectangle: four crisp edges at known
    /// coordinates, which is exactly what a snap must find.
    fn button(w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> GrayImage {
        let mut px = vec![0.1f32; w * h];
        for y in y0..y1 {
            for x in x0..x1 {
                px[y * w + x] = 0.9;
            }
        }
        GrayImage { w, h, px }
    }

    fn scaled(gray: &GrayImage, factor: usize) -> GrayImage {
        let (w, h) = (gray.w * factor, gray.h * factor);
        let mut px = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = gray.px[(y / factor) * gray.w + (x / factor)];
            }
        }
        GrayImage { w, h, px }
    }

    #[test]
    fn a_corner_snaps_on_both_axes_from_any_approach() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        for (dx, dy) in [(-4, -4), (4, 4), (-4, 4), (4, -4), (0, 3), (3, 0)] {
            let snap = map.snap(Point::new(20 + dx, 15 + dy), 6);
            assert_eq!(
                snap.apply(Point::new(20 + dx, 15 + dy)),
                Point::new(20, 15),
                "approach ({dx}, {dy})"
            );
        }
    }

    #[test]
    fn every_edge_of_the_button_is_found_on_its_own_axis() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        // Left and right verticals: x snaps, y finds nothing mid-edge.
        // The right boundary is 60, not 59: a boundary sits on the first
        // pixel of the new region, so the snapped rect is 40 wide.
        for x in [20, 60] {
            let snap = map.snap(Point::new(x + 3, 30), 6);
            assert_eq!(snap.x.map(|hit| hit.at), Some(x), "vertical at {x}");
            assert_eq!(snap.y, None, "no horizontal edge at mid-height");
        }
        // Top and bottom horizontals.
        for y in [15, 45] {
            let snap = map.snap(Point::new(40, y + 3), 6);
            assert_eq!(snap.y.map(|hit| hit.at), Some(y), "horizontal at {y}");
            assert_eq!(snap.x, None, "no vertical edge at mid-width");
        }
    }

    #[test]
    fn nothing_outside_the_radius_captures_the_point() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        let far = Point::new(35, 30);
        assert_eq!(map.snap(far, 6), Snap::default());
        assert_eq!(map.snap(far, 6).apply(far), far);
    }

    #[test]
    fn a_low_contrast_edge_below_threshold_does_not_capture() {
        // A 1% luma step: present, but not something a user pointed at.
        let mut gray = GrayImage {
            w: 80,
            h: 60,
            px: vec![0.50f32; 80 * 60],
        };
        for y in 0..60 {
            for x in 30..80 {
                gray.px[y * 80 + x] = 0.51;
            }
        }
        let map = EdgeMap::new(&gray);
        assert_eq!(map.snap(Point::new(28, 30), 6).x, None);
    }

    #[test]
    fn snapping_survives_a_scale_change_landing_on_the_scaled_edge() {
        let base = button(40, 30, 10, 8, 30, 22);
        let map = EdgeMap::new(&scaled(&base, 2));
        // The left edge is at 20 in the doubled image.
        let snap = map.snap(Point::new(24, 30), 6);
        assert_eq!(snap.x.map(|hit| hit.at), Some(20));
    }

    #[test]
    fn a_flipped_image_flips_where_the_snap_lands() {
        // Deliberately off-center, or mirroring would be a no-op.
        let gray = button(80, 60, 15, 15, 45, 45);
        let mut flipped = gray.clone();
        for y in 0..60 {
            for x in 0..80 {
                flipped.px[y * 80 + x] = gray.px[y * 80 + (79 - x)];
            }
        }
        let map = EdgeMap::new(&gray);
        let mirror = EdgeMap::new(&flipped);
        let hit = map.snap(Point::new(18, 30), 6).x.expect("left edge");
        assert_eq!(hit.at, 15);
        // A snapped coordinate is a boundary, not a pixel, and the two
        // reflect differently: pixel `p` maps to `79 - p`, boundary `b`
        // to `80 - b`. The query is a candidate boundary, so it reflects
        // the second way — reflecting it as a pixel would land one off.
        let mirrored = mirror.snap(Point::new(80 - 18, 30), 6).x.expect("mirrored");
        assert_eq!(mirrored.at, 80 - hit.at);
    }

    #[test]
    fn the_reported_span_covers_the_edge_and_stops_at_its_ends() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        let hit = map.snap(Point::new(22, 30), 6).x.expect("left edge");
        let (lo, hi) = hit.span;
        assert!(lo <= 30 && hi >= 30, "span contains the query row");
        // The button spans rows 15..45; the traced edge must not run the
        // whole frame.
        assert!(lo >= 12 && hi <= 47, "span {lo}..{hi} escaped the button");
    }

    #[test]
    fn a_flat_frame_offers_nothing_and_keeps_the_floor_threshold() {
        let map = EdgeMap::new(&GrayImage {
            w: 40,
            h: 40,
            px: vec![0.4f32; 40 * 40],
        });
        assert_eq!(map.threshold(), MIN_GRADIENT);
        assert_eq!(map.snap(Point::new(20, 20), 8), Snap::default());
    }

    #[test]
    fn a_nonpositive_radius_disables_snapping() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        let on_the_edge = Point::new(21, 30);
        assert_eq!(map.snap(on_the_edge, 0), Snap::default());
        assert_eq!(map.snap(on_the_edge, -5), Snap::default());
        assert_eq!(map.snap_x(on_the_edge, 0), None);
        assert_eq!(map.snap_y(on_the_edge, -1), None);
    }

    #[test]
    fn the_per_axis_searches_agree_with_the_combined_one() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        let p = Point::new(23, 18);
        let both = map.snap(p, 6);
        assert_eq!(map.snap_x(p, 6), both.x);
        assert_eq!(map.snap_y(p, 6), both.y);
    }

    #[test]
    fn a_point_outside_the_frame_answers_without_panicking() {
        let map = EdgeMap::new(&button(80, 60, 20, 15, 60, 45));
        for p in [
            Point::new(-100, -100),
            Point::new(1000, 1000),
            Point::new(-1, 30),
            Point::new(79, 59),
        ] {
            let _ = map.snap(p, 8);
        }
    }

    #[test]
    fn a_one_pixel_frame_builds_an_empty_map() {
        let map = EdgeMap::new(&GrayImage {
            w: 1,
            h: 1,
            px: vec![0.5],
        });
        assert_eq!(map.snap(Point::new(0, 0), 4), Snap::default());
    }

    #[test]
    fn snap_applies_only_the_axes_that_hit() {
        let only_x = Snap {
            x: Some(SnapHit {
                at: 42,
                span: (0, 9),
            }),
            y: None,
        };
        assert_eq!(only_x.apply(Point::new(40, 7)), Point::new(42, 7));
        assert!(only_x.is_hit());
        assert!(!Snap::default().is_hit());
    }

    #[test]
    fn equidistant_edges_break_toward_the_stronger_one() {
        // A strong step at 25 and a weak one at 35, both 5px from the
        // query — without a deliberate tiebreak a corner would wobble
        // between them from one frame to the next.
        let mut gray = GrayImage {
            w: 60,
            h: 40,
            px: vec![0.5f32; 60 * 40],
        };
        for y in 0..40 {
            for x in 0..25 {
                gray.px[y * 60 + x] = 0.0;
            }
            for x in 35..60 {
                gray.px[y * 60 + x] = 0.6;
            }
        }
        let map = EdgeMap::new(&gray);
        let hit = map.snap(Point::new(30, 20), 6).x.expect("an edge");
        assert_eq!(hit.at, 25);
    }

    #[test]
    fn the_nearer_of_two_equal_edges_wins() {
        // A dark bar: both its edges are the same 0.5 luma step, so only
        // distance can decide.
        let mut gray = GrayImage {
            w: 60,
            h: 40,
            px: vec![0.5f32; 60 * 40],
        };
        for y in 0..40 {
            for x in 20..30 {
                gray.px[y * 60 + x] = 0.0;
            }
        }
        let map = EdgeMap::new(&gray);
        // 6 from the left edge, 4 from the right, both inside the radius.
        let hit = map.snap(Point::new(26, 20), 6).x.expect("an edge");
        assert_eq!(hit.at, 30);
        let other = map.snap(Point::new(24, 20), 6).x.expect("an edge");
        assert_eq!(other.at, 20);
    }
}
