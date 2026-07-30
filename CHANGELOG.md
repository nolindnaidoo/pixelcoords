# Changelog

All notable changes are recorded here, written as they land. Versions
follow [Semantic Versioning](https://semver.org). Pre-1.0 policy:
**minor** (0.x.0) for new features and for any breaking change to the
CLI or the session schema; **patch** (0.x.y) for fixes. 1.0.0 comes when
the schema and CLI are declared stable.

## 0.3.0

### `--monitor` takes a name, not just a number

`--monitor` accepted one index. Indexes are enumeration order, so the only
way to discover one was `doctor`, and the number a script pinned last week
can address a different panel today. Windows have had a real matcher since
the start; monitors got a bare integer.

Now each `--monitor` value is an index, the word `primary`, or part of a
display's name — matched exact, then prefix, then substring,
case-insensitively, the same discipline `--target` uses on window titles.
The flag repeats and accepts commas, so `--monitor primary,DELL` freezes
both. Naming one display twice freezes it once, and the set comes back in
enumeration order however you asked for it. `shoot` takes the same flag.

Nothing falls back silently. A query matching nothing is an error listing
what is attached. A **name** matching two displays equally well is an error
listing both, rather than a guess — unlike windows, monitors have no
stacking order to break the tie with, and freezing the wrong screen is
discovered only after you have marked regions on it. Two panels of the
same model are exactly that case; address them by index.

`doctor` now says the name is addressable rather than leaving it as data
on a line.

A number is read as an index first — `--monitor 0` always means index 0 —
and falls through to a name when nothing carries that index. That is not
a nicety: macOS reports display names like `Display #41054`, so the digits
are the only distinctive thing to type, and reading them strictly as an
index made the most obvious query the one that could not work.

**One honest limit:** the name is whatever the capture backend reports,
and it is not always recognizable. Two displays on this project's macOS
test machine came back as `Display #41054` and `Display #15824` — so they
share the word `Display`, and matching on it is ambiguous rather than
useful. X11 and Windows typically report the model. The index and
`primary` work everywhere regardless, and `doctor` shows you what yours
are called.

`assert --monitor` is untouched and stays an index. It names a monitor
*recorded in a session*, where the index is the record's own identifier —
a different question from picking a display attached right now.

### `find` recognizes a display instead of counting them

Relocation matched a session's monitors to the attached ones by
enumeration index. That index is not a property of a display — it is the
order the OS happened to list them in, and it shuffles across replugs,
reboots, and dock/undock. So unplugging a monitor and putting it back in a
different port broke `find`, on hardware that had not changed at all.

Everything needed to recognize a panel was already written into
`session.json` at capture time — `name`, `size_px`, `scale`. Now it is
used: a session's monitor resolves against the displays attached now by
identity, and the index only breaks ties between two of the same model.
The tie breaks toward the display that held the session's index if it is
still present, so the common case resolves to the same panel it did
before rather than to whichever twin enumerates first.

Sessions get this for free. Nothing about `session.json` changed, and the
schema version stays 1.

### Two refusals where there was one

"Monitor 1 is no longer attached" covered both a display that was gone and
a display that was still there at a different resolution — one sentence
sending everyone to check a cable. They are now separate: a missing
display is named, with its recorded geometry and the list of what *is*
attached; a changed one keeps the honest "relocation needs the same
display setup" refusal, because template matching survives a window
moving, not the pixels underneath it being resampled.

## 0.2.1

Doc-only patch. `pixelcoords-core`'s install snippet said
`pixelcoords-core = "0.1"` in the 0.2.0 release. That is a caret range
against 0.1.x, so a reader copy-pasting from the top of the crates.io
page landed on 0.1.1 and the old API rather than 0.2.0. Snippet bumped
to `"0.2"`, and the doctest fixtures updated to match the current
version.

Adds a release checklist to `docs/DEVELOPMENT.md` naming this specific
trap first, so a future minor cut cannot repeat it.

## 0.2.0

**Breaking.** `--target` mode changed shape and one public API changed in
core.

### `--target` now means what its name says

Dragging, resizing, and drawing all obey the same rule: **the drawable
region is the window, not the monitor.** Grab an existing selection and
drag it, or grab a resize handle — the shape stops at the window edge
now instead of the monitor edge. Under the old build, moving a valid
selection outside the window silently produced an invalid one on save.

Yesterday: `--target` picked a monitor and *tagged* selections inside a
window with `window_px`, while letting you draw anywhere on the monitor.
Marks outside the window still got a `window_px` field — with negative
coordinates — because the code translated everything unconditionally.

Today: `--target` **locks the drawable region to the window's rect**.

- Pixels outside the window are dimmed in the overlay, so the boundary
  is visible.
- Clicks and drags outside the window do nothing — no shape starts. The
  cursor is `NotAllowed` there, not the crosshair, so it does not lie.
- Existing selections dragged inside the window still work; nothing
  escapes.
- The old behavior was misleading enough that the fix is a break, not a
  toggle. There is no flag to restore it.

### Sessions from older builds

A resumed session that predates this change may contain selections drawn
outside the window on the monitor. `restore_selections` now drops them
and reports the labels on stderr. The dropped crops stay on disk but are
no longer referenced by the session — safe to remove.

### Library API

`pixelcoords_core::geometry::Shape::compute_preview` signature changed:
`bounds: Size` → `region: Rect`. Same change to `Shape::clamp_move`,
`Shape::resize_to`, `Shape::clamp_move_rotated`, and
`Shape::resize_to_rotated` — every method that used to accept a `Size`
now takes a `Rect`, because the drawable region is not always the whole
frame. Passing `Rect::new(0, 0, size.w, size.h)` restores the previous
behavior at each call site.

`pixelcoords_core::session::restore_selections` now returns
`(Vec<Selection>, Vec<String>)` — the second element is the labels of
selections dropped because they lay outside the window. If you were
using it before, destructure the tuple; the empty vec is the "nothing
was dropped" case.

## 0.1.2

- **Captures no longer include the mouse pointer (macOS).** The pointer is
  drawn on top of the screen, not part of it, and compositing it into a
  capture corrupts exactly the thing this tool exists to do. It poisoned
  saved crops — mark a region while the pointer is over it and the crop has
  a pointer baked in — and it broke `find`, because whatever moved the
  pointer usually left it on the region about to be re-located. Measured
  cost on a low-detail region: **0.17 of match score**, enough to drop a
  perfect match below the 0.9 floor, while a busy region absorbed it
  entirely — so the symptom looked like flakiness rather than a bug.
  macOS now captures through `CGDisplayCreateImage`, which returns display
  contents alone, matching what the system's own `screencapture` does.
  Windows and Linux were already pointer-free.

  Found by building [pixelactions](https://github.com/nolindnaidoo/pixelactions),
  the executor half of this loop, which parks the pointer on whatever it
  just clicked and then asks `find` to re-locate that same region — the
  one workload guaranteed to hit this. Nothing here changed to accommodate
  that tool; the capture was simply wrong, and a second consumer made it
  obvious. pixelactions requires this version and refuses to run against
  anything older.

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
