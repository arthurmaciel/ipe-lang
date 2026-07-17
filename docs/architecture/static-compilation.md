# Static compilation

Status: design (spec-only). No code, no build wired yet.

This document specifies fully-static, portable single-binary artifacts for
ipê, and the allocator chosen to avoid the static-libc malloc cliff without
reflexively taking a C dependency. ipê emits a self-contained Rust cargo
crate (`crates/sky_backend_rust` + the `crates/skyc/src/lib.rs` build path)
and then cargo-builds it; static compilation is a build-config concern layered
over that emitted crate.

Decisions are gated on the project principle order:
**security > correctness > soundness > efficiency > completeness > readability**,
and on the two fundamental rules — *parse, don't validate* and *make invalid
states unrepresentable*. An unbuildable target/allocator/platform combination,
or the silent musl-malloc throughput cliff, must be refused or explicitly
acknowledged — never silently produced.

## Summary

1. Adopt musl-static Linux linking from the reference implementation (`../sky`)
   essentially verbatim. The linking scaffolding there is mature and
   principle-aligned; the single principle-relevant re-decision is the allocator.
2. **Default allocator = `dlmalloc` (pure Rust)** for every static / musl / wasm
   build. This flips `../sky`'s mimalloc default. Pure Rust removes the
   `build.rs` + C toolchain + unsafe C-FFI + frozen opaque-C-blob supply-chain
   surface from every static build — a principle-#1 (security) charge that a
   pure-Rust allocator does not levy. dlmalloc clears the musl cliff decisively
   and, being Rust std's wasm allocator, unifies the native-static and wasm
   allocator into one audited dependency.
3. `system` allocator on glibc-dynamic (the common dev build) — do-nothing,
   zero added surface, 1.00× baseline.
4. `mimalloc` demotes to an explicit, documented, benched **opt-in** for
   profiled high-concurrency servers that measured an allocator bottleneck and
   accept the C-toolchain cost. `jemalloc` / `snmalloc` are documented and
   rejected — not wired as selectable choices.
5. `--allocator system` on a musl-static target is a **hard refusal** unless
   `--allow-slow-allocator` is also passed. The 0.14× cliff must be
   constructible only on purpose, never by AUTO or a bare flag.
6. macOS cannot be fully-statically linked (no static libSystem). `--static`
   targeting a macOS artifact is **refused** with a cross-to-musl hint or a
   distinct `--macos-portable` opt-in. No silent degrade-to-dynamic.
7. Windows "static" means static-CRT via `+crt-static`: MSVC primary on a
   Windows host, `-gnu` for cross-from-Linux.
