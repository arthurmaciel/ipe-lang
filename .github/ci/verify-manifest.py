#!/usr/bin/env python3
"""Drift gate for the CI check-disposition SSOT (ci/check-manifest.yml).

Fails (exit 1) when the manifest and reality disagree, so no check can exist
without a declared disposition and no required check can silently stop gating.

Checks performed
  1. Every status context produced by .github/workflows/*.yml (matrix legs
     expanded) is present in the manifest, and is produced by exactly one
     workflow (a context two workflows post is ambiguous as a required check).
  2. Manifest self-consistency:
       - a known disposition (gate | gate-external | nightly-gate |
         informational | delete);
       - `informational` entries name an `owner`;
       - an entry that `guards` a Security/Soundness/SEAL invariant is NOT
         `informational` (a guarantee may not sit in an un-gated bucket);
       - a `gate`/`nightly-gate` entry has a producer workflow that exists and
         actually produces the context;
       - a `delete` entry has no producer (an orphan), else it is a live check.
  3. Every `gate` reports on every PR: its producer workflow has a
     `pull_request` trigger with no path/branch filter that could leave the
     required context unreported.
  4. A skipped job cannot mask a failure. Branch protection counts `skipped`
     as passing, and a job is skipped when an upstream fails. So for every
     `gate` job:
       - an aggregator (`if: always()` roll-up) lists its whole upstream
         closure in `needs`, hands every one to `gate-aggregate.sh`, and marks
         every unconditional root `must-pass`;
       - any other gate job's upstreams are themselves required contexts.
  5. `ci/required-set.json` equals the derived required set (every `gate` and
     `gate-external` context). `--emit-required-set` prints the derivation.
     `--ruleset FILE` additionally reconciles a JSON dump of the live ruleset.

`--self-test` drives every refusal above against inline fixtures.

Pure stdlib + PyYAML (already a CI dependency).  No network.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover - CI always has PyYAML
    print("verify-manifest: PyYAML is required (pip install pyyaml)", file=sys.stderr)
    sys.exit(2)

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKFLOW_GLOB = os.path.join(REPO_ROOT, "workflows", "*.yml")
MANIFEST = os.path.join(REPO_ROOT, "ci", "check-manifest.yml")
REQUIRED_SET = os.path.join(REPO_ROOT, "ci", "required-set.json")

VALID_DISPOSITIONS = {"gate", "gate-external", "nightly-gate", "informational", "delete"}
# The dispositions branch protection requires — the one definition every
# consumer of "the required set" derives from.
REQUIRED_DISPOSITIONS = frozenset({"gate", "gate-external"})
# Workflows whose jobs are release/automation plumbing, never PR/promotion
# status gates — excluded from the "produced context" set so the drift gate does
# not demand a disposition for a release upload job.
PLUMBING_WORKFLOWS = {
    "release.yml",
    "release-please.yml",
    "rerun-failed-once.yml",
    "nightly-full-gate.yml",
    "ci-health.yml",
}
# `pull_request` trigger filters that can leave a PR with no run at all.
PR_TRIGGER_FILTERS = ("paths", "paths-ignore", "branches", "branches-ignore")
AGGREGATE_ARG = re.compile(
    r"\b(must-pass|may-skip):([A-Za-z0-9_-]+)=\$\{\{\s*needs\.([A-Za-z0-9_-]+)\.result\s*\}\}"
)

Workflows = dict  # workflow filename -> parsed YAML document
Produced = dict  # context -> (workflow filename, job id)


def expand_matrix_names(name: str, strategy: dict) -> list[str]:
    """Expand a job `name:` containing ${{ matrix.KEY }} over its matrix values.

    Only the simple `matrix: {KEY: [a, b, ...]}` form is expanded (that covers
    every matrix in this repo).  A name with no matrix ref returns [name]; an
    unexpandable ref keeps the templated form.
    """
    refs = re.findall(r"\$\{\{\s*matrix\.([a-zA-Z0-9_]+)\s*\}\}", name)
    if not refs:
        return [name]
    matrix = (strategy or {}).get("matrix") or {}
    result = [name]
    for key in refs:
        values = matrix.get(key)
        if not isinstance(values, list):
            return [name]
        expanded = []
        for base in result:
            for v in values:
                expanded.append(base.replace("${{ matrix.%s }}" % key, str(v)))
        seen = set()
        result = [x for x in expanded if not (x in seen or seen.add(x))]
    return result


def jobs_of(doc) -> dict:
    jobs = doc.get("jobs") if isinstance(doc, dict) else None
    return {k: v for k, v in (jobs or {}).items() if isinstance(v, dict)}


def needs_of(job: dict) -> list[str]:
    needs = job.get("needs") or []
    return [needs] if isinstance(needs, str) else list(needs)


def triggers_of(doc) -> dict:
    """The workflow's `on:` block as a dict (PyYAML reads the key `on` as True)."""
    on = doc.get("on", doc.get(True)) if isinstance(doc, dict) else None
    if isinstance(on, str):
        return {on: None}
    if isinstance(on, list):
        return {t: None for t in on}
    return on if isinstance(on, dict) else {}


