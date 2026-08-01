# Output reference

Every save (`S`) writes one directory — default
`Downloads/pixelcoords-captures/<UTC timestamp>/`, or wherever `--out`
points. (A system with no known Downloads directory — headless Linux
without XDG user dirs — falls back to the working directory.)
Saving again during the same session updates the same directory: crops that
no longer exist are removed, the frozen screenshots are not re-encoded, and
files pixelcoords did not create are never touched.

A saved directory is not write-only: `pixelcoords resume` reopens one
for editing — the frozen screenshots come back as the canvas and later
saves update the same files. Bare `resume` offers an interactive pick of
everything under the captures folder; `--last` takes the newest;
`--session` accepts a path or just the folder name as an id.

If `--out` points somewhere already holding a `session.json`,
`screenshot-*.png`, `crop-*.png`, or `cutout-*.png` that pixelcoords did
not write, the save stops and says so rather than overwriting it. A
directory containing this tool's own `session.json` is fair game, so
re-running into the same `--out` works as before.

```
pixelcoords-captures/20260727-113542-097/
├── session.json
├── screenshot-0.png          # one full frozen capture per monitor
├── cutout-primary-0.png      # the frame with only the selections visible
├── cutout-inverse-0.png      # the complement: selections punched out
├── crop-0-login-button.png   # one crop per selection
└── crop-1.png                # unlabeled selections omit the slug
```

## session.json

```json
{
  "schema": 1,
  "app": { "name": "pixelcoords", "version": "0.4.0" },
  "created_utc": "2026-07-27T11:35:42Z",
  "monitors": [
    {
      "index": 0,
      "name": "Built-in Retina Display",
      "primary": true,
      "origin_px": { "x": 0, "y": 0 },
      "size_px": { "w": 3024, "h": 1964 },
      "scale": 2.0
    }
  ],
  "target": {
    "app": "Notepad",
    "title": "notes.txt",
    "monitor": 0,
    "origin_px": { "x": 400, "y": 250 },
    "size_px": { "w": 1600, "h": 1200 }
  },
  "selections": [
    {
      "shape": "rect",
      "label": "login button",
      "monitor": 0,
      "px": { "x": 520, "y": 448, "w": 300, "h": 88 },
      "global_px": { "x": 520, "y": 448, "w": 300, "h": 88 },
      "rot_deg": 15,
      "window_px": { "x": 120, "y": 198, "w": 300, "h": 88 },
      "crop": "crop-0-login-button.png"
    }
  ]
}
```

`target`, `window_px`, and `rot_deg` appear only when applicable.

### Provenance

`platform` records the OS the session was captured on (`macos`,
`windows`, `linux-x11`, `linux-wayland`) and `capture` how (`desktop`,
`window` via `--target`, `pick` via the portal). Together with the
`target` record's app and title, a consumer holds everything needed to
re-attach to the same window on the same platform at the recorded
coordinates. Both fields are optional — sessions written before they
existed simply lack them — and `resume` passes them through unchanged,
so an edited file keeps saying where it was captured.

## Coordinate spaces

All stored coordinates are **physical pixels** — the captured image's own
grid, which is what automation tools and screenshots use.

- `px` — monitor-local physical pixels; the authoritative value. Indexing
  `screenshot-<monitor>.png` with `px` lands exactly on the marked region.
- `global_px` — `px` translated by the monitor's `origin_px`; use it when
  working across monitors.
- `window_px` — present in `--target` sessions: `px` relative to the
  target window's top-left at freeze time. It stays valid when the window
  moves between sessions. Always non-negative and within the window's
  size — since 0.2.0, marking outside the window is refused at draw time.
  Older sessions may contain out-of-window marks with negative
  coordinates; `pixelcoords resume` drops them and reports the labels.
- Logical points (for APIs that want them) are `px / scale` using the
  monitor's `scale`.

## Shapes

- `"shape": "rect"` — `px` is `{x, y, w, h}`. If rotated, `rot_deg` gives
  degrees clockwise about the box center; the box itself stays
  axis-aligned so unrotated consumers still get a sane region.
- `"shape": "circle"` — `px` is `{cx, cy, r}`. Circles never carry
  rotation. Written by older versions; the drawing tool is the ellipse
  now, and a circle is an ellipse with equal radii.
- `"shape": "ellipse"` — `px` is `{cx, cy, rx, ry}`. Like a rect, an
  ellipse may carry `rot_deg` (clockwise about its center) while `px`
  stays axis-aligned.
- `"shape": "poly"` — `px` is `{points: [{x, y}, …]}`, a closed polygon
  in drawing order. Regular polygons and freehand strokes both store
  this way — one representation, one consumer code path. Rotation is
  baked into the vertices.
