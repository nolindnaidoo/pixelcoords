# pixelcoords-core

The platform-free core of [pixelcoords](https://crates.io/crates/pixelcoords)
— if you want the tool, install that crate; this one is its logic layer,
split out so every line of it can be unit-tested without a window system.

It carries the geometry (shapes, hit-testing, rotation, polygon math),
the selection/undo engine, the versioned session schema, `assert`'s
point verdicts, `emit`'s click-code generators, `find`'s masked template
matching, the CPU rasterizer and embedded JetBrains Mono (OFL 1.1), the
hotkey grammar, and the strict config parser. `#![forbid(unsafe_code)]`,
no platform dependencies, 90% per-module test coverage enforced in CI.

The API exists to serve the pixelcoords binary and changes with it — it
is not a stability-promised general-purpose library. If you are
consuming `session.json` from another language, you do not need this
crate: the schema is documented in the repository's
[docs/OUTPUT.md](https://github.com/nolindnaidoo/pixelcoords/blob/main/docs/OUTPUT.md).

MIT licensed. Bundled font: JetBrains Mono, OFL 1.1.
