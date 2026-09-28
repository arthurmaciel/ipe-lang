#!/usr/bin/env python3
"""Refusal proofs for `strict_yaml.py`, the loader every `.github/ci/*.py`
manifest verifier shares.

Each shape `yaml.safe_load` resolves silently but GitHub Actions does not
parse the same way — a duplicate key (top-level and nested), a `<<` merge
key, an anchor/alias pair — gets its own test, per PRINCIPLES.md "Prove the
refusals". `refuse_expression_assembly` (the separate check for a `run:`/
`env:` value that builds its target through a GitHub Actions expression
function instead of naming it literally) gets the same treatment. One
positive case proves the happy path still loads cleanly.

Pure stdlib `unittest` + PyYAML (already a CI dependency). No network.
"""

from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import strict_yaml


class TestStrictSafeLoader(unittest.TestCase):
    # ---- happy path --------------------------------------------------

    def test_ordinary_document_loads_cleanly(self) -> None:
        # No top-level `on:` key here: under YAML 1.1's implicit-boolean
        # resolution (a plain PyYAML property, not this loader's concern)
        # a bare `on` scalar resolves to `True`, not the string "on" — that
        # gotcha is orthogonal to what this test proves.
        doc = strict_yaml.safe_load(
            "name: ci\njobs:\n  build:\n    runs-on: ubuntu-latest\n"
            "    steps:\n      - run: cargo build\n      - run: cargo test\n"
        )
        self.assertEqual(
            doc,
            {
                "name": "ci",
                "jobs": {
                    "build": {
                        "runs-on": "ubuntu-latest",
                        "steps": [{"run": "cargo build"}, {"run": "cargo test"}],
                    }
                },
            },
        )

    # ---- duplicate keys ------------------------------------------------

    def test_duplicate_top_level_key_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError) as ctx:
            strict_yaml.safe_load("run: echo safe\nrun: echo evil\n")
        self.assertIn("duplicate key", str(ctx.exception))
        self.assertIn("'run'", str(ctx.exception))

    def test_duplicate_nested_key_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError) as ctx:
            strict_yaml.safe_load(
                "jobs:\n  build:\n    env:\n      RUSTC_WRAPPER: sccache\n"
                "      RUSTC_WRAPPER: evil\n"
            )
        self.assertIn("duplicate key", str(ctx.exception))

    def test_duplicate_key_is_still_refused_via_yaml_error_handler(self) -> None:
        # StrictYAMLError subclasses yaml.YAMLError so an unmodified
        # `except yaml.YAMLError` callsite still catches it.
        import yaml

        with self.assertRaises(yaml.YAMLError):
            strict_yaml.safe_load("a: 1\na: 2\n")

    # ---- merge keys -----------------------------------------------------

    def test_merge_key_via_alias_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError):
            strict_yaml.safe_load(
                "base: &base\n  runs-on: ubuntu-latest\n"
                "job:\n  <<: *base\n  steps: []\n"
            )

    def test_merge_key_with_inline_mapping_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError) as ctx:
            strict_yaml.safe_load("job:\n  <<:\n    runs-on: ubuntu-latest\n  steps: []\n")
        self.assertIn("merge key", str(ctx.exception))

    # ---- anchors and aliases --------------------------------------------

    def test_anchor_definition_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError) as ctx:
            strict_yaml.safe_load("a: &anchor 1\nb: 2\n")
        self.assertIn("anchor", str(ctx.exception))

    def test_alias_is_refused(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError):
            strict_yaml.safe_load("a: &anchor 1\nb: *anchor\n")

    # ---- unhashable key (defense against a crash on malformed input) ----

    def test_unhashable_key_is_refused_not_a_crash(self) -> None:
        with self.assertRaises(strict_yaml.StrictYAMLError):
            strict_yaml.safe_load("? [a, b]\n: 1\n")


class TestRefuseExpressionAssembly(unittest.TestCase):
    def test_format_call_is_refused(self) -> None:
        msg = strict_yaml.refuse_expression_assembly(
            "echo \"${{ format('RUSTC_{0}','WRAPPER') }}=sccache\" >> \"$GITHUB_ENV\"",
            "ci.yml:build:Build",
        )
        self.assertIsNotNone(msg)
        assert msg is not None
        self.assertIn("ci.yml:build:Build", msg)
        self.assertIn("format", msg)

    def test_join_call_is_refused(self) -> None:
        msg = strict_yaml.refuse_expression_assembly(
            "${{ join(github.event.inputs.*, '_') }}", "loc"
        )
        self.assertIsNotNone(msg)

    def test_tojson_call_is_refused(self) -> None:
        msg = strict_yaml.refuse_expression_assembly("${{ toJSON(github.event) }}", "loc")
        self.assertIsNotNone(msg)

    def test_plain_literal_text_is_not_refused(self) -> None:
        self.assertIsNone(
            strict_yaml.refuse_expression_assembly(
                "echo \"RUSTC_WRAPPER=sccache\" >> \"$GITHUB_ENV\"", "loc"
            )
        )

    def test_unrelated_expression_is_not_refused(self) -> None:
        self.assertIsNone(
            strict_yaml.refuse_expression_assembly("${{ github.ref }}", "loc")
        )

    def test_none_text_is_not_refused(self) -> None:
        self.assertIsNone(strict_yaml.refuse_expression_assembly(None, "loc"))


if __name__ == "__main__":
    unittest.main()
