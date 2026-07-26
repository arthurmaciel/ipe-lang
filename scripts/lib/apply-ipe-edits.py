#!/usr/bin/env python3
"""Apply an example's content-anchored edits over its mirrored Ipê tree.

Per-example semantic deltas that the shared token rewrite (rename-map.tsv)
cannot express live in examples/sky/ipe-edits/<name>.edits. Each edit is a
find/replace anchored on the EXACT source text — never a line number — so it
survives the line shifts, insertions, and reindentation upstream makes between
releases. An edit only fails when its `find` text genuinely changes upstream,
which is a real semantic drift that must be reviewed, not a silent mis-apply.

FILE FORMAT (dependency-free; parsed here, no TOML/JSON lib). A fence is a line
containing only three double-quotes; below, F marks where such a fence line goes:

    # Any leading '#' lines are the rationale for the whole file.

    [[edit]]
    file: src/Main.ipe
    find:
    F
    <exact text to find — verbatim, may span multiple lines>
    F
    replace:
    F
    <replacement text — verbatim>
    F

Rules:
  - `find` must occur EXACTLY ONCE in `file` (zero or many => error).
  - `all: true` before the `find:` block relaxes that to "every occurrence"
    (>= 1 required) — a file-scoped identifier rename, e.g. an env-var name.
  - Omit the whole `find:` block => `replace` overwrites the file entirely
    (for a total port, e.g. a Go-FFI example reimplemented on Ipe.Http.Server).
  - A fence (a lone triple-double-quote line) opens the block and the next lone
    fence line closes it. Fence content is verbatim; a source line that is
    itself only a triple-double-quote is unsupported (none exists in the corpus).

Usage: apply-ipe-edits.py <name>.edits <example-dir>
Exit:  0 ok · 2 an edit failed to apply (drift — surface as a RED row).
"""

import sys
from pathlib import Path


def parse_edits(text):
    """Parse the .edits file into a list of {file, find|None, replace} dicts."""
    lines = text.splitlines()
    edits = []
    cur = None
    i = 0
    n = len(lines)

    def read_fence(start):
        # `start` indexes the line that must be the opening `"""`.
        if start >= n or lines[start].strip() != '"""':
            raise ValueError(f'expected opening """ at line {start + 1}')
        body = []
        j = start + 1
        while j < n and lines[j].strip() != '"""':
            body.append(lines[j])
            j += 1
        if j >= n:
            raise ValueError('unterminated """ fence')
        return "\n".join(body), j + 1  # index past the closing fence

    while i < n:
        raw = lines[i]
        stripped = raw.strip()
        if cur is None and (stripped == "" or stripped.startswith("#")):
            i += 1
            continue
        if stripped == "[[edit]]":
            cur = {"file": None, "find": None, "replace": None, "all": False}
            edits.append(cur)
            i += 1
            continue
        if cur is None:
            raise ValueError(f'stray text before first [[edit]] at line {i + 1}: {raw!r}')
        if stripped.startswith("file:"):
            cur["file"] = stripped[len("file:"):].strip()
            i += 1
            continue
        if stripped.startswith("all:"):
            cur["all"] = stripped[len("all:"):].strip().lower() in ("true", "1", "yes")
            i += 1
            continue
        if stripped == "find:":
            cur["find"], i = read_fence(i + 1)
            continue
        if stripped == "replace:":
            cur["replace"], i = read_fence(i + 1)
            continue
        if stripped == "" or stripped.startswith("#"):
            i += 1
            continue
        raise ValueError(f'unrecognised line {i + 1}: {raw!r}')
    return edits


def apply_edit(exdir, edit):
    """Apply one edit in-place. Returns None on success or an error string."""
    rel = edit["file"]
    if not rel:
        return "edit missing `file:`"
    target = exdir / rel
    if not target.is_file():
        return f"{rel}: target file not found"
    if edit["replace"] is None:
        return f"{rel}: edit missing `replace:` block"

    if edit["find"] is None:
        # Whole-file overwrite (a total port). Trailing newline preserved.
        target.write_text(edit["replace"] + "\n", encoding="utf-8")
        return None

    src = target.read_text(encoding="utf-8")
    count = src.count(edit["find"])
    head = edit["find"].splitlines()[0] if edit["find"] else ""
    if count == 0:
        return f"{rel}: `find` text not present (upstream drifted?): {head!r}"
    if not edit["all"] and count > 1:
        return f"{rel}: `find` text is ambiguous ({count} matches — add context or `all: true`): {head!r}"
    n = -1 if edit["all"] else 1  # str.replace(-1) => all occurrences
    target.write_text(src.replace(edit["find"], edit["replace"], n), encoding="utf-8")
    return None


def main(argv):
    if len(argv) != 3:
        print("usage: apply-ipe-edits.py <name>.edits <example-dir>", file=sys.stderr)
        return 2
    edits_path = Path(argv[1])
    exdir = Path(argv[2])
    try:
        edits = parse_edits(edits_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"apply-ipe-edits: {edits_path}: {e}", file=sys.stderr)
        return 2
    for edit in edits:
        err = apply_edit(exdir, edit)
        if err:
            print(f"apply-ipe-edits: {edits_path.name}: {err}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