def load_workflows() -> Workflows:
    workflows: Workflows = {}
    for path in sorted(glob.glob(WORKFLOW_GLOB)):
        fname = os.path.basename(path)
        try:
            with open(path) as f:
                workflows[fname] = yaml.safe_load(f)
        except yaml.YAMLError as e:
            print(f"verify-manifest: {fname} is not valid YAML: {e}", file=sys.stderr)
            sys.exit(2)
    return workflows


def produced_contexts(workflows: Workflows) -> tuple[Produced, list[str]]:
    """Map each produced status context to its (workflow, job id); list clashes."""
    contexts: Produced = {}
    errors: list[str] = []
    for fname, doc in sorted(workflows.items()):
        if fname in PLUMBING_WORKFLOWS:
            continue
        for job_id, job in jobs_of(doc).items():
            name = job.get("name", job_id)
            for ctx in expand_matrix_names(str(name), job.get("strategy") or {}):
                prior = contexts.get(ctx)
                if prior and prior[0] != fname:
                    errors.append(
                        f"context {ctx!r} is produced by both {prior[0]} and {fname} "
                        "— a required context must have exactly one producer"
                    )
                    continue
                contexts.setdefault(ctx, (fname, job_id))
    return contexts, errors


def required_contexts(manifest: dict) -> list[str]:
    """The branch-protection required set, derived from the manifest."""
    return sorted(
        e["context"]
        for e in manifest["checks"]
        if e.get("context") and e.get("disposition") in REQUIRED_DISPOSITIONS
    )


def check_manifest_entries(entries: list) -> tuple[dict, list[str]]:
    by_context: dict[str, dict] = {}
    errors: list[str] = []
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
    return by_context, errors


def check_classified(by_context: dict, produced: Produced) -> list[str]:
    """Every produced context is classified; every live gate is produced."""
    errors: list[str] = []
    # A context named in some entry's `aggregates:` list is an internal matrix
    # leg / prep job whose result rolls up into that single promotable context.
    # `aggregates:` names the job id or the leg-name PREFIX (matrix legs expand
    # to "<name> (i/n)"), so match by exact id or by prefix.
    aggregated = [agg for e in by_context.values() for agg in e.get("aggregates") or []]

    def is_aggregated(ctx: str) -> bool:
        return any(ctx == p or ctx.startswith(p + " (") for p in aggregated)

    for ctx, (wf, _job) in sorted(produced.items()):
        if ctx in by_context or is_aggregated(ctx):
            continue
        errors.append(
            f"produced context {ctx!r} (from {wf}) has NO disposition in "
            "ci/check-manifest.yml — every check must be classified"
        )
    for ctx, e in by_context.items():
        if e.get("internal") or e.get("disposition") not in ("gate", "nightly-gate"):
            continue
        if ctx not in produced:
            errors.append(
                f"{ctx!r}: disposition={e['disposition']} with producer "
                f"{e.get('producer')!r} but NO workflow produces this context "
                "(orphaned required context — wire it or set disposition:delete)"
            )
        elif produced[ctx][0] != e.get("producer"):
            errors.append(
                f"{ctx!r}: manifest producer {e.get('producer')!r} but the context "
                f"is produced by {produced[ctx][0]}"
            )
    return errors


