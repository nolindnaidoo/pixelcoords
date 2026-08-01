# Changelog

All notable changes are recorded here, written as they land. Versions
follow [Semantic Versioning](https://semver.org): **minor** (0.x.0) for
new features and for any breaking change to the CLI or the session
schema; **patch** (0.x.y) for fixes. The version keeps incrementing
through 0.x — there is no 1.0 planned, so read the entry below, not the
version number, for what changed under you.

## Unreleased

### Breaking: one JSON envelope for every command that scores

`assert` and `find` printed differently shaped documents with separate
schema counters, both sitting at 1. Three more commands are being written
against the same primitives, and left alone each would have invented a
third shape — so a script reading two of them would need two parsers, and
five eventually.

They now share one envelope: `schema`, `command`, `captured_utc` when the
command actually captured, `ok`, and `results`. `find`'s `all_relocated`
is now `ok`. `assert`'s verdict became a row inside `results` and lost its
own `schema`, since the document carries the version.

`ok` is the **aggregate** the exit code mirrors, and it did not swallow
the per-row answers: `FindResult.found` and `Verdict.hit` stay where they
are. For a single point the two agree, which is exactly why the
distinction is easy to lose — and it stops being true the moment a command
answers about more than one region, which batch scoring will.

The counter starts at **2**, not 1: the two it replaces were both at 1,
and a consumer pinned to either would otherwise see a version it
recognizes on a shape it does not. `session.json` keeps its own counter,
still 1 — a file on disk and an answer on stdout version different
things.

`doctor` and `windows` are unchanged. They report on the machine rather
than on a session.

### Breaking: `assert --label` is now `assert --expect`

`--label` meant two different things depending on the command. In `find`
and `emit` it restricts *which regions the command looks at*; in `assert`
it decided *which region counted as a hit* while still reporting every
region the point landed in. Two of three commands agreed, and three more
were about to be written.

`--label` now means restrict-the-set everywhere. `assert`'s success
criterion is `--expect`, which keeps the behavior that makes it useful: a
miss still lists what the point *did* land in, so scoring a click tells
you it hit "cancel" instead of only that it missed "submit".

`assert --label` exits 2 with a message naming `--expect`. It is refused
rather than reinterpreted because the alternative is a script that still
runs, still exits 0 or 1, and quietly asks a different question. It comes
back on `assert` with the set-restricting meaning next release, which is
an addition rather than a second break.

### The library

`verdict::PointSpace` is now `space::Origin`, in a new `space` module
alongside `Units` (`physical`/`logical`/`auto`) and `logical_of`. An
origin says where `(0, 0)` is; units say whether a step is a device pixel
or a logical point. They were one concept in the CLI's `--space` flag and
are two questions, and the commands being written next have to ask both.
`emit::Platform` moved there too and is re-exported, so `emit`'s own API
is unchanged.

`locate::FindReport` is now `report::Report<FindResult>`;
`locate::all_relocated` computes the aggregate.

## 0.3.0

### `R` releases one display and keeps the rest frozen

Freezing three screens to mark one, then quitting and starting over
because closing a window quit the whole app, was the shape of the problem.
`R` now unfreezes the monitor under the cursor and closes its overlay,
leaving the others exactly as they were. A save afterwards records only
the monitors still frozen — the session says what was actually captured.

The last window still quits, so ending a run stays an explicit act, and a
monitor holding marks refuses with the count rather than discarding them:
deleting is undoable and a window close is not a confirmation dialog. A
release mid-drag is refused too, because the gesture holds an index into
the frames being renumbered.

**Why a keybinding rather than the window's close button:** there isn't
one. The overlay windows are borderless and undecorated on every platform,
so `CloseRequested` never fires from a user action — verified on macOS,
where `Cmd+W` does nothing at all. Closing the window was the obvious
design and it would have shipped unreachable. `R` is rebindable like every
other letter; `release_monitor` is the action name.

### `[capture] monitors`: a launch default for people without a terminal

A double-clicked binary froze every screen, every time, with no way to say
otherwise — no terminal to pass `--monitor` on, and a config file that
carried style and hotkeys only. CLI users had an answer; GUI users had
none.

```toml
[capture]
monitors = "primary"        # or "all", or ["DELL", "Built-in"]
```

Same grammar as `--monitor`, so it is one thing to learn. Precedence is
`--target`/`--pick`, then `--monitor`, then this, then every monitor — a
flag always beats the file rather than intersecting with it.

Nothing degrades quietly. An empty value or empty list is a config error,
`["all", "DELL"]` is a contradiction rather than something to resolve, and
a query naming no attached display fails the launch with the same message
`--monitor` gives instead of falling back to freezing everything.
`doctor --config` reports the shape errors without launching.

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

### crates.io metadata says what the crates are

The binary and the library shared one set of keywords and categories,
inherited from the workspace, and it fit neither. `pixelcoords-core` was
filed under `command-line-utilities`, which it is not — it is a library
with no CLI in it. Three of the five keywords pointed at the wrong
audience entirely: `coordinates` on crates.io is astronomy and geodesy,
`overlay` is struct patching and network overlays, and `capture` is mostly
webcams.

Each crate now carries its own. The binary keeps `screenshot` and takes
`screen-capture`, `computer-use`, and `hidpi`; the library leads with
`geometry`, `hidpi`, and `multi-monitor`, because the coordinate model is
what someone building on it is looking for. Both descriptions now name the
platforms and the DPI story, which is what a person scanning search
results actually needs to know.

No code changed, and nothing about the API or the CLI moved.

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
