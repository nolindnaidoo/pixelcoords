# pixelcoords — engineering standards

This is the source of truth for how code in this repository is written,
tested, and reviewed. It applies to every contributor, human or
AI-assisted. CI enforces the mechanical parts; reviewers enforce the rest.
If a pull request follows this document, review is fast.

## What this project is

Cross-platform screen coordinate picker. v1 is **snapshot mode**: capture a
frozen screenshot per monitor → fullscreen overlay showing it → draw
rectangle, ellipse, triangle, polygon, and freehand selections (move,
resize, rotate, label) plus measure rulers → save JSON coordinates + PNG
crops. This model is the only one that works on
Wayland, so it is the base for every platform. "Live mode" (a transparent
overlay over a live app) is a possible later addition for Windows/macOS/X11
— do not build toward it prematurely.

## Layout

- `crates/pixelcoords-core` — pure logic: geometry, selections, session
  schema, coordinate spaces and units, the shared report envelope, point
  verdicts, click-point resolution, region diffing, wait conditions,
  template relocation, code emitters, the point-stream and duration
  parsers, hotkey grammar, config, strings table, embedded vector font
  (fontdue), CPU rasterizer. **Zero
  platform deps, `#![forbid(unsafe_code)]`, everything unit-tested.** If a
  platform type (winit, xcap, Win32) appears in this crate, that is a bug.
- `crates/pixelcoords` — the binary: winit event loop, softbuffer
  presentation, xcap capture, CLI, file output. Platform-specific code
  lives in cfg-gated modules (`mac.rs`, …); everything else is
  platform-neutral.

Keep modules flat. Do not introduce layers, registries, managers, or
services. Do not add a trait with a single implementation — the binary has
exactly one trait (`CaptureProvider`), which exists for its test double.

## Control-flow style

Flat over nested, guards over branches:

- **No statement-position `else`.** Use guard clauses and early `return`
  (`if !ok { return ... }` / `let Some(x) = ... else { return }`), then
  fall through to the happy path. Ranked policies read as a list of early
  returns, not an else-if ladder.
- **Value-position `if/else` is fine** — `let x = if cond { a } else { b }`
  is Rust's ternary; replacing it would force `let mut` + reassignment.
- **`match` is fine and preferred** over any chain of condition tests on
  the same value — it is exhaustiveness-checked pattern dispatch, not an
  else-chain. Use match guards (`pattern if cond =>`) instead of `if/else`
  inside arms.
- Prefer combinators where they read cleanly: `bool::then_some`,
  `Option::map/filter/is_some_and`, `?`.
- No nesting deeper than two levels inside a function; extract a named
  helper instead.

## Hard rules

- **No inline `#[allow(...)]`** — CI greps and fails the build. Either fix
  the lint or add a visible, commented relaxation to `[workspace.lints]`
  in the root `Cargo.toml`.
- **Clippy pedantic, deny warnings.** `cargo clippy --workspace
  --all-targets -- -D warnings` must pass exactly as CI runs it.
- **No async runtime.** The winit event loop is the loop
  (`ControlFlow::Wait`; `WaitUntil` only for timed UI like the caret
  blink). Do not add tokio, async-std, or executors.
- **Dependencies are a cost.** Justify every new one in the PR
  description. Prefer the standard library; prefer what is already in the
  tree.
- **`unsafe` is forbidden in core** and allowed in the binary only inside
  platform modules for OS API calls.
- **Strict parsing, never silent defaults.** Bad config values, unknown
  fields, and malformed input are errors with actionable messages — not
  fallbacks. (`serde(deny_unknown_fields)` on config types is deliberate.)
- **User-visible strings** route through `core::strings` so localization
  can land later without a refactor. The embedded JetBrains Mono covers
  Latin, Cyrillic, and Greek; text outside that coverage will not render.

## Coordinates (read before touching geometry, capture, or save)

- The authoritative space is **monitor-local physical pixels** — the
  captured image's own grid. All drawing, hit-testing, clamping, and
  cropping happen there.
- `CoordMap` in `view.rs` is the only place a window↔capture scale factor
  may appear.
- Global coordinates are derived: `global_px = monitor origin_px + local
  px`. Window-relative coordinates likewise derive from the target
  record.
- Capture backends disagree about units: macOS reports logical points;
  Windows and X11 report physical pixels. `CoordSpace` /
  `NATIVE_COORD_SPACE` in `capture.rs` state this once — route every
  native→physical conversion through it and never write `origin * scale`
  anywhere else.
- Picked-window sessions (`--pick`) are their own coordinate space: the
  portal returns pixels but no position or scale, so the frame is a
  pseudo-monitor at origin (0, 0), scale 1.0, and `px` == `global_px` ==
  `window_px`. Do not invent desktop coordinates for them.
- A pick is presented `Presentation::Windowed` — an overlay sized to the
  capture, not fullscreen — because a picked window shares neither size
  nor shape with the display. When window and capture still disagree,
  `CoordMap::fit` letterboxes: one scale factor for both axes, centered,
  black margins. Never scale the axes independently; it distorts what the
  user is marking even though the inverse map keeps the coordinates right.
