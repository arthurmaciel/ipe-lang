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

A DB-SURFACE-MARKING pass marks the aliased raw-SQL / stringly-row-read Db
surface: rename-map.tsv marks it only fully-qualified (`Std.Db.query` ->
`Ipe.Db.unsafeQuery`), so the common `import Std.Db as Db` + `Db.query` form is
marked here (`Db.query` -> `Db.unsafeQuery`). Scoped to the STDLIB `Std.Db`
alias per file, so a project-local `import Lib.Db as Db` keeps its own members.

A REMOVED-MODULE pass then drops import lines for modules Ipê no longer has:
Sky's open `import Sky.Core.Prelude exposing (..)` becomes `Ipe.Prelude` under
the prefix rename, but Ipê auto-imports its Tier-A `Ipe.Basics` surface (ADR
0047), so `Ipe.Prelude` does not exist and the line is deleted rather than
emitted as an unresolvable import.

Three API-MIGRATION passes then bring the renamed source onto current Ipê APIs
whose shape changed since the mirrored Sky vintage: a raw `PubSub.publish "t" x`
is wrapped in the typed `Topic` handle (`PubSub.publish (PubSub.topic "t") x`); an
entry-boundary `main = let … _ = Task.run r in ()` is rewritten to return its
`Task Error ()` (the runtime is the single `Task.run` site); and a `Maybe.*` use
left unqualified by the dropped Prelude gets an injected `import Ipe.Maybe`.

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


def _lex_spans(text: str):
    """Yield `(segment, is_code)` spans: code vs verbatim (strings/comments).

    This is the single place that knows Sky's lexical spans. A `--` line comment,
    a nested `{- -}` block comment, and `"…"` / `\"\"\"…\"\"\"` string literals are
    verbatim (`is_code=False`); everything else is code. Inside a triple-quoted
    string a `{{ expr }}` interpolation is real Ipê, so its `expr` is emitted as a
    code span (the `{{`/`}}` markers stay verbatim); a `\\{{` escaped placeholder
    is literal. Adjacent same-kind spans may be emitted separately; callers that
    need whole code runs should treat consecutive code spans as contiguous.
    """
    i = 0
    n = len(text)
    code_start = 0

    def flush_code(end: int):
        nonlocal code_start
        if end > code_start:
            yield (text[code_start:end], True)
        code_start = end

    while i < n:
        c = text[i]
        two = text[i : i + 2]
        three = text[i : i + 3]

        # `--` line comment (Sky has no custom operators, so a bare `--` outside a
        # string always starts a comment).
        if two == "--":
            yield from flush_code(i)
            j = text.find("\n", i)
            j = n if j == -1 else j
            yield (text[i:j], False)
            i = j
            code_start = i
            continue

        # `{- ... -}` block comment, nested.
        if two == "{-":
            yield from flush_code(i)
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
            yield (text[i:j], False)
            i = j
            code_start = i
            continue

        # Triple-quoted multiline string with `{{ }}` interpolation.
        if three == '"""':
            yield from flush_code(i)
            yield ('"""', False)
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
                    yield (text[seg_start:j], False)  # literal chunk
                    k = text.find("}}", j + 2)
                    k = n if k == -1 else k
                    yield ("{{", False)
                    yield (text[j + 2 : k], True)  # interpolation expr is code
                    yield ("}}", False)
                    j = k + 2
                    seg_start = j
                    continue
                j += 1
            yield (text[seg_start:j], False)  # trailing literal chunk
            if j < n:
                yield ('"""', False)
                j += 3
            i = j
            code_start = i
            continue

        # `"..."` single-line string literal with `\` escapes.
        if c == '"':
            yield from flush_code(i)
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
            yield (text[i:j], False)
            i = j
            code_start = i
            continue

        i += 1

    yield from flush_code(n)


def walk_code(text: str, code_fn) -> str:
    """Rebuild `text`, passing each CODE segment through `code_fn`.

    `code_fn(segment) -> str` sees only code — never a string literal or a
    comment, which are copied verbatim. Shares the one lexer (`_lex_spans`) so
    every code-only rewrite (the qualifier prefix rename, the stdlib member move)
    skips strings/comments identically.
    """
    return "".join(
        code_fn(seg) if is_code else seg for seg, is_code in _lex_spans(text)
    )


# A leading `"…"` single-line string literal, used to peel a call's string-literal
# first argument off the front of a verbatim span.
_LEADING_STRING = re.compile(r'\A"[^"\n]*"')


def pair_code_head_with_string(text: str, head_at_end, rewrite) -> str:
    """Rewrite a call that straddles a code head and its string-literal argument.

    `_lex_spans` places a `<A>.member "lit"` call in two adjacent spans — a CODE
    span ending in the `<A>.member` head and the following VERBATIM string literal
    — so a lone `code_fn` never sees both halves. This walks the spans and,
    whenever a code span's tail matches `head_at_end` and the next span opens with
    a `"…"` literal, calls `rewrite(before, head, literal, rest)` — where `before`
    is the code span left of the matched head, `head` is the matched head text,
    and `rest` is the string span after the literal — to produce the replacement
    for the two spans. A `<A>.member "lit"` written inside a `--`/`{- -}` comment
    or a string literal is a single verbatim span — never a code head — so such
    mentions are copied untouched. Every other span passes through verbatim.
    """
    segments = list(_lex_spans(text))
    out: list[str] = []
    i = 0
    n = len(segments)
    while i < n:
        seg, is_code = segments[i]
        head_match = head_at_end.search(seg) if is_code else None
        if head_match is not None and i + 1 < n and not segments[i + 1][1]:
            literal = _LEADING_STRING.match(segments[i + 1][0])
            if literal is not None:
                before = seg[: head_match.start()]
                head = seg[head_match.start() :]
                rest = segments[i + 1][0][literal.end() :]
                out.append(rewrite(before, head, literal.group(0), rest))
                i += 2
                continue
        out.append(seg)
        i += 1
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


