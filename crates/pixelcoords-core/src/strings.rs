//! User-facing overlay strings. Everything the overlay renders as text
//! comes from here so a language table can replace `EN` later without a
//! refactor. ASCII only — the embedded font covers printable ASCII.

/// One control-panel row: a short key column and what the key does.
pub type HintRow = (&'static str, &'static str);

#[derive(Debug, Clone, Copy)]
pub struct Strings {
    /// The marking controls, shown while not editing a label.
    pub hud_hint_rows: &'static [HintRow],
    /// The label editor's controls.
    pub hud_edit_rows: &'static [HintRow],
    pub hud_saved_prefix: &'static str,
    pub hud_save_failed_prefix: &'static str,
    pub hud_quit_unsaved: &'static str,
    /// Closing a window that still holds marks. The count goes between the
    /// two halves; deleting stays an explicit act, so this refuses rather
    /// than offering to discard.
    pub hud_release_blocked_prefix: &'static str,
    pub hud_release_blocked_suffix: &'static str,
    /// A window closed while others remain: that display is live again.
    pub hud_released: &'static str,
    /// A release asked for mid-gesture. Refused rather than fatal.
    pub hud_release_busy: &'static str,
    /// Edge snapping toggled on or off for the rest of the run.
    pub hud_snap_on: &'static str,
    pub hud_snap_off: &'static str,
    /// The panel row's key column for the snap toggle, and the two states
    /// its action column shows.
    pub hud_snap_row_key: &'static str,
    pub hud_snap_row_on: &'static str,
    pub hud_snap_row_off: &'static str,
}

pub const EN: Strings = Strings {
    hud_hint_rows: &[
        ("drag", "draw / move / resize"),
        ("Shift", "lock ratio / 15 turns"),
        ("Q E", "rotate"),
        ("W", "tool"),
        ("A", "label"),
        ("S", "save"),
        ("D", "delete"),
        ("Z", "undo / Shift redo"),
        ("C", "cycle overlap"),
        ("Alt+drag", "duplicate"),
        ("arrows", "nudge (Shift 10, Alt size)"),
        ("M", "hold: loupe"),
        ("N", "name session"),
        ("X", "edge snap"),
        ("R", "release monitor"),
        ("3-9", "polygon sides"),
        ("Space", "hold: move panel"),
        ("H", "hide panel"),
        ("Esc", "quit"),
    ],
    hud_edit_rows: &[("type", "text"), ("Enter", "commit"), ("Esc", "cancel")],
    hud_saved_prefix: "Saved ",
    hud_save_failed_prefix: "Save failed: ",
    hud_quit_unsaved: "Unsaved work - S saves, Esc again quits",
    hud_release_blocked_prefix: "Monitor holds ",
    hud_release_blocked_suffix: " selections - delete them first",
    hud_released: "Monitor released",
    hud_release_busy: "Finish the drag first - then R releases the monitor",
    hud_snap_on: "SNAP ON",
    hud_snap_off: "SNAP OFF",
    hud_snap_row_key: "X",
    hud_snap_row_on: "edge snap: on",
    hud_snap_row_off: "edge snap: off",
};

/// `1 region`, `2 regions` — a count with its noun, pluralized by adding
/// an `s`.
///
/// Every noun this is used with pluralizes that way: region, selection,
/// session, point, poll. One that does not can be written by hand rather
/// than teaching this English.
///
/// Small, and worth having in one place. These counts appear in what the
/// MCP tools tell a model and in the resume picker a human reads, and
/// `1 selections` reads as a tool that is not paying attention — which is
/// the opposite of what a tool whose whole claim is careful measurement
/// wants to convey.
#[must_use]
pub fn count(n: usize, singular: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {singular}s")
    }
}

#[cfg(test)]
mod count_tests {
    use super::count;

    #[test]
    fn one_is_singular_and_the_rest_are_not() {
        assert_eq!(count(1, "region"), "1 region");
        assert_eq!(count(0, "region"), "0 regions");
        assert_eq!(count(2, "poll"), "2 polls");
    }
}
