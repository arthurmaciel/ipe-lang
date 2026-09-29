#!/usr/bin/env python3
"""Refusal proofs for `nightly_green.py`, the nightly-green gate SSOT.

Every way the gate must stay red (a red, cancelled, running, stale, future,
missing, or mis-scoped nightly; a malformed listing, event, or queue ref; an
unreadable API) and every way its wiring could be weakened into a pass gets its
own test, per PRINCIPLES.md "Prove the refusals". Pure stdlib `unittest`.
"""

from __future__ import annotations

import copy
import importlib.util
import os
import sys
import unittest
from datetime import datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))

_spec = importlib.util.spec_from_file_location("nightly_green", os.path.join(HERE, "nightly_green.py"))
assert _spec is not None and _spec.loader is not None
ng = importlib.util.module_from_spec(_spec)
sys.modules["nightly_green"] = ng
_spec.loader.exec_module(ng)

NOW = datetime(2026, 9, 29, 12, 0, 0, tzinfo=timezone.utc)
SHA = "a" * 40
OTHER = "b" * 40
REPO = "ipe-lang/compiler"


def _stamp(delta: timedelta) -> str:
    return (NOW - delta).strftime("%Y-%m-%dT%H:%M:%SZ")


def _run(**over: object) -> dict:
    run = {
        "id": 1,
        "event": "workflow_dispatch",
        "path": ".github/workflows/ci.yml",
        "status": "completed",
        "conclusion": "success",
        "head_branch": "main",
        "head_sha": OTHER,
        "created_at": _stamp(timedelta(hours=8)),
    }
    run.update(over)
    return run


def _listing(*runs: dict) -> dict:
    return {"total_count": len(runs), "workflow_runs": list(runs)}


class FakeApi:
    """Route `gh api` paths: a main-nightly listing, an own-commit listing, a PR."""

    def __init__(self, main: object, own: object = None, pr: object = None) -> None:
        self.main, self.own, self.pr = main, own if own is not None else _listing(), pr
        self.calls: list[str] = []

    def __call__(self, path: str) -> object:
        self.calls.append(path)
        if "/pulls/" in path:
            if self.pr is None:
                raise ng.NightlyError("no such PR")
            return self.pr
        if "head_sha=" in path:
            return self.own
        if "branch=main" in path:
            if isinstance(self.main, Exception):
                raise self.main
            return self.main
        raise AssertionError(f"unexpected path {path}")


def _verdict(api: FakeApi, **env: str) -> list[str]:
    base = {"EVENT": "pull_request", "HEAD_SHA": SHA, "HEAD_REF": "", "REPO": REPO}
    base.update(env)
    saved = ng._gh_json
    ng._gh_json = api
    try:
        return ng.verdict(base, NOW)
    finally:
        ng._gh_json = saved


class RunErrorsTest(unittest.TestCase):
    def check(self, run: dict | None) -> list[str]:
        return ng.run_errors(run, branch="main", sha=None, now=NOW)

    def test_green_fresh_nightly_passes(self) -> None:
        self.assertEqual(self.check(_run()), [])

    def test_missing_nightly_refused(self) -> None:
        self.assertTrue(self.check(None))

    def test_red_nightly_refused(self) -> None:
        for bad in ("failure", "cancelled", "timed_out", "skipped", "neutral", "action_required", None):
            self.assertTrue(self.check(_run(conclusion=bad)), bad)

    def test_running_nightly_refused(self) -> None:
        self.assertTrue(self.check(_run(status="in_progress", conclusion=None)))

    def test_stale_nightly_refused(self) -> None:
        self.assertTrue(any("older than" in e for e in self.check(_run(created_at=_stamp(timedelta(hours=ng.MAX_AGE_H, seconds=1))))))
        self.assertEqual(self.check(_run(created_at=_stamp(timedelta(hours=ng.MAX_AGE_H)))), [])

    def test_future_nightly_refused(self) -> None:
        self.assertTrue(any("future" in e for e in self.check(_run(created_at=_stamp(-timedelta(hours=1))))))

    def test_malformed_timestamp_refused(self) -> None:
        for bad in (None, "", "2026-09-29", "2026-09-29T12:00:00+00:00", 1759140000):
            self.assertTrue(self.check(_run(created_at=bad)), bad)

    def test_wrong_event_refused(self) -> None:
        for bad in ("push", "pull_request", "schedule", None):
            self.assertTrue(self.check(_run(event=bad)), bad)

    def test_wrong_workflow_refused(self) -> None:
        self.assertTrue(self.check(_run(path=".github/workflows/other.yml")))

    def test_wrong_branch_refused(self) -> None:
        self.assertTrue(self.check(_run(head_branch="feature")))

    def test_wrong_commit_refused(self) -> None:
        self.assertTrue(ng.run_errors(_run(head_sha=OTHER), branch=None, sha=SHA, now=None))


