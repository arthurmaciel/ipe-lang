#!/usr/bin/env python3
"""Drift gate for the CI check-disposition SSOT (ci/check-manifest.yml).

Fails (exit 1) when the manifest and reality disagree, so no check can exist
without a declared disposition and no `gate` can silently lose its producer.

Checks performed
  1. Every status context produced by .github/workflows/*.yml or *.yaml
     (matrix legs expanded) is present in the manifest.  An unclassified
     check is the exact "silent advisory red" this SSOT exists to forbid.
  2. Manifest self-consistency:
       - a known disposition (gate | nightly-gate | informational | delete);
       - `informational` entries name an `owner`;
       - an entry that `guards` a Security/Soundness/SEAL invariant is NOT
         `informational` (§0/§1/§3: a guarantee may not sit in an un-gated bucket);
       - a `gate`/`nightly-gate` entry has a producer workflow that exists;
       - a `delete` entry has no producer (an orphan), else it is a live check.
  3. Fail-closed dependency surfacing: GitHub reports a job whose `needs`
     failed as SKIPPED, and a skipped required check counts as passing.  So
     every job a gate transitively `needs` must itself surface in a gate (its
     own context is a gate, or a gate `aggregates` it); likewise a
     nightly-gate's ancestors must surface in a gate or nightly-gate.  A
     status context produced by two jobs is refused outright — a required
     context must resolve to exactly one producer.
  4. Required-set reconciliation (best-effort, non-fatal by default): every
     manifest `gate` context should be in the branch-protection required set and
     vice-versa.  Run with `--ruleset FILE` (a JSON dump of the ruleset's
     required contexts) to make mismatches fatal; without it the manifest is the
     SSOT and the check is skipped with a note.
  5. `ci/deterministic-checks.json` — the SSOT of (job, check step) pairs
     consumed by ci.yml's `cancel-on-cheap-red` watcher and
     rerun-failed-once.yml — is well-formed (exact keys, non-empty strings with
     no surrounding whitespace, no duplicate job), its job set equals the
     watcher's `needs:`, and each pair's step is a `name:` of that job's steps
     in ci.yml.
  6. sccache wiring: the ONLY sanctioned way a job gets sccache is
     `uses: ./.github/actions/sccache`, a composite action that installs
     `mozilla-actions/sccache-action` and writes RUSTC_WRAPPER/
     SCCACHE_GHA_ENABLED to `$GITHUB_ENV` itself, so the wrapper can never
     exist in a job without the binary that backs it. This refuses: a raw
     `mozilla-actions/sccache-action` reference anywhere else (case-folded);
     a workflow-, job-, or step-level `env:` key RUSTC_WRAPPER/
     SCCACHE_GHA_ENABLED (case-folded) outside the composite; a `run:` step
     writing RUSTC_WRAPPER/SCCACHE_* into `$GITHUB_ENV` outside the
     composite; and a job that uses the composite while also owning a step
     named in `ci/deterministic-checks.json` (sccache's GitHub Actions cache
     backend does network I/O a deterministic check must never risk). It
     also validates the composite action itself installs the action and
     writes both vars. Checked across every workflow, not just the
     non-plumbing ones `workflow_jobs()` covers for status contexts.

Pure stdlib + PyYAML (already a CI dependency).  No network.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import posixpath
import re
import sys
from dataclasses import dataclass

try:
    import yaml
except ImportError:  # pragma: no cover - CI always has PyYAML
    print("verify-manifest: PyYAML is required (pip install pyyaml)", file=sys.stderr)
    sys.exit(2)

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Both extensions: a workflow (or, for check 6, a local composite action) is a
# workflow whichever suffix its YAML uses — a `*.yml`-only glob silently drops
# a `*.yaml` file from every check below it feeds.
WORKFLOW_GLOBS = (
    os.path.join(REPO_ROOT, "workflows", "*.yml"),
    os.path.join(REPO_ROOT, "workflows", "*.yaml"),
)
MANIFEST = os.path.join(REPO_ROOT, "ci", "check-manifest.yml")
DETERMINISTIC_CHECKS_FILE = os.path.join(REPO_ROOT, "ci", "deterministic-checks.json")
CANCEL_WATCHER_WORKFLOW = "ci.yml"
CANCEL_WATCHER_JOB_ID = "cancel-on-cheap-red"

SCCACHE_ACTION_PREFIX = "mozilla-actions/sccache-action@"
SCCACHE_COMPOSITE_USES = "./.github/actions/sccache"
# Canonical repo-root-relative form of SCCACHE_COMPOSITE_USES, and the form
# every local `uses:` reference is normalized to before comparison (rule (d)
# used to compare the raw string, so "./.github/actions/sccache/" and
# "./.github/actions/./sccache" silently bypassed it).
SCCACHE_COMPOSITE_NORMALIZED = posixpath.normpath(SCCACHE_COMPOSITE_USES.rstrip("/"))
SCCACHE_COMPOSITE_REL_PATH = os.path.join("actions", "sccache", "action.yml")
SCCACHE_WRAPPER_VAR = "RUSTC_WRAPPER"
SCCACHE_GHA_VAR = "SCCACHE_GHA_ENABLED"
# SSOT: every env-var name that hands rustc a wrapper. Reused for BOTH the
# exact env-key match (b) and the run:-text scan (c) — a `printf`/`export`
# write that never matches a literal `KEY=` assignment is still a wiring
# write and must be refused the same as one that does.
SCCACHE_WRAPPER_KEY_NAMES = (
    SCCACHE_WRAPPER_VAR,
    "CARGO_BUILD_RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
)
SCCACHE_ENV_KEYS = {k.casefold() for k in SCCACHE_WRAPPER_KEY_NAMES} | {SCCACHE_GHA_VAR.casefold()}
# Any wrapper-shaped key above, OR a generic SCCACHE_* key (covers
# SCCACHE_GHA_ENABLED and any other sccache-shaped knob) — matched anywhere a
# `run:` script also mentions $GITHUB_ENV, with no requirement on assignment
# syntax. Over-strict by design (PRINCIPLES: fail closed on untrusted shape) —
# a refused false positive is cheap, a missed wiring write is not.
SCCACHE_KEY_RE = re.compile(
    "(?:" + "|".join(re.escape(k) for k in SCCACHE_WRAPPER_KEY_NAMES) + r"|SCCACHE_[A-Z0-9_]*)",
    re.IGNORECASE,
)


def _mentions_github_env_write(run: str) -> bool:
    """True when a `run:` script mentions $GITHUB_ENV alongside any
    sccache-wiring-shaped key, regardless of exact assignment syntax
    (`KEY=val >> $GITHUB_ENV`, `printf '%s=%s' KEY val >> "$GITHUB_ENV"`,
    `echo "KEY=$val" | tee -a "$GITHUB_ENV"`, ...). Only a co-occurrence
    check — deliberately not tied to `=`, which a shaped write need not use.
    """
    return "GITHUB_ENV" in run and bool(SCCACHE_KEY_RE.search(run))

VALID_DISPOSITIONS = {"gate", "gate-external", "nightly-gate", "informational", "delete"}
# Workflows whose jobs are release/automation plumbing, never PR/promotion
# status gates — excluded from the "produced context" set so the drift gate does
# not demand a disposition for a release upload job.
PLUMBING_WORKFLOWS = {
    "release.yml",
    "release-please.yml",
    "rerun-failed-once.yml",
    "nightly-full-gate.yml",
    "manifest-guard.yml",
    "ci-health.yml",
}


def expand_matrix_names(name: str, strategy: dict) -> list[str]:
    """Expand a job `name:` containing ${{ matrix.KEY }} over its matrix values.

    Only the simple `matrix: {KEY: [a, b, ...]}` form is expanded (that covers
    every matrix in this repo).  A name with no matrix ref returns [name]; an
    unexpandable ref falls back to a regex-friendly wildcard match later.
    """
    refs = re.findall(r"\$\{\{\s*matrix\.([a-zA-Z0-9_]+)\s*\}\}", name)
    if not refs:
        return [name]
    matrix = (strategy or {}).get("matrix") or {}
    result = [name]
    for key in refs:
        values = matrix.get(key)
        if not isinstance(values, list):
            return [name]  # cannot expand; keep the templated form
        expanded = []
        for base in result:
            for v in values:
                expanded.append(base.replace("${{ matrix.%s }}" % key, str(v)))
                expanded.append(
                    base.replace("${{ matrix.%s }}" % key.strip(), str(v))
                )
        # de-dup while preserving order
        seen = set()
        result = [x for x in expanded if not (x in seen or seen.add(x))]
    return result


class Job:
    """One workflow job: its status contexts and direct `needs`."""

    def __init__(self, workflow: str, job_id: str, contexts: list[str], needs: list[str]):
        self.workflow = workflow
        self.job_id = job_id
        self.contexts = contexts
        self.needs = needs


def workflow_jobs() -> list[Job]:
    """Every job of every non-plumbing workflow, matrix legs expanded."""
    jobs: list[Job] = []
    paths = sorted(p for g in WORKFLOW_GLOBS for p in glob.glob(g))
    for path in paths:
        fname = os.path.basename(path)
        if fname in PLUMBING_WORKFLOWS:
            continue
        try:
            doc = yaml.safe_load(open(path))
        except yaml.YAMLError as e:
            print(f"verify-manifest: {fname} is not valid YAML: {e}", file=sys.stderr)
            sys.exit(2)
        if not isinstance(doc, dict):
            continue
        for job_id, job in (doc.get("jobs") or {}).items():
            if not isinstance(job, dict):
                continue
            name = job.get("name", job_id)
            needs = job.get("needs") or []
            if isinstance(needs, str):
                needs = [needs]
            contexts = expand_matrix_names(str(name), job.get("strategy") or {})
            jobs.append(Job(fname, str(job_id), contexts, [str(n) for n in needs]))
    return jobs


def produced_contexts(jobs: list[Job]) -> dict[str, list[str]]:
    """Map produced status-context string -> producing workflow filenames."""
    contexts: dict[str, list[str]] = {}
    for job in jobs:
        for ctx in job.contexts:
            contexts.setdefault(ctx, []).append(job.workflow)
    return contexts


def load_deterministic_checks(errors: list[str]) -> list[tuple[str, str]] | None:
    """Parse `ci/deterministic-checks.json` into (context, step) pairs, or
    record why it is malformed.  Strict: the shell consumers match these
    strings byte-exactly, so anything a consumer could misread is rejected.
    """
    where = DETERMINISTIC_CHECKS_FILE
    try:
        with open(where) as f:
            doc = json.load(f)
    except (OSError, ValueError) as e:
        errors.append(f"cannot read {where}: {e}")
        return None
    if not isinstance(doc, dict) or set(doc) != {"about", "checks"}:
        errors.append(f"{where}: top level must be an object with exactly the keys 'about' and 'checks'")
        return None
    checks = doc["checks"]
    if not isinstance(checks, list) or not checks:
        errors.append(f"{where}: 'checks' must be a non-empty list")
        return None
    pairs: list[tuple[str, str]] = []
    seen: set[str] = set()
    for i, entry in enumerate(checks):
        if not isinstance(entry, dict) or set(entry) != {"context", "step"}:
            errors.append(f"{where}: checks[{i}] must be an object with exactly the keys 'context' and 'step'")
            continue
        ctx, step = entry["context"], entry["step"]
        bad = False
        for key, val in (("context", ctx), ("step", step)):
            if not isinstance(val, str) or not val or val != val.strip() or "\n" in val:
                errors.append(
                    f"{where}: checks[{i}].{key} = {val!r} must be a non-empty "
                    "single-line string with no leading/trailing whitespace"
                )
                bad = True
        if bad:
            continue
        if ctx in seen:
            errors.append(f"{where}: context {ctx!r} is listed more than once")
            continue
        seen.add(ctx)
        pairs.append((ctx, step))
    return pairs


def check_deterministic_set(jobs: list[Job], errors: list[str]) -> None:
    """`ci/deterministic-checks.json` is the one SSOT behind ci.yml's
    `cancel-on-cheap-red` watcher and rerun-failed-once.yml's retry skip.
    Its job set must equal the watcher's `needs:`, and each pair's step must
    be a literal `name:` of that job's steps — a renamed or unnamed check step
    would otherwise never match and silently disable both consumers.
    """
    pairs = load_deterministic_checks(errors)
    if pairs is None:
        return

    by_job_id = {
        j.job_id: j for j in jobs if j.workflow == CANCEL_WATCHER_WORKFLOW
    }
    watcher = by_job_id.get(CANCEL_WATCHER_JOB_ID)
    if watcher is None:
        errors.append(
            f"{CANCEL_WATCHER_WORKFLOW} has no {CANCEL_WATCHER_JOB_ID!r} job — "
            f"{DETERMINISTIC_CHECKS_FILE} has no watcher to check against"
        )
        return

    raw_jobs = yaml.safe_load(
        open(os.path.join(REPO_ROOT, "workflows", CANCEL_WATCHER_WORKFLOW))
    ).get("jobs") or {}

    # context -> job id, over single-context (non-matrix) jobs of ci.yml only:
    # a matrix leg's context cannot be tied to one check step unambiguously.
    ctx_to_job: dict[str, str] = {}
    for j in by_job_id.values():
        if len(j.contexts) == 1:
            ctx_to_job[j.contexts[0]] = j.job_id

    listed_ids: set[str] = set()
    for ctx, step in pairs:
        job_id = ctx_to_job.get(ctx)
        if job_id is None:
            errors.append(
                f"{DETERMINISTIC_CHECKS_FILE}: context {ctx!r} is not the "
                f"context of a single-context job in {CANCEL_WATCHER_WORKFLOW}"
            )
            continue
        listed_ids.add(job_id)
        raw_job = raw_jobs.get(job_id) or {}
        if "strategy" in raw_job:
            errors.append(
                f"{DETERMINISTIC_CHECKS_FILE}: job {job_id!r} has a `strategy:`; "
                "a deterministic check must be a single unexpanded job"
            )
        if "${{" in ctx or "${{" in step:
            errors.append(
                f"{DETERMINISTIC_CHECKS_FILE}: {ctx!r}/{step!r} contains an "
                "expression; consumers match literal names only"
            )
        steps = raw_job.get("steps") or []
        named = [st for st in steps if isinstance(st, dict) and st.get("name") == step]
        if len(named) != 1:
            errors.append(
                f"{DETERMINISTIC_CHECKS_FILE}: job {job_id!r} must have exactly "
                f"one step named {step!r} (found {len(named)})"
            )
            continue
        if "if" in named[0] or "continue-on-error" in named[0]:
            errors.append(
                f"{DETERMINISTIC_CHECKS_FILE}: step {step!r} of job {job_id!r} "
                "must run unconditionally: no `if:` and no `continue-on-error:`"
            )

    needed = set(watcher.needs)
    unknown = needed - set(by_job_id)
    if unknown:
        errors.append(
            f"{CANCEL_WATCHER_WORKFLOW}: {CANCEL_WATCHER_JOB_ID!r} needs "
            f"unknown job(s) {sorted(unknown)}"
        )
    missing = listed_ids - needed
    extra = needed - listed_ids - unknown
    if missing:
        errors.append(
            f"{DETERMINISTIC_CHECKS_FILE} lists job(s) {sorted(missing)} but "
            f"{CANCEL_WATCHER_JOB_ID!r} does not `needs:` them"
        )
    if extra:
        errors.append(
            f"{CANCEL_WATCHER_JOB_ID!r} needs {sorted(extra)} but they are "
            f"missing from {DETERMINISTIC_CHECKS_FILE}"
        )


@dataclass(frozen=True)
class Step:
    """One workflow (or composite action) step, typed just enough for the
    sccache-wiring checks: `uses:`/`name:`/`env:`/`run:` are read nowhere
    else in this module via raw `.get()`.
    """

    raw: dict

    @property
    def name(self) -> str | None:
        n = self.raw.get("name")
        return n if isinstance(n, str) else None

    @property
    def uses(self) -> str | None:
        u = self.raw.get("uses")
        return u if isinstance(u, str) else None

    @property
    def uses_folded(self) -> str:
        return (self.uses or "").casefold()

    @property
    def run(self) -> str | None:
        r = self.raw.get("run")
        return r if isinstance(r, str) else None


@dataclass(frozen=True)
class WorkflowJob:
    job_id: str
    raw: dict

    @property
    def steps(self) -> list[Step]:
        return [Step(st) for st in (self.raw.get("steps") or []) if isinstance(st, dict)]


@dataclass(frozen=True)
class SccacheWorkflow:
    fname: str
    doc: dict

    @property
    def jobs(self) -> list[WorkflowJob]:
        return [
            WorkflowJob(str(jid), j)
            for jid, j in (self.doc.get("jobs") or {}).items()
            if isinstance(j, dict)
        ]


def _load_sccache_workflows(root: str, errors: list[str]) -> list[SccacheWorkflow]:
    out: list[SccacheWorkflow] = []
    paths = sorted(
        p
        for pattern in ("*.yml", "*.yaml")
        for p in glob.glob(os.path.join(root, "workflows", pattern))
    )
    for path in paths:
        fname = os.path.basename(path)
        try:
            with open(path) as f:
                doc = yaml.safe_load(f)
        except yaml.YAMLError as e:
            errors.append(f"{fname} is not valid YAML: {e}")
            continue
        if isinstance(doc, dict):
            out.append(SccacheWorkflow(fname, doc))
    return out


def _env_keys_folded(env: dict) -> set[str]:
    return {str(k).casefold() for k in env}


def _scoped_env(container: dict, loc: str, errors: list[str]) -> dict:
    """Extract an `env:` mapping at one scope (workflow/job/step/container/
    service), failing CLOSED when `env:` is present but not a plain mapping
    (e.g. `env: ${{ fromJSON(vars.E) }}`) — such a value's keys cannot be
    determined statically, so "it does not set an sccache-wiring key" cannot
    be proven and must never be assumed (PRINCIPLES §1: fail closed absent
    proof of safety). A missing `env:` is simply empty, not an error.
    """
    if not isinstance(container, dict) or "env" not in container:
        return {}
    e = container["env"]
    if isinstance(e, dict):
        return e
    errors.append(
        f"{loc}: env: is not a plain mapping (got {type(e).__name__}: {e!r}) — "
        "cannot verify it does not set an sccache-wiring key; refused fail-closed"
    )
    return {}


def _normalize_local_uses(uses: str | None) -> str | None:
    """Repo-root-relative normalized form of a local `uses: ./...` action
    reference, or None when `uses` is not a local reference (a pinned
    third-party action, `docker://...`, etc). GitHub resolves a local `uses:`
    relative to the repository root regardless of which workflow or composite
    action contains it, so "./.github/actions/sccache",
    "./.github/actions/sccache/", and "./.github/actions/./sccache" must all
    normalize to the one identical path before being compared.
    """
    if uses is None or not uses.startswith("./"):
        return None
    return posixpath.normpath(uses.rstrip("/"))


def _discover_local_composites(root: str, errors: list[str]) -> dict[str, list[Step]]:
    """Every local composite action under `.github/actions/*/action.y*ml`,
    keyed by the normalized `uses:` path another workflow or composite would
    reference it by, mapped to its own steps. A local action that is not a
    composite (`using: node20`/docker) is opaque to this static YAML pass —
    excluded, since it cannot itself declare a nested `uses:` step to a
    composite this repo controls.
    """
    composites: dict[str, list[Step]] = {}
    for path in sorted(glob.glob(os.path.join(root, "actions", "*", "action.y*ml"))):
        rel_dir = os.path.relpath(os.path.dirname(path), root).replace(os.sep, "/")
        uses_id = posixpath.normpath(".github/" + rel_dir)
        try:
            with open(path) as f:
                doc = yaml.safe_load(f)
        except yaml.YAMLError as e:
            errors.append(f"{uses_id}/action.yml is not valid YAML: {e}")
            continue
        if not isinstance(doc, dict):
            continue
        runs = doc.get("runs")
        if not isinstance(runs, dict) or runs.get("using") != "composite":
            continue
        steps = [st for st in (runs.get("steps") or []) if isinstance(st, dict)]
        composites[uses_id] = [Step(st) for st in steps]
    return composites


def _composite_uses_graph(composites: dict[str, list[Step]]) -> dict[str, set[str]]:
    """`uses_id -> {other local uses_ids its steps reference}`, so nesting
    (a wrapper composite that itself `uses:` the sccache composite) is
    traversable rather than only visible one hop deep.
    """
    graph: dict[str, set[str]] = {cid: set() for cid in composites}
    for cid, steps in composites.items():
        for st in steps:
            norm = _normalize_local_uses(st.uses)
            if norm is not None:
                graph[cid].add(norm)
    return graph


def _reaches(start: str, graph: dict[str, set[str]], target: str, depth_limit: int = 20) -> bool:
    """BFS with a visited set (cycle-safe) and a depth bound (a runaway or
    self-referential composite chain terminates rather than looping forever)."""
    if start == target:
        return True
    visited = {start}
    frontier = [start]
    depth = 0
    while frontier and depth <= depth_limit:
        nxt_frontier: list[str] = []
        for node in frontier:
            for nxt in graph.get(node, ()):
                if nxt == target:
                    return True
                if nxt not in visited:
                    visited.add(nxt)
                    nxt_frontier.append(nxt)
        frontier = nxt_frontier
        depth += 1
    return False


def _audit_other_composites(composites: dict[str, list[Step]], errors: list[str]) -> None:
    """Apply rules (a)/(b)/(c) to every local composite action EXCEPT the
    sanctioned sccache one (which `_check_sccache_composite` audits under its
    own, different, expectations): a wrapper composite that runs the raw
    action, hand-sets the wiring env, or writes $GITHUB_ENV itself escapes
    check 6 just as effectively as a workflow step would.
    """
    for cid, steps in composites.items():
        if cid == SCCACHE_COMPOSITE_NORMALIZED:
            continue
        for st in steps:
            label = st.name or st.uses or "<unnamed>"
            loc = f"{cid}/action.yml"
            if st.uses is not None and st.uses_folded.startswith(SCCACHE_ACTION_PREFIX.casefold()):
                errors.append(
                    f"{loc}: step {label!r} runs the raw {SCCACHE_ACTION_PREFIX}... "
                    f"action directly — use {SCCACHE_COMPOSITE_USES} instead, the one "
                    "place it may run"
                )
            step_env = _scoped_env(st.raw, f"{loc}: step {label!r}", errors)
            for key in _env_keys_folded(step_env) & SCCACHE_ENV_KEYS:
                errors.append(
                    f"{loc}: step {label!r} env sets {key!r} — sccache wiring must "
                    f"come only from {SCCACHE_COMPOSITE_USES}, never a hand-set env:"
                )
            if st.run and _mentions_github_env_write(st.run):
                errors.append(
                    f"{loc}: step {label!r} writes {SCCACHE_WRAPPER_VAR}/"
                    f"{SCCACHE_GHA_VAR}-shaped output into $GITHUB_ENV outside "
                    f"{SCCACHE_COMPOSITE_USES} — the composite is the one "
                    "sanctioned setter"
                )


def _deterministic_step_names(root: str) -> set[str]:
    """Step names ci/deterministic-checks.json lists, read fresh — the SSOT,
    never hardcoded here.  A missing/malformed file yields no names; check 5
    (`check_deterministic_set`) is what holds the file itself accountable.
    """
    try:
        with open(os.path.join(root, "ci", "deterministic-checks.json")) as f:
            doc = json.load(f)
    except (OSError, ValueError):
        return set()
    checks = doc.get("checks") if isinstance(doc, dict) else None
    if not isinstance(checks, list):
        return set()
    return {e["step"] for e in checks if isinstance(e, dict) and isinstance(e.get("step"), str)}


def _check_sccache_composite(root: str, errors: list[str]) -> None:
    """The composite action is the one sanctioned place `sccache-action` may
    run and `$GITHUB_ENV` may be written — verify it actually does both, so a
    job trusting `uses: ./.github/actions/sccache` gets a real wrapper.
    """
    path = os.path.join(root, SCCACHE_COMPOSITE_REL_PATH)
    if not os.path.isfile(path):
        errors.append(f"{path} does not exist — no composite action to wire sccache through")
        return
    try:
        with open(path) as f:
            doc = yaml.safe_load(f)
    except yaml.YAMLError as e:
        errors.append(f"{path} is not valid YAML: {e}")
        return
    if not isinstance(doc, dict):
        errors.append(f"{path}: empty or non-mapping document")
        return
    runs = doc.get("runs") or {}
    if runs.get("using") != "composite":
        errors.append(f"{path}: `runs.using` must be 'composite'")
    steps = [Step(st) for st in (runs.get("steps") or []) if isinstance(st, dict)]
    if not any(st.uses_folded.startswith(SCCACHE_ACTION_PREFIX.casefold()) for st in steps):
        errors.append(f"{path}: no step installs {SCCACHE_ACTION_PREFIX}...")
    if not any(st.run and _mentions_github_env_write(st.run) for st in steps):
        errors.append(
            f"{path}: no step writes {SCCACHE_WRAPPER_VAR}/{SCCACHE_GHA_VAR} to "
            "$GITHUB_ENV — installing the binary alone never wires rustc to it"
        )


def _job_sub_env_scopes(job_raw: dict) -> list[tuple[str, dict]]:
    """(scope-name, raw-container) pairs for a job's `container:` and each
    `services.<id>:` sub-scope — each may carry its own `env:` a
    sccache-wiring key could hide in, same as the job's own `env:`."""
    scopes: list[tuple[str, dict]] = []
    container = job_raw.get("container")
    if isinstance(container, dict):
        scopes.append(("container", container))
    services = job_raw.get("services")
    if isinstance(services, dict):
        for sid, svc in services.items():
            if isinstance(svc, dict):
                scopes.append((f"service {sid!r}", svc))
    return scopes


