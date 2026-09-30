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
ADMIN_SHA = "c" * 40
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


def _admin_run(**over: object) -> dict:
    run = _run(event="schedule", path=".github/workflows/ruleset-admin-read.yml")
    run.update(over)
    return run


def _listing(*runs: dict) -> dict:
    return {"total_count": len(runs), "workflow_runs": list(runs)}


class FakeApi:
    """Route `gh api` paths: a main-nightly listing, an own-commit listing, a PR,
    the admin-read listing (green and fresh unless given)."""

    def __init__(
        self, main: object, own: object = None, pr: object = None, compare: object = None, admin: object = None
    ) -> None:
        self.main, self.own, self.pr = main, own if own is not None else _listing(), pr
        self.compare = compare if compare is not None else {"status": "ahead"}
        self.admin = admin if admin is not None else _listing(_admin_run())
        self.admin_compare: object = None
        self.calls: list[str] = []

    def __call__(self, path: str) -> object:
        self.calls.append(path)
        if "/pulls/" in path:
            if self.pr is None:
                raise ng.NightlyError("no such PR")
            return self.pr
        if "/compare/" in path:
            if self.admin_compare is not None and path.startswith(f"repos/{REPO}/compare/{ADMIN_SHA}"):
                return self.admin_compare
            return self.compare
        if "/workflows/ruleset-admin-read.yml/runs" in path:
            if "event=schedule" not in path or "branch=main" not in path:
                raise AssertionError(f"unexpected admin-read listing {path}")
            if isinstance(self.admin, Exception):
                raise self.admin
            return self.admin
        if "head_sha=" in path:
            return self.own
        if "branch=main" in path:
            if isinstance(self.main, Exception):
                raise self.main
            return self.main
        raise AssertionError(f"unexpected path {path}")


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
        self.assertIsNone(ng.latest_run(_listing()))

    def test_malformed_listing_refused(self) -> None:
        for bad in ({}, [], None, {"workflow_runs": {}}, {"workflow_runs": ["x"]}):
            with self.assertRaises(ng.NightlyError):
                ng.latest_run(bad)

    def test_newest_run_chosen_whatever_the_order(self) -> None:
        old = _run(id=1, created_at="2026-09-22T09:13:24Z")
        new = _run(id=2, created_at="2026-09-29T10:34:21Z")
        for listing in (_listing(old, new), _listing(new, old)):
            chosen = ng.latest_run(listing)
            self.assertIsNotNone(chosen)
            self.assertEqual(chosen and chosen["id"], 2)

    def test_unfinished_run_skipped_for_the_newest_completed(self) -> None:
        done = _run(id=1, created_at="2026-09-28T10:50:15Z")
        for status in ("in_progress", "queued", "waiting", None):
            with self.subTest(status):
                running = _run(id=2, status=status, conclusion=None, created_at="2026-09-29T10:34:21Z")
                chosen = ng.latest_run(_listing(running, done))
                self.assertEqual(chosen and chosen["id"], 1)
        self.assertIsNone(ng.latest_run(_listing(_run(status="in_progress", conclusion=None))))

    def test_listing_is_not_status_filtered(self) -> None:
        api = FakeApi(_listing(_run()))
        _verdict(api)
        self.assertTrue(api.calls)
        for path in api.calls:
            self.assertNotIn("status=", path)

    def test_untimed_entry_refused(self) -> None:
        for created in (None, "yesterday", 5):
            with self.assertRaises(ng.NightlyError):
                ng.latest_run(_listing(_run(id=1), _run(id=2, created_at=created)))


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


