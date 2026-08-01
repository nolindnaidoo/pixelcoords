mod app;
mod capture;
mod cli;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
mod regions;
mod render;
mod save;
mod state;
mod view;
#[cfg(target_os = "windows")]
mod win;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use pixelcoords_core::config::Config;

use crate::capture::{CaptureProvider, NATIVE_COORD_SPACE, XcapCapture};

fn main() -> Result<()> {
    // The failure mode of a fullscreen overlay tool is "the user's screen
    // is covered and something just died" — make the last words useful.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("pixelcoords crashed: {info}");
        eprintln!(
            "this is a bug — please report it at \
             https://github.com/nolindnaidoo/pixelcoords/issues"
        );
    }));
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Must precede every capture call: xcap reads the process awareness
    // state to decide whether it may ask for exact per-monitor DPI, and
    // winit would only set it when the event loop starts — after capture,
    // and never at all for the subcommands.
    #[cfg(target_os = "windows")]
    {
        let established = win::become_dpi_aware();
        log::debug!("per-monitor DPI awareness established by this call: {established}");
    }

    let args = cli::Cli::parse();

    match args.command {
        Some(cli::Command::Doctor { ref config, json }) => {
            let config = config.as_deref().or(args.config.as_deref());
            let healthy = if json {
                doctor_json(config)?
            } else {
                doctor(config)
            };
            if healthy {
                return Ok(());
            }
            // Nonzero so scripts can gate on permission/config state.
            std::process::exit(1)
        }
        Some(cli::Command::Windows { json }) => list_windows(&XcapCapture, json),
        Some(cli::Command::Shoot { out, ref monitor }) => shoot(out.or(args.out.clone()), monitor),
        Some(cli::Command::Assert {
            ref session,
            ref point,
            ref expect,
            ref label,
            space,
            monitor,
        }) => {
            if label.is_some() {
                eprintln!(
                    "pixelcoords assert: --label changed meaning in this release. \
                     Use --expect NAME for the old behavior — the region the point \
                     must land in. --label now restricts which regions a command \
                     looks at, matching find and emit, and assert accepts it with \
                     that meaning in the next release."
                );
                std::process::exit(2)
            }
            assert_command(session, point, expect.as_deref(), space, monitor)
        }
        Some(cli::Command::Emit {
            ref session,
            format,
            ref label,
        }) => {
            print!("{}", emit_snippet(session, format, label.as_deref())?);
            Ok(())
        }
        Some(cli::Command::Resume {
            ref session,
            last,
            ref out,
        }) => {
            let root = captures_root(dirs::download_dir());
            let path = resolve_resume_session(session.as_deref(), last, &root)?;
            run_resume(&args, &path, out.clone())
        }
        Some(cli::Command::Rename {
            ref session,
            ref name,
        }) => {
            let root = captures_root(dirs::download_dir());
            let path = resolve_resume_session(Some(session), false, &root)?;
            rename_session(&path, name)
        }
        Some(cli::Command::Find {
            ref session,
            ref label,
        }) => {
            let report = match run_find(&XcapCapture, session, label.as_deref()) {
                Ok(r) => r,
                // 2, not 1: "a region was not found" and "the question was
                // malformed" must stay distinguishable to a script.
                Err(e) => {
                    eprintln!("pixelcoords find: {e:#}");
                    std::process::exit(2)
                }
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.ok {
                return Ok(());
            }
            std::process::exit(1)
        }
        None => run_overlay(&args),
    }
}

/// Reopen a saved session: the frozen screenshots become the frames,
/// every selection is editable again, and saves land back in the session
/// directory (or `--out`) with the resave machinery treating the session
/// as its own previous save.
fn run_resume(args: &cli::Cli, path: &std::path::Path, out: Option<PathBuf>) -> Result<()> {
    let config = load_config(args.config.as_deref())?;
    let style = config.resolve_style()?;
    let bindings = config
        .resolve_bindings(&args.bind)
        .context("invalid hotkey binding")?;

    let session = load_session(path)?;
    anyhow::ensure!(
        !session.monitors.is_empty(),
        "the session describes no monitors"
    );
    let dir = session_dir(path);
    let mut frames = Vec::new();
    for record in &session.monitors {
        let file = dir.join(format!("screenshot-{}.png", record.index));
        let img = image::open(&file)
            .with_context(|| format!("reading {}", file.display()))?
            .to_rgba8();
        anyhow::ensure!(
            img.width() as i32 == record.size_px.w && img.height() as i32 == record.size_px.h,
            "{} is {}x{}, but the session records {}x{} — the directory's              files do not match its session.json",
            file.display(),
            img.width(),
            img.height(),
            record.size_px.w,
            record.size_px.h
        );
        let mut frame = app::MonitorFrame::new(monitor_from_record(record), img);
        // Resumed target sessions carry the same "drawable region is the
        // window" constraint their originals did.
        if let Some(target) = &session.target
            && target.monitor == frame.info.index
        {
            frame = frame.with_draw_rect(pixelcoords_core::geometry::Rect::new(
                target.origin_px.x,
                target.origin_px.y,
                target.size_px.w,
                target.size_px.h,
            ));
        }
        frames.push(frame);
    }

    let (selections, dropped) = pixelcoords_core::session::restore_selections(&session);
    if !dropped.is_empty() {
        eprintln!(
            "dropped {} selection(s) that landed outside the target window: {}",
            dropped.len(),
            dropped.join(", ")
        );
    }
    let out_dir = out.unwrap_or_else(|| dir.clone());
    let in_place = out_dir == dir;
    // Drop the crops that belonged to the filtered selections so the
    // resave path does not try to re-use them.
    let previous: Vec<crate::save::WrittenCrop> = session
        .selections
        .iter()
        .filter(|record| !dropped.contains(&record.label))
        .map(|record| crate::save::WrittenCrop {
            name: record.crop.clone(),
            shape: record.px.clone(),
            rot_deg: record.rot_deg.unwrap_or(0),
            monitor: record.monitor,
        })
        .collect();

    // Windowed like a picked session: a resumed session is a document,
    // not a live screen, so it opens on any monitor layout.
    let mut app = app::App::new(
        frames,
        style,
        bindings,
        out_dir,
        session.target.clone(),
        view::Presentation::Windowed,
    );
    // Provenance passes through untouched: the file keeps saying where it
    // was captured, not where it was edited.
    app.set_session_meta(crate::save::SessionMeta {
        platform: session.platform.clone(),
        capture: session.capture,
        name: session.name.clone(),
    });
    app.restore_session(selections, previous, in_place);
    app.restore_panel(state::load_panel());
    app.run()
}

/// A session's stored monitor record back into the capture backend's
/// native space — the exact inverse of what the save path recorded.
fn monitor_from_record(record: &pixelcoords_core::session::MonitorRecord) -> capture::MonitorInfo {
    use pixelcoords_core::geometry::{Point, Size};
    let from = |v: i32| crate::capture::from_physical(v, record.scale, NATIVE_COORD_SPACE);
    capture::MonitorInfo {
        index: record.index,
        name: record.name.clone(),
        primary: record.primary,
        origin: Point::new(from(record.origin_px.x), from(record.origin_px.y)),
        size_native: Size::new(from(record.size_px.w), from(record.size_px.h)),
        scale: record.scale,
    }
}

/// Re-locate every selection of a saved session in a fresh capture, using
/// each selection's crop as its search template.
fn run_find<P: CaptureProvider>(
    provider: &P,
    path: &std::path::Path,
    label: Option<&str>,
) -> Result<pixelcoords_core::report::Report<pixelcoords_core::locate::FindResult>> {
    use pixelcoords_core::locate;

    let session = load_session(path)?;
    let regions = regions::load(&session, &session_dir(path), label)?;
    let frames = regions::capture_frames(provider, &session, &regions)?;

    // Grayscale once per frame, not once per selection: several regions
    // routinely share a monitor. Consumed rather than borrowed so each
    // captured frame is freed as it is converted — on three 4K displays,
    // holding both representations at once would double peak memory for
    // no reason, since matching never looks at colour.
    let gray: std::collections::HashMap<usize, locate::GrayImage> = frames
        .into_iter()
        .map(|(index, img)| {
            let (w, h) = (img.width() as usize, img.height() as usize);
            (index, locate::GrayImage::from_rgba(w, h, img.as_raw()))
        })
        .collect();

    let attempts: Vec<(usize, &pixelcoords_core::session::SelectionRecord, _)> = regions
        .iter()
        .map(|region| {
            let frame = &gray[&region.monitor];
            (
                region.index,
                region.record,
                locate::Relocation {
                    crop_origin: region.origin,
                    outcome: locate::locate(frame, &region.template(), Some(region.origin)),
                },
            )
        })
        .collect();

    let captured = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("formatting timestamp")?;
    Ok(locate::report(&attempts, captured))
}

/// One saved session the resume picker can offer.
struct SessionEntry {
    path: PathBuf,
    name: String,
    summary: String,
    /// RFC3339 sorts lexically, so ordering needs no clock.
    created: String,
}

