#!/usr/bin/env python3
"""The branch-protection required set: derived from the manifest, checked
against `required-set.json` and against the live ruleset.

The required set is one `{context, integration_id}` pair per `gate` and
`gate-external` context of `ci/check-manifest.yml`, sorted by context — the
exact shape of a ruleset's `required_status_checks` parameter. A `gate` is
posted by a workflow, so its integration is GitHub Actions
(`GITHUB_ACTIONS_APP_ID`); a `gate-external` names the app that posts it in
its own `integration_id`. A required check without an integration is
satisfied by a status any app — or any token with `statuses: write` — posts,
so the pair, not the name, is the unit compared.

Modes:
  (none)      exit 0 iff `required-set.json` is the derived set.
  --write     regenerate `required-set.json` from the manifest.
  --live FILE also compare the ruleset JSON in FILE (as
              `gh api repos/OWNER/REPO/rulesets/ID` returns it).
  --fetch     also compare ruleset `RULESET_ID` read from the REST API
              ($REPO, $GH_TOKEN, optional $GITHUB_API_URL).

The live ruleset is parsed into a closed `Ruleset`: every key the API
returns is either examined and pinned or named as display metadata, and any
other key, rule type, or parameter is refused, so a protection GitHub adds
or an owner toggles is never waved through unread.  It must be active on the
default branch with no exclusions and no bypass actors, carry exactly one
`required_status_checks` rule (strict policy off, enforced on create) whose
pairs equal the derived set in both directions, and exactly one all-green
`merge_queue` rule. Every unreadable
or malformed input fails closed (exit 1) with nothing printed to stdout.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass

CI_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, CI_DIR)
import strict_yaml  # noqa: E402
import yaml  # noqa: E402

MANIFEST = os.path.join(CI_DIR, "check-manifest.yml")
REQUIRED_SET = os.path.join(CI_DIR, "required-set.json")
# The GitHub App id of GitHub Actions: the integration every workflow-posted
# check run carries.
GITHUB_ACTIONS_APP_ID = 15368
# The `main-protection` ruleset that enforces the required set on `main`.
RULESET_ID = 22326541

Pair = tuple[str, int]


class Refused(Exception):
    """An input this check cannot prove matches the derived set."""


def _is_app_id(v: object) -> bool:
    return isinstance(v, int) and not isinstance(v, bool) and v > 0


def derive(man: object) -> list[Pair]:
    """The manifest's required `(context, integration_id)` pairs, sorted."""
    checks = man.get("checks") if isinstance(man, dict) else None
    if not isinstance(checks, list):
        raise Refused("the manifest is not a `checks` list")
    pairs: list[Pair] = []
    for e in checks:
        if not isinstance(e, dict):
            raise Refused(f"manifest entry {e!r} is not a mapping")
        ctx, disp = e.get("context"), e.get("disposition")
        if not isinstance(ctx, str) or not isinstance(disp, str):
            raise Refused(f"manifest entry {e!r} lacks a string `context` and `disposition`")
        app = e.get("integration_id")
        if disp == "gate-external":
            if not _is_app_id(app):
                raise Refused(f"{ctx!r}: a gate-external entry must name its posting app's `integration_id`")
            pairs.append((ctx, app))
        elif "integration_id" in e:
            raise Refused(f"{ctx!r}: only a gate-external entry declares `integration_id`")
        elif disp == "gate":
            pairs.append((ctx, GITHUB_ACTIONS_APP_ID))
    names = [c for c, _ in pairs]
    if len(set(names)) != len(names):
        raise Refused("the manifest declares a required context twice")
    return sorted(pairs)


def parse_pairs(data: object, what: str) -> list[Pair]:
    """A `[{context, integration_id}]` list as pairs; a missing integration,
    an extra key, or a repeated context is refused."""
    if not isinstance(data, list):
        raise Refused(f"{what} is not a list of {{context, integration_id}}")
    pairs: list[Pair] = []
    for item in data:
        if not isinstance(item, dict) or set(item) != {"context", "integration_id"}:
            raise Refused(f"{what}: {item!r} is not exactly {{context, integration_id}}")
        ctx, app = item["context"], item["integration_id"]
        if not isinstance(ctx, str) or not ctx or not _is_app_id(app):
            raise Refused(f"{what}: {item!r} needs a non-empty context and a positive integration_id")
        pairs.append((ctx, app))
    names = [c for c, _ in pairs]
    if len(set(names)) != len(names):
        raise Refused(f"{what} lists a context twice")
    return pairs


def render(pairs: list[Pair]) -> str:
    return json.dumps([{"context": c, "integration_id": a} for c, a in pairs], indent=2) + "\n"


