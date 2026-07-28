//! macOS-only helpers. Screen Recording permission (TCC) preflight: without
//! the grant, capture silently returns wallpaper-only pixels, so we check
//! explicitly instead of inspecting captured images.

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether this process already holds the Screen Recording grant. Never
/// prompts.
pub fn has_screen_capture_access() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Ask the system to grant access; shows the TCC prompt on first call (per
/// process attribution — usually the terminal running us). Returns whether
/// access is held afterwards.
pub fn request_screen_capture_access() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

pub const GRANT_INSTRUCTIONS: &str = "\
Screen Recording permission is NOT granted.
pixelcoords can only capture your wallpaper until it is.

To grant it:
  1. Open System Settings -> Privacy & Security -> Screen & System Audio Recording
  2. Enable the terminal app you run pixelcoords from (Terminal, iTerm2, Ghostty, VS Code, ...)
  3. Quit and reopen that terminal app, then run `pixelcoords doctor` again

If the app is missing from the list, run `pixelcoords` once to trigger the
permission prompt, or add the terminal manually with the '+' button.";
