//! End-to-end scenarios against a real display, on every platform.
//!
//! This is a screen-capture tool, and until these existed nothing tested
//! it against a screen. The unit suite covers geometry, the schema and
//! the matcher; none of it captures anything. `AGENTS.md` says overlay
//! behaviour cannot be verified headless — true, and it stops there. The
//! *rest* of the tool can be, on all three platforms, and a GitHub runner
//! turns out to have a display on each of them.
//!
//! Driving the built binary as a subprocess rather than calling into it:
//! that is what a user does, it exercises argument parsing and exit codes
//! for free, and a binary crate's integration tests cannot import its
//! modules anyway.
//!
//! **Opt-in.** These capture the screen of whatever machine runs them, so
//! they stay off unless `PIXELCOORDS_SCENARIOS=1` is set. `AGENTS.md`
//! requires the ordinary suite to be deterministic, and a real desktop is
//! not.
//!
//!     PIXELCOORDS_SCENARIOS=1 cargo test --test scenarios
//!
//! The overlay is still out of scope. It is interactive.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Numbers each fixture directory.
///
/// A counter, not the thread id: these run single-threaded — concurrent
/// captures of one display fail on macOS — so every test shares a thread
/// and would therefore share a directory, and a scenario that rewrites
/// the session would leak into the next one. It did, and `find` failed
/// because of it.
static NEXT_FIXTURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Skip unless asked. Returns `false` when the scenarios are off.
fn enabled() -> bool {
    std::env::var("PIXELCOORDS_SCENARIOS").is_ok()
}

fn binary() -> PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by Cargo for integration tests, and
    // points at the binary this test was built alongside — so the
    // scenarios can never run against a stale install.
    PathBuf::from(env!("CARGO_BIN_EXE_pixelcoords"))
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .output()
        .expect("the binary runs")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout was not JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Capture the screen and mark the most detailed tile in it.
///
/// Detail is not optional: a flat crop correlates with everything, and
/// the tool refuses one by name rather than matching it anywhere. What is
/// on a runner's desktop differs per platform, so the region is found
/// rather than assumed.
fn session_over_the_screen(dir: &Path) -> Option<(i32, i32, i32, i32)> {
    let out = run(&["shoot", "--out", &dir.display().to_string()]);
    assert_eq!(
        code(&out),
        0,
        "shoot failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let shot = dir.join("screenshot-0.png");
    assert!(shot.is_file(), "shoot wrote no screenshot");
    let img = image::open(&shot)
        .expect("the capture is a readable PNG")
        .to_rgba8();
    let (sw, sh) = (img.width(), img.height());

    let (tile_w, tile_h) = (200u32, 60u32);
    let mut candidates: Vec<(f64, u32, u32)> = Vec::new();
    let mut top = 0;
    while top + tile_h < sh.min(700) {
        let mut left = 0;
        while left + tile_w < sw.min(900) {
            let mut sum = 0.0f64;
            let mut sum_sq = 0.0f64;
            let mut count = 0.0f64;
            // Every fourth pixel: enough to rank tiles against each other,
            // cheap enough to scan a screen's worth of them.
            for row in (top..top + tile_h).step_by(4) {
                for col in (left..left + tile_w).step_by(4) {
                    let rgba = img.get_pixel(col, row).0;
                    let luma = f64::from(rgba[0]).mul_add(
                        0.299,
                        f64::from(rgba[1]).mul_add(0.587, f64::from(rgba[2]) * 0.114),
                    );
                    sum += luma;
                    sum_sq += luma * luma;
                    count += 1.0;
                }
            }
            let variance = (sum_sq / count) - (sum / count).powi(2);
            candidates.push((variance, left, top));
            left += 100;
        }
        top += 80;
    }

    // Detail is necessary but not sufficient. A desktop can be busy *and*
    // repetitive — a gradient, a tiled background, a row of identical
    // icons — and a crop of a repeating thing matches in more than one
    // place, which the tool refuses because an ambiguous match yields no
    // point worth acting on. Variance cannot see that; `find` can.
    //
    // So: rank by detail, then ask the tool which candidate is actually
    // markable, and take the first it accepts. That is the same judgement
    // a human makes when marking a region, delegated to the thing that
    // owns it.
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    candidates.truncate(12);
    if candidates.first().is_none_or(|best| best.0 < 25.0) {
        eprintln!(
            "the desktop is flat — no scenario over it is meaningful (best variance {:.1})",
            candidates.first().map_or(0.0, |c| c.0)
        );
        return None;
    }

    let monitor = json(&run(&["doctor", "--json"]))["monitors"][0].clone();
    let scale = monitor["scale"].as_f64().unwrap_or(1.0);
    let (w, h) = (tile_w, tile_h);

    for (variance, x, y) in candidates {
        let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
        crop.save(dir.join("crop-0-target.png"))
            .expect("crop written");
        write_session(
            dir,
            &Marked {
                monitor: &monitor,
                scale,
                screen: (sw, sh),
                region: (x, y, w, h),
            },
        );

        let report = json(&run(&["find", "--session", &dir.display().to_string()]));
        let row = &report["results"][0];
        if row["found"] == true && row["ambiguous"] == false {
            eprintln!("marked {x},{y} {w}x{h} (variance {variance:.1})");
            return Some((
                i32::try_from(x).expect("in range"),
                i32::try_from(y).expect("in range"),
                i32::try_from(w).expect("in range"),
                i32::try_from(h).expect("in range"),
            ));
        }
    }

    eprintln!("no tile on this desktop is both detailed and unique — nothing to mark");
    None
}

/// One marked region, and the display it was marked on.
struct Marked<'a> {
    monitor: &'a serde_json::Value,
    scale: f64,
    screen: (u32, u32),
    region: (u32, u32, u32, u32),
}

