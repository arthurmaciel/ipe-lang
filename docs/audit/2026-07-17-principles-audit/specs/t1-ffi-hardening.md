# T1 — FFI trust-boundary hardening

Fixes the FFI-security theme: `co-ffi-001` (critical), `CLI-001` (high),
`co-ffi-002/003/004` (medium), `CLI-002/007/008` (low). Every fix below closes
the reachable path the judge confirmed in `verdicts/ffi-security.md`; each
carries an explicit residual-risk statement.

The theme has two independent root defects plus three narrower ones. They are
NOT one bug — a full fix requires both spines.

---

## Theme root causes

### RC-A — the type/path/field surface is untyped past the decode gate

The FFI decode boundary parses IDENTIFIER fields into validated newtypes
(`RustIdent`/`IdentPath`/`CrateName`/`PackageName`) but leaves every TYPE
string, CALL PATH segment, TRAIT-QUALIFIER, and ENUM pattern/tag/selector/field
as a raw `String`. Those raw strings are rendered VERBATIM into
`<slug>_bindings.rs`, which is compiled UNSANDBOXED as part of the user crate at
`ipe build` (`backend/rust/src/project.rs:1240` inserts `src/ffi.rs` + `mod
ffi;`). The `naming.rs:11` invariant ("a crate that names a symbol `;
std::process::Command::new(...)` can never reach generated source") is TRUE for
the `RustIdent`-gated names and FALSE for this surface.

Concretely untyped-and-emitted (`file:line`, current tree):
- `typeref.rs:73-74` `WireTypeRef::Prim(String)` / `Ctor(String, …)` →
  `InnerTypeRef::Prim/Ctor` (`typeref.rs:233,235`) with **no charset gate**;
  `InnerTypeRef::render` emits `p.clone()` and `format!("{nm}<…>")` verbatim
  (`typeref.rs:282,288`). A test even accepts `::std::string::String` as a ctor
  name (`typeref.rs:422`) — i.e. paths with `:`, `<`, `>`, spaces are in-domain.
- `call.rs:107-117` `Call.path: Vec<String>`, `method: Option<String>`,
  `trait_qualifier: Option<(String,String)>` — `Call::decode` (`call.rs:131`)
  validates only STRUCTURE (arg indices, arity, closure placement), never
  identifier CONTENT. `render_body` emits `self.path.join("::")` (`call.rs:406`)
  and `<{self_path} as {trait_path}>::{m}` (`call.rs:447`) verbatim.
- `pkginfo.rs:216-223` `Param.foreign_ty`/`ipe_type`/`rust_type` stored raw
  (doc comment: "verbatim from the inspector"); `recv_rust_type` raw
  (`pkginfo.rs:246`); `bindings.rs:220` `resolve_rust_type` →
  `absolutize_crate` (`bindings.rs:43`) returns the override near-verbatim —
  `absolutize_crate` only recognises a `<krate>::` prefix, passing every other
  byte (space, `{`, `}`, `;`, `(`, `)`) through untouched.
- `pkginfo.rs:159-208` `EnumArm.pattern`/`tag`, `EnumExtract.selector`,
  `EnumCtor.struct_fields` are raw `String`; routed only through
  `rust_safe_ident` (`naming.rs:233`) at the bindings emitter, which
  keyword-escapes but does NOT reject an injection charset. (`tag` is
  string-literal-escaped by `rust_str_lit` at `bindings.rs:69` — safe — but
  `pattern`/`selector`/`struct_fields` render as code, not literals.)

Two attacker doors reach the same sink:
- **(a) warm-cache** (lowest capability, no crate publish): `load_catalog`
  (`driver.rs:524`) reads `<slug>_bindings.rs` off disk (`driver.rs:553`) and
  only substring-checks `pub fn <ident>(` (`driver.rs:598-607`) — it NEVER
  re-parses the AST/type-refs. A hand-edited or CLI-002-planted `_bindings.rs`
  with injected Rust inside a REACHED wrapper region compiles unsandboxed.
- **(b) fresh install**: `emit_bindings(pkg)` (`bindings.rs:903`) renders the
  raw strings held on the un-validated `PkgInfo` into `_bindings.rs`, which
  `write_package` (`driver.rs:373`) persists — so even a first `ipe add` of a
  malicious inspector output injects.

**SEAL corollary**: a `rust_type` with an unbalanced `<`/`>` yields `ipe`
exit-0 (nothing checks Rust validity of the rendered region) then a `cargo
build` failure — a representable-but-illegal pipeline state, which THE SEAL
forbids.

The parse-don't-validate fix: these fields must become validated newtypes AT
DECODE, so an injection-bearing value is UNREPRESENTABLE in the emit path — the
same discipline `RustIdent`/`IdentPath` already apply to the name surface,
extended to cover the whole surface that reaches emission.

### RC-B — untrusted code executes with network egress on, and secrets are reachable during that phase

`ipe add` runs the inspector (which expands the target crate's proc-macros
always, and build scripts under `--allow-build-scripts` — arbitrary foreign
code) in a SINGLE `NetworkPolicy::FetchOnly` phase (`ffi.rs:220`,
`registry_cache: None` at `ffi.rs:224`). The sandbox crate SHIPS the
two-posture design to prevent exactly this — `NetworkPolicy::Denied` +
`JailSpec.registry_cache` (`lib.rs:187,235`) — but the sole consumer never uses
it. During that egress-on phase the WHOLE `~/.cargo` tree is re-bound read-only
at its absolute path past the `--tmpfs /home` mask (`ffi.rs:211` →
`lib.rs:304-308`), so `~/.cargo/credentials.toml` (the crates.io API token) is
readable by the jailed foreign code and POSTable out.

This is one root — "untrusted code runs with egress on" — with two symptoms:
co-ffi-004 (no confined-compile phase) and CLI-001 (a specific high-value
secret readable during it).

### RC-C — capability caps are advisory, dropped silently when a helper is absent

`bwrap_argv` wraps the wall clock and every rlimit in `if let Some(t) =
timeout` / `if let Some(p) = prlimit` (`lib.rs:266,346`); `probe()`
(`lib.rs:136`) returns `None` for an absent helper; the consumer gates only on
`Mechanism::Bwrap` presence (`ffi.rs:179`), never on cap availability. On a host
with `bwrap` but no `timeout`/`prlimit` on PATH, untrusted code runs with NO
wall-clock and NO rlimit — the safe outcome is not the only reachable outcome.
A latent deadlock compounds it: `run_in_bwrap_jail` drains stdout to completion,
THEN stderr, THEN waits (`lib.rs:517-519`) — a payload that fills the stderr
pipe then spins hangs the reader, backstopped only by the possibly-absent
`timeout`.

### RC-D — the `unshare` fallback tier is advertised but dead

`select_mechanism` returns `UnshareCandidate` (`lib.rs:171`) and the module doc
(`lib.rs:11`) presents it as a real second tier, but the only consumer
(`ffi.rs:179`) spawns only for `Bwrap`; `prove_isolation` (`lib.rs:408`) is
invoked solely by unit tests. On a common host class (hardened/CI kernels with
unprivileged userns disabled — bwrap unavailable, unshare present) the user is
steered to the strictly-less-safe `IPE_FFI_ALLOW_UNSANDBOXED=1` path instead of
the proven-namespace tier the crate ships.

### RC-E — unbounded upward cache discovery + predictable /tmp scratch + default-yes install

Three low-severity delivery/consent defects that widen the RC-A/RC-B blast
radius:
- `find_cache_root` (`ffi.rs:26-40`) walks UP to the filesystem root, returning
  the FIRST `.ipe/cache/ffi/rust` hit — no stop at `ipe.toml`, no ownership
  check. A planted ancestor cache is the concrete warm-cache delivery vector
  for RC-A(a). (CLI-002)
- `run_install` (`ffi.rs:362`) treats bare `ipe install` as `--yes`, skipping
  the per-crate trust prompt `ipe add` requires. (CLI-007)
- the jail scratch root is `temp_dir().join("ipe-ffi-add-{crate}-{pid}")`
  (`ffi.rs:192`) — a predictable name in world-writable `/tmp`, accepted
  pre-existing by `create_dir_all` (symlink-swap race), and a write-boundary
  violation (scratch belongs under `~/.cache/ipe/`). (CLI-008)

---

## Fixes

### F1 — validated type/path/selector newtypes at decode (closes co-ffi-001)

**Design.** Introduce three validated newtypes in `naming.rs`, alongside the
existing `RustIdent`/`IdentPath`, and thread them through the domain layer so
the raw `String` is gone from every value that reaches `render`.

New types (all with a private field + a single `parse` smart constructor +
`as_str`/`Display`, mirroring `RustIdent`):

```rust
// naming.rs
/// A validated Rust TYPE expression restricted to the closed grammar the FFI
/// emitter actually produces: `::`-paths of RustIdents, angle-bracket generic
/// application, `&`/`&mut ` borrow prefixes, tuples/unit `()`, and commas +
/// single spaces as separators. NO `;`, `{`, `}`, `(` (except `()`), string
/// bytes, or statement tokens — so a rendered type can never open a new item
/// or statement.
pub struct RustTypeExpr(String);
impl RustTypeExpr {
    pub fn parse(s: &str) -> Result<Self, WireDefect>; // WireDefect::InvalidType
}

/// A validated `<pattern>` fragment for an enum-accessor match arm: a
/// RustIdent variant head optionally followed by `(..)` or `{..}` (the only
/// two shapes `decode_arms`/`EnumExtract` emit). Rejects everything else.
pub struct RustPattern(String);       // WireDefect::InvalidPattern

/// A validated field SELECTOR: either a RustIdent (struct field) or a decimal
/// index (tuple position). Rejects any other byte.
pub struct FieldSelector(String);     // WireDefect::InvalidSelector
```

`RustTypeExpr::parse` is a small recursive-descent (or single linear scan with
a bracket-depth counter) accepting exactly: segments matching
`RustIdent`/`IdentPath`, `<`…`>` nesting, leading `&`/`&mut `, `()`, and `, `
separators. It is deliberately NARROWER than "valid Rust type" — it need only
admit what the inspector legitimately emits (primitives, `::crate::Path`,
`Vec<T>`, `Option<T>`, `Result<T, E>`, `serde_json::Value`, tuples). Anything
outside is refused. This also fixes the SEAL corollary: an unbalanced `<`/`>`
fails `parse`, so `ipe` rejects at decode instead of exit-0-then-cargo-fail.

Thread-through (make the raw `String` unrepresentable, not merely checked):
- `typeref.rs` `InnerTypeRef::Prim(RustTypeExpr)` and `Ctor(RustTypeExpr,
  Vec<Self>)` — the ctor NAME becomes a validated path. `TryFrom<WireTypeRef>`
  runs `RustTypeExpr::parse` on the `prim`/`ctor` strings (returns a new
  `CallDefect::TypeUnrenderable`/reuse `WireDefect::InvalidType`), so
  `InnerTypeRef::render` (`typeref.rs:277`) emits `.as_str()` — no verbatim
  interpolation of an unvalidated string survives.
- `call.rs` `Call.path: Vec<RustIdent>` (each segment parsed at decode),
  `method: Option<RustIdent>`, `trait_qualifier: Option<(RustTypeExpr,
  RustTypeExpr)>` (self-path + trait-path are type expressions). `render_body`
  then joins/interpolates validated values only. The wire→domain conversion in
  `Call::decode` (`call.rs:191`) parses each — a segment failing is
  `WireDefect::InvalidIdent`/`InvalidType`, dropping the binding (over-drop).
- `pkginfo.rs` `Param.rust_type: Option<RustTypeExpr>` and
  `FnInfo.recv_rust_type: Option<RustTypeExpr>` parsed in `param_from_wire`
  (`pkginfo.rs:583`) / `TryFrom<WireFunction>` (`pkginfo.rs:629`).
  `Param.foreign_ty`/`ipe_type` are the Ipê-facing surface (they drive
  `resolve_rust_type`'s Ipê branch and never render as Rust CODE) — keep as
  `String` but confirm every consumer path is data, not code; where
  `resolve_rust_type` (`bindings.rs:220`) falls back to the raw override, it now
  receives a `RustTypeExpr`.
- `pkginfo.rs` `EnumArm.pattern: RustPattern`, `EnumExtract.selector:
  FieldSelector`, `EnumCtor.struct_fields: Vec<RustIdent>` — parsed in
  `decode_shape`/`decode_arms` (`pkginfo.rs:507,526`). `absolutize_crate`
  callers and `rust_safe_ident` now operate on validated inputs.

**Close the warm-cache re-decode hole (door (a)) — the structural fix.** The
cache stores the ALREADY-RENDERED `_bindings.rs` and reloads it as opaque text;
the substring check is not a parse. Fix the STRUCTURE so a hand-edited
`_bindings.rs` is unrepresentable in the trusted set: stop trusting the stored
Rust text and re-derive it from the validated domain value on load.
- The `<slug>.kernel.json` artifact already carries the re-serialized VALIDATED
  domain (`to_wire_json` round-trips through the decode gate — proven by
  `call.rs:968`). Make `load_catalog` reconstruct `bindings_source` by
  re-decoding `kernel.json` through `PkgInfo`/`Call` and calling
  `emit_bindings` — NOT by reading `<slug>_bindings.rs` off disk. The
  `_bindings.rs` file then becomes a build-cache convenience whose CONTENT is
  never trusted: either drop it as an input entirely (regenerate every load) or
  keep it only as a checksum-verified mirror of the regenerated text (mismatch
  ⇒ refuse). Regenerating is simplest and removes the entire class.
- If regeneration is judged too costly per build, the fallback is a keyed
  integrity check: `emit_consumer_json` stores a MAC/hash of `_bindings.rs`
  under a per-project key that a planted file cannot forge — but this is
  strictly weaker than re-deriving from the validated `kernel.json`, so prefer
  regeneration. Record the choice in `docs/divergences-from-sky.md` only if it
  diverges from the Go reference's cache contract.

**Go/Elm parity.** The Go reference's FFI is a separate lineage; this is the
Rust-backend's own trust surface (per `ipe-lang-is-ipe-ancestor` memory), so
there is no upstream contract to match — the newtypes are a strict
security improvement, no divergence record needed unless the cache-artifact set
changes shape (then record it).

**Residual risk after F1.** The grammar `RustTypeExpr` accepts is a
hand-written allowlist; a legitimate inspector type outside it (e.g. an exotic
lifetime-parameterised path, `dyn Trait`, `impl Trait`, const-generic `[T; N]`)
is now REFUSED — that binding drops (over-drop, recorded in `pkginfo.dropped`),
never emitted. This trades completeness for security correctly (P1 > P5), but
the accepted grammar must be reviewed against the inspector's real output corpus
so common bindings don't silently drop; widen the grammar only with a
matching negative test proving injection shapes still fail. The
re-derive-from-kernel.json load path removes the verbatim-text trust entirely,
so the residual there is only a bug in `emit_bindings` itself (in-scope of the
existing SEAL gate).

### F2 — two-phase no-egress-while-executing sandbox + stop mounting ~/.cargo secrets (closes co-ffi-004 + CLI-001)

**Design.** Split the single `FetchOnly` inspection into the two phases the
sandbox crate already models:
1. **Fetch phase** — `NetworkPolicy::FetchOnly`, network on, but it runs ONLY
   `cargo fetch`/registry download into a scoped `registry_cache` dir. No
   proc-macro/build-script expansion, no rustdoc — no foreign CODE runs here.
2. **Compile/introspect phase** — `NetworkPolicy::Denied` (fresh empty net ns
   via `--unshare-net`), `registry_cache: Some(<the fetched dir>)` bound
   read-only, `CARGO_NET_OFFLINE=1` (already set for `Denied` at `lib.rs:325`).
   The inspector's rustdoc/proc-macro/build-script expansion — the foreign code
   — runs here, with NO egress.

Wire this in `ffi.rs::run_inspector`: replace the single `run_in_bwrap_jail`
with a fetch invocation then a denied-network introspection invocation over the
shared `scoped_tmp`/`registry_cache`. The inspector binary already accepts a
mode split (or add a `--offline`/`--registry-cache <dir>` flag to
`inspector_argv`, `driver.rs`); if the inspector cannot be cleanly two-phased,
the minimum viable fix is to run the whole inspection under `Denied` after a
separate `cargo fetch` pre-step, since the inspector's own network need is only
the registry download.

**Stop mounting the credential file (CLI-001).** In `ffi.rs::run_inspector`
(`ffi.rs:207-217`), do NOT push the whole `home.join(".cargo")` into
`toolchain_ro_binds`. Bind only the needed subtrees:
- `~/.cargo/bin` (already separately pushed to `path_prepend`) — bind that dir
  read-only, not the parent.
- the registry cache subtree, IF the offline compile phase needs the user's
  existing registry (prefer the scoped `registry_cache` from phase 1 instead,
  so no host `~/.cargo/registry` is bound at all).
- explicitly NEVER `~/.cargo/credentials.toml` / `~/.cargo/credentials`.

Defense-in-depth even under `Denied`: add a `JailSpec` mask so any
credentials path that would otherwise be reachable is tmpfs-masked before the
re-bind. But with F2's `Denied` compile phase, exfil has no egress even if the
file were readable — the two fixes reinforce.

**Residual risk after F2.** The fetch phase still runs `cargo` (trusted
tooling, not the target crate's code) with egress — a compromised cargo/registry
TLS path is out of this theme's scope. If a genuine two-phase inspector split is
infeasible and the fallback (whole inspection under `Denied` after a pre-fetch)
is taken, confirm the inspector needs no network DURING rustdoc expansion for
any supported crate; a crate whose build script fetches at build time will fail
closed under `Denied` — that is the correct secure outcome (record as a known
limitation, not a regression). The narrowed binds mean a crate needing a
`~/.cargo` path we did not enumerate fails closed (over-restrict) — widen only
with a specific bind, never by re-adding the parent.

### F3 — hard-fail when caps are unavailable + concurrent pipe drain (closes co-ffi-002)

**Design.** Make cap availability a REFUSAL, not a silent omission — the same
fail-closed posture as no-isolation.
- Add `SandboxDefect::CapsUnavailable { missing: Vec<&'static str> }` (still
  `IPE-F4410`). In `run_in_bwrap_jail` (`lib.rs:487`), when
  `caps.timeout.is_none()` or `caps.prlimit.is_none()`, return that defect
  BEFORE spawning — never build an argv that omits the wall clock or rlimits.
- `bwrap_argv`'s `Option` params become non-optional (`prlimit: &Path`,
  `timeout: &Path`) so an uncapped argv is UNREPRESENTABLE — the invalid state
  (a jail argv with no caps) cannot be constructed. The consumer gate
  (`ffi.rs:179`) already refuses without bwrap; extend the same message to name
  missing `timeout`/`prlimit`.
- Fix the pipe-drain deadlock: read stdout and stderr CONCURRENTLY (two threads,
  or a `select`/poll loop) instead of stdout-to-completion-then-stderr
  (`lib.rs:517-519`), so a stderr-filling payload cannot wedge the reader. With
  `timeout` now mandatory the wall clock is a guaranteed backstop, but the
  concurrent drain removes the hang independent of it.

**Residual risk after F3.** Hosts lacking `timeout`/`prlimit` now REFUSE `ipe
add` (with a clear "install coreutils/util-linux" message) rather than run
uncapped — a completeness cost taken deliberately (P1 > P5). An operator can
still `IPE_FFI_ALLOW_UNSANDBOXED=1` to bypass everything; that override remains
the single, warning-printed escape hatch (unchanged; documented as dangerous).

### F4 — wire the proven `unshare` tier, or delete the dead one (closes co-ffi-003)

**Design.** Two honest options; pick one, do not leave the advertised-but-dead
state.
- **(preferred) wire it**: in `ffi.rs::run_inspector`, add an
  `UnshareCandidate` arm that spawns the inspector under `unshare` with the
  child running `prove_isolation` (`lib.rs:408`) as its FIRST action before any
  untrusted code, refusing on any `IsolationDefect`. This realises the
  bwrap→unshare→refuse ladder the module doc promises. Requires a small child
  entry shim (the inspector, or a re-exec of `ipe`, calls `prove_isolation`
  first).
- **(if wiring is out of scope this cycle) delete it**: drop
  `Mechanism::UnshareCandidate`, `current_ns_ids`, `prove_isolation`,
  `assert_net_namespace_empty` and rewrite the module doc (`lib.rs:11`) to state
  the honest posture: bwrap-or-refuse. This removes the false advertisement and
  the dead code the `pedantic`/`nursery` deny-set would otherwise flag.

**Residual risk.** If wired: `prove_isolation` depends on `/proc`+`/sys`
readability inside the child; it already fails closed on an unreadable proof
(`ProcUnreadable`), so the residual is only a kernel that both disables userns
AND blocks the proof reads — that refuses, which is safe. If deleted:
unshare-only hosts have no sandboxed path and must use the explicit override or
install bwrap — a documented, honest limitation.

### F5 — cache root stops at ipe.toml + ownership check (closes CLI-002)

**Design.** In `find_cache_root` (`ffi.rs:26-40`), bound the upward walk and
verify ownership:
- Stop the walk at the project root — the nearest ancestor containing
  `ipe.toml` (the manifest). Do not walk above it. If no `ipe.toml` is found,
  there is no project and no cache (return `None`), never a filesystem-root hit.
- Before trusting a found `.ipe/cache/ffi/rust`, verify it is owned by the
  current uid (and not group/other-writable) via `std::fs::metadata` +
  `MetadataExt::uid()`/`mode()` (Unix). A cache dir not owned by the invoker is
  refused with a typed diagnostic (reuse `Diagnostic::ArtifactIo` or add a
  `CacheOwnership` variant), not silently loaded.

**Residual risk.** A project legitimately placed under a shared-ownership path
(e.g. a CI checkout owned by a build user differing from the runner uid) now
refuses — that is the correct secure default; the operator fixes ownership or
sets an explicit opt-in. Combined with F1's re-derive-from-`kernel.json` load,
even a same-uid planted cache cannot inject Rust (it would have to forge a
validated `kernel.json` that decodes AND emits benign wrappers), so F5 is
defense-in-depth narrowing the discovery surface rather than the sole barrier.

### F6 — scratch under ~/.cache/ipe + fresh-dir + default-prompt install (closes CLI-008, CLI-007)

**Design.**
- **CLI-008**: replace `temp_dir().join("ipe-ffi-add-{crate}-{pid}")`
  (`ffi.rs:192`) with a directory under `~/.cache/ipe/ffi-scratch/` (the
  sanctioned write-boundary root) created with a randomized component and
  `create_dir` semantics that FAIL if the path already exists (reject a planted
  symlink/dir), e.g. a random suffix + `fs::create_dir` (not `create_dir_all`).
  Bind that fresh dir RW into the jail as today.
- **CLI-007**: change `run_install` (`ffi.rs:362`) so bare `ipe install`
  PROMPTS (per-crate, or once for the whole `[rust.dependencies]` list, reusing
  `run_add`'s `trust_summary` + `read_yes_no`), and reserve the silent path for
  an explicit `--yes`. Update the usage string to match.

**Residual risk.** CLI-008: `~/.cache/ipe/` on a multi-user host is still the
invoker's own tree; the fresh-random + `create_dir`-fails-if-exists closes the
name race. CLI-007: a scripted/CI `ipe install --yes` is unchanged (explicit
consent); only the accidental bare-invocation gap closes. Both are
low-severity; they amplify rather than gate the RC-A/RC-B surface, so they ride
after the two spines.

---

## Impl plan (ordered, each step independently testable)

Steps are grouped by fix; F1 and F2 are the push-blockers and come first.

1. **F1a — newtypes.** Add `RustTypeExpr`, `RustPattern`, `FieldSelector` to
   `naming.rs` with `parse`/`as_str`/`Display` and the new `WireDefect`
   variants (`InvalidType`, `InvalidPattern`, `InvalidSelector`) in `diag.rs`.
   Unit tests in `naming.rs`: accept the real inspector grammar
   (`::std::string::String`, `Vec<::crate::T>`, `Result<T, E>`, `&mut T`, `()`,
   tuples); REJECT `String { } fn e(){…}`, `T; std::process::exit(1)`,
   `T)//`, unbalanced `<`, `foo(bar)`, embedded whitespace-newlines.
2. **F1b — thread through `typeref.rs`.** `InnerTypeRef::Prim/Ctor` carry
   `RustTypeExpr`; `TryFrom<WireTypeRef>` parses; `render` emits `.as_str()`.
   Update `to_wire_json`/`param_indices`/`any_serde`/`is_vec_ctor`. Tests: the
   existing round-trip still passes; a `{"prim": "x; std::process::exit(1)"}`
   now fails decode.
3. **F1c — thread through `call.rs`.** `Call.path: Vec<RustIdent>`, `method:
   Option<RustIdent>`, `trait_qualifier: Option<(RustTypeExpr, RustTypeExpr)>`;
   parse in `decode`; `render_body` uses validated values. Tests: existing
   render tests pass; a path segment `["::x)//evil"]` fails decode with a typed
   defect.
4. **F1d — thread through `pkginfo.rs`.** `Param.rust_type`/`recv_rust_type:
   Option<RustTypeExpr>`, `EnumArm.pattern: RustPattern`, `EnumExtract.selector:
   FieldSelector`, `EnumCtor.struct_fields: Vec<RustIdent>`; parse in
   `decode_shape`/`decode_arms`/`param_from_wire`/`TryFrom`. Update
   `bindings.rs::resolve_rust_type` and the enum emitters to consume validated
   values. Tests: `pkginfo` unit tests for an injection-bearing `rustType`,
   `enumStructFields`, `selector` each DROP the binding with the right defect.
5. **F1e — close the warm-cache re-decode (door (a)).** Change `load_catalog`
   (`driver.rs:524`) to reconstruct `bindings_source` by re-decoding
   `<slug>.kernel.json` through the (now type-validated) `PkgInfo`/`Call` gate
   and re-running `emit_bindings`, rather than reading `<slug>_bindings.rs` as
   trusted text. Remove or checksum-gate the `_bindings.rs` file input. Test in
   `src/compiler/ffi` (driver tests) + an end-to-end negative test.
6. **F1f — SEAL + injection negative tests** in
   `src/ipe-cli/tests/negative_suite.rs` (the audit's cited home for
   rejection tests): a project with a planted `_bindings.rs`/`kernel.json`
   carrying an injected wrapper body MUST be rejected (typed diagnostic, `ipe`
   never exit-0, no `src/ffi.rs` emitted), and an unbalanced-`<` `rust_type`
   MUST reject at decode (not exit-0-then-cargo-fail). Add a matching driver
   test that the warm-cache load refuses the planted cache.
7. **F2a — two-phase sandbox wiring** in `ffi.rs::run_inspector`: fetch phase
   (`FetchOnly`, no foreign code) → introspect phase (`Denied` +
   `registry_cache`). Add `inspector_argv` offline/registry-cache support in
   `driver.rs` if needed. Test: a `bwrap_argv`/spec-level unit test asserting
   the introspection phase carries `--unshare-net` + `CARGO_NET_OFFLINE=1` and
   a bound `registry_cache`.
8. **F2b — narrow the binds (CLI-001)**: replace the `home.join(".cargo")`
   bind with `~/.cargo/bin` only (+ scoped registry cache); never bind
   `credentials.toml`. Unit test: the assembled `toolchain_ro_binds` for a run
   contains no `.cargo/credentials` path and no bare `.cargo` parent.
9. **F3 — mandatory caps + concurrent drain** in `sandbox/src/lib.rs`: add
   `SandboxDefect::CapsUnavailable`; make `bwrap_argv` params non-optional;
   refuse in `run_in_bwrap_jail` when caps absent; concurrent stdout/stderr
   drain. Tests: `run_in_bwrap_jail` with `caps.timeout=None` returns
   `CapsUnavailable`; the `bwrap_argv` tests updated to the non-optional
   signature; a drain test with a stderr-heavy fake stream does not hang.
10. **F4 — wire or delete the unshare tier.** If wired: the `UnshareCandidate`
    arm + child `prove_isolation` shim + a refusal-on-defect test. If deleted:
    remove the dead items + rewrite `lib.rs:11` doc; the existing
    `isolation_proof_fails_outside_a_jail` test is removed with it.
11. **F5 — bounded cache root + ownership** in `ffi.rs::find_cache_root`: stop
    at the `ipe.toml` ancestor; uid/mode ownership check; typed refusal. Tests:
    a cache above the manifest root is NOT found; a non-owned cache dir is
    refused.
12. **F6 — scratch dir + install prompt**: scratch under `~/.cache/ipe/` with a
    random suffix + fail-if-exists; bare `ipe install` prompts. Tests: the
    scratch path is under the cache root and creation fails on a pre-existing
    path; `run_install` with no `--yes` takes the prompt path.

---

## Risk / blast radius

- **F1 is the widest change** — it retypes the FFI domain layer
  (`typeref`/`call`/`pkginfo`/`bindings`). Re-gate: the whole `src/compiler/ffi`
  unit suite, every FFI golden under `src/ipe-cli/tests/golden_ffi_*`, the
  negative suite, and a fresh `ipe add`+`ipe build` of a real crate (semver/uuid)
  end-to-end (the SEAL on a real binding). The `absolutize_crate`/`resolve_rust_type`
  edits touch the emitted wrapper text — re-bless any affected goldens ONLY after
  confirming the new text is byte-identical modulo validation (a changed golden
  that hides a dropped binding is a §0 violation).
- **F1e (warm-cache re-derive)** changes what `load_catalog` trusts — re-gate
  the driver tests and any warm-build/incremental FFI path; confirm a normal
  warm build still produces identical `src/ffi.rs`.
- **F2** changes the jail invocation shape — re-gate the sandbox unit tests and
  a real sandboxed `ipe add` on a Linux host with bwrap; confirm inspection of
  a proc-macro crate still succeeds under the `Denied` introspect phase.
- **F3** changes `bwrap_argv`'s signature (Option→ref) — every caller and the
  argv tests update together; a host without `timeout`/`prlimit` now refuses
  (intended).
- **F5/F6** are localised to `ffi.rs`; low blast radius.
- **Clippy**: new newtypes + parsers must satisfy the `pedantic`/`nursery`
  deny-set (backtick doc identifiers, no `unwrap`/`expect`, no `indexing_slicing`
  in the parsers — use iterators/`char_indices`).

---

## Proposed backlog entries

```json
{"id": "TBD", "priority": 1, "phase": "principles-audit-fix", "task": "F1: validated type/path/selector newtypes at the FFI decode boundary (RustTypeExpr/RustPattern/FieldSelector) threaded through typeref/call/pkginfo so injection-bearing type/path/enum strings are unrepresentable past decode; re-derive load_catalog bindings from the validated kernel.json instead of trusting _bindings.rs text; SEAL + injection negative tests", "notes": "Closes co-ffi-001 (critical, code injection into unsandboxed compiled bindings + SEAL breach via both fresh-install and warm-cache doors). Root cause RC-A. Steps 1-6. Residual: RustTypeExpr grammar is an allowlist — review against real inspector output so common bindings don't over-drop; widen only with matching negative tests.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": 1, "phase": "principles-audit-fix", "task": "F2: two-phase no-egress-while-executing sandbox for ipe add (FetchOnly fetch phase then NetworkPolicy::Denied introspect phase over a scoped registry_cache) and stop mounting ~/.cargo secrets into the jail (bind ~/.cargo/bin only, never credentials.toml)", "notes": "Closes co-ffi-004 (medium) + CLI-001 (high, crates.io token exfil). Shared root RC-B. Steps 7-8. Residual: fetch phase still runs trusted cargo with egress; a build-script that fetches at introspect time now fails closed under Denied (correct, document as limitation).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": 2, "phase": "principles-audit-fix", "task": "F3: hard-fail (typed IPE-F4410 refusal) when timeout/prlimit caps are unavailable — make bwrap_argv cap params non-optional so an uncapped jail argv is unrepresentable — plus concurrent stdout/stderr drain to remove the sequential-pipe deadlock", "notes": "Closes co-ffi-002 (medium). Root cause RC-C. Step 9. Residual: hosts lacking coreutils/util-linux now refuse ipe add (deliberate P1>P5 cost); IPE_FFI_ALLOW_UNSANDBOXED=1 remains the single warned override.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": 2, "phase": "principles-audit-fix", "task": "F4: wire the proven unshare fallback tier (spawn under unshare, child runs prove_isolation before any untrusted code, refuse on IsolationDefect) — or delete the dead UnshareCandidate/prove_isolation surface and rewrite the module doc to the honest bwrap-or-refuse posture", "notes": "Closes co-ffi-003 (medium). Root cause RC-D. Step 10. Prefer wiring; deletion is the honest fallback if wiring is out of cycle. Residual documented per branch in the spec.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": 3, "phase": "principles-audit-fix", "task": "F5: bound find_cache_root at the ipe.toml project root and add a uid/mode ownership check so a planted ancestor FFI cache cannot be discovered or loaded", "notes": "Closes CLI-002 (medium) — the concrete warm-cache delivery vector for co-ffi-001(a). Root cause RC-E. Step 11. Defense-in-depth behind F1e; residual is a legitimately shared-ownership checkout now refusing (correct default).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
{"id": "TBD", "priority": 3, "phase": "principles-audit-fix", "task": "F6: move the FFI jail scratch dir under ~/.cache/ipe/ with a randomized fail-if-exists name (write-boundary + symlink-race fix) and make bare `ipe install` prompt for trust instead of defaulting to --yes", "notes": "Closes CLI-008 + CLI-007 (low). Root cause RC-E. Step 12. Localised to ffi.rs; amplifies rather than gates the main surface.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t1-ffi-hardening.md", "blocked_by": [], "status": "pending"}
```
