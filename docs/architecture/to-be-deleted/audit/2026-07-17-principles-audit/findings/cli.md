# CLI findings

9 findings: 0 critical, 1 high, 2 medium, 6 low.

Audited: `src/ipe-cli/src/{main,lib,build_plan,project,cache,ffi,watch,stdlib,lsp}.rs`
(tests/ excluded per brief). Prior audit (`docs/architecture/runtime-audit-verdict.md`)
contains no CLI-partition items — all findings below are new.

## CLI-001 · `~/.cargo` (registry credentials) ro-bound into the network-on FFI jail
- severity: high
- axis: security
- principle: P1 — no secret leakage; on untrusted input the safe outcome is the only reachable outcome
- location: `src/ipe-cli/src/ffi.rs:208-217` (bind decision), `src/compiler/sandbox/src/lib.rs:304-308` (`--ro-bind` re-exposure through the tmpfs mask)
- reachability: `ipe add <crate>` / `ipe install` on any machine where the user has run `cargo login` (`~/.cargo/credentials.toml` exists). The jail runs the inspection of an arbitrary, attacker-published crates.io crate with `NetworkPolicy::FetchOnly` (network ON); untrusted code executes inside it (build scripts under `--allow-build-scripts`; proc-macro/rustdoc execution during inspection otherwise).
- problem: `run_inspector` pushes the WHOLE `home.join(".cargo")` into `toolchain_ro_binds`, and the sandbox re-binds it read-only at its absolute path through the `/home` tmpfs mask. Nothing masks `credentials.toml`. Untrusted crate code inside the jail can read the crates.io API token at its well-known path and exfiltrate it over the deliberately-open fetch network — defeating the confidentiality half of the exact boundary the jail exists to provide ("fetch posture: network on, everything else confined"). `CARGO_HOME` being redirected to the scoped tmp protects cargo's own flows, not the file's readability.
- fix direction: bind only the needed subtrees (`~/.cargo/bin`, registry cache) or tmpfs/empty-file-mask `credentials.toml`/`credentials` before the re-bind.
- prior: new

