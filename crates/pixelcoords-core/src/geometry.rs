//! Shapes and their interaction math, in monitor-local physical pixels.
//!
//! Ported from the predecessor's rectangle/circle tools; the drag semantics
//! (preview normalization, grab-offset moves, clamp-to-bounds) are preserved
//! so existing muscle memory carries over.
//!
//! # Domain
//!
//! Coordinates are `i32`, but the *domain* is [`MAX_COORD`] — see its docs
//! for why the bound exists rather than being total over `i32`. Within it,
//! nothing here panics and every operation is bounded in time;
//! `tests/geometry_extremes.rs` holds that to the whole surface. Outside
//! it, behavior is unspecified: an operation may panic under
//! `overflow-checks`, and `Poly::click_point`'s fallback scan grows with
//! the bounding box.
//!
//! Callers reading a session get this for free — `SessionFile::validate`
//! refuses anything out of range at the load seam, so a coordinate that
//! reaches this module has already been checked.

use serde::{Deserialize, Serialize};

/// The largest absolute value any coordinate or extent may carry.
///
/// A million pixels is more than an order of magnitude beyond the widest
/// desktop anyone assembles, and small enough that every difference, sum,
/// and product here stays far from an integer limit. Both halves matter:
/// the first means no real screen is ever refused, the second means input
/// inside the bound cannot overflow the arithmetic.
///
/// **Why a documented domain rather than saturating everything.** The
/// workspace sets `overflow-checks = true` in release on the reasoning
/// that a wrapped coordinate is silently wrong data in a file someone
/// feeds to automation, and that a crash is the better failure. Making
/// every operation total over all of `i32` means saturating, which
/// produces exactly the confident-but-wrong number that decision rejects.
/// A bound that real input never approaches, enforced once where untrusted
/// input enters, keeps the honest failure and costs nothing.
///
/// The cost of the bound is bounded too: at the very edge,
/// `Poly::click_point`'s worst-case scan takes about 25ms, and on a real
/// screen it is under a millisecond.
pub const MAX_COORD: i32 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.w && p.y < self.y + self.h
    }

    /// The nearest point inside this rect.
    ///
    /// The upper bounds are inclusive-exclusive to match `contains`, so a
    /// clamped point always satisfies it — a zero-sized rect is the one
    /// exception, and it clamps to the origin corner.
    #[must_use]
    pub fn clamp_point(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.x, self.x + (self.w - 1).max(0)),
            p.y.clamp(self.y, self.y + (self.h - 1).max(0)),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Rect,
    /// Legacy records only: the drawing tool is `Ellipse` now, and a
    /// circle is an ellipse with equal radii. Old sessions still parse.
    Circle,
    Ellipse,
    Triangle,
    /// The regular-N-gon drawing tool; its records store as `poly`.
    Polygon,
    /// The freehand drawing tool; its records store as `poly`.
    Freehand,
    /// The two-point ruler. Never appears in a `SelectionRecord` — it is
    /// only ever the *active tool*, and what it produces lands in the
    /// session's `measures` array instead.
    Measure,
    /// What polygon and freehand records are tagged as: one stored kind,
    /// one consumer code path, however the vertices were authored.
    Poly,
}

impl ToolKind {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Rect => Self::Ellipse,
            Self::Circle | Self::Ellipse => Self::Triangle,
            Self::Triangle => Self::Polygon,
            Self::Polygon => Self::Freehand,
            Self::Freehand => Self::Measure,
            Self::Measure | Self::Poly => Self::Rect,
        }
    }
}

/// A resize grip on a shape's border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeHandle {
    /// Dragging the rim of a circle: radius follows the cursor.
    CircleRadius,
    /// Dragging one or two rect edges; the others stay anchored.
    RectEdges {
        left: bool,
        right: bool,
        top: bool,
        bottom: bool,
    },
}

/// A committed or in-progress selection shape.
///
/// Serializes untagged: a rect is `{x, y, w, h}`, a circle is `{cx, cy, r}`,
/// a triangle is its three vertices `{ax, ay, bx, by, cx, cy}` (apex, then
/// base-left, then base-right as drawn — though any triangle is
/// representable). The field sets are disjoint, so deserialization is
/// unambiguous; the session schema also stores the discriminant in a
/// sibling `shape` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Shape {
    Rect(Rect),
    Circle {
        cx: i32,
        cy: i32,
        r: i32,
    },
    /// Axis-aligned ellipse; rotation, like a rect's, is metadata. The
    /// field set is disjoint from every other variant, so the untagged
    /// serde representation stays unambiguous.
    Ellipse {
        cx: i32,
        cy: i32,
        rx: i32,
        ry: i32,
    },
    Triangle {
        ax: i32,
        ay: i32,
        bx: i32,
        by: i32,
        cx: i32,
        cy: i32,
    },
    /// An arbitrary closed polygon — regular N-gons and freehand paths
    /// alike. Rotation is baked into the vertices, triangle-style, and
    /// the winding may be either direction.
    Poly {
        points: Vec<Point>,
    },
}

impl Shape {
    /// Shape previewed while dragging from `start` to `current`, or `None`
    /// while the drag is still degenerate. `current` is clamped into bounds
    /// so dragging off-screen keeps the preview on-screen.
    /// `lock` constrains the proportions (Shift held): an ellipse locks
    /// to a perfect circle.
    pub fn compute_preview(
        tool: ToolKind,
        start: Point,
        current: Point,
        region: Rect,
        lock: bool,
    ) -> Option<Self> {
        // The drawable region is not always the whole frame. In `--target`
        // mode it is the window's rect within the monitor, and `start` has
        // already been rejected outside that region — so the preview only
        // has to keep `current` from wandering out.
        let cx = current.x.clamp(region.x, region.x + region.w - 1);
        let cy = current.y.clamp(region.y, region.y + region.h - 1);
        match tool {
            ToolKind::Rect | ToolKind::Triangle | ToolKind::Ellipse => {
                let x = start.x.min(cx);
                let y = start.y.min(cy);
                let w = (start.x - cx).abs();
                let h = (start.y - cy).abs();
                if w <= 1 || h <= 1 {
                    return None;
                }
                let bbox = Rect::new(x, y, w, h);
                Some(match tool {
                    ToolKind::Rect => Self::Rect(bbox),
                    ToolKind::Ellipse => ellipse_in_box(bbox, lock),
                    _ => triangle_in_box(bbox),
                })
            }
            ToolKind::Circle => {
                let dx = f64::from(start.x - cx);
                let dy = f64::from(start.y - cy);
                let r = dx.hypot(dy) as i32;
                if r <= 0 {
                    return None;
                }
                Some(Self::Circle {
                    cx: start.x,
                    cy: start.y,
                    r,
                })
            }
            // The polygon and freehand tools build their previews in the
            // app (they need side counts and accumulated paths this
            // stateless helper cannot know); `Poly` is a record tag, not
            // a drawing tool. `Measure` draws a `Line`, which is not a
            // `Shape` at all — see `SelectionSet::add_measure`.
            ToolKind::Polygon | ToolKind::Freehand | ToolKind::Poly | ToolKind::Measure => None,
        }
    }

    pub const fn kind(&self) -> ToolKind {
        match self {
            Self::Rect(_) => ToolKind::Rect,
            Self::Circle { .. } => ToolKind::Circle,
            Self::Ellipse { .. } => ToolKind::Ellipse,
            Self::Triangle { .. } => ToolKind::Triangle,
            Self::Poly { .. } => ToolKind::Poly,
        }
    }

    /// Axis-aligned bounding box. Saturating math so absurd deserialized
    /// values (e.g. `r` near `i32::MAX`) misreport rather than panic.
    pub fn bbox(&self) -> Rect {
        match *self {
            Self::Poly { ref points } => {
                let mut x0 = i32::MAX;
                let mut y0 = i32::MAX;
                let mut x1 = i32::MIN;
                let mut y1 = i32::MIN;
                for p in points {
                    x0 = x0.min(p.x);
                    y0 = y0.min(p.y);
                    x1 = x1.max(p.x);
                    y1 = y1.max(p.y);
                }
                if points.is_empty() {
                    return Rect::new(0, 0, 0, 0);
                }
                Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
            }
            Self::Rect(r) => r,
            Self::Ellipse { cx, cy, rx, ry } => Rect::new(
                cx.saturating_sub(rx),
                cy.saturating_sub(ry),
                rx.saturating_mul(2),
                ry.saturating_mul(2),
            ),
            Self::Circle { cx, cy, r } => Rect::new(
                cx.saturating_sub(r),
                cy.saturating_sub(r),
                r.saturating_mul(2),
                r.saturating_mul(2),
            ),
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => {
                let x0 = min3(ax, bx, cx);
                let y0 = min3(ay, by, cy);
                Rect::new(
                    x0,
                    y0,
                    max3(ax, bx, cx).saturating_sub(x0),
                    max3(ay, by, cy).saturating_sub(y0),
                )
            }
        }
    }

    /// Whether `p` lies inside the shape (used for cursor hit-testing).
    /// Distance math is done in i64 so extreme coordinates cannot overflow.
    pub fn hit_test(&self, p: Point) -> bool {
        match *self {
            Self::Poly { ref points } => point_in_poly(points, p),
            Self::Rect(r) => r.contains(p),
            Self::Ellipse { cx, cy, rx, ry } => {
                // Normalized quadratic in i128: (dx*ry)^2 + (dy*rx)^2 <=
                // (rx*ry)^2, boundary inclusive like the circle's test.
                let dx = i128::from(p.x) - i128::from(cx);
                let dy = i128::from(p.y) - i128::from(cy);
                let rx = i128::from(rx);
                let ry = i128::from(ry);
                dx * dx * ry * ry + dy * dy * rx * rx <= rx * rx * ry * ry
            }
            Self::Circle { cx, cy, r } => {
                let dx = i64::from(p.x - cx);
                let dy = i64::from(p.y - cy);
                dx * dx + dy * dy <= i64::from(r) * i64::from(r)
            }
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => {
                // A degenerate (zero-area) triangle covers nothing — without
                // this, the sign test below reports the whole plane inside.
                if cross(cx, cy, ax, ay, bx, by) == 0 {
                    return false;
                }
                // Sign-of-cross-product test, edges inclusive: p is inside
                // unless it is strictly on both sides of the edge set.
                let d1 = cross(p.x, p.y, ax, ay, bx, by);
                let d2 = cross(p.x, p.y, bx, by, cx, cy);
                let d3 = cross(p.x, p.y, cx, cy, ax, ay);
                let has_neg = d1 < 0 || d2 < 0 || d3 < 0;
                let has_pos = d1 > 0 || d2 > 0 || d3 > 0;
                !(has_neg && has_pos)
            }
        }
    }

    /// Whether the shape covers pixel (`x`, `y`) — identical to `hit_test`,
    /// named separately because it is the crop/mask predicate.
    pub fn covers(&self, x: i32, y: i32) -> bool {
        self.hit_test(Point::new(x, y))
    }

