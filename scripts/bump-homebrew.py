#!/usr/bin/env python3
"""Point a Homebrew formula at a new release.

Called by the `homebrew` job in `.github/workflows/release.yml`, and
runnable by hand when a bump has to be redone:

    python3 scripts/bump-homebrew.py 0.7.7 <arm-sha256> <intel-sha256> \
        tap/Formula/pixelcoords.rb

A script rather than a heredoc inside the workflow: this is the part with
logic in it, and logic that only runs during a release is logic nobody
tests. Here it can be run against a checkout of the tap in a second.
"""

import pathlib
import re
import sys

TOOL = "pixelcoords"
BASE = f"https://github.com/nolindnaidoo/{TOOL}/releases/download"
TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
SHA256 = re.compile(r'sha256 "[0-9a-f]{64}"')


def bump(text: str, version: str, digests: dict[str, str]) -> str:
    """Rewrite every URL and checksum for `version`.

    The version appears twice in each URL — once as the tag, once in the
    file name — so the whole URL line is replaced rather than a substring
    patched, which would leave the tag and the file name disagreeing.
    """
    for target in TARGETS:
        pattern = rf'url "{re.escape(BASE)}/v[^"]*{re.escape(target)}\.tar\.gz"'
        replacement = f'url "{BASE}/v{version}/{TOOL}-v{version}-{target}.tar.gz"'
        text, count = re.subn(pattern, replacement, text)
        if count != 1:
            raise SystemExit(f"expected exactly one {target} url, rewrote {count}")

    # Checksums are positional: the formula lists arm first, then intel,
    # in the same order as TARGETS. Asserting the count catches a formula
    # that grew a third platform without this script learning about it.
    ordered = [digests[t] for t in TARGETS]
    found = SHA256.findall(text)
    if len(found) != len(ordered):
        raise SystemExit(f"expected {len(ordered)} sha256 lines, found {len(found)}")
    text = SHA256.sub(lambda _: f'sha256 "{ordered.pop(0)}"', text)
    return text


def main() -> None:
    if len(sys.argv) != 5:
        raise SystemExit(f"usage: {sys.argv[0]} VERSION ARM_SHA INTEL_SHA FORMULA")
    version, arm, intel, formula = sys.argv[1:5]
    for digest in (arm, intel):
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise SystemExit(f"not a sha256: {digest!r}")

    path = pathlib.Path(formula)
    updated = bump(path.read_text(), version, dict(zip(TARGETS, (arm, intel))))
    path.write_text(updated)
    print(f"{formula} -> {version}")


if __name__ == "__main__":
    main()
