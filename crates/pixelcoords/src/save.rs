//! Session output: `session.json`, the full screenshot, one PNG crop per
//! selection (circle crops get the outside-alpha mask), and a frame-sized
//! cutout per monitor with selections — the frame with everything outside
//! them transparent.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::RgbaImage;
use pixelcoords_core::draw::{
    apply_alpha_mask_outside, apply_cutout_mask, apply_inverse_cutout_mask,
};
use pixelcoords_core::geometry::{Rect, Shape, Size};
use pixelcoords_core::selection::SelectionSet;
use pixelcoords_core::session::{CaptureKind, MonitorRecord, SessionFile, TargetRecord};

use crate::capture::MonitorInfo;

/// A crop already on disk: its file name and the geometry it was rendered
/// from. The frozen frame never changes, so a crop whose name, shape, and
/// rotation all match is byte-for-byte what a re-encode would produce.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenCrop {
    pub name: String,
    pub shape: pixelcoords_core::geometry::Shape,
    pub rot_deg: i32,
    /// Which monitor the selection sat on — the cutout of a monitor is
    /// re-encoded only when its own selections changed.
    pub monitor: usize,
}

/// Session provenance carried into every save: the OS it was captured on
/// and how (desktop, targeted window, portal pick). A resumed session
/// passes through what it loaded rather than restamping this machine.
#[derive(Debug, Clone, Default)]
pub struct SessionMeta {
    pub platform: Option<String>,
    pub capture: Option<CaptureKind>,
    /// The friendly session name shown by pickers.
    pub name: Option<String>,
}

/// What a successful save produced; `crops` feeds the next save's
/// stale-crop cleanup and lets it skip re-encoding what has not changed.
pub struct SaveOutcome {
    pub json_path: PathBuf,
    pub crops: Vec<WrittenCrop>,
}

/// Write everything into `dir`. `frames` is one (monitor, frozen capture)
/// pair per captured monitor.
///
/// `first_save` forces the screenshots to be written; on later saves in the
/// same session they are skipped if present — the frozen frames never
/// change, and re-encoding multi-monitor Retina PNGs on every W would block
/// the UI for seconds.
///
/// `previous` is what THIS session wrote last save. Crops no longer
/// produced are removed, best-effort, only after everything new is on disk;
/// crops whose geometry is unchanged are left alone rather than re-encoded.
/// Files pixelcoords didn't write are never touched, and a failed save
/// never deletes anything.
pub fn write_session(
    dir: &Path,
    frames: &[(&MonitorInfo, &RgbaImage)],
    selections: &SelectionSet,
    target: Option<&TargetRecord>,
    meta: &SessionMeta,
    first_save: bool,
    previous: &[WrittenCrop],
) -> Result<SaveOutcome> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    if first_save {
        ensure_ours_to_write(dir)?;
    }

    let mut records = Vec::new();
    for (monitor, frame) in frames {
        let screenshot_name = format!("screenshot-{}.png", monitor.index);
        let path = dir.join(&screenshot_name);
        if first_save || !path.exists() {
            frame
                .save(&path)
                .with_context(|| format!("writing {screenshot_name}"))?;
        }
        records.push(MonitorRecord {
            index: monitor.index,
            name: monitor.name.clone(),
            primary: monitor.primary,
            // Normalized per-platform: exact for uniform-DPI layouts and
            // the single-monitor case; mixed-DPI global layouts are
            // inherently approximate.
            origin_px: monitor.origin_physical(),
            size_px: Size::new(frame.width() as i32, frame.height() as i32),
            scale: monitor.scale,
        });
    }

    let mut crops = Vec::new();
    for (i, sel) in selections.items().iter().enumerate() {
        let (_, frame) = frames
            .iter()
            .find(|(m, _)| m.index == sel.monitor)
            .with_context(|| format!("selection {i} references unknown monitor {}", sel.monitor))?;
        let crop = WrittenCrop {
            name: crop_file_name(i, &sel.label),
            shape: sel.shape.clone(),
            rot_deg: sel.rot_deg,
            monitor: sel.monitor,
        };
        // Re-encoding every crop on every save costs a PNG compression per
        // selection, on the event-loop thread; a session with many shapes
        // froze the overlay for seconds each time. The frozen frame never
        // changes, so an identical crop already on disk is identical.
        if !previous.contains(&crop) || !dir.join(&crop.name).exists() {
            write_crop(dir, &crop.name, frame, crop.shape.clone(), crop.rot_deg)?;
        }
        crops.push(crop);
    }

    // The composite cutouts, one pair per monitor holding selections:
    // `primary` keeps the selections in place and clears the rest,
    // `inverse` punches them out and keeps the rest. Same re-encode
    // economics as the crops — a monitor whose selections did not change
    // since the last save keeps its files.
    for (monitor, frame) in frames {
        let names = [
            cutout_file_name("primary", monitor.index),
            cutout_file_name("inverse", monitor.index),
        ];
        let current = shapes_on(&crops, monitor.index);
        if current.is_empty() {
            // Every selection here may have been deleted since the last
            // save; leftover cutouts would misrepresent this one.
            for name in &names {
                let path = dir.join(name);
                if path.exists()
                    && let Err(e) = std::fs::remove_file(&path)
                {
                    log::warn!("could not remove stale cutout {name}: {e}");
                }
            }
            continue;
        }
        let on_disk = names.iter().all(|n| dir.join(n).exists());
        if shapes_on(previous, monitor.index) == current && on_disk {
            continue;
        }
        write_cutouts(dir, &names, frame, &current)?;
    }

    let created = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting timestamp")?;
    let session = SessionFile::build(
        env!("CARGO_PKG_VERSION"),
        created,
        records,
        selections.items(),
        &crop_names(&crops),
        target.cloned(),
    )
    .with_meta(meta.platform.clone(), meta.capture, meta.name.clone());

    let json_path = dir.join("session.json");
    let json = serde_json::to_string_pretty(&session).context("serializing session")?;
    std::fs::write(&json_path, json).with_context(|| format!("writing {}", json_path.display()))?;

    // Everything new is on disk; now retire crops from the previous save
    // that this save no longer produced. Best-effort: a stubborn file is a
    // warning, not a failed save.
    for stale in previous {
        if crops.iter().any(|c| c.name == stale.name) {
            continue;
        }
        if let Err(e) = std::fs::remove_file(dir.join(&stale.name)) {
            log::warn!("could not remove stale crop {}: {e}", stale.name);
        }
    }

    Ok(SaveOutcome { json_path, crops })
}

