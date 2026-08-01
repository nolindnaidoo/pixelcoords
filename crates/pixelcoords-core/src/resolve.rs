//! "Where do I click for this label, right now?" — the logic behind
//! `pixelcoords resolve`.
//!
//! Every piece of this already existed, in pieces each consumer had to
//! reassemble: the session holds the region and its monitor's scale,
//! `geometry::click_point` finds an interior point to aim at, `locate`
//! corrects for drift, and `emit` knows which unit each platform's input
//! APIs expect. A consumer that wanted the one thing every consumer wants
//! had to call `find`, parse a bbox, pull in this crate for the click
//! point, then redo the logical/physical conversion `emit` already knew.
//!
//! Each reassembly is a chance to get DPI wrong, and it pushes geometry
//! into consumers — which is what this crate exists to prevent. `emit`
//! stays what it is, ready-to-paste code for humans; `resolve` is the
//! machine answer underneath it.

use serde::Serialize;
use thiserror::Error;

use crate::geometry::{Point, Shape};
use crate::locate::Delta;
use crate::session::{SelectionRecord, SessionFile};
use crate::space::{Origin, Resolved, logical_of};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("the session has no selections to resolve")]
    NoSelections,
    #[error("no selection is labeled {requested:?}; labels in this session: {available:?}")]
    UnknownLabel {
        requested: String,
        available: Vec<String>,
    },
    #[error(
        "selection {selection} references monitor {monitor}, which the \
         session does not describe"
    )]
    UnknownMonitor { selection: usize, monitor: usize },
    #[error(
        "the session has no target window — window-relative points need a \
         session captured with --target"
    )]
    NoTarget,
    #[error(
        "selection {selection} ({label:?}) has no window-relative \
         coordinates: it was marked on a different monitor than the target \
         window — ask in global or monitor space instead"
    )]
    OffTargetMonitor { selection: usize, label: String },
}

/// Where to act for one selection, and what the answer is measured in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolution {
    /// Index into `session.selections` — this row's identity.
    pub index: usize,
    pub label: String,
    pub monitor: usize,
    /// The monitor's DPI factor, so a consumer can check the conversion
    /// rather than trusting it.
    pub scale: f64,
    pub space: &'static str,
    pub units: &'static str,
    /// The point to act on.
    ///
    /// In logical units this is the *physical* interior point converted,
    /// not an interior point of the converted region. The two can differ
    /// by a pixel, and this is the one that matters: a consumer clicks in
    /// logical points and the window server maps that back to physical,
    /// landing inside the region a human actually marked. Deriving it
    /// from the rounded-down shape would optimize for a number that
    /// nothing clicks.
    pub point: Point,
    /// The region it came from, in the same space and units — so a caller
    /// can draw what it is about to click. Converted independently of
    /// `point`, so on a scaled display the two may round differently.
    pub region: Shape,
    /// Match score from the relocation pass; absent without `--relocate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// How far the region moved since the session was saved; absent
    /// without `--relocate`, and absent with it when nothing moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<Delta>,
}

/// The click points for a session's selections, or the labeled subset.
///
/// Pure session math: no capture, no permission, no clock. `drift` is
/// consulted per selection index when the caller has relocated first —
/// its `Delta` is applied in the session's own physical pixels, before
/// any unit conversion, because that is the space the match reported in.
///
/// **`Origin::Monitor`'s index is ignored.** Every selection is reported
/// in its own monitor's coordinates and every row carries `monitor`, so
/// there is nothing for an index to disambiguate. That is the opposite of
/// [`crate::verdict::assess`], which is handed one point and must be told
/// which monitor it belongs to.
pub fn resolve(
    session: &SessionFile,
    label: Option<&str>,
    origin: Origin,
    units: Resolved,
    drift: &dyn Fn(usize) -> Option<(f64, Delta)>,
) -> Result<Vec<Resolution>, ResolveError> {
    if session.selections.is_empty() {
        return Err(ResolveError::NoSelections);
    }
    let wanted = crate::session::select_by_label(session, label);
    if wanted.is_empty() {
        return Err(ResolveError::UnknownLabel {
            requested: label.unwrap_or_default().to_string(),
            available: crate::session::distinct_labels(session.selections.iter()),
        });
    }
    if matches!(origin, Origin::Window) && session.target.is_none() {
        return Err(ResolveError::NoTarget);
    }
    wanted
        .into_iter()
        .map(|(index, record)| one(session, index, record, origin, units, drift))
        .collect()
}