class ListingTest(unittest.TestCase):
    def test_empty_listing_is_no_run(self) -> None:
        self.assertIsNone(ng.latest_run(_listing()))

    def test_malformed_listing_refused(self) -> None:
        for bad in ({}, [], None, {"workflow_runs": {}}, {"workflow_runs": ["x"]}):
            with self.assertRaises(ng.NightlyError):
                ng.latest_run(bad)


class EventTest(unittest.TestCase):
    def test_pull_request_uses_head_sha(self) -> None:
        self.assertEqual(ng.change_sha_source("pull_request", SHA, ""), ("sha", SHA))

    def test_bad_head_sha_refused(self) -> None:
        for bad in ("", "a" * 39, "A" * 40, SHA + "\n", "a" * 41):
            with self.assertRaises(ng.NightlyError):
                ng.change_sha_source("pull_request", bad, "")

    def test_merge_group_parses_queue_ref(self) -> None:
        ref = f"refs/heads/gh-readonly-queue/main/pr-42-{SHA}"
        self.assertEqual(ng.change_sha_source("merge_group", "", ref), ("pr", "42"))

    def test_bad_queue_ref_refused(self) -> None:
        for bad in (
            "",
            f"refs/heads/gh-readonly-queue/dev/pr-42-{SHA}",
            f"refs/heads/gh-readonly-queue/main/pr-0-{SHA}",
            f"refs/heads/gh-readonly-queue/main/pr-x-{SHA}",
            "refs/heads/gh-readonly-queue/main/pr-42-abc",
            f"refs/heads/gh-readonly-queue/main/pr-42-{SHA}/../x",
        ):
            with self.assertRaises(ng.NightlyError):
                ng.change_sha_source("merge_group", "", bad)

    def test_unknown_event_refused(self) -> None:
        for bad in ("", "pull_request_target", "schedule", "workflow_run"):
            with self.assertRaises(ng.NightlyError):
                ng.change_sha_source(bad, SHA, "")


class VerdictTest(unittest.TestCase):
    def test_green_main_nightly_passes(self) -> None:
        self.assertEqual(_verdict(FakeApi(_listing(_run()))), [])

    def test_red_main_nightly_blocks(self) -> None:
        self.assertTrue(_verdict(FakeApi(_listing(_run(conclusion="failure")))))

    def test_no_main_nightly_blocks(self) -> None:
        self.assertTrue(_verdict(FakeApi(_listing())))

    def test_own_green_dispatch_recovers_red_nightly(self) -> None:
        own = _listing(_run(head_branch="fix-nightly", head_sha=SHA, created_at=_stamp(timedelta(days=9))))
        self.assertEqual(_verdict(FakeApi(_listing(_run(conclusion="failure")), own=own)), [])

    def test_own_red_dispatch_does_not_recover(self) -> None:
        own = _listing(_run(head_branch="fix-nightly", head_sha=SHA, conclusion="failure"))
        self.assertTrue(_verdict(FakeApi(_listing(_run(conclusion="failure")), own=own)))

    def test_own_dispatch_on_other_commit_does_not_recover(self) -> None:
        own = _listing(_run(head_branch="fix-nightly", head_sha=OTHER))
        self.assertTrue(_verdict(FakeApi(_listing(_run(conclusion="failure")), own=own)))

    def test_merge_group_resolves_pr_head(self) -> None:
        ref = f"refs/heads/gh-readonly-queue/main/pr-7-{OTHER}"
        own = _listing(_run(head_branch="fix", head_sha=SHA))
        api = FakeApi(_listing(_run(conclusion="failure")), own=own, pr={"head": {"sha": SHA}})
        self.assertEqual(_verdict(api, EVENT="merge_group", HEAD_SHA="", HEAD_REF=ref), [])
        self.assertTrue(any("/pulls/7" in c for c in api.calls))

    def test_merge_group_bad_pr_head_refused(self) -> None:
        ref = f"refs/heads/gh-readonly-queue/main/pr-7-{OTHER}"
        for pr in ({}, {"head": {}}, {"head": {"sha": "zz"}}, []):
            with self.assertRaises(ng.NightlyError):
                _verdict(FakeApi(_listing(_run()), pr=pr), EVENT="merge_group", HEAD_SHA="", HEAD_REF=ref)

    def test_bad_repo_refused(self) -> None:
        for bad in ("", "owner", "a/b/c", "a/b?x=1", "../..", "a/.."):
            with self.assertRaises(ng.NightlyError):
                _verdict(FakeApi(_listing(_run())), REPO=bad)

    def test_unreadable_api_is_red_via_main(self) -> None:
        saved = ng._gh_json

        def boom(path: str) -> object:
            raise ng.NightlyError("gh api down")

        ng._gh_json = boom
        env = {"EVENT": "pull_request", "HEAD_SHA": SHA, "HEAD_REF": "", "REPO": REPO}
        saved_env = dict(os.environ)
        try:
            os.environ.update(env)
            self.assertEqual(ng.main(["verdict"]), 1)
        finally:
            ng._gh_json = saved
            os.environ.clear()
            os.environ.update(saved_env)

    def test_usage_refused(self) -> None:
        self.assertEqual(ng.main([]), 2)
        self.assertEqual(ng.main(["verdict", "x"]), 2)