- `"shape": "triangle"` — `px` is the three vertices
  `{ax, ay, bx, by, cx, cy}`. Rotation is baked into the vertices at save
  time, so the stored geometry is always exact and `rot_deg` never
  appears.

## Crops

Each selection's crop is cut from its monitor's frozen screenshot over the
shape's (rotated) bounding box, clipped to the screen. Circles, triangles,
and rotated rects are transparent outside the shape, so the crop composites
cleanly. File names are `crop-<index>-<label-slug>.png`, or
`crop-<index>.png` when unlabeled.

## Cutouts

Each monitor with selections gets a frame-sized pair:

- **`cutout-primary-<monitor>.png`** — the frozen capture with the alpha
  of everything **outside** the selections zeroed. All of a monitor's
  selections stay visible in place at their original positions — where
  the per-selection crops isolate each region, the primary cutout
  preserves their spatial relationships on transparency.
- **`cutout-inverse-<monitor>.png`** — the exact complement: the
  selections punched out to transparency, everything else kept. Useful
  as a redaction (hide the marked regions) or as the backdrop the
  primary composites onto — every pixel is opaque in exactly one of the
  pair, so together they reassemble the screenshot.

Both are shape- and rotation-aware, using the same masks as the crops,
so the artifacts agree pixel-for-pixel. Written only for monitors that
hold at least one selection; deleting a monitor's last selection removes
its cutouts on the next save, and an unchanged selection set skips the
re-encode.

## Consuming it

```bash
# global coordinates of every selection labeled "login button"
jq '.selections[] | select(.label == "login button") | .global_px' session.json

# center of the first selection (rect shapes), as x,y
jq -r '.selections[0].px | "\(.x + .w/2),\(.y + .h/2)"' session.json
```

The schema is versioned: additions are optional fields, and any breaking
change bumps `"schema"`.

`session.json`'s `schema` counts separately from the one on command
output. The session format is still at 1 and has been since 0.1.0; the
commands share their own counter, now at 2. They version different
things — a file on disk and an answer on stdout — and tying them together
would force a session-format bump every time a report gained a field.

### Picked-window sessions (`--pick`, Linux)

A session captured through the desktop portal's window picker contains
one frame that *is* the picked window: a pseudo-monitor named
`picked window (portal)` at origin `(0, 0)` sized to the returned pixels,
and a `target` whose `app` is `xdg-desktop-portal`. Every selection's
`px`, `global_px`, and `window_px` coincide — all three are
window-relative, because the frame is the window. The portal reveals no
window title, desktop position, or DPI scale, so the session claims
none: `scale` is recorded as `1.0` and coordinates are as-rendered
pixels. Consumers keyed on `window_px` work unchanged.

## Verifying points: `assert`

`pixelcoords assert` answers "does this point land inside a marked
region?" without opening the overlay — ground truth for click automation
and computer-use agents, scored against regions a human marked once.

```bash
pixelcoords assert --session shots --point 812,440
pixelcoords assert --session shots --point 812,440 --expect submit
pixelcoords assert --session shots --point 100,50 --space window
pixelcoords assert --session shots --point 15,25 --space monitor --monitor 1
```

The exit code is the API: **0** the point hit (the `--expect` region when
given, any region otherwise), **1** it missed, **2** the question was
malformed — unreadable session, unknown label, window space on an
untargeted session. `--space` says which stored coordinates the point is
in: `global` (default, `global_px`), `monitor` (`px`; `--monitor` picks
which, optional on single-monitor sessions), or `window` (`window_px`,
`--target` sessions only). Labels match case-insensitively. The point is
always in **physical pixels** — there is no `--units` here, because a
logical input point at scale 2.0 covers a 2×2 physical block and no
rounding rule makes "hit" or "miss" the honest answer.

Stdout is the shared report envelope, holding one verdict:

```json
{
  "schema": 2,
  "command": "assert",
  "ok": false,
  "results": [
    {
      "point": { "x": 812, "y": 440 },
      "space": "global",
      "hit": false,
      "contained_in": [
        { "index": 0, "label": "cancel", "shape": "rect", "monitor": 0 }
      ],
      "nearest": {
        "region": { "index": 2, "label": "submit", "shape": "rect", "monitor": 0 },
        "bbox_distance_px": 42.5
      }
    }
  ]
}
```

No `captured_utc`: scoring a point is pure session math, and stamping a
time on it would imply a capture that never happened.

`ok` is the aggregate the exit code mirrors; `hit` is this row's own
answer. They agree for a single point and stop agreeing the moment a
command answers about several, which is why both exist.

