//! The winit application: one flat state struct, no generics. Input events
//! map to core actions through the tested hotkey grammar; all geometry
//! happens in capture space via the core crate. One overlay window per
//! captured monitor; selections are tagged with their monitor index.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use image::RgbaImage;
use pixelcoords_core::config::{SnapSettings, Style};
use pixelcoords_core::geometry::{Line, Point, Rect, ResizeHandle, Shape, Size, ToolKind};
use pixelcoords_core::hotkeys::{Action, Binding, Edge, KeyName, OverlayState, match_event};
use pixelcoords_core::locate::GrayImage;
use pixelcoords_core::selection::{GrabKind, Measure, MeasureGrab, Selection, SelectionSet};
use pixelcoords_core::session::{MAX_LABEL_LEN, TargetRecord};
use pixelcoords_core::snap::{EdgeMap, Snap};
use pixelcoords_core::strings::{EN, Strings};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, WindowId};

use crate::capture::MonitorInfo;
use crate::render::{self, FrameState};
use crate::view::{OverlayView, Presentation};

const CARET_BLINK: Duration = Duration::from_millis(500);
const FLASH_SAVE: Duration = Duration::from_millis(2500);
const FLASH_TOOL: Duration = Duration::from_millis(1200);
/// Border-grab tolerance in logical pixels (scaled per monitor).
const GRAB_TOLERANCE: i32 = 6;

/// One captured monitor: its metadata, the frozen frame, and the derived
/// presentation data.
pub struct MonitorFrame {
    pub info: MonitorInfo,
    pub rgba: RgbaImage,
    background: Vec<u32>,
    size: Size,
    /// The rectangle within this frame that a user may draw in. In desktop
    /// mode this is the whole frame; in `--target` mode it is the target
    /// window's rect on this monitor, and drawing outside it is refused.
    /// The overlay dims pixels outside this rect so the drawable area is
    /// obvious.
    draw_rect: pixelcoords_core::geometry::Rect,
    ui_scale: i32,
    /// Detected edges, built on first use. Lazy because a run with
    /// snapping off should not pay ~30ms and ~12MB per monitor for a map
    /// nobody queries; `App::new` warms it when the config says snapping
    /// starts on, so the cost lands during startup rather than as a hitch
    /// on the first mouse move.
    edges: OnceCell<EdgeMap>,
}

impl MonitorFrame {
    pub fn new(info: MonitorInfo, rgba: RgbaImage) -> Self {
        let size = Size::new(rgba.width() as i32, rgba.height() as i32);
        let background = rgba_to_0rgb(&rgba);
        let ui_scale = (info.scale.round() as i32).max(1);
        let draw_rect = pixelcoords_core::geometry::Rect::new(0, 0, size.w, size.h);
        Self {
            info,
            rgba,
            background,
            size,
            draw_rect,
            ui_scale,
            edges: OnceCell::new(),
        }
    }

    /// This frame's edge map, detecting on first call.
    pub fn edges(&self) -> &EdgeMap {
        self.edges.get_or_init(|| {
            EdgeMap::new(&GrayImage::from_rgba(
                self.size.w.max(0) as usize,
                self.size.h.max(0) as usize,
                self.rgba.as_raw(),
            ))
        })
    }

    /// Restrict drawing to `rect` within this frame. Clamped to the frame
    /// bounds so a bad target rect cannot escape the capture.
    pub fn with_draw_rect(mut self, rect: pixelcoords_core::geometry::Rect) -> Self {
        let x = rect.x.max(0).min(self.size.w);
        let y = rect.y.max(0).min(self.size.h);
        let w = (rect.x + rect.w).min(self.size.w).max(x) - x;
        let h = (rect.y + rect.h).min(self.size.h).max(y) - y;
        if w > 0 && h > 0 {
            self.draw_rect = pixelcoords_core::geometry::Rect::new(x, y, w, h);
        }
        self
    }

    pub fn draw_rect(&self) -> pixelcoords_core::geometry::Rect {
        self.draw_rect
    }
}

struct ViewSlot {
    frame: usize,
    view: OverlayView,
}

/// What a close request resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// The display is unfrozen and its window should go.
    Done,
    /// It still holds marks; nothing changed and the user was told.
    Blocked,
    /// The last window, or a gesture in flight — end the run instead.
    Quit,
}

/// Which array a label-editor index addresses. Selections and measures
/// are separate collections, so the index alone is ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    Selection,
    Measure,
}

enum Mode {
    Idle,
    /// `frame` locks the draw to the monitor it started on; `path` is the
    /// freehand tool's accumulated stroke, empty for every other tool.
    Drawing {
        frame: usize,
        start: Point,
        path: Vec<Point>,
    },
    Dragging {
        frame: usize,
        index: usize,
        grab_offset: Point,
        original: Shape,
    },
    Resizing {
        frame: usize,
        index: usize,
        handle: ResizeHandle,
        original: Shape,
    },
    /// Dragging an existing measure: one endpoint, or the whole ruler.
    MeasureDragging {
        frame: usize,
        index: usize,
        grab: MeasureGrab,
        original: Line,
        /// Cursor-to-`a` offset, so a whole-line move keeps its grip
        /// instead of snapping the endpoint to the pointer.
        grab_offset: Point,
    },
    LabelEditing {
        target: EditTarget,
        index: usize,
        text: String,
    },
    /// Typing a friendly session name (N); same editor grammar as labels.
    SessionNaming {
        text: String,
    },
}

pub struct App {
    frames: Vec<MonitorFrame>,
    style: Style,
    bindings: Vec<Binding>,
    strings: &'static Strings,
    out_dir: PathBuf,
    /// The `--target` window at freeze time, if one was matched.
    target: Option<TargetRecord>,
    /// Provenance written into every save (platform, capture kind).
    session_meta: crate::save::SessionMeta,
    /// Whether each frame owns a whole monitor or is a picked window.
    presentation: Presentation,

    views: Vec<ViewSlot>,
    selections: SelectionSet,
    tool: ToolKind,
    cursor: Point,
    /// Index into `frames` of the monitor the cursor was last seen on.
    cursor_frame: usize,
    mode: Mode,
    cursor_icon: CursorIcon,
    shift_down: bool,
    /// Alt is held: pressing on a shape drags a duplicate instead.
    alt_down: bool,
    /// Space is held: the control panel follows the cursor until release.
    panel_held: bool,
    /// The control panel is hidden (H toggles).
    panel_hidden: bool,
    /// M is held: the loupe magnifies around the cursor.
    loupe_held: bool,
    /// The polygon tool's side count, set with the number keys.
    polygon_sides: u32,
    /// Where the user parked the control panel: (frame index, top-left in
    /// that frame's capture space). `None` is the default corner.
    panel_origin: Option<(usize, Point)>,
    /// Selections changed since the last successful save.
    dirty: bool,
    /// At least one save succeeded (later saves skip re-encoding the
    /// frozen screenshots).
    saved_once: bool,
    /// Crops the previous save wrote — the only files a later save may
    /// retire, and what lets it skip re-encoding unchanged ones.
    last_crops: Vec<crate::save::WrittenCrop>,
    /// Q with unsaved work arms a second-press window instead of quitting.
    quit_armed_until: Option<Instant>,
    flash: Option<(String, Instant)>,
    caret_visible: bool,
    caret_deadline: Option<Instant>,
    error: Option<anyhow::Error>,
    /// A repaint runs on every mouse move, so a failing one would log once
    /// per event. Latched like the view's size-mismatch warning: report the
    /// first, stay quiet until a frame succeeds again.
    warned_render_failure: bool,
    /// Resolved config; `enabled` is the live state, flipped by the
    /// toggle key and deliberately not persisted.
    snap: SnapSettings,
    /// What the in-flight gesture snapped to, kept so the overlay can
    /// draw the edge that captured the point. Cleared on release.
    snap_hit: Snap,
    /// `cursor` with `snap_hit` applied — the point every gesture builds
    /// from. Held as state rather than recomputed at each use so one
    /// gesture step cannot snap twice and disagree with itself, and so
    /// the guides drawn always match the geometry committed.
    gesture_cursor: Point,
}

impl App {
    pub fn new(
        frames: Vec<MonitorFrame>,
        style: Style,
        bindings: Vec<Binding>,
        out_dir: PathBuf,
        target: Option<TargetRecord>,
        presentation: Presentation,
    ) -> Self {
        assert!(!frames.is_empty(), "at least one captured monitor");
        Self {
            frames,
            style,
            bindings,
            strings: &EN,
            out_dir,
            target,
            session_meta: crate::save::SessionMeta::default(),
            presentation,
            views: Vec::new(),
            selections: SelectionSet::new(),
            tool: ToolKind::Rect,
            cursor: Point::new(0, 0),
            cursor_frame: 0,
            mode: Mode::Idle,
            cursor_icon: CursorIcon::Crosshair,
            shift_down: false,
            alt_down: false,
            panel_held: false,
            panel_hidden: false,
            loupe_held: false,
            polygon_sides: 6,
            panel_origin: None,
            dirty: false,
            saved_once: false,
            last_crops: Vec::new(),
            quit_armed_until: None,
            flash: None,
            caret_visible: true,
            caret_deadline: None,
            error: None,
            warned_render_failure: false,
            snap: SnapSettings::default(),
            snap_hit: Snap::default(),
            gesture_cursor: Point::new(0, 0),
        }
    }

    /// Adopt resolved snapping settings, warming each frame's edge map
    /// when snapping starts on so detection does not stall the first
    /// mouse move.
    pub fn set_snap(&mut self, settings: SnapSettings) {
        self.snap = settings;
        if settings.enabled {
            for frame in &self.frames {
                let _ = frame.edges();
            }
        }
    }

    /// The snap radius on the frame under the cursor. Config gives it in
    /// logical pixels; a 2x display needs twice as many physical ones for
    /// the same reach under the hand.
    fn snap_radius(&self) -> i32 {
        if self.snap.enabled {
            self.snap.radius * self.frames[self.cursor_frame].ui_scale
        } else {
            0
        }
    }

    /// Where the cursor wants to be, recording what captured it.
    ///
    /// Called at the start of each gesture step rather than folded into
    /// `cursor_moved`, because the *idle* cursor must stay exact: the
    /// readout, the loupe, and hit-testing all report where the pointer
    /// actually is, and a hovering cursor that jumps to nearby edges
    /// would make the whole overlay feel broken.
    fn snap_cursor(&mut self) -> Point {
        let radius = self.snap_radius();
        if radius <= 0 {
            self.snap_hit = Snap::default();
            return self.cursor;
        }
        self.snap_hit = self.frames[self.cursor_frame]
            .edges()
            .snap(self.cursor, radius);
        self.snap_hit.apply(self.cursor)
    }

    /// Set the provenance stamped into every save.
    pub fn set_session_meta(&mut self, meta: crate::save::SessionMeta) {
        self.session_meta = meta;
    }

    /// Adopt a saved session's state for resumed editing: its selections
    /// (seeded without undo history — the resume point is the floor), its
    /// measures, and
    /// when saving back in place, the crops it wrote plus resave
    /// semantics, so the next save skips, retires, and re-encodes exactly
    /// as if the session never closed. A diverted `--out` starts fresh:
    /// screenshots must be written to the new directory.
    pub fn restore_session(
        &mut self,
        selections: Vec<Selection>,
        measures: Vec<Measure>,
        previous: Vec<crate::save::WrittenCrop>,
        in_place: bool,
    ) {
        self.selections = SelectionSet::seed(selections, measures);
        if in_place {
            self.last_crops = previous;
            self.saved_once = true;
        }
    }

    /// Restore the panel position a previous run parked, ignoring a
    /// frame index this capture does not have.
    pub fn restore_panel(&mut self, saved: Option<(usize, Point)>) {
        self.panel_origin = saved.filter(|&(frame, _)| frame < self.frames.len());
    }

