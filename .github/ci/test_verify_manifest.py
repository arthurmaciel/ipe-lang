#!/usr/bin/env python3
"""Refusal proofs for `verify-manifest.py`'s sccache-wiring check (check 6).

Each rejection the guard is supposed to make (a raw sccache-action reference,
a case-variant `uses:`, an env key at workflow/job/step level, a `$GITHUB_ENV`
write, a deterministic job pulling in sccache, a broken composite) gets its
own fixture tree and its own test, per PRINCIPLES.md "Prove the refusals": a
guard no test drives is a guard one edit away from silently vanishing. One
positive case proves the happy path still passes cleanly.

Pure stdlib `unittest`, no network, no PyYAML dependency beyond what
verify-manifest.py itself already requires.
"""

from __future__ import annotations

import importlib.util
import os
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
MODULE_PATH = os.path.join(HERE, "verify-manifest.py")

_spec = importlib.util.spec_from_file_location("verify_manifest", MODULE_PATH)
assert _spec is not None and _spec.loader is not None
verify_manifest = importlib.util.module_from_spec(_spec)
sys.modules["verify_manifest"] = verify_manifest
_spec.loader.exec_module(verify_manifest)

check_sccache_wiring = verify_manifest.check_sccache_wiring

VALID_COMPOSITE = textwrap.dedent(
    """\
    name: Install and wire sccache
    description: test fixture
    runs:
      using: composite
      steps:
        - uses: mozilla-actions/sccache-action@v0.0.9
        - name: Wire rustc through sccache
          shell: bash
          run: |
            echo "RUSTC_WRAPPER=sccache" >> "$GITHUB_ENV"
            echo "SCCACHE_GHA_ENABLED=true" >> "$GITHUB_ENV"
    """
)


def _write(path: str, content: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)


class SccacheFixture:
    """A scratch `.github`-shaped tree: workflows/, actions/sccache/, ci/."""

    def __init__(self, tmp: str, *, composite: str | None = VALID_COMPOSITE):
        self.root = tmp
        if composite is not None:
            _write(os.path.join(tmp, "actions", "sccache", "action.yml"), composite)

    def workflow(self, fname: str, content: str) -> None:
        _write(os.path.join(self.root, "workflows", fname), content)

    def composite(self, name: str, content: str) -> None:
        """Write an arbitrary local composite action at
        `.github/actions/<name>/action.yml` — for wrapper/nesting/escape
        fixtures, distinct from the sanctioned sccache composite itself."""
        _write(os.path.join(self.root, "actions", name, "action.yml"), content)

    def deterministic_checks(self, *, context: str, step: str) -> None:
        import json

        _write(
            os.path.join(self.root, "ci", "deterministic-checks.json"),
            json.dumps({"checks": [{"context": context, "step": step}]}),
        )

    def errors(self) -> list[str]:
        errors: list[str] = []
        check_sccache_wiring(errors, root=self.root)
        return errors


