//! The exit codes and refusals, against sessions built by hand.
//!
//! `scenarios.rs` needs a display and `pixelcoords` on PATH, so it is gated
//! behind an environment variable and only CI runs it. **This file needs
//! neither.** Every session here is written by the test, and every command
//! it drives answers without capturing the screen — so it runs in the
//! ordinary `cargo test --workspace`, on every platform, on every push, on
//! a developer's laptop as readily as on a runner.
//!
//! That division is deliberate: the exit codes are the API. A caller
//! programs against "0 means yes, 1 means no, 2 means the question was
//! malformed", and a change to which code a refusal returns breaks scripts
//! silently. Those deserve a gate that always runs, not one that needs a
//! screen to be plugged in.
//!
//! Sessions are built with two monitors at different scales, because the
//! interesting refusals are the ones that only exist when a session
//! describes more than one screen.

use std::path::{Path, PathBuf};
use std::process::Output;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pixelcoords"))
}

/// The exit code, with a signal reported as something that is not 0, 1 or 2
/// — so a killed process can never be read as an answer.
fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn run(args: &[&str]) -> Output {
    std::process::Command::new(binary())
        .args(args)
        .output()
        .expect("pixelcoords runs")
}

/// One rect selection, in physical pixels.
fn selection(label: &str, x: i32, y: i32, monitor: usize) -> serde_json::Value {
    let origin = if monitor == 0 { 0 } else { 3000 };
    serde_json::json!({
        "shape": "rect",
        "label": label,
        "monitor": monitor,
        "px": { "x": x, "y": y, "w": 40, "h": 20 },
        "global_px": { "x": x + origin, "y": y, "w": 40, "h": 20 },
        "rot_deg": null,
        "window_px": null,
        "crop": format!("crop-{label}.png"),
        "color": null,
    })
}