def _closed(obj: object, what: str, keys: frozenset[str], optional: frozenset[str] = frozenset()) -> dict:
    """`obj` as a mapping holding every one of `keys` and nothing outside
    `keys | optional`: a key this check does not examine is refused."""
    if not isinstance(obj, dict):
        raise Refused(f"{what} is not an object")
    missing = sorted(keys - obj.keys())
    if missing:
        raise Refused(f"{what} lacks {', '.join(missing)}")
    unknown = sorted(str(k) for k in obj.keys() - keys - optional)
    if unknown:
        raise Refused(f"{what} carries {', '.join(unknown)}, which this check does not examine")
    return obj


def _pin(value: object, want: object, what: str) -> None:
    # `type` too: `False == 0`, and a ruleset parameter's JSON type is part of it.
    if type(value) is not type(want) or value != want:
        raise Refused(f"{what} is {value!r}, not {want!r}")


def _typed(value: object, kind: type, what: str) -> None:
    if type(value) is not kind:
        raise Refused(f"{what} is {value!r}, not a {kind.__name__}")


# Top-level keys the API returns that carry no protection: they are display
# metadata, read by no comparison.
_RULESET_METADATA = frozenset({"name", "source", "node_id", "created_at", "updated_at", "_links"})
_RULESET_KEYS = frozenset({"id", "target", "enforcement", "conditions", "rules", "bypass_actors"})
# Keys present only in some responses; each is pinned when present.
_RULESET_VIEWER_KEYS = frozenset({"source_type", "current_user_can_bypass"})

_PULL_REQUEST_PARAMS: dict[str, type] = {
    "required_approving_review_count": int,
    "dismiss_stale_reviews_on_push": bool,
    "required_reviewers": list,
    "require_code_owner_review": bool,
    "dismissal_restriction": dict,
    "require_last_push_approval": bool,
    "required_review_thread_resolution": bool,
    "require_extra_approval_for_unattributed_changes": bool,
    "allowed_merge_methods": list,
}
_MERGE_QUEUE_PARAMS: dict[str, type] = {
    "merge_method": str,
    "max_entries_to_build": int,
    "min_entries_to_merge": int,
    "max_entries_to_merge": int,
    "min_entries_to_merge_wait_minutes": int,
    "grouping_strategy": str,
    "check_response_timeout_minutes": int,
}
_STATUS_PARAMS = frozenset(
    {"strict_required_status_checks_policy", "do_not_enforce_on_create", "required_status_checks"}
)
# Every rule type the ruleset must carry, exactly once each.  A type outside
# this set is one this check has not examined, so it is refused.
RULE_TYPES = frozenset({"deletion", "non_fast_forward", "pull_request", "required_status_checks", "merge_queue"})


@dataclass(frozen=True)
class Ruleset:
    """A live ruleset every key of which was examined: it applies, actively
    and without bypass, to the default branch and enforces `required`."""

    required: tuple[Pair, ...]


def _typed_params(params: object, what: str, shape: dict[str, type]) -> dict:
    params = _closed(params, what, frozenset(shape))
    for key, kind in shape.items():
        _typed(params[key], kind, f"{what} `{key}`")
    return params


def _rule(rule: object) -> tuple[str, tuple[Pair, ...]]:
    """A rule's type, and the pairs it requires when it is the status rule."""
    head = rule.get("type") if isinstance(rule, dict) else None
    if not isinstance(head, str) or head not in RULE_TYPES:
        raise Refused(f"the ruleset carries a rule of type {head!r}, which this check does not examine")
    what = f"the {head} rule"
    if head in ("deletion", "non_fast_forward"):
        _closed(rule, what, frozenset({"type"}))
        return head, ()
    params = _closed(rule, what, frozenset({"type", "parameters"}))["parameters"]
    if head == "pull_request":
        _typed_params(params, f"{what}'s parameters", _PULL_REQUEST_PARAMS)
        return head, ()
    if head == "merge_queue":
        params = _typed_params(params, f"{what}'s parameters", _MERGE_QUEUE_PARAMS)
        # The strict policy stays off because the queue builds every entry on
        # the combined tree; only `ALLGREEN` requires each entry's own checks.
        _pin(params["grouping_strategy"], "ALLGREEN", f"{what}'s grouping_strategy")
        return head, ()
    params = _closed(params, f"{what}'s parameters", _STATUS_PARAMS)
    _pin(params["strict_required_status_checks_policy"], False, f"{what}'s strict_required_status_checks_policy")
    _pin(params["do_not_enforce_on_create"], False, f"{what}'s do_not_enforce_on_create")
    pairs = parse_pairs(params["required_status_checks"], "the ruleset's required_status_checks")
    return head, tuple(pairs)