# ── Stdlib Db raw-surface marking (Std.Db aliased member -> Ipe.Db.Unsafe.*) ───
# rename-map.tsv marks the raw-SQL / stringly-row-read surface only in its
# FULLY-QUALIFIED form (`Std.Db.query` -> `Ipe.Db.Unsafe.unsafeQuery`, …). The
# common aliased form — `import Std.Db as Db` then `Db.query` — is a bare member
# on an alias, which the qualifier-prefix rewrite never sees, so those call sites
# would resolve to a non-existent `Db.query` (IPE-N0005). This pass marks them.
#
# After the #679 relocation the unsafe members live in `Ipe.Db.Unsafe`, not
# `Ipe.Db`. A file that uses BOTH safe (`Db.exec`) and unsafe (`Db.query`) stdlib
# Db members needs TWO imports: the original `import Std.Db as <A>` (which the
# prefix rename turns into `import Ipe.Db as <A>`) for the safe surface, and a
# new `import Ipe.Db.Unsafe as <A>Unsafe` for the relocated members. Call sites
# are requalified: `<A>.query` → `<A>Unsafe.unsafeQuery`, etc.
#
# SECURITY (load-bearing): the rewrite is scoped to the alias bound by the
# STDLIB `import Std.Db as <Alias>` in THIS file, and to that alias only. A
# project-LOCAL Db module (`import Lib.Db as Db`, `import <Project>.Db as Db`)
# defines its OWN `query`/`getField`/… with its own contract; marking those
# `unsafe*` would mis-attribute the SQL-injection surface (a Security-principle
# regression), so a local Db alias is never touched. Runs BEFORE the prefix
# rename, while the qualifier is `Std.Db`.
#
# member -> marked member name (typed `queryDecode` + Decode.* path stays
# unmarked; only the raw/stringly surface is marked).
_DB_MODULE = "Std.Db"
_DB_UNSAFE_MODULE = "Ipe.Db.Unsafe"
_DB_UNSAFE_MEMBERS: dict[str, str] = {
    "execRaw": "unsafeExecRaw",
    "query": "unsafeQuery",
    "getString": "unsafeGetString",
    "getInt": "unsafeGetInt",
    "getBool": "unsafeGetBool",
    "getField": "unsafeGetField",
}


def _stdlib_db_alias(text: str) -> str | None:
    """The alias bound by `import Std.Db as <Alias>` in this file, or None.

    Only an `import Std.Db as A` line qualifies — never `import Lib.Db as A` or
    any other module ending in `.Db` — so the returned alias is guaranteed to
    name the STDLIB Db module. If the stdlib Db is imported without an alias, or
    with `exposing`, there is nothing to key an `<alias>.<member>` rewrite on and
    None is returned (a bare-exposed raw member would need a different handling
    and does not occur in the mirrored corpus). Imports live at line starts,
    never inside a string or comment, so the scan is line-oriented.
    """
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == _DB_MODULE:
            alias = _import_alias(stripped[len("import ") :])
            if alias is not None and _exposing_names(stripped[len("import ") :]) is None:
                return alias
    return None


def mark_stdlib_db(text: str) -> str:
    """Mark aliased stdlib-Db raw members: `<A>.query` -> `<A>Unsafe.unsafeQuery`.

    Detects `import Std.Db as <A>`, rewrites unsafe call sites to `<A>Unsafe.*`,
    and injects `import Ipe.Db.Unsafe as <A>Unsafe` after the original import
    line. The original `import Std.Db as <A>` is kept so the prefix rename turns
    it into `import Ipe.Db as <A>` for the safe surface (`exec`, `queryDecode`,
    …). Only the members in `_DB_UNSAFE_MEMBERS` are requalified — `queryDecode`
    and the typed surface stay on the `<A>` alias. A right boundary prevents
    `<A>.query` from matching inside `<A>.queryDecode`. Call-site rewrites go
    through walk_code so strings and comments stay verbatim.
    """
    alias = _stdlib_db_alias(text)
    if alias is None:
        return text
    unsafe_alias = alias + "Unsafe"
    prefix = alias + "."

    def on_code(seg: str) -> str:
        out: list[str] = []
        i = 0
        n = len(seg)
        while i < n:
            left_ok = i == 0 or not (seg[i - 1].isalnum() or seg[i - 1] == "_")
            matched = False
            if left_ok and seg.startswith(prefix, i):
                for member, marked in _DB_UNSAFE_MEMBERS.items():
                    end = i + len(prefix) + len(member)
                    right = seg[end] if end < n else ""
                    if seg.startswith(prefix + member, i) and not (
                        right.isalnum() or right in ("_", ".")
                    ):
                        out.append(unsafe_alias + "." + marked)
                        i = end
                        matched = True
                        break
            if not matched:
                out.append(seg[i])
                i += 1
        return "".join(out)

    text = walk_code(text, on_code)

    # Inject `import Ipe.Db.Unsafe as <A>Unsafe` directly after the `import
    # Std.Db as <A>` line so the prefix rename sees the original line and the
    # new import is already in place. Only inject when the alias has at least
    # one unsafe call site (walk_code above already rewrote them, so check the
    # rewritten text for the unsafe alias).
    unsafe_prefix = unsafe_alias + "."
    if unsafe_prefix in text:
        new_import = f"import {_DB_UNSAFE_MODULE} as {unsafe_alias}"
        out_lines: list[str] = []
        for line in text.split("\n"):
            out_lines.append(line)
            stripped = line.lstrip()
            if stripped.startswith("import ") and _import_module(stripped) == _DB_MODULE:
                indent = line[: len(line) - len(stripped)]
                out_lines.append(f"{indent}{new_import}")
        text = "\n".join(out_lines)

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


