//! Template relocation: find where a saved crop sits in a fresh capture —
//! the logic behind `pixelcoords find`.
//!
//! A session's coordinates describe one frozen instant; the moment the UI
//! drifts they are silently stale. Every selection already ships with a
//! pixel-exact crop, so the crop doubles as a search template: normalized
//! cross-correlation over a fresh capture finds where the region sits
//! *now*, reports how far it moved, and — just as important — says so
//! honestly when it cannot: a region whose pixels changed scores below the
//! floor, and a region that appears more than once comes back flagged
//! ambiguous instead of silently picking one.
//!
//! This is drift correction, not computer vision: it survives movement,
//! not redesigns or scale changes, and the caller is expected to refuse
//! mismatched DPI up front.

use serde::Serialize;
use thiserror::Error;

use crate::geometry::{Point, Shape, Size};
use crate::report::{Command, Report};
use crate::session::SelectionRecord;

/// Matches scoring below this are reported not found. Normalized
/// cross-correlation is 1.0 for a pixel-identical region; anti-aliasing
/// differences after a move cost a little, a changed region costs a lot.
pub const SCORE_FLOOR: f64 = 0.9;

/// A credible second location within this gap of the best makes the match
/// ambiguous — five identical checkboxes must not silently resolve to one.
pub const AMBIGUITY_GAP: f64 = 0.03;

/// Grayscale pixels in `[0, 1]`, row-major.
#[derive(Debug, Clone)]
pub struct GrayImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl GrayImage {
    /// From RGBA8 bytes (4 per pixel), Rec. 601 luma.
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8]) -> Self {
        assert_eq!(rgba.len(), w * h * 4, "rgba buffer matches dimensions");
        let px = rgba
            .chunks_exact(4)
            .map(|p| luma(p[0], p[1], p[2]))
            .collect();
        Self { w, h, px }
    }
}

fn luma(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) / 255.0
}

/// The alpha at or above which a crop pixel counts as inside its
/// shape. Public because `diff` must apply the identical rule — two
/// definitions of "inside" could silently drift apart.
pub const MASK_ALPHA_FLOOR: u8 = 128;

/// A crop as a search template: luma plus a mask excluding the transparent
/// pixels a circle, triangle, or rotated-rect crop carries outside its
/// shape.
#[derive(Debug, Clone)]
pub struct Template {
    pub gray: GrayImage,
    pub mask: Vec<bool>,
}

impl Template {
    /// From RGBA8 bytes; pixels with alpha below half are masked out.
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8]) -> Self {
        let gray = GrayImage::from_rgba(w, h, rgba);
        let mask = rgba
            .chunks_exact(4)
            .map(|p| p[3] >= MASK_ALPHA_FLOOR)
            .collect();
        Self { gray, mask }
    }
}

/// Where a template was found, and how sure the match is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// Top-left of the best match, in the searched image's pixels.
    pub x: i32,
    pub y: i32,
    /// Normalized cross-correlation of the best match, `-1..=1`.
    pub score: f64,
    /// Best score at a location separated from the match by at least half
    /// the template, or `-1` when the image holds no separated location.
    pub runner_up: f64,
    /// The runner-up is itself above `SCORE_FLOOR` and within
    /// `AMBIGUITY_GAP` of the best — the match is not trustworthy.
    pub ambiguous: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LocateError {
    #[error("the crop is larger than the capture it is searched in")]
    TemplateLargerThanScreen,
    #[error("the crop has no visible pixels to match")]
    EmptyTemplate,
    #[error("the crop is a flat color, which matches anywhere rather than somewhere")]
    FlatTemplate,
}

/// Precomputed template statistics over its masked pixels.
struct TemplateStats {
    coords: Vec<(usize, usize)>,
    values: Vec<f64>,
    mean: f64,
    var: f64,
}

