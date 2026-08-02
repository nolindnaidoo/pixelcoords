//! Configuration types (serde) and their resolution into validated values.
//!
//! The structs here are plain data the binary deserializes from TOML; this
//! crate stays parser-agnostic. Resolution is strict: bad colors, silly
//! thicknesses, and unknown hotkey pieces are errors, never silently
//! defaulted (the predecessor's silent numeric fallbacks were a bug class).

use std::time::Duration;

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
    #[error("[snap] radius {0} is out of range (1-64 logical pixels)")]
    SnapRadius(u32),
    #[error(
        "[limits] label_length {0} is out of range (1-{max}); a label becomes part of a \
         crop's filename, and past {max} the name can outgrow what a filesystem accepts",
        max = crate::session::MAX_LABEL_LEN
    )]
    LabelLength(usize),
    #[error("[overlay] {field} {value} is out of range ({low}-{high})")]
    Overlay {
        field: &'static str,
        value: u64,
        low: u64,
        high: u64,
    },
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
    pub snap: SnapConfig,
    pub limits: LimitsConfig,
    pub overlay: OverlayConfig,
}

/// Constraints on what a session may hold.
///
/// Separate from [`OverlayConfig`] because these bound the *data*, not the
/// feel: a label that is too long is a filename the save cannot write,
/// whichever way it was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// How many characters a label may hold.
    ///
    /// The default is 64 and the ceiling is
    /// [`crate::session::MAX_LABEL_LEN`], which is derived from what a
    /// filename can be rather than chosen. Lower it if you want shorter
    /// crop names; there is no reason to, and no harm either.
    pub label_length: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self { label_length: 64 }
    }
}

/// How the overlay feels: reach, magnification, and how long it talks.
///
/// None of these change what is saved. They exist because the right
/// number is a matter of hand, screen, and eyesight rather than something
/// this project can pick for everyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OverlayConfig {
    /// Sides the polygon tool starts on.
    ///
    /// The digit keys reach 3 to 9 because that is what one keypress can
    /// say. This is how you get anything else: set it here and the tool
    /// opens on it.
    pub polygon_sides: u32,
    /// How close to a border counts as grabbing it, in logical pixels,
    /// scaled per monitor. Larger is easier on a trackpad and makes small
    /// shapes harder to click inside.
    pub grab_tolerance: u32,
    /// The magnifier's source radius: it shows a `2r+1` pixel square, so
    /// larger means more context at less magnification.
    pub loupe_radius: u32,
    /// How long a message stays on screen — saves, errors, the things
    /// worth reading twice.
    pub flash_ms: u64,
    /// How long the brief messages stay: tool switches and the like, the
    /// ones confirming something you just did on purpose. Raise it if
    /// 1.2 seconds is not long enough to read comfortably.
    pub flash_brief_ms: u64,
    /// Caret blink interval in the label editor. **`0` stops the blink**
    /// and leaves the caret solid, which is a supported value rather than
    /// an accident of the arithmetic.
    pub caret_blink_ms: u64,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            polygon_sides: 6,
            grab_tolerance: 6,
            loupe_radius: 15,
            flash_ms: 2500,
            flash_brief_ms: 1200,
            caret_blink_ms: 500,
        }
    }
}

/// Edge snapping: whether it starts on, and how far it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnapConfig {
    /// The launch default. Snapping starts **on**: the radius is small
    /// enough that placement away from an edge is untouched, and the
    /// feature is worthless if it has to be discovered before it helps.
    /// The toggle key flips it live and does not persist — this file owns
    /// the default.
    pub enabled: bool,
    /// Search radius in **logical** pixels, scaled per monitor's DPI, so
    /// one config behaves the same on a Retina panel and a 1x one.
    pub radius: u32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            radius: 8,
        }
    }
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

/// Validated snapping settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSettings {
    pub enabled: bool,
    /// Logical pixels; the overlay multiplies by each monitor's UI scale.
    pub radius: i32,
}

impl Default for SnapSettings {
    /// What `SnapConfig::default()` resolves to, without going through
    /// the fallible path — the defaults are in range by construction.
    fn default() -> Self {
        Self {
            enabled: true,
            radius: 8,
        }
    }
}

