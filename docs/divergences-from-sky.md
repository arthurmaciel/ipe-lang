# Divergences from Sky

## Framing

ipê is a Rust port of Sky. Sky — the Haskell compiler and its Go and
`feat/runtime-rust` backends — is the parity and capability **reference** ipê
was ported from: for the same well-typed program and the same input, Go /
behavioral parity is ipê's default contract, ideally byte-for-byte. This
document is the durable ledger of the places where ipê nonetheless **differs**
from that reference — some deliberate (recorded as sanctioned divergences with a
neutral rationale), some emergent from the host-language change (Haskell→Rust)
and the type system Rust brings. Every entry states only *what differs* and
*why*. Where ipê matches Sky, that is not a divergence and is omitted. Where ipê
follows a different, more principled target (e.g. Elm-conformance, a lossless
byte model, a closed typed registry), the technical fact is stated neutrally —
as a difference and its reason, never as a criticism of the reference.

The lens throughout is the PRINCIPLES order — **security > correctness >
soundness > efficiency > completeness > readability** — plus the two foundational
rules **parse, don't validate** and **make invalid states unrepresentable**.

Every divergence in §2 is recorded in-repo and non-silent: the oracle framework
(`tools/oracle`) pins each one with an `oracle_divergence = true` marker and a
tagged reason in `tests/golden/<name>/oracle.meta` + `sanctioned.divergence`
(policy: `docs/architecture/divergence-policy.md`). 42 goldens currently carry a
divergence marker.

---

## 2. Behavioral divergences

Tag key (from the divergence policy): **`divergence:`** = Sky's current behavior
differs and ipê follows a different target; **`sanctioned:`** = the reference
succeeds correctly and ipê is deliberately more correct still; **Go-failure** =
the reference cannot build/run the exact shape, so ipê's own output is the
recorded reference.

### B1 — `Math.min` / `Math.max`: Elm polymorphic comparable
- **Differs:** ipê compares `min`/`max` arguments at the argument type (Elm's
  `a -> a -> a` comparable). `Math.min 0.4 1.3 = 0.4`, `Math.max 0.4 1.3 = 1.3`,
  `Math.min "b" "a" = "a"`. The reference routes both arguments through `AsInt`
  before comparing (`Math.min 0.4 1.3 = 0`, and a non-meaningful compare on
  `String`).
- **Go-oracle relationship:** Go succeeds; outputs differ by design.
- **Rationale:** Elm-conformance. (`Math.abs` stays `Int -> Int` and is *not* a
  divergence.)
- **Sanctioned:** yes (`divergence:`). Goldens `m4c_math_{min,max}_{float,string}`.

### B2 — `Bytes` is a distinct `Vec<u8>` primitive
- **Differs:** Sky defines `type alias Bytes = String`; Go's `string` is an
  arbitrary byte sequence so the alias is cost-free there. ipê makes `Bytes` a
  distinct primitive lowering to `Vec<u8>`; `String ↔ Bytes` conversions are
  always explicit (`Bytes.fromString` UTF-8-encodes, `Bytes.toString` UTF-8-
  decodes → `Maybe String`).
- **Go-oracle relationship:** programs using `Sky.Core.Bytes` produce different
  output under the Go oracle.
- **Rationale:** Rust's `String` is UTF-8-constrained; a transparent alias would
  silently corrupt non-UTF-8 binary payloads. A lossless byte buffer makes the
  invalid state (non-UTF-8 in a `String`) unrepresentable.
- **Sanctioned:** yes (`divergence:`). Goldens `m4e_bytes_*`.

### B3 — `Encoding.base64Encode` / `hexEncode` over non-ASCII text
- **Differs:** for the String-as-bytes surface, ipê's runtime uses a Latin-1
  char-as-byte model (one codepoint U+0000..U+00FF → one byte), so
  `hexEncode "café" → "636166e9"`. Go encodes the UTF-8 bytes of the text
  (`→ "636166c3a9"`). **ASCII input is byte-identical.**
