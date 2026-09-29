#!/usr/bin/env python3
"""Refusal proofs for `manifest_lock_consistency.py`: a lock entry behind the
workspace version, a release commit that leaves a member's lock entry behind
or bumps a non-member, and every malformed input it parses are each refused;
the live repository passes."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import manifest_lock_consistency as mlc  # noqa: E402

_ROOT = """\
[workspace]
members = [
    "a",
    "b",
    "lit",
]

[workspace.package]
version = "1.2.3" # x-release-please-version
"""

_MEMBERS = {
    "a": '[package]\nname = "crate-a"\nversion.workspace = true\n',
    "b": '[package]\nname = "crate_b"\nversion = { workspace = true }\n\n[dependencies]\nversion = "9"\n',
    "lit": '[package]\nname = "literal"\nversion = "0.1.0"\n',
}

_LOCK = """\
version = 4

[[package]]
name = "crate-a"
version = "1.2.3"

[[package]]
name = "crate_b"
version = "1.2.3"
dependencies = [
 "serde",
]

[[package]]
name = "literal"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
"""


def _jsonpath(*names: str) -> str:
    return "$.package[?(" + " || ".join(f"@.name.value==='{n}'" for n in names) + ")].version"


def _config(extra: list[dict] | None = None) -> str:
    files = [{"type": "generic", "path": "Cargo.toml"}]
    files += (
        [{"type": "toml", "path": "Cargo.lock", "jsonpath": _jsonpath("crate-a", "crate_b")}]
        if extra is None
        else extra
    )
    return json.dumps({"packages": {".": {"release-type": "simple", "extra-files": files}}})


class TestManifestLockConsistency(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name
        self.put("Cargo.toml", _ROOT)
        for member, manifest in _MEMBERS.items():
            self.put(os.path.join(member, "Cargo.toml"), manifest)
        self.put("Cargo.lock", _LOCK)
        self.put(mlc.RELEASE_PLEASE_CONFIG, _config())

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def put(self, rel: str, content: str) -> None:
        path = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(content)

    def assertRefused(self, needle: str) -> None:
        errors = mlc.check(self.root)
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def test_consistent_tree_passes(self) -> None:
        self.assertEqual(mlc.check(self.root), [])

    def test_member_lock_entry_behind_is_refused(self) -> None:
        self.put("Cargo.lock", _LOCK.replace('name = "crate_b"\nversion = "1.2.3"', 'name = "crate_b"\nversion = "1.2.2"'))
        self.assertRefused("pins 'crate_b' at 1.2.2, Cargo.toml declares 1.2.3")

    def test_member_missing_from_lock_is_refused(self) -> None:
        self.put("Cargo.lock", _LOCK.replace('name = "crate-a"', 'name = "renamed"'))
        self.assertRefused("pins 'crate-a' at nothing")

    def test_registry_entry_does_not_stand_in_for_a_member(self) -> None:
        self.put("Cargo.lock", _LOCK.replace('name = "crate-a"\nversion = "1.2.3"\n', 'name = "crate-a"\nversion = "1.2.3"\nsource = "registry+x"\n'))
        self.assertRefused("pins 'crate-a' at nothing")

    def test_release_commit_missing_a_member_is_refused(self) -> None:
        self.put(mlc.RELEASE_PLEASE_CONFIG, _config([{"type": "toml", "path": "Cargo.lock", "jsonpath": _jsonpath("crate-a")}]))
        self.assertRefused("'crate_b' inherits the workspace version but the release commit leaves")

    def test_release_commit_bumping_a_literal_version_crate_is_refused(self) -> None:
        self.put(
            mlc.RELEASE_PLEASE_CONFIG,
            _config([{"type": "toml", "path": "Cargo.lock", "jsonpath": _jsonpath("crate-a", "crate_b", "literal")}]),
        )
        self.assertRefused("bumps 'literal' in Cargo.lock, which does not inherit")

    def test_release_commit_without_a_lock_bump_is_refused(self) -> None:
        for extra in (
            [],
            [{"type": "generic", "path": "Cargo.lock"}],
            [
                {"type": "toml", "path": "Cargo.lock", "jsonpath": _jsonpath("crate-a", "crate_b")},
                {"type": "toml", "path": "Cargo.lock", "jsonpath": _jsonpath("crate-a", "crate_b")},
            ],
        ):
            with self.subTest(extra=extra):
                self.put(mlc.RELEASE_PLEASE_CONFIG, _config(extra))
                self.assertRefused("the release commit does not bump Cargo.lock")

    def test_malformed_jsonpath_is_refused(self) -> None:
        for path, needle in (
            ("$.package[*].version", "unrecognised shape"),
            ("$.package[?(!@.source)].version", "is not a member name"),
            ("$.package[?(@.name.value==='crate-a' && @.x)].version", "is not a member name"),
            (_jsonpath("crate-a", "crate_b", "crate-a"), "names a member twice"),
        ):
            with self.subTest(path=path):
                self.put(mlc.RELEASE_PLEASE_CONFIG, _config([{"type": "toml", "path": "Cargo.lock", "jsonpath": path}]))
                self.assertRefused(needle)

    def test_malformed_inputs_are_refused(self) -> None:
        cases = (
            ("Cargo.toml", _ROOT.replace(" # x-release-please-version", ""), "exactly one `# x-release-please-version`"),
            ("Cargo.toml", _ROOT + 'version = "1" # x-release-please-version\n', "found 2"),
            ("Cargo.toml", _ROOT.replace("members = [", "members = [\n    \"crates/*\","), "is a glob"),
            ("Cargo.toml", _ROOT.replace("members", "default-members"), "no `members = [ ... ]` list"),
            ("a/Cargo.toml", "[package]\nversion.workspace = true\n", "has no `[package] name`"),
            (mlc.RELEASE_PLEASE_CONFIG, "{", "is not JSON"),
            ("Cargo.lock", _LOCK + '\n[[package]]\nname = "x"\n', "without exactly one name and version"),
        )
        for rel, content, needle in cases:
            with self.subTest(rel=rel, needle=needle):
                self.tearDown()
                self.setUp()
                self.put(rel, content)
                self.assertRefused(needle)

    def test_missing_member_manifest_is_refused(self) -> None:
        os.remove(os.path.join(self.root, "a", "Cargo.toml"))
        self.assertRefused("cannot read a/Cargo.toml")

    def test_no_inheriting_member_is_refused(self) -> None:
        for member in ("a", "b"):
            self.put(os.path.join(member, "Cargo.toml"), f'[package]\nname = "{member}"\nversion = "1.0.0"\n')
        self.assertRefused("no workspace member inherits")

    def test_live_repository_passes(self) -> None:
        self.assertEqual(mlc.check(), [])


if __name__ == "__main__":
    unittest.main()
