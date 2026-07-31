# Static compilation: aarch64 target + fully C-free build

Design for two extensions to `ipe build --static` (issue #270): an
`aarch64-unknown-linux-musl` static target, and an optional build path whose
emitted crate links **no C** — no `cc`/`cmake` invocation, no vendored C, no
system library. Companion to ADR 0031 (static compilation + allocator gate);
this doc plans the two extensions, not a change to that ADR's decisions.

## Baseline: what `--static` produces today

`ipe build --static [--target <triple>] [--allocator <a>]` resolves a typed
`StaticPlan { triple, allocator }` in the CLI (`build_plan.rs::resolve`), passes
a toolchain `preflight`, then makes the emitted crate static-correct by
construction (`static_build.rs`):

- `StaticTriple` is a **closed enum** — `x86_64-unknown-linux-musl` (default)
  and `aarch64-unknown-linux-musl`. Anything else is a typed refusal. Parse,
  don't validate: an unverifiable triple never reaches cargo.
- `StaticAllocator` is `Dlmalloc` (pure-Rust default), `Mimalloc` (C opt-in),
  or `System` (reachable only through the `--allow-slow-allocator` cliff
  acknowledgment).
- `cargo_config` writes `.cargo/config.toml` with
  `rustflags = ["-C", "target-feature=+crt-static"]`; for aarch64 it adds
  `linker=rust-lld` + `link-self-contained=yes`.
- `staticize_manifest` splices exactly one `alloc_*` feature into the emitted
  `default = [...]` list.
- `preflight` requires the rustup target to be installed **and** a
  musl-capable C compiler on `PATH` — because the default dependency graph
  carries two C compile units (`zstd`, `ring`).

CI (`static.yml`) proves x86_64-musl end to end per allocator: emit →
`cargo build --target …-musl` → `file`+`ldd` static-ness assertions → run in a
`scratch` container (nothing but the binary) → `cargo audit` over the frozen
emitted lockfile. Jobs for aarch64, Windows (`+crt-static`), and FreeBSD-cross
already exist but are `continue-on-error` (non-blocking) pending toolchain
confirmation.

**Where #270 stands relative to this baseline.** The aarch64 half is largely
*wired* already (enum variant, cross-linker config, a CI job). The remaining
aarch64 work is hardening and promoting the job to blocking. The C-free half is
the genuinely new design: the current graph cannot build without a C compiler,
and the preflight enforces that.

## Extension 1 — aarch64 static builds

### Target and linker

`aarch64-unknown-linux-musl` is already a `StaticTriple` variant. The chosen
cross-link strategy avoids the scarce `aarch64-linux-musl-gcc`: `cargo_config`
pins Rust's bundled `rust-lld` with `link-self-contained=yes`, so Rust supplies
the musl startup objects and the linker itself. A C **cross-compiler**
(`aarch64-linux-gnu-gcc`, widely packaged) is still needed for the C units
(`zstd`, `ring`) via `CC_aarch64_unknown_linux_musl`. Once Extension 2's C-free
path is selected, **no C cross-compiler is needed at all** — the two extensions
compose: a C-free aarch64 static build needs only the rustup target plus
`rust-lld`.

### Toolchain matrix

| Build host | rustup target | C cross-compiler | Linker | Run verification |
|---|---|---|---|---|
| x86_64 (cross) | `aarch64-unknown-linux-musl` | `aarch64-linux-gnu-gcc` (only if C deps present) | `rust-lld` self-contained | `qemu-aarch64-static` |
| native ARM64 runner | same | `musl-tools` (only if C deps present) | `rust-lld` self-contained | native exec |
| C-free path (either host) | same | **none** | `rust-lld` self-contained | qemu / native |

### Preflight gap (must fix)

`preflight` today probes only `x86_64-linux-musl-gcc` / `musl-gcc` for the
C-compiler check — it does not honour an aarch64 cross-compiler name, so an
aarch64 static build with C deps can wrongly refuse (or wrongly pass) depending
on host tooling. The fix: derive the probed compiler names from the plan's
triple (`aarch64-linux-musl-gcc`, `aarch64-linux-gnu-gcc`, plus the existing
`CC_<triple>` / `TARGET_CC` overrides), and — when the C-free path is selected —
**skip the C-compiler check entirely** (no C unit to compile). This is the one
required compiler-side change for aarch64 correctness.

### CI

Promote `linux-static-arm64` from `continue-on-error` to a required post-merge
gate once green. Prefer a native `ubuntu-24.04-arm` runner (no QEMU, real
execution) if available; otherwise keep the x86_64-host + `qemu-aarch64-static`
cross path already in the workflow. The scratch-container proof is x86_64-only
on a standard runner; the aarch64 equivalent is the QEMU run (or native run on
an ARM runner).

## Extension 2 — the fully C-free build path

### Goal and definition

A build is **C-free** when the emitted crate's `cargo build` invokes no C/C++
compiler and links no system library: every `build.rs` is pure Rust, no `cc`,
no `cmake`, no `bindgen`, no `*-sys` shim. The proof is mechanical: the build
succeeds with **no C toolchain installed at all** (not merely a musl one), and
the `scratch`-container run still passes. C-free is a strictly stronger
property than static: today's static binary is self-contained but its *build*
still needs a C compiler.

### Audit of the default dependency graph

Two C compile units exist in the default emitted graph (the CLI refusal names
both):

| Dep | C content | Why present | Pure-Rust path | Decision |
|---|---|---|---|---|
| `ring` (via `reqwest` `rustls-tls`) | C + asm crypto primitives | rustls default `CryptoProvider` | rustls with a pure-Rust provider (`rustls-rustcrypto`, backed by the RustCrypto crates already in the graph), **or** `aws-lc-rs` — but that is *also* C, so it is not the C-free answer | **Swap** provider to `rustls-rustcrypto` under the C-free feature; keep `ring` as the default (faster, audited) when C-free is off |
| `zstd` (`zstd`/`zstd-sys` → C libzstd) | vendored C libzstd | zstd (de)compression kernels | no pure-Rust drop-in with equal compression; `ruzstd` decompresses only | **Gate/degrade**: under C-free, route zstd through `ruzstd` for decompression and refuse/degrade compression, *or* drop the zstd kernels from the C-free feature set with a typed "unavailable under --cfree" diagnostic |
| `mimalloc` (opt-in) | vendored C | throughput allocator | `dlmalloc` (already the pure-Rust default) | **Already gated**: C-free simply forbids `--allocator mimalloc` (typed refusal), dlmalloc stays |

Deps that are **already C-free** and need no change (verified against the
emitted template + runtime manifests): `flate2` (default `miniz_oxide` backend
is pure Rust — *not* the `zlib`/`zlib-ng` C backends), the entire RustCrypto set
(`sha2`, `sha1`, `md-5`, `hmac`, `aes-gcm`, `chacha20poly1305`, `rsa`,
`pbkdf2`, `subtle`, `zeroize`), `bcrypt`, `jsonwebtoken`, `serde_json`, `serde`,
`regex`, `chrono`/`chrono-tz`, `rust_decimal`, `uuid`, `base64`, `hex`,
`percent-encoding`, `url`, `csv`, `toml`, `serde_yaml`, `dlmalloc`. Notably the
graph has **no** `openssl-sys`, `libsqlite3-sys`, `libz-sys`, or `native-tls`:
TLS is rustls (not OpenSSL), and `sqlx` is configured `runtime-tokio-rustls` +
the pure-Rust SQLite/Postgres drivers, not the C `libsqlite3`.

### Feature-gated (non-default) deps

The optional app-shape deps spliced by `project.rs` must each get a C-free
verdict, since a C-free promise is only honest per the features a given app
actually pulls:

| Feature dep | C status | C-free verdict |
|---|---|---|
| `sqlx` (`db`) | pure Rust when `runtime-tokio-rustls` + native drivers; **no** `libsqlite3-sys` | C-free as configured; keep |
| `axum` / `tower-http` (`web`/`live`) | pure Rust | C-free; keep |
| `crossterm` + `unicode-width` (`tui`) | pure Rust | C-free; keep |
| `tokio-tungstenite` (websocket) | pure Rust with rustls | C-free provided rustls provider is pure-Rust (see `ring` row) |
| `lettre` (`Ipe.Email`) | pure Rust with `tokio1-rustls-tls` (already the configured feature) | C-free; keep |
| `wry` + `tao` (`webview`) | **links system WebKit/WebView2** | already refused under `--static`; equally cannot be C-free. Reuse the existing `WebviewStatic` refusal — no new gate |

### Surfacing C-free: a new plan axis, not a new triple

C-free is orthogonal to the target triple and to `--static`, so it is a
**boolean on the plan**, not a `StaticTriple` variant:

- CLI: `--cfree` flag + `[rust] cFree = true` in `ipe.toml`, resolved through
  the same layered `StaticRequestLayer` merge as `--allocator`.
- `StaticPlan` gains `c_free: bool` (default `false`). Make-invalid-states-
  unrepresentable: when `c_free`, the resolver *rejects* `--allocator mimalloc`
  (typed refusal, names dlmalloc) rather than emitting a manifest that would
  pull C.
- `staticize_manifest` under `c_free` activates a `cfree` feature that (a)
  selects `reqwest`'s pure-Rust rustls provider, (b) selects the flate2
  pure-Rust backend explicitly (belt-and-suspenders, it is already default),
  and (c) removes/aliases the `zstd` kernels per the gate decision above.
- `preflight` under `c_free` **skips the C-compiler check** — its whole reason
  (zstd/ring) is gone.

The emitted `Cargo.toml` template grows a `cfree` feature and a
`reqwest`/`rustls` provider split behind it; the SSOT crate-version table
(`crate_specs.rs`) gains any new crate (`rustls-rustcrypto`, `ruzstd`) so the
drift tripwire keeps the runtime and golden manifests in lockstep.

## Phased implementation plan (dependency-ordered)

Startable now (no dep swap; independent):

1. **Preflight triple-aware C-compiler probe.** Derive probed compiler names
   from `plan.triple`; add aarch64 names + `CC_<triple>` handling. Unit-tested
   via `preflight_with`'s injected observations. *Unblocks correct aarch64
   refusals.*
2. **aarch64 CI hardening.** Confirm `linux-static-arm64` green (native ARM
   runner preferred), then drop `continue-on-error`. Add the `file`/run
   assertions symmetrically with the x64 job.

Blocked on a dep swap / feature wiring (do after 1–2):

3. **Introduce the `cfree` plan axis.** Add `c_free` to `StaticRequestLayer`,
   `resolve` (with the mimalloc-under-cfree refusal), and `StaticPlan`; thread
   `--cfree` / `cFree` through the CLI. Pure resolver change, fully unit-tested;
   no manifest change yet.
4. **rustls pure-Rust provider swap.** Add `rustls-rustcrypto` to the SSOT
   table; wire the `cfree` feature to select it for `reqwest`, `sqlx`,
   `tokio-tungstenite`, `lettre`. This removes the `ring` C unit.
5. **zstd gate/degrade.** Decide and implement the zstd C-free policy
   (decompress-via-`ruzstd` + typed "compression unavailable under --cfree", or
   full kernel-gate). This removes the `zstd` C unit — the last one.
6. **preflight cfree skip + emit wiring.** Under `c_free`, skip the C-compiler
   check; make `staticize_manifest`/`cargo_config` emit the `cfree` feature set.
   After 4+5 the graph is C-free; this makes the toolchain gate agree.

Only after 3–6 land does a `--cfree` build actually compile with no C
toolchain. Steps 4 and 5 are independent of each other and can run in parallel
lanes (own `CARGO_TARGET_DIR` each); both gate step 6.

## How to prove it

- **Static-ness** (unchanged): `file` + `ldd` assert `statically linked` /
  `not a dynamic executable`; run in a `FROM scratch` container so any hidden
  dynamic dep (loader, libc, certs, `/etc`) fails loudly. aarch64: `qemu-
  aarch64-static` (cross) or native-runner exec.
- **C-free (the distinguishing proof):** build the emitted crate on a host with
  **no C compiler installed at all** (uninstall `gcc`/`clang`/`cc`, unset
  `CC_*`/`TARGET_CC`) and assert the build succeeds. A stray `*-sys`/`cc`
  invocation fails immediately with "no C compiler". Belt-and-suspenders: parse
  `cargo build --build-plan` (or `-v` output) and assert no `build-script-build`
  from `zstd-sys`/`ring`/`mimalloc` runs. Add a `cfree` CI matrix arm to
  `static.yml` that runs in a container image with the C toolchain absent.
- **Supply chain:** the existing `cargo audit` over the frozen emitted lockfile
  already covers whatever the C-free set freezes; the new provider/zstd crates
  enter that audit automatically.

## Open questions and risks

- **zstd compression under C-free.** No pure-Rust encoder matches libzstd. The
  honest options are decompress-only (`ruzstd`) with a typed refusal on
  compression, or gating the zstd kernels out of the C-free feature entirely.
  Either way the limitation is documented, never a silent behaviour change —
  this is the one place C-free is genuinely *lossy* for an optional feature.
- **rustls provider parity.** `rustls-rustcrypto` is less battle-tested than
  `ring`/`aws-lc-rs`; keep `ring` the default and make the pure-Rust provider
  reachable only under explicit `--cfree`, so the security posture of the common
  path is unchanged (Principle 1 — the default keeps the audited provider).
- **aarch64 execution fidelity.** QEMU-user is not a native run; a native
  `ubuntu-24.04-arm` runner is preferred for the blocking gate so allocator/TLS
  behaviour is proven on real hardware.
- **Feature-combination explosion.** `cfree` × allocator × triple × app-shape
  is a large matrix; CI proves the representative corners (cfree+dlmalloc on
  both triples, cfree+db, cfree+web), and the typed refusals make the illegal
  corners (cfree+mimalloc, cfree+webview) unrepresentable rather than tested.
- **`--build-plan` stability.** The "no C build-script ran" assertion depends on
  cargo's `--build-plan` / `-v` output shape; the primary proof (build with no C
  compiler present) does not, so treat the build-plan parse as a secondary
  belt-and-suspenders check, not the gate.
