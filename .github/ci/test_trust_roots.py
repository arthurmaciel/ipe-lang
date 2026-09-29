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


def _pr(*, outside: bool = True, changed: int = 1, head: str = HEAD, state: str = "open") -> dict:
    return {
        "state": state,
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

    def test_fork_with_owner_approval_at_head_passes(self) -> None:
        reviews = [_review("OWNER", "CHANGES_REQUESTED", OTHER), _review("owner", "COMMENTED"), _review("owner", "APPROVED")]
        self.assertIn("approved", tr.decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_later_comment_keeps_approval(self) -> None:
        reviews = [_review("owner", "APPROVED"), _review("owner", "COMMENTED", OTHER)]
        self.assertIn("approved", tr.decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_fork_not_touching_trust_root_passes(self) -> None:
        self.assertIn("no trust root", tr.decide(self.roots, _pr(), [{"filename": "src/x.rs"}], []))

    def test_same_repo_branch_passes(self) -> None:
        out = tr.decide(self.roots, _pr(outside=False), [{"filename": "Cargo.toml"}], [])
        self.assertIn("branch of this repository", out)


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


class ApiGuards(unittest.TestCase):
    def test_non_https_base_is_refused(self) -> None:
        with self.assertRaises(tr.Refused):
            tr.Api("http://api.github.com", "o/r", "t")

    def test_malformed_repo_is_refused(self) -> None:
        with self.assertRaises(tr.Refused):
            tr.Api("https://api.github.com", "o/r/../x", "t")

    def test_off_origin_link_is_refused_before_any_request(self) -> None:
        api = tr.Api("https://api.github.com", "o/r", "t")
        with self.assertRaises(tr.Refused) as cm:
            api._get("https://evil.example/repos/o/r/pulls")
        self.assertIn("off the API origin", str(cm.exception))


if __name__ == "__main__":
    unittest.main()
