# Developing pixelcoords

Everything about building, testing, and shipping this repository.
[AGENTS.md](../AGENTS.md) is the engineering-standards document — layout
rules, control-flow style, coordinate conventions, the testing bar; this
file is the practical companion. On any conflict, AGENTS.md wins.

## Build from source

```bash
git clone https://github.com/nolindnaidoo/pixelcoords
cd pixelcoords
cargo build --workspace
```

Rust 1.88+ (the enforced MSRV). Linux needs build dependencies first:

```bash
# Debian/Ubuntu
sudo apt-get install -y libxcb1-dev libxcb-randr0-dev libpipewire-0.3-dev \
  libclang-dev libegl1-mesa-dev libgbm-dev pkg-config
# Fedora
sudo dnf install libxcb-devel pipewire-devel clang-devel \
  mesa-libEGL-devel mesa-libgbm-devel pkgconf
```

macOS and Windows need nothing beyond a Rust toolchain. Run the debug
build with `cargo run -p pixelcoords`.

## Workspace layout

Two crates, one boundary:

- **`crates/pixelcoords-core`** — pure logic, zero platform
  dependencies, `#![forbid(unsafe_code)]`. Modules: `geometry` (shapes,
  hit-testing, rotation, polygon math), `selection` (the undo/redo edit
  engine), `session` (the session.json schema), `verdict` (`assert`
  scoring), `emit` (click-code generators), `locate` (`find`'s masked
  template matching), `draw` (CPU rasterizer and masks), `font`
  (embedded JetBrains Mono via fontdue), `strings` (user-facing overlay
  text), `hotkeys` (binding grammar), `matcher` (window-title
  matching), `config` (TOML parsing, strict).
- **`crates/pixelcoords`** — the binary: winit event loop (`app`),
  softbuffer presentation (`render`, `view`), xcap capture (`capture`),
  CLI (`cli`, `main`), file output (`save`), UI-state persistence
  (`state`), and cfg-gated platform modules (`mac`, `win`, `linux`).

The rule that keeps the boundary honest: if a platform type appears in
core, that is a bug. New logic goes in core when it can (where it must
be unit-tested), in the binary only when it needs the window system or
OS APIs.

## The checks

Run exactly what CI runs before every push:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI (`.github/workflows/ci.yml`) runs seven required jobs on every PR —
all must pass before anything merges to `main`:

| Job | What it enforces |
|-----|------------------|
| `test` (macOS, Windows, Ubuntu) | fmt, clippy pedantic `-D warnings`, tests, build — per OS |
| `msrv` | the workspace builds on Rust 1.88 |
| `policy` | no inline `#[allow(...)]` anywhere (workspace-level relaxations only) |
| `coverage` | 90% line coverage floor **per module** in core |
| `audit` | `cargo audit`; exceptions live in `.cargo/audit.toml`, each with written reasoning |

## Testing

- Core is pure; everything in it is unit-tested, and invariants
  (clamping stays in bounds, rotation is periodic) carry property tests
  in `crates/pixelcoords-core/tests/` with committed
  `proptest-regressions/`.
- Binary logic that can run headless does: the App state machine runs
  windowless (`test_app()` in `app.rs`), capture-dependent code runs
  against fake `CaptureProvider`s (see the mixed-DPI fake in
  `main.rs`). Follow those patterns; do not mock the window system.
- Overlay behavior (windows, capture, permissions) is verified by
  manual runs on real hardware per platform — never claim visual
  behavior works without one.
- Every bug fix ships with a regression test that fails before the fix.

Measuring coverage like CI does:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov -p pixelcoords-core --summary-only
```

Scoped to core, as the floor is — measuring `--workspace` folds in
window-system plumbing the floor was never meant to cover. `--html`
writes a browsable report to `target/llvm-cov/html`; CI uploads the
same report as the `coverage-report` artifact on every run.

## Platform coordinate work

Capture backends disagree about units (macOS logical, Windows/X11
physical); `CoordSpace` in `capture.rs` states the convention once and
every conversion routes through it. Never write platform coordinate
math from assumption: the "Platform spike" workflow (Actions tab,
manual trigger) produces real coordinate tables from Windows and X11
runners to verify against. The full conventions live in AGENTS.md's
Coordinates section.

## Releases

Versioning policy (also stated in [CHANGELOG.md](../CHANGELOG.md)):
pre-1.0, **minor** for features and any CLI/schema break, **patch** for
fixes; 1.0.0 when schema and CLI are declared stable.

- Features land through issues → PRs (`Closes #N`), each writing its
  CHANGELOG entry under the upcoming version's heading in the same PR.
  The version in `Cargo.toml` does not move in feature PRs.
- A GitHub **milestone** per upcoming minor collects its issues; the
  milestone emptying is the release trigger.
- The release cut: one PR bumps the workspace version and the core dep
  pin. Then tag `v<X.Y.Z>` — the tag triggers
  `.github/workflows/release.yml`, which builds release binaries for
  macOS (arm64 + x86_64), Windows, and Linux, and opens a **draft**
  GitHub release with the archives attached.
- crates.io publish order matters: `cargo publish -p pixelcoords-core`
  first, then `-p pixelcoords` (the binary's dep pin must resolve).
  Publishes are manual and deliberate; nothing in CI publishes.

## Repository governance

- `main` accepts pull requests only — a branch ruleset blocks direct
  pushes, force pushes, and deletion, and requires all seven CI jobs
  green to merge. Anyone can open a PR; only the maintainer merges.
- Dependabot checks weekly (cargo + GitHub Actions, grouped PRs) and
  security advisories immediately. Patch/minor updates auto-merge once
  every required check passes; majors wait for human review. Actions
  are pinned to commit SHAs; Dependabot maintains the pins.
- Issue forms require the diagnostics a report needs (`doctor` output,
  OS, version); the feature form points at the README's Non-goals.
