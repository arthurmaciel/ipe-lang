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

## Intended required set (= manifest `gate` contexts)

See `ci/required-set.json`. As of this change, 27 contexts:

```
artifact-guard
capabilities-docs-drift
cargo-deny
clippy
diagnostic-tone
doc-string example gate
e2e-all
env-docs-drift
explain-page example gate (ADR 0059)
first-party check floor (ipe type-check only)
first-party shapes (build gate)
fmt
linux-arm64 (seccomp socket-deny + bubblewrap)
linux-x64 (seccomp socket-deny + bubblewrap)
macos-arm64 (sandbox-exec / Seatbelt)
no-reference-impl-leak
panic-scan
playground-jail
quick-check
registry-admission
runtime-full-features
seal-slice
seal-smoke
stdlib-docs-drift
test
wasm-floor
```

## Delta vs live `main-protection` ruleset (id 22326541)

Measured against the ruleset's current `required_status_checks`.

**Add to the required set** (already produced per-change, promote to required —
they are `gate` in the manifest but were not in the ruleset):

- `no-reference-impl-leak` — cheap `git`+`rg` reference-impl-leak scan (Security).
- `env-docs-drift` — deterministic env-docs diff (parity with `stdlib-docs-drift`).
- `capabilities-docs-drift` — deterministic capabilities-docs diff.
- `panic-scan` — panic-pattern scan (Soundness). Already required on `main`.
- `registry-admission` — registry admission gate.

No other changes: every other live required context is a manifest `gate` and stays.

## Applying the delta (human step)

Reconcile the live ruleset to `ci/required-set.json`. Example (review before running):

```bash
# Fetch, edit required_status_checks to match ci/required-set.json, then PATCH.
gh api repos/arthurmaciel/ipe-lang/rulesets/22326541 > /tmp/rs.json
# ... edit /tmp/rs.json required_status_checks to the 26 contexts ...
gh api -X PUT repos/arthurmaciel/ipe-lang/rulesets/22326541 --input /tmp/rs.json
```

`strict_required_status_checks_policy` should stay `false` (heavy `nightly-gate`
contexts must not be forced onto every PR); nightly-gate reds are enforced by the
fail-closed `promotion-ready` job on the next promotion, not by branch protection.

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
- **`asan`/`tsan`/`browser-e2e`/Linux jail-tier2 proofs — heavy, previously silent
  advisory reds.** Now `nightly-gate` with a surface; no longer un-watched.
- **`install-smoke ×4` — installer UX, network-dependent → intermittently noisy.**
  `informational`, owner `release`; the dedup issue keeps one surface per red
  instead of an email per run.

## Regenerate `ci/required-set.json`

```bash
python3 -c "import yaml,json; d=yaml.safe_load(open('ci/check-manifest.yml')); \
print(json.dumps(sorted(e['context'] for e in d['checks'] if e['disposition'] in ('gate')), indent=2))" \
  > ci/required-set.json
```