fn one(
    session: &SessionFile,
    index: usize,
    record: &SelectionRecord,
    origin: Origin,
    units: Resolved,
    drift: &dyn Fn(usize) -> Option<(f64, Delta)>,
) -> Result<Resolution, ResolveError> {
    let monitor = session
        .monitors
        .iter()
        .find(|m| m.index == record.monitor)
        .ok_or(ResolveError::UnknownMonitor {
            selection: index,
            monitor: record.monitor,
        })?;

    let stored = stored_shape(record, index, origin)?;
    let moved = drift(index);

    // Drift is measured in monitor-local physical pixels — the space the
    // capture was matched in — so it is applied before the origin is
    // reinterpreted or the units are converted. Doing it after would
    // scale the delta by the DPI factor a second time.
    let region = match moved {
        Some((_, d)) => stored.translated(d.dx, d.dy),
        None => stored,
    };
    let physical = region.click_point();

    let (point, region) = match units {
        Resolved::Physical => (physical, region),
        Resolved::Logical => (
            logical_of(physical, monitor.scale),
            scaled(&region, monitor.scale),
        ),
    };

    Ok(Resolution {
        index,
        label: record.label.clone(),
        monitor: record.monitor,
        scale: monitor.scale,
        space: origin.label(),
        units: units.label(),
        point,
        region,
        score: moved.map(|(score, _)| score),
        delta: moved.map(|(_, d)| d),
    })
}

/// The shape the session already stores for this origin — nothing is
/// derived here, so a resolved point is the same pixel `assert` tests
/// against.
fn stored_shape(
    record: &SelectionRecord,
    index: usize,
    origin: Origin,
) -> Result<Shape, ResolveError> {
    match origin {
        Origin::Global => Ok(record.global_px.clone()),
        Origin::Monitor(_) => Ok(record.px.clone()),
        Origin::Window => record
            .window_px
            .clone()
            .ok_or_else(|| ResolveError::OffTargetMonitor {
                selection: index,
                label: record.label.clone(),
            }),
    }
}

/// A shape's every coordinate through `logical_of`, so the reported
/// region is in the same units as the point inside it.
fn scaled(shape: &Shape, scale: f64) -> Shape {
    let p = |x: i32, y: i32| logical_of(Point::new(x, y), scale);
    match *shape {
        Shape::Rect(r) => {
            let origin = p(r.x, r.y);
            let far = p(r.x + r.w, r.y + r.h);
            Shape::Rect(crate::geometry::Rect::new(
                origin.x,
                origin.y,
                far.x - origin.x,
                far.y - origin.y,
            ))
        }
        Shape::Circle { cx, cy, r } => {
            let c = p(cx, cy);
            Shape::Circle {
                cx: c.x,
                cy: c.y,
                r: (f64::from(r) / scale).round() as i32,
            }
        }
        Shape::Ellipse { cx, cy, rx, ry } => {
            let c = p(cx, cy);
            Shape::Ellipse {
                cx: c.x,
                cy: c.y,
                rx: (f64::from(rx) / scale).round() as i32,
                ry: (f64::from(ry) / scale).round() as i32,
            }
        }
        Shape::Triangle {
            ax,
            ay,
            bx,
            by,
            cx,
            cy,
        } => {
            let (a, b, c) = (p(ax, ay), p(bx, by), p(cx, cy));
            Shape::Triangle {
                ax: a.x,
                ay: a.y,
                bx: b.x,
                by: b.y,
                cx: c.x,
                cy: c.y,
            }
        }
        Shape::Poly { ref points } => Shape::Poly {
            points: points.iter().map(|q| p(q.x, q.y)).collect(),
        },
    }
}

impl Resolved {
    /// The name these units carry in JSON output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Logical => "logical",
        }
    }
}

