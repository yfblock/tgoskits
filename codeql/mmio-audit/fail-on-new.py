#!/usr/bin/env python3
"""Fail if any SARIF result from the MMIO audit sits on a line ADDED by this
PR/commit relative to BASE_SHA.

The MMIO audit reports every ad-hoc `read_volatile` / `write_volatile` in the
workspace, including the large pre-existing backlog. We only want CI to block
on access introduced by the change under review, so this script intersects the
result locations with the added-line ranges of `git diff BASE...HEAD`.
"""
import json
import re
import subprocess
import sys


def added_line_ranges(path: str, base: str):
    """Return list of (start, end) line ranges added in `path` vs base."""
    try:
        out = subprocess.run(
            ["git", "diff", f"{base}...HEAD", "--unified=0", "--", path],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except subprocess.CalledProcessError:
        return []
    ranges = []
    for m in re.finditer(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", out, re.M):
        start = int(m.group(1))
        count = int(m.group(2)) if m.group(2) else 1
        if count > 0:
            ranges.append((start, start + count - 1))
    return ranges


def main():
    sarif_path = sys.argv[1]
    base = sys.argv[2]

    with open(sarif_path) as f:
        data = json.load(f)

    results = []
    for run in data.get("runs", []):
        for r in run.get("results", []):
            for loc in r.get("locations", []):
                pl = loc.get("physicalLocation", {})
                uri = pl.get("artifactLocation", {}).get("uri")
                line = pl.get("region", {}).get("startLine")
                if uri and line:
                    results.append((uri, line))

    if not results:
        print("MMIO audit: 0 results")
        sys.exit(0)

    ranges_by_file = {}
    for path, _ in results:
        if path not in ranges_by_file:
            ranges_by_file[path] = added_line_ranges(path, base)

    new_results = []
    for path, line in results:
        for start, end in ranges_by_file.get(path, []):
            if start <= line <= end:
                new_results.append((path, line))
                break

    print(
        f"MMIO audit: {len(results)} total ad-hoc site(s), "
        f"{len(new_results)} newly introduced by this change"
    )
    for path, line in new_results:
        print(f"  NEW: {path}:{line}")

    if new_results:
        print(
            f"::error::{len(new_results)} newly-introduced ad-hoc MMIO "
            f"read/write access site(s) — use mmio_api::MmioRaw::read/write "
            f"instead of read_volatile/write_volatile."
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
