//! Hotkey binding grammar: `KEY=ACTION[,EDGE][,WHEN]`.
//!
//! Ported from the predecessor's config grammar, minus Win32 virtual-key
//! codes: keys are platform-neutral names the binary maps from its window
//! system's key events. Parsing is strict — unknown actions, edges, or
//! conditions are errors, not silently dropped.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyName {
    /// A single printable character, stored uppercase.
    Character(char),
    Tab,
    CapsLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    #[default]
    Press,
    Release,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    HasSelection,
    CursorInShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Save,
    NextTool,
    DeleteAtCursor,
    LabelEditAtCursor,
    Undo,
    /// Re-apply the most recently undone edit.
    Redo,
    /// Send the topmost shape under the cursor to the bottom of the
    /// stack, so overlapped shapes become reachable.
    CycleOverlap,
    /// Show or hide the control panel.
    TogglePanel,
    /// Open the session-name editor.
    NameSession,
    /// Rotate the shape under the cursor counterclockwise.
    RotateCcw,
    /// Rotate the shape under the cursor clockwise.
    RotateCw,
    /// Unfreeze the monitor under the cursor and close its overlay window,
    /// leaving the others frozen. The only way to reach this: the overlay
    /// windows are borderless and undecorated, so no close button exists
    /// and `CloseRequested` never fires from a user action.
    ReleaseMonitor,
    /// Turn edge snapping on or off for the rest of the run. The config
    /// owns the launch default; this does not persist.
    ToggleSnap,
    /// Accepted by the grammar for forward compatibility; snapshot mode has
    /// no themes, so the binary treats it as a no-op.
    NextTheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: KeyName,
    pub action: Action,
    pub edge: Edge,
    pub when: Option<When>,
}

/// Everything a binding condition can observe about the app.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayState {
    pub has_selection: bool,
    pub cursor_in_shape: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HotkeyError {
    #[error("binding '{0}' is not KEY=ACTION[,EDGE][,WHEN]")]
    Malformed(String),
    #[error("unknown key '{0}' (single character, 'tab', or 'capslock')")]
    UnknownKey(String),
    #[error("unknown action '{0}'")]
    UnknownAction(String),
    #[error("unknown edge '{0}' (press, release, or repeat)")]
    UnknownEdge(String),
    #[error("unknown condition '{0}' (has_selection or cursor_in)")]
    UnknownWhen(String),
}

pub fn parse_key(s: &str) -> Result<KeyName, HotkeyError> {
    let t = s.trim();
    match t.to_ascii_lowercase().as_str() {
        "tab" => Ok(KeyName::Tab),
        "capslock" | "caps_lock" | "caps" => Ok(KeyName::CapsLock),
        _ => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_whitespace() => {
                    Ok(KeyName::Character(c.to_ascii_uppercase()))
                }
                _ => Err(HotkeyError::UnknownKey(t.to_string())),
            }
        }
    }
}

pub fn parse_action(s: &str) -> Result<Action, HotkeyError> {
    match s.trim().to_ascii_lowercase().as_str() {
        "quit" => Ok(Action::Quit),
        "save" => Ok(Action::Save),
        "next_tool" => Ok(Action::NextTool),
        "delete_at_cursor" | "delete_selection_at_cursor" => Ok(Action::DeleteAtCursor),
        "label_edit_at_cursor" => Ok(Action::LabelEditAtCursor),
        "undo" => Ok(Action::Undo),
        "redo" => Ok(Action::Redo),
        "cycle_overlap" => Ok(Action::CycleOverlap),
        "toggle_panel" => Ok(Action::TogglePanel),
        "name_session" => Ok(Action::NameSession),
        "rotate_ccw" => Ok(Action::RotateCcw),
        "rotate_cw" => Ok(Action::RotateCw),
        "release_monitor" => Ok(Action::ReleaseMonitor),
        "toggle_snap" => Ok(Action::ToggleSnap),
        "next_theme" => Ok(Action::NextTheme),
        other => Err(HotkeyError::UnknownAction(other.to_string())),
    }
}

