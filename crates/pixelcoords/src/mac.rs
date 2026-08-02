//! macOS-only helpers.
//!
//! Two jobs, both of which exist because the platform lies quietly rather
//! than erroring:
//!
//! - **Screen Recording permission (TCC) preflight.** Without the grant,
//!   capture silently returns wallpaper-only pixels, so we check
//!   explicitly instead of inspecting captured images.
//! - **Cursor-free display capture.** See [`capture_display`].

use std::ffi::c_void;

use anyhow::{Result, bail};
use image::RgbaImage;

// CoreGraphics' pointer types are opaque. Aliases would read better, but
// an extern block does not count as a use, so the compiler sees them as
// dead. Spelled out instead.

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;

    // Declared here rather than taken from `objc2-core-graphics` because
    // that crate marks this `#[deprecated]` (Apple points at
    // ScreenCaptureKit), and this workspace builds with `-D warnings`.
    // Silencing deprecation workspace-wide to use one function would cost
    // far more than it buys.
    fn CGDisplayCreateImage(display: u32) -> *mut c_void;
    fn CGImageGetWidth(image: *mut c_void) -> usize;
    fn CGImageGetHeight(image: *mut c_void) -> usize;
    fn CGImageGetBytesPerRow(image: *mut c_void) -> usize;
    fn CGImageGetBitsPerPixel(image: *mut c_void) -> usize;
    fn CGImageGetDataProvider(image: *mut c_void) -> *mut c_void;
    fn CGDataProviderCopyData(provider: *mut c_void) -> *const c_void;
    fn CGImageRelease(image: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFDataGetLength(data: *const c_void) -> isize;
    fn CFRelease(cf: *const c_void);
}

/// Capture one display's pixels, **without the mouse pointer**.
///
/// This exists because the pointer is not part of the screen — it is drawn
/// on top of it — and a tool that measures and re-finds regions must not
/// treat it as content.
///
/// `CGWindowListCreateImage`, which the capture crate uses, composites the
/// pointer in. That is invisible until something matches a saved crop
/// against a fresh capture: the pointer lands on whatever was just clicked
/// and shows up as a foreign object inside the very region being checked.
/// Measured on a flat region it costs about 0.17 of match score — enough to
/// drop a perfect match below the 0.9 floor — while a busy region absorbs
/// it entirely. So the failure is not merely intermittent, it is
/// *content-dependent*, which is worse: it looks like flakiness.
///
/// `CGDisplayCreateImage` returns the display contents alone, which is also
/// what the system's own `screencapture` does by default.
pub fn capture_display(display: u32) -> Result<RgbaImage> {
    // SAFETY: each call is a documented CoreGraphics entry point. The image
    // and the copied data are released on every path, including the error
    // paths below, and no pointer outlives this function.
    unsafe {
        let image = CGDisplayCreateImage(display);
        if image.is_null() {
            bail!(
                "CGDisplayCreateImage returned nothing for display {display} —                  the display may have been disconnected mid-capture"
            );
        }
        let result = copy_pixels(image);
        CGImageRelease(image);
        result
    }
}

/// The only pixel layout this reader understands: 32 bits per pixel, four
/// bytes, BGRA order.
///
/// It is what `CGDisplayCreateImage` returns for every display tested, but
/// it is not guaranteed. An HDR/EDR panel can hand back 64-bit RGBA-half,
/// and a reader that assumes 32 would take the left half of every row and
/// return a **plausible, silently wrong picture** — the one failure this
/// tool cannot tolerate, since every coordinate, crop, match score, and
/// diff downstream would inherit it while looking fine.
const SUPPORTED_BITS_PER_PIXEL: usize = 32;

/// Turn a `CGImage` into an `RgbaImage`, undoing two CoreGraphics
/// conventions: rows are padded to an alignment boundary, and pixels
/// arrive as BGRA.
///
/// SAFETY: `image` must be a live `CGImageRef`.
unsafe fn copy_pixels(image: *mut c_void) -> Result<RgbaImage> {
    unsafe {
        let width = CGImageGetWidth(image);
        let height = CGImageGetHeight(image);
        let bytes_per_row = CGImageGetBytesPerRow(image);
        let bits_per_pixel = CGImageGetBitsPerPixel(image);

        // Ask before assuming. Refusing loudly is the whole point: this
        // reader cannot convert an unexpected layout, and reading one as
        // if it were BGRA produces a wrong image rather than an error.
        if bits_per_pixel != SUPPORTED_BITS_PER_PIXEL {
            bail!(
                "the display returned {bits_per_pixel}-bit pixels; pixelcoords reads \
                 {SUPPORTED_BITS_PER_PIXEL}-bit BGRA. Reading them anyway would produce a \
                 wrong image rather than an error, so it stops here — please report this \
                 with your display setup at \
                 https://github.com/nolindnaidoo/pixelcoords/issues"
            );
        }
        // A zero stride would panic `chunks_exact`, and a stride narrower
        // than one row means the rows are not what they claim to be. The
        // zero case is checked on its own because a zero *width* makes the
        // comparison vacuously true and would let it through.
        if bytes_per_row == 0 || bytes_per_row < width * 4 {
            bail!(
                "the display reported a {bytes_per_row}-byte stride for {width}-pixel rows, \
                 which cannot hold them"
            );
        }

        let provider = CGImageGetDataProvider(image);
        if provider.is_null() {
            bail!("captured image had no data provider");
        }
        let data = CGDataProviderCopyData(provider);
        if data.is_null() {
            bail!("could not copy the captured image's pixels");
        }

        let bytes = CFDataGetBytePtr(data);
        let length = CFDataGetLength(data);
        if bytes.is_null() || length < 0 {
            CFRelease(data);
            bail!("captured image had an unreadable pixel buffer");
        }
        let raw = std::slice::from_raw_parts(bytes, length as usize);

        let mut buffer = Vec::with_capacity(width * height * 4);
        for row in raw.chunks_exact(bytes_per_row).take(height) {
            buffer.extend_from_slice(&row[..width * 4]);
        }
        for pixel in buffer.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        CFRelease(data);

        RgbaImage::from_raw(width as u32, height as u32, buffer).ok_or_else(|| {
            anyhow::anyhow!("captured {width}x{height} image did not fit its buffer")
        })
    }
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