- **Go-oracle relationship:** Go succeeds; only codepoints ≥ 0x80 diverge.
- **Rationale:** the binary pipeline (email attachments, compression, WebSocket
  frames, the `base64(hexDecode(hmac))` JWT path) must round-trip bytes ≥ 0x80
  losslessly through a Rust `String`. A tracked follow-up migrates `Encoding.*`
  onto the `Bytes` primitive so the text path can match Go while the binary path
  stays lossless.
- **Sanctioned:** yes (`divergence:`). Golden `m4f_encoding_nonascii_divergence`.

### B4 — `Std.Money.allocate` over a negative total
- **Differs:** ipê distributes the residue toward zero by sign so the shares sum
  to the exact input for negative totals as well as positive. The reference
  clamps the residue at zero for negative totals, so its shares no longer sum
  back to the input. For positive totals the two are byte-identical.
- **Go-oracle relationship:** Go succeeds; negative totals differ.
- **Rationale:** correctness — a fair split must sum to its input
  (`money.rs::money_allocate`, regression
  `test_allocate_negative_total_shares_sum_to_input`).
- **Sanctioned:** yes (`sanctioned:`).

### B5 — Full-Unicode default case mapping
- **Differs:** `String.toUpper`/`toLower`/`casefold` and `Char.toUpper`/`toLower`
  apply full Unicode `SpecialCasing` (`ß → SS`, `İ → i̇`). The reference uses
  simple per-rune mapping.
- **Go-oracle relationship:** Go succeeds; non-ASCII case differs (ASCII case is
  identical).
- **Rationale:** correctness; full-Unicode is free in Rust and matches the
  mainstream (Rust/Python/Swift/Haskell `Text`). Unicode lives permanently in the
  core. (Locale-tailored casing — Turkish ı/İ, Greek final sigma — is explicitly
  out of scope, not a divergence.)
- **Sanctioned:** yes (`sanctioned:`). Char predicates `isDigit`/`isLower`/
  `isUpper`/`isAlpha` match Go and are *not* divergences.

### B6 — `String.toFloat` grammar is stricter
- **Differs:** ipê accepts the standard float grammar and rejects Go's hex-float
  and underscore-separated literals.
- **Go-oracle relationship:** stricter, not looser.
- **Rationale:** parse-don't-validate at the numeric boundary.
- **Sanctioned:** yes (`sanctioned:`, stricter).

### B7 — Bare arity-0 `Uuid.v4` / `Uuid.v7` evaluate
- **Differs:** the import-less bare reference `Uuid.v4` / `Uuid.v7` evaluates to a
  fresh `String` on ipê (the documented bare-reference form). The Go reference
  leaves the bare reference as a kernel function value (CLAUDE.md Limitation #7 —
  arity-0 kernel codegen), so its length/version-nibble checks differ.
- **Go-oracle relationship:** Go succeeds; checks differ on this shape.
- **Rationale:** arity-0 kernel codegen. **Sanctioned:** yes (`sanctioned:`).
  Golden `m5b_uuid_format`.

### B8 — `Uuid.parse` accepts a canonical UUID
- **Differs:** ipê's `Uuid.parse` returns `Just` for a canonical hyphenated UUID
  and `Nothing` for malformed input. The Go reference returns `Nothing` for the
  same canonical UUID on this shape.
- **Go-oracle relationship:** Go succeeds; ipê is semantically correct.
- **Rationale:** correctness. **Sanctioned:** yes (`sanctioned:`). Golden
  `m5b_uuid_parse`.

### B9 — `Sky.Core.Jwt` flat-kernel interim surface
- **Differs:** ipê currently surfaces four flat kernels
  (`encodeHs256`/`decodeHs256`/`encodeRs256`/`decodeRs256`, claims as a JSON
  string). The Go backend exposes only the builder API
  (`Jwt.encode (Jwt.hs256 secret) (claims …)` + `Algorithm`/`Claims` types), so a
  builder-API program does not yet compile on ipê and the flat-kernel program
  does not compile on Go. **The emitted token bytes are byte-identical to Go**
  (same Go-parity primitives; byte equality asserted in
  `golden_m5b_uuid_jwt.rs` and the runtime jwt tests; RS256/PKCS#1 v1.5 is
  deterministic).
