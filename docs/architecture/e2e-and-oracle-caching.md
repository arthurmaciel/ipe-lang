# E2E shared target + cached Go oracle (queued infra)

> Status: QUEUED — run as one infra batch at the post-M2-lex idle gap, before M4
> (M4 explodes the golden count, so both wins compound there). Both touch the
> E2E/parity harness, which M2-lex is editing — that's why it waits.

## Problem
- Each E2E golden builds its emitted Rust project in its OWN `/tmp/skyc_<name>_e2e/target/`,
  which OVERRIDES the global `~/.cargo/config.toml` shared target. So heavy deps
  (tokio/rsa/syn/...) recompile per golden (×~18). sccache softens the rustc layer
  but link/codegen repeat. Also balloons /tmp to several GB per sweep.
- The Go reference output is recomputed live every parity run, though it's a pure
  function of (Main.sky, Go `sky` version) — independent of skyc.

## Fix 1 — E2E emitted projects share ONE target
Point every emitted-project `cargo build` at a single shared target (the global
`~/.cache/sky-rust-target`, or a dedicated `~/.cache/sky-e2e-target`) instead of a
per-project `/tmp/.../target`. Locate where the harness sets the per-project target
(crates/skyc/tests helper or the backend project emitter) and redirect it.
- Result: deps compile ONCE, reused across all goldens → e2e fast; /tmp stops
  ballooning.
- Caveat: concurrent e2e (nextest `--test-threads 2`) → 2 builds share one
  `.cargo-lock` → they serialize. Fine: each is fast once deps are cached; the
  first cold build holds the lock once. Net win >> the lock cost.

## Fix 2 — cache the Go oracle as a committed golden value

> Status: IMPLEMENTED. On-disk format = `tests/golden/<name>/expected_go.txt`
> (clean program stdout) + `oracle.meta` (`main_sky_sha256` + `go_sky_version` +
> `exit_code` + `oracle_divergence` [+ `divergence_reason`]). Format + staleness
> gate + the shared build/run core live in the `oracle` crate (`tools/oracle`);
> the `refresh-oracle` binary (`tools/refresh-oracle`) (re)captures the cache
> (Go success → cache Go; Go failure → cache skyc with `oracle_divergence=true`).
> The golden read path is `support::assert_go_parity` → `oracle::check_parity`
> (NO live Go), wired into the M2-lex goldens. `oracle`'s unit tests pin all four
> rigour invariants (match / stale / missing / divergence).
Per golden, commit `expected_go.txt` + `oracle.meta` = { `hash(Main.sky)`,
Go `sky` version }. Parity = run skyc, diff vs `expected_go.txt`. No Go build in the
hot path; parity also runs with no Go binary present (helps headless cron/CI).
- **Staleness gate:** if `hash(Main.sky) != stored hash` → FAIL "oracle stale —
  run refresh", never silently diff against a stale expected.
- **`refresh-oracle` step** (run when a golden is added/changed or the Go pin bumps):
  rebuild Go, recapture `expected_go.txt`, update meta.
- **Go-failure handling (load-bearing — the oracle can fail on a shape):** if Go
  panics / errors / exits non-zero on a golden (e.g. the 3-deep nested-pattern
  shape), DO NOT cache that as `expected_go.txt` (it would enshrine a non-zero exit
  as "correct"). Instead mark the golden `oracle_divergence = true` with a note + the
  reason, and use skyc's CORRECT output as the expected. See the parity-oracle caveat
  in `repo-layout-and-mirroring.md`. The refresh step must distinguish "Go produced a
  valid reference" from "Go failed" and route the latter to divergence, not to cache.
- Capture happens once, in the feature task that adds the golden (where the Go build
  already runs for first parity); reuse forever until invalidated.

## NOT doing: guardian review || e2e
Rejected. The guardian's adversarial probes are themselves SKY_E2E (CPU-heavy)
builds → contend with e2e on 4 cores + the target lock (not free LLM-only work).
Overlapping its reasoning with mech-e2e also breaks the cheap-first invariant
(don't spend Opus until mech green) → wasted tokens on the ~30-50% of batches where
mech fails. The right lever is making mech-e2e fast (Fixes 1+2), after which
sequential cheap-first costs little.
