//! The point stream `assert --stdin` reads: one point per line, with an
//! optional per-line expectation.
//!
//! Scoring an agent trajectory means asking the same question of hundreds
//! of clicks, and a stream that silently skips what it cannot read is
//! worse than one that stops: a run that scored 400 of 500 points and
//! said nothing looks exactly like a run that scored all 500. So every
//! malformed line is an error naming what was wrong with it, and the
//! caller reports which line it was.
//!
//! Blank lines and `#` comments are carried through the grammar rather
//! than skipped by the caller, because a hand-written fixture file is a
//! normal thing to want and stripping them at the wrong layer means each
//! caller reinvents it.

use thiserror::Error;

use crate::geometry::Point;

/// One line of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Empty or whitespace only.
    Blank,
    /// A `#` comment.
    Comment,
    /// A point, and the label it is expected to land in when the line
    /// carried one. A per-line expectation overrides the run's `--expect`
    /// for that line only, so one stream can score a heterogeneous
    /// trajectory.
    Point {
        point: Point,
        expect: Option<String>,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PointError {
    #[error("{input:?} is not X,Y[,label] — it has no comma")]
    NoComma { input: String },
    #[error("{value:?} in {input:?} is not a whole number of pixels")]
    NotANumber { value: String, input: String },
    #[error(
        "{input:?} ends in a comma with no label — drop the comma, or name \
         the region the point should land in"
    )]
    EmptyLabel { input: String },
}

/// Parse one line: `X,Y`, `X,Y,label`, blank, or `# comment`.
///
/// Coordinates are whole physical pixels and may be negative — a monitor
/// left of the primary has negative global coordinates. A label may
/// itself contain commas; everything after the second comma is the label,
/// so `100,200,row 3, column 4` labels the point `"row 3, column 4"`.
pub fn parse_line(text: &str) -> Result<Line, PointError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Line::Blank);
    }
    if trimmed.starts_with('#') {
        return Ok(Line::Comment);
    }

    let mut parts = trimmed.splitn(3, ',');
    let (Some(x), Some(y)) = (parts.next(), parts.next()) else {
        return Err(PointError::NoComma {
            input: trimmed.to_string(),
        });
    };
    let point = Point::new(coord(x, trimmed)?, coord(y, trimmed)?);

    let expect = match parts.next() {
        None => None,
        Some(label) => {
            let label = label.trim();
            if label.is_empty() {
                return Err(PointError::EmptyLabel {
                    input: trimmed.to_string(),
                });
            }
            Some(label.to_string())
        }
    };
    Ok(Line::Point { point, expect })
}

fn coord(value: &str, input: &str) -> Result<i32, PointError> {
    value
        .trim()
        .parse::<i32>()
        .map_err(|_| PointError::NotANumber {
            value: value.trim().to_string(),
            input: input.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(text: &str) -> Line {
        parse_line(text).expect("a valid line")
    }

    #[test]
    fn a_bare_pair_is_a_point_with_no_expectation() {
        assert_eq!(
            point("812,440"),
            Line::Point {
                point: Point::new(812, 440),
                expect: None,
            }
        );
    }

    #[test]
    fn a_third_field_is_the_expected_label() {
        assert_eq!(
            point("812,440,submit"),
            Line::Point {
                point: Point::new(812, 440),
                expect: Some("submit".into()),
            }
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored_everywhere() {
        assert_eq!(point("  812 , 440 ,  submit  "), point("812,440,submit"));
    }

    #[test]
    fn coordinates_may_be_negative() {
        // A display left of the primary has negative global coordinates;
        // refusing them would make half a desktop unscoreable.
        assert_eq!(
            point("-1920,-40"),
            Line::Point {
                point: Point::new(-1920, -40),
                expect: None,
            }
        );
    }

    #[test]
    fn a_label_may_contain_commas() {
        // Only the first two commas are structural, so a label never has
        // to be quoted or escaped.
        assert_eq!(
            point("1,2,row 3, column 4"),
            Line::Point {
                point: Point::new(1, 2),
                expect: Some("row 3, column 4".into()),
            }
        );
    }

    #[test]
    fn blanks_and_comments_are_part_of_the_grammar() {
        assert_eq!(parse_line("").unwrap(), Line::Blank);
        assert_eq!(parse_line("   \t ").unwrap(), Line::Blank);
        assert_eq!(parse_line("# the login flow").unwrap(), Line::Comment);
        assert_eq!(parse_line("   # indented").unwrap(), Line::Comment);
    }

    #[test]
    fn a_line_with_no_comma_names_itself() {
        assert_eq!(
            parse_line("812").unwrap_err(),
            PointError::NoComma {
                input: "812".into()
            }
        );
    }

    #[test]
    fn a_non_numeric_coordinate_names_the_offending_field() {
        let err = parse_line("12,x").unwrap_err();
        assert_eq!(
            err,
            PointError::NotANumber {
                value: "x".into(),
                input: "12,x".into(),
            }
        );
        // The message has to carry both, or a caller reading a 1,000-line
        // failure cannot tell which half was wrong.
        let text = err.to_string();
        assert!(text.contains("\"x\"") && text.contains("\"12,x\""));
    }

    #[test]
    fn decimals_are_refused_rather_than_rounded() {
        // Rounding silently would put the scored point somewhere the
        // caller did not ask about.
        assert!(matches!(
            parse_line("12.5,40").unwrap_err(),
            PointError::NotANumber { .. }
        ));
    }

    #[test]
    fn a_trailing_comma_is_an_error_not_an_empty_label() {
        assert_eq!(
            parse_line("1,2,").unwrap_err(),
            PointError::EmptyLabel {
                input: "1,2,".into()
            }
        );
        assert!(matches!(
            parse_line("1,2,   ").unwrap_err(),
            PointError::EmptyLabel { .. }
        ));
    }

    #[test]
    fn an_out_of_range_coordinate_is_refused() {
        assert!(matches!(
            parse_line("99999999999,0").unwrap_err(),
            PointError::NotANumber { .. }
        ));
    }
}
