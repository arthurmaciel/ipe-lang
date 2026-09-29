#!/usr/bin/env python3
"""Trust-root SSOT reader and the `trust-root-diff` merge decision.

`.github/CODEOWNERS` is the ONE list of trust roots: GitHub reads it to demand
the code owner's review, and this module parses the same file to decide
`trust-root-diff` and to let `verify-manifest.py` prove every rule is live.
There is no second list to drift.

The parser accepts a strict subset of CODEOWNERS (listed in the file's own
header) and refuses everything else, so the path set this module computes is
exactly the one GitHub protects: an unsupported glob or an ownerless rule is a
typed refusal, never a silently different match.

`check` (run by `.github/workflows/trust-root-diff.yml` on
`pull_request_target` and `merge_group`) never sees head code: it reads the
event payload GitHub wrote, then the PR, its file list, and its reviews over
the REST API. A PR that touches a trust root fails unless a code owner's latest
decisive review APPROVES the PR's current head commit. A code owner cannot
approve their own PR, so one PR passes without that review: a code owner's
(GitHub `User` named in CODEOWNERS), from a branch of this repository, whose
every commit a code owner authored. Every ambiguity (a file list the API truncated, a
PR that moved since the event, an unparseable merge-queue ref, an HTTP error)
fails closed.

Pure stdlib. Exit 0 = pass, 1 = refused, 2 = usage / malformed input.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass

# ---------------------------------------------------------------- parsing --

MAX_CODEOWNERS_BYTES = 64 * 1024
MAX_RULES = 512
# A GitHub login: 1-39 alphanumerics or single inner hyphens. Teams
# (`@org/team`) and emails are outside the accepted subset: the decision
# below compares review authors by login, and a team or email owner would
# need a membership lookup this check cannot make read-only.
_OWNER_RE = re.compile(r"^@[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}$")
_FORBIDDEN_PATTERN_CHARS = frozenset("\\[]!#")
# A rule line is printable ASCII and tab only. Python's `splitlines()` and
# `split()` also break on Unicode separators (U+2028, U+00A0, `\v`, `\f`, ...)
# that GitHub does not, so lines are split on `\n` alone and a rule may not
# carry any character on which the two readings could differ.
_RULE_LINE = re.compile(r"[\t\x20-\x7e]+")


class CodeownersError(ValueError):
    """The CODEOWNERS text leaves the accepted subset."""


@dataclass(frozen=True)
class Rule:
    """One compiled CODEOWNERS rule.

    `components` is the pattern split on `/` (anchor and directory suffix
    removed). An `anchored` rule matches its components from the repository
    root; an unanchored rule has exactly one component, matched at any depth.
    A `dir_only` rule matches only paths strictly inside a matching
    directory."""

    pattern: str
    line: int
    components: tuple[str, ...]
    anchored: bool
    dir_only: bool

    def matches(self, path: str) -> bool:
        parts = path.split("/")
        if self.anchored:
            k = len(self.components)
            if len(parts) < k:
                return False
            if not all(fnmatch.fnmatchcase(parts[i], self.components[i]) for i in range(k)):
                return False
            return len(parts) > k or not self.dir_only
        (component,) = self.components
        last = len(parts) - 1
        return any(
            fnmatch.fnmatchcase(part, component) and (i < last or not self.dir_only)
            for i, part in enumerate(parts)
        )


@dataclass(frozen=True)
class TrustRoots:
    rules: tuple[Rule, ...]
    # Case-folded logins without the `@`; identical for every rule.
    owners: frozenset[str]

    def is_trust_root(self, path: str) -> bool:
        return any(r.matches(path) for r in self.rules)


def _compile_pattern(pattern: str, line: int) -> Rule:
    bad = sorted(set(pattern) & _FORBIDDEN_PATTERN_CHARS)
    if bad:
        raise CodeownersError(f"line {line}: pattern {pattern!r} uses unsupported {''.join(bad)!r}")
    body = pattern
    dir_only = False
    if body.endswith("/**"):
        body, dir_only = body[:-3], True
    elif body.endswith("/"):
        body, dir_only = body[:-1], True
    anchored = body.startswith("/")
    if anchored:
        body = body[1:]
    if "**" in body:
        raise CodeownersError(f"line {line}: pattern {pattern!r}: `**` is only accepted as a final `/**`")
    components = tuple(body.split("/"))
    if any(c in ("", ".", "..") for c in components):
        raise CodeownersError(f"line {line}: pattern {pattern!r} has an empty, `.` or `..` component")
    if not anchored and len(components) != 1:
        raise CodeownersError(
            f"line {line}: pattern {pattern!r} contains `/` but does not start with `/`; "
            "anchor it explicitly"
        )
    return Rule(pattern, line, components, anchored, dir_only)


def parse_codeowners(text: str) -> TrustRoots:
    if len(text.encode("utf-8")) > MAX_CODEOWNERS_BYTES:
        raise CodeownersError(f"CODEOWNERS exceeds {MAX_CODEOWNERS_BYTES} bytes")
    if "\r" in text.replace("\r\n", ""):
        raise CodeownersError("CODEOWNERS contains a bare carriage return; lines end in `\\n` or `\\r\\n`")
    rules: list[Rule] = []
    owner_set: frozenset[str] | None = None
    for n, raw in enumerate(text.split("\n"), start=1):
        line = raw.removesuffix("\r").strip(" \t")
        if not line or line.startswith("#"):
            continue
        if not _RULE_LINE.fullmatch(line):
            bad = next(c for c in line if not _RULE_LINE.fullmatch(c))
            raise CodeownersError(f"line {n}: rule contains {bad!r}; a rule is printable ASCII and tabs only")
        if "#" in line:
            raise CodeownersError(f"line {n}: inline `#` is not accepted")
        # Checked before splitting: `\ ` escapes a space, so a split would
        # read one escaped pattern as a pattern plus a bogus owner.
        if "\\" in line:
            raise CodeownersError(f"line {n}: `\\` escapes are unsupported")
        pattern, *owners = line.split()
        if not owners:
            raise CodeownersError(
                f"line {n}: pattern {pattern!r} has no owner (an ownerless rule un-owns the paths it matches)"
            )
        for o in owners:
            if not _OWNER_RE.match(o):
                raise CodeownersError(f"line {n}: owner {o!r} is not an `@login`")
        folded = frozenset(o[1:].casefold() for o in owners)
        if owner_set is None:
            owner_set = folded
        elif folded != owner_set:
            raise CodeownersError(
                f"line {n}: owners differ from the first rule's; every rule must share one owner set"
            )
        rules.append(_compile_pattern(pattern, n))
        if len(rules) > MAX_RULES:
            raise CodeownersError(f"more than {MAX_RULES} rules")
    if owner_set is None:
        raise CodeownersError("CODEOWNERS has no rules")
    return TrustRoots(tuple(rules), owner_set)


def load_codeowners(path: str) -> TrustRoots:
    with open(path, "rb") as f:
        data = f.read(MAX_CODEOWNERS_BYTES + 1)
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as e:
        raise CodeownersError(f"CODEOWNERS is not UTF-8: {e}") from e
    return parse_codeowners(text)


# --------------------------------------------------------------- decision --

# GitHub's PR-files endpoint lists at most 3000 files; past that the list is
# truncated and the decision cannot see every path.
MAX_PR_FILES = 3000
_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
_QUEUE_REF_RE = re.compile(r"^refs/heads/gh-readonly-queue/.+/pr-([1-9][0-9]{0,9})-[0-9a-f]{40}$")
_DECISIVE = frozenset({"APPROVED", "CHANGES_REQUESTED", "DISMISSED"})


class Refused(Exception):
    """The check cannot pass; the message says why."""


def _field(obj: object, *keys: str) -> object:
    cur = obj
    for k in keys:
        if not isinstance(cur, dict) or k not in cur:
            raise Refused(f"API/event object lacks `{'.'.join(keys)}`")
        cur = cur[k]
    return cur


def _int(obj: object, *keys: str) -> int:
    v = _field(obj, *keys)
    if not isinstance(v, int) or isinstance(v, bool):
        raise Refused(f"`{'.'.join(keys)}` is not an integer")
    return v


def _sha(obj: object, *keys: str) -> str:
    v = _field(obj, *keys)
    if not isinstance(v, str) or not _SHA_RE.match(v):
        raise Refused(f"`{'.'.join(keys)}` is not a 40-hex commit id")
    return v


@dataclass(frozen=True)
class Owner:
    """A code owner: a GitHub `User` whose login CODEOWNERS names."""

    login: str


@dataclass(frozen=True)
class Other:
    """Any other author: a collaborator, a fork, or a bot."""

    login: str


Author = Owner | Other

_LOGIN_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}(?:\[bot\])?$")
# The API caps a PR's commit listing at this many; past it the list is partial.
MAX_PR_COMMITS = 250
# `github-actions[bot]` authors and commits with a workflow token of this
# repository (release-please's fallback token). A workflow that can write here
# runs from a trust root on main or from a branch only a write-access account
# can push, so it widens nothing past the owners' own reach.
_WORKFLOW_BOT = "github-actions[bot]"
# `web-flow` commits what an account does through GitHub's UI or API (the
# author is then the acting account).
_TRUSTED_COMMITTERS = frozenset({"web-flow", _WORKFLOW_BOT})


def parse_author(pr: dict, owners: frozenset[str]) -> Author:
    """The PR author, read once from `user.login` + `user.type`. A missing or
    malformed field is a refusal, never an `Other` that might later pass."""
    login = _field(pr, "user", "login")
    kind = _field(pr, "user", "type")
    if not isinstance(login, str) or not _LOGIN_RE.match(login) or not isinstance(kind, str):
        raise Refused("PR author has a malformed `user.login` or `user.type`")
    if kind == "User" and login.casefold() in owners:
        return Owner(login)
    return Other(login)


def _commit_login(commit: object, role: str) -> str | None:
    """The GitHub account the API linked to a commit's author or committer,
    or `None` when it linked none (an email no account claims)."""
    user = commit.get(role) if isinstance(commit, dict) else None
    login = user.get("login") if isinstance(user, dict) else None
    return login.casefold() if isinstance(login, str) else None


def commits_by_owners(pr: dict, commits: list, owners: frozenset[str]) -> bool:
    """True iff the API listed every commit of the PR and each one's author is
    a code owner or this repository's workflow bot and its committer one of
    those or `web-flow`, so no other account's push rides on an owner's PR."""
    expected = _int(pr, "commits")
    if expected < 1 or expected > MAX_PR_COMMITS or len(commits) != expected:
        return False
    authors = owners | {_WORKFLOW_BOT}
    committers = owners | _TRUSTED_COMMITTERS
    for c in commits:
        author = _commit_login(c, "author")
        committer = _commit_login(c, "committer")
        if author is None or author not in authors or committer is None or committer not in committers:
            return False
    return True


def is_outside(pr: dict) -> bool:
    """True unless the head branch provably lives in the base repository. A
    deleted head repository (`head.repo: null`) counts as outside."""
    head_repo = _field(pr, "head", "repo")
    if head_repo is None:
        return True
    return _int(head_repo, "id") != _int(pr, "base", "repo", "id")


def changed_paths(pr: dict, files: list) -> set[str]:
    """Every path the PR adds, modifies, removes, or renames FROM or TO."""
    expected = _int(pr, "changed_files")
    if expected > MAX_PR_FILES:
        raise Refused(f"PR changes {expected} files; the API lists at most {MAX_PR_FILES}")
    if len(files) != expected:
        raise Refused(f"API listed {len(files)} files but the PR reports {expected}")
    paths: set[str] = set()
    for f in files:
        name = _field(f, "filename")
        if not isinstance(name, str) or not name:
            raise Refused("a PR file entry has no filename")
        paths.add(name)
        prev = f.get("previous_filename") if isinstance(f, dict) else None
        if prev is not None:
            if not isinstance(prev, str) or not prev:
                raise Refused("a PR file entry has a malformed previous_filename")
            paths.add(prev)
    return paths


def owner_approved(reviews: list, owners: frozenset[str], head_sha: str) -> bool:
    """A code owner's latest decisive review APPROVES `head_sha`, and no code
    owner's latest decisive review requests changes. COMMENTED and PENDING
    reviews change nothing; a DISMISSED one withdraws an approval."""
    latest: dict[str, dict] = {}
    for r in reviews:
        # A deleted account's review has `user: null`; it names no owner.
        if isinstance(r, dict) and r.get("user", ...) is None:
            continue
        login = _field(r, "user", "login")
        state = _field(r, "state")
        if not isinstance(login, str) or not isinstance(state, str):
            raise Refused("a review has a malformed author or state")
        if login.casefold() in owners and state in _DECISIVE:
            latest[login.casefold()] = r
    if any(r["state"] == "CHANGES_REQUESTED" for r in latest.values()):
        return False
    return any(r["state"] == "APPROVED" and r.get("commit_id") == head_sha for r in latest.values())


def decide(roots: TrustRoots, pr: dict, files: list, reviews: list, commits: list) -> str:
    """Return a pass reason, or raise `Refused`."""
    head_sha = _sha(pr, "head", "sha")
    if _field(pr, "state") != "open":
        raise Refused("PR is not open")
    touched = sorted(p for p in changed_paths(pr, files) if roots.is_trust_root(p))
    if not touched:
        return "no trust root touched"
    author = parse_author(pr, roots.owners)
    if isinstance(author, Owner) and not is_outside(pr) and commits_by_owners(pr, commits, roots.owners):
        return f"{len(touched)} trust root(s) touched by code owner {author.login}"
    if owner_approved(reviews, roots.owners, head_sha):
        return f"{len(touched)} trust root(s) touched; code owner approved {head_sha}"
    shown = ", ".join(touched[:20]) + (" ..." if len(touched) > 20 else "")
    raise Refused(
        f"PR touches trust root(s) [{shown}] without a code owner's approval of head {head_sha}; "
        f"after that approval, re-run this job"
    )


def pr_number_for_event(event_name: str, event: dict) -> tuple[int, str | None]:
    """The PR number to judge and, for `pull_request_target`, the head sha the
    event was raised for (the live PR must still be at it)."""
    if event_name == "pull_request_target":
        return _int(event, "pull_request", "number"), _sha(event, "pull_request", "head", "sha")
    if event_name == "merge_group":
        ref = _field(event, "merge_group", "head_ref")
        m = _QUEUE_REF_RE.match(ref) if isinstance(ref, str) else None
        if m is None:
            raise Refused(f"merge-queue ref {ref!r} does not name a PR")
        return int(m.group(1)), None
    raise Refused(f"event {event_name!r} is not judged by this check")


# ------------------------------------------------------------------- HTTP --

MAX_PAGES = 30  # 30 x 100 = the API's own 3000-file ceiling
MAX_BODY_BYTES = 16 * 1024 * 1024
_NEXT_RE = re.compile(r'<([^>]+)>\s*;\s*rel="next"')


class _PinnedRedirects(urllib.request.HTTPRedirectHandler):
    """Follow a redirect only within the API origin: urllib re-sends the
    `Authorization` header to wherever a redirect points."""

    def __init__(self, base: str):
        self.base = base

    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: ANN001
        if not newurl.startswith(self.base + "/"):
            raise Refused(f"refusing to follow a redirect to {newurl!r} off the API origin")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class Api:
    def __init__(self, base_url: str, repo: str, token: str):
        if not re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", repo):
            raise Refused(f"malformed repository {repo!r}")
        parsed = urllib.parse.urlsplit(base_url)
        if parsed.scheme != "https" or not parsed.netloc:
            raise Refused(f"API URL {base_url!r} is not https")
        self.base = base_url.rstrip("/")
        self.repo = repo
        self.token = token
        self._opener = urllib.request.build_opener(_PinnedRedirects(self.base))

    def _get(self, url: str) -> tuple[object, str | None]:
        # The token is only ever sent to the configured API origin.
        if not url.startswith(self.base + "/"):
            raise Refused(f"refusing to follow {url!r} off the API origin")
        req = urllib.request.Request(
            url,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "trust-root-diff",
            },
        )
        try:
            with self._opener.open(req, timeout=30) as resp:
                body = resp.read(MAX_BODY_BYTES + 1)
                link = resp.headers.get("Link")
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise Refused(f"GET {url} failed: {e}") from e
        if len(body) > MAX_BODY_BYTES:
            raise Refused(f"GET {url}: response exceeds {MAX_BODY_BYTES} bytes")
        try:
            data = json.loads(body)
        except ValueError as e:
            raise Refused(f"GET {url}: response is not JSON") from e
        m = _NEXT_RE.search(link) if link else None
        return data, (m.group(1) if m else None)

    def get(self, path: str) -> dict:
        data, _ = self._get(f"{self.base}/repos/{self.repo}/{path}")
        if not isinstance(data, dict):
            raise Refused(f"GET {path}: expected an object")
        return data

    def get_all(self, path: str) -> list:
        url: str | None = f"{self.base}/repos/{self.repo}/{path}?per_page=100"
        items: list = []
        for _ in range(MAX_PAGES):
            if url is None:
                return items
            data, url = self._get(url)
            if not isinstance(data, list):
                raise Refused(f"GET {path}: expected a list")
            items.extend(data)
        if url is not None:
            raise Refused(f"GET {path}: more than {MAX_PAGES} pages")
        return items


def run_check(roots: TrustRoots, event_name: str, event: dict, api: Api) -> str:
    number, event_head = pr_number_for_event(event_name, event)
    pr = api.get(f"pulls/{number}")
    head_sha = _sha(pr, "head", "sha")
    if event_head is not None and head_sha != event_head:
        raise Refused(f"PR head moved from {event_head} to {head_sha}; the newer run decides")
    files = api.get_all(f"pulls/{number}/files")
    reviews = api.get_all(f"pulls/{number}/reviews")
    commits = api.get_all(f"pulls/{number}/commits")
    # The file list, reviews, and commits are read after the PR: a push in between
    # would pair the older head (and an approval of it) with newer files.
    after = _sha(api.get(f"pulls/{number}"), "head", "sha")
    if after != head_sha:
        raise Refused(f"PR head moved from {head_sha} to {after} while it was read; the newer run decides")
    return decide(roots, pr, files, reviews, commits)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check", help="judge the current pull_request_target / merge_group event")
    c.add_argument("--codeowners", required=True)
    args = ap.parse_args(argv)
    try:
        roots = load_codeowners(args.codeowners)
    except (OSError, CodeownersError) as e:
        print(f"trust-root-diff: CODEOWNERS refused: {e}", file=sys.stderr)
        return 2
    try:
        env = {k: os.environ.get(k, "") for k in ("GITHUB_EVENT_NAME", "GITHUB_EVENT_PATH", "GITHUB_REPOSITORY", "GH_TOKEN")}
        if not all(env.values()):
            missing = [k for k, v in env.items() if not v]
            raise Refused(f"missing environment: {', '.join(missing)}")
        with open(env["GITHUB_EVENT_PATH"], "rb") as f:
            event = json.loads(f.read(MAX_BODY_BYTES))
        if not isinstance(event, dict):
            raise Refused("event payload is not an object")
        api = Api(os.environ.get("GITHUB_API_URL") or "https://api.github.com", env["GITHUB_REPOSITORY"], env["GH_TOKEN"])
        reason = run_check(roots, env["GITHUB_EVENT_NAME"], event, api)
    except (Refused, OSError, ValueError) as e:
        print(f"trust-root-diff: FAIL: {e}", file=sys.stderr)
        return 1
    print(f"trust-root-diff: OK: {reason}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