/// Refuse to write into a directory holding files with our names that we
/// did not put there.
///
/// Cleanup already refuses to delete foreign files, but writing did not:
/// a `session.json`, `screenshot-0.png`, or `crop-0.png` belonging to
/// anything else was overwritten by the first save without a word.
///
/// A previous pixelcoords run is not foreign — re-running into the same
/// `--out` is ordinary use — so a `session.json` this tool wrote makes the
/// directory ours and the save proceeds. Anything else, including a
/// `session.json` that is not ours or no longer parses, stops the save.
fn ensure_ours_to_write(dir: &Path) -> Result<()> {
    let session = dir.join("session.json");
    if session.exists() {
        anyhow::ensure!(
            is_our_session(&session),
            "{} already holds a session.json that pixelcoords did not write; \
             choose another --out rather than overwrite it",
            dir.display()
        );
        return Ok(());
    }
    // No session.json to vouch for the directory, so anything wearing our
    // names came from somewhere else.
    let occupied = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| is_output_name(name));
    let Some(name) = occupied else {
        return Ok(());
    };
    anyhow::bail!(
        "{} already holds {name}, which pixelcoords did not write; \
         choose another --out rather than overwrite it",
        dir.display()
    )
}

/// The file names of `crops`, in order — what `session.json` records.
fn crop_names(crops: &[WrittenCrop]) -> Vec<String> {
    crops.iter().map(|c| c.name.clone()).collect()
}

/// Whether a `session.json` is one this tool produced.
fn is_our_session(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value["app"]["name"] == "pixelcoords"
}

/// Whether a file name is one a save would write over.
fn is_output_name(name: &str) -> bool {
    if name == "session.json" {
        return true;
    }
    if Path::new(name).extension() != Some(std::ffi::OsStr::new("png")) {
        return false;
    }
    name.starts_with("screenshot-") || name.starts_with("crop-") || name.starts_with("cutout-")
}

