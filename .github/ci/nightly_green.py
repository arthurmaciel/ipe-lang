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

and, always, on proof the nightly ruleset admin read is green:

  3. the latest completed `schedule` run of ruleset-admin-read.yml on main
     concluded `success`, is at most MAX_AGE_H hours old, and ran on a commit
     of main's own history. It proves the live ruleset has no bypass actor, a
     property of the repository rather than of any change, so no change's own
     run stands in for it. A red ruleset admin read caused by the ruleset or
     its token is fixed there, and the next scheduled run proves the fix (a
     `gh run rerun` keeps the run's `created_at`, so it never makes a stale
     run fresh); one caused by main's tree needs the break-glass sequence in
     .github/ci/RECONCILIATION.md. (`ADMIN_READ_RECOVERY`)

Absence is not a pass: no run, an unreadable listing, an unexpected shape, a
cancelled or stale nightly — each is a red.

Modes:
  --verdict  exit 0 iff the proof above holds for $EVENT_NAME / $HEAD_SHA in
             $REPO (a merge-group change is read from the runner's own
             $GITHUB_REF, the queue ref); exit 1 otherwise, naming why.
  --lint     fail unless nightly-green.yml runs `--verdict` unconditionally,
             the manifest declares `nightly-green` a gate and
             `ruleset-admin-read` a nightly-gate of ruleset-admin-read.yml,
             and that workflow triggers on `schedule` alone, with one job in
             the admin-read environment running exactly the pinned steps, the
             last one only `check_required_set.py --fetch-admin` with the
             admin token, and no other `secrets` or `env`
             (`admin_read_wiring_errors`). manifest-guard runs it.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(os.path.dirname(HERE))
CONTEXT = "nightly-green"
WORKFLOW_FILE = "nightly-green.yml"
MAIN = "main"


@dataclass(frozen=True)
class Proof:
    """A workflow whose run on main is evidence: its file, and the one event
    whose runs count."""

    workflow: str
    event: str
    what: str

    @property
    def path(self) -> str:
        return f".github/workflows/{self.workflow}"


FULL_GATE = Proof("ci.yml", "workflow_dispatch", "dispatched full gate")
ADMIN_READ = Proof("ruleset-admin-read.yml", "schedule", "scheduled ruleset admin read")
MAX_AGE_H = 48
# Runs fetched per listing; the newest of them is judged, whatever their order.
LISTING_PAGE = 20
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


def latest_run(listing: object) -> dict | None:
    """Return the newest-created completed run of a `.../runs` listing, or None when it lists none.

    Neither the listing's order nor its filters are trusted: the API's
    `status=completed` filter drops recent runs, so the listing is unfiltered and
    a run still queued or in progress is skipped here; the newest remaining run
    is chosen by `created_at`.
    """
    runs = listing.get("workflow_runs") if isinstance(listing, dict) else None
    if not isinstance(runs, list):
        raise NightlyError("run listing has no `workflow_runs` list")
    newest: tuple[datetime, dict] | None = None
    for run in runs:
        if not isinstance(run, dict):
            raise NightlyError("run listing entry is not an object")
        if run.get("status") != "completed":
            continue
        created = _parse_time(run.get("created_at"))
        if newest is None or created > newest[0]:
            newest = (created, run)
    return None if newest is None else newest[1]


def run_errors(
    run: dict | None, *, branch: str | None, sha: str | None, now: datetime | None, proof: Proof = FULL_GATE
) -> list[str]:
    """Return why `run` is not a green run of `proof`, or [] when it is.

    `branch`/`sha` pin where it must have run; `now` (when given) bounds its age.
    """
    if run is None:
        return [f"no completed {proof.what} exists"]
    errors: list[str] = []
    if run.get("event") != proof.event:
        errors.append(f"run event is {run.get('event')!r}, not {proof.event!r}")
    if run.get("path") != proof.path:
        errors.append(f"run workflow is {run.get('path')!r}, not {proof.path!r}")
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


def _runs_path(repo: str, query: str, proof: Proof = FULL_GATE) -> str:
    return (
        f"repos/{repo}/actions/workflows/{proof.workflow}/runs"
        f"?event={proof.event}&per_page={LISTING_PAGE}&{query}"
    )


def main_run_errors(repo: str, proof: Proof, now: datetime) -> list[str]:
    """Return why the latest completed run of `proof` on main is not a fresh
    green run of main's own history, or []."""
    run = latest_run(_gh_json(_runs_path(repo, f"branch={MAIN}", proof)))
    errors = run_errors(run, branch=MAIN, sha=None, now=now, proof=proof)
    if not errors and run is not None:
        # `head_branch` is only a name: a tag or a fork branch called `main`
        # carries it too. The run proves main only if its commit is on main.
        errors = on_main_errors(repo, run)
    return errors


def verdict(env: dict[str, str], now: datetime) -> list[str]:
    """Return [] when the gate passes, else every reason it does not."""
    repo = env.get("REPO", "")
    if not _REPO.fullmatch(repo):
        raise NightlyError(f"REPO {repo!r} is not owner/name")
    source = change_sha_source(env.get("EVENT_NAME", ""), env.get("HEAD_SHA", ""), env.get("GITHUB_REF", ""))
    admin = [f"ruleset admin read: {e}" for e in main_run_errors(repo, ADMIN_READ, now)]
    return full_gate_reasons(repo, source, now) + admin