fn template_stats(tpl: &Template) -> Result<TemplateStats, LocateError> {
    let mut coords = Vec::new();
    let mut values = Vec::new();
    for ty in 0..tpl.gray.h {
        for tx in 0..tpl.gray.w {
            if !tpl.mask[ty * tpl.gray.w + tx] {
                continue;
            }
            coords.push((tx, ty));
            values.push(f64::from(tpl.gray.px[ty * tpl.gray.w + tx]));
        }
    }
    if coords.is_empty() {
        return Err(LocateError::EmptyTemplate);
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>();
    if var <= f64::EPSILON {
        return Err(LocateError::FlatTemplate);
    }
    Ok(TemplateStats {
        coords,
        values,
        mean,
        var,
    })
}

/// Normalized cross-correlation of the template placed at (`ox`, `oy`).
fn ncc_at(screen: &GrayImage, stats: &TemplateStats, ox: usize, oy: usize) -> f64 {
    let n = stats.values.len() as f64;
    let mut sum_screen = 0.0;
    let mut sum_screen_sq = 0.0;
    let mut sum_cross = 0.0;
    for (i, &(tx, ty)) in stats.coords.iter().enumerate() {
        let s = f64::from(screen.px[(oy + ty) * screen.w + ox + tx]);
        sum_screen += s;
        sum_screen_sq += s * s;
        sum_cross += stats.values[i] * s;
    }
    let mean_screen = sum_screen / n;
    let var_screen = sum_screen_sq - n * mean_screen * mean_screen;
    if var_screen <= f64::EPSILON {
        return 0.0;
    }
    let cov = sum_cross - n * stats.mean * mean_screen;
    cov / (stats.var * var_screen).sqrt()
}

/// The best-scoring offset within the window, ends inclusive.
fn best_in_window(
    screen: &GrayImage,
    stats: &TemplateStats,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
) -> (usize, usize, f64) {
    let mut best = (x0, y0, f64::from(f32::MIN));
    for oy in y0..=y1 {
        for ox in x0..=x1 {
            let score = ncc_at(screen, stats, ox, oy);
            if score > best.2 {
                best = (ox, oy, score);
            }
        }
    }
    best
}

/// Find the template in `screen`. `hint` is where it was last seen —
/// always re-checked so a region that has not moved matches immediately.
pub fn locate(
    screen: &GrayImage,
    tpl: &Template,
    hint: Option<Point>,
) -> Result<Located, LocateError> {
    let (tw, th) = (tpl.gray.w, tpl.gray.h);
    if tw > screen.w || th > screen.h || tw == 0 || th == 0 {
        return Err(LocateError::TemplateLargerThanScreen);
    }
    let stats = template_stats(tpl)?;
    let max_x = screen.w - tw;
    let max_y = screen.h - th;

    // Coarse pass: downsampled full-image scan finds candidate locations
    // cheaply; small templates scan at full resolution instead. Scanning
    // the whole image even when the hint matches is what makes the
    // ambiguity flag honest — a duplicate elsewhere must be seen.
    let factor = match tw.min(th) {
        0..16 => 1,
        16..32 => 2,
        _ => 4,
    };
    let mut candidates = coarse_candidates(screen, tpl, factor)?;
    if let Some(p) = hint {
        candidates.push((
            p.x.clamp(0, max_x as i32) as usize,
            p.y.clamp(0, max_y as i32) as usize,
        ));
    }

    // Refine every candidate in a small full-resolution window.
    let margin = factor * 2;
    let mut refined: Vec<(usize, usize, f64)> = candidates
        .iter()
        .map(|&(cx, cy)| {
            let x0 = cx.saturating_sub(margin);
            let y0 = cy.saturating_sub(margin);
            best_in_window(
                screen,
                &stats,
                x0,
                y0,
                (cx + margin).min(max_x),
                (cy + margin).min(max_y),
            )
        })
        .collect();
    refined.sort_by(|a, b| b.2.total_cmp(&a.2));
    let best = refined[0];

    // The runner-up must be a genuinely different location: separated from
    // the best by at least half the template in some axis.
    let separated = |x: usize, y: usize| x.abs_diff(best.0) > tw / 2 || y.abs_diff(best.1) > th / 2;
    let runner_up = refined[1..]
        .iter()
        .filter(|&&(x, y, _)| separated(x, y))
        .map(|&(_, _, s)| s)
        .fold(-1.0f64, f64::max);

    let ambiguous = runner_up >= SCORE_FLOOR && best.2 - runner_up <= AMBIGUITY_GAP;
    Ok(Located {
        x: best.0 as i32,
        y: best.1 as i32,
        score: best.2,
        runner_up,
        ambiguous,
    })
}

/// Candidate locations from a full scan at `factor` downsampling: the best
/// cell and the best cell separated from it, mapped back to full
/// resolution. At factor 1 the scan is exact and the candidates are final
/// positions.
fn coarse_candidates(
    screen: &GrayImage,
    tpl: &Template,
    factor: usize,
) -> Result<Vec<(usize, usize)>, LocateError> {
    let (small_screen, small_stats) = if factor == 1 {
        (screen.clone(), template_stats(tpl)?)
    } else {
        let small_tpl = downsample_template(tpl, factor);
        (downsample(screen, factor), template_stats(&small_tpl)?)
    };
    let tw = small_stats.coords.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let th = small_stats.coords.iter().map(|c| c.1).max().unwrap_or(0) + 1;
    if tw > small_screen.w || th > small_screen.h {
        return Err(LocateError::TemplateLargerThanScreen);
    }
    let max_x = small_screen.w - tw;
    let max_y = small_screen.h - th;

    let mut scores = vec![0.0f64; (max_x + 1) * (max_y + 1)];
    for oy in 0..=max_y {
        for ox in 0..=max_x {
            scores[oy * (max_x + 1) + ox] = ncc_at(&small_screen, &small_stats, ox, oy);
        }
    }
    // The top cells, each outside the template footprint of those already
    // chosen. Three, not one: downsampling blurs away up to a cell of
    // phase, so the true peak can rank behind a lucky neighbor at coarse
    // resolution — refinement at full resolution settles it.
    let mut out: Vec<(usize, usize)> = Vec::new();
    for _ in 0..3 {
        let mut best = (0usize, 0usize, f64::from(f32::MIN));
        for oy in 0..=max_y {
            for ox in 0..=max_x {
                let suppressed = out.iter().any(|&(px, py)| {
                    ox.abs_diff(px / factor) <= tw / 2 && oy.abs_diff(py / factor) <= th / 2
                });
                if suppressed {
                    continue;
                }
                let s = scores[oy * (max_x + 1) + ox];
                if s > best.2 {
                    best = (ox, oy, s);
                }
            }
        }
        if best.2 <= f64::from(f32::MIN) {
            break;
        }
        out.push((best.0 * factor, best.1 * factor));
    }
    Ok(out)
}

/// Box-average downsample by `factor`.
fn downsample(img: &GrayImage, factor: usize) -> GrayImage {
    let w = img.w / factor;
    let h = img.h / factor;
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            for sy in 0..factor {
                for sx in 0..factor {
                    sum += img.px[(y * factor + sy) * img.w + x * factor + sx];
                }
            }
            px.push(sum / (factor * factor) as f32);
        }
    }
    GrayImage { w, h, px }
}

