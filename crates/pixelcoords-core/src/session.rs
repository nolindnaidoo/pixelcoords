//! The versioned `session.json` schema.
//!
//! Coordinates are stored twice per selection: `px` (monitor-local physical
//! pixels — the authoritative space everything is drawn and cropped in) and
//! `global_px` (derived: monitor origin + local). Per-monitor `scale` lets
//! consumers reconstruct logical points; the schema does not pretend there
//! is a universal logical space.

use serde::{Deserialize, Serialize};

use crate::geometry::{Point, Shape, Size, ToolKind};
use crate::selection::Selection;

pub const SCHEMA_VERSION: u32 = 1;
pub const APP_NAME: &str = "pixelcoords";

/// How the session's frames were obtained. Optional in the schema —
/// sessions written before it existed simply lack it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    /// Whole monitors, no window attachment.
    Desktop,
    /// Attached to a window via `--target`: the `target` record carries
    /// the window's identity for re-attachment.
    Window,
    /// One window chosen in the desktop portal's picker (`--pick`); the
    /// portal reveals no window identity, so `target` is a placeholder.
    Pick,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionFile {
    pub schema: u32,
    pub app: AppInfo,
    pub created_utc: String,
    /// The OS the session was captured on — what a consumer needs to know
    /// before re-attaching to the recorded window or coordinates:
    /// `macos`, `windows`, `linux-x11`, or `linux-wayland`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub platform: Option<String>,
    /// See [`CaptureKind`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capture: Option<CaptureKind>,
    /// A human-friendly session name ("microsoft teams") for pickers and
    /// listings; the folder name identifies, this describes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    pub monitors: Vec<MonitorRecord>,
    /// Present when the session was captured with `--target`: the matched
    /// window's identity and bounds at freeze time.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<TargetRecord>,
    pub selections: Vec<SelectionRecord>,
}

/// The `--target` window as it stood at the instant of the freeze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRecord {
    pub app: String,
    pub title: String,
    pub monitor: usize,
    /// Window origin in that monitor's local physical pixels.
    pub origin_px: Point,
    pub size_px: Size,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorRecord {
    pub index: usize,
    pub name: String,
    pub primary: bool,
    pub origin_px: Point,
    pub size_px: Size,
    pub scale: f64,
}

/// How a saved [`MonitorRecord`] resolved against the displays attached
/// now. Both non-`Missing` variants carry an index into the candidate
/// slice that was searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorMatch {
    /// Same display, same geometry — safe to relocate against.
    Found(usize),
    /// A display of that name is attached, but its size or scale moved.
    /// Kept distinct from `Missing` so the caller can say *what* changed
    /// instead of "not attached", which would send the user hunting for a
    /// cable when the real cause was a resolution change.
    Changed(usize),
    /// Nothing attached carries that name.
    Missing,
}

/// Resolve a session's monitor against the live enumeration by **identity**
/// — name, size and scale — rather than by enumeration index.
///
/// The index is not stable: it shuffles across replugs, reboots and
/// dock/undock, so matching on it alone breaks re-attachment for a display
/// that never actually changed. Everything needed to recognize the panel is
/// already recorded at capture time; this uses it.
///
/// Ties (two of the same model attached at once) break toward the
/// candidate whose index equals the record's, then toward the lowest index.
/// Preferring the recorded index first means the common case — nothing
/// moved, or something *else* was replugged — resolves to the same panel it
/// did before, rather than to whichever twin happens to enumerate first.
pub fn match_monitor(record: &MonitorRecord, candidates: &[MonitorRecord]) -> MonitorMatch {
    // Within a pool of equally valid candidates: the one that also carries
    // the recorded index, else the lowest index. Empty pool yields None,
    // which is what lets the two calls below fall through in order.
    let best = |pool: &[usize]| -> Option<usize> {
        pool.iter()
            .copied()
            .find(|&i| candidates[i].index == record.index)
            .or_else(|| pool.iter().copied().min_by_key(|&i| candidates[i].index))
    };

    let named: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.name == record.name)
        .map(|(i, _)| i)
        .collect();
    if named.is_empty() {
        return MonitorMatch::Missing;
    }
    let exact: Vec<usize> = named
        .iter()
        .copied()
        .filter(|&i| {
            let c = &candidates[i];
            // Scale is a float off the platform API; compare it the way the
            // rest of this codebase does rather than with `==`.
            c.size_px == record.size_px && (c.scale - record.scale).abs() < f64::EPSILON
        })
        .collect();
    if let Some(i) = best(&exact) {
        return MonitorMatch::Found(i);
    }
    best(&named).map_or(MonitorMatch::Missing, MonitorMatch::Changed)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRecord {
    pub shape: ToolKind,
    pub label: String,
    pub monitor: usize,
    pub px: Shape,
    pub global_px: Shape,
    /// Rotation in degrees (clockwise, `1..360`) about the bbox center of
    /// `px`. Absent means unrotated. Never present for triangles — their
    /// rotation is baked into the stored vertices — nor circles.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rot_deg: Option<i32>,
    /// Coordinates relative to the target window's top-left; present only
    /// in `--target` sessions, for selections on the target's monitor.
    /// Negative values mean the selection lies outside the window.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub window_px: Option<Shape>,
    /// File name of this selection's PNG crop, relative to the session dir.
    pub crop: String,
}