def check_gate_triggers(by_context: dict, workflows: Workflows) -> list[str]:
    """Every gate's producer exists and runs, unfiltered, on every PR."""
    errors: list[str] = []
    for ctx, e in sorted(by_context.items()):
        if e.get("disposition") != "gate":
            continue
        producer = e.get("producer")
        if producer not in workflows:
            errors.append(f"gate {ctx!r}: producer workflow {producer!r} does not exist")
            continue
        triggers = triggers_of(workflows[producer])
        if "pull_request" not in triggers:
            errors.append(
                f"gate {ctx!r}: producer {producer} has no `pull_request` trigger — "
                "a required check that never reports on a PR blocks every merge"
            )
            continue
        pr = triggers["pull_request"] or {}
        filters = [k for k in PR_TRIGGER_FILTERS if isinstance(pr, dict) and k in pr]
        if filters:
            errors.append(
                f"gate {ctx!r}: producer {producer} filters `pull_request` by "
                f"{filters} — a PR outside the filter never reports the required "
                "check; gate by a job-level `if:` on a required path-filter job instead"
            )
    return errors


def upstream_closure(jobs: dict, job_id: str) -> set[str]:
    seen: set[str] = set()
    stack = needs_of(jobs.get(job_id) or {})
    while stack:
        j = stack.pop()
        if j in seen:
            continue
        seen.add(j)
        stack.extend(needs_of(jobs.get(j) or {}))
    return seen


def run_text(job: dict) -> str:
    return "\n".join(str(s.get("run", "")) for s in job.get("steps") or [] if isinstance(s, dict))


def check_skip_closure(by_context: dict, workflows: Workflows, produced: Produced) -> list[str]:
    """A skipped upstream can never turn a required check green."""
    errors: list[str] = []
    required_jobs = {
        produced[c] for c, e in by_context.items() if e.get("disposition") == "gate" and c in produced
    }
    checked: set[tuple[str, str]] = set()
    for ctx, e in sorted(by_context.items()):
        if e.get("disposition") != "gate" or ctx not in produced or produced[ctx] in checked:
            continue
        fname, job_id = produced[ctx]
        checked.add((fname, job_id))
        jobs = jobs_of(workflows[fname])
        job = jobs[job_id]
        closure = upstream_closure(jobs, job_id)
        if e.get("aggregates"):
            direct = set(needs_of(job))
            if "always()" not in str(job.get("if", "")):
                errors.append(
                    f"aggregator {ctx!r}: needs `if: always()` — without it an upstream "
                    "failure skips the aggregator, and a skipped required check passes"
                )
            for member in e["aggregates"]:
                if member not in direct:
                    errors.append(f"aggregator {ctx!r}: does not `need` its member {member!r}")
            for j in sorted(closure - direct):
                errors.append(
                    f"aggregator {ctx!r}: upstream {j!r} is not a direct `need` — its "
                    "result is invisible to the roll-up, so its failure reads as a skip"
                )
            roles = {m[1]: m[0] for m in AGGREGATE_ARG.findall(run_text(job)) if m[1] == m[2]}
            for j in sorted(direct):
                upstream = jobs.get(j) or {}
                if j not in roles:
                    errors.append(
                        f"aggregator {ctx!r}: `{j}` is needed but not passed to "
                        f"gate-aggregate.sh as <role>:{j}=${{{{ needs.{j}.result }}}}"
                    )
                elif roles[j] != "must-pass" and not needs_of(upstream) and "if" not in upstream:
                    errors.append(
                        f"aggregator {ctx!r}: `{j}` always runs, so a skip means it never "
                        "ran — it must be `must-pass`"
                    )
        else:
            for j in sorted(closure):
                if (fname, j) not in required_jobs:
                    errors.append(
                        f"gate {ctx!r}: upstream {j!r} is not a required context — if it "
                        "fails, this gate is skipped and a skipped required check passes"
                    )
    return errors