def check_sccache_wiring(errors: list[str], root: str = REPO_ROOT) -> None:
    """The only sanctioned way a job gets sccache is `uses:
    ./.github/actions/sccache` (see `_check_sccache_composite`) — a wrapper
    can then never exist in a job without the binary that backs it, by
    construction. This refuses every other way RUSTC_WRAPPER/
    SCCACHE_GHA_ENABLED could reach a job:
      (a) a raw `mozilla-actions/sccache-action` reference in a workflow OR
          any other local composite action (case-folded `uses:` match — the
          sccache composite is the only legal site);
      (b) a workflow-, job-, step-, container-, or service-level `env:` key
          naming a wrapper var or SCCACHE_GHA_ENABLED (case-folded key), in a
          workflow or any other local composite action — and any `env:` that
          is present but not a plain mapping is refused outright, fail-closed,
          since its keys cannot be proven absent;
      (c) a `run:` step (in a workflow or any other local composite action)
          mentioning $GITHUB_ENV alongside a wrapper var or SCCACHE_* key, any
          assignment syntax;
      (d) a job that reaches the sccache composite — directly, or indirectly
          through a chain of local composite actions — while also owning a
          step named in `ci/deterministic-checks.json`; sccache's GitHub
          Actions cache backend does network I/O, which a deterministic
          (must-be-network-free) check step must never risk. `uses:` local
          references are compared normalized (`./x/`, `./x`, `./a/../x` all
          the same path), not as raw strings.
    Checked over EVERY workflow (`*.yml` and `*.yaml`) and EVERY local
    composite action under `.github/actions/*/action.y*ml`, not just the
    non-plumbing workflows `workflow_jobs()` covers for status contexts — a
    plumbing workflow, or a wrapper composite nobody scans directly, can wire
    sccache without ever producing a gated context.
    """
    _check_sccache_composite(root, errors)
    deterministic_steps = _deterministic_step_names(root)

    composites = _discover_local_composites(root, errors)
    composite_graph = _composite_uses_graph(composites)
    _audit_other_composites(composites, errors)

    def _sccache_reach(uses: str | None) -> tuple[bool, str | None]:
        """(reaches sccache?, the intermediate composite id if indirect)."""
        norm = _normalize_local_uses(uses)
        if norm is None:
            return False, None
        if norm == SCCACHE_COMPOSITE_NORMALIZED:
            return True, None
        if norm in composites and _reaches(norm, composite_graph, SCCACHE_COMPOSITE_NORMALIZED):
            return True, norm
        return False, None

    for wf in _load_sccache_workflows(root, errors):
        wf_env = _scoped_env(wf.doc, f"{wf.fname}: workflow-level env", errors)
        for key in _env_keys_folded(wf_env) & SCCACHE_ENV_KEYS:
            errors.append(
                f"{wf.fname}: workflow-level env sets {key!r} — sccache wiring "
                f"must come only from {SCCACHE_COMPOSITE_USES}, never inherited env"
            )
        for job in wf.jobs:
            job_env = _scoped_env(job.raw, f"{wf.fname}: job {job.job_id!r} env", errors)
            for key in _env_keys_folded(job_env) & SCCACHE_ENV_KEYS:
                errors.append(
                    f"{wf.fname}: job {job.job_id!r} env sets {key!r} — sccache "
                    f"wiring must come only from {SCCACHE_COMPOSITE_USES}, never "
                    "a hand-set env:"
                )
            for scope_name, scope_raw in _job_sub_env_scopes(job.raw):
                scope_env = _scoped_env(
                    scope_raw, f"{wf.fname}: job {job.job_id!r} {scope_name} env", errors
                )
                for key in _env_keys_folded(scope_env) & SCCACHE_ENV_KEYS:
                    errors.append(
                        f"{wf.fname}: job {job.job_id!r} {scope_name} env sets "
                        f"{key!r} — sccache wiring must come only from "
                        f"{SCCACHE_COMPOSITE_USES}, never a hand-set env:"
                    )
            uses_composite = False
            composite_via: str | None = None
            for st in job.steps:
                label = st.name or st.uses or "<unnamed>"
                if st.uses is not None and st.uses_folded.startswith(SCCACHE_ACTION_PREFIX.casefold()):
                    errors.append(
                        f"{wf.fname}: job {job.job_id!r} step {label!r} runs the "
                        f"raw {SCCACHE_ACTION_PREFIX}... action directly — use "
                        f"{SCCACHE_COMPOSITE_USES} instead, the one place it may run"
                    )
                reaches, via = _sccache_reach(st.uses)
                if reaches:
                    uses_composite = True
                    composite_via = composite_via or via
                step_env = _scoped_env(
                    st.raw, f"{wf.fname}: job {job.job_id!r} step {label!r} env", errors
                )
                for key in _env_keys_folded(step_env) & SCCACHE_ENV_KEYS:
                    errors.append(
                        f"{wf.fname}: job {job.job_id!r} step {label!r} env sets "
                        f"{key!r} — sccache wiring must come only from "
                        f"{SCCACHE_COMPOSITE_USES}, never a hand-set env:"
                    )
                if st.run and _mentions_github_env_write(st.run):
                    errors.append(
                        f"{wf.fname}: job {job.job_id!r} step {label!r} writes "
                        f"{SCCACHE_WRAPPER_VAR}/{SCCACHE_GHA_VAR}-shaped output into "
                        f"$GITHUB_ENV outside {SCCACHE_COMPOSITE_USES} — the "
                        "composite is the one sanctioned setter"
                    )
            if uses_composite:
                own_names = {st.name for st in job.steps if st.name is not None}
                hit = sorted(own_names & deterministic_steps)
                if hit:
                    via_note = f" (via {composite_via})" if composite_via else ""
                    errors.append(
                        f"{wf.fname}: job {job.job_id!r} uses {SCCACHE_COMPOSITE_USES}"
                        f"{via_note} but also owns deterministic check step(s) {hit} — "
                        "sccache's GitHub Actions cache backend does network I/O "
                        "inside a step that must be network-free "
                        "(ci/deterministic-checks.json)"
                    )


