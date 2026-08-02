#!/usr/bin/env bash
# End-to-end CLI timings: what a caller actually pays, including process
# start, session read, serialize, and print. The core example next door
# times the math; this times the trip.
#
#   scripts/bench-cli.sh <session-dir>
#
# hyperfine is a prerequisite of this script, not a dependency of the
# workspace — nothing in CI runs it. Install it however you install
# things, or read the numbers from docs/PERFORMANCE.md.
#
# Not a regression gate. A timing harness in CI is a flaky job.
set -euo pipefail

session="${1:-}"
if [ -z "$session" ] || [ ! -e "$session" ]; then
  echo "usage: $0 <session-dir-or-session.json>" >&2
  echo "a session pixelcoords wrote: run the overlay, mark something, press S" >&2
  exit 2
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine is not installed — it is what this script measures with." >&2
  echo "https://github.com/sharkdp/hyperfine" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/release/pixelcoords"
if [ ! -x "$bin" ]; then
  echo "building release binary first" >&2
  cargo build --release --manifest-path "$root/Cargo.toml"
fi

# The click point of the first selection, so `assert` is asking a
# question with a known answer rather than measuring a miss.
read -r px py <<<"$(
  python3 - "$session" <<'PY'
import json, os, sys
path = sys.argv[1]
if os.path.isdir(path):
    path = os.path.join(path, "session.json")
s = json.load(open(path))
r = s["selections"][0]["global_px"]
print(r["x"] + r["w"] // 2, r["y"] + r["h"] // 2)
PY
)"

points="$(mktemp)"
trap 'rm -f "$points"' EXIT
for _ in $(seq 1 100); do echo "$px,$py"; done >"$points"

echo "== the headless answers =="
hyperfine --warmup 3 --shell=none \
  -n "resolve --units auto" "$bin resolve --session $session --units auto" \
  -n "assert --point"       "$bin assert --session $session --point $px,$py" \
  -n "emit --format pyautogui" "$bin emit --session $session --format pyautogui"

echo
echo "== what --stdin amortizes =="
# 100 points through one process, against 100 processes. The claim being
# measured is that the second pays 100 session parses to answer what the
# first answered from one.
hyperfine --warmup 2 --shell=none \
  -n "assert --stdin (100 points, 1 process)" \
  "sh -c '$bin assert --session $session --stdin < $points'"
hyperfine --warmup 1 --runs 5 \
  -n "assert --point x100 (100 processes)" \
  "sh -c 'for i in \$(seq 1 100); do $bin assert --session $session --point $px,$py >/dev/null; done'"
