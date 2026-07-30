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

## UI state

Where you park the control panel (hold `Space`) survives between runs
in a small app-owned file — `state.toml` next to `config.toml` in the
OS config directory. It is safe to delete at any time; pixelcoords
rewrites it on exit and treats a corrupt or missing file as "use the
default corner".