/// Which session to resume: an explicit path, a bare folder name under
/// the captures root, the newest (`--last`), or an interactive pick over
/// everything the root holds.
fn resolve_resume_session(
    session: Option<&std::path::Path>,
    last: bool,
    root: &std::path::Path,
) -> Result<PathBuf> {
    if let Some(path) = session {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        // A bare name is an id under the captures root.
        let named = root.join(path);
        anyhow::ensure!(
            named.join("session.json").exists(),
            "{} is neither a path nor a session name under {}",
            path.display(),
            root.display()
        );
        return Ok(named);
    }
    let sessions = sessions_under(root);
    anyhow::ensure!(
        !sessions.is_empty(),
        "no sessions under {} — mark and save one first, or pass --session",
        root.display()
    );
    if last {
        return Ok(sessions[0].path.clone());
    }
    pick_session_interactively(&sessions)
}

/// Every pixelcoords session under `root`, newest first. Unreadable or
/// foreign directories are skipped, not fatal — this is a listing.
fn sessions_under(root: &std::path::Path) -> Vec<SessionEntry> {
    let Ok(read) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<SessionEntry> = read
        .flatten()
        .filter_map(|entry| session_entry(&entry.path()))
        .collect();
    entries.sort_by(|a, b| b.created.cmp(&a.created).then(b.name.cmp(&a.name)));
    entries
}

/// Summarize one directory as a pickable session, if it is one of ours.
fn session_entry(dir: &std::path::Path) -> Option<SessionEntry> {
    use std::fmt::Write as _;
    let text = std::fs::read_to_string(dir.join("session.json")).ok()?;
    let session: pixelcoords_core::session::SessionFile = serde_json::from_str(&text).ok()?;
    if session.app.name != pixelcoords_core::session::APP_NAME {
        return None;
    }
    let labels: Vec<&str> = session
        .selections
        .iter()
        .map(|s| s.label.as_str())
        .filter(|l| !l.is_empty())
        .take(4)
        .collect();
    let mut summary = format!("{} selections", session.selections.len());
    if !labels.is_empty() {
        // Writing to a String cannot fail.
        let _ = write!(summary, " [{}]", labels.join(", "));
    }
    if let Some(target) = &session.target {
        let _ = write!(summary, "  window: {:?}", target.title);
    }
    if let Some(platform) = &session.platform {
        let _ = write!(summary, "  {platform}");
    }
    let folder = dir.file_name()?.to_string_lossy().into_owned();
    let name = match &session.name {
        Some(friendly) => format!("{friendly}  ({folder})"),
        None => folder,
    };
    Some(SessionEntry {
        path: dir.to_path_buf(),
        name,
        summary,
        created: session.created_utc,
    })
}

/// A numbered stdin menu — deliberately plain, no TUI. Refuses politely
/// when stdin is not a terminal, so scripts get an actionable error
/// instead of a hang.
fn pick_session_interactively(sessions: &[SessionEntry]) -> Result<PathBuf> {
    use std::io::{IsTerminal, Write};
    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "stdin is not a terminal — pass --session <name-or-path> or --last"
    );
    println!(
        "Saved sessions, newest first:
"
    );
    for (i, s) in sessions.iter().enumerate() {
        println!("  [{}] {}  {}  {}", i + 1, s.name, s.created, s.summary);
    }
    print!(
        "
Resume which? [1-{}, Enter = 1] ",
        sessions.len()
    );
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let choice = parse_pick(&line, sessions.len())?;
    Ok(sessions[choice].path.clone())
}

/// A 1-based menu answer; empty means the first entry. Anything else is
/// an error, never a guess.
fn parse_pick(line: &str, len: usize) -> Result<usize> {
    let t = line.trim();
    if t.is_empty() {
        return Ok(0);
    }
    let n: usize = t
        .parse()
        .with_context(|| format!("{t:?} is not a number"))?;
    anyhow::ensure!((1..=len).contains(&n), "pick a number from 1 to {len}");
    Ok(n - 1)
}

/// Set (or clear, with an empty string) a saved session's friendly name
/// by rewriting only its session.json — images stay untouched.
fn rename_session(path: &std::path::Path, name: &str) -> Result<()> {
    let mut session = load_session(path)?;
    anyhow::ensure!(
        session.app.name == pixelcoords_core::session::APP_NAME,
        "{} was not written by pixelcoords",
        path.display()
    );
    let trimmed = name.trim();
    session.name = Some(trimmed.to_string()).filter(|n| !n.is_empty());
    let file = session_dir(path).join("session.json");
    let json = serde_json::to_string_pretty(&session).context("serializing session")?;
    std::fs::write(&file, json).with_context(|| format!("writing {}", file.display()))?;
    match &session.name {
        Some(n) => println!("named {} {n:?}", file.display()),
        None => println!("cleared the name on {}", file.display()),
    }
    Ok(())
}

/// The platform string stamped into sessions — what a consumer must
/// know before re-attaching to recorded windows or coordinates.
fn current_platform() -> String {
    #[cfg(target_os = "macos")]
    let platform = "macos".to_string();
    #[cfg(target_os = "windows")]
    let platform = "windows".to_string();
    #[cfg(target_os = "linux")]
    let platform = if linux::is_wayland() {
        "linux-wayland".to_string()
    } else {
        "linux-x11".to_string()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let platform = "unknown".to_string();
    platform
}

/// The directory holding a session's files, however the session was named.
fn session_dir(path: &std::path::Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    path.parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
}

/// The OS this binary was built for — which is where its sessions were
/// captured, and therefore the space `emit`'s pyautogui snippets need.
const EMIT_PLATFORM: pixelcoords_core::emit::Platform = if cfg!(target_os = "macos") {
    pixelcoords_core::emit::Platform::MacOs
} else if cfg!(target_os = "windows") {
    pixelcoords_core::emit::Platform::Windows
} else {
    pixelcoords_core::emit::Platform::Linux
};

/// Load the session and render the requested snippet.
fn emit_snippet(
    path: &std::path::Path,
    format: cli::FormatArg,
    label: Option<&str>,
) -> Result<String> {
    let session = load_session(path)?;
    let format = match format {
        cli::FormatArg::Pyautogui => pixelcoords_core::emit::EmitFormat::Pyautogui,
        cli::FormatArg::Cliclick => pixelcoords_core::emit::EmitFormat::Cliclick,
        cli::FormatArg::Xdotool => pixelcoords_core::emit::EmitFormat::Xdotool,
    };
    Ok(pixelcoords_core::emit::emit(
        &session,
        format,
        EMIT_PLATFORM,
        label,
    )?)
}

/// The `assert` subcommand: print the verdict, exit 0 on hit, 1 on
/// miss, 2 on a malformed question — a script must be able to tell the
/// difference.
fn assert_command(
    session: &std::path::Path,
    point: &str,
    expect: Option<&str>,
    space: cli::SpaceArg,
    monitor: Option<usize>,
) -> Result<()> {
    use pixelcoords_core::report::{Command, Report};

    let verdict = match assess_session(session, point, expect, space, monitor) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("pixelcoords assert: {e:#}");
            std::process::exit(2)
        }
    };
    // No `captured_utc`: scoring a point is pure session math, and
    // stamping a time on it would imply a capture that never happened.
    let hit = verdict.hit;
    let report = Report::offline(Command::Assert, hit, vec![verdict]);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if hit {
        return Ok(());
    }
    std::process::exit(1)
}

/// Load the session, parse the point, and produce the verdict — everything
/// `assert` does except printing and choosing the exit code.
fn assess_session(
    path: &std::path::Path,
    point_arg: &str,
    label: Option<&str>,
    space: cli::SpaceArg,
    monitor: Option<usize>,
) -> Result<pixelcoords_core::verdict::Verdict> {
    let session = load_session(path)?;
    let point = parse_point(point_arg)?;
    let space = resolve_space(space, monitor, &session)?;
    Ok(pixelcoords_core::verdict::assess(
        &session, point, space, label,
    )?)
}

/// Read a `session.json` — given directly, or found inside a directory —
/// refusing schemas this build does not understand.
fn load_session(path: &std::path::Path) -> Result<pixelcoords_core::session::SessionFile> {
    let file_path = if path.is_dir() {
        path.join("session.json")
    } else {
        path.to_path_buf()
    };
    let text = std::fs::read_to_string(&file_path)
        .with_context(|| format!("reading {}", file_path.display()))?;
    let session: pixelcoords_core::session::SessionFile =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", file_path.display()))?;
    anyhow::ensure!(
        session.schema == pixelcoords_core::session::SCHEMA_VERSION,
        "{} carries schema {}, but this pixelcoords understands schema {} — \
         upgrade pixelcoords to read it",
        file_path.display(),
        session.schema,
        pixelcoords_core::session::SCHEMA_VERSION
    );
    Ok(session)
}

/// Parse `"X,Y"` into a point; either coordinate may be negative.
fn parse_point(s: &str) -> Result<pixelcoords_core::geometry::Point> {
    let Some((x, y)) = s.split_once(',') else {
        anyhow::bail!("point {s:?} is not of the form X,Y");
    };
    let parse = |v: &str| -> Result<i32> {
        v.trim()
            .parse()
            .with_context(|| format!("point coordinate {v:?} is not an integer"))
    };
    Ok(pixelcoords_core::geometry::Point::new(parse(x)?, parse(y)?))
}