# ── Kernel-alias re-home (Ffi.kernel "<Mod>_<fn>" -> its published module) ─────
# A user module minting a kernel with `Ffi.kernel "<Mod>_<fn>"` is rejected by the
# capability gate (IPE-N0042): only the standard library and the generated FFI
# interface may mint a kernel, because a raw alias reaches the effect with no
# capability disclosed. The published, driver-recognised surface for these kernels
# is their kernel-qualifier module (`Ipe.Http.Middleware.withCors`, …) — a
# reference through the qualifier resolves to the SAME registered kernel the alias
# names, but as an ordinary member reference, not a user-minted alias. So this pass
# turns each point-free `<name> = Ffi.kernel "<Mod>_<fn>"` binding into
# `<name> = <Qualifier>.<fn>`, imports the qualifier module, and drops the now-dead
# `Ipe.Ffi` import when it carried only the kernel alias.
#
# HONEST DISCLOSURE: the mapping targets ONLY safe (server-tier) kernels whose
# published module is a plain qualifier — reaching them discloses the same
# capability the qualifier's own use would (e.g. `network`), never a silent
# `unsafe`. An unsafe-tier kernel has no plain qualifier here; its only sanctioned
# path stays its `Ipe.<M>.Unsafe` module (handled by `mark_stdlib_db` +
# `_needs_unsafe_capability`), so this pass never fabricates access to one.
#
# alias module prefix (before the first `_`) -> published qualifier module path.
_KERNEL_ALIAS_QUALIFIERS: dict[str, str] = {
    "Middleware": "Ipe.Http.Middleware",
}


def rehome_kernel_alias(text: str) -> str:
    """Re-home `Ffi.kernel "<Mod>_<fn>"` bindings onto their published qualifier.

    Finds the file's `import Ipe.Ffi as <A>` alias, rewrites every code-span
    `<A>.kernel "<Mod>_<fn>"` whose `<Mod>` has a published qualifier into
    `<Qualifier>.<fn>`, injects the qualifier imports, and — when the `Ipe.Ffi`
    import is left with no remaining `<A>.` reference — drops it. Runs AFTER the
    prefix rename, so the alias qualifier is already spelled `Ipe.Ffi`. A file
    without an `Ipe.Ffi` import, or whose aliases target no mapped module, is
    returned unchanged.

    The call straddles a code head (`<A>.kernel`) and its string-literal kernel
    name (`"<Mod>_<fn>"`), which `_lex_spans` places in adjacent spans, so
    `pair_code_head_with_string` pairs the two: a `<A>.kernel "…"` written inside
    a `--`/`{- -}` comment or a string literal is a single verbatim span, never a
    code head, so it is copied untouched.
    """
    ffi_alias: str | None = None
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == "Ipe.Ffi":
            ffi_alias = _import_alias(stripped[len("import ") :])
            break
    if ffi_alias is None:
        return text

    used_qualifiers: dict[str, str] = {}
    # A code span ending in `<A>.kernel` and trailing inline whitespace, anchored
    # on a non-identifier boundary so a longer alias is not a match and `\Z` at the
    # segment end so the following string-literal segment is the kernel name.
    head_at_end = re.compile(
        r"(?<![\w.])" + re.escape(ffi_alias) + r"\.kernel[ \t]+\Z"
    )
    kernel_name = re.compile(r'\A"([A-Za-z0-9]+)_([A-Za-z0-9]+)"\Z')

    def _rewrite(before: str, head: str, literal: str, rest: str) -> str:
        name = kernel_name.match(literal)
        if name is None:
            return before + head + literal + rest  # Not `<Mod>_<fn>` — leave as is.
        mod, fn = name.group(1), name.group(2)
        qual_mod = _KERNEL_ALIAS_QUALIFIERS.get(mod)
        if qual_mod is None:
            return before + head + literal + rest  # Unmapped module — leave alias.
        qual_alias = qual_mod.rsplit(".", 1)[-1]
        used_qualifiers[qual_mod] = qual_alias
        # `head` (`<A>.kernel` + separating whitespace) and the kernel-name literal
        # both fold into the point-free `<Qualifier>.<fn>` — the requalified form
        # takes no argument. `before` (the code left of the head) and `rest` (any
        # span text after the literal) are preserved.
        return f"{before}{qual_alias}.{fn}{rest}"

    new_text = pair_code_head_with_string(text, head_at_end, _rewrite)
    if not used_qualifiers:
        return text  # No mapped alias in this file — leave it untouched.

    # Inject the qualifier imports and drop the dead `Ipe.Ffi` import. The Ffi
    # import is dropped only when no `<A>.` reference survives (a file that also
    # used `Ffi` for something else keeps it). Both edits are line-oriented — an
    # import is always at a line start, never inside a string or comment.
    ffi_still_used = _alias_is_referenced(new_text, ffi_alias)
    qualifier_imports = [
        f"import {mod} as {alias}" for mod, alias in used_qualifiers.items()
    ]
    out_lines: list[str] = []
    injected = False
    for line in new_text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == "Ipe.Ffi":
            indent = line[: len(line) - len(stripped)]
            if not injected:
                out_lines.extend(f"{indent}{imp}" for imp in qualifier_imports)
                injected = True
            if ffi_still_used:
                out_lines.append(line)  # keep — still referenced elsewhere.
            continue  # drop the now-dead Ffi import.
        out_lines.append(line)
    return "\n".join(out_lines)


def _alias_is_referenced(text: str, alias: str) -> bool:
    """True if a code span references `<alias>.` (an import-alias member access)."""
    needle = alias + "."
    found = [False]

    def on_code(seg: str) -> str:
        i = 0
        while True:
            j = seg.find(needle, i)
            if j == -1:
                break
            left_ok = j == 0 or not (seg[j - 1].isalnum() or seg[j - 1] == "_")
            if left_ok:
                found[0] = True
                break
            i = j + len(needle)
        return seg

    walk_code(text, on_code)
    return found[0]


