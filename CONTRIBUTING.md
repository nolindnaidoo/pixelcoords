# Contributing

Thanks for your interest. pixelcoords runs verified on macOS, Windows,
and Linux, but not every combination has met real hardware: fractional
scaling is unverified everywhere, multi-monitor outside macOS is
test-only, and no *overlay* run since 0.5.1 has been driven by hand on
any platform. (0.7.0's MCP server was — it is headless, and adds no
overlay code.) The planned feature set is complete, so the most valuable
contributions now are bug reports from real machines and small, focused
fixes — a run on a machine we do not have is worth more than a patch.

## Bug reports

File through the issue form — it asks for what a report needs to be
actionable: OS and version (on Linux, session type and desktop
environment), `pixelcoords --version`, the full `pixelcoords doctor`
output, and what you did / expected / got. For misbehavior without a
crash, attach a rerun with `RUST_LOG=debug pixelcoords 2> debug.log`.

## Pull requests

- For anything larger than a bug fix, open an issue first so we agree
  on the approach before you invest time. Check the README's
  **Non-goals** first — those are settled.
- Read [AGENTS.md](AGENTS.md) — the engineering-standards document
  (layout, control-flow style, coordinate conventions, testing bar,
  definition of done). CI enforces the mechanical parts. PRs that
  follow it get reviewed fast.
- Every change needs tests where tests are possible; every bug fix
  includes a regression test. Window-system plumbing is exempt — don't
  mock it.
- Keep commits focused and describe the why, not just the what.

If you code with an AI assistant, point it at [CLAUDE.md](CLAUDE.md) /
[AGENTS.md](AGENTS.md) — they encode the same standards, and CI will
reject output that ignores them.

## Building, testing, CI, releases

All in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md): source builds per
platform, the workspace tour, the seven CI gates, coverage measurement,
platform-spike verification, and the release process.