/// Map the CLI's space flag onto the session: monitor space takes its index
/// from `--monitor`, or from the session when only one monitor is present.
fn resolve_space(
    space: cli::SpaceArg,
    monitor: Option<usize>,
    session: &pixelcoords_core::session::SessionFile,
) -> Result<pixelcoords_core::space::Origin> {
    use pixelcoords_core::space::Origin;
    match space {
        cli::SpaceArg::Global => Ok(Origin::Global),
        cli::SpaceArg::Window => Ok(Origin::Window),
        cli::SpaceArg::Monitor => {
            if let Some(index) = monitor {
                return Ok(Origin::Monitor(index));
            }
            let indices: Vec<usize> = session.monitors.iter().map(|m| m.index).collect();
            anyhow::ensure!(
                indices.len() == 1,
                "--space monitor is ambiguous on a session with monitors {indices:?} — \
                 add --monitor N"
            );
            Ok(Origin::Monitor(indices[0]))
        }
    }
}

/// Print every window `--target` could match, front-most first — as a
/// human table, or as JSON for scripts choosing a target programmatically.
fn list_windows<P: CaptureProvider>(provider: &P, json: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    if !json && !mac::has_screen_capture_access() {
        println!(
            "note: screen recording permission is not granted, so window titles\n\
             may show as empty — run `pixelcoords doctor` for instructions\n"
        );
    }

    let mut windows = provider.windows()?;
    windows.sort_by_key(|w| std::cmp::Reverse(w.z));
    if json {
        println!("{}", serde_json::to_string_pretty(&windows_json(&windows))?);
        return Ok(());
    }
    println!(
        "{} visible windows, front-most first. --target matches the title\n\
         (exact, then prefix, then substring), then the app name:\n",
        windows.len()
    );
    let (app_header, title_header) = ("APP", "TITLE");
    println!("{app_header:<24} {title_header:<52} POSITION AND SIZE");
    for w in &windows {
        println!(
            "{:<24} {:<52} ({}, {}) {}x{}",
            truncate(&w.app, 22),
            format!("{:?}", truncate(&w.title, 48)),
            w.origin.x,
            w.origin.y,
            w.size_native.w,
            w.size_native.h
        );
    }
    Ok(())
}

/// The `windows --json` document: front-most first, in the platform's
/// native coordinate space, named so a consumer needn't guess the units.
fn windows_json(windows: &[crate::capture::WindowInfo]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = windows
        .iter()
        .map(|w| {
            serde_json::json!({
                "app": w.app,
                "title": w.title,
                "z": w.z,
                "origin": { "x": w.origin.x, "y": w.origin.y },
                "size": { "w": w.size_native.w, "h": w.size_native.h },
            })
        })
        .collect();
    serde_json::json!({
        "schema": 1,
        "space": NATIVE_COORD_SPACE.label(),
        "windows": items,
    })
}

/// `doctor --json`: the same checks as `doctor`, one machine-readable
/// object on stdout. Returns overall health, which drives the exit code.
fn doctor_json(config: Option<&std::path::Path>) -> Result<bool> {
    let mut healthy = true;

    #[cfg(target_os = "macos")]
    let screen_recording = {
        let granted = mac::has_screen_capture_access();
        healthy &= granted;
        serde_json::json!({ "required": true, "granted": granted })
    };
    #[cfg(not(target_os = "macos"))]
    let screen_recording = serde_json::json!({ "required": false });

    // Meaningful on Windows only; null elsewhere rather than absent, so a
    // consumer keying on it sees the same shape on every platform.
    #[cfg(target_os = "windows")]
    let dpi_aware_v2 = {
        let aware = win::is_per_monitor_aware_v2();
        healthy &= aware;
        serde_json::Value::from(aware)
    };
    #[cfg(not(target_os = "windows"))]
    let dpi_aware_v2 = serde_json::Value::Null;

    let config_value = config_json(config, &mut healthy);

    let mut monitor_error = serde_json::Value::Null;
    let monitors: Vec<serde_json::Value> = match XcapCapture.monitors() {
        Ok(monitors) => monitors
            .iter()
            .map(|m| {
                serde_json::json!({
                    "index": m.index,
                    "name": m.name,
                    "primary": m.primary,
                    "origin": { "x": m.origin.x, "y": m.origin.y },
                    "size": { "w": m.size_native.w, "h": m.size_native.h },
                    "space": NATIVE_COORD_SPACE.label(),
                    "scale": m.scale,
                })
            })
            .collect(),
        Err(e) => {
            healthy = false;
            monitor_error = serde_json::Value::from(format!("{e:#}"));
            Vec::new()
        }
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": 1,
            "healthy": healthy,
            "screen_recording": screen_recording,
            "dpi_aware_v2": dpi_aware_v2,
            "config": config_value,
            "monitors": monitors,
            "monitor_error": monitor_error,
        }))?
    );
    Ok(healthy)
}

/// The config check as JSON, flipping `healthy` on an invalid file — the
/// same rule the human doctor applies.
fn config_json(config: Option<&std::path::Path>, healthy: &mut bool) -> serde_json::Value {
    match config_path(config) {
        Some(path) if path.exists() => match load_config(config) {
            Ok(_) => serde_json::json!({
                "path": path.display().to_string(),
                "status": "loaded",
            }),
            Err(e) => {
                *healthy = false;
                serde_json::json!({
                    "path": path.display().to_string(),
                    "status": "invalid",
                    "error": format!("{e:#}"),
                })
            }
        },
        Some(path) => serde_json::json!({
            "path": path.display().to_string(),
            "status": "absent, defaults in effect",
        }),
        None => serde_json::json!({
            "status": "no config directory, defaults in effect",
        }),
    }
}

/// Shorten to at most `max` characters, marking the cut with an ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// The pseudo-monitor and target records a picked-window session is built
/// on. The portal reveals pixels but no identity, position, or scale, so
/// the frame is its own coordinate space: origin (0, 0), the scale
/// recorded as 1.0, and every selection's `window_px` equal to its `px`.
/// The record names say so rather than posing as a real display.
#[cfg(any(target_os = "linux", test))]
fn picked_records(
    width: i32,
    height: i32,
) -> (
    capture::MonitorInfo,
    pixelcoords_core::session::TargetRecord,
) {
    let monitor = capture::MonitorInfo {
        index: 0,
        name: "picked window (portal)".into(),
        primary: true,
        origin: pixelcoords_core::geometry::Point::new(0, 0),
        size_native: pixelcoords_core::geometry::Size::new(width, height),
        scale: 1.0,
    };
    let record = pixelcoords_core::session::TargetRecord {
        app: "xdg-desktop-portal".into(),
        title: "picked window".into(),
        monitor: 0,
        origin_px: pixelcoords_core::geometry::Point::new(0, 0),
        size_px: pixelcoords_core::geometry::Size::new(width, height),
    };
    (monitor, record)
}

/// `--pick`: freeze one window chosen in the desktop portal's picker and
/// run the overlay on it alone.
#[cfg(target_os = "linux")]
fn run_overlay_picked(args: &cli::Cli) -> Result<()> {
    let config = load_config(args.config.as_deref())?;
    let style = config.resolve_style()?;
    let bindings = config
        .resolve_bindings(&args.bind)
        .context("invalid hotkey binding")?;
    let img = linux::portal_pick()?;
    log::info!("portal returned a {}x{} pick", img.width(), img.height());
    let (monitor, record) = picked_records(img.width() as i32, img.height() as i32);
    let frames = vec![app::MonitorFrame::new(monitor, img)];
    let out_dir = args.out.clone().unwrap_or_else(default_out_dir);
    // A pick is one window's pixels, not a monitor's: present it at its own
    // size rather than stretched across whatever display it lands on.
    let mut app = app::App::new(
        frames,
        style,
        bindings,
        out_dir,
        Some(record),
        view::Presentation::Windowed,
    );
    app.set_session_meta(crate::save::SessionMeta {
        platform: Some(current_platform()),
        capture: Some(pixelcoords_core::session::CaptureKind::Pick),
        name: args.name.clone(),
    });
    app.restore_panel(state::load_panel());
    app.run()
}

#[cfg(not(target_os = "linux"))]
fn run_overlay_picked(_args: &cli::Cli) -> Result<()> {
    anyhow::bail!(
        "--pick uses the Linux desktop portal — on this platform use \
         --target, which can find the window by itself"
    )
}

/// Snapshot mode: capture every monitor (or just `--monitor N`), then run
/// the frozen overlay across them.
fn run_overlay(args: &cli::Cli) -> Result<()> {
    if args.pick {
        return run_overlay_picked(args);
    }
    // Config + bindings resolve (and fail) before any permission or window
    // work.
    let config = load_config(args.config.as_deref())?;
    let style = config.resolve_style()?;
    let bindings = config
        .resolve_bindings(&args.bind)
        .context("invalid hotkey binding")?;

    #[cfg(target_os = "macos")]
    if !mac::has_screen_capture_access() && !mac::request_screen_capture_access() {
        anyhow::bail!(
            "screen recording permission denied — run `pixelcoords doctor` for instructions"
        );
    }

    let provider = XcapCapture;
    let monitors = provider.monitors()?;
    let (targets, target_record) = select_targets(args, &config, &provider, monitors)?;

    let mut frames = Vec::new();
    for monitor in targets {
        let img = provider.capture(&monitor)?;
        log::info!(
            "captured monitor {} ({}) at {}x{}",
            monitor.index,
            monitor.name,
            img.width(),
            img.height()
        );
        let mut frame = app::MonitorFrame::new(monitor, img);
        // In `--target` mode the drawable region is the window's rect on
        // its monitor, not the whole monitor. That both constrains new
        // marks and gives the overlay a boundary to dim around.
        if let Some(record) = &target_record
            && record.monitor == frame.info.index
        {
            frame = frame.with_draw_rect(pixelcoords_core::geometry::Rect::new(
                record.origin_px.x,
                record.origin_px.y,
                record.size_px.w,
                record.size_px.h,
            ));
        }
        frames.push(frame);
    }

    let out_dir = args.out.clone().unwrap_or_else(default_out_dir);
    let capture = if target_record.is_some() {
        pixelcoords_core::session::CaptureKind::Window
    } else {
        pixelcoords_core::session::CaptureKind::Desktop
    };
    let mut app = app::App::new(
        frames,
        style,
        bindings,
        out_dir,
        target_record,
        view::Presentation::Fullscreen,
    );
    app.set_session_meta(crate::save::SessionMeta {
        platform: Some(current_platform()),
        capture: Some(capture),
        name: args.name.clone(),
    });
    app.restore_panel(state::load_panel());
    app.run()
}

