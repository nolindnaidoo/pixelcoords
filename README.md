<h1 align="center">pixelcoords</h1>

<p align="center">
  <b>Freeze your screen, mark regions, get pixel-exact coordinates and crops</b><br/>
  <i>Rectangles, ellipses, triangles, N-gons, freehand, rulers — rotate, label, verify, regenerate</i>
</p>

<p align="center">
  <a href="https://github.com/nolindnaidoo/pixelcoords/actions/workflows/ci.yml">
    <img src="https://github.com/nolindnaidoo/pixelcoords/actions/workflows/ci.yml/badge.svg" alt="Build Status" />
  </a>
  <a href="https://docs.rs/pixelcoords-core">
    <img src="https://img.shields.io/docsrs/pixelcoords-core.svg" alt="docs.rs" />
  </a>
  <a href="https://crates.io/crates/pixelcoords">
    <img src="https://img.shields.io/crates/v/pixelcoords.svg" alt="crates.io" />
  </a>
  <img src="https://img.shields.io/badge/rustc-1.88+-93450a.svg" alt="MSRV: Rust 1.88+" />
  <a href="https://github.com/nolindnaidoo/pixelcoords/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" />
  </a>
  <a href="https://pixelcoords.dev">
    <img src="https://img.shields.io/badge/web-pixelcoords.dev-00A0FF.svg" alt="pixelcoords.dev" />
  </a>
</p>

<p align="center">
  <img src="https://github.com/nolindnaidoo/pixelcoords/raw/main/docs/assets/demo.gif" alt="pixelcoords demo: freeze a window, mark shapes, save machine-readable coordinates" style="max-width: 100%; height: auto;" />
</p>

