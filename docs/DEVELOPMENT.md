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

Before every publish, walk this list — the ones with easy misses first:

1. **Update the install snippet in `crates/pixelcoords-core/README.md`.**
   A pre-1.0 caret pin like `= "0.1"` resolves to the newest 0.1.x, not
   0.2.x — so a reader copy-pasting from crates.io lands on the old API.
   Bump the string to the current minor before every minor cut.
2. Update the workspace version and the core dep pin in
   `crates/pixelcoords/Cargo.toml`.
3. Write the CHANGELOG entry.
4. Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
   -- -D warnings`, and `cargo test --workspace`.
5. Dry-run: `cargo publish -p pixelcoords-core --dry-run`.


Versioning policy (also stated in [CHANGELOG.md](../CHANGELOG.md)):
**minor** (0.x.0) for features and any CLI/schema break, **patch**
(0.x.y) for fixes. There is no 1.0 on the plan — the version keeps
incrementing through 0.x, and the stability a release offers is whatever
its CHANGELOG entry says it offers.

### How releases are grouped

By **what a release does to the contract**, not by theme, and hardest
first:

- **The session schema does not move.** It has been version 1 since
  0.1.0, and every planned feature is additive by construction — a new
  optional field or a new optional top-level array, the same pattern
  `platform`, `capture`, and `name` established. Old sessions keep
  loading; old consumers ignore what they do not know.
- **The CLI was the part that still moved**, so it went first. 0.4.0 was
  the agent surface: a design pass settled the vocabulary the three new
  commands (`resolve`, `wait`, `diff`) share — written down in
  [The agent-surface contract](#the-agent-surface-contract) below — and
  they landed on it, along with `assert --stdin` and the two breaks that
  had to happen before anything was built against them. Grouping them
  together was the point: three commands answering the same question
  should not disagree about timeouts, `--label`, or exit codes, and
  fixing that cost least before any of them existed. **That contract is
  now the thing later releases are held to**, not a plan.
- **0.5.0 is reach**: new emit targets and external image input. Both
  add places the toolchain can point without changing what it already
  says to callers.
- **0.6.0 is overlay and marking polish** — color readout, the measure
  tool, edge snapping, localization. It touches neither the schema nor
  the CLI, so it can ship in any release; it is last because everything
  ahead of it is something other tools depend on.

A minor bump is cheap here, so nothing is held back for a number. What
is *not* cheap is changing an answer callers already script against —
which is why the shared-vocabulary work is grouped ahead of the commands
that would inherit its mistakes.

- Features land through issues → PRs (`Closes #N`), each writing its
  CHANGELOG entry under the upcoming version's heading in the same PR.
  The version in `Cargo.toml` does not move in feature PRs.
- A GitHub **milestone** per upcoming minor collects its issues; the
  milestone emptying is the release trigger.
- **The milestone is the only place an issue's target version lives.** An
  issue body must never name one — not in its acceptance list, not in its
  docs checklist, which says "the upcoming version's heading" and stops
  there. A version written into prose is a copy of the milestone that
  nothing keeps in sync: it survives re-planning, and it survives the
  release shipping without it. Ten of eleven open issues carried such a
  copy once, one of them naming a version that had already shipped. Moving
  an issue between releases must stay a one-click act.
- The release cut: one PR bumps the workspace version and the core dep
  pin. Then tag `v<X.Y.Z>` — the tag triggers
  `.github/workflows/release.yml`, which builds release binaries for
  macOS (arm64 + x86_64), Windows, and Linux, and opens a **draft**
  GitHub release with the archives attached.