class TestSccacheWiringRefusals(unittest.TestCase):
    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.fx = SccacheFixture(self._tmpdir.name)

    # ---- positive case ------------------------------------------------

    def test_composite_in_non_deterministic_job_passes_cleanly(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: actions/checkout@v7
                      - uses: ./.github/actions/sccache
                      - run: cargo build
                """
            ),
        )
        self.assertEqual(self.fx.errors(), [])

    # ---- (a) raw action reference outside the composite ----------------

    def test_raw_sccache_action_outside_composite_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: mozilla-actions/sccache-action@v0.0.9
                      - run: cargo build
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("runs the raw" in e and "build" in e for e in errors), errors
        )

    def test_case_variant_uses_is_still_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: Mozilla-Actions/Sccache-Action@v0.0.9
                      - run: cargo build
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(any("runs the raw" in e for e in errors), errors)

    # ---- (b) env key at each scope --------------------------------------

    def test_workflow_level_env_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                env:
                  RUSTC_WRAPPER: sccache
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("workflow-level env sets" in e and "rustc_wrapper" in e for e in errors),
            errors,
        )

    def test_job_level_env_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    env:
                      SCCACHE_GHA_ENABLED: "true"
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'build' env sets" in e and "sccache_gha_enabled" in e
                for e in errors
            ),
            errors,
        )

    def test_step_level_env_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                      - name: Some step
                        env:
                          RUSTC_WRAPPER: ""
                        run: cargo build
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("step 'Some step' env sets" in e for e in errors), errors
        )

    # ---- (c) $GITHUB_ENV write outside the composite --------------------

    def test_github_env_write_outside_composite_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - name: Sneak the wrapper in
                        run: |
                          echo "RUSTC_WRAPPER=sccache" >> "$GITHUB_ENV"
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("writes RUSTC_WRAPPER" in e and "GITHUB_ENV" in e for e in errors),
            errors,
        )

    # ---- (d) composite in a job owning a deterministic check step -------

    def test_composite_in_deterministic_job_is_refused(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  clippy:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                      - name: Run clippy
                        run: cargo clippy
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'clippy' uses ./.github/actions/sccache" in e
                and "Run clippy" in e
                for e in errors
            ),
            errors,
        )

    def test_composite_in_non_deterministic_job_is_fine_even_with_the_file_present(
        self,
    ) -> None:
        # Same deterministic-checks.json as above, but the composite is used by
        # a DIFFERENT job that does not own the named step — must not false-positive.
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  clippy:
                    runs-on: ubuntu-latest
                    steps:
                      - name: Run clippy
                        run: cargo clippy
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                      - run: cargo build
                """
            ),
        )
        self.assertEqual(self.fx.errors(), [])

    # ---- composite self-validation --------------------------------------

    def test_missing_composite_file_is_refused(self) -> None:
        empty_tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(empty_tmpdir.cleanup)
        fx = SccacheFixture(empty_tmpdir.name, composite=None)
        fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - run: cargo build
                """
            ),
        )
        errors = fx.errors()
        self.assertTrue(any("does not exist" in e for e in errors), errors)

    def test_composite_not_actually_composite_is_refused(self) -> None:
        fx = SccacheFixture(
            self._tmpdir.name,
            composite=textwrap.dedent(
                """\
                name: not composite
                runs:
                  using: node20
                  main: index.js
                """
            ),
        )
        fx.workflow(
            "ci.yml",
            "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: x\n",
        )
        errors = fx.errors()
        self.assertTrue(any("must be 'composite'" in e for e in errors), errors)

    def test_composite_missing_env_write_is_refused(self) -> None:
        fx = SccacheFixture(
            self._tmpdir.name,
            composite=textwrap.dedent(
                """\
                name: incomplete
                runs:
                  using: composite
                  steps:
                    - uses: mozilla-actions/sccache-action@v0.0.9
                """
            ),
        )
        fx.workflow(
            "ci.yml",
            "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: x\n",
        )
        errors = fx.errors()
        self.assertTrue(
            any("no step writes RUSTC_WRAPPER" in e for e in errors), errors
        )

    # ---- (1) normalized local `uses:` path, not raw-string, comparison ---

    def test_trailing_slash_composite_path_is_still_caught_by_rule_d(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  clippy:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache/
                      - name: Run clippy
                        run: cargo clippy
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'clippy' uses ./.github/actions/sccache" in e
                and "Run clippy" in e
                for e in errors
            ),
            errors,
        )

    def test_dotted_composite_path_is_still_caught_by_rule_d(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  clippy:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/./sccache
                      - name: Run clippy
                        run: cargo clippy
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'clippy' uses ./.github/actions/sccache" in e
                and "Run clippy" in e
                for e in errors
            ),
            errors,
        )

    # ---- (2) other local composite actions are audited, not just workflows

    def test_other_composite_running_raw_sccache_action_is_refused(self) -> None:
        self.fx.composite(
            "leaky",
            textwrap.dedent(
                """\
                name: leaky
                description: not the sanctioned composite
                runs:
                  using: composite
                  steps:
                    - uses: mozilla-actions/sccache-action@v0.0.9
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("runs the raw" in e and "leaky" in e for e in errors), errors
        )

    def test_other_composite_writing_github_env_is_refused(self) -> None:
        self.fx.composite(
            "sneaky",
            textwrap.dedent(
                """\
                name: sneaky
                description: hand-wires the wrapper itself
                runs:
                  using: composite
                  steps:
                    - name: Wire it by hand
                      shell: bash
                      run: |
                        echo "RUSTC_WRAPPER=sccache" >> "$GITHUB_ENV"
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("writes RUSTC_WRAPPER" in e and "sneaky" in e for e in errors), errors
        )

    def test_other_composite_nesting_sccache_is_reachable_via_rule_d(self) -> None:
        self.fx.composite(
            "wrapper",
            textwrap.dedent(
                """\
                name: wrapper
                description: wraps the sanctioned composite one level deep
                runs:
                  using: composite
                  steps:
                    - uses: ./.github/actions/sccache
                """
            ),
        )
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  clippy:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/wrapper
                      - name: Run clippy
                        run: cargo clippy
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'clippy' uses" in e
                and "via" in e
                and "wrapper" in e
                and "Run clippy" in e
                for e in errors
            ),
            errors,
        )

    # ---- (3) a $GITHUB_ENV write need not use `KEY=` assignment syntax ----

    def test_printf_style_github_env_write_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - name: Sneak the wrapper in
                        run: |
                          printf '%s=%s\\n' RUSTC_WRAPPER sccache >> "$GITHUB_ENV"
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "writes RUSTC_WRAPPER" in e and "GITHUB_ENV" in e and "build" in e
                for e in errors
            ),
            errors,
        )

    # ---- (4) the fuller wrapper-var SSOT is enforced, not just RUSTC_WRAPPER

    def test_cargo_build_rustc_wrapper_env_key_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    env:
                      CARGO_BUILD_RUSTC_WRAPPER: sccache
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'build' env sets" in e and "cargo_build_rustc_wrapper" in e
                for e in errors
            ),
            errors,
        )

    # ---- (5) `.yaml` workflows are scanned, not just `.yml` ---------------

    def test_dot_yaml_workflow_extension_is_scanned(self) -> None:
        self.fx.workflow(
            "nightly.yaml",
            textwrap.dedent(
                """\
                name: nightly
                on: schedule
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: mozilla-actions/sccache-action@v0.0.9
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("runs the raw" in e and "nightly.yaml" in e for e in errors), errors
        )

    # ---- (6) container:/services: env scopes are scanned too --------------

    def test_container_env_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    container:
                      image: rust:latest
                      env:
                        RUSTC_WRAPPER: sccache
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'build' container env sets" in e and "rustc_wrapper" in e
                for e in errors
            ),
            errors,
        )

    def test_service_env_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    services:
                      db:
                        image: postgres
                        env:
                          SCCACHE_GHA_ENABLED: "true"
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "job 'build' service 'db' env sets" in e
                and "sccache_gha_enabled" in e
                for e in errors
            ),
            errors,
        )

    # ---- (7) a non-mapping `env:` fails closed instead of defaulting empty

    def test_non_mapping_workflow_env_fails_closed(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                env: ${{ fromJSON(vars.E) }}
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any("not a plain mapping" in e and "ci.yml" in e for e in errors), errors
        )

    def test_non_mapping_step_env_fails_closed(self) -> None:
        self.fx.workflow(
            "ci.yml",
            textwrap.dedent(
                """\
                name: ci
                on: push
                jobs:
                  build:
                    runs-on: ubuntu-latest
                    steps:
                      - uses: ./.github/actions/sccache
                      - name: Some step
                        env: ${{ fromJSON(vars.E) }}
                        run: cargo build
                """
            ),
        )
        errors = self.fx.errors()
        self.assertTrue(
            any(
                "not a plain mapping" in e and "Some step" in e for e in errors
            ),
            errors,
        )


if __name__ == "__main__":
    unittest.main()
