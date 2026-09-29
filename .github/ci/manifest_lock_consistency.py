#!/usr/bin/env python3
"""Cargo.lock pins every workspace member that inherits the workspace version
to the version Cargo.toml declares, and release-please's release commit bumps
exactly those lock entries.

Two agreements, both refused on drift:

1. Lock: each member with `version.workspace = true` has one path entry in
   Cargo.lock (no `source`), at the `# x-release-please-version` version of the
   root manifest. A desynced lock fails `cargo build --locked` after a release.
2. Release commit: `.config/release-please-config.json` carries one `toml`
   extra-file for Cargo.lock whose jsonpath names exactly those members. The
   release commit then moves Cargo.toml and Cargo.lock together, so no push of
   the release branch ever carries one without the other.

Stdlib only and no toolchain, so it stays on the fast required PR path.
"""

from __future__ import annotations

import json
import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RELEASE_PLEASE_CONFIG = os.path.join(".config", "release-please-config.json")

_MARKER = re.compile(r'^version = "([^"]+)" # x-release-please-version\b', re.MULTILINE)
_MEMBERS = re.compile(r"^members = \[(.*?)^\]", re.MULTILINE | re.DOTALL)
_QUOTED = re.compile(r'"([^"]*)"')
_SECTION = re.compile(r"^\[")
_NAME = re.compile(r'^name = "([^"]+)"$')
_LOCK_VERSION = re.compile(r'^version = "([^"]+)"$')
_INHERITS = re.compile(r"^version(?:\.workspace = true| = \{ workspace = true \})$")
_JSONPATH = re.compile(r"\$\.package\[\?\((.+)\)\]\.version")
_JSONPATH_NAME = re.compile(r"@\.name\.value==='([A-Za-z0-9_-]+)'")


class Refusal(Exception):
    """A drift the gate refuses, with the fix in its message."""


def _read(root: str, rel: str) -> str:
    try:
        with open(os.path.join(root, rel)) as f:
            return f.read()
    except OSError as e:
        raise Refusal(f"cannot read {rel}: {e.strerror}") from e


def manifest_version(root_manifest: str) -> str:
    """The single `# x-release-please-version` version of the root manifest."""
    found = _MARKER.findall(root_manifest)
    if len(found) != 1:
        raise Refusal(
            f"Cargo.toml must carry exactly one `# x-release-please-version` line; found {len(found)}"
        )
    return found[0]


def inheriting_members(root: str, root_manifest: str) -> set[str]:
    """Package names of the workspace members whose `[package]` inherits the
    workspace version."""
    block = _MEMBERS.search(root_manifest)
    if block is None:
        raise Refusal("Cargo.toml has no `members = [ ... ]` list")
    names: set[str] = set()
    for member in _QUOTED.findall(block.group(1)):
        if any(c in member for c in "*?["):
            raise Refusal(f"workspace member {member!r} is a glob; list members explicitly")
        rel = os.path.join(member, "Cargo.toml")
        in_package = False
        name = None
        inherits = False
        for line in _read(root, rel).splitlines():
            line = line.strip()
            if _SECTION.match(line):
                in_package = line == "[package]"
            elif in_package:
                if m := _NAME.match(line):
                    name = m.group(1)
                elif _INHERITS.match(line):
                    inherits = True
        if name is None:
            raise Refusal(f"{rel} has no `[package] name`")
        if inherits:
            names.add(name)
    if not names:
        raise Refusal("no workspace member inherits the workspace version")
    return names


def path_lock_versions(lock: str) -> dict[str, list[str]]:
    """Name -> versions of every Cargo.lock entry with no `source` (a path
    package, which is what a workspace member locks as)."""
    out: dict[str, list[str]] = {}
    for block in lock.split("[[package]]")[1:]:
        fields = [line.strip() for line in block.strip().splitlines()]
        if any(f.startswith("source = ") for f in fields):
            continue
        names = [m.group(1) for f in fields if (m := _NAME.match(f))]
        versions = [m.group(1) for f in fields if (m := _LOCK_VERSION.match(f))]
        if len(names) != 1 or len(versions) != 1:
            raise Refusal("Cargo.lock has a path `[[package]]` without exactly one name and version")
        out.setdefault(names[0], []).append(versions[0])
    return out


def release_commit_names(config: str) -> set[str]:
    """The members the release-please `toml` extra-file for Cargo.lock bumps."""
    try:
        doc = json.loads(config)
    except json.JSONDecodeError as e:
        raise Refusal(f"{RELEASE_PLEASE_CONFIG} is not JSON: {e}") from e
    packages = doc.get("packages") if isinstance(doc, dict) else None
    root_pkg = packages.get(".") if isinstance(packages, dict) else None
    extra = root_pkg.get("extra-files") if isinstance(root_pkg, dict) else None
    lock_files = [
        e for e in extra or [] if isinstance(e, dict) and e.get("path") == "Cargo.lock"
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
        root_manifest = _read(root, "Cargo.toml")
        version = manifest_version(root_manifest)
        members = inheriting_members(root, root_manifest)
        locked = path_lock_versions(_read(root, "Cargo.lock"))
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
