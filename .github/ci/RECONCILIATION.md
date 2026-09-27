# CI required-set reconciliation

The branch-protection required status checks are **derived from**
`ci/check-manifest.yml` — every `gate` entry, and only those. `ci/required-set.json`
is that derived list (regenerate with the snippet at the bottom). `manifest-guard`
(`.github/workflows/manifest-guard.yml`) fails a PR whenever the manifest and the
produced workflow contexts drift apart.

This file records the **intended** `main-protection` required set and the delta
against the live ruleset. Applying the delta to the live ruleset is a manual,
human step (a ruleset edit is a security-relevant change and is intentionally not
automated by a workflow token).

## Intended required set (= manifest `gate` + `gate-external` contexts)

`ci/required-set.json` is the list; `manifest-guard` fails a PR when it drifts
from the manifest.

## Delta vs live `main-protection` ruleset (id 22326541)

Measured against the ruleset's current `required_status_checks`.

**Add to the required set** (manifest `gate`, produced per-change, absent from
the ruleset):

- `cli-docs-drift`, `cli-transcripts-drift`, `markdown-parity` — deterministic
  generated-docs / parse-SSOT snapshot diffs (parity with `stdlib-docs-drift`).
- `grammar` — tree-sitter grammar drift guard.
- `manifest-lock-consistency` — Cargo.lock / Cargo.toml version lockstep.
- `runtime-feature-combos` — every emitted-runtime feature combination builds
  (SEAL); path-gated on `emit`.
- `asan-all`, `tsan` — sanitizers over the runtime + compiler crates
  (Soundness); path-gated on `emit`.
- `linux-arm64-tier2 (fifth platform — fail-closed refuse-to-certify proof)` —
  aarch64 refuse-to-certify proof (Security); path-gated on `code`.
- `freebsd-x64 (jail(8) inside vmactions VM)`,
  `windows-x64 (Docker Windows container, process isolation)` — admission jail
  proofs (Security); path-gated on `code`.

Each is path-gated by a job-level `if:` on the `changes` outputs, so on a PR
outside its paths the job reports `skipped`, which satisfies a required context.

No removals: every live required context is a manifest `gate`.

## Applying the delta (human step)

Reconcile the live ruleset to `.github/ci/required-set.json`. Example (review before running):

```bash
# Fetch, edit required_status_checks to match ci/required-set.json, then PATCH.
gh api repos/arthurmaciel/ipe-lang/rulesets/22326541 > /tmp/rs.json
# ... edit /tmp/rs.json required_status_checks to the contexts in required-set.json ...
gh api -X PUT repos/arthurmaciel/ipe-lang/rulesets/22326541 --input /tmp/rs.json
```

`strict_required_status_checks_policy` should stay `false` (heavy `nightly-gate`
contexts must not be forced onto every PR); nightly-gate reds are enforced by the
fail-closed `promotion-ready` job on the next promotion, not by branch protection.

## Nightly-gate contexts (NOT branch-protection required)

Checks too slow for the per-PR path, or not produced on `pull_request`. A red
does not block a PR and is surfaced by `ci-health`. See the `nightly-gate`
entries in the manifest (seal-modset, browser-e2e, linux-x64-tier2,
linux-cfree-gate). A check that runs on every relevant PR and finishes in a few
minutes is a `gate`, never `nightly-gate` — otherwise a PR auto-merges red.

## Flagged: required-but-flaky and informational-but-noisy

Reconciling the current checks against the disposition table surfaced these:

- **`windows-static`, `freebsd-cross` — reported-green-when-red.** Both set
  `continue-on-error: true`, so a red reports GREEN even to a human — worse than
  advisory. Disposition `informational`; the fix is to DROP `continue-on-error`
  so the `ci-health` surface can see a real red. (Not changed in this PR — it
  touches `static.yml` job semantics; tracked here for the static.yml owner.)
- **macOS / Windows / FreeBSD jail Tier-2 proofs — un-greenable on hosted
  runners.** Not classified as CI contexts: they need real-OS substrate a
  GitHub-hosted runner lacks. Their containment is verified out-of-band (release
  checklist); `#2247`/`#2248`/`#2249` are the revival trigger for restoring them
  when self-hosted / real-OS runners exist. `macos-arm64` (Seatbelt) and the
  Linux Tier-2 jails remain the gating containment proofs.
- **`browser-e2e` / `linux-x64-tier2` — heavy or env-flaky, off the PR path.**
  `nightly-gate` with a surface; no longer un-watched.
- **`install-smoke ×4` — installer UX, network-dependent → intermittently noisy.**
  `informational`, owner `release`; the dedup issue keeps one surface per red
  instead of an email per run.

No check is left unclassified or guarantee-but-un-gated after this change.

## Regenerate `ci/required-set.json`

```bash
python3 -c "import yaml,json; d=yaml.safe_load(open('.github/ci/check-manifest.yml')); \
print(json.dumps(sorted(e['context'] for e in d['checks'] if e['disposition'] in ('gate')), indent=2))" \
  > .github/ci/required-set.json
```
