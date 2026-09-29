#!/usr/bin/env python3
"""Refusal proofs for `rerun_policy.py`, the infra-only automatic-rerun SSOT.

Every way the policy must refuse a rerun (a test failure, an unknown red, a
test failure beside an infra signature, an unreadable job, no failed job, a
malformed job listing) and every way `lint` must refuse (a fail-open signature,
a configured test retry, a rerun step not gated on the exact verdict) gets its
own test, per PRINCIPLES.md "Prove the refusals". Pure stdlib `unittest`.
"""

from __future__ import annotations

import contextlib
import copy
import importlib.util
import io
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

_spec = importlib.util.spec_from_file_location("rerun_policy", os.path.join(HERE, "rerun_policy.py"))
assert _spec is not None and _spec.loader is not None
rp = importlib.util.module_from_spec(_spec)
sys.modules["rerun_policy"] = rp
_spec.loader.exec_module(rp)

TS = "2026-09-29T04:12:33.1234567Z "
TEST_FAIL_LOG = [
    TS + "        PASS [   1.204s] ipe::emit ok_case",
    TS + "        FAIL [  12.881s] ipe::emit broken_case",
    TS + "error: test run failed",
    TS + "##[error]Process completed with exit code 100.",
]
ENOSPC_LOG = [
    TS + "   Compiling tokio v1.47.0",
    TS + "error: failed to write /home/runner/work/target/debug/deps/libtokio.rlib: No space left on device (os error 28)",
    TS + "##[error]Process completed with exit code 101.",
]


def ev(infra: set[str] | None = None, test_failure: bool = False) -> "rp.Evidence":
    return rp.Evidence(frozenset(infra or set()), test_failure)


class ScanTest(unittest.TestCase):
    def test_planted_test_failure_is_not_infra(self) -> None:
        e = rp.scan(TEST_FAIL_LOG)
        self.assertTrue(e.test_failure)
        self.assertFalse(e.infra_only)

    def test_simulated_enospc_is_infra(self) -> None:
        e = rp.scan(ENOSPC_LOG)
        self.assertEqual(e.infra, frozenset({"enospc"}))
        self.assertTrue(e.infra_only)

    def test_every_signature_family_matches_its_sample(self) -> None:
        samples = {
            "lost-runner": "The runner has received a shutdown signal. This can happen when the runner service is stopped",
            "enospc": "Error: ENOSPC: no space left on device, write",
            "sccache-5xx": "sccache: error: Server startup failed: cache storage failed to read: Unexpected, response: Parts { status: 503 }",
            "cache-5xx": "Warning: Failed to save: Cache service responded with 502",
            "download-5xx": "curl: (22) The requested URL returned error: 503",
            "download-network": "curl: (6) Could not resolve host: github.com",
        }
        for sid, line in samples.items():
            with self.subTest(sid):
                self.assertIn(sid, rp.scan([TS + line]).infra)

    def test_lost_communication_annotation_is_infra(self) -> None:
        line = "The hosted runner: GitHub Actions 12 lost communication with the server."
        self.assertEqual(rp.scan([line]).infra, frozenset({"lost-runner"}))

    def test_4xx_download_is_not_infra(self) -> None:
        for line in (
            "curl: (22) The requested URL returned error: 404",
            "failed to get successful HTTP response from `https://index.crates.io/x`, got 404",
            "Unexpected HTTP response: 403",
        ):
            with self.subTest(line):
                self.assertFalse(rp.scan([line]).infra)

    def test_lint_and_compile_reds_are_not_infra(self) -> None:
        log = [
            TS + "error: this `if` has identical blocks",
            TS + "error: could not compile `ipe` (lib) due to 1 previous error",
            TS + "Diff in src/lib.rs at line 3:",
        ]
        self.assertFalse(rp.scan(log).infra)

    def test_test_marker_beside_infra_vetoes(self) -> None:
        e = rp.scan([*ENOSPC_LOG, *TEST_FAIL_LOG])
        self.assertTrue(e.infra)
        self.assertFalse(e.infra_only)

    def test_every_test_marker_vetoes(self) -> None:
        for line in (
            "     TIMEOUT [ 900.001s] ipe::server_e2e hangs",
            "     SIGSEGV [   0.4s] ipe::runtime crash",
            "   SIGABRT [   2s] x y",
            "test result: FAILED. 3 passed; 1 failed",
            "       FLAKY 2/2 [   3.1s] ipe::x y",
        ):
            with self.subTest(line):
                self.assertTrue(rp.scan([TS + line]).test_failure)

    def test_over_long_log_refused(self) -> None:
        old = rp.MAX_LOG_LINES
        rp.MAX_LOG_LINES = 3
        try:
            with self.assertRaises(rp.PolicyError):
                rp.scan(["a", "b", "c", "d"])
        finally:
            rp.MAX_LOG_LINES = old

    def test_coloured_test_failure_vetoes(self) -> None:
        # ci.yml sets CARGO_TERM_COLOR=always: nextest wraps its status words
        # in SGR codes, which must not hide the marker beside an infra line.
        for line in (
            "\x1b[31;1m        FAIL\x1b[0m [  12.881s] ipe::emit broken_case",
            "\x1b[1m\x1b[31m     TIMEOUT\x1b[0m [ 900.001s] ipe::x y",
            "\x1b[31;1merror\x1b[0m: test run failed",
            "\x1b]8;;https://x\x1b\\       FLAKY\x1b]8;;\x07 2/2 [   3.1s] ipe::x y",
        ):
            with self.subTest(line):
                e = rp.scan([*ENOSPC_LOG, TS + line])
                self.assertTrue(e.test_failure)
                self.assertFalse(e.infra_only)

    def test_control_sequence_strip_is_linear(self) -> None:
        import time

        start = time.monotonic()
        rp.scan(["\x1b]" * 2000 + "\x1b[" + "1;" * 2000] * 50)
        self.assertLess(time.monotonic() - start, 5.0)

    def test_recovered_retry_is_not_infra(self) -> None:
        # A retry cargo recovered from is not why the job failed; the later
        # lint red must stay red.
        log = [
            TS + "warning: spurious network error (2 tries remaining): [6] Could not resolve host: index.crates.io",
            TS + "Retrying in 3 seconds: curl: (56) Recv failure",
            TS + "error: this `if` has identical blocks",
        ]
        self.assertFalse(rp.scan(log).infra)
        self.assertFalse(rp.decide([rp.scan(log)]))

    def test_unrecovered_network_error_is_infra(self) -> None:
        line = TS + "  [6] Could not resolve host: index.crates.io"
        self.assertEqual(rp.scan([line]).infra, frozenset({"download-network"}))

    def test_signature_past_the_line_cap_is_ignored(self) -> None:
        line = "x" * rp.MAX_LINE_CHARS + "No space left on device"
        self.assertFalse(rp.scan([line]).infra)