/// Which monitors to freeze, and the matched `--target` window if any.
/// Guard-clause dispatch: `--target` wins, then `--monitor`, then the
/// config's `[capture] monitors`, then all. Flags always beat the file.
fn select_targets<P: CaptureProvider>(
    args: &cli::Cli,
    config: &Config,
    provider: &P,
    monitors: Vec<capture::MonitorInfo>,
) -> Result<(
    Vec<capture::MonitorInfo>,
    Option<pixelcoords_core::session::TargetRecord>,
)> {
    if let Some(query) = args.target.as_deref() {
        let (monitor, record) = resolve_target(provider, monitors, query)?;
        return Ok((vec![monitor], Some(record)));
    }
    if !args.monitor.is_empty() {
        return Ok((resolve_monitors(&args.monitor, monitors)?, None));
    }
    // The double-clicked binary's only way to express a preference: no
    // terminal, no flags, so the file is the whole vocabulary.
    let configured = config.resolve_monitors()?;
    if !configured.is_empty() {
        return Ok((resolve_monitors(&configured, monitors)?, None));
    }
    Ok((monitors, None))
}

/// The available displays, as the error messages list them — index first,
/// because that is what a script pins, then the name, because that is what
/// a person recognizes.
fn describe_monitors(monitors: &[capture::MonitorInfo]) -> String {
    monitors
        .iter()
        .map(|m| {
            let primary = if m.primary { " (primary)" } else { "" };
            format!("  [{}] {:?}{primary}", m.index, m.name)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve `--monitor` queries to the displays they name, in enumeration
/// order. Each query is an index, the word `primary`, or part of a name.
///
/// Every failure is loud. Freezing the wrong screen — or silently falling
/// back to all of them — would be discovered only after the user had
/// already marked regions on it.
fn resolve_monitors(
    queries: &[String],
    monitors: Vec<capture::MonitorInfo>,
) -> Result<Vec<capture::MonitorInfo>> {
    let names: Vec<String> = monitors.iter().map(|m| m.name.clone()).collect();
    // Positions into `monitors`, not monitor indices: the caller wants the
    // subset in enumeration order, and a set keeps `--monitor 1,DELL`
    // naming the same display twice from freezing it twice.
    let mut chosen: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();

    for query in queries {
        let query = query.trim();
        if query.eq_ignore_ascii_case("primary") {
            // `normalize_primary` guarantees exactly one, so this cannot be
            // ambiguous the way a name can.
            let position = monitors.iter().position(|m| m.primary).with_context(|| {
                format!(
                    "no monitor is marked primary — attached:\n{}",
                    describe_monitors(&monitors)
                )
            })?;
            chosen.insert(position);
            continue;
        }
        // A number is an index first: `--monitor 0` has always meant index
        // 0 and must keep doing so. But an index nothing carries is free to
        // be a name — and it has to be, because a backend may report names
        // that are mostly digits ("Display #41054" on macOS), where the
        // number is the only distinctive thing to type. Refusing those
        // would make the most obvious query the one that cannot work.
        let numeric = query.parse::<usize>().ok();
        if let Some(index) = numeric
            && let Some(position) = monitors.iter().position(|m| m.index == index)
        {
            chosen.insert(position);
            continue;
        }
        let matches = pixelcoords_core::matcher::select_monitors(query, &names);
        anyhow::ensure!(
            !matches.is_empty(),
            "--monitor {query:?}: {} — attached:\n{}",
            if numeric.is_some() {
                "no monitor has that index, and no name matches either"
            } else {
                "no monitor name matches"
            },
            describe_monitors(&monitors)
        );
        // More than one match at the best rank is a question only the user
        // can answer; guessing would freeze a screen they did not mean.
        anyhow::ensure!(
            matches.len() == 1,
            "--monitor {query:?} is ambiguous — it matches:\n{}\nName one exactly, or use its index.",
            describe_monitors(
                &matches
                    .iter()
                    .map(|&i| monitors[i].clone())
                    .collect::<Vec<_>>()
            )
        );
        chosen.insert(matches[0]);
    }

    Ok(monitors
        .into_iter()
        .enumerate()
        .filter(|(i, _)| chosen.contains(i))
        .map(|(_, m)| m)
        .collect())
}

/// Match `query` against the visible windows, pick the monitor holding the
/// winner, and convert its bounds into that monitor's local physical pixels.
fn resolve_target<P: CaptureProvider>(
    provider: &P,
    monitors: Vec<capture::MonitorInfo>,
    query: &str,
) -> Result<(
    capture::MonitorInfo,
    pixelcoords_core::session::TargetRecord,
)> {
    use pixelcoords_core::matcher::{WindowCandidate, select};

    let windows = provider.windows()?;
    let candidates: Vec<WindowCandidate> = windows
        .iter()
        .map(|w| WindowCandidate {
            title: w.title.clone(),
            app: w.app.clone(),
            z: w.z,
        })
        .collect();
    let index = select(query, &candidates).with_context(|| {
        let titles: Vec<&str> = windows.iter().map(|w| w.title.as_str()).take(10).collect();
        format!("no window matches --target {query:?}; visible titles include {titles:?}")
    })?;
    let win = &windows[index];

    // The monitor containing the window's center. Both sides are in the
    // platform's native space, so they compare directly; the conversion to
    // physical happens below, once, via `to_physical`.
    let center_x = win.origin.x + win.size_native.w / 2;
    let center_y = win.origin.y + win.size_native.h / 2;
    let monitor = monitors
        .into_iter()
        .find(|m| {
            center_x >= m.origin.x
                && center_x < m.origin.x + m.size_native.w
                && center_y >= m.origin.y
                && center_y < m.origin.y + m.size_native.h
        })
        .with_context(|| {
            format!(
                "target window {:?} ({}) is not on any monitor",
                win.title, win.app
            )
        })?;

    let to_physical =
        |v: i32| crate::capture::to_physical(v, monitor.scale, crate::capture::NATIVE_COORD_SPACE);
    let record = pixelcoords_core::session::TargetRecord {
        app: win.app.clone(),
        title: win.title.clone(),
        monitor: monitor.index,
        origin_px: pixelcoords_core::geometry::Point::new(
            to_physical(win.origin.x - monitor.origin.x),
            to_physical(win.origin.y - monitor.origin.y),
        ),
        size_px: pixelcoords_core::geometry::Size::new(
            to_physical(win.size_native.w),
            to_physical(win.size_native.h),
        ),
    };
    log::info!(
        "targeting {:?} ({}) on monitor {} at ({}, {}) {}x{} physical",
        record.title,
        record.app,
        record.monitor,
        record.origin_px.x,
        record.origin_px.y,
        record.size_px.w,
        record.size_px.h
    );
    Ok((monitor, record))
}

/// The config path in effect: `--config` wins, else the OS config dir.
fn config_path(explicit: Option<&std::path::Path>) -> Option<PathBuf> {
    match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => dirs::config_dir().map(|d| d.join("pixelcoords").join("config.toml")),
    }
}

/// Load the config file if it exists. A missing default-location file means
/// defaults; a missing `--config` file is an error; a present-but-invalid
/// file is always an error, never silently ignored.
fn load_config(explicit: Option<&std::path::Path>) -> Result<Config> {
    let Some(path) = config_path(explicit) else {
        return Ok(Config::default());
    };
    if !path.exists() {
        anyhow::ensure!(
            explicit.is_none(),
            "config file {} not found",
            path.display()
        );
        return Ok(Config::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let config: Config =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    log::info!("loaded config from {}", path.display());
    Ok(config)
}

/// Report permission state, config, and the monitor table without
/// capturing. Returns whether everything checks out (drives the exit code).
fn doctor(config: Option<&std::path::Path>) -> bool {
    let mut healthy = true;
    #[cfg(target_os = "macos")]
    {
        let granted = mac::has_screen_capture_access();
        if granted {
            println!("screen recording permission: GRANTED");
        }
        if !granted {
            healthy = false;
            println!("screen recording permission: DENIED");
            println!("\n{}", mac::GRANT_INSTRUCTIONS);
        }
    }
    #[cfg(not(target_os = "macos"))]
    println!("screen recording permission: no gate on this platform");

    // An external override (manifest, app-compatibility shim) can leave us
    // in a different awareness mode, which silently degrades every scale
    // factor we print and save — worth failing the health check over.
    #[cfg(target_os = "windows")]
    {
        let aware = win::is_per_monitor_aware_v2();
        let state = if aware {
            "per-monitor v2"
        } else {
            "NOT per-monitor v2 — reported scale factors will be approximate"
        };
        println!("dpi awareness: {state}");
        healthy &= aware;
    }

    match config_path(config) {
        Some(path) if path.exists() => match load_config(config) {
            Ok(_) => println!("config: {} (loaded)", path.display()),
            Err(e) => {
                healthy = false;
                println!("config: {} (INVALID: {e:#})", path.display());
            }
        },
        Some(path) => println!("config: {} (not present, using defaults)", path.display()),
        None => println!("config: no config directory on this system, using defaults"),
    }

    // The name is the addressable handle, not decoration: `--monitor` takes
    // it, and unlike the index it survives a replug. Say so here, because
    // this listing is where people come to find out what to type.
    println!("\nmonitors (address any with --monitor <index> or --monitor <name>):");
    match XcapCapture.monitors() {
        Ok(monitors) => {
            for m in monitors {
                println!(
                    "  [{}] {:?} primary={} origin=({}, {}) {}={}x{} scale={}",
                    m.index,
                    m.name,
                    m.primary,
                    m.origin.x,
                    m.origin.y,
                    NATIVE_COORD_SPACE.label(),
                    m.size_native.w,
                    m.size_native.h,
                    m.scale
                );
            }
        }
        Err(e) => {
            healthy = false;
            println!("  enumeration FAILED: {e:#}");
        }
    }
    healthy
}

/// Capture every monitor to PNG — the M2 spike. Prints the logical-vs-
/// captured size table that pins down the coordinate mapping empirically.
fn shoot(out: Option<PathBuf>, monitor: &[String]) -> Result<()> {
    #[cfg(target_os = "macos")]
    if !mac::has_screen_capture_access() && !mac::request_screen_capture_access() {
        anyhow::bail!(
            "screen recording permission denied — run `pixelcoords doctor` for instructions"
        );
    }

    let dir = out.unwrap_or_else(default_out_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let provider = XcapCapture;
    let targets = match monitor {
        [] => provider.monitors()?,
        queries => resolve_monitors(queries, provider.monitors()?)?,
    };
    for m in targets {
        let img = provider.capture(&m)?;
        let path = dir.join(format!("screenshot-{}.png", m.index));
        img.save(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        let expect = m.size_physical();
        println!(
            "[{}] {:?} {}={}x{} scale={} captured={}x{} (expected physical {}x{}) -> {}",
            m.index,
            m.name,
            NATIVE_COORD_SPACE.label(),
            m.size_native.w,
            m.size_native.h,
            m.scale,
            img.width(),
            img.height(),
            expect.w,
            expect.h,
            path.display()
        );
    }
    Ok(())
}

fn default_out_dir() -> PathBuf {
    // Milliseconds, not seconds: two runs started in the same second would
    // otherwise share a directory, and the second would overwrite the
    // first's session.json. Scripts start runs back to back.
    let format = time::format_description::parse_borrowed::<2>(
        "[year][month][day]-[hour][minute][second]-[subsecond digits:3]",
    )
    .expect("static format string");
    let stamp = time::OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "capture".to_string());
    captures_root(dirs::download_dir()).join(stamp)
}

/// Where captures land when `--out` is not given: `pixelcoords-captures`
/// in the user's Downloads folder — the one place every OS gives a user
/// and every user knows to look, where an installed binary's working
/// directory is wherever the terminal happened to be. A system with no
/// known Downloads directory (headless Linux without XDG user dirs)
/// falls back to the working directory, the old behavior.
fn captures_root(downloads: Option<PathBuf>) -> PathBuf {
    let Some(base) = downloads else {
        return PathBuf::from("pixelcoords-captures");
    };
    base.join("pixelcoords-captures")
}

#[cfg(test)]
mod tests {
    use super::{
        Config, config_path, load_config, resolve_monitors, resolve_target, select_targets,
        truncate,
    };
    use crate::capture::{
        CaptureProvider, FakeCapture, MonitorInfo, NATIVE_COORD_SPACE, WindowInfo, to_physical,
    };
    use anyhow::Result;
    use clap::Parser;
    use image::RgbaImage;
    use pixelcoords_core::geometry::{Point, Size};

    /// Two monitors with different DPI scales; one window on the second.
    struct MixedDpi;

    impl CaptureProvider for MixedDpi {
        fn monitors(&self) -> Result<Vec<MonitorInfo>> {
            Ok(vec![
                MonitorInfo {
                    index: 0,
                    name: "Left".into(),
                    primary: true,
                    origin: Point::new(0, 0),
                    size_native: Size::new(100, 60),
                    scale: 1.0,
                },
                MonitorInfo {
                    index: 1,
                    name: "Right Retina".into(),
                    primary: false,
                    origin: Point::new(100, 0),
                    size_native: Size::new(80, 50),
                    scale: 2.0,
                },
            ])
        }

        fn capture(&self, _monitor: &MonitorInfo) -> Result<RgbaImage> {
            Ok(RgbaImage::new(1, 1))
        }

        fn windows(&self) -> Result<Vec<WindowInfo>> {
            Ok(vec![WindowInfo {
                id: 1,
                title: "Editor - main.rs".into(),
                app: "Editor".into(),
                z: 5,
                origin: Point::new(120, 10),
                size_native: Size::new(40, 20),
            }])
        }
    }

    #[test]
    fn resolve_target_converts_to_monitor_local_physical() {
        let provider = MixedDpi;
        let monitors = provider.monitors().unwrap();
        let (monitor, record) = resolve_target(&provider, monitors, "editor").unwrap();
        // The window's center (140, 20) lies on the second monitor.
        assert_eq!(monitor.index, 1);
        assert_eq!(record.monitor, 1);
        // Monitor-local native (20, 10) at scale 2, converted per the
        // platform's native space (logical on macOS -> x2; already
        // physical on Windows/X11 -> unchanged).
        let expected_origin = Point::new(
            to_physical(20, 2.0, NATIVE_COORD_SPACE),
            to_physical(10, 2.0, NATIVE_COORD_SPACE),
        );
        let expected_size = Size::new(
            to_physical(40, 2.0, NATIVE_COORD_SPACE),
            to_physical(20, 2.0, NATIVE_COORD_SPACE),
        );
        assert_eq!(record.origin_px, expected_origin);
        assert_eq!(record.size_px, expected_size);
        assert_eq!(record.app, "Editor");
    }

    #[test]
    fn resolve_target_miss_lists_visible_titles() {
        let provider = MixedDpi;
        let monitors = provider.monitors().unwrap();
        let err = resolve_target(&provider, monitors, "nope")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Editor - main.rs"), "got: {err}");
    }

    #[test]
    fn select_targets_dispatches_target_then_monitor_then_all() {
        let provider = FakeCapture;
        let monitors = || provider.monitors().unwrap();

        let all = crate::cli::Cli::try_parse_from(["pixelcoords"]).unwrap();
        let (targets, record) =
            select_targets(&all, &Config::default(), &provider, monitors()).unwrap();
        assert_eq!(targets.len(), 1); // FakeCapture has one monitor
        assert!(record.is_none());

        let one = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "0"]).unwrap();
        let (targets, record) =
            select_targets(&one, &Config::default(), &provider, monitors()).unwrap();
        assert_eq!(targets[0].index, 0);
        assert!(record.is_none());

        let bad = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "9"]).unwrap();
        assert!(select_targets(&bad, &Config::default(), &provider, monitors()).is_err());

        let targeted =
            crate::cli::Cli::try_parse_from(["pixelcoords", "--target", "fake window"]).unwrap();
        let (targets, record) =
            select_targets(&targeted, &Config::default(), &provider, monitors()).unwrap();
        assert_eq!(targets.len(), 1);
        let record = record.expect("target record");
        assert_eq!(record.origin_px, Point::new(10, 10));
        assert_eq!(record.size_px, Size::new(50, 30));
    }

    /// `--monitor` accepts what a person can actually remember: a name, the
    /// word `primary`, or the index a script pinned.
    #[test]
    fn monitor_queries_resolve_by_name_primary_and_index() {
        let provider = MixedDpi;
        let monitors = || provider.monitors().unwrap();
        let select = |args: &[&str]| {
            let cli = crate::cli::Cli::try_parse_from(args).unwrap();
            select_targets(&cli, &Config::default(), &provider, monitors())
                .map(|(targets, _)| targets.into_iter().map(|m| m.index).collect::<Vec<_>>())
        };

        assert_eq!(select(&["pixelcoords", "--monitor", "1"]).unwrap(), vec![1]);
        assert_eq!(
            select(&["pixelcoords", "--monitor", "primary"]).unwrap(),
            vec![0]
        );
        assert_eq!(
            select(&["pixelcoords", "--monitor", "retina"]).unwrap(),
            vec![1],
            "a name is matched case-insensitively, on a substring"
        );
    }

    #[test]
    fn a_monitor_list_freezes_the_named_set_in_enumeration_order() {
        let provider = MixedDpi;
        let cli = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "retina,primary"])
            .unwrap();
        let (targets, _) = select_targets(
            &cli,
            &Config::default(),
            &provider,
            provider.monitors().unwrap(),
        )
        .unwrap();
        let indices: Vec<usize> = targets.iter().map(|m| m.index).collect();
        assert_eq!(
            indices,
            vec![0, 1],
            "the subset comes back in enumeration order, not query order"
        );
    }

    #[test]
    fn naming_the_same_monitor_twice_freezes_it_once() {
        let provider = MixedDpi;
        let cli = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "0,primary,Left"])
            .unwrap();
        let (targets, _) = select_targets(
            &cli,
            &Config::default(),
            &provider,
            provider.monitors().unwrap(),
        )
        .unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].index, 0);
    }

    #[test]
    fn a_monitor_query_that_matches_nothing_lists_what_is_attached() {
        let provider = MixedDpi;
        let cli = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "nope"]).unwrap();
        let err = select_targets(
            &cli,
            &Config::default(),
            &provider,
            provider.monitors().unwrap(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Right Retina"), "got: {err}");
        assert!(err.contains("Left"), "got: {err}");
    }

    #[test]
    fn an_out_of_range_monitor_index_is_refused_by_number() {
        let provider = MixedDpi;
        let cli = crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "9"]).unwrap();
        let err = select_targets(
            &cli,
            &Config::default(),
            &provider,
            provider.monitors().unwrap(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains('9'), "got: {err}");
    }

    /// `[capture] monitors` is the double-clicked binary's only voice — no
    /// terminal, no flags — but a flag must still win when there is one.
    #[test]
    fn config_monitors_apply_when_no_flag_does_and_lose_when_one_does() {
        let provider = MixedDpi;
        let configured = |toml: &str| -> Config { ::toml::from_str(toml).expect("parses") };

        let cfg = configured("[capture]\nmonitors = \"Right Retina\"\n");
        let no_flags = crate::cli::Cli::try_parse_from(["pixelcoords"]).unwrap();
        let (targets, _) =
            select_targets(&no_flags, &cfg, &provider, provider.monitors().unwrap()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].index, 1);

        // The flag overrides the file, rather than intersecting with it.
        let flagged =
            crate::cli::Cli::try_parse_from(["pixelcoords", "--monitor", "primary"]).unwrap();
        let (targets, _) =
            select_targets(&flagged, &cfg, &provider, provider.monitors().unwrap()).unwrap();
        assert_eq!(targets[0].index, 0);

        // "all" in the file is the same as saying nothing.
        let cfg_all = configured("[capture]\nmonitors = \"all\"\n");
        let (targets, _) =
            select_targets(&no_flags, &cfg_all, &provider, provider.monitors().unwrap()).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn a_config_monitor_that_matches_nothing_fails_the_launch() {
        // The file is validated for shape in core; whether it names a real
        // display can only be answered here, and it must not degrade to
        // freeze-everything.
        let provider = MixedDpi;
        let cfg: Config = ::toml::from_str("[capture]\nmonitors = \"DELL\"\n").expect("parses");
        let no_flags = crate::cli::Cli::try_parse_from(["pixelcoords"]).unwrap();
        let err = select_targets(&no_flags, &cfg, &provider, provider.monitors().unwrap())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no monitor name matches"), "got: {err}");
    }

    /// Found on real hardware, not by a fake: macOS reports display names
    /// like "Display #41054", so the digits are the most distinctive thing
    /// to type — and they parsed as an index, failed to be one, and were
    /// refused. An index nothing carries has to fall through to a name.
    #[test]
    fn a_numeric_query_that_is_no_index_is_tried_as_a_name() {
        let display = |index: usize, id: &str| MonitorInfo {
            index,
            name: format!("Display #{id}"),
            primary: index == 0,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        let monitors = vec![display(0, "41054"), display(1, "15824")];

        let by_name = resolve_monitors(&["41054".to_string()], monitors.clone()).unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].index, 0);

        // An index that *does* exist still wins — `--monitor 1` cannot
        // start meaning something else.
        let by_index = resolve_monitors(&["1".to_string()], monitors.clone()).unwrap();
        assert_eq!(by_index[0].index, 1);

        // Neither reading works: say both were tried.
        let err = resolve_monitors(&["99999".to_string()], monitors)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no monitor has that index"), "got: {err}");
        assert!(err.contains("no name matches either"), "got: {err}");
    }

    #[test]
    fn two_displays_of_the_same_model_refuse_a_name_rather_than_guessing() {
        // The case the core selector returns both for. Freezing one at
        // random would be discovered only after the user marked regions on
        // the wrong screen, so this must refuse and say so.
        let twins = |index: usize| MonitorInfo {
            index,
            name: "DELL U2723QE".into(),
            primary: index == 0,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        let monitors = vec![twins(0), twins(1)];
        let err = resolve_monitors(&["DELL".to_string()], monitors.clone())
            .unwrap_err()
            .to_string();
        assert!(err.contains("ambiguous"), "got: {err}");
        assert!(err.contains("[0]") && err.contains("[1]"), "got: {err}");

        // The index still addresses them individually — ambiguity is a
        // property of the name, not of the displays.
        let picked = resolve_monitors(&["1".to_string()], monitors).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].index, 1);
    }

    #[test]
    fn windows_json_names_its_space_and_keeps_given_order() {
        use crate::capture::WindowInfo;
        let windows = vec![
            WindowInfo {
                id: 1,
                title: "front".into(),
                app: "A".into(),
                z: 9,
                origin: Point::new(1, 2),
                size_native: Size::new(30, 40),
            },
            WindowInfo {
                id: 2,
                title: "back".into(),
                app: "B".into(),
                z: 1,
                origin: Point::new(5, 6),
                size_native: Size::new(70, 80),
            },
        ];
        let json = super::windows_json(&windows);
        assert_eq!(json["schema"], 1);
        assert_eq!(json["space"], NATIVE_COORD_SPACE.label());
        assert_eq!(json["windows"][0]["title"], "front");
        assert_eq!(json["windows"][1]["size"]["w"], 70);
        assert_eq!(json["windows"][0]["origin"]["y"], 2);
    }

    #[test]
    fn config_json_reports_status_and_flips_health_on_invalid() {
        let missing = std::env::temp_dir().join("pixelcoords-test-doctor-json-absent.toml");
        let _ = std::fs::remove_file(&missing);
        // An absent file means defaults, not ill health — the same rule
        // the human doctor applies.
        let mut healthy = true;
        let json = super::config_json(Some(&missing), &mut healthy);
        assert_eq!(json["status"], "absent, defaults in effect");
        assert!(healthy);

        let bad = std::env::temp_dir().join("pixelcoords-test-doctor-json-bad.toml");
        std::fs::write(&bad, "not toml at all [").unwrap();
        let mut healthy = true;
        let json = super::config_json(Some(&bad), &mut healthy);
        assert_eq!(json["status"], "invalid");
        assert!(!healthy);
        std::fs::remove_file(&bad).unwrap();
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly-10", 10), "exactly-10");
        assert_eq!(truncate("much longer than allowed", 10), "much long…");
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
    }

    #[test]
    fn explicit_config_path_wins() {
        let p = std::path::Path::new("/some/where/custom.toml");
        assert_eq!(config_path(Some(p)).unwrap(), p);
    }

    #[test]
    fn missing_explicit_config_is_an_error() {
        let missing = std::env::temp_dir().join("pixelcoords-test-no-such-config.toml");
        let _ = std::fs::remove_file(&missing);
        let err = load_config(Some(&missing)).unwrap_err().to_string();
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn invalid_config_is_an_error_never_defaults() {
        let path = std::env::temp_dir().join("pixelcoords-test-bad-config.toml");
        std::fs::write(&path, "[style]\nthickness = \"fat\"\n").unwrap();
        assert!(load_config(Some(&path)).is_err());
        // Unknown fields are typo protection, not silently ignored.
        std::fs::write(&path, "[style]\npreview_colour = \"#F00\"\n").unwrap();
        assert!(load_config(Some(&path)).is_err());
        let _ = std::fs::remove_file(&path);
    }

    fn write_test_session(name: &str) -> std::path::PathBuf {
        use pixelcoords_core::geometry::{Rect, Shape};
        use pixelcoords_core::selection::Selection;
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 0);
        sel.label = "submit".into();
        let file = SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 0,
                name: "Main".into(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: Size::new(1920, 1080),
                scale: 1.0,
            }],
            &[sel],
            &["crop-0-submit.png".into()],
            None,
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn assess_session_hits_misses_and_filters_by_label() {
        let dir = write_test_session("pixelcoords-test-assert-roundtrip");
        let space = crate::cli::SpaceArg::Global;

        let hit = super::assess_session(&dir, "15,25", None, space, None).unwrap();
        assert!(hit.hit);
        let miss = super::assess_session(&dir, "500,500", None, space, None).unwrap();
        assert!(!miss.hit);
        assert_eq!(miss.nearest.as_ref().unwrap().region.label, "submit");
        let labeled = super::assess_session(&dir, "15,25", Some("SUBMIT"), space, None).unwrap();
        assert!(labeled.hit);
        let unknown = super::assess_session(&dir, "15,25", Some("nope"), space, None);
        assert!(unknown.is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn assess_session_accepts_the_json_path_and_defaults_the_monitor() {
        let dir = write_test_session("pixelcoords-test-assert-direct-path");
        // The file itself, not the directory; monitor space without
        // --monitor resolves to the session's only monitor.
        let v = super::assess_session(
            &dir.join("session.json"),
            "15,25",
            None,
            crate::cli::SpaceArg::Monitor,
            None,
        )
        .unwrap();
        assert!(v.hit);
        assert_eq!(v.monitor, Some(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_newer_session_schema_is_refused() {
        let dir = std::env::temp_dir().join("pixelcoords-test-assert-schema");
        std::fs::create_dir_all(&dir).unwrap();
        let dishonest = serde_json::json!({
            "schema": 99,
            "app": {"name": "pixelcoords", "version": "9.9.9"},
            "created_utc": "2026-07-27T00:00:00Z",
            "monitors": [],
            "selections": [],
        });
        std::fs::write(dir.join("session.json"), dishonest.to_string()).unwrap();
        let err = super::load_session(&dir).unwrap_err().to_string();
        assert!(err.contains("schema 99"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_or_malformed_session_is_an_error() {
        let missing = std::env::temp_dir().join("pixelcoords-test-assert-no-such-dir");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(super::load_session(&missing).is_err());

        let dir = std::env::temp_dir().join("pixelcoords-test-assert-malformed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), "not json").unwrap();
        let err = super::load_session(&dir).unwrap_err().to_string();
        assert!(err.contains("parsing"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Deterministic aperiodic patch — a periodic pattern would match at
    /// several offsets and trip the ambiguity flag by design.
    fn find_patch() -> Vec<u8> {
        let mut state = 0x1234_5678u32;
        (0..24 * 16)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state % 251) as u8
            })
            .collect()
    }

    /// One flat-gray monitor whose only feature, the patch, has moved from
    /// the session's (30, 20) to (90, 44) by the time `capture` runs.
    struct ShiftedScreen {
        scale: f64,
    }

    impl CaptureProvider for ShiftedScreen {
        fn monitors(&self) -> Result<Vec<MonitorInfo>> {
            Ok(vec![MonitorInfo {
                index: 0,
                name: "Fake".into(),
                primary: true,
                origin: Point::new(0, 0),
                size_native: Size::new(160, 120),
                scale: self.scale,
            }])
        }

        fn capture(&self, _monitor: &MonitorInfo) -> Result<RgbaImage> {
            let patch = find_patch();
            Ok(RgbaImage::from_fn(160, 120, |x, y| {
                let (dx, dy) = (x as i32 - 90, y as i32 - 44);
                if (0..24).contains(&dx) && (0..16).contains(&dy) {
                    let v = patch[(dy * 24 + dx) as usize];
                    return image::Rgba([v, v, v, 255]);
                }
                image::Rgba([128, 128, 128, 255])
            }))
        }

        fn windows(&self) -> Result<Vec<crate::capture::WindowInfo>> {
            Ok(vec![])
        }
    }

    /// A session dir whose one selection sat at (30, 20) with the patch as
    /// its crop. `with_crop: false` leaves the crop file missing.
    fn write_find_fixture(name: &str, with_crop: bool) -> std::path::PathBuf {
        use pixelcoords_core::geometry::{Rect, Shape};
        use pixelcoords_core::selection::Selection;
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let mut sel = Selection::new(Shape::Rect(Rect::new(30, 20, 24, 16)), 0);
        sel.label = "chip".into();
        let file = SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 0,
                name: "Fake".into(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: Size::new(160, 120),
                scale: 1.0,
            }],
            &[sel],
            &["crop-0-chip.png".into()],
            None,
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        if with_crop {
            let patch = find_patch();
            let crop = RgbaImage::from_fn(24, 16, |x, y| {
                let v = patch[(y * 24 + x) as usize];
                image::Rgba([v, v, v, 255])
            });
            crop.save(dir.join("crop-0-chip.png")).unwrap();
        }
        dir
    }

    #[test]
    fn run_find_relocates_a_moved_region() {
        use pixelcoords_core::geometry::{Rect, Shape};
        let dir = write_find_fixture("pixelcoords-test-find-moved", true);
        let report = super::run_find(&ShiftedScreen { scale: 1.0 }, &dir, None).unwrap();
        assert!(report.ok, "got: {report:?}");
        let r = &report.results[0];
        assert!(r.found && !r.ambiguous);
        assert!(r.score > 0.999, "exact patch scores ~1, got {}", r.score);
        assert_eq!(
            r.delta,
            Some(pixelcoords_core::locate::Delta { dx: 60, dy: 24 })
        );
        assert_eq!(r.new_px, Some(Shape::Rect(Rect::new(90, 44, 24, 16))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_find_filters_by_label_and_rejects_unknown_ones() {
        let dir = write_find_fixture("pixelcoords-test-find-label", true);
        let report = super::run_find(&ShiftedScreen { scale: 1.0 }, &dir, Some("CHIP")).unwrap();
        assert_eq!(report.results.len(), 1);
        assert_eq!(report.results[0].label, "chip");
        assert!(report.ok);

        let err = super::run_find(&ShiftedScreen { scale: 1.0 }, &dir, Some("nope"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("chip"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_find_refuses_a_changed_display() {
        let dir = write_find_fixture("pixelcoords-test-find-rescaled", true);
        let err = super::run_find(&ShiftedScreen { scale: 2.0 }, &dir, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("changed since the session"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_find_errors_on_a_missing_crop() {
        let dir = write_find_fixture("pixelcoords-test-find-cropless", false);
        let err = format!(
            "{:#}",
            super::run_find(&ShiftedScreen { scale: 1.0 }, &dir, None).unwrap_err()
        );
        assert!(err.contains("reading crop"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn emit_snippet_renders_from_a_saved_session() {
        let dir = write_test_session("pixelcoords-test-emit-roundtrip");
        // scale 1.0 in the fixture, so logical == physical and the
        // expected text is the same on every CI platform.
        let py = super::emit_snippet(&dir, crate::cli::FormatArg::Pyautogui, None).unwrap();
        assert!(py.contains("import pyautogui"), "got: {py}");
        assert!(
            py.contains("# submit — rect on monitor 0\npyautogui.click(25, 40)"),
            "got: {py}"
        );
        let xd = super::emit_snippet(&dir, crate::cli::FormatArg::Xdotool, None).unwrap();
        assert!(xd.contains("xdotool mousemove 25 40 click 1"), "got: {xd}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resumed_previous_crops_let_a_resave_skip_unchanged_files() {
        use pixelcoords_core::geometry::{Rect, Shape};
        use pixelcoords_core::selection::{Selection, SelectionSet};
        let dir = std::env::temp_dir().join("pixelcoords-test-resume-resave");
        let _ = std::fs::remove_dir_all(&dir);
        let frame_img = RgbaImage::from_fn(100, 60, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 251) as u8, 7, 255])
        });
        let info = MonitorInfo {
            index: 0,
            name: "Test".into(),
            primary: true,
            origin: Point::new(0, 0),
            size_native: Size::new(100, 60),
            scale: 1.0,
        };
        let mut sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 15)), 0);
        sel.rot_deg = 45;
        sel.label = "spun".into();
        let mut selections = SelectionSet::new();
        selections.add(sel);
        crate::save::write_session(
            &dir,
            &[(&info, &frame_img)],
            &selections,
            None,
            &crate::save::SessionMeta::default(),
            true,
            &[],
        )
        .unwrap();

        // Rebuild the resave inputs exactly the way run_resume does —
        // selections from the session, previous crops from its records —
        // then resave unchanged. Sentinel bytes must survive: the resumed
        // session behaves as if it never closed.
        let session = super::load_session(&dir).unwrap();
        let (restored, _dropped) = pixelcoords_core::session::restore_selections(&session);
        let previous: Vec<crate::save::WrittenCrop> = session
            .selections
            .iter()
            .map(|record| crate::save::WrittenCrop {
                name: record.crop.clone(),
                shape: record.px.clone(),
                rot_deg: record.rot_deg.unwrap_or(0),
                monitor: record.monitor,
            })
            .collect();
        let crop_name = previous[0].name.clone();
        assert_eq!(crop_name, "crop-0-spun.png");
        std::fs::write(dir.join(&crop_name), b"sentinel").unwrap();
        std::fs::write(dir.join("cutout-primary-0.png"), b"sentinel").unwrap();
        let seeded = SelectionSet::seed(restored);
        crate::save::write_session(
            &dir,
            &[(&info, &frame_img)],
            &seeded,
            None,
            &crate::save::SessionMeta::default(),
            false,
            &previous,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(dir.join(&crop_name)).unwrap(),
            b"sentinel",
            "unchanged crop must not re-encode"
        );
        assert_eq!(
            std::fs::read(dir.join("cutout-primary-0.png")).unwrap(),
            b"sentinel",
            "unchanged cutout must not re-encode"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn write_pickable_session(root: &std::path::Path, name: &str, created: &str, label: &str) {
        use pixelcoords_core::geometry::{Rect, Shape};
        use pixelcoords_core::selection::Selection;
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let mut sel = Selection::new(Shape::Rect(Rect::new(1, 1, 5, 5)), 0);
        sel.label = label.into();
        let file = SessionFile::build(
            "test",
            created.into(),
            vec![MonitorRecord {
                index: 0,
                name: "M".into(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: Size::new(100, 60),
                scale: 1.0,
            }],
            &[sel],
            &["crop-0.png".into()],
            None,
        )
        .with_meta(Some("macos".into()), None, None);
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn resume_resolution_orders_names_and_rejects_strangers() {
        let root = std::env::temp_dir().join("pixelcoords-test-resume-resolve");
        let _ = std::fs::remove_dir_all(&root);
        write_pickable_session(&root, "older", "2026-07-26T10:00:00Z", "a");
        write_pickable_session(&root, "newer", "2026-07-27T10:00:00Z", "b");
        // A directory that is not ours is not offered.
        std::fs::create_dir_all(root.join("stranger")).unwrap();
        std::fs::write(root.join("stranger").join("session.json"), "{}").unwrap();

        let sessions = super::sessions_under(&root);
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["newer", "older"], "newest first, stranger skipped");
        assert!(sessions[0].summary.contains("1 selections"));
        assert!(sessions[0].summary.contains("macos"));

        // --last takes the newest; a bare name resolves under the root; a
        // wrong name errors listing nothing silently.
        let last = super::resolve_resume_session(None, true, &root).unwrap();
        assert!(last.ends_with("newer"));
        let by_name =
            super::resolve_resume_session(Some(std::path::Path::new("older")), false, &root)
                .unwrap();
        assert!(by_name.ends_with("older"));
        assert!(
            super::resolve_resume_session(Some(std::path::Path::new("nope")), false, &root)
                .is_err()
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rename_writes_the_friendly_name_and_the_picker_leads_with_it() {
        let root = std::env::temp_dir().join("pixelcoords-test-rename");
        let _ = std::fs::remove_dir_all(&root);
        write_pickable_session(&root, "20260101-000000-000", "2026-01-01T00:00:00Z", "a");
        let dir = root.join("20260101-000000-000");

        super::rename_session(&dir, "  microsoft teams  ").unwrap();
        let session = super::load_session(&dir).unwrap();
        assert_eq!(session.name.as_deref(), Some("microsoft teams"));

        let sessions = super::sessions_under(&root);
        assert!(
            sessions[0].name.starts_with("microsoft teams  ("),
            "friendly first, folder as the id: {}",
            sessions[0].name
        );

        super::rename_session(&dir, "").unwrap();
        assert_eq!(super::load_session(&dir).unwrap().name, None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rename_refuses_a_session_pixelcoords_did_not_write() {
        let dir = std::env::temp_dir().join("pixelcoords-test-rename-foreign");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("session.json"),
            r#"{"schema":1,"app":{"name":"other","version":"1"},
                "created_utc":"t","monitors":[],"selections":[]}"#,
        )
        .unwrap();
        let err = super::rename_session(&dir, "x").unwrap_err().to_string();
        assert!(err.contains("not written by pixelcoords"), "got: {err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_pick_defaults_bounds_and_rejects_junk() {
        assert_eq!(super::parse_pick("", 3).unwrap(), 0);
        assert_eq!(super::parse_pick(" 2 \n", 3).unwrap(), 1);
        assert_eq!(super::parse_pick("3", 3).unwrap(), 2);
        assert!(super::parse_pick("0", 3).is_err());
        assert!(super::parse_pick("4", 3).is_err());
        assert!(super::parse_pick("two", 3).is_err());
    }

    #[test]
    fn monitor_from_record_round_trips_through_the_save_path() {
        use pixelcoords_core::session::MonitorRecord;
        let record = MonitorRecord {
            index: 1,
            name: "Right Retina".into(),
            primary: false,
            origin_px: Point::new(3024, 0),
            size_px: Size::new(3024, 1964),
            scale: 2.0,
        };
        let info = super::monitor_from_record(&record);
        // The reconstructed MonitorInfo reports the same physical values
        // the session recorded, on every platform's native space.
        assert_eq!(info.origin_physical(), record.origin_px);
        assert_eq!(info.size_physical(), record.size_px);
        assert!((info.scale - record.scale).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_point_reads_negatives_and_rejects_junk() {
        assert_eq!(super::parse_point("15,25").unwrap(), Point::new(15, 25));
        assert_eq!(
            super::parse_point(" -1920 , 0 ").unwrap(),
            Point::new(-1920, 0)
        );
        assert!(super::parse_point("15").is_err());
        assert!(super::parse_point("a,b").is_err());
        assert!(super::parse_point("1,2,3").is_err());
    }

    #[test]
    fn multi_monitor_sessions_demand_an_explicit_monitor() {
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let session = SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![
                MonitorRecord {
                    index: 0,
                    name: "A".into(),
                    primary: true,
                    origin_px: Point::new(0, 0),
                    size_px: Size::new(100, 100),
                    scale: 1.0,
                },
                MonitorRecord {
                    index: 1,
                    name: "B".into(),
                    primary: false,
                    origin_px: Point::new(100, 0),
                    size_px: Size::new(100, 100),
                    scale: 1.0,
                },
            ],
            &[],
            &[],
            None,
        );
        let err = super::resolve_space(crate::cli::SpaceArg::Monitor, None, &session)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--monitor N"), "got: {err}");
        let ok = super::resolve_space(crate::cli::SpaceArg::Monitor, Some(1), &session).unwrap();
        assert_eq!(ok, pixelcoords_core::space::Origin::Monitor(1));
    }

    #[test]
    fn picked_records_make_the_frame_its_own_window_space() {
        use pixelcoords_core::geometry::{Rect, Shape};
        use pixelcoords_core::selection::Selection;
        use pixelcoords_core::session::{MonitorRecord, SessionFile};
        let (monitor, record) = super::picked_records(800, 600);
        assert_eq!(monitor.index, 0);
        assert_eq!(monitor.origin, Point::new(0, 0));
        assert_eq!(monitor.size_native, Size::new(800, 600));
        assert!(
            monitor.name.contains("portal"),
            "the pseudo-frame must not pose as a real display: {}",
            monitor.name
        );
        assert_eq!(record.size_px, Size::new(800, 600));

        // Built into a session, every selection's window_px equals its px —
        // the frame is the window.
        let sel = Selection::new(Shape::Rect(Rect::new(10, 20, 30, 40)), 0);
        let file = SessionFile::build(
            "test",
            "2026-07-27T00:00:00Z".into(),
            vec![MonitorRecord {
                index: 0,
                name: monitor.name.clone(),
                primary: true,
                origin_px: Point::new(0, 0),
                size_px: Size::new(800, 600),
                scale: 1.0,
            }],
            &[sel],
            &["c.png".into()],
            Some(record),
        );
        assert_eq!(
            file.selections[0].window_px,
            Some(file.selections[0].px.clone())
        );
        assert_eq!(file.selections[0].global_px, file.selections[0].px);
    }

    #[test]
    fn captures_root_prefers_downloads_and_survives_without_it() {
        use std::path::PathBuf;
        assert_eq!(
            super::captures_root(Some(PathBuf::from("/home/u/Downloads"))),
            PathBuf::from("/home/u/Downloads/pixelcoords-captures")
        );
        assert_eq!(
            super::captures_root(None),
            PathBuf::from("pixelcoords-captures")
        );
    }

    #[test]
    fn pick_conflicts_with_target_and_monitor() {
        assert!(crate::cli::Cli::try_parse_from(["pixelcoords", "--pick"]).is_ok());
        assert!(
            crate::cli::Cli::try_parse_from(["pixelcoords", "--pick", "--target", "x"]).is_err()
        );
        assert!(
            crate::cli::Cli::try_parse_from(["pixelcoords", "--pick", "--monitor", "0"]).is_err()
        );
    }

    #[test]
    fn assert_cli_parses_hyphenated_points() {
        let args = crate::cli::Cli::try_parse_from([
            "pixelcoords",
            "assert",
            "--session",
            "shots",
            "--point",
            "-1920,540",
            "--expect",
            "submit",
            "--space",
            "global",
        ])
        .unwrap();
        let Some(crate::cli::Command::Assert {
            point,
            expect,
            space,
            ..
        }) = args.command
        else {
            panic!("expected the assert subcommand")
        };
        assert_eq!(point, "-1920,540");
        assert_eq!(expect.as_deref(), Some("submit"));
        assert_eq!(space, crate::cli::SpaceArg::Global);
    }

    /// `--label` still parses on `assert`, and that is deliberate: the
    /// flag has to be *recognized* in order to be refused with an
    /// explanation. Letting clap reject it as unknown would say "no such
    /// flag" about a flag that worked last release and will work again
    /// next release, meaning something else.
    #[test]
    fn assert_still_accepts_label_so_it_can_refuse_it() {
        let args = crate::cli::Cli::try_parse_from([
            "pixelcoords",
            "assert",
            "--session",
            "shots",
            "--point",
            "10,10",
            "--label",
            "submit",
        ])
        .unwrap();
        let Some(crate::cli::Command::Assert { label, expect, .. }) = args.command else {
            panic!("expected the assert subcommand")
        };
        assert_eq!(
            label.as_deref(),
            Some("submit"),
            "parsed, so main can exit 2 naming --expect"
        );
        assert!(expect.is_none());
    }

    #[test]
    fn valid_config_loads_and_applies() {
        let path = std::env::temp_dir().join("pixelcoords-test-good-config.toml");
        std::fs::write(&path, "[style]\nthickness = 7\nfill = true\n").unwrap();
        let config = load_config(Some(&path)).unwrap();
        let style = config.resolve_style().unwrap();
        assert_eq!(style.thickness, 7);
        assert!(style.fill);
        let _ = std::fs::remove_file(&path);
    }
}
