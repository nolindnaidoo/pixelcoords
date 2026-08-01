//! Point-in-region verdicts against a saved session — the logic behind
//! `pixelcoords assert`.
//!
//! A verdict answers "does this point land inside a marked region?" for a
//! point expressed in any of the session's coordinate spaces. It exists so
//! automation — a computer-use agent, a click script under test — can be
//! scored against regions a human marked once, without reimplementing the
//! session's geometry.

use serde::Serialize;
use thiserror::Error;

use crate::geometry::{Point, Rect, Shape, ToolKind};
use crate::session::{SelectionRecord, SessionFile};
use crate::space::Origin;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VerdictError {
    #[error(
        "the session has no target window — window-relative points need a \
         session captured with --target"
    )]
    NoTarget,
    #[error("monitor {requested} is not in this session; it has monitors {available:?}")]
    UnknownMonitor {
        requested: usize,
        available: Vec<usize>,
    },
    #[error("no selection is labeled {requested:?}; labels in this space: {available:?}")]
    UnknownLabel {
        requested: String,
        available: Vec<String>,
    },
    #[error("the session has no selections in {0} space to test against")]
    NoCandidates(&'static str),
}

/// The result of testing one point: one row of `assert`'s output.
///
/// Carries no `schema` of its own — it is a row inside
/// [`crate::report::Report`], which versions the document. `hit` stays
/// here rather than rising to the envelope's `ok`, because a batch run
/// answers per line and a caller scoring a trajectory needs to know
/// *which* click missed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub point: Point,
    pub space: &'static str,
    /// The monitor index the point is local to; present only in monitor
    /// space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor: Option<usize>,
    pub hit: bool,
    /// Every region containing the point, in session (stacking) order —
    /// last is topmost. A miss against `--expect` still lists what the
    /// point *did* land in.
    pub contained_in: Vec<RegionRef>,
    /// Present on a miss: the closest relevant region, for partial credit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearest: Option<Nearest>,
}

/// A selection referenced by its position in the session file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegionRef {
    pub index: usize,
    pub label: String,
    pub shape: ToolKind,
    pub monitor: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Nearest {
    pub region: RegionRef,
    /// Distance in pixels from the point to the region's rotated bounding
    /// box. 0 means inside the bbox but outside the shape itself.
    pub bbox_distance_px: f64,
}

/// One selection viewed through the requested coordinate space.
struct Candidate<'a> {
    index: usize,
    record: &'a SelectionRecord,
    shape: Shape,
}

impl Candidate<'_> {
    fn rot_deg(&self) -> i32 {
        self.record.rot_deg.unwrap_or(0)
    }

    fn region_ref(&self) -> RegionRef {
        RegionRef {
            index: self.index,
            label: self.record.label.clone(),
            shape: self.record.shape,
            monitor: self.record.monitor,
        }
    }
}

/// Test `point` against the session's regions.
///
/// `expect` names the label that has to be hit for `hit` to be true — the
/// `--expect` flag, not `--label`. It deliberately does *not* filter what
/// gets reported: `contained_in` still lists every region the point landed
/// in, so a miss tells you what you hit instead of merely that you missed.
/// Matching is ASCII case-insensitive, as the window matcher is.
pub fn assess(
    session: &SessionFile,
    point: Point,
    space: Origin,
    expect: Option<&str>,
) -> Result<Verdict, VerdictError> {
    let candidates = candidates(session, space)?;
    if candidates.is_empty() {
        return Err(VerdictError::NoCandidates(space.label()));
    }
    if let Some(wanted) = expect {
        let known = candidates
            .iter()
            .any(|c| c.record.label.eq_ignore_ascii_case(wanted));
        if !known {
            return Err(VerdictError::UnknownLabel {
                requested: wanted.to_string(),
                available: candidate_labels(&candidates),
            });
        }
    }

    let contained: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.shape.hit_test_rotated(c.rot_deg(), point))
        .collect();
    let hit = expect.map_or(!contained.is_empty(), |wanted| {
        contained
            .iter()
            .any(|c| c.record.label.eq_ignore_ascii_case(wanted))
    });
    let nearest = if hit {
        None
    } else {
        nearest_relevant(&candidates, point, expect)
    };
    Ok(Verdict {
        point,
        space: space.label(),
        monitor: match space {
            Origin::Monitor(index) => Some(index),
            _ => None,
        },
        hit,
        contained_in: contained.iter().map(|c| c.region_ref()).collect(),
        nearest,
    })
}