class BoundedLinesTest(unittest.TestCase):
    def test_long_line_is_read_in_capped_pieces(self) -> None:
        text = "x" * (rp.MAX_LINE_CHARS * 3 + 5) + "\nshort\n"
        pieces = list(rp.bounded_lines(io.StringIO(text)))
        self.assertTrue(all(len(p) <= rp.MAX_LINE_CHARS for p in pieces))
        self.assertEqual("".join(pieces), text)

    def test_marker_past_a_long_prefix_still_vetoes(self) -> None:
        text = "y" * (rp.MAX_LINE_CHARS * 2) + " No space left on device\n        FAIL [ 1.0s] a b\n"
        self.assertTrue(rp.scan(rp.bounded_lines(io.StringIO(text))).test_failure)

    def test_over_long_log_in_characters_refused(self) -> None:
        old = rp.MAX_LOG_CHARS
        rp.MAX_LOG_CHARS = 10
        try:
            with self.assertRaises(rp.PolicyError):
                list(rp.bounded_lines(io.StringIO("abcdef\nghijkl\n")))
        finally:
            rp.MAX_LOG_CHARS = old


class DecideTest(unittest.TestCase):
    def test_all_infra_reruns(self) -> None:
        self.assertTrue(rp.decide([ev({"enospc"}), ev({"lost-runner"})]))

    def test_no_failed_job_refused(self) -> None:
        self.assertFalse(rp.decide([]))

    def test_one_non_infra_job_blocks(self) -> None:
        self.assertFalse(rp.decide([ev({"enospc"}), ev()]))

    def test_one_test_failure_blocks(self) -> None:
        self.assertFalse(rp.decide([ev({"enospc"}), ev({"enospc"}, test_failure=True)]))

    def test_unreadable_job_blocks(self) -> None:
        self.assertFalse(rp.decide([ev({"enospc"}), None]))

    def test_too_many_jobs_refused(self) -> None:
        self.assertFalse(rp.decide([ev({"enospc"})] * (rp.MAX_FAILED_JOBS + 1)))


