#!/usr/bin/env python3
"""Prove a non-host test runner executed exactly the tests its cell claims.

Usage: `cargo test … | python3 .github/ci/wasm_test_count.py PACKAGE TARGET PLATFORM`,
where `TARGET` is `lib` or `test:<name>` and the three words name one cell of
`test-claims.yml`. The runner's output is echoed unchanged. The step fails
unless that output holds exactly one `test result: … N passed; M failed` line
with `N` equal to the cell's `expect_tests` and `M` zero; a missing line, a
second line, a short or long count, and any failure are each refused.

`load_cells` is the one reader of `test-claims.yml`; `verify-manifest` check
20 reads the table through it.
"""

from __future__ import annotations

import os
import re
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from typing import TextIO

import strict_yaml
import yaml

CLAIMS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "test-claims.yml")
_CELL_KEYS = frozenset({"package", "target", "platform", "features", "owner", "expect_tests"})
# The table covers the wasm32 platforms only; any other non-host platform
# needs its own runner proof before a cell can claim it.
_PLATFORM = re.compile(r"wasm32-[a-z0-9_-]+\Z")
_NAME = re.compile(r"[A-Za-z0-9_-]+\Z")
_SUMMARY = re.compile(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;")
# A runner line longer than this is echoed but never read as a summary.
MAX_LINE = 64 * 1024


@dataclass(frozen=True)
class Cell:
    """One (package, test target, platform) cell and the job that runs it.

    `target` is `lib` or `test:<name>`."""

    package: str
    target: str
    platform: str
    features: frozenset[str]
    owner: str
    expect_tests: int

    @property
    def key(self) -> tuple[str, str, str]:
        return (self.package, self.target, self.platform)


def _cell(raw: object, at: str) -> Cell | str:
    if not isinstance(raw, dict) or set(raw) != _CELL_KEYS:
        return f"{at} is not a mapping of exactly {sorted(_CELL_KEYS)}"
    package, target, platform = raw["package"], raw["target"], raw["platform"]
    features, owner, expect = raw["features"], raw["owner"], raw["expect_tests"]
    if not isinstance(package, str) or not _NAME.match(package):
        return f"{at}: package {package!r} is not a package name"
    if target == "lib":
        key = "lib"
    elif isinstance(target, dict) and set(target) == {"test"} and isinstance(target["test"], str) and _NAME.match(target["test"]):
        key = "test:" + target["test"]
    else:
        return f"{at}: target {target!r} is neither `lib` nor `{{test: <name>}}`"
    if not isinstance(platform, str) or not _PLATFORM.match(platform):
        return f"{at}: platform {platform!r} is not a wasm32 triple"
    if not isinstance(features, list) or not all(isinstance(f, str) and _NAME.match(f) for f in features):
        return f"{at}: features {features!r} is not a list of feature names"
    if len(set(features)) != len(features):
        return f"{at}: features {features!r} names a feature twice"
    if not isinstance(owner, str) or not _NAME.match(owner):
        return f"{at}: owner {owner!r} is not a job id"
    if not isinstance(expect, int) or isinstance(expect, bool) or expect < 1:
        return f"{at}: expect_tests {expect!r} is not a positive integer"
    return Cell(package, key, platform, frozenset(features), owner, expect)


def load_cells(path: str = CLAIMS) -> list[Cell] | str:
    """Every cell of the claims table at `path`, or why it is refused. A cell
    listed twice is refused: each has exactly one owner."""
    try:
        with open(path, encoding="utf-8") as f:
            doc = strict_yaml.safe_load(f)
    except (OSError, UnicodeDecodeError, yaml.YAMLError) as e:
        return f"{os.path.basename(path)} is unreadable: {e}"
    raw = doc.get("cells") if isinstance(doc, dict) and set(doc) == {"cells"} else None
    if not isinstance(raw, list) or not raw:
        return f"{os.path.basename(path)} is not a mapping of exactly a non-empty `cells` list"
    cells: list[Cell] = []
    seen: set[tuple[str, str, str]] = set()
    for i, entry in enumerate(raw):
        got = _cell(entry, f"cell {i}")
        if isinstance(got, str):
            return got
        if got.key in seen:
            return f"cell {i}: {' '.join(got.key)} is claimed twice"
        seen.add(got.key)
        cells.append(got)
    return cells


def count(lines: Iterable[str], expect: int, out: TextIO) -> str | None:
    """Echo `lines` to `out`; None when they hold exactly one summary of
    `expect` passed and none failed, else why not."""
    summaries: list[tuple[int, int]] = []
    for line in lines:
        out.write(line)
        if len(line) > MAX_LINE:
            continue
        m = _SUMMARY.search(line)
        if m:
            summaries.append((int(m.group(1)), int(m.group(2))))
    out.flush()
    if not summaries:
        return "the runner printed no `test result:` line"
    if len(summaries) > 1:
        return f"the runner printed {len(summaries)} `test result:` lines; a cell is one test binary"
    passed, failed = summaries[0]
    if failed:
        return f"{failed} test(s) failed"
    if passed != expect:
        return f"{passed} test(s) passed, but the cell claims {expect}"
    return None


def main(argv: list[str], stdin: TextIO, stdout: TextIO, stderr: TextIO, claims: str = CLAIMS) -> int:
    if len(argv) != 3:
        stderr.write("usage: wasm_test_count.py PACKAGE TARGET PLATFORM\n")
        return 2
    cells = load_cells(claims)
    if isinstance(cells, str):
        stderr.write(f"wasm_test_count: {cells}\n")
        return 2
    key = (argv[0], argv[1], argv[2])
    cell = next((c for c in cells if c.key == key), None)
    if cell is None:
        stderr.write(f"wasm_test_count: no cell {' '.join(key)} in test-claims.yml\n")
        return 2
    why = count(stdin, cell.expect_tests, stdout)
    if why is not None:
        stderr.write(f"wasm_test_count: {' '.join(key)}: {why}\n")
        return 1
    stderr.write(f"wasm_test_count: {' '.join(key)}: {cell.expect_tests} passed, as claimed\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:], sys.stdin, sys.stdout, sys.stderr))
