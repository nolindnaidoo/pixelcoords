//! Linux-only platform facts.
//!
//! X11 and Wayland differ in what they will tell an application. X11 exposes
//! every window's geometry through the window manager; Wayland deliberately
//! does not, so `windows` and `--target` cannot work there and must say so
//! rather than reporting an empty desktop.

use pixelcoords_core::geometry::{Point, Size};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

use crate::capture::WindowInfo;

/// Whether these environment values describe a Wayland session.
///
/// Split from the environment so the decision itself is a pure function:
/// `XDG_SESSION_TYPE` is authoritative when the session manager sets it, and
/// a non-empty `WAYLAND_DISPLAY` covers sessions that leave it unset.
pub fn is_wayland_env(session_type: Option<&str>, wayland_display: Option<&str>) -> bool {
    // The session manager's assertion wins when it made one, so an X11 login
    // that inherited WAYLAND_DISPLAY from a parent shell is still X11.
    if let Some(declared) = session_type.filter(|t| !t.is_empty()) {
        return declared.eq_ignore_ascii_case("wayland");
    }
    wayland_display.is_some_and(|d| !d.is_empty())
}

/// [`is_wayland_env`] applied to this process's environment.
pub fn is_wayland() -> bool {
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    is_wayland_env(session_type.as_deref(), wayland_display.as_deref())
}

/// Explain a monitor-enumeration failure that a Wayland user cannot
/// otherwise diagnose.
///
/// Capture on Wayland goes through the xdg-desktop-portal, but monitor
/// geometry still comes from `RandR`, so enumeration fails with a bare X
/// connection error on a session where Xwayland is absent or unreachable.
pub fn explain_enumeration_failure(err: anyhow::Error) -> anyhow::Error {
    annotate_enumeration_failure(err, is_wayland())
}

/// [`explain_enumeration_failure`] with the session decision supplied, so
/// the annotation is testable without a display server.
fn annotate_enumeration_failure(err: anyhow::Error, wayland: bool) -> anyhow::Error {
    if !wayland {
        return err;
    }
    err.context(
        "monitor geometry comes from RandR even on Wayland, so Xwayland must \
         be running and DISPLAY set — GNOME starts it on demand, but a \
         session without Xwayland at all is unsupported",
    )
}

/// Freeze one window through the xdg-desktop-portal's interactive
/// screenshot: the compositor shows its own picker (screen / window /
/// area) and returns exactly the pixels the user chose. This is the only
/// route to a window's pixels on Wayland, which never reveals window
/// geometry to applications — the user points at the window instead of
/// the app finding it, so no identity, position, or scale comes back:
/// just the pixels, which is exactly what window-relative marking needs.
pub fn portal_pick() -> anyhow::Result<image::RgbaImage> {
    use anyhow::Context;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::Value;

    let conn = Connection::session().context("connecting to the session DBus")?;
    let token = format!("pixelcoords{}", std::process::id());
    let request_path = request_object_path(
        conn.unique_name()
            .context("the session bus assigned no unique name")?
            .as_str(),
        &token,
    );
    // Subscribe to the response before asking, or it can race past us.
    let request: Proxy<'static> = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        request_path,
        "org.freedesktop.portal.Request",
    )
    .context("building the portal request proxy")?;
    let mut responses = request
        .receive_signal("Response")
        .context("subscribing to the portal response")?;

    let screenshot = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Screenshot",
    )
    .context("reaching the screenshot portal (is xdg-desktop-portal running?)")?;
    let mut options: std::collections::HashMap<&str, Value> = std::collections::HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert("interactive", Value::from(true));
    screenshot
        .call_method("Screenshot", &("", options))
        .context("asking the portal for an interactive screenshot")?;

    let message = responses
        .next()
        .context("the portal closed without answering")?;
    // A plain zvariant map rather than a derived struct: the derive would
    // drag serde in as a direct dependency for one field.
    let (code, mut results): (
        u32,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    ) = message
        .body()
        .deserialize()
        .context("reading the portal response")?;
    explain_portal_code(code)?;
    let uri: String = results
        .remove("uri")
        .context("the portal response carried no uri")?
        .try_into()
        .context("the portal uri is not a string")?;
    let path = uri_to_path(&uri)?;
    let img = image::open(&path)
        .with_context(|| format!("reading the portal screenshot {}", path.display()))?
        .to_rgba8();
    // The portal leaves its file behind (often in ~/Pictures); ours to
    // remove — we asked for it.
    let _ = std::fs::remove_file(&path);
    Ok(img)
}

