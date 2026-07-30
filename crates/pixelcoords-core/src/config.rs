//! Configuration types (serde) and their resolution into validated values.
//!
//! The structs here are plain data the binary deserializes from TOML; this
//! crate stays parser-agnostic. Resolution is strict: bad colors, silly
//! thicknesses, and unknown hotkey pieces are errors, never silently
//! defaulted (the predecessor's silent numeric fallbacks were a bug class).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::draw::Color;
use crate::hotkeys::{Binding, HotkeyError, default_bindings};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid color '{0}': expected hex RGB, 3 or 6 digits, optional '#'")]
    Color(String),
    #[error("thickness {0} is out of range (0-512)")]
    Thickness(u32),
    #[error(transparent)]
    Hotkey(#[from] HotkeyError),
    #[error(
        "[capture] monitors: {0} — expected \"all\", or a monitor query \
         (an index, \"primary\", or part of a display name), or a list of them"
    )]
    Monitors(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub style: StyleConfig,
    pub hotkeys: Vec<HotkeyEntry>,
    pub capture: CaptureConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CaptureConfig {
    /// Which monitors a no-flag launch freezes. `None` (the table absent,
    /// or the key absent) means all of them — the launch default, and the
    /// product's thesis. This is the answer for a double-clicked binary,
    /// which has no terminal to pass `--monitor` on.
    pub monitors: Option<MonitorsSetting>,
}

/// `monitors = "primary"` and `monitors = ["DELL", "Built-in"]` are both
/// natural to write, so both parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MonitorsSetting {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StyleConfig {
    /// Outline color while dragging out a shape.
    pub preview_color: String,
    /// Outline color of committed shapes.
    pub complete_color: String,
    /// Label and HUD text color.
    pub label_color: String,
    /// Border color drawn around the `--target` window.
    pub target_color: String,
    /// Outline thickness in pixels; 0 hides outlines.
    pub thickness: u32,
    /// Fill shapes instead of outlining them.
    pub fill: bool,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            preview_color: "#00A0FF".into(),
            complete_color: "#00FF66".into(),
            label_color: "#FFFFFF".into(),
            target_color: "#FFB000".into(),
            thickness: 2,
            fill: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotkeyEntry {
    pub key: String,
    pub action: String,
    pub edge: Option<String>,
    pub when: Option<String>,
}

/// Validated, ready-to-use style values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub preview: Color,
    pub complete: Color,
    pub label: Color,
    pub target: Color,
    pub thickness: i32,
    pub fill: bool,
}

impl Config {
    pub fn resolve_style(&self) -> Result<Style, ConfigError> {
        let s = &self.style;
        if s.thickness > 512 {
            return Err(ConfigError::Thickness(s.thickness));
        }
        Ok(Style {
            preview: parse_hex_color(&s.preview_color)?,
            complete: parse_hex_color(&s.complete_color)?,
            label: parse_hex_color(&s.label_color)?,
            target: parse_hex_color(&s.target_color)?,
            thickness: s.thickness as i32,
            fill: s.fill,
        })
    }

    /// The monitor queries a no-flag launch should use, or an empty vec
    /// meaning all of them.
    ///
    /// Only the *shape* is checked here — this crate has no idea what is
    /// plugged in. A query that names no attached display fails at launch
    /// with the same message `--monitor` gives, which is the honest place
    /// for it: the answer depends on the hardware, not the file. What is
    /// rejected here is a value that could never mean anything on any
    /// machine, because a silent default is the bug class this module
    /// exists to avoid.
    pub fn resolve_monitors(&self) -> Result<Vec<String>, ConfigError> {
        let raw = match &self.capture.monitors {
            None => return Ok(Vec::new()),
            Some(MonitorsSetting::One(one)) => vec![one.clone()],
            Some(MonitorsSetting::Many(many)) => {
                if many.is_empty() {
                    return Err(ConfigError::Monitors("the list is empty".into()));
                }
                many.clone()
            }
        };
        // "all" is the launch default said out loud, and only means that on
        // its own — in a list it would be a display name, which is a
        // contradiction worth naming rather than resolving.
        if raw.len() == 1 && raw[0].trim().eq_ignore_ascii_case("all") {
            return Ok(Vec::new());
        }
        for query in &raw {
            if query.trim().is_empty() {
                return Err(ConfigError::Monitors("an entry is empty".into()));
            }
            if query.trim().eq_ignore_ascii_case("all") {
                return Err(ConfigError::Monitors(
                    "\"all\" cannot be combined with other monitors".into(),
                ));
            }
        }
        Ok(raw)
    }