# ── PubSub raw-topic wrap (`PubSub.publish "t" x` -> typed `Topic` handle) ─────
# `Ipe.PubSub.publish` / `publishNoEcho` take a typed `Topic a` first argument
# (`publish : Topic a -> a -> Task Error Int`), constructed by `PubSub.topic`.
# Upstream Sky's `PubSub.publish` took a raw `String` topic, so a mirrored call
# reads `<A>.publish "todos.created" payload` and fails to type-check against the
# `Topic a` parameter. This pass wraps the bare string-literal topic in a
# `<A>.topic "…"` call: `<A>.publish "t" x` -> `<A>.publish (<A>.topic "t") x`.
#
# The call spans a code token (`<A>.publish`) AND its string-literal topic
# argument, so — like `rehome_kernel_alias` — it is matched on the whole line
# rather than inside a lone code span. `<A>` is the alias bound by
# `import Ipe.PubSub as <A>` in this file; a file without that import is left
# untouched. A call already passing a parenthesised handle (`<A>.topic …` or any
# expression) is not a bare string literal, so it never matches — the pass is
# idempotent and only rewrites the raw-String form.
_PUBSUB_MODULE = "Ipe.PubSub"
_PUBSUB_TOPIC_MEMBERS: frozenset[str] = frozenset({"publish", "publishNoEcho"})


def wrap_pubsub_topic(text: str) -> str:
    """Wrap a raw string topic: `<A>.publish "t" x` -> `<A>.publish (<A>.topic "t") x`.

    Finds the `import Ipe.PubSub as <A>` alias, then rewrites every CODE
    `<A>.publish "…"` / `<A>.publishNoEcho "…"` whose first argument is a bare
    string literal into the typed-`Topic` form via `<A>.topic`. Runs AFTER the
    prefix rename, so the alias qualifier is already `Ipe.PubSub`. A file without
    the import, or whose publish already passes a non-literal handle, is
    unchanged.

    The call straddles a code token (`<A>.publish`) and its string-literal topic
    argument, which `_lex_spans` places in adjacent spans, so
    `pair_code_head_with_string` pairs the two. A `PubSub.publish "…"` inside a
    `--`/`{- -}` comment or a string literal is a single verbatim span, never a
    code head, so comments and strings survive untouched.
    """
    pubsub_alias: str | None = None
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == _PUBSUB_MODULE:
            pubsub_alias = _import_alias(stripped[len("import ") :])
            break
    if pubsub_alias is None:
        return text

    members = "|".join(re.escape(m) for m in sorted(_PUBSUB_TOPIC_MEMBERS))
    # A code span ending in `<A>.<member>` and trailing inline whitespace. The head
    # is anchored on a non-identifier boundary so `MyPubSub.publish` (a different
    # alias) is not a match, and `\Z` anchors it at the span end so the following
    # string-literal span is its first argument.
    head_at_end = re.compile(
        r"(?<![\w.])" + re.escape(pubsub_alias) + r"\.(?:" + members + r")[ \t]+\Z"
    )

    def _rewrite(before: str, head: str, topic: str, rest: str) -> str:
        # `head` ends with the whitespace that separated the head from the topic
        # string, so it already provides the space before `(`; `rest` carries the
        # original whitespace before the payload — add neither. `before` (the code
        # left of the head) is preserved.
        return f"{before}{head}({pubsub_alias}.topic {topic}){rest}"

    return pair_code_head_with_string(text, head_at_end, _rewrite)


# ── Discard-binding → do-notation (`let _ = e … in body` → `do` block) ──────────
# Wave-3 parse gate will reject whole-pattern `let _ = e` on the user surface.
# Any `let` block that contains at least one `_ = expr` binding is rewritten to
# an equivalent `do` block:
#
#     let               →   do
#         x = pure          x = pure        (pure let-binding: unchanged)
#         _ = effect        effect          (discard: bare expression)
#     in                    body            (continuation: final line)
#         body
#
# Evaluation order is identical — `do` desugars to the same flat TaskSeq chain.
# Pure-only `let` blocks (no `_ =` binding at all) are left untouched.
#
# The scanner works line-by-line to respect Ipê's indentation-sensitive layout.
# It is careful not to rewrite inside string literals or comments: it skips lines
# that are part of a triple-quoted `"""…"""` span. Single-line `"…"` and `--`
# comments within a single line are left as-is (they don't affect block structure).

# Matches `_ = rest` (inline) OR `_ =` alone at end of line (multi-line value).
_DISCARD_BINDING = re.compile(r'^(?P<indent>[ \t]*)_[ \t]*=[ \t]*(?P<rest>.*)$')
# A whole-pattern discard whose RHS is a bare local identifier or a literal is a
# dead PURE-VALUE marker (`_ = list`), not an effect to run: dropping it is
# behaviour-preserving, whereas runifying it as a `do` statement type-errors (a
# `do` statement must be a `Task`). Effect discards (`_ = Io.println …`) are
# function applications — they contain a space — and are runified normally.
_PURE_VALUE_DISCARD = re.compile(r'^([a-z_][A-Za-z0-9_\']*|\[.*\]|\{.*\}|-?\d.*|".*")$')
_LET_LINE = re.compile(r'^(?P<indent>[ \t]*)let[ \t]*$')
_IN_LINE = re.compile(r'^(?P<indent>[ \t]*)in[ \t]*$')
_BINDING_LINE = re.compile(r'^(?P<indent>[ \t]*)(?P<name>[A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*(?P<rest>.*)$')


def _in_triple_quote_ranges(lines: list[str]) -> list[bool]:
    """Return a parallel bool list: True when line i is inside a triple-quoted string."""
    result = [False] * len(lines)
    inside = False
    for i, line in enumerate(lines):
        j = 0
        while j < len(line):
            if line[j:j+3] == '"""':
                inside = not inside
                j += 3
            else:
                j += 1
        if inside:
            result[i] = True
    return result


