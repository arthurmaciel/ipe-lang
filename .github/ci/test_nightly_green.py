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
        "head_repository": {"full_name": REPO},
    }
    run.update(over)
    return run


def _listing(*runs: dict) -> dict:
    return {"total_count": len(runs), "workflow_runs": list(runs)}


class FakeApi:
    """Route `gh api` paths: the three dispatch listings per scope, a PR, a compare.

    `main`/`own` answer the workflow listing filtered by event and branch/commit;
    `event_only` answers the workflow listing filtered by event alone;
    `repo_main`/`repo_own` answer the repository listing filtered by event and branch/commit;
    `by_id` answers a run's own document, by default its copy in the scope's last listings.
    """

    def __init__(
        self,
        main: object,
        own: object = None,
        pr: object = None,
        compare: object = None,
        event_only: object = None,
        repo_main: object = None,
        repo_own: object = None,
        by_id: dict | None = None,
    ) -> None:
        self.by_id = by_id or {}
        self.served: list[object] = []
        self.main, self.own, self.pr = main, own if own is not None else _listing(), pr
        self.event_only = event_only if event_only is not None else _listing()
        self.repo_main = repo_main if repo_main is not None else _listing()
        self.repo_own = repo_own if repo_own is not None else _listing()
        self.compare = compare if compare is not None else {"status": "ahead"}
        self.calls: list[str] = []

    def __call__(self, path: str) -> object:
        self.calls.append(path)
        served = self._route(path)
        if "/runs?" in path:
            self.served.append(served)
        return served

    def _route(self, path: str) -> object:
        if "/pulls/" in path:
            if self.pr is None:
                raise ng.NightlyError("no such PR")
            return self.pr
        if "/compare/" in path:
            return self.compare
        if path.startswith(f"repos/{REPO}/actions/runs/"):
            rid = int(path.rsplit("/", 1)[-1])
            if rid in self.by_id:
                return self.by_id[rid]
            for listing in self.served[-len(ng._listing_paths(REPO, "")):]:
                for run in listing.get("workflow_runs", []) if isinstance(listing, dict) else []:
                    if isinstance(run, dict) and run.get("id") == rid:
                        return run
            raise AssertionError(f"run {rid} was never listed")
        if f"repos/{REPO}/actions/runs?" in path:
            if "head_sha=" in path:
                return self.repo_own
            if "branch=main" in path:
                return self.repo_main
        elif f"repos/{REPO}/actions/workflows/ci.yml/runs?" in path:
            if "head_sha=" in path:
                return self.own
            if "branch=main" in path:
                if isinstance(self.main, Exception):
                    raise self.main
                return self.main
            return self.event_only
        raise AssertionError(f"unexpected path {path}")


def _pick(*listings: object, branch: str | None = "main", sha: str | None = None) -> dict | None:
    return ng.newest_dispatch(list(listings), branch=branch, sha=sha)


def _verdict(api: FakeApi, **env: str) -> list[str]:
    base = {"EVENT_NAME": "pull_request", "HEAD_SHA": SHA, "GITHUB_REF": "", "REPO": REPO}
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
        self.assertIsNone(_pick(_listing()))
        self.assertIsNone(_pick())

    def test_malformed_listing_refused(self) -> None:
        for bad in ({}, [], None, {"workflow_runs": {}}, {"workflow_runs": ["x"]}, _listing(_run(id="1")), _listing(_run(id=True)), _listing(_run(id=0))):
            with self.assertRaises(ng.NightlyError):
                _pick(bad)
            with self.assertRaises(ng.NightlyError):
                _pick(_listing(_run()), bad)

    def test_newest_run_chosen_whatever_the_order(self) -> None:
        old = _run(id=1, created_at="2026-09-22T09:13:24Z")
        new = _run(id=2, created_at="2026-09-29T10:34:21Z")
        for listing in (_listing(old, new), _listing(new, old)):
            chosen = _pick(listing)
            self.assertIsNotNone(chosen)
            self.assertEqual(chosen and chosen["id"], 2)

    def test_unfinished_run_skipped_for_the_newest_completed(self) -> None:
        done = _run(id=1, created_at="2026-09-28T10:50:15Z")
        for status in ("in_progress", "queued", "waiting", None):
            with self.subTest(status):
                running = _run(id=2, status=status, conclusion=None, created_at="2026-09-29T10:34:21Z")
                chosen = _pick(_listing(running, done))
                self.assertEqual(chosen and chosen["id"], 1)
        self.assertIsNone(_pick(_listing(_run(status="in_progress", conclusion=None))))

    def test_listing_is_not_status_filtered(self) -> None:
        api = FakeApi(_listing(_run()))
        _verdict(api)
        self.assertTrue(api.calls)
        for path in api.calls:
            self.assertNotIn("status=", path)

    def test_untimed_entry_refused(self) -> None:
        for created in (None, "yesterday", 5):
            with self.assertRaises(ng.NightlyError):
                _pick(_listing(_run(id=1), _run(id=2, created_at=created)))