- **Go-oracle relationship:** call-surface only; token bytes identical.
- **Rationale:** interim API surface; a tracked follow-up adds the builder API so
  the goldens become shared-source Go-parity goldens.
- **Sanctioned:** yes (`divergence:`). Goldens `m5b_jwt_*`.

### B10 — `Std.Db` emits Rust + `sqlx` (vs Go + SQLite/cgo)
- **Differs:** the full `Std.Db` surface is shared, but Go emits Go+SQLite (cgo)
  binaries while ipê emits Rust+`sqlx`. The in-memory SQLite connection-pool
  behavior and row-type representation differ enough that one `Main.sky` cannot
  run identically on both backends.
- **Go-oracle relationship:** both build; runtime representation differs, so ipê's
  output is the recorded reference.
- **Rationale:** backend runtime substrate. The parameterised-args channel
  (`?`-placeholder binding on `unsafeFindWhere` / `findByConditions`) is exercised
  to prove injection-safe operation on the sole sanctioned raw-SQL path.
- **Sanctioned:** yes (`sanctioned:`). Goldens `m5b_db_*`.

### B11 — `Std.Ui` HTML skeleton
- **Differs:** ipê emits compact inline CSS with no separate `<style>` reset
  block; the Go backend emits a different HTML skeleton (separate CSS reset tag,
  trailing spaces). Both render semantically-correct Flexbox layouts.
- **Go-oracle relationship:** Go succeeds; HTML bytes differ.
- **Rationale:** the two are separate renderers; strict byte-parity for HTML is a
  later goal. **Sanctioned:** yes (`divergence:`). Goldens `m7_stdui*`.

### B12 — `Cmd` / `Sub` are construct-only on ipê
- **Differs:** ipê provides TEA `Cmd`/`Sub` constructors; the Go backend has no
  equivalent constructors, so these goldens record ipê's output as the
  authoritative reference.
- **Go-oracle relationship:** Go has no equivalent surface.
- **Rationale:** TEA-everywhere surface. **Sanctioned:** yes (`sanctioned:`).
  Goldens `m5c_cmd_ctors` / `m5c_sub_ctors` / `m5c_perform_ctor`.

### B13 — Shapes the Go reference cannot build (ipê compiles + runs)
- **Differs:** several well-typed ipê programs are rejected by the Go reference's
  front-end, so ipê's output is recorded as the reference:
  - Recursive enum through a **tuple** payload — `type Chain = ChainEnd | ChainNode (Chain, Int)` (Go parse error; ipê boxes the cyclic edge so the Rust enum stays finite-sized). Golden `m3a_tuple_self_edge`.
  - Recursive enum through a **record** payload — `type RChain = REnd | RNode { rest : RChain, val : Int }` (Go parse error). Golden `m3a_record_self_edge`.
  - `Std.Ui` with `Html.htmlRender` — not exposed by the Go oracle (`sky dev`), which exits 1; ipê compiles and runs. Goldens `m7_stdui_onclick` / `m7_stdui_oninput_closure`.
  - `Set` generic / member on shapes the Go oracle exits 1 on. Goldens `m4d_set_generic` / `m4d_set_member`.
  - Invalid-encoding decode input where the Go oracle exits 1; ipê returns `Err`. Golden `m4f_encoding_invalid`.
- **Go-oracle relationship:** Go-failure (auto kind-1); ipê handles the shape.
- **Rationale:** capability/coverage; ipê's output is correct on these shapes.
- **Sanctioned:** yes (auto Go-failure).

