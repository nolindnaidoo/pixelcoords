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
    let mut best = (0.0f64, 0u32, 0u32);
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
            if variance > best.0 {
                best = (variance, left, top);
            }
            left += 100;
        }
        top += 80;
    }

    if best.0 < 25.0 {
        eprintln!(
            "the desktop is flat (best variance {:.1}) — no scenario over it is meaningful",
            best.0
        );
        return None;
    }

    let (_, x, y) = best;
    let (w, h) = (tile_w, tile_h);
    let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    crop.save(dir.join("crop-0-target.png"))
        .expect("crop written");

    // Built from what `doctor` reports, not from what a fixture would like
    // to be true: pixelcoords matches a session's monitor to an attached
    // one **by name**, and refuses the pair when they disagree. `size` is
    // logical and the session records physical, so it is scaled here —
    // getting that backwards is the mistake this tool exists to prevent.
    let monitor = json(&run(&["doctor", "--json"]))["monitors"][0].clone();
    let scale = monitor["scale"].as_f64().unwrap_or(1.0);
    let px = serde_json::json!({ "x": x, "y": y, "w": w, "h": h });
    let session = serde_json::json!({
        "schema": 1,
        "app": { "name": "pixelcoords", "version": env!("CARGO_PKG_VERSION") },
        "created_utc": "2026-01-01T00:00:00Z",
        "platform": null, "capture": null, "name": "scenarios",
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

    Some((
        i32::try_from(x).expect("in range"),
        i32::try_from(y).expect("in range"),
        i32::try_from(w).expect("in range"),
        i32::try_from(h).expect("in range"),
    ))
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

fn fixture() -> Option<Fixture> {
    if !enabled() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-scenarios-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let Some((x, y, w, h)) = session_over_the_screen(&dir) else {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    };
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