/// Write the session a human would have saved for one marked region.
///
/// Grouped into a struct rather than nine arguments, because the house
/// rule forbids silencing `too_many_arguments` inline and the lint is
/// right anyway.
fn write_session(dir: &Path, marked: &Marked<'_>) {
    let (sw, sh) = marked.screen;
    let (x, y, w, h) = marked.region;
    let (monitor, scale) = (marked.monitor, marked.scale);
    let px = serde_json::json!({ "x": x, "y": y, "w": w, "h": h });
    let session = serde_json::json!({
        "schema": 1,
        "app": { "name": "pixelcoords", "version": env!("CARGO_PKG_VERSION") },
        "created_utc": "2026-01-01T00:00:00Z",
        "platform": null, "capture": null, "name": "scenarios",
        // Named from what `doctor` reports, not from what a fixture would
        // like to be true: pixelcoords matches a session's monitor to an
        // attached one **by name**, and refuses the pair when they
        // disagree. The size comes from the capture itself, which is
        // physical — `doctor` reports logical, and conflating the two is
        // the class of bug this whole tool exists to prevent.
        "monitors": [{
            "index": 0,
            "name": monitor["name"],
            "primary": true,
            "origin_px": { "x": 0, "y": 0 },
            "size_px": { "w": sw, "h": sh },
            "scale": scale,
        }],
        "target": null, "measures": [],
        "selections": [{
            "shape": "rect", "label": "target", "monitor": 0,
            "px": px, "global_px": px, "rot_deg": null, "window_px": null,
            "crop": "crop-0-target.png", "color": null,
        }],
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec_pretty(&session).expect("session serializes"),
    )
    .expect("session written");
}

/// A scratch directory that removes itself, so a scenario run leaves no
/// screenshots of somebody's desktop lying around.
struct Fixture {
    dir: PathBuf,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    fn path(&self) -> String {
        self.dir.display().to_string()
    }
    fn centre(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// The one capture every scenario works from.
///
/// Captured once and copied, not re-captured per test. Sixteen sequential
/// captures is real pressure on a virtual display — Linux CI failed a
/// `shoot` partway through a run — and nothing here needs sixteen
/// captures of the same still screen. Each test still gets its own
/// directory, because a scenario that rewrites the session must not leak
/// into the next one.
static SHARED: std::sync::OnceLock<Option<Capture>> = std::sync::OnceLock::new();

/// Where the one capture lives, and the region marked in it.
#[derive(Clone)]
struct Capture {
    dir: PathBuf,
    region: (i32, i32, i32, i32),
}

fn shared_capture() -> Option<&'static Capture> {
    SHARED
        .get_or_init(|| {
            let dir =
                std::env::temp_dir().join(format!("pixelcoords-capture-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("capture dir");
            let region = session_over_the_screen(&dir)?;
            Some(Capture { dir, region })
        })
        .as_ref()
}

fn fixture() -> Option<Fixture> {
    if !enabled() {
        return None;
    }
    let capture = shared_capture()?;
    let (source, (x, y, w, h)) = (&capture.dir, capture.region);
    let seq = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-scenarios-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    for entry in std::fs::read_dir(source).expect("the capture is readable") {
        let entry = entry.expect("a directory entry");
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).expect("copied");
        }
    }
    Some(Fixture { dir, x, y, w, h })
}

/// Every scenario needs a captured screen, so they share one macro rather
/// than repeating the skip.
macro_rules! scenario {
    ($f:ident) => {
        let Some($f) = fixture() else { return };
    };
}

#[test]
fn shoot_writes_a_png_the_size_of_the_display() {
    scenario!(f);
    let monitor = json(&run(&["doctor", "--json"]))["monitors"][0].clone();
    // `doctor` reports logical, a capture is physical. On a 2x display
    // those differ by exactly the scale, and conflating them is the whole
    // class of bug this tool exists to prevent — so the assertion states
    // the relationship rather than assuming they are equal.
    let scale = monitor["scale"].as_f64().expect("scale");
    let logical_w = monitor["size"]["w"].as_f64().expect("width");
    let logical_h = monitor["size"]["h"].as_f64().expect("height");
    let img = image::open(PathBuf::from(f.path()).join("screenshot-0.png")).expect("readable");
    assert_eq!(
        (f64::from(img.width()), f64::from(img.height())),
        ((logical_w * scale).round(), (logical_h * scale).round()),
        "a capture must be the display's logical size times its scale"
    );
}

#[test]
fn find_locates_the_region_in_a_fresh_capture() {
    scenario!(f);
    // Exit 1 is "not found" — an answer. Only 2 means it could not look.
    let out = run(&["find", "--session", &f.path()]);
    assert_ne!(code(&out), 2, "{}", String::from_utf8_lossy(&out.stderr));
    let report = json(&out);
    let row = &report["results"][0];
    assert_eq!(row["found"], true, "{report}");
    assert_eq!(
        row["ambiguous"], false,
        "a unique crop must not be ambiguous: {report}"
    );
    let delta = &row["delta"];
    assert!(
        delta.is_null() || (delta["dx"] == 0 && delta["dy"] == 0),
        "the region has not moved: {report}"
    );
}

#[test]
fn resolve_answers_the_regions_centre() {
    scenario!(f);
    let report = json(&run(&[
        "resolve",
        "--session",
        &f.path(),
        "--units",
        "physical",
    ]));
    let point = &report["results"][0]["point"];
    let (cx, cy) = f.centre();
    assert_eq!(
        (point["x"].as_i64(), point["y"].as_i64()),
        (Some(i64::from(cx)), Some(i64::from(cy))),
        "{report}"
    );
}

#[test]
fn assert_scores_a_point_against_the_region() {
    scenario!(f);
    let (cx, cy) = f.centre();
    let inside = run(&[
        "assert",
        "--session",
        &f.path(),
        "--point",
        &format!("{cx},{cy}"),
        "--expect",
        "target",
    ]);
    assert_eq!(code(&inside), 0, "a point inside must exit 0");

    let outside = run(&[
        "assert",
        "--session",
        &f.path(),
        "--point",
        &format!("{},{}", f.x - 10_000, f.y - 10_000),
        "--expect",
        "target",
    ]);
    assert_eq!(code(&outside), 1, "a point outside must exit 1, not 2");
}

#[test]
fn diff_finds_an_unchanged_region_within_tolerance() {
    scenario!(f);
    let out = run(&["diff", "--session", &f.path()]);
    assert_ne!(code(&out), 2, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        json(&out)["ok"],
        true,
        "nothing has changed since the capture"
    );
}

#[test]
fn wait_returns_at_once_when_the_condition_already_holds() {
    scenario!(f);
    let out = run(&[
        "wait",
        "--session",
        &f.path(),
        "--for",
        "match",
        "--timeout",
        "5s",
        "--interval",
        "200ms",
    ]);
    assert_ne!(code(&out), 2, "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(json(&out)["ok"], true);
}

#[test]
fn emit_generates_for_every_format_it_advertises() {
    scenario!(f);
    for format in [
        "pyautogui",
        "cliclick",
        "xdotool",
        "powershell",
        "applescript",
        "ydotool",
    ] {
        let out = run(&["emit", "--session", &f.path(), "--format", format]);
        assert_eq!(code(&out), 0, "{format} failed");
        let text = String::from_utf8_lossy(&out.stdout);
        let body: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with(['#', '-']) && !l.trim().is_empty())
            .collect();
        assert!(!body.is_empty(), "{format} emitted only comments");
    }
}

#[test]
fn the_mcp_server_agrees_with_the_cli() {
    scenario!(f);
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"pixelcoords_find","arguments":{{"session":"{}"}}}}}}"#,
        f.path().replace('\\', "\\\\")
    );
    let mut child = Command::new(binary())
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("mcp starts");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{request}").expect("request written");
    }
    let out = child.wait_with_output().expect("mcp answers");
    let line = String::from_utf8_lossy(&out.stdout);
    let reply: serde_json::Value =
        serde_json::from_str(line.lines().next().expect("a reply")).expect("valid JSON");
    let result = &reply["result"];
    assert_eq!(result["isError"], false, "a located region is not an error");
    assert_eq!(
        result["structuredContent"]["ok"], true,
        "MCP disagrees with the CLI about the same session: {reply}"
    );
}

