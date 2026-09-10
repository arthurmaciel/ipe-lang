#!/usr/bin/env python3
"""The advisory-red surface for issue #2138 (driven by .github/workflows/ci-health.yml).

Reads the check-runs for a head commit, maps each to its `ci/check-manifest.yml`
disposition, and produces the single low-noise surface that replaces GitHub's
per-failure email:

  * a run-summary table (job summary) grouping checks by disposition;
  * one dedup tracking issue per RED non-gate context ("ci-health: <context>"):
    opened on first red, edited in place on continued red (no new notification),
    auto-closed when the context is green again.

`gate` reds are already forced into the merge path by branch protection, so they
appear in the table but never get a tracking issue (avoids duplicate surfaces).

Env: GH_TOKEN, REPO (owner/name), HEAD_SHA, SUMMARY (path to job-summary file).
Uses the `gh` CLI (present on GitHub runners). No third-party deps beyond PyYAML.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

import yaml

REPO = os.environ["REPO"]
HEAD_SHA = os.environ["HEAD_SHA"]
SUMMARY = os.environ.get("GITHUB_STEP_SUMMARY", "")
MANIFEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "ci", "check-manifest.yml")

ISSUE_PREFIX = "ci-health: "
DEDUP_MARKER = "<!-- ci-health:dedup -->"  # lets the workflow find its own issues


def gh(*args: str) -> str:
    return subprocess.run(
        ["gh", *args], capture_output=True, text=True, check=True
    ).stdout


def load_manifest() -> dict[str, dict]:
    doc = yaml.safe_load(open(MANIFEST))
    by_ctx: dict[str, dict] = {}
    for e in doc["checks"]:
        by_ctx[e["context"]] = e
    return by_ctx


def disposition_for(ctx: str, manifest: dict[str, dict]) -> tuple[str, dict | None]:
    """Resolve a check-run name to (disposition, entry).

    Matrix legs ("asan (1/6)") inherit the disposition of the aggregator whose
    `aggregates:` list names their prefix. Unknown contexts return ("unknown",
    None) — the summary flags them, which is itself a drift signal.
    """
    if ctx in manifest:
        return manifest[ctx]["disposition"], manifest[ctx]
    for e in manifest.values():
        for agg in e.get("aggregates") or []:
            if ctx == agg or ctx.startswith(agg + " ("):
                return e["disposition"], e
    return "unknown", None


def fetch_check_runs() -> list[dict]:
    """All check-runs for HEAD_SHA, latest attempt per name.

    `gh api --paginate --jq` streams one JSON object per line, so parse
    line-by-line (a single json.loads over the whole body would fail across
    page boundaries).
    """
    raw = gh(
        "api",
        "--paginate",
        f"repos/{REPO}/commits/{HEAD_SHA}/check-runs?per_page=100",
        "--jq",
        ".check_runs[] | {name: .name, conclusion: .conclusion, status: .status}",
    )
    latest: dict[str, dict] = {}
    for line in raw.splitlines():
        line = line.strip()
        if not line:
            continue
        obj = json.loads(line)
        latest[obj["name"]] = obj  # last wins = most recent attempt
    return list(latest.values())


def is_red(run: dict) -> bool:
    return run.get("status") == "completed" and run.get("conclusion") in (
        "failure",
        "timed_out",
        "startup_failure",
    )


def is_green(run: dict) -> bool:
    return run.get("status") == "completed" and run.get("conclusion") in (
        "success",
        "neutral",
        "skipped",
    )


def find_issue(context: str) -> dict | None:
    title = ISSUE_PREFIX + context
    out = gh(
        "issue",
        "list",
        "--repo",
        REPO,
        "--state",
        "open",
        "--search",
        f'in:title "{title}"',
        "--json",
        "number,title",
        "--limit",
        "50",
    )
    res = json.loads(out) if out.strip() else []
    for i in res:
        if i["title"] == title:
            return i
    return None


def surface_red(entry: dict, context: str, owner: str) -> None:
    title = ISSUE_PREFIX + context
    body = (
        f"{DEDUP_MARKER}\n"
        f"**Advisory-red CI check** (disposition: `{entry['disposition']}`"
        + (f", owner: `{owner}`" if owner else "")
        + ").\n\n"
        f"- Context: `{context}`\n"
        f"- Producer: `{entry.get('producer')}`\n"
        + (f"- Guards: `{entry['guards']}`\n" if entry.get("guards") else "")
        + f"- Last seen red at commit: `{HEAD_SHA}`\n\n"
        "This issue is bot-managed: it is edited in place while the check stays "
        "red (no new notification) and auto-closed when it is green again. It is "
        "the single surface for this non-blocking check — do not rely on email."
    )
    existing = find_issue(context)
    if existing:
        gh(
            "issue",
            "edit",
            str(existing["number"]),
            "--repo",
            REPO,
            "--body",
            body,
        )
        print(f"  edited #{existing['number']} (still red): {context}")
    else:
        out = gh(
            "issue",
            "create",
            "--repo",
            REPO,
            "--title",
            title,
            "--body",
            body,
        )
        print(f"  opened tracking issue: {context} -> {out.strip()}")


def surface_green(context: str) -> None:
    existing = find_issue(context)
    if existing:
        gh(
            "issue",
            "close",
            str(existing["number"]),
            "--repo",
            REPO,
            "--comment",
            f"Green again at `{HEAD_SHA}`. Auto-closed by ci-health.",
        )
        print(f"  closed #{existing['number']} (green again): {context}")


def write_summary(rows: list[tuple[str, str, str, str]]) -> None:
    if not SUMMARY:
        return
    buckets = {"gate": [], "nightly-gate": [], "informational": [], "delete": [], "unknown": []}
    for disp, ctx, result, owner in rows:
        buckets.setdefault(disp, []).append((ctx, result, owner))
    lines = [f"## CI health — `{HEAD_SHA[:12]}`", ""]
    order = ["gate", "nightly-gate", "informational", "unknown"]
    for disp in order:
        items = buckets.get(disp) or []
        if not items:
            continue
        reds = sum(1 for _, r, _ in items if r == "RED")
        lines.append(f"### {disp} ({reds} red / {len(items)})")
        lines.append("")
        lines.append("| Check | Result | Owner |")
        lines.append("|---|---|---|")
        for ctx, result, owner in sorted(items):
            mark = "🔴 RED" if result == "RED" else ("🟢 green" if result == "GREEN" else result)
            lines.append(f"| `{ctx}` | {mark} | {owner or ''} |")
        lines.append("")
    with open(SUMMARY, "a") as f:
        f.write("\n".join(lines) + "\n")


def main() -> int:
    manifest = load_manifest()
    runs = fetch_check_runs()
    if not runs:
        print("ci-health: no check-runs for this commit yet.")
        return 0

    rows: list[tuple[str, str, str, str]] = []
    seen_contexts: set[str] = set()

    for run in runs:
        ctx = run["name"]
        # skip this workflow's own check-run to avoid self-reference noise
        if ctx.startswith("ci-health"):
            continue
        disp, entry = disposition_for(ctx, manifest)
        owner = (entry or {}).get("owner", "") if entry else ""
        if is_red(run):
            result = "RED"
        elif is_green(run):
            result = "GREEN"
        else:
            result = run.get("status", "?")
        rows.append((disp, ctx, result, owner))

        # Only maintain dedup issues for aggregator/whole contexts (not every
        # matrix leg) and only for non-gate dispositions.
        if entry is None or entry.get("internal"):
            continue
        if disp not in ("informational", "nightly-gate"):
            continue
        if ctx in seen_contexts:
            continue
        seen_contexts.add(ctx)
        if result == "RED":
            surface_red(entry, ctx, owner)
        elif result == "GREEN":
            surface_green(ctx)

    write_summary(rows)
    red_total = sum(1 for d, c, r, o in rows if r == "RED")
    print(f"ci-health: surfaced {len(rows)} checks, {red_total} red.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as e:
        print("ci-health: gh call failed:", e.stderr, file=sys.stderr)
        # Never fail the surface job on a transient API hiccup — it is advisory.
        sys.exit(0)
