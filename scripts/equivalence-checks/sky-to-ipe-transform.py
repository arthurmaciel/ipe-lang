#!/usr/bin/env python3
"""Rewrite a mirrored Sky example into Ipe, SYNTACTIC-ONLY.

The examples-sweep mirrors an upstream Sky example verbatim, then runs this
transform to turn `Sky.Core.*` / `Sky.Http.*` / `Sky.Ffi` / `Sky.Test` / `Std.*`
module qualifiers into their `Ipe.*` equivalents (see examples/sky/rename-map.tsv).

The one hard rule: rewrite CODE only, never a string literal or a comment. Sky
example prose ("Sky.Live Counter", the "Std.Ui showcase" label, a window title)
must stay byte-identical — both because a syntactic patch may not change program
behaviour, and because the Go-vs-Rust equivalence diff compares the Rust build
against a Go reference built from the ORIGINAL Sky source, which prints those
strings verbatim. Rewriting them would manufacture a false divergence.

Recognised lexical spans that are skipped:
  - `--` line comments (to end of line)
  - `{- ... -}` block comments (nested)
  - `"..."` string literals (with `\\` escapes)
  - `\"\"\"...\"\"\"` triple-quoted multiline strings
Inside a triple-quoted string, a `{{ expr }}` interpolation IS code, so
qualifiers there are rewritten (the expression is real Ipe). A `\\{{` escaped
placeholder stays literal (not interpolation) and is left alone.

Usage: sky-to-ipe-transform.py <rename-map.tsv> <file.ipe> [<file.ipe> ...]
Edits each file in place. Exit 0 on success.
"""
from __future__ import annotations

import sys


def load_map(path: str) -> list[tuple[str, str]]:
    pairs: list[tuple[str, str]] = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2:
                continue
            src, dst = parts[0], parts[1]
            if src:
                pairs.append((src, dst))
    # Longest source prefix first so Sky.Core.Http.Stream binds before Sky.Core.
    pairs.sort(key=lambda p: len(p[0]), reverse=True)
    return pairs


def rewrite_code(segment: str, pairs: list[tuple[str, str]]) -> str:
    """Apply the qualifier rewrites to a CODE segment (no strings/comments).

    A qualifier match is anchored on its left by a non-identifier boundary so a
    longer identifier that merely ends in the prefix text can't be split. The
    prefixes all start with an uppercase module root (`Sky`/`Std`), which never
    begins mid-identifier in well-formed Sky, so a left boundary check suffices.
    """
    out = []
    i = 0
    n = len(segment)
    while i < n:
        matched = False
        # Left boundary: start of segment or a non-identifier char before.
        left_ok = i == 0 or not (segment[i - 1].isalnum() or segment[i - 1] == "_")
        if left_ok:
            for src, dst in pairs:
                if segment.startswith(src, i):
                    out.append(dst)
                    i += len(src)
                    matched = True
                    break
        if not matched:
            out.append(segment[i])
            i += 1
    return "".join(out)


def transform(text: str, pairs: list[tuple[str, str]]) -> str:
    out: list[str] = []
    i = 0
    n = len(text)
    code_start = 0  # start of the current un-flushed code run

    def flush_code(end: int) -> None:
        nonlocal code_start
        if end > code_start:
            out.append(rewrite_code(text[code_start:end], pairs))
        code_start = end

    while i < n:
        c = text[i]
        two = text[i : i + 2]
        three = text[i : i + 3]

        # `--` line comment (but not `-->`-style; Sky has no custom operators, so
        # a bare `--` always starts a comment outside a string).
        if two == "--":
            flush_code(i)
            j = text.find("\n", i)
            j = n if j == -1 else j
            out.append(text[i:j])  # verbatim
            i = j
            code_start = i
            continue

        # `{- ... -}` block comment, nested.
        if two == "{-":
            flush_code(i)
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if text[j : j + 2] == "{-":
                    depth += 1
                    j += 2
                elif text[j : j + 2] == "-}":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            out.append(text[i:j])  # verbatim
            i = j
            code_start = i
            continue

        # Triple-quoted multiline string with `{{ }}` interpolation.
        if three == '"""':
            flush_code(i)
            out.append('"""')
            j = i + 3
            seg_start = j
            while j < n:
                if text[j : j + 3] == '"""':
                    break
                # Escaped placeholder `\{{` — literal, skip the marker.
                if text[j] == "\\" and text[j + 1 : j + 3] == "{{":
                    j += 3
                    continue
                # Interpolation `{{ expr }}` — expr is CODE.
                if text[j : j + 2] == "{{":
                    out.append(text[seg_start:j])  # literal chunk, verbatim
                    k = text.find("}}", j + 2)
                    k = n if k == -1 else k
                    out.append("{{")
                    out.append(rewrite_code(text[j + 2 : k], pairs))
                    out.append("}}")
                    j = k + 2
                    seg_start = j
                    continue
                j += 1
            out.append(text[seg_start:j])  # trailing literal chunk
            if j < n:
                out.append('"""')
                j += 3
            i = j
            code_start = i
            continue

        # `"..."` single-line string literal with `\` escapes.
        if c == '"':
            flush_code(i)
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                if text[j] == "\n":  # unterminated; bail at EOL
                    break
                j += 1
            out.append(text[i:j])  # verbatim
            i = j
            code_start = i
            continue

        i += 1

    flush_code(n)
    return "".join(out)


def prefix_bare_imports(text: str, bare: frozenset[str]) -> str:
    """Prefix `import <Name>` -> `import Ipe.<Name>` for bare stdlib modules.

    Sky allows a bare top-level stdlib import (`import System`, `import Io`);
    Ipê requires the `Ipe.` prefix (`import Ipe.System`). Only the module PATH on
    an `import` statement is rewritten — the `as <Alias>` / `exposing (...)`
    remainder is untouched, so downstream call sites keep using the same alias
    and need no change. `bare` is the caller-computed set of names that ARE
    top-level Ipê stdlib modules AND are NOT shadowed by a local `<Name>.ipe` in
    the example (a composite's local `Server`/`Head`/`Auth` module wins and is
    kept out of the set), so a local module import is never mis-prefixed.
    """
    if not bare:
        return text
    out_lines = []
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import "):
            indent = line[: len(line) - len(stripped)]
            rest = stripped[len("import ") :]
            # The module path is the first whitespace-delimited token.
            head = rest.split(None, 1)
            name = head[0] if head else ""
            if name in bare:
                tail = head[1] if len(head) > 1 else ""
                line = f"{indent}import Ipe.{name}" + (f" {tail}" if tail else "")
        out_lines.append(line)
    return "\n".join(out_lines)


def main(argv: list[str]) -> int:
    args = argv[1:]
    bare: frozenset[str] = frozenset()
    rest: list[str] = []
    i = 0
    while i < len(args):
        if args[i] == "--bare-stdlib" and i + 1 < len(args):
            bare = frozenset(n for n in args[i + 1].split(",") if n)
            i += 2
            continue
        rest.append(args[i])
        i += 1
    if len(rest) < 2:
        sys.stderr.write(
            "usage: sky-to-ipe-transform.py [--bare-stdlib N1,N2] "
            "<rename-map.tsv> <file> [<file> ...]\n"
        )
        return 2
    pairs = load_map(rest[0])
    for path in rest[1:]:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        new = prefix_bare_imports(transform(text, pairs), bare)
        if new != text:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(new)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