/// The object path a portal request's response arrives on, derived from
/// the connection's unique name per the Request interface's documentation.
fn request_object_path(unique_name: &str, token: &str) -> String {
    let id = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{id}/{token}")
}

/// Portal response codes, as errors a user can act on.
fn explain_portal_code(code: u32) -> anyhow::Result<()> {
    match code {
        0 => Ok(()),
        1 => Err(anyhow::anyhow!(
            "the picker was cancelled — nothing was captured"
        )),
        other => Err(anyhow::anyhow!(
            "the portal refused the screenshot (response code {other})"
        )),
    }
}

/// A `file://` URI to a path, percent-decoded byte-for-byte — the portal
/// encodes spaces and non-ASCII in the filename it hands back.
fn uri_to_path(uri: &str) -> anyhow::Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow::anyhow!("the portal returned a non-file URI: {uri}"))?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(b) = input.next() {
        if b != b'%' {
            bytes.push(b);
            continue;
        }
        let hex: [u8; 2] = [
            input
                .next()
                .ok_or_else(|| anyhow::anyhow!("truncated percent escape in URI: {uri}"))?,
            input
                .next()
                .ok_or_else(|| anyhow::anyhow!("truncated percent escape in URI: {uri}"))?,
        ];
        let digit = |h: u8| -> anyhow::Result<u8> {
            (h as char)
                .to_digit(16)
                .map(|d| d as u8)
                .ok_or_else(|| anyhow::anyhow!("invalid percent escape in URI: {uri}"))
        };
        bytes.push(digit(hex[0])? * 16 + digit(hex[1])?);
    }
    Ok(std::path::PathBuf::from(std::ffi::OsString::from_vec(
        bytes,
    )))
}

/// The invisible border a toolkit draws outside the window it appears to
/// be. GTK apps under GNOME carry one for the drop shadow and resize grip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameExtents {
    pub left: i32,
    pub right: i32,
    pub top: i32,
    pub bottom: i32,
}

/// Shrink outer-frame bounds to the window a user can actually see.
///
/// Pure, so the arithmetic is tested without an X server. Degenerate
/// extents (wider than the window itself) clamp to a 1x1 rather than
/// producing an inverted rectangle downstream.
pub fn inset_to_visible(origin: Point, size: Size, extents: FrameExtents) -> (Point, Size) {
    let width = size.w - extents.left - extents.right;
    let height = size.h - extents.top - extents.bottom;
    (
        Point::new(origin.x + extents.left, origin.y + extents.top),
        Size::new(width.max(1), height.max(1)),
    )
}

/// Replace each window's outer-frame bounds with its visible bounds.
///
/// Windows without `_GTK_FRAME_EXTENTS` are left untouched: the property
/// is absent on non-GTK toolkits, whose reported bounds are already the
/// visible ones. Any X failure leaves every window as reported — a missing
/// shadow correction is worth far less than refusing to list windows.
pub fn strip_invisible_borders(windows: &mut [WindowInfo]) {
    let Ok((conn, _)) = x11rb::connect(None) else {
        return;
    };
    let Ok(cookie) = conn.intern_atom(true, b"_GTK_FRAME_EXTENTS") else {
        return;
    };
    let Ok(reply) = cookie.reply() else {
        return;
    };
    let atom = reply.atom;
    if atom == 0 {
        return;
    }
    for window in windows {
        let Some(extents) = frame_extents(&conn, atom, window.id) else {
            continue;
        };
        let (origin, size) = inset_to_visible(window.origin, window.size_native, extents);
        window.origin = origin;
        window.size_native = size;
    }
}

