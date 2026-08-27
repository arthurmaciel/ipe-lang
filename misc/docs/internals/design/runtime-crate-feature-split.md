# Runtime-crate feature split

Design: make every currently-unconditional external dependency of
`ipe-runtime-rust` optional and its consuming module `cfg(feature = …)`-gated,
so the reachability-driven feature selection
(`docs/internals/design/function-level-dependency-emission.md`) actually drops the
residual dependency floor. This is the runtime-crate half that design defers;
companion to ADR 0054 and
`precompiled-runtime-and-shared-target.md`.

Security first: a program that calls nothing must ship nothing. Every crate a
program links without needing it is supply-chain surface. The precedence
(`PRINCIPLES.md`) is Security > Correctness > Soundness > Efficiency; the
counter-obligation is that dropping a dependency a program needs is a
Correctness break (exit-0 emit, failing `cargo build`) — every gate below errs
toward inclusion and the SEAL machinery (§5) proves fail-closure mechanically.

## 1. The residual floor, measured

`cargo tree --no-default-features` on `src/runtime/rust` today resolves **42
crates** (44 with the `json` floor feature every program currently selects).
Direct non-optional deps in `src/runtime/rust/Cargo.toml`, the module(s) that
use each, the feature it moves behind, and the subtree it drops (unique crates
beyond those shared with a remaining floor; overlaps noted):

