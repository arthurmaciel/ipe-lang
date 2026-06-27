# Repo layout, project coupling, and the upstream-mirroring model

> **Status:** plan / decision record. Written 2026-06-27. Not yet executed.
> **Owner:** sky-rust coordinator (downstream of upstream Sky by anzellai).

## 1. The forces (why this is not a normal "merge the repos" question)

- **We are downstream.** Upstream Sky (the Haskell compiler + Go backend/runtime,
  by anzellai) is fast-moving, brilliant, industrial-grade. Our standing policy:
  **always pull upstream improvements into the Sky Rust compiler**, and the
  **Rust runtime is a behavioural mirror of the Go runtime** (born to mirror it;
  may diverge later, deliberately, once we have the skill to design our own).
- **Two distinct mirrors, two distinct references:**
  1. **Haskell compiler → Rust compiler** (frontend + lowering *behaviour*). The
     Haskell compiler is the spec; the **golden byte-diff oracle** enforces it.
  2. **Go runtime → Rust runtime** (kernel *behaviour*). The Go runtime is the
     reference; **behavioural parity tests** (same Sky program, Go vs Rust
     output) enforce it.
- **v0.17 "fully-typed codegen" is landing soon** (upstream
  `feat/v0.17-fully-typed-codegen`). `sky-rust` is **already** type-directed /
  fully-typed by design, so v0.17 is *convergent*, not disruptive to our
  architecture. Its main concrete effect on us: the Haskell Rust-backend's
  *emitted bytes* will shift, so our **golden re-baselines** against v0.17.
- **The Haskell compiler is still our correctness oracle**, so it (and the Go
  runtime) must stay reachable from our test harness for the foreseeable future.

Conclusion: the coupling decision is really "how do we make **continuous mirroring
of a fast upstream** sustainable," and the repo layout serves that.

## 2. Decisions

1. **Monorepo for our work**, with the Rust compiler + Rust backend + Rust runtime
   co-located, and **upstream Sky vendored as a pinned, read-only dependency**
   (git submodule or subtree, pinned to a tag, e.g. `v0.17.0`). The pinned
   upstream supplies *both* references: the Haskell compiler (golden oracle) and
   the Go runtime source (the runtime mirror reference).
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
    upstream-sky/                  # anzellai's Haskell sky, submodule pinned to a tag
                                   #   -> the Haskell compiler (golden oracle)
                                   #   -> runtime-go/ (the runtime mirror reference)
  tests/
    golden/                        # emitted-Rust byte targets, regenerated from vendor/upstream-sky
    parity/                        # Go-vs-Rust behavioural diffs over upstream examples
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
- During the port, the pinned upstream's Haskell compiler is built with its own
  toolchain in CI to (re)generate goldens and run parity diffs.

## 4. The mirroring model (the sustainable SOP)

Treat **upstream as the spec**, the **golden + parity harness as the enforcer**,
and the **two ledgers as the backlog**. This lets us track a fast upstream
mechanically — port *behaviour*, let the oracle *prove* we matched it — without a
deep design decision on every feature.

### 4a. Compiler mirror (Haskell → Rust)
Per upstream release:
1. Bump `vendor/upstream-sky` pin to the new tag.
2. Regenerate `tests/golden/` from the pinned Haskell compiler (`--backend rust`).
3. Run the **byte-diff oracle** + the **behavioural diff** (compile+run each
   example with the pinned Haskell compiler AND `skyc`, compare stdout/exit).
4. Triage divergences; for each, port the upstream change into the right pipeline
   stage (`sky_parse`/`sky_canon`/`sky_types`/`sky_lower`/`sky_backend_rust`).
5. Update `docs/parity/compiler-parity.md`; re-green; tag `sky-rust vX.Y`.
The existing `sky-rust-backend` skills are exactly this machinery —
`sync-with-upstream`, `build-sweep`, `run-sweep`, `web-sweep`, `perf-sweep`.

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
   read its codegen notes; because we are already type-directed, expect *fewer*
   divergences, possibly a smaller golden diff. Study the branch to map any new
   IR/typing concepts onto our `sky_ir`/`sky_lower`.
2. **Vendor upstream.** Add `vendor/upstream-sky` as a submodule pinned to
   `v0.17.0`. Wire CI to build its Haskell compiler + Go runtime.
3. **Move the runtime in.** Relocate `runtime-rust` → `backends/rust/runtime/`;
   switch `skyc` from sibling-path copy to embedded (`include_dir!`/`build.rs`);
   delete the `resolve_runtime` upward-search hack. (Coordinate with the upstream
   repo if it still vendors `runtime-rust` — at this point the monorepo owns the
   canonical Rust runtime; upstream's Haskell Rust-backend can reference the
   vendored copy or be retired.)
4. **Re-baseline goldens** from the pinned v0.17 Haskell compiler; apply the
   edition-2024 unification here (one window — see `BUMPING-EDITIONS.md`).
5. **Stand up the ledgers + parity harness** (`docs/parity/*`, `tests/parity/`)
   from the pinned upstream's example set.
6. **Tag `sky-rust v0.17.0`** mirroring Sky v0.17.0. From here, every upstream
   release is a pin-bump + the §4 SOP.

## 6. Open questions to resolve with upstream's author

- Does v0.17 change the *emitted Rust* shape materially (it will move toward
  typed; quantify the golden delta once it lands)?
- Is upstream willing to treat the monorepo's `backends/rust/runtime/` as the
  canonical Rust runtime (so it isn't maintained in two places)? If not, keep the
  submodule one-directional and accept the runtime lives upstream until we fully
  own it.
- Long-term: when does the Haskell compiler get retired as the oracle? Until then
  the vendored upstream is load-bearing; the monorepo makes that dependency local
  and pinned rather than a fragile sibling path.

## 7. One-line summary

Monorepo our compiler + Rust backend + Rust runtime, **vendor upstream Sky pinned
to a tag** as the dual reference (Haskell oracle + Go-runtime mirror source),
enforce both mirrors with the golden + parity harness and two ledgers, version-lock
to upstream, and do the reorg **once**, bundled with the v0.17 sync and the
edition-2024 bump — after the current error-code phases land.