    pub fn run(mut self) -> Result<()> {
        let event_loop = EventLoop::new()?;
        event_loop.run_app(&mut self)?;
        // Where the panel ended up survives into the next run.
        crate::state::save_panel(self.panel_origin);
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn frame_of_window(&self, id: WindowId) -> Option<usize> {
        self.views
            .iter()
            .find(|slot| slot.view.id() == id)
            .map(|slot| slot.frame)
    }

    fn redraw_all(&self) {
        for slot in &self.views {
            slot.view.request_redraw();
        }
    }

    /// Redraw only the view showing `frame` — mid-gesture updates touch one
    /// monitor, and recomposing a Retina buffer per view per cursor delta
    /// is the hot path.
    fn redraw_frame(&self, frame: usize) {
        if let Some(slot) = self.views.iter().find(|s| s.frame == frame) {
            slot.view.request_redraw();
        }
    }

    /// Selections changed: dirty the session and disarm any pending
    /// quit confirmation so new work always re-warns.
    fn mark_dirty(&mut self) {
        self.dirty = true;
        self.quit_armed_until = None;
    }

    fn set_flash(&mut self, message: String, duration: Duration) {
        self.flash = Some((message, Instant::now() + duration));
        self.redraw_all();
    }

    fn overlay_state(&self) -> OverlayState {
        OverlayState {
            has_selection: !self.selections.is_empty(),
            cursor_in_shape: self.hit_at_cursor().is_some(),
        }
    }

    fn hit_at_cursor(&self) -> Option<usize> {
        let monitor = self.frames[self.cursor_frame].info.index;
        self.selections.hit_topmost(monitor, self.cursor)
    }

    /// The measure being drawn or dragged on `frame_idx`, if any.
    ///
    /// Separate from `preview_for` because a measure is a `Line`, not a
    /// `Shape` — the whole reason it lives in its own session array.
    fn measure_preview(&self, frame_idx: usize) -> Option<Line> {
        match self.mode {
            Mode::Drawing { frame, start, .. }
                if frame == frame_idx && self.tool == ToolKind::Measure =>
            {
                let line = Line::new(start, self.gesture_cursor);
                // Shift constrains to horizontal, vertical, or 45 —
                // the same grammar the other tools give Shift.
                Some(if self.shift_down {
                    line.constrained()
                } else {
                    line
                })
            }
            _ => None,
        }
    }

    fn preview_for(&self, frame_idx: usize) -> Option<Shape> {
        let Mode::Drawing { frame, start, path } = &self.mode else {
            return None;
        };
        if *frame != frame_idx {
            return None;
        }
        match self.tool {
            // Center-to-vertex drag: one gesture sizes and orients the
            // N-gon at once.
            ToolKind::Polygon => {
                // Clamp the vertex direction into the drawable region so
                // dragging past the window edge does not extend the
                // polygon outside it.
                let region = self.frames[frame_idx].draw_rect;
                let vertex = Point::new(
                    self.gesture_cursor
                        .x
                        .clamp(region.x, region.x + region.w - 1),
                    self.gesture_cursor
                        .y
                        .clamp(region.y, region.y + region.h - 1),
                );
                Some(pixelcoords_core::geometry::regular_polygon(
                    *start,
                    vertex,
                    self.polygon_sides,
                ))
            }
            // The stroke so far, implicitly closed.
            ToolKind::Freehand => (path.len() >= 3).then(|| Shape::Poly {
                points: path.clone(),
            }),
            _ => Shape::compute_preview(
                self.tool,
                *start,
                self.gesture_cursor,
                self.frames[frame_idx].draw_rect,
                self.shift_down,
            ),
        }
    }

    fn render(&mut self, window_id: WindowId) {
        let Some(frame_idx) = self.frame_of_window(window_id) else {
            return;
        };
        let preview = self.preview_for(frame_idx);
        let editing = match &self.mode {
            Mode::LabelEditing {
                target: EditTarget::Selection,
                index,
                text,
            } => Some((*index, text.as_str(), self.caret_visible)),
            _ => None,
        };
        let measure_editing = match &self.mode {
            Mode::LabelEditing {
                target: EditTarget::Measure,
                index,
                text,
            } => Some((*index, text.as_str(), self.caret_visible)),
            _ => None,
        };
        let measure_preview = self.measure_preview(frame_idx);
        let flash = self
            .flash
            .as_ref()
            .filter(|(_, until)| Instant::now() < *until)
            .map(|(m, _)| m.as_str());
        let frame = &self.frames[frame_idx];
        // Thickness is configured in logical pixels; scale it per monitor so
        // outlines have the same visual weight on any display.
        let mut style = self.style;
        style.thickness *= frame.ui_scale;
        let target = self
            .target
            .as_ref()
            .filter(|t| t.monitor == frame.info.index)
            .map(|t| {
                pixelcoords_core::geometry::Rect::new(
                    t.origin_px.x,
                    t.origin_px.y,
                    t.size_px.w,
                    t.size_px.h,
                )
            });
        let naming = match &self.mode {
            Mode::SessionNaming { text } => Some((text.as_str(), self.caret_visible)),
            _ => None,
        };
        let state = FrameState {
            selections: &self.selections,
            monitor: frame.info.index,
            target,
            preview,
            editing,
            measure_editing,
            measure_preview,
            snap_enabled: self.snap.enabled,
            snap: if frame_idx == self.cursor_frame {
                self.snap_hit
            } else {
                Snap::default()
            },
            flash,
            strings: self.strings,
            style,
            ui_scale: frame.ui_scale,
            panel_origin: self
                .panel_origin
                .and_then(|(f, p)| (f == frame_idx).then_some(p)),
            panel_hidden: self.panel_hidden,
            tool: self.tool,
            polygon_sides: self.polygon_sides,
            cursor: (frame_idx == self.cursor_frame).then_some(self.cursor),
            loupe: self.loupe_held,
            naming,
        };
        let background = &frame.background;
        let Some(slot) = self.views.iter_mut().find(|s| s.frame == frame_idx) else {
            return;
        };
        let result = slot.view.present(|buffer, size| {
            render::compose(buffer, size, background, &state);
        });
        let Err(e) = result else {
            self.warned_render_failure = false;
            return;
        };
        if !self.warned_render_failure {
            log::error!("render failed: {e:#}");
            self.warned_render_failure = true;
        }
    }

    fn commit_label(&mut self) {
        let changed = match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::LabelEditing {
                target: EditTarget::Selection,
                index,
                text,
            } => self.selections.set_label(index, text),
            Mode::LabelEditing {
                target: EditTarget::Measure,
                index,
                text,
            } => self.selections.label_measure(index, text),
            other => {
                self.mode = other;
                false
            }
        };
        if changed {
            self.mark_dirty();
        }
        self.caret_deadline = None;
    }

    fn grab_tolerance(&self) -> i32 {
        GRAB_TOLERANCE * self.frames[self.cursor_frame].ui_scale
    }

    /// The measure under the cursor, but only while the measure tool is
    /// active — otherwise a ruler lying across a rect would swallow the
    /// other tools' delete and label keys.
    fn measure_at_cursor(&self) -> Option<usize> {
        if self.tool != ToolKind::Measure {
            return None;
        }
        let monitor = self.frames[self.cursor_frame].info.index;
        self.selections
            .grab_measure(monitor, self.cursor, self.grab_tolerance())
            .map(|(index, _)| index)
    }

    fn grab_at_cursor(&self) -> Option<(usize, GrabKind)> {
        let monitor = self.frames[self.cursor_frame].info.index;
        self.selections
            .grab_topmost(monitor, self.cursor, self.grab_tolerance())
    }

    fn mouse_pressed(&mut self) {
        if matches!(self.mode, Mode::LabelEditing { .. }) {
            self.commit_label();
        }
        if matches!(self.mode, Mode::SessionNaming { .. }) {
            self.commit_session_name();
        }
        // A drag's *first* point matters as much as its last: without
        // this the rect's origin lands wherever the click did and only
        // the opposite corner snaps, which is half a feature.
        self.gesture_cursor = self.snap_cursor();
        // The measure tool owns the pointer while it is active: a press
        // grabs an existing ruler's endpoint or body, and otherwise
        // starts a new one. Shapes are not grabbable in this mode, so a
        // ruler drawn over a rect stays reachable.
        if self.tool == ToolKind::Measure {
            let monitor = self.frames[self.cursor_frame].info.index;
            if let Some((index, grab)) =
                self.selections
                    .grab_measure(monitor, self.cursor, self.grab_tolerance())
            {
                let original = self.selections.measures()[index].line;
                self.mode = Mode::MeasureDragging {
                    frame: self.cursor_frame,
                    index,
                    grab,
                    original,
                    grab_offset: Point::new(
                        self.cursor.x - original.a.x,
                        self.cursor.y - original.a.y,
                    ),
                };
                self.redraw_all();
                return;
            }
            if !self.frames[self.cursor_frame]
                .draw_rect
                .contains(self.cursor)
            {
                return;
            }
            self.mode = Mode::Drawing {
                frame: self.cursor_frame,
                start: self.gesture_cursor,
                path: Vec::new(),
            };
            self.redraw_all();
            return;
        }
        match self.grab_at_cursor() {
            Some((index, GrabKind::Resize(handle))) => {
                self.mode = Mode::Resizing {
                    frame: self.cursor_frame,
                    index,
                    handle,
                    original: self.selections.items()[index].shape.clone(),
                };
            }
            Some((index, GrabKind::Move)) => {
                // Alt-drag peels off a duplicate: the copy joins the top of
                // the stack (an ordinary undoable add) and the drag moves
                // it, leaving the original where it was.
                let index = if self.alt_down {
                    let copy = self.selections.items()[index].clone();
                    self.selections.add(copy);
                    self.mark_dirty();
                    self.selections.len() - 1
                } else {
                    index
                };
                let sel = &self.selections.items()[index];
                let shape = sel.shape.clone();
                let origin = shape.grab_origin_rotated(sel.rot_deg);
                self.mode = Mode::Dragging {
                    frame: self.cursor_frame,
                    index,
                    grab_offset: Point::new(self.cursor.x - origin.x, self.cursor.y - origin.y),
                    original: shape,
                };
            }
            None => {
                // Only start drawing when the click lands inside the
                // frame's drawable region. In desktop mode this is the
                // whole frame; in `--target` mode it is the target window
                // — and a click on the surrounding dead space should
                // simply do nothing rather than start a shape that would
                // be refused at save time.
                if !self.frames[self.cursor_frame]
                    .draw_rect
                    .contains(self.cursor)
                {
                    return;
                }
                self.mode = Mode::Drawing {
                    frame: self.cursor_frame,
                    start: self.gesture_cursor,
                    path: vec![self.gesture_cursor],
                };
            }
        }
        self.redraw_all();
    }