/// Validated overlay comfort values, in the units the overlay uses.
///
/// `caret_blink` is an `Option` rather than a zero duration: "do not
/// blink" is a different instruction from "blink every zero
/// milliseconds", and the type says which one it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySettings {
    pub polygon_sides: u32,
    pub grab_tolerance: i32,
    pub loupe_radius: i32,
    pub flash: Duration,
    pub flash_brief: Duration,
    pub caret_blink: Option<Duration>,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        OverlayConfig::default()
            .into_settings()
            .expect("the defaults are in range by construction")
    }
}

impl OverlayConfig {
    fn into_settings(self) -> Result<OverlaySettings, ConfigError> {
        Config {
            overlay: self,
            ..Config::default()
        }
        .resolve_overlay()
    }
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

    /// Snapping settings, with the radius range-checked.
    ///
    /// A radius of 0 would be a silently disabled feature and a huge one
    /// would drag the cursor across half the screen; both are more likely
    /// a typo than an intent, and this module's whole premise is that a
    /// nonsense number is an error rather than a quiet default.
    pub fn resolve_snap(&self) -> Result<SnapSettings, ConfigError> {
        if self.snap.radius == 0 || self.snap.radius > 64 {
            return Err(ConfigError::SnapRadius(self.snap.radius));
        }
        Ok(SnapSettings {
            enabled: self.snap.enabled,
            radius: i32::try_from(self.snap.radius).unwrap_or(64),
        })
    }

    /// Validated limits.
    pub fn resolve_limits(&self) -> Result<LimitsConfig, ConfigError> {
        let len = self.limits.label_length;
        if len == 0 || len > crate::session::MAX_LABEL_LEN {
            return Err(ConfigError::LabelLength(len));
        }
        Ok(self.limits)
    }

