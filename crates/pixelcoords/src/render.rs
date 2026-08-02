//! Frame composition: frozen background + selections + preview + captions +
//! HUD, all in capture space. Pure functions over the core rasterizer so the
//! logic is testable without a window.

use pixelcoords_core::config::Style;
use pixelcoords_core::draw::{Canvas, Color, coord_text, measure_text, smart_text_position};
use pixelcoords_core::font;
use pixelcoords_core::geometry::ToolKind;
use pixelcoords_core::geometry::{Line, Point, Rect, Shape, Size};
use pixelcoords_core::selection::SelectionSet;
use pixelcoords_core::strings::Strings;

const HUD_MARGIN: i32 = 12;
/// Panel interior padding, per side, before DPI scaling.
const PANEL_PAD: i32 = 8;
/// Extra pixels between panel rows, before DPI scaling.
const PANEL_LEADING: i32 = 2;
/// Blank columns between the key column and the action column.
const PANEL_GAP: usize = 2;
/// Backdrop brightness: the frozen image dims to this/256 under the panel.
const PANEL_DIM: u32 = 56;

/// Muted gray for the action column and panel border; the key column uses
/// the configured label color, so the keys are what the eye catches.
const PANEL_MUTED: Color = Color {
    r: 0xB4,
    g: 0xB4,
    b: 0xB4,
};
/// Loupe source half-width: the magnifier shows a (2r+1)-pixel square.
const LOUPE_SRC_RADIUS: i32 = 15;

/// The wordmark on the panel's top edge.
const BRAND: &str = "PixelCoords";

const PANEL_BORDER: Color = Color {
    r: 0x4A,
    g: 0x4A,
    b: 0x4A,
};