/// Downsample a masked template: a cell is masked in when at least half
/// its source pixels are, and averages only those.
fn downsample_template(tpl: &Template, factor: usize) -> Template {
    let w = tpl.gray.w / factor;
    let h = tpl.gray.h / factor;
    let mut px = Vec::with_capacity(w * h);
    let mut mask = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0f32;
            let mut n = 0usize;
            for sy in 0..factor {
                for sx in 0..factor {
                    let i = (y * factor + sy) * tpl.gray.w + x * factor + sx;
                    if !tpl.mask[i] {
                        continue;
                    }
                    sum += tpl.gray.px[i];
                    n += 1;
                }
            }
            let keep = n * 2 >= factor * factor;
            mask.push(keep);
            px.push(if keep { sum / n as f32 } else { 0.0 });
        }
    }
    Template {
        gray: GrayImage { w, h, px },
        mask,
    }
}

/// Where a selection's crop was cut from its monitor frame: the rotated
/// bounding box, clipped to the frame — the mirror of the save path.
pub fn crop_origin(shape: &Shape, rot_deg: i32, frame: Size) -> Point {
    let bbox = shape.rotated_bbox(rot_deg);
    Point::new(bbox.x.max(0).min(frame.w), bbox.y.max(0).min(frame.h))
}

/// One selection's relocation attempt, ready for the report.
pub struct Relocation {
    /// Where the crop's top-left sat when the session was saved.
    pub crop_origin: Point,
    pub outcome: Result<Located, LocateError>,
}

/// How far a selection moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Delta {
    pub dx: i32,
    pub dy: i32,
}

/// The `find` subcommand's JSON output for one selection.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindResult {
    pub index: usize,
    pub label: String,
    pub monitor: usize,
    pub found: bool,
    pub ambiguous: bool,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub old_px: Shape,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_px: Option<Shape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_global_px: Option<Shape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<Delta>,
}

