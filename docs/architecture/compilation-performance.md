# Compilation performance

How `ipe build` / `ipe run` reach the build-time budget, why they miss it
today, and the ranked strategies that close the gap.

## The requirement

A user compiles **and runs a common program in milliseconds**, and a complex
application (server + database + UI) **in a few seconds**. This is a hard
requirement, not an aspiration: the Efficiency principle (`PRINCIPLES.md`)
covers the toolchain's own footprint on the developer's machine, and
"dev experience first" (`DEVELOPMENT.md` §6) makes the edit → run loop a
first-class product surface. A language whose hello world costs minutes of
`rustc` loses its users before its semantics matter.

The budget, per tier:

| Program | `ipe run` (dev loop) | `ipe build --release` |
|---|---|---|
| common (CLI, pure logic, small IO) | milliseconds | seconds |
| complex (server, Db, Ui, crypto) | a few seconds | tens of seconds |

## The current model

`ipe run` (`run_run` in `src/ipe-cli/src/lib.rs`) is a linear pipeline:

1. **ipe compile → emit** a self-contained Cargo project at `out/rust/`. The
   manifest comes from `src/compiler/backend/rust/templates/Cargo.toml`
   (embedded in `project.rs`); the runtime is **vendored as source** — ~96
   modules, ~3.4 MB, copied from `IPE_RUNTIME_DIR` into
   `out/rust/src/ipe_runtime/`, module set = the base
   `templates/ipe_runtime/mod.rs` plus kernel-driven appends.
2. **`cargo build`** the emitted project.
3. Resolve capabilities, jail, **exec** `target/debug/ipe-app`.

An emit-side cache exists (`src/ipe-cli/src/cache.rs`: IR-keyed,
compiler-revision epoch) — it skips re-emission, never the cargo build.

Optional heavy surfaces are already usage-gated by manifest surgery
(`project.rs` + the `crate_specs.rs` version SSOT): sqlx, axum, reqwest,
lettre, wry/tao, crossterm, flate2/zstd, csv, jsonwebtoken, aes-gcm, … appear
only when a reachable kernel needs them.

### Why hello world still compiles 110 crates

Gating stops at the optional surfaces. The **floor is fixed** — the base
template ships the same `[dependencies]` for every program:

> tokio, serde (+ `serde_derive`), serde_json, serde_urlencoded, regex,
> unicode-general-category, base64, hex, percent-encoding, chrono, chrono-tz,
> rust_decimal, hmac, sha2, subtle, zeroize, rsa, getrandom, uuid, bcrypt,
> url, futures-util (+ libc on unix)

`cargo tree` over that manifest resolves **110 crates**. The heavy transitive
roots:

| Root | Subtree | Pulled in by |
|---|---|---|
| `rsa` | 34 crates (num-bigint-dig, RustCrypto der/pkcs stack) | `Auth` token signing |
| `url` | 33 crates (idna → ICU4X: icu_normalizer, zerovec, yoke + derive macros; `syn` twice) | `Url` kernels |
| `bcrypt` | 14 | `Auth` password hashing |
| `rust_decimal` | 11 | `Decimal`/`Money` |
| `sha2`, `futures-util` | 10 each | crypto core, task plumbing |
| `chrono-tz` | 8 (embedded tz database) | `Time` zones |
| `tokio`, `serde` | 7 each | `block_on` entry, codecs |

`Io.println` needs none of them, yet the emitted entry point still wraps
`ipe_main` in `block_on` — every program spins tokio.

### Where the time goes

1. **rustc over ~109 dependency crates** — dominant. Minutes cold on a user
   machine; 16 s even on a warm sccache + mold dev box (measured: base
   manifest + trivial main).
2. **rustc over the one emitted crate** — user code plus the entire vendored
   3.4 MB runtime, recompiled per project and per emit change.
3. **Link** — ~1 s with mold; several seconds with the default linker.

`docs/rust-perf-improvement.md` documents the opt-in per-machine mitigations
(sccache, mold/lld, cranelift). They shave constants; they do not change the
structure that makes hello world pay for `rsa`.

## Strategies

Scored against the strict precedence Security > Correctness > Soundness >
Efficiency > Completeness > Readability — **and** against total adherence to
all six. Efficiency is the axis currently failing; no strategy may buy it back
by weakening a higher principle.

