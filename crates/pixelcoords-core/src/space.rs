//! Where a coordinate is measured from, and what units it is in.
//!
//! These are two independent questions and the CLI spells them as two
//! flags. An origin says which corner `(0, 0)` is; units say whether one
//! step is a device pixel or a logical point. Folding them into one
//! vocabulary — the shape `resolve` was first drafted with — cannot
//! express `--space monitor --units logical`, which is a perfectly
//! ordinary thing to ask for on a Retina secondary display.
//!
//! The arithmetic lives here; the monitor lookup does not. Each caller
//! finds its own monitor record and raises its own error for a missing
//! one, because a shared error type for that would belong to nobody.

use crate::geometry::Point;

/// Which origin a coordinate is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The desktop's own grid: `global_px`.
    Global,
    /// One monitor's top-left, by its index in the session: `px`.
    Monitor(usize),
    /// The `--target` window's top-left: `window_px`.
    Window,
}

impl Origin {
    /// The name this origin carries in JSON output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Monitor(_) => "monitor",
            Self::Window => "window",
        }
    }
}

/// The OS a coordinate will be handed to. Passed as a value rather than
/// read from `cfg!` so core stays platform-free and one headless test run
/// can cover all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

/// The units a coordinate is expressed in, as spelled on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Physical,
    Logical,
    /// Whatever this platform's input APIs expect. The one value most
    /// callers want, and the reason the flag exists: the mismatch it
    /// hides is where consumers of these coordinates go wrong.
    Auto,
}

/// `Units` with `Auto` already answered — the only thing the arithmetic
/// accepts, so "did I resolve auto?" cannot be forgotten at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    Physical,
    Logical,
}

impl Units {
    /// macOS input APIs speak logical points; Windows and X11 speak
    /// physical pixels. This is the same split `emit`'s per-format table
    /// documents, stated once.
    #[must_use]
    pub const fn resolve(self, platform: Platform) -> Resolved {
        match self {
            Self::Physical => Resolved::Physical,
            Self::Logical => Resolved::Logical,
            Self::Auto => match platform {
                Platform::MacOs => Resolved::Logical,
                Platform::Windows | Platform::Linux => Resolved::Physical,
            },
        }
    }
}

/// Physical pixels to logical points at `scale`.
///
/// Monitor origins were divided by this same per-monitor factor when the
/// session was written, so the conversion inverts cleanly even when two
/// displays disagree about scale.
#[must_use]
pub fn logical_of(physical: Point, scale: f64) -> Point {
    Point::new(
        (f64::from(physical.x) / scale).round() as i32,
        (f64::from(physical.y) / scale).round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_the_platforms_input_api() {
        assert_eq!(Units::Auto.resolve(Platform::MacOs), Resolved::Logical);
        assert_eq!(Units::Auto.resolve(Platform::Windows), Resolved::Physical);
        assert_eq!(Units::Auto.resolve(Platform::Linux), Resolved::Physical);
    }

    #[test]
    fn an_explicit_unit_ignores_the_platform() {
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            assert_eq!(Units::Physical.resolve(platform), Resolved::Physical);
            assert_eq!(Units::Logical.resolve(platform), Resolved::Logical);
        }
    }

    #[test]
    fn logical_halves_a_retina_point_and_leaves_scale_one_alone() {
        assert_eq!(logical_of(Point::new(100, 50), 2.0), Point::new(50, 25));
        assert_eq!(logical_of(Point::new(100, 50), 1.0), Point::new(100, 50));
    }

    #[test]
    fn logical_rounds_rather_than_truncating() {
        // 1.5 physical at scale 2 is 0.75 logical: 1, not 0. Truncation
        // would bias every odd coordinate toward the origin.
        assert_eq!(logical_of(Point::new(3, 3), 2.0), Point::new(2, 2));
        assert_eq!(logical_of(Point::new(-3, -3), 2.0), Point::new(-2, -2));
    }

    #[test]
    fn origins_label_themselves_for_json() {
        assert_eq!(Origin::Global.label(), "global");
        assert_eq!(Origin::Monitor(3).label(), "monitor");
        assert_eq!(Origin::Window.label(), "window");
    }
}