class AdminReadTest(unittest.TestCase):
    """The scheduled ruleset admin read is required on every verdict, and only
    a fresh green `schedule` run of main's own history proves it."""

    def reds(self, admin: object, **kw: object) -> list[str]:
        api = FakeApi(_listing(_run()), admin=admin)
        for k, v in kw.items():
            setattr(api, k, v)
        return [r for r in _verdict(api) if r.startswith("ruleset admin read:")]

    def test_green_fresh_admin_read_passes(self) -> None:
        self.assertEqual(self.reds(_listing(_admin_run())), [])
        self.assertEqual(_verdict(FakeApi(_listing(_run()), admin=_listing(_admin_run()))), [])

    def test_missing_admin_read_is_red(self) -> None:
        self.assertTrue(self.reds(_listing()))
        self.assertTrue(self.reds(_listing(_admin_run(status="in_progress", conclusion=None))))

    def test_failed_admin_read_is_red(self) -> None:
        for bad in ("failure", "cancelled", "timed_out", "skipped", "neutral", "action_required", None):
            with self.subTest(bad):
                self.assertTrue(self.reds(_listing(_admin_run(conclusion=bad))))

    def test_stale_admin_read_is_red(self) -> None:
        stale = _admin_run(created_at=_stamp(timedelta(hours=ng.MAX_AGE_H, seconds=1)))
        self.assertTrue(any("older than" in r for r in self.reds(_listing(stale))))
        self.assertEqual(self.reds(_listing(_admin_run(created_at=_stamp(timedelta(hours=ng.MAX_AGE_H))))), [])

    def test_non_schedule_admin_read_is_red(self) -> None:
        for event in ("workflow_dispatch", "push", "pull_request", "merge_group", None):
            with self.subTest(event):
                self.assertTrue(self.reds(_listing(_admin_run(event=event))))

    def test_other_workflow_is_not_an_admin_read(self) -> None:
        self.assertTrue(self.reds(_listing(_admin_run(path=".github/workflows/ci.yml"))))

    def test_non_main_admin_read_is_red(self) -> None:
        self.assertTrue(self.reds(_listing(_admin_run(head_branch="feature"))))
        run = _admin_run(head_sha=ADMIN_SHA)
        for compare in ({"status": "diverged"}, {"status": "behind"}, {}):
            with self.subTest(compare):
                self.assertTrue(self.reds(_listing(run), admin_compare=compare))
        self.assertTrue(self.reds(_listing(_admin_run(head_repository={"full_name": "fork/compiler"}))))

    def test_unreadable_admin_listing_is_red(self) -> None:
        with self.assertRaises(ng.NightlyError):
            self.reds(ng.NightlyError("gh api down"))
        for bad in ({}, [], {"workflow_runs": {}}):
            with self.subTest(bad), self.assertRaises(ng.NightlyError):
                self.reds(bad)

    def test_own_green_dispatch_does_not_stand_in_for_the_admin_read(self) -> None:
        own = _listing(_run(head_branch="fix", head_sha=SHA))
        api = FakeApi(_listing(_run(conclusion="failure")), own=own, admin=_listing(_admin_run(conclusion="failure")))
        reasons = _verdict(api)
        self.assertTrue(reasons)
        self.assertTrue(all(r.startswith("ruleset admin read:") for r in reasons), reasons)

    def test_admin_read_required_on_every_event(self) -> None:
        ref = f"refs/heads/gh-readonly-queue/main/pr-7-{OTHER}"
        red = _listing(_admin_run(conclusion="failure"))
        for env in (
            {},
            {"EVENT_NAME": "merge_group", "HEAD_SHA": "", "GITHUB_REF": ref},
            {"EVENT_NAME": "push", "HEAD_SHA": ""},
            {"EVENT_NAME": "workflow_dispatch", "HEAD_SHA": ""},
        ):
            with self.subTest(env):
                own = _listing(_run(head_branch="fix", head_sha=SHA))
                api = FakeApi(_listing(_run()), own=own, pr={"head": {"sha": SHA}}, admin=red)
                self.assertTrue(_verdict(api, **env))


CHECKOUT_USES = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
SETUP_PYTHON_USES = "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97"
PIP_RUN = (
    "PIP_CONFIG_FILE=/dev/null python3 -m pip install --isolated --require-hashes "
    '--only-binary :all: -r "$GITHUB_WORKSPACE/.github/ci/requirements.txt"'
)
ADMIN_STEP = {
    "name": "admin read",
    "env": {"GH_TOKEN": "${{ secrets.RULESET_READ_TOKEN }}", "REPO": "${{ github.repository }}"},
    "run": "python3 .github/ci/check_required_set.py --fetch-admin",
}
ADMIN_WORKFLOW = {
    "name": "Ruleset admin read",
    "on": {"schedule": [{"cron": "30 4 * * *"}]},
    "permissions": {"contents": "read"},
    "jobs": {
        "ruleset-admin-read": {
            "name": "ruleset-admin-read",
            "runs-on": "ubuntu-latest",
            "environment": "ruleset-admin-read",
            "timeout-minutes": 5,
            "steps": [
                {"uses": CHECKOUT_USES, "with": {"persist-credentials": False}},
                {"uses": SETUP_PYTHON_USES, "with": {"python-version": "3.13"}},
                {"name": "Install PyYAML", "run": PIP_RUN},
                ADMIN_STEP,
            ],
        }
    },
}