### Scoring a trajectory: `assert --stdin`

One process, one session parse, one point per line:

```bash
printf '# login flow\n\n850,440\n1050,440,submit\n' \
  | pixelcoords assert --session shots --stdin
```

Lines are `X,Y` or `X,Y,label`; blank lines and `#` comments are skipped.
A line's own label overrides `--expect` for that line only, so one stream
can score a heterogeneous run. Only the first two commas are structural,
so a label may contain commas without quoting: `1,2,row 3, column 4`.

Each result gains a **`line`** — 1-based, counted over *input* lines, so
skipped blanks and comments keep their numbering and a reported line
matches the file you wrote:

```json
{
  "schema": 2,
  "command": "assert",
  "ok": false,
  "results": [
    { "line": 3, "point": { "x": 850, "y": 440 }, "space": "global",
      "hit": true,
      "contained_in": [ { "index": 0, "label": "cancel", "shape": "rect", "monitor": 0 } ] },
    { "line": 4, "point": { "x": 1050, "y": 440 }, "space": "global",
      "hit": false, "contained_in": [],
      "nearest": { "region": { "index": 1, "label": "submit", "shape": "rect", "monitor": 0 },
                   "bbox_distance_px": 12.0 } }
  ]
}
```

`ok` is `true` only when every line hit, and the exit code follows it. A
single `--point` run omits `line` entirely — a batch *adds* a field, it
never removes one, so a consumer written against one shape reads the
other.

**A malformed line stops the run.** The error names the line
(`line 7: "12,x" is not a whole number of pixels`), the exit code is 2,
and no document is printed. Scoring the first six points of a trajectory
and stopping would report a pass rate over a prefix, which reads exactly
like a complete run — worse than no answer. Naming a label the session
does not carry is the same kind of mistake and stops the run too. Callers
who want lenient streams should pre-validate.

A stream with no points at all — every line blank or a comment — is also
exit 2 rather than a vacuous pass.

`contained_in` lists every region holding the point in stacking order
(last is topmost) — a miss against `--expect` still shows what the point
*did* land in. `nearest` appears only on misses: the closest relevant
region and the distance in pixels to its rotated bounding box, for
partial-credit scoring. Points are tested with the same geometry the overlay draws —
circle boundaries inclusive, rotated rects as their rotated silhouette,
triangles by their edges rather than their bounding box.

## Click snippets: `emit`

`pixelcoords emit` prints ready-to-paste click code — one click per
selection, aimed at the shape's own center (a rect's center doubles as
its rotation pivot, so rotation never moves it; triangles use their
centroid, which stays interior where a bbox center may not).

```bash
pixelcoords emit --session shots --format pyautogui   # Python
pixelcoords emit --session shots --format cliclick    # macOS shell
pixelcoords emit --session shots --format xdotool     # X11 shell
```

Each format encodes its tool's coordinate convention exactly once — the
place hand-written glue gets silently burned:

| Format | Space | Notes |
|--------|-------|-------|
| `pyautogui` | logical points on macOS, physical pixels on Windows/X11 | pyautogui makes itself DPI-aware on import, hence physical on Windows |
| `cliclick` | logical points | negative coordinates get cliclick's `=` escape |
| `xdotool` | physical pixels | `global_px` verbatim, no conversion |

Logical conversion divides by each selection's own monitor scale, so
mixed-DPI setups come out right per selection. Sessions are
machine-local: run the snippet on the machine and monitor layout that
was captured. The macOS conversions are verified against real hardware;
the Windows pyautogui convention follows its documented behavior and has
not yet been verified on hardware.

## Where do I click: `resolve`

Everything an executor needs already lived in this repo, in pieces each
consumer had to reassemble: the region and its monitor's scale in the
session, the interior point in `click_point`, drift correction in `find`,
and the per-platform unit convention in `emit`. Reassembling them is
where DPI goes wrong. `resolve` is that composition, done once.

```bash
pixelcoords resolve --session shots
pixelcoords resolve --session shots --label submit --units logical
pixelcoords resolve --session shots --space monitor --relocate
```

Exit codes: **0** every label resolved; **1** one could not be — with
`--relocate`, a region that was not found unambiguously; **2** the
question was malformed.

```json
{
  "schema": 2,
  "command": "resolve",
  "captured_utc": "2026-08-01T14:02:11Z",
  "ok": true,
  "results": [
    { "index": 1, "label": "submit", "monitor": 1, "scale": 2.0,
      "space": "global", "units": "logical",
      "point": { "x": 1020, "y": 115 },
      "region": { "x": 1010, "y": 100, "w": 20, "h": 30 },
      "score": 0.998, "delta": { "dx": 0, "dy": -120 } }
  ]
}
```