    /// The point a click should aim for: the bbox center for rects (the
    /// rotation pivot, so it holds for rotated rects unchanged), a
    /// circle's center, and the centroid for triangles — always interior,
    /// where a thin diagonal triangle's bbox center may fall outside.
    /// i64 arithmetic so extreme deserialized vertices cannot overflow.
    pub fn click_point(&self) -> Point {
        match *self {
            Self::Poly { ref points } => poly_interior_point(points),
            Self::Rect(_) => self.pivot(),
            Self::Circle { cx, cy, .. } | Self::Ellipse { cx, cy, .. } => Point::new(cx, cy),
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => Point::new(
                ((i64::from(ax) + i64::from(bx) + i64::from(cx)) / 3) as i32,
                ((i64::from(ay) + i64::from(by) + i64::from(cy)) / 3) as i32,
            ),
        }
    }

    /// The reference point a drag-move grabs: bbox origin, or a circle's
    /// center.
    pub fn grab_origin(&self) -> Point {
        match *self {
            Self::Rect(r) => Point::new(r.x, r.y),
            Self::Circle { cx, cy, .. } | Self::Ellipse { cx, cy, .. } => Point::new(cx, cy),
            Self::Triangle { .. } | Self::Poly { .. } => {
                let b = self.bbox();
                Point::new(b.x, b.y)
            }
        }
    }

    /// New shape position for a drag-move, clamped so the shape cannot leave
    /// `bounds`. `grab_offset` is cursor-at-grab minus `grab_origin`.
    #[must_use]
    pub fn clamp_move(&self, grab_offset: Point, cursor: Point, region: Rect) -> Self {
        // The drawable region may be a subrect of the frame (in `--target`
        // mode it is the window's rect). Every extent that used to be
        // `[0, bounds]` is now `[region.origin, region.origin + region.size]`.
        let right = region.x + region.w;
        let bottom = region.y + region.h;
        match *self {
            Self::Rect(rect) => {
                let nx = (cursor.x - grab_offset.x).clamp(region.x, (right - rect.w).max(region.x));
                let ny =
                    (cursor.y - grab_offset.y).clamp(region.y, (bottom - rect.h).max(region.y));
                Self::Rect(Rect::new(nx, ny, rect.w, rect.h))
            }
            Self::Circle { r, .. } => {
                let min_x = region.x + r.max(0);
                let min_y = region.y + r.max(0);
                let cx = (cursor.x - grab_offset.x).clamp(min_x, (right - r).max(min_x));
                let cy = (cursor.y - grab_offset.y).clamp(min_y, (bottom - r).max(min_y));
                Self::Circle { cx, cy, r }
            }
            Self::Ellipse { rx, ry, .. } => {
                let min_x = region.x + rx.max(0);
                let min_y = region.y + ry.max(0);
                let cx = (cursor.x - grab_offset.x).clamp(min_x, (right - rx).max(min_x));
                let cy = (cursor.y - grab_offset.y).clamp(min_y, (bottom - ry).max(min_y));
                Self::Ellipse { cx, cy, rx, ry }
            }
            Self::Triangle { .. } | Self::Poly { .. } => {
                let b = self.bbox();
                let nx = (cursor.x - grab_offset.x).clamp(region.x, (right - b.w).max(region.x));
                let ny = (cursor.y - grab_offset.y).clamp(region.y, (bottom - b.h).max(region.y));
                self.translated(nx - b.x, ny - b.y)
            }
        }
    }

    /// Which resize handle, if any, `p` grabs: the rim of a circle, or an
    /// edge/corner of a rect, within `tolerance` pixels on either side of
    /// the border.
    pub fn resize_grab(&self, p: Point, tolerance: i32) -> Option<ResizeHandle> {
        let tolerance = tolerance.max(1);
        match *self {
            Self::Circle { cx, cy, r } => {
                let dist = f64::from(p.x - cx).hypot(f64::from(p.y - cy));
                let on_rim = (dist - f64::from(r)).abs() <= f64::from(tolerance);
                on_rim.then_some(ResizeHandle::CircleRadius)
            }
            // Rects grab their own border; triangles grab their bounding
            // box's border (the same frame the resize scales them in).
            Self::Rect(rect) => box_border_grab(rect, p, tolerance),
            Self::Ellipse { .. } | Self::Triangle { .. } | Self::Poly { .. } => {
                box_border_grab(self.bbox(), p, tolerance)
            }
        }
    }

    /// The shape resized by dragging `handle` to `cursor` (clamped into
    /// `bounds`), anchored on the parts not being dragged: a circle keeps
    /// its center, a rect keeps its ungrabbed edges. Dimensions never drop
    /// below 2, so a resize can't destroy a shape.
    ///
    /// `keep_aspect` (Shift held) preserves `self`'s width:height ratio —
    /// the ratio at drag start, so it stays stable through the whole drag.
    /// On a corner the opposite corner anchors and the dominant cursor axis
    /// sets the scale; on a single edge the perpendicular axis scales with
    /// it, centered. Circles are inherently proportional and ignore it.
    #[must_use]
    pub fn resize_to(
        &self,
        handle: ResizeHandle,
        cursor: Point,
        region: Rect,
        keep_aspect: bool,
    ) -> Self {
        let clamped = Point::new(
            cursor.x.clamp(region.x, region.x + region.w - 1),
            cursor.y.clamp(region.y, region.y + region.h - 1),
        );
        self.resize_to_local(handle, clamped, region, keep_aspect)
    }

    /// `resize_to` without the cursor-to-bounds clamp — used by the rotated
    /// path, where the cursor is clamped in the *visual* frame before being
    /// inverse-rotated into this local one.
    #[must_use]
    fn resize_to_local(
        &self,
        handle: ResizeHandle,
        clamped: Point,
        region: Rect,
        keep_aspect: bool,
    ) -> Self {
        const MIN: i32 = 2;
        match (self.clone(), handle) {
            (Self::Circle { cx, cy, .. }, ResizeHandle::CircleRadius) => {
                let r = f64::from(clamped.x - cx).hypot(f64::from(clamped.y - cy)) as i32;
                Self::Circle {
                    cx,
                    cy,
                    r: r.max(MIN),
                }
            }
            (
                Self::Rect(rect),
                ResizeHandle::RectEdges {
                    left,
                    right,
                    top,
                    bottom,
                },
            ) => Self::Rect(resize_box(
                rect,
                (left, right, top, bottom),
                clamped,
                region,
                keep_aspect,
            )),
            (
                ell @ Self::Ellipse { .. },
                ResizeHandle::RectEdges {
                    left,
                    right,
                    top,
                    bottom,
                },
            ) => {
                // The ellipse rides its bounding box: resize the box like a
                // rect, then re-inscribe.
                let bb = resize_box(
                    ell.bbox(),
                    (left, right, top, bottom),
                    clamped,
                    region,
                    keep_aspect,
                );
                ellipse_in_box(bb, false)
            }
            (
                poly @ Self::Poly { .. },
                ResizeHandle::RectEdges {
                    left,
                    right,
                    top,
                    bottom,
                },
            ) => {
                let old = poly.bbox();
                let new = resize_box(
                    old,
                    (left, right, top, bottom),
                    clamped,
                    region,
                    keep_aspect,
                );
                scale_into_box(&poly, old, new)
            }
            (
                tri @ Self::Triangle { .. },
                ResizeHandle::RectEdges {
                    left,
                    right,
                    top,
                    bottom,
                },
            ) => {
                let old = tri.bbox();
                let new = resize_box(
                    old,
                    (left, right, top, bottom),
                    clamped,
                    region,
                    keep_aspect,
                );
                tri.mapped_between_boxes(old, new)
            }
            // Handle/shape mismatch cannot arise from grab-then-resize; be
            // inert rather than panic.
            (shape, _) => shape,
        }
    }

    /// The shape with its vertices affinely remapped from bbox `old` to
    /// bbox `new` — how triangles scale under a bbox resize.
    #[must_use]
    fn mapped_between_boxes(&self, old: Rect, new: Rect) -> Self {
        let map_x = |v: i32| {
            new.x
                + (f64::from(v - old.x) * f64::from(new.w) / f64::from(old.w.max(1))).round() as i32
        };
        let map_y = |v: i32| {
            new.y
                + (f64::from(v - old.y) * f64::from(new.h) / f64::from(old.h.max(1))).round() as i32
        };
        match self.clone() {
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => Self::Triangle {
                ax: map_x(ax),
                ay: map_y(ay),
                bx: map_x(bx),
                by: map_y(by),
                cx: map_x(cx),
                cy: map_y(cy),
            },
            other => other,
        }
    }

    /// The same shape translated by (`dx`, `dy`) — used to derive global
    /// desktop coordinates from monitor-local ones.
    #[must_use]
    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        match *self {
            Self::Poly { ref points } => Self::Poly {
                points: points
                    .iter()
                    .map(|p| Point::new(p.x + dx, p.y + dy))
                    .collect(),
            },
            Self::Rect(r) => Self::Rect(Rect::new(r.x + dx, r.y + dy, r.w, r.h)),
            Self::Circle { cx, cy, r } => Self::Circle {
                cx: cx + dx,
                cy: cy + dy,
                r,
            },
            Self::Ellipse { cx, cy, rx, ry } => Self::Ellipse {
                cx: cx + dx,
                cy: cy + dy,
                rx,
                ry,
            },
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => Self::Triangle {
                ax: ax + dx,
                ay: ay + dy,
                bx: bx + dx,
                by: by + dy,
                cx: cx + dx,
                cy: cy + dy,
            },
        }
    }
}

/// Degrees normalized into `0..360`.
pub fn normalize_deg(deg: i32) -> i32 {
    deg.rem_euclid(360)
}

