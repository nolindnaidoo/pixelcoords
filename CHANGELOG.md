# Changelog

All notable changes are recorded here, written as they land. Versions
follow [Semantic Versioning](https://semver.org). Pre-1.0 policy:
**minor** (0.x.0) for new features and for any breaking change to the
CLI or the session schema; **patch** (0.x.y) for fixes. 1.0.0 comes when
the schema and CLI are declared stable.

## 0.1.1

- **README links work on crates.io now.** crates.io resolves a README's
  relative links against the crate's own subdirectory
  (`crates/pixelcoords/`), so every docs link — and the demo GIF — was
  broken on the crate page. All README links and the GIF are absolute
  URLs now. No code changes.

## 0.1.0

The initial release: a screen-coordinate toolchain built around a frozen
screen.

- **The overlay.** Freeze every monitor — or one (`--monitor`), or a
  single window (`--target`, and `--pick` on Wayland) — then mark
  regions with five tools: rectangle, ellipse, triangle, regular N-gon
  (`3`–`9` sides), and freehand. Move, resize, rotate, label, alt-drag
  duplicate, cycle overlapped shapes, undo/redo, pixel nudging with
  arrows, a magnifier loupe on `M`, live cursor coordinates, and a
  movable control panel that remembers its parking spot. Every key is
  rebindable (config file or `--bind`).
- **The outputs.** A versioned `session.json` — monitor-local, global,
  and window-relative physical-pixel coordinates, per-monitor DPI
  scale, platform and capture provenance, an optional friendly name —
  plus full screenshots, per-selection PNG crops (non-rect shapes
  alpha-masked), and a frame-sized cutout pair per monitor: selections
  kept in place, and their exact complement. Saves never overwrite
  files pixelcoords did not write.
- **The subcommands.** `assert` scores a point against saved regions
  with the exit code as the API; `emit` prints ready-to-paste click
  snippets (pyautogui, cliclick, xdotool) with each tool's coordinate
  convention already applied; `find` re-locates regions in a fresh
  capture by their saved crops and reports the drift; `resume` reopens
  a session for editing and saves it in place; `rename` names a
  session for the interactive picker; `shoot` is the scripted
  no-overlay capture; `windows` lists targetable windows; `doctor`
  checks permissions, config, and monitors. `--json` everywhere a
  script would read the answer.
- **Platforms.** macOS, Windows, and Linux — X11 fully, Wayland
  snapshot and `--pick` — each verified on real hardware; the README's
  platform table records exactly what has been exercised where.