`captured_utc`, `score`, and `delta` appear only with `--relocate` —
without it nothing was captured, and claiming a capture time would be a
lie about where the answer came from.

What the fields mean, honestly:

- **`units: auto`** resolves to `logical` on macOS and `physical` on
  Windows and X11, matching what those platforms' input APIs take. It is
  the one value most callers want, and the mismatch it hides is the
  single most common way screen coordinates get clicked in the wrong
  place.
- **`scale`** is reported so a consumer can check the conversion instead
  of trusting it. Each selection converts through *its own* monitor's
  scale — there is no desktop-wide logical space, and pretending
  otherwise breaks the moment two displays disagree.
- **`point` in logical units is the physical interior point converted**,
  not an interior point of the converted region. A consumer clicks in
  logical points and the window server maps that back to physical,
  landing inside the region a human marked. Deriving it from the
  rounded-down shape would optimize a number nothing clicks — so `point`
  and `region` may round differently by a pixel on a scaled display.
- **`space: monitor`** needs no monitor index: every row says which
  monitor it is on, and each is answered in that monitor's coordinates.
- **A missing `score` with `--relocate`** means the region was not found,
  or was found in more than one place. The row still reports its stored
  coordinates, and `ok` is false — acting on the wrong instance is worse
  than not acting.

## Waiting for the screen to settle: `wait`

Automation that clicks needs to wait — for a dialog to appear, a spinner
to leave, a state to settle. Every consumer of a session was writing that
loop by hand: capture, compare, sleep, repeat. The primitives were all
here already; this is the missing verb.

```bash
pixelcoords wait --session shots --label dialog --for match
pixelcoords wait --session shots --for change --timeout 2m --interval 1s
```

Exit codes: **0** the condition held; **1** it did not before the budget
ran out; **2** the question was malformed (unreadable session, a display
that changed, a crop that could never match, an unparseable duration).

A timeout is **1**. It is a negative answer, not a broken question.

```json
{
  "schema": 2,
  "command": "wait",
  "captured_utc": "2026-08-01T14:02:41Z",
  "polls": 7,
  "elapsed_ms": 3204,
  "ok": true,
  "results": [
    { "index": 0, "label": "dialog", "monitor": 0,
      "score": 0.997, "matching": true }
  ]
}
```

`match` needs every watched region back; `change` fires on the first that
differs. The report is printed on a timeout too, with each region's final
score — knowing a region reached 0.87 against a 0.9 floor is the
difference between "the UI is still settling" and "that region is gone".

### `--timeout` is a poll budget

It is converted to a poll count before the loop starts, and the loop
counts rather than reading a clock. `30s` at `500ms` is 61 polls: one
immediately, then sixty. **Capture time is not counted**, so the wall
clock exceeds `--timeout` by roughly what the captures cost.

That is a deliberate trade. Against a real deadline, a loaded machine
spends more of the window capturing and therefore gives the UI *fewer*
chances precisely when the UI is slowest — backwards for a
synchronization primitive. A budget gives the same number of chances
everywhere. `polls` and `elapsed_ms` are both reported so the actual cost
is visible rather than inferred.

### What `--for change` cannot see

Correlation is brightness- and contrast-normalized, which is what lets
`find` survive a theme tweak. The same property means a region that
changes *uniformly* — a modal backdrop dimming behind it, display
auto-brightness, a luminance-only theme switch — still scores near 1.0,
and `--for change` will not fire.

This is inherent to the metric, not a bug to route around. A caller who
means "any pixel differs at all" wants `diff --tolerance 0`, which
compares RGB directly.

### If `--for change` fires immediately, lower `--min-score`

The mirror of the above. Correlation is also *disproportionately*
sensitive to a few very high-contrast pixels on a region that is
otherwise low-detail — the same reason a mouse pointer costs about 0.17
of match score on a plain region and nothing at all on a busy one.

A blinking text cursor is the common case. Measured on a real 240×160
region: **101 pixels** out of 38,400 — 0.26% of it — flipping between
near-black and near-white dropped the score from 1.0 to **0.805**. That
is under the default 0.9 floor, so `--for change` fires on the blink
rather than on whatever you were waiting for. It fires within a second,
every time.

`--min-score` is the fix. At `--min-score 0.5`, the same region stops
firing on the cursor and a genuine change still trips it, because a real
change scores far lower than a blink does:

```bash
pixelcoords wait --session shots --for change --min-score 0.5
```

