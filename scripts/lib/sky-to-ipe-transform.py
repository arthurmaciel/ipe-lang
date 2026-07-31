#!/usr/bin/env python3
"""Rewrite a mirrored Sky example into Ipe, SYNTACTIC-ONLY.

The examples-sweep mirrors an upstream Sky example verbatim, then runs this
transform to turn `Sky.Core.*` / `Sky.Http.*` / `Sky.Ffi` / `Sky.Test` / `Std.*`
module qualifiers into their `Ipe.*` equivalents (see examples/sky/rename-map.tsv).

Beyond the qualifier-prefix rename, a small MEMBER-MOVE pass relocates named
values that changed module between Sky and Ipê: `println`/`eprintln` moved from
`Std.Log` to `Ipe.Io`, so both the import statement and every call site are
rewritten together (an exposed bare `println` -> `Io.println`; a `Log.println`
alias call -> `Io.println`). A `Std.Log` import used only for its remaining
members (infoWith/errorWith/…) is left to the ordinary prefix rename (-> Ipe.Log).

A REMOVED-MODULE pass then drops import lines for modules Ipê no longer has:
Sky's open `import Sky.Core.Prelude exposing (..)` becomes `Ipe.Prelude` under
the prefix rename, but Ipê auto-imports its Tier-A `Ipe.Basics` surface (ADR
0047), so `Ipe.Prelude` does not exist and the line is deleted rather than
emitted as an unresolvable import.

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

import re
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


def walk_code(text: str, code_fn) -> str:
    """Rebuild `text`, passing each CODE segment through `code_fn`.

    `code_fn(segment) -> str` sees only code — never a string literal or a
    comment, which are copied verbatim. This is the single place that knows Sky's
    lexical spans, so every code-only rewrite (the qualifier prefix rename, the
    stdlib member move) shares one correct string/comment skipper.
    """
    out: list[str] = []
    i = 0
    n = len(text)
    code_start = 0  # start of the current un-flushed code run

    def flush_code(end: int) -> None:
        nonlocal code_start
        if end > code_start:
            out.append(code_fn(text[code_start:end]))
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
                    out.append(code_fn(text[j + 2 : k]))
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


def transform(text: str, pairs: list[tuple[str, str]]) -> str:
    """Apply the qualifier-prefix rename to every code span (rename-map.tsv)."""
    return walk_code(text, lambda seg: rewrite_code(seg, pairs))


# ── Shape-scoped Cmd/Sub re-home (semantic, per-example) ──────────────────────
# `Cmd` / `Sub` are shape-specific: user code reaches them through the app's own
# shape (`Ipe.Tea.Web.Cmd`, `Ipe.Tea.Terminal.Sub`, …), never a global
# `Ipe.Cmd` / `Ipe.Sub`. This is NOT a qualifier-prefix rename — the target
# depends on the EXAMPLE's shape, which the token map cannot know — so it runs as
# a per-example pass over the whole file set once the shape is known.

# The shape-module import each example carries, keyed to its shape name. Keyed
# off the IMPORTED SHAPE MODULE (not the entry-kernel call) so a multi-module
# example whose `Cmd` / `Sub` import lives in a helper still re-homes onto the
# app's shape once any of its files names the shape. `WebView` is checked before
# `Web` so the longer name wins. The upstream Sky vocabulary (`Web` / `WebView` /
# `Tui` / `Console` / `Cli`) and the current `Ipe.Tea.<Shape>` forms both map
# here.
_SHAPE_MODULES: list[tuple[str, str]] = [
    ("Ipe.Tea.WebView", "WebView"),
    ("Ipe.WebView", "WebView"),
    ("Ipe.Tea.Web", "Web"),
    ("Ipe.Web", "Web"),
    ("Ipe.Live", "Web"),
    ("Ipe.Tea.Terminal", "Terminal"),
    ("Ipe.Terminal", "Terminal"),
    ("Ipe.Tui", "Terminal"),
    ("Ipe.Console", "Terminal"),
    ("Ipe.Cli", "Terminal"),
]


def detect_shape(texts: list[str]) -> str | None:
    """The example's TEA shape, proven from the shape module it imports.

    Scans the already-prefix-renamed sources for an `import <shape module>` line
    (`Ipe.Web`, `Ipe.Tui`, `Ipe.Tea.Terminal`, …). Returns the shape name (`Web`
    / `Terminal` / `WebView`) or `None` when no shape module is imported (a plain
    Program, or an example that does not use `Cmd` / `Sub`). Longer module names
    are checked first so `Ipe.WebView` wins over `Ipe.Web`.
    """
    for module, shape in _SHAPE_MODULES:
        pat = rf"^[ \t]*import[ \t]+{re.escape(module)}(?:[ \t]|$)"
        if any(re.search(pat, t, flags=re.MULTILINE) for t in texts):
            return shape
    return None


def rehome_cmd_sub(text: str, shape: str) -> str:
    """Rewrite `import Ipe.Cmd` / `import Ipe.Sub` onto the example's shape.

    Only the import module PATH moves; the `as Alias` binding (and therefore
    every `Cmd.` / `Sub.` call site) is untouched. Matches the whole
    `Ipe.Cmd` / `Ipe.Sub` module qualifier on an `import` line, so a longer path
    that merely starts with it (there is none today) cannot be split.
    """

    def repl(m: "re.Match[str]") -> str:
        return f"import Ipe.Tea.{shape}.{m.group('leaf')}{m.group('tail')}"

    return re.sub(
        r"^(?P<indent>[ \t]*)import[ \t]+Ipe\.(?P<leaf>Cmd|Sub)(?P<tail>[ \t]|$)",
        lambda m: f"{m.group('indent')}" + repl(m),
        text,
        flags=re.MULTILINE,
    )


# ── Stdlib member move (Std.Log's println/eprintln -> Ipe.Io) ─────────────────
# A member move is NOT a qualifier-prefix rename (which rename-map.tsv expresses):
# it takes named values OUT of one module and puts them in ANOTHER, so the import
# statement and every call site must change together. `println`/`eprintln` moved
# from Std.Log to Ipe.Io; Std.Log's remaining members (infoWith/errorWith/…) still
# map by prefix to Ipe.Log, so a file that uses only those is untouched here and
# handled by the ordinary Std. -> Ipe. row.
#
# Each entry: source module -> (target module path, target import alias, moved
# member names). Longest-prefix concerns don't apply — a member move keys off the
# whole module qualifier, not a dotted prefix.
# `Sky.Core.Pure` has no Ipê counterpart: its members are point-free companions
# of arity-0 effect kernels, and Ipê registers those kernels directly at
# `() -> Task Error a`, so `Pure.foo ()` desugars to the canonical kernel form
# `<Module>.<name> ()` — a different destination module AND a different member
# name per companion. That is neither a prefix rename (rename-map.tsv) nor a
# single-destination member move (MEMBER_MOVES), so it gets its own pass.
#
# member -> (canonical module path, import alias, canonical member name)
PURE_DESUGAR: dict[str, tuple[str, str, str]] = {
    "uuidV4": ("Ipe.Uuid", "Uuid", "v4"),
    "uuidV7": ("Ipe.Uuid", "Uuid", "v7"),
    "timeNow": ("Ipe.Time", "Time", "now"),
    "timeUnixMillis": ("Ipe.Time", "Time", "unixMillis"),
    "systemArgs": ("Ipe.System", "System", "args"),
    "systemCwd": ("Ipe.System", "System", "cwd"),
    "systemLoadEnv": ("Ipe.System", "System", "loadEnv"),
    "ioReadLine": ("Ipe.Io", "Io", "readLine"),
    "dbConnect": ("Ipe.Db", "Db", "connect"),
}


MEMBER_MOVES: dict[str, tuple[str, str, frozenset[str]]] = {
    "Std.Log": ("Ipe.Io", "Io", frozenset({"println", "eprintln"})),
}


def _rewrite_calls(text: str, src_alias: str, dst_alias: str, members: frozenset[str]) -> str:
    """Requalify moved call sites in CODE only: `<src>.m`/bare `m` -> `<dst>.m`.

    `src_alias` is the import alias the moved members were reached through in the
    source file — the module's own `as` alias (`Log.println`), or "" when they were
    exposed unqualified (bare `println`). Only the listed members are touched, so a
    same-alias non-moved reference (`Log.infoWith`) is left for the prefix rename.
    """
    def on_code(seg: str) -> str:
        out: list[str] = []
        i = 0
        n = len(seg)
        while i < n:
            left_ok = i == 0 or not (seg[i - 1].isalnum() or seg[i - 1] == "_")
            matched = False
            if left_ok:
                for m in members:
                    tok = f"{src_alias}.{m}" if src_alias else m
                    end = i + len(tok)
                    # Right boundary: the token must not be the head of a longer
                    # identifier (`printlnRaw`) or itself qualify a further member.
                    right = seg[end] if end < n else ""
                    if seg.startswith(tok, i) and not (right.isalnum() or right in ("_", ".")):
                        out.append(f"{dst_alias}.{m}")
                        i = end
                        matched = True
                        break
            if not matched:
                out.append(seg[i])
                i += 1
        return "".join(out)

    return walk_code(text, on_code)


def apply_member_moves(text: str) -> str:
    """Move stdlib members to their new home: rewrite the import + its call sites.

    For each moved source module actually imported by the file:
      • `import <Src> exposing (m, …)` where every exposed name moved to one
        target -> `import <Target> as <Alias>`, and bare `m` -> `<Alias>.m`.
      • `import <Src> as A` used ONLY with moved members -> `import <Target> as
        <Alias>`, and `A.m` -> `<Alias>.m`. If the file also uses a NON-moved
        `A.x`, the import is left untouched (the prefix rename maps <Src> -> its
        Ipe.* module) and only the moved `A.m` sites are requalified.
    Runs BEFORE the prefix rename, while the source qualifier is still spelled
    `Std.*`. Imports live at the start of a line and never inside a string or
    comment, so the import scan is line-oriented; call-site rewrites go through
    walk_code so strings/comments stay verbatim.
    """
    for src_mod, (dst_mod, dst_alias, members) in MEMBER_MOVES.items():
        lines = text.split("\n")
        exposing_hit = False
        alias_name: str | None = None
        new_lines: list[str] = []
        for line in lines:
            stripped = line.lstrip()
            if stripped.startswith("import ") and _import_module(stripped) == src_mod:
                indent = line[: len(line) - len(stripped)]
                rest = stripped[len("import ") :]
                exposed = _exposing_names(rest)
                alias = _import_alias(rest)
                if exposed is not None and exposed and exposed <= members:
                    # Pure exposed-move: the whole import becomes the target one.
                    new_lines.append(f"{indent}import {dst_mod} as {dst_alias}")
                    exposing_hit = True
                    continue
                if alias is not None and exposed is None:
                    # `import Src as A` — decide by how A is used below.
                    alias_name = alias
            new_lines.append(line)
        text = "\n".join(new_lines)

        if exposing_hit:
            text = _rewrite_calls(text, "", dst_alias, members)

        if alias_name is not None:
            uses_moved, uses_nonmoved = _alias_member_usage(text, alias_name, members)
            if uses_moved and not uses_nonmoved:
                # The alias is used ONLY for moved members — retarget its import to
                # the new home and requalify the call sites. An import that uses a
                # non-moved member (Log.infoWith), or none at all, is left for the
                # ordinary Std. -> Ipe. prefix rename (-> Ipe.Log).
                text = _retarget_alias_import(text, src_mod, dst_mod, dst_alias)
                text = _rewrite_calls(text, alias_name, dst_alias, members)
    return text


def desugar_pure(text: str) -> str:
    """Rewrite `Sky.Core.Pure` companions to their canonical kernel form.

    `import Sky.Core.Pure as <A>` + `<A>.<member> …` become the canonical
    `<Module>.<name> …` per `PURE_DESUGAR`: the `Pure` import is replaced by the
    (deduplicated) canonical-module imports actually used, and every call site is
    requalified and renamed. Runs while the qualifier is still `Sky.Core.Pure`,
    before the prefix rename. A file that imports `Pure` under a plain (aliasless)
    `import Sky.Core.Pure exposing (…)` is handled the same way, with the exposed
    members reached bare.
    """
    src_mod = "Sky.Core.Pure"
    alias: str | None = None
    exposing: frozenset[str] | None = None
    import_indent = ""
    has_import = False
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == src_mod:
            rest = stripped[len("import ") :]
            alias = _import_alias(rest)
            exposing = _exposing_names(rest)
            import_indent = line[: len(line) - len(stripped)]
            has_import = True
            break
    if not has_import:
        return text

    # Reach members via the alias (`Pure.foo`), or bare when `exposing (...)`.
    src_alias = alias if alias is not None else ""
    used_modules: list[tuple[str, str]] = []

    def on_code(seg: str) -> str:
        out: list[str] = []
        i = 0
        n = len(seg)
        while i < n:
            left_ok = i == 0 or not (seg[i - 1].isalnum() or seg[i - 1] == "_")
            matched = False
            if left_ok:
                for member, (mod, dst_alias, name) in PURE_DESUGAR.items():
                    tok = f"{src_alias}.{member}" if src_alias else member
                    end = i + len(tok)
                    right = seg[end] if end < n else ""
                    if seg.startswith(tok, i) and not (right.isalnum() or right in ("_", ".")):
                        out.append(f"{dst_alias}.{name}")
                        if (mod, dst_alias) not in used_modules:
                            used_modules.append((mod, dst_alias))
                        i = end
                        matched = True
                        break
            if not matched:
                out.append(seg[i])
                i += 1
        return "".join(out)

    text = walk_code(text, on_code)

    # Replace the `Pure` import line with the canonical imports it now needs,
    # skipping any whose qualifier the file already provides. An import's
    # qualifier is its `as` alias, else the module path's last segment — so a
    # bare `import System` (later prefixed to `Ipe.System`) already covers the
    # `System.*` call sites and no duplicate is added.
    def qualifier_of(import_line: str) -> str:
        stripped = import_line.lstrip()
        rest = stripped[len("import ") :]
        alias_of = _import_alias(rest)
        if alias_of is not None:
            return alias_of
        return _import_module(stripped).rsplit(".", 1)[-1]

    provided = {
        qualifier_of(l) for l in text.split("\n") if l.lstrip().startswith("import ")
    }
    new_imports = [
        f"{import_indent}import {mod} as {dst_alias}"
        for (mod, dst_alias) in used_modules
        if dst_alias not in provided
    ]
    out_lines: list[str] = []
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == src_mod:
            out_lines.extend(new_imports)
        else:
            out_lines.append(line)
    return "\n".join(out_lines)


def _import_module(stripped_import: str) -> str:
    """Module path of a `import <path> …` line (stripped of leading space)."""
    rest = stripped_import[len("import ") :]
    head = rest.split(None, 1)
    return head[0] if head else ""


def _import_alias(rest: str) -> str | None:
    """The `A` in `import <path> as A …`, or None."""
    toks = rest.split()
    for k in range(len(toks) - 1):
        if toks[k] == "as":
            return toks[k + 1]
    return None


def _exposing_names(rest: str) -> frozenset[str] | None:
    """Names in `import <path> exposing (a, b)`, or None if no `exposing`.

    Returns an empty set for `exposing ()`; an `exposing (..)` wildcard yields a
    set containing ".." so it never satisfies `<= members` (a wildcard export is
    never a clean member move).
    """
    idx = rest.find("exposing")
    if idx == -1:
        return None
    open_paren = rest.find("(", idx)
    close_paren = rest.find(")", open_paren)
    if open_paren == -1 or close_paren == -1:
        return frozenset()
    inner = rest[open_paren + 1 : close_paren]
    return frozenset(tok.strip() for tok in inner.split(",") if tok.strip())


def _alias_member_usage(text: str, alias: str, members: frozenset[str]) -> tuple[bool, bool]:
    """How the file uses `alias.<name>` in CODE: (uses_moved, uses_nonmoved).

    `uses_moved` is True if some `alias.m` names a moved member; `uses_nonmoved`
    is True if some `alias.x` names anything else. A file that never references the
    alias yields (False, False) — its import is an ordinary prefix rename.
    """
    seen_moved = [False]
    seen_other = [False]

    def on_code(seg: str) -> str:
        i = 0
        n = len(seg)
        needle = alias + "."
        while True:
            j = seg.find(needle, i)
            if j == -1:
                break
            left_ok = j == 0 or not (seg[j - 1].isalnum() or seg[j - 1] == "_")
            k = j + len(needle)
            m = k
            while m < n and (seg[m].isalnum() or seg[m] == "_"):
                m += 1
            name = seg[k:m]
            if left_ok and name:
                if name in members:
                    seen_moved[0] = True
                else:
                    seen_other[0] = True
            i = j + len(needle)
        return seg

    walk_code(text, on_code)
    return seen_moved[0], seen_other[0]


def _retarget_alias_import(text: str, src_mod: str, dst_mod: str, dst_alias: str) -> str:
    """Rewrite the `import <src_mod> as A` line to `import <dst_mod> as <alias>`."""
    out_lines = []
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == src_mod \
                and _import_alias(stripped[len("import ") :]) is not None \
                and _exposing_names(stripped[len("import ") :]) is None:
            indent = line[: len(line) - len(stripped)]
            out_lines.append(f"{indent}import {dst_mod} as {dst_alias}")
        else:
            out_lines.append(line)
    return "\n".join(out_lines)


# ── Removed-module import drop (Ipe.Prelude) ──────────────────────────────────
# Ipê has no `Ipe.Prelude` module: its Tier-A (`Ipe.Basics`) surface is
# auto-imported (ADR 0047), so an open `import Ipe.Prelude exposing (..)` is
# meaningless — there is nothing to bring into scope, and the module does not
# resolve. Upstream Sky opens `Sky.Core.Prelude`, which the prefix rename turns
# into `Ipe.Prelude`; that line is then dropped entirely rather than emitted as an
# unresolvable import. Runs AFTER the prefix rename so it sees the `Ipe.` form.
_REMOVED_IMPORT_MODULES: frozenset[str] = frozenset({"Ipe.Prelude"})


def drop_removed_imports(text: str) -> str:
    """Delete `import <M> …` lines for modules Ipê no longer has (`Ipe.Prelude`).

    Imports live at the start of a line, never inside a string or comment, so the
    scan is line-oriented. Only whole-module-path matches are dropped, so a longer
    path that merely starts with the name (there is none today) is untouched.
    """
    out_lines: list[str] = []
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) in _REMOVED_IMPORT_MODULES:
            continue
        out_lines.append(line)
    return "\n".join(out_lines)


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
    paths = rest[1:]

    # Phase 1 — per-file rewrites: member moves (while the source qualifier is
    # still `Std.*`), the qualifier-prefix rename, then bare-stdlib prefixing.
    originals: dict[str, str] = {}
    transformed: dict[str, str] = {}
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        originals[path] = text
        transformed[path] = prefix_bare_imports(
            drop_removed_imports(transform(apply_member_moves(desugar_pure(text)), pairs)),
            bare,
        )

    # Phase 2 — shape-scoped Cmd/Sub re-home across the whole example. The shape
    # is proven from the entry kernel any file head-calls, so a multi-module
    # example whose `Cmd` / `Sub` import lives in a helper still re-homes onto
    # the app's shape. Skip when no shape entry is present (a plain Program, or
    # an example that never imports `Cmd` / `Sub`).
    shape = detect_shape(list(transformed.values()))
    if shape is not None:
        for path in paths:
            transformed[path] = rehome_cmd_sub(transformed[path], shape)

    for path in paths:
        if transformed[path] != originals[path]:
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(transformed[path])
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
