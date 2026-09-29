#!/usr/bin/env python3
"""SSOT for the e2e SEAL shard partition: its selection, its runner, its proof.

Every `e2e` matrix shard runs `e2e_shard.py run K`, so a shard's test selection
is defined here once. `cover` proves, from the run's own nextest archive, that
the shards partition the archive exactly: every test is selected by exactly one
shard and no shard selects a test outside the archive. `seal-slice` runs it on
every run, so the "e2e covers every archived test" fact behind the SEAL is
checked, never assumed. `matrix` proves ci.yml's `e2e` matrix enumerates exactly
the shards defined here and that each runs through `run`; manifest-guard runs it.

Subcommands:
  run K     exec `cargo nextest run` for shard K (1..SHARDS) from the archive.
  cover     fail unless the SHARDS selections partition the archive's test set.
  matrix    fail unless ci.yml's `e2e` job runs exactly shards 1..SHARDS via `run`.

Every failure exits non-zero; a malformed listing or an unexpected shape is a
failure, never a pass.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
CI_WORKFLOW = os.path.join(REPO, ".github", "workflows", "ci.yml")
E2E_JOB = "e2e"
RUN_INVOCATION = 'python3 .github/ci/e2e_shard.py run "${{ matrix.shard }}"'

# The goldens whose emitted project drives a real multi-minute cold `cargo
# build` (webview/wry, server/axum, the live-HTTP set, the watch daemons). The
# `ci` nextest profile serializes this set into `heavy-server-e2e`
# (max-threads=1), so the HEAVY shards spread it across parallel runners and
# the LIGHT shards run the heavy-free complement at full width.
HEAVY = (
    "binary(watch_sigterm) | binary(watch_integration) | binary(watch_hot_appearance)"
    " | binary(watch_bluegreen) | binary(watch_cancellation) | binary(server_e2e)"
    " | binary(live_e2e) | binary(http_e2e) | binary(g_http_live) | binary(webview_e2e)"
    " | binary(test_command) | binary(verify) | binary(stdlib_coverage_dynamic)"
)
HEAVY_SHARDS = 5
LIGHT_SHARDS = 9
SHARDS = HEAVY_SHARDS + LIGHT_SHARDS

ARCHIVE_ARGS = ("--archive-file", "nextest.tar.zst", "--workspace-remap", ".", "--profile", "ci")
# `--test-threads=2`: each E2E test drives a memory-heavy emitted-project
# `cargo build` that itself uses every core; two in flight fit the runner.
RUN_ONLY_ARGS = ("--no-fail-fast", "--test-threads=2")


class ShardError(Exception):
    """A shard index, listing, or workflow shape that proves nothing."""


def selection(shard: int) -> list[str]:
    """Return the nextest filter + partition arguments of one shard."""
    if isinstance(shard, bool) or not isinstance(shard, int) or not 1 <= shard <= SHARDS:
        raise ShardError(f"shard must be an integer in 1..{SHARDS}, got {shard!r}")
    if shard <= HEAVY_SHARDS:
        return ["-E", HEAVY, "--partition", f"count:{shard}/{HEAVY_SHARDS}"]
    return ["-E", f"not ({HEAVY})", "--partition", f"count:{shard - HEAVY_SHARDS}/{LIGHT_SHARDS}"]


def parse_shard(text: str) -> int:
    """Parse a CLI shard index; anything but a canonical decimal is refused."""
    if not text.isascii() or not text.isdigit() or text != str(int(text)):
        raise ShardError(f"shard must be a decimal integer, got {text!r}")
    shard = int(text)
    selection(shard)
    return shard


def matched_tests(listing: str) -> list[str]:
    """Return the `binary-id test-name` of every test a nextest JSON listing selects."""
    try:
        doc = json.loads(listing)
    except json.JSONDecodeError as exc:
        raise ShardError(f"nextest listing is not JSON: {exc}") from exc
    suites = doc.get("rust-suites") if isinstance(doc, dict) else None
    if not isinstance(suites, dict):
        raise ShardError("nextest listing has no `rust-suites` object")
    out: list[str] = []
    for binary_id, suite in suites.items():
        cases = suite.get("testcases") if isinstance(suite, dict) else None
        if not isinstance(cases, dict):
            raise ShardError(f"suite {binary_id!r} has no `testcases` object")
        for name, case in cases.items():
            match = case.get("filter-match") if isinstance(case, dict) else None
            status = match.get("status") if isinstance(match, dict) else None
            if status not in ("matches", "mismatch"):
                raise ShardError(f"test {binary_id} {name} has no known filter-match status")
            if status == "matches":
                out.append(f"{binary_id} {name}")
    return out


def partition_errors(full: list[str], shards: dict[int, list[str]]) -> list[str]:
    """Return why the shard selections fail to partition `full`, or [] when they do."""
    errors: list[str] = []
    if not full:
        errors.append("the archive lists no tests: an empty set proves no coverage")
    if sorted(shards) != list(range(1, SHARDS + 1)):
        errors.append(f"shards listed {sorted(shards)}, expected 1..{SHARDS}")
    full_set = set(full)
    if len(full_set) != len(full):
        errors.append("the archive listing repeats a test id")
    owners: Counter[str] = Counter()
    for shard in sorted(shards):
        for test in shards[shard]:
            owners[test] += 1
            if test not in full_set:
                errors.append(f"shard {shard} selects {test}, which the archive does not list")
    for test in sorted(full_set):
        count = owners.get(test, 0)
        if count == 0:
            errors.append(f"no shard runs {test}")
        elif count > 1:
            errors.append(f"{test} runs in {count} shards")
    return errors


def _list(extra: list[str]) -> list[str]:
    cmd = ["cargo", "nextest", "list", *ARCHIVE_ARGS, "--message-format", "json", *extra]
    proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if proc.returncode != 0:
        raise ShardError(f"`{' '.join(cmd)}` exited {proc.returncode}:\n{proc.stderr}")
    return matched_tests(proc.stdout)


def cover() -> int:
    """Fail unless the SHARDS selections partition the archive's test set exactly."""
    full = _list([])
    shards = {k: _list(selection(k)) for k in range(1, SHARDS + 1)}
    errors = partition_errors(full, shards)
    for err in errors:
        print(f"e2e shard cover: {err}", file=sys.stderr)
    if errors:
        return 1
    print(f"e2e shard cover: {SHARDS} shards partition all {len(full)} archived tests exactly once.")
    return 0


