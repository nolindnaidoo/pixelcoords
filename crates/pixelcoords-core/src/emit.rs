//! Ready-to-paste click snippets from a session — the logic behind
//! `pixelcoords emit`.
//!
//! Each emitter encodes one automation tool's coordinate convention in
//! exactly one place, because that conversion is where hand-written glue
//! gets silently burned: pyautogui speaks logical points on macOS but
//! physical pixels on Windows and X11; cliclick speaks logical points;
//! xdotool speaks physical pixels. Coordinates are the session's own
//! `global_px`, divided by the selection's monitor scale only where the
//! target tool wants logical points. Sessions are machine-local, so a
//! snippet is meant to run on the machine and monitor layout that was
//! captured.

use std::fmt::Write as _;

use thiserror::Error;

use crate::geometry::Point;
use crate::session::{SelectionRecord, SessionFile};
use crate::space::{Resolved, logical_of};

/// The OS the snippet will run on. Only pyautogui branches on it — the
/// other tools each exist on a single platform.
///
/// Re-exported from [`crate::space`], where it now lives: "which OS is
/// this coordinate for" is the same question `--units auto` asks, and one
/// answer serves both.
pub use crate::space::Platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitFormat {
    Pyautogui,
    Cliclick,
    Xdotool,
    /// Windows with nothing installed: `SetCursorPos` + `mouse_event`
    /// through a P/Invoke preamble.
    Powershell,
    /// macOS with nothing installed: System Events.
    Applescript,
    /// The Wayland answer, where xdotool cannot reach.
    Ydotool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmitError {
    #[error("the session has no selections to emit")]
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
}

/// One click the snippet will perform.
struct Target {
    comment: String,
    point: Point,
}

/// Render the session's selections as a ready-to-paste snippet for
/// `format`, ending with a newline.
pub fn emit(
    session: &SessionFile,
    format: EmitFormat,
    platform: Platform,
    label: Option<&str>,
) -> Result<String, EmitError> {
    match format {
        EmitFormat::Pyautogui => pyautogui(session, platform, label),
        EmitFormat::Cliclick => cliclick(session, label),
        EmitFormat::Xdotool => xdotool(session, label),
        EmitFormat::Powershell => powershell(session, label),
        EmitFormat::Applescript => applescript(session, label),
        EmitFormat::Ydotool => ydotool(session, label),
    }
}

fn pyautogui(
    session: &SessionFile,
    platform: Platform,
    label: Option<&str>,
) -> Result<String, EmitError> {
    let (units, space_note) = match platform {
        Platform::MacOs => (Resolved::Logical, "logical points (macOS)"),
        // pyautogui makes its process DPI-aware on import, so it addresses
        // true physical pixels on Windows.
        Platform::Windows => (Resolved::Physical, "physical pixels (Windows)"),
        Platform::Linux => (Resolved::Physical, "physical pixels (X11)"),
    };
    let targets = click_targets(session, units, label)?;
    let mut out = header("#", session, space_note);
    out.push_str("import pyautogui\n");
    for t in targets {
        // Writing to a String cannot fail.
        let _ = write!(
            out,
            "\n# {}\npyautogui.click({}, {})\n",
            t.comment, t.point.x, t.point.y
        );
    }
    Ok(out)
}

fn cliclick(session: &SessionFile, label: Option<&str>) -> Result<String, EmitError> {
    let targets = click_targets(session, Resolved::Logical, label)?;
    let mut out = header("#", session, "logical points (macOS)");
    for t in targets {
        let _ = writeln!(
            out,
            "cliclick c:{},{}  # {}",
            cliclick_coord(t.point.x),
            cliclick_coord(t.point.y),
            t.comment
        );
    }
    Ok(out)
}

/// cliclick parses a bare leading `-` as an option; its documented escape
/// for negative coordinates is an `=` prefix.
fn cliclick_coord(v: i32) -> String {
    if v < 0 {
        return format!("={v}");
    }
    v.to_string()
}

fn xdotool(session: &SessionFile, label: Option<&str>) -> Result<String, EmitError> {
    let targets = click_targets(session, Resolved::Physical, label)?;
    let mut out = header("#", session, "physical pixels (X11)");
    for t in targets {
        let _ = writeln!(
            out,
            "xdotool mousemove {} {} click 1  # {}",
            t.point.x, t.point.y, t.comment
        );
    }
    Ok(out)
}

/// Windows without Python. The Win32 cursor APIs speak physical pixels on
/// a per-monitor-DPI-aware process, which is what the session records on
/// Windows, so no conversion happens here.
///
/// The P/Invoke preamble is emitted once rather than per click: pasting
/// `Add-Type` for the same type twice in one session is an error, not a
/// no-op, so a per-click preamble would break on the second selection.
fn powershell(session: &SessionFile, label: Option<&str>) -> Result<String, EmitError> {
    let targets = click_targets(session, Resolved::Physical, label)?;
    let mut out = header("#", session, "physical pixels (Windows)");
    out.push_str(
        "\nAdd-Type @\"\n\
         using System;\n\
         using System.Runtime.InteropServices;\n\
         public class PixelCoords {\n\
         \x20 [DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int x, int y);\n\
         \x20 [DllImport(\"user32.dll\")] public static extern void mouse_event(uint f, uint x, uint y, uint d, int i);\n\
         }\n\
         \"@\n",
    );
    for t in targets {
        // 0x0002 is MOUSEEVENTF_LEFTDOWN, 0x0004 MOUSEEVENTF_LEFTUP.
        let _ = write!(
            out,
            "\n# {}\n\
             [PixelCoords]::SetCursorPos({}, {})\n\
             [PixelCoords]::mouse_event(0x0002, 0, 0, 0, 0)\n\
             [PixelCoords]::mouse_event(0x0004, 0, 0, 0, 0)\n",
            t.comment, t.point.x, t.point.y
        );
    }
    Ok(out)
}

/// macOS without Homebrew. System Events speaks logical points, like
/// cliclick.
///
/// One `tell` block wraps every click rather than one per selection —
/// the snippet is meant to be pasted whole, and re-entering the same
/// application context per click is noise a reader has to skip.
fn applescript(session: &SessionFile, label: Option<&str>) -> Result<String, EmitError> {
    let targets = click_targets(session, Resolved::Logical, label)?;
    let mut out = header("--", session, "logical points (macOS)");
    out.push_str(
        "-- System Events clicking needs Accessibility permission:\n\
         -- System Settings > Privacy & Security > Accessibility\n\
         \ntell application \"System Events\"\n",
    );
    for t in targets {
        let _ = write!(
            out,
            "\t-- {}\n\tclick at {{{}, {}}}\n",
            t.comment, t.point.x, t.point.y
        );
    }
    out.push_str("end tell\n");
    Ok(out)
}

/// The Wayland answer, completing the `--pick` story: xdotool speaks X11
/// only, and a Wayland compositor will not answer it.
///
/// Physical pixels, like xdotool — ydotool writes to an uinput device
/// below the compositor, so it addresses the raw device grid.
fn ydotool(session: &SessionFile, label: Option<&str>) -> Result<String, EmitError> {
    let targets = click_targets(session, Resolved::Physical, label)?;
    let mut out = header("#", session, "physical pixels (Wayland)");
    out.push_str(
        "# needs the ydotoold daemon running and permission on its socket —\n\
         # this is setup on your side, not something the snippet can do\n",
    );
    for t in targets {
        // 0xC0 is ydotool's left-button press-and-release.
        let _ = writeln!(
            out,
            "ydotool mousemove --absolute -x {} -y {} && ydotool click 0xC0  # {}",
            t.point.x, t.point.y, t.comment
        );
    }
    Ok(out)
}

fn header(prefix: &str, session: &SessionFile, space_note: &str) -> String {
    format!(
        "{prefix} generated by pixelcoords from a session captured {}\n\
         {prefix} coordinates: {space_note} — run on the machine and \
         monitor layout that was captured\n",
        session.created_utc
    )
}

/// Every selection's click point in global coordinates, converted to the
/// requested units via its own monitor's scale — mixed-DPI setups scale
/// each selection independently.
fn click_targets(
    session: &SessionFile,
    units: Resolved,
    label: Option<&str>,
) -> Result<Vec<Target>, EmitError> {
    if session.selections.is_empty() {
        return Err(EmitError::NoSelections);
    }
    let wanted = crate::session::select_by_label(session, label);
    if wanted.is_empty() {
        // Only a label filter can empty a non-empty session.
        return Err(EmitError::UnknownLabel {
            requested: label.unwrap_or_default().to_string(),
            available: crate::session::distinct_labels(session.selections.iter()),
        });
    }
    wanted
        .into_iter()
        .map(|(index, record)| {
            // The click point of the stored global shape. Rect rotation
            // pivots on the bbox center — the click point itself — so
            // `rot_deg` cannot move it; triangles store rotation baked.
            let physical = record.global_px.click_point();
            let point = match units {
                Resolved::Physical => physical,
                Resolved::Logical => to_logical(session, index, record, physical)?,
            };
            Ok(Target {
                comment: describe(index, record),
                point,
            })
        })
        .collect()
}

/// `global_px` through the selection's own monitor scale. The lookup and
/// its error stay here; the arithmetic is `space::logical_of`, shared
/// with every other command that has to answer the same question.
fn to_logical(
    session: &SessionFile,
    index: usize,
    record: &SelectionRecord,
    physical: Point,
) -> Result<Point, EmitError> {
    let monitor = session
        .monitors
        .iter()
        .find(|m| m.index == record.monitor)
        .ok_or(EmitError::UnknownMonitor {
            selection: index,
            monitor: record.monitor,
        })?;
    Ok(logical_of(physical, monitor.scale))
}

fn describe(index: usize, record: &SelectionRecord) -> String {
    let shape = match record.shape {
        crate::geometry::ToolKind::Rect => "rect",
        crate::geometry::ToolKind::Circle => "circle",
        crate::geometry::ToolKind::Ellipse => "ellipse",
        crate::geometry::ToolKind::Polygon
        | crate::geometry::ToolKind::Freehand
        | crate::geometry::ToolKind::Poly => "poly",
        crate::geometry::ToolKind::Triangle => "triangle",
        // A measure is never stored as a selection — it lives in the
        // session's `measures` array — so this arm exists to keep the
        // match total, not because it can be reached.
        crate::geometry::ToolKind::Measure => "measure",
    };
    if record.label.is_empty() {
        return format!("selection {index} — {shape} on monitor {}", record.monitor);
    }
    format!("{} — {shape} on monitor {}", record.label, record.monitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Rect, Shape, Size};
    use crate::selection::Selection;
    use crate::session::MonitorRecord;

    fn monitor(index: usize, ox: i32, oy: i32, scale: f64) -> MonitorRecord {
        MonitorRecord {
            index,
            name: format!("Display {index}"),
            primary: index == 0,
            origin_px: Point::new(ox, oy),
            size_px: Size::new(1920, 1080),
            scale,
        }
    }

    fn labeled(shape: Shape, monitor: usize, label: &str) -> Selection {
        let mut sel = Selection::new(shape, monitor);
        sel.label = label.into();
        sel
    }

    fn session(monitors: Vec<MonitorRecord>, selections: &[Selection]) -> SessionFile {
        let crops: Vec<String> = (0..selections.len()).map(|i| format!("c{i}.png")).collect();
        SessionFile::build(
            "test",
            "2026-07-27T11:35:42Z".into(),
            monitors,
            selections,
            &crops,
            None,
        )
    }

    /// One selection per monitor on a mixed-DPI desktop: monitor 0 at
    /// scale 1 and monitor 1 at scale 2, offset to global x=1920. The
    /// click points are physical (850, 440) and (2040, 230); in logical
    /// units the second halves to (1020, 115) and the first does not
    /// move. Every format below is checked against the same two.
    fn mixed_dpi() -> SessionFile {
        session(
            vec![monitor(0, 0, 0, 1.0), monitor(1, 1920, 0, 2.0)],
            &[
                labeled(Shape::Rect(Rect::new(800, 400, 100, 80)), 0, "left"),
                labeled(Shape::Rect(Rect::new(100, 200, 40, 60)), 1, "right"),
            ],
        )
    }

    #[test]
    fn powershell_emits_physical_pixels_and_one_preamble() {
        let out = emit(
            &mixed_dpi(),
            EmitFormat::Powershell,
            Platform::Windows,
            None,
        )
        .unwrap();
        assert_eq!(
            out,
            "# generated by pixelcoords from a session captured 2026-07-27T11:35:42Z\n\
             # coordinates: physical pixels (Windows) — run on the machine and \
             monitor layout that was captured\n\
             \n\
             Add-Type @\"\n\
             using System;\n\
             using System.Runtime.InteropServices;\n\
             public class PixelCoords {\n\
             \x20 [DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int x, int y);\n\
             \x20 [DllImport(\"user32.dll\")] public static extern void mouse_event(uint f, uint x, uint y, uint d, int i);\n\
             }\n\
             \"@\n\
             \n\
             # left — rect on monitor 0\n\
             [PixelCoords]::SetCursorPos(850, 440)\n\
             [PixelCoords]::mouse_event(0x0002, 0, 0, 0, 0)\n\
             [PixelCoords]::mouse_event(0x0004, 0, 0, 0, 0)\n\
             \n\
             # right — rect on monitor 1\n\
             [PixelCoords]::SetCursorPos(2040, 230)\n\
             [PixelCoords]::mouse_event(0x0002, 0, 0, 0, 0)\n\
             [PixelCoords]::mouse_event(0x0004, 0, 0, 0, 0)\n"
        );
        assert_eq!(
            out.matches("Add-Type").count(),
            1,
            "pasting Add-Type twice for one type is an error, not a no-op"
        );
    }

    #[test]
    fn applescript_emits_logical_points_per_monitor_scale() {
        let out = emit(&mixed_dpi(), EmitFormat::Applescript, Platform::MacOs, None).unwrap();
        assert_eq!(
            out,
            "-- generated by pixelcoords from a session captured 2026-07-27T11:35:42Z\n\
             -- coordinates: logical points (macOS) — run on the machine and \
             monitor layout that was captured\n\
             -- System Events clicking needs Accessibility permission:\n\
             -- System Settings > Privacy & Security > Accessibility\n\
             \n\
             tell application \"System Events\"\n\
             \t-- left — rect on monitor 0\n\
             \tclick at {850, 440}\n\
             \t-- right — rect on monitor 1\n\
             \tclick at {1020, 115}\n\
             end tell\n"
        );
    }

    #[test]
    fn ydotool_emits_physical_pixels_with_the_daemon_caveat() {
        let out = emit(&mixed_dpi(), EmitFormat::Ydotool, Platform::Linux, None).unwrap();
        assert_eq!(
            out,
            "# generated by pixelcoords from a session captured 2026-07-27T11:35:42Z\n\
             # coordinates: physical pixels (Wayland) — run on the machine and \
             monitor layout that was captured\n\
             # needs the ydotoold daemon running and permission on its socket —\n\
             # this is setup on your side, not something the snippet can do\n\
             ydotool mousemove --absolute -x 850 -y 440 && ydotool click 0xC0  \
             # left — rect on monitor 0\n\
             ydotool mousemove --absolute -x 2040 -y 230 && ydotool click 0xC0  \
             # right — rect on monitor 1\n"
        );
    }

    #[test]
    fn each_format_applies_its_own_convention_to_the_same_session() {
        // The point of the table: one session, two monitors at different
        // scales, and each target gets the units *it* speaks — with the
        // second selection converted through monitor 1's scale, never a
        // desktop-wide one.
        let file = mixed_dpi();
        let physical = [
            EmitFormat::Xdotool,
            EmitFormat::Powershell,
            EmitFormat::Ydotool,
        ];
        for format in physical {
            let out = emit(&file, format, Platform::Linux, None).unwrap();
            assert!(out.contains("2040"), "{format:?} should be physical");
            assert!(!out.contains("1020"), "{format:?} must not halve");
        }
        for format in [EmitFormat::Cliclick, EmitFormat::Applescript] {
            let out = emit(&file, format, Platform::MacOs, None).unwrap();
            assert!(out.contains("1020"), "{format:?} should be logical");
            assert!(
                out.contains("850"),
                "{format:?}: the scale-1 monitor must not move"
            );
        }
    }

    #[test]
    fn a_label_filter_reaches_the_new_formats_too() {
        let file = mixed_dpi();
        for format in [
            EmitFormat::Powershell,
            EmitFormat::Applescript,
            EmitFormat::Ydotool,
        ] {
            let out = emit(&file, format, Platform::MacOs, Some("right")).unwrap();
            assert!(out.contains("right"), "{format:?}");
            assert!(!out.contains("# left"), "{format:?} emitted the wrong one");

            let err = emit(&file, format, Platform::MacOs, Some("nope")).unwrap_err();
            assert!(matches!(err, EmitError::UnknownLabel { .. }), "{format:?}");
        }
    }

    #[test]
    fn pyautogui_on_macos_emits_logical_points() {
        // Rect center at physical (100, 60) on a 2x monitor -> (50, 30).
        let file = session(
            vec![monitor(0, 0, 0, 2.0)],
            &[labeled(Shape::Rect(Rect::new(80, 40, 40, 40)), 0, "submit")],
        );
        let out = emit(&file, EmitFormat::Pyautogui, Platform::MacOs, None).unwrap();
        assert_eq!(
            out,
            "# generated by pixelcoords from a session captured 2026-07-27T11:35:42Z\n\
             # coordinates: logical points (macOS) — run on the machine and \
             monitor layout that was captured\n\
             import pyautogui\n\
             \n\
             # submit — rect on monitor 0\n\
             pyautogui.click(50, 30)\n"
        );
    }

    #[test]
    fn pyautogui_elsewhere_emits_physical_pixels() {
        let file = session(
            vec![monitor(0, 0, 0, 2.0)],
            &[labeled(Shape::Rect(Rect::new(80, 40, 40, 40)), 0, "submit")],
        );
        for (platform, note) in [
            (Platform::Windows, "physical pixels (Windows)"),
            (Platform::Linux, "physical pixels (X11)"),
        ] {
            let out = emit(&file, EmitFormat::Pyautogui, platform, None).unwrap();
            assert!(out.contains("pyautogui.click(100, 60)"), "got: {out}");
            assert!(out.contains(note), "got: {out}");
        }
    }

    #[test]
    fn cliclick_escapes_negative_logical_coordinates() {
        // A monitor left of the primary: global physical (-1800, 40) at
        // scale 2 -> logical (-900, 20), with cliclick's `=` escape.
        let file = session(
            vec![monitor(0, -3840, 0, 2.0)],
            &[labeled(Shape::Rect(Rect::new(2020, 20, 40, 40)), 0, "back")],
        );
        let out = emit(&file, EmitFormat::Cliclick, Platform::MacOs, None).unwrap();
        assert!(
            out.contains("cliclick c:=-900,20  # back — rect on monitor 0"),
            "got: {out}"
        );
    }

    #[test]
    fn xdotool_emits_physical_pixels_untouched() {
        let file = session(
            vec![monitor(0, 0, 0, 2.0)],
            &[labeled(
                Shape::Circle {
                    cx: 500,
                    cy: 300,
                    r: 25,
                },
                0,
                "dot",
            )],
        );
        let out = emit(&file, EmitFormat::Xdotool, Platform::Linux, None).unwrap();
        assert!(
            out.contains("xdotool mousemove 500 300 click 1  # dot — circle on monitor 0"),
            "got: {out}"
        );
    }

    #[test]
    fn mixed_dpi_scales_each_selection_by_its_own_monitor() {
        let file = session(
            vec![monitor(0, 0, 0, 1.0), monitor(1, 1920, 0, 2.0)],
            &[
                labeled(Shape::Rect(Rect::new(100, 100, 20, 20)), 0, "left"),
                labeled(Shape::Rect(Rect::new(100, 100, 20, 20)), 1, "right"),
            ],
        );
        let out = emit(&file, EmitFormat::Pyautogui, Platform::MacOs, None).unwrap();
        // Monitor 0 at scale 1: physical (110, 110) stays put. Monitor 1 at
        // scale 2: global physical (2030, 110) -> logical (1015, 55).
        assert!(out.contains("pyautogui.click(110, 110)"), "got: {out}");
        assert!(out.contains("pyautogui.click(1015, 55)"), "got: {out}");
    }

    #[test]
    fn triangles_click_their_centroid_and_unlabeled_selections_get_names() {
        let file = session(
            vec![monitor(0, 0, 0, 1.0)],
            &[labeled(
                Shape::Triangle {
                    ax: 30,
                    ay: 0,
                    bx: 0,
                    by: 60,
                    cx: 60,
                    cy: 60,
                },
                0,
                "",
            )],
        );
        let out = emit(&file, EmitFormat::Xdotool, Platform::Linux, None).unwrap();
        assert!(
            out.contains("xdotool mousemove 30 40 click 1  # selection 0 — triangle on monitor 0"),
            "got: {out}"
        );
    }

    #[test]
    fn a_rotated_rect_clicks_its_pivot() {
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 10, 40, 10)), 0);
        sel.rot_deg = 90;
        let file = session(vec![monitor(0, 0, 0, 1.0)], &[sel]);
        let out = emit(&file, EmitFormat::Xdotool, Platform::Linux, None).unwrap();
        // The pivot (30, 15) is rotation-invariant, so the click lands
        // inside the silhouette at any angle.
        assert!(
            out.contains("xdotool mousemove 30 15 click 1"),
            "got: {out}"
        );
    }

    #[test]
    fn a_label_filter_emits_only_matching_selections() {
        let file = session(
            vec![monitor(0, 0, 0, 1.0)],
            &[
                labeled(Shape::Rect(Rect::new(0, 0, 10, 10)), 0, "Cancel"),
                labeled(Shape::Rect(Rect::new(100, 100, 10, 10)), 0, "Submit"),
            ],
        );
        let out = emit(&file, EmitFormat::Xdotool, Platform::Linux, Some("submit")).unwrap();
        assert!(out.contains("xdotool mousemove 105 105"), "got: {out}");
        assert!(!out.contains("mousemove 5 5"), "got: {out}");
        // The comment keeps the selection's original session index.
        assert!(out.contains("Submit — rect on monitor 0"), "got: {out}");

        let err = emit(&file, EmitFormat::Xdotool, Platform::Linux, Some("send")).unwrap_err();
        assert_eq!(
            err,
            EmitError::UnknownLabel {
                requested: "send".into(),
                available: vec!["Cancel".into(), "Submit".into()],
            }
        );
    }

    #[test]
    fn an_empty_session_is_an_error() {
        let file = session(vec![monitor(0, 0, 0, 1.0)], &[]);
        let err = emit(&file, EmitFormat::Xdotool, Platform::Linux, None).unwrap_err();
        assert_eq!(err, EmitError::NoSelections);
    }

    #[test]
    fn a_selection_on_an_undescribed_monitor_is_an_error_for_logical_units() {
        let file = session(
            vec![monitor(0, 0, 0, 2.0)],
            &[labeled(Shape::Rect(Rect::new(0, 0, 10, 10)), 3, "orphan")],
        );
        let err = emit(&file, EmitFormat::Cliclick, Platform::MacOs, None).unwrap_err();
        assert_eq!(
            err,
            EmitError::UnknownMonitor {
                selection: 0,
                monitor: 3,
            }
        );
        // Physical units never look the monitor up, so the same session
        // still emits for xdotool.
        assert!(emit(&file, EmitFormat::Xdotool, Platform::Linux, None).is_ok());
    }
}
