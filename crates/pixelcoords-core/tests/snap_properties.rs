//! Property tests for edge snapping.
//!
//! The example tests beside the code check a synthetic button whose edges
//! someone worked out by hand. These check the rules that must hold for
//! any image and any query: a snap never moves a point further than the
//! radius, mirroring the image mirrors the answer, and a frame with no
//! contrast offers nothing.
//!
//! A wrong answer here is a coordinate the user believed was on an edge,
//! which is worse than no snapping at all — so the invariants are worth
//! more than the examples.

use pixelcoords_core::geometry::Point;
use pixelcoords_core::locate::GrayImage;
use pixelcoords_core::snap::EdgeMap;
use proptest::prelude::*;

const W: usize = 48;
const H: usize = 36;

/// A frame with one light rectangle on a dark field: arbitrary position
/// and size, but always a real, crisp set of edges.
fn framed_button() -> impl Strategy<Value = (GrayImage, usize, usize, usize, usize)> {
    (2usize..20, 2usize..16, 6usize..24, 6usize..16).prop_map(|(x0, y0, w, h)| {
        let (x1, y1) = ((x0 + w).min(W - 2), (y0 + h).min(H - 2));
        let mut px = vec![0.15f32; W * H];
        for y in y0..y1 {
            for x in x0..x1 {
                px[y * W + x] = 0.85;
            }
        }
        (GrayImage { w: W, h: H, px }, x0, y0, x1, y1)
    })
}

fn mirrored(gray: &GrayImage) -> GrayImage {
    let mut px = vec![0.0f32; gray.w * gray.h];
    for y in 0..gray.h {
        for x in 0..gray.w {
            px[y * gray.w + x] = gray.px[y * gray.w + (gray.w - 1 - x)];
        }
    }
    GrayImage {
        w: gray.w,
        h: gray.h,
        px,
    }
}

