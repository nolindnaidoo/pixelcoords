# Command reference

Every command, flag, and exit code. The tool is a loop:

```
pixelcoords            # 1. freeze, mark, save a session
pixelcoords assert     # 2. score points against it
pixelcoords resolve    # 3. ask where to act, in your API's units
pixelcoords emit       # 4. generate click code from it
pixelcoords diff       # 5. check its regions still look right
pixelcoords find       # 6. re-locate its regions after the UI drifts
pixelcoords resume     # 7. reopen it and keep editing
```

Exit codes are the API everywhere: **0** success/hit/found, **1**
miss/not-found/unhealthy, **2** the question was malformed — a script can
always tell a negative answer from a broken invocation.

Every machine-readable answer shares one envelope — `schema`, `command`,
`captured_utc` when the command captured, an aggregate `ok` mirroring the
exit code, and `results`. Per-region answers stay on the rows, so a
report tells you *which* region failed, not merely that one did. The
contract behind it is written up in
[DEVELOPMENT.md](DEVELOPMENT.md#the-agent-surface-contract).

## `pixelcoords` (the overlay)

Freezes the screen and opens the marking overlay.

| Flag | Meaning |
|------|---------|
| `--monitor <QUERY>` | Freeze only these monitors (default: all). Repeatable, or comma-separated — see below |
| `--target <TITLE>` | Attach to a window: match its title (exact, prefix, substring), then app name. Locks the drawable region to the window (see below) and records `window_px` coordinates on every mark |
| `--pick` | Linux: freeze one window chosen in the system picker — the Wayland answer to `--target` |
| `--out <DIR>` | Output directory (default: `Downloads/pixelcoords-captures/<timestamp>`) |
| `--name <TEXT>` | Friendly session name for the resume picker |
| `--config <FILE>` | Config file (default: the OS config dir) |
| `--bind KEY=ACTION[,EDGE][,WHEN]` | Extra key binding, repeatable — see [CONFIGURATION.md](CONFIGURATION.md) |

### Choosing monitors

Each `--monitor` value is one of three things:

| Query | Means |
|-------|-------|
| `0`, `1`, … | The monitor with that index, as `doctor` lists it |
| `primary` | The display the OS marks primary — exactly one always is |
| any other text | Part of a display's name: exact, then prefix, then substring, case-insensitive |

A number is read as an index first, so `--monitor 0` always means index 0.
If no monitor carries that index it is tried as a name instead — which
matters on platforms whose display names are mostly digits, where the
number is the only distinctive thing to type.

Repeat the flag or separate with commas; the same display named twice is
frozen once, and the set comes back in enumeration order regardless of the
order you asked:

```bash
pixelcoords --monitor primary
pixelcoords --monitor 0,2
pixelcoords --monitor primary --monitor DELL
```

Nothing here falls back silently. A query matching no monitor is an error
listing what is attached; a **name** matching two displays equally well is
an error listing both, because guessing would freeze a screen you did not
mean and you would only find out after marking it. Two panels of the same
model are exactly this case — address them by index.

**How useful a name is depends on your platform.** The name is whatever the
capture backend reports, and that is not always something you would
recognize: on this project's macOS test machine two displays came back as
`Display #41054` and `Display #15824`, while X11 and Windows typically
report the model. Note what that means for matching — every such name
shares the word `Display`, so `--monitor Display` is ambiguous there and
refuses; the digits are what actually distinguishes them. Run `doctor` to
see what yours are called. The index and `primary` work everywhere
regardless.

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
| `R` | Release the monitor under the cursor: unfreeze that display and close its window, leaving the others frozen. Refused while it still holds marks |
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

Capture monitors straight to PNG files — no overlay, no session. Every
monitor by default, or the ones you name.

| Flag | Meaning |
|------|---------|
| `--out <DIR>` | Output directory (default: the Downloads default) |
| `--monitor <QUERY>` | Capture only these monitors — same grammar as the overlay's, see [Choosing monitors](#choosing-monitors) |

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
2 malformed. Stdout is a versioned JSON report whose single result is the
verdict (misses report the nearest region and its distance).

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--point <X,Y>` | The point (negatives allowed); required unless `--stdin` |
| `--stdin` | Read points from stdin instead — one `X,Y` or `X,Y,label` per line. Conflicts with `--point` |
| `--expect <TEXT>` | The label the point must land in for a hit (case-insensitive) |
| `--space global\|monitor\|window` | Which stored coordinates the point is in (default `global`) |
| `--monitor <N>` | The monitor for `--space monitor` — an **index only**, and a different flag from the overlay's `--monitor` above. This one names a monitor *recorded in the session*, where the index is the record's own identifier; the overlay's picks a display attached right now |

`--stdin` scores a whole trajectory in one process: the session is read
once instead of once per click. Each result carries its 1-based `line`,
counted over *input* lines, so blank lines and `#` comments keep their
numbering and a reported line matches the file you wrote. A line's own
label overrides `--expect` for that line only. A malformed line — or one
naming a region the session does not have — stops the run with the line
number and prints nothing: a partially scored trajectory would report a
pass rate over a prefix, and the caller could not tell.

`--expect` used to be spelled `--label`, and it does **not** filter the
report: a miss still lists every region the point did land in, which is
what makes `assert` useful for scoring a click rather than just failing
it. `--label` now means "restrict which regions a command looks at" on
every command that has it, so `assert --label` exits 2 and names
`--expect`. It comes back on `assert` with the set-restricting meaning in
the next release; erroring for one release is what stops a script from
silently changing what it asks.

## `emit`

Print ready-to-paste click snippets, one per selection, in the target
tool's own coordinate convention.

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--format pyautogui\|cliclick\|xdotool` | The automation tool |
| `--label <TEXT>` | Emit only this label |

## `resolve`

Answer "where do I act for this label, right now" — the click point per
selection, in the space and units your API speaks. Exit 0 resolved, 1 not
resolvable, 2 malformed. Stdout is a versioned JSON report.

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--label <TEXT>` | Resolve only this label |
| `--space global\|monitor\|window` | Which origin the answer is measured from (default `global`) |
| `--units auto\|physical\|logical` | Which scale the answer is in (default `auto`) |
| `--relocate` | Capture first and correct for drift |

`--units auto` is the flag's reason to exist: logical points on macOS,
physical pixels on Windows and X11 — what each platform's input APIs
actually expect. Each selection converts through **its own monitor's**
scale, so a mixed-DPI desktop comes out right without the caller knowing
it was mixed.

`--space` and `--units` are separate questions: an origin says where
`(0, 0)` is, units say whether one step is a device pixel or a point.
Unlike `assert --space monitor`, this one needs no `--monitor` index —
every row reports the monitor it belongs to, and each is answered in that
monitor's own coordinates.

Without `--relocate` it is pure session math: headless, instant, no
capture and no screen-recording permission. With it, one capture per
monitor serves every label, drift is applied before the units convert,
and a region that could not be found unambiguously comes back without a
point and takes `ok` to false — a region matching in two places has no
coordinate worth handing to an executor.

`emit` remains the human-facing sibling: ready-to-paste code. `resolve`
is the machine answer underneath it.

## `diff`

Compare each region's saved crop against the same rectangle of the screen
now, or of stored artifacts. Exit 0 every region within tolerance, 1 any
region over, 2 malformed. Stdout is a versioned JSON report.

| Flag | Meaning |
|------|---------|
| `--session <PATH>` | The session |
| `--against <DIR\|IMAGE>` | Compare against another session directory's screenshots, or one PNG standing in for a single-monitor capture, instead of capturing |
| `--label <TEXT>` | Compare only this label |
| `--tolerance <PCT>` | Percent of a region's masked pixels allowed to differ (default `0` — exact) |

This is visual regression testing over **regions a human marked**, not
whole screenshots: a change outside your regions is not a difference, and
shaped selections compare by their own silhouette because a saved crop
already carries its shape in its alpha channel.

`--tolerance` defaults to exact. Anti-aliasing and font smoothing make a
small nonzero value the practical choice in CI, but a diff tool that
rounds by default is lying by default — start at `0`, raise it only as
far as your own noise requires, and note that the denominator is the
region's **masked** pixels, so one number means the same thing for a
rect and for a circle.

`--against` is the offline form: no capture, no permission, so it runs in
CI against artifacts. An image whose dimensions do not match the session's
monitor is refused rather than compared — the measurement would describe
the resize, not the UI.

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
