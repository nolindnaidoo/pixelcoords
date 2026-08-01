//! Per-region pixel comparison — the logic behind `pixelcoords diff`.
//!
//! `assert` answers "is this point inside a region"; `find` answers
//! "where did my region go". Neither answers "do my regions still look
//! the same", which is visual regression testing over regions a human
//! marked rather than over whole screenshots.
//!
//! The mask is not computed here. A saved crop already carries its shape
//! in its alpha channel — `save::write_crop` runs
//! `draw::apply_alpha_mask_outside` for every kind except an unrotated
//! rect, which is fully opaque and therefore fully in-mask — so rotated
//! and concave regions compare by their own silhouette for free, using
//! the same rule [`crate::locate::Template`] matches by.

use serde::Serialize;
use thiserror::Error;

use crate::geometry::{Point, Shape};
use crate::locate::MASK_ALPHA_FLOOR;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiffError {
    #[error("the crop has no visible pixels to compare")]
    EmptyCrop,
    #[error("the crop is larger than the frame it is compared against")]
    CropLargerThanFrame,
    #[error("the crop sits outside the frame at ({x}, {y})")]
    OutOfFrame { x: i32, y: i32 },
}

/// A saved crop prepared for comparison: its colour, and the shape mask
/// its alpha channel already carries.
///
/// Preparing it is where a crop with nothing visible is refused, so
/// `changed_pct` can never divide by zero.
#[derive(Debug, Clone)]
pub struct Baseline {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
    mask: Vec<bool>,
    masked_px: u64,
}

impl Baseline {
    /// From a crop's RGBA8 bytes. Alpha is the mask, not content.
    pub fn from_rgba(w: usize, h: usize, rgba: &[u8]) -> Result<Self, DiffError> {
        assert_eq!(rgba.len(), w * h * 4, "rgba buffer matches dimensions");
        let mask: Vec<bool> = rgba
            .chunks_exact(4)
            .map(|p| p[3] >= MASK_ALPHA_FLOOR)
            .collect();
        let masked_px = mask.iter().filter(|m| **m).count() as u64;
        if masked_px == 0 {
            return Err(DiffError::EmptyCrop);
        }
        Ok(Self {
            w,
            h,
            rgba: rgba.to_vec(),
            mask,
            masked_px,
        })
    }

    /// Pixels the shape covers — the denominator `changed_pct` uses.
    #[must_use]
    pub const fn masked_px(&self) -> u64 {
        self.masked_px
    }
}

/// How much of one region changed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RegionDiff {
    /// Pixels the shape covers. Reported so the denominator below is
    /// inspectable rather than implied.
    pub masked_px: u64,
    pub changed_px: u64,
    /// `changed_px / masked_px * 100`.
    ///
    /// Masked pixels, not the crop's area: a circle crop is roughly a
    /// fifth transparent, and dividing by area would make one
    /// `--tolerance` mean a different thing for every shape kind.
    pub changed_pct: f64,
    /// Mean absolute channel difference over the pixels that changed;
    /// exactly `0.0` when none did.
    pub mean_delta: f64,
}

/// Compare a baseline against the same-sized window of `frame` at `at`,
/// over the baseline's mask.
///
/// RGB only. Alpha is the mask, so comparing it would test the crop's
/// transparency against the capture's opaque 255 and mark every masked-in
/// pixel changed. Note the masked-*out* pixels still hold real colour —
/// cropping copies RGB and PNG is not premultiplied — so exclusion goes
/// through the mask, never through "the colour looks empty".
pub fn compare(
    baseline: &Baseline,
    frame_w: usize,
    frame_h: usize,
    frame: &[u8],
    at: Point,
) -> Result<RegionDiff, DiffError> {
    assert_eq!(
        frame.len(),
        frame_w * frame_h * 4,
        "rgba buffer matches dimensions"
    );
    if baseline.w > frame_w || baseline.h > frame_h {
        return Err(DiffError::CropLargerThanFrame);
    }
    if at.x < 0
        || at.y < 0
        || at.x as usize + baseline.w > frame_w
        || at.y as usize + baseline.h > frame_h
    {
        return Err(DiffError::OutOfFrame { x: at.x, y: at.y });
    }

    let (ox, oy) = (at.x as usize, at.y as usize);
    let mut changed_px = 0u64;
    let mut delta_total = 0u64;
    for row in 0..baseline.h {
        for col in 0..baseline.w {
            let index = row * baseline.w + col;
            if !baseline.mask[index] {
                continue;
            }
            let saved = &baseline.rgba[index * 4..index * 4 + 3];
            let live_at = ((oy + row) * frame_w + ox + col) * 4;
            let live = &frame[live_at..live_at + 3];
            let delta: u32 = (0..3)
                .map(|channel| u32::from(saved[channel].abs_diff(live[channel])))
                .sum();
            if delta > 0 {
                changed_px += 1;
                delta_total += u64::from(delta);
            }
        }
    }

    // Divides by the changed count, so an unchanged region would be 0/0.
    // NaN is not serializable, and every clean run would fail to print.
    let mean_delta = if changed_px == 0 {
        0.0
    } else {
        delta_total as f64 / (changed_px as f64 * 3.0)
    };
    Ok(RegionDiff {
        masked_px: baseline.masked_px,
        changed_px,
        changed_pct: changed_px as f64 * 100.0 / baseline.masked_px as f64,
        mean_delta,
    })
}