    fn mouse_released(&mut self) {
        // A release is allowed to be the first event at its position —
        // no move need have been delivered there — so the commit point is
        // resolved here rather than trusted from the last move.
        self.refresh_gesture_cursor();
        let preview = self.preview_for(self.cursor_frame);
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Drawing { frame, start, .. } if self.tool == ToolKind::Measure => {
                let line = Line::new(start, self.gesture_cursor);
                let line = if self.shift_down {
                    line.constrained()
                } else {
                    line
                };
                // A click with no drag is not a measurement; it would
                // save a zero-length ruler nobody meant to make.
                if line.length() >= f64::from(self.grab_tolerance()) {
                    self.selections
                        .add_measure(Measure::new(line, self.frames[frame].info.index));
                    self.mark_dirty();
                }
            }
            Mode::MeasureDragging {
                index, original, ..
            } => {
                if self.selections.commit_measure(index, original) {
                    self.mark_dirty();
                }
            }
            Mode::Drawing { frame, .. } => {
                let shape = match (self.tool, preview) {
                    // A freehand stroke commits simplified: the jitter
                    // goes, the shape stays.
                    (ToolKind::Freehand, Some(Shape::Poly { points })) => {
                        let simplified = pixelcoords_core::geometry::simplify_path(&points, 2.0);
                        (simplified.len() >= 3).then_some(Shape::Poly { points: simplified })
                    }
                    (_, preview) => preview,
                };
                if let Some(shape) = shape
                    && shape_is_committable(&shape)
                {
                    self.selections
                        .add(Selection::new(shape, self.frames[frame].info.index));
                    self.mark_dirty();
                }
            }
            Mode::Dragging {
                index, original, ..
            }
            | Mode::Resizing {
                index, original, ..
            } => {
                if self.selections.commit_move(index, original) {
                    self.mark_dirty();
                }
            }
            other => self.mode = other,
        }
        self.redraw_all();
    }

    fn cursor_moved(&mut self, window_id: WindowId, position: winit::dpi::PhysicalPosition<f64>) {
        let Some(frame_idx) = self.frame_of_window(window_id) else {
            return;
        };
        // Mid-drag positions from other monitors are ignored: shapes are
        // per-monitor in v1 and the drag stays clamped to where it started.
        match self.mode {
            Mode::Drawing { frame, .. }
            | Mode::Dragging { frame, .. }
            | Mode::Resizing { frame, .. }
                if frame != frame_idx =>
            {
                return;
            }
            _ => {}
        }
        let previous_frame = self.cursor_frame;
        self.cursor_frame = frame_idx;
        let Some(slot) = self.views.iter().find(|s| s.frame == frame_idx) else {
            return;
        };
        self.cursor = slot.view.coord_map().window_to_capture(position);
        if self.panel_held {
            self.panel_origin = Some((frame_idx, self.cursor));
        }
        // The live readout tracks every move; crossing monitors clears the
        // chip baked into the frame the cursor left.
        if previous_frame != frame_idx {
            self.redraw_frame(previous_frame);
        }
        self.redraw_frame(frame_idx);
        self.update_active_gesture();
    }

    /// Open the label editor on whatever the cursor is over — the ruler
    /// first, since the measure tool is the only way to reach one.
    fn begin_label_edit(&mut self) {
        let target = self.measure_at_cursor().map_or_else(
            || {
                self.hit_at_cursor()
                    .map(|index| (EditTarget::Selection, index))
            },
            |index| Some((EditTarget::Measure, index)),
        );
        let Some((target, index)) = target else {
            return;
        };
        let text = match target {
            EditTarget::Selection => self.selections.items()[index].label.clone(),
            EditTarget::Measure => self.selections.measures()[index].label.clone(),
        };
        self.mode = Mode::LabelEditing {
            target,
            index,
            text,
        };
        self.caret_visible = true;
        self.caret_deadline = Some(Instant::now() + CARET_BLINK);
        self.redraw_all();
    }

    /// Where a measure drag puts the ruler, given what was grabbed.
    fn dragged_measure(
        &self,
        grab: MeasureGrab,
        original: Line,
        grab_offset: Point,
        frame: usize,
    ) -> Line {
        let MeasureGrab::Endpoint(is_a) = grab else {
            let target = Point::new(
                self.gesture_cursor.x - grab_offset.x,
                self.gesture_cursor.y - grab_offset.y,
            );
            return original.translated(target.x - original.a.x, target.y - original.a.y);
        };
        let free = self.frames[frame]
            .draw_rect()
            .clamp_point(self.gesture_cursor);
        let dragged = if is_a {
            Line::new(free, original.b)
        } else {
            Line::new(original.a, free)
        };
        if !self.shift_down {
            return dragged;
        }
        // `constrained` pivots on `a`, so dragging `a` needs the line
        // reversed on the way in and back out — the endpoint under the
        // hand is the one that must snap.
        if is_a {
            let snapped = Line::new(dragged.b, dragged.a).constrained();
            Line::new(snapped.b, snapped.a)
        } else {
            dragged.constrained()
        }
    }

    /// Bring `gesture_cursor` and `snap_hit` up to date with the pointer.
    ///
    /// Every gesture step wants the snapped point, and computing it in
    /// one place keeps `snap_hit` in step with what the gesture actually
    /// used — the overlay draws that edge, so a second computation could
    /// draw a guide the committed geometry never touched. Idle clears
    /// instead: a hovering cursor must stay exact, or the readout, the
    /// loupe, and hit-testing all start lying.
    fn refresh_gesture_cursor(&mut self) {
        if matches!(
            self.mode,
            Mode::Idle | Mode::LabelEditing { .. } | Mode::SessionNaming { .. }
        ) {
            self.snap_hit = Snap::default();
        } else {
            self.gesture_cursor = self.snap_cursor();
        }
    }

    /// Advance whatever gesture is in flight to `self.cursor`. Split from
    /// `cursor_moved` so headless tests can drive a drag by setting the
    /// cursor, without a window to convert positions from.
    fn update_active_gesture(&mut self) {
        self.refresh_gesture_cursor();
        match &mut self.mode {
            Mode::Drawing { frame, path, .. } => {
                let frame = *frame;
                // The freehand stroke grows only on meaningful movement;
                // 2px spacing keeps point counts sane before simplify.
                if self.tool == ToolKind::Freehand
                    && self.frames[frame].draw_rect.contains(self.cursor)
                    && path.last().is_none_or(|last| {
                        (self.cursor.x - last.x).abs() + (self.cursor.y - last.y).abs() >= 2
                    })
                {
                    path.push(self.cursor);
                }
                self.redraw_frame(frame);
            }
            Mode::Dragging {
                frame,
                index,
                grab_offset,
                original,
            } => {
                let (frame, index, grab_offset, original) =
                    (*frame, *index, *grab_offset, original.clone());
                // Defensive: the gesture gate should make a stale index
                // impossible, but a missing selection must never panic.
                let Some(rot) = self.selections.get(index).map(|s| s.rot_deg) else {
                    self.mode = Mode::Idle;
                    return;
                };
                let moved = original.clamp_move_rotated(
                    rot,
                    grab_offset,
                    self.gesture_cursor,
                    self.frames[frame].draw_rect(),
                );
                self.selections.set_shape_live(index, moved);
                self.redraw_frame(frame);
            }
            Mode::Resizing {
                frame,
                index,
                handle,
                original,
            } => {
                let (frame, index, handle, original) = (*frame, *index, *handle, original.clone());
                let Some(rot) = self.selections.get(index).map(|s| s.rot_deg) else {
                    self.mode = Mode::Idle;
                    return;
                };
                let resized = original.resize_to_rotated(
                    rot,
                    handle,
                    self.gesture_cursor,
                    self.frames[frame].draw_rect(),
                    self.shift_down,
                );
                self.selections.set_shape_live(index, resized);
                self.redraw_frame(frame);
            }
            Mode::MeasureDragging {
                frame,
                index,
                grab,
                original,
                grab_offset,
            } => {
                let (frame, index, grab, original, grab_offset) =
                    (*frame, *index, *grab, *original, *grab_offset);
                let moved = self.dragged_measure(grab, original, grab_offset, frame);
                self.selections.set_measure_line_live(index, moved);
                self.redraw_frame(frame);
            }
            Mode::Idle => self.update_hover_cursor(),
            Mode::LabelEditing { .. } | Mode::SessionNaming { .. } => {}
        }
    }

    /// Cursor-icon feedback while idle: resize arrows on borders, a move
    /// cursor inside shapes, crosshair elsewhere.
    fn update_hover_cursor(&mut self) {
        if self.tool == ToolKind::Measure {
            let icon = match self.measure_at_cursor() {
                Some(_) => CursorIcon::Move,
                None if !self.frames[self.cursor_frame]
                    .draw_rect()
                    .contains(self.cursor) =>
                {
                    CursorIcon::NotAllowed
                }
                None => CursorIcon::Crosshair,
            };
            self.set_cursor_icon(icon);
            return;
        }
        let icon = match self.grab_at_cursor() {
            Some((index, GrabKind::Resize(handle))) => {
                resize_icon(handle, &self.selections.items()[index].shape, self.cursor)
            }
            Some((_, GrabKind::Move)) => CursorIcon::Move,
            // Outside the drawable region a click would do nothing, so
            // stop advertising "draw here" — a plain arrow tells the
            // truth. The overlay already dims those pixels; the cursor
            // has to match, or it reads as a bug.
            None if !self.frames[self.cursor_frame]
                .draw_rect()
                .contains(self.cursor) =>
            {
                CursorIcon::NotAllowed
            }
            None => CursorIcon::Crosshair,
        };
        self.set_cursor_icon(icon);
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if icon == self.cursor_icon {
            return;
        }
        self.cursor_icon = icon;
        if let Some(slot) = self.views.iter().find(|s| s.frame == self.cursor_frame) {
            slot.view.set_cursor(icon);
        }
    }

    /// The winit-facing shim: decode, decide, and hand back only what
    /// needs the event loop.
    fn key_event(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        if let Some(action) = self.handle_key(&Keystroke::decode(event)) {
            self.apply_action(event_loop, action);
        }
    }

    /// Everything a keystroke does, minus the two actions that need the
    /// event loop — those come back for the caller to apply.
    fn handle_key(&mut self, key: &Keystroke) -> Option<Action> {
        // A Space release always parks the panel, whatever mode we are in —
        // the label editor eats key events, and a release swallowed there
        // would leave the panel glued to the cursor.
        if !key.pressed && key.builtin == Some(Builtin::Space) {
            self.panel_held = false;
        }
        if matches!(self.mode, Mode::LabelEditing { .. }) {
            if key.pressed {
                self.label_editor_key(key);
            }
            return None;
        }
        if matches!(self.mode, Mode::SessionNaming { .. }) {
            if key.pressed {
                self.name_editor_key(key);
            }
            return None;
        }

        // Esc cancels whatever is in progress; with nothing in progress it
        // asks to quit (twice within the grace period when work is
        // unsaved) — the same double-tap guard the old Q binding had.
        if key.pressed && key.builtin == Some(Builtin::Escape) {
            let quit = match std::mem::replace(&mut self.mode, Mode::Idle) {
                Mode::Dragging {
                    index, original, ..
                }
                | Mode::Resizing {
                    index, original, ..
                } => {
                    self.selections.set_shape_live(index, original);
                    false
                }
                Mode::Idle => true,
                _ => false,
            };
            self.redraw_all();
            return quit.then_some(Action::Quit);
        }

        // M is a hold like Space: the loupe lives while it is down.
        if key.builtin == Some(Builtin::Loupe) {
            self.loupe_held = key.pressed;
            self.redraw_frame(self.cursor_frame);
            return None;
        }

        // Space is a hold, not a binding: while it is down the control
        // panel rides the cursor; the release is handled above.
        if key.builtin == Some(Builtin::Space) {
            if key.pressed {
                self.panel_held = true;
            }
            return None;
        }

        // Number keys size the polygon tool: 3 to 9 sides.
        if self.tool == ToolKind::Polygon
            && key.pressed
            && let Some(Builtin::Digit(d)) = key.builtin
            && (3..=9).contains(&d)
        {
            self.polygon_sides = d;
            self.set_flash(format!("Polygon sides: {d}"), FLASH_TOOL);
            return None;
        }

        // Arrows nudge the shape under the cursor — 1px, 10px with Shift,
        // Alt resizes instead. Built in like Esc and Space (arrows are
        // named keys the binding grammar does not cover); holding the key
        // repeats.
        if let Some(Builtin::Arrow { dx, dy }) = key.builtin {
            if key.pressed && matches!(self.mode, Mode::Idle) {
                self.nudge(dx, dy);
            }
            return None;
        }

        let action = match_event(
            &self.bindings,
            key.binding?,
            key.edge(),
            self.overlay_state(),
        )?;
        // Shift turns undo into redo — modifiers are not part of the
        // binding grammar, and Shift+Z is what every hand expects.
        let action = if action == Action::Undo && self.shift_down {
            Action::Redo
        } else {
            action
        };
        // Mid-gesture, most actions could mutate the selection set out
        // from under the gesture's held index (undo/delete) or hijack
        // the mode (label edit) — refuse them until the mouse is up.
        if !matches!(self.mode, Mode::Idle) && !allowed_mid_gesture(action) {
            log::debug!("ignoring {action:?} during an active mouse gesture");
            return None;
        }
        // The two that need the event loop go back to the caller; the rest
        // are done here, so a test can drive everything but quitting.
        if matches!(action, Action::Quit | Action::ReleaseMonitor) {
            return Some(action);
        }
        self.apply_local_action(action);
        None
    }

    fn label_editor_key(&mut self, key: &Keystroke) {
        let Mode::LabelEditing { text, .. } = &mut self.mode else {
            return;
        };
        match key.builtin {
            Some(Builtin::Enter) => self.commit_label(),
            Some(Builtin::Escape) => {
                self.mode = Mode::Idle;
                self.caret_deadline = None;
            }
            Some(Builtin::Backspace) => {
                text.pop();
            }
            _ => {
                if append_typed(text, key.text.as_deref()) {
                    self.set_flash(
                        format!("Label limit {MAX_LABEL_LEN} characters"),
                        FLASH_TOOL,
                    );
                }
            }
        }
        self.caret_visible = true;
        self.caret_deadline = Some(Instant::now() + CARET_BLINK);
        self.redraw_all();
    }

    fn name_editor_key(&mut self, key: &Keystroke) {
        let Mode::SessionNaming { text } = &mut self.mode else {
            return;
        };
        match key.builtin {
            Some(Builtin::Enter) => self.commit_session_name(),
            Some(Builtin::Escape) => {
                self.mode = Mode::Idle;
                self.caret_deadline = None;
            }
            Some(Builtin::Backspace) => {
                text.pop();
            }
            _ => {
                if append_typed(text, key.text.as_deref()) {
                    self.set_flash(
                        format!("Label limit {MAX_LABEL_LEN} characters"),
                        FLASH_TOOL,
                    );
                }
            }
        }
        self.caret_visible = true;
        self.caret_deadline = Some(Instant::now() + CARET_BLINK);
        self.redraw_all();
    }

    /// Commit the typed session name; empty clears it. A name change is
    /// an edit — it needs a save to persist.
    fn commit_session_name(&mut self) {
        let Mode::SessionNaming { text } = std::mem::replace(&mut self.mode, Mode::Idle) else {
            return;
        };
        self.caret_deadline = None;
        let name = Some(text.trim().to_string()).filter(|t| !t.is_empty());
        if self.session_meta.name == name {
            return;
        }
        let message = match &name {
            Some(n) => format!("Session named {n:?} - S saves it"),
            None => "Session name cleared - S saves it".to_string(),
        };
        self.session_meta.name = name;
        self.mark_dirty();
        self.set_flash(message, FLASH_SAVE);
    }

    /// Re-apply the resize constraint when Shift is pressed or released
    /// mid-gesture, so the shape follows the modifier without the cursor
    /// having to move.
    fn shift_changed(&mut self, shift: bool) {
        if shift == self.shift_down {
            return;
        }
        self.shift_down = shift;
        // Every gesture that reads the modifier has to answer to it, not
        // just resizing. A drag preview is derived at render time, so it
        // only needs the repaint; a measure drag is held as live state and
        // needs recomputing the way a resize does.
        match &self.mode {
            Mode::Drawing { frame, .. } => {
                let frame = *frame;
                self.redraw_frame(frame);
            }
            Mode::Resizing { frame, .. } | Mode::MeasureDragging { frame, .. } => {
                let frame = *frame;
                self.update_active_gesture();
                self.redraw_frame(frame);
            }
            Mode::Idle
            | Mode::Dragging { .. }
            | Mode::LabelEditing { .. }
            | Mode::SessionNaming { .. } => {}
        }
    }

    /// Quit, unless there is unsaved work — then arm a second-press window
    /// and tell the user.
    /// Move (or, with Alt, resize) the shape under the cursor by one
    /// step. Refuses to push any part off the frame rather than clamping
    /// silently — a nudge is precision work, and 7 of the 10 requested
    /// pixels landing is worse than none.
    fn nudge(&mut self, dx: i32, dy: i32) {
        let Some(index) = self.hit_at_cursor() else {
            return;
        };
        let step = if self.shift_down { 10 } else { 1 };
        let (dx, dy) = (dx * step, dy * step);
        let sel = &self.selections.items()[index];
        let original = sel.shape.clone();
        let rot = sel.rot_deg;
        let candidate = if self.alt_down {
            resize_by(&original, dx, dy)
        } else {
            Some(original.translated(dx, dy))
        };
        let Some(candidate) = candidate else { return };
        let size = self.frames[self.cursor_frame].size;
        let bb = candidate.rotated_bbox(rot);
        if bb.x < 0 || bb.y < 0 || bb.x + bb.w > size.w || bb.y + bb.h > size.h {
            return;
        }
        self.selections.set_shape_live(index, candidate);
        if self.selections.commit_move(index, original) {
            self.mark_dirty();
            self.redraw_frame(self.cursor_frame);
        }
    }

    /// Closing one overlay window when others remain releases that display
    /// instead of quitting: the window goes away and the monitor is live
    /// again. The last window keeps quit semantics, so the only way to end
    /// the run is still an explicit one.
    ///
    /// Refused, rather than negotiated, in two cases. A monitor still
    /// holding marks would strand them — deleting is an explicit, undoable
    /// act and a window close is not a confirmation dialog. And a release
    /// mid-gesture would pull the frame out from under a drag whose stored
    /// index points into `frames`; the same reasoning as
    /// `allowed_mid_gesture`.
    fn release_frame(&mut self, event_loop: &ActiveEventLoop, frame: usize) {
        if self.try_release_frame(frame) == Release::Quit {
            self.request_quit(event_loop);
        }
    }

    /// The half of a release that touches no window-system types, so the
    /// bookkeeping below can be tested headless — which matters more here
    /// than anywhere else in this file, because getting it wrong points a
    /// live index at the wrong display rather than crashing.
    fn try_release_frame(&mut self, frame: usize) -> Release {
        if self.frames.len() <= 1 || !matches!(self.mode, Mode::Idle) {
            return Release::Quit;
        }
        let monitor = self.frames[frame].info.index;
        // Rulers count as marks: releasing a display they sit on would
        // orphan a measurement with no way to see or delete it.
        let held = self
            .selections
            .items()
            .iter()
            .filter(|s| s.monitor == monitor)
            .count()
            + self
                .selections
                .measures()
                .iter()
                .filter(|m| m.monitor == monitor)
                .count();
        if held > 0 {
            self.set_flash(
                format!(
                    "{}{held}{}",
                    self.strings.hud_release_blocked_prefix,
                    self.strings.hud_release_blocked_suffix
                ),
                FLASH_SAVE,
            );
            return Release::Blocked;
        }

        self.frames.remove(frame);
        self.views.retain(|slot| slot.frame != frame);
        // Everything holding a *position* into `frames` shifts down. Miss
        // one and it silently addresses the wrong display: `ViewSlot.frame`
        // in particular backs `frame_of_window`, which every later event
        // resolves through.
        for slot in &mut self.views {
            if slot.frame > frame {
                slot.frame -= 1;
            }
        }
        if self.cursor_frame > frame {
            self.cursor_frame -= 1;
        } else if self.cursor_frame == frame {
            self.cursor_frame = 0;
        }
        self.panel_origin = match self.panel_origin {
            // The panel's host is gone: it moves to where the cursor now
            // is rather than vanishing with the display.
            Some((host, _)) if host == frame => None,
            Some((host, at)) if host > frame => Some((host - 1, at)),
            other => other,
        };
        crate::state::save_panel(self.panel_origin);
        // Undo entries can reference a frame that no longer exists, and
        // filtering them would be guesswork about what a half-valid history
        // means. Truncating matches resume, where the reopen point is the
        // floor.
        self.selections = SelectionSet::seed(
            self.selections.items().to_vec(),
            self.selections.measures().to_vec(),
        );
        self.set_flash(self.strings.hud_released.to_string(), FLASH_SAVE);
        self.redraw_all();
        Release::Done
    }

    fn request_quit(&mut self, event_loop: &ActiveEventLoop) {
        let unsaved = self.dirty;
        let armed = self
            .quit_armed_until
            .is_some_and(|until| Instant::now() < until);
        if unsaved && !armed {
            self.quit_armed_until = Some(Instant::now() + FLASH_SAVE);
            self.set_flash(self.strings.hud_quit_unsaved.to_string(), FLASH_SAVE);
            return;
        }
        event_loop.exit();
    }

    fn apply_action(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        match action {
            Action::Quit => self.request_quit(event_loop),
            Action::ReleaseMonitor => self.release_frame(event_loop, self.cursor_frame),
            other => self.apply_local_action(other),
        }
    }

    /// Every action that does not need the event loop — which is every
    /// action except quitting and releasing a monitor. Split out so the
    /// keyboard surface is reachable from headless tests.
    fn apply_local_action(&mut self, action: Action) {
        match action {
            // Intercepted above; listed so the match stays exhaustive.
            Action::Quit | Action::ReleaseMonitor => {}
            Action::Save => self.save(),
            Action::NextTool => {
                // The mid-gesture gate guarantees Idle here.
                self.tool = self.tool.next();
                let name = match self.tool {
                    ToolKind::Rect => "TOOL: RECTANGLE",
                    ToolKind::Circle => "TOOL: CIRCLE",
                    ToolKind::Ellipse => "TOOL: ELLIPSE",
                    ToolKind::Triangle => "TOOL: TRIANGLE",
                    ToolKind::Polygon => "TOOL: POLYGON",
                    ToolKind::Freehand => "TOOL: FREEHAND",
                    ToolKind::Poly => "TOOL: POLY",
                    ToolKind::Measure => "TOOL: MEASURE",
                };
                self.set_flash(name.to_string(), FLASH_TOOL);
            }
            Action::DeleteAtCursor => {
                if let Some(index) = self.measure_at_cursor() {
                    self.selections.delete_measure(index);
                    self.mark_dirty();
                    self.redraw_all();
                } else if let Some(index) = self.hit_at_cursor() {
                    self.selections.delete(index);
                    self.mark_dirty();
                    self.redraw_all();
                }
            }
            Action::LabelEditAtCursor => self.begin_label_edit(),
            Action::RotateCcw | Action::RotateCw => {
                if let Some(index) = self.hit_at_cursor() {
                    let step = if self.shift_down { 15 } else { 1 };
                    let delta = if matches!(action, Action::RotateCw) {
                        step
                    } else {
                        -step
                    };
                    if let Some(deg) = self.selections.rotate(index, delta) {
                        self.mark_dirty();
                        self.set_flash(format!("ROTATION {deg}"), FLASH_TOOL);
                    }
                }
            }
            Action::Undo => {
                if self.selections.undo() {
                    self.mark_dirty();
                    self.redraw_all();
                }
            }
            Action::Redo => {
                if self.selections.redo() {
                    self.mark_dirty();
                    self.redraw_all();
                }
            }
            Action::NameSession => {
                // The mid-gesture gate guarantees Idle here; open with the
                // current name so editing appends rather than restarts.
                self.mode = Mode::SessionNaming {
                    text: self.session_meta.name.clone().unwrap_or_default(),
                };
                self.caret_visible = true;
                self.caret_deadline = Some(Instant::now() + CARET_BLINK);
                self.redraw_all();
            }
            Action::ToggleSnap => {
                self.snap.enabled = !self.snap.enabled;
                if self.snap.enabled {
                    for frame in &self.frames {
                        let _ = frame.edges();
                    }
                } else {
                    self.snap_hit = Snap::default();
                }
                let message = if self.snap.enabled {
                    self.strings.hud_snap_on
                } else {
                    self.strings.hud_snap_off
                };
                self.set_flash(message.to_string(), FLASH_TOOL);
                self.redraw_all();
            }
            Action::TogglePanel => {
                self.panel_hidden = !self.panel_hidden;
                if self.panel_hidden {
                    // A fading hint, so hiding is never a trap.
                    self.set_flash("H shows the panel again".to_string(), FLASH_TOOL);
                }
                self.redraw_all();
            }
            Action::CycleOverlap => {
                let monitor = self.frames[self.cursor_frame].info.index;
                if self.selections.cycle_at(monitor, self.cursor) {
                    // Stacking order is save order, so this is an edit.
                    self.mark_dirty();
                    self.redraw_all();
                }
            }
            Action::NextTheme => log::debug!("next_theme is a no-op in snapshot mode"),
        }
    }

    fn save(&mut self) {
        let pairs: Vec<(&MonitorInfo, &RgbaImage)> =
            self.frames.iter().map(|f| (&f.info, &f.rgba)).collect();
        match crate::save::write_session(
            &self.out_dir,
            &pairs,
            &self.selections,
            self.target.as_ref(),
            &self.session_meta,
            !self.saved_once,
            &self.last_crops,
        ) {
            Ok(outcome) => {
                self.dirty = false;
                self.quit_armed_until = None;
                self.saved_once = true;
                let message = format!(
                    "{}{}",
                    self.strings.hud_saved_prefix,
                    outcome.json_path.display()
                );
                self.last_crops = outcome.crops;
                self.set_flash(message, FLASH_SAVE);
            }
            Err(e) => {
                log::error!("save failed: {e:#}");
                // `{e:#}`, not `{e}`: the overlay is covering the terminal
                // the log went to, so the on-screen copy is the only one
                // the user can read — it needs the cause, not just the
                // outermost context ("writing crop-0.png" alone says
                // nothing about a full disk).
                let message = format!("{}{e:#}", self.strings.hud_save_failed_prefix);
                self.set_flash(message, FLASH_SAVE);
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.views.is_empty() {
            return;
        }
        let mut available: Vec<winit::monitor::MonitorHandle> =
            event_loop.available_monitors().collect();
        for (frame_idx, frame) in self.frames.iter().enumerate() {
            let handle = claim_monitor(self.presentation, frame, &mut available);
            match OverlayView::new(event_loop, handle.as_ref(), frame.size, self.presentation) {
                Ok(view) => self.views.push(ViewSlot {
                    frame: frame_idx,
                    view,
                }),
                Err(e) => {
                    self.fail(event_loop, e.context("creating overlay window"));
                    return;
                }
            }
        }
        self.redraw_all();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.frame_of_window(window_id).is_none() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => match self.frame_of_window(window_id) {
                Some(frame) => self.release_frame(event_loop, frame),
                None => self.request_quit(event_loop),
            },
            WindowEvent::RedrawRequested => self.render(window_id),
            WindowEvent::Resized(_) => self.redraw_all(),
            WindowEvent::CursorMoved { position, .. } => self.cursor_moved(window_id, position),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.shift_changed(modifiers.state().shift_key());
                self.alt_down = modifiers.state().alt_key();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.mouse_pressed(),
                ElementState::Released => self.mouse_released(),
            },
            WindowEvent::KeyboardInput { event, .. } => self.key_event(event_loop, &event),
            _ => {}
        }
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if !matches!(cause, StartCause::ResumeTimeReached { .. }) {
            return;
        }
        let now = Instant::now();
        if matches!(self.mode, Mode::LabelEditing { .. })
            && self.caret_deadline.is_some_and(|d| d <= now)
        {
            self.caret_visible = !self.caret_visible;
            self.caret_deadline = Some(now + CARET_BLINK);
            self.redraw_all();
        }
        if self.flash.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.flash = None;
            self.redraw_all();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut deadline: Option<Instant> = None;
        if matches!(self.mode, Mode::LabelEditing { .. }) {
            deadline = self.caret_deadline;
        }
        if let Some((_, until)) = &self.flash {
            deadline = Some(deadline.map_or(*until, |d| d.min(*until)));
        }
        event_loop.set_control_flow(match deadline {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// Take the monitor a frame's overlay belongs on out of `available`, so
/// two frames cannot claim the same display.
///
/// A windowed overlay claims none: it is sized to its own capture and the
/// window system places it, so there is no monitor to match and nothing to
/// warn about when none matches.
fn claim_monitor(
    presentation: Presentation,
    frame: &MonitorFrame,
    available: &mut Vec<winit::monitor::MonitorHandle>,
) -> Option<winit::monitor::MonitorHandle> {
    if presentation == Presentation::Windowed {
        return None;
    }
    let expected = winit::dpi::PhysicalSize::new(frame.size.w as u32, frame.size.h as u32);
    let origin = frame.info.origin_physical();
    let expected_pos = (f64::from(origin.x), f64::from(origin.y));
    let candidates: Vec<(
        winit::dpi::PhysicalSize<u32>,
        winit::dpi::PhysicalPosition<i32>,
    )> = available.iter().map(|m| (m.size(), m.position())).collect();
    let Some(index) = pick_monitor(&candidates, expected, expected_pos) else {
        log::warn!(
            "no unused monitor matches capture {}x{} for '{}'; using the current screen",
            expected.width,
            expected.height,
            frame.info.name
        );
        return None;
    };
    Some(available.swap_remove(index))
}

/// Pick the available monitor for a captured frame: same pixel size,
/// nearest to the expected physical position. Position is the tiebreaker
/// that keeps two identical displays from getting each other's frozen
/// capture — size alone cannot tell them apart.
fn pick_monitor(
    candidates: &[(
        winit::dpi::PhysicalSize<u32>,
        winit::dpi::PhysicalPosition<i32>,
    )],
    expected_size: winit::dpi::PhysicalSize<u32>,
    expected_pos: (f64, f64),
) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, (size, _))| *size == expected_size)
        .min_by_key(|(_, (_, pos))| {
            let dx = f64::from(pos.x) - expected_pos.0;
            let dy = f64::from(pos.y) - expected_pos.1;
            (dx * dx + dy * dy) as i64
        })
        .map(|(i, _)| i)
}