> **Useful?** A star is how other developers find it —
> [★ GitHub](https://github.com/nolindnaidoo/pixelcoords) ·
> [pixelcoords.dev](https://pixelcoords.dev)

Every screen tool that measures pixels ends at a human's eyeball: a ruler
shows you a number, a screenshot app draws an arrow, a mouse tracker prints
a position you copy by hand. pixelcoords starts from a different premise —
**the real consumer of a coordinate is a machine.**

Freeze the screen so nothing moves while you measure, mark regions with real
shapes, and what you marked becomes **data, not a picture**: versioned JSON
in physical pixels with per-monitor DPI scale, labeled crops, and
ready-to-paste click code for your automation stack.

## Install

| Route | Command | Worth knowing |
|---|---|---|
| **Homebrew** | `brew tap nolindnaidoo/tap`<br>`brew install pixelcoords` | **macOS only.** The formula has no Linux build — the Linux binary needs a capture stack (`libxcb`, `libpipewire`, `libegl`, `libgbm`) wired to system paths Homebrew does not own. Tap once; after that it installs like any formula. |
| **winget** | `winget install nolindnaidoo.pixelcoords` | Windows 10+. A **portable** install: winget unpacks the exe and registers a PATH alias, so there is nothing in Add/Remove Programs. |
| **cargo** | `cargo install pixelcoords` | Any platform, needs **Rust 1.88+**. It compiles the capture stack from source, so expect minutes rather than seconds. On Linux install the build dependencies below first. |
| **Prebuilt binary** | [releases page](https://github.com/nolindnaidoo/pixelcoords/releases) | macOS (arm64 + x86_64), Windows, Linux. No toolchain needed — download, unpack, run. **No auto-update**: you come back here for the next version. Each release ships checksums. |

Building on Linux needs the capture stack first:

```bash
sudo apt-get install -y libxcb1-dev libxcb-randr0-dev libpipewire-0.3-dev \
  libclang-dev libegl1-mesa-dev libgbm-dev pkg-config
```

macOS asks for Screen Recording permission on first run.

## Sixty seconds

```bash
pixelcoords                        # freeze every screen; drag to mark, A labels, S saves
pixelcoords resolve --session ~/Downloads/pixelcoords-captures/<stamp>
```

`resolve` prints the click point for every label you marked, in the space
and units your automation API speaks:

```json
{ "label": "submit", "monitor": 0, "scale": 2.0, "units": "logical",
  "point": { "x": 812, "y": 440 } }
```

That is the whole loop: **mark once, resolve forever.** The session is a
directory of JSON and PNGs you can commit.

## Commands

| Command | What it does |
|---|---|
| `pixelcoords` | Freeze every screen and open the marking overlay |
| `resolve` | Where to click for each label, in your API's space and units |
| `find` | Re-locate regions in a fresh capture — for when the UI moved |
| `assert` | Did this point land in the right region? One point or a stream |
| `diff` | Do the regions still look like they did? |
| `wait` | Block until regions match again, or until one stops matching |
| `emit` | Ready-to-paste click code for pyautogui, cliclick, xdotool, and more |
| `shoot` | Plain scripted screenshot, no overlay, same DPI handling |
| `resume` | Reopen a saved session and keep editing |
| `rename` | Give a session a friendly name for the resume picker |
| `windows` | List visible windows, for `--target` |
| `doctor` | Check permissions, config, and the monitor setup |
| `mcp` | Serve the agent surface over MCP on stdio |

Full reference with every flag:
**[docs/CLI.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/CLI.md)**

## Things worth knowing early

**Coordinates are physical pixels, and `--units` is the knob.** A session
records what the display actually has. `--units auto` converts to what your
platform's input API expects — logical points on macOS, physical pixels on
Windows and X11. Guessing wrong on a Retina display puts every click at
half the intended position.

**Exit codes are the API.** Scripts branch on them:

| Code | Meaning |
|---|---|
| 0 | yes — resolved, matched, hit |
| 1 | a real answer, and it is no — not found, missed, timed out |
| 2 | the question was malformed — bad label, unreadable session |

**Wayland withholds window geometry.** `windows` and `--target` refuse
there and point you at `--pick` instead. That refusal is deliberate; a
guessed window position would be worse than no answer.

**`--min-score` and `--tolerance` are different quantities.** The first is
a correlation score in `0..=1`; the second is a percentage of pixels in
`0..=100`. Both are bounds-checked, so a swapped value is refused rather
than silently matching nothing.

## Configure it

Colors, key bindings, snapping, and overlay behavior live in a config file
you can also override per run:

```bash
pixelcoords --bind u=undo --config ./my-config.toml
```

Every setting and every bindable action:
**[docs/CONFIGURATION.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/CONFIGURATION.md)**

## Platform support

| Platform | Capture | Window targeting | Verified by hand |
|---|---|---|---|
| macOS | Yes | `--target` | overlay through 0.5.1; multi-monitor + mixed-DPI on real hardware |
| Windows 11 | Yes | `--target` | through 0.4.0; multi-monitor test-only |
| Linux (X11) | Yes | `--target` | through 0.4.0; multi-monitor test-only |
| Linux (Wayland) | Yes, via portal | `--pick` only | through 0.4.0; `windows`/`--target` refuse by design |

Fractional scaling is unverified everywhere. Claims here match runs — where
a run has not happened it says so, and
[CONTRIBUTING.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/CONTRIBUTING.md)
carries the full record.

## Performance

Measured on an Apple M5 Pro, `--release`, at 0.5.3. Reproduce with:

```bash
cargo run --release -p pixelcoords-core --example bench   # the math
scripts/bench-cli.sh <session-dir>                        # the round trip
```


| Operation | Size | Median |
|---|---|---|
| `resolve`, all labels | 400 selections | 14.3 µs |
| `assert`, one point | 400 selections | 2.0 µs |
| `diff`, one region | 160×90 crop | 20.6 µs |
| `find` (full-frame NCC) | 160×90 crop | 198 ms |
| `find` (full-frame NCC) | 400×300 crop | 1.40 s |

Resolving is free; **matching is the expensive part**, and it scales with
crop area rather than screen size. Method and caveats:
[docs/PERFORMANCE.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/PERFORMANCE.md)

## Testing

| Layer | What it covers |
|---|---|
| Unit + property tests | `pixelcoords-core`, **90% line coverage floor per module** |
| Contract tests | every exit code, every command — no display needed |
| Scenario tests | the binary driven against a **real display**, on macOS, Windows and Linux every push |
| Manual gates | the overlay itself — tests cannot speak for it, so it is verified by hand and said so plainly |

659 tests. CI runs fmt, clippy pedantic (`-D warnings`), the suite, MSRV,
`cargo audit`, and a policy job that fails on any inline `#[allow]`.

## Non-goals

Not a screen recorder, not an OCR tool, not a UI-testing framework, and
**not an executor** — pixelcoords never moves your mouse. It answers
*where*, and stops. [pixelactions](https://github.com/nolindnaidoo/pixelactions)
is the other half of that loop.

## Documentation

- **[pixelcoords.dev](https://pixelcoords.dev)** — demo, comparisons, how-to
- [docs/CLI.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/CLI.md) — every command, flag, control, and exit code
- [docs/OUTPUT.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/OUTPUT.md) — session.json schema, crops, cutouts, jq recipes
- [docs/CONFIGURATION.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/CONFIGURATION.md) — colors, key bindings, config file
- [docs/TROUBLESHOOTING.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/TROUBLESHOOTING.md) — fixes, behaviors, FAQ
- [docs/PERFORMANCE.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/PERFORMANCE.md) — measured timings, and what this tool is not
- [docs/DEVELOPMENT.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/DEVELOPMENT.md) — building, CI gates, tests, releases
- [CHANGELOG.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/CHANGELOG.md) — what changed and why

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

MIT — see [LICENSE](https://github.com/nolindnaidoo/pixelcoords/blob/main/LICENSE). Bundled: JetBrains Mono (OFL 1.1).