#[test]
fn an_unknown_label_exits_two() {
    scenario!(f);
    let out = run(&["resolve", "--session", &f.path(), "--label", "nosuchlabel"]);
    assert_eq!(code(&out), 2, "a malformed question exits 2");
}

// ---------------------------------------------------------------------
// Deeper surface. Everything above proves the happy path of one command;
// these cover the flags, the shape kinds and the commands that had no
// coverage against a real display at all.
// ---------------------------------------------------------------------

#[test]
fn windows_answers_or_says_why_it_cannot() {
    if !enabled() {
        return;
    }
    let out = run(&["windows", "--json"]);
    // X11 and macOS list; Wayland exits nonzero and points at `--pick`,
    // because the protocol withholds window geometry. Both are correct
    // answers — what would be wrong is a crash or an empty success that
    // implies there are no windows.
    match code(&out) {
        0 => {
            let report = json(&out);
            assert!(
                report.get("windows").is_some() || report.is_array(),
                "a zero exit must carry a list: {report}"
            );
        }
        other => {
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(
                !said.trim().is_empty(),
                "a refusal must say why, exit {other}"
            );
        }
    }
}

#[test]
fn rename_sticks_and_is_read_back() {
    scenario!(f);
    let out = run(&[
        "rename",
        "--session",
        &f.path(),
        "--name",
        "a friendly name",
    ]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let session: serde_json::Value = serde_json::from_slice(
        &std::fs::read(PathBuf::from(f.path()).join("session.json")).unwrap(),
    )
    .expect("session still parses after a rename");
    assert_eq!(
        session["name"], "a friendly name",
        "the name is written where the resume picker reads it"
    );
}

#[test]
fn assert_streams_a_trajectory_in_one_process() {
    scenario!(f);
    let (cx, cy) = f.centre();
    // Three inside, one far outside: the aggregate must be a miss while
    // the rows still say which was which, because scoring a trajectory is
    // the reason `--stdin` exists.
    let points = format!(
        "{cx},{cy}\n{cx},{cy}\n{},{}\n{cx},{cy}\n",
        f.x - 9_000,
        f.y - 9_000
    );
    let mut child = Command::new(binary())
        .args(["assert", "--session", &f.path(), "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("assert starts");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(points.as_bytes())
            .expect("points written");
    }
    let out = child.wait_with_output().expect("assert answers");
    assert_eq!(out.status.code(), Some(1), "one miss makes the run a miss");
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("a report on stdout");
    let rows = report["results"].as_array().expect("rows");
    assert_eq!(rows.len(), 4, "one row per point: {report}");
    assert_eq!(rows[0]["hit"], true);
    assert_eq!(rows[2]["hit"], false, "the third point was outside");
    assert_eq!(rows[3]["hit"], true, "a miss does not poison the rest");
}

#[test]
fn wait_for_change_times_out_on_a_still_screen() {
    scenario!(f);
    let out = run(&[
        "wait",
        "--session",
        &f.path(),
        "--for",
        "change",
        "--timeout",
        "1s",
        "--interval",
        "200ms",
    ]);
    // Nothing is changing, so the condition never holds. That is exit 1 —
    // a negative answer — and emphatically not 2, which would mean the
    // question was malformed.
    assert_eq!(
        code(&out),
        1,
        "a timeout is exit 1: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json(&out)["ok"], false);
}

#[test]
fn diff_against_the_original_capture_finds_nothing_changed() {
    scenario!(f);
    // `--against` compares to stored artifacts instead of capturing. The
    // session's own screenshot is by definition identical to itself, so
    // this isolates the comparison from anything moving on screen.
    let out = run(&["diff", "--session", &f.path(), "--against", &f.path()]);
    assert_ne!(code(&out), 2, "{}", String::from_utf8_lossy(&out.stderr));
    let report = json(&out);
    assert_eq!(
        report["ok"], true,
        "a capture differs from itself by nothing: {report}"
    );
    assert_eq!(report["results"][0]["changed_px"], 0);
}

/// Every shape a human can mark, resolved against a real session.
///
/// The unit suite property-tests these over synthetic coordinates. What
/// it cannot show is that a session carrying one round-trips through the
/// schema and comes back out of `resolve` with a click point inside it —
/// which is the only thing a caller actually asks for.
#[test]
fn every_shape_kind_resolves_to_a_point_inside_itself() {
    scenario!(f);
    let dir = PathBuf::from(f.path());
    let (x, y, w, h) = (f.x, f.y, f.w, f.h);
    let (cx, cy) = (x + w / 2, y + h / 2);

    // Each is sized to sit within the region already marked, so the
    // shapes are on real pixels rather than off the edge of the screen.
    let shapes = [
        (
            "rect",
            serde_json::json!({ "x": x, "y": y, "w": w, "h": h }),
        ),
        (
            "circle",
            serde_json::json!({ "cx": cx, "cy": cy, "r": h / 3 }),
        ),
        (
            "ellipse",
            serde_json::json!({ "cx": cx, "cy": cy, "rx": w / 3, "ry": h / 3 }),
        ),
        (
            "triangle",
            serde_json::json!({
                "ax": x, "ay": y + h, "bx": x + w, "by": y + h, "cx": cx, "cy": y
            }),
        ),
        (
            "poly",
            serde_json::json!({ "points": [
                { "x": x, "y": y }, { "x": x + w, "y": y },
                { "x": x + w, "y": y + h }, { "x": x, "y": y + h }
            ] }),
        ),
        (
            "freehand",
            serde_json::json!({ "points": [
                { "x": x, "y": y }, { "x": x + w, "y": y + h / 2 },
                { "x": x, "y": y + h }
            ] }),
        ),
    ];

    for (kind, px) in shapes {
        let mut session: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap())
                .expect("the session parses");
        session["selections"][0]["shape"] = serde_json::json!(kind);
        session["selections"][0]["px"] = px.clone();
        session["selections"][0]["global_px"] = px.clone();
        std::fs::write(
            dir.join("session.json"),
            serde_json::to_vec_pretty(&session).unwrap(),
        )
        .unwrap();

        let out = run(&["resolve", "--session", &f.path(), "--units", "physical"]);
        assert_eq!(
            code(&out),
            0,
            "{kind} did not resolve: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let point = &json(&out)["results"][0]["point"];
        let (px_x, px_y) = (
            point["x"].as_i64().expect("x"),
            point["y"].as_i64().expect("y"),
        );

        // The click point must be a point the shape actually contains —
        // `assert` is the tool's own answer to that, so the two are held
        // against each other rather than against my arithmetic.
        let scored = run(&[
            "assert",
            "--session",
            &f.path(),
            "--point",
            &format!("{px_x},{px_y}"),
            "--expect",
            "target",
        ]);
        assert_eq!(
            code(&scored),
            0,
            "{kind} resolved to ({px_x}, {px_y}), which it does not contain"
        );
    }
}

/// A rotated selection is still hit-tested at the angle it was saved.
#[test]
fn a_rotated_region_resolves_to_a_point_inside_itself() {
    scenario!(f);
    let dir = PathBuf::from(f.path());
    let mut session: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap()).unwrap();
    session["selections"][0]["rot_deg"] = serde_json::json!(30);
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec_pretty(&session).unwrap(),
    )
    .unwrap();

    let out = run(&["resolve", "--session", &f.path(), "--units", "physical"]);
    assert_eq!(code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let point = &json(&out)["results"][0]["point"];
    let scored = run(&[
        "assert",
        "--session",
        &f.path(),
        "--point",
        &format!("{},{}", point["x"], point["y"]),
        "--expect",
        "target",
    ]);
    assert_eq!(
        code(&scored),
        0,
        "a rotated region resolved to a point outside itself"
    );
}