class JobIdsTest(unittest.TestCase):
    def test_ids_parse(self) -> None:
        self.assertEqual(rp.parse_job_ids("12\n34\n"), [12, 34])
        self.assertEqual(rp.parse_job_ids(""), [])

    def test_malformed_ids_refused(self) -> None:
        for bad in ("12\nabc", "012", " 12", "12 ", "-1", "0", "1.0", "{}", "٣"):
            with self.subTest(bad), self.assertRaises(rp.PolicyError):
                rp.parse_job_ids(bad)

    def test_every_unpassed_conclusion_needs_proof(self) -> None:
        listing = "1\tsuccess\n2\tfailure\n3\ttimed_out\n4\tcancelled\n5\tskipped\n6\tnull\n7\tneutral\n8\tstartup_failure"
        self.assertEqual(rp.failed_job_ids(listing), [2, 3, 4, 6, 8])

    def test_malformed_job_listing_refused(self) -> None:
        for bad in ("12", "12\t", "x\tfailure", "12\tFailure", "12\tfailure\n12\tsuccess", "\tfailure", "12\tfail ure"):
            with self.subTest(bad), self.assertRaises(rp.PolicyError):
                rp.failed_job_ids(bad)

    def test_jobs_jq_emits_id_and_conclusion(self) -> None:
        self.assertEqual(rp.JOBS_JQ, '.jobs[] | "\\(.id)\\t\\(.conclusion)"')

    def test_repeated_id_refused(self) -> None:
        with self.assertRaises(rp.PolicyError):
            rp.parse_job_ids("7\n7")

    def test_bad_repo_or_run_refused(self) -> None:
        for repo, run in (("", "1"), ("a/b; rm", "1"), ("a/b", ""), ("a/b", "1x"), ("a/b/c", "1")):
            with self.subTest((repo, run)), self.assertRaises(rp.PolicyError):
                rp.run_decide(repo, run)

    def test_decide_prints_false_without_gh_context(self) -> None:
        old = {k: os.environ.pop(k, None) for k in ("REPO", "RUN_ID")}
        out = io.StringIO()
        try:
            with contextlib.redirect_stdout(out):
                self.assertEqual(rp.main(["decide"]), 0)
            self.assertEqual(out.getvalue(), "false\n")
        finally:
            for k, v in old.items():
                if v is not None:
                    os.environ[k] = v

    def test_unknown_subcommand_exits_nonzero(self) -> None:
        self.assertEqual(rp.main([]), 2)
        self.assertEqual(rp.main(["decide", "x"]), 2)


class SignatureTableTest(unittest.TestCase):
    def test_repo_tables_pass(self) -> None:
        self.assertEqual(rp.signature_errors(rp.SIGNATURES, rp.TEST_FAILURE_MARKERS), [])

    def test_match_everything_signature_refused(self) -> None:
        for pat in (".*", "error", r"\s*", "FAIL"):
            with self.subTest(pat):
                errs = rp.signature_errors((("x", pat),), rp.TEST_FAILURE_MARKERS)
                self.assertTrue(any("generic" in e for e in errs), errs)

    def test_bad_patterns_refused(self) -> None:
        for pat in ("", " ENOSPC", "(", "a.*b.*c"):
            with self.subTest(pat):
                self.assertTrue(rp.signature_errors((("x", pat),), rp.TEST_FAILURE_MARKERS))

    def test_empty_or_repeated_tables_refused(self) -> None:
        self.assertTrue(rp.signature_errors((), rp.TEST_FAILURE_MARKERS))
        self.assertTrue(rp.signature_errors(rp.SIGNATURES, ()))
        self.assertTrue(rp.signature_errors((("a", "ENOSPC"), ("b", "ENOSPC")), rp.TEST_FAILURE_MARKERS))

    def test_bad_id_refused(self) -> None:
        for sid in ("", "Bad", "a_b", "-a"):
            with self.subTest(sid):
                self.assertTrue(rp.signature_errors(((sid, "ENOSPC"),), rp.TEST_FAILURE_MARKERS))


def _tree(nextest: str | None, workflows: dict[str, str]) -> tempfile.TemporaryDirectory:
    tmp = tempfile.TemporaryDirectory()
    root = tmp.name
    os.makedirs(os.path.join(root, ".config"))
    os.makedirs(os.path.join(root, ".github", "workflows"))
    os.makedirs(os.path.join(root, ".github", "actions", "x"))
    if nextest is not None:
        with open(os.path.join(root, ".config", "nextest.toml"), "w", encoding="utf-8") as fh:
            fh.write(nextest)
    for rel, body in workflows.items():
        with open(os.path.join(root, ".github", rel), "w", encoding="utf-8") as fh:
            fh.write(body)
    return tmp