/// Read `_GTK_FRAME_EXTENTS` off one window, if it has one.
fn frame_extents<C: Connection>(conn: &C, atom: u32, window: u32) -> Option<FrameExtents> {
    let reply = conn
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 4)
        .ok()?
        .reply()
        .ok()?;
    let values: Vec<u32> = reply.value32()?.collect();
    let [left, right, top, bottom] = values[..] else {
        return None;
    };
    Some(FrameExtents {
        left: left as i32,
        right: right as i32,
        top: top as i32,
        bottom: bottom as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_paths_follow_the_portal_convention() {
        assert_eq!(
            request_object_path(":1.42", "pixelcoords7"),
            "/org/freedesktop/portal/desktop/request/1_42/pixelcoords7"
        );
    }

    #[test]
    fn portal_codes_map_to_actionable_errors() {
        assert!(explain_portal_code(0).is_ok());
        let cancelled = explain_portal_code(1).unwrap_err().to_string();
        assert!(cancelled.contains("cancelled"), "got: {cancelled}");
        let refused = explain_portal_code(2).unwrap_err().to_string();
        assert!(refused.contains("response code 2"), "got: {refused}");
    }

    #[test]
    fn file_uris_percent_decode_to_paths() {
        assert_eq!(
            uri_to_path("file:///home/user/Pictures/Screenshot%20From%202026.png").unwrap(),
            std::path::PathBuf::from("/home/user/Pictures/Screenshot From 2026.png")
        );
        assert!(uri_to_path("https://example.com/x.png").is_err());
        assert!(uri_to_path("file:///bad%2").is_err());
        assert!(uri_to_path("file:///bad%zz").is_err());
    }

    #[test]
    fn session_type_wayland_is_wayland() {
        assert!(is_wayland_env(Some("wayland"), None));
    }

    #[test]
    fn session_type_match_ignores_case() {
        assert!(is_wayland_env(Some("Wayland"), None));
    }

    #[test]
    fn session_type_x11_is_not_wayland() {
        assert!(!is_wayland_env(Some("x11"), None));
    }

    #[test]
    fn wayland_display_alone_is_wayland() {
        assert!(!is_wayland_env(None, None));
        assert!(is_wayland_env(None, Some("wayland-0")));
    }

    #[test]
    fn empty_wayland_display_is_not_wayland() {
        assert!(!is_wayland_env(None, Some("")));
    }

    const GNOME_SHADOW: FrameExtents = FrameExtents {
        left: 26,
        right: 26,
        top: 23,
        bottom: 29,
    };

    #[test]
    fn the_visible_window_sits_inside_the_reported_frame() {
        // Measured from Firefox under GNOME 46: xcap reports the outer
        // frame at (47, 11) 1099x1178 while the window a user sees starts
        // 26px right and 23px down from it.
        let (origin, size) =
            inset_to_visible(Point::new(47, 11), Size::new(1099, 1178), GNOME_SHADOW);
        assert_eq!(origin, Point::new(73, 34));
        assert_eq!(size, Size::new(1047, 1126));
    }

    #[test]
    fn zero_extents_leave_bounds_untouched() {
        let extents = FrameExtents {
            left: 0,
            right: 0,
            top: 0,
            bottom: 0,
        };
        let (origin, size) = inset_to_visible(Point::new(10, 20), Size::new(300, 200), extents);
        assert_eq!((origin, size), (Point::new(10, 20), Size::new(300, 200)));
    }

    #[test]
    fn extents_wider_than_the_window_clamp_instead_of_inverting() {
        let (_, size) = inset_to_visible(Point::new(0, 0), Size::new(40, 40), GNOME_SHADOW);
        assert_eq!(size, Size::new(1, 1));
    }

    #[test]
    fn wayland_enumeration_failures_name_xwayland() {
        let err = annotate_enumeration_failure(anyhow::anyhow!("Connection closed"), true);
        let chain = format!("{err:#}");
        assert!(chain.contains("Xwayland"), "{chain}");
        // The underlying cause survives the annotation.
        assert!(chain.contains("Connection closed"), "{chain}");
    }

    #[test]
    fn x11_enumeration_failures_are_left_alone() {
        let err = annotate_enumeration_failure(anyhow::anyhow!("Connection closed"), false);
        assert_eq!(format!("{err:#}"), "Connection closed");
    }

    #[test]
    fn x11_session_wins_over_a_stray_wayland_display() {
        // An X11 login that inherited WAYLAND_DISPLAY from a parent shell is
        // still X11; XDG_SESSION_TYPE is what the session manager asserts.
        assert!(!is_wayland_env(Some("x11"), Some("wayland-0")));
    }
}