- crates.io publish order matters: `cargo publish -p pixelcoords-core`
  first, then `-p pixelcoords` (the binary's dep pin must resolve).
  Publishes are manual and deliberate; nothing in CI publishes.

## The agent-surface contract

`assert`, `find`, `resolve`, `wait`, and `diff` all answer one class of
question — *what is true on screen right now?* — over the same
primitives. This is the vocabulary they share. It is written here rather
than in [CLI.md](CLI.md) on purpose: CLI.md describes the surface that
exists, and a contract is agreed before the code that honors it. New
commands in this family are reviewed against this section.

### `--label` restricts the set. `--expect` is a success criterion

`--label` selects **which regions a command operates on**, everywhere it
appears. An unknown label is exit 2, listing the labels the session
carries.

`assert` is the one command asking about a *point* rather than a set, so
"which label counts as a hit" is a different question and gets a
different word: **`--expect`**. That split is what lets a miss keep
reporting every region the point *did* land in — the thing that makes
`assert` useful for scoring a trajectory rather than just failing it.

`assert --label` used to mean what `--expect` means now. For one release
it is refused outright, naming the new flag, rather than quietly
switching to the set meaning: a rename a script notices is cheap, and a
silent change of meaning is not. It comes back as an ordinary set filter
one release later.

### `--space` is an origin. `--units` is a scale. Neither is universal

- `--space` — `global | monitor | window`. Which origin a coordinate is
  measured from.
- `--units` — `physical | logical | auto`. `auto` is what the platform's
  input APIs expect: logical points on macOS, physical pixels on Windows
  and X11.

Orthogonal as vocabulary; that does not mean every command carries both:

| command | `--space` | `--units` | why |
|---------|-----------|-----------|-----|
| `resolve` | yes | yes | the only command emitting a point to act on |
| `assert` | yes | no | its point is input, and stays physical |
| `emit` | no | no | each format *defines* its units |
| `find` | no | no | reports session-space shapes |
| `wait` | no | no | reports scores |
| `diff` | no | no | reports pixel counts; its bbox is provenance |

`emit` is worth stating outright: `cliclick` is logical, `xdotool` is
physical, and pyautogui branches on platform. A `--units` override there
would let a caller generate code that is provably wrong for the tool it
targets — the exact failure this project exists to prevent.

`--units` describes output only. A logical *input* point at scale 2.0
covers a 2×2 physical block, and no rounding rule makes "hit" or "miss"
the right answer, so `assert --point` is physical and says so.

### One envelope, one schema counter

Every machine-readable document in this family is:

```json
{ "schema": 2, "command": "find", "captured_utc": "…", "ok": true, "results": [] }
```

`captured_utc` is absent when the command does not capture. `ok` is the
**aggregate** — and row-level answers stay on the rows: `Verdict.hit` and
`FindResult.found` do not move up, because `assert --stdin` returns one
verdict per input line and a per-line answer is the whole point.

One counter (`report::CLI_SCHEMA_VERSION`) covers all of them. It starts
at 2 because the two counters it replaced were both at 1 and these
documents do not have that shape. `doctor` and `windows` keep their own
`json!` documents; they answer a different kind of question and are not
part of this surface.

### Exit codes: 0, 1, 2

**0** success, hit, found, condition met. **1** a negative answer — miss,
not found, over tolerance, timed out. **2** the question was malformed.

A `wait` timeout is **1**. It is a negative answer, not a broken
question, and a script must be able to tell those apart without parsing
stderr.

### Durations: one integer, one unit

`500ms`, `30s`, `2m`. No bare numbers, no decimals, no compounds
(`1m30s`), no uppercase — strict parsing per
[AGENTS.md](../AGENTS.md), so a bad duration names the accepted
spellings instead of silently defaulting. `0s` parses; whether a
command accepts zero is that command's rule.

### `--min-score` and `--tolerance` are not synonyms

- **`--min-score`** — a normalized correlation score in `0..=1`. Used by
  `wait`; `find`'s `SCORE_FLOOR` (0.9) is the same quantity.
- **`--tolerance`** — a percentage of masked pixels allowed to differ.
  Used by `diff`, default 0.

Different units, different directions, different defaults. They must
never share a word.

One consequence worth knowing before choosing between them: NCC is
brightness- and contrast-normalized, so a region that changes *uniformly*
— a dimming backdrop, auto-brightness, a luminance-only theme switch —
still scores ~1.0. `wait --for change` will not fire on it. A caller who
means "any pixel differs" wants `diff --tolerance 0`, which compares RGB
directly.

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