/// Hotkey actions that stay safe while a mouse gesture (draw, drag, or
/// resize) is in progress. Everything else is refused mid-gesture: undo and
/// delete would invalidate the gesture's held selection index, label edit
/// would hijack the mode, and tool/save changes mid-drag are never what the
/// user meant.
const fn allowed_mid_gesture(action: Action) -> bool {
    matches!(action, Action::Quit)
}

/// Which arrow cursor a resize grab should show. Rect corners get the
/// diagonal matching their orientation; circle rims pick the axis the
/// cursor is most displaced along from the center.
fn resize_icon(handle: ResizeHandle, shape: &Shape, cursor: Point) -> CursorIcon {
    let ResizeHandle::RectEdges {
        left,
        right,
        top,
        bottom,
    } = handle
    else {
        // Circle rim: pick the axis the cursor is most displaced along.
        let center = shape.grab_origin();
        let horizontal = (cursor.x - center.x).abs() >= (cursor.y - center.y).abs();
        return if horizontal {
            CursorIcon::EwResize
        } else {
            CursorIcon::NsResize
        };
    };
    if (left && top) || (right && bottom) {
        return CursorIcon::NwseResize;
    }
    if (right && top) || (left && bottom) {
        return CursorIcon::NeswResize;
    }
    if left || right {
        return CursorIcon::EwResize;
    }
    CursorIcon::NsResize
}