### B14 — Runtime-fork behavioral hardening (vs the reference's Rust runtime)
The 48 `sky_runtime` modules are a vendored fork shared by name with the
reference's `feat/runtime-rust` runtime; the divergence is within-module. ipê is
uniformly at-or-ahead — several behavioral differences vs the reference's Rust
runtime (each either matches Go or is more correct):
- **auth** — ipê fail-closes on an id-column decode error; the reference's Rust
  runtime `unwrap_or(0)` (authenticating as user 0). *Security.*
- **jwt** — ipê rejects `now == exp` (Go parity) and makes `exp`/`nbf` optional;
  the reference's Rust runtime accepts one instant past expiry and rejects
  legitimately exp-less tokens. *Correctness/security.*
- **http/ws/http_stream** — ipê redacts URL userinfo/query in errors, defaults
  SSRF-deny ON in production, and returns `Err` on an invalid HTTP method; the
  reference's Rust runtime echoes the URL, is SSRF-opt-in, and silently downgrades
  an invalid method to GET. *Security/correctness.*
- **decimal/money** — ipê rounds `toStringFixed`/`formatWith` half-away-from-zero
  (Go `StringFixed` parity), caps division at 16 dp (Go `DivisionPrecision = 16`),
  and `saturating_abs` in `allocate`; the reference's Rust runtime banker's-rounds
  `toStringFixed`, leaves division uncapped, and wraps at `i64::MIN`.
- **cache** — ipê uses saturating counters; the reference's Rust runtime uses raw
  `+=`/`-=` (debug overflow panic). *Soundness.*
- **regex split** — ipê drops the trailing zero-width empty (Go `regexp.Split`
  parity); the reference's Rust runtime keeps boundary empties.
- **env** — ipê routes env access through a process-global lock; the reference's
  Rust runtime uses raw `std::env`. *Soundness (env data-race).*
- **telemetry/trace** — ipê strips CRLF from CSP frame-ancestors and scrubs log
  controls + U+2028/9. *Security.*
- **Sanctioned:** these are runtime-fork differences, not source-program
  divergences; they are captured in `docs/architecture/sky-rust-backend-reference-audit.md` §Runtime.

### B15 — Float scientific-notation threshold — **RESOLVED (ipê-correct)**
- **Differs:** ipê's `stringify.rs` switches to scientific notation at exponent ≥ 6
  (`!(-4..6)`); the reference's Rust backend switches at exponent ≥ 21 (`!(-4..21)`).
- **Go-oracle relationship:** RESOLVED by a direct probe of Go 1.26.2 (task #52,
  commit `1903654`): `fmt %v` ≡ `strconv 'g',-1` cuts to scientific notation at
  decimal exponent ≥ 6 (and < −4) for every input — there is no exp-21 behaviour.
  `1000000 → "1e+06"`, `1e15 → "1e+15"`, `999999 → "999999"`. ipê matches Go
  byte-for-byte; the reference's exp ≥ 21 is the value that diverges from Go.
- **Rationale:** Go `%v` parity, now oracle-confirmed and pinned by discriminating
  regression tests (`float_go_v_parity` / `ff_go_g_threshold_is_six_not_twentyone`,
  proven RED under a scratch `!(-4..21)` flip).
- **Sanctioned:** N/A — ipê matches the Go oracle exactly, so this is a difference
  from the *reference's Rust fork* only, not from Go. No sanctioned-divergence marker
  needed.

### Note — Decimal rounding modes are parity, not a divergence
`Decimal.round` uses banker's rounding (Go `RoundBank`) and
`Decimal.toStringFixed`/`formatWith` use half-away-from-zero (Go `StringFixed`);
both match Go exactly and are therefore *not* divergences from Sky. Recorded here
only to pre-empt mis-listing (see CLAUDE.md "Agent learnings").

---

## 3. Architectural divergences (compiler + runtime structure)

These are structural consequences of porting a Haskell compiler that emits
Go/Rust into a Rust compiler that emits Rust. Confirmed against `../sky`
(`feat/runtime-rust`) and the ipê tree.