| Dep (Cargo.toml line) | Using modules (`src/runtime/rust/src/`) | New/changed gate | Subtree dropped |
|---|---|---|---|
| `serde` (+`derive`) :69 | `core.rs`, `error.rs`, `basics.rs`, `decimal.rs`, `html.rs`, `dom/{diff,dispatch,form}.rs`, `ui/helpers.rs`, `secret.rs` (doc-only), plus already-gated `json/http_stream/ws_client/web` | `serde` (new) | 7 (`serde`, `serde_core`, `serde_derive`, `syn`, `quote`, `proc-macro2`, `unicode-ident`) |
| `serde_json` :27 (optional, but floor via unconditional `Json`) | `json.rs`, `stringify.rs` (gated impl), `core.rs` (tests only) + gated surfaces | `json` demoted from unconditional selection | ~4 (`serde_json`, `itoa`, `ryu`; `memchr` shared with regex) |
| `serde_urlencoded` :72 | `dom/form.rs` only | module gate `cfg(any(feature = "web", feature = "wasm-client"))`; dep optional under `web`/`wasm-client` | 2 (`serde_urlencoded`, `form_urlencoded`) |
| `regex` :95 | `regex_kernel.rs`; `string.rs` (only `string_is_url`, :498) | `regex` (new) | 5 (`aho-corasick`, `memchr`, `regex-automata`, `regex-syntax`) |
| `chrono` :106 | `log.rs` (:49 timestamp), `time.rs`, `db.rs`, `telemetry_spill.rs`, `web/*` | `log` + `time-core` (new); `db`/`web`/`time` imply | 3 (`iana-time-zone`, `num-traits` shared w/ decimal) |
| `uuid` :103 | `uuid_kernel.rs`, `server.rs` (:1731 boundary token), `web/{csrf,mod}.rs` | `uuid` (new); `server`/`web` imply | 1 (rest shared) |
| `rust_decimal` :112 (`features=["serde"]`) | `decimal.rs`, `money.rs`, `db.rs` (:518 `SqlValue`) | `decimal` (new); `db` implies; serde half via weak `rust_decimal?/serde` | 2 own (`arrayvec`, `num-traits`) + it is the floor's serde-stack driver |
| `base64` :102, `hex` :104, `percent-encoding` :105 | `encoding.rs`, `bytes.rs`; `crypto_core.rs` (:199 randomToken base64url), `db.rs` (hex), `server.rs` (percent), gated `email/jwt/web` | `encoding` (new); `crypto-core`/`server`/`db`/`jwt`/`email`/`web` imply | 3 |
| `sha2` :35, `hmac` :84, `sha1` :85, `md-5` :86 | `crypto_core.rs`, `crypto.rs` (sha1/md5 ONLY — module already gated `crypto`∨wasm), `db.rs` (:1042 migration checksum), `email.rs`, `web/mod.rs` (SRI) | `crypto-core` (new) for sha2/hmac; sha1+md-5 move under `crypto` + `wasm-client` | ~11 (`digest`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`, `cpufeatures`, `cfg-if`) |
| `subtle` :73 | `crypto_core.rs`, `secret.rs`, `server.rs`, `web/{console,csrf}.rs` | `crypto-core` (and `secret` implies it) | 1 |
| `zeroize` :83 | `secret.rs` only | `secret` (new) | 1 |
| `getrandom` :79 | `crypto_core.rs`, `random.rs` | `random` + `crypto-core` (either enables) | 1 |
| `unicode-general-category` :101 | `char_kernel.rs` only (the General_Category kernels) | `char-category` (new) | 1 |
| `libc` (unix target) :132 | `io.rs` (readSecret termios), `live/console_proxy.rs` (gated) | stays floor (tiny, ubiquitous); optional stretch: `io-terminal` feature keyed on `Io.readSecret` | 1 |

End state: the true floor — `core`, `error`, `basics`, `stringify`, `task`
(sync half), `io`, `file`, `system`, `path`/`path_core`, `list`, `dict`,
`set`, `string` (minus `string_is_url`), `math`, `bitwise`, `telemetry`,
`config` (env half), `debug`, `ffi_polyfills`, `html`/`dom`/`ui`/`css*`
(serde-free halves), `locale` (stub half) — is **std-only**. Bare
`Io.println` Program: app crate + `ipe_runtime` + `libc` = **3 crates** (2
with the `io-terminal` stretch), from ~45 today.

## 2. Serde decoupling (the crux)

### 2.1 Where the floor needs serde today

- `core.rs`: `IpeMaybe` derives `serde::Serialize` (:283) and carries a
  hand-written generic `Deserialize` visitor (:309–…); `IpeResult` likewise.
  The derives are generic-BOUND (`impl<T: Serialize>`), so a non-serde inner
  type is unaffected — the coupling is purely that the `serde` crate must be
  in the graph.
- `error.rs`: the `IpeError` family derives Serialize/Deserialize (:36, :74,
  :82, :93, :108, :121).
- `basics.rs`: `IpeOrder` derives (:32).
- `decimal.rs`/`money`: `IpeDecimal` derives (:10) + `rust_decimal/serde`.
- `html.rs`, `dom/{diff,dispatch}.rs`, `ui/helpers.rs`: the DOM patch/event
  wire types derive serde — consumed only by the Web SSE wire, the Webview
  bridge, and the browser-WASM sink.
- `dom/form.rs`: `serde_urlencoded` typed form decode — same three consumers.
- Emitted user types do NOT force serde on the floor: `emit_types.rs` gates
  the serde derive on the serde-admissibility predicate AND the web surface
  (`self_serde && ctx.uses_web`, :671) — a bare Program's records derive only
  `Clone, Debug, PartialEq` + `IpeStringify`.

### 2.2 The plan: feature-gate the impls, not split the types

No type splits, no parallel serde-free representations — one `IpeMaybe`, its
serde impls conditional:

1. New runtime feature `serde = ["dep:serde", "rust_decimal?/serde"]` (weak
   feature: `rust_decimal`'s serde half activates only when `decimal` is also
   selected — no dep introduced by the feature itself).
2. Every floor derive becomes
   `#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]`
   (Serialize-only where that is today's shape). The hand-written `IpeMaybe`
   visitor module and any `impl … serde …` block get
   `#[cfg(feature = "serde")]`.
3. Every serializing feature implies it: `json = ["serde_json", "serde"]`,
   and `serde` is added to `db`, `web`, `config`, `email`, `http_client`
   (stream decode), `websocket_client`, `wasm-client`. `serde_urlencoded`
   becomes optional, listed by `web` and `wasm-client`, and `dom/form.rs` is
   gated `cfg(any(feature = "web", feature = "wasm-client"))` (the exact
   pattern `log.rs` uses for its wasm console sink).
4. `runtime_features.rs` stops inserting `RuntimeFeature::Json`
   unconditionally: `Json` is keyed on a new `uses_json` flag (Json/Decoder/
   stringify-of-`Json.Value` kernels reachable), and `RuntimeFeature::Serde`
   is keyed on the union of its implying selectors (in practice the SSOT only
   inserts `Serde` when a serde-needing surface is selected — the crate-side
   feature implications carry the closure, mirroring how `jwt` implies
   `crypto` today).

Programs that use Json keep byte-identical behavior: `json` implies `serde`,
the derives reappear, the visitor compiles — nothing about the serde
representation changes, only its presence.

### 2.3 The boundary risk and its handling

The hazard: a floor type crossing a serialization boundary in a program whose
feature set did not select `serde` — e.g. a Web Model field of type
`Maybe Int` (needs `IpeMaybe: Serialize`), a form-decode target, an FFI
struct. Handling, by construction rather than by review:

- Every emit path that requires `Serialize`/`DeserializeOwned` on a floor
  type is downstream of a surface flag that implies `serde`: the Web Model
  gate (`ir_type_is_serde`, enforced by `emit_model_gate.rs`) only exists
  under `uses_web`; form targets only under web/wasm; JSON encode/decode only
  under `uses_json`; config decode under `uses_config`. There is no emitted
  `serde` bound reachable from a feature set that excludes `serde` — and the
  featureset-closure SEAL's reference scan (§5) makes that mechanical, not
  aspirational.
- FFI: `Callee::Ffi` stays fail-closed (forces its bound crates); FFI
  wrapper types that serialize do so under their own crate deps, not the
  runtime floor. `ffi_polyfills.rs` is serde-free.
- `Secret` never implements serde by design (`secret.rs` doc) — the split
  cannot regress it.

## 3. Feature and module gating

New `[features]` in `src/runtime/rust/Cargo.toml` (dep moved to
`optional = true` in the same change), and the matching `RuntimeFeature`
variants in `src/compiler/backend/rust/src/runtime_features.rs`:

| Feature | Enables | Gated code | `RuntimeFeature` | Selected when (reachability flag) |
|---|---|---|---|---|
| `serde` | `dep:serde`, `rust_decimal?/serde` | derives/impls of §2.2 | `Serde` | implied by serializing surfaces |
| `log` | `dep:chrono` | `log.rs` module | `Log` | a `Log.*` kernel reachable (`uses_log`, new) |
| `time-core` | `dep:chrono` | `time.rs` module (`time` = zone DB implies it) | `TimeCore` | a `Time.*` kernel reachable (existing `uses_time` splits: any Time kernel ⇒ `time-core`; IANA-zone kernels ⇒ also `time`) |
| `regex` | `dep:regex` | `regex_kernel.rs`; `string_is_url` relocates into it | `Regex` | a regex-backed kernel (incl. `String.isUrl`) reachable (`uses_regex`, new) |
| `uuid` | `dep:uuid` | `uuid_kernel.rs` | `Uuid` | a `Uuid.*` kernel reachable (`uses_uuid`, new); implied by `server`, `web` |
| `random` | `dep:getrandom` | `random.rs` | `Random` | a `Random.*` kernel reachable (`uses_random`, new) |
| `decimal` | `dep:rust_decimal` | `decimal.rs` + `money.rs` | `Decimal` | a `Decimal.*`/`Money.*` kernel reachable (`uses_decimal`, new); implied by `db` |
| `encoding` | `dep:base64`, `dep:hex`, `dep:percent-encoding` | `encoding.rs` + `bytes.rs` | `Encoding` | an encoding/bytes kernel reachable (`uses_encoding`, new); implied by `crypto-core`, `server`, `db`, `jwt`, `email`, `web` |
| `char-category` | `dep:unicode-general-category` | the General_Category kernels of `char_kernel.rs` (module split: category half moves to a gated sibling; the std-only half stays floor) | `CharCategory` | a category kernel reachable (`uses_char_category`, new) |
| `crypto-core` | `dep:sha2`, `dep:hmac`, `dep:subtle`, `dep:getrandom`, `encoding` | `crypto_core.rs` module | `CryptoCore` | a crypto-floor kernel reachable (`uses_crypto_core`, new); implied by `crypto`, `jwt`, `db` (migration checksum), `web` (SRI + csrf), `email`, `server` |
| `secret` | `dep:zeroize`, `dep:subtle` | `secret.rs` module | `Secret` | a `Secret.*` kernel or `Secret`-typed value reachable (`uses_secret`, new) |
| (existing `crypto`) | + `dep:sha1`, `dep:md-5` | already-gated `crypto.rs` | — | unchanged |
| (existing `json`) | + `serde` | unchanged | `Json` | demoted: `uses_json` (new), no longer unconditional |

Notes:

- Wasm32 target-table twins (`getrandom` `js`, `uuid` `js` at Cargo.toml
  :137–:144) become `optional = true` with the same names — cargo unifies
  optionality per dep name, so the `js`-feature union survives unchanged.
- The selection machinery is untouched: the reachability pass already scans
  kernels per reachable function into `KernelUsage`/`ir::Module.uses_*`; this
  design only adds flags (`KernelUsage::record` arms + `ir::Module` fields +
  `EmitCtx` threading) and SSOT insertions. No new analysis.
- `mod.rs` module gates follow the existing house pattern (`#[cfg(feature =
  …)] pub mod X; #[cfg(feature = …)] pub use X::*;`), with wasm unions
  (`all(target_arch = "wasm32", feature = "wasm-client")`) exactly where
  today's `crypto`/`http_client`/`ws_client`/`tea` gates put them.
- Item-level gating is avoided wherever a relocation suffices (e.g.
  `string_is_url` → `regex_kernel.rs`): the SEAL's cfg-satisfaction proof is
  module-granular, and a module-granular gate is the shape it can verify.
  Where an item gate is unavoidable, the kernel enters the item-gate registry
  (§5).

### Prerequisite

This crate split only pays off once the emitted prelude stops
hard-referencing floor modules. In `project.rs`, only the `Http` prelude
section is conditional (`native_runtime_bindings`, :1011); the sectioned
prelude and the IR-reachability restriction of `scan_kernel_usage` are
specified in `function-level-dependency-emission.md` §3.1/§3.3 and
implemented on the in-flight reachability branch — **that work must be merged
first**. Until then, a bare program's `main.rs` references
`ipe_runtime::{log,time,random,crypto_core,json,…}` (templates/main.rs
wrapper block) and none of those modules may be cfg'd off.

## 4. Emitted-manifest surfaces

Three manifests consume the split:

- **Dependency model (default)** — `templates/Cargo.dep.toml`: nothing to
  change structurally; `__IPE_RUNTIME_FEATURES__` shrinks per program. This
  is where the floor collapse manifests.
- **Vendored fallback** (`IPE_RUNTIME_VENDORED=1`) — `templates/Cargo.toml`:
  keeps its fat unconditional dep set (it is the debugging escape hatch, not
  the shipped path), but must DECLARE the new feature names (empty, like its
  existing `tokio`/`crypto`/… entries) and default the floor features ON so
  the vendored sources' new `cfg` gates stay satisfied and check-cfg stays
  quiet.
- **Wasm** — `WASM_CARGO_TOML` (project.rs :73): the wasm emit is a closed
  vendoring template (`runtime_dep` has no effect on it) and
  `WASM_RUNTIME_MOD_RS` declares the full floor module set statically. Same
  treatment as vendored: declare the new features, default them on, keep the
  dep list as-is — wasm output is byte-identical, parity preserved. In the
  standalone crate, `wasm-client` implies the floor features its static
  module set needs (`serde`, `json`, `encoding`, `random`, `uuid`,
  `decimal`, `regex`, `char-category`, `secret`, `crypto-core`, `log`,
  `time-core`) so `--target wasm32-unknown-unknown --features wasm-client`
  keeps compiling (the wasm-floor CI build). Per-program wasm trimming is a
  later, separate step.

## 5. SEAL extension (fail-closed)

The protected invariant: exit-0 emit ⇒ `cargo build` succeeds; no needed
crate ever dropped.

- `tests/runtime_featureset_closure.rs` already (1) checks every selected
  feature exists in the crate's `[features]`, (2) resolves the set closed,
  (3) scans emitted `ipe_runtime::<module>::` references against the crate's
  own `src/mod.rs` cfg gates. New gated modules are covered automatically by
  the same source parse — each new feature needs only its `RuntimeFeature`
  variant and mask coverage.
- **Sweep growth**: both closure SEALs enumerate `2^FLAG_COUNT` masks
  (`FLAG_COUNT = 18` today); the ~10 new flags make the exhaustive sweep
  infeasible (2^28). Restructure BEFORE adding flags: (a) per-feature closure
  proofs — for each single feature, the resolved set is closed and its
  module references satisfied (linear); (b) a monotonicity proof — cargo
  features are additive, and the SSOT is monotone in the flags (each flag
  only inserts), so any union of individually-closed sets is closed; (c) the
  existing exhaustive sweep retained over the 18 legacy surface bits with
  floor bits sampled (all-off, all-on, each-singleton); (d) the ground-truth
  `cargo build` E2E on representative fixtures (bare, log-only, regex-only,
  json-only, web).
- **Item-gate registry**: kernels whose implementation is item-gated inside
  an always-compiled module (if any survive the relocation policy of §3) are
  listed in one exhaustiveness-tested table (kernel → required feature); the
  SEAL asserts every emitted wrapper referencing such a kernel has its
  feature selected. This closes the one hole module-granular scanning cannot
  see.
- **Drop-a-feature proofs**: per new variant, deleting its SSOT insertion (or
  its kernel→flag arm) must turn a SEAL or fixture test red — proven once per
  feature at introduction, as the existing `jwt`/`url` gates were.
- The subset-differential obligation of the parent design (new feature set ⊆
  old, build stays green) applies to every step here and is already
  specified there.

## 6. Golden re-bless scope

Nearly every dependency-model golden's `Cargo.toml` `features = […]` line
shrinks (most visibly: `"json"` disappears from programs that never touch
Json), and prelude sections drop with the sectioned-prelude prerequisite.
Churn is wide but shallow — one manifest line + wrapper-block deletions per
golden; no user-code emission changes. The regeneration is mechanical and
double-regen-stable (the feature list is a canonical `BTreeSet` order via
`RuntimeFeatureSet::as_feature_names`; prelude sections are emitted in fixed
registry order). Re-bless per landing, review the diff, not its size.

## 7. Implementation plan (test-first, least-risky first, independently landable)

Gate for every step: workspace build + clippy + modset/featureset SEALs +
`cargo check -p ipe-runtime-rust --no-default-features` and per-new-feature
`--features <f>` alone + the wasm-floor build
(`--target wasm32-unknown-unknown --features wasm-client`) + examples sweep +
a crate-count delta assertion on the bare fixture (`cargo tree` on the
emitted project; the number must only go down).

0. **Prerequisite check** — the IR-reachability pass + sectioned prelude
   (parent design §3.1/§3.3) merged to main. Blocked until true.
1. **SEAL restructure first** (no behavior change). Failing test: a unit test
   asserting the closure proofs cover a synthetic 30-flag universe in
   bounded time. Deliverable: per-feature closure proofs + monotonicity +
   sampled sweep (§5) replacing the raw `2^N` enumeration in BOTH closure
   SEALs; item-gate registry scaffold (empty). Lands green with today's 18
   flags.
2. **Leaf codecs** (`encoding`, `char-category`; `sha1`/`md-5` → `crypto`).
   Failing test: bare fixture's `cargo tree` contains no
   `base64`/`hex`/`percent-encoding`/`unicode-general-category`/`sha1`/`md-5`.
   Smallest blast radius: sole-consumer modules, zero cross-module
   implications beyond adding `encoding` to `crypto-core`-adjacent surfaces.
3. **`regex` + `uuid` + `random`**. Failing test per feature (bare tree lacks
   the crate; a `String.isUrl` fixture still builds+runs). Includes the
   `string_is_url` relocation into `regex_kernel.rs` (prelude wrapper path
   re-bless).
4. **`chrono` split** (`log`, `time-core`). Failing test: bare tree lacks
   `chrono`; a `Log.info` fixture keeps it. Adds `time-core` to `time`,
   `db`, `web`.
5. **`decimal`**. Failing test: bare tree lacks `rust_decimal`; a `Money`
   fixture and a Db fixture keep it.
6. **`secret` + `crypto-core` demotion**. Failing test: bare tree lacks
   `sha2`/`hmac`/`subtle`/`zeroize`/`getrandom`; jwt/db/web/email fixtures
   keep the crypto floor. Highest implication fan-out of the non-serde
   steps — every gated surface that touches the crypto floor gains the
   implication in the same commit as its fixture proof.
7. **Serde decoupling** (§2 — the crux, last). Failing test: bare fixture's
   tree contains NO `serde*` crate and the runtime builds
   `--no-default-features`; a Json fixture, a Web-Model-with-`Maybe` fixture,
   and a form-decode fixture stay green. Deliverable: `cfg_attr` derives,
   gated visitor impls, `serde`/`json` feature rewiring, `serde_urlencoded` →
   web/wasm, `Json` demotion in `runtime_features` keyed on `uses_json`.

   **Status — crate-level decoupling landed; emitted-app `json` demotion
   pending.** Done: `serde` + `serde_urlencoded` are optional; a new `serde`
   feature (`["dep:serde", "rust_decimal?/serde"]`) gates every floor derive
   (`IpeMaybe`/`IpeResult` in `core.rs`, the `IpeError` family, `IpeOrder`,
   `Decimal`, `dom::diff::Patch`) via `#[cfg_attr]` and the hand-written
   `IpeMaybe` visitor via `#[cfg]`; `dom/form.rs` is gated
   `cfg(any(feature = "web", feature = "wasm-client"))`; `json`, `db`, `web`,
   `email`, `http_client`, `websocket_client`, `redis_store`, and `wasm-client`
   each imply `serde`. Proof: `cargo build -p ipe-runtime-rust
   --no-default-features` resolves **2 crates** (`ipe-runtime-rust` + `libc`),
   down from 14 — every `serde*`/`syn`/`quote`/`proc-macro2`/`itoa`/`ryu`/
   `form_urlencoded` node dropped. All feature builds, both closure SEALs, the
   1413-test golden suite (byte-stable, no re-bless), and the Json / Maybe /
   server behaviour fixtures stay green.

   Not yet done — the emitted-app floor: the SSOT still inserts
   `RuntimeFeature::Json` unconditionally, so a bare app selects `json` and
   therefore still links `serde` + `serde_json`. Demoting it needs a
   `reaches_json` flag (fail-closed: set on any reachable `IrType::Json` type
   mention — a `Value`/`Decoder`/wildcard-`any` — not just a `Json.*` kernel)
   and gating the two unconditional prelude typedefs a non-Json app emits —
   `type Value = JsonVal;` and
   `pub type Decoder<T> = ipe_runtime::json::Decoder<IpeError, T>;` — on it
   (a new `PreludeReach.json` section, mirroring `log`/`time`/`crypto_core`).
   This is the sectioned-prelude prerequisite of §3 step 0 and carries the
   `["json"]`-shrinks-to-`[]` golden re-bless of §6; kept fail-closed (forcing
   `json` off the floor before those typedefs are conditional would be an
   E0433 under-inclusion on every bare app).
8. **Remeasure + record**. Re-run the measured-floor table of
   ADR 0054 (bare / log-only / regex tool / db / web) and
   record the numbers there. Target: bare Program ≤ 3 crates.

Each of 2–7: dep → `optional = true`, `[features]` entry, `mod.rs` cfg gate,
`RuntimeFeature` variant + SSOT insertion + flag threading
(`KernelUsage`/`ir::Module`/`EmitCtx`), per-feature closure proof,
drop-a-feature red test, vendored+wasm template feature declarations, golden
re-bless.

## 8. Risks and cost

- **Serde blast radius** (why it lands last): the derives thread through
  core/error/basics/decimal/html/dom/ui and the emitted Web-Model/form/JSON
  bounds. Mitigations: the boundary argument of §2.3 is enforced by the SEAL
  reference scan, the fixture matrix covers each serializing surface, and
  the change is purely presence/absence — no representation change, so
  behavior parity is trivially preserved where serde is on.
- **Under-inclusion** (the forbidden failure): a kernel→flag arm missed ⇒
  exit-0 emit + E0425/E0433. Guarded by the drop-a-feature proofs, the
  item-gate registry, the subset-differential tests, and the E2E cargo
  fixtures. Over-inclusion is the accepted precision loss.
- **SEAL restructure correctness**: replacing the exhaustive sweep weakens
  nothing only if the monotonicity argument holds — it holds because cargo
  feature resolution is a union-closure and the SSOT only ever inserts;
  encode that as a property test (random flag pairs: features(a ∪ b) =
  features(a) ∪ features(b)) before relying on it.
- **Wasm parity**: the static wasm module set means `wasm-client` implies
  most floor features; a missed implication fails the wasm-floor build in
  the step gate, not in the field. The wasm manifest stays byte-identical
  until wasm trimming is taken up separately.
- **Vendored fallback drift**: the vendored template must gain each feature
  declaration in the same commit as the crate gate or `IPE_RUNTIME_VENDORED=1`
  breaks; the modset SEAL's source walk covers the vendored `mod.rs`, and a
  vendored-emit E2E fixture stays in the step gate.
- **FFI boundary**: unchanged — FFI stays fail-closed and serde-independent;
  no floor type crosses FFI needing serde (§2.3).
- **Golden churn**: wide, shallow, mechanical (§6).
- **Cost**: ~10 new flags/features threaded through `KernelUsage` →
  `ir::Module` → `EmitCtx` → SSOT → SEALs — mechanical but broad; the SEAL
  restructure (step 1) is the only genuinely novel test engineering.
