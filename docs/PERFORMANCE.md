# Performance

**What these numbers are.** Timings for looking up coordinates in regions
somebody already marked. That is the whole scope. pixelcoords does not do
open-world grounding — it does not look at a screen and find "the submit
button" — and nothing here should be read as a claim that it does. The
comparison at the bottom is about a different problem, and it is included
because knowing which problem you have is most of choosing a tool.

Every number below carries the machine it was taken on. A number without
a machine attached is not a measurement.

| | |
|---|---|
| Machine | Apple M5 Pro, 24 GB |
| OS | macOS 26.4.1 |
| Toolchain | rustc 1.97.1, `--release` (LTO thin, `codegen-units = 1`) |
| Version | pixelcoords 0.5.3 |
| Taken | 2026-08-02 |

The version is when these were *taken*, not the last version they
describe. 0.6.0 changed configuration plumbing and removed two unused
functions; it touched none of the paths measured below, so re-running
would only re-measure the run-to-run noise. Re-run it yourself if you
want numbers for your own machine — that is what the harnesses are for.

Reproduce with:

```bash
cargo run --release -p pixelcoords-core --example bench   # the math
scripts/bench-cli.sh <session-dir>                        # the round trip
```

Neither is a test or a CI gate. `AGENTS.md` requires tests to be
deterministic, and a clock is the opposite of that.

## The core math

Median of 21 runs, one untimed warm-up. Synthesized inputs: a session
across two monitors at different DPI scales, and a 3024×1964 textured
frame — a real screenshot is neither uniform nor noise, and correlation
against a flat image is not a number anyone wants.

| Operation | Size | Median |
|---|---|---|
| `resolve`, all labels | 1 selection | 84 ns |
| `resolve`, all labels | 40 selections | 2.0 µs |
| `resolve`, all labels | 400 selections | 14.3 µs |
| `resolve`, one label | 400 selections | 2.0 µs |
| `assert`, one point | 400 selections | 2.0 µs |
| `locate` (full-frame NCC) | 48×24 crop | 272 ms |
| `locate` (full-frame NCC) | 160×90 crop | 198 ms |
| `locate` (full-frame NCC) | 400×300 crop | 1.40 s |
| `diff`, one region | 48×24 crop | 1.8 µs |
| `diff`, one region | 160×90 crop | 20.6 µs |
| `diff`, one region | 400×300 crop | 165 µs |

Read the microsecond rows as an order of magnitude, not a constant. A
median of 21 runs still moves 30% between runs at that scale — repeated
measurement put `resolve` over 400 selections at 14µs and 20µs on the
same machine minutes apart. The millisecond rows are stable to within a
percent or two, which is why the `locate` conclusion below is worth
drawing and a 2µs-versus-3µs difference is not.

**`resolve` really is instant.** The README calls it that; it is
microseconds, and it captures nothing.

**`locate` is the expensive one, and it was the number nobody had.**
Normalized cross-correlation searches the whole frame, so `find` and
`wait --for match` pay this per region per poll. A `wait --interval 500ms`
watching one 160×90 region spends about 40% of each interval correlating;
three regions at that size will not keep up with a 500 ms interval, and
the poll budget is what stops it running away.

The 48×24 row is slower than 160×90, which is not a typo and not
monotonic — a smaller template leaves more candidate positions to score.
It is reported as measured rather than explained away.

## The round trip

What a caller actually pays, including process start, session read,
parse, serialize, and print. Against a real captured session (2
selections, 1 measure, 3600×2338 screenshot). Mean of 30 runs; the script
uses `hyperfine` and reports properly, this was taken with a loop because
`hyperfine` was not installed on the machine above.

| Command | Mean |
|---|---|
| `resolve --units auto` | 15.8 ms |
| `assert --point` | 4.2 ms |
| `emit --format pyautogui` | 4.3 ms |

The gap between 2 µs of `assert` math and 4.2 ms of `assert` command is
the trip, not the answer: process start, reading the session, and
printing. That ratio — roughly **2000×** — is the whole case for
`--stdin`, and the next table is it measured.

### What `--stdin` amortizes

`CLI.md` says a thousand `assert` processes pay a thousand session parses
to answer a question the first one already had the data for. Measured,
100 points:

| | Total | Per point |
|---|---|---|
| One process, `--stdin` | 7.8 ms | **0.078 ms** |
| 100 processes, `--point` | 384.8 ms | 3.848 ms |

**49× per point.** The claim holds, and the reason is what it said: the
per-point math is microseconds either way — what disappears is 99 process
starts and 99 session parses.

## What this is not

The numbers above are lookups against regions a human marked. The
alternative approach — hand a screenshot to a vision model and ask it to
find the element — solves a genuinely harder problem, and published
results give a sense of where that stands.

These are **published third-party results, not pixelcoords
measurements.** Nothing here was run by this project.

| Benchmark | Reported | Retrieved |
|---|---|---|
| [ScreenSpot-Pro](https://llm-stats.com/benchmarks/screenspot-pro) — grounding in professional, high-resolution UI, 1,581 expert-annotated screenshots | 0.863 (GPT-5.2, top of leaderboard); target elements average **0.07%** of image area, against 2.01% in mainstream benchmarks | 2026-08-02 |
| [OSWorld](https://arxiv.org/html/2606.29537v1) — original short-horizon benchmark | ~12% (Apr 2024) → ~85% (Jun 2026) | 2026-08-02 |
| [OSWorld 2.0](https://arxiv.org/html/2606.29537v1) — 108 long-horizon workflows, median task ≈1.6 h of skilled human work | Best frontier system completes **20.6%** of tasks (54.8% partial) | 2026-08-02 |

Read that as: grounding a single element from a screenshot is largely
solved on mainstream targets and still hard on small, dense professional
UI; stringing many such steps into a long task is not solved at all.

### The token column, which is arithmetic rather than a measurement

A session lookup sends **zero image tokens**, because it sends no image —
it reads a JSON file on disk. A vision-grounding step sends one
screenshot per step. At the OSWorld 2.0 average of ~318 tool calls per
task, that is ~318 screenshots for a workflow where the coordinates could
have been read from a file.

That is division, not a benchmark, and it is stated as such. It is also
the entire argument for this tool: **if the region is one you can mark
once, marking it once is cheaper than looking at it every time.** If the
region is not one you can mark in advance, this tool is the wrong one and
the numbers above do not apply to your problem.

## Sources

- [ScreenSpot-Pro leaderboard](https://llm-stats.com/benchmarks/screenspot-pro) — retrieved 2026-08-02
- [ScreenSpot-Pro benchmark overview](https://www.emergentmind.com/topics/screenspot-pro-benchmark) — retrieved 2026-08-02
- [OSWorld 2.0: Benchmarking Computer Use Agents on Long-Horizon Real-World Tasks](https://arxiv.org/html/2606.29537v1) — retrieved 2026-08-02
