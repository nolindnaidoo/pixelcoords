//! Screen capture behind one small trait: the real implementation is xcap
//! on every platform; `FakeCapture` exists so app logic stays testable in
//! headless CI. This is deliberately the binary's only trait.

use anyhow::{Context, Result};
use image::RgbaImage;
use pixelcoords_core::geometry::{Point, Size};

/// Which space xcap reports monitor/window origins and sizes in. Verified
/// against xcap 0.9 sources and the platform-spike CI artifacts: macOS uses
/// logical points (multiply by `scale` for physical); Windows (DEVMODE) and
/// Linux/X11 report physical pixels directly — multiplying those by scale
/// would double-scale on any `HiDPI` Windows display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordSpace {
    Logical,
    Physical,
}

impl CoordSpace {
    /// What to call a size reported in this space, for diagnostic output.
    /// `size_native` holds logical points on macOS and physical pixels on
    /// Windows and X11, so diagnostics have to say which they are showing —
    /// it is the number a user reads to check DPI behavior.
    pub const fn label(self) -> &'static str {
        match self {
            CoordSpace::Logical => "logical",
            CoordSpace::Physical => "physical",
        }
    }
}

/// The space the running platform's capture backend reports in.
pub const NATIVE_COORD_SPACE: CoordSpace = if cfg!(target_os = "macos") {
    CoordSpace::Logical
} else {
    CoordSpace::Physical
};

/// Convert a native-space coordinate to physical pixels.
/// Inverse of [`to_physical`]: a stored physical value back into the
/// platform's native space — resuming a session rebuilds `MonitorInfo`
/// from the physical records `session.json` keeps.
pub fn from_physical(v: i32, scale: f64, space: CoordSpace) -> i32 {
    match space {
        CoordSpace::Logical => (f64::from(v) / scale).round() as i32,
        CoordSpace::Physical => v,
    }
}

pub fn to_physical(v: i32, scale: f64, space: CoordSpace) -> i32 {
    match space {
        CoordSpace::Logical => (f64::from(v) * scale) as i32,
        CoordSpace::Physical => v,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Stable index into the enumeration order; selections reference this.
    pub index: usize,
    pub name: String,
    pub primary: bool,
    /// Desktop-global origin in the platform's native space
    /// (`NATIVE_COORD_SPACE`); use `origin_physical` for the normalized
    /// value.
    pub origin: Point,
    /// Monitor size in the platform's native space (differs from the
    /// captured image's physical size by `scale` on macOS).
    pub size_native: Size,
    pub scale: f64,
}

impl MonitorInfo {
    /// Desktop-global origin normalized to physical pixels.
    pub fn origin_physical(&self) -> Point {
        Point::new(
            to_physical(self.origin.x, self.scale, NATIVE_COORD_SPACE),
            to_physical(self.origin.y, self.scale, NATIVE_COORD_SPACE),
        )
    }

    /// Monitor size normalized to physical pixels.
    pub fn size_physical(&self) -> Size {
        Size::new(
            to_physical(self.size_native.w, self.scale, NATIVE_COORD_SPACE),
            to_physical(self.size_native.h, self.scale, NATIVE_COORD_SPACE),
        )
    }
}

/// Force exactly one monitor to carry the primary flag.
///
/// The backend reads this from `RandR`, which answers before a freshly
/// started Xwayland has designated a primary output — so the same desktop
/// reports no primary at all on one run and the right one moments later,
/// and every `session.json` written in between inherits the discrepancy.
///
/// A desktop always has exactly one primary. When the backend names none
/// (or names several), the monitor at the desktop origin is it: every
/// platform lays the virtual desktop out from the primary's top-left
/// corner, so the origin identifies it without asking the window system
/// anything.
fn normalize_primary(monitors: &mut [MonitorInfo]) {
    if monitors.iter().filter(|m| m.primary).count() == 1 {
        return;
    }
    let at_origin = monitors
        .iter()
        .position(|m| m.origin == Point::new(0, 0))
        .unwrap_or(0);
    for (index, monitor) in monitors.iter_mut().enumerate() {
        monitor.primary = index == at_origin;
    }
}

/// Refuse a capture whose monitor is no longer the one that was enumerated.
///
/// Capturing re-enumerates, and the backend addresses monitors by position
/// in that list. A display connecting or disconnecting in between shifts
/// the list, so the same index can name a different screen — which would
/// pair one monitor's pixels with another's origin, scale, and name in
/// `session.json`, silently and with every coordinate wrong.
fn ensure_same_monitor(index: usize, expected: &str, found: &str) -> Result<()> {
    anyhow::ensure!(
        expected == found,
        "monitor {index} was {expected:?} when enumerated and is {found:?} now — \
         the display layout changed mid-capture; re-run to pick it up"
    );
    Ok(())
}

/// Refuse a capture with no area.
///
/// Everything downstream assumes a positive-area image: the rasterizer
/// asserts it, the coordinate map clamps against `width - 1`, and the crop
/// writer indexes into it. A zero dimension would surface as a panic from
/// somewhere deep in the overlay rather than as a message naming a monitor.
fn ensure_drawable(width: u32, height: u32, index: usize, name: &str) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0,
        "monitor {index} ({name}) captured as {width}x{height} — a capture with no area \
         cannot be marked up"
    );
    Ok(())
}

