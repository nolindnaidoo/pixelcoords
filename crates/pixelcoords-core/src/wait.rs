//! Blocking until a region matches or stops matching — the decisions
//! behind `pixelcoords wait`.
//!
//! The loop itself lives in the binary, because polling needs a capture.
//! What lives here is everything that decides *when it ends*, and the
//! reason it lives here is that a clock would otherwise be load-bearing:
//! `--timeout` is turned into a **poll budget** once, up front, so the
//! loop counts rather than consults the time.
//!
//! That is not only about testability, though it does mean `wait` needs
//! no `Clock` trait and no injected sleep. A wall-clock deadline gives
//! the UI *fewer* chances exactly when the machine is slowest, because
//! more of the deadline goes to capturing — backwards for a
//! synchronization primitive. A budget gives the same number of chances
//! everywhere.

use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

/// One watched region's final state — a row of `wait`'s report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegionWatch {
    /// Index into `session.selections` — this row's identity.
    pub index: usize,
    pub label: String,
    pub monitor: usize,
    /// Correlation with the saved crop at the last poll.
    pub score: f64,
    /// Whether that score cleared the floor. Reported per region because
    /// the aggregate cannot say *which* one held things up.
    pub matching: bool,
}

/// What `wait` is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    /// Every targeted region matches its saved crop again.
    Match,
    /// Any targeted region has stopped matching — "tell me when something
    /// happens here".
    Change,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WaitError {
    #[error("--interval 0 would poll as fast as capture allows — pass a nonzero interval")]
    ZeroInterval,
    #[error(
        "--interval {interval:?} is longer than --timeout {timeout:?}, so nothing \
         would be polled twice — shorten the interval or lengthen the timeout"
    )]
    IntervalExceedsTimeout {
        interval: Duration,
        timeout: Duration,
    },
}

/// How many polls a timeout allows.
///
/// The first poll is immediate, so `30s` at `500ms` allows 61: one at
/// zero, then sixty more. Capture time is deliberately *not* counted —
/// see the module note, and say so in the docs, because it means the wall
/// clock exceeds `--timeout` by roughly the cost of the captures.
pub fn poll_budget(timeout: Duration, interval: Duration) -> Result<u32, WaitError> {
    if interval.is_zero() {
        return Err(WaitError::ZeroInterval);
    }
    if interval > timeout {
        return Err(WaitError::IntervalExceedsTimeout { interval, timeout });
    }
    let spans = timeout.as_millis() / interval.as_millis();
    // Saturating rather than wrapping: a 2m timeout at 1ms is 120_001
    // polls, well inside u32, and nothing sensible reaches the ceiling.
    Ok(u32::try_from(spans).unwrap_or(u32::MAX).saturating_add(1))
}

/// Whether one poll's scores end the wait.
///
/// `match` needs every region at or above the floor; `change` fires on
/// the first region below it. Those are the semantics that make each verb
/// useful: waiting for a screen to settle means all of it, and waiting
/// for something to happen means any of it.
///
/// No regions satisfies neither — a wait that verified nothing has not
/// succeeded.
#[must_use]
pub fn satisfied(condition: Condition, scores: &[f64], min_score: f64) -> bool {
    if scores.is_empty() {
        return false;
    }
    match condition {
        Condition::Match => scores.iter().all(|s| *s >= min_score),
        Condition::Change => scores.iter().any(|s| *s < min_score),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_budget_is_the_one_computed() {
        // The number the docs promise, so the promise is checked.
        assert_eq!(
            poll_budget(Duration::from_secs(30), Duration::from_millis(500)).unwrap(),
            61,
            "one immediate poll, then sixty"
        );
    }

    #[test]
    fn an_interval_equal_to_the_timeout_still_polls_twice() {
        assert_eq!(
            poll_budget(Duration::from_secs(5), Duration::from_secs(5)).unwrap(),
            2,
            "once now, once at the deadline"
        );
    }

    #[test]
    fn a_ragged_division_keeps_the_polls_that_fit() {
        // 1s / 300ms = 3 whole intervals, plus the immediate one.
        assert_eq!(
            poll_budget(Duration::from_secs(1), Duration::from_millis(300)).unwrap(),
            4
        );
    }

    #[test]
    fn a_zero_interval_is_refused_rather_than_spinning() {
        assert_eq!(
            poll_budget(Duration::from_secs(1), Duration::ZERO).unwrap_err(),
            WaitError::ZeroInterval
        );
    }

    #[test]
    fn an_interval_past_the_timeout_is_refused_as_a_mistake() {
        let err = poll_budget(Duration::from_secs(1), Duration::from_secs(2)).unwrap_err();
        assert_eq!(
            err,
            WaitError::IntervalExceedsTimeout {
                interval: Duration::from_secs(2),
                timeout: Duration::from_secs(1),
            },
            "a single poll makes the timeout meaningless — say so"
        );
        assert!(err.to_string().contains("polled twice"));
    }

    #[test]
    fn match_needs_every_region_and_change_needs_one() {
        let all_high = [0.99, 0.95];
        let one_low = [0.99, 0.10];

        assert!(satisfied(Condition::Match, &all_high, 0.9));
        assert!(!satisfied(Condition::Match, &one_low, 0.9), "match is all");

        assert!(satisfied(Condition::Change, &one_low, 0.9), "change is any");
        assert!(!satisfied(Condition::Change, &all_high, 0.9));
    }

    #[test]
    fn the_floor_is_inclusive_on_both_verbs() {
        // A score exactly at the floor counts as matching, so the two
        // conditions stay exact complements for a single region.
        assert!(satisfied(Condition::Match, &[0.9], 0.9));
        assert!(!satisfied(Condition::Change, &[0.9], 0.9));
    }

    #[test]
    fn nothing_to_watch_satisfies_neither() {
        assert!(!satisfied(Condition::Match, &[], 0.9));
        assert!(
            !satisfied(Condition::Change, &[], 0.9),
            "a wait that verified nothing has not succeeded"
        );
    }
}