### A1 — Rust-all-the-way `skyc` vs a Haskell compiler
The reference is a Haskell compiler (`src/Sky/…`) emitting Go and Rust. ipê is a
single Rust pipeline — parse → canon → types → lower → IR → Rust-emit — split
across `crates/sky_canon`, `crates/sky_types`, `crates/sky_lower`,
`crates/sky_ir`, `crates/sky_backend_rust`. Strategies and invariants are ported,
never literal code.

### A2 — Typed IR checkpoint vs AST→string emitters
ipê lowers `canon → typed sky_ir::Expr → Rust` (two stage). The reference walks
`Can.Expr_` → Rust string in one pass
(`src/Sky/Generate/Rust/Builder/ExprEmitter.hs`). *Rationale:* a malformed shape
is unrepresentable in the typed IR rather than caught only at `rustc` —
make-invalid-states-unrepresentable.

### A3 — Typed `TailRecur`/`TailLoop` IR + self-authored Rust `loop` emission
ipê represents tail recursion as typed IR nodes (`Expr::TailRecur` /
`Expr::TailLoop`, `sky_lower/src/lower.rs`) and emits a Rust `loop { … }`. The
reference transports the jump as a stringly kernel-name sentinel
(`tcoMarker = "__tco_jump__"`, `src/Sky/Build/TailCallOpt.hs:140`) and emits TCO
**Go-only** — its Rust backend has no TCO. ipê ports the reference's
backend-agnostic `isTailRecursive` analysis but authors the Rust loop emission
itself. *Rationale:* soundness — constant stack vs an uncatchable stack-overflow
trap; and a typed jump vs a stringly sentinel.

### A4 — Closed typed kernel registry + fail-closed default
ipê dispatches through a closed **424-variant** `KernelFn` enum with a typed
`StdlibKernel` registry (`crates/sky_kernels`), indexed anti-drift from
`StdlibKernel::ALL`; an unknown kernel fails **closed** with `SKY-L0108`. The
reference dispatches `(mod,name)` via a string `case` and falls **open** to a
`toSnakeCase` default (`src/Sky/Generate/Rust/Builder/Kernel.hs:801-802`).
*Rationale:* security → correctness — a fail-open snake_case default is the exact
"skyc exits 0 then the emitted code fails to build" class; the enum *is* the
registry.

### A5 — `render_type : IrType → DResult`, no `"String"` default
ipê's type renderer is a closed function returning `DResult<String>`
(`emit_types.rs:73`) with no catch-all. The reference's `TypeRenderer.hs` falls
back to a `"String"` default on an unmatched type. *Rationale:* soundness floor —
the renderer is total by construction.

### A6 — First-class opaque `IrType` variants vs `{M}`-placeholder strings
ipê models opaque/parametric types as first-class closed `IrType` variants with a
structural `Box<IrType>` message parameter. The reference uses a stringly-keyed
`Map (String,String) String` with `{M}`-placeholder substitution and
re-derivation. *Rationale:* invalid-states.

### A7 — Exact-key record resolution, fail-loud
ipê resolves record aliases by exact sorted-key match and raises a `CompilerBug`
on a miss. The reference widens to the best superset row and falls back to
`"String"` on a miss. *Rationale:* soundness > completeness (a superset fallback
would be added only if a real example trips the guard).

### A8 — Uniform `Box<dyn Fn>` callbacks (reference is more complete here)
ipê renders effectful callbacks uniformly as `Box<dyn Fn>` (with a handler
special-case). The reference uses a 3-way classification (stored
`Arc<dyn Fn>+Send+Sync` / passed `impl Fn` / ADT-embedded bare `fn`). This is one
axis where the reference is more complete; ipê adopts the 3-way split when a
`derive`/`Clone` callback subsystem lands. *Neutral: reference-ahead on
completeness.*