/// The `(shape, rotation)` set a monitor's cutout renders, in save order.
fn shapes_on(crops: &[WrittenCrop], monitor: usize) -> Vec<(Shape, i32)> {
    crops
        .iter()
        .filter(|c| c.monitor == monitor)
        .map(|c| (c.shape.clone(), c.rot_deg))
        .collect()
}

fn cutout_file_name(kind: &str, index: usize) -> String {
    format!("cutout-{kind}-{index}.png")
}

/// The cutout pair: the frame with everything outside `shapes` cleared
/// (`names[0]`, primary) and its exact complement with the shapes
/// punched out (`names[1]`, inverse). Triangles bake their rotation
/// exactly as crops do, so all three artifacts agree pixel-for-pixel.
fn write_cutouts(
    dir: &Path,
    names: &[String; 2],
    frame: &RgbaImage,
    shapes: &[(Shape, i32)],
) -> Result<()> {
    let baked: Vec<(Shape, i32)> = shapes
        .iter()
        .map(|(shape, deg)| match shape {
            t @ (Shape::Triangle { .. } | Shape::Poly { .. }) => (t.with_rotation_baked(*deg), 0),
            s => (s.clone(), *deg),
        })
        .collect();
    let (w, h) = (frame.width() as i32, frame.height() as i32);

    let mut primary = frame.clone();
    apply_cutout_mask(primary.as_mut(), w, h, &baked);
    primary
        .save(dir.join(&names[0]))
        .with_context(|| format!("writing {}", names[0]))?;

    let mut inverse = frame.clone();
    apply_inverse_cutout_mask(inverse.as_mut(), w, h, &baked);
    inverse
        .save(dir.join(&names[1]))
        .with_context(|| format!("writing {}", names[1]))
}

fn crop_file_name(index: usize, label: &str) -> String {
    let slug: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return format!("crop-{index}.png");
    }
    format!("crop-{index}-{slug}.png")
}