/// A visible window at enumeration time, in the capture library's native
/// space (`NATIVE_COORD_SPACE`) — same convention as `MonitorInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// The window system's own handle — the X window id on Linux, where it
    /// is needed to ask for the invisible frame extents.
    pub id: u32,
    pub title: String,
    pub app: String,
    /// Stacking order; higher is closer to the front.
    pub z: i32,
    pub origin: Point,
    pub size_native: Size,
}

pub trait CaptureProvider {
    fn monitors(&self) -> Result<Vec<MonitorInfo>>;
    fn capture(&self, monitor: &MonitorInfo) -> Result<RgbaImage>;
    fn windows(&self) -> Result<Vec<WindowInfo>>;
}

pub struct XcapCapture;

impl XcapCapture {
    fn all() -> Result<Vec<xcap::Monitor>> {
        let monitors = xcap::Monitor::all().context("enumerating monitors");
        #[cfg(target_os = "linux")]
        let monitors = monitors.map_err(crate::linux::explain_enumeration_failure);
        monitors
    }
}

impl CaptureProvider for XcapCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        let mut out = Vec::new();
        for (index, m) in Self::all()?.into_iter().enumerate() {
            out.push(MonitorInfo {
                index,
                name: m.name().with_context(|| format!("monitor {index}: name"))?,
                primary: m
                    .is_primary()
                    .with_context(|| format!("monitor {index}: is_primary"))?,
                origin: Point::new(
                    m.x().with_context(|| format!("monitor {index}: x"))?,
                    m.y().with_context(|| format!("monitor {index}: y"))?,
                ),
                size_native: Size::new(
                    m.width()
                        .with_context(|| format!("monitor {index}: width"))?
                        as i32,
                    m.height()
                        .with_context(|| format!("monitor {index}: height"))?
                        as i32,
                ),
                scale: f64::from(
                    m.scale_factor()
                        .with_context(|| format!("monitor {index}: scale_factor"))?,
                ),
            });
        }
        anyhow::ensure!(!out.is_empty(), "no monitors found");
        normalize_primary(&mut out);
        Ok(out)
    }

    fn capture(&self, monitor: &MonitorInfo) -> Result<RgbaImage> {
        let monitors = Self::all()?;
        let m = monitors
            .get(monitor.index)
            .with_context(|| format!("monitor {} disappeared", monitor.index))?;
        let name = m
            .name()
            .with_context(|| format!("monitor {}: name", monitor.index))?;
        ensure_same_monitor(monitor.index, &monitor.name, &name)?;
        let img = m
            .capture_image()
            .with_context(|| format!("capturing monitor {} ({})", monitor.index, monitor.name))?;
        ensure_drawable(img.width(), img.height(), monitor.index, &monitor.name)?;
        Ok(img)
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        // Wayland answers this call successfully with an empty list rather
        // than failing, so without this guard `windows` would report a bare
        // desktop and exit 0 — indistinguishable, to a script, from a real
        // session that happens to have nothing open.
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            !crate::linux::is_wayland(),
            "window enumeration is unavailable on Wayland, which does not \
             expose window geometry to applications — `windows` and --target \
             need an X11 session (log out and pick one at the login screen), \
             or freeze a single window with --pick, which works on Wayland \
             via the desktop portal's own picker"
        );

        let mut out = Vec::new();
        for w in xcap::Window::all().context(
            "enumerating windows (on X11 this needs an EWMH window manager \
             running — bare X servers like Xvfb have none)",
        )? {
            // System surfaces and minimized windows aren't in the frozen
            // capture; a window whose getters fail is skipped, not fatal.
            let info = (|| -> xcap::XCapResult<Option<WindowInfo>> {
                if w.is_minimized()? {
                    return Ok(None);
                }
                Ok(Some(WindowInfo {
                    id: w.id()?,
                    title: w.title()?,
                    app: w.app_name()?,
                    z: w.z()?,
                    origin: Point::new(w.x()?, w.y()?),
                    size_native: Size::new(w.width()? as i32, w.height()? as i32),
                }))
            })();
            match info {
                Ok(Some(info)) => out.push(info),
                Ok(None) => {}
                Err(e) => log::debug!("skipping window: {e}"),
            }
        }
        // Reported bounds are the outer frame; a GTK window's visible edge
        // sits inside it by the width of the drop shadow.
        #[cfg(target_os = "linux")]
        crate::linux::strip_invisible_borders(&mut out);
        Ok(out)
    }
}

