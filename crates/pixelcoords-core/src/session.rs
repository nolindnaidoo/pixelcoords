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
pub fn restore_selections(file: &SessionFile) -> Vec<Selection> {
    file.selections
        .iter()
        .map(|record| Selection {
            shape: record.px.clone(),
            label: record.label.clone(),
            monitor: record.monitor,
            rot_deg: record.rot_deg.unwrap_or(0),
        })
        .collect()
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
        let restored = restore_selections(&first);
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