/// Scale a vertex shape from `old` box into `new` box, vertex by vertex —
/// the same mapping triangles use, for any point list.
fn scale_into_box(shape: &Shape, old: Rect, new: Rect) -> Shape {
    let map_x = |v: i32| {
        new.x + (f64::from(v - old.x) * f64::from(new.w) / f64::from(old.w.max(1))).round() as i32
    };
    let map_y = |v: i32| {
        new.y + (f64::from(v - old.y) * f64::from(new.h) / f64::from(old.h.max(1))).round() as i32
    };
    match shape {
        Shape::Poly { points } => Shape::Poly {
            points: points
                .iter()
                .map(|p| Point::new(map_x(p.x), map_y(p.y)))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Edge-inclusive point-in-polygon: on any edge counts as inside; else
/// even-odd ray crossing. Exact integer arithmetic throughout.
fn point_in_poly(points: &[Point], p: Point) -> bool {
    if points.len() < 3 {
        return false;
    }
    let n = points.len();
    let mut inside = false;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        if on_segment(a, b, p) {
            return true;
        }
        // Even-odd crossing of the horizontal ray to +x.
        if (a.y > p.y) != (b.y > p.y) {
            let cross = i64::from(b.x - a.x) * i64::from(p.y - a.y)
                - i64::from(b.y - a.y) * i64::from(p.x - a.x);
            let crosses = if b.y > a.y { cross > 0 } else { cross < 0 };
            if crosses {
                inside = !inside;
            }
        }
    }
    inside
}

/// Whether `p` lies exactly on segment `a`..`b`.
fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross =
        i64::from(b.x - a.x) * i64::from(p.y - a.y) - i64::from(b.y - a.y) * i64::from(p.x - a.x);
    cross == 0
        && p.x >= a.x.min(b.x)
        && p.x <= a.x.max(b.x)
        && p.y >= a.y.min(b.y)
        && p.y <= a.y.max(b.y)
}

/// A point guaranteed inside the polygon when any pixel is: the vertex
/// average when it lands inside (cheap, common), else the first covered
/// pixel of a bbox scan. Concave freehand shapes are exactly why the
/// fallback exists.
fn poly_interior_point(points: &[Point]) -> Point {
    if points.is_empty() {
        return Point::new(0, 0);
    }
    let n = points.len() as i64;
    let sx: i64 = points.iter().map(|p| i64::from(p.x)).sum();
    let sy: i64 = points.iter().map(|p| i64::from(p.y)).sum();
    let mean = Point::new((sx / n) as i32, (sy / n) as i32);
    if point_in_poly(points, mean) {
        return mean;
    }
    let shape = Shape::Poly {
        points: points.to_vec(),
    };
    let bb = shape.bbox();
    for y in bb.y..=bb.y.saturating_add(bb.h) {
        for x in bb.x..=bb.x.saturating_add(bb.w) {
            if point_in_poly(points, Point::new(x, y)) {
                return Point::new(x, y);
            }
        }
    }
    mean
}

/// Fewer than three points is not a polygon. A geometric floor, not a
/// preference.
pub const MIN_POLYGON_SIDES: u32 = 3;

/// The most sides a regular polygon may have.
///
/// Not a judgement about how many anyone should want — a thousand-sided
/// polygon inscribed in any real screen already has several vertices per
/// pixel, so past this the extra points are indistinguishable from the
/// circle they approximate. The bound exists because `sides` becomes a
/// point-per-vertex allocation, and an unbounded count is an allocation
/// request rather than a shape.
///
/// The overlay reaches 3 to 9, because those are the digit keys. That is
/// the keyboard's reach, not this limit.
pub const MAX_POLYGON_SIDES: u32 = 1_000;

/// The regular `sides`-gon centered on `center` with its first vertex at
/// `toward` — dragging both sizes and orients it in one gesture.
///
/// `sides` outside [`MIN_POLYGON_SIDES`]`..=`[`MAX_POLYGON_SIDES`] is
/// clamped into it. Documented rather than silent: the previous ceiling
/// was 12, which nothing said and nothing could reach, so a caller asking
/// for 100 got a dodecagon and no indication why.
pub fn regular_polygon(center: Point, toward: Point, sides: u32) -> Shape {
    let sides = sides.clamp(MIN_POLYGON_SIDES, MAX_POLYGON_SIDES) as usize;
    let dx = f64::from(toward.x) - f64::from(center.x);
    let dy = f64::from(toward.y) - f64::from(center.y);
    let r = dx.hypot(dy);
    let base = dy.atan2(dx);
    let step = std::f64::consts::TAU / sides as f64;
    let points = (0..sides)
        .map(|i| {
            let a = base + step * i as f64;
            Point::new(
                f64::from(center.x).mul_add(1.0, r * a.cos()).round() as i32,
                f64::from(center.y).mul_add(1.0, r * a.sin()).round() as i32,
            )
        })
        .collect();
    Shape::Poly { points }
}

/// Ramer–Douglas–Peucker path simplification: keep the points that
/// matter, drop the mouse jitter. `epsilon` is the maximum distance a
/// dropped point may sit from the simplified path.
pub fn simplify_path(points: &[Point], epsilon: f64) -> Vec<Point> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let (mut worst, mut worst_dist) = (start, -1.0f64);
        for (i, p) in points.iter().enumerate().take(end).skip(start + 1) {
            let d = point_segment_distance(*p, points[start], points[end]);
            if d > worst_dist {
                worst = i;
                worst_dist = d;
            }
        }
        if worst_dist > epsilon {
            keep[worst] = true;
            stack.push((start, worst));
            stack.push((worst, end));
        }
    }
    points
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(p, _)| *p)
        .collect()
}

/// Euclidean distance from `p` to segment `a`..`b`.
fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (px, py) = (f64::from(p.x), f64::from(p.y));
    let (ax, ay) = (f64::from(a.x), f64::from(a.y));
    let (bx, by) = (f64::from(b.x), f64::from(b.y));
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    if len2 <= f64::EPSILON {
        return (px - ax).hypot(py - ay);
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// The ellipse inscribed in `bbox`; `lock` makes it the inscribed circle
/// centered in the box.
fn ellipse_in_box(bbox: Rect, lock: bool) -> Shape {
    let cx = bbox.x + bbox.w / 2;
    let cy = bbox.y + bbox.h / 2;
    let (rx, ry) = (bbox.w / 2, bbox.h / 2);
    if lock {
        let r = rx.min(ry).max(1);
        return Shape::Ellipse {
            cx,
            cy,
            rx: r,
            ry: r,
        };
    }
    Shape::Ellipse {
        cx,
        cy,
        rx: rx.max(1),
        ry: ry.max(1),
    }
}

/// `p` rotated by `deg` degrees (clockwise, screen coordinates) about
/// `center`, rounded to the pixel grid.
pub fn rotate_point_about(p: Point, center: Point, deg: i32) -> Point {
    let rad = f64::from(deg).to_radians();
    let (sin, cos) = rad.sin_cos();
    // All arithmetic in f64: extreme deserialized coordinates saturate at
    // the final cast instead of overflowing i32 on the way.
    let dx = f64::from(p.x) - f64::from(center.x);
    let dy = f64::from(p.y) - f64::from(center.y);
    Point::new(
        (f64::from(center.x) + (dx * cos - dy * sin).round()) as i32,
        (f64::from(center.y) + (dx * sin + dy * cos).round()) as i32,
    )
}

impl Shape {
    /// The pivot every rotation turns around: the unrotated bbox center.
    pub fn pivot(&self) -> Point {
        let b = self.bbox();
        Point::new(b.x.saturating_add(b.w / 2), b.y.saturating_add(b.h / 2))
    }

    /// AABB of the shape after rotating it `deg` about its pivot.
    pub fn rotated_bbox(&self, deg: i32) -> Rect {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.bbox();
        }
        let b = self.bbox();
        let pivot = self.pivot();
        let (bx1, by1) = (b.x.saturating_add(b.w), b.y.saturating_add(b.h));
        let corners = [
            Point::new(b.x, b.y),
            Point::new(bx1, b.y),
            Point::new(b.x, by1),
            Point::new(bx1, by1),
        ]
        .map(|c| rotate_point_about(c, pivot, deg));
        let x0 = corners.iter().map(|c| c.x).min().unwrap_or(b.x);
        let y0 = corners.iter().map(|c| c.y).min().unwrap_or(b.y);
        let x1 = corners.iter().map(|c| c.x).max().unwrap_or(bx1);
        let y1 = corners.iter().map(|c| c.y).max().unwrap_or(by1);
        Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    /// `hit_test`/`covers` for the shape rotated `deg` about its pivot:
    /// the point is inverse-rotated into the shape's local space.
    pub fn hit_test_rotated(&self, deg: i32, p: Point) -> bool {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.hit_test(p);
        }
        self.hit_test(rotate_point_about(p, self.pivot(), -deg))
    }

    /// `resize_grab` in the rotated frame: the cursor is inverse-rotated,
    /// so grips sit on the shape as the user sees it.
    pub fn resize_grab_rotated(&self, deg: i32, p: Point, tolerance: i32) -> Option<ResizeHandle> {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.resize_grab(p, tolerance);
        }
        self.resize_grab(rotate_point_about(p, self.pivot(), -deg), tolerance)
    }

    /// `resize_to` in the rotated frame; rotation itself is unchanged.
    #[must_use]
    pub fn resize_to_rotated(
        &self,
        deg: i32,
        handle: ResizeHandle,
        cursor: Point,
        region: Rect,
        keep_aspect: bool,
    ) -> Self {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.resize_to(handle, cursor, region, keep_aspect);
        }
        // Clamp in the visual frame (where the cursor actually lives), THEN
        // inverse-rotate — clamping the local-frame point instead makes the
        // cursor stop tracking near region edges.
        let visual = Point::new(
            cursor.x.clamp(region.x, region.x + region.w - 1),
            cursor.y.clamp(region.y, region.y + region.h - 1),
        );
        let local = rotate_point_about(visual, self.pivot(), -deg);
        self.resize_to_local(handle, local, region, keep_aspect)
    }

    /// `clamp_move` keeping the *rotated* silhouette on screen.
    #[must_use]
    pub fn clamp_move_rotated(
        &self,
        deg: i32,
        grab_offset: Point,
        cursor: Point,
        region: Rect,
    ) -> Self {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.clamp_move(grab_offset, cursor, region);
        }
        let bb = self.rotated_bbox(deg);
        let right = region.x.saturating_add(region.w);
        let bottom = region.y.saturating_add(region.h);
        let nx = cursor
            .x
            .saturating_sub(grab_offset.x)
            .clamp(region.x, right.saturating_sub(bb.w).max(region.x));
        let ny = cursor
            .y
            .saturating_sub(grab_offset.y)
            .clamp(region.y, bottom.saturating_sub(bb.h).max(region.y));
        self.translated(nx.saturating_sub(bb.x), ny.saturating_sub(bb.y))
    }

    /// The grab reference for a rotated move: the rotated AABB origin.
    pub fn grab_origin_rotated(&self, deg: i32) -> Point {
        if normalize_deg(deg) == 0 || matches!(self, Self::Circle { .. }) {
            return self.grab_origin();
        }
        let bb = self.rotated_bbox(deg);
        Point::new(bb.x, bb.y)
    }

    /// A triangle with the rotation baked into its vertices (exact, single
    /// rounding); other shapes are returned unchanged — rects carry their
    /// rotation as metadata instead.
    #[must_use]
    pub fn with_rotation_baked(&self, deg: i32) -> Self {
        if let Self::Poly { points } = self {
            if normalize_deg(deg) == 0 {
                return self.clone();
            }
            let pivot = self.pivot();
            return Self::Poly {
                points: points
                    .iter()
                    .map(|p| rotate_point_about(*p, pivot, deg))
                    .collect(),
            };
        }
        match self.clone() {
            Self::Triangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } if normalize_deg(deg) != 0 => {
                let pivot = self.pivot();
                let a = rotate_point_about(Point::new(ax, ay), pivot, deg);
                let b = rotate_point_about(Point::new(bx, by), pivot, deg);
                let c = rotate_point_about(Point::new(cx, cy), pivot, deg);
                Self::Triangle {
                    ax: a.x,
                    ay: a.y,
                    bx: b.x,
                    by: b.y,
                    cx: c.x,
                    cy: c.y,
                }
            }
            other => other,
        }
    }
}

