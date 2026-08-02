//! The versioned `session.json` schema.
//!
//! Coordinates are stored twice per selection: `px` (monitor-local physical
//! pixels — the authoritative space everything is drawn and cropped in) and
//! `global_px` (derived: monitor origin + local). Per-monitor `scale` lets
//! consumers reconstruct logical points; the schema does not pretend there
//! is a universal logical space.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::geometry::{Point, Shape, Size, ToolKind};
use crate::selection::Selection;

pub const SCHEMA_VERSION: u32 = 1;
pub const APP_NAME: &str = "pixelcoords";

pub use crate::geometry::MAX_COORD;

/// Why a session file is not usable, even though it parsed.
///
/// Parsing proves the shape; this proves the values mean something. The
/// split matters because `serde` will happily accept `"scale": 0.0` — it
/// is a valid `f64` — and every consumer downstream then divides by it.
#[derive(Debug, Error, PartialEq)]
pub enum SessionError {
    #[error("monitor {index} ({name:?}) has scale {scale}, which is not a positive finite number")]
    Scale {
        index: usize,
        name: String,
        scale: f64,
    },
    #[error("monitor {index} ({name:?}) has size {w}x{h}; a display cannot be empty")]
    MonitorSize {
        index: usize,
        name: String,
        w: i32,
        h: i32,
    },
    #[error("the target window has size {w}x{h}; a window cannot be empty")]
    TargetSize { w: i32, h: i32 },
    #[error(
        "{what} carries the coordinate {value}, beyond the +/-{MAX_COORD} a session may describe"
    )]
    Coordinate { what: String, value: i32 },
}

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
    /// Two-point measurements, when any were taken. A separate top-level
    /// array rather than a shape kind: a measure has no interior, so
    /// every consumer of `selections` would have to special-case one.
    /// Additive and omitted when empty, so the schema does not move and
    /// sessions without measures look exactly as they always did.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub measures: Vec<MeasureRecord>,
}

/// One measurement, with its derived values precomputed so a consumer
/// never re-derives geometry to read a number off a ruler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasureRecord {
    pub label: String,
    pub monitor: usize,
    /// Endpoints in monitor-local physical pixels.
    pub px: LineRecord,
    /// The same endpoints on the global desktop grid.
    pub global_px: LineRecord,
    pub length_px: f64,
    pub dx: i32,
    pub dy: i32,
    /// Degrees in `[0, 360)`, `0` pointing right along +X, increasing
    /// clockwise because screen Y grows downward.
    pub angle_deg: f64,
}

/// A measure's two endpoints, flat rather than nested, so a consumer
/// reads `ax` instead of `a.x` for a thing that is always exactly two
/// points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRecord {
    pub ax: i32,
    pub ay: i32,
    pub bx: i32,
    pub by: i32,
}

impl From<crate::geometry::Line> for LineRecord {
    fn from(line: crate::geometry::Line) -> Self {
        Self {
            ax: line.a.x,
            ay: line.a.y,
            bx: line.b.x,
            by: line.b.y,
        }
    }
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
    /// The captured pixel at this selection's click point, as uppercase
    /// `#RRGGBB`.
    ///
    /// The same interior point `assert` and `emit` aim at, so it
    /// describes the pixel automation will actually click — a consumer
    /// can sanity-check that the button was still blue when it was
    /// marked. Optional and additive: absent in sessions written before
    /// it existed, and the schema does not move for it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,
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
                    // Filled by `with_colors`: the sample comes from the
                    // frozen frame, which this crate never sees.
                    color: None,
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
            measures: Vec::new(),
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

