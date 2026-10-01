#!/usr/bin/env python3
"""SSOT for the `nightly-green` gate: a red nightly blocks the next merge.

The nightly is the "Build & test" workflow (ci.yml) dispatched on main by
nightly-full-gate.yml; a dispatch runs every `nightly-gate` job. `nightly-green`
is a required context that passes only on proof the full gate is green:

  1. the change's own commit passed a dispatched full gate (the recovery path:
     a PR that fixes a red nightly is dispatched on its branch, proves itself,
     and can merge), or
  2. the latest concluded nightly on main (a re-run in flight counts, its
     verdict not yet known) concluded `success`, is at most
     MAX_AGE_H hours old (a stopped nightly is not a green one), and ran on a
     commit of main's own history in this repository (a tag or branch merely
     named `main` proves nothing).

Absence is not a pass: no run, an unreadable listing, an unexpected shape, a
cancelled or stale nightly — each is a red.

Modes:
  --verdict  exit 0 iff the proof above holds for $EVENT_NAME / $HEAD_SHA in
             $REPO (a merge-group change is read from the runner's own
             $GITHUB_REF, the queue ref); exit 1 otherwise, naming why.
  --lint     fail unless nightly-green.yml runs `--verdict` unconditionally and
             the manifest declares `nightly-green` a gate. manifest-guard runs it.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from datetime import datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(os.path.dirname(HERE))
CONTEXT = "nightly-green"
WORKFLOW_FILE = "nightly-green.yml"
NIGHTLY_WORKFLOW_PATH = ".github/workflows/ci.yml"
NIGHTLY_EVENT = "workflow_dispatch"
MAIN = "main"
MAX_AGE_H = 48
# Runs fetched per listing; the newest matching run across all listings is judged.
LISTING_PAGE = 20
# Fields a run is judged on; two equally fresh copies of one run must agree on them.
_JUDGED_FIELDS = ("event", "path", "status", "conclusion", "head_branch", "head_sha", "created_at")
GH_TIMEOUT_S = 60
VERDICT_INVOCATION = "python3 .github/ci/nightly_green.py --verdict"
EXPECTED_ENV = {
    "EVENT_NAME": "${{ github.event_name }}",
    "HEAD_SHA": "${{ github.event.pull_request.head.sha }}",
    "REPO": "${{ github.repository }}",
    "GH_TOKEN": "${{ github.token }}",
}

_SHA = re.compile(r"[0-9a-f]{40}")
_REPO = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*")
_QUEUE_REF = re.compile(r"refs/heads/gh-readonly-queue/main/pr-([1-9][0-9]{0,9})-[0-9a-f]{40}")


class NightlyError(Exception):
    """A listing, run, or event shape that proves nothing."""


def _parse_time(text: object) -> datetime:
    if not isinstance(text, str) or not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", text):
        raise NightlyError(f"run timestamp {text!r} is not an ISO-8601 UTC instant")
    return datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def _run_id(run: dict) -> int:
    rid = run.get("id")
    if not isinstance(rid, int) or isinstance(rid, bool) or rid <= 0:
        raise NightlyError(f"run listing entry id {rid!r} is not a positive integer")
    return rid


def _attempt(run: dict) -> int:
    attempt = run.get("run_attempt", 1)
    if not isinstance(attempt, int) or isinstance(attempt, bool) or attempt <= 0:
        raise NightlyError(f"run {run.get('id')} attempt {attempt!r} is not a positive integer")
    return attempt


def _judged(run: dict) -> tuple[object, ...]:
    head_repo = run.get("head_repository")
    return (
        *(run.get(key) for key in _JUDGED_FIELDS),
        head_repo.get("full_name") if isinstance(head_repo, dict) else head_repo,
    )


def _fresher(a: dict, b: dict) -> dict:
    """Return the copy of one run that reflects its later state; copies equally fresh must agree."""
    rank_a = (_attempt(a), _parse_time(a.get("updated_at", a.get("created_at"))))
    rank_b = (_attempt(b), _parse_time(b.get("updated_at", b.get("created_at"))))
    if rank_a != rank_b:
        return a if rank_a > rank_b else b
    if _judged(a) != _judged(b):
        raise NightlyError(f"run listings disagree on run {a.get('id')} at the same attempt and update time")
    return a


def newest_dispatch(listings: list[object], *, branch: str | None, sha: str | None) -> dict | None:
    """Return the newest completed nightly-workflow dispatch on `branch` / at `sha` across `listings`, or None.

    No single listing is trusted to be complete, ordered, or filtered as asked:
    a server-side filter can serve a stale index or drop recent runs. Every
    listing is read, runs are unioned by id (the freshest copy of each wins),
    and selection happens here: event, workflow path, branch or commit, and
    completion are matched client-side, and the newest by `created_at` is
    chosen. A run re-running (attempt above 1, not completed) already
    concluded once, so it is a candidate: it shadows every older run and is
    judged unfinished, never skipped for an older green. A listing only ever
    omits runs, so the union is at least as fresh as any one listing. Any
    malformed listing or entry refuses.
    """
    union: dict[int, dict] = {}
    for listing in listings:
        runs = listing.get("workflow_runs") if isinstance(listing, dict) else None
        if not isinstance(runs, list):
            raise NightlyError("run listing has no `workflow_runs` list")
        for run in runs:
            if not isinstance(run, dict):
                raise NightlyError("run listing entry is not an object")
            rid = _run_id(run)
            _attempt(run)
            _parse_time(run.get("created_at"))
            seen = union.get(rid)
            union[rid] = run if seen is None else _fresher(seen, run)
    newest: tuple[datetime, dict] | None = None
    for run in union.values():
        if (
            (run.get("status") != "completed" and _attempt(run) == 1)
            or run.get("event") != NIGHTLY_EVENT
            or run.get("path") != NIGHTLY_WORKFLOW_PATH
            or (branch is not None and run.get("head_branch") != branch)
            or (sha is not None and run.get("head_sha") != sha)
        ):
            continue
        created = _parse_time(run.get("created_at"))
        if newest is None or created > newest[0]:
            newest = (created, run)
    return None if newest is None else newest[1]


def run_errors(run: dict | None, *, branch: str | None, sha: str | None, now: datetime | None) -> list[str]:
    """Return why `run` is not a green dispatched full gate, or [] when it is.

    `branch`/`sha` pin where it must have run; `now` (when given) bounds its age.
    """
    if run is None:
        return ["no completed dispatched full gate exists"]
    errors: list[str] = []
    if run.get("event") != NIGHTLY_EVENT:
        errors.append(f"run event is {run.get('event')!r}, not {NIGHTLY_EVENT!r}")
    if run.get("path") != NIGHTLY_WORKFLOW_PATH:
        errors.append(f"run workflow is {run.get('path')!r}, not {NIGHTLY_WORKFLOW_PATH!r}")
    if run.get("status") != "completed":
        errors.append(f"run status is {run.get('status')!r}, not 'completed'")
    if run.get("conclusion") != "success":
        errors.append(f"run {run.get('html_url', run.get('id'))} concluded {run.get('conclusion')!r}")
    if branch is not None and run.get("head_branch") != branch:
        errors.append(f"run branch is {run.get('head_branch')!r}, not {branch!r}")
    if sha is not None and run.get("head_sha") != sha:
        errors.append(f"run commit is {run.get('head_sha')!r}, not {sha!r}")
    if now is not None:
        try:
            created = _parse_time(run.get("created_at"))
        except NightlyError as exc:
            errors.append(str(exc))
        else:
            if created > now + timedelta(minutes=5):
                errors.append(f"run was created in the future ({created.isoformat()})")
            elif now - created > timedelta(hours=MAX_AGE_H):
                errors.append(
                    f"latest nightly is older than {MAX_AGE_H}h (run {run.get('id')}, created {created.isoformat()})"
                )
    return errors


def ancestry_errors(sha: object, compare: object, repo: str, run_repo: object) -> list[str]:
    """Return why a `compare/<sha>...main` result does not put `sha` on main, or []."""
    errors: list[str] = []
    if run_repo != repo:
        errors.append(f"run head repository is {run_repo!r}, not {repo!r}")
    status = compare.get("status") if isinstance(compare, dict) else None
    if status not in ("identical", "ahead"):
        errors.append(f"run commit {str(sha)[:12]} is not on {MAIN} (compare status {status!r})")
    return errors


def on_main_errors(repo: str, run: dict) -> list[str]:
    sha = run.get("head_sha")
    if not isinstance(sha, str) or not _SHA.fullmatch(sha):
        return [f"run commit {sha!r} is not a 40-hex commit"]
    head_repo = run.get("head_repository")
    run_repo = head_repo.get("full_name") if isinstance(head_repo, dict) else None
    return ancestry_errors(sha, _gh_json(f"repos/{repo}/compare/{sha}...{MAIN}"), repo, run_repo)


def change_sha_source(event: str, head_sha: str, head_ref: str) -> tuple[str, str] | None:
    """Return how to find the change's own commit: ("sha", s) or ("pr", n).

    None means the event carries no change commit (a push or a dispatch on
    main), so only the main nightly can prove it. An event this gate does not
    know, or a malformed payload value, is refused.
    """
    if event == "pull_request":
        if not _SHA.fullmatch(head_sha):
            raise NightlyError(f"pull_request head sha {head_sha!r} is not a 40-hex commit")
        return ("sha", head_sha)
    if event == "merge_group":
        m = _QUEUE_REF.fullmatch(head_ref)
        if not m:
            raise NightlyError(f"merge_group head ref {head_ref!r} is not a main queue ref")
        return ("pr", m.group(1))
    if event in ("push", "workflow_dispatch"):
        return None
    raise NightlyError(f"event {event!r} is not one nightly-green judges")


def _gh_json(path: str) -> object:
    proc = subprocess.run(
        ["gh", "api", path], check=False, capture_output=True, text=True, timeout=GH_TIMEOUT_S
    )
    if proc.returncode != 0:
        raise NightlyError(f"`gh api {path}` exited {proc.returncode}: {proc.stderr.strip()}")
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        raise NightlyError(f"`gh api {path}` returned non-JSON: {exc}") from exc


def _listing_paths(repo: str, scope: str) -> tuple[str, ...]:
    """Independent `.../runs` listings that each may hold the dispatches in `scope`.

    The workflow listing filtered by event and scope, the workflow listing
    filtered by event only, and the repository listing filtered by event and
    scope are separate server-side indexes; none is trusted alone.
    """
    workflow = NIGHTLY_WORKFLOW_PATH.rsplit("/", 1)[-1]
    page = f"event={NIGHTLY_EVENT}&per_page={LISTING_PAGE}"
    return (
        f"repos/{repo}/actions/workflows/{workflow}/runs?{page}&{scope}",
        f"repos/{repo}/actions/workflows/{workflow}/runs?{page}",
        f"repos/{repo}/actions/runs?{page}&{scope}",
    )


def _reread(repo: str, run: dict) -> dict:
    """Return the freshest of `run`'s listed copy and its own `actions/runs/<id>` document.

    Every listing can carry a stale copy of the chosen run (an earlier attempt's
    verdict); the run's own document is read and the later state judged.
    """
    rid = _run_id(run)
    fresh = _gh_json(f"repos/{repo}/actions/runs/{rid}")
    if not isinstance(fresh, dict) or _run_id(fresh) != rid:
        raise NightlyError(f"run {rid} document is not that run")
    _parse_time(fresh.get("created_at"))
    return _fresher(run, fresh)


def _newest(repo: str, scope: str, *, branch: str | None, sha: str | None) -> dict | None:
    listings = [_gh_json(path) for path in _listing_paths(repo, scope)]
    chosen = newest_dispatch(listings, branch=branch, sha=sha)
    return None if chosen is None else _reread(repo, chosen)


def verdict(env: dict[str, str], now: datetime) -> list[str]:
    """Return [] when the gate passes, else every reason it does not."""
    repo = env.get("REPO", "")
    if not _REPO.fullmatch(repo):
        raise NightlyError(f"REPO {repo!r} is not owner/name")
    source = change_sha_source(env.get("EVENT_NAME", ""), env.get("HEAD_SHA", ""), env.get("GITHUB_REF", ""))
    reasons: list[str] = []
    if source is not None:
        kind, value = source
        sha = value
        if kind == "pr":
            pr = _gh_json(f"repos/{repo}/pulls/{value}")
            head = pr.get("head") if isinstance(pr, dict) else None
            sha = head.get("sha") if isinstance(head, dict) else None
            if not isinstance(sha, str) or not _SHA.fullmatch(sha):
                raise NightlyError(f"PR #{value} has no 40-hex head sha")
        own = _newest(repo, f"head_sha={sha}", branch=None, sha=sha)
        own_errors = run_errors(own, branch=None, sha=sha, now=None)
        if not own_errors:
            return []
        reasons += [f"change commit {sha[:12]}: {e}" for e in own_errors]
    main_run = _newest(repo, f"branch={MAIN}", branch=MAIN, sha=None)
    main_errors = run_errors(main_run, branch=MAIN, sha=None, now=now)
    if not main_errors and main_run is not None:
        # `head_branch` is only a name: a tag or a fork branch called `main`
        # carries it too. The run proves main only if its commit is on main.
        main_errors = on_main_errors(repo, main_run)
    if not main_errors:
        return []
    return reasons + [f"main nightly: {e}" for e in main_errors]


def _strip_expr(text: str) -> str:
    return " ".join(text.split())


def wiring_errors(workflow: object, manifest: object) -> list[str]:
    """Return why nightly-green could pass without `--verdict` passing, or []."""
    errors: list[str] = []
    jobs = workflow.get("jobs") if isinstance(workflow, dict) else None
    if not isinstance(jobs, dict) or len(jobs) != 1:
        return [f"{WORKFLOW_FILE} must define exactly one job"]
    (job_id, job), = jobs.items()
    if not isinstance(job, dict):
        return [f"{WORKFLOW_FILE} job {job_id!r} is not a mapping"]
    if job.get("name", job_id) != CONTEXT:
        errors.append(f"the job must report context {CONTEXT!r}")
    for key in ("if", "continue-on-error", "needs", "strategy"):
        if key in job:
            errors.append(f"the job must not set `{key}` (the gate runs unconditionally, once)")
    steps = job.get("steps")
    if not isinstance(steps, list):
        return errors + ["the job has no steps list"]
    verdict_steps = [s for s in steps if isinstance(s, dict) and VERDICT_INVOCATION in str(s.get("run", ""))]
    if len(verdict_steps) != 1:
        errors.append(f"exactly one step must run `{VERDICT_INVOCATION}`")
    for step in steps:
        if isinstance(step, dict) and ("continue-on-error" in step or "if" in step):
            errors.append(f"step {step.get('name', step.get('uses'))!r} must not set `if` or `continue-on-error`")
    if len(verdict_steps) == 1:
        env = verdict_steps[0].get("env")
        if not isinstance(env, dict) or {k: _strip_expr(str(v)) for k, v in env.items()} != EXPECTED_ENV:
            errors.append(f"the verdict step's env must be exactly {EXPECTED_ENV}")
        run = str(verdict_steps[0].get("run", "")).strip()
        if run != VERDICT_INVOCATION:
            errors.append(f"the verdict step must run only `{VERDICT_INVOCATION}`, got {run!r}")
    entries = manifest.get("checks") if isinstance(manifest, dict) else None
    mine = [e for e in entries or [] if isinstance(e, dict) and e.get("context") == CONTEXT]
    if len(mine) != 1 or mine[0].get("disposition") != "gate" or mine[0].get("producer") != WORKFLOW_FILE:
        errors.append(f"check-manifest.yml must declare {CONTEXT!r} once as a `gate` produced by {WORKFLOW_FILE}")
    return errors


def lint(root: str = REPO_ROOT) -> int:
    sys.path.insert(0, HERE)
    import strict_yaml  # noqa: PLC0415  # PyYAML-backed; only `lint` needs it

    try:
        with open(os.path.join(root, ".github", "workflows", WORKFLOW_FILE), encoding="utf-8") as fh:
            workflow = strict_yaml.safe_load(fh)
        with open(os.path.join(root, ".github", "ci", "check-manifest.yml"), encoding="utf-8") as fh:
            manifest = strict_yaml.safe_load(fh)
    except Exception as exc:  # noqa: BLE001  # any read or parse failure is a refusal
        print(f"nightly-green lint: unreadable input: {exc}", file=sys.stderr)
        return 1
    errors = wiring_errors(workflow, manifest)
    for err in errors:
        print(f"nightly-green lint: {err}", file=sys.stderr)
    if errors:
        return 1
    print(f"nightly-green lint: {WORKFLOW_FILE} runs the verdict unconditionally; the manifest gates it.")
    return 0


def main(argv: list[str]) -> int:
    if argv == ["--verdict"]:
        try:
            reasons = verdict(dict(os.environ), datetime.now(timezone.utc))
        except (NightlyError, OSError, subprocess.SubprocessError) as exc:
            reasons = [str(exc)]
        for reason in reasons:
            print(f"nightly-green: {reason}", file=sys.stderr)
        if reasons:
            print(
                "nightly-green: RED — fix main's nightly, or prove this commit with "
                "`gh workflow run 'Build & test' --ref <branch>` and re-run this check.",
                file=sys.stderr,
            )
            return 1
        print("nightly-green: the full gate is green.")
        return 0
    if argv == ["--lint"]:
        return lint()
    print("usage: nightly_green.py --verdict | --lint", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