/// Deterministic stand-in for tests: one 100x60 monitor whose capture is a
/// solid-color image.
#[cfg(test)]
pub struct FakeCapture;

#[cfg(test)]
impl CaptureProvider for FakeCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>> {
        Ok(vec![MonitorInfo {
            index: 0,
            name: "Fake Display".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        }])
    }

    fn capture(&self, _monitor: &MonitorInfo) -> Result<RgbaImage> {
        Ok(RgbaImage::from_pixel(100, 60, image::Rgba([1, 2, 3, 255])))
    }

    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(vec![WindowInfo {
            id: 1,
            title: "Fake Window".into(),
            app: "FakeApp".into(),
            z: 0,
            origin: Point::new(10, 10),
            size_native: Size::new(50, 30),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_physical_scales_only_logical_space() {
        assert_eq!(to_physical(20, 2.0, CoordSpace::Logical), 40);
        assert_eq!(to_physical(20, 2.0, CoordSpace::Physical), 20);
        assert_eq!(to_physical(-100, 1.5, CoordSpace::Logical), -150);
        assert_eq!(to_physical(-100, 1.5, CoordSpace::Physical), -100);
    }

    #[test]
    fn from_physical_inverts_to_physical() {
        for space in [CoordSpace::Logical, CoordSpace::Physical] {
            for v in [-1920, -1, 0, 1, 982, 1512] {
                let physical = to_physical(v, 2.0, space);
                assert_eq!(from_physical(physical, 2.0, space), v, "{space:?} {v}");
            }
        }
    }

    #[test]
    fn coord_space_labels_name_their_own_space() {
        assert_eq!(CoordSpace::Logical.label(), "logical");
        assert_eq!(CoordSpace::Physical.label(), "physical");
    }

    #[test]
    fn native_label_follows_the_platform() {
        // macOS reports logical points; Windows (DEVMODE) and X11 report
        // physical pixels. Diagnostics must say which they are showing.
        let expected = if cfg!(target_os = "macos") {
            "logical"
        } else {
            "physical"
        };
        assert_eq!(NATIVE_COORD_SPACE.label(), expected);
    }

    #[test]
    fn monitor_normalization_follows_native_space() {
        let m = MonitorInfo {
            index: 0,
            name: "M".into(),
            primary: true,
            origin: Point::new(100, 50),
            size_native: Size::new(800, 600),
            scale: 2.0,
        };
        let expected_origin = Point::new(
            to_physical(100, 2.0, NATIVE_COORD_SPACE),
            to_physical(50, 2.0, NATIVE_COORD_SPACE),
        );
        assert_eq!(m.origin_physical(), expected_origin);
        assert_eq!(
            m.size_physical(),
            Size::new(
                to_physical(800, 2.0, NATIVE_COORD_SPACE),
                to_physical(600, 2.0, NATIVE_COORD_SPACE)
            )
        );
    }

    fn monitor(primary: bool, ox: i32, oy: i32) -> MonitorInfo {
        MonitorInfo {
            index: 0,
            name: "Display".into(),
            primary,
            origin: Point::new(ox, oy),
            size_native: Size::new(1920, 1080),
            scale: 1.0,
        }
    }

    fn primary_flags(monitors: &[MonitorInfo]) -> Vec<bool> {
        monitors.iter().map(|m| m.primary).collect()
    }

    #[test]
    fn the_same_monitor_passes_the_identity_check() {
        assert!(ensure_same_monitor(0, "Virtual-1", "Virtual-1").is_ok());
    }

    #[test]
    fn a_swapped_monitor_is_refused_rather_than_captured() {
        // A display connecting or disconnecting between enumeration and
        // capture shifts the backend's list, so index 1 can address a
        // different screen than the one whose origin and scale were
        // recorded. Capturing it anyway pairs one monitor's pixels with
        // another's coordinates.
        let err = ensure_same_monitor(1, "DP-1", "HDMI-2").unwrap_err();
        let message = format!("{err}");
        assert!(message.contains("DP-1"), "{message}");
        assert!(message.contains("HDMI-2"), "{message}");
    }

    #[test]
    fn a_normal_capture_is_drawable() {
        assert!(ensure_drawable(1920, 1200, 0, "Virtual-1").is_ok());
    }

    #[test]
    fn an_empty_capture_is_refused_instead_of_panicking_later() {
        // Regression: a zero dimension reached Shape::compute_preview and
        // panicked on `clamp(0, width - 1)` with min > max, surfacing as a
        // crash report rather than a message naming the monitor.
        for (w, h) in [(0, 1200), (1920, 0), (0, 0)] {
            let err = ensure_drawable(w, h, 0, "Virtual-1").unwrap_err();
            assert!(format!("{err}").contains("no area"), "{w}x{h}");
        }
    }

    #[test]
    fn a_lone_monitor_is_primary_even_when_randr_says_otherwise() {
        // Regression: a cold Xwayland reports no primary output, so the only
        // monitor on the desktop came back primary=false and session.json
        // recorded it that way.
        let mut monitors = vec![monitor(false, 0, 0)];
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), vec![true]);
    }

    #[test]
    fn with_no_primary_the_monitor_at_the_origin_wins() {
        let mut monitors = vec![monitor(false, -1920, 0), monitor(false, 0, 0)];
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), vec![false, true]);
    }

    #[test]
    fn with_no_primary_and_no_monitor_at_the_origin_the_first_wins() {
        let mut monitors = vec![monitor(false, 100, 100), monitor(false, 2020, 100)];
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), vec![true, false]);
    }

    #[test]
    fn several_primaries_are_reduced_to_the_one_at_the_origin() {
        let mut monitors = vec![monitor(true, -1920, 0), monitor(true, 0, 0)];
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), vec![false, true]);
    }

    #[test]
    fn a_backend_that_names_one_primary_is_left_alone() {
        // Not the monitor at the origin: when the window system answers, its
        // answer stands.
        let mut monitors = vec![monitor(true, -1920, 0), monitor(false, 0, 0)];
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), vec![true, false]);
    }

    #[test]
    fn normalizing_is_idempotent() {
        let mut monitors = vec![monitor(false, 0, 0), monitor(false, 1920, 0)];
        normalize_primary(&mut monitors);
        let once = primary_flags(&monitors);
        normalize_primary(&mut monitors);
        assert_eq!(primary_flags(&monitors), once);
    }

    #[test]
    fn fake_capture_matches_its_monitor() {
        let provider = FakeCapture;
        let monitors = provider.monitors().unwrap();
        let img = provider.capture(&monitors[0]).unwrap();
        assert_eq!(
            (img.width() as i32, img.height() as i32),
            (monitors[0].size_native.w, monitors[0].size_native.h)
        );
    }
}
