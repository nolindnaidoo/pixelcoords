//! The overlay window: a winit window + softbuffer surface showing the
//! frozen capture, fullscreen on the monitor it came from or sized to a
//! picked window. `CoordMap` is the only place a scale factor between
//! window coordinates and capture coordinates may exist.

use std::num::NonZeroU32;
use std::sync::Arc;

use anyhow::{Context, Result};
use pixelcoords_core::geometry::{Point, Size};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::{CursorIcon, Window, WindowAttributes};

/// Maps window-physical coordinates to capture-pixel coordinates. Identity
/// in the expected case (a window at the capture's own size, fullscreen or
/// not); otherwise it inverts the [`Fit`] the capture was drawn at, so
/// selections stay correct in capture space either way.
#[derive(Debug, Clone, Copy)]
pub struct CoordMap {
    pub window: Size,
    pub capture: Size,
}

/// Where the capture is drawn inside the window: the largest centered
/// rect with the capture's own aspect ratio that fits. The whole window,
/// whenever the two already agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub origin: Point,
    pub size: Size,
}

impl CoordMap {
    pub fn is_identity(&self) -> bool {
        self.window == self.capture
    }

    /// The capture letterboxed into the window: one scale factor for both
    /// axes, centered, margins left over.
    ///
    /// A window system does not always grant the size we ask for — GNOME
    /// clamps a window to the work area, so a picked window taller than
    /// the space under the top bar comes back short. Scaling each axis to
    /// fill would squash the pick; scaling both by the smaller factor
    /// shows it whole and undistorted, just smaller.
    pub fn fit(&self) -> Fit {
        let scale = f64::min(
            f64::from(self.window.w) / f64::from(self.capture.w),
            f64::from(self.window.h) / f64::from(self.capture.h),
        );
        let w = ((f64::from(self.capture.w) * scale) as i32).clamp(1, self.window.w);
        let h = ((f64::from(self.capture.h) * scale) as i32).clamp(1, self.window.h);
        Fit {
            origin: Point::new((self.window.w - w) / 2, (self.window.h - h) / 2),
            size: Size::new(w, h),
        }
    }

    pub fn window_to_capture(&self, pos: PhysicalPosition<f64>) -> Point {
        let fit = self.fit();
        let x =
            (pos.x - f64::from(fit.origin.x)) * f64::from(self.capture.w) / f64::from(fit.size.w);
        let y =
            (pos.y - f64::from(fit.origin.y)) * f64::from(self.capture.h) / f64::from(fit.size.h);
        Point::new(
            (x as i32).clamp(0, self.capture.w - 1),
            (y as i32).clamp(0, self.capture.h - 1),
        )
    }
}

/// How the overlay occupies the display.
///
/// A captured monitor is presented fullscreen on that monitor, where one
/// window pixel is one capture pixel. A window picked through the portal
/// has no monitor of its own — it is smaller than the screen and usually a
/// different shape — so it gets a plain window of exactly the capture's
/// size, which keeps that same 1:1 mapping. Fullscreening a pick would
/// blow it up to a display it shares neither size nor shape with, and the
/// user would mark up a picture several times larger than the window it
/// depicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// Fill `monitor` edge to edge.
    Fullscreen,
    /// A window sized to the capture, placed by the window system.
    Windowed,
}

pub struct OverlayView {
    window: Arc<Window>,
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    capture_size: Size,
    /// Scratch buffer for the non-identity path: compose in capture space,
    /// then letterbox into the window.
    scratch: Vec<u32>,
    /// When the overlay was created. The window system applies our size
    /// after mapping the window — X11 fullscreen, or a compositor honoring
    /// a windowed request — so the opening frames legitimately differ from
    /// the capture size for a few tens of milliseconds; only a mismatch
    /// outliving that says anything about the saved coordinates.
    created: std::time::Instant,
    warned_mismatch: bool,
}

/// How long the window may disagree with the capture before the mismatch
/// is treated as real rather than as the window manager still settling.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(1);

