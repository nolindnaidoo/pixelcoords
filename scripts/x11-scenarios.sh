#!/usr/bin/env bash
# End-to-end scenarios against a real X server.
#
# This is a screen-capture tool with, until now, no test that ever
# captured a screen. Unit tests cover the geometry, the schema and the
# matcher; nothing covered `shoot` producing a real PNG of a real display,
# or `find` locating a region in a fresh capture of one.
#
# The overlay stays out of scope — it is interactive, and `AGENTS.md` says
# so. Everything else is here: capture, the five headless answers, the MCP
# server, and the exit codes.
#
#   xvfb-run -a --server-args="-screen 0 1280x1024x24" scripts/x11-scenarios.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/debug/pixelcoords"
[ -x "$bin" ] || bin="$root/target/release/pixelcoords"
[ -x "$bin" ] || { echo "no pixelcoords binary — cargo build first" >&2; exit 2; }
command -v convert >/dev/null || { echo "ImageMagick is needed to cut a crop" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"; [ -n "${msg_pid:-}" ] && kill "$msg_pid" 2>/dev/null; true' EXIT
export XDG_SESSION_TYPE=x11

pass=0; fail=0
check() { # check <name> <expected> <actual>
  if [ "$2" = "$3" ]; then echo "  ok    $1"; pass=$((pass + 1))
  else echo "  FAIL  $1: expected [$2], got [$3]"; fail=$((fail + 1)); fi
}
json() { python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(eval(sys.argv[2]))" "$1" "$2" 2>/dev/null || echo "unreadable"; }

echo "== something detailed and unique on screen"
# Detail and uniqueness are both required, and the tool refuses each by
# name when it is missing: a flat crop "matches anywhere rather than
# somewhere", and repeating content "matched in more than one place".
# Random hex is dense and never repeats.
lines=""
for _ in $(seq 1 16); do
  lines="$lines$(head -c 26 /dev/urandom | od -An -tx1 | tr -d ' \n')
"
done
xmessage -geometry 900x620+30+30 "$lines" & msg_pid=$!
sleep 2

echo "== doctor sees the display"
"$bin" doctor --json >"$work/doctor.json" 2>/dev/null || true
check "doctor reports one monitor" "1" "$(json "$work/doctor.json" 'len(d["monitors"])')"

echo "== shoot captures the real screen"
"$bin" shoot --out "$work" >/dev/null 2>&1 || true
shot="$work/screenshot-0.png"
[ -f "$shot" ] && wrote=yes || wrote=no
check "a PNG was written" "yes" "$wrote"
if [ "$wrote" = yes ]; then
  dims=$(convert "$shot" -format "%wx%h" info:)
  check "at the display's size" "1280x1024" "$dims"
fi

echo "== a session over a region of that capture"
W=200; H=60; X=0; Y=0; best=-1
for cy in 60 140 220 300 380; do
  for cx in 60 160 260 360 460; do
    dev=$(convert "$shot" -crop "${W}x${H}+${cx}+${cy}" +repage -format "%[fx:standard_deviation]" info: 2>/dev/null || echo 0)
    dev_i=$(printf '%.0f' "$(echo "$dev * 100000" | bc -l 2>/dev/null || echo 0)")
    if [ "${dev_i:-0}" -gt "$best" ]; then best=$dev_i; X=$cx; Y=$cy; fi
  done
done
echo "  most detailed tile at ${X},${Y} (deviation ${best})"
[ "$best" -gt 100 ] || { echo "  FAIL  the capture is flat — no scenario over it is meaningful" >&2; exit 1; }
convert "$shot" -crop "${W}x${H}+${X}+${Y}" +repage "$work/crop-0-target.png"
python3 - "$work" "$X" "$Y" "$W" "$H" <<'SESSION'
import json, sys
work, x, y, w, h = sys.argv[1], *map(int, sys.argv[2:6])
px = {"x": x, "y": y, "w": w, "h": h}
json.dump({
    "schema": 1, "app": {"name": "pixelcoords", "version": "0.7.0"},
    "created_utc": "2026-01-01T00:00:00Z", "platform": "linux",
    "capture": None, "name": "x11 scenarios",
    "monitors": [{"index": 0, "name": "screen", "primary": True,
                  "origin_px": {"x": 0, "y": 0},
                  "size_px": {"w": 1280, "h": 1024}, "scale": 1.0}],
    "target": None, "measures": [],
    "selections": [{"shape": "rect", "label": "target", "monitor": 0,
                    "px": px, "global_px": px, "rot_deg": None,
                    "window_px": None, "crop": "crop-0-target.png",
                    "color": None}],
}, open(f"{work}/session.json", "w"))
SESSION

echo "== find locates it in a fresh capture"
# Exit 1 means "not found" — an answer, not a failure. Letting `set -e`
# treat it as one would break the exit-code contract under test.
"$bin" find --session "$work" >"$work/find.json" 2>/dev/null || true
check "found" "True" "$(json "$work/find.json" 'd["results"][0]["found"]')"
check "unambiguously" "False" "$(json "$work/find.json" 'd["results"][0]["ambiguous"]')"
check "where it was marked" "True" "$(json "$work/find.json" 'd["results"][0].get("delta") in (None, {"dx":0,"dy":0})')"

echo "== resolve answers in this platform's units"
"$bin" resolve --session "$work" --units auto >"$work/resolve.json" 2>/dev/null || true
check "physical pixels on X11" "physical" "$(json "$work/resolve.json" 'd["results"][0]["units"]')"
check "the region's centre" "$((X + W / 2)),$((Y + H / 2))" "$(json "$work/resolve.json" 'str(d["results"][0]["point"]["x"]) + "," + str(d["results"][0]["point"]["y"])')"

echo "== assert scores a point against it"
"$bin" assert --session "$work" --point "$((X + W / 2)),$((Y + H / 2))" --expect target >/dev/null 2>&1 && hit=0 || hit=$?
check "a point inside exits 0" "0" "$hit"
"$bin" assert --session "$work" --point "1279,1023" --expect target >/dev/null 2>&1 && miss=0 || miss=$?
check "a point outside exits 1" "1" "$miss"

echo "== diff compares the region against the screen"
"$bin" diff --session "$work" >"$work/diff.json" 2>/dev/null || true
check "unchanged, within tolerance" "True" "$(json "$work/diff.json" 'd["ok"]')"

echo "== wait returns the instant the condition holds"
"$bin" wait --session "$work" --for match --timeout 5s --interval 200ms >"$work/wait.json" 2>/dev/null || true
check "a matching region satisfies --for match" "True" "$(json "$work/wait.json" 'd["ok"]')"

echo "== emit generates for a named tool"
"$bin" emit --session "$work" --format xdotool >"$work/emit.txt" 2>/dev/null || true
grep -q 'xdotool mousemove' "$work/emit.txt" && emitted=yes || emitted=no
check "xdotool output is runnable" "yes" "$emitted"

echo "== the MCP server answers against a real session"
printf '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"pixelcoords_find","arguments":{"session":"%s"}}}\n' "$work" \
  | "$bin" mcp >"$work/mcp.json" 2>/dev/null || true
check "find over MCP agrees with the CLI" "True" "$(json "$work/mcp.json" 'd["result"]["structuredContent"]["ok"]')"
check "and it is not an error" "False" "$(json "$work/mcp.json" 'd["result"]["isError"]')"

echo "== exit codes are the API"
"$bin" resolve --session "$work" --label nosuchlabel >/dev/null 2>&1 && code=0 || code=$?
check "an unknown label exits 2" "2" "$code"

echo
echo "== $pass passed, $fail failed"
[ "$fail" -eq 0 ]
