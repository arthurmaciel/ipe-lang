#!/usr/bin/env python3
"""Refusal proofs for `verify-manifest.py`'s step checks (checks 6 and 7).

Check 7 covers content-pinned `uses:`/images, the hash-checked pip shape, and
env-file writes only through `github-env.sh`; its cases sit in
`TestPinnedInputsAndEnvFileWrites` and `TestGithubEnvHelper`.

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

check_workflow_steps = verify_manifest.check_workflow_steps

# The live sanctioned composite is the fixture: the canonical form is proven
# against the file CI actually runs, never a hand-kept copy.
with open(os.path.join(os.path.dirname(HERE), "actions", "sccache", "action.yml")) as _f:
    VALID_COMPOSITE = _f.read()
with open(os.path.join(HERE, "github-env-allowlist.txt")) as _f:
    VALID_ENV_ALLOWLIST = _f.read()


def _write(path: str, content: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)


class SccacheFixture:
    """A scratch repository whose `.github/` holds workflows/, actions/sccache/,
    ci/ — `repo` is the repository root local `uses: ./...` resolve against."""

    def __init__(self, tmp: str, *, composite: str | None = VALID_COMPOSITE):
        self.repo = tmp
        self.root = os.path.join(tmp, ".github")
        os.makedirs(self.root, exist_ok=True)
        tmp = self.root
        if composite is not None:
            _write(os.path.join(tmp, "actions", "sccache", "action.yml"), composite)
        _write(os.path.join(tmp, "ci", "github-env-allowlist.txt"), VALID_ENV_ALLOWLIST)
        self.deterministic_checks(context="unrelated", step="Unrelated step")

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
            json.dumps({"about": "fixture", "checks": [{"context": context, "step": step}]}),
        )

    def errors(self) -> list[str]:
        errors: list[str] = []
        check_workflow_steps(errors, root=self.root)
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
                      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
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
                      - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad
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
                      - uses: Mozilla-Actions/Sccache-Action@7d986dd989559c6ecdb630a3fd2557667be217ad
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
                    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad
                """
            ),
        )
        fx.workflow(
            "ci.yml",
            "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: x\n",
        )
        errors = fx.errors()
        self.assertTrue(
            any("exactly two steps" in e for e in errors), errors
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
                    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad
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
                      - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad
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



def _ci(job_body: str, *, top: str = "") -> str:
    """A one-job `ci.yml` (job id `clippy`) with `job_body` under the job."""
    return (
        "name: ci\non: push\n"
        + top
        + "jobs:\n  clippy:\n    runs-on: ubuntu-latest\n"
        + textwrap.indent(textwrap.dedent(job_body), "    ")
    )


class TestSccacheWiringClosure(unittest.TestCase):
    """Each class of wiring check 6 must refuse, one fixture per shape."""

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.fx = SccacheFixture(self._tmpdir.name)

    def assertRefused(self, *needles: str) -> list[str]:
        errors = self.fx.errors()
        self.assertTrue(any(all(n in e for n in needles) for e in errors), errors)
        return errors

    def ci(self, job_body: str, **kw: str) -> None:
        self.fx.workflow("ci.yml", _ci(job_body, **kw))

    def repo_action(self, rel_dir: str, content: str, fname: str = "action.yml") -> None:
        _write(os.path.join(self.fx.repo, rel_dir, fname), textwrap.dedent(content))

    # ---- positive: benign shapes stay clean ------------------------------

    def test_yaml_comment_naming_the_wrapper_is_not_scanned(self) -> None:
        self.fx.workflow(
            "ci.yml",
            "# a job that wants RUSTC_WRAPPER uses ./.github/actions/sccache\n"
            + _ci("steps:\n  - uses: ./.github/actions/sccache\n  - run: cargo build\n"),
        )
        self.assertEqual(self.fx.errors(), [])

    def test_benign_composite_outside_github_actions_passes(self) -> None:
        self.repo_action(
            "tools/ci/setup",
            "runs:\n  using: composite\n  steps:\n    - shell: bash\n      run: rustc --version\n",
        )
        self.ci("steps:\n  - uses: ./tools/ci/setup\n  - run: cargo build\n")
        self.assertEqual(self.fx.errors(), [])

    # ---- (c) free-text wiring, no $GITHUB_ENV needed ----------------------

    def test_inline_env_in_run_is_refused(self) -> None:
        self.ci("steps:\n  - name: Build\n    run: RUSTC_WRAPPER=sccache cargo build\n")
        self.assertRefused("step 'Build' run:", "RUSTC_WRAPPER")

    def test_export_in_run_is_refused(self) -> None:
        self.ci("steps:\n  - name: Build\n    run: export rustc_workspace_wrapper=sccache\n")
        self.assertRefused("step 'Build' run:")

    def test_step_shell_wrapper_is_refused(self) -> None:
        self.ci(
            "steps:\n  - name: Build\n    shell: env RUSTC_WRAPPER=sccache bash -e {0}\n"
            "    run: cargo build\n"
        )
        self.assertRefused("step 'Build' shell:")

    def test_job_defaults_run_shell_is_refused(self) -> None:
        self.ci(
            "defaults:\n  run:\n    shell: env RUSTC_WRAPPER=sccache bash {0}\n"
            "steps:\n  - run: cargo build\n"
        )
        self.assertRefused("job 'clippy' defaults.run.shell")

    def test_workflow_defaults_run_shell_is_refused(self) -> None:
        self.ci(
            "steps:\n  - run: cargo build\n",
            top="defaults:\n  run:\n    shell: env SCCACHE_GHA_ENABLED=true bash {0}\n",
        )
        self.assertRefused("ci.yml: workflow-level defaults.run.shell")

    def test_cargo_config_flag_is_refused(self) -> None:
        self.ci("steps:\n  - name: Build\n    run: cargo --config build.rustc-wrapper='\"sccache\"' build\n")
        self.assertRefused("step 'Build' run:", "rustc-wrapper")

    def test_cargo_config_file_write_is_refused(self) -> None:
        self.ci(
            "steps:\n  - name: Cfg\n"
            "    run: printf '[build]\\nrustc-wrapper = \"sccache\"\\n' >> ~/.cargo/config.toml\n"
        )
        self.assertRefused("step 'Cfg' run:")

    def test_with_input_naming_the_wrapper_is_refused(self) -> None:
        self.ci("steps:\n  - uses: some/action@0123456789abcdef0123456789abcdef01234567\n    with:\n      rustc-wrapper: sccache\n")
        self.assertRefused("with.rustc-wrapper")

    def test_composite_step_shell_wrapper_is_refused(self) -> None:
        self.fx.composite(
            "w",
            "runs:\n  using: composite\n  steps:\n"
            "    - shell: env RUSTC_WRAPPER=sccache bash {0}\n      run: cargo build\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml", "shell:")

    # ---- (b) every wrapper key, every scope ------------------------------

    def test_workspace_wrapper_env_keys_are_refused(self) -> None:
        for key in ("RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
            with self.subTest(key=key):
                self.ci(f"env:\n  {key}: sccache\nsteps:\n  - run: cargo build\n")
                self.assertRefused("job 'clippy' env sets", key.casefold())

    def test_env_key_inside_other_composite_is_refused(self) -> None:
        self.fx.composite(
            "w",
            "runs:\n  using: composite\n  steps:\n"
            "    - name: B\n      shell: bash\n      run: cargo build\n"
            "      env:\n        RUSTC_WRAPPER: sccache\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml: step 'B' env sets")

    def test_non_mapping_env_at_every_scope_fails_closed(self) -> None:
        expr = "${{ fromJSON(vars.E) }}"
        cases = {
            "job": f"env: {expr}\nsteps: []\n",
            "container": f"container:\n  image: rust\n  env: {expr}\nsteps: []\n",
            "service": f"services:\n  db:\n    image: pg\n    env: {expr}\nsteps: []\n",
        }
        for scope, body in cases.items():
            with self.subTest(scope=scope):
                self.ci(body)
                self.assertRefused("not a plain mapping")
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.fx.composite(
            "w",
            f"runs:\n  using: composite\n  steps:\n    - name: B\n      shell: bash\n"
            f"      run: x\n      env: {expr}\n",
        )
        self.assertRefused("./.github/actions/w/action.yml: step 'B'", "not a plain mapping")

    # ---- local `uses:` resolution (F1) -----------------------------------

    def test_unresolved_local_action_is_refused(self) -> None:
        for uses in ("./.github/actions/sccache@main", "./tools/ci/cache", "./.github/actions/nope"):
            with self.subTest(uses=uses):
                self.ci(f"steps:\n  - uses: {uses}\n")
                self.assertRefused("does not exist")

    def test_local_action_escaping_repo_is_refused(self) -> None:
        self.ci("steps:\n  - uses: ./../elsewhere\n")
        self.assertRefused("escapes the repository root")

    def test_node_and_docker_local_actions_are_refused(self) -> None:
        for using in ("node20", "docker"):
            with self.subTest(using=using):
                self.fx.composite("opaque", f"runs:\n  using: {using}\n  main: index.js\n")
                self.ci("steps:\n  - uses: ./.github/actions/opaque\n")
                self.assertRefused("must be 'composite'", repr(using))

    def test_both_action_yml_and_yaml_is_refused(self) -> None:
        self.fx.composite("w", "runs:\n  using: composite\n  steps: []\n")
        _write(
            os.path.join(self.fx.root, "actions", "w", "action.yaml"),
            "runs:\n  using: composite\n  steps: []\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("both action.yml and action.yaml")

    def test_action_yaml_extension_is_resolved(self) -> None:
        _write(
            os.path.join(self.fx.root, "actions", "w", "action.yaml"),
            "runs:\n  using: composite\n  steps:\n    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml", "runs the raw")

    def test_composite_outside_github_actions_reaching_sccache_hits_rule_d(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.repo_action(
            "tools/ci/cache",
            "runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/sccache\n",
        )
        self.ci("steps:\n  - uses: ./tools/ci/cache\n  - name: Run clippy\n    run: cargo clippy\n")
        self.assertRefused("job 'clippy' uses", "via ./tools/ci/cache", "Run clippy")

    def test_nested_path_composite_running_raw_action_is_refused(self) -> None:
        self.fx.composite("x/y", "runs:\n  using: composite\n  steps:\n    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n")
        self.ci("steps:\n  - uses: ./.github/actions/x/y\n")
        self.assertRefused("./.github/actions/x/y/action.yml", "runs the raw")

    def test_case_variant_sanctioned_path_hits_rule_d(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        self.ci(
            "steps:\n  - uses: ./.github/actions/sccache\n  - uses: ./.github/Actions/SCCACHE\n"
            "  - name: Run clippy\n    run: cargo clippy\n"
        )
        self.assertRefused("job 'clippy' uses ./.github/actions/sccache", "Run clippy")

    def test_nesting_past_the_bound_is_refused(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        limit = verify_manifest.LOCAL_ACTION_DEPTH_LIMIT
        for i in range(limit + 5):
            self.fx.composite(
                f"c{i}", f"runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/c{i + 1}\n"
            )
        self.fx.composite(
            f"c{limit + 5}", "runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/sccache\n"
        )
        self.ci("steps:\n  - uses: ./.github/actions/c0\n  - name: Run clippy\n    run: cargo clippy\n")
        self.assertRefused("nesting exceeds")

    def test_nesting_at_the_bound_is_decided(self) -> None:
        self.fx.deterministic_checks(context="clippy", step="Run clippy")
        limit = verify_manifest.LOCAL_ACTION_DEPTH_LIMIT
        for i in range(limit - 1):
            self.fx.composite(
                f"c{i}", f"runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/c{i + 1}\n"
            )
        self.fx.composite(
            f"c{limit - 1}", "runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/sccache\n"
        )
        self.ci("steps:\n  - uses: ./.github/actions/c0\n  - name: Run clippy\n    run: cargo clippy\n")
        errors = self.assertRefused("job 'clippy' uses", "via ./.github/actions/c0")
        self.assertFalse(any("nesting exceeds" in e for e in errors), errors)

    def test_local_action_cycle_is_refused(self) -> None:
        self.fx.composite("a", "runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/b\n")
        self.fx.composite("b", "runs:\n  using: composite\n  steps:\n    - uses: ./.github/actions/a\n")
        self.ci("steps:\n  - uses: ./.github/actions/a\n")
        self.assertRefused("local action cycle")

    def test_job_level_reusable_workflow_is_refused(self) -> None:
        self.fx.workflow("called.yml", _ci("steps:\n  - run: cargo build\n"))
        for call in (
            "./.github/workflows/gone.yml",
            "./.github/workflows/called.yml",
            "owner/repo/.github/workflows/w.yml@v1",
        ):
            with self.subTest(call=call):
                self.fx.workflow("ci.yml", f"name: ci\non: push\njobs:\n  a:\n    uses: {call}\n")
                self.assertRefused("job 'a': calls a reusable workflow", call)

    # ---- byte-exact local-action identity ---------------------------------

    def test_referenced_case_variant_of_sanctioned_composite_is_refused(self) -> None:
        self.fx.composite(
            "SCCACHE",
            "runs:\n  using: composite\n  steps:\n    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            "    - shell: bash\n      run: echo \"RUSTC_WRAPPER=\" >> \"$GITHUB_ENV\"\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/SCCACHE\n")
        errors = self.assertRefused("'./.github/actions/SCCACHE' is case-fold-equal to '.github/actions/sccache'")
        self.assertTrue(any("holds case-fold-equal entries 'SCCACHE' and 'sccache'" in e for e in errors), errors)

    def test_unreferenced_case_variant_sibling_is_refused(self) -> None:
        self.fx.composite("SCCACHE", "runs:\n  using: composite\n  steps: []\n")
        self.ci("steps:\n  - uses: ./.github/actions/sccache\n  - run: cargo build\n")
        self.assertRefused("holds case-fold-equal entries 'SCCACHE' and 'sccache'")

    def test_case_variant_exemption_needs_the_sanctioned_file_absent_too(self) -> None:
        fx = SccacheFixture(self._tmpdir.name + "/bare", composite=None)
        fx.composite("SCCACHE", "runs:\n  using: composite\n  steps:\n    - uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n")
        fx.workflow("ci.yml", _ci("steps:\n  - uses: ./.github/actions/SCCACHE\n"))
        errors = fx.errors()
        self.assertTrue(any("case-fold-equal" in e or "runs the raw" in e for e in errors), errors)
        self.assertFalse(any("uses ./.github/actions/sccache" in e for e in errors), errors)

    def test_reference_differing_in_case_from_disk_is_refused(self) -> None:
        self.repo_action("tools/w", "runs:\n  using: composite\n  steps: []\n")
        self.ci("steps:\n  - uses: ./tools/W\n")
        self.assertRefused("names 'W' but the directory holds 'w'", "byte-exact")

    def test_case_variant_twin_directory_on_reference_path_is_refused(self) -> None:
        benign = "runs:\n  using: composite\n  steps: []\n"
        self.repo_action("tools/ci/setup", benign)
        self.repo_action("tools/CI/setup", benign)
        self.ci("steps:\n  - uses: ./tools/ci/setup\n")
        self.assertRefused("passes through", "case-fold-equal entries ['CI', 'ci']")

    # ---- env values and rustc-replacing keys ------------------------------

    def test_env_value_naming_a_wiring_key_is_refused_at_every_scope(self) -> None:
        write = "run: echo \"$K=sccache\" >> \"$GITHUB_ENV\"\n"
        cases = {
            "workflow": ({"top": "env:\n  K: RUSTC_WRAPPER\n"}, f"steps:\n  - {write}", "workflow-level env.K"),
            "job": ({}, f"env:\n  K: RUSTC_WRAPPER\nsteps:\n  - {write}", "job 'clippy' env.K"),
            "step": ({}, f"steps:\n  - name: S\n    env:\n      K: RUSTC_WRAPPER\n    {write}", "step 'S' env.K"),
            "container": (
                {},
                f"container:\n  image: rust\n  env:\n    K: RUSTC_WRAPPER\nsteps:\n  - {write}",
                "job 'clippy' container.env.K",
            ),
            "service": (
                {},
                f"services:\n  db:\n    image: pg\n    env:\n      K: SCCACHE_DIR\nsteps:\n  - {write}",
                "job 'clippy' services.db.env.K",
            ),
        }
        for scope, (kw, body, needle) in cases.items():
            with self.subTest(scope=scope):
                self.ci(body, **kw)
                self.assertRefused(needle)

    def test_env_value_in_other_composite_is_refused(self) -> None:
        self.fx.composite(
            "w",
            "runs:\n  using: composite\n  steps:\n    - name: B\n      shell: bash\n"
            "      env:\n        K: cargo_build_rustc_wrapper\n"
            "      run: echo \"$K=sccache\" >> \"$GITHUB_ENV\"\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml: step 'B' env.K")

    def test_rustc_replacing_env_keys_are_refused(self) -> None:
        for key in ("RUSTC", "CARGO_BUILD_RUSTC", "rustc", "Cargo_Build_Rustc"):
            with self.subTest(key=key):
                self.ci(f"env:\n  {key}: /tmp/fake-rustc\nsteps:\n  - run: cargo build\n")
                self.assertRefused("job 'clippy' env sets", repr(key.casefold()))

    def test_rustc_replacing_text_is_refused(self) -> None:
        for run in (
            "RUSTC=/tmp/r cargo build",
            "export CARGO_BUILD_RUSTC=/tmp/r",
            "echo \"RUSTC=/tmp/r\" >> \"$GITHUB_ENV\"",
            "cargo --config build.rustc='\"/tmp/r\"' build",
            "printf '[build]\\nrustc = \"/tmp/r\"\\n' >> ~/.cargo/config.toml",
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: Build\n    run: {run}\n")
                self.assertRefused("step 'Build' run:")
        self.ci("steps:\n  - uses: some/action@0123456789abcdef0123456789abcdef01234567\n    with:\n      rustc: /tmp/r\n")
        self.assertRefused("with.rustc")

    def test_benign_rustc_mentions_pass(self) -> None:
        self.ci("env:\n  RUSTFLAGS: -Dwarnings\nsteps:\n  - run: rustc --version && cargo build\n")
        self.assertEqual(self.fx.errors(), [])

    # ---- strict positive proof of the sanctioned composite (F2) ----------

    def _sanctioned(self, steps: str) -> None:
        _write(
            os.path.join(self.fx.root, "actions", "sccache", "action.yml"),
            "runs:\n  using: composite\n  steps:\n" + textwrap.indent(textwrap.dedent(steps), "    "),
        )
        self.ci("steps:\n  - run: cargo build\n")

    def test_composite_writing_only_one_var_is_refused(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            "- shell: bash\n  run: echo SCCACHE_GHA_ENABLED=true >> $GITHUB_ENV\n"
        )
        self.assertRefused("step 2 is not the canonical wiring step")

    def test_composite_loose_write_is_not_proof(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            "- shell: bash\n  run: |\n"
            "    # RUSTC_WRAPPER=sccache SCCACHE_GHA_ENABLED=true $GITHUB_ENV\n"
            "    printf '%s=%s\\n' RUSTC_WRAPPER sccache >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("step 2 is not the canonical wiring step")

    def test_composite_conditional_wiring_is_not_proof(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            "- shell: bash\n  if: false\n  run: |\n"
            "    echo \"RUSTC_WRAPPER=sccache\" >> \"$GITHUB_ENV\"\n"
            "    echo \"SCCACHE_GHA_ENABLED=true\" >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("step 2 is not the canonical wiring step")

    def test_composite_conditional_install_is_not_proof(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n  if: false\n"
            "- shell: bash\n  run: |\n"
            "    echo \"RUSTC_WRAPPER=sccache\" >> \"$GITHUB_ENV\"\n"
            "    echo \"SCCACHE_GHA_ENABLED=true\" >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("step 1 must be exactly")

    _WIRE = (
        "- name: Wire rustc through sccache\n  shell: bash\n  run: |\n"
        + textwrap.indent(verify_manifest.SCCACHE_WIRE_RUN, "    ")
    )

    def test_canonical_composite_with_other_install_ref_passes(self) -> None:
        self._sanctioned("- uses: mozilla-actions/sccache-action@0123456789abcdef0123456789abcdef01234567\n" + self._WIRE)
        self.assertEqual(self.fx.errors(), [])

    def test_composite_wiring_under_dead_branch_is_refused(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            "- name: Wire rustc through sccache\n  shell: bash\n  run: |\n"
            "    if false; then\n"
            + textwrap.indent(verify_manifest.SCCACHE_WIRE_RUN, "    ")
            + "    fi\n"
        )
        self.assertRefused("step 2 is not the canonical wiring step")

    def test_composite_later_step_unwiring_is_refused(self) -> None:
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n"
            + self._WIRE
            + "- shell: bash\n  run: echo \"RUSTC_WRAPPER=\" >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("exactly two steps")

    def test_composite_nested_local_uses_is_refused(self) -> None:
        self.repo_action(
            "tools/act",
            "runs:\n  using: composite\n  steps:\n    - shell: bash\n      run: echo \"RUSTC_WRAPPER=\" >> \"$GITHUB_ENV\"\n",
        )
        self._sanctioned("- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n" + self._WIRE + "- uses: ./tools/act\n")
        self.assertRefused("exactly two steps")
        self._sanctioned("- uses: ./tools/act\n" + self._WIRE)
        self.assertRefused("step 1 must be exactly")

    def test_composite_install_step_with_extra_keys_is_refused(self) -> None:
        for extra in ("  with:\n    version: v0.8.0\n", "  continue-on-error: true\n", "  name: x\n"):
            with self.subTest(extra=extra):
                self._sanctioned("- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n" + extra + self._WIRE)
                self.assertRefused("step 1 must be exactly")

    def test_composite_reordered_or_env_bearing_wire_is_refused(self) -> None:
        self._sanctioned(self._WIRE + "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n")
        self.assertRefused("step 1 must be exactly")
        self._sanctioned(
            "- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n" + self._WIRE + "  env:\n    RUSTC_WRAPPER: ''\n"
        )
        self.assertRefused("step 2 is not the canonical wiring step")

    def test_composite_extra_document_or_runs_keys_are_refused(self) -> None:
        body = "  steps:\n" + textwrap.indent("- uses: mozilla-actions/sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad\n" + self._WIRE, "    ")
        path = os.path.join(self.fx.root, "actions", "sccache", "action.yml")
        self.ci("steps:\n  - run: cargo build\n")
        _write(path, "inputs:\n  x:\n    default: y\nruns:\n  using: composite\n" + body)
        self.assertRefused("keys ['inputs'] are outside the canonical composite")
        _write(path, "runs:\n  using: composite\n  post: x\n" + body)
        self.assertRefused("`runs:` keys must be exactly")

    # ---- deterministic-checks SSOT (F4) -----------------------------------

    def test_missing_deterministic_checks_file_is_refused(self) -> None:
        os.remove(os.path.join(self.fx.root, "ci", "deterministic-checks.json"))
        self.ci("steps:\n  - uses: ./.github/actions/sccache\n  - name: Run clippy\n    run: cargo clippy\n")
        self.assertRefused("cannot establish the deterministic check steps")

    def test_malformed_deterministic_checks_file_is_refused(self) -> None:
        _write(os.path.join(self.fx.root, "ci", "deterministic-checks.json"), '{"checks": "x"}')
        self.ci("steps:\n  - run: cargo build\n")
        self.assertRefused("cannot establish the deterministic check steps")

    # ---- malformed shapes (F5) --------------------------------------------

    def test_malformed_shapes_are_refused(self) -> None:
        cases = {
            "doc": ("- a\n- b\n", "the workflow document is not a mapping"),
            "jobs": ("name: ci\non: push\njobs: [1]\n", "jobs: is not a non-empty mapping"),
            "job": ("name: ci\non: push\njobs:\n  clippy: 3\n", "the job is not a mapping"),
            "steps": (_ci("steps: oops\n"), "steps: is not a list"),
            "step": (_ci("steps:\n  - just-a-string\n"), "steps[0] is not a mapping"),
            "services": (_ci("services: [pg]\nsteps: []\n"), "services: is not a mapping"),
            "container": (_ci("container: [rust]\nsteps: []\n"), "container: is not"),
            "defaults": (_ci("defaults: x\nsteps: []\n"), "defaults: is not a mapping"),
            "run": (_ci("steps:\n  - run: [a]\n"), "run: is not a string"),
        }
        for what, (doc, needle) in cases.items():
            with self.subTest(what=what):
                self.fx.workflow("ci.yml", doc)
                self.assertRefused(needle)

    def test_malformed_composite_steps_are_refused(self) -> None:
        self.fx.composite("w", "runs:\n  using: composite\n  steps: nope\n")
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("steps: is not a list")

    # ---- expression-assembly in run:/env: (strict_yaml.refuse_expression_assembly) --

    def test_format_assembly_in_run_is_refused(self) -> None:
        self.ci(
            "steps:\n  - name: Build\n"
            "    run: echo \"${{ format('RUSTC_{0}', 'WRAPPER') }}=sccache\" >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("step 'Build' run:", "expression-assembly")

    def test_join_assembly_in_env_is_refused(self) -> None:
        self.ci(
            "steps:\n  - name: Build\n    env:\n"
            "      X: \"${{ join(github.event.inputs.*, '_') }}\"\n"
            "    run: cargo build\n"
        )
        self.assertRefused("step 'Build' env.X", "expression-assembly")

    def test_tojson_assembly_in_run_is_refused(self) -> None:
        self.ci(
            "steps:\n  - name: Build\n"
            "    run: echo '${{ toJSON(github.event) }}' >> \"$GITHUB_ENV\"\n"
        )
        self.assertRefused("step 'Build' run:", "expression-assembly")

    # ---- YAML structural ambiguities feed the workflow loader closed
    # (strict_yaml.StrictSafeLoader), not just check_workflow_steps' text scan --

    def test_duplicate_key_workflow_is_refused_not_silently_resolved(self) -> None:
        self.fx.workflow(
            "ci.yml",
            "name: ci\non: push\njobs:\n  clippy:\n    runs-on: ubuntu-latest\n"
            "    steps:\n      - run: cargo build\n"
            "    steps:\n      - run: RUSTC_WRAPPER=evil cargo build\n",
        )
        self.assertRefused("ci.yml is not valid YAML", "duplicate key")

    def test_merge_key_workflow_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            "base: &base\n  runs-on: ubuntu-latest\n"
            "name: ci\non: push\njobs:\n  clippy:\n"
            "    <<: *base\n    steps:\n      - run: cargo build\n",
        )
        self.assertRefused("ci.yml is not valid YAML")

    def test_anchor_alias_workflow_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            "name: ci\non: push\njobs:\n  clippy: &clippy\n    runs-on: ubuntu-latest\n"
            "    steps:\n      - run: cargo build\n  clippy2: *clippy\n",
        )
        self.assertRefused("ci.yml is not valid YAML")


_HELPER_CALL = 'bash "$GITHUB_WORKSPACE/.github/ci/github-env.sh"'
_REQS = '"$GITHUB_WORKSPACE/.github/ci/requirements.txt"'
_PINNED_SHA = "0123456789abcdef0123456789abcdef01234567"
_DIGEST = "sha256:" + "ab" * 32


class TestPinnedInputsAndEnvFileWrites(unittest.TestCase):
    """Check 7: CI inputs pinned by content, env-file writes only through the helper."""

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.fx = SccacheFixture(self._tmpdir.name)

    def assertRefused(self, *needles: str) -> list[str]:
        errors = self.fx.errors()
        self.assertTrue(any(all(n in e for n in needles) for e in errors), errors)
        return errors

    def ci(self, job_body: str, **kw: str) -> None:
        self.fx.workflow("ci.yml", _ci(job_body, **kw))

    def allowlist(self, content: str) -> None:
        _write(os.path.join(self.fx.root, "ci", "github-env-allowlist.txt"), content)

    # ---- env-file writes: every bypass of a name-matching scan ----------

    def test_env_file_write_bypasses_are_refused(self) -> None:
        cases = {
            "adjacent-quote concat": 'echo "RUSTC_""WRAPPER=x" >> "$GITHUB_ENV"',
            "variable indirection": 'W=RUSTC_WRAPPER; echo "$W=x" >> "$GITHUB_ENV"',
            "base64 payload": "echo UlVTVENfV1JBUFBFUj14 | base64 -d >> $GITHUB_ENV",
            "path file": 'echo /tmp/evil >> "$GITHUB_PATH"',
            "lower-case name": 'echo "A=b" >> "$github_env"',
            "braced name": 'echo "A=b" >> "${GITHUB_ENV}"',
            "runner command file": "echo A=b >> /home/runner/work/_temp/_runner_file_commands/set_env_1",
            "legacy set-env": 'echo "::set-env name=A::b"',
            "legacy add-path": 'echo "::add-path::/tmp/evil"',
        }
        for name, run in cases.items():
            with self.subTest(name):
                self.ci(f"steps:\n  - name: W\n    run: {run!r}\n")
                self.assertRefused("step 'W' run:", "written only through")

    def test_env_file_benign_allowlisted_key_value_is_still_refused_raw(self) -> None:
        self.ci("steps:\n  - name: W\n    run: echo \"CI_JOB_BIN_NAME=ipe\" >> \"$GITHUB_ENV\"\n")
        self.assertRefused("step 'W' run:", "'GITHUB_ENV'")

    def test_legacy_command_switch_in_env_is_refused(self) -> None:
        self.ci("env:\n  ACTIONS_ALLOW_UNSECURE_COMMANDS: 'true'\nsteps:\n  - run: cargo build\n")
        self.assertRefused("env.ACTIONS_ALLOW_UNSECURE_COMMANDS", "written only through")

    def test_env_file_name_in_with_and_shell_is_refused(self) -> None:
        self.ci(f"steps:\n  - uses: some/action@{_PINNED_SHA}\n    with:\n      target: $GITHUB_ENV\n")
        self.assertRefused("with.target", "written only through")
        self.ci("steps:\n  - name: S\n    shell: bash --rcfile $GITHUB_ENV {0}\n    run: x\n")
        self.assertRefused("step 'S' shell:", "written only through")

    def test_env_file_write_inside_local_composite_is_refused(self) -> None:
        self.fx.composite(
            "w", "runs:\n  using: composite\n  steps:\n    - shell: bash\n      run: echo A=b >> $GITHUB_ENV\n"
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml", "written only through")

    # ---- runner command files: every spelling, every string scalar -------

    def test_runner_file_spellings_are_refused(self) -> None:
        cases = {
            "github.env expression": "echo x >> ${{ github.env }}",
            "github.path expression": "echo /tmp >> ${{ github.path }}",
            "upper-case context": "echo x >> ${{ GITHUB.ENV }}",
            "spaced access": "echo x >> ${{ github . env }}",
            "bracket index": "echo x >> ${{ github['env'] }}",
            "assembled index": "echo x >> ${{ github[format('{0}', 'env')] }}",
            "whole context": "echo '${{ toJSON(github) }}'",
            "filter": "echo '${{ github.* }}'",
            "whole env context": "echo '${{ toJSON(env) }}'",
            "env context name": "echo x >> ${{ env.GITHUB_ENV }}",
            "state file": 'echo a=b >> "$GITHUB_STATE"',
            "state property": "echo a=b >> ${{ github.state }}",
            "output property": "echo a=b >> ${{ github.output }}",
            "summary property": "echo a=b >> ${{ github.step_summary }}",
            "output rewritten": 'echo a=b >> "${GITHUB_OUTPUT/OUTPUT/ENV}"',
            "output reassigned": 'GITHUB_OUTPUT=$GITHUB_ENV; echo a=b >> "$GITHUB_OUTPUT"',
            "output overwritten": 'echo a=b > "$GITHUB_OUTPUT"',
            "output suffix": 'echo a=b >> "$GITHUB_OUTPUTX"',
            "lower-case output": 'echo a=b >> "$github_output"',
            "pwsh output other param": "Set-Content -Path $env:GITHUB_OUTPUT a=b",
            "legacy save-state": 'echo "::save-state name=a::b"',
            "legacy set-output": 'echo "::set-output name=a::b"',
            "state command file": "echo a >> /x/_temp/_runner_file_commands/save_state_1",
        }
        for name, run in cases.items():
            with self.subTest(name):
                self.ci(f"steps:\n  - name: W\n    run: {run!r}\n")
                self.assertRefused("step 'W' run:", "written only")

    def test_runner_file_spelling_in_if_is_refused(self) -> None:
        for cond in ("github.env != ''", "${{ github.path }}", "GITHUB['ENV']", "toJSON(github)"):
            with self.subTest(cond=cond):
                self.ci(f"steps:\n  - name: W\n    if: {cond!r}\n    run: x\n")
                self.assertRefused("step 'W' if:", "written only")

    def test_runner_file_spelling_in_any_string_scalar_is_refused(self) -> None:
        for name, body, needle in (
            ("step name", "steps:\n  - name: '${{ github.env }}'\n    run: x\n", "name:"),
            ("nested with", f"steps:\n  - uses: a/b@{_PINNED_SHA}\n    with:\n      c: ${{{{ github.path }}}}\n",
             "with.c"),
            ("with key", f"steps:\n  - uses: a/b@{_PINNED_SHA}\n    with:\n      GITHUB_ENV: x\n", "with.GITHUB_ENV"),
            ("step env key", "steps:\n  - env:\n      GITHUB_OUTPUT: /x\n    run: x\n", "env.GITHUB_OUTPUT"),
            ("step env value", "steps:\n  - env:\n      A: ${{ github.env }}\n    run: x\n", "env.A"),
            ("job matrix", "strategy:\n  matrix:\n    f: ['${{ github.env }}']\nsteps:\n  - run: x\n",
             "strategy.matrix.f[0]"),
            ("job env key", "env:\n  GITHUB_PATH: /x\nsteps:\n  - run: x\n", "env.GITHUB_PATH"),
            ("job if", "if: github.env\nsteps:\n  - run: x\n", "if:"),
        ):
            with self.subTest(name):
                self.ci(body)
                self.assertRefused(needle, "written only")

    def test_runner_file_spelling_at_workflow_level_is_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            "name: ci\non:\n  workflow_dispatch:\n    inputs:\n      t:\n        default: '${{ github.env }}'\n"
            "jobs:\n  clippy:\n    runs-on: ubuntu-latest\n    steps:\n      - run: x\n",
        )
        self.assertRefused("ci.yml: workflow-level", "written only")
        self.ci("steps:\n  - run: x\n", top="env:\n  GITHUB_ENV: /x\n")
        self.assertRefused("ci.yml: workflow-level", "env.GITHUB_ENV", "written only")

    def test_runner_file_spelling_in_local_action_metadata_is_refused(self) -> None:
        self.fx.composite(
            "w",
            "inputs:\n  t:\n    default: '${{ github.env }}'\n"
            "runs:\n  using: composite\n  steps:\n    - shell: bash\n      run: echo ok\n",
        )
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml", "inputs.t.default", "written only")

    def test_helper_call_outside_a_step_run_is_refused(self) -> None:
        call = f"{_HELPER_CALL} CI_JOB_BIN_NAME x"
        for name, body in (
            ("with", f"steps:\n  - uses: a/b@{_PINNED_SHA}\n    with:\n      c: {call!r}\n"),
            ("step env", f"steps:\n  - env:\n      A: {call!r}\n    run: x\n"),
            ("job env", f"env:\n  A: {call!r}\nsteps:\n  - run: x\n"),
            ("step name", f"steps:\n  - name: {call!r}\n    run: x\n"),
        ):
            with self.subTest(name):
                self.ci(body)
                self.assertRefused("outside its one canonical call")

    def test_sanctioned_runner_file_and_context_uses_pass(self) -> None:
        self.ci(
            "steps:\n"
            "  - name: A\n    run: echo \"a=b\" >> \"$GITHUB_OUTPUT\"\n"
            "  - name: B\n    run: echo x >> \"${GITHUB_STEP_SUMMARY}\"\n"
            "  - name: C\n    run: echo x >> $GITHUB_STEP_SUMMARY\n"
            "  - name: D\n    shell: pwsh\n    run: '\"a=b\" | Out-File -FilePath $env:GITHUB_OUTPUT -Append'\n"
            "  - name: E\n    run: cd \"${{ github.workspace }}\" && echo \"${{ env.CI_JOB_BIN_NAME }}\"\n"
            "  - name: F\n    if: github.actor != 'github-actions' && github.event_name != 'env'\n    run: x\n"
            "  - name: G\n    run: ls \"$GITHUB_WORKSPACE\" \"${GITHUB_WORKSPACE}/x\"\n"
        )
        self.assertEqual(self.fx.errors(), [])

    def test_github_workspace_write_is_refused(self) -> None:
        for name, body in (
            ("assignment", "steps:\n  - name: W\n    run: GITHUB_WORKSPACE=/x\n"),
            ("reference assignment", "steps:\n  - name: W\n    run: '$GITHUB_WORKSPACE=/x'\n"),
            ("export", "steps:\n  - name: W\n    run: export GITHUB_WORKSPACE\n"),
            ("default expansion", "steps:\n  - name: W\n    run: 'echo ${GITHUB_WORKSPACE:=/x}'\n"),
            ("lower case", "steps:\n  - name: W\n    run: github_workspace=/x\n"),
            ("pwsh", "steps:\n  - name: W\n    shell: pwsh\n    run: \"$env:GITHUB_WORKSPACE = 'x'\"\n"),
            ("step env key", "steps:\n  - name: W\n    env:\n      GITHUB_WORKSPACE: /x\n    run: x\n"),
            ("job env key", "env:\n  GITHUB_WORKSPACE: /x\nsteps:\n  - name: W\n    run: x\n"),
        ):
            with self.subTest(name):
                self.ci(body)
                self.assertRefused("other than as a plain read")

    def test_string_scalar_nesting_past_the_limit_is_refused(self) -> None:
        errors: list[str] = []
        deep: object = "x"
        for _ in range(verify_manifest.STRING_SCALAR_DEPTH_LIMIT + 1):
            deep = [deep]
        verify_manifest._string_scalars({"k": deep}, "loc", errors)
        self.assertTrue(any("nests deeper than" in e for e in errors), errors)
        errors.clear()
        shallow: object = "x"
        for _ in range(verify_manifest.STRING_SCALAR_DEPTH_LIMIT - 2):
            shallow = [shallow]
        self.assertIn(("k" + "[0]" * (verify_manifest.STRING_SCALAR_DEPTH_LIMIT - 2), "x", False),
                      verify_manifest._string_scalars({"k": shallow}, "loc", errors))
        self.assertEqual(errors, [])

    # ---- the helper: one canonical call, bare allowlisted key ------------

    def test_canonical_helper_call_with_allowlisted_key_passes(self) -> None:
        self.ci(f"steps:\n  - name: W\n    run: 'X=ipe; {_HELPER_CALL} CI_JOB_BIN_NAME \"$X\"'\n")
        self.assertEqual(self.fx.errors(), [])

    def test_helper_call_with_non_allowlisted_key_is_refused(self) -> None:
        self.ci(f"steps:\n  - name: W\n    run: '{_HELPER_CALL} OTHER_KEY x'\n")
        self.assertRefused("step 'W' run:", "'OTHER_KEY'", "not in ci/github-env-allowlist.txt")

    def test_helper_call_outside_canonical_form_is_refused(self) -> None:
        for name, run in {
            "variable key": f'W=CI_JOB_BIN_NAME; {_HELPER_CALL} "$W" x',
            "quoted key": f'{_HELPER_CALL} "CI_JOB_BIN_NAME" x',
            "concatenated key": f"{_HELPER_CALL} CI_JOB_BIN_\"\"NAME x",
            "relative path": "bash .github/ci/github-env.sh CI_JOB_BIN_NAME x",
            "sourced": 'source "$GITHUB_WORKSPACE/.github/ci/github-env.sh" CI_JOB_BIN_NAME x',
            "other interpreter": 'sh "$GITHUB_WORKSPACE/.github/ci/github-env.sh" CI_JOB_BIN_NAME x',
            "copied helper": 'cp .github/ci/github-env.sh /tmp/w.sh',
            "no value": f"{_HELPER_CALL} CI_JOB_BIN_NAME ",
        }.items():
            with self.subTest(name):
                self.ci(f"steps:\n  - name: W\n    run: {run!r}\n")
                self.assertRefused("step 'W' run:", "outside its one canonical call")

    # ---- the allowlist itself --------------------------------------------

    def test_allowlist_refuses_dangerous_keys(self) -> None:
        for key in ("PATH", "RUSTC_WRAPPER", "SCCACHE_DIR", "LD_PRELOAD", "BASH_ENV", "NODE_OPTIONS",
                    "GITHUB_TOKEN", "RUNNER_TEMP", "CARGO_HOME", "lower_case", "A-B"):
            with self.subTest(key=key):
                self.allowlist(f"# header\nCI_JOB_BIN_NAME\n{key}\n")
                self.ci("steps:\n  - run: cargo build\n")
                self.assertRefused("github-env-allowlist.txt:3:", repr(key))

    def test_allowlist_refuses_keys_outside_the_job_shape(self) -> None:
        for key in ("BIN_NAME", "CI_JOB_", "CI_JOB_lower", "ci_job_x", "CI_JOBX", "X_CI_JOB_Y", "CI_JOB_A-B"):
            with self.subTest(key=key):
                self.allowlist(f"CI_JOB_BIN_NAME\n{key}\n")
                self.ci("steps:\n  - run: cargo build\n")
                self.assertRefused("github-env-allowlist.txt:2:", repr(key))

    def test_allowlist_duplicate_key_is_refused(self) -> None:
        self.allowlist("CI_JOB_BIN_NAME\nCI_JOB_BIN_NAME\n")
        self.ci("steps:\n  - run: cargo build\n")
        self.assertRefused("github-env-allowlist.txt:2:", "listed twice")

    def test_missing_allowlist_fails_closed(self) -> None:
        os.remove(os.path.join(self.fx.root, "ci", "github-env-allowlist.txt"))
        self.ci(f"steps:\n  - name: W\n    run: '{_HELPER_CALL} CI_JOB_BIN_NAME x'\n")
        errors = self.assertRefused("github-env-allowlist.txt", "cannot be read")
        self.assertTrue(any("'CI_JOB_BIN_NAME'" in e and "not in" in e for e in errors), errors)

    # ---- `uses:` pinned by content ---------------------------------------

    def test_unpinned_uses_forms_are_refused(self) -> None:
        for uses in (
            "actions/checkout@v7",
            "actions/checkout@main",
            "actions/checkout@3d3c42e",
            "actions/checkout@3D3C42E5AAC5BA805825DA76410C181273BA90B1",
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1x",
            "actions/checkout",
            "github/codeql-action/init@v3",
            "docker://alpine:3.20",
            "docker://alpine",
        ):
            with self.subTest(uses=uses):
                self.ci(f"steps:\n  - uses: {uses}\n")
                self.assertRefused(repr(uses), "not pinned by content")

    def test_pinned_uses_forms_pass(self) -> None:
        self.ci(
            f"steps:\n  - uses: actions/checkout@{_PINNED_SHA} # v7\n"
            f"  - uses: github/codeql-action/init@{_PINNED_SHA}\n"
            f"  - uses: docker://alpine@{_DIGEST}\n"
        )
        self.assertEqual(self.fx.errors(), [])

    def test_non_string_uses_is_refused(self) -> None:
        self.ci("steps:\n  - uses: [actions/checkout]\n")
        self.assertRefused("uses:", "a string")

    def test_unpinned_uses_inside_local_composite_is_refused(self) -> None:
        self.fx.composite("w", "runs:\n  using: composite\n  steps:\n    - uses: actions/checkout@v7\n")
        self.ci("steps:\n  - uses: ./.github/actions/w\n")
        self.assertRefused("./.github/actions/w/action.yml", "not pinned by content")

    def test_sanctioned_composite_tag_pinned_install_is_refused(self) -> None:
        _write(
            os.path.join(self.fx.root, "actions", "sccache", "action.yml"),
            VALID_COMPOSITE.replace(
                "sccache-action@7d986dd989559c6ecdb630a3fd2557667be217ad", "sccache-action@v0.0.9"
            ),
        )
        self.ci("steps:\n  - run: cargo build\n")
        self.assertRefused("step 1 must be exactly")

    def test_unpinned_job_images_are_refused(self) -> None:
        for body, needle in (
            ("container: rust:1\n", "job 'clippy' container image 'rust:1'"),
            ("container:\n  image: rust:1\n", "job 'clippy' container image 'rust:1'"),
            ("services:\n  db:\n    image: postgres:16\n", "service 'db' image 'postgres:16'"),
        ):
            with self.subTest(body=body):
                self.ci(body + "steps:\n  - run: cargo build\n")
                self.assertRefused(needle, "not pinned by sha256 digest")

    def test_digest_pinned_job_images_pass(self) -> None:
        self.ci(
            f"container:\n  image: rust@{_DIGEST}\n"
            f"services:\n  db:\n    image: postgres@{_DIGEST}\n"
            "steps:\n  - run: cargo build\n"
        )
        self.assertEqual(self.fx.errors(), [])

    # ---- pip: only the hash-checked shape --------------------------------

    def test_unhashed_pip_installs_are_refused(self) -> None:
        for run in (
            "pip install pyyaml",
            "python3 -m pip install --quiet pyyaml==6.0.3",
            "pip install -r .github/ci/requirements.txt",
            "pip install --require-hashes -r .github/ci/requirements.txt",
            "pip install --require-hashes --only-binary :all: -r r.txt pyyaml",
            "pip install --require-hashes --only-binary :all: --index-url https://x -r r.txt",
            "pip install --require-hashes --only-binary :all: -r r.txt --no-deps -e .",
            "PIP3 install pyyaml",
            "/usr/bin/pip3.12 install pyyaml",
            "cd x && pip install pyyaml",
            "pip3 --quiet install pyyaml",
            "pipx install pyyaml",
            "easy_install pyyaml",
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertRefused("step 'P' run:")

    def test_pip_install_across_line_continuation_is_refused(self) -> None:
        self.ci("steps:\n  - name: P\n    run: |\n      pip \\\n        install pyyaml\n")
        self.assertRefused("step 'P' run:", "outside the hash-checked shape")

    def test_pip_install_outside_the_one_requirements_file_is_refused(self) -> None:
        base = "pip install --require-hashes --only-binary :all:"
        for name, (run, needle) in {
            "same file twice": (f"{base} -r {_REQS} -r {_REQS}", "2 times, not exactly once"),
            "second file": (f"{base} -r {_REQS} -r other.txt", "'other.txt' is not"),
            "other file": (f"{base} -r r.txt", "'r.txt' is not"),
            "relative canonical file": (f"{base} -r .github/ci/requirements.txt", "is not"),
            "bare package": (f"{base} -r {_REQS} pyyaml", "'pyyaml' is outside"),
            "editable": (f"{base} -r {_REQS} -e .", "'-e' is outside"),
            "url": (f"{base} -r {_REQS} https://x/p.whl", "'https://x/p.whl' is outside"),
            "url requirements": (f"{base} -r https://x/r.txt", "'https://x/r.txt' is not"),
            "no requirements": (base, "lacks"),
        }.items():
            with self.subTest(name):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertRefused("step 'P' run:", needle)

    def test_every_pip_spelling_is_seen(self) -> None:
        for run in (
            "python3 -m pip install pyyaml",
            "python3 -mpip install pyyaml",
            f"python3 -mpip install --require-hashes --only-binary :all: -r {_REQS}",
            "python3 -m  pip install pyyaml",
            "pip3.12 install pyyaml",
            "pipx run pyyaml",
            "easy_install pyyaml",
            "EASY_INSTALL pyyaml",
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertRefused("step 'P' run:")

    def test_canonical_hashed_pip_install_passes(self) -> None:
        for run in (
            f"python3 -m pip install --quiet --require-hashes --only-binary :all: -r {_REQS}",
            f"pip install --only-binary=:all: --require-hashes --requirement {_REQS}",
            "pip --version",
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertEqual(self.fx.errors(), [])


class TestGithubEnvHelper(unittest.TestCase):
    """The helper re-checks its own contract at run time (defence in depth)."""

    HELPER = os.path.join(HERE, "github-env.sh")

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.env_file = os.path.join(self._tmpdir.name, "env")
        open(self.env_file, "w").close()

    def run_helper(self, *args: str) -> int:
        import subprocess

        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "GITHUB_ENV": self.env_file}
        return subprocess.run(["bash", self.HELPER, *args], env=env, capture_output=True, check=False).returncode

    def written(self) -> str:
        with open(self.env_file) as f:
            return f.read()

    def test_allowlisted_key_is_written(self) -> None:
        self.assertEqual(self.run_helper("CI_JOB_BIN_NAME", "ipe"), 0)
        self.assertEqual(self.written(), "CI_JOB_BIN_NAME=ipe\n")

    def test_refusals_write_nothing(self) -> None:
        for args in (
            ("OTHER_KEY", "x"),
            ("CI_JOB_OTHER", "x"),
            ("CI_JOB_", "x"),
            ("RUSTC_WRAPPER", "sccache"),
            ("ci_job_bin_name", "x"),
            ("BIN_NAME", "x"),
            ("# Keys", "x"),
            ("CI_JOB_BIN_NAME", "ipe\nRUSTC_WRAPPER=evil"),
            ("CI_JOB_BIN_NAME", "ipe\r"),
            ("CI_JOB_BIN_NAME",),
            ("CI_JOB_BIN_NAME", "a", "b"),
        ):
            with self.subTest(args=args):
                self.assertNotEqual(self.run_helper(*args), 0)
                self.assertEqual(self.written(), "")


if __name__ == "__main__":
    unittest.main()
