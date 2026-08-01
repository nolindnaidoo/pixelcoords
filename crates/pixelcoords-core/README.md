# pixelcoords-core

The platform-free core of [pixelcoords](https://crates.io/crates/pixelcoords):
the geometry, the `session.json` schema, coordinate spaces and units,
template relocation, point verdicts, click-point resolution, region
diffing, and click-code generation — with no window system, no capture
backend, and `#![forbid(unsafe_code)]`.

**Want the tool?** Install the binary: `cargo install pixelcoords`.
**Want to build something on top of it?** That's this crate.

```toml
[dependencies]
pixelcoords-core = "0.4"
```

## What you'd use it for

A pixelcoords session is a human's answer to "where is this thing on
screen, exactly" — labeled regions, pixel-exact, with crops. This crate
is everything you need to *consume* that answer in Rust: read the
schema, resolve a label to the point you should click, ask whether a
region is still where it was, or check that a coordinate landed inside
it.

That covers most tools built here: test harnesses that assert on screen
positions, automation runners, screenshot annotators, converters between
DPI spaces, and alternative front-ends over the same session format.

**You may not need this crate at all.** If you only want to read
`session.json` from another language, the schema is plain JSON and fully
documented in
[docs/OUTPUT.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/OUTPUT.md).
Reach for this crate when you want the *behavior* too — the matching,
the scoring, the geometry — not just the data.

## The coordinate model, which you must get right

Everything else here is downstream of this, and it is the thing tools
get wrong.

- A session stores **physical pixels**, twice per selection: `px` is
  **monitor-local**, `global_px` is the same region on the **global
  desktop grid**.
- Each `MonitorRecord` carries `origin_px` (its position on that global
  grid) and `scale` (its DPI factor), so
  `global = monitor.origin_px + local`.
- **There is no universal logical space, and the schema does not pretend
  there is.** For logical points, divide by *the containing monitor's*
  scale — never a global one. Mixed-DPI desktops are the normal case.
- Input APIs disagree about which space they want: macOS `CGEvent` takes
  logical points; Windows `SendInput` and X11 `XTEST` take physical
  pixels. Converting to the wrong one clicks the wrong place without
  erroring.

## Read a session and resolve its click points

The core loop of nearly every consumer: for each labeled region, find
the point you would actually click.

```rust
use pixelcoords_core::session::SessionFile;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let session: SessionFile = serde_json::from_str(EXAMPLE_SESSION)?;

for selection in &session.selections {
    let monitor = session
        .monitors
        .iter()
        .find(|m| m.index == selection.monitor)
        .expect("a session describes every monitor it references");

    // `click_point` is an interior point of the shape — the centroid for
    // a rect, and a point guaranteed *inside* a triangle or freehand
    // polygon, where the centroid can fall outside the shape entirely.
    let local = selection.px.click_point();
    let global_x = monitor.origin_px.x + local.x;
    let global_y = monitor.origin_px.y + local.y;

    // Logical points, for an API like macOS CGEvent.
    let logical_x = f64::from(global_x) / monitor.scale;
    let logical_y = f64::from(global_y) / monitor.scale;

    println!(
        "{}: physical ({global_x}, {global_y}) · logical ({logical_x}, {logical_y})",
        selection.label
    );
}
# assert_eq!(session.selections.len(), 1);
# Ok(())
# }
#
# const EXAMPLE_SESSION: &str = r#"{
#   "schema": 1,
#   "app": { "name": "pixelcoords", "version": "0.4.0" },
#   "created_utc": "2026-07-29T00:00:00Z",
#   "monitors": [
#     { "index": 0, "name": "Built-in", "primary": true,
#       "origin_px": { "x": 0, "y": 0 },
#       "size_px": { "w": 3600, "h": 2338 }, "scale": 2.0 }
#   ],
#   "selections": [
#     { "shape": "rect", "label": "submit", "monitor": 0,
#       "px":        { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "global_px": { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "crop": "submit.png" }
#   ]
# }"#;
```

## Geometry

`Shape` covers every tool pixelcoords draws — `Rect`, `Circle`,
`Ellipse`, `Triangle`, and `Poly` (regular N-gons and freehand alike) —
with an untagged serde representation, so shapes round-trip through JSON
on their own.

```rust
use pixelcoords_core::geometry::{Point, Rect, Shape};

let button = Shape::Rect(Rect::new(800, 400, 100, 80));

assert_eq!(button.click_point(), Point::new(850, 440));
assert!(button.hit_test(Point::new(810, 410)));
assert!(!button.hit_test(Point::new(10, 10)));
assert_eq!(button.bbox(), Rect::new(800, 400, 100, 80));

// Rotation is metadata on rects and ellipses, so hit-testing takes the
// angle rather than mutating the shape.
assert!(button.hit_test_rotated(45, button.click_point()));
```

Also here: `regular_polygon` (N-gon construction from a center and a
point to aim at), `simplify_path` (Ramer–Douglas–Peucker, for freehand),
`rotate_point_about`, `normalize_deg`, and the clamped move/resize
helpers the overlay drives.

## Is this region still where it was?

`locate` is masked template matching — the engine behind `pixelcoords
find`. Give it a grayscale screen and a template cut from the saved crop,
and it reports where that region is *now*.

Two constants define the trust model, and they matter more than the
algorithm: `SCORE_FLOOR` (0.9) is the minimum normalized correlation to
count as found, and `AMBIGUITY_GAP` (0.03) is how far the best match must
beat the runner-up. **A template that matches in two places is reported
as ambiguous rather than picked between** — the caller is expected to
refuse, because acting on the wrong instance is worse than not acting.

`Template` carries an optional mask, so non-rectangular regions match on
the pixels the human actually marked instead of the bounding box.
`report()` assembles a `Report<FindResult>` — the same JSON `pixelcoords
find` prints, with per-label `Delta` values, so you can tell what moved
and by how much.

`report::Report<T>` is the envelope every scoring command shares:
`schema`, `command`, `captured_utc`, an aggregate `ok`, and the rows.
Row-level answers stay on the rows — `FindResult::found`,
`Verdict::hit` — because a caller usually needs to know *which* one
failed, not merely that one did.

## Did this point land in the right place?

`verdict::assess` answers that in whichever space your point already is
— `Origin::Global`, `Origin::Monitor(i)`, or `Origin::Window` for
`--target` sessions. An origin says where `(0, 0)` is; `space::Units`
answers the separate question of whether a coordinate is in device pixels
or logical points.

```rust
use pixelcoords_core::geometry::Point;
use pixelcoords_core::session::SessionFile;
use pixelcoords_core::space::Origin;
use pixelcoords_core::verdict::assess;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let session: SessionFile = serde_json::from_str(EXAMPLE_SESSION)?;
let verdict = assess(&session, Point::new(850, 440), Origin::Global, None)?;

assert!(verdict.hit);
assert_eq!(verdict.contained_in[0].label, "submit");

// A miss still reports what you nearly hit, and how far off you were.
let miss = assess(&session, Point::new(1600, 440), Origin::Global, None)?;
assert!(!miss.hit);
assert_eq!(miss.nearest.expect("a nearest region").region.label, "submit");
# Ok(())
# }
#
# const EXAMPLE_SESSION: &str = r#"{
#   "schema": 1,
#   "app": { "name": "pixelcoords", "version": "0.4.0" },
#   "created_utc": "2026-07-29T00:00:00Z",
#   "monitors": [
#     { "index": 0, "name": "Built-in", "primary": true,
#       "origin_px": { "x": 0, "y": 0 },
#       "size_px": { "w": 3600, "h": 2338 }, "scale": 2.0 }
#   ],
#   "selections": [
#     { "shape": "rect", "label": "submit", "monitor": 0,
#       "px":        { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "global_px": { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "crop": "submit.png" }
#   ]
# }"#;
```

`contained_in` lists **every** region containing the point, in stacking
order, so overlaps are disambiguated by the caller rather than silently
by the library.

## Generate click code

```rust
use pixelcoords_core::emit::{EmitFormat, Platform, emit};
use pixelcoords_core::session::SessionFile;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let session: SessionFile = serde_json::from_str(EXAMPLE_SESSION)?;
let snippet = emit(&session, EmitFormat::Pyautogui, Platform::MacOs, None)?;
assert!(snippet.contains("pyautogui"));
# Ok(())
# }
#
# const EXAMPLE_SESSION: &str = r#"{
#   "schema": 1,
#   "app": { "name": "pixelcoords", "version": "0.4.0" },
#   "created_utc": "2026-07-29T00:00:00Z",
#   "monitors": [
#     { "index": 0, "name": "Built-in", "primary": true,
#       "origin_px": { "x": 0, "y": 0 },
#       "size_px": { "w": 3600, "h": 2338 }, "scale": 2.0 }
#   ],
#   "selections": [
#     { "shape": "rect", "label": "submit", "monitor": 0,
#       "px":        { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "global_px": { "x": 800, "y": 400, "w": 100, "h": 80 },
#       "crop": "submit.png" }
#   ]
# }"#;
```

`Platform` exists because the same coordinate means different things per
tool: pyautogui makes its process DPI-aware on import, so it addresses
physical pixels on Windows but logical points on macOS. `cliclick` and
`xdotool` each exist on one platform and don't branch.

## The modules, and how much they move

| Module | What it is | Reuse |
|---|---|---|
| `session` | the `session.json` schema | **the contract** — versioned, grows additively |
| `geometry` | shapes, hit-testing, rotation, polygon math | stable, pure arithmetic |
| `space` | coordinate origins and units, and the conversion between them | stable shape |
| `report` | the `Report<T>` envelope every command prints | **the CLI contract** — versioned |
| `locate` | masked template matching, scoring, fixed-location `TemplateStats` | stable shape, tuned thresholds |
| `verdict` | point-in-region assessment | stable shape |
| `resolve` | click points per selection, in a chosen space and units | stable shape |
| `diff` | per-region masked pixel comparison | stable shape |
| `wait` | poll budgets and the match/change conditions | stable shape |
| `emit` | pyautogui / cliclick / xdotool generators | stable shape |
| `points`, `duration` | the `X,Y[,label]` stream grammar, `30s`/`500ms` durations | strict parsers, stable |
| `selection` | the undo/redo edit engine | overlay internals |
| `draw`, `font` | CPU rasterizer, embedded JetBrains Mono | overlay internals |
| `hotkeys`, `config`, `strings`, `matcher` | binding grammar, strict TOML config, UI text, window-title matching | overlay internals |

**Stability, honestly.** This stays on 0.x and shares a version with the
binary, so a minor bump can change any signature here. **Two things carry
a real compatibility promise**, and they are versioned separately because
they version different things: the session schema on disk
(`session::SCHEMA_VERSION`, still 1) and the documents the commands print
(`report::CLI_SCHEMA_VERSION`, now 2). Additions to either are optional
fields, and consumers that ignore unknown keys keep working. The top rows
are what tools are actually built on; the overlay internals exist to serve
the binary and move with it. Pin a caret range and read the
[CHANGELOG](https://github.com/nolindnaidoo/pixelcoords/blob/main/CHANGELOG.md)
before upgrading.

Every example above is compiled and run as a doctest in CI, so nothing
on this page can rot silently.

## See also

- [pixelcoords](https://crates.io/crates/pixelcoords) — the tool this serves
- [pixelactions](https://github.com/nolindnaidoo/pixelactions) — the executor half: consumes these sessions and performs the interactions
- [API docs](https://docs.rs/pixelcoords-core) · [repository](https://github.com/nolindnaidoo/pixelcoords) · [pixelcoords.dev](https://pixelcoords.dev)
- [docs/OUTPUT.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/OUTPUT.md) — every JSON shape this crate produces

MIT licensed. Bundled font: JetBrains Mono, OFL 1.1.