impl Binding {
    /// Parse one `KEY=ACTION[,EDGE][,WHEN]` spec. EDGE and WHEN may appear
    /// in either order, matching the predecessor's CLI.
    pub fn parse(spec: &str) -> Result<Self, HotkeyError> {
        let (key_part, rest) = spec
            .split_once('=')
            .ok_or_else(|| HotkeyError::Malformed(spec.to_string()))?;
        let mut parts = rest.split(',');
        let action_part = parts.next().unwrap_or_default();
        if action_part.trim().is_empty() {
            return Err(HotkeyError::Malformed(spec.to_string()));
        }
        let key = parse_key(key_part)?;
        let action = parse_action(action_part)?;
        let mut edge = Edge::default();
        let mut when = None;
        for part in parts {
            let t = part.trim().to_ascii_lowercase();
            match t.as_str() {
                "press" => edge = Edge::Press,
                "release" => edge = Edge::Release,
                "repeat" => edge = Edge::Repeat,
                "has_selection" => when = Some(When::HasSelection),
                "cursor_in" => when = Some(When::CursorInShape),
                "hold" | "down" | "up" => return Err(HotkeyError::UnknownEdge(t)),
                _ => return Err(HotkeyError::UnknownWhen(t)),
            }
        }
        Ok(Self {
            key,
            action,
            edge,
            when,
        })
    }

    const fn condition_met(self, state: OverlayState) -> bool {
        match self.when {
            None => true,
            Some(When::HasSelection) => state.has_selection,
            Some(When::CursorInShape) => state.cursor_in_shape,
        }
    }
}

/// Default bindings; user config and CLI `--bind` entries are appended after
/// these, and the *last* matching binding wins, so later sources override.
pub fn default_bindings() -> Vec<Binding> {
    [
        // The left hand covers everything, game-cluster style: QE turn,
        // WASD does the rest, Z undoes. Quit lives on Esc in the app, so
        // no letter is spent on it.
        "w=next_tool",
        "tab=next_tool",
        "a=label_edit_at_cursor,release,cursor_in",
        "s=save,has_selection",
        "d=delete_at_cursor,press,cursor_in",
        "z=undo",
        "c=cycle_overlap,press,cursor_in",
        "h=toggle_panel",
        "n=name_session",
        "x=toggle_snap",
        // The only trigger for releasing one display; see Action::ReleaseMonitor.
        "r=release_monitor",
        // Rotation binds press AND repeat so holding the key keeps turning.
        "q=rotate_ccw,press,cursor_in",
        "q=rotate_ccw,repeat,cursor_in",
        "e=rotate_cw,press,cursor_in",
        "e=rotate_cw,repeat,cursor_in",
    ]
    .into_iter()
    .map(|s| Binding::parse(s).expect("default bindings are valid"))
    .collect()
}

