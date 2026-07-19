# co-ffi findings

4 findings: 1 critical, 1 high, 2 medium.

Audited: `src/compiler/sandbox/src/lib.rs`, `src/compiler/sandbox/Cargo.toml`,
`src/compiler/ffi/src/{lib,driver,pkginfo,typeref,naming,call,num_coerce,bindings,emit,instance,interface,diag}.rs`
(the last three skimmed for the emit sinks), plus the one external reachability
anchor `src/ipe-cli/src/ffi.rs` (the sole sandbox/inspector consumer).

Positives confirmed (NOT findings): crate names, git URLs, git pins, fn/method
names, enum variant names, and `IdentPath` submodule call-paths ARE gated
(`CrateName`/`GitSource`/`RustIdent`/`IdentPath`); trait bounds are closed to
the `MODELLABLE_5` allowlist via `trait_to_rust_path` (unknown bounds dropped,
not emitted); `bwrap_argv` is a pure shell-free direct-argv builder with
`--clearenv` + fresh namespaces + `--ro-bind / /`; `num_coerce` casts are total
and saturating; `Call::decode` structurally validates arg indices/arity/closure
placement. The build-script capability is correctly default-DENY
(`allow_build_scripts = false` unless `--allow-build-scripts`).

---

## co-ffi-001 · Foreign-call AST + wrapper type strings bypass the identifier gate and reach unsandboxed-compiled `_bindings.rs`
- severity: critical
- axis: security
- principle: "Rule: parse, don't validate / make invalid states unrepresentable" + P1 (no injection) + THE SEAL
- location: `src/compiler/ffi/src/call.rs:105` (`Call.path`/`method`/`trait_qualifier` stored raw), `call.rs:405` (`render_body` emits `self.path.join("::")`, `::{m}`, `<{self_path} as {trait_path}>` verbatim); `src/compiler/ffi/src/typeref.rs:68` (`WireTypeRef::Prim`/`Ctor` name `String`s), `typeref.rs:277` (`InnerTypeRef::render` emits `p.clone()` / `format!("{nm}<...>")` verbatim); `src/compiler/ffi/src/bindings.rs:220` (`resolve_rust_type`→`absolutize_crate` returns `rust_type`/`recv_rust_type` near-verbatim), `bindings.rs:431` (`translate_rust_ret` fallback returns raw type), `bindings.rs:1203`/`1284` (enum `struct_fields`/`selector` through `rust_safe_ident`, keyword-escape only — no validation)
- reachability: `PkgInfo::decode_json` (inspector stdout) and `load_catalog`/warm-build re-decode of the project-tree `kernel.json` both build `Call`/`InnerTypeRef`/`Param` from these strings; `emit_bindings` + the generic-instance emitter (`instance.rs:593` `call.render_body`) splice them verbatim into `<crate>_bindings.rs`, which is compiled UNSANDBOXED as part of the user crate at `ipe build`. The `kernel.json` re-decode is documented (`call.rs:262`, `typeref.rs:329`) as re-rejecting "a hand-corrupted cache", but decode gates only STRUCTURE, never the identifier CONTENT of `path`/`method`/`traitQualifier`/`prim`/`ctor`/`rustType` — so a hand-edited or inspector-emitted path segment carrying `){…arbitrary Rust…}//` injects tokens that pass every gate.
- problem: `naming.rs:11` claims "a crate that names a symbol `; std::process::Command::new(...)` can never reach generated source — the injection class dies at the trusted decode surface." That invariant holds ONLY for the `RustIdent`-gated names; the foreign-call AST paths/methods/trait-qualifiers and every `Prim`/`Ctor`/`rustType`/`struct_field`/`selector` string are trusted verbatim. The value type is `String` where it must be a validated `IdentPath`/`RustIdent` newtype, so an injection-bearing state is representable past decode and reaches an unsandboxed compile. Even absent full RCE, this also breaks THE SEAL: a `rustType` with unbalanced `<`/`>` or stray tokens yields `ipe` exit-0 then `cargo build` failure (`load_catalog` only checks each forwarded wrapper exists as `pub fn`, never that the region body is valid Rust).
- fix direction: parse every rendered path/method/type-ref segment through `RustIdent`/`IdentPath` at `Call::decode` and `WireTypeRef→InnerTypeRef` conversion (and `Param.rust_type`/`recv_rust_type` at `PkgInfo` conversion), storing validated newtypes so an unvalidated identifier is unrepresentable in the emit path.
- prior: new (runtime-audit-verdict `ffi` group covered only `ffi_polyfills.rs`, not the compiler-side FFI emit).