8. The pure-Rust default plus the runtime's existing rustls TLS gives a C-free
   default dependency graph, so musl cross-builds run on
   `rustup target add … + rust-lld` with **no external C cross-toolchain**.
   `cargo-zigbuild` is the fallback only when a C dependency is present
   (mimalloc opt-in, or the Compression feature's `zstd`).
9. Static linking freezes bundled dependencies into the artifact, so the
   CVE-response path is rebuild-and-redeploy: committed `Cargo.lock`, reproducible
   `--locked` rebuilds, a `cargo audit` / `cargo deny` CI gate, and a per-artifact
   SBOM. The pure-Rust default shrinks this frozen surface (no opaque C allocator).
10. The build is modelled as a typed `BuildPlan` sum type constructed through a
    smart constructor returning `Result<BuildPlan, Refusal>`; invalid combos are
    unrepresentable downstream of the parse.
11. The measure-before-finalize gate benchmarks `{musl-malloc, dlmalloc, mimalloc}`
    on the actual ipê runtime. Its output confirms the cliff is cleared and *sizes*
    the mimalloc opt-in recommendation — it does **not** flip the default. The
    principle order decides the default.
12. Divergence from `../sky` is limited to the allocator default and to tightening
    warn-paths into refusals; it is recorded as a sanctioned `oracle_divergence`.

## Allocator recommendation (locked)

**Choose the pure-Rust allocator `dlmalloc` as the default; do not take a C
allocator dependency by default.** "Locked" here means *locked by the principle
order*, not *locked by measurement*. The default is set by
`security > efficiency`: it eliminates the C toolchain / `build.rs` / unsafe C-FFI /
frozen-C-blob supply-chain surface from every static build (principle #1), and no
efficiency figure can promote a C dependency past that gate to become the default
(principle #4). The pure-Rust throughput numbers quoted below and in §1
(~5–11× over musl malloc, roughly glibc-class single-threaded; the residual
concurrency-only gap to mimalloc from a single global lock vs per-thread sharding)
are **predictions**, not measurements — no dlmalloc/talc figures exist on the ipê
runtime yet (see the §4.5 measure-before-finalize gate). The benchmark gate does
**not** flip the default; it confirms the cliff is cleared and *sizes* the
mimalloc opt-in recommendation. The one contingency in which it could flip the
default is the unlikely case that dlmalloc *fails* to clear the cliff under
concurrency — a data-driven exception recorded as an `oracle_divergence`, not an
efficiency override of the security gate. mimalloc remains available as an informed
opt-in for the operator who has *measured* a concurrent-allocation bottleneck.

---

## §1 — Allocator trade study

### The cliff is musl-malloc-specific, not a cost of avoiding C

`../sky`'s measured allocator 2×2 (`runtime-rust/docs/TECHNICAL-DETAILS.md`,
`Sky.Http.Server`, `ab -c50`, allocation-heavy):

| Variant | Throughput | RSS |
|---|--:|--:|
| A  dynamic + glibc | 1.00× (baseline) | 8.5 MB |
| B  dynamic + mimalloc | 1.72× | 16.3 MB |
| C  static-musl + mimalloc | 1.48× | 14.7 MB |
| D  static-musl + **musl malloc** | **0.14×** (the cliff) | 7.8 MB |

The doc's own characterisation: musl malloc is "not contention-driven … just
slow for high-volume small allocations." The 0.14× cliff is a weak
general-purpose free-list, not an intrinsic cost of a non-C allocator. Any
competent free-list allocator clears it. glibc's own ptmalloc is a dlmalloc
descendant. So the real question is not "can pure-Rust beat 0.14×" (it does,
trivially) but "does pure-Rust reach mimalloc's 1.48–1.72×, and if not, is the
gap worth a C toolchain in every static build?"

### Perf reality for the pure-Rust candidates

Both pure-Rust candidates use a single global lock (dlmalloc: global-lock arena;
talc: a spin/mutex-guarded single heap) rather than mimalloc's per-thread sharded
free-lists. Consequences:

- **Single-threaded / one-shot** (CLI, cron, TUI, batch — the majority of
  artifacts by count): dlmalloc and talc are ~glibc-class (≈0.9–1.1× A).
  mimalloc's advantage nearly vanishes.
- **Concurrent churn** (Sky.Live / Sky.Http.Server, `-c50` — where the allocator
  matters): a pure-Rust global-lock allocator is still dramatically faster than
  musl malloc (~4–6× above the cliff, ≈0.6–0.9× A) but trails mimalloc's 1.48×
  static-musl by roughly 1.5–3× under high-core contention.

Honest gap statement: choosing pure-Rust forgoes ~40–70% of concurrent-server
throughput headroom versus mimalloc, in exchange for removing a C toolchain, a
`build.rs`, an unsafe-FFI boundary, a frozen opaque-C blob, and a class of
C-allocator CVEs from every static build. Under `security > efficiency`, that
trade is correct as the default; the efficiency is recovered via an explicit
opt-in for operators who measured a need.

> Note on measured data: the only numbers that exist are `../sky`'s
> mimalloc-vs-musl-malloc figures. No dlmalloc/talc numbers exist on the ipê
> runtime. See the measure-before-finalize gate in §4; the perf estimates above
> are predictions the gate is expected to confirm.

### Trade-study table — 6 candidates × 7 axes

Scale: ✅ excellent · 🟢 good · 🟡 fair · 🔴 poor / blocking.

| Candidate | 1 Security (C-dep / build.rs / FFI / supply chain) | 2 Perf (vs cliff / vs glibc / vs mimalloc) | 3 Binary size | 4 Maintenance / maturity | 5 musl | 6 wasm | 7 no_std |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| **dlmalloc** (pure Rust) | ✅ no C, no build.rs, no FFI | 🟢 clears cliff; ≈glibc single-thread; 🟡 global-lock under heavy concurrency | ✅ small (+~30–60 KB) | 🟢 rust-lang org; **is the std wasm allocator** | ✅ trivial | ✅ **native — the wasm default** | ✅ |
| **talc** (pure Rust) | ✅ no C, no build.rs, no FFI; internal `unsafe` is Miri/geiger-auditable | 🟢 competitive single-thread; 🔴 worse than dlmalloc under contention | ✅ tiny; ≈musl-lean | 🟡 young; embedded/no_std focus; single-maintainer risk | ✅ trivial | ✅ | ✅ (designed for it) |
| **mimalloc** (C) | 🔴 vendored C, `cc` build.rs, unsafe FFI, C toolchain every static build, opaque frozen blob | ✅ **best** — 1.72× dyn / 1.48× static; per-thread sharded | 🟡 +~150–250 KB (14.7 MB static) | ✅ Microsoft, production | 🟢 works; needs C cross-linker | 🔴 no clean wasm | 🔴 no |
| **jemalloc** (C) | 🔴 C, build.rs, FFI, toolchain | ✅ mimalloc-class multi-thread | 🔴 largest (+~300–600 KB) | 🟡 upstream archived; tikv fork only | 🟡 historically awkward (page-size/TLS) | 🔴 no | 🔴 no |
| **snmalloc** (C++) | 🔴 **C++17** toolchain (heaviest), FFI | ✅ excellent (message-passing) | 🟡 moderate–large | 🟢 MS Research, newer | 🟡 C++-on-musl fiddly (libc++/libstdc++) | 🔴 no | 🟡 limited |
| **system** | ✅ nothing added | glibc: 🟢 1.00× baseline · **musl: 🔴 0.14× cliff** (bimodal) | ✅ zero; musl-static leanest (7.8 MB) | ✅ | 🔴 this is the cliff | n/a | n/a |

### Verdict, principle-ordered

1. **Security eliminates jemalloc and snmalloc outright.** Each carries the same
   (or worse — C++) toolchain / FFI cost as mimalloc with no compensating
   advantage for ipê's profile, plus worse musl / wasm / size (jemalloc upstream
   archived). Documented and rejected; not selectable.
2. **Security ranks dlmalloc / talc / system above mimalloc** (pure Rust vs C
   supply-chain surface).
3. **Among the pure-Rust pair, dlmalloc > talc.** Once security is tied,
   correctness/soundness/maturity (ranks #2–4) break the tie: dlmalloc is Rust
   std's wasm allocator, so it unifies native-static and wasm into one audited
   `#[global_allocator]` and one CVE-track; it lives in the rust-lang org with
   higher maturity; and it has better concurrency behaviour than talc's single
   spinlock. talc's only edges — a single-threaded microbench win and a slightly
   smaller binary — are efficiency/size (ranks #3–4) and do not overturn the
   order. talc's `Talck<Lock, OomHandler>` also exposes a lock/OOM-handler config
   surface (soundness cost) that dlmalloc's zero-config `GlobalDlmalloc` avoids.
4. **Efficiency (rank #4) is where mimalloc wins**, but only on concurrent
   workloads and only after the higher principles are satisfied. Efficiency
   cannot promote a C dependency past the security gate to become the default;
   it justifies mimalloc as an explicit opt-in.

**Winner: `dlmalloc`** — the highest-maturity allocator carrying zero C-toolchain
security surface that clears the musl cliff while unifying static-native with wasm.
`talc` is retained as a documented `no_std` / embedded override, not a mainline
default.

### AUTO rule, default, and override

CLI: `ipe build --static [--target <triple>] [--allocator <choice>] [--allow-slow-allocator]`

`--allocator` is a closed enum: **`{auto, system, dlmalloc, talc, mimalloc}`**.
Unknown values are rejected at parse time (no silent string-fallthrough).
`jemalloc` / `snmalloc` are not accepted values.

AUTO resolution by target:

| Resolved target | AUTO allocator | Rationale |
|---|---|---|
| host-native dynamic (glibc) | **system** | 1.00× baseline; adding a `#[global_allocator]` is needless surface |
| `*-linux-musl` static | **dlmalloc** | clears cliff; pure Rust; C-free with rustls |
| `*-windows-*` +crt-static | **dlmalloc** | pure Rust; no C allocator needed |
| `wasm32-*` | **dlmalloc** | already the wasm default; one audited allocator across targets |
| macOS | **system** | cannot be fully-static (§2); Apple malloc has no cliff |

Override semantics:

- `dlmalloc` / `talc` — pure Rust, no acknowledgment required. `talc` for
  `no_std` / embedded / RSS-lean niches.
- `mimalloc` — permitted; emits a one-line notice: *"mimalloc adds a C toolchain
  and unsafe FFI, vendors C source, and freezes it into the artifact for
  CVE-rebuild purposes; chosen explicitly."* Works on dynamic builds too
  (dynamic + mimalloc = the single fastest variant, B at 1.72×).
- `system` on a musl-static target → **hard refusal** unless
  `--allow-slow-allocator` is also passed. musl + system-malloc is a *valid but
  sharp* choice (leanest 7.8 MB RSS), so it is an acknowledgment gate, not a ban.
  The 0.14× cliff must be a deliberate two-key action, never an AUTO or bare-flag
  outcome.

Bonus security payoff: with dlmalloc (or talc) the musl-static build has no C
compile unit from the allocator, so it links with `rust-lld` and needs **no
`musl-gcc` cross-linker**. mimalloc forces the musl C cross-linker. Choosing the
pure-Rust default removes the last allocator-side C toolchain dependency from the
static path — a concrete DX and supply-chain simplification independent of the
raw perf delta.

---

## §2 — Per-platform static-target matrix (Q1)

| Platform | Target(s) | "Static" means | Policy |
|---|---|---|---|
| Linux (portable) | `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | fully static ELF, zero runtime deps | Adopt `../sky` verbatim. Runs on scratch/distroless/Alpine/any glibc distro alike. |
| Linux glibc | `*-linux-gnu` + `+crt-static` | *partial* only | Not offered as a static path. glibc `dlopen`s NSS/`getaddrinfo` at runtime, so "static glibc" segfaults on name resolution — a footgun. musl is the sole Linux static path. |
| Windows | `x86_64-pc-windows-msvc` + `-C target-feature=+crt-static` (primary on a Windows host); `x86_64-pc-windows-gnu` + `+crt-static` (cross-from-Linux) | static CRT (UCRT/vcruntime or MinGW runtime); **not** static OS | `kernel32.dll` / `ntdll.dll` are always dynamically imported — they are the OS ABI and ship with every Windows, so this is not a portability problem. "Static" = no redistributable/runtime DLLs required. MSVC preferred (standard, redistributable-free); gnu is the cross option. |
| macOS | none (fully static impossible) | — | Apple ships no static libSystem; the syscall ABI is deliberately unstable and only the dynamic `libSystem.dylib` is a supported boundary. `--static` targeting a macOS artifact is **refused** (see §3.6). A normal macOS build already links only libSystem/frameworks dynamically; `--macos-portable` names that build explicitly. Cross to `x86_64-unknown-linux-musl` from a Mac host to produce a real static Linux ELF. |

Portability claim: "portable single-binary artifact" means Linux musl fully-static
and Windows static-CRT. macOS is explicitly out of scope as a static *target* — a
stated hard product boundary, not a silent degrade.

---

## §3 — CLI, emitted-crate wiring, and `#[global_allocator]` (Q3)

### 3.1 CLI surface

```
ipe build --static [--target <triple>] [--allocator <auto|system|dlmalloc|talc|mimalloc>] \
          [--allow-slow-allocator] [--macos-portable] [--locked]
```

`--allocator` is a first-class typed clap enum (no pre-strip-into-env dance,
unlike `../sky`'s pre-optparse env preprocessing). Unknown allocator → parse-time
rejection.

### 3.2 Precedence and `sky.toml`

Precedence: **CLI flag > env (`IPE_STATIC` / `IPE_TARGET` / `IPE_ALLOC`) > `sky.toml` > AUTO.**

```toml
[rust]
static             = true
target             = "x86_64-unknown-linux-musl"  # optional; --static without target implies host musl
allocator          = "auto"                        # auto | system | dlmalloc | talc | mimalloc
allowSlowAllocator = false                          # sky.toml equivalent of --allow-slow-allocator
```

`[rust].allocator` is validated to the closed enum at toml-parse time (parse,
don't validate).

**The cliff acknowledgment has a `sky.toml` equivalent (`allowSlowAllocator`).**
Without it there is a dead config: because precedence is CLI > env > `sky.toml` >
AUTO, a project pinning `[rust] allocator = "system"` with a musl target resolves
to `StaticMusl { alloc = System }`, which the smart constructor refuses with
`Refusal::MuslMallocCliff` — yet `--allow-slow-allocator` was CLI-only, so that
`sky.toml` combination could *never* build (it would refuse on every invocation,
including plain `ipe build`). The acknowledgment therefore has a first-class
`sky.toml` key, `allowSlowAllocator = true`, that satisfies the same gate the CLI
flag does. The gate itself is unchanged — the 0.14× cliff is still constructible
only on purpose, via a deliberate second key (CLI flag *or* toml opt-in), never by
AUTO or a bare `allocator = "system"`. The key is likewise validated at toml-parse
time; the CLI flag still overrides the toml value.

### 3.3 Parse, don't validate — the typed `BuildPlan`

The flags do not feed cargo directly. A smart constructor resolves
`(host, target, static, allocator, app-shape)` into `Result<BuildPlan, Refusal>`
*before* any cargo invocation:

```
enum BuildPlan {
    DynamicGlibc   { target, alloc = System },
    StaticMusl     { triple, alloc },          // alloc ∈ { Dlmalloc, Talc, Mimalloc, System(+ack) }
    StaticWindows  { triple, alloc },          // triple ∈ { msvc, gnu }; the variant IS the +crt-static case
    MacPortable    { target, alloc },          // libSystem dynamic; notice emitted
    Wasm           { target, alloc = Dlmalloc },
}

enum Refusal {
    MacStaticUnsupported { cross_hint },
    WebviewStatic,
    MuslMallocCliff,                 // system + musl without --allow-slow-allocator
    TalcMultiThreadedApp,            // talc on a Sky.Live / Sky.Http.Server (tokio) shape
    TargetNotInstalled { rustup_cmd },
    UnknownAllocator,
}
```

Illegal combinations never construct a `BuildPlan`; they surface a loud `Refusal`.
Because `Allocator` is a closed enum and the emitted crate has at most one active
allocator feature, "two `#[global_allocator]`s" is unrepresentable at the plan
level (and would additionally be a compile error — a safe failure).

**No `crt_static: bool` on `StaticWindows` (make invalid states unrepresentable).**
An earlier sketch carried `StaticWindows { crt_static: bool }`, which can encode
`crt_static = false` — a non-static "static Windows" plan, a representable-but-
illegal state. The variant *is* the `+crt-static` case, so the boolean is dropped;
`+crt-static` is emitted unconditionally for this variant. A dynamic-CRT Windows
build is `DynamicGlibc`'s Windows analogue reached through a different plan, never
a `StaticWindows` with the flag flipped off.

**`Talc` on a multi-threaded app shape → `Refusal::TalcMultiThreadedApp`.** talc's
single spinlock collapses under the contention a tokio server generates (🔴 in the
§1 trade study), so `--allocator talc` is refused at plan construction for the
Sky.Live / Sky.Http.Server shapes and permitted only for single-threaded
CLI / Sky.Cli / TUI / batch artifacts. This is an *appropriateness / efficiency*
gate; it is layered on top of — not a substitute for — the unconditional
`spin::Mutex` soundness floor in §3.5. UB-freedom rests on the emitted lock, not
on this refusal firing; the refusal exists so the pathological-perf combination is
never produced by AUTO or a bare flag. (dlmalloc is the default for every server
shape anyway, so this refusal is rarely reached.)

### 3.4 Emitted `Cargo.toml` — mutually-exclusive allocator features

Replaces `../sky`'s single `static_alloc = ["mimalloc"]` (currently inherited in
`tests/golden/basics/Cargo.toml`) with a per-allocator feature family, so the choice
is a feature selection and "both allocators" is impossible to express:

```toml
[features]
default        = ["tokio", "crypto", "json"]   # no allocator feature ⇒ system
alloc_dlmalloc = ["dep:dlmalloc"]
alloc_talc     = ["dep:talc"]
alloc_mimalloc = ["dep:mimalloc"]

[dependencies]
dlmalloc = { version = "0.2", features = ["global"], optional = true }
talc     = { version = "4",   optional = true }
mimalloc = { version = "0.1", optional = true, default-features = false }
```

The builder passes at most one `--features alloc_*`. Only `alloc_mimalloc`
introduces a `build.rs` / C compile unit. The emitter mutates the manifest via the
same anchored-manifest approach the crate already uses for the `[profile.dev]`
anchor in `crates/sky_backend_rust/src/project.rs`. Migrating the golden fixtures
away from `static_alloc = ["mimalloc"]` to this feature family (and rebaselining
the oracle byte-diff) is part of landing this design — see Open Decision D1.

### 3.5 `#[global_allocator]` emission

All arms are emitted cfg-gated into the crate (deterministic source regardless of
choice; the *selection* is the cargo `--features` decision):

```rust
#[cfg(feature = "alloc_dlmalloc")]
#[global_allocator]
static GLOBAL: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

// talc MUST be backed by a real lock — NEVER AssumeUnlockable. A
// #[global_allocator] is reachable from every thread the process spawns;
// AssumeUnlockable asserts the allocator is never entered concurrently,
// which is a data race (= undefined behaviour) the moment any thread other
// than main allocates. tokio's worker pool alone breaks that invariant. The
// spin::Mutex backing is the soundness floor and is emitted unconditionally
// for the talc arm, so UB-freedom does NOT depend on the app-shape
// classifier being correct.
#[cfg(feature = "alloc_talc")]
#[global_allocator]
static GLOBAL: talc::Talck<spin::Mutex<()>, talc::ClaimOnOom> = /* … Talc::new(...).lock() */;

#[cfg(feature = "alloc_mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// no alloc_* feature ⇒ system allocator: emit nothing.
```

For `system`, the emitter omits the item entirely rather than emitting a no-op.

**Soundness floor — talc lock is not optional.** `talc::locking::AssumeUnlockable`
is *only* sound in a provably single-threaded program; installed as a
`#[global_allocator]` in any binary that can spawn a thread it is a data race in
the allocator, i.e. undefined behaviour. Since `--allocator talc` is selectable
and the *same emitter* serves multi-threaded Sky.Live / Sky.Http.Server (tokio)
shapes, the talc arm is emitted with a real `spin::Mutex` lock unconditionally —
the pure-Rust `Sync` lock adds no C dependency and keeps the security property
intact. This is defence-in-depth *below* the app-shape refusal (§3.7): even a
mis-classified shape cannot reach UB, because the emitted allocator is
thread-safe by construction. `GlobalDlmalloc` (the default) is already internally
locked and `Sync`; no change there.

### 3.6 Linker flags — committed `.cargo/config.toml`, not ambient RUSTFLAGS

`../sky` mutates the process `RUSTFLAGS` / `CARGO_TARGET_<triple>_LINKER` env
(ephemeral, invisible to a user running `cargo build` directly). ipê instead emits
a per-crate `.cargo/config.toml` (durable, reproducible, standalone-`cargo`-buildable):

```toml
# emitted crate's .cargo/config.toml

# Pure-Rust graph (dlmalloc/talc + rustls): use the toolchain-bundled lld +
# self-contained musl CRT. NOT a hardcoded `linker = "rust-lld"` — that is a
# bare binary name that is not reliably on PATH (rust-lld ships INSIDE the
# toolchain sysroot, not as a PATH executable), so pinning it by name breaks on
# a clean host. `link-self-contained=yes` tells rustc to link the bundled CRT
# objects and drive its bundled lld itself — no external linker binary assumed:
[target.x86_64-unknown-linux-musl]
rustflags = ["-C", "target-feature=+crt-static", "-C", "link-self-contained=yes"]
# When alloc_mimalloc (or the Compression feature's zstd) is present, a C
# compile unit exists, so instead pin an explicit C cross-linker that IS on
# PATH: rustflags += ["-C", "linker=x86_64-linux-musl-gcc"] (or route via
# cargo-zigbuild — see §4.1).

[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]

[target.x86_64-pc-windows-gnu]
rustflags = ["-C", "target-feature=+crt-static"]
```

The self-contained-vs-explicit-linker choice is plan-conditional: the `BuildPlan`
variant already encodes whether a C compile unit is present, so the emitted config
is correct by construction — the pure-Rust arms take `link-self-contained=yes` with
no external linker binary, and only the C-dep arms name a cross-linker (which is
then checked for presence by the §4.2 preflight). The §4.5 / CI gate (§4.3
`linux-static-x64`) MUST verify the emitted config links **on a clean host** — no
`rust-lld` on PATH, no `musl-tools` installed for the pure-Rust arm — so a
PATH-assumption regression is caught before release. Interaction with the repo-root target-dir pin (the only content of
the current `.cargo/config.toml`) is handled by writing the linker/rustflags into
the *emitted crate's* config, leaving the workspace pin untouched — see Open
Decision D2.

### 3.7 App-shape refusals

- **Sky.Webview** apps link the system WebKit / WebView2 and cannot be static →
  `Refusal::WebviewStatic` at plan construction (adopted from `../sky` verbatim).
- Any app shape that `dlopen`s at runtime, or the console reverse-proxy shape that
  spawns a child binary, is reviewed against the same rule; refuse at parse time
  rather than emit an artifact that fails at runtime.

---

## §4 — Cross-compilation and CI (Q4)

### 4.1 Cross mechanism — pure-Rust default is the dividend

With the pure-Rust default (dlmalloc + the runtime's existing rustls) the default
dependency graph has no C code, so cross-to-musl is:

```
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --features alloc_dlmalloc
```

`rust-lld` links it — **no `musl-gcc`, no `musl-tools`, no external C toolchain.**
This is the primary documented path.

Fallbacks, ranked, for the C-dependency case (`--allocator mimalloc`, or the
Compression feature's C `zstd`, or C-FFI deps added via `ipe add`):

1. **cargo-zigbuild** — `zig cc` as the cross C compiler/linker; one tool
   cross-builds musl and (separately) the macOS-cross path with near-zero host
   setup. Primary for the C-dep case.
2. **cross** (Docker) — most hermetic; heavier. Fallback for exotic triples.
3. **native `musl-gcc`** — `apt install musl-tools` / `brew … musl-cross`. When a
   C dep is present, ipê checks for the musl C cross-linker and errors with an
   actionable install hint rather than a cryptic link failure (adopted from
   `../sky`).

### 4.2 Toolchain preflight

Adopt `../sky`'s fail-fast preflight: verify the target is installed
(`rustup target list --installed`) and, when a C dep requires it, the cross-linker
is present, before cargo runs. An uninstalled target is refused with the exact
`rustup target add …` command (parse, don't validate) rather than handed to cargo
to fail opaquely. When `rustup` itself is absent, fail-soft (let cargo error) as
`../sky` does — refusing there would be hostile to non-rustup toolchains.

### 4.3 CI matrix

| Job | Target | Allocator | Verification |
|---|---|---|---|
| linux-static-x64 | `x86_64-unknown-linux-musl` | dlmalloc (default) | `file` = "statically linked"; `ldd` = "not a dynamic executable"; run on a `scratch` container |
| linux-static-arm64 | `aarch64-unknown-linux-musl` | dlmalloc | qemu-user run in distroless |
| linux-static-mimalloc | `x86_64-unknown-linux-musl` | mimalloc | keep the C path green |
| windows-static | `x86_64-pc-windows-msvc` +crt-static | dlmalloc | `dumpbin` imports = only kernel32/ntdll; run on clean Windows (no VC++ redist) |
| windows-cross | `x86_64-pc-windows-gnu` +crt-static (from Linux) | dlmalloc | runs |
| macos | `aarch64/x86_64-apple-darwin` | system | assert `--static` is **refused** (negative test); dynamic build runs |
| wasm | `wasm32-*` | dlmalloc | module instantiates; allocator parity with native-static default |
| perf-smoke | `x86_64-unknown-linux-musl` | dlmalloc vs mimalloc vs system | allocator-churn microbench; fail if any static artifact regresses toward 0.14× |
| supply-chain | all | — | `cargo audit` + `cargo deny` over the emitted `Cargo.lock`; SBOM emitted |

CI builds **and runs** (a `--build-only` sweep misses the "static binary segfaults
on startup" class). Static-ness is asserted, not assumed: a dynamically-linked
"static" build is a hard CI failure. The example sweep (task #35) gains a
`--static` variant; the perf sweep (`sky-rust-backend:examples-perf-sweep`) adds
the dlmalloc-vs-mimalloc-vs-system rows that replace the stale `../sky` 2×2 table
with ipê's own numbers.

### 4.4 Reproducible builds

Static + a committed `Cargo.lock` + a pinned toolchain *targets* bit-reproducible
artifacts (a supply-chain asset). The word is "targets", not "achieves":
lock + toolchain pin are necessary but not sufficient. Byte-identical rebuilds
across hosts additionally need the build path and timestamps normalised — at
minimum `-C link-arg=--remap-path-prefix` / `--remap-path-prefix` to strip the
absolute build directory out of embedded paths, and a pinned `SOURCE_DATE_EPOCH`
so any timestamp baked into the artifact is stable. These two knobs are noted here
as the remaining gap between "targets" and "achieves"; wiring and verifying them
(a diff-oscope / rebuild-and-compare CI job) is follow-up work, not claimed as done.
The pure-Rust default helps regardless: a C `build.rs` output is less reproducible
than pure-Rust codegen. `ipe build --static --locked` is the reproducible-rebuild
entry point.

### 4.5 Measure-before-finalize gate

No dlmalloc/talc numbers exist on the ipê runtime — every perf figure in §1 is a
prediction. Before hard-committing, benchmark `{musl-malloc, dlmalloc, mimalloc}`
(single- and multi-thread microbench + a Sky.Live/Http concurrent-request bench)
on the actual runtime.

Purpose of the gate — and its explicit limit: it (a) confirms dlmalloc clears the
cliff (expected ≥ ~0.5× glibc concurrent, well above 0.14×) and (b) *sizes* the
mimalloc opt-in recommendation ("if you run high-core concurrent churn, here is the
measured delta"). It does **not** flip the default: the principle order
(security > efficiency) decides that. The one contingency: if dlmalloc *fails* to
clear the cliff under concurrency (considered unlikely), the default flips to
mimalloc as a *measured* override, recorded as an `oracle_divergence` — a
data-driven exception, not an efficiency override of the security gate. Ownership
of running the gate is an open item — see Open Decision D3.

---

## §5 — Pros / cons and strategy comparison vs `../sky`

### Pros / cons of this design

| Aspect | Pros | Cons |
|---|---|---|
| dlmalloc default | zero C toolchain / build.rs / FFI on the common path; clears the cliff; unifies native-static + wasm into one audited allocator; C-free default graph (with rustls); trivial musl cross with no `musl-gcc`; small binary | ~1.5–3× behind mimalloc on concurrent server churn (recoverable via opt-in) |
| mimalloc opt-in | best perf when profiled and needed | drags a C toolchain + vendored C into that build; larger binary; frozen C blob in the CVE surface |
| musl-static Linux | genuinely portable single binary | freezes bundled deps → CVE-rebuild story (mitigated below) |
| typed `BuildPlan` + refusals | cliff and unbuildable combos unrepresentable; flags never lie | more up-front CLI/parse code than a string-match |
| committed `.cargo/config.toml` | reproducible, standalone-buildable, no ambient env leak | must not collide with the workspace target-dir pin (D2) |

### Strategy comparison vs `../sky`

**Adopted verbatim** (already principle-sound):

| Element | Principle served |
|---|---|
| musl as the sole Linux static target | correctness / soundness (glibc-static is a name-resolution footgun) |
| the `--static` / `--target <triple>` / `--allocator <choice>` CLI shape | — |
| parsed build-plan (sum type) before cargo | soundness — invalid combos unrepresentable |
| Sky.Webview-under-static refusal | correctness — links system WebKit/WebView2 |
| cross-linker presence check + actionable remediation | usability, no silent link failure |
| `+crt-static` for Windows / gnu | correctness |
| fail-soft when `rustup` is absent | pragmatism |

**Improved / diverged** (each gated on the principle it serves):

| Change | Principle | Rationale |
|---|---|---|
| **Default allocator dlmalloc (pure Rust), not mimalloc** | #1 security > #4 efficiency | removes build.rs + C toolchain + unsafe C-FFI + frozen-C blob from every static build; unifies wasm; clears the cliff. `../sky` chose mimalloc before pure-Rust was weighed. |
| Allocator as mutually-exclusive `alloc_*` features (not one `static_alloc`) | #3 soundness | "two allocators" becomes plan-level unrepresentable |
| Closed `--allocator` enum (reject unknowns at parse) | #2 correctness | `../sky` string-matches with silent fallthrough |
| `system` + musl → hard refuse unless `--allow-slow-allocator` | correctness / invalid-states | `../sky` warns-and-proceeds; a warning still produces the 0.14× artifact from one flag |
| macOS `--static` → refuse (+ cross-hint / `--macos-portable`) | correctness / parse-don't-validate | `../sky` warns-then-degrades to dynamic (the `RbDegradeMac` branch prints a warning, then produces a dynamic binary) — a binary the user asked to be static must not ship dynamic even with a warning; ipê refuses so the request is answered, not silently down-graded |
| Windows: prefer MSVC +crt-static as primary | #2 correctness | standard, redistributable-free artifact; gnu = cross fallback |
| Emit `.cargo/config.toml` vs mutating process RUSTFLAGS | #3 soundness / reproducibility | target-scoped, durable, no workspace leak |
| Pure-Rust allocator removes the musl C cross-linker | #1 security + DX | link with `rust-lld`; drop the last allocator-side C toolchain |
| `cargo audit`/`deny` + committed lockfile + per-artifact SBOM | #1 security | static freezes deps → rebuild-on-CVE story must be enforced, not implicit |
| `ruzstd` (pure-Rust decode) for the Compression feature | #1 security | otherwise Compression reintroduces C `zstd` on musl even with dlmalloc — see Open Decision D4 |

Net: the linking dimension is adopted from `../sky`; the sole principle-relevant
divergence is the allocator default (mimalloc → dlmalloc), plus tightening three
warn-paths into refusals/acknowledgment-gates.

### Default posture per app shape

Static remains **opt-in everywhere** until the CVE-rebuild automation (§6) is in
CI. Once automated, static is the *recommended* default for single-binary CLI /
Sky.Cli tools (a self-contained binary is the whole point) and for
Docker/Cloud-Run Sky.Live deployments (small scratch images). Long-running servers
carry a higher CVE-rebuild exposure, so the recommendation there is gated on the
audit/SBOM automation being green. Sky.Webview is never static (refused).

### Divergence recording

The allocator default change is logged as a sanctioned `oracle_divergence` per
`docs/architecture/divergence-policy.md`, with the reason-string contract:

> `allocator-default: pure-Rust dlmalloc over mimalloc — security principle #1
> (eliminate C-toolchain / unsafe-FFI / vendored-C supply-chain surface) over
> efficiency principle #4; concurrent-churn gap to mimalloc measured at <X> and
> deemed acceptable; mimalloc retained as an explicit opt-in.`

`<X>` is filled from the §4.5 measurement before the divergence is finalized.

---

## §6 — Security

### Allocator C-dependency supply-chain (only if a C allocator is used)

The default (dlmalloc/talc) adds **no** C toolchain, `build.rs`, or unsafe C-FFI —
the pure-Rust surface is auditable Rust (Miri / cargo-geiger) rather than an opaque
vendored C blob. The only path that reintroduces a C allocator is the explicit
`--allocator mimalloc` opt-in, which:

- compiles vendored C via a `build.rs` (a C toolchain requirement on every such
  build),
- crosses an `unsafe extern` FFI boundary,
- freezes an opaque-to-Rust C codebase into the artifact, carrying that codebase's
  own CVE history.

The opt-in emits an explicit notice so the cost is acknowledged, never silent. This
is the central security tension resolved in the principle order's favour: a C
allocator is accepted only when an operator has measured a concurrency bottleneck
and chosen to pay the cost.

### Static-link CVE-rebuild story

Static linking freezes every bundled crate (allocator, rustls, tokio, sqlx, redis,
lettre, reqwest, zstd, serde, and transitive deps) into the binary. There is no
dynamic-library hot-patch path: a dependency CVE requires **rebuild + redeploy**,
not an OS package bump. Mandated obligations:

1. **Commit the emitted crate's `Cargo.lock`** — exact version pins.
2. **`ipe build --static --locked`** — reproducible, byte-stable rebuilds.
3. **`cargo audit` / `cargo deny` CI gate** over that lockfile — fail the build on
   an unpatched RUSTSEC advisory in any bundled crate. **Coverage limit, stated
   explicitly:** RUSTSEC / `cargo audit` / `cargo deny` see only the *Rust
   dependency graph* (the `Cargo.lock`). They are **blind to CVEs in vendored C
   sources** compiled through a `build.rs` — the `mimalloc` opt-in's vendored C
   allocator (and the Compression feature's C `libzstd`, until D4's `ruzstd` swap)
   are invisible to the automated gate. This is a direct reinforcement of the
   pure-Rust default: the default graph has no such blind spot, and a build that
   opts into a C dependency also opts out of automated advisory coverage for that
   dependency.
4. **Per-artifact SBOM** — machine-readable manifest (cargo metadata + allocator
   choice + exact versions + build commit) emitted alongside each static binary, so
   "which advisory hits which shipped artifact" is tractable. **The SBOM MUST
   record the exact vendored C source versions** — the pinned `mimalloc-C` upstream
   version compiled by the `mimalloc` crate's `build.rs`, and the C `libzstd`
   version for the Compression feature — not merely the Rust wrapper-crate version.
   This is what makes the C blind spot in (3) *manually* auditable: a maintainer can
   run the recorded C version against OSV / the mimalloc CVE feed even though the
   automated Rust-graph gate cannot. Without the vendored-C version in the SBOM, a
   mimalloc-opt-in artifact ships an unauditable C blob.
5. **Pinned ipê toolchain version** so a CVE rebuild is one reproducible command.
6. **Documented rebuild lifecycle** surfaced at `--static` time.

The pure-Rust default *shrinks* this frozen surface: one fewer vendored C codebase
baked in, and the historical mimalloc/jemalloc C-allocator CVE class is simply
absent — a second-order security win reinforcing the default.

---

## Open decisions (for the user)

- **D1 — Golden rebaseline authority.** `tests/golden/basics/Cargo.toml` already ships
  `static_alloc = ["mimalloc"]` and `main.rs` emits the mimalloc
  `#[global_allocator]`. This design rips out that inherited mimalloc default in
  favour of the `alloc_*` feature family with a dlmalloc default. Who signs off on
  regenerating the goldens and rebaselining the oracle byte-diff, given mimalloc
  was landed without the pure-Rust weighing this study performed?
- **D2 — `.cargo/config.toml` collision.** The emitted per-crate config carries
  linker/rustflags; the repo-root `.cargo/config.toml` currently pins only the
  shared target-dir. Confirm the emitted-crate config is written into the crate
  dir (not the workspace root) and that the target-dir pin is preserved.
- **D3 — Measure-before-finalize ownership.** Who runs the §4.5 benchmark, on which
  fixture, and what exact throughput bar defines "clears the cliff" (≥1.0× A?
  ≥0.7× of mimalloc's 1.48×? ≥0.5× A)? The divergence reason-string `<X>` cannot be
  filled until this is fixed.
- **D4 — `zstd` → `ruzstd`.** The default graph is C-free (dlmalloc + rustls) only
  while the Compression feature is off; enabling it pulls C `libzstd` back onto the
  musl path. Swapping to pure-Rust `ruzstd` (decode) is the follow-up that makes
  "C-free static default" unconditional. In-scope for the first static milestone or
  a follow-up?
- **D5 — Missing wasm-target doc.** The referenced
  `docs/architecture/wasm-target.md` does not exist in this repo (the live wasm
  spec lives in `../sky` and is blocked on `SkyTask`'s `Send` bound). dlmalloc is
  the wasm default, so this allocator decision should be cross-referenced from both
  epics. Authoring/porting that doc is a prerequisite for the wasm cross-reference
  to resolve.
- **D6 — `aarch64-unknown-linux-musl` timing.** Commit ARM musl in the first static
  milestone alongside x86_64, or defer to a follow-up?
- **D7 — Static as a recommended default.** Once the CVE-rebuild automation (§6) is
  green in CI, which app shapes flip from opt-in to recommended-default (CLI tools
  and Docker/Cloud-Run Sky.Live are the candidates; long-running servers carry
  higher CVE exposure)?
