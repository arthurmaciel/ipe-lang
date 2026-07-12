#!/usr/bin/env python3
"""backlog_table.py — the ONLY place that knows the BACKLOG.md GFM-table
format. backlog.sh shells out to this for the two operations that need real
table parsing: the one-time `migrate` (BACKLOG.md -> backlog.jsonl) and the
ongoing `render` (backlog.jsonl -> BACKLOG.md), which MUST be exact inverses
of each other — `render(migrate(BACKLOG.md)) == BACKLOG.md` byte-for-byte is
the proof that this parser is sound, not a fragile ad-hoc regex trusted on
faith. Everyday CRUD (add/claim/unclaim/close/list) lives in backlog.sh and
only ever appends/rewrites whole JSONL lines — it never re-parses markdown.

Cell-splitting rule: split each table row on `|` NOT preceded by `\` — this
file's one existing table already escapes its one literal-pipe-in-a-cell
case (`{ r \| f : T }`) rather than relying on backtick-aware parsing, so a
plain "unescaped pipe" split is correct for this table without needing full
GFM code-span awareness. If a future row introduces an unescaped `|` inside
a code span, `verify` (render == source) will fail loudly rather than
silently misparsing — that is the intended failure mode.
"""
import json
import re
import sys

PHASES = [
    "Sweep to green",
    "Security hardening",
    "CI, oracle & publish",
    "Hardening follow-ups",
    "FFI",
    "Post-completion",
    "Longer-horizon",
    "Designed targets",
]
PRIORITIES = ["Critical", "High", "Medium", "Low"]

HEADER_MARKER = "| Priority | Road map phase | Task | Notes | Spec |"
SEP_MARKER = "|---|---|---|---|---|"

ID_RE = re.compile(r"^(#[A-Za-z0-9][\w.\-]*|[A-Z]\.\d+)\b")


def split_row(line):
    inner = line.strip()
    assert inner.startswith("|") and inner.endswith("|"), f"not a table row: {line!r}"
    inner = inner[1:-1]
    parts = re.split(r"(?<!\\)\|", inner)
    return [p.strip().replace("\\|", "|") for p in parts]


def join_row(cells):
    escaped = [c.replace("|", "\\|") for c in cells]
    tokens = [f" {c} " if c else " " for c in escaped]
    return "|" + "|".join(tokens) + "|"


def extract_id(task, seq):
    m = ID_RE.match(task)
    if m:
        return m.group(1).lstrip("#")
    return f"row-{seq:03d}"


def migrate(md_path, jsonl_path):
    with open(md_path, encoding="utf-8") as f:
        lines = f.read().split("\n")

    header_idx = next(i for i, l in enumerate(lines) if l == HEADER_MARKER)
    assert lines[header_idx + 1] == SEP_MARKER, "unexpected separator row"
    preamble = lines[:header_idx]

    rows = []
    i = header_idx + 2
    seq = 0
    while i < len(lines) and lines[i].startswith("|"):
        cells = split_row(lines[i])
        assert len(cells) == 5, f"expected 5 cells, got {len(cells)}: {lines[i]!r}"
        priority, phase, task, notes, spec = cells
        assert priority in PRIORITIES, f"unknown priority {priority!r} in row: {task[:60]!r}"
        assert phase in PHASES, f"unknown phase {phase!r} in row: {task[:60]!r}"
        seq += 1
        rows.append(
            {
                "id": extract_id(task, seq),
                "priority": priority,
                "phase": phase,
                "task": task,
                "notes": notes,
                "spec": spec,
                "status": "pending",
            }
        )
        i += 1
    trailer = lines[i:]

    ids = [r["id"] for r in rows]
    dupes = {x for x in ids if ids.count(x) > 1}
    if dupes:
        print(f"WARNING: duplicate extracted ids (kept as-is, review manually): {dupes}", file=sys.stderr)

    with open(jsonl_path, "w", encoding="utf-8") as f:
        f.write(json.dumps({"__preamble__": preamble}) + "\n")
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
        f.write(json.dumps({"__trailer__": trailer}) + "\n")

    print(f"migrated {len(rows)} rows -> {jsonl_path}", file=sys.stderr)


def render(jsonl_path, md_path):
    preamble, trailer, rows = None, None, []
    with open(jsonl_path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            obj = json.loads(line)
            if "__preamble__" in obj:
                preamble = obj["__preamble__"]
            elif "__trailer__" in obj:
                trailer = obj["__trailer__"]
            else:
                rows.append(obj)

    assert preamble is not None and trailer is not None, "jsonl missing preamble/trailer sentinel lines"

    out = list(preamble)
    out.append(HEADER_MARKER)
    out.append(SEP_MARKER)
    for r in rows:
        if r.get("status") == "done":
            continue
        out.append(join_row([r["priority"], r["phase"], r["task"], r["notes"], r["spec"]]))
    out.extend(trailer)

    with open(md_path, "w", encoding="utf-8") as f:
        f.write("\n".join(out))
        if not out or out[-1] != "":
            f.write("\n")


def main():
    if len(sys.argv) < 4 or sys.argv[1] not in ("migrate", "render"):
        print("usage: backlog_table.py migrate <BACKLOG.md> <backlog.jsonl>", file=sys.stderr)
        print("       backlog_table.py render  <backlog.jsonl> <BACKLOG.md>", file=sys.stderr)
        sys.exit(2)
    cmd, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
    if cmd == "migrate":
        migrate(a, b)
    else:
        render(a, b)


if __name__ == "__main__":
    main()