/// One region's comparison, with the identity every report row carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegionReport {
    /// Index into `session.selections` — this row's identity.
    pub index: usize,
    pub label: String,
    pub monitor: usize,
    /// Where the region sits, in the session's own physical pixels.
    /// Provenance, not a coordinate to act on — `resolve` answers that.
    pub region: Shape,
    #[serde(flatten)]
    pub diff: RegionDiff,
}

/// Every region within tolerance — the aggregate `diff` reports as `ok`.
///
/// `tolerance` is a percentage of masked pixels allowed to differ, and it
/// lives here rather than inside `compare` so a stored report can be
/// re-judged at a different bar without re-measuring.
#[must_use]
pub fn within(results: &[RegionReport], tolerance: f64) -> bool {
    !results.is_empty() && results.iter().all(|r| r.diff.changed_pct <= tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` RGBA buffer of one colour, fully opaque.
    fn solid(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect()
    }

    #[test]
    fn an_identical_region_is_clean_and_reports_no_mean() {
        let px = solid(4, 4, [10, 20, 30]);
        let base = Baseline::from_rgba(4, 4, &px).unwrap();
        let d = compare(&base, 4, 4, &px, Point::new(0, 0)).unwrap();
        assert_eq!((d.masked_px, d.changed_px), (16, 0));
        assert!(d.changed_pct.abs() < f64::EPSILON);
        assert!(
            d.mean_delta.abs() < f64::EPSILON,
            "0/0 would be NaN, which serde_json refuses to serialize"
        );
        // Serializing is the actual regression guard.
        serde_json::to_value(d).expect("a clean diff must serialize");
    }

    #[test]
    fn one_flipped_pixel_inside_the_mask_is_caught() {
        let base_px = solid(4, 4, [0, 0, 0]);
        let base = Baseline::from_rgba(4, 4, &base_px).unwrap();
        let mut frame = base_px.clone();
        frame[4 * 5] = 9; // pixel (1,1), red channel
        let d = compare(&base, 4, 4, &frame, Point::new(0, 0)).unwrap();
        assert_eq!(d.changed_px, 1);
        assert!((d.changed_pct - 6.25).abs() < 1e-9, "1 of 16 masked");
        assert!((d.mean_delta - 3.0).abs() < 1e-9, "9 over three channels");
    }

    #[test]
    fn a_change_outside_the_mask_is_ignored() {
        // Left column transparent: outside the shape, and still carrying
        // real colour, exactly as a shaped crop does.
        let mut px = solid(2, 2, [5, 5, 5]);
        px[3] = 0;
        px[4 * 2 + 3] = 0;
        let base = Baseline::from_rgba(2, 2, &px).unwrap();
        assert_eq!(base.masked_px(), 2);

        let mut frame = solid(2, 2, [5, 5, 5]);
        frame[0] = 200; // masked-out pixel changes wildly
        let d = compare(&base, 2, 2, &frame, Point::new(0, 0)).unwrap();
        assert_eq!(
            d.changed_px, 0,
            "shaped regions compare by their own pixels"
        );
    }

    #[test]
    fn alpha_is_the_mask_and_never_content() {
        // The crop is semi-transparent where it is in-mask; the capture
        // is opaque. Comparing alpha would call every pixel changed.
        let mut px = solid(2, 2, [7, 7, 7]);
        for i in 0..4 {
            px[i * 4 + 3] = 200;
        }
        let base = Baseline::from_rgba(2, 2, &px).unwrap();
        let frame = solid(2, 2, [7, 7, 7]);
        let d = compare(&base, 2, 2, &frame, Point::new(0, 0)).unwrap();
        assert_eq!(d.changed_px, 0);
    }

    #[test]
    fn the_region_is_compared_where_it_sits_in_the_frame() {
        let base = Baseline::from_rgba(2, 2, &solid(2, 2, [1, 2, 3])).unwrap();
        let mut frame = solid(8, 8, [0, 0, 0]);
        // Paint the matching patch at (5, 3).
        for y in 0..2 {
            for x in 0..2 {
                let j = ((3 + y) * 8 + 5 + x) * 4;
                frame[j..j + 3].copy_from_slice(&[1, 2, 3]);
            }
        }
        assert_eq!(
            compare(&base, 8, 8, &frame, Point::new(5, 3))
                .unwrap()
                .changed_px,
            0
        );
        assert!(
            compare(&base, 8, 8, &frame, Point::new(0, 0))
                .unwrap()
                .changed_px
                > 0,
            "the same crop elsewhere in the frame does not match"
        );
    }

    #[test]
    fn a_fully_transparent_crop_is_refused_rather_than_dividing_by_zero() {
        let px = vec![0u8; 4 * 4 * 4];
        assert_eq!(
            Baseline::from_rgba(4, 4, &px).unwrap_err(),
            DiffError::EmptyCrop
        );
    }

    #[test]
    fn a_crop_that_does_not_fit_is_refused_by_which_way_it_fails() {
        let base = Baseline::from_rgba(4, 4, &solid(4, 4, [1, 1, 1])).unwrap();
        let small = solid(2, 2, [1, 1, 1]);
        assert_eq!(
            compare(&base, 2, 2, &small, Point::new(0, 0)).unwrap_err(),
            DiffError::CropLargerThanFrame
        );

        let frame = solid(8, 8, [1, 1, 1]);
        assert_eq!(
            compare(&base, 8, 8, &frame, Point::new(6, 0)).unwrap_err(),
            DiffError::OutOfFrame { x: 6, y: 0 },
            "the crop would run off the right edge"
        );
        assert_eq!(
            compare(&base, 8, 8, &frame, Point::new(-1, 0)).unwrap_err(),
            DiffError::OutOfFrame { x: -1, y: 0 }
        );
    }

    fn row(diff: RegionDiff) -> RegionReport {
        RegionReport {
            index: 0,
            label: "submit".into(),
            monitor: 0,
            region: Shape::Rect(crate::geometry::Rect::new(0, 0, 10, 10)),
            diff,
        }
    }

    #[test]
    fn tolerance_is_a_bar_applied_to_results_not_baked_into_them() {
        let base_px = solid(10, 10, [0, 0, 0]);
        let base = Baseline::from_rgba(10, 10, &base_px).unwrap();
        let mut frame = base_px.clone();
        for i in 0..5 {
            frame[i * 4] = 255; // 5 of 100 pixels
        }
        let measured = compare(&base, 10, 10, &frame, Point::new(0, 0)).unwrap();
        assert!((measured.changed_pct - 5.0).abs() < 1e-9);

        // One measurement, judged at three bars — which is why the bar
        // lives out here and not inside `compare`.
        let rows = [row(measured)];
        assert!(!within(&rows, 0.0), "the default is exact");
        assert!(!within(&rows, 4.9));
        assert!(within(&rows, 5.0), "the bar is inclusive");
        assert!(!within(&[], 100.0), "nothing compared is not a pass");
    }

    #[test]
    fn a_row_flattens_its_measurement_into_one_json_object() {
        let base = Baseline::from_rgba(2, 2, &solid(2, 2, [0, 0, 0])).unwrap();
        let measured = compare(&base, 2, 2, &solid(2, 2, [0, 0, 0]), Point::new(0, 0)).unwrap();
        let json = serde_json::to_value(row(measured)).unwrap();
        // Flattened, not nested: a consumer reads changed_pct off the row
        // rather than off a sub-object it has to know the name of.
        for key in [
            "index",
            "label",
            "monitor",
            "region",
            "masked_px",
            "changed_px",
            "changed_pct",
            "mean_delta",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
    }
}
