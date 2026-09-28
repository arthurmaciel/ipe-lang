#!/usr/bin/env python3
"""Publish `ci/deterministic-checks.json`'s (context, step) pairs as a step output.

Appends `checks=<compact JSON array of {"context", "step"}>` to
"$GITHUB_OUTPUT", one line, so a later step reads the pairs through `env:`
instead of naming `.github/ci/**` itself. Any shape other than a `checks`
list of objects with exactly string `context` and `step` (an unreadable or
non-JSON file included) fails (exit 1) and writes nothing, so a consumer sees no output and keeps its fail-safe default.
"""
from __future__ import annotations

import json
import os
import sys

SSOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "deterministic-checks.json")


def load_pairs() -> list[dict[str, str]] | None:
    try:
        with open(SSOT, encoding="utf-8") as f:
            doc = json.load(f)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    checks = doc.get("checks") if isinstance(doc, dict) else None
    if not isinstance(checks, list) or not checks:
        return None
    pairs: list[dict[str, str]] = []
    for entry in checks:
        if not isinstance(entry, dict) or set(entry) != {"context", "step"}:
            return None
        ctx, step = entry["context"], entry["step"]
        if not isinstance(ctx, str) or not isinstance(step, str):
            return None
        pairs.append({"context": ctx, "step": step})
    return pairs


def main() -> int:
    pairs = load_pairs()
    if pairs is None:
        print(f"{SSOT}: not a non-empty `checks` list of {{context, step}} strings", file=sys.stderr)
        return 1
    out_path = os.environ.get("GITHUB_OUTPUT")
    if not out_path:
        print("GITHUB_OUTPUT is unset; nothing published", file=sys.stderr)
        return 1
    line = json.dumps(pairs, separators=(",", ":"), ensure_ascii=True)
    with open(out_path, "a", encoding="utf-8") as out:
        out.write(f"checks={line}\n")
    print(f"published {len(pairs)} deterministic check pairs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
