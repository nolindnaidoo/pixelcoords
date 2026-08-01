//! Human durations on the CLI: `500ms`, `30s`, `2m`.
//!
//! One integer and one unit, and nothing else. A bare number is refused
//! because `--timeout 30` reads as seconds to one person and
//! milliseconds to another, and a tool that picks silently will be wrong
//! for half its users. Decimals, compounds (`1m30s`), uppercase, and
//! internal spaces are refused for the same reason `config` refuses
//! unknown keys: a value this program cannot honour exactly is an error
//! with an actionable message, never a guess.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DurationError {
    #[error("a duration cannot be empty — write it as 30s, 500ms, or 2m")]
    Empty,
    #[error("{input:?} has no unit — write 30s, 500ms, or 2m; a bare number is ambiguous")]
    NoUnit { input: String },
    #[error("{unit:?} in {input:?} is not a duration unit — use ms, s, or m")]
    UnknownUnit { unit: String, input: String },
    #[error("{digits:?} in {input:?} is not a whole number — durations are integers, not 1.5s")]
    NotAnInteger { digits: String, input: String },
    #[error("{input:?} is too large to represent")]
    Overflow { input: String },
}

/// Parse `<integer><unit>` where unit is `ms`, `s`, or `m`.
///
/// Zero parses (`0s`): this owns the syntax, and whether a command
/// accepts a zero timeout or interval is that command's rule to state.
pub fn parse_duration(text: &str) -> Result<Duration, DurationError> {
    if text.is_empty() {
        return Err(DurationError::Empty);
    }
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| DurationError::NoUnit {
            input: text.to_string(),
        })?;
    let (digits, unit) = text.split_at(split);
    if digits.is_empty() {
        return Err(DurationError::NotAnInteger {
            digits: digits.to_string(),
            input: text.to_string(),
        });
    }
    let millis_per = match unit {
        "ms" => 1u64,
        "s" => 1_000,
        "m" => 60_000,
        // A decimal point lands here as part of the "unit", which reads
        // worse than saying the number is the problem.
        _ if unit.starts_with('.') => {
            return Err(DurationError::NotAnInteger {
                digits: text.to_string(),
                input: text.to_string(),
            });
        }
        _ => {
            return Err(DurationError::UnknownUnit {
                unit: unit.to_string(),
                input: text.to_string(),
            });
        }
    };
    let value: u64 = digits.parse().map_err(|_| DurationError::Overflow {
        input: text.to_string(),
    })?;
    // overflow-checks is on even in release, so a wrap here would be a
    // panic in a fullscreen tool rather than a wrong number.
    let millis = value
        .checked_mul(millis_per)
        .ok_or_else(|| DurationError::Overflow {
            input: text.to_string(),
        })?;
    Ok(Duration::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_units_parse() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
    }

    #[test]
    fn zero_is_syntax_not_policy() {
        // The parser owns the grammar; `wait` decides whether a zero
        // interval is a sensible thing to ask for.
        assert_eq!(parse_duration("0s").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0ms").unwrap(), Duration::ZERO);
    }

    #[test]
    fn a_bare_number_is_refused_rather_than_assumed() {
        assert_eq!(
            parse_duration("30").unwrap_err(),
            DurationError::NoUnit { input: "30".into() },
            "seconds to one reader, milliseconds to another"
        );
    }

    #[test]
    fn an_empty_string_says_what_to_write() {
        assert_eq!(parse_duration("").unwrap_err(), DurationError::Empty);
        let text = DurationError::Empty.to_string();
        assert!(text.contains("30s") && text.contains("500ms"));
    }

    #[test]
    fn decimals_are_named_as_a_number_problem_not_a_unit_one() {
        assert_eq!(
            parse_duration("1.5s").unwrap_err(),
            DurationError::NotAnInteger {
                digits: "1.5s".into(),
                input: "1.5s".into(),
            },
            "the unit is fine; the quantity is not"
        );
    }

    #[test]
    fn unknown_units_list_the_ones_that_work() {
        for bad in ["30sec", "2h", "5d", "30S", "1M"] {
            let err = parse_duration(bad).unwrap_err();
            assert!(
                matches!(err, DurationError::UnknownUnit { .. }),
                "{bad}: {err}"
            );
            assert!(err.to_string().contains("ms, s, or m"));
        }
    }

    #[test]
    fn compounds_and_whitespace_are_refused() {
        // `1m30s` would need a grammar, and a half-parsed duration is
        // worse than a rejected one.
        assert!(parse_duration("1m30s").is_err());
        assert!(parse_duration("30 s").is_err());
        assert!(parse_duration(" 30s").is_err());
        assert!(parse_duration("30s ").is_err());
    }

    #[test]
    fn a_leading_unit_or_sign_is_not_a_duration() {
        assert!(matches!(
            parse_duration("ms").unwrap_err(),
            DurationError::NotAnInteger { .. }
        ));
        assert!(
            parse_duration("-5s").is_err(),
            "durations do not run backwards"
        );
    }

    #[test]
    fn values_that_cannot_be_represented_are_refused_not_wrapped() {
        assert_eq!(
            parse_duration("99999999999999999999s").unwrap_err(),
            DurationError::Overflow {
                input: "99999999999999999999s".into()
            },
            "too large for u64 before the unit is even applied"
        );
        // Fits u64 as a number, overflows once scaled to milliseconds.
        assert!(matches!(
            parse_duration("18446744073709551m").unwrap_err(),
            DurationError::Overflow { .. }
        ));
    }
}