fn write_crop(
    dir: &Path,
    name: &str,
    frame: &RgbaImage,
    shape: pixelcoords_core::geometry::Shape,
    rot_deg: i32,
) -> Result<()> {
    // Crop in the same representation session.json stores: triangles bake
    // rotation into their vertices, so the crop origin equals the stored
    // shape's bbox and a consumer can align the two exactly.
    let (shape, rot_deg) = match shape {
        s @ pixelcoords_core::geometry::Shape::Triangle { .. } => {
            (s.with_rotation_baked(rot_deg), 0)
        }
        s => (s, rot_deg),
    };
    let frame_bounds = Rect::new(0, 0, frame.width() as i32, frame.height() as i32);
    let bbox = shape.rotated_bbox(rot_deg);
    let x0 = bbox.x.max(frame_bounds.x);
    let y0 = bbox.y.max(frame_bounds.y);
    let x1 = (bbox.x + bbox.w).min(frame_bounds.w);
    let y1 = (bbox.y + bbox.h).min(frame_bounds.h);
    anyhow::ensure!(
        x1 > x0 && y1 > y0,
        "selection {name} lies outside the capture"
    );

    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let mut crop = image::imageops::crop_imm(frame, x0 as u32, y0 as u32, w, h).to_image();

    // Anything but an unrotated rect is transparent outside the shape.
    let axis_aligned_rect = matches!(shape, pixelcoords_core::geometry::Shape::Rect(_))
        && pixelcoords_core::geometry::normalize_deg(rot_deg) == 0;
    if !axis_aligned_rect {
        let local = shape.translated(-x0, -y0);
        apply_alpha_mask_outside(crop.as_mut(), w as i32, h as i32, &local, rot_deg);
    }

    crop.save(dir.join(name))
        .with_context(|| format!("writing {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixelcoords_core::geometry::{Point, Shape};
    use pixelcoords_core::selection::Selection;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pixelcoords-guard-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_directory_is_ours_to_write() {
        let dir = scratch_dir("empty");
        assert!(ensure_ours_to_write(&dir).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unrelated_files_do_not_block_a_save() {
        let dir = scratch_dir("unrelated");
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        std::fs::write(dir.join("photo.png"), b"mine").unwrap();
        assert!(ensure_ours_to_write(&dir).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_foreign_file_wearing_our_name_stops_the_save() {
        // Previously the first save overwrote this without a word.
        let dir = scratch_dir("foreign");
        std::fs::write(dir.join("crop-0.png"), b"someone else's work").unwrap();
        let err = ensure_ours_to_write(&dir).unwrap_err();
        assert!(format!("{err}").contains("crop-0.png"), "{err}");
        // And the file is still intact.
        assert_eq!(
            std::fs::read(dir.join("crop-0.png")).unwrap(),
            b"someone else's work"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_foreign_session_json_stops_the_save() {
        let dir = scratch_dir("foreign-session");
        std::fs::write(dir.join("session.json"), br#"{"app":{"name":"other"}}"#).unwrap();
        assert!(ensure_ours_to_write(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unparseable_session_json_stops_the_save() {
        let dir = scratch_dir("bad-session");
        std::fs::write(dir.join("session.json"), b"not json at all").unwrap();
        assert!(ensure_ours_to_write(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn our_own_previous_run_is_not_foreign() {
        // Re-running into the same --out is ordinary use and must keep
        // working, so a session.json we wrote vouches for the directory.
        let dir = scratch_dir("ours");
        std::fs::write(
            dir.join("session.json"),
            br#"{"app":{"name":"pixelcoords","version":"0.1.0"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("crop-0.png"), b"ours").unwrap();
        assert!(ensure_ours_to_write(&dir).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn checker_frame(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255])
        })
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            index: 0,
            name: "Test".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        }
    }

    #[test]
    fn cutout_keeps_selections_in_place_and_only_where_selections_exist() {
        let dir = std::env::temp_dir().join("pixelcoords-test-cutout");
        let _ = std::fs::remove_dir_all(&dir);
        let frame_a = checker_frame(100, 60);
        let frame_b = checker_frame(100, 60);
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(10, 20, 30, 15)),
            0,
        ));
        let m0 = monitor();
        let m1 = MonitorInfo {
            index: 1,
            name: "Second".into(),
            primary: false,
            origin: Point::new(100, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        write_session(
            &dir,
            &[(&m0, &frame_a), (&m1, &frame_b)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();

        // Only the monitor holding a selection gets cutouts.
        assert!(!dir.join("cutout-primary-1.png").exists());
        assert!(!dir.join("cutout-inverse-1.png").exists());
        let primary = image::open(dir.join("cutout-primary-0.png"))
            .unwrap()
            .to_rgba8();
        // Frame-sized, selection pixels in place, the rest transparent.
        assert_eq!((primary.width(), primary.height()), (100, 60));
        assert_eq!(primary.get_pixel(15, 25), frame_a.get_pixel(15, 25));
        assert_eq!(primary.get_pixel(0, 0)[3], 0);
        assert_eq!(primary.get_pixel(80, 50)[3], 0);
        // The inverse is the exact complement: selection punched out, the
        // rest kept.
        let inverse = image::open(dir.join("cutout-inverse-0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(inverse.get_pixel(15, 25)[3], 0);
        assert_eq!(inverse.get_pixel(0, 0), frame_a.get_pixel(0, 0));
        assert_eq!(inverse.get_pixel(80, 50), frame_a.get_pixel(80, 50));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resave_skips_unchanged_cutouts_and_removes_emptied_ones() {
        let dir = std::env::temp_dir().join("pixelcoords-test-cutout-resave");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(10, 20, 30, 15)),
            0,
        ));
        let m = monitor();
        let first = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();

        // Unchanged geometry: the files are left alone, not re-encoded —
        // sentinel bytes survive the resave.
        std::fs::write(dir.join("cutout-primary-0.png"), b"sentinel").unwrap();
        std::fs::write(dir.join("cutout-inverse-0.png"), b"sentinel").unwrap();
        let second = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            false,
            &first.crops,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(dir.join("cutout-primary-0.png")).unwrap(),
            b"sentinel"
        );
        assert_eq!(
            std::fs::read(dir.join("cutout-inverse-0.png")).unwrap(),
            b"sentinel"
        );

        // Moved geometry: both re-encoded.
        let mut moved = SelectionSet::new();
        moved.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(40, 20, 30, 15)),
            0,
        ));
        let third = write_session(
            &dir,
            &[(&m, &frame)],
            &moved,
            None,
            &SessionMeta::default(),
            false,
            &second.crops,
        )
        .unwrap();
        let primary = image::open(dir.join("cutout-primary-0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(primary.get_pixel(45, 25), frame.get_pixel(45, 25));
        assert_eq!(primary.get_pixel(15, 25)[3], 0, "old position now cleared");
        let inverse = image::open(dir.join("cutout-inverse-0.png"))
            .unwrap()
            .to_rgba8();
        assert_eq!(inverse.get_pixel(45, 25)[3], 0);

        // Every selection deleted: both cutouts go with them.
        let empty = SelectionSet::new();
        write_session(
            &dir,
            &[(&m, &frame)],
            &empty,
            None,
            &SessionMeta::default(),
            false,
            &third.crops,
        )
        .unwrap();
        assert!(!dir.join("cutout-primary-0.png").exists());
        assert!(!dir.join("cutout-inverse-0.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_foreign_cutout_stops_the_save() {
        let dir = scratch_dir("foreign-cutout");
        std::fs::write(dir.join("cutout-inverse-0.png"), b"someone else's").unwrap();
        let err = ensure_ours_to_write(&dir).unwrap_err().to_string();
        assert!(err.contains("cutout-inverse-0.png"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn crop_names_slugify_labels() {
        assert_eq!(crop_file_name(0, ""), "crop-0.png");
        assert_eq!(
            crop_file_name(1, "Login Button!"),
            "crop-1-login-button.png"
        );
        assert_eq!(crop_file_name(2, "---"), "crop-2.png");
    }

    #[test]
    fn resave_skips_crops_whose_geometry_is_unchanged() {
        // Every save used to re-encode every crop, so a session with many
        // shapes stalled the event loop on each W. The frozen frame never
        // changes, so only moved or renamed shapes need rewriting.
        let dir = scratch_dir("unchanged-crops");
        let frame = checker_frame(100, 60);
        let m = monitor();
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(10, 20, 30, 15)),
            0,
        ));
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(50, 20, 20, 15)),
            0,
        ));
        let first = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();
        let untouched = std::fs::metadata(dir.join("crop-0.png"))
            .unwrap()
            .modified()
            .unwrap();

        // Move only the second selection, then save again.
        selections.set_shape_live(
            1,
            Shape::Rect(pixelcoords_core::geometry::Rect::new(5, 5, 9, 9)),
        );
        let second = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            false,
            &first.crops,
        )
        .unwrap();

        assert_eq!(
            std::fs::metadata(dir.join("crop-0.png"))
                .unwrap()
                .modified()
                .unwrap(),
            untouched,
            "an unchanged selection must not be re-encoded"
        );
        assert_ne!(
            second.crops[1].shape, first.crops[1].shape,
            "the moved selection is recorded with its new geometry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resave_removes_stale_crops_and_skips_screenshot_reencode() {
        let dir = std::env::temp_dir().join("pixelcoords-test-resave");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let m = monitor();

        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(10, 20, 30, 15)),
            0,
        ));
        selections.items(); // two crops on first save
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(50, 20, 20, 15)),
            0,
        ));
        let first = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();
        assert!(dir.join("crop-1.png").exists());
        // A file pixelcoords did not write must never be touched.
        std::fs::write(dir.join("crop-not-ours.png"), b"user data").unwrap();
        let screenshot_mtime = std::fs::metadata(dir.join("screenshot-0.png"))
            .unwrap()
            .modified()
            .unwrap();

        // Delete one selection and re-save: the orphaned crop disappears
        // and the frozen screenshot is not re-encoded.
        selections.delete(1);
        write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            false,
            &first.crops,
        )
        .unwrap();
        assert!(dir.join("crop-0.png").exists());
        assert!(
            !dir.join("crop-1.png").exists(),
            "stale crop must be removed"
        );
        assert!(
            dir.join("crop-not-ours.png").exists(),
            "foreign files must never be deleted"
        );
        assert_eq!(
            std::fs::metadata(dir.join("screenshot-0.png"))
                .unwrap()
                .modified()
                .unwrap(),
            screenshot_mtime,
            "screenshot must not be re-encoded on re-save"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_crop_matches_frame_subregion() {
        let dir = std::env::temp_dir().join("pixelcoords-test-crop");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(10, 20, 30, 15)),
            0,
        ));

        let m = monitor();
        let json_path = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap()
        .json_path;
        assert!(json_path.exists());

        let crop = image::open(dir.join("crop-0.png")).unwrap().to_rgba8();
        assert_eq!((crop.width(), crop.height()), (30, 15));
        assert_eq!(crop.get_pixel(0, 0), frame.get_pixel(10, 20));
        assert_eq!(crop.get_pixel(29, 14), frame.get_pixel(39, 34));

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["selections"][0]["crop"], "crop-0.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_monitor_session_writes_all_screenshots_and_offsets_globals() {
        let dir = std::env::temp_dir().join("pixelcoords-test-multimon");
        let _ = std::fs::remove_dir_all(&dir);
        let frame0 = checker_frame(100, 60);
        let frame1 = checker_frame(80, 50);
        let m0 = monitor();
        let m1 = MonitorInfo {
            index: 1,
            name: "Second".into(),
            primary: false,
            origin: Point::new(100, 0),
            size_native: Size::new(80, 50),
            scale: 1.0,
        };
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Rect(pixelcoords_core::geometry::Rect::new(5, 6, 10, 10)),
            1,
        ));

        let json_path = write_session(
            &dir,
            &[(&m0, &frame0), (&m1, &frame1)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap()
        .json_path;
        assert!(dir.join("screenshot-0.png").exists());
        assert!(dir.join("screenshot-1.png").exists());

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(json["monitors"].as_array().unwrap().len(), 2);
        // global = origin_px (100, 0) + local (5, 6).
        assert_eq!(json["selections"][0]["global_px"]["x"], 105);
        assert_eq!(json["selections"][0]["global_px"]["y"], 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn triangle_crop_is_masked_outside() {
        let dir = std::env::temp_dir().join("pixelcoords-test-tri-crop");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Triangle {
                ax: 50,
                ay: 10,
                bx: 30,
                by: 50,
                cx: 70,
                cy: 50,
            },
            0,
        ));

        let m = monitor();
        write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();
        let crop = image::open(dir.join("crop-0.png")).unwrap().to_rgba8();
        assert_eq!((crop.width(), crop.height()), (40, 40));
        // Crop-local: apex is at (20, 0); centroid opaque, top corners not.
        assert_eq!(crop.get_pixel(20, 30)[3], 255, "interior opaque");
        assert_eq!(crop.get_pixel(0, 0)[3], 0, "top-left transparent");
        assert_eq!(crop.get_pixel(39, 0)[3], 0, "top-right transparent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotated_triangle_crop_aligns_with_stored_baked_shape() {
        let dir = std::env::temp_dir().join("pixelcoords-test-rot-tri-crop");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let tri = Shape::Triangle {
            ax: 50,
            ay: 10,
            bx: 30,
            by: 50,
            cx: 70,
            cy: 50,
        };
        let mut sel = Selection::new(tri.clone(), 0);
        sel.rot_deg = 90;
        let mut selections = SelectionSet::new();
        selections.add(sel);

        let m = monitor();
        let outcome = write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&outcome.json_path).unwrap()).unwrap();

        // The stored (baked) triangle's bbox must exactly match the crop
        // dimensions, so a consumer can align crop to coordinates.
        let baked = tri.with_rotation_baked(90);
        let bb = baked.bbox();
        let crop = image::open(dir.join(&outcome.crops[0].name))
            .unwrap()
            .to_rgba8();
        assert_eq!((crop.width() as i32, crop.height() as i32), (bb.w, bb.h));
        // And the stored px really is the baked shape.
        assert_eq!(
            json["selections"][0]["px"],
            serde_json::to_value(baked).unwrap()
        );
        assert!(json["selections"][0].get("rot_deg").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn circle_crop_is_masked_outside() {
        let dir = std::env::temp_dir().join("pixelcoords-test-circle-crop");
        let _ = std::fs::remove_dir_all(&dir);
        let frame = checker_frame(100, 60);
        let mut selections = SelectionSet::new();
        selections.add(Selection::new(
            Shape::Circle {
                cx: 50,
                cy: 30,
                r: 10,
            },
            0,
        ));

        let m = monitor();
        write_session(
            &dir,
            &[(&m, &frame)],
            &selections,
            None,
            &SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();
        let crop = image::open(dir.join("crop-0.png")).unwrap().to_rgba8();
        assert_eq!((crop.width(), crop.height()), (20, 20));
        assert_eq!(crop.get_pixel(10, 10)[3], 255, "center opaque");
        assert_eq!(crop.get_pixel(0, 0)[3], 0, "corner transparent");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
