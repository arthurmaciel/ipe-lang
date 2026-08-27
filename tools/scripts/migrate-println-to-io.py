#!/usr/bin/env python3
"""Migrate `println` from the (removed) `Ipe.Log`/Prelude surface to `Io.println`.

`Ipe.Log` is observability-only; bare line printing now lives in `Ipe.Io`
(`Io.println` / `Io.eprintln`). Every `.ipe` file that printed a line via the
old unqualified Prelude `println`, an `import Ipe.Log exposing (println)`, or a
qualified `Log.println` is rewritten to call `Io.println`, importing
`Ipe.Io as Io` exactly once.

Idempotent: a file already on `Io.println` with an `Ipe.Io` import is left as-is.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Word-boundary `println` NOT already qualified as `Io.println` / `Log.println`
# and not part of a longer identifier. A preceding `.` (any qualifier) is
# excluded so only the BARE unqualified call is rewritten by this pattern;
# `Log.println` is handled separately below.
BARE_PRINTLN = re.compile(r"(?<![\w.])println\b")
QUALIFIED_LOG_PRINTLN = re.compile(r"\bLog\.println\b")

IMPORT_IO = "import Ipe.Io as Io"


def is_skipped(path: Path) -> bool:
    parts = path.as_posix()
    # The embedded standard-library sources ARE the kernel definitions of
    # `Ipe.Io` / `Ipe.Log` / etc. — `println`/`eprintln` there are the kernel
    # `Ffi.kernel` bindings, not call sites, and must never be rewritten.
    if "/stdlib/" in parts:
        return True
    return False


def has_io_import(text: str) -> bool:
    return re.search(r"^import Ipe\.Io\b", text, re.MULTILINE) is not None


def uses_other_log_member(text: str) -> bool:
    # Any `Log.<member>` other than `println`.
    return re.search(r"\bLog\.(?!println\b)\w+", text) is not None


def last_import_line_index(lines: list[str]) -> int:
    idx = -1
    for i, line in enumerate(lines):
        if line.startswith("import "):
            idx = i
    return idx


def migrate(text: str) -> str:
    lines = text.split("\n")
    needs_io_import = False
    io_already = has_io_import(text)

    # --- Import rewrites -----------------------------------------------------
    new_lines: list[str] = []
    for line in lines:
        stripped = line.rstrip()
        if stripped == "import Ipe.Log exposing (println)":
            # Log was imported ONLY to expose println → replace with Io (unless
            # the file already imports Io, in which case just drop the line).
            if not io_already:
                new_lines.append(IMPORT_IO)
                io_already = True
            continue
        if stripped == "import Ipe.Log as Log exposing (println)":
            # Keep the `as Log` alias (other Log.* members may be used); drop
            # the println exposure and add the Io import if not already present.
            new_lines.append("import Ipe.Log as Log")
            if not io_already:
                new_lines.append(IMPORT_IO)
                io_already = True
            continue
        new_lines.append(line)
    lines = new_lines
    text = "\n".join(lines)

    # A file that qualified `Log.println` but imported ONLY `import Ipe.Log as
    # Log` (no other Log member used) can drop the Log import in favour of Io.
    if QUALIFIED_LOG_PRINTLN.search(text) and not uses_other_log_member(
        QUALIFIED_LOG_PRINTLN.sub("", text)
    ):
        # Remove a bare `import Ipe.Log as Log` line if present.
        lines = text.split("\n")
        kept: list[str] = []
        removed_log = False
        for line in lines:
            if line.rstrip() == "import Ipe.Log as Log" and not removed_log:
                removed_log = True
                continue
            kept.append(line)
        if removed_log:
            lines = kept
            text = "\n".join(lines)

    # --- Call-site rewrites --------------------------------------------------
    before = text
    text = QUALIFIED_LOG_PRINTLN.sub("Io.println", text)
    text = BARE_PRINTLN.sub("Io.println", text)

    io_already = has_io_import(text)
    if text != before and not io_already:
        needs_io_import = True

    # --- Ensure the Io import exists exactly once ----------------------------
    if needs_io_import and not io_already:
        lines = text.split("\n")
        insert_at = last_import_line_index(lines)
        if insert_at >= 0:
            lines.insert(insert_at + 1, IMPORT_IO)
        else:
            # No imports at all — put it after the module header (line 0).
            lines.insert(1, IMPORT_IO)
        text = "\n".join(lines)

    return text


def main() -> int:
    roots = [REPO / "examples", REPO / "tests", REPO / "src"]
    changed = 0
    scanned = 0
    for root in roots:
        for path in root.rglob("*.ipe"):
            if is_skipped(path):
                continue
            scanned += 1
            original = path.read_text()
            migrated = migrate(original)
            if migrated != original:
                path.write_text(migrated)
                changed += 1
    print(f"scanned {scanned} .ipe files, rewrote {changed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
