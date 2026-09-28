#!/usr/bin/env python3
"""Shared strict YAML loader: exactly the document GitHub Actions runs, and
nothing looser.

A YAML document with a duplicate mapping key, a `<<` merge key, or an anchor/
alias has no single canonical parse under `yaml.safe_load`: a duplicate key
silently keeps the last occurrence and drops every earlier one, a merge key
splices in a payload that never appears at the parsed key's own location, and
an alias resolves to whatever its anchor happened to capture. A CI verifier
loading a workflow that way can certify a value GitHub never runs — the first,
discarded copy of a duplicate `run:`/`env:` key. `StrictSafeLoader` makes each
of those shapes unrepresentable instead of silently resolved, so the parsed
value and the value GitHub executes are the same document by construction.

LIMIT: this loader guards YAML's structural ambiguities only. A workflow
expression (`${{ ... }}`) built by string assembly — `format(`, `join(`,
`toJSON(` — inside a `run:`/`env:` value is syntactically unambiguous YAML
that still hides its assembled target (e.g. an env-var name built as
`format('RUSTC_{0}', 'WRAPPER')`) from a literal-text scan for that name.
`refuse_expression_assembly` below is the separate, blanket check for that
class; every verifier that scans `run:`/`env:` text for a sensitive name must
also run it, not rely on this loader alone.
"""

from __future__ import annotations

import re

import yaml

# GitHub Actions expression functions that assemble a string at run time
# rather than naming one literally. Refused wherever they appear inside a
# `${{ ... }}` expression in a `run:` or `env:` value: any such value can
# assemble a sensitive target name (an env-var key, `$GITHUB_ENV`,
# `$GITHUB_PATH`, an `RUSTC*` wrapper var) out of pieces no single literal
# scan will ever match. Refused unconditionally rather than only when a
# sensitive name is *also* literally present — a scoped check is exactly the
# gap this closes, since the whole point of assembly is to keep the sensitive
# name out of the literal text.
_EXPRESSION_ASSEMBLY_RE = re.compile(r"\$\{\{[^}]*\b(?:format|join|toJSON)\s*\(")


class StrictYAMLError(yaml.YAMLError):
    """A structurally ambiguous YAML document: duplicate key, merge key, or
    anchor/alias. Subclasses `yaml.YAMLError` so an existing
    `except yaml.YAMLError` callsite catches it with no per-site change."""


class StrictSafeLoader(yaml.SafeLoader):
    """`yaml.SafeLoader` with every structurally ambiguous construct refused
    instead of resolved."""

    def compose_node(self, parent, index):
        if self.check_event(yaml.AliasEvent):
            event = self.peek_event()
            raise StrictYAMLError(
                f"alias {event.anchor!r} at line {event.start_mark.line + 1}: "
                "aliases are refused; write the value out in full"
            )
        event = self.peek_event()
        if event.anchor is not None:
            raise StrictYAMLError(
                f"anchor {event.anchor!r} at line {event.start_mark.line + 1}: "
                "anchors are refused; write the value out in full"
            )
        return super().compose_node(parent, index)

    def flatten_mapping(self, node):
        for key_node, _value_node in node.value:
            if key_node.tag == "tag:yaml.org,2002:merge":
                raise StrictYAMLError(
                    f"merge key '<<' at line {key_node.start_mark.line + 1}: "
                    "merge keys are refused; write the mapping out in full"
                )
        super().flatten_mapping(node)

    def construct_mapping(self, node, deep=False):
        if isinstance(node, yaml.MappingNode):
            self.flatten_mapping(node)
        mapping: dict = {}
        for key_node, value_node in node.value:
            key = self.construct_object(key_node, deep=deep)
            try:
                duplicate = key in mapping
            except TypeError as e:
                raise StrictYAMLError(
                    f"unhashable key at line {key_node.start_mark.line + 1}: {e}"
                ) from e
            if duplicate:
                raise StrictYAMLError(
                    f"duplicate key {key!r} at line {key_node.start_mark.line + 1}: "
                    "the first occurrence already set this key"
                )
            mapping[key] = self.construct_object(value_node, deep=deep)
        return mapping


def safe_load(stream):
    """Strict drop-in for `yaml.safe_load`: same success shape and the same
    `yaml.YAMLError` on malformed YAML, but additionally refuses duplicate
    keys, merge keys, and anchors/aliases instead of silently resolving
    them."""
    return yaml.load(stream, Loader=StrictSafeLoader)


def refuse_expression_assembly(text: str | None, loc: str) -> str | None:
    """`None` if `text` is absent or contains no GitHub Actions
    expression-assembly call; otherwise an error message naming `loc`."""
    if text is None:
        return None
    m = _EXPRESSION_ASSEMBLY_RE.search(text)
    if m is None:
        return None
    return (
        f"{loc}: expression-assembly call {m.group(0)[:-1].split('{{')[-1].strip()}(...) "
        "in a run:/env: value — a value built from string-assembly functions "
        "(format/join/toJSON) can hide a target name from any literal-text scan; "
        "write the value as a literal instead"
    )