/// Resolve a key event against the binding list. Later bindings shadow
/// earlier ones for the same key + edge; a shadowing binding whose condition
/// fails suppresses the shadowed one rather than falling through.
pub fn match_event(
    bindings: &[Binding],
    key: KeyName,
    edge: Edge,
    state: OverlayState,
) -> Option<Action> {
    bindings
        .iter()
        .rev()
        .find(|b| b.key == key && b.edge == edge)
        .filter(|b| b.condition_met(state))
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_form() {
        let b = Binding::parse("E=label_edit_at_cursor,release,cursor_in").unwrap();
        assert_eq!(b.key, KeyName::Character('E'));
        assert_eq!(b.action, Action::LabelEditAtCursor);
        assert_eq!(b.edge, Edge::Release);
        assert_eq!(b.when, Some(When::CursorInShape));
    }

    #[test]
    fn edge_defaults_to_press() {
        let b = Binding::parse("q=quit").unwrap();
        assert_eq!(b.edge, Edge::Press);
        assert_eq!(b.when, None);
    }

    #[test]
    fn edge_and_when_order_is_flexible() {
        let a = Binding::parse("w=save,has_selection,release").unwrap();
        let b = Binding::parse("w=save,release,has_selection").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn key_is_case_insensitive_and_uppercased() {
        assert_eq!(parse_key("q").unwrap(), KeyName::Character('Q'));
        assert_eq!(parse_key("Q").unwrap(), KeyName::Character('Q'));
        assert_eq!(parse_key(" TAB ").unwrap(), KeyName::Tab);
        assert_eq!(parse_key("caps_lock").unwrap(), KeyName::CapsLock);
    }

    #[test]
    fn rejects_unknown_pieces() {
        assert_eq!(
            Binding::parse("qq=quit").unwrap_err(),
            HotkeyError::UnknownKey("qq".into())
        );
        assert_eq!(
            Binding::parse("q=fly").unwrap_err(),
            HotkeyError::UnknownAction("fly".into())
        );
        assert_eq!(
            Binding::parse("q=quit,hold").unwrap_err(),
            HotkeyError::UnknownEdge("hold".into())
        );
        assert_eq!(
            Binding::parse("q=quit,when_happy").unwrap_err(),
            HotkeyError::UnknownWhen("when_happy".into())
        );
        assert_eq!(
            Binding::parse("just_a_key").unwrap_err(),
            HotkeyError::Malformed("just_a_key".into())
        );
        assert_eq!(
            Binding::parse("q=").unwrap_err(),
            HotkeyError::Malformed("q=".into())
        );
    }

    #[test]
    fn legacy_action_alias_accepted() {
        assert_eq!(
            parse_action("delete_selection_at_cursor").unwrap(),
            Action::DeleteAtCursor
        );
    }

    #[test]
    fn match_requires_edge() {
        let bindings = default_bindings();
        let state = OverlayState::default();
        assert_eq!(
            match_event(&bindings, KeyName::Character('Z'), Edge::Press, state),
            Some(Action::Undo)
        );
        assert_eq!(
            match_event(&bindings, KeyName::Character('Z'), Edge::Release, state),
            None
        );
    }

    #[test]
    fn match_gates_on_conditions() {
        let bindings = default_bindings();
        let none = OverlayState::default();
        assert_eq!(
            match_event(&bindings, KeyName::Character('S'), Edge::Press, none),
            None
        );
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('S'),
                Edge::Press,
                OverlayState {
                    has_selection: true,
                    ..none
                }
            ),
            Some(Action::Save)
        );
        assert_eq!(
            match_event(&bindings, KeyName::Character('D'), Edge::Press, none),
            None
        );
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('D'),
                Edge::Press,
                OverlayState {
                    cursor_in_shape: true,
                    ..none
                }
            ),
            Some(Action::DeleteAtCursor)
        );
    }

    #[test]
    fn later_binding_shadows_earlier() {
        let mut bindings = default_bindings();
        bindings.push(Binding::parse("q=undo").unwrap());
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('Q'),
                Edge::Press,
                OverlayState::default()
            ),
            Some(Action::Undo)
        );
    }

    #[test]
    fn shadowing_binding_with_failed_condition_suppresses() {
        let mut bindings = default_bindings();
        bindings.push(Binding::parse("q=save,has_selection").unwrap());
        // The rebind of Q is conditional and the condition fails: Q does
        // nothing rather than falling back to quit.
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('Q'),
                Edge::Press,
                OverlayState::default()
            ),
            None
        );
    }

    #[test]
    fn rotation_defaults_fire_on_press_and_repeat() {
        let bindings = default_bindings();
        let state = OverlayState {
            cursor_in_shape: true,
            ..OverlayState::default()
        };
        for edge in [Edge::Press, Edge::Repeat] {
            assert_eq!(
                match_event(&bindings, KeyName::Character('Q'), edge, state),
                Some(Action::RotateCcw)
            );
            assert_eq!(
                match_event(&bindings, KeyName::Character('E'), edge, state),
                Some(Action::RotateCw)
            );
        }
        // Not over a shape: no rotation.
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('Q'),
                Edge::Press,
                OverlayState::default()
            ),
            None
        );
    }

    #[test]
    fn defaults_cover_expected_keys() {
        let bindings = default_bindings();
        assert_eq!(bindings.len(), 15);
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('R'),
                Edge::Press,
                OverlayState::default()
            ),
            Some(Action::ReleaseMonitor),
            "R releases a monitor — the only trigger, since undecorated \
             overlay windows have no close button"
        );
        // W and Tab both cycle the tool; Z undoes; quit is not in the
        // table at all — it lives on Esc in the app.
        for key in [KeyName::Character('W'), KeyName::Tab] {
            assert_eq!(
                match_event(&bindings, key, Edge::Press, OverlayState::default()),
                Some(Action::NextTool)
            );
        }
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('Z'),
                Edge::Press,
                OverlayState::default()
            ),
            Some(Action::Undo)
        );
        assert!(
            !bindings.iter().any(|b| b.action == Action::Quit),
            "quit is Esc's job, not a letter's"
        );
        assert_eq!(
            match_event(
                &bindings,
                KeyName::Character('X'),
                Edge::Press,
                OverlayState::default()
            ),
            Some(Action::ToggleSnap),
        );
    }
}