/// Every selection resolved — the aggregate `resolve` reports as `ok`.
///
/// Without relocation this is always true: session math cannot fail once
/// the labels resolved. With it, a region that was not found
/// unambiguously has no trustworthy point, and saying so is the point.
#[must_use]
pub fn all_resolved(results: &[Resolution], relocated: bool) -> bool {
    !results.is_empty() && (!relocated || results.iter().all(|r| r.score.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::selection::Selection;
    use crate::session::MonitorRecord;

    fn none(_: usize) -> Option<(f64, Delta)> {
        None
    }

    /// Two monitors: index 0 at scale 1, index 1 at scale 2 and offset,
    /// so a mixed-DPI conversion is the default case rather than a
    /// special one.
    fn mixed_dpi(labels: &[(&str, usize, Rect)]) -> SessionFile {
        let selections: Vec<Selection> = labels
            .iter()
            .map(|&(label, monitor, rect)| {
                let mut sel = Selection::new(Shape::Rect(rect), monitor);
                sel.label = label.to_string();
                sel
            })
            .collect();
        let crops: Vec<String> = (0..labels.len()).map(|i| format!("c{i}.png")).collect();
        SessionFile::build(
            "test",
            "2026-08-01T00:00:00Z".into(),
            vec![
                MonitorRecord {
                    index: 0,
                    name: "Left".into(),
                    primary: true,
                    origin_px: Point::new(0, 0),
                    size_px: crate::geometry::Size::new(1920, 1080),
                    scale: 1.0,
                },
                MonitorRecord {
                    index: 1,
                    name: "Right Retina".into(),
                    primary: false,
                    origin_px: Point::new(1920, 0),
                    size_px: crate::geometry::Size::new(2560, 1440),
                    scale: 2.0,
                },
            ],
            &selections,
            &crops,
            None,
        )
    }

    #[test]
    fn a_physical_answer_is_the_stored_click_point() {
        let file = mixed_dpi(&[("submit", 0, Rect::new(800, 400, 100, 80))]);
        let out = resolve(&file, None, Origin::Global, Resolved::Physical, &none).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].point, Point::new(850, 440));
        assert!((out[0].scale - 1.0).abs() < f64::EPSILON);
        assert_eq!(out[0].units, "physical");
        assert_eq!(out[0].space, "global");
    }

    #[test]
    fn each_selection_converts_through_its_own_monitors_scale() {
        // The trap this command exists to remove: one desktop, two DPI
        // factors, and a consumer that divides everything by one of them.
        let file = mixed_dpi(&[
            ("left", 0, Rect::new(800, 400, 100, 80)),
            ("right", 1, Rect::new(100, 200, 40, 60)),
        ]);
        let out = resolve(&file, None, Origin::Global, Resolved::Logical, &none).unwrap();

        // Monitor 0 is scale 1: logical == physical.
        assert_eq!(out[0].point, Point::new(850, 440));
        assert!((out[0].scale - 1.0).abs() < f64::EPSILON);

        // Monitor 1 is scale 2 at global origin (1920, 0): the stored
        // global click point is (2040, 230), halved to (1020, 115).
        assert!((out[1].scale - 2.0).abs() < f64::EPSILON);
        assert_eq!(out[1].point, Point::new(1020, 115));
    }

    #[test]
    fn monitor_space_answers_in_monitor_local_coordinates() {
        let file = mixed_dpi(&[("right", 1, Rect::new(100, 200, 40, 60))]);
        let global = resolve(&file, None, Origin::Global, Resolved::Physical, &none).unwrap();
        let local = resolve(&file, None, Origin::Monitor(1), Resolved::Physical, &none).unwrap();
        assert_eq!(global[0].point, Point::new(2040, 230));
        assert_eq!(local[0].point, Point::new(120, 230), "origin removed");
        assert_eq!(local[0].space, "monitor");
    }

    #[test]
    fn monitor_space_needs_no_index_and_spans_monitors() {
        // Each selection comes back in its own monitor's coordinates, so
        // a two-monitor session is answerable without naming one — the
        // index in `Origin::Monitor` is not consulted, and passing a
        // different one changes nothing.
        let file = mixed_dpi(&[
            ("left", 0, Rect::new(800, 400, 100, 80)),
            ("right", 1, Rect::new(100, 200, 40, 60)),
        ]);
        let zero = resolve(&file, None, Origin::Monitor(0), Resolved::Physical, &none).unwrap();
        let one = resolve(&file, None, Origin::Monitor(1), Resolved::Physical, &none).unwrap();
        assert_eq!(zero, one, "the index carries nothing here");
        assert_eq!(zero[0].point, Point::new(850, 440), "monitor 0, local");
        assert_eq!(zero[1].point, Point::new(120, 230), "monitor 1, local");
        assert_eq!((zero[0].monitor, zero[1].monitor), (0, 1));
    }

    #[test]
    fn the_region_travels_in_the_same_units_as_the_point() {
        let file = mixed_dpi(&[("right", 1, Rect::new(100, 200, 40, 60))]);
        let out = resolve(&file, None, Origin::Monitor(1), Resolved::Logical, &none).unwrap();
        let Shape::Rect(r) = out[0].region else {
            panic!("a rect stays a rect")
        };
        assert_eq!((r.x, r.y, r.w, r.h), (50, 100, 20, 30));
        assert!(
            r.contains(out[0].point),
            "the reported point must lie inside the reported region"
        );
    }

    #[test]
    fn drift_is_applied_before_the_units_are_converted() {
        // A 40px physical move on a scale-2 display is 20 logical points.
        // Converting first and translating after would report 40.
        let file = mixed_dpi(&[("right", 1, Rect::new(100, 200, 40, 60))]);
        let moved = |_: usize| Some((0.97, Delta { dx: 40, dy: 0 }));
        let out = resolve(&file, None, Origin::Monitor(1), Resolved::Logical, &moved).unwrap();
        assert_eq!(out[0].point, Point::new(80, 115));
        assert_eq!(out[0].delta, Some(Delta { dx: 40, dy: 0 }));
        assert_eq!(out[0].score, Some(0.97));
    }

    #[test]
    fn without_relocation_no_score_or_delta_is_reported() {
        let file = mixed_dpi(&[("submit", 0, Rect::new(800, 400, 100, 80))]);
        let out = resolve(&file, None, Origin::Global, Resolved::Physical, &none).unwrap();
        assert!(out[0].score.is_none() && out[0].delta.is_none());

        let json = serde_json::to_value(&out[0]).unwrap();
        assert!(json.get("score").is_none() && json.get("delta").is_none());
    }

    #[test]
    fn a_label_restricts_the_set_and_an_unknown_one_lists_what_exists() {
        let file = mixed_dpi(&[
            ("left", 0, Rect::new(0, 0, 10, 10)),
            ("right", 1, Rect::new(0, 0, 10, 10)),
        ]);
        let out = resolve(
            &file,
            Some("RIGHT"),
            Origin::Global,
            Resolved::Physical,
            &none,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, "right");

        let err = resolve(
            &file,
            Some("nope"),
            Origin::Global,
            Resolved::Physical,
            &none,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ResolveError::UnknownLabel {
                requested: "nope".into(),
                available: vec!["left".into(), "right".into()],
            }
        );
    }

    #[test]
    fn a_selection_off_the_target_monitor_has_no_window_answer() {
        use crate::session::TargetRecord;
        // A target session records window-relative coordinates only for
        // selections on the target's own monitor (`SelectionRecord::
        // window_px`). One marked on the *other* display has no window
        // answer at all, and inventing one would be a coordinate pointing
        // nowhere — so it is named instead.
        let mut on_target = Selection::new(Shape::Rect(Rect::new(110, 60, 20, 20)), 0);
        on_target.label = "on-target".into();
        let mut elsewhere = Selection::new(Shape::Rect(Rect::new(10, 10, 20, 20)), 1);
        elsewhere.label = "elsewhere".into();
        let file = SessionFile::build(
            "test",
            "2026-08-01T00:00:00Z".into(),
            vec![
                MonitorRecord {
                    index: 0,
                    name: "Left".into(),
                    primary: true,
                    origin_px: Point::new(0, 0),
                    size_px: crate::geometry::Size::new(1920, 1080),
                    scale: 1.0,
                },
                MonitorRecord {
                    index: 1,
                    name: "Right".into(),
                    primary: false,
                    origin_px: Point::new(1920, 0),
                    size_px: crate::geometry::Size::new(1920, 1080),
                    scale: 1.0,
                },
            ],
            &[on_target, elsewhere],
            &["c0.png".into(), "c1.png".into()],
            Some(TargetRecord {
                app: "Editor".into(),
                title: "main.rs".into(),
                monitor: 0,
                origin_px: Point::new(100, 50),
                size_px: crate::geometry::Size::new(800, 600),
            }),
        );

        // On the target's monitor, window space subtracts the window origin.
        let ok = resolve(
            &file,
            Some("on-target"),
            Origin::Window,
            Resolved::Physical,
            &none,
        )
        .unwrap();
        assert_eq!(ok[0].point, Point::new(20, 20));

        let err = resolve(
            &file,
            Some("elsewhere"),
            Origin::Window,
            Resolved::Physical,
            &none,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ResolveError::OffTargetMonitor {
                selection: 1,
                label: "elsewhere".into(),
            }
        );
    }

    #[test]
    fn window_space_needs_a_target_session() {
        let file = mixed_dpi(&[("submit", 0, Rect::new(0, 0, 10, 10))]);
        assert_eq!(
            resolve(&file, None, Origin::Window, Resolved::Physical, &none).unwrap_err(),
            ResolveError::NoTarget
        );
    }

    #[test]
    fn an_empty_session_is_refused_before_the_label_is_considered() {
        let file = mixed_dpi(&[]);
        assert_eq!(
            resolve(
                &file,
                Some("anything"),
                Origin::Global,
                Resolved::Physical,
                &none
            )
            .unwrap_err(),
            ResolveError::NoSelections,
            "an empty session and an unmatched label are different mistakes"
        );
    }

    #[test]
    fn ok_is_true_without_relocation_and_follows_the_scores_with_it() {
        let file = mixed_dpi(&[("submit", 0, Rect::new(0, 0, 10, 10))]);
        let still = resolve(&file, None, Origin::Global, Resolved::Physical, &none).unwrap();
        assert!(all_resolved(&still, false));
        assert!(
            !all_resolved(&still, true),
            "asked to relocate and given no score, the point is not trustworthy"
        );

        let moved = |_: usize| Some((0.99, Delta { dx: 1, dy: 1 }));
        let found = resolve(&file, None, Origin::Global, Resolved::Physical, &moved).unwrap();
        assert!(all_resolved(&found, true));
        assert!(!all_resolved(&[], false), "nothing resolved is not success");
    }

    #[test]
    fn every_shape_kind_converts_within_a_pixel_of_its_own_click_point() {
        // `point` is the physical click point converted, *not* the click
        // point of the converted region — see the note on `Resolution`.
        // The two are within a pixel of each other, and this pins that:
        // a larger gap would mean `scaled` had distorted the shape rather
        // than just rounded it.
        for shape in [
            Shape::Circle {
                cx: 100,
                cy: 200,
                r: 40,
            },
            Shape::Ellipse {
                cx: 100,
                cy: 200,
                rx: 40,
                ry: 20,
            },
            Shape::Triangle {
                ax: 0,
                ay: 0,
                bx: 100,
                by: 0,
                cx: 50,
                cy: 80,
            },
            Shape::Poly {
                points: vec![Point::new(0, 0), Point::new(100, 0), Point::new(100, 100)],
            },
        ] {
            let converted = scaled(&shape, 2.0).click_point();
            let reported = logical_of(shape.click_point(), 2.0);
            assert!(
                (converted.x - reported.x).abs() <= 1 && (converted.y - reported.y).abs() <= 1,
                "{shape:?}: converted {converted:?} vs reported {reported:?}"
            );
        }
    }

    #[test]
    fn the_reported_point_is_the_true_interior_point_converted() {
        // The distinction that matters when a consumer clicks: the point
        // must map back to a pixel inside the *real* region, so it is the
        // physical interior point converted, not an interior point of the
        // rounded-down shape.
        let concave = Shape::Poly {
            points: vec![
                Point::new(0, 0),
                Point::new(100, 0),
                Point::new(100, 20),
                Point::new(20, 20),
                Point::new(20, 100),
                Point::new(0, 100),
            ],
        };
        let mut sel = Selection::new(concave.clone(), 1);
        sel.label = "L-shape".into();
        let file = SessionFile::build(
            "test",
            "2026-08-01T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 1,
                name: "Retina".into(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: crate::geometry::Size::new(2560, 1440),
                scale: 2.0,
            }],
            &[sel],
            &["c0.png".into()],
            None,
        );
        let out = resolve(&file, None, Origin::Monitor(1), Resolved::Logical, &none).unwrap();
        assert_eq!(out[0].point, logical_of(concave.click_point(), 2.0));
        assert!(
            concave.hit_test(concave.click_point()),
            "the physical point it derives from is inside the real shape"
        );
    }
}