class UnionTest(unittest.TestCase):
    """A stale or partial server-side listing can neither hide a fresh nightly nor pass a mis-scoped run."""

    def test_stale_branch_listing_loses_to_fresh_event_listing(self) -> None:
        stale = _listing(_run(id=1, head_sha=SHA, created_at=_stamp(timedelta(days=7))))
        fresh = _run(id=2, created_at=_stamp(timedelta(hours=2)))
        for where in ("event_only", "repo_main"):
            with self.subTest(where):
                api = FakeApi(stale, **{where: _listing(fresh)})
                self.assertEqual(_verdict(api), [])
                self.assertTrue(any(f"/compare/{OTHER}...main" in c for c in api.calls))
        self.assertTrue(any("older than" in r for r in _verdict(FakeApi(stale))))

    def test_mis_scoped_run_never_selected(self) -> None:
        main_green = _run(id=1, created_at=_stamp(timedelta(hours=9)))
        newer = _stamp(timedelta(hours=1))
        for bad in (
            _run(id=2, head_branch="feature", conclusion="failure", created_at=newer),
            _run(id=2, event="push", conclusion="failure", created_at=newer),
            _run(id=2, path=".github/workflows/other.yml", conclusion="failure", created_at=newer),
        ):
            with self.subTest(bad):
                chosen = _pick(_listing(main_green), _listing(bad), _listing(bad))
                self.assertEqual(chosen and chosen["id"], 1)
                api = FakeApi(_listing(main_green), event_only=_listing(bad), repo_main=_listing(bad))
                self.assertEqual(_verdict(api), [])
        for bad in (
            _run(id=2, head_branch="feature", created_at=newer),
            _run(id=2, event="push", created_at=newer),
            _run(id=2, path=".github/workflows/other.yml", created_at=newer),
        ):
            with self.subTest(bad):
                red = _listing(_run(id=1, conclusion="failure"))
                self.assertTrue(_verdict(FakeApi(red, event_only=_listing(bad), repo_main=_listing(bad))))
                self.assertIsNone(_pick(_listing(bad), _listing(bad)))

    def test_commit_scope_never_selects_other_commit(self) -> None:
        other = _run(id=2, head_branch="fix", head_sha=OTHER)
        self.assertIsNone(_pick(_listing(other), branch=None, sha=SHA))
        own = _run(id=3, head_branch="fix", head_sha=SHA, created_at=_stamp(timedelta(days=9)))
        chosen = _pick(_listing(other), _listing(own), branch=None, sha=SHA)
        self.assertEqual(chosen and chosen["id"], 3)

    def test_all_listings_empty_is_red(self) -> None:
        reasons = _verdict(FakeApi(_listing()))
        self.assertTrue(any("no completed dispatched full gate exists" in r for r in reasons))

    def test_every_listing_is_read(self) -> None:
        api = FakeApi(_listing(_run(conclusion="failure")))
        _verdict(api)
        for scope in (f"head_sha={SHA}", "branch=main"):
            for path in ng._listing_paths(REPO, scope):
                self.assertIn(path, api.calls)

    def test_stale_own_listing_loses_to_repo_listing(self) -> None:
        own = _listing(_run(head_branch="fix-nightly", head_sha=SHA))
        api = FakeApi(_listing(_run(conclusion="failure")), repo_own=own)
        self.assertEqual(_verdict(api), [])

    def test_unreadable_listing_is_red(self) -> None:
        for where in ("event_only", "repo_main"):
            with self.subTest(where):
                with self.assertRaises(ng.NightlyError):
                    _verdict(FakeApi(_listing(_run()), **{where: {"total_count": 0}}))

    def test_fresher_copy_of_a_run_wins(self) -> None:
        stale_green = _run(id=5, run_attempt=1, updated_at=_stamp(timedelta(hours=7)))
        fresh_red = _run(id=5, run_attempt=2, conclusion="failure", updated_at=_stamp(timedelta(hours=1)))
        later_red = _run(id=5, run_attempt=1, conclusion="failure", updated_at=_stamp(timedelta(hours=1)))
        rerunning = _run(id=5, run_attempt=2, status="in_progress", conclusion=None, updated_at=_stamp(timedelta(hours=1)))
        for fresh in (fresh_red, later_red):
            for order in ((stale_green, fresh), (fresh, stale_green)):
                with self.subTest(fresh=fresh, order=order):
                    chosen = _pick(_listing(order[0]), _listing(order[1]))
                    self.assertEqual(chosen and chosen["conclusion"], "failure")
                    self.assertTrue(_verdict(FakeApi(_listing(order[0]), event_only=_listing(order[1]))))
        chosen = _pick(_listing(stale_green), _listing(rerunning))
        self.assertEqual(chosen and chosen["status"], "in_progress")

    def test_rerun_in_flight_shadows_an_older_green(self) -> None:
        # A nightly that concluded red and is being re-run has a verdict not yet
        # known; an older green nightly must not stand in for it.
        older_green = _run(id=4, created_at=_stamp(timedelta(hours=30)))
        rerunning = _run(id=5, run_attempt=2, status="in_progress", conclusion=None)
        chosen = _pick(_listing(older_green, rerunning))
        self.assertEqual(chosen and chosen["id"], 5)
        reasons = _verdict(FakeApi(_listing(older_green, rerunning)))
        self.assertTrue(any("status is 'in_progress'" in r for r in reasons), reasons)
        first_run = _run(id=5, status="in_progress", conclusion=None)
        self.assertEqual(_verdict(FakeApi(_listing(older_green, first_run))), [])

    def test_chosen_run_reread_beats_stale_listed_copy(self) -> None:
        stale_green = _run(id=5, run_attempt=1, updated_at=_stamp(timedelta(hours=7)))
        for fresh in (
            _run(id=5, run_attempt=2, conclusion="failure", updated_at=_stamp(timedelta(hours=1))),
            _run(id=5, run_attempt=2, status="in_progress", conclusion=None, updated_at=_stamp(timedelta(hours=1))),
            _run(id=5, run_attempt=1, conclusion="failure", updated_at=_stamp(timedelta(hours=1))),
        ):
            with self.subTest(fresh):
                api = FakeApi(_listing(stale_green), by_id={5: fresh})
                self.assertTrue(_verdict(api))
                self.assertIn(f"repos/{REPO}/actions/runs/5", api.calls)
        self.assertEqual(_verdict(FakeApi(_listing(stale_green), by_id={5: dict(stale_green)})), [])

    def test_chosen_run_reread_malformed_refused(self) -> None:
        listed = _run(id=5)
        for bad in ([], None, _run(id=6), _run(id="5"), _run(id=5, created_at=None), _run(id=5, conclusion="failure")):
            with self.subTest(bad), self.assertRaises(ng.NightlyError):
                _verdict(FakeApi(_listing(listed), by_id={5: bad}))

    def test_equally_fresh_copies_that_disagree_refused(self) -> None:
        a = _run(id=5, updated_at=_stamp(timedelta(hours=1)))
        b = _run(id=5, conclusion="failure", updated_at=_stamp(timedelta(hours=1)))
        with self.assertRaises(ng.NightlyError):
            _pick(_listing(a), _listing(b))
        self.assertEqual(_pick(_listing(a), _listing(dict(a))), a)

    def test_malformed_copy_rank_refused(self) -> None:
        a = _run(id=5)
        for bad in (_run(id=5, run_attempt=0), _run(id=5, run_attempt="2"), _run(id=5, updated_at="soon")):
            with self.subTest(bad), self.assertRaises(ng.NightlyError):
                _pick(_listing(a), _listing(bad))


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

    def test_main_nightly_off_main_refused(self) -> None:
        # A tag or fork branch named `main` reports head_branch "main"; only a
        # commit on main's history proves main.
        for compare in ({"status": "diverged"}, {"status": "behind"}, {}, [], {"status": None}):
            with self.subTest(compare):
                api = FakeApi(_listing(_run()), compare=compare)
                self.assertTrue(any("is not on main" in r for r in _verdict(api)))
                self.assertTrue(any(f"/compare/{OTHER}...main" in c for c in api.calls))

    def test_main_nightly_at_or_behind_main_tip_passes(self) -> None:
        for status in ("identical", "ahead"):
            with self.subTest(status):
                self.assertEqual(_verdict(FakeApi(_listing(_run()), compare={"status": status})), [])

    def test_main_nightly_from_other_repo_refused(self) -> None:
        for head in ({"full_name": "fork/compiler"}, {}, None, "ipe-lang/compiler"):
            with self.subTest(head):
                run = _run(head_repository=head)
                self.assertTrue(any("head repository" in r for r in _verdict(FakeApi(_listing(run)))))

    def test_main_nightly_bad_sha_refused(self) -> None:
        for sha in ("", "zz", None, "A" * 40):
            with self.subTest(sha):
                self.assertTrue(_verdict(FakeApi(_listing(_run(head_sha=sha)))))

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
        self.assertEqual(_verdict(api, EVENT_NAME="merge_group", HEAD_SHA="", GITHUB_REF=ref), [])
        self.assertTrue(any("/pulls/7" in c for c in api.calls))

    def test_merge_group_bad_pr_head_refused(self) -> None:
        ref = f"refs/heads/gh-readonly-queue/main/pr-7-{OTHER}"
        for pr in ({}, {"head": {}}, {"head": {"sha": "zz"}}, []):
            with self.assertRaises(ng.NightlyError):
                _verdict(FakeApi(_listing(_run()), pr=pr), EVENT_NAME="merge_group", HEAD_SHA="", GITHUB_REF=ref)

    def test_bad_repo_refused(self) -> None:
        for bad in ("", "owner", "a/b/c", "a/b?x=1", "../..", "a/.."):
            with self.assertRaises(ng.NightlyError):
                _verdict(FakeApi(_listing(_run())), REPO=bad)

    def test_unreadable_api_is_red_via_main(self) -> None:
        saved = ng._gh_json

        def boom(path: str) -> object:
            raise ng.NightlyError("gh api down")

        ng._gh_json = boom
        env = {"EVENT_NAME": "pull_request", "HEAD_SHA": SHA, "GITHUB_REF": "", "REPO": REPO}
        saved_env = dict(os.environ)
        try:
            os.environ.update(env)
            self.assertEqual(ng.main(["--verdict"]), 1)
        finally:
            ng._gh_json = saved
            os.environ.clear()
            os.environ.update(saved_env)

    def test_usage_refused(self) -> None:
        self.assertEqual(ng.main([]), 2)
        for argv in (["verdict"], ["lint"], ["--verdict", "x"], ["-verdict"], ["--verdict", "--lint"]):
            self.assertEqual(ng.main(argv), 2, argv)


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
        for key, val in (("EVENT_NAME", "pull_request"), ("HEAD_REF", "${{ github.event.merge_group.head_ref }}"), ("GITHUB_REF", "refs/heads/main"), ("HEAD_SHA", "${{ github.sha }}"), ("GH_TOKEN", "${{ secrets.X }}")):
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