- **Windows must be per-monitor v2 DPI-aware before the first xcap call**
  (`win.rs`, called from `main`). xcap branches on process awareness: when
  unaware it refuses `GetDpiForMonitor` and derives the scale factor from a
  virtualized `DESKTOPHORZRES / HORZRES` ratio instead. winit sets awareness
  at event-loop creation — after capture, and never for the subcommands — so
  it cannot be relied on. Monitor origins and sizes come from `DEVMODE` and
  window bounds from DWM, which are true physical either way; only the scale
  factor is affected.
- **Never write platform coordinate math from assumption.** The "Platform
  spike" workflow (Actions tab, manual trigger) produces real coordinate
  tables from Windows and X11 runners; verify against those or against
  the platform's source before coding.

## Captures exclude the mouse pointer

A capture must contain screen *content* and nothing else. The pointer is
drawn on top of the screen by the window server; it is not content, and
treating it as content breaks both halves of this tool:

- a crop saved while the pointer sat over the region has a pointer baked
  into it, and then only matches when a pointer is in the same place;
- `find` re-locates against a fresh capture, and whatever just moved the
  pointer has usually left it on the very region being re-located.

Measured on real hardware: the pointer costs about **0.17 of match score**
on a low-detail region — enough to push a perfect match under the 0.9
floor — and nothing at all on a busy one. That content-dependence is why
it presents as flakiness instead of a bug, and why it must be fixed at the
capture layer rather than compensated for downstream.

Windows and Linux capture without it already. macOS goes through
`mac::capture_display` (`CGDisplayCreateImage`) rather than the capture
crate's `CGWindowListCreateImage` path, which composites the pointer in.
**Do not route macOS monitor capture back through the crate's own
`capture_image()`.**

## Data and schema

- `session.json` carries `"schema": 1`. New fields are optional and
  skipped when absent (`skip_serializing_if`); a breaking change bumps the
  version. Rotation is stored as `rot_deg` metadata for rects and ellipses but
  baked into vertices for triangles and polys — keep stored geometry
  exact for consumers.
- Saves must never destroy user data: only files this session wrote may
  be cleaned up, deletions happen only after the new save is fully on
  disk, and foreign files in the output directory are untouchable.
- Every user-visible mutation is undoable and recorded exactly once;
  no-op edits (unchanged label, zero-distance move) must not pollute the
  undo stack or mark the session dirty.

## Testing

The bar, enforced by review:

- **`pixelcoords-core`: 90% line coverage floor per module.** Everything in
  core is pure; if something is hard to test there, the design is wrong.
  CI enforces this per module rather than on the crate total, because a
  total lets one module slide while the others carry it.
- **Invariants get property tests.** Example-based tests check the cases
  someone thought of; the rules that hold for every input — a clamped
  shape stays in bounds, rotation is periodic, a bbox bounds its shape —
  live in `crates/pixelcoords-core/tests/`. Add one whenever a change
  turns on an invariant rather than a case.
- **Headless-testable binary logic is tested headless.** The App state
  machine runs windowless (`test_app()` in `app.rs` — zero views, redraws
  are no-ops); capture-dependent logic runs against fake
  `CaptureProvider` implementations (see the mixed-DPI fake in
  `main.rs`). Follow those patterns for new logic.
- **Every bug fix ships with a regression test** that fails before the
  fix.
- **Do not mock the window system.** Window creation, present, event
  routing, real capture, and permission externs are verified by manual
  gates on real hardware per platform, not by unit tests. Roughly half
  the binary's lines are this plumbing; chasing coverage there produces
  fake confidence.

Measuring coverage locally:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov -p pixelcoords-core --summary-only
```

Scoped to the core crate, as the floor is. Measuring `--workspace` folds
in the binary, which is largely window-system plumbing that is verified on
hardware rather than by unit tests, so the number comes out low against a
floor that was never meant to cover it. `--html` instead of
`--summary-only` writes a browsable report to `target/llvm-cov/html`; CI
uploads that same report as the `coverage-report` artifact on every run.

Tests that touch the filesystem use unique paths under the system temp dir
and clean up after themselves. Tests must be deterministic — no clocks, no
randomness, no network (the tool itself has no network).

The property tests are the one exception, and only in how they choose
inputs: generation is randomized so they explore beyond hand-picked cases.
A failure is therefore reproducible rather than repeatable — proptest
shrinks it to a minimal counterexample and records it under
`proptest-regressions/`, which is committed so the case is replayed
forever after. Assertions themselves stay clock-free and network-free.

## Verification — the definition of done

All three, exactly as CI runs them, before every push:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI additionally builds on macOS, Windows, and Linux, checks the Rust 1.88
minimum version, enforces the per-module coverage floor, and runs
`cargo audit`. Advisory exceptions live in `.cargo/audit.toml`, each with
the reasoning written down — the same rule as `[workspace.lints]`. A change is not done because it compiles; it is done
when it is tested, linted, documented where behavior changed (README /
CHANGELOG / this file), and honest — claims in docs must match the code.

## Commits and pull requests

- Imperative subject line; body explains the *why* and the user-visible
  consequences, not a list of files touched.
- One concern per PR. Refactors and behavior changes travel separately.
- If docs describe the thing you changed, update them in the same PR —
  README, CHANGELOG, `config.example.toml`, and this file are part of the
  code.