## co-ffi-002 · Best-effort resource caps + sequential pipe drain let an untrusted payload run with no wall-clock/rlimit and hang the host
- severity: medium
- axis: security
- principle: P1 "no unbounded resource a remote party can exhaust" / P3 (no unbounded blocking)
- location: `src/compiler/sandbox/src/lib.rs:258` (`bwrap_argv`: `if let Some(t) = timeout` / `if let Some(p) = prlimit` — both optional), `lib.rs:487` (`run_in_bwrap_jail` passes `caps.timeout`/`caps.prlimit` as `Option`, never requires them), `lib.rs:516` (stdout read to completion, THEN stderr, THEN `child.wait()`); consumer `src/ipe-cli/src/ffi.rs:179` gates only on `bwrap` presence
- reachability: `ipe add <malicious-crate>`. The jail runs the untrusted crate's proc-macros (and, with consent, build scripts) inside rustdoc. On a host where `bwrap` exists but `timeout`/`prlimit` are not on `PATH`, `probe()` returns `None` for them, `bwrap_argv` silently omits the wall-clock and every rlimit, and the run proceeds with no CPU/RSS/fd/proc/wall cap. Separately, `read_bounded` drains stdout fully before touching stderr, so a payload that fills the ~64 KiB stderr pipe then spins deadlocks the reader; the only backstop is the `timeout` wrapper that may be absent.
- problem: the caps that make the RCE surface bounded are advisory, dropped without warning or refusal when a helper binary is missing — the safe outcome is not the only reachable outcome. The sequential drain adds a latent deadlock that, without `timeout`, is an unbounded host hang.
- fix direction: require `timeout` + `prlimit` (or an in-process `setrlimit` + wall-clock kill + concurrent stdout/stderr drain) and refuse (`IPE-F4410`) when caps cannot be applied, mirroring the no-isolation refusal.
- prior: new.

## co-ffi-003 · The proven-`unshare` fallback tier is dead; unshare-only hosts are steered to the fully-unsandboxed override
- severity: medium
- axis: completeness
- principle: P1 (secure default) / Completeness (advertised capability that never applies)
- location: `src/compiler/sandbox/src/lib.rs:154` (`Mechanism::UnshareCandidate`), `lib.rs:381`/`408`/`438` (`current_ns_ids`/`prove_isolation`/`assert_net_namespace_empty` — used only by unit tests); consumer branch `src/ipe-cli/src/ffi.rs:179` (`!matches!(mechanism, Bwrap(_)) && !unsandboxed_ok → refuse`)
- reachability: `ipe add` on a host without `bwrap`. `select_mechanism` returns `UnshareCandidate`, but the sole consumer never spawns via `unshare`+`prove_isolation`; it either refuses or, when `IPE_FFI_ALLOW_UNSANDBOXED=1`, runs the inspector with NO sandbox at all (`ffi.rs:244`). `bwrap` needs unprivileged user namespaces, which hardened/CI kernels frequently disable — so this is a common path, not exotic.
- problem: the module doc (`lib.rs:11`) presents `unshare` as a real second tier "sound ONLY with a post-spawn proof," but that tier is never wired. Users on unshare-only hosts are pushed to the strictly-less-safe fully-unsandboxed override instead of the proven-namespace path the crate ships and tests. Untrusted proc-macros/build-scripts then execute with zero isolation.
- fix direction: wire the `UnshareCandidate` path (spawn under `unshare`, run `prove_isolation` as the child's first act before any untrusted code, refuse on failure) as the real bwrap→unshare→refuse ladder, or delete the dead mechanism + its doc so the honest posture is bwrap-or-refuse.
- prior: new.

## co-ffi-004 · Real introspection executes untrusted proc-macros/build-scripts with network egress ON; the crate's no-egress compile posture is never used
- severity: medium
- axis: security
- principle: P1 (no untrusted-code egress / SSRF/exfil surface)
- location: `src/compiler/sandbox/src/lib.rs:187` (`NetworkPolicy::Denied` + `lib.rs:235` `registry_cache` — the no-egress-while-compiling posture) vs consumer `src/ipe-cli/src/ffi.rs:220` (`network: NetworkPolicy::FetchOnly`, `registry_cache: None`) — the ONLY integrated invocation
- reachability: `ipe add <malicious-crate>` runs a single `FetchOnly` (egress-enabled) phase in which rustdoc expands the crate's proc-macros (always) and its build scripts (with `--allow-build-scripts`). Untrusted code thus executes with full outbound network access.
- problem: the sandbox crate provides `NetworkPolicy::Denied` + a `registry_cache` field precisely to split an egress-enabled fetch from an egress-denied compile, so foreign code never runs while the network is reachable. The consumer collapses both into one egress-on phase and never sets `Denied`/`registry_cache`, so the "no egress while executing" guarantee the two-posture design implies does not hold — a malicious proc-macro/build-script can exfiltrate or attack internal services from inside the jail. (Root wiring is in `ffi.rs`, out of partition, but the sandbox crate ships the unused stronger posture.)
- fix direction: run introspection as a fetch phase (download to a scoped `registry_cache`) followed by a `NetworkPolicy::Denied` rustdoc/compile phase over that cache, so proc-macro/build-script execution never coincides with egress.
- prior: new.
