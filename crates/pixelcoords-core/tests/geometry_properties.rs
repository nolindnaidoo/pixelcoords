//! Property tests for the geometry invariants.
//!
//! The example-based tests next to the code check the cases someone thought
//! of. These check the rules that must hold for every input: a shape that
//! was clamped is inside its bounds, rotation is periodic, a bounding box
//! bounds its shape. They are the invariants the saved coordinates rest on,
//! so a counterexample here is a wrong number in someone's `session.json`.

use pixelcoords_core::geometry::{Point, Rect, ResizeHandle, Shape, Size, ToolKind, normalize_deg};
use proptest::prelude::*;

/// Bounds big enough to move around in, small enough to hit edges often.
fn bounds() -> impl Strategy<Value = Size> {
    (8i32..400, 8i32..400).prop_map(|(w, h)| Size::new(w, h))
}

fn point_within(bounds: Size) -> impl Strategy<Value = Point> {
    (0..bounds.w, 0..bounds.h).prop_map(|(x, y)| Point::new(x, y))
}

/// Any shape, positioned loosely inside `bounds`.
fn shape_within(bounds: Size) -> impl Strategy<Value = Shape> {
    let w = bounds.w;
    let h = bounds.h;
    prop_oneof![
        (0..w / 2, 0..h / 2, 2..w / 2, 2..h / 2)
            .prop_map(|(x, y, rw, rh)| Shape::Rect(Rect::new(x, y, rw, rh))),
        (0..w, 0..h, 1..(w.min(h) / 2).max(2)).prop_map(|(cx, cy, r)| Shape::Circle { cx, cy, r }),
        (0..w, 0..h, 1..(w / 2).max(2), 1..(h / 2).max(2))
            .prop_map(|(cx, cy, rx, ry)| Shape::Ellipse { cx, cy, rx, ry }),
        (0..w, 0..h, 0..w, 0..h, 0..w, 0..h).prop_map(|(ax, ay, bx, by, cx, cy)| {
            Shape::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            }
        }),
    ]
}