def check_required_set(expected: list[str], actual: list[str], what: str) -> list[str]:
    errors: list[str] = []
    for c in sorted(set(expected) - set(actual)):
        errors.append(f"{what}: missing required context {c!r}")
    for c in sorted(set(actual) - set(expected)):
        errors.append(f"{what}: extra context {c!r} is not a manifest gate/gate-external")
    return errors


def collect_errors(manifest: dict, workflows: Workflows, required_set: list[str]) -> tuple[list[str], Produced]:
    by_context, errors = check_manifest_entries(manifest["checks"])
    produced, clashes = produced_contexts(workflows)
    errors += clashes
    errors += check_classified(by_context, produced)
    errors += check_gate_triggers(by_context, workflows)
    errors += check_skip_closure(by_context, workflows, produced)
    errors += check_required_set(
        required_contexts(manifest), required_set, "ci/required-set.json is stale (regenerate: --emit-required-set)"
    )
    return errors, produced


def self_test() -> int:
    """Drive every refusal against inline fixtures; exit 1 if one is not refused."""

    def wf(on, jobs):
        return {True: on, "jobs": jobs}

    def agg(needs, args, cond="always()"):
        return {"needs": needs, "if": cond, "steps": [{"run": "gate-aggregate.sh x " + " ".join(args)}]}

    def arg(role, j):
        return f"{role}:{j}=${{{{ needs.{j}.result }}}}"

    nightly = {"nightly.yml": wf({"schedule": []}, {"late": {}})}
    good_roll = [arg("must-pass", "changes"), arg("must-pass", "quick"), arg("may-skip", "shard")]
    good_ci = {
        "changes": {"name": "changes-ci"},
        "quick": {},
        "shard": {"needs": ["changes", "quick"], "if": "x"},
        "roll": agg(["changes", "quick", "shard"], good_roll),
        "solo": {"needs": ["changes"], "if": "x"},
    }
    good_manifest = [
        {"context": "changes-ci", "disposition": "gate", "producer": "ci.yml"},
        {"context": "quick", "disposition": "gate", "producer": "ci.yml"},
        {"context": "roll", "disposition": "gate", "producer": "ci.yml", "aggregates": ["shard"]},
        {"context": "solo", "disposition": "gate", "producer": "ci.yml"},
        {"context": "ext", "disposition": "gate-external"},
        {"context": "late", "disposition": "nightly-gate", "producer": "nightly.yml"},
        {"context": "info", "disposition": "informational", "producer": "ci.yml", "owner": "infra"},
    ]
    good_required = ["changes-ci", "ext", "quick", "roll", "solo"]

    def ci(on=None, **overrides):
        return {"ci.yml": wf({"pull_request": None} if on is None else on, {**good_ci, **overrides}), **nightly}

    def run(manifest=None, workflows=None, required=None):
        m = {"checks": good_manifest if manifest is None else manifest}
        errs, _ = collect_errors(m, ci() if workflows is None else workflows, good_required if required is None else required)
        return errs

    ungated_filter = [e for e in good_manifest if e["context"] != "changes-ci"] + [
        {"context": "changes-ci", "disposition": "informational", "producer": "ci.yml", "owner": "infra"}
    ]
    cases = [
        ("missing producer workflow", run(workflows=nightly), "does not exist"),
        ("gate without pull_request", run(workflows=ci(on={"push": None})), "no `pull_request` trigger"),
        ("path-filtered pull_request", run(workflows=ci(on={"pull_request": {"paths": ["src/**"]}})), "filters `pull_request`"),
        (
            "context from two workflows",
            run(workflows={**ci(), "nightly.yml": wf({"schedule": []}, {"late": {}, "quick": {}})}),
            "produced by both",
        ),
        (
            "manifest names the wrong producer",
            run(manifest=[dict(e, producer="nightly.yml") if e["context"] == "solo" else e for e in good_manifest]),
            "but the context is produced by ci.yml",
        ),
        (
            "aggregator without always()",
            run(workflows=ci(roll=agg(["changes", "quick", "shard"], good_roll, cond="success()"))),
            "needs `if: always()`",
        ),
        (
            "aggregator missing an upstream",
            run(workflows=ci(roll=agg(["quick", "shard"], good_roll[1:]))),
            "upstream 'changes' is not a direct `need`",
        ),
        (
            "aggregator ignores a need's result",
            run(workflows=ci(roll=agg(["changes", "quick", "shard"], good_roll[1:]))),
            "`changes` is needed but not passed",
        ),
        (
            "aggregator lets a root skip",
            run(workflows=ci(roll=agg(["changes", "quick", "shard"], [arg("may-skip", "changes"), *good_roll[1:]]))),
            "it must be `must-pass`",
        ),
        (
            "gate needs an unrequired job",
            run(manifest=ungated_filter, required=["ext", "quick", "roll", "solo"]),
            "upstream 'changes' is not a required context",
        ),
        ("required-set misses gate-external", run(required=["changes-ci", "quick", "roll", "solo"]), "missing required context 'ext'"),
        ("required-set carries a nightly-gate", run(required=[*good_required, "late"]), "extra context 'late'"),
    ]
    failed = 0
    baseline = run()
    if baseline:
        failed += 1
        print("self-test FAIL: the good fixture is refused:", *baseline, sep="\n  ", file=sys.stderr)
    for label, errs, needle in cases:
        if not any(needle in e for e in errs):
            failed += 1
            print(f"self-test FAIL: {label}: no error containing {needle!r}; got {errs}", file=sys.stderr)
    if failed:
        return 1
    print(f"verify-manifest self-test: OK — good fixture accepted, {len(cases)} refusals proven.")
    return 0