/// A two-monitor session at different scales, written to a fresh directory.
///
/// No crop files are written. Nothing here matches anything — these are the
/// commands that answer from the file alone, and a missing crop is itself
/// one of the refusals worth pinning.
fn session(name: &str, selections: &[serde_json::Value]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-contracts-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");

    let session = serde_json::json!({
        "schema": 1,
        "app": { "name": "pixelcoords", "version": "0.7.0" },
        "created_utc": "2026-01-01T00:00:00Z",
        "platform": "macos",
        "capture": null,
        "name": name,
        "monitors": [
            { "index": 0, "name": "built-in", "primary": true,
              "origin_px": { "x": 0, "y": 0 },
              "size_px": { "w": 3000, "h": 2000 }, "scale": 2.0 },
            { "index": 1, "name": "external", "primary": false,
              "origin_px": { "x": 3000, "y": 0 },
              "size_px": { "w": 1920, "h": 1080 }, "scale": 1.0 },
        ],
        "target": null,
        "measures": [],
        "selections": selections.to_vec(),
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec_pretty(&session).expect("serialises"),
    )
    .expect("session written");
    dir
}

/// The ordinary session most tests want: one label per region.
fn two_monitor_session(name: &str) -> PathBuf {
    session(
        name,
        &[selection("near", 10, 10, 0), selection("far", 200, 200, 1)],
    )
}

fn path(dir: &Path) -> String {
    dir.display().to_string()
}

// ---------------------------------------------------------------------------
// Exit 0 and 1: the answer
// ---------------------------------------------------------------------------

/// A point inside a region is a hit, and a hit is exit 0. The region on the
/// unscaled monitor spans [3200, 3240) x [200, 220) in global pixels.
#[test]
fn a_point_inside_a_region_is_exit_zero() {
    let dir = two_monitor_session("hit");
    for point in ["3200,200", "3201,201", "3239,219"] {
        let out = run(&["assert", "--session", &path(&dir), "--point", point]);
        assert_eq!(code(&out), 0, "{point} should hit: {}", said(&out));
    }
}

/// A point outside every region is a *negative answer*, not an error — the
/// distinction a caller programs against. Exit 1.
///
/// The far edges are exclusive: a 40-wide region starting at 3200 covers up
/// to 3239, and 3240 is the first pixel outside it. Pinned because an
/// off-by-one here silently shifts every click a caller makes.
#[test]
fn a_point_outside_every_region_is_exit_one() {
    let dir = two_monitor_session("miss");
    for point in ["3199,199", "3240,220", "0,0"] {
        let out = run(&["assert", "--session", &path(&dir), "--point", point]);
        assert_eq!(code(&out), 1, "{point} should miss: {}", said(&out));
    }
}

/// `resolve` answers from the file alone, so it needs no screen — and a
/// session with monitors at different scales must still resolve each
/// region against *its own* monitor.
#[test]
fn resolve_answers_each_region_on_its_own_monitor() {
    let dir = two_monitor_session("resolve");
    let out = run(&["resolve", "--session", &path(&dir)]);
    assert_eq!(code(&out), 0, "{}", said(&out));

    let report: serde_json::Value = serde_json::from_str(&said(&out)).expect("JSON");
    let scales: Vec<f64> = report["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter_map(|r| r["scale"].as_f64())
        .collect();
    assert_eq!(
        scales,
        [2.0, 1.0],
        "each region carries its own monitor's scale: {report}"
    );
}

// ---------------------------------------------------------------------------
// Exit 2: the question was malformed
// ---------------------------------------------------------------------------

/// Monitor-local coordinates are meaningless without saying which monitor,
/// and guessing would put the point on the wrong screen. Naming the
/// monitors it does have is what makes the message actionable.
#[test]
fn monitor_space_without_a_monitor_is_refused_on_a_multi_monitor_session() {
    let dir = two_monitor_session("ambiguous-space");
    let out = run(&[
        "assert",
        "--session",
        &path(&dir),
        "--point",
        "5,5",
        "--space",
        "monitor",
    ]);
    assert_eq!(code(&out), 2, "{}", said(&out));
    assert!(
        said(&out).contains("[0, 1]"),
        "the refusal names the monitors it has: {}",
        said(&out)
    );
}

/// A monitor that is not in the session is a malformed question, and the
/// answer says which ones are.
#[test]
fn an_unknown_monitor_is_refused_and_the_real_ones_named() {
    let dir = two_monitor_session("unknown-monitor");
    let out = run(&[
        "assert",
        "--session",
        &path(&dir),
        "--point",
        "5,5",
        "--space",
        "monitor",
        "--monitor",
        "9",
    ]);
    assert_eq!(code(&out), 2, "{}", said(&out));
    assert!(
        said(&out).contains("[0, 1]"),
        "the refusal names the monitors it has: {}",
        said(&out)
    );
}

/// An unknown label is exit 2 — a caller's flow is wrong, which is not the
/// same as the region not being on screen. The message lists what it does
/// have, because "no such label" without the alternatives is a dead end.
#[test]
fn an_unknown_label_is_refused_and_the_real_ones_listed() {
    let dir = two_monitor_session("unknown-label");
    let out = run(&["resolve", "--session", &path(&dir), "--label", "nope"]);
    assert_eq!(code(&out), 2, "{}", said(&out));
    let said = said(&out);
    assert!(
        said.contains("near") && said.contains("far"),
        "the refusal lists the labels it has: {said}"
    );
}

/// A session directory that is not there.
#[test]
fn a_missing_session_is_refused() {
    let out = run(&["resolve", "--session", "/no/such/session/anywhere"]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

/// Every command that reads a session refuses an unreadable one the same
/// way — exit 2.
///
/// `rename` used to exit **1** here. It bubbled its error out of `main`,
/// which is Rust's default failure code, and 1 is the code that means "a
/// real answer, and the answer is no". A script checking for 2 to mean
/// "your input is wrong" would have missed it. `rename` already exited 2
/// for a session that was *missing*, so it disagreed with itself as well
/// as with the other six.
#[test]
fn every_command_refuses_an_unreadable_session_with_the_same_code() {
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-contracts-{}-unreadable-all",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(dir.join("session.json"), "not json at all").expect("written");
    let at = path(&dir);

    let commands: [&[&str]; 7] = [
        &["resolve", "--session", &at],
        &["assert", "--session", &at, "--point", "1,1"],
        &["emit", "--session", &at],
        &["diff", "--session", &at],
        &["find", "--session", &at],
        &["wait", "--session", &at, "--timeout", "1ms"],
        &["rename", "--session", &at, "--name", "x"],
    ];
    for command in commands {
        let out = run(command);
        assert_eq!(
            code(&out),
            2,
            "{} should exit 2 on an unreadable session: {}",
            command[0],
            said(&out)
        );
    }

    // And a session that is not there at all. `rename` and `resume`
    // resolve the path before they load it, and that half kept bubbling
    // out of `main` as exit 1 after the load half was fixed.
    let absent = "/no/such/session/anywhere";
    let missing: [&[&str]; 8] = [
        &["resolve", "--session", absent],
        &["assert", "--session", absent, "--point", "1,1"],
        &["emit", "--session", absent],
        &["diff", "--session", absent],
        &["find", "--session", absent],
        &["wait", "--session", absent, "--timeout", "1ms"],
        &["rename", "--session", absent, "--name", "x"],
        &["resume", "--session", absent],
    ];
    for command in missing {
        let out = run(command);
        assert_eq!(
            code(&out),
            2,
            "{} should exit 2 on a missing session: {}",
            command[0],
            said(&out)
        );
    }
}

/// A session file that is not JSON at all.
#[test]
fn an_unreadable_session_is_refused() {
    let dir =
        std::env::temp_dir().join(format!("pixelcoords-contracts-{}-junk", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(dir.join("session.json"), "this is not json").expect("written");

    let out = run(&["resolve", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

/// A session with no selections at all has nothing to answer about.
#[test]
fn a_session_with_no_selections_is_refused() {
    let dir = session("empty", &[]);
    let out = run(&["resolve", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

/// A selection can name a monitor the session does not describe — a
/// hand-edited file, or one saved before a display was unplugged. Answering
/// would mean inventing a screen.
#[test]
fn a_selection_on_an_undescribed_monitor_is_refused() {
    let dir = session("stray-monitor", &[selection("stray", 10, 10, 7)]);
    let out = run(&["resolve", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
    assert!(
        said(&out).contains('7'),
        "the refusal names the monitor it could not find: {}",
        said(&out)
    );
}

// ---------------------------------------------------------------------------
// Arguments that are quantities
// ---------------------------------------------------------------------------

/// `--min-score` is a correlation score and `--tolerance` is a percentage.
/// They are easy to confuse and a confused one is silently wrong rather
/// than loud — 50 as a score matches nothing, 0.5 as a tolerance is nearly
/// exact — so both are bounds-checked and the message says which is which.
#[test]
fn a_score_outside_its_range_is_refused() {
    let dir = two_monitor_session("score");
    for bad in ["50", "-1", "1.5"] {
        let out = run(&[
            "wait",
            "--session",
            &path(&dir),
            "--min-score",
            bad,
            "--timeout",
            "1ms",
        ]);
        assert_eq!(code(&out), 2, "--min-score {bad}: {}", said(&out));
    }
}

#[test]
fn a_tolerance_outside_its_range_is_refused() {
    let dir = two_monitor_session("tolerance");
    for bad in ["-1", "101"] {
        let out = run(&["diff", "--session", &path(&dir), "--tolerance", bad]);
        assert_eq!(code(&out), 2, "--tolerance {bad}: {}", said(&out));
    }
}

/// A duration this tool cannot parse is refused rather than defaulted.
#[test]
fn an_unparseable_duration_is_refused() {
    let dir = two_monitor_session("duration");
    let out = run(&["wait", "--session", &path(&dir), "--timeout", "whenever"]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

// ---------------------------------------------------------------------------
// Things that must not need a screen
// ---------------------------------------------------------------------------

/// `emit` prints a snippet per selection in another tool's convention. It
/// reads the session and nothing else, so it must work with no display —
/// which is the case a CI user has.
#[test]
fn emit_works_from_the_file_alone_in_every_format() {
    let dir = two_monitor_session("emit");

    // Read the formats out of the help rather than listing them here, so a
    // format added later is covered without anyone remembering to.
    let help = said(&run(&["emit", "--help"]));
    let formats: Vec<String> = help
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("- ")?;
            let name = rest.split(':').next()?.trim();
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase()))
                .then(|| name.to_string())
        })
        .collect();
    assert!(
        formats.len() >= 4,
        "expected the advertised formats, parsed: {formats:?}"
    );

    for format in formats {
        let out = run(&["emit", "--session", &path(&dir), "--format", &format]);
        assert_eq!(code(&out), 0, "{format}: {}", said(&out));
        assert!(
            !said(&out).trim().is_empty(),
            "{format} advertised a format and emitted nothing"
        );
    }
}

/// `rename` writes to the session and reads back through the picker, and
/// neither half touches the screen.
#[test]
fn rename_round_trips_without_a_display() {
    let dir = two_monitor_session("rename");
    let out = run(&["rename", "--session", &path(&dir), "--name", "a good name"]);
    assert_eq!(code(&out), 0, "{}", said(&out));

    let written = std::fs::read_to_string(dir.join("session.json")).expect("readable");
    let session: serde_json::Value = serde_json::from_str(&written).expect("JSON");
    assert_eq!(session["name"], "a good name", "{session}");
}

/// Clearing the name is deliberate, not an error: an empty name is how a
/// session goes back to being listed by its folder.
#[test]
fn an_empty_rename_clears_the_name() {
    let dir = two_monitor_session("rename-empty");
    assert_eq!(
        code(&run(&[
            "rename",
            "--session",
            &path(&dir),
            "--name",
            "temporary"
        ])),
        0
    );
    let out = run(&["rename", "--session", &path(&dir), "--name", ""]);
    assert_eq!(code(&out), 0, "{}", said(&out));

    let written = std::fs::read_to_string(dir.join("session.json")).expect("readable");
    let session: serde_json::Value = serde_json::from_str(&written).expect("JSON");
    assert!(
        session["name"].is_null() || session["name"] == "",
        "the name should be gone: {session}"
    );
}

/// Labels are matched case-insensitively, and that is a promise the help
/// text makes. A caller who types the label as it appears on screen should
/// not have to match the case they happened to save it with.
#[test]
fn a_label_matches_regardless_of_case() {
    let dir = two_monitor_session("case");
    let out = run(&["resolve", "--session", &path(&dir), "--label", "NEAR"]);
    assert_eq!(code(&out), 0, "{}", said(&out));

    let report: serde_json::Value = serde_json::from_str(&said(&out)).expect("JSON");
    assert_eq!(report["results"][0]["label"], "near", "{report}");
}

// ---------------------------------------------------------------------------
// The config file, which `doctor` is the headless door to
// ---------------------------------------------------------------------------

/// Write a config file and return the path.
fn config_file(name: &str, body: &str) -> String {
    let dir = std::env::temp_dir().join(format!("pixelcoords-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join(name);
    std::fs::write(&file, body).expect("written");
    file.display().to_string()
}

/// What `doctor` says about the config file, on its own.
///
/// The process exit code is the health of *everything* — permissions, the
/// monitor table, the config — and a headless Linux runner has no display,
/// so it is unhealthy for reasons that have nothing to do with the file
/// under test. These assertions read the config verdict directly.
fn config_verdict(args: &[&str]) -> serde_json::Value {
    let out = run(args);
    let report: serde_json::Value = serde_json::from_str(&said(&out))
        .unwrap_or_else(|e| panic!("doctor should speak JSON ({e}): {}", said(&out)));
    report["config"].clone()
}

/// `load_config` refuses a file the user *named* that is not there, but
/// `doctor` reported "absent, defaults in effect" and exited 0 — so a
/// typo'd `--config` path passed the health check and then failed the
/// launch. The comment on the range check says the whole point is that
/// "`doctor` refuses the same values a launch would"; this is the case
/// where it did not.
#[test]
fn doctor_refuses_a_config_file_that_was_named_but_is_not_there() {
    let missing = config_file("placeholder.toml", "");
    let missing = missing.replace("placeholder.toml", "no-such-config.toml");

    let verdict = config_verdict(&["doctor", "--config", &missing, "--json"]);
    assert_eq!(verdict["status"], "missing", "{verdict}");

    // An invalid config forces unhealthy on its own, so the exit code is
    // meaningful here even where a runner has no display.
    let out = run(&["doctor", "--config", &missing, "--json"]);
    assert_ne!(code(&out), 0, "{}", said(&out));
}

/// The other half: with no `--config` at all, an absent file at the default
/// location is normal and must not be complained about. Otherwise the fix
/// above turns every fresh install unhealthy.
///
/// Read off the config verdict rather than the exit code — a headless
/// runner has no display and is unhealthy for reasons that have nothing to
/// do with the config file.
#[test]
fn doctor_is_content_when_no_config_was_asked_for() {
    let verdict = config_verdict(&["doctor", "--json"]);
    let status = verdict["status"].as_str().unwrap_or_default();
    assert!(
        status.contains("defaults in effect") || status.contains("no config directory"),
        "no --config means defaults, not a complaint: {verdict}"
    );
}

/// A config whose *syntax* is broken is reported against the file, not
/// swallowed.
#[test]
fn doctor_refuses_a_config_that_is_not_toml() {
    let path = config_file("broken.toml", "this is not toml at all [[[\n");
    let verdict = config_verdict(&["doctor", "--config", &path, "--json"]);
    assert_eq!(verdict["status"], "invalid", "{verdict}");
    assert_ne!(code(&run(&["doctor", "--config", &path, "--json"])), 0);
}

/// A config that parses but names a key this build does not have is
/// refused too — a silently-ignored key is a setting someone believes is
/// in effect and is not.
#[test]
fn doctor_refuses_a_config_with_an_unknown_key() {
    let path = config_file("unknown-key.toml", "resolve_style = \"centroid\"\n");
    let verdict = config_verdict(&["doctor", "--config", &path, "--json"]);
    let error = verdict["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("unknown field"),
        "the refusal names the key: {verdict}"
    );
    assert_ne!(code(&run(&["doctor", "--config", &path, "--json"])), 0);
}

/// A well-formed config loads and leaves the tool healthy.
#[test]
fn doctor_accepts_a_config_it_understands() {
    let path = config_file("fine.toml", "[style]\n");
    let verdict = config_verdict(&["doctor", "--config", &path, "--json"]);
    assert_eq!(verdict["status"], "loaded", "{verdict}");
}

/// A config naming a hotkey action this build does not have used to load
/// clean: `hotkeys` was the one member of the config left out of the range
/// check, so `doctor` called the file healthy and the overlay died on it at
/// launch.
#[test]
fn doctor_refuses_a_config_with_an_unusable_hotkey() {
    for (name, body) in [
        (
            "bad-action.toml",
            "[[hotkeys]]\nkey = \"u\"\naction = \"no_such_action\"\n",
        ),
        (
            "bad-key.toml",
            "[[hotkeys]]\nkey = \"F5\"\naction = \"undo\"\n",
        ),
    ] {
        let path = config_file(name, body);
        let verdict = config_verdict(&["doctor", "--config", &path, "--json"]);
        assert_eq!(verdict["status"], "invalid", "{name}: {verdict}");
    }
}

/// A hotkey this build can actually bind leaves it healthy.
#[test]
fn doctor_accepts_a_hotkey_it_can_bind() {
    let path = config_file(
        "good-hotkey.toml",
        "[[hotkeys]]\nkey = \"u\"\naction = \"undo\"\n",
    );
    let verdict = config_verdict(&["doctor", "--config", &path, "--json"]);
    assert_eq!(verdict["status"], "loaded", "{verdict}");
}

// ---------------------------------------------------------------------------
// `resume`, as far as it goes without a window
// ---------------------------------------------------------------------------

/// `resume` opens the overlay, so most of it needs a desktop. Its
/// *refusals* do not: it loads the config, the bindings and the session
/// before it builds a single window, so everything it rejects it rejects
/// headless.
///
/// Worth pinning because `resume` used to exit 1 here — it bubbled its
/// error out of `main`, where 1 is Rust's default — and 1 is the code that
/// means "a real answer, and the answer is no".
#[test]
fn resume_refuses_a_session_it_cannot_read_without_opening_anything() {
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-contracts-{}-resume-junk",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(dir.join("session.json"), "not json").expect("written");

    let out = run(&["resume", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

/// A session describing no monitors cannot be reopened onto anything, and
/// `resume` says so rather than opening an empty window.
#[test]
fn resume_refuses_a_session_with_no_monitors() {
    let dir = std::env::temp_dir().join(format!(
        "pixelcoords-contracts-{}-resume-nomonitors",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let session = serde_json::json!({
        "schema": 1,
        "app": { "name": "pixelcoords", "version": "0.7.0" },
        "created_utc": "2026-01-01T00:00:00Z",
        "platform": "macos", "capture": null, "name": "no monitors",
        "monitors": [], "target": null, "measures": [], "selections": [],
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec_pretty(&session).expect("serialises"),
    )
    .expect("written");

    let out = run(&["resume", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
}

/// A hotkey binding `resume` cannot bind is rejected before the overlay
/// opens — the bindings resolve above the session load, so this is the
/// earliest refusal there is.
#[test]
fn resume_refuses_an_unbindable_hotkey_before_opening() {
    let dir = two_monitor_session("resume-bad-bind");
    let out = run(&["--bind", "F5=undo", "resume", "--session", &path(&dir)]);
    assert_eq!(code(&out), 2, "{}", said(&out));
    // Asserted on the message, not just the code: a runner with no display
    // fails `resume` anyway, and a test that cannot tell the two apart
    // would pass without proving the binding was ever looked at.
    assert!(
        said(&out).contains("hotkey binding"),
        "the refusal should name the binding, not the display: {}",
        said(&out)
    );
}
