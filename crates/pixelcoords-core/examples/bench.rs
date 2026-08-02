//! Times the core math the README makes claims about.
//!
//! Run it, paste the table, stamp it with your machine:
//!
//! ```text
//! cargo run --release -p pixelcoords-core --example bench
//! ```
//!
//! **Not a test and not a CI gate.** `AGENTS.md` requires tests to be
//! deterministic, and a timing harness is the opposite — a clock in CI is
//! a flaky job waiting to happen. This exists so the numbers in
//! `docs/PERFORMANCE.md` come from somewhere reproducible rather than
//! from an assertion nobody checked.
//!
//! Everything is synthesized rather than committed as a fixture: the
//! numbers then reproduce on any machine without an interactive capture,
//! and the session it builds spans two monitors at different DPI, which
//! is the path `--units auto` exists for.

use std::time::{Duration, Instant};

use pixelcoords_core::diff::{self, Baseline};
use pixelcoords_core::geometry::{Point, Rect, Shape, Size, ToolKind};
use pixelcoords_core::locate::{self, GrayImage, Template};
use pixelcoords_core::resolve;
use pixelcoords_core::session::{
    AppInfo, MonitorRecord, SCHEMA_VERSION, SelectionRecord, SessionFile,
};
use pixelcoords_core::space::{Origin, Resolved};
use pixelcoords_core::verdict;

/// Runs per measurement. The median is reported, so this only has to be
/// odd and large enough to step over one unlucky scheduling slice.
const RUNS: usize = 21;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Time `body` `RUNS` times and report the median. One untimed call first,
/// so the first-touch page faults land outside the measurement.
fn time<T>(mut body: impl FnMut() -> T) -> Duration {
    let _ = body();
    median(
        (0..RUNS)
            .map(|_| {
                let started = Instant::now();
                let value = body();
                let elapsed = started.elapsed();
                drop(value);
                elapsed
            })
            .collect(),
    )
}

fn row(name: &str, detail: &str, elapsed: Duration) {
    println!("| {name:<28} | {detail:<24} | {elapsed:>12.3?} |");
}

/// Pseudo-random but reproducible: a real screenshot is neither uniform
/// nor noise, and NCC on a flat image is not the number anyone wants.
fn textured(w: usize, h: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for pixel in rgba.chunks_exact_mut(4) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let v = (state >> 24) as u8;
        pixel[0] = v;
        pixel[1] = v.wrapping_mul(3);
        pixel[2] = v.wrapping_add(64);
        pixel[3] = 255;
    }
    rgba
}

/// A session across two monitors at different scales, so the per-monitor
/// conversion `--units auto` performs is actually exercised.
fn session(selections: usize) -> SessionFile {
    let monitors = vec![
        MonitorRecord {
            index: 0,
            name: "Built-in".into(),
            primary: true,
            origin_px: Point::new(0, 0),
            size_px: Size::new(3024, 1964),
            scale: 2.0,
        },
        MonitorRecord {
            index: 1,
            name: "External".into(),
            primary: false,
            origin_px: Point::new(3024, 0),
            size_px: Size::new(1920, 1080),
            scale: 1.0,
        },
    ];
    let selections = (0..selections)
        .map(|i| {
            let monitor = i % 2;
            let x = 40 + (i as i32 % 20) * 90;
            let y = 40 + (i as i32 / 20) * 70;
            let px = Shape::Rect(Rect::new(x, y, 80, 44));
            SelectionRecord {
                shape: ToolKind::Rect,
                label: format!("target-{i}"),
                monitor,
                px: px.clone(),
                global_px: px,
                rot_deg: None,
                window_px: None,
                crop: format!("crop-{i}.png"),
                color: None,
            }
        })
        .collect();
    SessionFile {
        schema: SCHEMA_VERSION,
        app: AppInfo {
            name: "pixelcoords".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        created_utc: "1970-01-01T00:00:00Z".into(),
        platform: None,
        capture: None,
        name: None,
        monitors,
        target: None,
        selections,
        measures: Vec::new(),
    }
}

fn main() {
    println!(
        "pixelcoords-core {} — median of {RUNS} runs",
        env!("CARGO_PKG_VERSION")
    );
    println!();
    println!(
        "| {:<28} | {:<24} | {:>12} |",
        "operation", "size", "median"
    );
    println!("|{:-<30}|{:-<26}|{:-<14}|", "", "", "");

    // --- resolve: the answer the README calls instant ----------------
    for count in [1usize, 40, 400] {
        let file = session(count);
        let no_drift = |_: usize| None;
        let elapsed =
            time(|| resolve::resolve(&file, None, Origin::Global, Resolved::Logical, &no_drift));
        row(
            "resolve (all labels)",
            &format!("{count} selections"),
            elapsed,
        );
    }
    let file = session(400);
    let no_drift = |_: usize| None;
    let elapsed = time(|| {
        resolve::resolve(
            &file,
            Some("target-399"),
            Origin::Global,
            Resolved::Logical,
            &no_drift,
        )
    });
    row("resolve (one label)", "400 selections", elapsed);

    // --- assert: one point, then the amortization --stdin claims -----
    let file = session(400);
    let point = Point::new(80, 60);
    let single = time(|| verdict::assess(&file, point, Origin::Global, None));
    row("assert (one point)", "400 selections", single);

    for stream in [100usize, 10_000] {
        let elapsed = time(|| {
            for i in 0..stream {
                let p = Point::new(60 + (i as i32 % 400), 50 + (i as i32 % 300));
                let _ = verdict::assess(&file, p, Origin::Global, None);
            }
        });
        row(
            "assert (streamed)",
            &format!("{stream} points"),
            elapsed / u32::try_from(stream).unwrap_or(1),
        );
    }

    // --- locate: normalized cross-correlation, the expensive one -----
    let frame_w = 3024;
    let frame_h = 1964;
    let frame_rgba = textured(frame_w, frame_h);
    let screen = GrayImage::from_rgba(frame_w, frame_h, &frame_rgba);
    for (cw, ch) in [(48usize, 24usize), (160, 90), (400, 300)] {
        let mut crop = vec![0u8; cw * ch * 4];
        for y in 0..ch {
            let src = ((y + 500) * frame_w + 700) * 4;
            crop[y * cw * 4..(y + 1) * cw * 4].copy_from_slice(&frame_rgba[src..src + cw * 4]);
        }
        let template = Template::from_rgba(cw, ch, &crop);
        let elapsed = time(|| locate::locate(&screen, &template, None));
        row(
            "locate (full-frame NCC)",
            &format!("{cw}x{ch} crop"),
            elapsed,
        );
    }

    // --- diff: per-region comparison at the same sizes ---------------
    for (cw, ch) in [(48usize, 24usize), (160, 90), (400, 300)] {
        let mut crop = vec![0u8; cw * ch * 4];
        for y in 0..ch {
            let src = ((y + 500) * frame_w + 700) * 4;
            crop[y * cw * 4..(y + 1) * cw * 4].copy_from_slice(&frame_rgba[src..src + cw * 4]);
        }
        let Ok(baseline) = Baseline::from_rgba(cw, ch, &crop) else {
            continue;
        };
        let at = Point::new(700, 500);
        let elapsed = time(|| diff::compare(&baseline, frame_w, frame_h, &frame_rgba, at));
        row("diff (one region)", &format!("{cw}x{ch} crop"), elapsed);
    }
}
