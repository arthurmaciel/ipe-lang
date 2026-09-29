#!/usr/bin/env python3
"""Refusal proofs for `e2e_shard.py` (the e2e SEAL shard-partition SSOT) and
`tools/scripts/e2e-shard-cover.py` (its per-run partition proof).

Every way the partition proof must refuse (a test no shard runs, a test two
shards run, a shard selecting outside the archive, an empty archive, a missing
shard, a malformed listing or plan) and every way the ci.yml wiring check must
refuse (a dropped or extra shard, a bypassing `nextest run`, a shard or cover
reading its selection off the plan, a masked plan step) gets its own test, per
PRINCIPLES.md "Prove the refusals". Pure stdlib `unittest`.
"""

from __future__ import annotations

import contextlib
import copy
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def _load(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


es = _load("e2e_shard", os.path.join(HERE, "e2e_shard.py"))
ec = _load("e2e_shard_cover", os.path.join(REPO, "tools", "scripts", "e2e-shard-cover.py"))


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

    def test_unknown_mode_exits_nonzero(self) -> None:
        for argv in ([], ["run", "1"], ["plan"], ["lint"], ["matrix"], ["cover"], ["--plan", "x"], ["--plan", "--lint"]):
            with self.subTest(argv):
                self.assertEqual(es.main(argv), 2)


class PlanTest(unittest.TestCase):
    def test_plan_is_every_selection(self) -> None:
        plan = es.plan()
        self.assertEqual(sorted(plan, key=int), [str(k) for k in range(1, es.SHARDS + 1)])
        for k in range(1, es.SHARDS + 1):
            self.assertEqual(["-E", plan[str(k)]["filter"], "--partition", plan[str(k)]["partition"]], es.selection(k))

    def test_cover_parses_the_published_plan(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "out")
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(es.write_plan(out), 0)
            with open(out, encoding="utf-8") as fh:
                key, sep, value = fh.read().rstrip("\n").partition("=")
        self.assertEqual((key, sep), ("plan", "="))
        self.assertNotIn("\n", value)
        parsed = ec.parse_plan(value)
        self.assertEqual({k: ["-E", f, "--partition", p] for k, (f, p) in parsed.items()},
                         {k: es.selection(k) for k in range(1, es.SHARDS + 1)})

    def test_plan_without_output_file_is_red(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(es.write_plan(""), 1)
            with tempfile.TemporaryDirectory() as tmp:
                self.assertEqual(es.write_plan(os.path.join(tmp, "no", "such")), 1)


class PlanParseTest(unittest.TestCase):
    GOOD = {"1": {"filter": "all()", "partition": "count:1/2"}, "2": {"filter": "all()", "partition": "count:2/2"}}

    def test_good_plan_parses(self) -> None:
        self.assertEqual(ec.parse_plan(json.dumps(self.GOOD)), {1: ("all()", "count:1/2"), 2: ("all()", "count:2/2")})

    def test_malformed_plan_refused(self) -> None:
        entry = {"filter": "all()", "partition": "count:1/1"}
        for text in (
            "",
            "not json",
            "[]",
            "{}",
            json.dumps({"0": entry}),
            json.dumps({"2": entry}),
            json.dumps({"01": entry}),
            json.dumps({"1": entry, "3": entry}),
            json.dumps({"1": entry, "1 ": entry}),
            json.dumps({"1": {"filter": "all()"}}),
            json.dumps({"1": dict(entry, extra="x")}),
            json.dumps({"1": {"filter": "", "partition": "count:1/1"}}),
            json.dumps({"1": {"filter": "all()", "partition": " "}}),
            json.dumps({"1": {"filter": 1, "partition": "count:1/1"}}),
            json.dumps({"1": "all()"}),
            json.dumps({str(k): entry for k in range(1, ec.MAX_SHARDS + 2)}),
            " " * (ec.MAX_PLAN_CHARS + 1),
        ):
            with self.subTest(text[:60]), self.assertRaises(ec.CoverError):
                ec.parse_plan(text)

    def test_cover_main_refuses_args_and_bad_plan(self) -> None:
        self.assertEqual(ec.main(["x"]), 2)
        old = os.environ.pop("E2E_PLAN", None)
        try:
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(ec.main([]), 1)
        finally:
            if old is not None:
                os.environ["E2E_PLAN"] = old


class ListingTest(unittest.TestCase):
    def test_only_matching_tests_are_selected(self) -> None:
        got = ec.matched_tests(_listing({"a": {"x": "matches", "y": "mismatch"}, "b": {"z": "matches"}}))
        self.assertEqual(got, ["a x", "b z"])

    def test_non_json_refused(self) -> None:
        with self.assertRaises(ec.CoverError):
            ec.matched_tests("not json")

    def test_missing_suites_refused(self) -> None:
        for doc in ("{}", "[]", '{"rust-suites": []}'):
            with self.assertRaises(ec.CoverError):
                ec.matched_tests(doc)

    def test_missing_testcases_refused(self) -> None:
        with self.assertRaises(ec.CoverError):
            ec.matched_tests(json.dumps({"rust-suites": {"a": {}}}))

    def test_unknown_status_refused(self) -> None:
        for status in ("", "match", None):
            doc = json.dumps({"rust-suites": {"a": {"testcases": {"x": {"filter-match": {"status": status}}}}}})
            with self.assertRaises(ec.CoverError):
                ec.matched_tests(doc)

    def test_missing_filter_match_refused(self) -> None:
        doc = json.dumps({"rust-suites": {"a": {"testcases": {"x": {}}}}})
        with self.assertRaises(ec.CoverError):
            ec.matched_tests(doc)


class PartitionTest(unittest.TestCase):
    def test_exact_partition_passes(self) -> None:
        self.assertEqual(ec.partition_errors(FULL, _exact_partition(), es.SHARDS), [])

    def test_test_no_shard_runs_refused(self) -> None:
        shards = _exact_partition()
        shards[1].remove(FULL[0])
        self.assertTrue(any("no shard runs" in e for e in ec.partition_errors(FULL, shards, es.SHARDS)))

    def test_test_two_shards_run_refused(self) -> None:
        shards = _exact_partition()
        shards[2].append(FULL[0])
        self.assertTrue(any("runs in 2 shards" in e for e in ec.partition_errors(FULL, shards, es.SHARDS)))

    def test_selection_outside_archive_refused(self) -> None:
        shards = _exact_partition()
        shards[3].append("bin ghost")
        self.assertTrue(any("does not list" in e for e in ec.partition_errors(FULL, shards, es.SHARDS)))

    def test_empty_archive_refused(self) -> None:
        shards: dict[int, list[str]] = {k: [] for k in range(1, es.SHARDS + 1)}
        self.assertTrue(any("empty set" in e for e in ec.partition_errors([], shards, es.SHARDS)))

    def test_missing_shard_refused(self) -> None:
        shards = _exact_partition()
        del shards[es.SHARDS]
        self.assertTrue(any("expected 1.." in e for e in ec.partition_errors(FULL, shards, es.SHARDS)))

    def test_repeated_archive_id_refused(self) -> None:
        self.assertTrue(any("repeats" in e for e in ec.partition_errors([*FULL, FULL[0]], _exact_partition(), es.SHARDS)))


PLAN_STEP = {"name": "plan", "id": es.PLAN_STEP_ID, "run": es.PLAN_INVOCATION}
SHARD_STEP = {"name": "shard", "env": {"IPE_E2E": "1", **es.SHARD_ENV}, "run": es.RUN_COMMAND}
COVER_STEP = {"name": "cover", "if": "needs.test-prep.result == 'success'", "env": dict(es.COVER_ENV), "run": es.COVER_INVOCATION}
WORKFLOW = {
    "jobs": {
        "changes": {
            "outputs": {es.PLAN_OUTPUT: es.PLAN_OUTPUT_VALUE},
            "steps": [{"uses": "actions/checkout@v7"}, PLAN_STEP],
        },
        "e2e": {
            "needs": ["changes", "test-prep"],
            "strategy": {"fail-fast": False, "matrix": {"shard": list(range(1, es.SHARDS + 1))}},
            "steps": [{"uses": "actions/checkout@v7"}, SHARD_STEP],
        },
        "seal-slice": {"needs": ["changes", "e2e"], "steps": [{"run": "gate"}, COVER_STEP]},
    }
}


def _refused(mutate, needle: str = "") -> bool:
    wf = copy.deepcopy(WORKFLOW)
    mutate(wf["jobs"])
    errors = es.wiring_errors(wf)
    return bool(errors) and any(needle in e for e in errors)


class WiringTest(unittest.TestCase):
    def test_ssot_wiring_passes(self) -> None:
        self.assertEqual(es.wiring_errors(WORKFLOW), [])

    def test_repo_ci_yml_passes(self) -> None:
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(es.lint(), 0)

    def test_run_command_is_built_from_the_ssot_args(self) -> None:
        self.assertEqual(
            es.RUN_COMMAND,
            "cargo nextest run --archive-file nextest.tar.zst --workspace-remap . --profile ci"
            ' --no-fail-fast --test-threads=2 --no-tests=fail -E "$SHARD_FILTER" --partition "$SHARD_PARTITION"',
        )

    def test_missing_jobs_refused(self) -> None:
        self.assertTrue(es.wiring_errors({}))
        for job in ("changes", "e2e", "seal-slice"):
            with self.subTest(job):
                self.assertTrue(_refused(lambda j, job=job: j.pop(job), f"no `{job}` job"))

    def test_dropped_extra_or_string_shard_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["e2e"]["strategy"]["matrix"]["shard"].pop(), "expected 1.."))
        self.assertTrue(_refused(lambda j: j["e2e"]["strategy"]["matrix"]["shard"].append(es.SHARDS + 1), "expected 1.."))
        self.assertTrue(_refused(lambda j: j["e2e"]["strategy"]["matrix"].update(shard=[str(k) for k in range(1, es.SHARDS + 1)])))

    def test_extra_matrix_key_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["e2e"]["strategy"]["matrix"].update(include=[{"shard": 99}]), "exactly one key"))

    def test_missing_or_duplicate_shard_run_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["e2e"].update(steps=[{"run": "echo hi"}]), "exactly one step"))
        self.assertTrue(_refused(lambda j: j["e2e"]["steps"].append(dict(SHARD_STEP)), "exactly one step"))

    def test_second_nextest_run_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["e2e"]["steps"].append({"run": "cargo nextest run --archive-file nextest.tar.zst"}), "exactly one step"))

    def test_masked_or_widened_shard_run_refused(self) -> None:
        for run in (
            f"{es.RUN_COMMAND} || true",
            f"{es.RUN_COMMAND} -E 'not all()'",
            es.RUN_COMMAND.replace(" --no-tests=fail", ""),
            es.RUN_COMMAND.replace('--partition "$SHARD_PARTITION"', "--partition count:1/1"),
        ):
            with self.subTest(run):
                self.assertTrue(_refused(lambda j, run=run: j["e2e"]["steps"][1].update(run=run)))
        self.assertTrue(_refused(lambda j: j["e2e"]["steps"][1].update({"continue-on-error": True}), "continue-on-error"))

    def test_shard_selection_off_the_plan_refused(self) -> None:
        for key, val in (("SHARD_FILTER", "all()"), ("SHARD_PARTITION", "count:1/1"), ("SHARD_FILTER", None)):
            with self.subTest(key):

                def mutate(j, key=key, val=val):
                    env = j["e2e"]["steps"][1]["env"]
                    if val is None:
                        del env[key]
                    else:
                        env[key] = val

                self.assertTrue(_refused(mutate, "selection env"))
        self.assertTrue(_refused(lambda j: j["e2e"]["steps"][1].pop("env"), "selection env"))

    def test_e2e_without_changes_need_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["e2e"].update(needs=["test-prep"]), "must need `changes`"))
        self.assertTrue(_refused(lambda j: j["e2e"].pop("needs"), "must need `changes`"))

    def test_shard_selection_read_elsewhere_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"].append({"run": 'echo "$SHARD_FILTER"'}), "outside `e2e`"))

    def test_plan_step_tampering_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["changes"]["steps"].pop(1), "exactly one step"))
        self.assertTrue(_refused(lambda j: j["changes"]["steps"].append(dict(PLAN_STEP, id="x")), "exactly one step"))
        self.assertTrue(_refused(lambda j: j["changes"]["steps"][1].update(id="plan"), "id: e2e-plan"))
        self.assertTrue(_refused(lambda j: j["changes"]["steps"][1].update(run=f"{es.PLAN_INVOCATION} || true"), "run only"))
        for key, val in (("if", "false"), ("env", {"GITHUB_OUTPUT": "/dev/null"}), ("continue-on-error", True)):
            with self.subTest(key):
                self.assertTrue(_refused(lambda j, key=key, val=val: j["changes"]["steps"][1].update({key: val})))

    def test_plan_output_tampering_refused(self) -> None:
        for val in (None, "", '{"1": {"filter": "none()", "partition": "count:1/1"}}', "${{ steps.other.outputs.plan }}"):
            with self.subTest(val):

                def mutate(j, val=val):
                    if val is None:
                        del j["changes"]["outputs"][es.PLAN_OUTPUT]
                    else:
                        j["changes"]["outputs"][es.PLAN_OUTPUT] = val

                self.assertTrue(_refused(mutate, "output `e2e_plan`"))

    def test_cover_tampering_refused(self) -> None:
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"].pop(1), "exactly one step"))
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"][1].update(run=f"{es.COVER_INVOCATION} || true"), "run only"))
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"][1].update({"continue-on-error": True}), "continue-on-error"))
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"][1].update(env={"E2E_PLAN": '{"1": {}}'}), "cover step's env"))
        self.assertTrue(_refused(lambda j: j["seal-slice"]["steps"][1].pop("env"), "cover step's env"))
        self.assertTrue(_refused(lambda j: j["seal-slice"].update(needs=["e2e"]), "must need `changes`"))


if __name__ == "__main__":
    unittest.main()
