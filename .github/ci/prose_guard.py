#!/usr/bin/env python3
"""Verdict: the change classifier's PROSE set is provably unread.

Fails unless no build, test, script, or CI step can read a PROSE path and no
workflow runs a second path filter (`change_class.guard`). A separate tool from
the classifier because the two carry different roles: the classifier is
advisory (its crash runs every tier), this guard is a verdict (its failure
blocks the merge).
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import change_class  # noqa: E402


def main(argv: list[str]) -> int:
    if argv:
        print(__doc__, file=sys.stderr)
        return 2
    root = change_class.git(".", "rev-parse", "--show-toplevel").decode().strip()
    errors = change_class.guard(root, change_class.tracked_files(root))
    for e in errors:
        print(f"prose guard: {e}", file=sys.stderr)
    if errors:
        print(
            "A PROSE entry must be provably unread; remove the reader or the "
            "entry in .github/ci/change_class.py.",
            file=sys.stderr,
        )
        return 1
    print(f"prose guard: PROSE set sound ({len(change_class.prose_entries())} entries)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
