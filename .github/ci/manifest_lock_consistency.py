#!/usr/bin/env python3
"""Cargo.lock pins every workspace member that inherits the workspace version
to the version Cargo.toml declares, and release-please's release commit bumps
exactly those lock entries.

Two agreements, both refused on drift:

1. Lock: each member whose `package.version` is `{ workspace = true }` has one
   path entry in Cargo.lock (no `source`), at the workspace version, which the
   root manifest's `# x-release-please-version` line must mark. A desynced lock
   fails `cargo build --locked` after a release.
2. Release commit: `.config/release-please-config.json` carries one `toml`
   extra-file for Cargo.lock whose jsonpath names exactly those members. The
   release commit then moves Cargo.toml and Cargo.lock together, so no push of
   the release branch ever carries one without the other. The jsonpath matches
   by name alone, so no named member may also be a registry or git package in
   Cargo.lock.

Every path package in Cargo.lock (no `source`) must be a listed workspace
member or a `[workspace] exclude` crate with a literal version: any other path
dependency would lock without either agreement ever reading its manifest.

Manifests are parsed as TOML, never pattern-matched: a member `package.version`
is either `{ workspace = true }` or a literal string, and any other shape is
refused, so no spelling of inheritance can read as a literal and drop the
member from both agreements. Stdlib `tomllib` (`tomli` below Python 3.11) and
no toolchain, so it stays on the fast required PR path.
"""

from __future__ import annotations

import json
import os
import re
import sys

try:
    import tomllib
except ImportError:  # Python < 3.11 outside CI.
    import tomli as tomllib  # type: ignore[no-redef]

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RELEASE_PLEASE_CONFIG = os.path.join(".config", "release-please-config.json")

_MARKER = re.compile(r'^version = "([^"]+)" # x-release-please-version\b', re.MULTILINE)
_HEADER = re.compile(r"^\[\s*([A-Za-z0-9_.-]+)\s*\]", re.MULTILINE)
_JSONPATH = re.compile(r"\$\.package\[\?\((.+)\)\]\.version")
_JSONPATH_NAME = re.compile(r"@\.name\.value==='([A-Za-z0-9_-]+)'")
_INHERITED = {"workspace": True}


class Refusal(Exception):
    """A drift the gate refuses, with the fix in its message."""


def _read(root: str, rel: str) -> str:
    try:
        with open(os.path.join(root, rel)) as f:
            return f.read()
    except OSError as e:
        raise Refusal(f"cannot read {rel}: {e.strerror}") from e


def _toml(root: str, rel: str) -> tuple[str, dict]:
    text = _read(root, rel)
    try:
        return text, tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        raise Refusal(f"{rel} is not TOML: {e}") from e


def _table(doc: object, key: str) -> dict:
    value = doc.get(key) if isinstance(doc, dict) else None
    return value if isinstance(value, dict) else {}


def manifest_version(root_text: str, root_doc: dict) -> str:
    """The workspace version: `[workspace.package] version`, which the single
    `# x-release-please-version` line must mark."""
    found = list(_MARKER.finditer(root_text))
    if len(found) != 1:
        raise Refusal(
            f"Cargo.toml must carry exactly one `# x-release-please-version` line; found {len(found)}"
        )
    headers = _HEADER.findall(root_text, 0, found[0].start())
    if not headers or headers[-1] != "workspace.package":
        raise Refusal(
            "the `# x-release-please-version` line must sit in `[workspace.package]`; "
            f"it sits in [{headers[-1] if headers else ''}]"
        )
    marked = found[0].group(1)
    declared = _table(_table(root_doc, "workspace"), "package").get("version")
    if declared != marked:
        raise Refusal(
            f"the `# x-release-please-version` line reads {marked!r} but "
            f"`[workspace.package] version` is {declared!r}; mark the workspace version itself"
        )
    return marked


def inheriting_members(root: str, root_doc: dict) -> tuple[set[str], set[str]]:
    """Package names of the workspace members whose `package.version` is
    `{ workspace = true }`, and of every package the root declares: each
    listed member plus each `[workspace] exclude` directory, which carries its
    own `[workspace]` and so a literal version no release bump touches."""
    members = _table(root_doc, "workspace").get("members")
    if not isinstance(members, list) or not all(isinstance(m, str) for m in members):
        raise Refusal("Cargo.toml has no `[workspace] members` list of paths")
    names: set[str] = set()
    listed: set[str] = set()
    for member in members:
        if any(c in member for c in "*?["):
            raise Refusal(f"workspace member {member!r} is a glob; list members explicitly")
        rel = os.path.join(member, "Cargo.toml")
        package = _table(_toml(root, rel)[1], "package")
        name = package.get("name")
        if not isinstance(name, str):
            raise Refusal(f"{rel} has no `[package] name`")
        listed.add(name)
        version = package.get("version")
        if version == _INHERITED:
            names.add(name)
        elif not isinstance(version, str):
            raise Refusal(
                f"{rel} `package.version` is {version!r}; declare `version.workspace = true` "
                "or a literal version string"
            )
    excluded = _table(root_doc, "workspace").get("exclude", [])
    if not isinstance(excluded, list) or not all(isinstance(e, str) for e in excluded):
        raise Refusal("Cargo.toml `[workspace] exclude` is not a list of paths")
    for path in excluded:
        if any(c in path for c in "*?["):
            raise Refusal(f"workspace exclude {path!r} is a glob; list excluded crates explicitly")
        rel = os.path.join(path, "Cargo.toml")
        package = _table(_toml(root, rel)[1], "package")
        name = package.get("name")
        if not isinstance(name, str) or not isinstance(package.get("version"), str):
            raise Refusal(f"{rel} needs a `[package] name` and a literal `version` string")
        listed.add(name)
    if not names:
        raise Refusal("no workspace member inherits the workspace version")
    return names, listed


