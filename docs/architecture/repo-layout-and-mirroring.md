# Repo layout, project coupling, and the upstream-mirroring model

> **Status:** plan / decision record. Written 2026-06-27. Not yet executed.
> **Owner:** sky-rust coordinator (downstream of upstream Sky by anzellai).

## 0. Pivot (2026-06-27): drop the fork's Haskell Rust backend; mirror behaviour, not bytes

The Haskell compiler's Rust backend (`Generate/Rust/*`) **and** `runtime-rust`
were authored by *us* (the coordinator + agents), not by anzellai — `runtime-rust`
is already the **canonical** Rust runtime. We are **abandoning the fork's Haskell
Rust backend** and focusing solely on **sky-rust (compiler + Rust backend + Rust
runtime)**, mirroring **functionality/behaviour** against anzellai's mainline:
the Haskell **frontend** (parse/canon/types semantics) and the **Go**
backend+runtime (observable behaviour).

Why this is correct, not a compromise:
- Byte-matching the fork's Haskell-emitted Rust was **circular** — it only proved
  "I reproduced my own earlier output," and faithfully reproduced its bugs. A
  bootstrap crutch, never a real oracle.
- `PRINCIPLES.md` point 2 already defines correctness as *"match the **Go
  reference's** observable behaviour."* The Go backend/runtime is the mature,
  external reference. So the pivot **returns to the stated principle**; the
  byte-diff was the temporary proxy.

## 1. The forces (why this is not a normal "merge the repos" question)

- **We are downstream.** Upstream Sky (the Haskell compiler + Go backend/runtime,
  by anzellai) is fast-moving, brilliant, industrial-grade. Our standing policy:
  **always pull upstream improvements into sky-rust**, and the **Rust runtime is a
  behavioural mirror of the Go runtime** (born to mirror it; may diverge later,
  deliberately, once we have the skill to design our own).
- **Two distinct mirrors, two distinct references — both anzellai's mainline:**
  1. **Haskell frontend → Rust compiler** (parse/canon/types/lower *behaviour*).
     Enforced **end-to-end**: the program must compile + run the same, and reject
     the same ill-formed inputs (a should-reject corpus + the error-code system).
  2. **Go runtime/backend → Rust runtime/backend** (kernel + emission *behaviour*).
     Enforced by **behavioural parity**: same Sky program, `sky`(Go) output vs
     `skyc`(Rust) output (stdout/exit/HTTP), the example sweep, ported Sky.Test.
- **Emission is snapshot-tested, not byte-diffed against Haskell.** A self-owned
  insta-style snapshot of `skyc`'s output is a *regression guard* ("did my codegen
  change, and did I mean to?"), not a correctness oracle. Codegen *quality* (worse
  Rust, same behaviour) is covered by perf-sweep + clippy/Miri on emitted code.
- **v0.17 "fully-typed codegen" is landing soon** (upstream
  `feat/v0.17-fully-typed-codegen`). `sky-rust` is **already** type-directed /
  fully-typed by design, so v0.17 is *convergent*. With the byte-diff gone, v0.17
  no longer forces a golden re-baseline — it's a *semantics* sync (port any new
  typing/lowering behaviour), validated by behavioural parity.
- **anzellai's mainline (Haskell frontend + Go backend/runtime) is the reference**
  and must stay reachable from our test harness (a released `sky` binary +
  `runtime-go` source) for the foreseeable future. We do **not** maintain the
  fork's Rust backend.

Conclusion: the coupling decision is really "how do we make **continuous mirroring
of a fast upstream** sustainable," and the repo layout serves that.

## 2. Decisions

1. **Monorepo for our work**, with the Rust compiler + Rust backend + Rust runtime
   co-located, and **upstream Sky vendored as a pinned, read-only dependency**
   (git submodule or subtree, pinned to a tag, e.g. `v0.17.0`). The pinned
   upstream supplies *both* references: a built/released `sky` binary with the
   **Go** backend (the behavioural oracle + the Haskell frontend as semantics
   reading) and `runtime-go/` source (the runtime-mirror reference). The fork's
   Haskell **Rust** backend is **not** vendored — it is abandoned (see §0).
2. **Preserve the `sky_ir` backend boundary.** Co-locating must not privilege
   Rust. The runtime lives *under the backend*, so future backends slot in
   cleanly.