### A9 — Crate-version SSOT as a typed `const` table + drift test
ipê holds crate name+version in a typed `const CrateSpec` table
(`crates/sky_backend_rust/src/crate_specs.rs`) read by every manifest-emitting
function, with a co-located drift test asserting the SSOT ≡ `runtime/Cargo.toml`
(all crates) **and** ≡ the golden base manifest. The reference holds the same SSOT
as an embedded `crate-specs.toml` re-parsed at build
(`src/Sky/Generate/Rust/Builder/crate-specs.toml` + `CrateSpecs.hs`) with a sync
test. *Rationale:* compiler-checked structured data over a string re-parse; ipê's
drift test additionally covers the golden base manifest.

### A10 — Kernel-registry drift tripwires
ipê builds the canon `stdlib_index` anti-drift from `StdlibKernel::ALL`
(`sky_canon/src/env.rs`), so a registered kernel and its call-site resolution
cannot skew silently. *Rationale:* parse-don't-validate at the registry boundary.

### A11 — Runtime as a vendored fork that has since diverged
The 48-module `sky_runtime` is a vendored fork shared by name with the
reference's Rust runtime; ipê's copy is a strict superset — every module is
equal-or-larger with the reference's logic plus the security/correctness/soundness
hardening enumerated in B14. Structurally: runtime divergence is *within-module*,
not a different module layout. *Rationale:* the reference's Rust runtime is not
cargo-culted back in.

### A12 — Fail-closed refutable function-argument patterns
For a refutable function-arg pattern (`f (Just x) = …`) ipê refuses at lower
(SKY-L0115/0116) and closes the gap via a front-end desugar to `case`. The
reference synthesises a `let … else { panic! }` (a reachable `panic!`).
*Rationale:* soundness — "no panic from well-typed Sky" outranks the completeness
the reference gains. (Front-end desugar is the completeness close.)

### A13 — Fail-closed nested-constructor-payload patterns (ipê currently less complete)
A nested list/cons/record inside a constructor payload (`Just (h :: t)`,
`Ok {name}`) is rejected fail-closed in ipê
(`Err(NestedCtorDiscrimination/NestedPayloadPatterns)`); the reference recurses
and compiles it. *Rationale:* soundness over completeness for now; the
completeness gap is a tracked front-end item. *Neutral: reference-ahead on
completeness.*

---

## 4. Stdlib / surface divergences

Surface-shape differences (several overlap the behavioral entries; listed here for
API-shape review):

- **`Bytes` conversion API** — because `Bytes` is a distinct `Vec<u8>` primitive
  (B2), ipê exposes explicit `Bytes.fromString` / `Bytes.toString : Maybe String`
  where Sky's `Bytes = String` alias needs none.
- **`Sky.Core.Jwt` call surface** — flat kernels vs the Go builder API (B9); token
  bytes identical, call surface differs until the builder API lands.
- **`Cmd` / `Sub` constructors** — present on ipê, absent on the Go backend (B12).
- **`Std.Db` substrate** — `sqlx`/Rust vs SQLite/cgo/Go (B10); identical Sky
  surface.
- **Front-end capability gaps (ipê not-yet, reference-ahead)** — neutral coverage
  differences, each a tracked front-end item, none a principle divergence:
  - Bare `.field` accessor-as-function (no canon AST variant yet).
  - Refutable function-argument patterns (A12 — closes via desugar).
  - Nested constructor-payload patterns (A13).
  - Mutual / let-rec tail-call optimization is out of scope for the current TCO
    (self-recursion only).

---

## 5. README-liftable summary table

