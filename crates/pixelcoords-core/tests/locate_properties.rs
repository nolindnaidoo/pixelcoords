//! Property test for template relocation: a crop cut from a capture must
//! be found exactly where it was cut, whatever the content — the invariant
//! `pixelcoords find`'s coordinates rest on.

use pixelcoords_core::locate::{GrayImage, LocateError, SCORE_FLOOR, Template, locate};
use proptest::prelude::*;

/// Deterministic xorshift noise, smoothed so neighboring pixels correlate
/// the way real screen content does — iid noise defeats any pyramid by
/// construction, and screens are not noise. Every window is still unique,
/// so the match is well-posed. Determinism comes from the seed being a
/// proptest input — the assertion itself is clock-free and randomness-free.
fn textured(w: usize, h: usize, seed: u32) -> GrayImage {
    let mut state = seed | 1;
    let mut px: Vec<f32> = (0..w * h)
        .map(|_| {
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

proptest! {
    #[test]
    fn a_cut_crop_relocates_to_where_it_was_cut(
        seed in 1u32..,
        (sw, sh) in (40usize..90, 40usize..90),
        (tw, th) in (8usize..24, 8usize..24),
        (fx, fy) in (0.0f64..1.0, 0.0f64..1.0),
    ) {
        let screen = textured(sw, sh, seed);
        let x = (fx * (sw - tw) as f64) as usize;
        let y = (fy * (sh - th) as f64) as usize;
        let mut px = Vec::with_capacity(tw * th);
        for ty in 0..th {
            for tx in 0..tw {
                px.push(screen.px[(y + ty) * sw + x + tx]);
            }
        }
        let tpl = Template {
            gray: GrayImage { w: tw, h: th, px },
            mask: vec![true; tw * th],
        };
        match locate(&screen, &tpl, None) {
            // Noise is never flat in practice, but a flat cut is a legal
            // refusal, not a wrong answer.
            Err(LocateError::FlatTemplate) => {}
            Err(e) => prop_assert!(false, "unexpected error: {e}"),
            Ok(loc) => {
                prop_assert_eq!((loc.x, loc.y), (x as i32, y as i32));
                prop_assert!(loc.score >= SCORE_FLOOR, "score {}", loc.score);
                prop_assert!(!loc.ambiguous, "noise has no duplicates: {loc:?}");
            }
        }
    }
}
