# Contributing

Thanks for your interest. pixelcoords runs verified on macOS, Windows,
and Linux, but multi-monitor and fractional-scaling setups are still
unverified on real hardware — so the most valuable contributions are
bug reports from real machines and small, focused fixes.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

All four must pass — they are exactly what CI runs. The pure-logic core
crate holds a 90% per-module line coverage floor (`cargo llvm-cov`,
enforced in CI) and its geometry invariants carry property tests; no
warnings or inline lint suppressions are allowed in any commit.

## Bug reports

Open a GitHub issue with:

- OS and version; on Linux also the session type (`echo $XDG_SESSION_TYPE`)
  and desktop environment
- The output of `pixelcoords doctor`
- What you did, what you expected, what happened
- For misbehavior without a crash: rerun with
  `RUST_LOG=debug pixelcoords 2> debug.log` and attach the log

## Pull requests

- For anything larger than a bug fix, open an issue first so we can agree
  on the approach before you invest time.
- Read [AGENTS.md](AGENTS.md) — it is the engineering-standards document
  (layout, control-flow style, coordinate conventions, testing bar,
  definition of done), and CI enforces the mechanical parts of it. PRs
  that follow it get reviewed fast.
- Every change needs tests where tests are possible: the core crate holds
  a 90% line coverage floor per module, headless-testable binary logic is
  tested headless (see the patterns named in AGENTS.md), and every bug
  fix includes a regression test. Window-system plumbing is exempt —
  don't mock it.
- Keep commits focused and describe the why, not just the what.

If you code with an AI assistant, point it at [CLAUDE.md](CLAUDE.md) /
[AGENTS.md](AGENTS.md) — they encode the same standards, and CI will
reject output that ignores them.

Before pushing, run exactly what CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

To check coverage like we do:

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
cargo llvm-cov -p pixelcoords-core --summary-only
```

Scoped to the core crate, as the floor is. Measuring `--workspace` folds
in the binary, which is largely window-system plumbing that is verified on
hardware rather than by unit tests, so the number comes out low against a
floor that was never meant to cover it. `--html` instead of
`--summary-only` writes a browsable report to `target/llvm-cov/html`; CI
uploads that same report as the `coverage-report` artifact on every run.

CI runs the same checks on macOS, Windows, and Linux, plus a Rust 1.88
minimum-version build and a grep that fails on any inline `#[allow]`.

## Platform work

The ports are data-driven: the "Platform spike" workflow captures real
coordinate tables from CI runners, and platform-specific behavior gets
verified on real hardware before it merges. If you want to help with the
Windows or Wayland work, open an issue — testing on real machines is the
bottleneck, not code.