    /// Validated overlay comfort values.
    ///
    /// Every bound here is a range a person could plausibly want, widened
    /// generously — the point of the table is to stop the tool deciding
    /// for you, so the checks exist to catch a typo rather than to hold an
    /// opinion. `caret_blink_ms` starts at 0 on purpose: 0 means "do not
    /// blink", which is the one value someone may actively need.
    pub fn resolve_overlay(&self) -> Result<OverlaySettings, ConfigError> {
        let o = self.overlay;
        let check = |field, value: u64, low: u64, high: u64| {
            if value < low || value > high {
                return Err(ConfigError::Overlay {
                    field,
                    value,
                    low,
                    high,
                });
            }
            Ok(())
        };
        check(
            "polygon_sides",
            u64::from(o.polygon_sides),
            u64::from(crate::geometry::MIN_POLYGON_SIDES),
            u64::from(crate::geometry::MAX_POLYGON_SIDES),
        )?;
        check("grab_tolerance", u64::from(o.grab_tolerance), 1, 256)?;
        check("loupe_radius", u64::from(o.loupe_radius), 1, 256)?;
        check("flash_ms", o.flash_ms, 1, 60_000)?;
        check("flash_brief_ms", o.flash_brief_ms, 1, 60_000)?;
        check("caret_blink_ms", o.caret_blink_ms, 0, 60_000)?;
        Ok(OverlaySettings {
            polygon_sides: o.polygon_sides,
            grab_tolerance: i32::try_from(o.grab_tolerance).unwrap_or(6),
            loupe_radius: i32::try_from(o.loupe_radius).unwrap_or(15),
            flash: Duration::from_millis(o.flash_ms),
            flash_brief: Duration::from_millis(o.flash_brief_ms),
            caret_blink: (o.caret_blink_ms > 0).then(|| Duration::from_millis(o.caret_blink_ms)),
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

    #[test]
    fn the_new_tables_default_to_todays_behavior() {
        // The whole point of a default is that an absent table changes
        // nothing, so these are the constants they replaced.
        let c = Config::default();
        assert_eq!(c.resolve_limits().unwrap().label_length, 64);
        let o = c.resolve_overlay().unwrap();
        assert_eq!(o.polygon_sides, 6);
        assert_eq!(o.grab_tolerance, 6);
        assert_eq!(o.loupe_radius, 15);
        assert_eq!(o.flash, Duration::from_millis(2500));
        assert_eq!(o.flash_brief, Duration::from_millis(1200));
        assert_eq!(o.caret_blink, Some(Duration::from_millis(500)));
    }

    #[test]
    fn an_absent_table_is_the_default_not_an_error() {
        let c: Config = toml::from_str("[style]\nthickness = 3\n").unwrap();
        assert_eq!(c.limits, LimitsConfig::default());
        assert_eq!(c.overlay, OverlayConfig::default());
    }

    #[test]
    fn a_zero_caret_blink_means_do_not_blink() {
        // Not an error and not a zero-length interval: the one value
        // someone may actively need, so it has to be expressible.
        let c: Config = toml::from_str("[overlay]\ncaret_blink_ms = 0\n").unwrap();
        assert_eq!(c.resolve_overlay().unwrap().caret_blink, None);
    }

    #[test]
    fn a_polygon_default_past_the_digit_keys_is_allowed() {
        // The reason this knob exists: 3..=9 is what one keypress can
        // say, and this is how anything else is reachable.
        let c: Config = toml::from_str("[overlay]\npolygon_sides = 24\n").unwrap();
        assert_eq!(c.resolve_overlay().unwrap().polygon_sides, 24);
    }

    #[test]
    fn out_of_range_overlay_values_are_errors_naming_the_field() {
        let cases = [
            ("polygon_sides = 2", "polygon_sides"),
            ("polygon_sides = 100000", "polygon_sides"),
            ("grab_tolerance = 0", "grab_tolerance"),
            ("loupe_radius = 0", "loupe_radius"),
            ("flash_ms = 0", "flash_ms"),
            ("flash_brief_ms = 999999", "flash_brief_ms"),
            ("caret_blink_ms = 999999", "caret_blink_ms"),
        ];
        for (line, field) in cases {
            let c: Config = toml::from_str(&format!("[overlay]\n{line}\n")).unwrap();
            let Err(err) = c.resolve_overlay() else {
                panic!("{line} was accepted");
            };
            let rendered = err.to_string();
            assert!(rendered.contains(field), "{rendered:?} omits {field}");
        }
    }

    #[test]
    fn a_label_length_past_what_a_filename_holds_is_refused() {
        for len in [0, crate::session::MAX_LABEL_LEN + 1] {
            let c: Config = toml::from_str(&format!("[limits]\nlabel_length = {len}\n")).unwrap();
            assert_eq!(c.resolve_limits(), Err(ConfigError::LabelLength(len)));
        }
        // The ceiling itself is allowed — a bound that rejects its own
        // limit is an off-by-one nobody thinks to test for.
        let c: Config = toml::from_str(&format!(
            "[limits]\nlabel_length = {}\n",
            crate::session::MAX_LABEL_LEN
        ))
        .unwrap();
        assert!(c.resolve_limits().is_ok());
    }

    #[test]
    fn the_new_tables_refuse_unknown_keys_like_every_other() {
        assert!(toml::from_str::<Config>("[limits]\nlabel_len = 4\n").is_err());
        assert!(toml::from_str::<Config>("[overlay]\nloupe = 4\n").is_err());
    }

    #[test]
    fn snap_defaults_are_on_with_a_small_radius() {
        let settings = Config::default().resolve_snap().unwrap();
        assert!(settings.enabled, "snapping is on unless turned off");
        assert_eq!(settings.radius, 8);
    }

    #[test]
    fn a_snap_radius_outside_the_range_is_an_error_not_a_default() {
        for radius in [0, 65, 10_000] {
            let mut config = Config::default();
            config.snap.radius = radius;
            assert_eq!(
                config.resolve_snap(),
                Err(ConfigError::SnapRadius(radius)),
                "radius {radius}"
            );
        }
    }

    #[test]
    fn the_snap_table_parses_and_rejects_unknown_keys() {
        let config: Config = toml::from_str("[snap]\nenabled = false\nradius = 16\n").unwrap();
        let settings = config.resolve_snap().unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.radius, 16);
        assert!(toml::from_str::<Config>("[snap]\nradius_px = 4\n").is_err());
    }

    #[test]
    fn an_absent_snap_table_is_the_default_not_an_error() {
        let config: Config = toml::from_str("[style]\nthickness = 3\n").unwrap();
        assert_eq!(config.snap, SnapConfig::default());
    }
}
