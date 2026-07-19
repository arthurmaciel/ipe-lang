Status: Accepted
Date: 2026-07-03

# 0026. Go-parity divergence oracle and tiered verification

## Context

The correctness contract (PRINCIPLES.md §2) is that for the same well-typed Ipê
program and the same input, the Rust output matches the Go reference's observable
behaviour — ideally byte-for-byte. With multiple golden tests and an examples sweep,
there needed to be a single, versioned record of which Go outputs are the ground
truth, so that "does ipe match Go?" could be answered without running Go live in the
hot path. Three related problems required simultaneous decisions:

1. Each E2E golden rebuilt its emitted Rust project in its own per-golden target dir,
   causing heavy deps (tokio/rsa/syn) to recompile for every golden.
2. The Go reference output was recomputed live on every parity run, even though it is
   a pure function of (Main.ipe, Go `sky` version) and changes only when those change.
3. A divergence from Go output needed to be clearly categorised as a bug (fix it),
   a sanctioned deliberate choice (document it), or a Go-failure case (Go is wrong).

## Decision

Introduce the `tools/oracle` crate and `refresh-oracle` tool encoding three policies:

- **Default — byte parity.** Every runnable golden caches the Go reference's clean
  stdout as `tests/golden/*/expected_go.txt` + `oracle.meta`. `oracle::check_parity`
  diffs ipe's output against it on every run; a mismatch is a hard failure.
- **Three tagged divergence kinds** (never silent), all via `oracle_divergence = true`
  in `oracle.meta`: (1) `go-fail:` — Go panics/exits-non-zero on a shape ipe handles;
  (2) `sanctioned:` — deliberate, principle-justified departure recorded with rationale;
  (3) `ipe-gap:` — ipe not yet handling a shape, Go is correct. Oracle files regenerated
  only by `refresh-oracle`, never hand-edited.
- **Shared emitted-project target** (`IPE_ORACLE_SHARED_TARGET`): all E2E goldens build
  into one machine-global cargo target so heavy deps compile once. The `oracle` crate
  rewrites each emitted project's package name to be unique, so binaries coexist.

The oracle is live Go (`sky v0.16.29` / `go1.26.2`) for the full examples sweep; the
cached `expected_go.txt` snapshot for the fast unit golden suite.

## Consequences

- Adding or changing a golden that touches Go-observable output requires `refresh-oracle`
  to regenerate the cache; PRs that forget this fail CI immediately.
- A new deliberate divergence from Go must carry `sanctioned:` + rationale in
  `oracle.meta` AND an entry in `docs/divergences-from-sky.md` — two-factor recording.
- The shared emitted-project target serialises concurrent E2E builds (one cargo lock),
  which is acceptable because per-dep compile cost vastly exceeds the lock wait.
- Downstream: the examples sweep (`scripts/equivalence-checks/examples-sweep.sh`) and
  the full gate both rely on this oracle; weakening the oracle is a correctness regression.