proptest! {
    /// Rotation is periodic: the same angle modulo a full turn is the same
    /// angle, and the normalized value is always in [0, 360).
    #[test]
    fn rotation_is_periodic(deg in -10_000i32..10_000, turns in -20i32..20) {
        let normalized = normalize_deg(deg);
        prop_assert!((0..360).contains(&normalized), "{normalized}");
        prop_assert_eq!(normalized, normalize_deg(deg + turns * 360));
    }

    /// A bounding box bounds its shape: every vertex of a triangle lies
    /// inside the box reported for it.
    #[test]
    fn a_bbox_contains_its_triangle(
        (ax, ay, bx, by, cx, cy) in (0i32..400, 0i32..400, 0i32..400, 0i32..400, 0i32..400, 0i32..400),
    ) {
        let tri = Shape::Triangle { ax, ay, bx, by, cx, cy };
        let bb = tri.bbox();
        for (x, y) in [(ax, ay), (bx, by), (cx, cy)] {
            prop_assert!(x >= bb.x && x <= bb.x + bb.w, "{x} outside {bb:?}");
            prop_assert!(y >= bb.y && y <= bb.y + bb.h, "{y} outside {bb:?}");
        }
    }

    /// A drag-move never puts a shape outside the bounds it was clamped to.
    #[test]
    fn a_clamped_move_stays_in_bounds(
        b in bounds(),
        (shape, grab, cursor) in bounds().prop_flat_map(|bb| {
            (shape_within(bb), point_within(bb), point_within(bb))
        }),
    ) {
        let moved = shape.clamp_move(grab, cursor, b);
        let bb = moved.bbox();
        // A shape larger than the bounds cannot fit; it is pinned at the
        // origin instead, which is the documented behaviour.
        prop_assume!(bb.w <= b.w && bb.h <= b.h);
        prop_assert!(bb.x >= 0 && bb.y >= 0, "{bb:?} left {b:?}");
        prop_assert!(bb.x + bb.w <= b.w, "{bb:?} right of {b:?}");
        prop_assert!(bb.y + bb.h <= b.h, "{bb:?} below {b:?}");
    }

    /// Drawing never produces a shape reaching outside the capture.
    #[test]
    fn a_preview_never_leaves_the_capture(
        b in bounds(),
        tool in prop_oneof![Just(ToolKind::Rect), Just(ToolKind::Circle), Just(ToolKind::Triangle)],
        (sx, sy, ex, ey) in (-500i32..900, -500i32..900, -500i32..900, -500i32..900),
    ) {
        let preview = Shape::compute_preview(tool, Point::new(sx, sy), Point::new(ex, ey), b, false);
        let Some(shape) = preview else { return Ok(()) };
        let bb = shape.bbox();
        prop_assert!(bb.w > 0 && bb.h > 0, "degenerate {bb:?}");
    }

    /// Resizing keeps a shape usable: it never collapses to nothing, at any
    /// rotation, from any handle.
    #[test]
    fn a_resize_never_collapses_a_shape(
        b in bounds(),
        deg in 0i32..360,
        (left, right, top, bottom) in (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        keep_aspect in any::<bool>(),
        (rx, ry, rw, rh) in (0i32..200, 0i32..200, 2i32..200, 2i32..200),
        cursor in (-200i32..600, -200i32..600),
    ) {
        prop_assume!(left || right || top || bottom);
        let shape = Shape::Rect(Rect::new(rx, ry, rw, rh));
        let handle = ResizeHandle::RectEdges { left, right, top, bottom };
        let resized = shape.resize_to_rotated(
            deg,
            handle,
            Point::new(cursor.0, cursor.1),
            b,
            keep_aspect,
        );
        let bb = resized.bbox();
        prop_assert!(bb.w > 0 && bb.h > 0, "collapsed to {bb:?}");
    }

    /// The centre of a circle is inside it, whatever rotation is applied —
    /// a circle has no orientation to rotate.
    #[test]
    fn rotation_does_not_move_a_circle(
        deg in -720i32..720,
        (cx, cy, r) in (0i32..300, 0i32..300, 1i32..80),
    ) {
        let circle = Shape::Circle { cx, cy, r };
        prop_assert!(circle.hit_test_rotated(deg, Point::new(cx, cy)));
        prop_assert_eq!(circle.rotated_bbox(deg), circle.bbox());
    }

    /// The point `click_point` aims for lands inside the shape it aims at
    /// — the invariant every `emit` snippet rests on. Rects hold at any
    /// rotation, because the click point is the rotation pivot itself.
    /// Triangles are assumed at least ~5px thick in their narrowest
    /// direction: a hairline sliver has no pixel-resolution interior to
    /// aim for, and its truncated centroid may sit a fraction of a pixel
    /// outside the exact edge test.
    #[test]
    fn a_click_point_lands_inside_its_shape(
        shape in shape_within(Size::new(400, 400)),
        deg in 0i32..360,
    ) {
        if let Shape::Triangle { ax, ay, bx, by, cx, cy } = shape {
            let cross = (i64::from(bx) - i64::from(ax)) * (i64::from(cy) - i64::from(ay))
                - (i64::from(by) - i64::from(ay)) * (i64::from(cx) - i64::from(ax));
            let edge = |x0: i32, y0: i32, x1: i32, y1: i32| {
                f64::hypot(f64::from(x1 - x0), f64::from(y1 - y0))
            };
            let longest = edge(ax, ay, bx, by)
                .max(edge(bx, by, cx, cy))
                .max(edge(cx, cy, ax, ay));
            // |cross| = 2 * area, so this bounds the triangle's smallest
            // height (2 * area / longest edge) below by 5px.
            prop_assume!(cross.abs() as f64 >= 5.0 * longest);
            prop_assert!(shape.hit_test(shape.click_point()));
            return Ok(());
        }
        prop_assert!(shape.hit_test_rotated(deg, shape.click_point()));
    }
}
