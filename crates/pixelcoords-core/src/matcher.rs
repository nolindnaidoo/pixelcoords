//! Window-target matching for `--target`: given the windows visible at
//! freeze time, deterministically pick the one the query means.
//!
//! Policy (ported in spirit from the predecessor's exact-or-substring
//! matcher): all comparison is case-insensitive; better match kinds always
//! beat worse ones; within a kind the front-most window (highest z) wins,
//! then the lowest enumeration index. Title matches outrank app-name
//! matches so `--target "Notepad"` prefers a window *titled* Notepad over
//! any window merely owned by Notepad.exe.

/// A visible window at freeze time, in enumeration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    pub title: String,
    pub app: String,
    /// Stacking order; higher is closer to the front.
    pub z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchKind {
    AppContains,
    AppExact,
    TitleContains,
    TitleStartsWith,
    TitleExact,
}

fn match_kind(query: &str, candidate: &WindowCandidate) -> Option<MatchKind> {
    let q = query.to_lowercase();
    let title = candidate.title.to_lowercase();
    let app = candidate.app.to_lowercase();
    // Ranked policy, best match first — each guard returns as soon as it
    // applies.
    if title == q {
        return Some(MatchKind::TitleExact);
    }
    if title.starts_with(&q) {
        return Some(MatchKind::TitleStartsWith);
    }
    if title.contains(&q) {
        return Some(MatchKind::TitleContains);
    }
    if app == q {
        return Some(MatchKind::AppExact);
    }
    if app.contains(&q) {
        return Some(MatchKind::AppContains);
    }
    None
}

/// Pick the best candidate for `query`. Returns the index into
/// `candidates`, or `None` when nothing matches. Empty queries match
/// nothing.
pub fn select(query: &str, candidates: &[WindowCandidate]) -> Option<usize> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            match_kind(query, c).map(|kind| ((kind, c.z, std::cmp::Reverse(i)), i))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(title: &str, app: &str, z: i32) -> WindowCandidate {
        WindowCandidate {
            title: title.into(),
            app: app.into(),
            z,
        }
    }

    #[test]
    fn exact_title_beats_substring() {
        let candidates = [
            win("Notepad - notes.txt", "Notepad", 5),
            win("Notepad", "Notepad", 1),
        ];
        assert_eq!(select("notepad", &candidates), Some(1));
    }

    #[test]
    fn title_match_beats_app_match() {
        let candidates = [
            win("Untitled", "Safari", 9),
            win("Safari release notes", "TextEdit", 1),
        ];
        assert_eq!(select("safari", &candidates), Some(1));
    }

    #[test]
    fn front_most_wins_within_a_kind() {
        let candidates = [
            win("report draft", "Word", 1),
            win("report final", "Word", 7),
            win("report old", "Word", 3),
        ];
        assert_eq!(select("report", &candidates), Some(1));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let candidates = [win("My APP Window", "Thing", 0)];
        assert_eq!(select("my app", &candidates), Some(0));
    }

    #[test]
    fn no_match_and_empty_query_return_none() {
        let candidates = [win("Something", "App", 0)];
        assert_eq!(select("zzz", &candidates), None);
        assert_eq!(select("", &candidates), None);
        assert_eq!(select("   ", &candidates), None);
        assert_eq!(select("x", &[]), None);
    }
}