/// The isoceles triangle inscribed in `bbox`: apex top-center, flat base.
const fn triangle_in_box(bbox: Rect) -> Shape {
    Shape::Triangle {
        ax: bbox.x + bbox.w / 2,
        ay: bbox.y,
        bx: bbox.x,
        by: bbox.y + bbox.h,
        cx: bbox.x + bbox.w,
        cy: bbox.y + bbox.h,
    }
}

/// Cross product of (b - a) x (p - a) in i64: the side of segment a->b
/// that p lies on.
const fn cross(px: i32, py: i32, ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
    let abx = bx as i64 - ax as i64;
    let aby = by as i64 - ay as i64;
    let apx = px as i64 - ax as i64;
    let apy = py as i64 - ay as i64;
    abx * apy - aby * apx
}

const fn min3(a: i32, b: i32, c: i32) -> i32 {
    if a <= b && a <= c {
        return a;
    }
    if b <= c {
        return b;
    }
    c
}

const fn max3(a: i32, b: i32, c: i32) -> i32 {
    if a >= b && a >= c {
        return a;
    }
    if b >= c {
        return b;
    }
    c
}

/// The border-grab test for an axis-aligned box (used by rects directly and
/// by triangles via their bbox).
fn box_border_grab(rect: Rect, p: Point, tolerance: i32) -> Option<ResizeHandle> {
    let (x1, y1) = (rect.x + rect.w, rect.y + rect.h);
    let within_x = p.x >= rect.x - tolerance && p.x <= x1 + tolerance;
    let within_y = p.y >= rect.y - tolerance && p.y <= y1 + tolerance;
    let left_d = (p.x - rect.x).abs();
    let right_d = (p.x - x1).abs();
    let top_d = (p.y - rect.y).abs();
    let bottom_d = (p.y - y1).abs();
    let mut left = left_d <= tolerance && within_y;
    let mut right = right_d <= tolerance && within_y;
    let mut top = top_d <= tolerance && within_x;
    let mut bottom = bottom_d <= tolerance && within_x;
    // A box narrower than the tolerance band grabs the nearer edge, never
    // both.
    if left && right {
        right = right_d < left_d;
        left = !right;
    }
    if top && bottom {
        bottom = bottom_d < top_d;
        top = !bottom;
    }
    let grabbed = left || right || top || bottom;
    grabbed.then_some(ResizeHandle::RectEdges {
        left,
        right,
        top,
        bottom,
    })
}

/// Resize an axis-aligned box by dragging the given edges to `clamped`,
/// optionally keeping `rect`'s original aspect ratio. Shared by rect and
/// triangle resizing.
fn resize_box(
    rect: Rect,
    (left, right, top, bottom): (bool, bool, bool, bool),
    clamped: Point,
    region: Rect,
    keep_aspect: bool,
) -> Rect {
    const MIN: i32 = 2;
    let mut x0 = rect.x;
    let mut x1 = rect.x + rect.w;
    let mut y0 = rect.y;
    let mut y1 = rect.y + rect.h;
    if left {
        x0 = clamped.x.min(x1 - MIN);
    }
    if right {
        x1 = clamped.x.max(x0 + MIN);
    }
    if top {
        y0 = clamped.y.min(y1 - MIN);
    }
    if bottom {
        y1 = clamped.y.max(y0 + MIN);
    }
    if keep_aspect && rect.w >= MIN && rect.h >= MIN {
        let (w0, h0) = (f64::from(rect.w), f64::from(rect.h));
        // Dispatch on which axes are grabbed: corner, horizontal edge, or
        // vertical edge.
        match (left || right, top || bottom) {
            (true, true) => {
                // Corner: dominant axis sets the scale, capped so the
                // locked box never leaves the region.
                let mut s = (f64::from(x1 - x0) / w0).max(f64::from(y1 - y0) / h0);
                let region_right = region.x + region.w;
                let region_bottom = region.y + region.h;
                let avail_w = if left {
                    x1 - region.x
                } else {
                    region_right - x0
                };
                let avail_h = if top {
                    y1 - region.y
                } else {
                    region_bottom - y0
                };
                s = s.min(f64::from(avail_w) / w0).min(f64::from(avail_h) / h0);
                let w = ((w0 * s).round() as i32).max(MIN);
                let h = ((h0 * s).round() as i32).max(MIN);
                (x0, x1) = if left { (x1 - w, x1) } else { (x0, x0 + w) };
                (y0, y1) = if top { (y1 - h, y1) } else { (y0, y0 + h) };
            }
            (true, false) => {
                // Horizontal edge: height follows proportionally, centered
                // on where the box was.
                let h = ((f64::from(x1 - x0) * h0 / w0).round() as i32)
                    .max(MIN)
                    .min(region.h);
                let center_y = rect.y + rect.h / 2;
                y0 = (center_y - h / 2).clamp(region.y, region.y + region.h - h);
                y1 = y0 + h;
            }
            (false, _) => {
                let w = ((f64::from(y1 - y0) * w0 / h0).round() as i32)
                    .max(MIN)
                    .min(region.w);
                let center_x = rect.x + rect.w / 2;
                x0 = (center_x - w / 2).clamp(region.x, region.x + region.w - w);
                x1 = x0 + w;
            }
        }
    }
    let (w, h) = (x1 - x0, y1 - y0);
    if rect.w >= MIN && rect.h >= MIN {
        // Normal case: edges land where the (clamped) cursor put them and
        // anchored edges never move. Under rotation the local box may
        // legitimately exceed the visual bounds — clamping it here would
        // drift the anchor.
        return Rect::new(x0, y0, w, h);
    }
    // Sub-MIN input box only: the MIN floor can push it past a screen
    // edge; shift it back inside without shrinking.
    let x0 = x0.clamp(region.x, (region.x + region.w - w).max(region.x));
    let y0 = y0.clamp(region.y, (region.y + region.h - h).max(region.y));
    Rect::new(x0, y0, w, h)
}

/// A two-point measurement: a ruler laid on the frozen image.
///
/// Not a [`Shape`]. A measure has no interior, no crop, no cutout
/// contribution and no click point, so putting it through the selection
/// path would make `assert`, `emit`, `find`, and the crop writer each
/// grow a special case for a thing none of them can answer about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub a: Point,
    pub b: Point,
}

impl Line {
    #[must_use]
    pub const fn new(a: Point, b: Point) -> Self {
        Self { a, b }
    }

    /// Signed horizontal and vertical extent, `b - a`.
    ///
    /// Saturating, because the answer is an `i32` and the true difference
    /// between two `i32`s need not be one. Screen coordinates never come
    /// close, but a session is a file a human can edit, and a bounded
    /// number beats a panic when the input was already meaningless.
    /// [`Self::length`] and [`Self::angle_deg`] do not go through this —
    /// they widen to `f64` first and stay exact at any input.
    #[must_use]
    pub const fn delta(self) -> (i32, i32) {
        (
            self.b.x.saturating_sub(self.a.x),
            self.b.y.saturating_sub(self.a.y),
        )
    }

    /// Euclidean length in pixels.
    #[must_use]
    pub fn length(self) -> f64 {
        let (dx, dy) = self.delta_f64();
        dx.hypot(dy)
    }

    /// The delta in `f64`, which holds any difference of two `i32`s
    /// exactly. The integer [`Self::delta`] saturates; this does not.
    fn delta_f64(self) -> (f64, f64) {
        (
            f64::from(self.b.x) - f64::from(self.a.x),
            f64::from(self.b.y) - f64::from(self.a.y),
        )
    }

    /// Direction in degrees within `[0, 360)`, `0` pointing right along
    /// +X and increasing **clockwise**.
    ///
    /// Clockwise because screen Y grows downward: a ruler dragged
    /// visually down-and-right has to read as a positive angle, which the
    /// mathematical convention would report as negative.
    ///
    /// A zero-length measure has no direction; it reports `0.0` rather
    /// than the NaN `atan2(0, 0)` would be entitled to.
    #[must_use]
    pub fn angle_deg(self) -> f64 {
        let (dx, dy) = self.delta_f64();
        if dx == 0.0 && dy == 0.0 {
            return 0.0;
        }
        let deg = dy.atan2(dx).to_degrees();
        if deg < 0.0 { deg + 360.0 } else { deg }
    }

    /// The smallest rect containing both endpoints — what the caption
    /// placement needs, since a line has no `Shape::bbox`.
    #[must_use]
    pub fn bbox(self) -> Rect {
        let x = self.a.x.min(self.b.x);
        let y = self.a.y.min(self.b.y);
        Rect::new(
            x,
            y,
            i32::try_from(self.a.x.abs_diff(self.b.x)).unwrap_or(i32::MAX),
            i32::try_from(self.a.y.abs_diff(self.b.y)).unwrap_or(i32::MAX),
        )
    }

    #[must_use]
    pub const fn translated(self, dx: i32, dy: i32) -> Self {
        Self::new(
            Point::new(self.a.x + dx, self.a.y + dy),
            Point::new(self.b.x + dx, self.b.y + dy),
        )
    }

    /// The endpoint `p` grabs, if either is within `tolerance`.
    ///
    /// `a` wins a tie: the two coincide only on a zero-length measure,
    /// where the choice cannot matter, and preferring one keeps the pick
    /// deterministic.
    #[must_use]
    pub fn endpoint_grab(self, p: Point, tolerance: i32) -> Option<bool> {
        let near = |q: Point| {
            let (dx, dy) = (i64::from(p.x - q.x), i64::from(p.y - q.y));
            dx * dx + dy * dy <= i64::from(tolerance) * i64::from(tolerance)
        };
        if near(self.a) {
            return Some(true);
        }
        near(self.b).then_some(false)
    }

    /// Whether `p` is within `tolerance` of the segment — the whole-line
    /// grab, used after the endpoints have had their chance.
    #[must_use]
    pub fn hit_test(self, p: Point, tolerance: i32) -> bool {
        self.distance_to(p) <= f64::from(tolerance)
    }

    /// Shortest distance from `p` to the segment, clamped at the ends so
    /// a point beyond `b` measures to `b` rather than to the infinite
    /// line through it.
    #[must_use]
    pub fn distance_to(self, p: Point) -> f64 {
        let (dx, dy) = self.delta();
        let (dx, dy) = (f64::from(dx), f64::from(dy));
        let len_sq = dx.mul_add(dx, dy * dy);
        let (px, py) = (f64::from(p.x - self.a.x), f64::from(p.y - self.a.y));
        if len_sq <= f64::EPSILON {
            return px.hypot(py);
        }
        let t = px.mul_add(dx, py * dy) / len_sq;
        let t = t.clamp(0.0, 1.0);
        (px - t * dx).hypot(py - t * dy)
    }