proptest! {
    /// The contract a user relies on: snapping refines a placement, it
    /// does not relocate it. A snap further than the radius would mean
    /// the pointer jumped somewhere the user never aimed.
    #[test]
    fn a_snap_never_moves_a_point_further_than_the_radius(
        (gray, ..) in framed_button(),
        x in 0i32..W as i32,
        y in 0i32..H as i32,
        radius in 1i32..12,
    ) {
        let map = EdgeMap::new(&gray);
        let p = Point::new(x, y);
        let snapped = map.snap(p, radius).apply(p);
        prop_assert!((snapped.x - p.x).abs() <= radius);
        prop_assert!((snapped.y - p.y).abs() <= radius);
    }

    /// Mirroring the image mirrors how far the snap reaches.
    ///
    /// Distance, not location, and that is not a weaker claim — it is the
    /// only one that is true. Two edges equally distant and equally strong
    /// are a genuine tie, and every deterministic tiebreak has an
    /// orientation: `snap_axis` takes the lower coordinate, so the direct
    /// image picks the left edge and the mirror picks *its* left edge,
    /// which is the reflection of the right one. Asserting locations
    /// would be asserting that a tiebreak is orientation-free, which no
    /// rule can be.
    ///
    /// The query reflects as `w - x`, not `w - 1 - x`: a snapped
    /// coordinate is a *boundary*, not a pixel, and the two mirror
    /// differently. Reflecting it as a pixel shifts every distance by one.
    #[test]
    fn mirroring_the_image_mirrors_how_far_the_snap_reaches(
        (gray, ..) in framed_button(),
        x in 1i32..W as i32 - 1,
        y in 1i32..H as i32 - 1,
        radius in 2i32..10,
    ) {
        let map = EdgeMap::new(&gray);
        let mirror = EdgeMap::new(&mirrored(&gray));
        let width = W as i32;
        let direct = map.snap_x(Point::new(x, y), radius);
        let reflected = mirror.snap_x(Point::new(width - x, y), radius);
        // Found in one iff found in the other — the mirror cannot make an
        // edge appear or vanish.
        prop_assert_eq!(direct.is_some(), reflected.is_some());
        prop_assert_eq!(
            direct.map(|hit| (hit.at - x).abs()),
            reflected.map(|hit| (hit.at - (width - x)).abs()),
        );
    }

    /// A horizontal edge is a horizontal edge regardless of where along
    /// it you ask: the two axes are genuinely independent searches, and
    /// the vertical answer must not drift with the horizontal query.
    #[test]
    fn the_vertical_snap_is_the_same_all_along_a_horizontal_edge(
        (gray, x0, y0, x1, _y1) in framed_button(),
        offset in 0usize..6,
        radius in 3i32..8,
    ) {
        prop_assume!(x1 > x0 + 8);
        let map = EdgeMap::new(&gray);
        let y = y0 as i32 + 2;
        let a = map.snap_y(Point::new(x0 as i32 + 4, y), radius);
        let b = map.snap_y(Point::new(x0 as i32 + 4 + offset as i32, y), radius);
        prop_assert_eq!(a.map(|hit| hit.at), b.map(|hit| hit.at));
    }

    /// No contrast, no edges — whatever the query. A uniform frame that
    /// offered a snap would be inventing one.
    #[test]
    fn a_uniform_frame_never_snaps(
        level in 0.0f32..1.0,
        x in 0i32..W as i32,
        y in 0i32..H as i32,
        radius in 1i32..16,
    ) {
        let map = EdgeMap::new(&GrayImage { w: W, h: H, px: vec![level; W * H] });
        let p = Point::new(x, y);
        prop_assert_eq!(map.snap(p, radius).apply(p), p);
    }

    /// The reported extent is well-formed and reaches the query's
    /// neighbourhood.
    ///
    /// Not "contains the query": the edge is found by scanning the whole
    /// radius, so approaching a corner from outside legitimately reports
    /// an edge whose extent starts past the query's own row. It must
    /// still come within the radius, though — a span describing an edge
    /// further away than the search looked would be feedback pointing at
    /// something the snap did not use.
    ///
    /// The earlier version of this asserted containment and passed while
    /// the code returned a *degenerate* `(q, q)` span for exactly that
    /// corner case, because a one-pixel span at the query trivially
    /// contains it. That is why this one also demands the span reach the
    /// edge rather than collapse onto the pointer.
    #[test]
    fn a_reported_span_is_well_formed_and_near_the_query(
        (gray, ..) in framed_button(),
        x in 0i32..W as i32,
        y in 0i32..H as i32,
        radius in 1i32..10,
    ) {
        let map = EdgeMap::new(&gray);
        let snap = map.snap(Point::new(x, y), radius);
        if let Some(hit) = snap.x {
            prop_assert!(hit.span.0 <= hit.span.1, "span {:?}", hit.span);
            prop_assert!(hit.span.0 <= y + radius && hit.span.1 >= y - radius);
        }
        if let Some(hit) = snap.y {
            prop_assert!(hit.span.0 <= hit.span.1, "span {:?}", hit.span);
            prop_assert!(hit.span.0 <= x + radius && hit.span.1 >= x - radius);
        }
    }

    /// A snap on a real border reports a span longer than a pixel.
    ///
    /// The guide the overlay draws is the whole point of the span, and a
    /// dot is not a guide. `framed_button` always draws borders at least
    /// six pixels long, so any edge found in one is traceable.
    #[test]
    fn a_snap_onto_a_real_border_reports_more_than_a_dot(
        (gray, ..) in framed_button(),
        x in 0i32..W as i32,
        y in 0i32..H as i32,
        radius in 2i32..10,
    ) {
        let map = EdgeMap::new(&gray);
        let snap = map.snap(Point::new(x, y), radius);
        if let Some(hit) = snap.x {
            prop_assert!(hit.span.1 > hit.span.0, "vertical span {:?}", hit.span);
        }
        if let Some(hit) = snap.y {
            prop_assert!(hit.span.1 > hit.span.0, "horizontal span {:?}", hit.span);
        }
    }
}
