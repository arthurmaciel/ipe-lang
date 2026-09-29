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

The live comparison refuses a ruleset that is not active on the default
branch, that carries other than exactly one `required_status_checks` rule, or
whose pairs differ from the derived set in either direction. Every unreadable
or malformed input fails closed (exit 1) with nothing printed to stdout.
"""
from __future__ import annotations

import argparse
import json
import os
import sys

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


def ruleset_pairs(rs: object) -> list[Pair]:
    """The required pairs a ruleset enforces on the default branch."""
    if not isinstance(rs, dict):
        raise Refused("the ruleset is not an object")
    if rs.get("id") != RULESET_ID:
        raise Refused(f"the ruleset is {rs.get('id')!r}, not {RULESET_ID}")
    if rs.get("target") != "branch" or rs.get("enforcement") != "active":
        raise Refused(
            f"the ruleset is not an active branch ruleset (target {rs.get('target')!r}, "
            f"enforcement {rs.get('enforcement')!r})"
        )
    cond = rs.get("conditions")
    ref = cond.get("ref_name") if isinstance(cond, dict) else None
    include = ref.get("include") if isinstance(ref, dict) else None
    if not isinstance(include, list) or "~DEFAULT_BRANCH" not in include or ref.get("exclude"):
        raise Refused("the ruleset does not apply to the default branch without exclusions")
    rules = rs.get("rules")
    if not isinstance(rules, list):
        raise Refused("the ruleset has no `rules` list")
    rsc = [r for r in rules if isinstance(r, dict) and r.get("type") == "required_status_checks"]
    if len(rsc) != 1:
        raise Refused(f"the ruleset has {len(rsc)} required_status_checks rules, not 1")
    params = rsc[0].get("parameters")
    if not isinstance(params, dict):
        raise Refused("the required_status_checks rule has no parameters")
    return parse_pairs(params.get("required_status_checks"), "the ruleset's required_status_checks")


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
                required, ruleset_pairs(rs), f"ruleset {RULESET_ID}",
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
