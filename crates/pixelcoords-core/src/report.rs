//! The document every scoring command prints.
//!
//! `assert`, `find`, `resolve`, `wait`, and `diff` answer one class of
//! question — what is true on screen right now — and a caller that reads
//! two of them should not have to write two parsers. One envelope, one
//! schema counter, one place the aggregate answer lives.
//!
//! The aggregate is `ok`, and it is the only thing that moves up here.
//! Row-level answers stay on the rows: `assert --stdin` returns one
//! verdict per input line, and a caller scoring a trajectory needs to
//! know *which* click missed, not merely that one did.
//!
//! `doctor` and `windows` keep their own documents. They report on the
//! machine rather than on a session, and forcing them into `results[]`
//! would buy nothing.

use serde::Serialize;

/// The schema version of every document in this module.
///
/// Starts at 2 rather than 1: it replaces two independent counters that
/// were both sitting at 1, and a consumer that pinned either of them
/// would otherwise see a version it recognizes on a shape it does not.
pub const CLI_SCHEMA_VERSION: u32 = 2;

/// Which command produced a document, so a consumer reading a stored
/// report does not have to infer it from which fields are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Command {
    Assert,
    Find,
    Resolve,
    Wait,
    Diff,
}

/// A command's answer: the rows it produced, and whether they pass.
///
/// Generic over the row type rather than an enum of every command's
/// shape, so adding a command adds a row type instead of editing a type
/// every existing consumer matches on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report<T> {
    pub schema: u32,
    pub command: Command,
    /// When the frames behind this answer were taken. Absent for the
    /// commands that do not capture — `assert`, and `resolve` without
    /// `--relocate`. Supplied by the caller; this crate has no clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_utc: Option<String>,
    /// How many times the screen was polled to reach this answer, and how
    /// long that took. Present only for commands that loop — `wait` — and
    /// provenance in the same sense `captured_utc` is: they describe how
    /// the answer was obtained, never what it is.
    ///
    /// `elapsed_ms` is measured rather than derived. `wait` decides when
    /// to stop by counting polls, not by watching a clock, so the two do
    /// not imply each other: capture time is real and is not budgeted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// The aggregate the exit code mirrors: 0 when true, 1 when false.
    ///
    /// Each command computes this from its own rows, in its own module.
    /// What makes a command report failure is the most important line it
    /// has, and it belongs where a reader will look for it rather than in
    /// a trait implementation five files away.
    pub ok: bool,
    pub results: Vec<T>,
}

impl<T> Report<T> {
    /// A document from a command that captured.
    pub fn captured(command: Command, captured_utc: String, ok: bool, results: Vec<T>) -> Self {
        Self {
            schema: CLI_SCHEMA_VERSION,
            command,
            captured_utc: Some(captured_utc),
            polls: None,
            elapsed_ms: None,
            ok,
            results,
        }
    }

    /// A document from a command that answered from the session alone.
    pub fn offline(command: Command, ok: bool, results: Vec<T>) -> Self {
        Self {
            schema: CLI_SCHEMA_VERSION,
            command,
            captured_utc: None,
            polls: None,
            elapsed_ms: None,
            ok,
            results,
        }
    }

    /// Record how much polling produced this answer.
    ///
    /// Nothing calls this: `wait` sets `polls` and `elapsed_ms` on the
    /// fields directly. Kept for now rather than removed mid-patch,
    /// because dropping a public item from a published crate is a break
    /// and belongs in a minor.
    #[must_use]
    pub fn polled(mut self, polls: u32, elapsed_ms: u64) -> Self {
        self.polls = Some(polls);
        self.elapsed_ms = Some(elapsed_ms);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct Row {
        hit: bool,
    }

    #[test]
    fn a_captured_document_carries_its_timestamp() {
        let report = Report::captured(
            Command::Find,
            "2026-07-31T00:00:00Z".into(),
            true,
            vec![Row { hit: true }],
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema"], 2);
        assert_eq!(json["command"], "find");
        assert_eq!(json["captured_utc"], "2026-07-31T00:00:00Z");
        assert_eq!(json["ok"], true);
        assert_eq!(json["results"][0]["hit"], true);
    }

    #[test]
    fn an_offline_document_omits_the_timestamp_rather_than_nulling_it() {
        let report = Report::offline(Command::Assert, false, vec![Row { hit: false }]);
        let json = serde_json::to_value(&report).unwrap();
        assert!(
            json.get("captured_utc").is_none(),
            "a command that did not capture has no capture time to report, \
             and null would invite a consumer to parse one"
        );
        assert_eq!(json["command"], "assert");
        assert_eq!(json["ok"], false);
    }

    #[test]
    fn row_answers_survive_the_aggregate() {
        // The whole reason `hit` does not move up to `ok`: a caller
        // scoring a trajectory needs to know which click missed.
        let report = Report::offline(
            Command::Assert,
            false,
            vec![Row { hit: true }, Row { hit: false }],
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["results"][0]["hit"], true);
        assert_eq!(json["results"][1]["hit"], false);
    }

    #[test]
    fn every_command_serializes_lowercase() {
        for (command, name) in [
            (Command::Assert, "assert"),
            (Command::Find, "find"),
            (Command::Resolve, "resolve"),
            (Command::Wait, "wait"),
            (Command::Diff, "diff"),
        ] {
            assert_eq!(serde_json::to_value(command).unwrap(), name);
        }
    }

    #[test]
    fn only_a_polling_command_reports_polls() {
        // The two fields are provenance for the one command that loops.
        // Every other document must not grow them, or a consumer would
        // start expecting a poll count from `assert`.
        let still = Report::offline(Command::Assert, true, vec![Row { hit: true }]);
        let json = serde_json::to_value(&still).unwrap();
        assert!(json.get("polls").is_none() && json.get("elapsed_ms").is_none());

        let polled = Report::captured(Command::Wait, "t".into(), true, vec![Row { hit: true }])
            .polled(61, 30_412);
        let json = serde_json::to_value(&polled).unwrap();
        assert_eq!(json["polls"], 61);
        assert_eq!(
            json["elapsed_ms"], 30_412,
            "measured, not derived from the budget — capture time is real"
        );
    }

    #[test]
    fn an_empty_result_set_still_serializes_as_an_array() {
        let report: Report<Row> = Report::offline(Command::Diff, true, Vec::new());
        let json = serde_json::to_value(&report).unwrap();
        assert!(
            json["results"].is_array(),
            "a consumer indexing results[] must not have to handle null"
        );
    }
}
