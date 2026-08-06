#!/usr/bin/env python3
"""Translate a Sky manifest's `["go.dependencies"]` into Ipê `[rust.dependencies]`.

The upstream Sky manifest names Go packages the program binds through Sky's
Go-FFI. Ipê binds crates.io crates instead, declared under `[rust.dependencies]`
and materialised by `ipe install`. This pass reads the Go→Rust map
(tools/scripts/lib/go-to-rust-crates.tsv) and rewrites an already-`entry`-renamed
ipe.toml in place: the `["go.dependencies"]` table is removed and replaced by a
`[rust.dependencies]` block holding the mapped crates.

Three outcomes per Go package (governed by the map's confidence column):
  • mapped  — a reviewed crates.io equivalent is emitted as a live dependency.
  • stdlib  — the capability lives in Ipê's own stdlib / Rust std; NO crate line
              is emitted (the `-` crate sentinel). A short comment records it.
  • unsure  — a plausible but UNVERIFIED crate. Emitted COMMENTED OUT with an
              `# UNSURE` marker so a human confirms it — never asserted silently
              (PRINCIPLES.md §Security fail-closed, §0 honest-not-silent).
An unknown Go package (absent from the map) is treated as `unsure` with an
`# UNMAPPED` marker, again commented out — the converter never invents a crate.

This is INTENTIONALLY a table lookup + manifest rewrite, NOT the deep FFI binding
generation. The actual crate→Ipê-value binding is the `ipe install` machinery
(src/ipe-cli/src/ffi.rs); emitting the `[rust.dependencies]` block is the input
that machinery consumes. A project whose Go deps are all `mapped` becomes a
buildable Ipê FFI project once `ipe install` binds the listed crates.

Usage: go-deps-to-rust.py <go-to-rust-crates.tsv> <ipe.toml>
Rewrites <ipe.toml> in place. Exit 0 on success (0 also when no go.dependencies).
"""
from __future__ import annotations

import re
import sys


def load_crate_map(path: str) -> dict[str, tuple[str, str, str]]:
    """Parse the TSV into {go-import-path: (rust-crate, confidence, note)}."""
    out: dict[str, tuple[str, str, str]] = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 3:
                continue
            go_path = parts[0].strip()
            crate = parts[1].strip()
            confidence = parts[2].strip()
            note = parts[3].strip() if len(parts) > 3 else ""
            if go_path:
                out[go_path] = (crate, confidence, note)
    return out


def _extract_go_block(text: str) -> tuple[str, list[str]] | None:
    """Find the `["go.dependencies"]` / `[go.dependencies]` table.

    Returns (whole-span-including-header, [go-import-path, …]) or None. The span
    runs from the header line to the next top-level `[` table header or EOF, so
    removing it excises the whole section. Keys are read from `"key" = …` /
    `key = …` lines within the span (both TOML quoted and bare key forms).

    Assumes one `key = <single-line value>` per line — the shape Sky manifests
    always use for `["go.dependencies"]` (each is `"pkg" = "latest"`). A key
    misread from an unexpected multi-line value fails CLOSED: it maps to no crate
    and surfaces as a `# UNMAPPED` comment, never a silent live dependency.
    """
    header = re.compile(r'^[ \t]*\[["\']?go\.dependencies["\']?\][ \t]*$', re.MULTILINE)
    m = header.search(text)
    if m is None:
        return None
    start = m.start()
    # End = next line that starts a new table header, or EOF.
    tail = text[m.end():]
    nxt = re.search(r'^[ \t]*\[', tail, re.MULTILINE)
    end = m.end() + nxt.start() if nxt else len(text)
    span = text[start:end]
    keys: list[str] = []
    for line in span.splitlines()[1:]:
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        km = re.match(r'"?([^"=]+?)"?\s*=', s)
        if km:
            keys.append(km.group(1).strip())
    return span, keys


def render_rust_deps(keys: list[str], crate_map: dict[str, tuple[str, str, str]]) -> str:
    """Build the `[rust.dependencies]` block text for the mapped Go packages.

    Deduplicates on the emitted crate NAME (the token before ` =`) so two Go
    submodules of one Rust crate (firebase v4 + v4/auth -> rs-firebase-admin-sdk)
    produce a single dependency line. `stdlib` packages become a single trailing
    comment; `unsure` / unmapped crates are emitted commented out.
    """
    live: list[str] = []           # emitted dependency lines, deduped by crate name
    live_seen: set[str] = set()
    commented: list[str] = []      # `# UNSURE`/`# UNMAPPED` lines, human to confirm
    stdlib_pkgs: list[str] = []    # go paths handled by Ipê stdlib (no crate)

    for go_path in keys:
        entry = crate_map.get(go_path)
        if entry is None:
            commented.append(f'# UNMAPPED: no reviewed Rust crate for Go "{go_path}"')
            continue
        crate, confidence, note = entry
        if confidence == "stdlib":
            stdlib_pkgs.append(go_path)
            continue
        if confidence == "mapped":
            # A reviewed live dependency. Dedup by crate name (token before " =").
            name = crate.split("=", 1)[0].strip()
            if name in live_seen:
                continue
            live_seen.add(name)
            live.append(crate)
            continue
        # Fail closed (PRINCIPLES.md §Security): `unsure` — and ANY unrecognized
        # confidence (a typo like `maped`, an empty/mixed-case token) — never emits
        # a live crate. It is commented out for a human to confirm, so an
        # unreviewed mapping can never silently enter [rust.dependencies].
        if confidence == "unsure":
            reason = note or "unverified equivalent"
        else:
            reason = f'unrecognized confidence "{confidence}"'
        commented.append(f'# UNSURE ({go_path}): {reason}')
        if crate and crate != "-":
            commented.append(f'# {crate}')

    lines: list[str] = ["[rust.dependencies]"]
    if stdlib_pkgs:
        lines.append(
            "# Ipê-stdlib / Rust-std packages (no crate needed): "
            + ", ".join(stdlib_pkgs)
        )
    lines.extend(live)
    if commented:
        lines.append("# Unconfirmed mappings — review before enabling:")
        lines.extend(commented)
    return "\n".join(lines) + "\n"


def translate(text: str, crate_map: dict[str, tuple[str, str, str]]) -> str:
    """Replace the manifest's go.dependencies table with a rust.dependencies one."""
    found = _extract_go_block(text)
    if found is None:
        return text
    span, keys = found
    block = render_rust_deps(keys, crate_map)
    replaced = text.replace(span.rstrip("\n") + "\n", block, 1)
    if replaced == text:
        # The span did not end in a newline (EOF without trailing NL).
        replaced = text.replace(span, block.rstrip("\n"), 1)
    return replaced


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        sys.stderr.write(
            "usage: go-deps-to-rust.py <go-to-rust-crates.tsv> <ipe.toml>\n"
        )
        return 2
    crate_map = load_crate_map(argv[1])
    manifest = argv[2]
    with open(manifest, encoding="utf-8") as fh:
        text = fh.read()
    out = translate(text, crate_map)
    if out != text:
        with open(manifest, "w", encoding="utf-8") as fh:
            fh.write(out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