def load_manifest() -> dict:
    with open(MANIFEST) as f:
        doc = yaml.safe_load(f)
    if not isinstance(doc, dict) or "checks" not in doc:
        print("verify-manifest: manifest missing top-level `checks:`", file=sys.stderr)
        sys.exit(2)
    return doc


def main() -> int:
    ap = argparse.ArgumentParser()
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--self-test", action="store_true", help="prove every refusal against inline fixtures")
    mode.add_argument(
        "--emit-required-set",
        action="store_true",
        help="print the required set derived from the manifest (the content of ci/required-set.json)",
    )
    ap.add_argument(
        "--ruleset",
        help="JSON file: a list of required status-check context strings from the "
        "live ruleset. When given, mismatches with the derived set are fatal.",
    )
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    manifest = load_manifest()
    if args.emit_required_set:
        print(json.dumps(required_contexts(manifest), indent=2))
        return 0

    with open(REQUIRED_SET) as f:
        required_set = json.load(f)
    errors, produced = collect_errors(manifest, load_workflows(), required_set)
    if args.ruleset:
        with open(args.ruleset) as f:
            errors += check_required_set(required_contexts(manifest), json.load(f), "live ruleset")
    else:
        print("verify-manifest: no --ruleset given; skipping live-ruleset reconciliation (see ci/RECONCILIATION.md).")

    if errors:
        print("\nverify-manifest: FAIL\n", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        print(file=sys.stderr)
        return 1

    counts = {d: sum(1 for e in manifest["checks"] if e["disposition"] == d) for d in sorted(VALID_DISPOSITIONS)}
    print(
        f"verify-manifest: OK — {len(manifest['checks'])} checks classified ("
        + ", ".join(f"{n} {d}" for d, n in counts.items())
        + f"); {len(produced)} produced contexts, all covered; "
        f"{len(required_contexts(manifest))} required contexts in sync."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