def _admin_job(wf: dict) -> dict:
    return wf["jobs"]["ruleset-admin-read"]


def _steps(wf: dict) -> list:
    return _admin_job(wf)["steps"]


ADMIN_MANIFEST = {
    "checks": [{"context": "ruleset-admin-read", "disposition": "nightly-gate", "producer": "ruleset-admin-read.yml"}]
}


class AdminReadWiringTest(unittest.TestCase):
    def test_schedule_only_nightly_gate_passes(self) -> None:
        self.assertEqual(ng.admin_read_wiring_errors(ADMIN_WORKFLOW, ADMIN_MANIFEST), [])
        wf = {k: v for k, v in ADMIN_WORKFLOW.items() if k != "on"}
        self.assertEqual(ng.admin_read_wiring_errors({True: ADMIN_WORKFLOW["on"], **wf}, ADMIN_MANIFEST), [])

    def test_the_fixture_pins_are_the_manifest_pins(self) -> None:
        vm = ng._verify_manifest()
        self.assertIn(CHECKOUT_USES, vm.PINNED_CHECKOUT_USES)
        self.assertIn(SETUP_PYTHON_USES, vm.PINNED_SETUP_PYTHON_USES)
        self.assertEqual(PIP_RUN, vm.CANONICAL_PIP_INSTALL)

    def test_extra_or_other_trigger_refused(self) -> None:
        for on in (
            {"schedule": [], "workflow_dispatch": {}},
            {"schedule": [], "push": {}},
            {"workflow_dispatch": {}},
            "schedule",
            ["schedule"],
            None,
        ):
            with self.subTest(on):
                self.assertTrue(ng.admin_read_wiring_errors(dict(ADMIN_WORKFLOW, on=on), ADMIN_MANIFEST))

    def assertAdminRefused(self, mutate, needle: str) -> None:
        wf = copy.deepcopy(ADMIN_WORKFLOW)
        mutate(wf)
        errors = ng.admin_read_wiring_errors(wf, ADMIN_MANIFEST)
        self.assertTrue(any(needle in e for e in errors), errors)

    def test_workflow_token_read_refused(self) -> None:
        def swapped(wf: dict) -> None:
            step = _steps(wf)[-1]
            step["run"] = "python3 .github/ci/check_required_set.py --fetch"
            step["env"]["GH_TOKEN"] = "${{ github.token }}"

        self.assertAdminRefused(swapped, "(read) `run` must be exactly")
        self.assertAdminRefused(swapped, "(read) `env` must be exactly")
        for run in ("python3 .github/ci/check_required_set.py --fetch",
                    "python3 .github/ci/check_required_set.py --fetch-admin || true",
                    "python3 .github/ci/check_required_set.py",
                    None):
            with self.subTest(run=run):
                self.assertAdminRefused(lambda wf, r=run: _steps(wf)[-1].update(run=r), "(read) `run` must be exactly")
        for env in ({"GH_TOKEN": "${{ github.token }}", "REPO": "${{ github.repository }}"},
                    {"GH_TOKEN": "${{ secrets.GITHUB_TOKEN }}", "REPO": "${{ github.repository }}"},
                    {"GH_TOKEN": "${{ secrets.RULESET_READ_TOKEN }}"},
                    {"GH_TOKEN": "${{ secrets.RULESET_READ_TOKEN }}", "REPO": "o/r"},
                    {**ADMIN_STEP["env"], "GITHUB_API_URL": "https://evil.example"},
                    None):
            with self.subTest(env=env):
                self.assertAdminRefused(lambda wf, e=env: _steps(wf)[-1].update(env=e), "(read) `env` must be exactly")

    def test_step_list_is_pinned(self) -> None:
        self.assertAdminRefused(lambda wf: _steps(wf).pop(), "exactly 4 steps")
        self.assertAdminRefused(lambda wf: _steps(wf).append(dict(ADMIN_STEP)), "exactly 4 steps")
        self.assertAdminRefused(lambda wf: _steps(wf).insert(3, {"run": "echo hi"}), "exactly 4 steps")
        self.assertAdminRefused(lambda wf: _admin_job(wf).pop("steps"), "exactly 4 steps")
        self.assertAdminRefused(lambda wf: _admin_job(wf).update(steps="x"), "exactly 4 steps")
        self.assertAdminRefused(lambda wf: _steps(wf).reverse(), "step 1 (checkout)")
        self.assertAdminRefused(lambda wf: _steps(wf).__setitem__(0, "checkout"), "step 1 (checkout) is not a mapping")

    def test_each_step_is_matched_exactly(self) -> None:
        cases = (
            (0, {"uses": "actions/checkout@v7"}, "step 1 (checkout) `uses`"),
            (0, {"uses": "evil/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"}, "step 1 (checkout) `uses`"),
            (0, {"with": {"persist-credentials": True}}, "step 1 (checkout) `with`"),
            (0, {"with": {"persist-credentials": False, "ref": "evil"}}, "step 1 (checkout) `with`"),
            (0, {"with": None}, "step 1 (checkout) `with`"),
            (1, {"uses": "actions/setup-python@v6"}, "step 2 (setup-python) `uses`"),
            (1, {"with": {"python-version": "3.13", "cache": "pip"}}, "step 2 (setup-python) `with`"),
            (2, {"run": PIP_RUN + " evil"}, "step 3 (pip install) `run`"),
            (2, {"run": "pip install pyyaml"}, "step 3 (pip install) `run`"),
            (3, {"uses": CHECKOUT_USES}, "step 4 (read) must not set ['uses']"),
        )
        for index, update, needle in cases:
            with self.subTest(index=index, update=update):
                self.assertAdminRefused(lambda wf, i=index, u=update: _steps(wf)[i].update(u), needle)

    def test_a_step_may_add_only_a_name(self) -> None:
        for index in range(4):
            for key in ("env", "if", "continue-on-error", "shell", "working-directory", "id", "timeout-minutes"):
                if index == 3 and key == "env":
                    continue
                with self.subTest(index=index, key=key):
                    self.assertAdminRefused(lambda wf, i=index, k=key: _steps(wf)[i].update({k: "x"}), f"must not set ['{key}']")
        self.assertEqual(
            ng.admin_read_wiring_errors(ADMIN_WORKFLOW, ADMIN_MANIFEST), [],
        )

    def test_secrets_outside_the_read_step_refused(self) -> None:
        cases = (
            lambda wf: _steps(wf)[0]["with"].update(token="${{ secrets.RULESET_READ_TOKEN }}"),
            lambda wf: _steps(wf)[2].update(name="${{ secrets.RULESET_READ_TOKEN }}"),
            lambda wf: _admin_job(wf).update({"runs-on": "${{ secrets.RUNNER }}"}),
            lambda wf: _admin_job(wf).update(secrets="inherit"),
            lambda wf: wf.update(name="read ${{ secrets.RULESET_READ_TOKEN }}"),
            lambda wf: wf["on"]["schedule"].append({"cron": "secrets"}),
        )
        for mutate in cases:
            with self.subTest(mutate=mutate):
                self.assertAdminRefused(mutate, "names `secrets`; only the read step's pinned `env` may")
        # The read step's own env is the one sanctioned mention.
        wf = copy.deepcopy(ADMIN_WORKFLOW)
        self.assertFalse([e for e in ng.admin_read_wiring_errors(wf, ADMIN_MANIFEST) if "secrets" in e])

    def test_secrets_scan_covers_a_misshapen_step_list(self) -> None:
        self.assertAdminRefused(
            lambda wf: _steps(wf).append({"env": {"T": "${{ secrets.RULESET_READ_TOKEN }}"}, "run": "curl x"}),
            "names `secrets`",
        )

    def test_the_workflow_keys_are_closed(self) -> None:
        for key, value in (
            ("env", {"GH_HOST": "evil.example"}),
            ("defaults", {"run": {"shell": "bash -c 'curl evil'"}}),
            ("concurrency", {"group": "x", "cancel-in-progress": True}),
            ("run-name", "x"),
            ("x-extra", 1),
        ):
            with self.subTest(key=key):
                self.assertAdminRefused(lambda wf, k=key, v=value: wf.update({k: v}), f"must not set workflow-level ['{key}']")
        self.assertAdminRefused(lambda wf: wf.update({True: wf["on"]}), "sets `on` twice")
        self.assertEqual(ng.admin_read_wiring_errors({k: v for k, v in ADMIN_WORKFLOW.items() if k != "name"},
                                                     ADMIN_MANIFEST), [])

    def test_the_workflow_permissions_are_read_only_contents(self) -> None:
        for perms in ({"contents": "write"}, {"contents": "read", "actions": "read"}, {}, "read-all", "write-all", None):
            with self.subTest(perms=perms):
                self.assertAdminRefused(lambda wf, p=perms: wf.update(permissions=p), "`permissions` must be exactly")
        self.assertAdminRefused(lambda wf: wf.pop("permissions"), "`permissions` must be exactly")

    def test_the_job_keys_are_closed(self) -> None:
        for key, value in (
            ("env", {"GH_HOST": "evil.example"}),
            ("defaults", {"run": {"working-directory": "/tmp"}}),
            ("permissions", {"contents": "write"}),
            ("outputs", {"token": "${{ steps.x.outputs.t }}"}),
            ("container", "evil/image:latest"),
            ("services", {"proxy": {"image": "evil/mitm"}}),
            ("concurrency", "x"),
            ("if", "false"),
            ("continue-on-error", True),
            ("needs", "other"),
            ("strategy", {"matrix": {"n": [1, 2]}}),
            ("uses", "./.github/workflows/other.yml"),
        ):
            with self.subTest(key=key):
                self.assertAdminRefused(lambda wf, k=key, v=value: _admin_job(wf).update({k: v}), f"must not set ['{key}']")
        job = {k: v for k, v in _admin_job(ADMIN_WORKFLOW).items() if k != "name"}
        self.assertEqual(ng.admin_read_wiring_errors(dict(ADMIN_WORKFLOW, jobs={"ruleset-admin-read": job}),
                                                     ADMIN_MANIFEST), [])

    def test_the_job_runs_on_the_hosted_ubuntu_latest_label(self) -> None:
        for runs_on in ("self-hosted", ["self-hosted", "linux"], "ubuntu-24.04", "Ubuntu-Latest",
                        "${{ 'ubuntu-latest' }}", {"group": "x"}, None):
            with self.subTest(runs_on=runs_on):
                self.assertAdminRefused(lambda wf, r=runs_on: _admin_job(wf).update({"runs-on": r}),
                                        "must declare `runs-on: ubuntu-latest`")
        self.assertAdminRefused(lambda wf: _admin_job(wf).pop("runs-on"), "must declare `runs-on: ubuntu-latest`")

    def test_the_job_timeout_is_a_positive_integer_literal(self) -> None:
        for minutes in ("${{ 5 }}", "5", 0, -1, 5.0, True, None):
            with self.subTest(minutes=minutes):
                self.assertAdminRefused(lambda wf, m=minutes: _admin_job(wf).update({"timeout-minutes": m}),
                                        "`timeout-minutes` must be a positive integer literal")
        self.assertAdminRefused(lambda wf: _admin_job(wf).pop("timeout-minutes"),
                                "`timeout-minutes` must be a positive integer literal")

    def test_secrets_are_matched_in_any_case(self) -> None:
        for spelling in ("SECRETS", "Secrets", "sEcReTs"):
            with self.subTest(spelling=spelling):
                self.assertAdminRefused(
                    lambda wf, w=spelling: _steps(wf)[0]["with"].update(token=f"${{{{ {w}.RULESET_READ_TOKEN }}}}"),
                    "names `secrets`",
                )
                self.assertAdminRefused(lambda wf, w=spelling: wf.update(name=f"read ${{{{ {w}.X }}}}"), "names `secrets`")

    def test_the_read_step_name_is_scanned(self) -> None:
        self.assertAdminRefused(lambda wf: _steps(wf)[-1].update(name="${{ secrets.RULESET_READ_TOKEN }}"),
                                "steps[3].name names `secrets`")
        self.assertAdminRefused(lambda wf: _steps(wf)[-1].update(name="${{ SECRETS.RULESET_READ_TOKEN }}"),
                                "steps[3].name names `secrets`")

    def test_a_drifted_read_env_is_not_sanctioned(self) -> None:
        self.assertAdminRefused(
            lambda wf: _steps(wf)[-1]["env"].update(EXTRA="${{ secrets.OTHER }}"),
            "steps[3].env.EXTRA names `secrets`",
        )

    def test_the_parse_yields_the_typed_job(self) -> None:
        vm = ng._verify_manifest()
        parsed = ng.parse_admin_read_workflow(copy.deepcopy(ADMIN_WORKFLOW))
        self.assertIsInstance(parsed, ng.AdminReadJob)
        self.assertEqual(parsed.runner, vm.RunnerLabel("ubuntu-latest"))
        self.assertEqual(parsed.timeout, vm.Timeout(5))
        self.assertEqual(parsed.environment, "ruleset-admin-read")
        self.assertEqual(parsed.read_step["run"], ng.ADMIN_READ_INVOCATION)
        for bad in (None, [], "x"):
            with self.subTest(bad=bad):
                self.assertEqual(ng.parse_admin_read_workflow(bad), ["ruleset-admin-read.yml is not a mapping"])

    def test_environment_refused_unless_the_admin_read_environment(self) -> None:
        for env in (None, "prod", "Ruleset-Admin-Read", {"name": "ruleset-admin-read"}, "${{ 'ruleset-admin-read' }}"):
            with self.subTest(env=env):
                self.assertAdminRefused(lambda wf, e=env: _admin_job(wf).update(environment=e), "environment: ruleset-admin-read")
        self.assertAdminRefused(lambda wf: _admin_job(wf).pop("environment"), "environment: ruleset-admin-read")

    def test_job_shape_refused(self) -> None:
        self.assertAdminRefused(lambda wf: wf["jobs"].update(other={"steps": []}), "exactly one job")
        self.assertAdminRefused(lambda wf: wf.update(jobs={"x": wf["jobs"]["ruleset-admin-read"]}), "exactly one job")
        self.assertAdminRefused(lambda wf: wf.update(jobs=None), "exactly one job")
        self.assertAdminRefused(lambda wf: _admin_job(wf).update(name="other"), "must report context")

    def test_manifest_drift_refused(self) -> None:
        for manifest in (
            {"checks": []},
            {"checks": [dict(ADMIN_MANIFEST["checks"][0], disposition="informational")]},
            {"checks": [dict(ADMIN_MANIFEST["checks"][0], producer="ci.yml")]},
            {"checks": ADMIN_MANIFEST["checks"] * 2},
            None,
        ):
            with self.subTest(manifest):
                self.assertTrue(ng.admin_read_wiring_errors(ADMIN_WORKFLOW, manifest))


