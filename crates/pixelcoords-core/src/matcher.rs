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

/// How well a query matched a monitor name. Ranked worst to best, so
/// deriving `Ord` gives the comparison the selector wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NameMatch {
    Contains,
    StartsWith,
    Exact,
}

/// Pick the monitors whose names match `query`, best rank only.
///
/// Same discipline as [`select`] — case-insensitive, exact beats prefix
/// beats substring — but the return shape differs on purpose. Windows are
/// stacked, so `select` can always break a tie with z-order and hand back a
/// single winner. Monitors have no such ordering: two displays matching a
/// query equally well is a question only the user can answer, and silently
/// picking one would freeze the wrong screen. So this returns **every**
/// candidate at the best rank and leaves the caller to reject more than one.
///
/// Indices are into `names`, in the order given. An empty query matches
/// nothing, as with windows.
pub fn select_monitors(query: &str, names: &[String]) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let ranked: Vec<(NameMatch, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let name = name.to_lowercase();
            if name == query {
                return Some((NameMatch::Exact, i));
            }
            if name.starts_with(&query) {
                return Some((NameMatch::StartsWith, i));
            }
            if name.contains(&query) {
                return Some((NameMatch::Contains, i));
            }
            None
        })
        .collect();
    let Some(best) = ranked.iter().map(|(kind, _)| *kind).max() else {
        return Vec::new();
    };
    ranked
        .into_iter()
        .filter(|(kind, _)| *kind == best)
        .map(|(_, i)| i)
        .collect()
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

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn monitor_exact_beats_prefix_beats_substring() {
        let all = names(&["DELL U2723QE Secondary", "DELL U2723QE", "Old DELL U2723QE"]);
        assert_eq!(select_monitors("dell u2723qe", &all), vec![1]);
    }

    #[test]
    fn a_monitor_prefix_beats_a_substring() {
        let all = names(&["Thunderbolt Display", "Built-in Thunderbolt"]);
        assert_eq!(select_monitors("thunderbolt", &all), vec![0]);
    }

    #[test]
    fn monitor_matching_is_case_insensitive_and_trims() {
        let all = names(&["Built-in Retina Display"]);
        assert_eq!(select_monitors("  BUILT-IN  ", &all), vec![0]);
    }

    #[test]
    fn every_equally_good_monitor_comes_back_so_the_caller_can_refuse() {
        // Two identical panels, one query, no z-order to separate them.
        // Returning both is the point: picking one would freeze the wrong
        // screen and never say so.
        let all = names(&["DELL U2723QE", "DELL U2723QE"]);
        assert_eq!(select_monitors("dell", &all), vec![0, 1]);
    }

    #[test]
    fn a_better_rank_still_wins_over_several_worse_ones() {
        // Ambiguity is only ambiguity at the same rank — an exact match
        // resolves a query that three substrings also matched.
        let all = names(&["DELL U2723QE Left", "DELL", "DELL U2723QE Right"]);
        assert_eq!(select_monitors("dell", &all), vec![1]);
    }

    #[test]
    fn no_monitor_match_and_empty_query_return_nothing() {
        let all = names(&["Built-in Retina Display"]);
        assert!(select_monitors("zzz", &all).is_empty());
        assert!(select_monitors("", &all).is_empty());
        assert!(select_monitors("   ", &all).is_empty());
        assert!(select_monitors("x", &[]).is_empty());
    }
}