def rewrite_discard_bindings(text: str) -> str:
    """Rewrite `let` blocks with `_ = e` bindings to `do` blocks (fixed point).

    Runs the single-pass rewriter to a fixed point so that nested `let` blocks
    inside continuation lines (e.g., a discard whose value is a `case` expression
    containing a further `let _ = e`) are also rewritten. Terminates because each
    pass strictly reduces the number of `let _ =` blocks.
    """
    while True:
        next_text = _rewrite_discard_bindings_once(text)
        if next_text == text:
            return text
        text = next_text


def _rewrite_discard_bindings_once(text: str) -> str:
    """Single-pass rewrite of `let` blocks with `_ = e` to `do` blocks."""
    lines = text.split("\n")
    in_tq = _in_triple_quote_ranges(lines)
    out: list[str] = []
    i = 0
    n = len(lines)

    while i < n:
        line = lines[i]

        # Skip lines inside triple-quoted strings untouched.
        if in_tq[i]:
            out.append(line)
            i += 1
            continue

        let_m = _LET_LINE.match(line)
        if let_m is None or in_tq[i]:
            out.append(line)
            i += 1
            continue

        let_indent = let_m.group("indent")
        binding_indent = let_indent + "    "

        # Collect binding lines at binding_indent, then an `in` line at let_indent.
        # A binding may span multiple lines: continuation lines are indented deeper
        # than binding_indent. We collect all lines up to and including `in`.
        j = i + 1
        # Gather the binding block: lines until we see `in` at let_indent.
        binding_lines: list[str] = []
        found_in = False
        in_line_idx = -1
        while j < n:
            if in_tq[j]:
                binding_lines.append(lines[j])
                j += 1
                continue
            in_m = _IN_LINE.match(lines[j])
            if in_m is not None and in_m.group("indent") == let_indent:
                found_in = True
                in_line_idx = j
                break
            binding_lines.append(lines[j])
            j += 1

        if not found_in:
            # No matching `in` at same indent — leave the `let` untouched.
            out.append(line)
            i += 1
            continue

        # Check whether any binding is a whole-pattern discard `_ = e`.
        has_discard = any(
            _DISCARD_BINDING.match(bl) is not None
            and _DISCARD_BINDING.match(bl).group("indent") == binding_indent
            and not in_tq[i + 1 + k]
            for k, bl in enumerate(binding_lines)
        )
        if not has_discard:
            out.append(line)
            i += 1
            continue

        # Rewrite: `let` → `do`, discard bindings → bare exprs, `in` dropped.
        out.append(let_indent + "do")

        # Process binding_lines: group them into top-level bindings + their
        # continuation lines. A top-level binding starts at exactly binding_indent;
        # continuation lines are indented further.
        k = 0
        nb = len(binding_lines)
        while k < nb:
            bl = binding_lines[k]
            # Triple-quoted interior: copy verbatim.
            line_abs = i + 1 + k
            if line_abs < len(in_tq) and in_tq[line_abs]:
                out.append(bl)
                k += 1
                continue
            # Blank or empty line inside binding block: preserve.
            if bl.strip() == "":
                out.append(bl)
                k += 1
                continue
            # Collect continuation lines (deeper indent or blank).
            conts: list[str] = []
            m2 = k + 1
            while m2 < nb:
                nbl = binding_lines[m2]
                if nbl.strip() == "":
                    conts.append(nbl)
                    m2 += 1
                    continue
                nbl_indent = len(nbl) - len(nbl.lstrip())
                bind_len = len(binding_indent)
                if nbl_indent > bind_len:
                    conts.append(nbl)
                    m2 += 1
                else:
                    break

            # Is this a discard binding at the right indent?
            disc_m = _DISCARD_BINDING.match(bl)
            if disc_m is not None and disc_m.group("indent") == binding_indent:
                # Bare expression: strip the `_ = ` prefix.
                bare_rest = disc_m.group("rest")
                if bare_rest and _PURE_VALUE_DISCARD.match(bare_rest.strip()):
                    # Dead pure-value discard (`_ = list`): drop it — do NOT
                    # runify a non-Task value into a `do` statement.
                    out.extend(conts)
                elif bare_rest:
                    # Inline: `_ = expr` → `expr` on the same line.
                    out.append(binding_indent + bare_rest)
                    out.extend(conts)
                else:
                    # Multi-line: `_ =` alone; value is on continuation lines.
                    # The continuation lines were indented relative to `_ =`
                    # (deeper than binding_indent). Re-anchor them so the first
                    # non-blank continuation starts at binding_indent.
                    non_blank = [c for c in conts if c.strip()]
                    if non_blank:
                        first_ws = len(non_blank[0]) - len(non_blank[0].lstrip())
                        shift = first_ws - len(binding_indent)
                        for c in conts:
                            if c.strip():
                                out.append(c[shift:] if shift > 0 else c)
                            else:
                                out.append(c)
                    else:
                        out.extend(conts)
            else:
                # Pure let-binding or anything else: preserve as-is.
                out.append(bl)
                out.extend(conts)
            k = m2

        # The `in` line is consumed; the body lines (after `in`) become the final
        # lines of the `do` block — just append them at the same indentation they
        # already have (they were already indented relative to `let`).
        # Skip the `in` line itself (in_line_idx), carry on from j+1.
        i = in_line_idx + 1
        # Append all remaining body lines (the `in` body may itself be multiple
        # lines at deeper indent — they follow immediately after `in`).
        # We do NOT consume them here: the outer loop will process them normally,
        # which handles nested `let` blocks correctly.
        continue

    return "\n".join(out)