def load_manifest() -> dict:
    doc = yaml.safe_load(open(MANIFEST))
    if not isinstance(doc, dict) or "checks" not in doc:
        print("verify-manifest: manifest missing top-level `checks:`", file=sys.stderr)
        sys.exit(2)
    return doc


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--ruleset",
        help="JSON file: a list of required status-check context strings. "
        "When given, gate<->required mismatches are fatal.",
    )
    args = ap.parse_args()

    manifest = load_manifest()
    entries = manifest["checks"]
    by_context: dict[str, dict] = {}
    errors: list[str] = []

    # ---- 2. manifest self-consistency ----
    for e in entries:
        ctx = e.get("context")
        disp = e.get("disposition")
        if not ctx:
            errors.append(f"manifest entry without a context: {e!r}")
            continue
        if ctx in by_context:
            errors.append(f"duplicate manifest context: {ctx!r}")
        by_context[ctx] = e
        if disp not in VALID_DISPOSITIONS:
            errors.append(f"{ctx!r}: invalid disposition {disp!r} (want one of {sorted(VALID_DISPOSITIONS)})")
            continue
        if disp == "informational" and not e.get("owner"):
            errors.append(f"{ctx!r}: informational check must name an `owner`")
        if disp == "informational" and e.get("guards"):
            errors.append(
                f"{ctx!r}: guards {e['guards']!r} but is informational — a "
                "Security/Soundness/SEAL guarantee may not be un-gated"
            )
        if disp in ("gate", "nightly-gate") and not e.get("producer"):
            errors.append(f"{ctx!r}: {disp} entry has no producer workflow")
        if disp == "gate-external" and e.get("producer"):
            errors.append(
                f"{ctx!r}: disposition=gate-external must have no producer — the "
                "status is posted out-of-band, not by a CI workflow"
            )
        if disp == "delete" and e.get("producer"):
            errors.append(
                f"{ctx!r}: disposition=delete but a producer is set — a live "
                "check may not be marked delete"
            )

    # ---- 1. every produced context is classified ----
    jobs = workflow_jobs()
    produced = produced_contexts(jobs)
    manifest_ctxs = set(by_context)

    # A context named in some entry's `aggregates:` list is an internal matrix
    # leg / prep job whose result rolls up into that single promotable context;
    # it inherits the aggregator's disposition and is considered classified.
    # `aggregates:` names the job id or the leg-name PREFIX (matrix legs expand
    # to "<name> (i/n)"), so match by exact id or by prefix.
    aggregated_prefixes: list[str] = []
    aggregated_exact: set[str] = set()
    for e in by_context.values():
        for agg in e.get("aggregates") or []:
            aggregated_exact.add(agg)
            aggregated_prefixes.append(agg)

    def is_aggregated(ctx: str) -> bool:
        if ctx in aggregated_exact:
            return True
        # matrix leg "asan (1/6)" is aggregated by "asan"
        return any(ctx.startswith(p + " (") or ctx == p for p in aggregated_prefixes)

    for ctx, wfs in sorted(produced.items()):
        if len(wfs) > 1:
            errors.append(
                f"context {ctx!r} is produced by {len(wfs)} jobs ({', '.join(wfs)}) — "
                "a required context must resolve to exactly one producer; give "
                "each job a unique `name:`"
            )
        if ctx in manifest_ctxs or is_aggregated(ctx):
            continue
        errors.append(
            f"produced context {ctx!r} (from {wfs[0]}) has NO disposition in "
            "ci/check-manifest.yml — every check must be classified"
        )

    # ---- 5. ci/deterministic-checks.json vs the watcher + ci.yml steps ----
    check_deterministic_set(jobs, errors)

    # ---- 6. sccache wiring: action <-> RUSTC_WRAPPER/SCCACHE_GHA_ENABLED ----
    check_sccache_wiring(errors)

    # ---- 3. fail-closed dependency surfacing ----
    def surfaced_dispositions(job: Job) -> set[str]:
        """Dispositions of the manifest entries this job's outcome reaches."""
        disps: set[str] = set()
        for ctx in job.contexts:
            entry = by_context.get(ctx)
            if entry:
                disps.add(entry["disposition"])
        for entry in by_context.values():
            for agg in entry.get("aggregates") or []:
                if agg == job.job_id or any(
                    c == agg or c.startswith(agg + " (") for c in job.contexts
                ):
                    disps.add(entry["disposition"])
        return disps

    surfacing = {"gate": {"gate"}, "nightly-gate": {"gate", "nightly-gate"}}
    for job in jobs:
        direct = {by_context[c]["disposition"] for c in job.contexts if c in by_context}
        siblings = {j.job_id: j for j in jobs if j.workflow == job.workflow}
        for disp, allowed in surfacing.items():
            if disp not in direct:
                continue
            seen: set[str] = set()
            pending = list(job.needs)
            while pending:
                dep_id = pending.pop()
                if dep_id in seen:
                    continue
                seen.add(dep_id)
                dep = siblings.get(dep_id)
                if dep is None:
                    errors.append(f"{job.workflow}: job {job.job_id!r} needs unknown job {dep_id!r}")
                    continue
                if surfaced_dispositions(dep).isdisjoint(allowed):
                    errors.append(
                        f"{job.workflow}: {disp} {job.contexts[0]!r} needs {dep_id!r}, "
                        f"which surfaces in no {'/'.join(sorted(allowed))} context — its "
                        "failure would skip the gate, and a skipped required check "
                        "passes (fail-open)"
                    )
                pending.extend(dep.needs)

    # A manifest gate/nightly-gate that claims a live producer but is not
    # actually produced (an orphan the other way).  gate-external is excluded:
    # its whole purpose is to be required without a CI producer.
    for ctx, e in by_context.items():
        if e.get("internal"):
            continue
        if e["disposition"] in ("gate", "nightly-gate") and ctx not in produced:
            errors.append(
                f"{ctx!r}: disposition={e['disposition']} with producer "
                f"{e.get('producer')!r} but NO workflow produces this context "
                "(orphaned required context — wire it or set disposition:delete)"
            )

    # ---- 4. required-set reconciliation ----
    # gate-external contexts are required by the ruleset even though no CI
    # workflow produces them; include them alongside plain gate entries.
    gate_ctxs = {c for c, e in by_context.items() if e["disposition"] in ("gate", "gate-external")}
    if args.ruleset:
        required = set(json.load(open(args.ruleset)))
        missing_from_ruleset = gate_ctxs - required
        extra_in_ruleset = required - gate_ctxs
        for c in sorted(missing_from_ruleset):
            errors.append(f"gate {c!r} is NOT in the required set (add it to the ruleset)")
        for c in sorted(extra_in_ruleset):
            errors.append(
                f"required context {c!r} is not a manifest `gate` "
                "(remove from the ruleset or re-classify)"
            )
    else:
        print(
            "verify-manifest: no --ruleset given; skipping live required-set "
            "reconciliation. The manifest is the SSOT; see ci/RECONCILIATION.md "
            "for the intended required set."
        )

    if errors:
        print("\nverify-manifest: FAIL\n", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        print(file=sys.stderr)
        return 1

    n_gate = sum(1 for e in entries if e["disposition"] == "gate")
    n_gate_ext = sum(1 for e in entries if e["disposition"] == "gate-external")
    n_nightly = sum(1 for e in entries if e["disposition"] == "nightly-gate")
    n_info = sum(1 for e in entries if e["disposition"] == "informational")
    n_del = sum(1 for e in entries if e["disposition"] == "delete")
    print(
        f"verify-manifest: OK — {len(entries)} checks classified "
        f"({n_gate} gate, {n_gate_ext} gate-external, {n_nightly} nightly-gate, "
        f"{n_info} informational, {n_del} delete); "
        f"{len(produced)} produced contexts, all covered."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