    /// `b` snapped to the nearest horizontal, vertical, or 45° direction
    /// from `a` — what Shift does while dragging, matching the constraint
    /// the other tools apply.
    ///
    /// The length along the chosen direction is preserved as the
    /// projection of the free endpoint onto it, so the ruler tracks the
    /// pointer instead of jumping to a fixed radius.
    #[must_use]
    pub fn constrained(self) -> Self {
        // Eight directions, 45° apart, as unit vectors.
        const AXES: [(f64, f64); 8] = [
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ),
            (
                std::f64::consts::FRAC_1_SQRT_2,
                -std::f64::consts::FRAC_1_SQRT_2,
            ),
            (
                -std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ),
            (
                -std::f64::consts::FRAC_1_SQRT_2,
                -std::f64::consts::FRAC_1_SQRT_2,
            ),
        ];
        let (dx, dy) = self.delta();
        if dx == 0 && dy == 0 {
            return self;
        }
        let (fx, fy) = (f64::from(dx), f64::from(dy));
        let mut best = (f64::NEG_INFINITY, 0.0, 0.0);
        for (ax, ay) in AXES {
            let projection = fx.mul_add(ax, fy * ay);
            if projection > best.0 {
                best = (projection, ax, ay);
            }
        }
        let (projection, ax, ay) = best;
        let projection = projection.max(0.0);
        Self::new(
            self.a,
            Point::new(
                self.a.x + (projection * ax).round() as i32,
                self.a.y + (projection * ay).round() as i32,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_delta_are_the_plain_arithmetic() {
        let line = Line::new(Point::new(10, 20), Point::new(40, 60));
        assert_eq!(line.delta(), (30, 40));
        assert!((line.length() - 50.0).abs() < 1e-9, "3-4-5 triangle");
    }

    #[test]
    fn length_is_invariant_under_translation() {
        let line = Line::new(Point::new(-5, 7), Point::new(11, -3));
        let moved = line.translated(1000, -400);
        assert!((line.length() - moved.length()).abs() < 1e-9);
        assert_eq!(line.delta(), moved.delta());
    }

    #[test]
    fn angle_is_clockwise_from_positive_x() {
        // Screen Y grows downward, so "down" must read as +90, not -90.
        let at = |dx, dy| Line::new(Point::new(0, 0), Point::new(dx, dy)).angle_deg();
        assert!((at(10, 0) - 0.0).abs() < 1e-9, "right");
        assert!((at(0, 10) - 90.0).abs() < 1e-9, "down");
        assert!((at(-10, 0) - 180.0).abs() < 1e-9, "left");
        assert!((at(0, -10) - 270.0).abs() < 1e-9, "up");
        assert!((at(10, 10) - 45.0).abs() < 1e-9, "down-right");
    }

    #[test]
    fn angle_is_antisymmetric_under_endpoint_swap() {
        // The invariant the issue names: angle(A,B) == (angle(B,A) + 180) % 360.
        for (ax, ay, bx, by) in [
            (0, 0, 10, 0),
            (3, 7, -11, 2),
            (-5, -5, 5, 5),
            (100, -20, 100, 40),
        ] {
            let ab = Line::new(Point::new(ax, ay), Point::new(bx, by)).angle_deg();
            let ba = Line::new(Point::new(bx, by), Point::new(ax, ay)).angle_deg();
            let expected = (ba + 180.0) % 360.0;
            assert!((ab - expected).abs() < 1e-9, "{ab} vs {expected}");
        }
    }

    #[test]
    fn a_zero_length_measure_has_no_direction_rather_than_nan() {
        let dot = Line::new(Point::new(4, 4), Point::new(4, 4));
        assert!((dot.length() - 0.0).abs() < f64::EPSILON);
        assert!(dot.angle_deg().is_finite(), "atan2(0,0) must not escape");
        assert!((dot.angle_deg() - 0.0).abs() < f64::EPSILON);
        // And it is still grabbable, so a mis-drag can be deleted.
        assert!(dot.hit_test(Point::new(4, 4), 6));
    }

    #[test]
    fn distance_clamps_at_the_ends_rather_than_using_the_infinite_line() {
        let line = Line::new(Point::new(0, 0), Point::new(100, 0));
        // Beside the middle: perpendicular distance.
        assert!((line.distance_to(Point::new(50, 10)) - 10.0).abs() < 1e-9);
        // Past b: measured to b, not to the line through it, which would
        // report 0 for a point far off the end.
        assert!((line.distance_to(Point::new(200, 0)) - 100.0).abs() < 1e-9);
        assert!((line.distance_to(Point::new(-30, 40)) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn grabbing_prefers_an_endpoint_then_the_segment() {
        let line = Line::new(Point::new(0, 0), Point::new(100, 0));
        assert_eq!(line.endpoint_grab(Point::new(2, 2), 6), Some(true), "a");
        assert_eq!(line.endpoint_grab(Point::new(98, 1), 6), Some(false), "b");
        assert_eq!(line.endpoint_grab(Point::new(50, 0), 6), None, "middle");
        assert!(line.hit_test(Point::new(50, 3), 6), "still on the line");
        assert!(!line.hit_test(Point::new(50, 40), 6));
    }

    #[test]
    fn shift_snaps_to_the_eight_directions_and_tracks_the_pointer() {
        let from = Point::new(100, 100);
        // Slightly off horizontal snaps flat, keeping the horizontal reach.
        let nearly = Line::new(from, Point::new(200, 104)).constrained();
        assert_eq!(nearly.b.y, 100, "snapped to horizontal");
        assert!((nearly.length() - 100.0).abs() < 1.0, "reach preserved");

        // Slightly off the diagonal snaps to 45 degrees.
        let diag = Line::new(from, Point::new(160, 172)).constrained();
        assert!(
            ((diag.b.x - from.x) - (diag.b.y - from.y)).abs() <= 1,
            "equal legs: {diag:?}"
        );
        assert!((diag.angle_deg() - 45.0).abs() < 1.0);

        // Upward-left still works: the constraint is eight-way, not four.
        let up_left = Line::new(from, Point::new(30, 26)).constrained();
        assert!((up_left.angle_deg() - 225.0).abs() < 1.0, "{up_left:?}");
    }

    #[test]
    fn constraining_a_zero_length_measure_leaves_it_alone() {
        let dot = Line::new(Point::new(9, 9), Point::new(9, 9));
        assert_eq!(dot.constrained(), dot);
    }

    const BOUNDS: Size = Size::new(1920, 1080);
    const BOUNDS_RECT: Rect = Rect::new(0, 0, BOUNDS.w, BOUNDS.h);

    #[test]
    fn rect_preview_normalizes_inverted_drag() {
        let s = Shape::compute_preview(
            ToolKind::Rect,
            Point::new(100, 200),
            Point::new(40, 50),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(s, Some(Shape::Rect(Rect::new(40, 50, 60, 150))));
    }

    #[test]
    fn rect_preview_clamps_cursor_to_bounds() {
        let s = Shape::compute_preview(
            ToolKind::Rect,
            Point::new(1900, 1000),
            Point::new(5000, 5000),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(s, Some(Shape::Rect(Rect::new(1900, 1000, 19, 79))));
    }

    #[test]
    fn rect_preview_degenerate_is_none() {
        assert_eq!(
            Shape::compute_preview(
                ToolKind::Rect,
                Point::new(10, 10),
                Point::new(10, 300),
                BOUNDS_RECT,
                false
            ),
            None
        );
        assert_eq!(
            Shape::compute_preview(
                ToolKind::Rect,
                Point::new(10, 10),
                Point::new(10, 10),
                BOUNDS_RECT,
                false
            ),
            None
        );
    }

    #[test]
    fn circle_preview_radius_is_distance() {
        let s = Shape::compute_preview(
            ToolKind::Circle,
            Point::new(100, 100),
            Point::new(103, 104),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(
            s,
            Some(Shape::Circle {
                cx: 100,
                cy: 100,
                r: 5
            })
        );
    }

    #[test]
    fn circle_preview_zero_radius_is_none() {
        assert_eq!(
            Shape::compute_preview(
                ToolKind::Circle,
                Point::new(7, 7),
                Point::new(7, 7),
                BOUNDS_RECT,
                false
            ),
            None
        );
    }

    #[test]
    fn rect_hit_test_edges() {
        let s = Shape::Rect(Rect::new(10, 10, 20, 20));
        assert!(s.hit_test(Point::new(10, 10)));
        assert!(s.hit_test(Point::new(29, 29)));
        assert!(!s.hit_test(Point::new(30, 30)));
        assert!(!s.hit_test(Point::new(9, 10)));
    }

    #[test]
    fn circle_hit_test_boundary_inclusive() {
        let s = Shape::Circle {
            cx: 0,
            cy: 0,
            r: 10,
        };
        assert!(s.hit_test(Point::new(10, 0)));
        assert!(s.hit_test(Point::new(6, 8)));
        assert!(!s.hit_test(Point::new(8, 8)));
    }

    #[test]
    fn circle_hit_test_survives_extreme_coords() {
        let s = Shape::Circle { cx: 0, cy: 0, r: 5 };
        assert!(!s.hit_test(Point::new(i32::MAX, i32::MAX)));
    }

    #[test]
    fn bbox_of_circle() {
        let s = Shape::Circle {
            cx: 50,
            cy: 60,
            r: 10,
        };
        assert_eq!(s.bbox(), Rect::new(40, 50, 20, 20));
    }

    #[test]
    fn rect_clamp_move_never_escapes_bounds() {
        let s = Shape::Rect(Rect::new(0, 0, 300, 200));
        let grab = Point::new(0, 0);
        for cx in [-500, 0, 960, 5000] {
            for cy in [-500, 0, 540, 5000] {
                let Shape::Rect(r) = s.clamp_move(grab, Point::new(cx, cy), BOUNDS_RECT) else {
                    panic!("rect stayed rect");
                };
                assert!(r.x >= 0 && r.y >= 0, "({cx},{cy}) gave {r:?}");
                assert!(
                    r.x + r.w <= BOUNDS.w && r.y + r.h <= BOUNDS.h,
                    "({cx},{cy}) gave {r:?}"
                );
            }
        }
    }

    #[test]
    fn circle_clamp_move_never_escapes_bounds() {
        let s = Shape::Circle {
            cx: 500,
            cy: 500,
            r: 40,
        };
        let grab = Point::new(0, 0);
        for cx in [-500, 0, 960, 5000] {
            for cy in [-500, 0, 540, 5000] {
                let Shape::Circle {
                    cx: ncx,
                    cy: ncy,
                    r,
                } = s.clamp_move(grab, Point::new(cx, cy), BOUNDS_RECT)
                else {
                    panic!("circle stayed circle");
                };
                assert!(
                    ncx - r >= 0 && ncy - r >= 0,
                    "({cx},{cy}) gave center ({ncx},{ncy})"
                );
                assert!(
                    ncx + r <= BOUNDS.w && ncy + r <= BOUNDS.h,
                    "({cx},{cy}) gave center ({ncx},{ncy})"
                );
            }
        }
    }

    #[test]
    fn oversized_circle_clamp_is_stable() {
        // Circle larger than the window: clamps to the r-pinned position
        // instead of oscillating or going negative (predecessor behavior).
        let s = Shape::Circle {
            cx: 100,
            cy: 100,
            r: 2000,
        };
        let moved = s.clamp_move(Point::new(0, 0), Point::new(0, 0), BOUNDS_RECT);
        assert_eq!(
            moved,
            Shape::Circle {
                cx: 2000,
                cy: 2000,
                r: 2000
            }
        );
    }

    #[test]
    fn translated_shifts_both_kinds() {
        assert_eq!(
            Shape::Rect(Rect::new(1, 2, 3, 4)).translated(10, 20),
            Shape::Rect(Rect::new(11, 22, 3, 4))
        );
        assert_eq!(
            Shape::Circle { cx: 1, cy: 2, r: 3 }.translated(10, 20),
            Shape::Circle {
                cx: 11,
                cy: 22,
                r: 3
            }
        );
    }

    #[test]
    fn circle_rim_grab_within_tolerance_only() {
        let s = Shape::Circle {
            cx: 100,
            cy: 100,
            r: 50,
        };
        assert_eq!(
            s.resize_grab(Point::new(153, 100), 5),
            Some(ResizeHandle::CircleRadius)
        );
        assert_eq!(
            s.resize_grab(Point::new(147, 100), 5),
            Some(ResizeHandle::CircleRadius)
        );
        assert_eq!(s.resize_grab(Point::new(100, 100), 5), None); // center
        assert_eq!(s.resize_grab(Point::new(160, 100), 5), None); // far outside
    }

    #[test]
    fn rect_edge_and_corner_grabs() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        assert_eq!(
            s.resize_grab(Point::new(100, 150), 5),
            Some(ResizeHandle::RectEdges {
                left: true,
                right: false,
                top: false,
                bottom: false
            })
        );
        assert_eq!(
            s.resize_grab(Point::new(302, 150), 5), // just outside right edge
            Some(ResizeHandle::RectEdges {
                left: false,
                right: true,
                top: false,
                bottom: false
            })
        );
        assert_eq!(
            s.resize_grab(Point::new(298, 202), 5), // bottom-right corner
            Some(ResizeHandle::RectEdges {
                left: false,
                right: true,
                top: false,
                bottom: true
            })
        );
        assert_eq!(s.resize_grab(Point::new(200, 150), 5), None); // interior
        assert_eq!(s.resize_grab(Point::new(90, 150), 5), None); // outside band
    }

    #[test]
    fn tiny_rect_grabs_nearer_edge_not_both() {
        let s = Shape::Rect(Rect::new(100, 100, 6, 6));
        let Some(ResizeHandle::RectEdges { left, right, .. }) =
            s.resize_grab(Point::new(101, 103), 5)
        else {
            panic!("expected an edge grab");
        };
        assert!(left && !right);
    }

    #[test]
    fn circle_resize_follows_cursor_distance() {
        let s = Shape::Circle {
            cx: 100,
            cy: 100,
            r: 50,
        };
        let resized = s.resize_to(
            ResizeHandle::CircleRadius,
            Point::new(100, 180),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(
            resized,
            Shape::Circle {
                cx: 100,
                cy: 100,
                r: 80
            }
        );
        // Collapsing onto the center clamps to the minimum, not zero.
        let tiny = s.resize_to(
            ResizeHandle::CircleRadius,
            Point::new(100, 100),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(
            tiny,
            Shape::Circle {
                cx: 100,
                cy: 100,
                r: 2
            }
        );
    }

    #[test]
    fn rect_corner_resize_anchors_opposite_corner() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        let resized = s.resize_to(handle, Point::new(400, 300), BOUNDS_RECT, false);
        assert_eq!(resized, Shape::Rect(Rect::new(100, 100, 300, 200)));
    }

    #[test]
    fn rect_edge_resize_moves_one_axis_only() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        let resized = s.resize_to(handle, Point::new(50, 999), BOUNDS_RECT, false);
        assert_eq!(resized, Shape::Rect(Rect::new(50, 100, 250, 100)));
    }

    #[test]
    fn rect_resize_cannot_invert_or_vanish() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        // Dragging the left edge far past the right edge stops at MIN width.
        let resized = s.resize_to(handle, Point::new(500, 150), BOUNDS_RECT, false);
        assert_eq!(resized, Shape::Rect(Rect::new(298, 100, 2, 100)));
    }

    #[test]
    fn resize_cursor_is_clamped_to_bounds() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let resized = s.resize_to(handle, Point::new(99_999, 150), BOUNDS_RECT, false);
        assert_eq!(
            resized,
            Shape::Rect(Rect::new(100, 100, BOUNDS.w - 1 - 100, 100))
        );
    }

    #[test]
    fn locked_corner_resize_keeps_ratio_dominant_axis_wins() {
        // 2:1 rect, drag the bottom-right corner. Cursor asks for 300x200;
        // height is the dominant scale (2x), so the result is 400x200.
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let corner = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        let resized = s.resize_to(corner, Point::new(400, 300), BOUNDS_RECT, true);
        assert_eq!(resized, Shape::Rect(Rect::new(100, 100, 400, 200)));
    }

    #[test]
    fn locked_corner_resize_anchors_the_opposite_corner() {
        // Dragging the top-left corner keeps (x1, y1) fixed.
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let corner = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: true,
            bottom: false,
        };
        let resized = s.resize_to(corner, Point::new(0, 80), BOUNDS_RECT, true);
        let Shape::Rect(r) = resized else {
            panic!("still a rect")
        };
        assert_eq!((r.x + r.w, r.y + r.h), (300, 200), "anchor moved");
        assert_eq!(r.w * 100, r.h * 200, "ratio drifted: {r:?}");
    }

    #[test]
    fn locked_corner_resize_caps_scale_at_bounds() {
        // Anchored at (100, 100) with a 2:1 ratio on a 1920x1080 canvas:
        // width hits the right edge first (1820/200 = 9.1x vs 980/100 =
        // 9.8x), so the scale caps there and the rect stays inside.
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let corner = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        let resized = s.resize_to(
            corner,
            Point::new(BOUNDS_RECT.w - 1, BOUNDS_RECT.h - 1),
            BOUNDS_RECT,
            true,
        );
        let Shape::Rect(r) = resized else {
            panic!("still a rect")
        };
        assert!(
            r.x + r.w <= BOUNDS.w && r.y + r.h <= BOUNDS.h,
            "escaped: {r:?}"
        );
        assert_eq!(r.w, BOUNDS.w - 100);
        assert_eq!(r.w, 2 * r.h);
    }

    #[test]
    fn locked_edge_resize_scales_other_axis_centered() {
        // Dragging the right edge to double the width also doubles the
        // height, centered on the original vertical middle.
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let edge = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let resized = s.resize_to(edge, Point::new(500, 150), BOUNDS_RECT, true);
        assert_eq!(resized, Shape::Rect(Rect::new(100, 50, 400, 200)));
    }

    #[test]
    fn locked_edge_resize_clamps_centered_axis_to_bounds() {
        // A rect near the top: the proportional height growth would go
        // negative, so it shifts down to stay on screen.
        let s = Shape::Rect(Rect::new(100, 10, 200, 100));
        let edge = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let resized = s.resize_to(edge, Point::new(500, 60), BOUNDS_RECT, true);
        let Shape::Rect(r) = resized else {
            panic!("still a rect")
        };
        assert_eq!((r.w, r.h), (400, 200));
        assert_eq!(r.y, 0, "clamped to the top edge");
    }

    #[test]
    fn locked_circle_resize_is_unchanged_by_lock() {
        let s = Shape::Circle {
            cx: 100,
            cy: 100,
            r: 50,
        };
        let unlocked = s.resize_to(
            ResizeHandle::CircleRadius,
            Point::new(100, 180),
            BOUNDS_RECT,
            false,
        );
        let locked = s.resize_to(
            ResizeHandle::CircleRadius,
            Point::new(100, 180),
            BOUNDS_RECT,
            true,
        );
        assert_eq!(unlocked, locked);
    }

    #[test]
    fn mismatched_handle_is_inert() {
        let s = Shape::Circle { cx: 5, cy: 5, r: 5 };
        let handle = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        assert_eq!(
            s.resize_to(handle, Point::new(50, 50), BOUNDS_RECT, false),
            s
        );
    }

    #[test]
    fn ellipse_preview_inscribes_the_drag_box_and_shift_locks_a_circle() {
        let free = Shape::compute_preview(
            ToolKind::Ellipse,
            Point::new(10, 10),
            Point::new(50, 30),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(
            free,
            Some(Shape::Ellipse {
                cx: 30,
                cy: 20,
                rx: 20,
                ry: 10,
            })
        );
        let locked = Shape::compute_preview(
            ToolKind::Ellipse,
            Point::new(10, 10),
            Point::new(50, 30),
            BOUNDS_RECT,
            true,
        );
        assert_eq!(
            locked,
            Some(Shape::Ellipse {
                cx: 30,
                cy: 20,
                rx: 10,
                ry: 10,
            }),
            "Shift inscribes the circle instead"
        );
    }

    #[test]
    fn ellipse_hit_test_is_boundary_inclusive_and_excludes_bbox_corners() {
        let e = Shape::Ellipse {
            cx: 50,
            cy: 40,
            rx: 30,
            ry: 10,
        };
        assert!(e.hit_test(Point::new(50, 40)));
        assert!(e.hit_test(Point::new(80, 40)), "rx vertex inclusive");
        assert!(e.hit_test(Point::new(50, 30)), "ry vertex inclusive");
        assert!(!e.hit_test(Point::new(80, 30)), "bbox corner outside");
        assert!(!e.hit_test(Point::new(81, 40)));
        assert_eq!(e.bbox(), Rect::new(20, 30, 60, 20));
    }

    #[test]
    fn ellipse_resize_rides_its_bounding_box() {
        let e = Shape::Ellipse {
            cx: 50,
            cy: 40,
            rx: 20,
            ry: 10,
        };
        // Grab the right edge of the bbox (x = 70) and pull to x = 90.
        let handle = e.resize_grab(Point::new(70, 40), 2).expect("edge grab");
        let resized = e.resize_to(handle, Point::new(90, 40), BOUNDS_RECT, false);
        assert_eq!(
            resized,
            Shape::Ellipse {
                cx: 60,
                cy: 40,
                rx: 30,
                ry: 10,
            },
            "left edge anchored, rx grew"
        );
    }

    #[test]
    fn rotated_ellipse_hit_follows_the_turn() {
        let e = Shape::Ellipse {
            cx: 50,
            cy: 40,
            rx: 30,
            ry: 8,
        };
        // Turned 90, the wide ellipse stands tall.
        assert!(e.hit_test_rotated(90, Point::new(50, 65)));
        assert!(!e.hit_test_rotated(90, Point::new(75, 40)));
        assert!(e.hit_test(Point::new(75, 40)), "unrotated it lies flat");
    }

    #[test]
    fn point_in_poly_handles_concave_shapes_edges_included() {
        // A U shape: the notch between the arms is outside.
        let u = vec![
            Point::new(0, 0),
            Point::new(10, 0),
            Point::new(10, 30),
            Point::new(20, 30),
            Point::new(20, 0),
            Point::new(30, 0),
            Point::new(30, 40),
            Point::new(0, 40),
        ];
        let shape = Shape::Poly { points: u };
        assert!(shape.hit_test(Point::new(5, 20)), "left arm");
        assert!(shape.hit_test(Point::new(25, 20)), "right arm");
        assert!(shape.hit_test(Point::new(15, 35)), "base");
        assert!(!shape.hit_test(Point::new(15, 10)), "the notch is outside");
        assert!(shape.hit_test(Point::new(0, 0)), "vertex inclusive");
        assert!(shape.hit_test(Point::new(5, 0)), "edge inclusive");
        assert!(!shape.hit_test(Point::new(-1, 20)));
        // The interior click point avoids the notch.
        assert!(shape.hit_test(shape.click_point()));
    }

    #[test]
    fn regular_polygon_puts_the_first_vertex_at_the_cursor() {
        let hex = regular_polygon(Point::new(100, 100), Point::new(140, 100), 6);
        let Shape::Poly { ref points } = hex else {
            panic!("regular polygon is a poly")
        };
        assert_eq!(points.len(), 6);
        assert_eq!(points[0], Point::new(140, 100), "first vertex at cursor");
        for p in points {
            let d = f64::from(p.x - 100).hypot(f64::from(p.y - 100));
            assert!((d - 40.0).abs() < 1.5, "vertex {p:?} off the radius: {d}");
        }
        // The side count clamps to something drawable.
        let tri = regular_polygon(Point::new(0, 0), Point::new(10, 0), 1);
        let Shape::Poly { points } = tri else {
            panic!()
        };
        assert_eq!(points.len(), 3);
    }

    #[test]
    fn simplify_path_drops_jitter_and_keeps_corners() {
        // A noisy L: collinear runs with 1px wobble collapse; the corner
        // survives.
        let path: Vec<Point> = (0..=20)
            .map(|x| Point::new(x * 5, i32::from(x % 2 != 0)))
            .chain((1..=10).map(|y| Point::new(100, y * 5)))
            .collect();
        let simplified = simplify_path(&path, 2.0);
        assert!(
            simplified.len() <= 5,
            "expected a handful of points, got {}",
            simplified.len()
        );
        assert_eq!(*simplified.first().unwrap(), Point::new(0, 0));
        assert_eq!(*simplified.last().unwrap(), Point::new(100, 50));
        assert!(
            simplified.contains(&Point::new(100, 1)) || simplified.contains(&Point::new(100, 0)),
            "the corner survives: {simplified:?}"
        );
    }

    #[test]
    fn poly_moves_resizes_and_rotates_like_any_shape() {
        let square = Shape::Poly {
            points: vec![
                Point::new(10, 10),
                Point::new(30, 10),
                Point::new(30, 30),
                Point::new(10, 30),
            ],
        };
        assert_eq!(square.bbox(), Rect::new(10, 10, 20, 20));
        let moved = square.translated(5, -5);
        assert_eq!(moved.bbox(), Rect::new(15, 5, 20, 20));
        // Bbox-edge resize scales every vertex.
        let handle = square.resize_grab(Point::new(30, 20), 2).expect("edge");
        let grown = square.resize_to(handle, Point::new(50, 20), BOUNDS_RECT, false);
        assert_eq!(grown.bbox(), Rect::new(10, 10, 40, 20));
        // Rotation bakes into the vertices.
        let turned = square.with_rotation_baked(90);
        assert_eq!(turned.bbox(), square.bbox(), "square is 90-symmetric");
        assert!(matches!(turned, Shape::Poly { .. }));
    }

    #[test]
    fn click_point_centers_each_kind() {
        assert_eq!(
            Shape::Rect(Rect::new(10, 20, 30, 40)).click_point(),
            Point::new(25, 40)
        );
        assert_eq!(
            Shape::Circle { cx: 5, cy: 6, r: 7 }.click_point(),
            Point::new(5, 6)
        );
        let tri = Shape::Triangle {
            ax: 30,
            ay: 0,
            bx: 0,
            by: 60,
            cx: 60,
            cy: 60,
        };
        assert_eq!(tri.click_point(), Point::new(30, 40));
        assert!(tri.hit_test(tri.click_point()));
        // The click point is the rotation pivot, so it stays inside the
        // silhouette at any angle.
        let rect = Shape::Rect(Rect::new(10, 10, 40, 10));
        assert!(rect.hit_test_rotated(90, rect.click_point()));
    }

    #[test]
    fn clamp_point_lands_inside_and_leaves_interior_points_alone() {
        let r = Rect::new(10, 20, 30, 40);
        let inside = Point::new(15, 25);
        assert_eq!(r.clamp_point(inside), inside);
        // The far edge is exclusive, matching `contains`.
        assert_eq!(r.clamp_point(Point::new(100, 100)), Point::new(39, 59));
        assert_eq!(r.clamp_point(Point::new(-5, -5)), Point::new(10, 20));
        for p in [
            Point::new(100, 100),
            Point::new(-5, -5),
            Point::new(15, 900),
        ] {
            assert!(r.contains(r.clamp_point(p)));
        }
    }

    #[test]
    fn clamp_point_on_a_zero_sized_rect_gives_the_origin_corner() {
        let r = Rect::new(7, 9, 0, 0);
        assert_eq!(r.clamp_point(Point::new(100, 100)), Point::new(7, 9));
    }

    #[test]
    fn line_bbox_spans_both_endpoints_in_any_direction() {
        let down = Line::new(Point::new(10, 20), Point::new(40, 60));
        let up = Line::new(Point::new(40, 60), Point::new(10, 20));
        assert_eq!(down.bbox(), Rect::new(10, 20, 30, 40));
        assert_eq!(up.bbox(), down.bbox());
        // A zero-length ruler still has a placeable caption anchor.
        let dot = Line::new(Point::new(5, 5), Point::new(5, 5));
        assert_eq!(dot.bbox(), Rect::new(5, 5, 0, 0));
    }

    #[test]
    fn tool_kind_cycles_through_the_drawing_tools() {
        assert_eq!(ToolKind::Rect.next(), ToolKind::Ellipse);
        assert_eq!(ToolKind::Ellipse.next(), ToolKind::Triangle);
        assert_eq!(ToolKind::Triangle.next(), ToolKind::Polygon);
        assert_eq!(ToolKind::Polygon.next(), ToolKind::Freehand);
        assert_eq!(ToolKind::Freehand.next(), ToolKind::Measure);
        assert_eq!(ToolKind::Measure.next(), ToolKind::Rect);
        // Record-only kinds cycle back into the modern set.
        assert_eq!(ToolKind::Circle.next(), ToolKind::Triangle);
        assert_eq!(ToolKind::Poly.next(), ToolKind::Rect);
    }

    #[test]
    fn triangle_preview_is_apex_top_center_in_drag_box() {
        let s = Shape::compute_preview(
            ToolKind::Triangle,
            Point::new(100, 100),
            Point::new(300, 200),
            BOUNDS_RECT,
            false,
        );
        assert_eq!(
            s,
            Some(Shape::Triangle {
                ax: 200,
                ay: 100,
                bx: 100,
                by: 200,
                cx: 300,
                cy: 200,
            })
        );
    }

    #[test]
    fn triangle_hit_test_excludes_bbox_corners() {
        let tri = Shape::Triangle {
            ax: 200,
            ay: 100,
            bx: 100,
            by: 200,
            cx: 300,
            cy: 200,
        };
        assert!(tri.hit_test(Point::new(200, 150))); // centroid area
        assert!(tri.hit_test(Point::new(200, 100))); // apex, inclusive
        assert!(tri.hit_test(Point::new(150, 200))); // on the base
        assert!(!tri.hit_test(Point::new(105, 105))); // bbox top-left, empty
        assert!(!tri.hit_test(Point::new(295, 105))); // bbox top-right, empty
    }

    #[test]
    fn triangle_bbox_and_move_clamp() {
        let tri = Shape::Triangle {
            ax: 200,
            ay: 100,
            bx: 100,
            by: 200,
            cx: 300,
            cy: 200,
        };
        assert_eq!(tri.bbox(), Rect::new(100, 100, 200, 100));
        // Dragged far off-screen: the bbox pins to the corner and all three
        // vertices translate together.
        let moved = tri.clamp_move(Point::new(0, 0), Point::new(-500, -500), BOUNDS_RECT);
        assert_eq!(moved.bbox(), Rect::new(0, 0, 200, 100));
        assert_eq!(
            moved,
            Shape::Triangle {
                ax: 100,
                ay: 0,
                bx: 0,
                by: 100,
                cx: 200,
                cy: 100,
            }
        );
    }

    #[test]
    fn triangle_resize_scales_vertices_into_new_bbox() {
        let tri = Shape::Triangle {
            ax: 200,
            ay: 100,
            bx: 100,
            by: 200,
            cx: 300,
            cy: 200,
        };
        // Drag the bottom-right bbox corner to double both dimensions.
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        let resized = tri.resize_to(handle, Point::new(500, 300), BOUNDS_RECT, false);
        assert_eq!(
            resized,
            Shape::Triangle {
                ax: 300,
                ay: 100,
                bx: 100,
                by: 300,
                cx: 500,
                cy: 300,
            }
        );
    }

    #[test]
    fn triangle_resize_grab_is_on_the_bbox_border() {
        let tri = Shape::Triangle {
            ax: 200,
            ay: 100,
            bx: 100,
            by: 200,
            cx: 300,
            cy: 200,
        };
        // Top edge of the bbox (empty space next to the apex) still grabs.
        assert_eq!(
            tri.resize_grab(Point::new(150, 100), 5),
            Some(ResizeHandle::RectEdges {
                left: false,
                right: false,
                top: true,
                bottom: false
            })
        );
        assert_eq!(tri.resize_grab(Point::new(200, 150), 5), None); // interior
    }

    #[test]
    fn degenerate_triangles_cover_nothing() {
        let point = Shape::Triangle {
            ax: 0,
            ay: 0,
            bx: 0,
            by: 0,
            cx: 0,
            cy: 0,
        };
        assert!(!point.hit_test(Point::new(500, 500)));
        assert!(!point.hit_test(Point::new(0, 0)));
        let line = Shape::Triangle {
            ax: 0,
            ay: 0,
            bx: 10,
            by: 10,
            cx: 20,
            cy: 20,
        };
        assert!(!line.hit_test(Point::new(400, 400)));
        assert!(!line.hit_test(Point::new(5, 5)));
    }

    #[test]
    fn extreme_shapes_do_not_panic() {
        let huge = Shape::Circle {
            cx: 0,
            cy: 0,
            r: 2_000_000_000,
        };
        let bb = huge.bbox();
        assert!(bb.w > 0);
        let far = Shape::Rect(Rect::new(
            2_000_000_000,
            2_000_000_000,
            400_000_000,
            400_000_000,
        ));
        let _ = far.rotated_bbox(45);
    }

    #[test]
    fn resize_of_sub_min_rect_stays_in_bounds() {
        // A 1px-thin rect (below the resize MIN floor): dragging its left
        // edge to the screen edge must not push it to x = -1.
        let s = Shape::Rect(Rect::new(0, 0, 1, 100));
        let handle = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        let Shape::Rect(r) = s.resize_to(handle, Point::new(0, 50), BOUNDS_RECT, false) else {
            panic!("still a rect")
        };
        assert!(r.x >= 0, "escaped left: {r:?}");
        // Mirror case at the right edge.
        let s = Shape::Rect(Rect::new(BOUNDS.w - 1, 0, 1, 100));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let Shape::Rect(r) = s.resize_to(
            handle,
            Point::new(BOUNDS_RECT.w - 1, 50),
            BOUNDS_RECT,
            false,
        ) else {
            panic!("still a rect")
        };
        assert!(r.x + r.w <= BOUNDS.w, "escaped right: {r:?}");
    }

    #[test]
    fn rotated_resize_never_moves_the_anchored_edge() {
        // Regression: the sub-MIN shift-clamp must not fire for normal
        // boxes — under rotation the local box can exceed visual bounds,
        // and clamping it drifted the anchor by up to the rotated diagonal.
        let s = Shape::Rect(Rect::new(800, 500, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let Shape::Rect(r) =
            s.resize_to_rotated(45, handle, Point::new(1900, 1000), BOUNDS_RECT, false)
        else {
            panic!("still a rect")
        };
        assert_eq!(r.x, 800, "anchored left edge moved");
        assert_eq!(r.y, 500, "anchored top edge moved");
    }

    #[test]
    fn resize_of_offscreen_local_box_does_not_teleport() {
        // Regression: a rotated move can leave the local box partially
        // off-screen; a later resize must adjust one edge, not relocate
        // the shape to the origin.
        let s = Shape::Rect(Rect::new(-90, 0, 200, 20));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let Shape::Rect(r) = s.resize_to(handle, Point::new(120, 10), BOUNDS_RECT, false) else {
            panic!("still a rect")
        };
        assert_eq!(r.x, -90, "shape teleported");
        assert_eq!(r.w, 210);
    }

    #[test]
    fn rotated_resize_tracks_cursor_at_screen_edge() {
        // Cursor clamps in the visual frame: resizing a rotated shape with
        // the cursor at the screen corner still lands on-screen local
        // coordinates instead of freezing early.
        let s = Shape::Rect(Rect::new(800, 500, 200, 100));
        let handle = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let r45 = s.resize_to_rotated(45, handle, Point::new(99_999, 99_999), BOUNDS_RECT, false);
        // The local resize saw a finite, in-bounds visual point.
        assert_ne!(r45, s);
    }

    #[test]
    fn rotate_point_quarter_turn() {
        let center = Point::new(100, 100);
        // 90 deg clockwise in screen coords: (110, 100) -> (100, 110).
        assert_eq!(
            rotate_point_about(Point::new(110, 100), center, 90),
            Point::new(100, 110)
        );
        assert_eq!(
            rotate_point_about(Point::new(110, 100), center, -90),
            Point::new(100, 90)
        );
        assert_eq!(
            rotate_point_about(Point::new(110, 100), center, 360),
            Point::new(110, 100)
        );
    }

    #[test]
    fn normalize_deg_wraps_into_range() {
        assert_eq!(normalize_deg(0), 0);
        assert_eq!(normalize_deg(-1), 359);
        assert_eq!(normalize_deg(360), 0);
        assert_eq!(normalize_deg(725), 5);
    }

    #[test]
    fn rotated_bbox_of_quarter_turned_rect_swaps_dimensions() {
        let s = Shape::Rect(Rect::new(100, 100, 200, 100));
        let bb = s.rotated_bbox(90);
        assert_eq!((bb.w, bb.h), (100, 200));
        // Same center as the unrotated shape.
        assert_eq!(bb.x + bb.w / 2, 200);
        assert_eq!(bb.y + bb.h / 2, 150);
        // Rotation 0 and circles are identity.
        assert_eq!(s.rotated_bbox(0), s.bbox());
        let c = Shape::Circle {
            cx: 50,
            cy: 50,
            r: 20,
        };
        assert_eq!(c.rotated_bbox(45), c.bbox());
    }

    #[test]
    fn rotated_hit_test_follows_the_turned_shape() {
        // Wide flat rect turned 90 deg: a point above the center (inside
        // the turned shape, outside the original) now hits, and a far-right
        // point (inside the original) no longer does.
        let s = Shape::Rect(Rect::new(100, 100, 200, 20));
        assert!(s.hit_test_rotated(90, Point::new(200, 30)));
        assert!(!s.hit_test_rotated(90, Point::new(290, 110)));
        assert!(s.hit_test_rotated(0, Point::new(290, 110)));
    }

    #[test]
    fn rotated_resize_grab_finds_the_visual_edge() {
        // The turned rect's visually-left edge maps to a local edge grab.
        let s = Shape::Rect(Rect::new(100, 100, 200, 20));
        // After 90 deg the shape occupies x in [190, 210], y in [10, 210].
        assert!(s.resize_grab_rotated(90, Point::new(190, 110), 5).is_some());
        assert!(s.resize_grab_rotated(90, Point::new(150, 110), 5).is_none());
    }

    #[test]
    fn baked_triangle_rotates_vertices_others_unchanged() {
        let tri = Shape::Triangle {
            ax: 200,
            ay: 100,
            bx: 100,
            by: 200,
            cx: 300,
            cy: 200,
        };
        let baked = tri.with_rotation_baked(180);
        // Pivot is the bbox center (200, 150): apex flips below.
        assert_eq!(
            baked,
            Shape::Triangle {
                ax: 200,
                ay: 200,
                bx: 300,
                by: 100,
                cx: 100,
                cy: 100,
            }
        );
        let rect = Shape::Rect(Rect::new(1, 2, 3, 4));
        assert_eq!(rect.with_rotation_baked(90), rect);
        assert_eq!(tri.with_rotation_baked(0), tri);
    }

    #[test]
    fn triangle_serde_is_distinct_from_rect_and_circle() {
        let tri = Shape::Triangle {
            ax: 1,
            ay: 2,
            bx: 3,
            by: 4,
            cx: 5,
            cy: 6,
        };
        let json = serde_json::to_string(&tri).unwrap();
        let back: Shape = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tri);
        // The old kinds still round-trip to themselves.
        let rect: Shape = serde_json::from_str(r#"{"x":1,"y":2,"w":3,"h":4}"#).unwrap();
        assert_eq!(rect, Shape::Rect(Rect::new(1, 2, 3, 4)));
        let circle: Shape = serde_json::from_str(r#"{"cx":1,"cy":2,"r":3}"#).unwrap();
        assert_eq!(circle, Shape::Circle { cx: 1, cy: 2, r: 3 });
    }

    #[test]
    fn a_triangle_grabs_from_its_bbox_origin() {
        let tri = Shape::Triangle {
            ax: 50,
            ay: 10,
            bx: 20,
            by: 70,
            cx: 80,
            cy: 70,
        };
        assert_eq!(tri.grab_origin(), Point::new(20, 10));
    }

    #[test]
    fn a_rotated_move_clamps_the_rotated_box_to_bounds() {
        let rect = Shape::Rect(Rect::new(10, 10, 40, 20));
        let bounds = Size::new(200, 200);
        // Dragged far past the corner: the rotated AABB, not the unrotated
        // rect, is what must stay inside.
        let moved = rect.clamp_move_rotated(
            45,
            Point::new(0, 0),
            Point::new(500, 500),
            Rect::new(0, 0, bounds.w, bounds.h),
        );
        let bb = moved.rotated_bbox(45);
        assert!(bb.x >= 0 && bb.y >= 0, "{bb:?}");
        assert!(bb.x + bb.w <= bounds.w, "{bb:?}");
        assert!(bb.y + bb.h <= bounds.h, "{bb:?}");
    }

    #[test]
    fn a_rotated_grab_references_the_rotated_box_origin() {
        let rect = Shape::Rect(Rect::new(10, 10, 40, 20));
        assert_eq!(rect.grab_origin_rotated(0), rect.grab_origin());
        let rotated = rect.grab_origin_rotated(45);
        assert_eq!(
            rotated,
            Point::new(rect.rotated_bbox(45).x, rect.rotated_bbox(45).y)
        );
        // A circle has no orientation, so rotation cannot move its grab.
        let circle = Shape::Circle {
            cx: 40,
            cy: 40,
            r: 9,
        };
        assert_eq!(circle.grab_origin_rotated(30), circle.grab_origin());
    }

    #[test]
    fn min3_and_max3_pick_each_position() {
        assert_eq!(min3(1, 2, 3), 1);
        assert_eq!(min3(2, 1, 3), 1);
        assert_eq!(min3(3, 2, 1), 1);
        assert_eq!(max3(3, 2, 1), 3);
        assert_eq!(max3(1, 3, 2), 3);
        assert_eq!(max3(1, 2, 3), 3);
    }

    #[test]
    fn a_proportional_vertical_edge_resize_keeps_the_aspect() {
        // Grabbing only a vertical edge with Shift: width follows height.
        let rect = Shape::Rect(Rect::new(20, 20, 40, 20));
        let resized = rect.resize_to_rotated(
            0,
            ResizeHandle::RectEdges {
                left: false,
                right: false,
                top: true,
                bottom: false,
            },
            Point::new(30, 0),
            Rect::new(0, 0, 300, 300),
            true,
        );
        let bb = resized.bbox();
        assert!(bb.w >= 2 && bb.h >= 2, "{bb:?}");
        assert!(bb.x >= 0 && bb.x + bb.w <= 300, "{bb:?}");
    }
}
