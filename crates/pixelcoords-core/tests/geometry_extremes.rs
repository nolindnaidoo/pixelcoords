//! Nothing in `geometry` panics anywhere inside its documented domain.
//!
//! `MAX_COORD` is a promise, and this is what holds the module to it.
//! Every public entry point is called at the extremes of that range —
//! both signs of the bound, and zero — because that is where the bugs
//! were: six sites subtracted in `i32` and widened afterward, so the
//! author's intent to be careful was one operation late.
//!
//! Deliberately *not* the whole `i32` range. Making the module total over
//! all of `i32` means saturating everywhere, which manufactures the
//! confident-but-wrong coordinate that `overflow-checks = true` exists to
//! prevent. The bound is enforced once, where untrusted input enters
//! (`SessionFile::validate`), and everything downstream may assume it.
//!
//! These assert that nothing panics, not what the answer is. A shape a
//! million pixels wide has no meaningful click point; it has a
//! *terminating* one.

use pixelcoords_core::geometry::{
    Line, MAX_COORD, Point, Rect, ResizeHandle, Shape, Size, ToolKind, normalize_deg,
    regular_polygon, simplify_path,
};
use proptest::prelude::*;

/// Weighted toward the edges of the domain, because the middle was never
/// the problem.
fn coord() -> impl Strategy<Value = i32> {
    prop_oneof![
        3 => -MAX_COORD..=MAX_COORD,
        1 => Just(MAX_COORD),
        1 => Just(-MAX_COORD),
        1 => Just(0),
    ]
}

fn point() -> impl Strategy<Value = Point> {
    (coord(), coord()).prop_map(|(x, y)| Point::new(x, y))
}

/// Polygon vertices use the same range as everything else.
///
/// They did not, for one release: `Poly::click_point` scanned its whole
/// bounding box, so a single generated case at the edge of the domain
/// took this suite from three seconds to over two hundred. The scan is
/// gone, and this is the check that it stayed gone.
fn poly_point() -> impl Strategy<Value = Point> {
    point()
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (coord(), coord(), coord(), coord())
            .prop_map(|(x, y, w, h)| Shape::Rect(Rect::new(x, y, w, h))),
        (coord(), coord(), coord()).prop_map(|(cx, cy, r)| Shape::Circle { cx, cy, r }),
        (coord(), coord(), coord(), coord()).prop_map(|(cx, cy, rx, ry)| Shape::Ellipse {
            cx,
            cy,
            rx,
            ry
        }),
        (coord(), coord(), coord(), coord(), coord(), coord()).prop_map(
            |(ax, ay, bx, by, cx, cy)| Shape::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy
            }
        ),
        proptest::collection::vec(poly_point(), 0..8).prop_map(|points| Shape::Poly { points }),
    ]
}

fn handle() -> impl Strategy<Value = ResizeHandle> {
    prop_oneof![
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
            |(left, right, top, bottom)| ResizeHandle::RectEdges {
                left,
                right,
                top,
                bottom
            }
        ),
        Just(ResizeHandle::CircleRadius),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn shape_queries_survive_the_domain(s in shape(), p in point(), deg in -100_000i32..100_000) {
        let _ = s.bbox();
        let _ = s.click_point();
        let _ = s.hit_test(p);
        let _ = s.hit_test_rotated(deg, p);
        let _ = s.rotated_bbox(deg);
        let _ = s.grab_origin();
        let _ = s.grab_origin_rotated(deg);
        let _ = normalize_deg(deg);
    }

    #[test]
    fn moving_and_resizing_survive_the_domain(
        s in shape(),
        grab in point(),
        cursor in point(),
        region in (coord(), coord(), 1i32..=MAX_COORD, 1i32..=MAX_COORD)
            .prop_map(|(x, y, w, h)| Rect::new(x, y, w, h)),
        h in handle(),
        deg in -100_000i32..100_000,
        lock in any::<bool>(),
    ) {
        let _ = s.clamp_move(grab, cursor, region);
        let _ = s.clamp_move_rotated(deg, grab, cursor, region);
        let _ = s.resize_to(h, cursor, region, lock);
        let _ = s.resize_to_rotated(deg, h, cursor, region, lock);
        let _ = region.clamp_point(cursor);
        let _ = region.contains(cursor);
    }

    #[test]
    fn line_queries_survive_the_domain(
        a in point(),
        b in point(),
        p in point(),
        tolerance in 0i32..=MAX_COORD,
    ) {
        let line = Line::new(a, b);
        let _ = line.delta();
        let _ = line.bbox();
        let _ = line.length();
        let _ = line.angle_deg();
        let _ = line.distance_to(p);
        let _ = line.hit_test(p, tolerance);
        let _ = line.endpoint_grab(p, tolerance);
        let _ = line.constrained();
        let _ = line.translated(0, 0);
    }

    /// Length and angle widen to `f64` before subtracting, so unlike
    /// `delta` they stay exact even where an `i32` difference cannot.
    #[test]
    fn length_and_angle_stay_finite_where_delta_saturates(a in point(), b in point()) {
        let line = Line::new(a, b);
        prop_assert!(line.length().is_finite());
        let angle = line.angle_deg();
        prop_assert!(angle.is_finite());
        prop_assert!((0.0..360.0).contains(&angle), "{angle}");
    }

    #[test]
    fn construction_helpers_survive_the_domain(
        center in point(),
        toward in point(),
        sides in 0u32..1_000,
        tool in prop_oneof![
            Just(ToolKind::Rect),
            Just(ToolKind::Ellipse),
            Just(ToolKind::Triangle),
            Just(ToolKind::Polygon),
            Just(ToolKind::Freehand),
            Just(ToolKind::Measure),
        ],
        bounds in (1i32..100_000, 1i32..100_000).prop_map(|(w, h)| Size::new(w, h)),
        shift in any::<bool>(),
        path in proptest::collection::vec(poly_point(), 0..12),
    ) {
        let _ = regular_polygon(center, toward, sides);
        let _ = Shape::compute_preview(
            tool,
            center,
            toward,
            Rect::new(0, 0, bounds.w, bounds.h),
            shift,
        );
        let _ = simplify_path(&path, 2.0);
    }
}