/// The selections testable in `space`, each carrying the shape already
/// stored for that space.
fn candidates(session: &SessionFile, space: Origin) -> Result<Vec<Candidate<'_>>, VerdictError> {
    let records = session.selections.iter().enumerate();
    match space {
        Origin::Global => Ok(records
            .map(|(index, record)| Candidate {
                index,
                record,
                shape: record.global_px.clone(),
            })
            .collect()),
        Origin::Monitor(wanted) => {
            let available: Vec<usize> = session.monitors.iter().map(|m| m.index).collect();
            if !available.contains(&wanted) {
                return Err(VerdictError::UnknownMonitor {
                    requested: wanted,
                    available,
                });
            }
            Ok(records
                .filter(|(_, record)| record.monitor == wanted)
                .map(|(index, record)| Candidate {
                    index,
                    record,
                    shape: record.px.clone(),
                })
                .collect())
        }
        Origin::Window => {
            if session.target.is_none() {
                return Err(VerdictError::NoTarget);
            }
            Ok(records
                .filter_map(|(index, record)| {
                    record.window_px.clone().map(|shape| Candidate {
                        index,
                        record,
                        shape,
                    })
                })
                .collect())
        }
    }
}

/// The labels `--expect` could have named: those among the candidates
/// this space admits, not the whole session — a monitor-space question
/// cannot be answered by a selection on another monitor.
fn candidate_labels(candidates: &[Candidate]) -> Vec<String> {
    crate::session::distinct_labels(candidates.iter().map(|c| c.record))
}

/// The closest region the miss was measured against: the labeled ones when
/// a label was requested, otherwise all of them.
fn nearest_relevant(
    candidates: &[Candidate],
    point: Point,
    expect: Option<&str>,
) -> Option<Nearest> {
    candidates
        .iter()
        .filter(|c| expect.is_none_or(|wanted| c.record.label.eq_ignore_ascii_case(wanted)))
        .map(|c| Nearest {
            region: c.region_ref(),
            bbox_distance_px: bbox_distance(c.shape.rotated_bbox(c.rot_deg()), point),
        })
        .min_by(|a, b| a.bbox_distance_px.total_cmp(&b.bbox_distance_px))
}

/// Euclidean distance from `p` to the nearest pixel of `rect`, 0 inside.
/// i64/f64 throughout so extreme deserialized coordinates cannot overflow.
fn bbox_distance(rect: Rect, p: Point) -> f64 {
    let dx = axis_distance(p.x, rect.x, rect.w);
    let dy = axis_distance(p.y, rect.y, rect.h);
    f64::hypot(dx as f64, dy as f64)
}