### S1 — Usage-driven dependency floor (gate everything, floor = std)

**Mechanism.** Extend the existing kernel-driven module/manifest appends until
the base floor is (near) dependency-free. Each heavy root — rsa, bcrypt,
url/idna, chrono-tz, regex, rust_decimal, uuid, the serde stack, tokio itself
— appears only when a reachable kernel needs it; a program with no async
kernel gets a synchronous `fn main`. Per-kernel dependency attribution is
fail-closed under THE SEAL: a runtime module whose crate the manifest lacks is
a compile-time drift error (the `crate_specs_match_manifests` pattern), never
a cargo failure. No security-bearing parser is replaced by a hand-rolled
"tiny" one — `serde_json`/`url` stay wherever `Json`/`Url` is actually used;
gating removes them where they are not.

**Buys.** 110 → ~0–10 crates for common programs; cold builds minutes →
seconds; smaller binaries.

**Risks.** Attribution table maintenance (mitigated by the existing drift
guards); more manifest variants to test.

**Principles.** Security **improves** (smaller supply-chain and attack
surface, smaller `cargo-deny` scope, less code in the shipped binary).
Correctness/Soundness neutral — same rustc, same code, only less of it.
Efficiency: order-of-magnitude. Completeness neutral (capability appears on
use). Readability of the emit improves. **Highest total adherence.**

### S2 — Build-once shared runtime target (local, from source)

**Mechanism.** `ipe` manages a shared `CARGO_TARGET_DIR` under
`~/.cache/ipe/`, keyed by (ipe version, toolchain, resolved feature set). The
first build on a machine compiles the dependency closure from source, once;
every subsequent project reuses the compiled artifacts. Reuse correctness is
cargo's own fingerprinting — no new trust or equivalence argument.

**Buys.** Per-project builds collapse to *user crate + link* ≈ ~1 s warm.
Multiplies with S1: a small floor makes even the first build cheap.