3. **Version-lock to upstream.** A `sky-rust` release is tagged against the Sky
   version it mirrors (`sky-rust v0.17.x` mirrors Sky `v0.17.x`). This makes
   "which runtime does this compiler emit against" unambiguous and compiler↔runtime
   skew impossible.
4. **Timing: do the disruptive reorg ONCE, bundled with the v0.17 sync.** Don't
   merge now and again at v0.17. See §5.

## 3. Target layout

```
sky-rust/                          # the monorepo
  crates/                          # backend-AGNOSTIC compiler
    sky_intern/ sky_diagnostics/
    sky_syntax/ sky_parse/ sky_canon/ sky_types/
    sky_ir/                        # the single backend boundary
    sky_lower/
    sky_backend/                   # the Backend trait
    skyc/                          # driver
  backends/
    rust/
      sky_backend_rust/            # the Rust emitter
      runtime/                     # sky_runtime — MIRRORS runtime-go module-for-module
    # future: backends/wasm/, backends/c/, each with its own runtime/
  vendor/
    upstream-sky/                  # anzellai's mainline sky, submodule pinned to a tag
                                   #   -> sky (Haskell+Go) = behavioural oracle
                                   #   -> Haskell frontend  = semantics reading
                                   #   -> runtime-go/        = runtime mirror reference
                                   #   (the fork's Haskell Rust backend is NOT here)
  tests/
    snapshots/                     # self-owned insta snapshots of skyc's emission (regression guard)
    parity/                        # behavioural diffs: sky(Go) output vs skyc(Rust) over upstream examples
  docs/
    parity/
      compiler-parity.md           # ledger: upstream feature -> rust-compiler status
      runtime-parity.md            # ledger: each Go runtime fn -> rust runtime status
    architecture/                  # this file, BUMPING-EDITIONS, etc.
```

Notes:
- `vendor/upstream-sky` is **read-only** to us; we never patch it, we bump its pin.
- The runtime moves from `sky/runtime-rust` into `backends/rust/runtime/` and is
  **embedded** into `skyc` (via `include_dir!`/`build.rs`, mirroring how the
  Haskell binary TH-embeds its runtime), killing the current sibling-path
  `resolve_runtime` hack and making CI self-contained.
- During the port, the pinned upstream's `sky` (Haskell+**Go**) is built/used in CI
  to run behavioural parity diffs and as the semantics reference. The fork's
  Haskell Rust backend is not built or used.

## 4. The mirroring model (the sustainable SOP)

