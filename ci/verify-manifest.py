#!/usr/bin/env python3
"""Drift gate for the CI check-disposition SSOT (ci/check-manifest.yml).

Fails (exit 1) when the manifest and reality disagree, so no check can exist
without a declared disposition and no `gate` can silently lose its producer.

Checks performed
  1. Every status context produced by .github/workflows/*.yml (matrix legs
     expanded) is present in the manifest.  An unclassified check is the exact
     "silent advisory red" this SSOT exists to forbid.
  2. Manifest self-consistency:
       - a known disposition (gate | nightly-gate | informational | delete);
       - `informational` entries name an `owner`;
       - an entry that `guards` a Security/Soundness/SEAL invariant is NOT
         `informational` (§0/§1/§3: a guarantee may not sit in an un-gated bucket);
       - a `gate`/`nightly-gate` entry has a producer workflow that exists;
       - a `delete` entry has no producer (an orphan), else it is a live check.
  3. Required-set reconciliation (best-effort, non-fatal by default): every
     manifest `gate` context should be in the branch-protection required set and
     vice-versa.  Run with `--ruleset FILE` (a JSON dump of the ruleset's
     required contexts) to make mismatches fatal; without it the manifest is the
     SSOT and the check is skipped with a note.

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
WORKFLOW_GLOB = os.path.join(REPO_ROOT, ".github", "workflows", "*.yml")
MANIFEST = os.path.join(REPO_ROOT, "ci", "check-manifest.yml")

VALID_DISPOSITIONS = {"gate", "gate-external", "nightly-gate", "informational", "delete"}
# Workflows whose jobs are release/automation plumbing, never PR/promotion
# status gates — excluded from the "produced context" set so the drift gate does
# not demand a disposition for a release upload job.
PLUMBING_WORKFLOWS = {
    "release.yml",
    "release-please.yml",
    "promote.yml",
    "rerun-failed-once.yml",
    "nightly-full-gate.yml",
    "close-issues-on-development.yml",
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


def produced_contexts() -> dict[str, str]:
    """Map produced status-context string -> producing workflow filename."""
    contexts: dict[str, str] = {}
    for path in sorted(glob.glob(WORKFLOW_GLOB)):
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
        jobs = doc.get("jobs") or {}
        for job_id, job in jobs.items():
            if not isinstance(job, dict):
                continue
            name = job.get("name", job_id)
            for ctx in expand_matrix_names(str(name), job.get("strategy") or {}):
                contexts.setdefault(ctx, fname)
    return contexts


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
    produced = produced_contexts()
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

    for ctx, wf in sorted(produced.items()):
        if ctx in manifest_ctxs or is_aggregated(ctx):
            continue
        errors.append(
            f"produced context {ctx!r} (from {wf}) has NO disposition in "
            "ci/check-manifest.yml — every check must be classified"
        )

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

    # ---- 3. required-set reconciliation ----
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
