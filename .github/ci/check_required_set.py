#!/usr/bin/env python3
"""Check that `ci/required-set.json` is the manifest's derived required set.

The required set is every `gate` context of `ci/check-manifest.yml`, sorted.
A difference in either direction is reported as missing/extra and fails
(exit 1); the file is regenerated per `ci/RECONCILIATION.md`, never hand-kept.
"""
from __future__ import annotations

import json
import os
import sys

CI_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, CI_DIR)
import strict_yaml  # noqa: E402

MANIFEST = os.path.join(CI_DIR, "check-manifest.yml")
REQUIRED_SET = os.path.join(CI_DIR, "required-set.json")


def main() -> int:
    with open(MANIFEST, encoding="utf-8") as f:
        man = strict_yaml.safe_load(f)
    required = sorted(e["context"] for e in man["checks"] if e["disposition"] == "gate")
    with open(REQUIRED_SET, encoding="utf-8") as f:
        on_disk = json.load(f)
    if not isinstance(on_disk, list) or not all(isinstance(c, str) for c in on_disk):
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