/// Append the printable characters of `typed` to a label being edited,
/// stopping at the length cap. Free rather than a method because `text` is
/// borrowed out of the mode it lives in.
/// Append what a keystroke typed, stopping at the cap.
///
/// Returns whether the cap turned any character away, so the caller can
/// say so. A keystroke that vanishes with nothing on screen reads as a
/// dropped input rather than a limit — the same silent refusal this
/// project rejects everywhere else.
fn append_typed(text: &mut String, typed: Option<&str>) -> bool {
    let Some(typed) = typed else {
        return false;
    };
    let mut refused = false;
    for c in typed.chars().filter(|c| !c.is_control()) {
        if text.chars().count() >= MAX_LABEL_LEN {
            refused = true;
            break;
        }
        text.push(c);
    }
    refused
}

fn shape_is_committable(shape: &Shape) -> bool {
    match shape {
        Shape::Rect(r) => r.w >= 2 && r.h >= 2,
        Shape::Circle { r, .. } => *r >= 2,
        Shape::Ellipse { rx, ry, .. } => *rx >= 2 && *ry >= 2,
        Shape::Triangle { .. } | Shape::Poly { .. } => {
            let b = shape.bbox();
            b.w >= 2 && b.h >= 2
        }
    }
}

/// The unit step an arrow key asks for, screen-down positive.
/// Keys the overlay handles itself, outside the binding grammar.
///
/// The grammar covers letters and Tab; these are the named keys and the
/// two characters (`M`, the digits) whose behavior is not a bindable
/// action but a mode of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Escape,
    Space,
    Enter,
    Backspace,
    Arrow {
        dx: i32,
        dy: i32,
    },
    /// `M`, the loupe hold.
    Loupe,
    Digit(u32),
}

/// One keyboard event, decoded out of winit's vocabulary into this
/// crate's.
///
/// The split exists so the keyboard logic can be tested. `KeyEvent` is
/// winit's and cannot be constructed outside it, which left the whole
/// key path — modifier tracking, the mid-gesture gate, nudging, the text
/// editors — unreachable from a test. Decoding happens in one place that
/// makes no decisions, and every decision happens in `handle_key`, which
/// takes this.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Keystroke {
    pressed: bool,
    repeat: bool,
    /// What the binding grammar calls this key, when it names it.
    binding: Option<KeyName>,
    /// What the overlay handles directly, when it does.
    builtin: Option<Builtin>,
    /// The characters this keystroke types, for the text editors.
    text: Option<String>,
}

impl Keystroke {
    /// Decode a winit event. Decisions belong in `handle_key`; this only
    /// translates.
    fn decode(event: &KeyEvent) -> Self {
        let builtin = match &event.logical_key {
            Key::Named(NamedKey::Escape) => Some(Builtin::Escape),
            Key::Named(NamedKey::Space) => Some(Builtin::Space),
            Key::Named(NamedKey::Enter) => Some(Builtin::Enter),
            Key::Named(NamedKey::Backspace) => Some(Builtin::Backspace),
            Key::Character(s) if s.eq_ignore_ascii_case("m") => Some(Builtin::Loupe),
            Key::Character(s) => s
                .chars()
                .next()
                .and_then(|c| c.to_digit(10))
                .map(Builtin::Digit),
            key => arrow_delta(key).map(|(dx, dy)| Builtin::Arrow { dx, dy }),
        };
        Self {
            pressed: event.state == ElementState::Pressed,
            repeat: event.repeat,
            binding: key_name(event),
            builtin,
            text: event.text.as_deref().map(str::to_owned),
        }
    }

    /// Which edge of the key this is, in the binding grammar's terms.
    const fn edge(&self) -> Edge {
        match (self.pressed, self.repeat) {
            (true, false) => Edge::Press,
            (true, true) => Edge::Repeat,
            (false, _) => Edge::Release,
        }
    }
}

fn arrow_delta(key: &Key) -> Option<(i32, i32)> {
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some((-1, 0)),
        Key::Named(NamedKey::ArrowRight) => Some((1, 0)),
        Key::Named(NamedKey::ArrowUp) => Some((0, -1)),
        Key::Named(NamedKey::ArrowDown) => Some((0, 1)),
        _ => None,
    }
}

/// The Alt+arrow resize: rects grow width/height, circles grow their
/// radius (right/down grow, left/up shrink), triangles have no single
/// obvious axis and are left to mouse resizing.
fn resize_by(shape: &Shape, dx: i32, dy: i32) -> Option<Shape> {
    match *shape {
        Shape::Rect(r) => {
            let (w, h) = (r.w + dx, r.h + dy);
            (w >= 2 && h >= 2).then(|| Shape::Rect(Rect::new(r.x, r.y, w, h)))
        }
        Shape::Circle { cx, cy, r } => {
            let grown = r + dx + dy;
            (grown >= 1).then_some(Shape::Circle { cx, cy, r: grown })
        }
        Shape::Ellipse { cx, cy, rx, ry } => {
            // Horizontal arrows size rx, vertical size ry.
            let (nrx, nry) = (rx + dx, ry + dy);
            (nrx >= 1 && nry >= 1).then_some(Shape::Ellipse {
                cx,
                cy,
                rx: nrx,
                ry: nry,
            })
        }
        Shape::Triangle { .. } | Shape::Poly { .. } => None,
    }
}

fn key_name(event: &KeyEvent) -> Option<KeyName> {
    match &event.logical_key {
        Key::Named(NamedKey::Tab) => Some(KeyName::Tab),
        Key::Named(NamedKey::CapsLock) => Some(KeyName::CapsLock),
        Key::Character(s) => s
            .chars()
            .next()
            .map(|c| KeyName::Character(c.to_ascii_uppercase())),
        _ => None,
    }
}