    /// Attach the session's measurements, converting each to global
    /// coordinates through its own monitor's origin and precomputing the
    /// derived values.
    ///
    /// A builder rather than an argument to `build` for the same reason
    /// the colors are: a session without measures is an ordinary session,
    /// so it cannot be required of every caller.
    #[must_use]
    pub fn with_measures(mut self, measures: &[crate::selection::Measure]) -> Self {
        self.measures = measures
            .iter()
            .map(|m| {
                let origin = self
                    .monitors
                    .iter()
                    .find(|mon| mon.index == m.monitor)
                    .map_or(Point::new(0, 0), |mon| mon.origin_px);
                let (dx, dy) = m.line.delta();
                MeasureRecord {
                    label: m.label.clone(),
                    monitor: m.monitor,
                    px: m.line.into(),
                    global_px: m.line.translated(origin.x, origin.y).into(),
                    length_px: m.line.length(),
                    dx,
                    dy,
                    angle_deg: m.line.angle_deg(),
                }
            })
            .collect();
        self
    }

    /// Attach the sampled click-point color to each selection, in the
    /// same order [`Self::build`] took them.
    ///
    /// Separate from `build` because the color comes from the frozen
    /// frames, which only the caller holds — and because a session
    /// without colors is a valid session, so this cannot be a required
    /// argument. A shorter slice leaves the rest without a color rather
    /// than shifting them onto the wrong selection.
    #[must_use]
    pub fn with_colors(mut self, colors: &[Option<String>]) -> Self {
        for (record, color) in self.selections.iter_mut().zip(colors) {
            record.color.clone_from(color);
        }
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

/// Every raw coordinate a shape carries, in no particular order.
///
/// Deliberately arithmetic-free. `bbox()` is the natural way to ask a
/// shape where it is, and it is exactly the wrong tool here: computing a
/// bounding box on an out-of-range shape performs the very subtraction
/// this check exists to prevent.
fn raw_values(shape: &Shape) -> Vec<i32> {
    match shape {
        Shape::Rect(r) => vec![r.x, r.y, r.w, r.h],
        Shape::Circle { cx, cy, r } => vec![*cx, *cy, *r],
        Shape::Ellipse { cx, cy, rx, ry } => vec![*cx, *cy, *rx, *ry],
        Shape::Triangle {
            ax,
            ay,
            bx,
            by,
            cx,
            cy,
        } => vec![*ax, *ay, *bx, *by, *cx, *cy],
        Shape::Poly { points } => points.iter().flat_map(|p| [p.x, p.y]).collect(),
    }
}

fn in_range(values: &[i32], what: &str) -> Result<(), SessionError> {
    for &value in values {
        if value.abs() > MAX_COORD {
            return Err(SessionError::Coordinate {
                what: what.to_string(),
                value,
            });
        }
    }
    Ok(())
}

impl SessionFile {
    /// Check that a parsed session describes something a coordinate can
    /// mean.
    ///
    /// Called at the load seam so every command and `doctor` refuse the
    /// same file, the way `Config`'s resolution is checked once when the
    /// config is read rather than at each use. A file that fails here is
    /// malformed, not merely unusual, and the caller reports it as such.
    pub fn validate(&self) -> Result<(), SessionError> {
        for monitor in &self.monitors {
            if !monitor.scale.is_finite() || monitor.scale <= 0.0 {
                return Err(SessionError::Scale {
                    index: monitor.index,
                    name: monitor.name.clone(),
                    scale: monitor.scale,
                });
            }
            if monitor.size_px.w <= 0 || monitor.size_px.h <= 0 {
                return Err(SessionError::MonitorSize {
                    index: monitor.index,
                    name: monitor.name.clone(),
                    w: monitor.size_px.w,
                    h: monitor.size_px.h,
                });
            }
            let label = format!("monitor {}", monitor.index);
            in_range(
                &[
                    monitor.origin_px.x,
                    monitor.origin_px.y,
                    monitor.size_px.w,
                    monitor.size_px.h,
                ],
                &label,
            )?;
        }
        if let Some(target) = &self.target {
            if target.size_px.w <= 0 || target.size_px.h <= 0 {
                return Err(SessionError::TargetSize {
                    w: target.size_px.w,
                    h: target.size_px.h,
                });
            }
            in_range(
                &[
                    target.origin_px.x,
                    target.origin_px.y,
                    target.size_px.w,
                    target.size_px.h,
                ],
                "the target window",
            )?;
        }
        for (index, record) in self.selections.iter().enumerate() {
            let label = format!("selection {index}");
            in_range(&raw_values(&record.px), &label)?;
            in_range(&raw_values(&record.global_px), &label)?;
            if let Some(window) = &record.window_px {
                in_range(&raw_values(window), &label)?;
            }
        }
        for (index, record) in self.measures.iter().enumerate() {
            let label = format!("measure {index}");
            for line in [&record.px, &record.global_px] {
                in_range(&[line.ax, line.ay, line.bx, line.by], &label)?;
            }
        }
        Ok(())
    }
}

/// The measures a saved session carries, back as editable rulers.
///
/// Unlike `restore_selections` this drops nothing: a measure has no crop
/// to orphan and no window-relative frame to fall outside of, so there is
/// nothing a target session could invalidate. Derived values are
/// recomputed from the endpoints on the next save rather than trusted, so
/// a hand-edited file cannot smuggle a length that does not match its
/// line.
#[must_use]
pub fn restore_measures(file: &SessionFile) -> Vec<crate::selection::Measure> {
    file.measures
        .iter()
        .map(|record| crate::selection::Measure {
            line: crate::geometry::Line::new(
                Point::new(record.px.ax, record.px.ay),
                Point::new(record.px.bx, record.px.by),
            ),
            label: record.label.clone(),
            monitor: record.monitor,
        })
        .collect()
}

/// The selections a `--label` restricts to, paired with their index in
/// the session — the identity every report row carries. `None` selects
/// everything. Matching is ASCII case-insensitive, as the window matcher
/// is.
///
/// An empty result is the caller's to report: each command refuses in its
/// own error type, and only the caller knows whether an empty *session*
/// or an unmatched *label* is the cause. Pair it with `distinct_labels`
/// to say what the session does carry.
pub fn select_by_label<'a>(
    session: &'a SessionFile,
    label: Option<&str>,
) -> Vec<(usize, &'a SelectionRecord)> {
    session
        .selections
        .iter()
        .enumerate()
        .filter(|(_, record)| label.is_none_or(|want| record.label.eq_ignore_ascii_case(want)))
        .collect()
}

/// The labels a `--label` could have matched, in session order,
/// deduplicated ASCII case-insensitively; unlabeled selections contribute
/// nothing.
///
/// Takes an iterator rather than the session because the caller decides
/// what "could have matched" means: `assert` lists labels among its
/// *space-filtered* candidates, since a monitor-space question cannot be
/// answered by a selection on another monitor.
pub fn distinct_labels<'a>(records: impl Iterator<Item = &'a SelectionRecord>) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    for record in records {
        if record.label.is_empty() {
            continue;
        }
        if labels.iter().any(|l| l.eq_ignore_ascii_case(&record.label)) {
            continue;
        }
        labels.push(record.label.clone());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    #[test]
    fn measures_are_absent_from_a_session_that_has_none() {
        let file = labeled(&["submit"]);
        let json = serde_json::to_value(&file).unwrap();
        assert!(
            json.get("measures").is_none(),
            "a session without measures must look exactly as it always did"
        );
        assert_eq!(json["schema"], 1);
    }

    #[test]
    fn a_measure_records_its_globals_and_derived_values() {
        use crate::geometry::Line;
        use crate::selection::Measure;
        // Monitor 1 sits at global x=1920, so the local ruler at x=100
        // reports globals 1920 higher.
        let mut m = Measure::new(Line::new(Point::new(100, 80), Point::new(262, 80)), 1);
        m.label = "toolbar-gap".into();
        let file = SessionFile::build(
            "test",
            "2026-08-01T00:00:00Z".into(),
            vec![monitor(0, 0, 0), monitor(1, 1920, 0)],
            &[],
            &[],
            None,
        )
        .with_measures(&[m]);

        let json = serde_json::to_value(&file).unwrap();
        let rec = &json["measures"][0];
        assert_eq!(rec["label"], "toolbar-gap");
        assert_eq!(rec["monitor"], 1);
        assert_eq!(rec["px"]["ax"], 100);
        assert_eq!(rec["global_px"]["ax"], 2020, "origin added");
        assert_eq!(rec["global_px"]["bx"], 2182);
        assert_eq!(rec["dx"], 162);
        assert_eq!(rec["dy"], 0);
        assert_eq!(rec["length_px"], 162.0);
        assert_eq!(rec["angle_deg"], 0.0);
        assert_eq!(json["schema"], 1, "measures are additive");
    }

    #[test]
    fn stored_derived_values_match_recomputing_them() {
        use crate::geometry::Line;
        use crate::selection::Measure;
        // The reason they are stored at all is so a consumer never has to
        // re-derive geometry — which is only safe if they agree.
        let line = Line::new(Point::new(-30, 12), Point::new(45, -60));
        let file = SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None)
            .with_measures(&[Measure::new(line, 0)]);
        let rec = &file.measures[0];
        assert!((rec.length_px - line.length()).abs() < f64::EPSILON);
        assert!((rec.angle_deg - line.angle_deg()).abs() < f64::EPSILON);
        assert_eq!((rec.dx, rec.dy), line.delta());
    }

    #[test]
    fn restoring_measures_recovers_every_ruler_and_recomputes_nothing_wrong() {
        let base =
            || SessionFile::build("test", "t".into(), vec![monitor(0, 100, 0)], &[], &[], None);
        let file = base().with_measures(&[
            crate::selection::Measure::new(
                crate::geometry::Line::new(Point::new(10, 20), Point::new(40, 60)),
                0,
            ),
            crate::selection::Measure {
                line: crate::geometry::Line::new(Point::new(1, 2), Point::new(3, 4)),
                label: "gutter".into(),
                monitor: 0,
            },
        ]);

        let restored = restore_measures(&file);

        assert_eq!(restored.len(), 2);
        assert_eq!(
            restored[0].line,
            crate::geometry::Line::new(Point::new(10, 20), Point::new(40, 60)),
            "monitor-local endpoints, not the global ones"
        );
        assert_eq!(restored[1].label, "gutter");
        // A resave reproduces the same records: the round trip is closed.
        assert_eq!(base().with_measures(&restored).measures, file.measures);
    }

    #[test]
    fn an_empty_target_window_is_refused() {
        let mut file =
            SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        file.target = Some(TargetRecord {
            app: "App".into(),
            title: "T".into(),
            monitor: 0,
            origin_px: Point::new(0, 0),
            size_px: Size::new(0, 400),
        });
        assert!(matches!(
            file.validate(),
            Err(SessionError::TargetSize { w: 0, h: 400 })
        ));
    }

    #[test]
    fn a_target_window_past_the_bound_is_refused() {
        let mut file =
            SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        file.target = Some(TargetRecord {
            app: "App".into(),
            title: "T".into(),
            monitor: 0,
            origin_px: Point::new(MAX_COORD + 1, 0),
            size_px: Size::new(400, 400),
        });
        assert!(matches!(
            file.validate(),
            Err(SessionError::Coordinate { .. })
        ));
    }

    #[test]
    fn every_refusal_names_the_field_and_the_value() {
        // These strings are what a user sees when a session is rejected,
        // so they are output and get tested like output. A message that
        // says "invalid session" and stops sends someone reading JSON by
        // hand with no idea which number to look at.
        let cases = [
            (
                SessionError::Scale {
                    index: 2,
                    name: "DELL".into(),
                    scale: 0.0,
                },
                vec!["monitor 2", "DELL", "0", "positive finite"],
            ),
            (
                SessionError::MonitorSize {
                    index: 1,
                    name: "Built-in".into(),
                    w: 0,
                    h: 1080,
                },
                vec!["monitor 1", "Built-in", "0x1080"],
            ),
            (
                SessionError::TargetSize { w: 640, h: 0 },
                vec!["target window", "640x0"],
            ),
            (
                SessionError::Coordinate {
                    what: "selection 3".into(),
                    value: 2_000_000_000,
                },
                vec!["selection 3", "2000000000", "1000000"],
            ),
        ];
        for (error, expected) in cases {
            let rendered = error.to_string();
            for needle in expected {
                assert!(
                    rendered.contains(needle),
                    "{rendered:?} does not mention {needle:?}"
                );
            }
        }
    }

    #[test]
    fn a_valid_session_passes_validation() {
        let file = SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        assert_eq!(file.validate(), Ok(()));
    }

    #[test]
    fn a_scale_that_cannot_divide_is_refused() {
        // The reported defect: `scale: 0` divided into an inf, which the
        // float-to-int cast saturated into i32::MAX and reported as a
        // successful click point.
        for bad in [0.0, -2.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let mut file =
                SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
            file.monitors[0].scale = bad;
            let Err(SessionError::Scale { scale, index, .. }) = file.validate() else {
                panic!("scale {bad} was accepted");
            };
            assert_eq!(index, 0);
            // Bit-compare: the error must carry back the exact value it
            // rejected, and NaN is not equal to itself.
            assert_eq!(scale.to_bits(), bad.to_bits());
        }
    }

    #[test]
    fn a_positive_scale_below_one_is_fine() {
        // Fractional scaling is unusual, not invalid — the check is
        // "can this divide", not "is this a round number".
        let mut file =
            SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        file.monitors[0].scale = 0.75;
        assert_eq!(file.validate(), Ok(()));
    }

    #[test]
    fn an_empty_display_is_refused() {
        for (w, h) in [(0, 1080), (1920, 0), (-1920, 1080)] {
            let mut file =
                SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
            file.monitors[0].size_px = Size::new(w, h);
            assert!(
                matches!(file.validate(), Err(SessionError::MonitorSize { .. })),
                "{w}x{h} was accepted"
            );
        }
    }

    #[test]
    fn a_coordinate_past_the_bound_is_refused_wherever_it_hides() {
        let far = MAX_COORD + 1;
        let base =
            || SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);

        let mut in_monitor = base();
        in_monitor.monitors[0].origin_px = Point::new(far, 0);
        assert!(matches!(
            in_monitor.validate(),
            Err(SessionError::Coordinate { .. })
        ));

        let mut in_selection = base();
        in_selection.selections.push(SelectionRecord {
            shape: ToolKind::Rect,
            label: String::new(),
            monitor: 0,
            px: Shape::Rect(Rect::new(far, 0, 10, 10)),
            global_px: Shape::Rect(Rect::new(0, 0, 10, 10)),
            rot_deg: None,
            window_px: None,
            crop: "c.png".into(),
            color: None,
        });
        assert!(matches!(
            in_selection.validate(),
            Err(SessionError::Coordinate { .. })
        ));

        let mut in_measure = base().with_measures(&[crate::selection::Measure::new(
            crate::geometry::Line::new(Point::new(far, 0), Point::new(0, 0)),
            0,
        )]);
        in_measure.monitors = vec![monitor(0, 0, 0)];
        assert!(matches!(
            in_measure.validate(),
            Err(SessionError::Coordinate { .. })
        ));
    }

    #[test]
    fn the_bound_itself_is_allowed() {
        // A boundary that rejects its own limit would be a silent
        // off-by-one nobody would think to test for.
        let mut file =
            SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        file.monitors[0].origin_px = Point::new(MAX_COORD, -MAX_COORD);
        assert_eq!(file.validate(), Ok(()));
    }

    #[test]
    fn every_shape_kind_is_walked_for_coordinates() {
        // `raw_values` matches on the variant, so a new shape kind that
        // forgets to list its fields would silently stop being checked.
        let far = MAX_COORD + 1;
        let shapes = [
            Shape::Rect(Rect::new(far, 0, 1, 1)),
            Shape::Circle {
                cx: far,
                cy: 0,
                r: 1,
            },
            Shape::Ellipse {
                cx: far,
                cy: 0,
                rx: 1,
                ry: 1,
            },
            Shape::Triangle {
                ax: far,
                ay: 0,
                bx: 1,
                by: 1,
                cx: 2,
                cy: 2,
            },
            Shape::Poly {
                points: vec![Point::new(far, 0), Point::new(1, 1), Point::new(2, 2)],
            },
        ];
        for shape in shapes {
            assert!(
                raw_values(&shape).iter().any(|v| v.abs() > MAX_COORD),
                "{shape:?} hid its out-of-range coordinate"
            );
        }
    }

    #[test]
    fn restoring_a_session_without_measures_yields_none() {
        let file = SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None);
        assert!(restore_measures(&file).is_empty());
    }

    #[test]
    fn a_session_with_measures_round_trips() {
        use crate::geometry::Line;
        use crate::selection::Measure;
        let file = SessionFile::build("test", "t".into(), vec![monitor(0, 0, 0)], &[], &[], None)
            .with_measures(&[Measure::new(
                Line::new(Point::new(1, 2), Point::new(3, 4)),
                0,
            )]);
        let text = serde_json::to_string(&file).unwrap();
        let back: SessionFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back, file);
    }

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

    /// A session of labeled rects on monitor 0, in the given order.
    fn labeled(labels: &[&str]) -> SessionFile {
        let selections: Vec<Selection> = labels
            .iter()
            .map(|label| {
                let mut sel = Selection::new(Shape::Rect(Rect::new(0, 0, 10, 10)), 0);
                sel.label = (*label).to_string();
                sel
            })
            .collect();
        let crops: Vec<String> = (0..labels.len()).map(|i| format!("crop-{i}.png")).collect();
        SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![monitor(0, 0, 0)],
            &selections,
            &crops,
            None,
        )
    }

    #[test]
    fn select_by_label_keeps_session_indices() {
        let file = labeled(&["submit", "cancel", "submit"]);

        let all = select_by_label(&file, None);
        assert_eq!(all.len(), 3, "no label selects everything");
        assert_eq!(all.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);

        // The index is the record's identity in the file, not its position
        // in the filtered result — every report row is keyed by it.
        let some = select_by_label(&file, Some("submit"));
        assert_eq!(some.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 2]);
    }

    #[test]
    fn select_by_label_matches_case_insensitively_and_can_come_up_empty() {
        let file = labeled(&["Submit"]);
        assert_eq!(select_by_label(&file, Some("SUBMIT")).len(), 1);
        assert!(
            select_by_label(&file, Some("nope")).is_empty(),
            "an unmatched label is an empty result, not an error — the \
             caller decides how to refuse"
        );
    }

    #[test]
    fn distinct_labels_dedupes_case_insensitively_and_drops_blanks() {
        let file = labeled(&["submit", "", "SUBMIT", "cancel"]);
        assert_eq!(
            distinct_labels(file.selections.iter()),
            ["submit", "cancel"],
            "first spelling wins, session order is kept, unlabeled \
             selections contribute nothing"
        );
    }

    #[test]
    fn distinct_labels_reports_only_what_it_is_given() {
        // The iterator is the point: a monitor-space question lists the
        // labels on *that* monitor, not every label in the session.
        let file = labeled(&["submit", "cancel"]);
        let first_only = distinct_labels(file.selections.iter().take(1));
        assert_eq!(first_only, ["submit"]);
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