# ── Entry-boundary Task.run strip (`main = let … _ = Task.run r in ()`) ────────
# Ipê's runtime is the single `Task.run` site: an idiomatic entry point RETURNS
# its `Task Error ()` and lets the runtime run it (IPE-N0036). Upstream Sky runs
# the task itself at `main`, spelled:
#
#     main =
#         let
#             run = <entry expr>
#             _ = Task.run run
#         in
#             ()
#
# This pass rewrites that exact entry idiom to `main = <entry expr>`, returning
# the task. It is SCOPED to the `main` binding's own `let` whose body is `()` and
# whose sole side-effecting binding is `_ = <TaskAlias>.run <var>` — it never
# touches a `Task.run` in expression position (the synchronous-bridge use inside
# a helper, e.g. `Result.withDefault "" (Task.run (System.getenv …))`), which Ipê
# still supports. Runs AFTER the prefix rename, so the alias is already `Task`.
_ENTRY_TASK_RUN = re.compile(
    r"^main[ \t]*=\n"
    r"[ \t]*let\n"
    r"[ \t]*(?P<runvar>[A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*(?P<expr>[^\n]+)\n"
    r"[ \t]*_[ \t]*=[ \t]*(?P<taskalias>[A-Za-z_][A-Za-z0-9_.]*)\.run[ \t]+(?P=runvar)[ \t]*\n"
    r"[ \t]*in\n"
    r"[ \t]*\(\)[ \t]*$",
    flags=re.MULTILINE,
)


def return_entry_task(text: str) -> str:
    """Rewrite the entry `main = let run = e … _ = Task.run run in ()` to `main = e`.

    Matches only the whole entry idiom — a top-level `main =` whose `let` binds
    `run = <expr>`, discards `_ = <alias>.run run`, and returns `()` — and
    replaces it with the idiomatic entry point:

        main : Task Error ()
        main =
            <expr>

    so the runtime becomes the single `Task.run` site and `main` returns its
    `Task Error ()`. The signature is added because a Sky `main` is unsignatured;
    a `Task.run` in any other position is untouched. The `<expr>` is a single-line
    RHS in the mirrored corpus (the `entry () |> Task.onError …` pipe). Runs after
    the prefix rename so `<alias>` is already `Task`.
    """

    def _repl(m: "re.Match[str]") -> str:
        return f"main : Task Error ()\nmain =\n    {m.group('expr').rstrip()}"

    return _ENTRY_TASK_RUN.sub(_repl, text)


# ── Entry effect-discard hoist (`main = let … _ = <effect> in ()` → effect) ────
# Upstream Sky eagerly runs a discarded effect at the entry point and returns
# unit, spelled as a `let` whose final binding discards a side-effecting call and
# whose body is `()`:
#
#     main =
#         let
#             x = pure
#             _ = <effect>
#         in
#             ()
#
# In Ipê a `main` that returns `()` never runs the effect — the runtime runs the
# `Task` that `main` RETURNS. The idiomatic form hoists the effect out of the
# discard and makes it the returned body, preserving any earlier pure bindings:
#
#     main =
#         let
#             x = pure
#         in
#         <effect>
#
# When the discard is the only binding, the `let` collapses to `main = <effect>`.
# This pass is SCOPED to the top-level `main` binding whose `let` ends in a
# `_ = <effect>` discard immediately before an `in ()` body; the effect must be a
# function application (it contains inline whitespace after the head), so a dead
# pure-value discard (`_ = someValue`) is left for the discard-dropping pass. A
# non-`main` `let … _ = e in ()` is untouched. Runs BEFORE the discard→do rewrite
# so the entry idiom is turned into an effect-valued body rather than a `do` block
# that silently drops its trailing effect.
_ENTRY_MAIN_HEAD = re.compile(r"^main[ \t]*=[ \t]*$")
_ENTRY_LET_HEAD = re.compile(r"^(?P<indent>[ \t]+)let[ \t]*$")
_ENTRY_IN_UNIT = re.compile(r"^(?P<indent>[ \t]*)in[ \t]*$")
_ENTRY_UNIT_BODY = re.compile(r"^[ \t]*\(\)[ \t]*$")
# An effect discard: `_ = <head> <arg…>` — the RHS is a function application, so it
# carries inline whitespace after a non-space head. A bare value (`_ = xs`) does
# not match and is left to the discard-dropping pass.
_ENTRY_EFFECT_DISCARD = re.compile(
    r"^(?P<indent>[ \t]+)_[ \t]*=[ \t]*(?P<effect>\S+[ \t]+\S.*)$"
)
# A `_ = <alias>.run <arg>` discard is the synchronous entry-boundary bridge, not
# a plain effect to hoist — it is the domain of `return_entry_task` and the
# content-anchored ipe-edits, which turn it into a Task-returning `main`. Leaving
# it for them keeps `hoist_entry_effect` from disturbing those anchors.
_ENTRY_TASK_RUN_DISCARD = re.compile(r"^[A-Za-z_][A-Za-z0-9_.]*\.run[ \t]")