Watching a region with a clock, a spinner, a caret, or a progress
indicator in it? Either lower the floor or mark a region without one.

`--for match` does not need this. It keeps polling, and a blinking
region matches on whichever poll catches it in the state the crop was
saved in — the retry absorbs the flicker that a single `find` would
report as a miss.

## Did this still look right: `diff`

`assert` answers whether a point is inside a region; `find` answers where
a region went. Neither answers whether a region still *looks* the same,
which is visual regression testing — scoped to regions a human marked
rather than to whole screenshots, so a change elsewhere on screen is not
a failure.

```bash
pixelcoords diff --session shots
pixelcoords diff --session shots --against baseline/ --tolerance 0.5
pixelcoords diff --session shots --against ci-artifact.png
```

Exit codes: **0** every region within tolerance; **1** one is over;
**2** the question was malformed (unreadable session, missing crop, a
display that changed since the capture, or an `--against` image whose
size does not match).

```json
{
  "schema": 2,
  "command": "diff",
  "captured_utc": "2026-08-01T14:02:11Z",
  "ok": false,
  "results": [
    { "index": 0, "label": "btn", "monitor": 0,
      "region": { "x": 40, "y": 40, "w": 20, "h": 20 },
      "masked_px": 400, "changed_px": 3,
      "changed_pct": 0.75, "mean_delta": 172.67 }
  ]
}
```

`captured_utc` appears only when the screen was actually captured —
`--against` compares stored artifacts and claims no capture time.

What the numbers mean, honestly:

- **`masked_px`** is the region's own pixel count, and the denominator
  `changed_pct` uses. Reported so the denominator is inspectable rather
  than implied: a circle crop is roughly a fifth transparent, and
  dividing by the crop's *area* would make one `--tolerance` mean a
  different thing for every shape kind.
- **`changed_px`** counts masked pixels where any of R, G, or B differs
  by at least 1. Alpha is the mask, not content — comparing it would test
  the crop's transparency against the capture's opaque 255 and call every
  pixel changed.
- **`mean_delta`** averages the absolute channel difference over the
  pixels that *changed*, so it says how badly, not how widely. It is
  exactly `0.0` on a clean run rather than undefined.
- **`region`** is provenance in the session's own physical pixels, not a
  coordinate to act on. `resolve` answers that question.
- **`--tolerance` is applied to the measurement, not baked into it**, so
  a stored report can be re-judged at a different bar without
  re-capturing.

Shaped selections compare by their true silhouette — the same alpha-mask
rule `find` matches by, so rotated rects, triangles, circles, and concave
polygons all compare by the pixels a human actually marked. `diff` refuses
a display whose resolution or DPI scale changed since the session, for the
same reason `find` does.

## Self-healing coordinates: `find`

A session's coordinates describe one frozen instant; the moment the UI
drifts they are silently stale. Every selection already ships with a
pixel-exact crop, so `find` uses it as a search template: a fresh capture
of each monitor, normalized cross-correlation to locate every crop, and a
JSON report of where each region sits *now*.

```bash
pixelcoords find --session shots
```

Exit codes: **0** every region was found, unambiguously; **1** at least
one was not; **2** the question was malformed (unreadable session,
missing crop file, or a display that changed since the capture).

```json
{
  "schema": 2,
  "command": "find",
  "captured_utc": "2026-07-27T14:02:11Z",
  "ok": true,
  "results": [
    {
      "index": 0, "label": "submit", "monitor": 0,
      "found": true, "ambiguous": false, "score": 0.998,
      "old_px":  { "x": 812, "y": 440, "w": 96, "h": 40 },
      "new_px":  { "x": 812, "y": 320, "w": 96, "h": 40 },
      "new_global_px": { "x": 812, "y": 320, "w": 96, "h": 40 },
      "delta": { "dx": 0, "dy": -120 }
    }
  ]
}
```

What the flags mean, honestly:

- `found: false` — the best match scored below the floor (0.9): the
  region's pixels changed (theme, label, redesign) or it is gone.
  Re-mark it; `find` corrects drift, it does not do computer vision.
- `ambiguous: true` — a second location matched almost as well (five
  identical checkboxes): the region was found but not trusted, and no
  coordinates are handed out. Mark a more distinctive region, or include
  more surrounding context in the selection.
- `reason` — the crop could not be used at all: a flat single-color crop
  matches anywhere rather than somewhere.

Transparent pixels in circle, triangle, and rotated-rect crops are
excluded from matching, so shaped selections relocate by their own pixels
only. `find` refuses to run against a display whose resolution or DPI
scale changed since the session — template matching survives movement,
not rescaling — and searches each selection's own monitor.