STEP = {
    "name": "verdict",
    "env": dict(ng.EXPECTED_ENV),
    "run": ng.VERDICT_INVOCATION,
}
WORKFLOW = {
    "jobs": {
        "nightly-green": {
            "name": "nightly-green",
            "runs-on": "ubuntu-latest",
            "steps": [{"uses": "actions/checkout@v7"}, STEP],
        }
    }
}
MANIFEST = {"checks": [{"context": "nightly-green", "disposition": "gate", "producer": "nightly-green.yml"}]}


def _job(wf: dict) -> dict:
    return wf["jobs"]["nightly-green"]


class WiringTest(unittest.TestCase):
    def test_ssot_wiring_passes(self) -> None:
        self.assertEqual(ng.wiring_errors(WORKFLOW, MANIFEST), [])

    def test_repo_wiring_passes(self) -> None:
        self.assertEqual(ng.lint(), 0)

    def test_job_level_escape_refused(self) -> None:
        for key, val in (("if", "false"), ("continue-on-error", True), ("needs", ["x"]), ("strategy", {})):
            wf = copy.deepcopy(WORKFLOW)
            _job(wf)[key] = val
            self.assertTrue(ng.wiring_errors(wf, MANIFEST), key)

    def test_step_escape_refused(self) -> None:
        for key, val in (("if", "false"), ("continue-on-error", True)):
            wf = copy.deepcopy(WORKFLOW)
            _job(wf)["steps"][1][key] = val
            self.assertTrue(ng.wiring_errors(wf, MANIFEST), key)

    def test_masked_invocation_refused(self) -> None:
        for run in (f"{ng.VERDICT_INVOCATION} || true", f"{ng.VERDICT_INVOCATION}; exit 0", "true"):
            wf = copy.deepcopy(WORKFLOW)
            _job(wf)["steps"][1]["run"] = run
            self.assertTrue(ng.wiring_errors(wf, MANIFEST), run)

    def test_duplicate_invocation_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        _job(wf)["steps"].append(dict(STEP))
        self.assertTrue(ng.wiring_errors(wf, MANIFEST))

    def test_env_tampering_refused(self) -> None:
        for key, val in (("EVENT", "pull_request"), ("HEAD_SHA", "${{ github.sha }}"), ("GH_TOKEN", "${{ secrets.X }}")):
            wf = copy.deepcopy(WORKFLOW)
            _job(wf)["steps"][1]["env"][key] = val
            self.assertTrue(ng.wiring_errors(wf, MANIFEST), key)
        wf = copy.deepcopy(WORKFLOW)
        del _job(wf)["steps"][1]["env"]["REPO"]
        self.assertTrue(ng.wiring_errors(wf, MANIFEST))

    def test_renamed_context_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        _job(wf)["name"] = "nightly"
        self.assertTrue(ng.wiring_errors(wf, MANIFEST))

    def test_extra_job_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        wf["jobs"]["other"] = {"steps": []}
        self.assertTrue(ng.wiring_errors(wf, MANIFEST))

    def test_manifest_downgrade_refused(self) -> None:
        for manifest in (
            {"checks": []},
            {"checks": [{"context": "nightly-green", "disposition": "informational", "producer": "nightly-green.yml"}]},
            {"checks": [{"context": "nightly-green", "disposition": "gate", "producer": "ci.yml"}]},
            {"checks": MANIFEST["checks"] * 2},
            None,
        ):
            self.assertTrue(ng.wiring_errors(WORKFLOW, manifest), manifest)


if __name__ == "__main__":
    unittest.main()