fn rgba_to_0rgb(img: &RgbaImage) -> Vec<u32> {
    img.pixels()
        .map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelcoords_core::config::Config;
    use pixelcoords_core::hotkeys::default_bindings;

    /// A windowless App over one fake 100x60 monitor — the event loop never
    /// runs, so only the pure state machine is exercised (redraws are
    /// no-ops with zero views).
    #[test]
    fn nudge_moves_resizes_and_respects_the_frame_edge() {
        let mut app = test_app();
        app.selections
            .add(Selection::new(Shape::Rect(Rect::new(10, 10, 20, 15)), 0));
        app.cursor = Point::new(15, 15);

        app.nudge(1, 0);
        assert_eq!(
            app.selections.items()[0].shape,
            Shape::Rect(Rect::new(11, 10, 20, 15))
        );
        // Shift steps 10.
        app.shift_down = true;
        app.nudge(0, 1);
        assert_eq!(
            app.selections.items()[0].shape,
            Shape::Rect(Rect::new(11, 20, 20, 15))
        );
        app.shift_down = false;
        // The 10px jump moved the shape off the cursor — nudging targets
        // the shape under the cursor, so follow it before resizing.
        app.cursor = Point::new(15, 25);
        // Alt resizes instead of moving.
        app.alt_down = true;
        app.nudge(1, 0);
        assert_eq!(
            app.selections.items()[0].shape,
            Shape::Rect(Rect::new(11, 20, 21, 15))
        );
        app.alt_down = false;
        // The frame is 100x60: a nudge that would leave it is refused
        // whole, not clamped partway.
        app.shift_down = true;
        for _ in 0..10 {
            app.nudge(1, 0);
        }
        let shape = app.selections.items()[0].shape.clone();
        let bb = shape.bbox();
        assert!(bb.x + bb.w <= 100, "never pushed past the edge: {bb:?}");
        // Every effective nudge recorded undo history.
        assert!(app.selections.undo());
    }

    #[test]
    fn nudge_without_a_shape_under_the_cursor_is_inert() {
        let mut app = test_app();
        app.selections
            .add(Selection::new(Shape::Rect(Rect::new(10, 10, 20, 15)), 0));
        app.cursor = Point::new(90, 50);
        app.nudge(1, 0);
        assert_eq!(
            app.selections.items()[0].shape,
            Shape::Rect(Rect::new(10, 10, 20, 15))
        );
        assert!(!app.dirty);
    }

    #[test]
    fn committing_a_session_name_dirties_and_empty_clears() {
        let mut app = test_app();
        app.mode = Mode::SessionNaming {
            text: "  microsoft teams  ".into(),
        };
        app.commit_session_name();
        assert_eq!(app.session_meta.name.as_deref(), Some("microsoft teams"));
        assert!(app.dirty, "a name change needs a save to persist");

        app.dirty = false;
        app.mode = Mode::SessionNaming { text: "   ".into() };
        app.commit_session_name();
        assert_eq!(app.session_meta.name, None, "whitespace clears");
        assert!(app.dirty);

        // Committing the same name again is a no-op.
        app.dirty = false;
        app.mode = Mode::SessionNaming {
            text: String::new(),
        };
        app.commit_session_name();
        assert!(!app.dirty);
    }

    #[test]
    fn restore_panel_ignores_a_frame_this_capture_lacks() {
        let mut app = test_app();
        app.restore_panel(Some((0, Point::new(40, 30))));
        assert_eq!(app.panel_origin, Some((0, Point::new(40, 30))));
        app.restore_panel(Some((7, Point::new(40, 30))));
        assert_eq!(app.panel_origin, None, "a stale frame index is dropped");
    }

    fn test_app() -> App {
        let info = MonitorInfo {
            index: 0,
            name: "Fake".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        let frame = MonitorFrame::new(
            info,
            RgbaImage::from_pixel(100, 60, image::Rgba([9, 9, 9, 255])),
        );
        App::new(
            vec![frame],
            Config::default().resolve_style().unwrap(),
            default_bindings(),
            std::env::temp_dir().join("pixelcoords-app-test-unused"),
            None,
            Presentation::Fullscreen,
        )
    }

    /// One fake monitor showing a light button on a dark field, so edge
    /// detection has something real to find. The button spans x 20..60
    /// and y 15..45, making its boundaries 20/60 and 15/45.
    fn test_app_with_button() -> App {
        let info = MonitorInfo {
            index: 0,
            name: "Fake".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        let mut rgba = RgbaImage::from_pixel(100, 60, image::Rgba([20, 20, 20, 255]));
        for y in 15..45 {
            for x in 20..60 {
                rgba.put_pixel(x, y, image::Rgba([230, 230, 230, 255]));
            }
        }
        let mut app = App::new(
            vec![MonitorFrame::new(info, rgba)],
            Config::default().resolve_style().unwrap(),
            default_bindings(),
            std::env::temp_dir().join("pixelcoords-app-test-unused"),
            None,
            Presentation::Fullscreen,
        );
        app.set_snap(Config::default().resolve_snap().unwrap());
        app
    }

    /// Three frames, so a release from the middle has both a frame below
    /// it (index unchanged) and one above it (index must shift down).
    fn test_app_multi() -> App {
        let frames: Vec<MonitorFrame> = (0..3)
            .map(|index| {
                let info = MonitorInfo {
                    index,
                    name: format!("Fake {index}"),
                    primary: index == 0,
                    origin: Point::new(index as i32 * 100, 0),
                    size_native: Size::new(100, 60),
                    scale: 1.0,
                };
                MonitorFrame::new(
                    info,
                    RgbaImage::from_pixel(100, 60, image::Rgba([9, 9, 9, 255])),
                )
            })
            .collect();
        App::new(
            frames,
            Config::default().resolve_style().unwrap(),
            default_bindings(),
            std::env::temp_dir().join("pixelcoords-app-test-unused"),
            None,
            Presentation::Fullscreen,
        )
    }

    #[test]
    fn releasing_a_frame_shifts_every_position_above_it_down() {
        let mut app = test_app_multi();
        // Positions into `frames` that must survive a removal below them.
        app.cursor_frame = 2;
        app.panel_origin = Some((2, Point::new(5, 5)));

        assert_eq!(app.try_release_frame(1), Release::Done);

        assert_eq!(app.frames.len(), 2);
        assert_eq!(
            app.frames.iter().map(|f| f.info.index).collect::<Vec<_>>(),
            vec![0, 2],
            "the surviving frames keep their monitor indices"
        );
        assert_eq!(app.cursor_frame, 1, "was 2, the frame below it went");
        assert_eq!(app.panel_origin, Some((1, Point::new(5, 5))));
    }

    #[test]
    fn releasing_a_frame_leaves_positions_below_it_alone() {
        let mut app = test_app_multi();
        app.cursor_frame = 0;
        app.panel_origin = Some((0, Point::new(5, 5)));

        assert_eq!(app.try_release_frame(2), Release::Done);

        assert_eq!(app.cursor_frame, 0);
        assert_eq!(app.panel_origin, Some((0, Point::new(5, 5))));
    }

    #[test]
    fn releasing_the_panels_own_frame_sends_it_back_to_the_default_corner() {
        let mut app = test_app_multi();
        app.panel_origin = Some((1, Point::new(5, 5)));
        app.cursor_frame = 1;

        assert_eq!(app.try_release_frame(1), Release::Done);

        assert_eq!(app.panel_origin, None, "its host is gone");
        assert_eq!(app.cursor_frame, 0, "the cursor's frame went with it");
    }

    #[test]
    fn a_frame_holding_marks_refuses_release_and_says_how_many() {
        let mut app = test_app_multi();
        // Two marks on the monitor behind frame 1.
        for _ in 0..2 {
            app.selections
                .add(Selection::new(rect(1, 1, 5, 5), app.frames[1].info.index));
        }

        assert_eq!(app.try_release_frame(1), Release::Blocked);

        assert_eq!(app.frames.len(), 3, "nothing was released");
        let flash = app.flash.as_ref().expect("a flash").0.clone();
        assert!(flash.contains('2'), "got: {flash}");
    }

    #[test]
    fn a_frame_holding_only_a_ruler_still_refuses_release() {
        let mut app = test_app_multi();
        app.selections.add_measure(Measure::new(
            Line::new(Point::new(1, 1), Point::new(20, 20)),
            app.frames[1].info.index,
        ));

        assert_eq!(app.try_release_frame(1), Release::Blocked);
        assert_eq!(app.frames.len(), 3, "nothing was released");
    }

    #[test]
    fn releasing_a_frame_keeps_the_rulers_on_the_others() {
        let mut app = test_app_multi();
        let kept = Measure::new(
            Line::new(Point::new(1, 1), Point::new(20, 20)),
            app.frames[0].info.index,
        );
        app.selections.add_measure(kept.clone());

        assert_eq!(app.try_release_frame(1), Release::Done);

        assert_eq!(
            app.selections.measures(),
            [kept],
            "reseeding must not drop what the released display never held"
        );
    }

    #[test]
    fn the_last_frame_quits_instead_of_releasing() {
        let mut app = test_app();
        assert_eq!(app.try_release_frame(0), Release::Quit);
        assert_eq!(app.frames.len(), 1, "still frozen until the run ends");
    }

    #[test]
    fn a_release_mid_gesture_quits_rather_than_pulling_the_frame_out() {
        let mut app = test_app_multi();
        app.mode = Mode::Drawing {
            frame: 2,
            start: Point::new(1, 1),
            path: vec![Point::new(1, 1)],
        };
        assert_eq!(app.try_release_frame(1), Release::Quit);
        assert_eq!(app.frames.len(), 3);
    }

    #[test]
    fn releasing_truncates_undo_history_but_keeps_the_marks() {
        let mut app = test_app_multi();
        // A mark on a frame that is NOT the one being released, so the
        // release is allowed and the mark must survive it.
        app.selections
            .add(Selection::new(rect(1, 1, 5, 5), app.frames[0].info.index));
        assert!(app.selections.undo(), "history exists before the release");
        app.selections
            .add(Selection::new(rect(2, 2, 5, 5), app.frames[0].info.index));

        assert_eq!(app.try_release_frame(2), Release::Done);

        assert_eq!(app.selections.len(), 1, "the mark survived");
        assert!(
            !app.selections.undo(),
            "history is truncated — entries could reference a gone frame"
        );
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Shape {
        Shape::Rect(pixelcoords_core::geometry::Rect::new(x, y, w, h))
    }

    #[test]
    fn a_drag_near_a_button_snaps_both_corners_onto_its_edges() {
        let mut app = test_app_with_button();
        // Start and end a few pixels off on every axis.
        app.cursor = Point::new(23, 18);
        app.mouse_pressed();
        app.cursor = Point::new(57, 42);
        app.update_active_gesture();
        app.mouse_released();

        assert_eq!(
            app.selections.items()[0].shape,
            rect(20, 15, 40, 30),
            "the rect is the button's true size, not four pixels short"
        );
    }

    #[test]
    fn snapping_turned_off_leaves_the_drag_exactly_where_it_was() {
        let mut app = test_app_with_button();
        app.apply_local_action(Action::ToggleSnap);
        assert!(!app.snap.enabled);

        app.cursor = Point::new(23, 18);
        app.mouse_pressed();
        app.cursor = Point::new(57, 42);
        app.update_active_gesture();
        app.mouse_released();

        assert_eq!(app.selections.items()[0].shape, rect(23, 18, 34, 24));
    }

    #[test]
    fn the_toggle_flips_back_and_says_so() {
        let mut app = test_app_with_button();
        app.apply_local_action(Action::ToggleSnap);
        assert_eq!(
            app.flash.as_ref().map(|(m, _)| m.as_str()),
            Some("SNAP OFF")
        );
        app.apply_local_action(Action::ToggleSnap);
        assert!(app.snap.enabled);
        assert_eq!(app.flash.as_ref().map(|(m, _)| m.as_str()), Some("SNAP ON"));
    }

    #[test]
    fn nudging_beside_an_edge_still_moves_exactly_one_pixel() {
        let mut app = test_app_with_button();
        // Parked one pixel inside the button's left boundary: a snap
        // would pull it back, and the arrow keys' whole contract is that
        // they do not.
        app.selections.add(Selection::new(rect(21, 30, 10, 6), 0));
        app.cursor = Point::new(25, 33);

        app.nudge(1, 0);

        assert_eq!(app.selections.items()[0].shape, rect(22, 30, 10, 6));
    }

    #[test]
    fn an_idle_cursor_never_snaps() {
        let mut app = test_app_with_button();
        app.cursor = Point::new(23, 18);
        app.update_active_gesture();
        assert_eq!(app.snap_hit, Snap::default(), "nothing is in flight");
    }

    #[test]
    fn a_measure_endpoint_snaps_to_the_edge_under_it() {
        let mut app = test_app_with_button();
        app.tool = ToolKind::Measure;
        app.cursor = Point::new(23, 30);
        app.mouse_pressed();
        app.cursor = Point::new(57, 30);
        app.update_active_gesture();
        app.mouse_released();

        assert_eq!(
            app.selections.measures()[0].line,
            Line::new(Point::new(20, 30), Point::new(60, 30)),
            "the ruler measures the button's real width"
        );
        assert!((app.selections.measures()[0].line.length() - 40.0).abs() < f64::EPSILON);
    }

    /// Build a keystroke the way `Keystroke::decode` would, without
    /// needing a winit event nobody outside winit can construct — which is
    /// the whole reason the decoded type exists.
    fn stroke(builtin: Option<Builtin>, binding: Option<KeyName>, pressed: bool) -> Keystroke {
        Keystroke {
            pressed,
            repeat: false,
            binding,
            builtin,
            text: None,
        }
    }

    fn typed(text: &str) -> Keystroke {
        Keystroke {
            pressed: true,
            repeat: false,
            binding: None,
            builtin: None,
            text: Some(text.to_string()),
        }
    }

    #[test]
    fn space_holds_the_panel_and_the_release_always_parks_it() {
        let mut app = test_app();
        assert_eq!(
            app.handle_key(&stroke(Some(Builtin::Space), None, true)),
            None
        );
        assert!(app.panel_held);
        app.handle_key(&stroke(Some(Builtin::Space), None, false));
        assert!(!app.panel_held);
    }

    #[test]
    fn a_space_release_parks_the_panel_even_while_the_label_editor_has_focus() {
        // The editor swallows key events; a release lost in there would
        // leave the panel glued to the cursor for the rest of the session.
        let mut app = test_app();
        app.handle_key(&stroke(Some(Builtin::Space), None, true));
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: String::new(),
        };

        app.handle_key(&stroke(Some(Builtin::Space), None, false));

        assert!(!app.panel_held);
        assert!(
            matches!(app.mode, Mode::LabelEditing { .. }),
            "still editing"
        );
    }

    #[test]
    fn m_holds_the_loupe() {
        let mut app = test_app();
        app.handle_key(&stroke(Some(Builtin::Loupe), None, true));
        assert!(app.loupe_held);
        app.handle_key(&stroke(Some(Builtin::Loupe), None, false));
        assert!(!app.loupe_held);
    }

    #[test]
    fn escape_cancels_a_gesture_and_restores_the_shape() {
        let mut app = test_app();
        let original = rect(10, 10, 30, 30);
        app.selections.add(Selection::new(original.clone(), 0));
        app.mode = Mode::Dragging {
            frame: 0,
            index: 0,
            grab_offset: Point::new(1, 1),
            original: original.clone(),
        };
        app.selections.set_shape_live(0, rect(90, 90, 30, 30));

        let action = app.handle_key(&stroke(Some(Builtin::Escape), None, true));

        assert_eq!(action, None, "cancelling a gesture must not quit");
        assert!(matches!(app.mode, Mode::Idle));
        assert_eq!(app.selections.items()[0].shape, original);
    }

    #[test]
    fn escape_when_idle_asks_to_quit() {
        let mut app = test_app();
        assert_eq!(
            app.handle_key(&stroke(Some(Builtin::Escape), None, true)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn digits_size_the_polygon_tool_only_while_it_is_selected() {
        let mut app = test_app();
        app.tool = ToolKind::Rect;
        app.handle_key(&stroke(Some(Builtin::Digit(7)), None, true));
        assert_eq!(app.polygon_sides, 6, "the rect tool ignores digits");

        app.tool = ToolKind::Polygon;
        app.handle_key(&stroke(Some(Builtin::Digit(7)), None, true));
        assert_eq!(app.polygon_sides, 7);

        // Outside 3..=9 the digit is not a side count and falls through.
        app.handle_key(&stroke(Some(Builtin::Digit(1)), None, true));
        assert_eq!(app.polygon_sides, 7, "1 is not a polygon");
    }

    #[test]
    fn arrows_nudge_only_when_idle() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.cursor = Point::new(15, 15);

        app.handle_key(&stroke(Some(Builtin::Arrow { dx: 1, dy: 0 }), None, true));
        assert_eq!(app.selections.items()[0].shape, rect(11, 10, 20, 20));

        // Mid-gesture the arrow must not move anything.
        app.mode = Mode::Drawing {
            frame: 0,
            start: Point::new(50, 50),
            path: Vec::new(),
        };
        app.handle_key(&stroke(Some(Builtin::Arrow { dx: 1, dy: 0 }), None, true));
        assert_eq!(app.selections.items()[0].shape, rect(11, 10, 20, 20));
    }

    #[test]
    fn shift_turns_undo_into_redo() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        let z = || stroke(None, Some(KeyName::Character('Z')), true);

        app.handle_key(&z());
        assert!(app.selections.is_empty(), "undo removed the add");

        app.shift_down = true;
        app.handle_key(&z());
        assert_eq!(app.selections.len(), 1, "shift redid it");
    }

    #[test]
    fn a_binding_is_refused_mid_gesture_but_quit_still_works() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::Drawing {
            frame: 0,
            start: Point::new(50, 50),
            path: Vec::new(),
        };

        // D would delete out from under the gesture's held index.
        app.cursor = Point::new(15, 15);
        app.handle_key(&stroke(None, Some(KeyName::Character('D')), true));
        assert_eq!(app.selections.len(), 1, "delete was refused");
    }

    #[test]
    fn the_label_editor_types_backspaces_and_commits() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: String::new(),
        };

        app.handle_key(&typed("s"));
        app.handle_key(&typed("u"));
        app.handle_key(&typed("x"));
        app.handle_key(&stroke(Some(Builtin::Backspace), None, true));
        app.handle_key(&typed("b"));
        app.handle_key(&stroke(Some(Builtin::Enter), None, true));

        assert_eq!(app.selections.items()[0].label, "sub");
        assert!(matches!(app.mode, Mode::Idle));
    }

    #[test]
    fn the_label_editor_discards_on_escape() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: "half typed".into(),
        };

        app.handle_key(&stroke(Some(Builtin::Escape), None, true));

        assert!(matches!(app.mode, Mode::Idle));
        assert_eq!(app.selections.items()[0].label, "", "nothing was committed");
    }

    #[test]
    fn a_label_stops_at_the_length_cap() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: String::new(),
        };
        for _ in 0..MAX_LABEL_LEN + 20 {
            app.handle_key(&typed("x"));
        }
        let Mode::LabelEditing { text, .. } = &app.mode else {
            panic!("still editing")
        };
        assert_eq!(text.chars().count(), MAX_LABEL_LEN);
    }

    #[test]
    fn control_characters_never_reach_a_label() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: String::new(),
        };
        app.handle_key(&typed("a\u{7}b\u{1b}c"));
        let Mode::LabelEditing { text, .. } = &app.mode else {
            panic!("still editing")
        };
        assert_eq!(text, "abc");
    }

    #[test]
    fn the_session_name_editor_works_the_same_way() {
        let mut app = test_app();
        app.mode = Mode::SessionNaming {
            text: String::new(),
        };
        app.handle_key(&typed("r"));
        app.handle_key(&typed("u"));
        app.handle_key(&typed("n"));
        app.handle_key(&stroke(Some(Builtin::Enter), None, true));
        assert_eq!(app.session_meta.name.as_deref(), Some("run"));
    }

    #[test]
    fn an_edge_is_read_from_press_and_repeat() {
        let press = stroke(None, Some(KeyName::Character('Q')), true);
        assert_eq!(press.edge(), Edge::Press);
        let mut repeat = press.clone();
        repeat.repeat = true;
        assert_eq!(repeat.edge(), Edge::Repeat);
        let mut release = press;
        release.pressed = false;
        release.repeat = true;
        assert_eq!(release.edge(), Edge::Release, "a release is a release");
    }

    #[test]
    fn shift_circles_an_ellipse_preview_without_moving_the_cursor() {
        // The ellipse is the drawing tool that reads the modifier — the
        // rect preview ignores `lock` entirely, so this is the shape that
        // can demonstrate the bug.
        let mut app = test_app();
        app.tool = ToolKind::Ellipse;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(60, 30);
        app.update_active_gesture();
        let Some(Shape::Ellipse { rx, ry, .. }) = app.preview_for(0) else {
            panic!("drawing an ellipse")
        };
        assert_ne!(rx, ry, "an oval to start with");

        // Shift alone, no mouse movement at all.
        app.shift_changed(true);

        assert!(app.shift_down);
        let Some(Shape::Ellipse { rx, ry, .. }) = app.preview_for(0) else {
            panic!("still drawing")
        };
        assert_eq!(rx, ry, "the lock must apply before the next mouse move");
    }

    #[test]
    fn shift_snaps_a_measure_preview_without_moving_the_cursor() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(60, 12);
        app.update_active_gesture();
        assert_eq!(
            app.measure_preview(0),
            Some(Line::new(Point::new(10, 10), Point::new(60, 12)))
        );

        app.shift_changed(true);

        let snapped = app.measure_preview(0).expect("still drawing");
        assert_eq!(snapped.b.y, 10, "45-degree snap flattened it: {snapped:?}");
    }

    #[test]
    fn shift_reaims_a_dragged_measure_endpoint_without_moving_the_cursor() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        let line = Line::new(Point::new(10, 10), Point::new(60, 10));
        app.selections.add_measure(Measure::new(line, 0));

        // Grab endpoint b and drag it somewhere near-diagonal.
        app.cursor = Point::new(60, 10);
        app.mouse_pressed();
        app.cursor = Point::new(50, 48);
        app.update_active_gesture();
        let free = app.selections.measures()[0].line;
        assert_eq!(free.b, Point::new(50, 48));

        app.shift_changed(true);

        let snapped = app.selections.measures()[0].line;
        assert_ne!(snapped.b, free.b, "the endpoint re-aimed on its own");
        let (dx, dy) = snapped.delta();
        assert_eq!(dx.abs(), dy.abs(), "snapped to 45 degrees: {snapped:?}");
    }

    #[test]
    fn releasing_shift_undoes_the_constraint_just_as_readily() {
        let mut app = test_app();
        app.tool = ToolKind::Ellipse;
        app.shift_down = true;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(60, 30);
        app.update_active_gesture();
        let Some(Shape::Ellipse { rx, ry, .. }) = app.preview_for(0) else {
            panic!("drawing")
        };
        assert_eq!(rx, ry, "locked to a circle");

        app.shift_changed(false);

        let Some(Shape::Ellipse { rx, ry, .. }) = app.preview_for(0) else {
            panic!("still drawing")
        };
        assert_ne!(rx, ry, "an oval again, without the cursor moving");
    }

    #[test]
    fn shift_still_re_resizes_the_way_it_always_did() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 40, 20), 0));
        app.cursor = Point::new(50, 30);
        app.mouse_pressed();
        assert!(
            matches!(app.mode, Mode::Resizing { .. }),
            "grabbed a corner"
        );
        // Drag to a shape that breaks the original 2:1 ratio, so locked
        // and free cannot coincide.
        app.cursor = Point::new(70, 80);
        app.update_active_gesture();
        let free = app.selections.items()[0].shape.clone();

        app.shift_changed(true);

        assert_ne!(
            app.selections.items()[0].shape,
            free,
            "the ratio lock re-applied without the cursor moving"
        );
    }

    #[test]
    fn shift_while_idle_or_moving_changes_only_the_flag() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 40, 30), 0));
        let before = app.selections.items()[0].shape.clone();

        app.shift_changed(true);
        assert!(app.shift_down);
        assert_eq!(app.selections.items()[0].shape, before, "idle is untouched");

        // A whole-shape move does not read the modifier, so it must not
        // twitch when Shift is pressed mid-drag. Grab well inside, or the
        // grab tolerance turns this into a resize.
        app.cursor = Point::new(30, 20);
        app.mouse_pressed();
        assert!(matches!(app.mode, Mode::Dragging { .. }));
        app.cursor = Point::new(38, 28);
        app.update_active_gesture();
        let moved = app.selections.items()[0].shape.clone();

        app.shift_changed(false);

        assert_eq!(
            app.selections.items()[0].shape,
            moved,
            "the move is unaffected"
        );
    }

    #[test]
    fn repeating_the_same_shift_state_does_nothing() {
        let mut app = test_app();
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(60, 30);
        app.update_active_gesture();
        let before = app.preview_for(0);

        app.shift_changed(false);

        assert_eq!(app.preview_for(0), before);
    }

    #[test]
    fn hitting_the_label_cap_says_so_instead_of_swallowing_the_key() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 20, 20), 0));
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: "x".repeat(MAX_LABEL_LEN - 1),
        };

        app.handle_key(&typed("y"));
        assert!(
            app.flash.is_none(),
            "the last character fits, so no message"
        );

        app.handle_key(&typed("z"));
        let flash = app.flash.as_ref().map(|(m, _)| m.clone());
        assert!(
            flash.as_deref().is_some_and(|m| m.contains("64")),
            "the refusal must name the limit, got {flash:?}"
        );
    }

    #[test]
    fn append_typed_reports_whether_it_refused_anything() {
        let mut text = String::new();
        assert!(!append_typed(&mut text, Some("fits")));
        let mut full = "x".repeat(MAX_LABEL_LEN);
        assert!(append_typed(&mut full, Some("more")));
        assert_eq!(full.chars().count(), MAX_LABEL_LEN);
        // Control characters are dropped, but dropping them is not a
        // refusal — nothing was turned away for being too long.
        let mut text = String::new();
        assert!(!append_typed(&mut text, Some("a\u{7}b")));
        assert_eq!(text, "ab");
    }

    #[test]
    fn draw_gesture_commits_and_dirties() {
        let mut app = test_app();
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        assert!(matches!(app.mode, Mode::Drawing { .. }));
        app.cursor = Point::new(40, 40);
        app.mouse_released();
        assert!(matches!(app.mode, Mode::Idle));
        assert_eq!(app.selections.len(), 1);
        assert_eq!(app.selections.items()[0].shape, rect(10, 10, 30, 30));
        assert!(app.dirty);
    }

    #[test]
    fn measure_gesture_commits_a_ruler_not_a_shape() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        assert!(matches!(app.mode, Mode::Drawing { .. }));
        app.cursor = Point::new(40, 50);
        app.update_active_gesture();
        assert_eq!(
            app.measure_preview(0),
            Some(Line::new(Point::new(10, 10), Point::new(40, 50)))
        );
        app.mouse_released();

        assert!(app.selections.is_empty(), "a ruler is not a selection");
        assert_eq!(app.selections.measures().len(), 1);
        assert_eq!(
            app.selections.measures()[0].line,
            Line::new(Point::new(10, 10), Point::new(40, 50))
        );
        assert!(app.dirty);
    }

    #[test]
    fn a_measure_click_without_a_drag_commits_nothing() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(12, 11); // shorter than the grab tolerance
        app.mouse_released();
        assert!(app.selections.measures().is_empty());
        assert!(!app.dirty);
    }

    #[test]
    fn shift_constrains_a_measure_to_45_degrees() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        app.shift_down = true;
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(50, 12); // nearly horizontal
        app.mouse_released();
        let line = app.selections.measures()[0].line;
        assert_eq!(line.b.y, 10, "snapped flat: {line:?}");
    }

    #[test]
    fn dragging_a_measure_endpoint_moves_only_that_end() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        let line = Line::new(Point::new(10, 10), Point::new(50, 10));
        app.selections.add_measure(Measure::new(line, 0));
        app.dirty = false;

        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        assert!(matches!(
            app.mode,
            Mode::MeasureDragging {
                grab: MeasureGrab::Endpoint(true),
                ..
            }
        ));
        app.cursor = Point::new(10, 40);
        app.update_active_gesture();
        app.mouse_released();

        let moved = app.selections.measures()[0].line;
        assert_eq!(moved, Line::new(Point::new(10, 40), Point::new(50, 10)));
        assert!(app.dirty);
    }

    #[test]
    fn dragging_a_measures_body_translates_it_whole() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        let line = Line::new(Point::new(10, 10), Point::new(50, 10));
        app.selections.add_measure(Measure::new(line, 0));

        app.cursor = Point::new(30, 10);
        app.mouse_pressed();
        assert!(matches!(
            app.mode,
            Mode::MeasureDragging {
                grab: MeasureGrab::Move,
                ..
            }
        ));
        app.cursor = Point::new(35, 20);
        app.update_active_gesture();
        app.mouse_released();

        assert_eq!(
            app.selections.measures()[0].line,
            Line::new(Point::new(15, 20), Point::new(55, 20)),
            "length and angle survive a move"
        );
    }

    #[test]
    fn measure_keys_reach_rulers_only_while_the_measure_tool_is_active() {
        let mut app = test_app();
        app.selections.add_measure(Measure::new(
            Line::new(Point::new(10, 10), Point::new(50, 10)),
            0,
        ));
        app.cursor = Point::new(30, 10);

        // Rect tool: the ruler is inert, so D and A belong to shapes.
        assert_eq!(app.measure_at_cursor(), None);
        app.tool = ToolKind::Measure;
        assert_eq!(app.measure_at_cursor(), Some(0));
    }

    #[test]
    fn deleting_and_labeling_at_the_cursor_reach_the_measure_under_it() {
        let mut app = test_app();
        app.tool = ToolKind::Measure;
        app.selections.add_measure(Measure::new(
            Line::new(Point::new(10, 10), Point::new(50, 10)),
            0,
        ));
        app.cursor = Point::new(30, 10);

        app.apply_local_action(Action::LabelEditAtCursor);
        assert!(matches!(
            app.mode,
            Mode::LabelEditing {
                target: EditTarget::Measure,
                index: 0,
                ..
            }
        ));
        let Mode::LabelEditing { text, .. } = &mut app.mode else {
            unreachable!()
        };
        text.push_str("gap");
        app.commit_label();
        assert_eq!(app.selections.measures()[0].label, "gap");

        app.apply_local_action(Action::DeleteAtCursor);
        assert!(app.selections.measures().is_empty());
    }

    #[test]
    fn sub_threshold_draw_commits_nothing_and_stays_clean() {
        let mut app = test_app();
        app.cursor = Point::new(10, 10);
        app.mouse_pressed();
        app.cursor = Point::new(11, 11); // 1x1: below the commit threshold
        app.mouse_released();
        assert!(app.selections.is_empty());
        assert!(!app.dirty);
    }

    #[test]
    fn click_inside_shape_without_motion_stays_clean() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.dirty = false; // as after a save
        app.cursor = Point::new(20, 20);
        app.mouse_pressed();
        assert!(matches!(app.mode, Mode::Dragging { .. }));
        app.mouse_released();
        assert!(!app.dirty, "no-op move must not dirty the session");
        // Undo stack holds only the add.
        assert!(app.selections.undo());
        assert!(app.selections.is_empty());
    }

    #[test]
    fn real_move_dirties_and_disarms_quit() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.dirty = false;
        app.quit_armed_until = Some(Instant::now() + Duration::from_secs(60));
        app.cursor = Point::new(20, 20);
        app.mouse_pressed();
        // Simulate the cursor_moved drag arm's effect.
        app.selections.set_shape_live(0, rect(30, 10, 30, 30));
        app.mouse_released();
        assert!(app.dirty);
        assert!(
            app.quit_armed_until.is_none(),
            "new work must re-arm the quit warning"
        );
    }

    #[test]
    fn border_press_starts_a_resize() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.cursor = Point::new(10, 25); // on the left edge
        app.mouse_pressed();
        assert!(matches!(app.mode, Mode::Resizing { .. }));
    }

    #[test]
    fn typed_text_stops_at_the_label_cap() {
        let mut text = "x".repeat(MAX_LABEL_LEN - 1);
        append_typed(&mut text, Some("ab"));
        assert_eq!(text.chars().count(), MAX_LABEL_LEN);
        append_typed(&mut text, Some("cd"));
        assert_eq!(text.chars().count(), MAX_LABEL_LEN, "cap is not exceeded");
    }

    #[test]
    fn typed_text_drops_control_characters() {
        let mut text = String::new();
        append_typed(&mut text, Some("a\tb\nc"));
        assert_eq!(text, "abc");
    }

    #[test]
    fn no_typed_text_is_a_noop() {
        let mut text = "kept".to_string();
        append_typed(&mut text, None);
        assert_eq!(text, "kept");
    }

    #[test]
    fn label_commit_dirties_only_on_change() {
        let mut app = test_app();
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.dirty = false;
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: "target".into(),
        };
        app.commit_label();
        assert_eq!(app.selections.items()[0].label, "target");
        assert!(app.dirty);

        app.dirty = false;
        app.mode = Mode::LabelEditing {
            target: EditTarget::Selection,
            index: 0,
            text: "target".into(),
        };
        app.commit_label();
        assert!(!app.dirty, "unchanged label must not dirty the session");
    }

    #[test]
    fn save_writes_files_resets_dirty_and_retires_crops_on_resave() {
        let dir = std::env::temp_dir().join("pixelcoords-app-test-save");
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = test_app();
        app.out_dir.clone_from(&dir);
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.selections.add(Selection::new(rect(50, 10, 20, 20), 0));
        app.dirty = true;

        app.save();
        assert!(dir.join("session.json").exists());
        assert!(dir.join("screenshot-0.png").exists());
        assert!(dir.join("crop-0.png").exists());
        assert!(dir.join("crop-1.png").exists());
        assert!(!app.dirty);
        assert!(app.saved_once);
        assert_eq!(app.last_crops.len(), 2);

        // Delete one and re-save: the orphaned crop is retired end-to-end.
        app.selections.delete(1);
        app.save();
        assert!(dir.join("crop-0.png").exists());
        assert!(!dir.join("crop-1.png").exists());
        assert_eq!(app.last_crops.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hit_and_preview_respect_state() {
        let mut app = test_app();
        assert_eq!(app.hit_at_cursor(), None);
        assert_eq!(app.preview_for(0), None);
        app.selections.add(Selection::new(rect(10, 10, 30, 30), 0));
        app.cursor = Point::new(20, 20);
        assert_eq!(app.hit_at_cursor(), Some(0));
        assert!(app.overlay_state().has_selection);
        assert!(app.overlay_state().cursor_in_shape);

        app.mode = Mode::Drawing {
            frame: 0,
            start: Point::new(50, 40),
            path: vec![Point::new(50, 40)],
        };
        app.cursor = Point::new(70, 55);
        app.update_active_gesture();
        assert_eq!(app.preview_for(0), Some(rect(50, 40, 20, 15)));
        assert_eq!(app.preview_for(1), None, "preview is frame-local");
    }

    #[test]
    fn a_freehand_release_commits_the_simplified_stroke() {
        let mut app = test_app();
        app.tool = ToolKind::Freehand;
        // A noisy diagonal stroke plus a corner.
        let path: Vec<Point> = (0..20)
            .map(|i| Point::new(i * 2, i * 2 + (i % 2)))
            .chain((0..10).map(|i| Point::new(40 + i * 2, 40)))
            .collect();
        app.mode = Mode::Drawing {
            frame: 0,
            start: path[0],
            path,
        };
        app.mouse_released();
        assert_eq!(app.selections.len(), 1, "the stroke committed");
        let Shape::Poly { points } = &app.selections.items()[0].shape else {
            panic!("freehand commits a poly");
        };
        assert!(
            points.len() <= 6,
            "jitter simplified away, got {} points",
            points.len()
        );
        assert!(app.dirty);
    }

    #[test]
    fn a_polygon_drag_commits_a_regular_ngon_with_the_chosen_sides() {
        let mut app = test_app();
        app.tool = ToolKind::Polygon;
        app.polygon_sides = 5;
        app.mode = Mode::Drawing {
            frame: 0,
            start: Point::new(50, 30),
            path: vec![Point::new(50, 30)],
        };
        app.cursor = Point::new(70, 30);
        app.mouse_released();
        assert_eq!(app.selections.len(), 1);
        let Shape::Poly { points } = &app.selections.items()[0].shape else {
            panic!("polygon commits a poly");
        };
        assert_eq!(points.len(), 5);
        assert_eq!(points[0], Point::new(70, 30));
    }

    #[test]
    fn committable_threshold_discards_tiny_shapes() {
        assert!(!shape_is_committable(&Shape::Rect(
            pixelcoords_core::geometry::Rect::new(0, 0, 1, 5)
        )));
        assert!(shape_is_committable(&Shape::Rect(
            pixelcoords_core::geometry::Rect::new(0, 0, 2, 2)
        )));
        assert!(!shape_is_committable(&Shape::Circle { cx: 0, cy: 0, r: 1 }));
        assert!(shape_is_committable(&Shape::Circle { cx: 0, cy: 0, r: 2 }));
    }

    #[test]
    fn rgba_conversion_packs_0rgb() {
        let img = RgbaImage::from_pixel(1, 1, image::Rgba([0x12, 0x34, 0x56, 0xFF]));
        assert_eq!(rgba_to_0rgb(&img), vec![0x0012_3456]);
    }

    #[test]
    fn resize_icons_match_handle_orientation() {
        let rect = Shape::Rect(pixelcoords_core::geometry::Rect::new(0, 0, 100, 100));
        let corner = ResizeHandle::RectEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        assert_eq!(
            resize_icon(corner, &rect, Point::new(100, 100)),
            CursorIcon::NwseResize
        );
        let edge = ResizeHandle::RectEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        assert_eq!(
            resize_icon(edge, &rect, Point::new(0, 50)),
            CursorIcon::EwResize
        );
        let circle = Shape::Circle {
            cx: 50,
            cy: 50,
            r: 30,
        };
        assert_eq!(
            resize_icon(ResizeHandle::CircleRadius, &circle, Point::new(50, 80)),
            CursorIcon::NsResize
        );
        assert_eq!(
            resize_icon(ResizeHandle::CircleRadius, &circle, Point::new(80, 50)),
            CursorIcon::EwResize
        );
    }

    #[test]
    fn mid_gesture_gate_allows_only_quit() {
        assert!(allowed_mid_gesture(Action::Quit));
        for action in [
            Action::Save,
            Action::NextTool,
            Action::DeleteAtCursor,
            Action::LabelEditAtCursor,
            Action::Undo,
            Action::RotateCcw,
            Action::RotateCw,
            Action::NextTheme,
        ] {
            assert!(!allowed_mid_gesture(action), "{action:?} must be gated");
        }
    }

    #[test]
    fn identical_monitors_pair_by_position() {
        use winit::dpi::{PhysicalPosition, PhysicalSize};
        let size = PhysicalSize::new(3024, 1964);
        let candidates = [
            (size, PhysicalPosition::new(3024, 0)), // right display
            (size, PhysicalPosition::new(0, 0)),    // left display
        ];
        // Frame captured at logical origin (1512, 0) scale 2 => physical
        // (3024, 0): must pick the right display, not the first size match.
        assert_eq!(pick_monitor(&candidates, size, (3024.0, 0.0)), Some(0));
        assert_eq!(pick_monitor(&candidates, size, (0.0, 0.0)), Some(1));
        // No size match at all.
        assert_eq!(
            pick_monitor(&candidates, PhysicalSize::new(800, 600), (0.0, 0.0)),
            None
        );
    }

    #[test]
    fn monitor_frame_derives_scale_and_size() {
        let info = MonitorInfo {
            index: 0,
            name: "Test".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(50, 30),
            scale: 2.0,
        };
        let frame = MonitorFrame::new(info, RgbaImage::new(100, 60));
        assert_eq!(frame.size, Size::new(100, 60));
        assert_eq!(frame.ui_scale, 2);
    }
}