class RetryBanTest(unittest.TestCase):
    def test_repo_has_no_retry(self) -> None:
        self.assertEqual(rp.retry_errors(), [])

    def test_clean_tree_passes(self) -> None:
        with _tree("# retries are banned\n[profile.ci]\nfail-fast = false\n", {"workflows/a.yml": "x: 1\n"}) as root:
            self.assertEqual(rp.retry_errors(root), [])

    def test_nextest_retries_refused(self) -> None:
        for body in ("retries = 1\n", "[profile.ci]\n  retries = { backoff = 'fixed', count = 2 }\n",
                     "[[profile.ci.overrides]]\nfilter = 'all()'\nretries = 1 # one\n"):
            with self.subTest(body), _tree(body, {}) as root:
                self.assertTrue(any("retries" in e for e in rp.retry_errors(root)))

    def test_quoted_or_hidden_nextest_retries_refused(self) -> None:
        for body in ('"retries" = 1\n', "[profile.ci]\n'retries'=2\n", "profile.ci.retries = 1\n",
                     'x = { filter = "#", retries = 1 }\n'):
            with self.subTest(body), _tree(body, {}) as root:
                self.assertTrue(any("retries" in e for e in rp.retry_errors(root)))

    def test_workflow_retry_after_hash_refused(self) -> None:
        with _tree("", {"workflows/a.yml": 'run: echo "#" && cargo nextest run --retries 2\n'}) as root:
            self.assertTrue(any("retry" in e for e in rp.retry_errors(root)))

    def test_workflow_and_action_retries_refused(self) -> None:
        for rel, body in (
            ("workflows/a.yml", "run: cargo nextest run --retries 2\n"),
            ("workflows/b.yaml", "env:\n  NEXTEST_RETRIES: 1\n"),
            ("actions/x/action.yml", "run: cargo nextest run --flaky-result pass\n"),
        ):
            with self.subTest(rel), _tree("", {rel: body}) as root:
                self.assertTrue(any("retry" in e for e in rp.retry_errors(root)))

    def test_missing_nextest_config_refused(self) -> None:
        with _tree(None, {}) as root:
            self.assertTrue(any("unreadable" in e for e in rp.retry_errors(root)))


GATE = {"id": "gate", "run": rp.DECIDE_INVOCATION}
RERUN = {"if": "${{ steps.gate.outputs.rerun == 'true' }}", "run": 'gh run rerun --failed "$RUN_ID"'}
WORKFLOW = {"jobs": {"rerun": {"steps": [{"uses": "actions/checkout@v7"}, GATE, RERUN]}}}


class WiringTest(unittest.TestCase):
    def test_wired_workflow_passes(self) -> None:
        self.assertEqual(rp.wiring_errors(WORKFLOW), [])

    def test_repo_workflow_passes(self) -> None:
        self.assertEqual(rp.lint(), 0)

    def test_fail_open_rerun_if_refused(self) -> None:
        for cond in (
            None,
            "${{ !cancelled() && steps.gate.outputs.rerun != 'false' }}",
            "always()",
            "${{ steps.gate.outputs.rerun == 'true' }} && x",
            "steps.gate.outputs.rerun == 'true' || true",
        ):
            with self.subTest(cond):
                wf = copy.deepcopy(WORKFLOW)
                step = wf["jobs"]["rerun"]["steps"][2]
                if cond is None:
                    del step["if"]
                else:
                    step["if"] = cond
                self.assertTrue(rp.wiring_errors(wf))

    def test_missing_or_duplicate_decide_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        wf["jobs"]["rerun"]["steps"].pop(1)
        self.assertTrue(rp.wiring_errors(wf))
        wf = copy.deepcopy(WORKFLOW)
        wf["jobs"]["rerun"]["steps"].insert(1, dict(GATE, id="other"))
        self.assertTrue(rp.wiring_errors(wf))

    def test_decide_without_gate_id_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        wf["jobs"]["rerun"]["steps"][1]["id"] = "verdict"
        self.assertTrue(rp.wiring_errors(wf))

    def test_continue_on_error_refused(self) -> None:
        for idx in (1, 2):
            with self.subTest(idx):
                wf = copy.deepcopy(WORKFLOW)
                wf["jobs"]["rerun"]["steps"][idx]["continue-on-error"] = True
                self.assertTrue(rp.wiring_errors(wf))

    def test_second_rerun_step_refused(self) -> None:
        wf = copy.deepcopy(WORKFLOW)
        wf["jobs"]["other"] = {"steps": [{"run": "gh run rerun 1"}]}
        self.assertTrue(rp.wiring_errors(wf))

    def test_no_jobs_refused(self) -> None:
        self.assertTrue(rp.wiring_errors({}))
        self.assertTrue(rp.wiring_errors({"jobs": {"a": {}}}))


if __name__ == "__main__":
    unittest.main()
