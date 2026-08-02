# Configuration

pixelcoords reads one optional TOML file. Everything has a sensible
default; the file only needs the values you want to change. A complete
annotated example lives at
[config.example.toml](../config.example.toml).

## Location

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/pixelcoords/config.toml` |
| Linux | `~/.config/pixelcoords/config.toml` |
| Windows | `%APPDATA%\pixelcoords\config.toml` |

`--config PATH` overrides the default location. A missing default file
means defaults; a missing `--config` file or invalid TOML is an error —
pixelcoords never silently falls back. `pixelcoords doctor` reports which
config is in effect and whether it parses.

## Capture

Which monitors a launch freezes when no `--monitor` flag is given.

```toml
[capture]
monitors = "all"                    # the default
# monitors = "primary"
# monitors = ["DELL", "Built-in"]
```

Each value is a monitor query with the same grammar as
[`--monitor`](CLI.md#choosing-monitors): an index, `primary`, or part of a
display's name. `"all"` means every monitor, and only means that on its
own — `["all", "DELL"]` is a contradiction and an error, not a resolution.

This exists for the launch that has nowhere to put a flag: a
double-clicked binary has no terminal, and before this its only option was
freezing every screen. Set it once and every GUI launch honors it.

**Precedence**, highest first:

1. `--target` / `--pick`
2. `--monitor`
3. `[capture] monitors`
4. every monitor

A flag always beats the file — it never intersects with it.

Nothing here degrades quietly. An empty value or an empty list is a config
error, and a query naming no attached display fails the launch with the
same message `--monitor` gives, rather than falling back to freezing
everything. `pixelcoords doctor --config` reports the shape errors without
launching.

## Snapping

The point you are placing is pulled onto UI edges detected in the frozen
screenshot, so a rect drawn around a button lands on the button rather
than four pixels off it.

```toml
[snap]
enabled = true   # the default
radius = 8       # logical px, 1-64
```

`radius` is in **logical** pixels and scaled per monitor, so the reach
under your hand is the same on a Retina panel and a 1x one. A value
outside `1-64` is a config error: zero would be a silently disabled
feature and a huge one would drag the pointer across half the screen, and
neither is more likely to be intended than typed.

`X` toggles snapping for the rest of a run, with the control panel's `X`
row showing the live state. The toggle deliberately does not persist —
this file owns the default.

**What snaps:** the point you are actively placing. Drawing a shape snaps
both the press and the release, resizing snaps the dragged handle, and a
measure ruler snaps each endpoint. **What does not:** arrow-key nudging,
ever. Arrows mean exactly one pixel, and a feature that quietly overrode
them would make the one exact tool in the overlay inexact. A freehand
stroke is not snapped either — a path of snapped points is a jagged path.

Detection is pixels only — a luma gradient, no accessibility tree and no
toolkit introspection — so it works the same on a native app, a game, and
a screenshot of either. The threshold adapts to each frame rather than
being an absolute number, because a cut that finds edges on a light theme
finds nothing on a dark one. Below the floor nothing is offered: a subtle
background gradient is not an edge, and snapping to one would be worse
than not snapping.

When a snap happens the overlay draws the edge that captured the point,
along its detected extent. Seeing *what* you snapped to is the difference
between trusting the placement and wondering why the cursor moved.

## Style

```toml
[style]
preview_color = "#00A0FF"  # outline while dragging a shape out
complete_color = "#00FF66" # outline of committed shapes
label_color = "#FFFFFF"    # captions and the HUD
target_color = "#FFB000"   # border around the --target window
thickness = 2              # outline width in logical px (0 hides outlines)
fill = false               # fill shapes instead of outlining
```

Colors are hex RGB — `#RGB` or `#RRGGBB`, `#` optional. `thickness` is in
logical pixels and is scaled by each monitor's DPI factor, so outlines have
the same visual weight on any display.

## Key bindings

Defaults:

| Key | Action | Fires | Condition |
|-----|--------|-------|-----------|
| `w` / `tab` | `next_tool` | press | — |
| `a` | `label_edit_at_cursor` | release | `cursor_in` |
| `s` | `save` | press | `has_selection` |
| `d` | `delete_at_cursor` | press | `cursor_in` |
| `z` | `undo` | press | — |
| `c` | `cycle_overlap` | press | `cursor_in` |
| `h` | `toggle_panel` | press | — |
| `n` | `name_session` | press | — |
| `r` | `release_monitor` | press | — |
| `q` / `e` | `rotate_ccw` / `rotate_cw` | press and repeat | `cursor_in` |

Quit is not a binding: `Esc` cancels whatever is in progress, and quits
when nothing is (asking once when there is unsaved work). The arrow
keys are also built in rather than bindable: they nudge the shape under
the cursor by 1px (`Shift` 10px; `Alt` resizes instead of moving), as
are the holds — `Space` moves the panel, `M` shows the loupe.

Rebind in the config file:

```toml
[[hotkeys]]
key = "s"
action = "save"
when = "has_selection"
```

or per run: `--bind KEY=ACTION[,EDGE][,WHEN]` (repeatable).

- **KEY** — a single character, `tab`, or `capslock`.
- **ACTION** — `quit`, `save`, `next_tool`, `delete_at_cursor`,
  `label_edit_at_cursor`, `undo`, `redo`, `cycle_overlap`, `toggle_panel`,
  `name_session`, `release_monitor`, `rotate_ccw`, `rotate_cw`.
- **EDGE** — `press` (default), `release`, or `repeat` (fires while held).
- **WHEN** — `has_selection` (at least one shape exists) or `cursor_in`
  (the cursor is over a shape). Omitted means always.

Binding a key removes *all* default bindings for that key, on every edge —
rebinding `q` will not leave the default's repeat-edge rotation behind.
Among your own bindings, later entries win for the same key and edge.
Unknown keys, actions, edges, or conditions are startup errors, not silent
no-ops.

`Esc`, `Enter`, and `Backspace` are fixed: `Esc` cancels the drag or label
edit in progress (never quits), `Enter` commits a label, `Backspace`
deletes while labeling.

## Limits and resources

Two lists, and the second is the more useful one.

### What is deliberately unbounded

These grow with what you do, and nothing stops them. That is a decision,
not an oversight — a wall you hit mid-session is worse than memory you
chose to spend, and none of these has a natural number to stop at.

- **Undo history.** Every edit is kept for the life of the session, and a
  freehand stroke's undo entry carries its whole point list. A long
  session with hundreds of edits holds all of them in memory. Nothing
  truncates.
- **Selections and measures per session.** No cap. Each selection also
  writes a crop on save, so a hundred marks means a hundred PNGs.
- **Session and capture size.** One frozen frame per monitor is held for
  the life of the overlay, at that monitor's full physical resolution. On
  a 6K display that is around 100 MB before anything is drawn.

If you work at a scale where this matters, it is yours to manage — quit
and reopen for a fresh session, or split the work across sessions. The
tool will not decide for you.

### What has a ceiling, and why

Each of these refuses out-of-range values loudly, naming the field. The
numbers are current defaults rather than permanent truths.

| Setting | Ceiling | The reason behind the number |
|---|---|---|
| `[snap] radius` | 64 logical px | Scoring is O(radius²) — about 15µs per query at radius 16 on a 3600×2338 frame. Past roughly 100 the pointer visibly lags, so this is a cost curve, not a preference |
| `[style] thickness` | 512 px | Nothing breaks above it; the bound exists to catch a typo in a value you are already setting by hand |
| Label length | 64 characters | A label becomes part of a crop's filename, and most filesystems stop at 255 bytes. Enforced when you type and when a session is read |
| Polygon sides | 1000 | Past a few hundred at any real screen size the vertices land on the same pixels. Some ceiling has to exist because the count is a per-vertex allocation. The overlay reaches 3–9, which is the digit keys' reach rather than the shape's limit |

A cap you hit is a bug report; a cost you chose is a feature. Where a
number above is a genuine cost, it is stated so you can decide. Where it
is only typo protection, it says that too.

## UI state

Where you park the control panel (hold `Space`) survives between runs
in a small app-owned file — `state.toml` next to `config.toml` in the
OS config directory. It is safe to delete at any time; pixelcoords
rewrites it on exit and treats a corrupt or missing file as "use the
default corner".
