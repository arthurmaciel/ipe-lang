#!/usr/bin/env python3
"""Tests for `wasm_test_count`: the claims-table reader and the runner count proof."""

from __future__ import annotations

import io
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import wasm_test_count  # noqa: E402

_CELL = """  - package: p
    target: {test: t}
    platform: wasm32-unknown-unknown
    features: [a]
    owner: j
    expect_tests: 2
"""
_OK = "running 2 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored\n"


def _table(body: str) -> str:
    fd, path = tempfile.mkstemp(suffix=".yml")
    with os.fdopen(fd, "w") as f:
        f.write(body)
    return path


def _run(argv: list[str], stdin: str, table: str = "cells:\n" + _CELL) -> tuple[int, str, str]:
    path = _table(table)
    try:
        out, err = io.StringIO(), io.StringIO()
        code = wasm_test_count.main(argv, io.StringIO(stdin), out, err, claims=path)
        return code, out.getvalue(), err.getvalue()
    finally:
        os.unlink(path)


_ARGS = ["p", "test:t", "wasm32-unknown-unknown"]


class TestCount(unittest.TestCase):
    def test_exact_count_passes_and_echoes(self) -> None:
        code, out, err = _run(_ARGS, _OK)
        self.assertEqual(code, 0, err)
        self.assertEqual(out, _OK)
        self.assertIn("2 passed, as claimed", err)

    def test_refusals(self) -> None:
        for name, stdin, needle in (
            ("zero", "test result: ok. 0 passed; 0 failed; 0 ignored\n", "0 test(s) passed, but the cell claims 2"),
            ("missing", "running 2 tests\n", "no `test result:` line"),
            ("empty", "", "no `test result:` line"),
            ("short", "test result: ok. 1 passed; 0 failed;\n", "1 test(s) passed, but the cell claims 2"),
            ("long", "test result: ok. 3 passed; 0 failed;\n", "3 test(s) passed, but the cell claims 2"),
            ("failed", "test result: FAILED. 2 passed; 1 failed;\n", "1 test(s) failed"),
            ("two", _OK + _OK, "2 `test result:` lines"),
            ("oversized", "x" * (wasm_test_count.MAX_LINE + 1) + "test result: ok. 2 passed; 0 failed;\n", "no `test result:`"),
        ):
            with self.subTest(name):
                code, _, err = _run(_ARGS, stdin)
                self.assertEqual(code, 1, err)
                self.assertIn(needle, err)

    def test_usage_and_unknown_cell(self) -> None:
        for argv, needle in (
            (["p", "test:t"], "usage"),
            (["p", "lib", "wasm32-unknown-unknown"], "no cell p lib"),
            (["p", "test:t", "wasm32-wasip1"], "no cell"),
        ):
            with self.subTest(argv=argv):
                code, _, err = _run(argv, _OK)
                self.assertEqual(code, 2)
                self.assertIn(needle, err)


class TestLoadCells(unittest.TestCase):
    def refused(self, table: str, needle: str) -> None:
        path = _table(table)
        try:
            got = wasm_test_count.load_cells(path)
        finally:
            os.unlink(path)
        assert isinstance(got, str), got
        self.assertIn(needle, got)

    def test_bad_tables_refused(self) -> None:
        for name, table, needle in (
            ("empty", "cells: []\n", "non-empty `cells`"),
            ("extra top key", "cells:\n" + _CELL + "more: 1\n", "exactly a non-empty"),
            ("twice", "cells:\n" + _CELL + _CELL, "claimed twice"),
            ("bad target", "cells:\n" + _CELL.replace("{test: t}", "tests"), "neither `lib`"),
            ("host platform", "cells:\n" + _CELL.replace("wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"), "not a wasm32 triple"),
            ("bool expect", "cells:\n" + _CELL.replace("expect_tests: 2", "expect_tests: true"), "positive integer"),
            ("zero expect", "cells:\n" + _CELL.replace("expect_tests: 2", "expect_tests: 0"), "positive integer"),
            ("feature twice", "cells:\n" + _CELL.replace("[a]", "[a, a]"), "names a feature twice"),
            ("missing key", "cells:\n" + _CELL.replace("    owner: j\n", ""), "exactly"),
            ("unknown key", "cells:\n" + _CELL + "    note: x\n", "exactly"),
            ("duplicate yaml key", "cells:\n" + _CELL + "    owner: k\n", "unreadable"),
        ):
            with self.subTest(name):
                self.refused(table, needle)

    def test_repo_table_loads(self) -> None:
        cells = wasm_test_count.load_cells()
        assert isinstance(cells, list), cells
        self.assertTrue(all(c.platform.startswith("wasm32-") for c in cells))


if __name__ == "__main__":
    unittest.main()