impl SessionFile {
    /// Assemble a session. `created_utc` is supplied by the caller (this
    /// crate has no clock); `crops` pairs 1:1 with `selections`.
    pub fn build(
        app_version: &str,
        created_utc: String,
        monitors: Vec<MonitorRecord>,
        selections: &[Selection],
        crops: &[String],
        target: Option<TargetRecord>,
    ) -> Self {
        assert_eq!(selections.len(), crops.len(), "one crop name per selection");
        let records = selections
            .iter()
            .zip(crops)
            .map(|(s, crop)| {
                let origin = monitors
                    .iter()
                    .find(|m| m.index == s.monitor)
                    .map_or(Point::new(0, 0), |m| m.origin_px);
                // Triangles bake rotation into their vertices (exact);
                // rects keep an axis-aligned box plus rot_deg metadata.
                let shape = s.shape.with_rotation_baked(s.rot_deg);
                let rot_deg = match shape {
                    Shape::Rect(_) | Shape::Ellipse { .. } => {
                        Some(crate::geometry::normalize_deg(s.rot_deg)).filter(|d| *d != 0)
                    }
                    _ => None,
                };
                let window_px = target
                    .as_ref()
                    .filter(|t| t.monitor == s.monitor)
                    .map(|t| shape.translated(-t.origin_px.x, -t.origin_px.y));
                SelectionRecord {
                    shape: shape.kind(),
                    label: s.label.clone(),
                    monitor: s.monitor,
                    global_px: shape.translated(origin.x, origin.y),
                    px: shape,
                    rot_deg,
                    window_px,
                    crop: crop.clone(),
                }
            })
            .collect();
        Self {
            schema: SCHEMA_VERSION,
            app: AppInfo {
                name: APP_NAME.to_string(),
                version: app_version.to_string(),
            },
            created_utc,
            platform: None,
            capture: None,
            name: None,
            monitors,
            target,
            selections: records,
        }
    }

    /// Stamp provenance onto a built session. A resumed session passes
    /// through what it loaded, so a file edited on another machine keeps
    /// saying where it was captured.
    #[must_use]
    pub fn with_meta(
        mut self,
        platform: Option<String>,
        capture: Option<CaptureKind>,
        name: Option<String>,
    ) -> Self {
        self.platform = platform;
        self.capture = capture;
        self.name = name;
        self
    }
}

