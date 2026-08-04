# Changelog

All notable changes are recorded here, written as they land. Versions
follow [Semantic Versioning](https://semver.org): **minor** (0.x.0) for
new features and for any breaking change to the CLI or the session
schema; **patch** (0.x.y) for fixes. The version keeps incrementing
through 0.x — there is no 1.0 planned, so read the entry below, not the
version number, for what changed under you.

## 0.7.2 — 2026-08-04

**"1 selections."** The resume picker could not count, and a test asserted
that it could not — the string was pinned in place by
`resume_resolution_orders_names_and_rejects_strangers`.

The MCP tools had the same fault in its form-letter dress: `1 region(s)
resolved`, `1 point(s) hit`, `1 poll(s)`, `1 session(s)`. Those are what a
model reads back after asking where to click.

Counts are written in one place now — `strings::count` — and read `1
region` or `n regions` everywhere they appear: the resume picker, the
dropped-selection warning, and all six MCP tool summaries.

## 0.7.1 — 2026-08-04

### MCP callers are no longer told to fix flags they never used

The MCP tools call the same `run_*` functions the subcommands do —
deliberately, so the two surfaces cannot answer differently. But those
functions refuse in the CLI's vocabulary, and five of those refusals were
reachable from a tool call.

A caller passing `min_score` was told `--min-score is a correlation score
in 0..=1 (diff's --tolerance is the percentage…)`: two command-line flags,
neither of which exists in the argument surface it was using. A model
reading that may relay it to someone who is not at a command line, or try
passing `--tolerance` as an argument name.

`min_score` and `tolerance` are now bounded in `mcp.rs`, in the argument's
own name and **before the session is read**, so a caller learns what is
wrong with their argument rather than what is wrong with their path. That
is the pattern `timeout_secs` and `interval_ms` already used.

Two more live on paths both surfaces reach, so they name the concept
rather than one spelling of it: monitor-space ambiguity, and comparing
against a single image. The CLI keeps its flag names everywhere they are
the right word.

Tests assert a refusal does **not** contain `--`. The leak is the bug, so
the check is for its absence.

### The capture path is tested against a real screen, on every platform

A screen-capture tool had no test that ever captured a screen. Unit tests
covered the geometry, the schema and the matcher; nothing covered `shoot`
producing a PNG of a real display, or `find` locating a region in a fresh
capture of one.

`crates/pixelcoords/tests/scenarios.rs` runs on every push against a real
display on **macOS, Windows and Linux** — all three runners turn out to
have one, macOS without a permission prompt and Windows with a real
desktop. It captures, marks the most detailed region of what it found,
and drives `find`, `resolve`, `assert`, `diff`, `wait`, `emit`, the MCP
server and the exit codes against it.

Windows matters most of the three: it has the least hand-verification
behind it, and now the capture path there is checked on every push.

Opt-in via `PIXELCOORDS_SCENARIOS=1`, because they capture the screen of
whatever machine runs them and `AGENTS.md` requires the ordinary suite to
be deterministic. Single-threaded, because concurrent captures of one
display fail on macOS. The overlay stays out — it is interactive.

Sixteen scenarios: the nine above plus `windows` (which must either list
or say why it cannot — Wayland withholds window geometry and refusing is
the right answer), `rename` round-tripping, `assert --stdin` scoring a
trajectory where one miss makes the run a miss without poisoning the other
rows, `wait --for change` timing out at exit **1** rather than 2,
`diff --against` the session's own capture finding nothing changed, and
**every shape kind** — rect, circle, ellipse, triangle, poly, freehand,
plus a rotated one — resolving to a point the shape actually contains.

That last one is checked by asking `assert`, so the two commands are held
against each other rather than against arithmetic written in the test.

One assertion is worth naming: a capture must be the display's **logical**
size times its scale. `doctor` reports logical and a capture is physical,
and conflating those is the entire class of bug this tool exists to
prevent.

## 0.7.0

### `pixelcoords mcp` — the agent surface, served to the agent

`assert`, `resolve`, `find`, `wait`, and `diff` were built on the premise
that the real consumer of a coordinate is a machine. Reaching them still
took a human: someone wired a subprocess call, remembered the flag
spellings, and parsed stdout. The new subcommand serves them over the
Model Context Protocol on stdio, so a model calls them directly.

```json
{
  "mcpServers": {
    "pixelcoords": { "command": "pixelcoords", "args": ["mcp"] }
  }
}
```

Six tools — `pixelcoords_sessions`, `_resolve`, `_assert`, `_wait`,
`_find`, `_diff` — each calling the same function as the subcommand it
is named for. No command changed to make this work; the split that keeps
`run_*` returning a report and printing nothing was already there.

**It is read-only.** Marking regions is interactive, so no tool opens the
overlay, renames a session, or edits one. The shape is *mark once, run
many*: a human saves a session, and from then on the model asks about it.
`pixelcoords_sessions` is the one tool with no subcommand behind it — a
model has to discover what exists before it can ask, and it needs the
labels as an array rather than the prose line the resume picker shows.

**A negative answer is not an error.** Exit 0 and exit 1 both come back
with `isError: false`; the answer is in `structuredContent.ok`, and the
report inside it is the same envelope the CLI prints, same `schema`
counter. Only exit 2 — a malformed question — is a JSON-RPC error. The
distinction is the whole reason this family reports misses as data: a
model that reads a miss as a broken tool retries the call instead of
reacting to it.

Every tool's description says whether it captures the screen, because on
macOS that is the difference between an instant answer and a permission
prompt. `resolve` without `relocate` and `assert` are pure session math —
a file read, microseconds, no image sent — and their descriptions say to
prefer them.

`wait` blocks the server while it polls, since stdio serves one client;
over MCP its timeout is capped at 120 seconds so a model cannot park the
connection. The CLI is uncapped as before.

Protocol revision `2026-07-28`, `2025-11-25` also accepted. Stateless, so
no `initialize` handshake is needed — one is still answered for clients
that send it. No new dependency and no async runtime: a conformant stdio
server is a read loop and a `match`.

### Also in this release

Three documentation fixes that landed after 0.6.0 was tagged: the
platform table now says outright what has not been driven by hand since
0.5.1, three documents that disagreed about the same gap now agree, and
the release checklist grew the five steps that four releases taught it.

Both crates also carry new crates.io keywords and categories. Nothing in
either crate's API or behavior changes — it is how they are found.

## 0.6.0

### Breaking: two unused items leave `pixelcoords-core`

`Snap::is_hit` and `Report::polled` are gone. Neither had a caller —
`wait` sets `polls` and `elapsed_ms` on the fields directly, and the
overlay reads `Snap`'s axes rather than asking the convenience question.
Both are one expression to write inline if you were using them.

Small, but a break: removing a public item from a published crate is one
whatever its size, and that is what makes this release a minor rather
than a patch.

### `R` mid-drag refuses instead of quitting

`Release::Quit` meant two unrelated things — "this is the last frame, so
end the run" and "a gesture is in flight". A release arriving mid-drag
took the second path into the first and ended the session, unsaved marks
included.

It was unreachable from the keyboard: `allowed_mid_gesture` lets nothing
but `Quit` through while the mouse is down. But the safety lived in a
gate in a different function with nothing connecting the two, and quitting
was the wrong half of a false choice — refusing renumbers nothing *and*
keeps the marks. Now it refuses, with the same flash a frame still
holding marks already gave.

### The overlay's numbers are yours now

Seven values that were constants in the source are configuration, each
defaulting to exactly what it was — an absent table behaves as before, so
a config written yesterday still means the same thing.

```toml
[limits]
label_length = 64          # characters

[overlay]
polygon_sides = 6          # sides the polygon tool opens on
grab_tolerance = 6         # logical px, scaled per monitor
loupe_radius = 15          # the magnifier shows a 2r+1 pixel square
flash_ms = 2500            # saves and errors
flash_brief_ms = 1200      # tool switches
caret_blink_ms = 500       # 0 stops the blink
```

**`polygon_sides` is the one that adds reach rather than taste.** The
digit keys stop at 9 because that is what a single keypress can say, so
until now a 24-gon was unreachable however much you wanted one. Set it
here and the tool opens on it.

**Two of these are accessibility rather than preference.**
`caret_blink_ms = 0` stops the blink and leaves the caret solid — a
supported value, not an accident of the arithmetic. And 1.2 seconds is
not long enough for everyone to read a tool-switch message, which is what
`flash_brief_ms` is for.

Out-of-range values are errors naming the field, checked when the config
loads, like every other table.

`label_length`'s ceiling moved from 64 to **80**, and the number is now
derived rather than picked. A label becomes `crop-<index>-<slug>.png`,
the slug is one ASCII byte per character, `char::to_lowercase` can turn
one character into three, and filesystems stop at 255 bytes. That
arithmetic gives 80, so 80 is what the schema accepts and 64 is what the
default lets you type.

## 0.5.3

### The speed claims are measured now

The README calls `resolve` instant and `CLI.md` says `--stdin` exists
because a thousand `assert` processes pay a thousand session parses. Both
were true and neither had ever been measured.
[PERFORMANCE.md](docs/PERFORMANCE.md) carries the numbers, stamped with
the machine and date they were taken on, plus the two harnesses that
produce them — a core example and a `hyperfine` script. Neither is a test
or a CI gate; a clock in CI is a flaky job.

`resolve` is microseconds and `--stdin` is 49x per point, so both claims
hold. The number nobody had is `locate`: a full-frame normalized
cross-correlation is 196ms for a 160x90 crop against a 3024x1964 frame,
which is what `find` and `wait --for match` pay per region per poll. That
is worth knowing before setting `--interval`.

The page also states plainly what the tool is not. These are lookups
against regions somebody already marked, not open-world grounding, and it
cites published results for the harder problem rather than pretending the
comparison does not exist.

### A click point no longer scans the screen to find itself

`click_point` answers "where would automation click this region", and for
a concave shape — any freehand stroke that curves back on itself — the
vertex average lands in the hollow rather than on the shape. The fallback
was a raster scan of the whole bounding box, testing pixel after pixel
until one landed inside: correct, and **O(width x height x vertices)**.

It now cuts the shape with a horizontal line and takes the middle of the
widest slice. Any horizontal line strictly inside a polygon's bounding
box crosses its boundary an even number of times, so one line is normally
enough; a few more are tried for degenerate rows where a line grazes a
vertex.

Measured on a chevron, worst case for the old scan:

| Shape width | Before | After |
|---|---|---|
| 7,680 px | 252µs | 208ns |
| 1,000,000 px | 24ms | 208ns |

The cost no longer grows with the shape's size at all, only with its
vertex count.

**This can move a click point.** For a concave shape the returned point
is still guaranteed inside, but it need not be the same pixel the scan
would have found — the scan returned the first covered pixel in
top-to-bottom order, and this returns the middle of a horizontal slice.
Convex shapes, and any shape whose vertex average is already inside, are
unaffected. If you recorded a click point from a concave freehand
selection and compare it against a fresh `resolve`, expect a different —
still interior — coordinate.

## 0.5.2

### Nothing happens silently

Three places did something without telling you, and one of them was not
doing it at all.

**Shift mid-gesture only reached resizing.** The function that re-applies
the modifier said it existed "so the shape follows the modifier without
the cursor having to move" — and then handled one gesture and returned
early for the rest, with no repaint. Hold Shift while drawing an ellipse
expecting a circle, or while dragging a measure endpoint expecting a 45°
snap, and nothing happened until you moved the mouse a pixel. Every
gesture that reads the modifier now answers to it.

That also removed a duplicate: the function carried its own copy of the
resize math and passed the raw cursor where every other resize passes the
snapped one, so pressing Shift mid-resize used to jump the shape off the
edge it had snapped to.

**Typing past a label's 64 characters did nothing at all** — no
character, no message, no reason. The cap is right; a label becomes part
of a crop's filename and filesystems have opinions about length. It now
says so when it turns a keystroke away.

It was also enforced only where you type. A session read from disk
skipped it, so an edited file with a long label sailed in and failed at
the filesystem on the next save, which is a confusing place to learn
about a limit. Both paths hold it now.

**Regular polygons clamped at 12 sides**, which nothing documented and
nothing could reach — the digit keys stop at 9, so 10 through 12 were a
dead range, and asking the library for a 100-gon returned a dodecagon
with no indication why. The bound is 1000 now and explains itself.

### Limits and resources, written down

Undo history, selection count, and session size are unbounded on purpose:
a wall you hit mid-session is worse than memory you chose to spend. That
was true before and nowhere stated, which makes it indistinguishable from
an oversight. [CONFIGURATION.md](docs/CONFIGURATION.md#limits-and-resources)
now lists what is unbounded and what each ceiling costs — a snap radius
is a real O(radius²) cost curve, a thickness bound is typo protection,
and the doc says which is which.

### Under the hood

The keyboard layer had no test coverage — not because the logic needed a
window, but because it sat behind a winit type a test cannot construct.
Decoding now happens in one place that makes no decisions, and the
decisions happen somewhere a test can reach. No behavior changed; all 160
existing tests passed unmodified before a single new one was written, and
there are now 24 more covering what was dark. The Shift bug was hiding in
that dark.

## 0.5.1

### Untrusted input cannot crash or lie

Three places assumed the shape of data they did not produce. All three
are corrected the same way — ask before trusting, and refuse by name
rather than reading on.

**A session with `"scale": 0` used to answer confidently and wrongly.**
Scale is a divisor; zero produced `inf`, which saturates to `2147483647`
on the way back into an integer, and `resolve` reported that as a click
point with `ok: true` and exit 0. A script consuming it would have
clicked there. Negative and non-finite scales were wrong more quietly.

**Extreme coordinates used to panic.** A session carrying values near the
integer limits reached the geometry and overflowed a subtraction, exiting
101 through every command that reads a session.

Both now exit **2** — the documented "malformed question" — with a
message naming the field and the value. Sessions are checked once where
they are read, so every command and `doctor` refuse the same file, which
is the rule the config file already followed. `session.json` gains no
fields; the rules it was always expected to satisfy are now written down
in [OUTPUT.md](docs/OUTPUT.md) and enforced.

**Six arithmetic sites widened one operation too late.** `(bx - ax) as
i64` subtracts in the narrow type and then widens, which is the bug the
cast was reaching for in the first place. They widen first now. Where
every value must stay an integer there is nowhere wider to go, so those
saturate instead; `length` and `angle_deg` stopped routing through the
integer delta entirely and are exact at inputs where it cannot be.

Rather than make the geometry total over every possible integer — which
means saturating everywhere, and a saturated coordinate is exactly the
confidently-wrong number this project would rather crash than produce —
the module now documents its domain: ±1,000,000, an order of magnitude
past the widest desktop anyone assembles. Inside it nothing panics and
everything terminates, and a property test holds the whole public surface
to that.

**The macOS capture assumed 32-bit BGRA without asking.** It is what
every tested display returns, but an HDR panel can return 64-bit pixels,
and this reader would have taken the left half of every row and produced
a plausible, silently wrong screenshot — the one failure that would
poison every coordinate downstream while looking fine. The layout is
checked now, and an unexpected one is refused rather than misread.

### Also

`pixelcoords` exits **101** when it panics. That is Rust's number, not a
fourth answer, and [CLI.md](docs/CLI.md) now says so — a script switching
on 0/1/2 should treat anything else as a crash, because a panic means no
answer was produced at all.

## 0.5.0

### Marks land on edges

The last few pixels of a precise mark used to be done by eye — the loupe
and the arrow keys got you there, but you did the aiming. Now the point
you are placing is pulled onto the UI edges already present in the frozen
screenshot: drag a rect roughly around a button and it lands on the
button, exactly, in one gesture.

A frozen screen is the right substrate for this. The image cannot change
under the detector, so a snap is reproducible, and there is no live
element to fight. Detection is pixels only — a luma gradient, no
accessibility tree, no toolkit introspection — so it works identically on
a native app, a game, and a screenshot of either.

Two decisions are worth knowing because you can feel both. The threshold
**adapts to each frame** rather than being an absolute number: a cut
tuned on a light theme finds nothing on a dark one. And a boundary sits
on the **first pixel of the new region**, so snapping both sides of a
40-pixel button gives 20 and 60 and the rect between them is 40 wide —
not 39. When a snap happens the overlay draws the edge that captured the
point, along its detected extent, because seeing *what* caught you is the
difference between trusting the placement and wondering why the pointer
moved.

`X` toggles snapping for the run and the control panel's `X` row shows
the state. It does not persist — `[snap]` in the config owns the default,
with `enabled` and a `radius` in logical pixels so one setting behaves the
same on a Retina panel and a 1x one.

**Arrow-key nudging is never snapped.** Arrows mean exactly one pixel;
a feature that quietly overrode them would make the one exact tool in the
overlay inexact. Freehand strokes are not snapped either — a path of
snapped points is a jagged path.

Config values are now range-checked when the file loads rather than when
the feature is first used, so `pixelcoords doctor --config` refuses
exactly what a launch would. A file that looks valid until you reach the
setting it breaks is a file that lies.

### A ruler on the screen

`W` now cycles to a sixth tool that draws a measurement instead of a
region. Drag out a ruler and it captions itself live —
Δ30,40 · 50px · 53° — with `Shift` snapping to the eight 45°
directions. Grab an endpoint to re-aim it, its middle to move it whole.
`A` labels it, `D` deletes it, and `Z` undoes it off the same stack the
shapes use, so a mixed session undoes in the order you actually worked.

Measurements are not selections and do not pretend to be. A ruler marks
a distance, not a region, so it produces no crop and no cutout, and it
serializes into a **top-level `measures` array** beside `selections`
rather than inside it. Each record carries both endpoints in `px` and
`global_px`, plus `length_px`, `dx`, `dy`, and `angle_deg` — all
derivable from the endpoints, stored so a consumer reads the number off
the ruler instead of reimplementing the geometry, and so the overlay and
the file can never disagree about which way the angle turns. It is
clockwise from +X, because screen Y grows downward.

The array is omitted when empty. The schema stays 1: a session without
measures is byte-identical to one written before the tool existed.

### The pixel under the cursor, read out and recorded

Every tool in this one's comparison set shows the color under the
cursor; this one did not. A frozen screen makes it trivial — the pixel
cannot change while you read it — and the cursor chip now carries it:
`1234, 567  #3A7BD5`. Holding `M` puts the same hex under the loupe,
where at that zoom *which* pixel it describes is finally unambiguous.

It is more than a readout, which is the reason to bother. Every
selection records the color at its **click point** — the same interior
point `assert` and `emit` aim at — so `session.json` now says what was
on the pixel automation will actually click. A consumer can check that
the button was still blue when the region was marked without any new
tooling. `assert --color` becomes possible later precisely because the
value is recorded now; it is not in this change.

Both readouts sample the frozen capture rather than the composed frame.
By the time the chip is drawn, outlines and captions are already painted
over the image, and sampling that would report the color of the chrome
sitting on the pixel instead of the pixel. There is a test that parks the
cursor under a selection's own outline and fails if the reported color is
the outline's.

The recorded value is resampled on every save rather than remembered,
which is what keeps a resumed session honest: the frames are the
session's own screenshots, so a selection that moved gets the color it
moved onto and one that did not gets the identical byte back. A region
hanging off an edge, whose click point lands outside the frame, records
no color rather than the nearest one.

`color` is optional and additive — the schema stays 1, old sessions load
unchanged, and consumers that do not know the field ignore it.

### Three more `emit` targets: powershell, applescript, ydotool

`emit` spoke pyautogui, cliclick, and xdotool — Python, macOS shell, X11
shell. That left three real gaps: Windows without Python, macOS without
Homebrew, and Wayland at all, where `xdotool` simply cannot reach.

**`powershell`** uses `SetCursorPos` and `mouse_event` through an
`Add-Type` P/Invoke preamble, so it needs nothing installed. Physical
pixels: the Win32 cursor APIs speak physical on a per-monitor-DPI-aware
process, which is what the session records on Windows. The preamble is
emitted once rather than per click — pasting `Add-Type` twice for the
same type is an error, not a no-op, so a per-click preamble would break
on the second selection.

**`applescript`** uses System Events, so it needs nothing installed
either. Logical points, like cliclick. One `tell` block wraps every
click, and the snippet says in a comment that System Events needs
Accessibility permission, because finding that out from a silently
ineffective click is worse.

**`ydotool`** completes the `--pick` story. Physical pixels: ydotool
writes to a uinput device below the compositor, so it addresses the raw
device grid. The header says the `ydotoold` daemon has to be running
rather than pretending the snippet is turnkey.

Each converts through *its own* selection's monitor scale, so one snippet
off a mixed-DPI desktop has the scale-1 selection unmoved and the scale-2
one halved — checked by a test that runs the same two-monitor session
through every format.

No `json` target, though the original plan had one. `resolve` shipped in
0.4.0 and *is* that output; a `json` emit target would be the same
document under a second name, on the command whose whole remit is code a
human pastes.

## 0.4.0

### `wait`: block until the screen settles

Automation that clicks needs to wait — for a dialog to appear, a spinner
to leave, a state to settle — and every consumer of a session was writing
that loop by hand: capture, compare, sleep, repeat. The primitives were
already here. This is the verb.

```
pixelcoords wait --session shots --label dialog --for match
```

`match` needs every watched region back; `change` fires on the first that
differs. The report prints on a timeout too, with each region's final
score: knowing a region reached 0.87 against a 0.9 floor is the
difference between "still settling" and "gone". A timeout exits **1**,
not 2 — it is a negative answer, not a broken question.

**`--timeout` is a poll budget, not a deadline.** It becomes a poll count
before the loop starts, so the loop counts instead of consulting a clock.
`30s` at `500ms` is 61 polls, and capture time is not charged against it,
so the wall clock runs longer by what the captures cost. That is the
trade: against a real deadline, a loaded machine spends more of the window
capturing and gives the UI *fewer* chances exactly when the UI is slowest.
`polls` and `elapsed_ms` are both reported, so the real cost is visible
rather than inferred.

Polling scores at the region's recorded location instead of re-scanning
the frame to rediscover where it already is. That needed a new public
primitive — `locate::TemplateStats`, with `prepare` and a bounds-checked
`score_at`. Preparing it before the loop is also where a crop that could
never match is refused: a flat, featureless crop correlates with
everything, and finding that out on poll sixty means having burned the
whole timeout first.

One thing worth knowing before reaching for `--for change`: correlation is
brightness-normalized, which is what lets `find` survive a theme tweak.
The same property means a region that changes *uniformly* — a dimming
backdrop, auto-brightness — still scores near 1.0 and will not fire it.
That is inherent to the metric; `diff --tolerance 0` is the answer when
"any pixel differs" is what you mean.

Durations are one integer and one unit: `500ms`, `30s`, `2m`. A bare `30`
is refused rather than assumed, because it reads as seconds to one person
and milliseconds to another. `--min-score` is a correlation in `0..=1` and
is deliberately not spelled `--tolerance`, which is `diff`'s percentage of
pixels — different quantity, different direction, different default.

`--min-score` also turns out to be the answer to the one rough edge found
while verifying this on hardware: a blinking text cursor inside a watched
region will fire `--for change` on its own. Correlation weighs a few
very high-contrast pixels heavily when the rest of the region is plain —
101 pixels out of 38,400 measured at 0.805, under the 0.9 floor. Lowering
the floor ignores the cursor and still catches a real change. Documented
with the numbers in `docs/OUTPUT.md`; `--for match` is unaffected, because
polling absorbs the flicker.

`polls` and `elapsed_ms` are new optional fields on the shared report
envelope, absent from every command that does not loop. They are
provenance in the same sense `captured_utc` is: how the answer was
obtained, never what it is.

### `diff`: did my regions still look right

`assert` answers whether a point is inside a region and `find` answers
where a region went. Neither answers whether a region still *looks* the
same, which is visual regression testing — and none of the tools in this
space do it over saved, shape-aware regions.

```
pixelcoords diff --session shots --against baseline/ --tolerance 0.5
```

Scoped to regions a human marked rather than whole screenshots, so a
change elsewhere on screen is not a failure. Shaped selections compare by
their true silhouette for free: a saved crop already carries its shape in
its alpha channel, which is the same rule `find` matches by. That rule is
now `locate::MASK_ALPHA_FLOOR` rather than a threshold written out twice,
because two definitions of "inside the shape" could drift apart silently.

`--tolerance` is a percentage of a region's **masked** pixels, so one
number means the same thing for a rect and for a circle — dividing by the
crop's area instead would make a circle's bar a fifth looser than a
rect's. It defaults to exact: anti-aliasing makes a small nonzero value
practical in CI, but a diff tool that rounds by default is lying by
default. The bar is applied to the measurement rather than baked into it,
so a stored report can be re-judged without re-capturing.

`--against` compares stored artifacts instead of capturing — offline, no
permission, runs in CI. An image whose dimensions do not match the
session's monitor is refused rather than compared, because the
measurement would describe the resize and not the UI.

`mean_delta` averages over the pixels that changed, so it says how badly
rather than how widely. A clean run reports exactly `0.0`: the average of
nothing is undefined, and shipping NaN would make every passing run fail
to serialize.

### `resolve`: where to act for a label, in the units your API speaks

Everything an executor needs already lived here, in pieces each consumer
had to reassemble: the region and its monitor's scale in the session, the
interior point in `click_point`, drift correction in `find`, and the
per-platform unit convention baked into `emit`'s snippets. A consumer
that wanted the one thing every consumer wants — the point to act on,
right now, in the space its API speaks — had to call `find`, parse a
bbox, link this crate for the click point, then redo a DPI conversion
`emit` already knew how to do.

Every reassembly is a chance to get DPI wrong, and it pushes geometry
into consumers, which is what `pixelcoords-core` exists to prevent.

```
pixelcoords resolve --session shots --label submit --units auto
```

`--units auto` is the flag's reason to exist: logical points on macOS,
physical pixels on Windows and X11. Each selection converts through *its
own* monitor's scale, so a mixed-DPI desktop comes out right without the
caller knowing it was mixed. `scale` is reported alongside, so the
conversion can be checked rather than trusted.

Without `--relocate` it is pure session math — headless, instant, no
capture and no screen-recording permission, so it runs in CI. With it,
one capture per monitor serves every label, drift is applied in physical
pixels before the units convert, and a region found in two places comes
back with no score and takes `ok` false: acting on the wrong instance is
worse than not acting.

`--space monitor` needs no `--monitor` index here, unlike `assert`. Every
row says which monitor it belongs to, and each is answered in that
monitor's own coordinates — there is nothing for an index to
disambiguate.

`emit` is unchanged and stays what it is: ready-to-paste code for humans.
`resolve` is the machine answer underneath it, which is why the `json`
emit target that was once planned is not being built.

### `assert --stdin` scores a whole trajectory in one process

Scoring an agent run meant one `assert` process per click: a process
spawn, a session read, and a JSON parse multiplied by every point, with
the caller stitching the results back together. The session was already
loaded by the first one.

`--stdin` reads points instead — `X,Y` or `X,Y,label`, one per line,
blank lines and `#` comments skipped — and answers in a single report
whose rows are in input order. A thousand points score in one process.

Each row carries its 1-based `line`, counted over *input* lines rather
than scored points, so a reported line matches the file you wrote. A
line's own label overrides `--expect` for that line only, so a stream can
score a run that clicks different things. Only the first two commas are
structural, so a label can contain commas without quoting.

A single `--point` run omits `line` entirely: a batch adds a field and
never removes one, so a consumer written against either shape reads the
other.

Malformed input stops the run naming the line, prints nothing, and exits
2 — for a malformed line, and for a line naming a region the session does
not have. Scoring the first six points of a trajectory and stopping would
report a pass rate over a prefix, which is indistinguishable from a
complete run. A stream with no points in it at all is refused for the
same reason, rather than passing vacuously.

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

### Two `find` messages, said the way the other commands say them

Four commands now share one loader, so `find`'s wording had to stop being
its own. Its empty-session message drops "to find", and its unknown-label
list is deduplicated case-insensitively the way `emit`'s and `assert`'s
always were. Neither changes an exit code or a document.

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