**Risks.** Disk footprint (the existing reclaim discipline covers it);
concurrent builds (cargo's own locking).

**Principles.** Security/Correctness/Soundness neutral — everything is still
built from source, locally, against the pinned lockfile. Efficiency: large.

### S3 — Precompiled runtime crate instead of vendored source

**Mechanism.** Make `ipe_runtime` a real dependency (path crate with cargo
features mirroring today's module trimming) rather than 3.4 MB of source
copied into every project. Compiled once into the S2 shared target, reused by
every project on the machine; the emitted project shrinks to the user's code.
The runtime version pins exactly to the compiler version (already released
together).

**Buys.** Removes the per-project runtime recompile — the second-largest cost.

**Risks.** Module trimming becomes feature flags (a mechanical mapping);
feature unification across projects handled by the per-feature-set cache key.

**Principles.** Neutral on Security/Correctness/Soundness; strong Efficiency;
Readability of the emitted project improves (user code only).

### S4 — Shipped prebuilt binary artifacts (rlib closure / prewarmed cache)

**Mechanism.** Download per-platform, per-toolchain precompiled rlibs (or a
prewarmed sccache dir) so even the first build links instead of compiles.

**Buys.** Fast first-run out of the box.

**Risks.** **Security**: users execute shipped object code they did not
compile; provenance, signing, and reproducible-build verification become
load-bearing. Rust's unstable ABI/metadata ties every artifact to one exact
toolchain; a platform × toolchain matrix to host and audit.

**Principles.** The naive form trades Security for Efficiency — **rejected on
precedence**. Acceptable only as a later, optional channel with reproducible
builds verified against source and signature checking; S1+S2 already make the
first build cheap enough that the payoff is small.

### S5 — Prelinked `ipe-std` dylib (`-C prefer-dynamic`)

**Mechanism.** Dev binaries dynamically link one locally-built
`libipe_std.so`; release builds stay static.

**Buys.** Link-only user builds; smaller relinks.

**Risks.** Rust's unstable ABI ties the dylib to the exact rustc (fails
loudly, so an ops nuisance, not a soundness hole); dev/release divergence in
link mode; marginal gain over S2 + mold.

**Principles.** Neutral on the top three; modest Efficiency; adds a moving
part (Readability/maintainability cost). Optional refinement, not a pillar.

### S6 — IR interpreter for `ipe run` (AOT stays for `ipe build`)

**Mechanism.** `ipe run` executes `ipe_ir` directly (tree-walking interpreter
or bytecode VM); `ipe build` and `--release` keep the rustc AOT path
unchanged. The only strategy that removes rustc from the dev loop — and
therefore the only route to true milliseconds.

**Buys.** Common-program `ipe run` in ~10–100 ms, independent of crate count.

**Risks.** **Dual semantics** — an interpreter that disagrees with the emitted
Rust is a Correctness violation. Mitigation is mandatory and structural: a
differential gate runs every golden and every example under both engines and
byte-compares observable output (the same oracle discipline the backend
already applies). The interpreter obeys the same no-panic soundness rules as
the runtime, and runs under the same capability jail — dev execution is never
less confined than production. `Rust.` FFI crossings are not interpretable:
programs that use them fall back, fail-closed, to the AOT path.

**Principles.** Security neutral (shared jail, and no cargo/rustc invocation
on untrusted-input paths). Correctness conditional on the differential gate —
without it, rejected; with it, equivalent in kind to the existing Go-oracle
discipline. Soundness: same bar as the runtime. Efficiency: transformative —
the only way to meet the ms budget. Completeness: initially partial (FFI
programs use AOT), explicitly fail-closed. Largest engineering cost.

### S7 — Cranelift codegen for dev builds

**Mechanism.** `rustc_codegen_cranelift` for debug codegen (2–5× over LLVM),
as `docs/rust-perf-improvement.md` documents. Auto-use when installed.

**Principles.** Neutral everywhere except a constant-factor Efficiency win.
Nightly-only component → cannot be the default floor; stays opt-in.

### S8 — Linker + profile defaults

**Mechanism.** Prefer mold/lld when present; the emitted profile already sets
`debug = 0`, `incremental = true`. Free constant-factor wins; keep.

## Ranking

| Rank | Strategy | Budget impact | Principle balance |
|---|---|---|---|
| 1 | S1 usage-driven floor | 110 → ~0–10 crates; minutes → seconds cold | improves Security *and* Efficiency; nothing traded |
| 2 | S2 shared build-once target | repeat builds → ~1 s | pure Efficiency, all else neutral |
| 3 | S3 precompiled runtime crate | removes 3.4 MB/project recompile | Efficiency + emit Readability |
| 4 | S6 IR interpreter for `ipe run` | seconds → milliseconds | Efficiency transformative; Correctness gated by mandatory differential oracle |
| 5 | S8 linker/profile defaults | link 3–5 s → ~1 s | free |
| 6 | S7 cranelift (opt-in) | 2–5× debug codegen | free but nightly-only |
| 7 | S5 `ipe-std` dylib | marginal over S2+S8 | adds an ABI-pinned moving part |
| 8 | S4 shipped prebuilt binaries | fast first run | naive form trades Security for Efficiency — rejected on precedence; revisit only signed + reproducible |

## Recommendation

A combination, adopted in dependency order:

1. **S1 + S3**: make the dependency floor and the runtime usage-driven — one
   runtime crate with per-kernel features, manifest floor near std-only,
   attribution fail-closed under THE SEAL. This is the structural fix: it
   improves Security while removing most of the cost, and every other
   strategy compounds on it.
2. **S2 + S8**: an ipe-managed shared target keyed by (version, toolchain,
   feature set), plus linker defaults. Together with S1 this puts common
   programs at ~1 s and complex apps within the few-seconds budget — from
   source, with no new trust surface.
3. **S6**: the IR interpreter behind `ipe run`, landing only together with its
   differential gate (every golden + example, both engines, byte-compared) and
   the shared capability jail. This alone reaches the milliseconds budget.

S7 stays a documented accelerator; S5 is an optional refinement if link time
ever dominates again; S4 is rejected in its naive form and only worth
revisiting as a signed, reproducible artifact channel.

**Next investigation.** Validate the top of the ranking empirically: take
`examples/sky/ipe/01-hello-world`, hand-trim the emitted manifest and runtime
module set to the true floor its kernels need, and measure cold and warm
build+run times against the current 110-crate emit — that single measurement
proves (or refutes) the S1+S2 budget claim before any emitter work. The
follow-on spike is a tree-walking evaluator over `ipe_ir` for the pure kernel
floor, differential-tested against the AOT output of the same goldens.
