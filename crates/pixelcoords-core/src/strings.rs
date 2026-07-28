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
        ("3-9", "polygon sides"),
        ("Space", "hold: move panel"),
        ("H", "hide panel"),
        ("Esc", "quit"),
    ],
    hud_edit_rows: &[("type", "text"), ("Enter", "commit"), ("Esc", "cancel")],
    hud_saved_prefix: "Saved ",
    hud_save_failed_prefix: "Save failed: ",
    hud_quit_unsaved: "Unsaved work - S saves, Esc again quits",
};
