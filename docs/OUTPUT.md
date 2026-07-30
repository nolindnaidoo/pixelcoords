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
  "app": { "name": "pixelcoords", "version": "0.3.0" },
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
pixelcoords assert --session shots --point 812,440 --label submit
pixelcoords assert --session shots --point 100,50 --space window
pixelcoords assert --session shots --point 15,25 --space monitor --monitor 1
```

The exit code is the API: **0** the point hit (a region with `--label`'s
label when given, any region otherwise), **1** it missed, **2** the
question was malformed — unreadable session, unknown label, window space
on an untargeted session. `--space` says which stored coordinates the
point is in: `global` (default, `global_px`), `monitor` (`px`; `--monitor`
picks which, optional on single-monitor sessions), or `window`
(`window_px`, `--target` sessions only). Labels match case-insensitively.

Stdout is a JSON verdict, itself versioned:

```json
{
  "schema": 1,
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
```

`contained_in` lists every region holding the point in stacking order
(last is topmost) — a labeled miss still shows what the point *did* land
in. `nearest` appears only on misses: the closest relevant region and the
distance in pixels to its rotated bounding box, for partial-credit
scoring. Points are tested with the same geometry the overlay draws —
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
  "schema": 1,
  "captured_utc": "2026-07-27T14:02:11Z",
  "all_relocated": true,
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
