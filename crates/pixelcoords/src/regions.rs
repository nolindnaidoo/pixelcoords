//! Everything a command needs to compare a saved session against the
//! screen right now: which selections a `--label` picks, their crops
//! decoded from disk, and one identity-checked capture per monitor those
//! selections live on.
//!
//! `find` was the only command that needed this, so it lived inline. Four
//! commands need it now, and the part worth sharing is not the loop — it
//! is the *refusals*. A display that is gone and a display that changed
//! resolution are different sentences pointing at different fixes, and
//! deriving them independently in four places is how one of them ends up
//! saying the wrong one.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use image::RgbaImage;
use pixelcoords_core::geometry::Point;
use pixelcoords_core::locate;
use pixelcoords_core::session::{MonitorMatch, MonitorRecord, SelectionRecord, SessionFile};

use crate::capture::{self, CaptureProvider};

/// One selection resolved into what a fresh-capture comparison needs:
/// which monitor's frame it belongs to, where its crop was cut from that
/// frame, and the crop's pixels — whose alpha carries the shape mask,
/// baked in at save time.
pub struct Region<'a> {
    /// Index into `session.selections`. Every report row's identity, and
    /// the reason this is carried rather than recomputed.
    pub index: usize,
    pub record: &'a SelectionRecord,
    pub monitor: usize,
    pub origin: Point,
    pub crop: RgbaImage,
}

impl Region<'_> {
    /// The crop as a match template: grayscale plus the alpha-derived
    /// mask, so a shaped selection matches on its own pixels rather than
    /// its bounding box.
    pub fn template(&self) -> locate::Template {
        locate::Template::from_rgba(
            self.crop.width() as usize,
            self.crop.height() as usize,
            self.crop.as_raw(),
        )
    }
}

/// The selections `--label` selects, with their crops decoded and their
/// origins computed. Refuses an unknown label by naming what the session
/// carries, and an empty session before that — the two are different
/// mistakes.
pub fn load<'a>(
    session: &'a SessionFile,
    dir: &Path,
    label: Option<&str>,
) -> Result<Vec<Region<'a>>> {
    anyhow::ensure!(
        !session.selections.is_empty(),
        "the session has no selections"
    );
    let wanted = pixelcoords_core::session::select_by_label(session, label);
    if wanted.is_empty() {
        let labels = pixelcoords_core::session::distinct_labels(session.selections.iter());
        anyhow::bail!(
            "no selection is labeled {:?}; labels in this session: {labels:?}",
            label.unwrap_or_default()
        );
    }
    wanted
        .into_iter()
        .map(|(index, record)| {
            // The session-side monitor lookup stays on the index: it
            // addresses records *within* one session, where the index is
            // the record's own identifier and cannot shuffle.
            let monitor = session
                .monitors
                .iter()
                .find(|m| m.index == record.monitor)
                .with_context(|| {
                    format!("the session does not describe monitor {}", record.monitor)
                })?;
            let crop_path = dir.join(&record.crop);
            let crop = image::open(&crop_path)
                .with_context(|| format!("reading crop {}", crop_path.display()))?
                .to_rgba8();
            Ok(Region {
                index,
                record,
                monitor: record.monitor,
                origin: locate::crop_origin(
                    &record.px,
                    record.rot_deg.unwrap_or(0),
                    monitor.size_px,
                ),
                crop,
            })
        })
        .collect()
}

/// One capture per monitor these regions live on, each checked against
/// the session by identity first. Refused up front when a display no
/// longer matches: template matching survives a window moving, not the
/// pixels underneath it being resampled.
pub fn capture_frames<P: CaptureProvider>(
    provider: &P,
    session: &SessionFile,
    regions: &[Region],
) -> Result<HashMap<usize, RgbaImage>> {
    let current = provider.monitors()?;

    // Every attached display in the shape the matcher compares against.
    // Built once: the loop below asks about each monitor the selections
    // live on, and re-deriving this per iteration would be the same work
    // repeated.
    let live: Vec<MonitorRecord> = current.iter().map(record_from_monitor).collect();

    let mut frames: HashMap<usize, RgbaImage> = HashMap::new();
    for index in regions.iter().map(|r| r.monitor) {
        if frames.contains_key(&index) {
            continue;
        }
        let record = session
            .monitors
            .iter()
            .find(|m| m.index == index)
            .with_context(|| format!("the session does not describe monitor {index}"))?;
        // The live lookup does not stay on the index, because enumeration
        // order shuffles across replugs and reboots. See `match_monitor`.
        let monitor = live_monitor_for(record, &current, &live)?;
        frames.insert(index, provider.capture(monitor)?);
    }
    Ok(frames)
}

/// A live monitor in the shape `match_monitor` compares against — the same
/// normalization the save path applies, so an attached display and the
/// record written from it are directly comparable.
pub fn record_from_monitor(monitor: &capture::MonitorInfo) -> MonitorRecord {
    MonitorRecord {
        index: monitor.index,
        name: monitor.name.clone(),
        primary: monitor.primary,
        origin_px: monitor.origin_physical(),
        size_px: monitor.size_physical(),
        scale: monitor.scale,
    }
}

/// Resolve one of a session's monitors against the displays attached now,
/// or refuse with the reason. The two refusals are deliberately different
/// sentences: a display that is *gone* sends the user to a cable, and one
/// that *changed* sends them to display settings — the old shared message
/// ("no longer attached") sent everyone to the cable.
fn live_monitor_for<'a>(
    record: &MonitorRecord,
    current: &'a [capture::MonitorInfo],
    live: &[MonitorRecord],
) -> Result<&'a capture::MonitorInfo> {
    match pixelcoords_core::session::match_monitor(record, live) {
        MonitorMatch::Found(i) => Ok(&current[i]),
        MonitorMatch::Changed(i) => {
            let now = current[i].size_physical();
            anyhow::bail!(
                "{} changed since the session ({}x{} scale {} now, {}x{} scale {} then) — \
                 relocation needs the same display setup",
                record.name,
                now.w,
                now.h,
                current[i].scale,
                record.size_px.w,
                record.size_px.h,
                record.scale
            )
        }
        MonitorMatch::Missing => {
            let attached: Vec<&str> = current.iter().map(|m| m.name.as_str()).collect();
            anyhow::bail!(
                "the session's monitor {} ({}, {}x{} scale {}) is not attached — \
                 attached now: {attached:?}",
                record.index,
                record.name,
                record.size_px.w,
                record.size_px.h,
                record.scale
            )
        }
    }
}

/// Refuse before capturing when macOS has not granted screen recording.
/// Four commands capture now; four copies of this check is how one of
/// them ends up without it.
#[cfg(target_os = "macos")]
pub fn ensure_capture_permission(command: &str) -> Result<()> {
    if crate::mac::has_screen_capture_access() || crate::mac::request_screen_capture_access() {
        return Ok(());
    }
    anyhow::bail!(
        "pixelcoords {command}: screen recording permission denied — run \
         `pixelcoords doctor` for instructions"
    )
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_capture_permission(_command: &str) -> Result<()> {
    Ok(())
}
