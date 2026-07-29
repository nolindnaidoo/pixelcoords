//! The winit application: one flat state struct, no generics. Input events
//! map to core actions through the tested hotkey grammar; all geometry
//! happens in capture space via the core crate. One overlay window per
//! captured monitor; selections are tagged with their monitor index.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use image::RgbaImage;
use pixelcoords_core::config::Style;
use pixelcoords_core::geometry::{Point, Rect, ResizeHandle, Shape, Size, ToolKind};
use pixelcoords_core::hotkeys::{Action, Binding, Edge, KeyName, OverlayState, match_event};
use pixelcoords_core::selection::{GrabKind, Selection, SelectionSet};
use pixelcoords_core::session::TargetRecord;
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
const MAX_LABEL_LEN: usize = 64;
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
        }
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
    LabelEditing {
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
        }
    }

    /// Set the provenance stamped into every save.
    pub fn set_session_meta(&mut self, meta: crate::save::SessionMeta) {
        self.session_meta = meta;
    }

    /// Adopt a saved session's state for resumed editing: its selections
    /// (seeded without undo history — the resume point is the floor), and
    /// when saving back in place, the crops it wrote plus resave
    /// semantics, so the next save skips, retires, and re-encodes exactly
    /// as if the session never closed. A diverted `--out` starts fresh:
    /// screenshots must be written to the new directory.
    pub fn restore_session(
        &mut self,
        selections: Vec<Selection>,
        previous: Vec<crate::save::WrittenCrop>,
        in_place: bool,
    ) {
        self.selections = SelectionSet::seed(selections);
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
                    self.cursor.x.clamp(region.x, region.x + region.w - 1),
                    self.cursor.y.clamp(region.y, region.y + region.h - 1),
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
                self.cursor,
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
            Mode::LabelEditing { index, text } => Some((*index, text.as_str(), self.caret_visible)),
            _ => None,
        };
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
        if let Mode::LabelEditing { index, text } = std::mem::replace(&mut self.mode, Mode::Idle)
            && self.selections.set_label(index, text)
        {
            self.mark_dirty();
        }
        self.caret_deadline = None;
    }

    fn grab_tolerance(&self) -> i32 {
        GRAB_TOLERANCE * self.frames[self.cursor_frame].ui_scale
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
                    start: self.cursor,
                    path: vec![self.cursor],
                };
            }
        }
        self.redraw_all();
    }

    fn mouse_released(&mut self) {
        let preview = self.preview_for(self.cursor_frame);
        match std::mem::replace(&mut self.mode, Mode::Idle) {
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
                    self.cursor,
                    self.frames[frame].size,
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
                    self.cursor,
                    self.frames[frame].size,
                    self.shift_down,
                );
                self.selections.set_shape_live(index, resized);
                self.redraw_frame(frame);
            }
            Mode::Idle => self.update_hover_cursor(),
            Mode::LabelEditing { .. } | Mode::SessionNaming { .. } => {}
        }
    }

    /// Cursor-icon feedback while idle: resize arrows on borders, a move
    /// cursor inside shapes, crosshair elsewhere.
    fn update_hover_cursor(&mut self) {
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
        if icon != self.cursor_icon {
            self.cursor_icon = icon;
            if let Some(slot) = self.views.iter().find(|s| s.frame == self.cursor_frame) {
                slot.view.set_cursor(icon);
            }
        }
    }

    fn key_event(&mut self, event_loop: &ActiveEventLoop, event: &KeyEvent) {
        // A Space release always parks the panel, whatever mode we are in —
        // the label editor eats key events, and a release swallowed there
        // would leave the panel glued to the cursor.
        if event.state == ElementState::Released && event.logical_key == Key::Named(NamedKey::Space)
        {
            self.panel_held = false;
        }
        if matches!(self.mode, Mode::LabelEditing { .. }) {
            if event.state == ElementState::Pressed {
                self.label_editor_key(event);
            }
            return;
        }
        if matches!(self.mode, Mode::SessionNaming { .. }) {
            if event.state == ElementState::Pressed {
                self.name_editor_key(event);
            }
            return;
        }

        // Esc cancels whatever is in progress; with nothing in progress it
        // asks to quit (twice within the grace period when work is
        // unsaved) — the same double-tap guard the old Q binding had.
        if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape)
        {
            match std::mem::replace(&mut self.mode, Mode::Idle) {
                Mode::Dragging {
                    index, original, ..
                }
                | Mode::Resizing {
                    index, original, ..
                } => {
                    self.selections.set_shape_live(index, original);
                }
                Mode::Idle => self.request_quit(event_loop),
                _ => {}
            }
            self.redraw_all();
            return;
        }

        // M is a hold like Space: the loupe lives while it is down.
        if let Key::Character(s) = &event.logical_key
            && s.eq_ignore_ascii_case("m")
        {
            self.loupe_held = event.state == ElementState::Pressed;
            self.redraw_frame(self.cursor_frame);
            return;
        }

        // Space is a hold, not a binding: while it is down the control
        // panel rides the cursor; the release is handled above.
        if event.logical_key == Key::Named(NamedKey::Space) {
            if event.state == ElementState::Pressed {
                self.panel_held = true;
            }
            return;
        }

        // Number keys size the polygon tool: 3 to 9 sides.
        if self.tool == ToolKind::Polygon
            && event.state == ElementState::Pressed
            && let Key::Character(text) = &event.logical_key
            && let Some(d) = text.chars().next().and_then(|c| c.to_digit(10))
            && (3..=9).contains(&d)
        {
            self.polygon_sides = d;
            self.set_flash(format!("Polygon sides: {d}"), FLASH_TOOL);
            return;
        }

        // Arrows nudge the shape under the cursor — 1px, 10px with Shift,
        // Alt resizes instead. Built in like Esc and Space (arrows are
        // named keys the binding grammar does not cover); holding the key
        // repeats.
        if let Some((dx, dy)) = arrow_delta(&event.logical_key) {
            if event.state == ElementState::Pressed && matches!(self.mode, Mode::Idle) {
                self.nudge(dx, dy);
            }
            return;
        }

        let Some(key) = key_name(event) else { return };
        let edge = match (event.state, event.repeat) {
            (ElementState::Pressed, false) => Edge::Press,
            (ElementState::Pressed, true) => Edge::Repeat,
            (ElementState::Released, _) => Edge::Release,
        };
        if let Some(action) = match_event(&self.bindings, key, edge, self.overlay_state()) {
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
                return;
            }
            self.apply_action(event_loop, action);
        }
    }

    fn label_editor_key(&mut self, event: &KeyEvent) {
        let Mode::LabelEditing { text, .. } = &mut self.mode else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => self.commit_label(),
            Key::Named(NamedKey::Escape) => {
                self.mode = Mode::Idle;
                self.caret_deadline = None;
            }
            Key::Named(NamedKey::Backspace) => {
                text.pop();
            }
            _ => append_typed(text, event.text.as_deref()),
        }
        self.caret_visible = true;
        self.caret_deadline = Some(Instant::now() + CARET_BLINK);
        self.redraw_all();
    }

    fn name_editor_key(&mut self, event: &KeyEvent) {
        let Mode::SessionNaming { text } = &mut self.mode else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Enter) => self.commit_session_name(),
            Key::Named(NamedKey::Escape) => {
                self.mode = Mode::Idle;
                self.caret_deadline = None;
            }
            Key::Named(NamedKey::Backspace) => {
                text.pop();
            }
            _ => append_typed(text, event.text.as_deref()),
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
        let Mode::Resizing {
            frame,
            index,
            handle,
            original,
        } = &self.mode
        else {
            return;
        };
        let (frame, index, handle, original) = (*frame, *index, *handle, original.clone());
        let Some(rot) = self.selections.get(index).map(|s| s.rot_deg) else {
            self.mode = Mode::Idle;
            return;
        };
        let resized =
            original.resize_to_rotated(rot, handle, self.cursor, self.frames[frame].size, shift);
        self.selections.set_shape_live(index, resized);
        self.redraw_frame(frame);
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
                };
                self.set_flash(name.to_string(), FLASH_TOOL);
            }
            Action::DeleteAtCursor => {
                if let Some(index) = self.hit_at_cursor() {
                    self.selections.delete(index);
                    self.mark_dirty();
                    self.redraw_all();
                }
            }
            Action::LabelEditAtCursor => {
                if let Some(index) = self.hit_at_cursor() {
                    let text = self.selections.items()[index].label.clone();
                    self.mode = Mode::LabelEditing { index, text };
                    self.caret_visible = true;
                    self.caret_deadline = Some(Instant::now() + CARET_BLINK);
                    self.redraw_all();
                }
            }
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
            WindowEvent::CloseRequested => self.request_quit(event_loop),
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
fn append_typed(text: &mut String, typed: Option<&str>) {
    let Some(typed) = typed else {
        return;
    };
    for c in typed.chars().filter(|c| !c.is_control()) {
        if text.chars().count() >= MAX_LABEL_LEN {
            return;
        }
        text.push(c);
    }
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

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Shape {
        Shape::Rect(pixelcoords_core::geometry::Rect::new(x, y, w, h))
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
            index: 0,
            text: "target".into(),
        };
        app.commit_label();
        assert_eq!(app.selections.items()[0].label, "target");
        assert!(app.dirty);

        app.dirty = false;
        app.mode = Mode::LabelEditing {
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