    /// Defaults, then config-file entries, then `extra` (CLI `--bind`).
    /// Binding any key removes ALL default bindings for that key (every
    /// edge), so rebinding `[` doesn't leave the default's repeat-edge
    /// rotation alive; among user bindings, later entries shadow earlier
    /// ones per key + edge.
    pub fn resolve_bindings(&self, extra: &[String]) -> Result<Vec<Binding>, ConfigError> {
        let mut user: Vec<Binding> = Vec::new();
        for entry in &self.hotkeys {
            let mut spec = format!("{}={}", entry.key, entry.action);
            for part in [&entry.edge, &entry.when].into_iter().flatten() {
                spec.push(',');
                spec.push_str(part);
            }
            user.push(Binding::parse(&spec)?);
        }
        for spec in extra {
            user.push(Binding::parse(spec)?);
        }
        let user_keys: std::collections::HashSet<_> = user.iter().map(|b| b.key).collect();
        let mut bindings: Vec<Binding> = default_bindings()
            .into_iter()
            .filter(|b| !user_keys.contains(&b.key))
            .collect();
        bindings.extend(user);
        Ok(bindings)
    }
}

/// Parse `RGB`/`RRGGBB` with optional `#`, matching the predecessor's rules.
pub fn parse_hex_color(input: &str) -> Result<Color, ConfigError> {
    let s = input
        .trim()
        .strip_prefix('#')
        .unwrap_or_else(|| input.trim());
    let expanded: String = match s.len() {
        3 => s.chars().flat_map(|c| [c, c]).collect(),
        6 => s.to_string(),
        _ => return Err(ConfigError::Color(input.to_string())),
    };
    if !expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ConfigError::Color(input.to_string()));
    }
    let channel = |range| u8::from_str_radix(&expanded[range], 16).unwrap_or_default();
    Ok(Color {
        r: channel(0..2),
        g: channel(2..4),
        b: channel(4..6),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkeys::{Action, Edge, KeyName, OverlayState, match_event};

    #[test]
    fn hex_six_digit_with_hash() {
        assert_eq!(
            parse_hex_color("#FF8000").unwrap(),
            Color {
                r: 255,
                g: 128,
                b: 0
            }
        );
    }

    #[test]
    fn hex_without_hash_and_lowercase() {
        assert_eq!(
            parse_hex_color("00a0ff").unwrap(),
            Color {
                r: 0,
                g: 160,
                b: 255
            }
        );
    }

    #[test]
    fn hex_three_digit_expands() {
        assert_eq!(
            parse_hex_color("#F80").unwrap(),
            Color {
                r: 255,
                g: 136,
                b: 0
            }
        );
    }

    #[test]
    fn hex_rejects_bad_input() {
        for bad in ["", "#", "12345", "1234567", "GGGGGG", "#12 456"] {
            assert!(parse_hex_color(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    fn capture(toml: &str) -> Result<Vec<String>, ConfigError> {
        let cfg: Config = ::toml::from_str(toml).expect("parses");
        cfg.resolve_monitors()
    }

    #[test]
    fn no_capture_table_means_every_monitor() {
        assert!(capture("").unwrap().is_empty());
        assert!(capture("[capture]\n").unwrap().is_empty());
    }

    #[test]
    fn all_is_the_launch_default_said_out_loud() {
        assert!(
            capture("[capture]\nmonitors = \"all\"\n")
                .unwrap()
                .is_empty()
        );
        assert!(
            capture("[capture]\nmonitors = \"ALL\"\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_single_query_and_a_list_both_parse() {
        assert_eq!(
            capture("[capture]\nmonitors = \"primary\"\n").unwrap(),
            vec!["primary".to_string()]
        );
        assert_eq!(
            capture("[capture]\nmonitors = [\"DELL\", \"Built-in\"]\n").unwrap(),
            vec!["DELL".to_string(), "Built-in".to_string()]
        );
    }

    #[test]
    fn empty_and_contradictory_values_are_errors_not_silent_defaults() {
        // The bug class this module exists to avoid: a value that means
        // nothing quietly becoming "freeze everything".
        assert!(capture("[capture]\nmonitors = \"\"\n").is_err());
        assert!(capture("[capture]\nmonitors = \"   \"\n").is_err());
        assert!(capture("[capture]\nmonitors = []\n").is_err());
        assert!(capture("[capture]\nmonitors = [\"DELL\", \"\"]\n").is_err());
        // "all" is only the default on its own; alongside a name it is a
        // contradiction rather than a display.
        assert!(capture("[capture]\nmonitors = [\"all\", \"DELL\"]\n").is_err());
    }

    #[test]
    fn an_unknown_capture_key_is_refused_like_every_other_table() {
        assert!(::toml::from_str::<Config>("[capture]\nmonitor = \"primary\"\n").is_err());
    }

    #[test]
    fn default_config_resolves() {
        let cfg = Config::default();
        let style = cfg.resolve_style().unwrap();
        assert_eq!(style.thickness, 2);
        assert!(!style.fill);
        assert_eq!(
            style.label,
            Color {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    #[test]
    fn thickness_out_of_range_errors() {
        let mut cfg = Config::default();
        cfg.style.thickness = 513;
        assert_eq!(
            cfg.resolve_style().unwrap_err(),
            ConfigError::Thickness(513)
        );
    }

    #[test]
    fn toml_round_trip_and_hotkey_merge() {
        let toml_src = r##"
            [style]
            preview_color = "#F00"
            thickness = 4

            [[hotkeys]]
            key = "x"
            action = "save"
            when = "has_selection"
        "##;
        let cfg: Config = toml::from_str(toml_src).unwrap();
        let style = cfg.resolve_style().unwrap();
        assert_eq!(style.preview, Color { r: 255, g: 0, b: 0 });
        assert_eq!(style.thickness, 4);
        // Unspecified fields keep defaults.
        assert!(!style.fill);

        let bindings = cfg.resolve_bindings(&[]).unwrap();
        let state = OverlayState {
            has_selection: true,
            cursor_in_shape: false,
        };
        assert_eq!(
            match_event(&bindings, KeyName::Character('X'), Edge::Press, state),
            Some(Action::Save)
        );
    }

    #[test]
    fn unknown_toml_field_is_rejected() {
        let err = toml::from_str::<Config>("[style]\npreview_colour = \"#F00\"\n");
        assert!(err.is_err());
    }

    #[test]
    fn rebinding_a_key_removes_all_its_default_edges() {
        // 'Q' has press AND repeat defaults for rotate_ccw; rebinding it
        // must silence both, not leave the repeat default alive.
        let cfg = Config::default();
        let bindings = cfg.resolve_bindings(&["q=next_tool".to_string()]).unwrap();
        let state = OverlayState {
            cursor_in_shape: true,
            ..OverlayState::default()
        };
        assert_eq!(
            match_event(&bindings, KeyName::Character('Q'), Edge::Press, state),
            Some(Action::NextTool)
        );
        assert_eq!(
            match_event(&bindings, KeyName::Character('Q'), Edge::Repeat, state),
            None,
            "repeat-edge default must be gone"
        );
        // Untouched keys keep their defaults.
        assert_eq!(
            match_event(&bindings, KeyName::Character('E'), Edge::Repeat, state),
            Some(Action::RotateCw)
        );
    }

    #[test]
    fn cli_bind_shadows_defaults() {
        let cfg = Config::default();
        let bindings = cfg.resolve_bindings(&["q=undo".to_string()]).unwrap();
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
    fn bad_hotkey_entry_is_an_error() {
        let mut cfg = Config::default();
        cfg.hotkeys.push(HotkeyEntry {
            key: "z".into(),
            action: "teleport".into(),
            edge: None,
            when: None,
        });
        assert!(matches!(
            cfg.resolve_bindings(&[]),
            Err(ConfigError::Hotkey(_))
        ));
    }
}
