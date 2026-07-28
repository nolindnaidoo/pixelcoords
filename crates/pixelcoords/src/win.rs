//! Windows-only helpers. Process DPI awareness has to be established before
//! anything asks the OS about monitors, because xcap branches on it: when
//! the process is not DPI-aware it refuses `GetDpiForMonitor` and derives
//! the scale factor from `DESKTOPHORZRES / HORZRES` instead — a ratio
//! against the *virtualized* logical width, which reports 1.5002180337905884
//! rather than 1.5 on a 150% display.
//!
//! winit establishes awareness too, but only when the event loop is built,
//! which is long after monitors are enumerated and captured — and never at
//! all for `doctor`, `windows`, and `shoot`.

/// `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`. This is the context winit
/// prefers as well, so the process ends up in the same state no matter which
/// of us wins the race to set it.
const PER_MONITOR_AWARE_V2: isize = -4;

// Requires Windows 10 1703 or newer. Rust itself has required Windows 10
// since 1.78, so anything that can run this binary already has the entry
// point; a static link is safe and keeps the module free of dependencies.
#[link(name = "user32")]
unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    fn GetThreadDpiAwarenessContext() -> isize;
    fn AreDpiAwarenessContextsEqual(a: isize, b: isize) -> i32;
}

/// Declare this process per-monitor DPI-aware. Best-effort and idempotent:
/// Windows refuses the call once awareness is already set — by an earlier
/// call, an application manifest, or an app-compatibility override — which
/// is not an error. Returns whether *this* call established it.
pub fn become_dpi_aware() -> bool {
    unsafe { SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) != 0 }
}

/// Whether the process is in fact per-monitor v2 aware. Reported by
/// `doctor`, because an external override that forces a different awareness
/// mode silently degrades every scale factor we print and save.
pub fn is_per_monitor_aware_v2() -> bool {
    unsafe {
        AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), PER_MONITOR_AWARE_V2) != 0
    }
}