## CLI-002 · FFI cache root found by unbounded upward walk — planted ancestor cache injects arbitrary Rust
- severity: medium
- axis: security
- principle: P1 — no foothold from untrusted input; parse-don't-validate at the trust boundary
- location: `src/ipe-cli/src/ffi.rs:26-40` (`find_cache_root` walks to filesystem root), `src/ipe-cli/src/ffi.rs:126-131` (`bindings_source` spliced verbatim into the emitted `src/ffi.rs`), consumed at `src/ipe-cli/src/lib.rs:429-441`
- reachability: any `ipe build`/`ipe run` of a project whose directory has an attacker-writable ancestor — e.g. a project checked out under `/tmp/<dir>` on a multi-user host, where another user pre-plants `/tmp/.ipe/cache/ffi/rust`. The walk stops at the FIRST hit, which may be above the project root and above any directory the user considers theirs.
- problem: the installed-crate catalog is discovered by walking UP from the entry with no stop at the project root (`ipe.toml`) and no ownership check. Catalog artifacts are attacker-self-consistent files whose `bindings_source` is concatenated verbatim into the emitted project's Rust source; `ipe run` then compiles and executes it. A directory position alone (not any action by the user on the planted files) silently expands the trusted-code set.
- fix direction: stop the walk at the project root (the `ipe.toml` directory / entry's ancestor chain up to it only), or require the cache dir to be under the manifest root.
- prior: new

## CLI-003 · wasm × static composition gate covers only the CLI flag layer
- severity: medium
- axis: correctness
- principle: P2 + make-invalid-states-unrepresentable — a gate enforced at one of three precedence layers is a bypassable gate
- location: `src/ipe-cli/src/lib.rs:1464-1473` (refusal checks `static_flags` only), `src/ipe-cli/src/lib.rs:1506` (`resolve_static_plan` still merges env + `ipe.toml` layers), `src/ipe-cli/src/lib.rs:938-963` (`write_emitted_project` applies the plan to the wasm project)
- reachability: documented workflows compose it: a project with `[rust] static = true` in `ipe.toml` (or `IPE_STATIC=1` in the environment) built with `ipe build --target wasm`.
- problem: `--target wasm` + `--static`/`--allocator` is refused, but only when the static request arrives via CLI flags. When it arrives via env or manifest, `resolve_static_plan` resolves a full musl `StaticPlan` and the build proceeds with `target: WasmClient` + `static_plan: Some(..)`: the musl preflight runs (refusing a perfectly valid wasm build on machines without musl-gcc / the musl std), and on machines that pass it the wasm cdylib `Cargo.toml` gets `staticize_manifest` surgery plus a generated `.cargo/config.toml` — a nonsense artifact configuration (or a `CompilerBug` "manifest anchor drift" error) for a build the user asked nothing static of. `ipe run --target wasm` similarly falls into static-triple resolution with a misleading `TargetRequiresStatic`/`UnknownStaticTarget` message instead of a "run has no wasm denotation" refusal.
- fix direction: decide the wasm-vs-native axis BEFORE merging static layers and force the merged static request to `None` (or a loud refusal naming the layer) under `Target::WasmClient`.
- prior: new

## CLI-004 · watch conflates any signal-killed cargo with "superseded" — genuine failures silently dropped
- severity: low
- axis: correctness
- principle: P2 — wrong error semantics; a swallowed failure presented as a non-event
- location: `src/ipe-cli/src/watch.rs:1143-1153` (`is_killed_status`: any `status.signal().is_some()`), routed at `src/ipe-cli/src/watch.rs:1109-1115` and dropped at `src/ipe-cli/src/watch.rs:855-858`
- reachability: `ipe watch` sessions where `cargo build` dies to the kernel OOM killer (SIGKILL), a segfault, or an external kill — realistic on memory-constrained dev boxes.
- problem: the waiter classifies EVERY signal termination as `CargoOutcome::Killed` ("we superseded it"), which the orchestrator drops without printing anything. An OOM-killed build for the CURRENT (non-superseded) generation produces zero user-visible output: the watch appears healthy while serving the stale last-good binary, and nothing rebuilds until the next save. The orchestrator knows whether it actually called `.kill()` for that generation but that fact is not consulted.
- fix direction: record "killed-by-us" per generation (a flag set where `child.kill()` is called) and treat any other signal exit as `CargoOutcome::Red`.
- prior: new

## CLI-005 · project-mode build silently ignores the explicitly named entry file
- severity: low
- axis: correctness
- principle: P2 — the CLI accepts an argument then silently does something else; §0 no silent divergence
- location: `src/ipe-cli/src/lib.rs:1217` (`entry_path = vec!["Main"]` hardcoded in `build_project_with_options`), routing at `src/ipe-cli/src/lib.rs:1504,1519-1522`; misleading fallout at `src/ipe-cli/src/lib.rs:633-635`
- reachability: any project with a `ipe.toml`: `ipe build src/Other.ipe` (a second entry, a scratch module, or a typo'd path — the named file's existence is never even checked) builds `Main` instead, exit 0.
- problem: once the upward walk finds a manifest, the user-supplied `.ipe` path is used only to LOCATE the manifest; compilation always roots at `Main`. Building a non-Main entry silently produces the Main artifact; a project missing `src/Main.ipe` surfaces as the internal-flavoured `"internal: entry module not in source map"` usage error rather than a user-facing "project has no Main module" diagnostic.
- fix direction: derive the entry module from the named file when one was given (or refuse loudly when it is not the project's entry), and give the missing-Main case a real diagnostic.
- prior: new

## CLI-006 · watch and LSP resolution paths omit the FFI seam that `ipe build` has
- severity: low
- axis: completeness
- principle: P5 — a claimed capability (same module set as the batch build) that partially works
- location: `src/ipe-cli/src/watch.rs:718-765` (no `ffi::load_catalog_for`/`inject_interfaces`; `BuildConfig::new(.., None, ..)` hardcodes no FFI emit; `create_source_root` called with an empty `ffi_injected` at `watch.rs:740-745`), `src/ipe-cli/src/lsp.rs:18-53` (`DriverLoader` likewise)
- reachability: any project with installed FFI crates (`ipe add` used): `ipe watch` and `ipe lsp` on it.
- problem: `compile_modules_observed` injects `Rust.<Crate>` interface modules and threads `FfiEmit`; the watch orchestrator and the LSP loader — whose module docs claim they mirror `run_build`'s resolution so "the module set the editor analyzes can never diverge" — do neither. An FFI-using project red-loops in watch (module-not-found on every rebuild, `Rust.*` imports) and shows false diagnostics in the editor while `ipe build` succeeds. Loud, not silent, but a permanent capability gap contradicting the stated mirroring contract.
- fix direction: route the FFI catalog load + interface injection + `FfiEmit` through `resolve_project_sources` (shared with LSP) and the watch `BuildConfig`.
- prior: new

## CLI-007 · `ipe install` defaults to the `--yes` posture, skipping `ipe add`'s trust prompt
- severity: low
- axis: security
- principle: P1 — a consent gate that exists on one path is bypassed by default on a sibling path
- location: `src/ipe-cli/src/ffi.rs:362` (`assume_yes = .. || rest.is_empty()`), contrast `src/ipe-cli/src/ffi.rs:325-333` (`run_add`'s prompt)
- reachability: `ipe install` in any project with `[rust.dependencies]` — e.g. right after cloning an untrusted project, exactly the moment the trust summary matters most.
- problem: `ipe add <crate>` without `--yes` prints a trust summary and requires confirmation before inspecting a foreign crate; `ipe install` with no flags silently behaves as `--yes`, inspecting every manifest-listed crate with no prompt. The `[--yes]` in its usage string implies interactive-by-default, so the actual behaviour also contradicts the documented surface.
- fix direction: make bare `ipe install` prompt (per crate or once for the list); reserve the silent path for an explicit `--yes`.
- prior: new

## CLI-008 · predictable `/tmp` scratch dir for the FFI jail (write-boundary violation + symlink hazard)
- severity: low
- axis: security
- principle: P1 + Write-boundary ("scratch build state → under `~/.cache/ipe/` ONLY. Never `/tmp`")
- location: `src/ipe-cli/src/ffi.rs:192-197` (`std::env::temp_dir().join(format!("ipe-ffi-add-{crate}-{pid}"))` + `create_dir_all`), bound read-write into the jail at `src/ipe-cli/src/ffi.rs:219-229`
- reachability: every sandboxed `ipe add`/`ipe install` on a multi-user host.
- problem: the jail's writable scratch root is created at a predictable name in world-writable `/tmp` with `create_dir_all` (which happily accepts a pre-existing directory or a pre-planted symlink-to-directory at that path), then bind-mounted RW into the sandbox. A local attacker who wins the name race redirects everything the jailed inspection writes into a directory of their choosing under the victim's uid. Independent of the race, the location itself violates the repo's own write-boundary rule (scratch state never in `/tmp`).
- fix direction: create the scratch dir under `~/.cache/ipe/` with a randomized component (fail if it already exists).
- prior: new

## CLI-009 · hand-rolled `ipe.toml` parser silently mangles quoted values with trailing content
- severity: low
- axis: correctness
- principle: parse-don't-validate — the manifest boundary accepts malformed input and forwards a mangled value
- location: `src/ipe-cli/src/project.rs:167-181` (`val.trim().trim_matches('"')`, no inline-comment handling)
- reachability: any `ipe.toml` using an inline comment or unusual quoting on a recognised key, e.g. `name = "my-app"  # prod`.
- problem: values are extracted by `split_once('=')` + `trim_matches('"')` with no comment stripping: `name = "my-app" # prod` yields the silently-wrong name `my-app" # prod` (accepted, no error). Typed fields (`driver`, `[rust] static`, `allocator`) at least fail loudly — but with a confusing echoed value containing the comment text. Valid-TOML inputs are thus either silently mis-parsed or rejected with misleading messages.
- fix direction: strip inline comments before value extraction (or parse values with a small quoted-string-aware scanner) so a quoted value is taken exactly and trailing junk is a loud error.
- prior: new