/// Rebuild editable selections from a saved session — the inverse of
/// [`SessionFile::build`]. Shapes come back in monitor-local px; rects
/// reclaim their `rot_deg` metadata, triangles keep rotation baked in
/// their vertices (their records never carry `rot_deg`), and circles are
/// rotation-free. Feed the result to `SelectionSet::seed`.
///
/// In a target session, selections whose shape falls outside the window's
/// rect are **dropped**. Older builds recorded whatever the user marked
/// on the whole monitor, including junk outside the window, and their
/// stored `window_px` came out with negative coordinates. This build
/// refuses to let a user act on those, so a resumed session should not
/// bring them back. The dropped labels are returned so the caller can
/// tell the user what happened.
pub fn restore_selections(file: &SessionFile) -> (Vec<Selection>, Vec<String>) {
    let target_rect = file.target.as_ref().map(|t| {
        (
            t.monitor,
            crate::geometry::Rect::new(0, 0, t.size_px.w, t.size_px.h),
        )
    });
    let mut kept = Vec::with_capacity(file.selections.len());
    let mut dropped = Vec::new();
    for record in &file.selections {
        // Compare against `window_px` (already translated to window-local)
        // rather than reconstructing coordinates from `px`; the two must
        // agree for a valid record, and window_px is the primary frame in
        // a target session.
        if let Some((monitor, rect)) = target_rect
            && record.monitor == monitor
        {
            let Some(shape) = &record.window_px else {
                dropped.push(record.label.clone());
                continue;
            };
            let bbox = shape.bbox();
            let inside = bbox.x >= rect.x
                && bbox.y >= rect.y
                && bbox.x + bbox.w <= rect.x + rect.w
                && bbox.y + bbox.h <= rect.y + rect.h;
            if !inside {
                dropped.push(record.label.clone());
                continue;
            }
        }
        kept.push(Selection {
            shape: record.px.clone(),
            label: record.label.clone(),
            monitor: record.monitor,
            rot_deg: record.rot_deg.unwrap_or(0),
        });
    }
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

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

    /// A display identified by name, so tests can express "the same panel,
    /// enumerated somewhere else".
    fn panel(index: usize, name: &str, w: i32, h: i32, scale: f64) -> MonitorRecord {
        MonitorRecord {
            index,
            name: name.into(),
            primary: index == 0,
            origin_px: Point::new(0, 0),
            size_px: Size::new(w, h),
            scale,
        }
    }

    #[test]
    fn a_replug_that_reorders_enumeration_still_finds_the_panel() {
        // The bug this matcher exists for: same two displays, swapped
        // enumeration order. Index-based lookup would hand back the wrong
        // panel — or nothing.
        let saved = panel(1, "DELL U2723QE", 3840, 2160, 1.0);
        let live = [
            panel(0, "DELL U2723QE", 3840, 2160, 1.0),
            panel(1, "Built-in Retina Display", 3600, 2338, 2.0),
        ];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Found(0));
    }

    #[test]
    fn nothing_moved_resolves_to_the_recorded_index() {
        let saved = panel(1, "Built-in Retina Display", 3600, 2338, 2.0);
        let live = [
            panel(0, "DELL U2723QE", 3840, 2160, 1.0),
            panel(1, "Built-in Retina Display", 3600, 2338, 2.0),
        ];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Found(1));
    }

    #[test]
    fn identical_twins_break_toward_the_recorded_index_then_the_lowest() {
        let live = [
            panel(0, "DELL U2723QE", 3840, 2160, 1.0),
            panel(1, "DELL U2723QE", 3840, 2160, 1.0),
        ];
        // The recorded index is present among the twins, so it wins.
        let saved_one = panel(1, "DELL U2723QE", 3840, 2160, 1.0);
        assert_eq!(match_monitor(&saved_one, &live), MonitorMatch::Found(1));

        // The recorded index is gone; the tie breaks toward the lowest,
        // deterministically rather than on enumeration luck.
        let saved_seven = panel(7, "DELL U2723QE", 3840, 2160, 1.0);
        assert_eq!(match_monitor(&saved_seven, &live), MonitorMatch::Found(0));
    }

    #[test]
    fn the_lowest_index_wins_regardless_of_enumeration_order() {
        // Candidates are searched in slice order, but the tie-break is on
        // the recorded index — so a twin listed first does not win by
        // position alone.
        let saved = panel(9, "DELL U2723QE", 3840, 2160, 1.0);
        let live = [
            panel(3, "DELL U2723QE", 3840, 2160, 1.0),
            panel(1, "DELL U2723QE", 3840, 2160, 1.0),
        ];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Found(1));
    }

    #[test]
    fn a_resized_display_is_changed_not_missing() {
        // Template matching survives movement, not a resolution change —
        // but the user needs to hear "it changed", not "it is unplugged".
        let saved = panel(0, "DELL U2723QE", 3840, 2160, 1.0);
        let live = [panel(0, "DELL U2723QE", 2560, 1440, 1.0)];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Changed(0));
    }

    #[test]
    fn a_rescaled_display_is_changed_not_missing() {
        let saved = panel(0, "Built-in Retina Display", 3600, 2338, 2.0);
        let live = [panel(0, "Built-in Retina Display", 3600, 2338, 1.0)];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Changed(0));
    }

    #[test]
    fn an_exact_match_beats_a_changed_one_of_the_same_name() {
        // Two panels share a name; one still matches the session exactly.
        // Identity must win over the recorded index.
        let saved = panel(0, "DELL U2723QE", 3840, 2160, 1.0);
        let live = [
            panel(0, "DELL U2723QE", 2560, 1440, 1.0),
            panel(1, "DELL U2723QE", 3840, 2160, 1.0),
        ];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Found(1));
    }

    #[test]
    fn an_absent_display_is_missing_even_when_something_else_fits() {
        // Same geometry, different panel: not the display the session used.
        let saved = panel(0, "DELL U2723QE", 3840, 2160, 1.0);
        let live = [panel(0, "LG UltraFine", 3840, 2160, 1.0)];
        assert_eq!(match_monitor(&saved, &live), MonitorMatch::Missing);
    }

    #[test]
    fn no_displays_at_all_is_missing() {
        let saved = panel(0, "DELL U2723QE", 3840, 2160, 1.0);
        assert_eq!(match_monitor(&saved, &[]), MonitorMatch::Missing);
    }

    #[test]
    fn global_is_origin_plus_local() {
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 1);
        sel.label = "target".into();
        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0), monitor(1, 1920, 0)],
            &[sel],
            &["crop-0-target.png".into()],
            None,
        );
        assert_eq!(
            file.selections[0].px,
            Shape::Rect(Rect::new(10, 20, 30, 40))
        );
        assert_eq!(
            file.selections[0].global_px,
            Shape::Rect(Rect::new(1930, 20, 30, 40))
        );
    }

    #[test]
    fn json_shape_is_stable() {
        let sel = Selection::new(Shape::Circle { cx: 5, cy: 6, r: 7 }, 0);
        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["crop-0.png".into()],
            None,
        );
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["app"]["name"], "pixelcoords");
        assert_eq!(json["selections"][0]["shape"], "circle");
        assert_eq!(json["selections"][0]["px"]["cx"], 5);
        assert_eq!(json["selections"][0]["px"]["r"], 7);
        assert_eq!(json["monitors"][0]["scale"], 2.0);
    }

    #[test]
    fn round_trips_through_json() {
        let sel = Selection::new(Shape::Rect(Rect::new(1, 2, 3, 4)), 0);
        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["c.png".into()],
            None,
        );
        let json = serde_json::to_string(&file).unwrap();
        let back: SessionFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, file);
    }

    #[test]
    fn target_yields_window_relative_coords() {
        let on_target = Selection::new(Shape::Rect(Rect::new(500, 300, 40, 20)), 0);
        let elsewhere = Selection::new(Shape::Rect(Rect::new(1, 1, 5, 5)), 1);
        let target = TargetRecord {
            app: "Notepad".into(),
            title: "notes.txt".into(),
            monitor: 0,
            origin_px: Point::new(400, 250),
            size_px: Size::new(800, 600),
        };
        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0), monitor(1, 1920, 0)],
            &[on_target, elsewhere],
            &["a.png".into(), "b.png".into()],
            Some(target),
        );
        assert_eq!(
            file.selections[0].window_px,
            Some(Shape::Rect(Rect::new(100, 50, 40, 20)))
        );
        assert_eq!(file.selections[1].window_px, None);
        assert_eq!(file.target.as_ref().unwrap().title, "notes.txt");
    }

    #[test]
    fn rotation_is_metadata_for_rects_and_baked_for_triangles() {
        let mut rect_sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 0);
        rect_sel.rot_deg = 45;
        let mut tri_sel = Selection::new(
            Shape::Triangle {
                ax: 200,
                ay: 100,
                bx: 100,
                by: 200,
                cx: 300,
                cy: 200,
            },
            0,
        );
        tri_sel.rot_deg = 180;
        let plain = Selection::new(Shape::Rect(Rect::new(1, 2, 3, 4)), 0);

        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0)],
            &[rect_sel, tri_sel, plain],
            &["a.png".into(), "b.png".into(), "c.png".into()],
            None,
        );
        // Rect: axis-aligned px + rot_deg metadata.
        assert_eq!(
            file.selections[0].px,
            Shape::Rect(Rect::new(10, 20, 30, 40))
        );
        assert_eq!(file.selections[0].rot_deg, Some(45));
        // Triangle: rotation baked into vertices, no rot_deg.
        assert_eq!(file.selections[1].rot_deg, None);
        assert_eq!(
            file.selections[1].px,
            Shape::Triangle {
                ax: 200,
                ay: 200,
                bx: 300,
                by: 100,
                cx: 100,
                cy: 100,
            }
        );
        // Unrotated: no rot_deg key at all in the JSON.
        let json = serde_json::to_value(&file).unwrap();
        assert!(json["selections"][2].get("rot_deg").is_none());
        assert_eq!(json["selections"][0]["rot_deg"], 45);
    }

    #[test]
    fn untargeted_session_omits_target_fields_in_json() {
        let sel = Selection::new(Shape::Rect(Rect::new(1, 2, 3, 4)), 0);
        let file = SessionFile::build(
            "0.1.0",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["c.png".into()],
            None,
        );
        let json = serde_json::to_value(&file).unwrap();
        assert!(json.get("target").is_none());
        assert!(json["selections"][0].get("window_px").is_none());
    }

    #[test]
    fn restore_then_rebuild_reproduces_every_selection_record() {
        // A rotated rect (metadata), a rotated triangle (baked), a circle,
        // and a label: build -> restore -> build must reproduce the
        // records exactly, which is what makes resume lossless.
        let mut rect_sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 0);
        rect_sel.rot_deg = 45;
        rect_sel.label = "spun".into();
        let mut tri_sel = Selection::new(
            Shape::Triangle {
                ax: 200,
                ay: 100,
                bx: 100,
                by: 200,
                cx: 300,
                cy: 200,
            },
            1,
        );
        tri_sel.rot_deg = 90;
        let circle_sel = Selection::new(Shape::Circle { cx: 9, cy: 9, r: 5 }, 0);

        let monitors = vec![monitor(0, 0, 0), monitor(1, 1920, 0)];
        let crops: Vec<String> = vec!["a.png".into(), "b.png".into(), "c.png".into()];
        let first = SessionFile::build(
            "test",
            "t".into(),
            monitors.clone(),
            &[rect_sel, tri_sel, circle_sel],
            &crops,
            None,
        );
        let (restored, _) = restore_selections(&first);
        let second = SessionFile::build("test", "t".into(), monitors, &restored, &crops, None);
        assert_eq!(first.selections, second.selections);
    }

    #[test]
    fn provenance_is_optional_and_survives_round_trips() {
        let sel = Selection::new(Shape::Rect(Rect::new(1, 2, 3, 4)), 0);
        let file = SessionFile::build(
            "test",
            "t".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["c.png".into()],
            None,
        )
        .with_meta(
            Some("macos".into()),
            Some(CaptureKind::Desktop),
            Some("microsoft teams".into()),
        );
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(json["platform"], "macos");
        assert_eq!(json["capture"], "desktop");
        assert_eq!(json["name"], "microsoft teams");
        let back: SessionFile = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(back, file);

        // A session written before these fields existed still parses,
        // and one built without them omits the keys entirely.
        let old = r#"{"schema":1,"app":{"name":"pixelcoords","version":"0"},
            "created_utc":"t","monitors":[],"selections":[]}"#;
        let parsed: SessionFile = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.platform, None);
        assert_eq!(parsed.capture, None);
        assert_eq!(parsed.name, None);
        let bare = SessionFile::build("test", "t".into(), vec![], &[], &[], None);
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("platform").is_none());
        assert!(json.get("capture").is_none());
        assert!(json.get("name").is_none());
    }

    #[test]
    fn untagged_shape_deserializes_by_fields() {
        let rect: Shape = serde_json::from_str(r#"{"x":1,"y":2,"w":3,"h":4}"#).unwrap();
        assert_eq!(rect, Shape::Rect(Rect::new(1, 2, 3, 4)));
        let circle: Shape = serde_json::from_str(r#"{"cx":1,"cy":2,"r":3}"#).unwrap();
        assert_eq!(circle, Shape::Circle { cx: 1, cy: 2, r: 3 });
        let ellipse: Shape = serde_json::from_str(r#"{"cx":1,"cy":2,"rx":3,"ry":4}"#).unwrap();
        assert_eq!(
            ellipse,
            Shape::Ellipse {
                cx: 1,
                cy: 2,
                rx: 3,
                ry: 4,
            }
        );
    }

    #[test]
    fn poly_records_serialize_their_vertices_and_round_trip() {
        let sel = Selection::new(
            Shape::Poly {
                points: vec![Point::new(1, 2), Point::new(9, 2), Point::new(5, 9)],
            },
            0,
        );
        let file = SessionFile::build(
            "test",
            "t".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["c.png".into()],
            None,
        );
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(json["selections"][0]["shape"], "poly");
        assert_eq!(json["selections"][0]["px"]["points"][2]["x"], 5);
        assert_eq!(
            json["selections"][0].get("rot_deg"),
            None,
            "poly rotation is baked, never metadata"
        );
        let back: SessionFile = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(back, file);
    }

    #[test]
    fn ellipse_records_carry_rotation_metadata_like_rects() {
        let mut sel = Selection::new(
            Shape::Ellipse {
                cx: 50,
                cy: 40,
                rx: 20,
                ry: 10,
            },
            0,
        );
        sel.rot_deg = 30;
        let file = SessionFile::build(
            "test",
            "t".into(),
            vec![monitor(0, 0, 0)],
            &[sel],
            &["c.png".into()],
            None,
        );
        assert_eq!(file.selections[0].rot_deg, Some(30));
        let json = serde_json::to_value(&file).unwrap();
        assert_eq!(json["selections"][0]["shape"], "ellipse");
        assert_eq!(json["selections"][0]["px"]["rx"], 20);
        let back: SessionFile = serde_json::from_str(&json.to_string()).unwrap();
        assert_eq!(back, file);
    }
}