def lock_entries(lock_doc: dict) -> tuple[dict[str, list[str]], set[str]]:
    """Name -> versions of every Cargo.lock entry with no `source` (a path
    package, which is what a workspace member locks as), and the names of the
    entries that carry one (registry or git packages)."""
    packages = lock_doc.get("package")
    if not isinstance(packages, list):
        raise Refusal("Cargo.lock has no `[[package]]` entries")
    paths: dict[str, list[str]] = {}
    sourced: set[str] = set()
    for entry in packages:
        name = entry.get("name") if isinstance(entry, dict) else None
        version = entry.get("version") if isinstance(entry, dict) else None
        if not isinstance(name, str) or not isinstance(version, str):
            raise Refusal("Cargo.lock has a `[[package]]` without a string name and version")
        if "source" in entry:
            sourced.add(name)
        else:
            paths.setdefault(name, []).append(version)
    return paths, sourced


def release_commit_names(config: str) -> set[str]:
    """The members the release-please `toml` extra-file for Cargo.lock bumps."""
    try:
        doc = json.loads(config)
    except json.JSONDecodeError as e:
        raise Refusal(f"{RELEASE_PLEASE_CONFIG} is not JSON: {e}") from e
    extra = _table(_table(doc, "packages"), ".").get("extra-files")
    lock_files = [
        e
        for e in (extra if isinstance(extra, list) else [])
        if isinstance(e, dict) and e.get("path") == "Cargo.lock"
    ]
    fix = (
        f'give {RELEASE_PLEASE_CONFIG} one extra-file {{"type": "toml", "path": "Cargo.lock", '
        "\"jsonpath\": \"$.package[?(@.name.value==='<member>' || ...)].version\"}"
    )
    if len(lock_files) != 1 or lock_files[0].get("type") != "toml":
        raise Refusal(f"the release commit does not bump Cargo.lock; {fix}")
    m = _JSONPATH.fullmatch(str(lock_files[0].get("jsonpath", "")))
    if m is None:
        raise Refusal(f"the Cargo.lock jsonpath has an unrecognised shape; {fix}")
    names: list[str] = []
    for disjunct in m.group(1).split(" || "):
        n = _JSONPATH_NAME.fullmatch(disjunct)
        if n is None:
            raise Refusal(f"the Cargo.lock jsonpath term {disjunct!r} is not a member name; {fix}")
        names.append(n.group(1))
    if len(names) != len(set(names)):
        raise Refusal("the Cargo.lock jsonpath names a member twice")
    return set(names)


def check(root: str = REPO_ROOT) -> list[str]:
    """Every refusal, as the gate prints it; empty when consistent."""
    try:
        root_text, root_doc = _toml(root, "Cargo.toml")
        version = manifest_version(root_text, root_doc)
        members, listed = inheriting_members(root, root_doc)
        locked, sourced = lock_entries(_toml(root, "Cargo.lock")[1])
        bumped = release_commit_names(_read(root, RELEASE_PLEASE_CONFIG))
    except Refusal as e:
        return [str(e)]
    errors = []
    for name in sorted(members):
        got = locked.get(name, [])
        if got != [version]:
            errors.append(
                f"Cargo.lock pins {name!r} at {', '.join(got) or 'nothing'}, Cargo.toml declares "
                f"{version}; run 'cargo update --workspace' and commit Cargo.lock"
            )
    for name in sorted(members - bumped):
        errors.append(
            f"{name!r} inherits the workspace version but the release commit leaves its Cargo.lock "
            f"entry behind; add it to the Cargo.lock jsonpath in {RELEASE_PLEASE_CONFIG}"
        )
    for name in sorted(bumped - members):
        errors.append(
            f"the release commit bumps {name!r} in Cargo.lock, which does not inherit the "
            f"workspace version; remove it from the Cargo.lock jsonpath in {RELEASE_PLEASE_CONFIG}"
        )
    for name in sorted(set(locked) - listed):
        errors.append(
            f"Cargo.lock holds path package {name!r}, which is neither a listed workspace member "
            "nor an excluded crate, so neither agreement covers it; list its directory in "
            "`[workspace] members` or `exclude`"
        )
    for name in sorted(bumped.intersection(sourced)):
        errors.append(
            f"the Cargo.lock jsonpath matches {name!r}, which is also a registry or git package "
            "in Cargo.lock that the release commit would bump with it; rename the member"
        )
    return errors


def main() -> int:
    errors = check()
    for e in errors:
        print(f"manifest-lock-consistency: {e}", file=sys.stderr)
    if errors:
        return 1
    print("manifest-lock-consistency: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