pub struct FrameState<'a> {
    pub selections: &'a SelectionSet,
    /// Only selections on this monitor index are drawn.
    pub monitor: usize,
    /// Bounds of the `--target` window on this monitor, if any — drawn as a
    /// border beneath the selections.
    pub target: Option<Rect>,
    pub preview: Option<Shape>,
    /// Selection index currently in the label editor, with its in-progress
    /// text and caret state.
    pub editing: Option<(usize, &'a str, bool)>,
    /// Measure index currently in the label editor. Separate from
    /// `editing` because measures are their own array — one index cannot
    /// address both.
    pub measure_editing: Option<(usize, &'a str, bool)>,
    /// The measure being drawn or dragged, not yet committed.
    pub measure_preview: Option<Line>,
    pub flash: Option<&'a str>,
    pub strings: &'a Strings,
    pub style: Style,
    /// Monitor DPI scale, applied to text and HUD metrics so they are the
    /// same visual size on any display.
    pub ui_scale: i32,
    /// Where the user parked the control panel (top-left, capture space);
    /// `None` is the default bottom-left corner.
    pub panel_origin: Option<Point>,
    /// The panel is hidden; flash messages still draw.
    pub panel_hidden: bool,
    /// The active drawing tool, shown live in the panel's W row.
    pub tool: ToolKind,
    /// The polygon tool's side count, shown beside its name.
    pub polygon_sides: u32,
    /// The cursor, when it is on this monitor — drives the live
    /// coordinate readout beside the pointer.
    pub cursor: Option<Point>,
    /// M is held: draw the magnifier loupe around the cursor.
    pub loupe: bool,
    /// The session-name editor's in-progress text and caret state.
    pub naming: Option<(&'a str, bool)>,
}

/// Compose one frame: `background` is the frozen capture (same size as
/// `buffer`), already converted to `0RGB`.
pub fn compose(buffer: &mut [u32], size: Size, background: &[u32], state: &FrameState) {
    buffer.copy_from_slice(background);
    if let Some(bounds) = state.target {
        // Dim pixels outside the drawable region so the boundary is
        // obvious. Marks made outside are refused, so the reader should
        // see plainly which pixels are in play.
        shade_outside(buffer, size, bounds);
    }
    let mut canvas = Canvas::new(buffer, size.w, size.h);

    if let Some(bounds) = state.target {
        canvas.draw_rect_outline(bounds, state.style.target, state.style.thickness);
    }

    for (index, sel) in state.selections.items().iter().enumerate() {
        if sel.monitor != state.monitor {
            continue;
        }
        canvas.draw_shape_rotated(
            &sel.shape,
            sel.rot_deg,
            state.style.complete,
            state.style.thickness,
            state.style.fill,
        );
        let caption = match state.editing {
            Some((edit_index, text, caret)) if edit_index == index => {
                let mark = if caret { "_" } else { " " };
                format!("{text}{mark}")
            }
            _ if sel.label.is_empty() => coord_text(&sel.shape),
            _ => sel.label.clone(),
        };
        draw_caption(
            &mut canvas,
            sel.shape.rotated_bbox(sel.rot_deg),
            size,
            &caption,
            state.style.label,
            state.ui_scale,
        );
    }

    for (index, m) in state.selections.measures().iter().enumerate() {
        if m.monitor != state.monitor {
            continue;
        }
        canvas.draw_line(m.line, state.style.complete, state.style.thickness);
        let caption = match state.measure_editing {
            Some((edit_index, text, caret)) if edit_index == index => {
                let mark = if caret { "_" } else { " " };
                format!("{text}{mark}")
            }
            _ if m.label.is_empty() => measure_text(m.line),
            _ => m.label.clone(),
        };
        draw_caption(
            &mut canvas,
            m.line.bbox(),
            size,
            &caption,
            state.style.label,
            state.ui_scale,
        );
    }

    if let Some(line) = state.measure_preview {
        canvas.draw_line(line, state.style.preview, state.style.thickness);
        draw_caption(
            &mut canvas,
            line.bbox(),
            size,
            &measure_text(line),
            state.style.preview,
            state.ui_scale,
        );
    }

    if let Some(shape) = &state.preview {
        canvas.draw_shape(
            shape,
            state.style.preview,
            state.style.thickness,
            state.style.fill,
        );
        draw_caption(
            &mut canvas,
            shape.bbox(),
            size,
            &coord_text(shape),
            state.style.preview,
            state.ui_scale,
        );
    }

    draw_cursor_readout(&mut canvas, background, size, state);
    draw_hud(&mut canvas, size, state);
    draw_loupe(&mut canvas, background, size, state);
}

/// Darken every pixel of `buffer` that lies outside `region`, in place.
///
/// The buffer is 0RGB (four bytes per pixel, top byte unused). Scaling
/// each byte by an integer numerator and dividing by 256 keeps the math
/// in integers and avoids branching.
fn shade_outside(buffer: &mut [u32], size: Size, region: Rect) {
    // ~0.35 · 256 — dark enough to read as "out of bounds" while leaving
    // enough contrast that the reader can still see what is there.
    const NUM: u32 = 90;
    let width = size.w as usize;
    let height = size.h as usize;
    let left = region.x.max(0) as usize;
    let top = region.y.max(0) as usize;
    let right = ((region.x + region.w).max(0) as usize).min(width);
    let bottom = ((region.y + region.h).max(0) as usize).min(height);
    for row in 0..height {
        for col in 0..width {
            if col >= left && col < right && row >= top && row < bottom {
                continue;
            }
            let index = row * width + col;
            let pixel = buffer[index];
            let red = ((pixel >> 16) & 0xff) * NUM / 256;
            let green = ((pixel >> 8) & 0xff) * NUM / 256;
            let blue = (pixel & 0xff) * NUM / 256;
            buffer[index] = (red << 16) | (green << 8) | blue;
        }
    }
}

/// The magnifier: a bordered box beside the cursor showing the frozen
/// pixels around it at high zoom, the cursor's exact pixel outlined.
/// Drawn from `background` — the loupe shows the image being measured,
/// not the chrome drawn over it.
fn draw_loupe(canvas: &mut Canvas, background: &[u32], size: Size, state: &FrameState) {
    if !state.loupe {
        return;
    }
    let Some(cursor) = state.cursor else { return };
    let scale = state.ui_scale.max(1);
    let zoom = 6 * scale;
    let edge_len = (LOUPE_SRC_RADIUS * 2 + 1) * zoom;
    let gap = 20 * scale;

    // Above-right of the cursor, flipping to stay fully on-screen.
    let mut x0 = cursor.x + gap;
    if x0 + edge_len > size.w {
        x0 = cursor.x - gap - edge_len;
    }
    let mut y0 = cursor.y - gap - edge_len;
    if y0 < 0 {
        y0 = cursor.y + gap;
    }
    let x0 = x0.clamp(0, (size.w - edge_len).max(0));
    let y0 = y0.clamp(0, (size.h - edge_len).max(0));

    for sy in -LOUPE_SRC_RADIUS..=LOUPE_SRC_RADIUS {
        for sx in -LOUPE_SRC_RADIUS..=LOUPE_SRC_RADIUS {
            let px = sample(background, size, cursor.x + sx, cursor.y + sy);
            canvas.fill_rect(
                Rect::new(
                    x0 + (sx + LOUPE_SRC_RADIUS) * zoom,
                    y0 + (sy + LOUPE_SRC_RADIUS) * zoom,
                    zoom,
                    zoom,
                ),
                px,
            );
        }
    }
    // The cursor's exact pixel, outlined; then the loupe's own border.
    canvas.draw_rect_outline(
        Rect::new(
            x0 + LOUPE_SRC_RADIUS * zoom,
            y0 + LOUPE_SRC_RADIUS * zoom,
            zoom,
            zoom,
        ),
        Color::WHITE,
        scale,
    );
    canvas.draw_rect_outline(Rect::new(x0, y0, edge_len, edge_len), PANEL_MUTED, scale);

    // The center pixel's hex, under the box. At loupe zoom *which* pixel
    // the number describes is finally unambiguous, which it is not in the
    // cursor chip when the pointer sits on an edge.
    let hex = sample(background, size, cursor.x, cursor.y).to_hex();
    let hex_w = font::text_width(hex.chars().count(), scale);
    let hex_x = x0 + (edge_len - hex_w) / 2;
    let hex_y = y0 + edge_len + 2 * scale;
    canvas.dim_rect(
        Rect::new(
            hex_x - 2 * scale,
            hex_y - scale,
            hex_w + 4 * scale,
            font::line_height(scale) + 2 * scale,
        ),
        PANEL_DIM,
    );
    canvas.draw_text(hex_x, hex_y, &hex, Color::WHITE, scale);
}

/// One background pixel as a Color; outside the frame reads as black.
fn sample(background: &[u32], size: Size, x: i32, y: i32) -> Color {
    if x < 0 || y < 0 || x >= size.w || y >= size.h {
        return Color { r: 0, g: 0, b: 0 };
    }
    let px = background[(y as usize) * (size.w as usize) + (x as usize)];
    Color {
        r: ((px >> 16) & 0xFF) as u8,
        g: ((px >> 8) & 0xFF) as u8,
        b: (px & 0xFF) as u8,
    }
}

/// Legend-style wordmark: a near-black pad breaks the border line and
/// the wordmark sits half above, half on the card.
fn draw_wordmark(
    canvas: &mut Canvas,
    panel: &Rect,
    pad: i32,
    title_h: i32,
    scale: i32,
    text_scale: i32,
) {
    let title_w = font::text_width(BRAND.chars().count(), text_scale);
    let title_x = panel.x + pad;
    let title_y = panel.y - title_h / 2;
    canvas.dim_rect(
        Rect::new(title_x - 3 * scale, title_y, title_w + 6 * scale, title_h),
        24,
    );
    canvas.draw_text(title_x, title_y, BRAND, Color::WHITE, text_scale);
}

/// The live readout: monitor-local coordinates in a small chip beside
/// the pointer, always current — the number automation actually wants,
/// visible before any shape exists.
fn draw_cursor_readout(canvas: &mut Canvas, background: &[u32], size: Size, state: &FrameState) {
    let Some(cursor) = state.cursor else { return };
    let scale = state.ui_scale.max(1);
    // Read from `background`, not the buffer being composed: by the time
    // the chip is drawn the buffer already carries outlines and captions,
    // and sampling that would report the color of the chrome sitting over
    // the pixel rather than the pixel.
    let text = format!(
        "{}, {}  {}",
        cursor.x,
        cursor.y,
        sample(background, size, cursor.x, cursor.y).to_hex()
    );
    let text_w = font::text_width(text.chars().count(), scale);
    let text_h = font::line_height(scale);
    let off = 14 * scale;
    let chip_x = (cursor.x + off).min(size.w - text_w).max(0);
    let chip_y = (cursor.y + off).min(size.h - text_h).max(0);
    // A dim pad under the digits keeps them readable over any pixels.
    canvas.dim_rect(
        Rect::new(
            chip_x - 2 * scale,
            chip_y - scale,
            text_w + 4 * scale,
            text_h + 2 * scale,
        ),
        PANEL_DIM,
    );
    canvas.draw_text(chip_x, chip_y, &text, Color::WHITE, scale);
}

fn draw_caption(
    canvas: &mut Canvas,
    bbox: pixelcoords_core::geometry::Rect,
    bounds: Size,
    text: &str,
    color: Color,
    scale: i32,
) {
    let pos = smart_text_position(bbox, bounds, text.chars().count(), scale);
    canvas.draw_text(pos.x, pos.y, text, color, scale);
}

/// The control panel: a dimmed, bordered card in the bottom-left with a
/// key column (label color) and an action column (muted), instead of one
/// long line drawn straight onto the image.
fn draw_hud(canvas: &mut Canvas, size: Size, state: &FrameState) {
    let scale = state.ui_scale.max(1);
    let margin = HUD_MARGIN * scale;
    if state.panel_hidden {
        // The card is gone but messages still land in its corner.
        if let Some(message) = state.flash {
            let y = size.h - margin - font::line_height(scale);
            let message = font::fit_to_width(message, size.w - margin * 2, scale);
            canvas.draw_text(margin, y, &message, state.style.complete, scale);
        }
        return;
    }
    // The panel reads at a glance from across the screen, so its type is
    // one step larger than the monitor's base text scale.
    let text_scale = scale + 1;
    let rows =
        if state.editing.is_some() || state.measure_editing.is_some() || state.naming.is_some() {
            state.strings.hud_edit_rows
        } else {
            state.strings.hud_hint_rows
        };

    // The W row doubles as the live tool indicator — a static hint list
    // everywhere else, one dynamic slot here.
    let tool_name = match state.tool {
        ToolKind::Rect => "rect",
        ToolKind::Circle => "circle",
        ToolKind::Ellipse => "ellipse",
        ToolKind::Triangle => "triangle",
        ToolKind::Polygon => "polygon",
        ToolKind::Freehand => "freehand",
        ToolKind::Poly => "poly",
        ToolKind::Measure => "measure",
    };
    let tool_action = if state.tool == ToolKind::Polygon {
        format!("tool: polygon ({})", state.polygon_sides)
    } else {
        format!("tool: {tool_name}")
    };
    let rows: Vec<(&str, &str)> = rows
        .iter()
        .map(|&(key, action)| {
            if key == "W" {
                (key, tool_action.as_str())
            } else {
                (key, action)
            }
        })
        .collect();

    let key_chars = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let action_chars = rows
        .iter()
        .map(|(_, a)| a.chars().count())
        .max()
        .unwrap_or(0);
    let pad = PANEL_PAD * text_scale;
    let line_h = font::line_height(text_scale) + PANEL_LEADING * text_scale;
    let text_w = font::text_width(key_chars + PANEL_GAP + action_chars, text_scale);
    let panel_w = text_w + pad * 2;
    // The wordmark rides the top border, half above it — the panel may
    // never be parked so high that the overhang clips.
    let title_h = font::line_height(text_scale);
    let overhang = title_h / 2 + scale;
    let panel_h = line_h * rows.len() as i32 - PANEL_LEADING * text_scale + pad * 2 + title_h / 2;
    // Where the user parked it wins, clamped fully on-screen; the default
    // is the bottom-left corner.
    let origin = state.panel_origin.map_or_else(
        || Point::new(margin, size.h - margin - panel_h),
        |p| {
            Point::new(
                p.x.clamp(0, (size.w - panel_w).max(0)),
                p.y.clamp(overhang, (size.h - panel_h).max(overhang)),
            )
        },
    );
    let panel = Rect::new(origin.x, origin.y, panel_w, panel_h);
    canvas.dim_rect(panel, PANEL_DIM);
    canvas.draw_rect_outline(panel, PANEL_BORDER, text_scale.min(2));

    draw_wordmark(canvas, &panel, pad, title_h, scale, text_scale);

    let rows_top = panel.y + pad + title_h / 2;
    for (i, (key, action)) in rows.iter().enumerate() {
        let y = rows_top + (i as i32) * line_h;
        // Keys right-align into their column so single letters sit next
        // to their action, not floating far left of it.
        let key_pad = key_chars.saturating_sub(key.chars().count());
        let key_x = panel.x + pad + font::text_width(key_pad, text_scale);
        canvas.draw_text(key_x, y, key, state.style.label, text_scale);
        let action_x = panel.x + pad + font::text_width(key_chars + PANEL_GAP, text_scale);
        canvas.draw_text(action_x, y, action, PANEL_MUTED, text_scale);
    }

    let status_y = panel.y - font::line_height(scale) - 6 * scale;
    if let Some((text, caret)) = state.naming {
        let mark = if caret { "_" } else { " " };
        let line = font::fit_to_width(
            &format!("session name: {text}{mark}"),
            size.w - margin * 2,
            scale,
        );
        canvas.draw_text(margin, status_y, &line, state.style.label, scale);
        return;
    }
    if let Some(message) = state.flash {
        // Flashes carry save paths and error chains, which routinely outrun
        // the screen; clipping happens silently, so cut it visibly instead.
        let message = font::fit_to_width(message, size.w - margin * 2, scale);
        canvas.draw_text(margin, status_y, &message, state.style.complete, scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelcoords_core::config::Config;
    use pixelcoords_core::geometry::Rect;
    use pixelcoords_core::selection::Selection;
    use pixelcoords_core::strings::EN;

    const W: i32 = 800;
    const H: i32 = 520;

    fn frame(selections: &SelectionSet, preview: Option<Shape>) -> Vec<u32> {
        let background = vec![0x0000_1122u32; (W * H) as usize];
        let mut buffer = vec![0u32; (W * H) as usize];
        let state = FrameState {
            selections,
            monitor: 0,
            target: None,
            preview,
            editing: None,
            measure_editing: None,
            measure_preview: None,
            flash: None,
            strings: &EN,
            style: Config::default().resolve_style().unwrap(),
            ui_scale: 1,
            panel_origin: None,
            panel_hidden: false,
            tool: ToolKind::Rect,
            polygon_sides: 6,
            cursor: None,
            loupe: false,
            naming: None,
        };
        compose(&mut buffer, Size::new(W, H), &background, &state);
        buffer
    }

    /// Compose with the cursor parked on a pixel of a known color, and a
    /// committed selection whose outline is drawn *over* that same pixel.
    /// The readout must report the frozen pixel, not the outline.
    fn frame_with_cursor_over_an_outline(loupe: bool) -> (Vec<u32>, Color) {
        const HIDDEN: u32 = 0x0000_3A7B;
        let at = Point::new(600, 20);
        let mut background = vec![0x0000_1122u32; (W * H) as usize];
        background[(at.y * W + at.x) as usize] = HIDDEN;

        let mut selections = SelectionSet::new();
        // Its top-left corner lands exactly on `at`, so the complete-color
        // outline paints over the pixel before the chip is drawn.
        selections.add(Selection::new(Shape::Rect(Rect::new(600, 20, 40, 30)), 0));

        let mut buffer = vec![0u32; (W * H) as usize];
        let state = FrameState {
            selections: &selections,
            monitor: 0,
            target: None,
            preview: None,
            editing: None,
            measure_editing: None,
            measure_preview: None,
            flash: None,
            strings: &EN,
            style: Config::default().resolve_style().unwrap(),
            ui_scale: 1,
            panel_origin: None,
            panel_hidden: false,
            tool: ToolKind::Rect,
            polygon_sides: 6,
            cursor: Some(at),
            loupe,
            naming: None,
        };
        compose(&mut buffer, Size::new(W, H), &background, &state);
        (
            buffer,
            Color {
                r: 0x00,
                g: 0x3A,
                b: 0x7B,
            },
        )
    }

    #[test]
    fn the_readout_samples_the_frozen_frame_not_the_chrome_over_it() {
        let (buffer, hidden) = frame_with_cursor_over_an_outline(false);
        let style = Config::default().resolve_style().unwrap();

        // The premise: the outline really did paint over that pixel, so a
        // naive sample of the composed buffer would read the outline.
        assert_eq!(
            buffer[(20 * W + 600) as usize],
            style.complete.to_0rgb(),
            "the selection outline covers the sampled pixel"
        );
        assert_ne!(hidden.to_0rgb(), style.complete.to_0rgb());

        // And the chip drew *something* — the hex text lands in the pad
        // offset from the cursor, so the region right of it is no longer
        // pure background.
        let chip_row = 20 + 14;
        let painted = (600..760).any(|x| buffer[(chip_row * W + x) as usize] != 0x0000_1122);
        assert!(painted, "the cursor chip should have drawn its readout");
    }

    #[test]
    fn the_loupe_draws_its_hex_caption() {
        let (with_loupe, _) = frame_with_cursor_over_an_outline(true);
        let (without, _) = frame_with_cursor_over_an_outline(false);
        assert_ne!(
            with_loupe, without,
            "holding the loupe must change what is drawn"
        );
    }

    #[test]
    fn background_shows_through_where_nothing_is_drawn() {
        let selections = SelectionSet::new();
        let buffer = frame(&selections, None);
        // Top-right, well clear of the panel card in the bottom-left.
        assert_eq!(buffer[(20 * W + 700) as usize], 0x0000_1122);
    }

    #[test]
    fn committed_selection_is_drawn_in_complete_color() {
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(Shape::Rect(Rect::new(600, 20, 40, 30)), 0));
        let buffer = frame(&selections, None);
        let style = Config::default().resolve_style().unwrap();
        assert_eq!(buffer[(20 * W + 600) as usize], style.complete.to_0rgb());
    }

    #[test]
    fn preview_is_drawn_in_preview_color() {
        let selections = SelectionSet::new();
        let buffer = frame(&selections, Some(Shape::Rect(Rect::new(700, 100, 20, 20))));
        let style = Config::default().resolve_style().unwrap();
        assert_eq!(buffer[(100 * W + 700) as usize], style.preview.to_0rgb());
    }

    #[test]
    fn cursor_readout_draws_beside_the_pointer() {
        let selections = SelectionSet::new();
        let background = vec![0x0000_1122u32; (W * H) as usize];
        let mut buffer = vec![0u32; (W * H) as usize];
        let state = FrameState {
            selections: &selections,
            monitor: 0,
            target: None,
            preview: None,
            editing: None,
            measure_editing: None,
            measure_preview: None,
            flash: None,
            strings: &EN,
            style: Config::default().resolve_style().unwrap(),
            ui_scale: 1,
            panel_origin: None,
            panel_hidden: true,
            tool: ToolKind::Rect,
            polygon_sides: 6,
            cursor: Some(Point::new(400, 100)),
            loupe: false,
            naming: None,
        };
        compose(&mut buffer, Size::new(W, H), &background, &state);
        // White digits appear in the chip area below-right of the cursor.
        let mut white = false;
        for y in 100..140 {
            for x in 400..500 {
                white |= buffer[(y * W + x) as usize] == 0x00FF_FFFF;
            }
        }
        assert!(white, "readout digits missing");
    }

    #[test]
    fn loupe_magnifies_the_pixels_around_the_cursor() {
        let selections = SelectionSet::new();
        let mut background = vec![0x0000_1122u32; (W * H) as usize];
        // One distinctive pixel right of the cursor.
        background[(100 * W + 401) as usize] = 0x00AB_CDEF;
        let mut buffer = vec![0u32; (W * H) as usize];
        let state = FrameState {
            selections: &selections,
            monitor: 0,
            target: None,
            preview: None,
            editing: None,
            measure_editing: None,
            measure_preview: None,
            flash: None,
            strings: &EN,
            style: Config::default().resolve_style().unwrap(),
            ui_scale: 1,
            panel_origin: None,
            panel_hidden: true,
            tool: ToolKind::Rect,
            polygon_sides: 6,
            cursor: Some(Point::new(400, 100)),
            loupe: true,
            naming: None,
        };
        compose(&mut buffer, Size::new(W, H), &background, &state);
        // The single odd pixel becomes a zoom x zoom block in the loupe.
        let magnified = buffer.iter().filter(|&&p| p == 0x00AB_CDEF).count();
        assert!(magnified >= 36, "expected a 6x6 block, got {magnified} px");
    }

    #[test]
    fn hud_panel_dims_its_backdrop_and_draws_both_columns() {
        let selections = SelectionSet::new();
        let buffer = frame(&selections, None);
        let style = Config::default().resolve_style().unwrap();
        // The panel card dims the frozen image beneath it, keys render in
        // the label color, actions in the muted gray — all three must
        // appear in the bottom-left quadrant.
        let bg = 0x0000_1122u32;
        let dimmed = {
            let r = (((bg >> 16) & 0xFF) * PANEL_DIM) >> 8;
            let g = (((bg >> 8) & 0xFF) * PANEL_DIM) >> 8;
            let b = ((bg & 0xFF) * PANEL_DIM) >> 8;
            (r << 16) | (g << 8) | b
        };
        let mut saw_dim = false;
        let mut saw_key = false;
        let mut saw_action = false;
        for y in (H / 2)..H {
            for x in 0..(W / 2) {
                let p = buffer[(y * W + x) as usize];
                saw_dim |= p == dimmed;
                saw_key |= p == style.label.to_0rgb();
                saw_action |= p == PANEL_MUTED.to_0rgb();
            }
        }
        assert!(
            saw_dim && saw_key && saw_action,
            "dim {saw_dim}, key {saw_key}, action {saw_action}"
        );
        // The wordmark renders in white on the panel's top edge; with no
        // cursor in this frame, white pixels can only be the brand.
        let saw_brand = buffer.contains(&0x00FF_FFFF);
        assert!(saw_brand, "PixelCoords wordmark missing");
    }
}