impl OverlayView {
    /// Create the overlay on `monitor`. The window starts hidden and is
    /// shown only once its final size is applied, avoiding a flash.
    pub fn new(
        event_loop: &ActiveEventLoop,
        monitor: Option<&MonitorHandle>,
        capture_size: Size,
        presentation: Presentation,
    ) -> Result<Self> {
        let attrs = WindowAttributes::default()
            .with_title("pixelcoords")
            .with_visible(false)
            .with_decorations(false)
            // Ask for the capture's size up front. Windowed, this is the
            // size that stays. Fullscreen, X11 window managers apply
            // _NET_WM_STATE_FULLSCREEN only after the window is mapped, so
            // without this the first frame renders at winit's 800x600
            // default and scale-blits the whole screen into it.
            .with_inner_size(PhysicalSize::new(
                capture_size.w.max(1) as u32,
                capture_size.h.max(1) as u32,
            ));

        #[cfg(not(target_os = "macos"))]
        let attrs = match presentation {
            Presentation::Fullscreen => attrs.with_fullscreen(Some(
                winit::window::Fullscreen::Borderless(monitor.cloned()),
            )),
            Presentation::Windowed => attrs,
        };

        let window = Arc::new(event_loop.create_window(attrs).context("creating window")?);

        // Only a fullscreen overlay is pinned to a monitor; a windowed one
        // has no monitor of its own to be moved onto.
        if let Some(m) = monitor.filter(|_| presentation == Presentation::Fullscreen) {
            window.set_outer_position(m.position());
        }

        // macOS: same-Space fullscreen — no Space slide animation, which
        // would destroy the frozen-screen illusion.
        #[cfg(target_os = "macos")]
        if presentation == Presentation::Fullscreen {
            use winit::platform::macos::WindowExtMacOS;
            window.set_simple_fullscreen(true);
        }

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("softbuffer surface: {e}"))?;

        window.set_cursor(CursorIcon::Crosshair);
        window.set_visible(true);
        window.focus_window();

        Ok(Self {
            window,
            surface,
            capture_size,
            scratch: Vec::new(),
            created: std::time::Instant::now(),
            warned_mismatch: false,
        })
    }

    pub fn id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn set_cursor(&self, icon: CursorIcon) {
        self.window.set_cursor(icon);
    }

    pub fn coord_map(&self) -> CoordMap {
        let inner = self.window.inner_size();
        CoordMap {
            window: Size::new(inner.width.max(1) as i32, inner.height.max(1) as i32),
            capture: self.capture_size,
        }
    }

    /// Compose a frame in capture space via `compose`, then present it,
    /// scale-blitting if the window size differs from the capture size.
    pub fn present(&mut self, compose: impl FnOnce(&mut [u32], Size)) -> Result<()> {
        let map = self.coord_map();
        let (win_w, win_h) = (map.window.w as u32, map.window.h as u32);
        self.surface
            .resize(
                NonZeroU32::new(win_w).context("zero-width window")?,
                NonZeroU32::new(win_h).context("zero-height window")?,
            )
            .map_err(|e| anyhow::anyhow!("surface resize: {e}"))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("surface buffer: {e}"))?;

        if map.is_identity() {
            compose(&mut buffer, map.capture);
            buffer
                .present()
                .map_err(|e| anyhow::anyhow!("present: {e}"))?;
            return Ok(());
        }

        // Size-mismatch path: compose in capture space, then letterbox.
        let fit = map.fit();
        if self.created.elapsed() > SETTLE && !self.warned_mismatch {
            log::warn!(
                "window {}x{} != capture {}x{}; drawing it letterboxed at {}x{} \
                 (selections stay in capture space)",
                map.window.w,
                map.window.h,
                map.capture.w,
                map.capture.h,
                fit.size.w,
                fit.size.h
            );
            self.warned_mismatch = true;
        }
        let cap_len = (map.capture.w as usize) * (map.capture.h as usize);
        self.scratch.resize(cap_len, 0);
        compose(&mut self.scratch, map.capture);
        letterbox_blit(&self.scratch, map.capture, &mut buffer, map.window, fit);
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("present: {e}"))?;
        Ok(())
    }
}