/// Every selection was found, unambiguously — `find`'s aggregate, which
/// becomes the report's `ok` and the process's exit code.
///
/// An empty result set is *not* success: `find` was asked about regions
/// and answered about none.
#[must_use]
pub fn all_relocated(results: &[FindResult]) -> bool {
    !results.is_empty() && results.iter().all(|r| r.found && !r.ambiguous)
}

/// Assemble the report from the attempted selections — the whole session
/// or a labeled subset. Each attempt is `(index, record, relocation)`:
/// the record's index in the session file, which every row carries as its
/// identity, and the record itself.
///
/// The record is passed rather than looked up by index inside a
/// `SessionFile`, so a caller cannot hand over an index that does not
/// resolve. `session::select_by_label` returns exactly these pairs.
/// `captured_utc` is supplied by the caller — this crate has no clock.
pub fn report(
    attempts: &[(usize, &SelectionRecord, Relocation)],
    captured_utc: String,
) -> Report<FindResult> {
    let results: Vec<FindResult> = attempts
        .iter()
        .map(|(index, record, reloc)| {
            let index = *index;
            let base = FindResult {
                index,
                label: record.label.clone(),
                monitor: record.monitor,
                found: false,
                ambiguous: false,
                score: 0.0,
                reason: None,
                old_px: record.px.clone(),
                new_px: None,
                new_global_px: None,
                delta: None,
            };
            match &reloc.outcome {
                Err(e) => FindResult {
                    reason: Some(e.to_string()),
                    ..base
                },
                Ok(loc) => {
                    let found = loc.score >= SCORE_FLOOR;
                    let moved = found && !loc.ambiguous;
                    let dx = loc.x - reloc.crop_origin.x;
                    let dy = loc.y - reloc.crop_origin.y;
                    FindResult {
                        found,
                        ambiguous: found && loc.ambiguous,
                        score: loc.score,
                        new_px: moved.then(|| record.px.translated(dx, dy)),
                        new_global_px: moved.then(|| record.global_px.translated(dx, dy)),
                        delta: moved.then_some(Delta { dx, dy }),
                        ..base
                    }
                }
            }
        })
        .collect();
    let ok = all_relocated(&results);
    Report::captured(Command::Find, captured_utc, ok, results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    /// Deterministic pseudo-texture, smoothed so neighboring pixels
    /// correlate the way real screen content does — iid noise defeats any
    /// pyramid by construction, and screens are not noise. Every window is
    /// still unique, so the match stays well-posed.
    fn textured(w: usize, h: usize, seed: u32) -> GrayImage {
        let mut state = seed | 1;
        let mut px: Vec<f32> = (0..w * h)
            .map(|_| {
                // xorshift32 — deterministic, no clock, no rand crate.
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state % 1000) as f32 / 1000.0
            })
            .collect();
        for _ in 0..2 {
            px = blur3(&px, w, h);
        }
        GrayImage { w, h, px }
    }

    fn blur3(px: &[f32], w: usize, h: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                out.push(blurred_at(px, w, h, x, y));
            }
        }
        out
    }

    fn blurred_at(px: &[f32], width: usize, height: usize, col: usize, row: usize) -> f32 {
        let mut sum = 0.0f32;
        let mut count = 0.0f32;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = col as i32 + dx;
                let ny = row as i32 + dy;
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                sum += px[ny as usize * width + nx as usize];
                count += 1.0;
            }
        }
        sum / count
    }

    fn cut(img: &GrayImage, x: usize, y: usize, w: usize, h: usize) -> Template {
        let mut px = Vec::with_capacity(w * h);
        for ty in 0..h {
            for tx in 0..w {
                px.push(img.px[(y + ty) * img.w + x + tx]);
            }
        }
        Template {
            gray: GrayImage { w, h, px },
            mask: vec![true; w * h],
        }
    }

    #[test]
    fn a_cut_template_is_found_where_it_was_cut() {
        let screen = textured(200, 150, 7);
        let tpl = cut(&screen, 63, 41, 24, 18);
        // A stale hint must not stop the full-image scan finding the truth.
        let loc = locate(&screen, &tpl, Some(Point::new(5, 5))).unwrap();
        assert_eq!((loc.x, loc.y), (63, 41));
        assert!(loc.score > 0.999, "exact cut scores ~1, got {}", loc.score);
        assert!(!loc.ambiguous);
    }

    #[test]
    fn a_large_template_takes_the_pyramid_path_and_still_lands_exactly() {
        let screen = textured(400, 300, 11);
        let tpl = cut(&screen, 137, 92, 80, 64);
        let loc = locate(&screen, &tpl, None).unwrap();
        assert_eq!((loc.x, loc.y), (137, 92));
        assert!(loc.score > 0.999);
    }

    #[test]
    fn a_masked_template_ignores_its_transparent_corners() {
        let screen = textured(160, 120, 23);
        let mut tpl = cut(&screen, 50, 40, 32, 32);
        // Mask out the corners (a circle crop's transparency), then
        // vandalize those pixels in the template: the match must not care.
        for (i, m) in tpl.mask.iter_mut().enumerate() {
            let (x, y) = (i % 32, i / 32);
            let (cx, cy) = (16i32, 16i32);
            let (dx, dy) = (x as i32 - cx, y as i32 - cy);
            if dx * dx + dy * dy > 16 * 16 {
                *m = false;
            }
        }
        for (i, p) in tpl.gray.px.iter_mut().enumerate() {
            if !tpl.mask[i] {
                *p = 1.0 - *p;
            }
        }
        let loc = locate(&screen, &tpl, None).unwrap();
        assert_eq!((loc.x, loc.y), (50, 40));
        assert!(loc.score > 0.999);
    }

    #[test]
    fn changed_pixels_score_below_the_floor() {
        let screen = textured(160, 120, 31);
        let tpl = cut(&screen, 50, 40, 24, 24);
        let elsewhere = textured(160, 120, 97);
        let loc = locate(&elsewhere, &tpl, None).unwrap();
        assert!(
            loc.score < SCORE_FLOOR,
            "unrelated content must not match, got {}",
            loc.score
        );
    }

    #[test]
    fn a_duplicated_region_is_ambiguous_not_silently_resolved() {
        let mut screen = textured(300, 100, 43);
        // Stamp the patch at 20,30 onto 200,30 pixel-for-pixel.
        for ty in 0..24 {
            for tx in 0..24 {
                let v = screen.px[(30 + ty) * 300 + 20 + tx];
                screen.px[(30 + ty) * 300 + 200 + tx] = v;
            }
        }
        let tpl = cut(&screen, 20, 30, 24, 24);
        let loc = locate(&screen, &tpl, None).unwrap();
        assert!(loc.ambiguous, "two identical regions: {loc:?}");
        assert!(loc.runner_up > 0.999);
    }

    #[test]
    fn degenerate_templates_are_refused_with_reasons() {
        let screen = textured(50, 50, 3);
        let flat = Template {
            gray: GrayImage {
                w: 8,
                h: 8,
                px: vec![0.5; 64],
            },
            mask: vec![true; 64],
        };
        assert_eq!(
            locate(&screen, &flat, None).unwrap_err(),
            LocateError::FlatTemplate
        );
        let empty = Template {
            gray: GrayImage {
                w: 8,
                h: 8,
                px: vec![0.5; 64],
            },
            mask: vec![false; 64],
        };
        assert_eq!(
            locate(&screen, &empty, None).unwrap_err(),
            LocateError::EmptyTemplate
        );
        let huge = cut(&textured(80, 80, 5), 0, 0, 80, 80);
        assert_eq!(
            locate(&screen, &huge, None).unwrap_err(),
            LocateError::TemplateLargerThanScreen
        );
    }

    #[test]
    fn rgba_conversion_masks_transparency_and_weights_luma() {
        // Two pixels: opaque pure green, transparent white.
        let rgba = [0u8, 255, 0, 255, 255, 255, 255, 0];
        let tpl = Template::from_rgba(2, 1, &rgba);
        assert!(tpl.mask[0] && !tpl.mask[1]);
        assert!((tpl.gray.px[0] - 0.587).abs() < 1e-4);
        assert!((tpl.gray.px[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn crop_origin_is_the_clipped_rotated_bbox() {
        let frame = Size::new(1920, 1080);
        assert_eq!(
            crop_origin(&Shape::Rect(Rect::new(100, 50, 40, 30)), 0, frame),
            Point::new(100, 50)
        );
        // Rotated 90 about (120, 65): the tall silhouette starts left of
        // the unrotated box.
        assert_eq!(
            crop_origin(&Shape::Rect(Rect::new(100, 50, 40, 30)), 90, frame),
            Point::new(105, 45)
        );
        // A shape hanging off the top-left is clipped to the frame.
        assert_eq!(
            crop_origin(&Shape::Rect(Rect::new(-20, -10, 40, 30)), 0, frame),
            Point::new(0, 0)
        );
    }

    fn session_of_one() -> crate::session::SessionFile {
        use crate::selection::Selection;
        use crate::session::{MonitorRecord, SessionFile};
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 0);
        sel.label = "submit".into();
        SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 0,
                name: "Main".into(),
                primary: true,
                origin_px: Point::new(100, 0),
                size_px: Size::new(1920, 1080),
                scale: 1.0,
            }],
            &[sel],
            &["c0.png".into()],
            None,
        )
    }

    #[test]
    fn report_translates_found_selections_and_flags_the_rest() {
        let session = session_of_one();
        let found = report(
            &[(
                0,
                &session.selections[0],
                Relocation {
                    crop_origin: Point::new(10, 20),
                    outcome: Ok(Located {
                        x: 14,
                        y: 8,
                        score: 0.97,
                        runner_up: 0.1,
                        ambiguous: false,
                    }),
                },
            )],
            "2026-07-27T01:00:00Z".into(),
        );
        assert!(found.ok);
        let r = &found.results[0];
        assert_eq!(r.delta, Some(Delta { dx: 4, dy: -12 }));
        assert_eq!(r.new_px, Some(Shape::Rect(Rect::new(14, 8, 30, 40))));
        assert_eq!(
            r.new_global_px,
            Some(Shape::Rect(Rect::new(114, 8, 30, 40))),
            "global keeps the monitor origin offset"
        );

        let miss = report(
            &[(
                0,
                &session.selections[0],
                Relocation {
                    crop_origin: Point::new(10, 20),
                    outcome: Ok(Located {
                        x: 0,
                        y: 0,
                        score: 0.4,
                        runner_up: 0.1,
                        ambiguous: false,
                    }),
                },
            )],
            "t".into(),
        );
        assert!(!miss.ok);
        assert!(!miss.results[0].found);
        assert!(miss.results[0].new_px.is_none());

        let ambiguous = report(
            &[(
                0,
                &session.selections[0],
                Relocation {
                    crop_origin: Point::new(10, 20),
                    outcome: Ok(Located {
                        x: 14,
                        y: 8,
                        score: 0.99,
                        runner_up: 0.98,
                        ambiguous: true,
                    }),
                },
            )],
            "t".into(),
        );
        assert!(!ambiguous.ok);
        assert!(ambiguous.results[0].found && ambiguous.results[0].ambiguous);
        assert!(
            ambiguous.results[0].new_px.is_none(),
            "an ambiguous match must not hand out coordinates"
        );

        let errored = report(
            &[(
                0,
                &session.selections[0],
                Relocation {
                    crop_origin: Point::new(10, 20),
                    outcome: Err(LocateError::FlatTemplate),
                },
            )],
            "t".into(),
        );
        assert!(!errored.ok);
        assert!(
            errored.results[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("flat color")
        );
    }

    #[test]
    fn find_report_json_shape_is_stable() {
        let session = session_of_one();
        let rep = report(
            &[(
                0,
                &session.selections[0],
                Relocation {
                    crop_origin: Point::new(10, 20),
                    outcome: Ok(Located {
                        x: 10,
                        y: 20,
                        score: 1.0,
                        runner_up: 0.0,
                        ambiguous: false,
                    }),
                },
            )],
            "2026-07-27T01:00:00Z".into(),
        );
        let json = serde_json::to_value(&rep).unwrap();
        assert_eq!(json["schema"], 2);
        assert_eq!(json["command"], "find");
        assert_eq!(json["captured_utc"], "2026-07-27T01:00:00Z");
        assert_eq!(json["ok"], true, "all_relocated is spelled ok now");
        assert_eq!(json["results"][0]["label"], "submit");
        assert_eq!(json["results"][0]["delta"]["dx"], 0);
        assert_eq!(json["results"][0]["new_px"]["x"], 10);
        assert!(json["results"][0].get("reason").is_none());
        assert!(
            json["results"][0]["found"].as_bool().unwrap(),
            "the row keeps its own answer; ok is the aggregate over rows"
        );
    }
}