def hoist_entry_effect(text: str) -> str:
    """Rewrite `main = let … _ = <effect> in ()` to return the effect as its body.

    The discarded trailing effect becomes the `let` body (or the whole `main` RHS
    when it was the sole binding), so the entry point returns the `Task` the
    runtime then runs, instead of a `do`/`let` body of `()` that drops it. Only the
    top-level `main` binding's own `let` is rewritten, and only when its final
    binding is an effect discard directly before an `in ()`.
    """
    lines = text.split("\n")
    in_tq = _in_triple_quote_ranges(lines)
    n = len(lines)
    i = 0
    while i < n:
        if in_tq[i] or _ENTRY_MAIN_HEAD.match(lines[i]) is None:
            i += 1
            continue
        # `main =` followed by an indented `let`.
        if i + 1 >= n or in_tq[i + 1]:
            i += 1
            continue
        let_m = _ENTRY_LET_HEAD.match(lines[i + 1])
        if let_m is None:
            i += 1
            continue
        let_indent = let_m.group("indent")
        binding_indent = let_indent + "    "

        # Collect the binding lines up to the `in` at the `let`'s own indent.
        j = i + 2
        binding_lines: list[str] = []
        in_idx = -1
        while j < n and not in_tq[j]:
            in_m = _ENTRY_IN_UNIT.match(lines[j])
            if in_m is not None and in_m.group("indent") == let_indent:
                in_idx = j
                break
            binding_lines.append(lines[j])
            j += 1
        if in_idx == -1:
            i += 1
            continue
        # Body must be exactly `()`.
        if in_idx + 1 >= n or in_tq[in_idx + 1] or _ENTRY_UNIT_BODY.match(lines[in_idx + 1]) is None:
            i += 1
            continue
        # The last non-blank binding must be an effect discard at binding_indent.
        last = len(binding_lines) - 1
        while last >= 0 and binding_lines[last].strip() == "":
            last -= 1
        if last < 0:
            i += 1
            continue
        disc_m = _ENTRY_EFFECT_DISCARD.match(binding_lines[last])
        if disc_m is None or disc_m.group("indent") != binding_indent:
            i += 1
            continue
        effect = disc_m.group("effect").rstrip()
        # A `Task.run`-style bridge discard is left for return_entry_task / edits.
        if _ENTRY_TASK_RUN_DISCARD.match(effect) is not None:
            i += 1
            continue
        kept = binding_lines[:last]  # bindings before the discard
        kept_nonblank = [b for b in kept if b.strip() != ""]

        rebuilt: list[str] = [lines[i]]  # `main =`
        if kept_nonblank:
            rebuilt.append(lines[i + 1])  # `let`
            rebuilt.extend(kept)
            rebuilt.append(f"{let_indent}in")
            rebuilt.append(f"{let_indent}{effect}")
        else:
            # Sole binding was the effect: collapse to `main = <effect>`, the
            # effect re-anchored to the `let`'s own (body) indent.
            rebuilt.append(f"{let_indent}{effect}")
        # Splice: replace lines[i .. in_idx+1] with the rebuilt block.
        lines = lines[:i] + rebuilt + lines[in_idx + 2 :]
        n = len(lines)
        in_tq = _in_triple_quote_ranges(lines)
        i += len(rebuilt)
    return "\n".join(lines)


# ── Upstream issue-reference comment strip (`sky#NNN` annotations) ─────────────
# Some upstream files carry a leading `--` comment block that annotates the file
# against an upstream tracker issue (`sky#NNN` / `anzellai/sky#NNN`), e.g. a
# regression note. That reference is meaningless in the port, so the whole
# contiguous `--` comment block introduced by such a reference is dropped. A
# comment block whose first line carries no issue reference is kept verbatim.
_ISSUE_REF = re.compile(r"(?:[A-Za-z0-9_.-]+/)?sky#\d+")
_LINE_COMMENT = re.compile(r"^[ \t]*--")


def strip_issue_ref_comments(text: str) -> str:
    """Drop contiguous leading `--` comment blocks that cite an upstream issue.

    A run of consecutive `--` comment lines whose FIRST line contains an
    `(<owner>/)?sky#<n>` reference is removed in full; the reference introduces an
    upstream-tracker annotation that does not belong in the port. Comment blocks
    with no such reference on their opening line are left untouched, as are `--`
    comments that trail code (only whole-line comment blocks are considered).
    """
    lines = text.split("\n")
    in_tq = _in_triple_quote_ranges(lines)
    out: list[str] = []
    i = 0
    n = len(lines)
    while i < n:
        if (
            not in_tq[i]
            and _LINE_COMMENT.match(lines[i]) is not None
            and _ISSUE_REF.search(lines[i]) is not None
        ):
            # Drop this comment line and any immediately-following comment lines.
            i += 1
            while i < n and not in_tq[i] and _LINE_COMMENT.match(lines[i]) is not None:
                i += 1
            continue
        out.append(lines[i])
        i += 1
    return "\n".join(out)


# ── Maybe-import injection (Prelude drop leaves `Maybe.*` unqualified) ─────────
# Sky's open `Sky.Core.Prelude exposing (..)` re-exported `Ipe.Maybe`'s surface,
# so upstream files reach `Maybe.withDefault` through the Prelude without a direct
# `import Sky.Core.Maybe`. Ipê auto-imports only its Tier-A `Ipe.Basics` (ADR
# 0047), which does NOT include `Maybe.*`; the Prelude import is dropped, so a
# file that used `Maybe.<member>` via the Prelude now references an unimported
# qualifier (IPE-N0034). This pass injects `import Ipe.Maybe as Maybe` when a file
# uses `Maybe.` in code yet imports no `Ipe.Maybe`. A file that already imports it
# (directly in upstream) is untouched. Runs AFTER the prefix rename and the
# Prelude drop, so the missing import is observable.
_MAYBE_MODULE = "Ipe.Maybe"


def inject_maybe_import(text: str) -> str:
    """Add `import Ipe.Maybe as Maybe` when `Maybe.*` is used with no such import.

    Injects the import only when the file references `Maybe.` in a code span and
    has no `import Ipe.Maybe` line. The new import is placed after the last
    existing `import` line so it joins the import block. A file that already
    imports `Ipe.Maybe`, or never references `Maybe.` in code, is unchanged.
    """
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith("import ") and _import_module(stripped) == _MAYBE_MODULE:
            return text  # already imported

    if not _alias_is_referenced(text, "Maybe"):
        return text  # no qualified `Maybe.` use to satisfy

    lines = text.split("\n")
    last_import = -1
    for idx, line in enumerate(lines):
        if line.lstrip().startswith("import "):
            last_import = idx
    if last_import == -1:
        return text  # no import block to extend; leave it to the compiler

    indent = lines[last_import][: len(lines[last_import]) - len(lines[last_import].lstrip())]
    lines.insert(last_import + 1, f"{indent}import {_MAYBE_MODULE} as Maybe")
    return "\n".join(lines)


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


