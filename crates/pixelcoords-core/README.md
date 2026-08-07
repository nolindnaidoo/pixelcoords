# pixelcoords-core

<p align="center">
  <a href="https://crates.io/crates/pixelcoords-core"><img src="https://img.shields.io/crates/v/pixelcoords-core.svg" alt="crates.io" /></a>
  <a href="https://docs.rs/pixelcoords-core"><img src="https://img.shields.io/docsrs/pixelcoords-core.svg" alt="docs.rs" /></a>
  <img src="https://img.shields.io/badge/rustc-1.88+-93450a.svg" alt="MSRV: Rust 1.88+" />
  <img src="https://img.shields.io/badge/unsafe-forbidden-success.svg" alt="forbid(unsafe_code)" />
  <a href="https://github.com/nolindnaidoo/pixelcoords/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
</p>

Every screen tool that measures pixels ends at a human's eyeball: a ruler
shows you a number, a screenshot app draws an arrow, a mouse tracker prints
a position you copy by hand. pixelcoords starts from a different premise —
**the real consumer of a coordinate is a machine.**

This is its platform-free core: the geometry, the `session.json` schema,
coordinate spaces and units, template relocation, point verdicts,
click-point resolution, region diffing, and click-code generation — with no
window system, no capture backend, and `#![forbid(unsafe_code)]`.

**Want the tool?** `cargo install pixelcoords`.
**Want to build on it?** This crate.

```toml
[dependencies]
pixelcoords-core = "0.7"
```

## Start here

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
#   "app": { "name": "pixelcoords", "version": "0.7.7" },
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

`Line` is the measure tool's primitive and deliberately not a `Shape` —
it marks a distance, not a region, so it has no interior to hit-test or
crop. It carries `length`, `delta`, `angle_deg` (clockwise from +X,
because screen Y grows downward), endpoint and segment hit-testing for
grabbing, and `constrained` for the 45° snap.

```rust
use pixelcoords_core::geometry::{Line, Point};

let gap = Line::new(Point::new(0, 0), Point::new(30, 40));

assert_eq!(gap.length(), 50.0);
assert_eq!(gap.delta(), (30, 40));
assert!((gap.angle_deg() - 53.13).abs() < 0.01);
```

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
#   "app": { "name": "pixelcoords", "version": "0.7.7" },
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
#   "app": { "name": "pixelcoords", "version": "0.7.7" },
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

## Modules

| Module | What it owns | Moves? |
|---|---|---|
| `session` | the `session.json` schema, parsing, validation | schema 1 since 0.1.0 — additive only |
| `geometry` | shapes, hit-testing, click points, rotation, bboxes | stable |
| `space` | monitor/global/window origins, logical ↔ physical | stable |
| `resolve` | the click point for a label, with drift applied | stable |
| `locate` | template relocation (NCC), scores, ambiguity | stable |
| `diff` | did this region change, and by how much | stable |
| `verdict` | did a point land inside, and what did it land in | stable |
| `wait` | poll budgets and conditions | stable |
| `emit` | click code for pyautogui, cliclick, xdotool, and more | grows with targets |
| `report` | one envelope and schema counter for every command | stable |
| `points`, `duration`, `config` | parsing and support | stable |

Full API, every item:
**[docs.rs/pixelcoords-core](https://docs.rs/pixelcoords-core)**

Some modules are `pub` and **not** part of this API — the overlay's
rasterizer, embedded font, string table, editing state, snapping, key
grammar and window matcher. They are public only because the binary is a
separate crate and can reach nothing else. They carry `#[doc(hidden)]`,
do not appear on docs.rs, and are not covered by this crate's versioning.

## Testing

| Layer | What it covers |
|---|---|
| Unit tests | every module — **90% line coverage floor per module**, enforced in CI |
| Property tests | the invariants: a clamped shape stays in bounds, rotation is periodic, a bbox bounds its shape |
| Doctests | every example in this file compiles and runs — the README *is* the crate docs |

Examples here are not illustrative. They are `include_str!`'d into the
crate and run on every push, so an example that stopped compiling would
fail CI rather than mislead you.

## Relationship to the CLI

`pixelcoords` the binary is this crate plus a window system: capture, the
overlay, permissions, and the subcommands. Anything that can be decided
without a screen lives here, which is why this crate has no platform code
and the binary has no geometry.

A session written by any pixelcoords build parses here, and the schema has
been version 1 since 0.1.0 — additive changes only, so old sessions keep
working.

## Also by nolindnaidoo

**Rust**

- **[pixelactions](https://github.com/nolindnaidoo/pixelactions)** - Perform the interaction and confirm it landed · [pixelactions.dev](https://pixelactions.dev)
- **[scrape-le](https://github.com/nolindnaidoo/scrape-le/tree/main/crate)** - Check whether a page is scrapeable before the scraper is written · [crates.io](https://crates.io/crates/scrape-le)

**VS Code Extensions** — every tool in the family, one page: **[letools.dev](https://letools.dev)**

- **[String-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.string-le)** - Extract string values for i18n from JSON, YAML, CSV, TOML, INI, and .env
- **[Numbers-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.numbers-le)** - Extract numeric values from JSON, YAML, CSV, TOML, INI, and .env
- **[EnvSync-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.envsync-le)** - Spot missing keys across your .env files, with a markdown report
- **[Paths-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.paths-le)** - Extract file paths from JS/TS imports, JSON, HTML, CSS, TOML, CSV, and .env
- **[Secrets-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.secrets-le)** - Detect and sanitize credentials locally, before you commit
- **[Scrape-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.scrape-le)** - Check whether a page is scrapeable before you write the scraper
- **[Colors-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.colors-le)** - Extract and analyze colors from CSS, SCSS, LESS, Stylus, HTML, JS/TS, and SVG
- **[URLs-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.urls-le)** - Extract URLs from documentation, configs, and code
- **[Regex-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.regex-le)** - Find, test, and validate the regex patterns in the current file
- **[Dates-LE](https://marketplace.visualstudio.com/items?itemName=nolindnaidoo.dates-le)** - Extract and analyze dates from logs, configs, and code

**Contact Developer** — [GitHub](https://github.com/nolindnaidoo) · [LinkedIn](https://www.linkedin.com/in/nolindnaidoo/)

## License

MIT — see [LICENSE](https://github.com/nolindnaidoo/pixelcoords/blob/main/LICENSE).