/// Distance from `v` to the half-open interval `[start, start + len)` on
/// one axis, 0 inside — the same inclusion rule as `Rect::contains`.
fn axis_distance(v: i32, start: i32, len: i32) -> i64 {
    let v = i64::from(v);
    let start = i64::from(start);
    let end = start + i64::from(len) - 1;
    if v < start {
        return start - v;
    }
    if v > end {
        return v - end;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, Size};
    use crate::selection::Selection;
    use crate::session::{MonitorRecord, TargetRecord};

    fn monitor(index: usize, ox: i32, oy: i32) -> MonitorRecord {
        MonitorRecord {
            index,
            name: format!("Display {index}"),
            primary: index == 0,
            origin_px: Point::new(ox, oy),
            size_px: Size::new(1920, 1080),
            scale: 2.0,
        }
    }

    fn labeled(shape: Shape, monitor: usize, label: &str) -> Selection {
        let mut sel = Selection::new(shape, monitor);
        sel.label = label.into();
        sel
    }

    fn session(selections: &[Selection], target: Option<TargetRecord>) -> SessionFile {
        let crops: Vec<String> = (0..selections.len()).map(|i| format!("c{i}.png")).collect();
        SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0), monitor(1, 1920, 0)],
            selections,
            &crops,
            target,
        )
    }

    #[test]
    fn global_space_hits_through_the_monitor_origin() {
        // Local (10, 20) on monitor 1 sits at global (1930, 20).
        let file = session(
            &[labeled(Shape::Rect(Rect::new(10, 20, 30, 40)), 1, "submit")],
            None,
        );
        let hit = assess(&file, Point::new(1935, 25), Origin::Global, None).unwrap();
        assert!(hit.hit);
        assert_eq!(hit.contained_in[0].label, "submit");
        assert!(hit.nearest.is_none());
        let miss = assess(&file, Point::new(15, 25), Origin::Global, None).unwrap();
        assert!(!miss.hit);
    }

    #[test]
    fn monitor_space_tests_local_pixels_of_that_monitor_only() {
        let file = session(
            &[
                labeled(Shape::Rect(Rect::new(10, 20, 30, 40)), 1, "right"),
                labeled(Shape::Rect(Rect::new(10, 20, 30, 40)), 0, "left"),
            ],
            None,
        );
        let v = assess(&file, Point::new(15, 25), Origin::Monitor(1), None).unwrap();
        assert!(v.hit);
        assert_eq!(v.monitor, Some(1));
        // Both monitors hold an identical local rect; only monitor 1's is
        // eligible, so exactly one region contains the point.
        assert_eq!(v.contained_in.len(), 1);
        assert_eq!(v.contained_in[0].label, "right");
    }

    #[test]
    fn unknown_monitor_is_an_error_naming_the_real_ones() {
        let file = session(&[labeled(Shape::Rect(Rect::new(0, 0, 5, 5)), 0, "a")], None);
        let err = assess(&file, Point::new(0, 0), Origin::Monitor(7), None).unwrap_err();
        assert_eq!(
            err,
            VerdictError::UnknownMonitor {
                requested: 7,
                available: vec![0, 1],
            }
        );
    }

    #[test]
    fn monitor_with_no_selections_is_no_candidates_not_a_miss() {
        let file = session(&[labeled(Shape::Rect(Rect::new(0, 0, 5, 5)), 0, "a")], None);
        let err = assess(&file, Point::new(2, 2), Origin::Monitor(1), None).unwrap_err();
        assert_eq!(err, VerdictError::NoCandidates("monitor"));
    }

    #[test]
    fn window_space_needs_a_target_session() {
        let file = session(&[labeled(Shape::Rect(Rect::new(0, 0, 5, 5)), 0, "a")], None);
        let err = assess(&file, Point::new(2, 2), Origin::Window, None).unwrap_err();
        assert_eq!(err, VerdictError::NoTarget);
    }

    #[test]
    fn window_space_uses_window_relative_coords_and_skips_other_monitors() {
        let target = TargetRecord {
            app: "Editor".into(),
            title: "main.rs".into(),
            monitor: 0,
            origin_px: Point::new(400, 250),
            size_px: Size::new(800, 600),
        };
        let file = session(
            &[
                labeled(Shape::Rect(Rect::new(500, 300, 40, 20)), 0, "on target"),
                labeled(Shape::Rect(Rect::new(1, 1, 5, 5)), 1, "elsewhere"),
            ],
            Some(target),
        );
        // Window-relative: the rect sits at (100, 50) from the window origin.
        let v = assess(&file, Point::new(110, 55), Origin::Window, None).unwrap();
        assert!(v.hit);
        assert_eq!(v.contained_in.len(), 1);
        assert_eq!(v.contained_in[0].label, "on target");
    }

    #[test]
    fn label_filter_is_case_insensitive_and_sees_through_overlap() {
        let file = session(
            &[
                labeled(Shape::Rect(Rect::new(0, 0, 100, 100)), 0, "Cancel"),
                labeled(Shape::Rect(Rect::new(200, 0, 50, 50)), 0, "Submit"),
            ],
            None,
        );
        let v = assess(&file, Point::new(10, 10), Origin::Global, Some("SUBMIT")).unwrap();
        // The point is inside "Cancel", so the verdict is a labeled miss
        // that still reports what was actually hit — and how far the
        // wanted region is.
        assert!(!v.hit);
        assert_eq!(v.contained_in.len(), 1);
        assert_eq!(v.contained_in[0].label, "Cancel");
        let nearest = v.nearest.unwrap();
        assert_eq!(nearest.region.label, "Submit");
        assert!(nearest.bbox_distance_px > 0.0);

        let hit = assess(&file, Point::new(210, 10), Origin::Global, Some("submit")).unwrap();
        assert!(hit.hit);
    }

    #[test]
    fn unknown_label_is_an_error_listing_the_real_ones() {
        let file = session(
            &[
                labeled(Shape::Rect(Rect::new(0, 0, 5, 5)), 0, "Submit"),
                labeled(Shape::Rect(Rect::new(9, 9, 5, 5)), 0, "submit"),
                labeled(Shape::Rect(Rect::new(20, 20, 5, 5)), 0, ""),
            ],
            None,
        );
        let err = assess(&file, Point::new(0, 0), Origin::Global, Some("send")).unwrap_err();
        // Case-insensitive duplicates collapse; the unlabeled one is not
        // offered.
        assert_eq!(
            err,
            VerdictError::UnknownLabel {
                requested: "send".into(),
                available: vec!["Submit".into()],
            }
        );
    }

    #[test]
    fn empty_session_is_no_candidates() {
        let file = session(&[], None);
        let err = assess(&file, Point::new(0, 0), Origin::Global, None).unwrap_err();
        assert_eq!(err, VerdictError::NoCandidates("global"));
    }

    #[test]
    fn rotated_rect_is_tested_as_the_user_saw_it() {
        // A wide rect rotated 90° stands tall: x 25..35, y -5..35 visually.
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 10, 40, 10)), 0);
        sel.rot_deg = 90;
        let file = session(&[sel], None);
        let inside_rotated = Point::new(28, 27);
        let v = assess(&file, inside_rotated, Origin::Global, None).unwrap();
        assert!(v.hit, "point inside the rotated silhouette must hit");
        let inside_unrotated_only = Point::new(45, 15);
        let v = assess(&file, inside_unrotated_only, Origin::Global, None).unwrap();
        assert!(
            !v.hit,
            "the axis-aligned box no longer applies once rotated"
        );
    }

    #[test]
    fn circle_and_triangle_hits_use_their_own_geometry() {
        let file = session(
            &[
                labeled(
                    Shape::Circle {
                        cx: 50,
                        cy: 50,
                        r: 10,
                    },
                    0,
                    "dot",
                ),
                labeled(
                    Shape::Triangle {
                        ax: 200,
                        ay: 100,
                        bx: 150,
                        by: 200,
                        cx: 250,
                        cy: 200,
                    },
                    0,
                    "tri",
                ),
            ],
            None,
        );
        // On the circle's rim (inclusive), inside the triangle, and in each
        // shape's bbox corner — which its geometry excludes.
        assert!(
            assess(&file, Point::new(60, 50), Origin::Global, None)
                .unwrap()
                .hit
        );
        assert!(
            assess(&file, Point::new(200, 150), Origin::Global, None)
                .unwrap()
                .hit
        );
        let circle_corner = assess(&file, Point::new(41, 41), Origin::Global, None).unwrap();
        assert!(!circle_corner.hit);
        let tri_corner = assess(&file, Point::new(151, 101), Origin::Global, None).unwrap();
        assert!(!tri_corner.hit);
    }

    #[test]
    fn overlapping_hits_come_back_in_stacking_order() {
        let file = session(
            &[
                labeled(Shape::Rect(Rect::new(0, 0, 100, 100)), 0, "below"),
                labeled(Shape::Rect(Rect::new(0, 0, 50, 50)), 0, "above"),
            ],
            None,
        );
        let v = assess(&file, Point::new(10, 10), Origin::Global, None).unwrap();
        let labels: Vec<&str> = v.contained_in.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["below", "above"]);
        assert_eq!(v.contained_in[1].index, 1);
    }

    #[test]
    fn miss_distance_is_euclidean_to_the_bbox() {
        let file = session(
            &[labeled(Shape::Rect(Rect::new(10, 10, 20, 20)), 0, "box")],
            None,
        );
        // 3 left of x=10, 4 above y=10: a 3-4-5 triangle.
        let v = assess(&file, Point::new(7, 6), Origin::Global, None).unwrap();
        assert!(!v.hit);
        let nearest = v.nearest.unwrap();
        assert_eq!(nearest.region.label, "box");
        assert!((nearest.bbox_distance_px - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn miss_distance_uses_the_rotated_bbox() {
        // Rotated 90°, the wide rect's silhouette spans x 25..35 — a point
        // just right of it measures against that tall box, not the
        // original wide one.
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 10, 40, 10)), 0);
        sel.rot_deg = 90;
        let file = session(&[sel], None);
        let v = assess(&file, Point::new(40, 0), Origin::Global, None).unwrap();
        assert!(!v.hit);
        // The rotated bbox reaches x=35 at y=0; distance is 40-34=6 in x
        // per half-open inclusion, 0 in y — well under the unrotated
        // bbox's answer (which would include a 10px y gap).
        let d = v.nearest.unwrap().bbox_distance_px;
        assert!(
            d < 8.0,
            "distance {d} should measure the rotated silhouette"
        );
    }

    #[test]
    fn verdict_json_shape_is_stable() {
        let file = session(
            &[labeled(Shape::Rect(Rect::new(10, 10, 20, 20)), 0, "box")],
            None,
        );
        let hit = assess(&file, Point::new(15, 15), Origin::Global, None).unwrap();
        let json = serde_json::to_value(&hit).unwrap();
        assert!(
            json.get("schema").is_none(),
            "a verdict is a row; report::Report versions the document"
        );
        assert_eq!(json["space"], "global");
        assert_eq!(json["hit"], true);
        assert_eq!(json["point"]["x"], 15);
        assert_eq!(json["contained_in"][0]["label"], "box");
        assert!(json.get("monitor").is_none(), "global space has no monitor");
        assert!(json.get("nearest").is_none(), "hits carry no nearest");

        let miss = assess(&file, Point::new(500, 500), Origin::Monitor(0), None).unwrap();
        let json = serde_json::to_value(&miss).unwrap();
        assert_eq!(json["space"], "monitor");
        assert_eq!(json["monitor"], 0);
        assert_eq!(json["nearest"]["region"]["label"], "box");
    }

    #[test]
    fn space_labels_name_their_own_space() {
        assert_eq!(Origin::Global.label(), "global");
        assert_eq!(Origin::Monitor(3).label(), "monitor");
        assert_eq!(Origin::Window.label(), "window");
    }
}