# ── Unsafe modules whose import discloses the `unsafe` capability ─────────────
# Any Ipê source file that imports one of these modules causes the whole program
# to require `unsafe` in its manifest `[capabilities] declared` list. The set is
# checked AFTER the full per-file transform (when the imports are already in Ipê
# form), so the strings here use the post-rename Ipê module paths.
_UNSAFE_IMPORT_MODULES: frozenset[str] = frozenset({
    "Ipe.Db.Unsafe",
    "Ipe.Html.Unsafe",
    "Ipe.Web.Head.Unsafe",
    "Ipe.Secret.Unsafe",
})


def _needs_unsafe_capability(texts: list[str]) -> bool:
    """True if any transformed source file imports an unsafe-disclosing module."""
    for text in texts:
        for line in text.split("\n"):
            stripped = line.lstrip()
            if stripped.startswith("import ") and _import_module(stripped) in _UNSAFE_IMPORT_MODULES:
                return True
    return False


def _inject_unsafe_capability(manifest_path: str) -> None:
    """Add `unsafe` to the `[capabilities] declared` list in ipe.toml if absent.

    Reads the manifest, finds or creates the `[capabilities]` section, and
    ensures `"unsafe"` is in the `declared` list. A manifest that already
    declares `unsafe` is left byte-identical. Non-fatal: a missing or
    unparseable manifest is silently skipped (the capability gate in `ipe check`
    will surface the omission at build time).
    """
    import os
    import re as _re
    if not os.path.isfile(manifest_path):
        return
    try:
        with open(manifest_path, encoding="utf-8") as fh:
            content = fh.read()
    except OSError:
        return

    # Already declares unsafe — nothing to do.
    if '"unsafe"' in content or "'unsafe'" in content:
        return

    # Find an existing `declared = [...]` line inside a `[capabilities]` section.
    # We support single-line arrays only (the format used by the transform output
    # and the fixture corpus). A multi-line array is left untouched; `ipe check`
    # will surface the missing capability at build time.
    cap_section = _re.search(r'^\[capabilities\]', content, flags=_re.MULTILINE)
    declared_line = _re.search(
        r'^(\s*declared\s*=\s*\[)([^\]]*?)(\])',
        content,
        flags=_re.MULTILINE,
    )

    if declared_line is not None:
        # Append `"unsafe"` to the existing declared list.
        before = content[: declared_line.start(2)]
        inner = declared_line.group(2).strip()
        after = content[declared_line.end(2) :]
        sep = ", " if inner else ""
        new_inner = f'{inner}{sep}"unsafe"'
        content = before + new_inner + after
    elif cap_section is not None:
        # `[capabilities]` section exists but no `declared` line — add one.
        insert_at = cap_section.end()
        # Skip to end of the section header line.
        nl = content.find("\n", insert_at)
        if nl == -1:
            nl = len(content)
        content = content[: nl + 1] + 'declared = ["unsafe"]\n' + content[nl + 1 :]
    else:
        # No `[capabilities]` section at all — append one.
        if not content.endswith("\n"):
            content += "\n"
        content += '\n[capabilities]\ndeclared = ["unsafe"]\n'

    try:
        with open(manifest_path, "w", encoding="utf-8") as fh:
            fh.write(content)
    except OSError:
        pass


def main(argv: list[str]) -> int:
    args = argv[1:]
    bare: frozenset[str] = frozenset()
    manifest: str | None = None
    rewrite_discards_only: bool = False
    rest: list[str] = []
    i = 0
    while i < len(args):
        if args[i] == "--bare-stdlib" and i + 1 < len(args):
            bare = frozenset(n for n in args[i + 1].split(",") if n)
            i += 2
            continue
        if args[i] == "--manifest" and i + 1 < len(args):
            manifest = args[i + 1]
            i += 2
            continue
        if args[i] == "--rewrite-discards-only":
            rewrite_discards_only = True
            i += 1
            continue
        rest.append(args[i])
        i += 1
    if rewrite_discards_only:
        # Post-edits pass: apply only rewrite_discard_bindings to the given files.
        # No rename-map argument required; files are given directly.
        paths = rest
        if not paths:
            sys.stderr.write(
                "usage: sky-to-ipe-transform.py --rewrite-discards-only <file> [<file> ...]\n"
            )
            return 2
        for path in paths:
            with open(path, encoding="utf-8") as fh:
                original = fh.read()
            rewritten = rewrite_discard_bindings(original)
            if rewritten != original:
                with open(path, "w", encoding="utf-8") as fh:
                    fh.write(rewritten)
        return 0
    if len(rest) < 2:
        sys.stderr.write(
            "usage: sky-to-ipe-transform.py [--bare-stdlib N1,N2] "
            "[--manifest ipe.toml] <rename-map.tsv> <file> [<file> ...]\n"
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
        # After the prefix rename + Prelude drop, three API migrations run on the
        # already-`Ipe.` form: wrap a raw PubSub topic string in a typed handle,
        # strip an entry-boundary `Task.run` so `main` returns its task, and
        # inject a missing `Ipe.Maybe` import left unqualified by the Prelude drop.
        # NOTE: rewrite_discard_bindings is intentionally NOT applied here. It
        # runs after Step 2 (ipe-edits) via mirror.sh's Step 3 / --rewrite-discards-only
        # so that edit `find` anchors can match against the un-converted text they
        # were authored against before the `let _ = e` → `do` rewrite changes it.
        renamed = drop_removed_imports(
            transform(
                mark_stdlib_db(
                    apply_member_moves(desugar_pure(strip_issue_ref_comments(text)))
                ),
                pairs,
            )
        )
        migrated = inject_maybe_import(
            hoist_entry_effect(
                return_entry_task(wrap_pubsub_topic(rehome_kernel_alias(renamed)))
            )
        )
        transformed[path] = prefix_bare_imports(migrated, bare)

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

    # Phase 3 — manifest unsafe-capability injection. When any source file now
    # imports an unsafe-disclosing module, add `"unsafe"` to the manifest's
    # `[capabilities] declared` list so `ipe check` can gate on it.
    if manifest is not None and _needs_unsafe_capability(list(transformed.values())):
        _inject_unsafe_capability(manifest)

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
