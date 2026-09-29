# CI required-set reconciliation

The status checks `main` requires are **derived from** `ci/check-manifest.yml`:
one `{context, integration_id}` pair per `gate` and `gate-external` entry,
sorted by context. `ci/check_required_set.py` is the one derivation;
`ci/required-set.json` is its committed output, in the exact shape of a
ruleset's `required_status_checks` parameter.

`integration_id` pins which GitHub App may satisfy a required check. A `gate`
is posted by a workflow, so it carries the GitHub Actions app id
(`GITHUB_ACTIONS_APP_ID` in `check_required_set.py`). A `gate-external` entry
names the app that posts it in its own `integration_id`; no other entry may
declare one. A required check with no integration is satisfied by a status
any app — or any token holding `statuses: write` — posts under that name, so
the pair, not the name, is what every comparison below checks.

## Where the set is enforced

| Boundary | Compares | Runs |
|----------|----------|------|
| `verify-manifest.py` check 4 | manifest ⇄ `required-set.json` | `manifest-guard`, the local gate |
| `check_required_set.py` | manifest ⇄ `required-set.json` | `manifest-guard` |
| `check_required_set.py --fetch` | manifest ⇄ the live ruleset | `ruleset-drift` job in `ci.yml` |

The live ruleset is `main-protection` (`RULESET_ID` in
`check_required_set.py`). `--fetch` parses it into a closed `Ruleset`: every
key the API returns is examined and pinned or named as display metadata, and
any other key, rule type, or rule parameter is refused. It must be an active
branch ruleset on `~DEFAULT_BRANCH` with no exclusions and no
`bypass_actors`, carrying `deletion`, `non_fast_forward`, `pull_request`,
`merge_queue` (grouping `ALLGREEN`), and `required_status_checks` once each,
the last with pairs equal to the derived set in both directions. A key GitHub
adds to the response turns `ruleset-drift` red until this check examines it. `ruleset-drift` is a `nightly-gate`: a red nightly makes the
required `nightly-green` context hold every merge until the ruleset is
reconciled. On a pull request it is not required; there it flags a
required-set change the ruleset has not taken yet.

`strict_required_status_checks_policy` ("require branches to be up to date")
is pinned `false` and `do_not_enforce_on_create` is pinned `false`. The strict
policy stays off because the `ALLGREEN` merge queue already runs the required
checks on the combined tree of each queued change, which is the property the
strict policy would buy; the queue's grouping is pinned for that reason.

## Changing the required set

1. Change the entry's disposition in `ci/check-manifest.yml`.
2. `python3 .github/ci/check_required_set.py --write` and commit the result
   with the manifest change.
3. The repository owner applies the set to the live ruleset. Editing a
   ruleset is a security-relevant change, so no workflow token does it:

   ```bash
   repo=ipe-lang/compiler
   id=$(python3 -c 'import sys; sys.path.insert(0, ".github/ci"); import check_required_set as c; print(c.RULESET_ID)')
   gh api "repos/$repo/rulesets/$id" > /tmp/rs.json
   jq --slurpfile want .github/ci/required-set.json \
     '{name, target, enforcement, conditions, bypass_actors,
       rules: [.rules[] | if .type == "required_status_checks"
                          then .parameters.required_status_checks = $want[0] else . end]}' \
     /tmp/rs.json > /tmp/rs-new.json
   gh api -X PUT "repos/$repo/rulesets/$id" --input /tmp/rs-new.json
   python3 .github/ci/check_required_set.py --live <(gh api "repos/$repo/rulesets/$id")
   ```

   Review `/tmp/rs-new.json` before the `PUT`: it rewrites the whole ruleset.

## Contexts outside the required set

- `nightly-gate` contexts run on the nightly full gate, not per change; a red
  one blocks the next merge through `nightly-green`.
- `informational` contexts never block; `ci-health` surfaces a red one.
- `windows-static` and `freebsd-cross` (`static.yml`) set
  `continue-on-error: true`, so a red reports green even to `ci-health`.
  Dropping `continue-on-error` is the fix; it is the `release` owner's.
- The macOS, Windows, and FreeBSD Tier-2 jail proofs are not CI contexts:
  they need a real-OS substrate a GitHub-hosted runner lacks, so their
  containment is verified by the release checklist until such runners exist
  (`#2247`, `#2248`, `#2249`). `macos-arm64` (Seatbelt) and the Linux Tier-2
  jails are the containment proofs CI produces.