def matrix_errors(workflow: dict) -> list[str]:
    """Return why ci.yml's `e2e` job does not run exactly shards 1..SHARDS via `run`."""
    jobs = workflow.get("jobs") if isinstance(workflow, dict) else None
    job = jobs.get(E2E_JOB) if isinstance(jobs, dict) else None
    if not isinstance(job, dict):
        return [f"ci.yml has no `{E2E_JOB}` job"]
    errors: list[str] = []
    strategy = job.get("strategy")
    matrix = strategy.get("matrix") if isinstance(strategy, dict) else None
    if not isinstance(matrix, dict) or set(matrix) != {"shard"}:
        errors.append(f"`{E2E_JOB}` matrix must have exactly one key, `shard`")
    else:
        shards = matrix["shard"]
        if shards != list(range(1, SHARDS + 1)):
            errors.append(f"`{E2E_JOB}` matrix shard is {shards!r}, expected 1..{SHARDS}")
    steps = job.get("steps")
    runs = [s.get("run") for s in steps if isinstance(s, dict)] if isinstance(steps, list) else []
    texts = [r for r in runs if isinstance(r, str)]
    if sum(RUN_INVOCATION in t for t in texts) != 1:
        errors.append(f"`{E2E_JOB}` must invoke `{RUN_INVOCATION}` in exactly one step")
    if any("nextest run" in t for t in texts):
        errors.append(f"`{E2E_JOB}` runs nextest directly; every selection must come from e2e_shard.py")
    return errors


def matrix(path: str = CI_WORKFLOW) -> int:
    """Fail unless ci.yml's `e2e` job enumerates exactly this module's shards."""
    sys.path.insert(0, HERE)
    import strict_yaml  # noqa: PLC0415  # PyYAML-backed; only this subcommand needs it

    with open(path, encoding="utf-8") as fh:
        workflow = strict_yaml.safe_load(fh)
    errors = matrix_errors(workflow)
    for err in errors:
        print(f"e2e shard matrix: {err}", file=sys.stderr)
    if errors:
        return 1
    print(f"e2e shard matrix: ci.yml runs shards 1..{SHARDS} through e2e_shard.py.")
    return 0


def main(argv: list[str]) -> int:
    try:
        if len(argv) == 2 and argv[0] == "run":
            args = ["cargo", "nextest", "run", *ARCHIVE_ARGS, *RUN_ONLY_ARGS, *selection(parse_shard(argv[1]))]
            sys.stdout.flush()
            os.execvp("cargo", args)
        if argv == ["cover"]:
            return cover()
        if argv == ["matrix"]:
            return matrix()
    except ShardError as exc:
        print(f"e2e_shard: {exc}", file=sys.stderr)
        return 1
    print("usage: e2e_shard.py run K | cover | matrix", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
