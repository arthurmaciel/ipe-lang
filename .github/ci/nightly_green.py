#!/usr/bin/env python3
"""SSOT for the `nightly-green` gate: a red nightly blocks the next merge.

The nightly is the "Build & test" workflow (ci.yml) dispatched on main by
nightly-full-gate.yml; a dispatch runs every `nightly-gate` job. `nightly-green`
is a required context that passes only on proof the full gate is green:

  1. the change's own commit passed a dispatched full gate (the recovery path:
     a PR that fixes a red nightly is dispatched on its branch, proves itself,
     and can merge), or
  2. the latest completed nightly on main concluded `success`, is at most
     MAX_AGE_H hours old (a stopped nightly is not a green one), and ran on a
     commit of main's own history in this repository (a tag or branch merely
     named `main` proves nothing).

Absence is not a pass: no run, an unreadable listing, an unexpected shape, a
cancelled or stale nightly — each is a red.

Subcommands:
  verdict   exit 0 iff the proof above holds for $EVENT / $HEAD_SHA /
            $HEAD_REF in $REPO; exit 1 otherwise, naming why.
  lint      fail unless nightly-green.yml runs `verdict` unconditionally and
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
GH_TIMEOUT_S = 60
VERDICT_INVOCATION = "python3 .github/ci/nightly_green.py verdict"
EXPECTED_ENV = {
    "EVENT": "${{ github.event_name }}",
    "HEAD_SHA": "${{ github.event.pull_request.head.sha }}",
    "HEAD_REF": "${{ github.event.merge_group.head_ref }}",
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


def latest_run(listing: object) -> dict | None:
    """Return the first run of a `.../runs` listing, or None when it lists none."""
    runs = listing.get("workflow_runs") if isinstance(listing, dict) else None
    if not isinstance(runs, list):
        raise NightlyError("run listing has no `workflow_runs` list")
    if not runs:
        return None
    run = runs[0]
    if not isinstance(run, dict):
        raise NightlyError("run listing entry is not an object")
    return run


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
                errors.append(f"latest nightly is older than {MAX_AGE_H}h (created {created.isoformat()})")
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


def _runs_path(repo: str, query: str) -> str:
    return (
        f"repos/{repo}/actions/workflows/ci.yml/runs"
        f"?event={NIGHTLY_EVENT}&status=completed&per_page=1&{query}"
    )


def verdict(env: dict[str, str], now: datetime) -> list[str]:
    """Return [] when the gate passes, else every reason it does not."""
    repo = env.get("REPO", "")
    if not _REPO.fullmatch(repo):
        raise NightlyError(f"REPO {repo!r} is not owner/name")
    source = change_sha_source(env.get("EVENT", ""), env.get("HEAD_SHA", ""), env.get("HEAD_REF", ""))
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
        own = latest_run(_gh_json(_runs_path(repo, f"head_sha={sha}")))
        own_errors = run_errors(own, branch=None, sha=sha, now=None)
        if not own_errors:
            return []
        reasons += [f"change commit {sha[:12]}: {e}" for e in own_errors]
    main_run = latest_run(_gh_json(_runs_path(repo, f"branch={MAIN}")))
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
    """Return why nightly-green could pass without `verdict` passing, or []."""
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
    if argv == ["verdict"]:
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
    if argv == ["lint"]:
        return lint()
    print("usage: nightly_green.py verdict | lint", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
