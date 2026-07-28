//! Embedded vector font — antialiased text for the overlay.
//!
//! `JetBrains Mono` Regular, rasterized on demand with `fontdue`. The font
//! is monospace, so every layout computation stays "count x advance" — the
//! same model the old bitmap table used, with the blockiness gone. The
//! typeface is (c) 2020 The `JetBrains Mono` Project Authors, SIL Open Font
//! License 1.1; the license text ships in `assets/JetBrainsMono-OFL.txt`.

use std::sync::OnceLock;

const FONT_BYTES: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");

/// Text size in pixels at UI scale 1; `scale` multiplies it.
const BASE_PX: f32 = 12.0;

fn font() -> &'static fontdue::Font {
    static FONT: OnceLock<fontdue::Font> = OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(FONT_BYTES, fontdue::FontSettings::default())
            .expect("the embedded font parses")
    })
}

fn px_size(scale: i32) -> f32 {
    // The cap keeps rasterization sane for absurd caller values.
    BASE_PX * scale.clamp(1, 64) as f32
}

/// Horizontal advance of one glyph — monospace, so every glyph's.
pub fn advance(scale: i32) -> i32 {
    font().metrics('M', px_size(scale)).advance_width.round() as i32
}

/// Vertical space one line of text occupies.
pub fn line_height(scale: i32) -> i32 {
    let m = font()
        .horizontal_line_metrics(px_size(scale))
        .expect("a horizontal font has line metrics");
    (m.ascent - m.descent).round() as i32
}

/// Baseline offset from the top of the line box.
pub fn ascent(scale: i32) -> i32 {
    let m = font()
        .horizontal_line_metrics(px_size(scale))
        .expect("a horizontal font has line metrics");
    m.ascent.round() as i32
}

/// Pixel width of `len` glyphs at `scale`.
pub fn text_width(len: usize, scale: i32) -> i32 {
    (len as i32) * advance(scale)
}

/// How many glyphs fit in `max_width` pixels at `scale`.
pub fn fits_in_width(max_width: i32, scale: i32) -> usize {
    usize::try_from(max_width / advance(scale).max(1)).unwrap_or(0)
}

/// `text` shortened to fit `max_width` pixels, marking a cut with `..`.
///
/// Drawing clips silently at the buffer edge, so an over-long message —
/// an error naming a path, most of all — would lose its tail without any
/// sign that it had been cut.
pub fn fit_to_width(text: &str, max_width: i32, scale: i32) -> String {
    let budget = fits_in_width(max_width, scale);
    if text.chars().count() <= budget {
        return text.to_string();
    }
    // Two glyphs of the budget go to the marker; below that there is no
    // room to say anything useful at all.
    if budget <= 2 {
        return String::new();
    }
    let kept: String = text.chars().take(budget - 2).collect();
    format!("{kept}..")
}

/// Rasterize one glyph at `scale`: placement metrics plus a row-major
/// coverage bitmap (0 = transparent, 255 = full ink).
pub fn rasterize(ch: char, scale: i32) -> (fontdue::Metrics, Vec<u8>) {
    font().rasterize(ch, px_size(scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_fits_is_returned_whole() {
        let width = text_width(20, 1);
        assert_eq!(fit_to_width("Saved", width, 1), "Saved");
    }

    #[test]
    fn over_long_text_is_cut_visibly() {
        let width = text_width(10, 1);
        let fitted = fit_to_width("Save failed: no space left on device", width, 1);
        assert_eq!(fitted.chars().count(), 10);
        assert!(fitted.ends_with(".."), "{fitted}");
        assert!(text_width(fitted.chars().count(), 1) <= width);
    }

    #[test]
    fn fitting_accounts_for_scale() {
        // Ten glyphs' worth of scale-1 pixels holds fewer larger glyphs.
        let width = text_width(10, 1);
        assert!(fits_in_width(width, 2) < 10);
        assert!(fits_in_width(width, 2) >= 4);
    }

    #[test]
    fn a_hopeless_budget_yields_nothing_rather_than_junk() {
        assert_eq!(fit_to_width("Save failed", text_width(2, 1), 1), "");
        assert_eq!(fit_to_width("Save failed", 0, 1), "");
    }

    #[test]
    fn metrics_grow_with_scale() {
        assert!(advance(1) > 0);
        assert!(advance(2) > advance(1));
        assert!(line_height(2) > line_height(1));
        assert!(ascent(1) > 0 && ascent(1) < line_height(1));
    }

    #[test]
    fn glyphs_rasterize_with_ink_and_space_without() {
        let (_, cov) = rasterize('A', 2);
        assert!(cov.contains(&255), "solid ink somewhere in 'A'");
        let (m, cov) = rasterize(' ', 2);
        assert!(cov.iter().all(|&a| a == 0), "space has no ink");
        assert_eq!(m.width * m.height, cov.len());
    }

    #[test]
    fn lowercase_and_unicode_have_distinct_glyphs() {
        let (_, a) = rasterize('a', 2);
        let (_, upper) = rasterize('A', 2);
        assert_ne!(a, upper);
        // The typeface covers far more than ASCII now.
        let (_, e_acute) = rasterize('\u{00E9}', 2);
        assert!(e_acute.iter().any(|&v| v > 0));
    }

    #[test]
    fn glyphs_fit_the_monospace_cell() {
        // Every printable ASCII glyph's ink stays within one advance and
        // one line box — the guarantee the column layout rests on.
        for b in 0x20u8..=0x7E {
            let (m, _) = rasterize(b as char, 2);
            assert!(
                m.xmin >= -1 && m.xmin + m.width as i32 <= advance(2) + 1,
                "{:?} escapes its cell horizontally",
                b as char
            );
            assert!(
                m.height as i32 <= line_height(2),
                "{:?} escapes its line box",
                b as char
            );
        }
    }
}