/// Nearest-neighbor blit from `src` (capture space) into `fit` within
/// `dst` (window space). Whatever `fit` does not cover is left black —
/// the margins of a letterboxed capture are not part of the picture.
fn letterbox_blit(src: &[u32], src_size: Size, dst: &mut [u32], dst_size: Size, fit: Fit) {
    dst.fill(0);
    for dy in 0..fit.size.h {
        let sy = (i64::from(dy) * i64::from(src_size.h) / i64::from(fit.size.h)) as i32;
        let src_row = (sy as usize) * (src_size.w as usize);
        let dst_row =
            ((dy + fit.origin.y) as usize) * (dst_size.w as usize) + fit.origin.x.max(0) as usize;
        for dx in 0..fit.size.w {
            let sx = (i64::from(dx) * i64::from(src_size.w) / i64::from(fit.size.w)) as i32;
            dst[dst_row + dx as usize] = src[src_row + sx as usize];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_map_passes_coordinates_through() {
        let map = CoordMap {
            window: Size::new(3024, 1964),
            capture: Size::new(3024, 1964),
        };
        assert!(map.is_identity());
        let p = map.window_to_capture(PhysicalPosition::new(100.0, 200.0));
        assert_eq!(p, Point::new(100, 200));
    }

    #[test]
    fn mismatched_map_scales_and_clamps() {
        let map = CoordMap {
            window: Size::new(1512, 982),
            capture: Size::new(3024, 1964),
        };
        let p = map.window_to_capture(PhysicalPosition::new(100.0, 200.0));
        assert_eq!(p, Point::new(200, 400));
        let edge = map.window_to_capture(PhysicalPosition::new(5000.0, 5000.0));
        assert_eq!(edge, Point::new(3023, 1963));
    }

    /// A portal pick is neither the size nor the shape of the screen it
    /// lands on: the 763x957 pick from a 1512x949 display was drawn at
    /// nearly double width when it was presented fullscreen. Letterboxed,
    /// it keeps its shape and gains black margins instead.
    #[test]
    fn a_picked_window_keeps_its_shape_on_a_screen_shaped_nothing_like_it() {
        let map = CoordMap {
            window: Size::new(1512, 949),
            capture: Size::new(763, 957),
        };
        assert!(!map.is_identity());
        let fit = map.fit();
        // Height is the binding axis, so the pick is drawn just under its
        // own size and centered, not stretched to 1512 wide.
        assert_eq!(fit.size, Size::new(756, 949));
        assert_eq!(fit.origin, Point::new(378, 0));
        // Same aspect ratio as the capture, within a pixel of rounding.
        let aspect = f64::from(fit.size.w) / f64::from(fit.size.h);
        assert!((aspect - 763.0 / 957.0).abs() < 0.001, "aspect {aspect}");
    }

    /// What the compositor actually did: GNOME clamped the 957-tall pick
    /// to the 917 px work area under the top bar. Filling that window
    /// would squash the picture 4%; the fit shrinks both axes instead.
    #[test]
    fn a_window_clamped_to_the_work_area_shrinks_rather_than_squashes() {
        let map = CoordMap {
            window: Size::new(763, 917),
            capture: Size::new(763, 957),
        };
        let fit = map.fit();
        assert_eq!(fit.size, Size::new(731, 917));
        assert_eq!(fit.origin, Point::new(16, 0));
        // A click at the middle of the drawn picture is the middle of the
        // capture, not somewhere 4% off down the window.
        let center = map.window_to_capture(PhysicalPosition::new(
            f64::from(fit.origin.x) + f64::from(fit.size.w) / 2.0,
            f64::from(fit.size.h) / 2.0,
        ));
        assert_eq!(center, Point::new(381, 478));
    }

    #[test]
    fn a_window_matching_the_capture_needs_no_letterbox() {
        let pick = Size::new(763, 957);
        let map = CoordMap {
            window: pick,
            capture: pick,
        };
        assert!(map.is_identity());
        assert_eq!(map.fit().origin, Point::new(0, 0));
        assert_eq!(map.fit().size, pick);
        assert_eq!(
            map.window_to_capture(PhysicalPosition::new(271.0, 417.0)),
            Point::new(271, 417)
        );
    }

    #[test]
    fn letterbox_blit_2x_upscale_replicates_pixels() {
        let src = vec![1u32, 2, 3, 4];
        let mut dst = vec![0u32; 16];
        let size = Size::new(4, 4);
        let fit = Fit {
            origin: Point::new(0, 0),
            size,
        };
        letterbox_blit(&src, Size::new(2, 2), &mut dst, size, fit);
        assert_eq!(dst[0], 1);
        assert_eq!(dst[1], 1);
        assert_eq!(dst[2], 2);
        assert_eq!(dst[4], 1);
        assert_eq!(dst[15], 4);
    }

    #[test]
    fn letterbox_blit_leaves_the_margins_black() {
        let src = vec![7u32; 4];
        let mut dst = vec![9u32; 16];
        let fit = Fit {
            origin: Point::new(1, 1),
            size: Size::new(2, 2),
        };
        letterbox_blit(&src, Size::new(2, 2), &mut dst, Size::new(4, 4), fit);
        // The centered 2x2 carries the capture; every margin pixel is
        // cleared, including the stale 9s that were there before.
        assert_eq!(dst[5], 7);
        assert_eq!(dst[6], 7);
        assert_eq!(dst[9], 7);
        assert_eq!(dst[10], 7);
        assert_eq!(dst[0], 0);
        assert_eq!(dst[4], 0);
        assert_eq!(dst[7], 0);
        assert_eq!(dst[15], 0);
    }
}