| Aspect | Sky (reference) | ipê | Why |
|---|---|---|---|
| Compiler host | Haskell, emits Go + Rust | Rust, emits Rust (`skyc`) | Single-language port |
| IR | AST → string emitters | Typed `sky_ir::Expr` (two-stage) | Malformed shapes unrepresentable |
| Tail-call jump | Stringly `__tco_jump__` sentinel; Rust backend has no TCO | Typed `TailRecur`/`TailLoop` IR → Rust `loop` | Constant stack; typed jump |
| Kernel dispatch | `(mod,name)` case; fail-open `toSnakeCase` default | Closed 424-variant `KernelFn`; fail-closed `SKY-L0108` | No exit-0-then-cargo-fail class |
| Type render | `_ -> "String"` fallback | `IrType → DResult`, closed, no default | Total by construction |
| Record alias | superset-widen or `"String"` | exact sorted-key, `CompilerBug` on miss | Soundness > completeness |
| Refutable arg pattern | synthesised `panic!` | fail-closed + desugar to `case` | No panic from well-typed code |
| `Bytes` | `type alias Bytes = String` | distinct `Vec<u8>` primitive | Rust `String` is UTF-8-constrained; lossless bytes |
| `Math.min`/`max` | `AsInt`-coerced compare | Elm polymorphic comparable | Elm-conformance |
| Case mapping | simple per-rune | full-Unicode `SpecialCasing` | Correctness; Unicode in core |
| `Money.allocate` (negative) | residue clamped at zero | residue distributed; shares sum to input | Fair split must sum to input |
| `Uuid.parse` | `Nothing` on canonical UUID (this shape) | `Just` on canonical, `Nothing` on malformed | Correctness |
| JWT | builder API | flat kernels (interim); **token bytes identical** | Interim surface; codec unchanged |
| `Std.Db` | Go + SQLite (cgo) | Rust + `sqlx` | Backend substrate |
| `Std.Ui` HTML | skeleton + `<style>` reset | compact inline CSS, no reset block | Separate renderer; byte-parity later |
| Runtime | shared fork baseline | strict superset (auth/jwt/SSRF/decimal/cache/env/telemetry hardening) | Security/correctness/soundness |
| Float sci-notation | exp ≥ 21 (reference Rust) | exp ≥ 6 (Go `%v` parity) — **confirmed vs Go 1.26.2 (#52)** | ipê matches Go; the reference's Rust fork diverges |

---

## Counts

- **Behavioral divergences:** 15 classes (B1–B15). Sanctioned/recorded: 42 goldens
  carry a marker (`Math` 4, `Bytes` 5, `Encoding` 2, `Jwt` 5, `Db` 11, `Ui` 6,
  `Cmd`/`Sub` 3, `Uuid` 2, plus Go-failure kind-1 shapes and Money/case/toFloat
  sanctioned entries).
- **Architectural divergences:** 13 (A1–A13); A8 and A13 are reference-ahead on
  completeness.
- **Stdlib/surface divergences:** 4 API-shape + 4 front-end capability gaps.

## Could not confirm / verify before README

- ~~**B15 float sci-notation threshold.**~~ RESOLVED (task #52, commit `1903654`):
  probed Go 1.26.2 directly — `%v` cuts to scientific at exp ≥ 6, no exp-21
  behaviour; ipê matches Go byte-for-byte. No longer an open item.
- ~~The reference `TypeRenderer.hs` `"String"` default and `ExprEmitter.hs`
  single-pass shape line-cites.~~ CONFIRMED against the reference tree at
  `src/Sky/Generate/Rust/Builder/`:
  - **A5 — `TypeRenderer.hs` `"String"` fallback:** the catch-all is
    `_ -> "String"` at `TypeRenderer.hs:345` (the closing arm of
    `typeToRustString`); a second `| otherwise -> "String"` sits at line 211.
    Safe to quote verbatim.
  - **A4 — `Kernel.hs` fail-open snake_case default:** the fallthrough is
    `_ -> toSnakeCase (map (\c -> if c == '.' then '_' else c) mod ++ "_" ++ name)`
    at `Kernel.hs:802` (the file's last line), preceded by the `Rust_`-prefix
    arm at 800–801. Matches the A4 `Kernel.hs:801-802` cite.
  - **A2 — `ExprEmitter.hs` single-pass shape:** the emitter walks `Can.Expr`
    to a Rust `String` in one pass (e.g. `argToRustString :: EmitCtx -> Bool ->
    Can.Expr -> String` at `ExprEmitter.hs:793`, one of many `… -> String`
    renderers across the 4324-line module) — no typed-IR checkpoint. Confirms
    the A2 characterisation.