def parse_ruleset(rs: object) -> Ruleset:
    """The ruleset GET body as a `Ruleset`, or `Refused`."""
    rs = _closed(rs, "the ruleset", _RULESET_KEYS, _RULESET_METADATA | _RULESET_VIEWER_KEYS)
    _pin(rs["id"], RULESET_ID, "the ruleset id")
    _pin(rs["target"], "branch", "the ruleset target")
    _pin(rs["enforcement"], "active", "the ruleset enforcement")
    if "source_type" in rs:
        _pin(rs["source_type"], "Repository", "the ruleset source_type")
    if "current_user_can_bypass" in rs:
        _pin(rs["current_user_can_bypass"], "never", "the ruleset current_user_can_bypass")
    _pin(rs["bypass_actors"], [], "the ruleset bypass_actors")
    cond = _closed(rs["conditions"], "the ruleset conditions", frozenset({"ref_name"}))
    ref = _closed(cond["ref_name"], "the ruleset ref_name condition", frozenset({"include", "exclude"}))
    include = ref["include"]
    if not isinstance(include, list) or "~DEFAULT_BRANCH" not in include:
        raise Refused("the ruleset does not apply to the default branch")
    _pin(ref["exclude"], [], "the ruleset ref_name exclude")
    rules = rs["rules"]
    if not isinstance(rules, list):
        raise Refused("the ruleset has no `rules` list")
    seen: dict[str, int] = {}
    required: tuple[Pair, ...] = ()
    for rule in rules:
        head, pairs = _rule(rule)
        seen[head] = seen.get(head, 0) + 1
        if head == "required_status_checks":
            required = pairs
    for head in sorted(RULE_TYPES):
        if seen.get(head, 0) != 1:
            raise Refused(f"the ruleset has {seen.get(head, 0)} {head} rules, not 1")
    return Ruleset(required=required)


def diff(expected: list[Pair], actual: list[Pair], what: str, fix: str) -> list[str]:
    """Lines naming each pair missing from and extra in `actual`, or []."""
    missing = sorted(set(expected) - set(actual))
    extra = sorted(set(actual) - set(expected))
    if not missing and not extra:
        return []
    out = [f"{what} differs from the manifest's required set — {fix}:"]
    out += [f"  missing: {c!r} (integration {a})" for c, a in missing]
    out += [f"  extra:   {c!r} (integration {a})" for c, a in extra]
    return out


def fetch_ruleset() -> object:
    import trust_roots  # noqa: PLC0415  # the pinned-origin authenticated GET

    repo, token = os.environ.get("REPO", ""), os.environ.get("GH_TOKEN", "")
    if not repo or not token:
        raise Refused("--fetch needs $REPO and $GH_TOKEN")
    try:
        api = trust_roots.Api(os.environ.get("GITHUB_API_URL") or "https://api.github.com", repo, token)
        return api.get(f"rulesets/{RULESET_ID}")
    except trust_roots.Refused as e:
        raise Refused(str(e)) from e


def _load_manifest() -> object:
    try:
        with open(MANIFEST, encoding="utf-8") as f:
            return strict_yaml.safe_load(f)
    except (OSError, UnicodeDecodeError, yaml.YAMLError) as e:
        raise Refused(f".github/ci/check-manifest.yml is unreadable: {e}") from e


def _load_json(path: str, what: str) -> object:
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as e:
        raise Refused(f"{what} is unreadable: {e}") from e


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--live", metavar="FILE")
    mode.add_argument("--fetch", action="store_true")
    args = ap.parse_args(argv)
    try:
        required = derive(_load_manifest())
        if args.write:
            with open(REQUIRED_SET, "w", encoding="utf-8") as f:
                f.write(render(required))
            print(f"wrote .github/ci/required-set.json ({len(required)} required contexts).")
            return 0
        on_disk = parse_pairs(
            _load_json(REQUIRED_SET, ".github/ci/required-set.json"), ".github/ci/required-set.json"
        )
        problems = diff(
            required, on_disk, ".github/ci/required-set.json",
            "regenerate it: python3 .github/ci/check_required_set.py --write",
        )
        if args.live or args.fetch:
            rs = _load_json(args.live, args.live) if args.live else fetch_ruleset()
            problems += diff(
                required, list(parse_ruleset(rs).required), f"ruleset {RULESET_ID}",
                "reconcile it per .github/ci/RECONCILIATION.md",
            )
    except Refused as e:
        print(f"check_required_set: {e}", file=sys.stderr)
        return 1
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    live = args.live or args.fetch
    where = f"required-set.json and ruleset {RULESET_ID} match" if live else "required-set.json matches"
    print(f"{where} the manifest ({len(required)} required contexts).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