def _normalised(text: str) -> str:
    return " ".join(" ".join(line.strip().lstrip("#") for line in text.splitlines()).split())


class AdminReadRecoveryTest(unittest.TestCase):
    """Every home of the admin-read recovery advice carries `ADMIN_READ_RECOVERY` verbatim."""

    def test_every_home_states_the_one_recovery(self) -> None:
        want = _normalised(ng.ADMIN_READ_RECOVERY)
        homes = {"nightly_green.py docstring": ng.__doc__ or ""}
        for home, path in (
            ("nightly-green.yml", os.path.join(ng.REPO_ROOT, ".github", "workflows", ng.WORKFLOW_FILE)),
            ("ruleset-admin-read.yml", os.path.join(ng.REPO_ROOT, ng.ADMIN_READ.path)),
            ("RECONCILIATION.md", os.path.join(HERE, "RECONCILIATION.md")),
        ):
            with open(path, encoding="utf-8") as fh:
                homes[home] = fh.read()
        for home, text in homes.items():
            with self.subTest(home=home):
                self.assertIn(want, _normalised(text))

    def test_the_recovery_never_offers_a_rerun_as_the_fix(self) -> None:
        self.assertIn("never makes a stale run fresh", ng.ADMIN_READ_RECOVERY)
        self.assertNotIn("re-run with `gh run rerun`", ng.ADMIN_READ_RECOVERY)
        self.assertNotIn("recovered with `gh run rerun`", ng.ADMIN_READ_RECOVERY)

    def test_the_red_verdict_prints_the_recovery(self) -> None:
        import contextlib
        import io
        from unittest import mock

        err = io.StringIO()
        with mock.patch.object(ng, "verdict", return_value=["x"]), contextlib.redirect_stderr(err):
            self.assertEqual(ng.main(["--verdict"]), 1)
        self.assertIn(ng.ADMIN_READ_RECOVERY, err.getvalue())


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
