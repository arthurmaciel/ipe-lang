#!/usr/bin/env python3
"""Check that `ci/required-set.json` is the manifest's derived required set.

The required set is every `gate` context of `ci/check-manifest.yml`, sorted.
A difference in either direction is reported as missing/extra and fails
(exit 1), as does either file being unreadable or malformed, with nothing
printed to stdout; the file is regenerated per `ci/RECONCILIATION.md`, never hand-kept.
"""
from __future__ import annotations

import json
import os
import sys

CI_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, CI_DIR)
import strict_yaml  # noqa: E402
import yaml  # noqa: E402

MANIFEST = os.path.join(CI_DIR, "check-manifest.yml")
REQUIRED_SET = os.path.join(CI_DIR, "required-set.json")


def load_required() -> list[str] | None:
    """The manifest's sorted `gate` contexts, or None when the manifest is
    unreadable or not a `checks` list of entries with string `context` and
    `disposition`."""
    try:
        with open(MANIFEST, encoding="utf-8") as f:
            man = strict_yaml.safe_load(f)
    except (OSError, UnicodeDecodeError, yaml.YAMLError):
        return None
    checks = man.get("checks") if isinstance(man, dict) else None
    if not isinstance(checks, list):
        return None
    required: list[str] = []
    for e in checks:
        if not isinstance(e, dict):
            return None
        ctx, disp = e.get("context"), e.get("disposition")
        if not isinstance(ctx, str) or not isinstance(disp, str):
            return None
        if disp == "gate":
            required.append(ctx)
    return sorted(required)


def load_on_disk() -> list[str] | None:
    """`required-set.json` as a list of context names, or None when it is
    unreadable or any other shape."""
    try:
        with open(REQUIRED_SET, encoding="utf-8") as f:
            on_disk = json.load(f)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return None
    if not isinstance(on_disk, list) or not all(isinstance(c, str) for c in on_disk):
        return None
    return on_disk


def main() -> int:
    required = load_required()
    if required is None:
        print(".github/ci/check-manifest.yml is not a `checks` list of {context, disposition} strings", file=sys.stderr)
        return 1
    on_disk = load_on_disk()
    if on_disk is None:
        print(".github/ci/required-set.json is not a JSON list of context names", file=sys.stderr)
        return 1
    if required != sorted(on_disk):
        print(
            ".github/ci/required-set.json is stale — regenerate it (see .github/ci/RECONCILIATION.md):",
            file=sys.stderr,
        )
        print("  missing:", sorted(set(required) - set(on_disk)), file=sys.stderr)
        print("  extra:  ", sorted(set(on_disk) - set(required)), file=sys.stderr)
        return 1
    print(f"ci/required-set.json matches the manifest ({len(required)} required contexts).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
