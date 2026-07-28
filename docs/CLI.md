# Command reference

Every command, flag, and exit code. The tool is a loop:

```
pixelcoords            # 1. freeze, mark, save a session
pixelcoords assert     # 2. score points against it
pixelcoords emit       # 3. generate click code from it
pixelcoords find       # 4. re-locate its regions after the UI drifts
pixelcoords resume     # 5. reopen it and keep editing
```

Exit codes are the API everywhere: **0** success/hit/found, **1**
miss/not-found/unhealthy, **2** the question was malformed — a script can
always tell a negative answer from a broken invocation.

## `pixelcoords` (the overlay)

Freezes the screen and opens the marking overlay.

| Flag | Meaning |
|------|---------|
| `--monitor <N>` | Freeze only this monitor (default: all) |
| `--target <TITLE>` | Attach to a window: match its title (exact, prefix, substring), then app name; adds `window_px` coordinates |
| `--pick` | Linux: freeze one window chosen in the system picker — the Wayland answer to `--target` |
| `--out <DIR>` | Output directory (default: `Downloads/pixelcoords-captures/<timestamp>`) |
| `--name <TEXT>` | Friendly session name for the resume picker |
| `--config <FILE>` | Config file (default: the OS config dir) |
| `--bind KEY=ACTION[,EDGE][,WHEN]` | Extra key binding, repeatable — see [CONFIGURATION.md](CONFIGURATION.md) |

### Overlay controls

| Input | Action |
|-------|--------|
| Drag | Draw the current shape; release commits, `Esc` cancels |
| Drag inside / edge | Move / resize (`Shift` locks ratio) |
| `Q` / `E` | Rotate 1° per press, held repeats, `Shift` steps 15° |
| `W` / `Tab` | Cycle tool: rect, ellipse, triangle, polygon, freehand |
| `3`–`9` | Polygon tool: side count |
| `A` | Edit the label under the cursor |
| `S` | Save (stays open) |
| `D` | Delete under the cursor |
| `Z` / `Shift+Z` | Undo / redo |
| `C` | Cycle overlapped shapes under the cursor |
| `Alt`+drag | Drag out a duplicate |
| Arrows | Nudge 1px (`Shift` 10px, `Alt` resizes) |
| `M` (hold) | Magnifier loupe |
| `N` | Name the session |
| `Space` (hold) | Move the control panel (position persists) |
| `H` | Hide / show the control panel |
| `Esc` | Cancel what's active; quit when idle (asks once if unsaved) |

Every letter key is rebindable; `Esc`, `Space`, `M`, and the arrows are
built in. Output files are documented in [OUTPUT.md](OUTPUT.md).

## `doctor`

Check permissions, config, and the monitor table. Exits nonzero when
anything is unhealthy, so scripts can gate on it.

| Flag | Meaning |
|------|---------|
| `--config <FILE>` | Validate this config file |
| `--json` | Machine-readable report on stdout |

## `windows`

List every window `--target` could match, front-most first.

| Flag | Meaning |
|------|---------|
| `--json` | Machine-readable list (native coordinate space named) |

X11 only on Linux; on Wayland it exits nonzero and points at `--pick`.

## `shoot`

Capture every monitor straight to PNG files — no overlay, no session.

| Flag | Meaning |
|------|---------|
| `--out <DIR>` | Output directory (default: the Downloads default) |

## `resume`

Reopen a saved session for editing in windows sized to the capture;
saves update the session in place.

| Flag | Meaning |
|------|---------|
| `--session <PATH\|NAME>` | A session dir, session.json path, or folder name under the captures root |
| `--last` | The newest session, no questions |
| `--out <DIR>` | Save somewhere else instead of in place |

With no flags: an interactive numbered picker (newest first, showing
name, time, selections, target, platform). Refuses politely on a
non-terminal stdin.

## `rename`

Set (or clear, with `""`) a session's friendly name. Rewrites only
`session.json`.

| Flag | Meaning |
|------|---------|
| `--session <PATH\|NAME>` | Which session |
| `--name <TEXT>` | The name |

## `assert`

Score a point against a saved session's regions. Exit 0 hit, 1 miss,
2 malformed. Stdout is a versioned JSON verdict (misses report the
nearest region and its distance).

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--point <X,Y>` | The point (negatives allowed) |
| `--label <TEXT>` | Only this label counts as a hit (case-insensitive) |
| `--space global\|monitor\|window` | Which stored coordinates the point is in (default `global`) |
| `--monitor <N>` | The monitor for `--space monitor` |

## `emit`

Print ready-to-paste click snippets, one per selection, in the target
tool's own coordinate convention.

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--format pyautogui\|cliclick\|xdotool` | The automation tool |
| `--label <TEXT>` | Emit only this label |

## `find`

Re-locate every selection in a fresh capture by its saved crop
(template matching). Exit 0 all found unambiguously, 1 otherwise, 2
malformed. Stdout is a versioned JSON report with new coordinates and
deltas; changed regions are reported missing, duplicated regions
ambiguous.

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--label <TEXT>` | Re-locate only this label |