def full_gate_reasons(repo: str, source: tuple[str, str] | None, now: datetime) -> list[str]:
    """Return [] when the change's own dispatch or main's nightly proves the
    full gate green, else every reason neither does."""
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
    main_errors = main_run_errors(repo, FULL_GATE, now)
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


ADMIN_READ_CONTEXT = "ruleset-admin-read"
ADMIN_READ_INVOCATION = "python3 .github/ci/check_required_set.py --fetch-admin"
ADMIN_READ_ENV = {
    "GH_TOKEN": "${{ secrets.RULESET_READ_TOKEN }}",
    "REPO": "${{ github.repository }}",
}
# How a red `ADMIN_READ` run is recovered; the docstring, the `--verdict` RED
# message and nightly-green.yml's header carry it verbatim (a test holds them).
ADMIN_READ_RECOVERY = (
    "A red ruleset admin read caused by the ruleset or its token is fixed there, "
    "and the next scheduled run proves the fix (a `gh run rerun` keeps the run's "
    "`created_at`, so it never makes a stale run fresh); one caused by main's tree "
    "needs the break-glass sequence in .github/ci/RECONCILIATION.md."
)
# Keys that would put an `env:` or a shell under the read step that its own
# pinned `env:` and `run:` do not show.
_ADMIN_READ_SCOPE_KEYS = ("env", "defaults")


def _verify_manifest() -> object:
    """Load verify-manifest.py, the SSOT of the pinned actions and the pip install."""
    import importlib.util  # noqa: PLC0415  # only the admin-read lint needs it

    loaded = sys.modules.get("verify_manifest")
    if loaded is not None:
        return loaded
    if HERE not in sys.path:
        sys.path.insert(0, HERE)
    spec = importlib.util.spec_from_file_location("verify_manifest", os.path.join(HERE, "verify-manifest.py"))
    if spec is None or spec.loader is None:
        raise NightlyError("verify-manifest.py is not loadable")
    module = importlib.util.module_from_spec(spec)
    sys.modules["verify_manifest"] = module  # dataclasses resolve their module by name
    try:
        spec.loader.exec_module(module)
    except BaseException:
        del sys.modules["verify_manifest"]
        raise
    return module


def admin_read_steps() -> list[tuple[str, dict[str, object]]]:
    """Return the admin-read job's steps in order, each as (what, pinned
    fields): a checkout without persisted credentials, the interpreter setup,
    the canonical hash-checked pip install, and the read. A step may add only a
    `name`; `uses` is matched against its pin set, every other field exactly."""
    vm = _verify_manifest()
    return [
        ("checkout", {"uses": vm.PINNED_CHECKOUT_USES, "with": {"persist-credentials": False}}),
        ("setup-python", {"uses": vm.PINNED_SETUP_PYTHON_USES, "with": {"python-version": "3.13"}}),
        ("pip install", {"run": vm.CANONICAL_PIP_INSTALL}),
        ("read", {"run": ADMIN_READ_INVOCATION, "env": ADMIN_READ_ENV}),
    ]


def _step_errors(index: int, step: object, what: str, pinned: dict[str, object]) -> list[str]:
    where = f"{ADMIN_READ.workflow} step {index + 1} ({what})"
    if not isinstance(step, dict):
        return [f"{where} is not a mapping"]
    errors: list[str] = []
    extra = sorted(str(k) for k in set(step) - set(pinned) - {"name"})
    if extra:
        errors.append(f"{where} must not set {extra} (only `name` and {sorted(pinned)})")
    for key, want in pinned.items():
        got = step.get(key)
        if isinstance(want, frozenset):
            ok = isinstance(got, str) and got in want
        elif isinstance(want, dict):
            ok = isinstance(got, dict) and {str(k): _strip_expr(v) if isinstance(v, str) else v
                                            for k, v in got.items()} == want
        else:
            ok = isinstance(got, str) and got.strip() == want
        if not ok:
            shown = sorted(want) if isinstance(want, frozenset) else want
            errors.append(f"{where} `{key}` must be exactly {shown!r}, got {got!r}")
    return errors


def _secret_mentions(node: object, skip: object, path: str) -> list[str]:
    """Return the path of every key or scalar under `node` naming `secrets`,
    skipping the one object `skip` (the read step)."""
    if node is skip:
        return []
    if isinstance(node, dict):
        found = [f"{path}.{k}" for k in node if "secrets" in str(k)]
        for k, v in node.items():
            found += _secret_mentions(v, skip, f"{path}.{k}")
        return found
    if isinstance(node, list):
        return [m for i, v in enumerate(node) for m in _secret_mentions(v, skip, f"{path}[{i}]")]
    return [path] if "secrets" in str(node) else []


