#!/usr/bin/env python3
"""Refusal proofs for `e2e_shard.py`, the e2e SEAL shard-partition SSOT.

Every way the partition proof must refuse (a test no shard runs, a test two
shards run, a shard selecting outside the archive, an empty archive, a missing
shard, a malformed listing) and every way the ci.yml matrix check must refuse
(a dropped or extra shard, a bypassing `nextest run`, no SSOT invocation) gets
its own test, per PRINCIPLES.md "Prove the refusals". Pure stdlib `unittest`.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

_spec = importlib.util.spec_from_file_location("e2e_shard", os.path.join(HERE, "e2e_shard.py"))
assert _spec is not None and _spec.loader is not None
es = importlib.util.module_from_spec(_spec)
sys.modules["e2e_shard"] = es
_spec.loader.exec_module(es)


def _listing(cases: dict[str, dict[str, str]]) -> str:
    """Build a nextest JSON listing: {binary: {test: status}}."""
    return json.dumps(
        {
            "rust-suites": {
                binary: {"testcases": {name: {"filter-match": {"status": status}} for name, status in tests.items()}}
                for binary, tests in cases.items()
            }
        }
    )


FULL = [f"bin t{i}" for i in range(20)]


def _exact_partition() -> dict[int, list[str]]:
    shards: dict[int, list[str]] = {k: [] for k in range(1, es.SHARDS + 1)}
    for i, test in enumerate(FULL):
        shards[i % es.SHARDS + 1].append(test)
    return shards


class SelectionTest(unittest.TestCase):
    def test_heavy_and_light_legs_are_complementary(self) -> None:
        self.assertEqual(es.selection(1), ["-E", es.HEAVY, "--partition", f"count:1/{es.HEAVY_SHARDS}"])
        last = es.selection(es.SHARDS)
        self.assertEqual(last, ["-E", f"not ({es.HEAVY})", "--partition", f"count:{es.LIGHT_SHARDS}/{es.LIGHT_SHARDS}"])

    def test_every_shard_has_a_selection(self) -> None:
        for k in range(1, es.SHARDS + 1):
            self.assertEqual(len(es.selection(k)), 4)

    def test_out_of_range_shard_refused(self) -> None:
        for bad in (0, -1, es.SHARDS + 1):
            with self.assertRaises(es.ShardError):
                es.selection(bad)

    def test_non_int_shard_refused(self) -> None:
        for bad in (True, 1.0, "1", None):
            with self.assertRaises(es.ShardError):
                es.selection(bad)  # type: ignore[arg-type]

    def test_non_canonical_cli_shard_refused(self) -> None:
        for bad in ("", "01", "+1", " 1", "1 ", "٣", "0x1", "15", "0"):
            with self.assertRaises(es.ShardError):
                es.parse_shard(bad)
        self.assertEqual(es.parse_shard("14"), 14)

    def test_run_with_bad_shard_exits_nonzero(self) -> None:
        self.assertEqual(es.main(["run", "0"]), 1)
        self.assertEqual(es.main(["run", "x"]), 1)

    def test_unknown_subcommand_exits_nonzero(self) -> None:
        self.assertEqual(es.main([]), 2)
        self.assertEqual(es.main(["run"]), 2)
        self.assertEqual(es.main(["cover", "extra"]), 2)


class ListingTest(unittest.TestCase):
    def test_only_matching_tests_are_selected(self) -> None:
        got = es.matched_tests(_listing({"a": {"x": "matches", "y": "mismatch"}, "b": {"z": "matches"}}))
        self.assertEqual(got, ["a x", "b z"])

    def test_non_json_refused(self) -> None:
        with self.assertRaises(es.ShardError):
            es.matched_tests("not json")

    def test_missing_suites_refused(self) -> None:
        for doc in ("{}", "[]", '{"rust-suites": []}'):
            with self.assertRaises(es.ShardError):
                es.matched_tests(doc)

    def test_missing_testcases_refused(self) -> None:
        with self.assertRaises(es.ShardError):
            es.matched_tests(json.dumps({"rust-suites": {"a": {}}}))

    def test_unknown_status_refused(self) -> None:
        for status in ("", "match", None):
            doc = json.dumps({"rust-suites": {"a": {"testcases": {"x": {"filter-match": {"status": status}}}}}})
            with self.assertRaises(es.ShardError):
                es.matched_tests(doc)

    def test_missing_filter_match_refused(self) -> None:
        doc = json.dumps({"rust-suites": {"a": {"testcases": {"x": {}}}}})
        with self.assertRaises(es.ShardError):
            es.matched_tests(doc)


class PartitionTest(unittest.TestCase):
    def test_exact_partition_passes(self) -> None:
        self.assertEqual(es.partition_errors(FULL, _exact_partition()), [])

    def test_test_no_shard_runs_refused(self) -> None:
        shards = _exact_partition()
        shards[1].remove(FULL[0])
        self.assertTrue(any("no shard runs" in e for e in es.partition_errors(FULL, shards)))

    def test_test_two_shards_run_refused(self) -> None:
        shards = _exact_partition()
        shards[2].append(FULL[0])
        self.assertTrue(any("runs in 2 shards" in e for e in es.partition_errors(FULL, shards)))

    def test_selection_outside_archive_refused(self) -> None:
        shards = _exact_partition()
        shards[3].append("bin ghost")
        self.assertTrue(any("does not list" in e for e in es.partition_errors(FULL, shards)))

    def test_empty_archive_refused(self) -> None:
        shards: dict[int, list[str]] = {k: [] for k in range(1, es.SHARDS + 1)}
        self.assertTrue(any("empty set" in e for e in es.partition_errors([], shards)))

    def test_missing_shard_refused(self) -> None:
        shards = _exact_partition()
        del shards[es.SHARDS]
        self.assertTrue(any("expected 1.." in e for e in es.partition_errors(FULL, shards)))

    def test_repeated_archive_id_refused(self) -> None:
        self.assertTrue(any("repeats" in e for e in es.partition_errors([*FULL, FULL[0]], _exact_partition())))


E2E_JOB = {
    "strategy": {"fail-fast": False, "matrix": {"shard": list(range(1, es.SHARDS + 1))}},
    "steps": [{"uses": "actions/checkout@v7"}, {"name": "shard", "run": es.RUN_INVOCATION}],
}


def _workflow(job: dict) -> dict:
    return {"jobs": {"e2e": job}}


class MatrixTest(unittest.TestCase):
    def test_ssot_matrix_passes(self) -> None:
        self.assertEqual(es.matrix_errors(_workflow(E2E_JOB)), [])

    def test_repo_ci_yml_passes(self) -> None:
        self.assertEqual(es.matrix(), 0)

    def test_missing_job_refused(self) -> None:
        self.assertTrue(es.matrix_errors({"jobs": {}}))
        self.assertTrue(es.matrix_errors({}))

    def test_dropped_shard_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["strategy"]["matrix"]["shard"].pop()
        self.assertTrue(any("expected 1.." in e for e in es.matrix_errors(_workflow(job))))

    def test_extra_shard_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["strategy"]["matrix"]["shard"].append(es.SHARDS + 1)
        self.assertTrue(any("expected 1.." in e for e in es.matrix_errors(_workflow(job))))

    def test_string_shards_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["strategy"]["matrix"]["shard"] = [str(k) for k in range(1, es.SHARDS + 1)]
        self.assertTrue(es.matrix_errors(_workflow(job)))

    def test_extra_matrix_key_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["strategy"]["matrix"]["include"] = [{"shard": 99}]
        self.assertTrue(any("exactly one key" in e for e in es.matrix_errors(_workflow(job))))

    def test_missing_invocation_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["steps"] = [{"run": "echo hi"}]
        self.assertTrue(any("exactly one step" in e for e in es.matrix_errors(_workflow(job))))

    def test_duplicate_invocation_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["steps"].append({"run": es.RUN_INVOCATION})
        self.assertTrue(any("exactly one step" in e for e in es.matrix_errors(_workflow(job))))

    def test_direct_nextest_run_refused(self) -> None:
        job = copy.deepcopy(E2E_JOB)
        job["steps"].append({"run": "cargo nextest run --archive-file nextest.tar.zst"})
        self.assertTrue(any("directly" in e for e in es.matrix_errors(_workflow(job))))


if __name__ == "__main__":
    unittest.main()
