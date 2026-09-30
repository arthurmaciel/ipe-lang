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
        "commits": 1,
        "changed_files": changed,
        "head": {"sha": head, "repo": {"id": 2 if outside else 1}},
        "base": {"repo": {"id": 1}},
    }


_SAME = "<author>"


def _commit(author: str | None, committer: str | None = _SAME, sha: str = HEAD) -> dict:
    committer = author if committer == _SAME else committer
    return {
        "sha": sha,
        "author": None if author is None else {"login": author},
        "committer": None if committer is None else {"login": committer},
    }


def _decide(roots: tr.TrustRoots, pr: dict, files: list, reviews: list, commits: list | None = None) -> str:
    """`decide` with, by default, one commit the PR's own author made."""
    if commits is None:
        user = pr.get("user")
        commits = [_commit(user.get("login") if isinstance(user, dict) else None)]
    return tr.decide(roots, pr, files, reviews, commits)


def _review(login: str, state: str, commit: str = HEAD) -> dict:
    return {"user": {"login": login}, "state": state, "commit_id": commit}


ROOTS = "/.github/ @Owner\nCargo.toml @Owner\n"


class Decision(unittest.TestCase):
    def setUp(self) -> None:
        self.roots = _roots(ROOTS)

    def refused(self, pr: dict, files: list, reviews: list, needle: str) -> None:
        with self.assertRaises(tr.Refused) as cm:
            _decide(self.roots, pr, files, reviews)
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
        self.assertIn("approved", _decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_deleted_account_approval_does_not_count(self) -> None:
        reviews = [{"user": None, "state": "APPROVED", "commit_id": HEAD}]
        self.refused(_pr(), [{"filename": "Cargo.toml"}], reviews, "without")

    def test_fork_with_owner_approval_at_head_passes(self) -> None:
        reviews = [_review("OWNER", "CHANGES_REQUESTED", OTHER), _review("owner", "COMMENTED"), _review("owner", "APPROVED")]
        self.assertIn("approved", _decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_later_comment_keeps_approval(self) -> None:
        reviews = [_review("owner", "APPROVED"), _review("owner", "COMMENTED", OTHER)]
        self.assertIn("approved", _decide(self.roots, _pr(), [{"filename": "Cargo.toml"}], reviews))

    def test_fork_not_touching_trust_root_passes(self) -> None:
        self.assertIn("no trust root", _decide(self.roots, _pr(), [{"filename": "src/x.rs"}], []))

    def test_same_repo_branch_of_non_owner_fails(self) -> None:
        self.refused(_pr(outside=False), [{"filename": "Cargo.toml"}], [], "without a code owner")

    def test_same_repo_branch_of_owner_passes(self) -> None:
        out = _decide(self.roots, _pr(outside=False, login="Owner"), [{"filename": "Cargo.toml"}], [])
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

    def test_owner_pr_carrying_another_accounts_commit_fails(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["commits"] = 2
        for other in [
            _commit("someone"),
            _commit("owner", "someone"),
            _commit("web-flow"),
            _commit("renovate[bot]"),
            _commit(None, "owner"),
            _commit("owner", None),
        ]:
            self.refused_with(pr, [_commit("owner"), other])

    def test_owner_pr_with_partial_commit_list_fails(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["commits"] = 2
        self.refused_with(pr, [_commit("owner")])
        pr["commits"] = tr.MAX_PR_COMMITS + 1
        self.refused_with(pr, [_commit("owner")] * (tr.MAX_PR_COMMITS + 1))

    def test_owner_pr_with_github_side_committers_passes(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["commits"] = 2
        commits = [_commit("owner", "web-flow"), _commit("github-actions[bot]")]
        self.assertIn("code owner", _decide(self.roots, pr, [{"filename": "Cargo.toml"}], [], commits))

    def test_owner_pr_whose_last_commit_is_not_head_fails(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["commits"] = 2
        self.refused_with(pr, [_commit("owner"), _commit("owner", sha=OTHER)])
        self.refused_with(pr, [_commit("owner"), "not a commit"])

    def test_owner_pr_with_no_commits_fails(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["commits"] = 0
        self.refused_with(pr, [])

    def test_owner_pr_with_deleted_head_repo_fails(self) -> None:
        pr = _pr(outside=False, login="Owner")
        pr["head"]["repo"] = None
        self.refused_with(pr, [_commit("owner")])

    def test_non_owner_pr_of_owner_commits_fails(self) -> None:
        self.refused_with(_pr(outside=False, login="someone"), [_commit("owner")])

    def refused_with(self, pr: dict, commits: list) -> None:
        with self.assertRaises(tr.Refused) as cm:
            _decide(self.roots, pr, [{"filename": "Cargo.toml"}], [], commits)
        self.assertIn("without a code owner", str(cm.exception))

    def test_untouched_trust_roots_ignore_author(self) -> None:
        pr = _pr(outside=False)
        pr["user"] = None
        self.assertIn("no trust root", _decide(self.roots, pr, [{"filename": "src/x.rs"}], []))


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

    def test_owner_pass_reads_the_commits_endpoint(self) -> None:
        def run(commits: list) -> str:
            class FakeApi:
                def get(self, path: str) -> dict:
                    return _pr(outside=False, login="Owner")

                def get_all(self, path: str) -> list:
                    return {"files": [{"filename": "Cargo.toml"}], "reviews": [], "commits": commits}[path.rsplit("/", 1)[1]]

            ev = {"pull_request": {"number": 7, "head": {"sha": HEAD}}}
            return tr.run_check(_roots(ROOTS), "pull_request_target", ev, FakeApi())  # type: ignore[arg-type]

        self.assertIn("code owner Owner", run([_commit("owner")]))
        with self.assertRaises(tr.Refused):
            run([_commit("someone")])


class ApiGuards(unittest.TestCase):
    def test_non_https_base_is_refused(self) -> None:
        with self.assertRaises(tr.Refused):
            tr.Api("http://api.github.com", "o/r", "t")

    def test_unsendable_token_is_refused_without_echoing_it(self) -> None:
        for token in ("", "zq7\nX-Injected: 1", "zq7 zq7", "zq7\u00e9", "zq7\x00"):
            with self.subTest(token=token), self.assertRaises(tr.Refused) as cm:
                tr.Api("https://api.github.com", "o/r", token)
            if token:
                self.assertNotIn("zq7", str(cm.exception))

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


class _FakeResponse:
    def __init__(self, body: bytes, link: str | None = None):
        self._body = body
        self.headers = {"Link": link} if link else {}

    def __enter__(self) -> "_FakeResponse":
        return self

    def __exit__(self, *exc: object) -> None:
        return None

    def read(self, n: int) -> bytes:
        return self._body[:n]


class _FakeOpener:
    """An opener that answers every request with `outcome`: a response, or an
    exception it raises."""

    def __init__(self, outcome: object):
        self.outcome = outcome
        self.requests: list = []

    def open(self, req: object, timeout: float) -> _FakeResponse:
        self.requests.append(req)
        if isinstance(self.outcome, BaseException):
            raise self.outcome
        return self.outcome  # type: ignore[return-value]


class ApiErrorKinds(unittest.TestCase):
    """Every failed GET is one typed `ApiError`, still a `Refused`, whose
    message never carries the token."""

    TOKEN = "sentinel-tok-51b8"

    def api_answering(self, outcome: object) -> tr.Api:
        api = tr.Api("https://api.github.com", "o/r", self.TOKEN)
        api._opener = _FakeOpener(outcome)  # type: ignore[assignment]
        return api

    def assertKind(self, api: tr.Api, kind: type, path: str = "pulls/1") -> tr.ApiError:
        with self.assertRaises(kind) as cm:
            api.get(path)
        self.assertIsInstance(cm.exception, tr.Refused)
        self.assertNotIn(self.TOKEN, str(cm.exception))
        return cm.exception

    def test_an_http_error_status_is_http_status(self) -> None:
        import urllib.error

        for code in (401, 403, 404, 500, 502):
            with self.subTest(code=code):
                err = urllib.error.HTTPError("https://api.github.com/repos/o/r/pulls/1", code, "x", {}, None)  # type: ignore[arg-type]
                e = self.assertKind(self.api_answering(err), tr.HttpStatus)
                self.assertEqual(e.status, code)  # type: ignore[attr-defined]

    def test_an_unreachable_origin_is_transport(self) -> None:
        import urllib.error

        for cause in (urllib.error.URLError("refused"), TimeoutError("slow"), ConnectionResetError("reset")):
            with self.subTest(cause=cause):
                self.assertKind(self.api_answering(cause), tr.Transport)

    def test_a_connection_broken_mid_response_is_transport(self) -> None:
        import http.client

        for cause in (http.client.IncompleteRead(b"par", 10), http.client.BadStatusLine("x"), http.client.HTTPException("h")):
            with self.subTest(cause=cause):
                self.assertKind(self.api_answering(cause), tr.Transport)

    def test_off_origin_names_only_scheme_and_host(self) -> None:
        h = tr._PinnedRedirects("https://api.github.com")
        for url in (
            "https://evil.example/secret-path/x?code=q1w2e3#frag",
            "https://user:pw-5e1@evil.example:8443/secret-path?code=q1w2e3",
            "http://api.github.com/secret-path?code=q1w2e3",
        ):
            with self.subTest(url=url), self.assertRaises(tr.OffOrigin) as cm:
                h.redirect_request(None, None, 302, "Found", {}, url)
            msg = str(cm.exception)
            for leak in ("secret-path", "code=", "q1w2e3", "frag", "user", "pw-5e1", "8443"):
                self.assertNotIn(leak, msg)
            self.assertIn("off the API origin", msg)
        with self.assertRaises(tr.OffOrigin) as cm:
            h.redirect_request(None, None, 302, "Found", {}, "https://evil.example/secret-path")
        self.assertIn("https://evil.example", str(cm.exception))
        with self.assertRaises(tr.OffOrigin) as cm:
            h.redirect_request(None, None, 302, "Found", {}, "https://[bad/secret-path")
        self.assertNotIn("secret-path", str(cm.exception))

    def test_an_off_origin_redirect_is_off_origin(self) -> None:
        self.assertKind(self.api_answering(tr.OffOrigin("https://evil.example/x")), tr.OffOrigin)
        h = tr._PinnedRedirects("https://api.github.com")
        with self.assertRaises(tr.OffOrigin):
            h.redirect_request(None, None, 301, "Moved", {}, "https://evil.example/x")

    def test_an_off_origin_next_link_is_off_origin_and_not_sent(self) -> None:
        api = self.api_answering(_FakeResponse(b"[]", '<https://evil.example/repos/o/r/pulls?page=2>; rel="next"'))
        with self.assertRaises(tr.OffOrigin) as cm:
            api.get_all("pulls")
        self.assertNotIn(self.TOKEN, str(cm.exception))
        self.assertEqual(len(api._opener.requests), 1)  # type: ignore[attr-defined]

    def test_an_oversized_body_is_too_large(self) -> None:
        self.assertKind(self.api_answering(_FakeResponse(b" " * (tr.MAX_BODY_BYTES + 1))), tr.TooLarge)

    def test_too_many_pages_is_too_large(self) -> None:
        api = self.api_answering(_FakeResponse(b"[]", '<https://api.github.com/repos/o/r/pulls?page=2>; rel="next"'))
        with self.assertRaises(tr.TooLarge):
            api.get_all("pulls")
        self.assertEqual(len(api._opener.requests), tr.MAX_PAGES)  # type: ignore[attr-defined]

    def test_a_non_json_or_misshapen_body_is_malformed(self) -> None:
        self.assertKind(self.api_answering(_FakeResponse(b"<html>")), tr.Malformed)
        self.assertKind(self.api_answering(_FakeResponse(b"[]")), tr.Malformed)
        with self.assertRaises(tr.Malformed):
            self.api_answering(_FakeResponse(b"{}")).get_all("pulls")

    def test_a_too_deeply_nested_body_is_malformed(self) -> None:
        deep = b"[" * 200_000 + b"]" * 200_000
        self.assertKind(self.api_answering(_FakeResponse(deep)), tr.Malformed)
        self.assertKind(self.api_answering(_FakeResponse(b'{"a":' * 200_000 + b"1" + b"}" * 200_000)), tr.Malformed)

    def test_a_json_object_is_returned(self) -> None:
        self.assertEqual(self.api_answering(_FakeResponse(b'{"a": 1}')).get("pulls/1"), {"a": 1})


if __name__ == "__main__":
    unittest.main()