def admin_read_wiring_errors(workflow: object, manifest: object) -> list[str]:
    """Return why a green `ADMIN_READ` run could fail to be the manifest's
    ruleset admin read, or []: the workflow triggers on `ADMIN_READ.event`
    alone and sets no workflow-level `env`/`defaults`; its one job
    `ADMIN_READ_CONTEXT` runs unconditionally in the admin-read environment
    with no job-level `env`/`defaults`, and runs exactly `admin_read_steps()`,
    the last one only `ADMIN_READ_INVOCATION` with env exactly
    `ADMIN_READ_ENV` (the admin token, not the workflow token); no other part
    of the workflow names `secrets`; the manifest declares the context a
    `nightly-gate` the workflow produces."""
    if HERE not in sys.path:
        sys.path.insert(0, HERE)
    import check_required_set  # noqa: PLC0415  # PyYAML-backed; owns the environment's name

    errors: list[str] = []
    on = workflow.get(True, workflow.get("on")) if isinstance(workflow, dict) else None
    if not isinstance(on, dict) or set(on) != {ADMIN_READ.event}:
        errors.append(f"{ADMIN_READ.workflow} must trigger on `{ADMIN_READ.event}` alone")
    for key in _ADMIN_READ_SCOPE_KEYS:
        if isinstance(workflow, dict) and key in workflow:
            errors.append(f"{ADMIN_READ.workflow} must not set a workflow-level `{key}`")
    jobs = workflow.get("jobs") if isinstance(workflow, dict) else None
    job = jobs.get(ADMIN_READ_CONTEXT) if isinstance(jobs, dict) and len(jobs) == 1 else None
    if not isinstance(job, dict):
        errors.append(f"{ADMIN_READ.workflow} must define exactly one job, {ADMIN_READ_CONTEXT!r}")
        job = {}
    elif job.get("name", ADMIN_READ_CONTEXT) != ADMIN_READ_CONTEXT:
        errors.append(f"the {ADMIN_READ.workflow} job must report context {ADMIN_READ_CONTEXT!r}")
    for key in ("if", "continue-on-error", "needs", "strategy"):
        if key in job:
            errors.append(f"the {ADMIN_READ.workflow} job must not set `{key}` (the read runs unconditionally, once)")
    for key in _ADMIN_READ_SCOPE_KEYS:
        if key in job:
            errors.append(f"the {ADMIN_READ.workflow} job must not set a job-level `{key}`")
    if job and job.get("environment") != check_required_set.ADMIN_READ_ENVIRONMENT:
        errors.append(
            f"the {ADMIN_READ.workflow} job must declare `environment: {check_required_set.ADMIN_READ_ENVIRONMENT}`"
        )
    steps = job.get("steps")
    expected = admin_read_steps()
    read_step: object = None
    if not isinstance(steps, list) or len(steps) != len(expected):
        errors.append(
            f"the {ADMIN_READ.workflow} job must run exactly {len(expected)} steps: "
            + ", ".join(what for what, _ in expected)
        )
    else:
        for i, (step, (what, pinned)) in enumerate(zip(steps, expected)):
            errors += _step_errors(i, step, what, pinned)
        read_step = steps[-1]
    for where in _secret_mentions(workflow, read_step, ADMIN_READ.workflow):
        errors.append(f"{where} names `secrets`; only the read step may")
    entries = manifest.get("checks") if isinstance(manifest, dict) else None
    mine = [e for e in entries or [] if isinstance(e, dict) and e.get("context") == ADMIN_READ_CONTEXT]
    if len(mine) != 1 or mine[0].get("disposition") != "nightly-gate" or mine[0].get("producer") != ADMIN_READ.workflow:
        errors.append(
            f"check-manifest.yml must declare {ADMIN_READ_CONTEXT!r} once as a `nightly-gate` produced by {ADMIN_READ.workflow}"
        )
    return errors


def lint(root: str = REPO_ROOT) -> int:
    sys.path.insert(0, HERE)
    import strict_yaml  # noqa: PLC0415  # PyYAML-backed; only `lint` needs it

    try:
        with open(os.path.join(root, ".github", "workflows", WORKFLOW_FILE), encoding="utf-8") as fh:
            workflow = strict_yaml.safe_load(fh)
        with open(os.path.join(root, ".github", "ci", "check-manifest.yml"), encoding="utf-8") as fh:
            manifest = strict_yaml.safe_load(fh)
        with open(os.path.join(root, ADMIN_READ.path), encoding="utf-8") as fh:
            admin_workflow = strict_yaml.safe_load(fh)
    except Exception as exc:  # noqa: BLE001  # any read or parse failure is a refusal
        print(f"nightly-green lint: unreadable input: {exc}", file=sys.stderr)
        return 1
    errors = wiring_errors(workflow, manifest) + admin_read_wiring_errors(admin_workflow, manifest)
    for err in errors:
        print(f"nightly-green lint: {err}", file=sys.stderr)
    if errors:
        return 1
    print(
        f"nightly-green lint: {WORKFLOW_FILE} runs the verdict unconditionally; the manifest gates it; "
        f"{ADMIN_READ.workflow} is {ADMIN_READ.event}-only and reads with the admin token in its environment."
    )
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
                "`gh workflow run 'Build & test' --ref <branch>`, then re-run this check. "
                + ADMIN_READ_RECOVERY,
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