Treat **upstream (anzellai's mainline) as the spec**, the **parity harness as the
enforcer**, and the **two ledgers as the backlog**. Port *behaviour*; let the Go
reference *prove* we matched it — no deep design decision per feature. Emission is
snapshot-guarded for regressions, not pinned to any external bytes.

### 4a. Compiler mirror (Haskell frontend + Go backend → sky-rust)
Per upstream release:
1. Bump `vendor/upstream-sky` pin to the new tag.
2. Run the **behavioural diff**: compile + run each example with the pinned
   `sky`(Go) AND `skyc`(Rust); compare stdout / exit / HTTP. Run the should-reject
   corpus: both must reject the same ill-formed inputs (our error-code message is
   ours; the *accept/reject decision* must match).
3. Refresh `tests/snapshots/` only when WE intentionally change emission (insta
   review) — it is a regression guard, not a spec.
4. Triage behavioural divergences; for each, port the upstream change into the
   right stage (`sky_parse`/`sky_canon`/`sky_types`/`sky_lower`/`sky_backend_rust`).
5. Update `docs/parity/compiler-parity.md`; re-green; tag `sky-rust vX.Y`.
The existing `sky-rust-backend` skills are this machinery — `sync-with-upstream`,
`build-sweep`, `run-sweep`, `web-sweep`, `perf-sweep`.

### 4b. Runtime mirror (Go → Rust)
- Keep `backends/rust/runtime/sky_runtime/` **structurally 1:1** with
  `vendor/upstream-sky/runtime-go/rt/` — same module names, same function names
  (snake_cased). "Go added fn X to module Y" → "add X to Rust module Y" becomes a
  mechanical, reviewable diff.
- `tests/parity/` asserts **same observable output** per kernel/example (Go vs
  Rust). The `keep-go-parity` skill drives this.
- `docs/parity/runtime-parity.md` is the per-function ledger: `present` /
  `missing` / `diverged`. **Deliberate divergence is allowed but MUST be recorded
  with a rationale + its own tests** (PRINCIPLES: a divergence is documented, never
  silently wrong). This ledger is also the graduation path from *follower* to
  *designer*: a function flips from "mirrors Go" to "intentional Rust design +
  rationale" one entry at a time, low-risk and reversible.

### 4c. Why the layout serves the mirror
- Backend-agnostic `crates/` means upstream *frontend* changes (parse/types/lower
  semantics) land in one place and are shared by every backend.
- Per-backend `runtime/` means upstream *runtime* changes touch only
  `backends/rust/runtime/`, diffed against the vendored `runtime-go`.
- The `sky_ir` boundary means when you eventually *design* (not mirror) — a new
  backend, or a Rust-native runtime feature — you change one layer without
  disturbing the upstream-tracked frontend.

## 5. Migration plan (sequenced)

**Do not start until the in-flight error-code phases (E/F) have landed and
`sky-rust` is green.** Then bundle the reorg with the v0.17 sync + the edition-2024
unification (see `BUMPING-EDITIONS.md`) so the tree is disrupted **once**.

1. **Wait for / fetch v0.17.** When `feat/v0.17-fully-typed-codegen` releases,
   read its codegen + typing notes; because we are already type-directed, expect a
   *semantics* sync, not a re-baseline. Map any new typing/lowering concepts onto
   `sky_ir`/`sky_lower`.
2. **Vendor upstream mainline.** Add `vendor/upstream-sky` as a submodule pinned to
   `v0.17.0`. Wire CI to build/use `sky` (Haskell+**Go**) as the behavioural oracle
   + the `runtime-go` source. (Not the fork's Rust backend.)
3. **Move the runtime in.** Relocate `runtime-rust` → `backends/rust/runtime/`
   (it is already canonical and ours); switch `skyc` from sibling-path copy to
   embedded (`include_dir!`/`build.rs`); delete the `resolve_runtime` upward-search
   hack. No upstream coordination needed — the fork's Rust backend is retired, so
   nothing else consumes `runtime-rust`.
4. **Apply the edition-2024 unification** (now a *one-repo* change — the emitted
   edition is purely ours; no Haskell-emitter coordination — see the simplified
   `BUMPING-EDITIONS.md`). Refresh `tests/snapshots/` via insta review.
5. **Stand up the ledgers + parity harness** (`docs/parity/*`, `tests/parity/`)
   driving `sky`(Go) vs `skyc`(Rust) over the pinned upstream's example set.
6. **Tag `sky-rust v0.17.0`** mirroring Sky v0.17.0. From here, every upstream
   release is a pin-bump + the §4 SOP.

## 6. Open questions / notes

- v0.17 is a *semantics* sync, not an emission re-baseline (byte-diff is gone).
  Still worth reading its typing model to mirror behaviour precisely.
- `backends/rust/runtime/` is the **sole canonical** Rust runtime (the fork's
  Haskell Rust backend that previously also vendored a copy is abandoned) — no
  dual maintenance.
- Long-term, anzellai's mainline (`sky` Haskell+Go) stays the behavioural oracle
  and the runtime-mirror source; the monorepo makes that a local pinned submodule
  rather than a fragile sibling path. Retiring it as the oracle is a far-future
  decision (only once sky-rust is independently trusted).

## 7. One-line summary

Abandon the fork's Haskell Rust backend; mirror **behaviour** (not bytes) against
anzellai's mainline. Monorepo our compiler + Rust backend + Rust runtime, **vendor
upstream pinned to a tag** as the dual reference (Go behavioural oracle +
`runtime-go` mirror source), enforce with behavioural-parity + two ledgers
(emission is snapshot-guarded), version-lock to upstream, and do the reorg **once**,
bundled with the v0.17 sync + the (now one-repo) edition-2024 bump — after the
current error-code phases land.

## Parity-oracle caveat — the Go reference may differ from the target

Behavioural parity vs the Go backend is the correctness oracle, but the oracle
records Sky's *current* behaviour, which may differ from Sky-Rust's target on
some shapes. When a parity mismatch occurs, triage it first: determine which
side is correct, record the difference as a documented divergence
(`divergence-policy.md`), and never silently accept a mismatch as a regression.

Known instance: deeply nested constructor patterns (≥3 deep, e.g.
`Som (Som (Som x))` discrimination) — the Go oracle exits non-zero on this
shape; Sky-Rust compiles + runs correctly. This routes to the auto Go-failure
divergence branch: skyc's output is recorded as the expected value.
