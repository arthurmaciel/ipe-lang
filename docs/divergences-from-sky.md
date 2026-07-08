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

### B-Lazy — `Std.Ui.Lazy`: no memoisation in v1 (eager evaluation)
- **Differs:** `Lazy.lazy f a` / `lazy2` / `lazy3` / `lazy4` / `lazy5` evaluate
  eagerly in ipê v1 — calling `f(a)` (etc.) directly without caching. Sky's Go
  runtime memoises the subtree using an LRU keyed on the function pointer and
  shallow argument equality (`reflect.DeepEqual`); re-renders with identical
  arguments short-circuit the diff layer by reusing the last `Element` value.
- **Go-oracle relationship:** Output is byte-identical for any *first* render.
  Repeated renders with the same arguments that would be short-circuited by the
  Go LRU *could* differ if `f` is impure (side-effecting view functions are not
  a supported pattern in Sky; memoisation is purely a performance optimisation
  in the reference). In practice, for well-typed Sky code the rendered HTML is
  always the same value regardless of caching, so observable output is
  byte-identical.
- **Rationale:** The TEA diff layer that would make a keyed memoisation cache
  reachable at render time does not exist in the ipê Rust backend yet. The
  `Std.Ui.Lazy` *module and kernels* are registered so `import Std.Ui.Lazy as
  Lazy` compiles and `Lazy.lazy viewItem item` lowers correctly; the caching
  optimisation is a v2 follow-on.
- **Sanctioned:** `sanctioned:` (deliberate deferral; observable semantics
  identical for pure view functions). Reference:
  `runtime/src/sky_runtime/ui/lazy.rs`.

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

### B3 — `Encoding.base64Encode` / `hexEncode` over non-ASCII text — ~~divergence~~ RETIRED (task #55a)
- **Was:** ipê's runtime used a Latin-1 char-as-byte model that silently
  truncated codepoints > 255 (`c as u8`), so `hexEncode "café" → "636166e9"` vs
  Go's UTF-8 `"636166c3a9"`.
- **Now (task #55a):** the `Encoding.*` text codecs encode a `String`'s UTF-8
  bytes — **byte-identical to Go for BOTH ASCII and non-ASCII**. The
  silent-truncation hole (a real security bug: two Basic-auth passwords differing
  only above 0xFF collided) is closed; codepoints > 255 no longer collapse.
  Golden `m4f_encoding_nonascii` now carries `oracle_divergence = false`.
- **Related behavior change (recorded):** `base64Decode` / `hexDecode` now require
  the decoded bytes to be valid UTF-8 and return `Err` otherwise (previously a
  never-erroring lossy Latin-1 reinterpretation). This keeps
  `decode (encode s) == Ok s` for every `String s`; raw-byte round-tripping moved
  to `Std.Bytes` (`Vec<u8>`). No reachable caller depended on the old behavior
  (the ASCII goldens round-trip identically; `jwt.rs` owns its own base64/hex).
- **Deferred (#55b):** the runtime-internal binary pipelines (`compression.rs`,
  `email.rs`, `ws_client.rs`) still use the Latin-1 `sky_bytes`/`bytes_to_sky`
  helpers because they have no Sky-facing module in the skyc port yet; #55b
  migrates them onto `Bytes`(`Vec<u8>`) and deletes the helpers. See
  `docs/architecture/encoding-bytes-migration.md`.

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

### B16 — True last-use clone analysis vs reference's use-count≥2 blanket (#104)
- **Differs:** For a local binding of non-`Copy` type that appears in multiple
  owned-consume positions, the reference clones **all** occurrences — including
  the last one — when the binding is in `ecCloneVars` (the set of locals used
  ≥ 2 times; `ExprEmitter.hs` `collectVarLocalsMulti`, `varLocalRead:781-787`).
  ipê performs true last-use analysis: borrow-position reads (comparison operands,
  `++`, interpolation) are emitted bare; among owned-consume reads the **last** is
  emitted as a move (zero clones), and every earlier one is `x.clone()`. Result:
  N uses → N−1 clones instead of N; borrow positions are excluded from the count.
- **Go-oracle relationship:** Go succeeds; output is identical (clone count is an
  internal concern). The divergence is in emitted-Rust efficiency only.
- **Rationale:** soundness/efficiency — Rust's move semantics let the last use
  move; over-cloning the last use is incorrect by Rust's standards (wastes an
  allocation on a path where the original is never touched again). Strictly better
  than the reference. See `docs/architecture/seal-noncopy-move-design.md §4.1`.
- **Verified:** design doc §4.1; reference `ExprEmitter.hs:294,781-787`.
- **Sanctioned:** yes (`sanctioned:`). Pending fixture goldens for #104.

### B17 — Refutable as-pattern alias bind vs reference's drop-the-alias bug (#99)
- **Differs:** For a match-arm alias pattern `name @ inner` over an owned
  scrutinee, the reference drops the alias name: `patternToMatchString` renders
  `((a,b) as w)` as just `(a, b)` and never binds `w`
  (`ExprEmitter.hs:4206`). A body that uses `w` would fail with E0425 "cannot
  find value `w`" — the alias whole is lost. ipê correctly binds the whole by
  move (`name @ skeleton`) and reconstructs the inner bindings from
  `name.clone()` in the arm prelude, routing through `emit_binding_stmts` for
  nested aliases. The reference's `let-else` + `patternIsIrrefutable` discipline
  (`Pattern.hs:113-171`) is ported for the refutable reconstruction branch.
- **Go-oracle relationship:** Go backend does not generate Rust, so the reference
  pattern is the Haskell-emitting-Rust path. ipê's output is correct on this
  shape; the reference's output is wrong when `name` is used in the arm body.
- **Rationale:** correctness — the reference has a latent bug (alias name dropped
  → E0425 on use). ipê fixes it. See `docs/architecture/seal-noncopy-move-design.md §4.2`.
- **Verified:** design doc §4.2; `ExprEmitter.hs:4206`; existing #96 clone-split
  machinery (`emit_expr.rs:3237`) extended into `emit_arm_head`.
- **Sanctioned:** yes (`sanctioned:` — correctness improvement over a reference
  latent bug). Pending fixture goldens for #99.

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

### B18 — `Ws.sendBinaryToClient` takes `Vec<u8>`, not `String`
- **Differs:** Sky defines `type alias Bytes = String`, so the Go reference's
  `sendBinaryToClient` / `sendBinary` take a `String` (raw bytes in a Go string;
  cost-free alias). ipê's `Bytes` is a distinct `Vec<u8>` primitive (B2), so
  `Ws.sendBinaryToClient` takes `Bytes` (`Vec<u8>`) — no lossy UTF-8 hop.
  Programs that pass binary data through `sendBinaryToClient` work correctly on
  ipê; the same program on the Go reference relies on Go's transparent
  `string` ↔ `[]byte` relationship.
- **Go-oracle relationship:** Go succeeds; binary frames are representationally
  different (`String` vs `Vec<u8>`). For ASCII-range payloads, output is
  identical. For non-UTF-8 binary payloads, ipê is the correct implementation
  (no silent corruption).
- **Rationale:** B2 consequence (`Bytes` = distinct `Vec<u8>` primitive — lossless
  byte model). **Sanctioned:** yes (`divergence:`).

### B19 — WS server `sendToClient` / `broadcast` are bounded fail-fast (D4)
- **Differs:** Go's reference WS server (`runtime-go/rt/server_websocket.go`)
  blocks up to ~30 s on a full write buffer before returning an error. ipê's
  `ws_loop` uses a `tokio::sync::mpsc::channel` of capacity
  `SKY_WS_SEND_BUFFER` (default 256 frames) with `try_send`: when the queue is
  full the send returns `Err` immediately without blocking. Frames from a slow
  or dead consumer are dropped rather than causing handler-task pileup.
- **Go-oracle relationship:** Go succeeds; error timing and behavior on a slow
  peer differ.
- **Rationale:** security/soundness — a blocking send behind one slow peer can
  pile up goroutines/tasks (memory exhaustion), while a bounded fail-fast
  channel keeps back-pressure explicit and configurable. The 256-frame default
  (overridable via `SKY_WS_SEND_BUFFER`) is sufficient for all non-streaming
  uses. If Go's 30 s blocking semantic is required, change 3 lines in the
  adapters to `tx.send_timeout(out, Duration::from_secs(30))`.
  **Sanctioned:** yes (`divergence:`).

### ~~B20 — `ws_loop` does not send Ping heartbeat frames~~ — **CLOSED (#135)**
- ~~**Differs:** Go's reference WS server sends a Ping frame every 30 s with a
  10 s timeout (`runtime-go/rt/server_websocket.go`, `wsDefaultPingInterval
  = 30s`). ipê's `ws_loop` had no Ping `select!` arm — dead peers lingered in
  the registry until TCP gave up.~~
- **RESOLVED (#135):** `ws_loop` now has a third `select!` arm driven by
  `tokio::time::interval(Duration::from_secs(ws_heartbeat_secs()))`.  Default
  interval 30 s (Go parity).  Override via `SKY_WS_HEARTBEAT` (seconds, > 0).
  axum auto-replies to incoming Pong frames.  Confirmed green in
  `ws_adapter_tests` unit-tests.
- **Go-oracle relationship:** parity restored — both send a Ping every 30 s.

### B21 — Unknown type names fail-closed at canon (SKY-N0002) vs deferred ICE (#138)
- **Differs:** Sky's Haskell canonicaliser resolves an unqualified uppercase type
  name by calling `Map.findWithDefault []` on `type_home_map`, silently supplying
  an empty home `[]` for any name absent from the map. An empty home downstream
  in the Go code-gen is a silent runtime error (or, in the Rust backend, a
  `SKY-I0001` ICE via the `ir_type_from_canon` unique-match heuristic). ipê's
  `canonicalise_type` now classifies every unqualified upper-case type name
  explicitly at canon time:
  - **Known builtins** (`RESERVED_BUILTIN_TYPES` + `EXTRA_BUILTIN_TYPE_NAMES`) →
    empty-home sentinel `home = []` as before; the lowerer resolves them via
    explicit named arms.
  - **User-defined / unknown names** → `TypeNotFound` / `SKY-N0002` with a
    did-you-mean suggestion list from `type_home_map` + `ctx.aliases`. The ICE
    path and the unique-match `enum_variants` heuristic in `ir_type_from_canon`
    and `ir_type_from_ty` are removed.
- **Go-oracle relationship:** the Go backend accepts a program where a type name
  is referenced without importing its home module (the empty-home fallback is
  harmless in Go's stringly-typed codegen). ipê rejects such programs with a
  clear user error instead.
- **Rationale:** correctness / robustness — a reference to a type that is not in
  scope is a genuine user error; silently giving it an empty home and deferring
  the failure to codegen (or crashing with an ICE) violates "make invalid states
  unrepresentable". The Sky Haskell compiler's `findWithDefault ""` is a known
  deferred-failure hole; this is the stricter-is-better class.
- **Sanctioned:** yes (`sanctioned:`). Regression gate: `golden_i138_total_resolution`
  (error fixtures `i138_empty_home_bridge` / `i138_optbridge` → must emit
  SKY-N0002 not SKY-I0001; positive control `i138_kernel_implicit_positive` →
  must compile clean).

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

### A14 — Non-HOF `List` ops as iterative kernels vs pure-Sky recursion (efficiency-only, output-identical)
ipê wires the non-HOF `List` combinators
(`append`/`concat`/`take`/`drop`/`zip`/`cons`/`isEmpty`) as **iterative Rust
kernels** (constant native stack), whereas the Go "Sky" backend classifies them
as non-tail-recursive pure-Sky (O(N) call-stack). Output is byte-identical
across all Elm edges (negative/over-length `take`/`drop`, shorter-truncating
`zip`, empty `concat`); ipê additionally has a strictly better stack profile (no
200k+-element stack-depth risk). *Rationale:* `List.*` is anchored to
`VarHome::Kernel` in ipê canonicalisation (task #68), so the kernel path is the
only exit-0-safe wiring — the improved stack behaviour is a free consequence, not
a behavioural change. `concatMap`/`indexedMap` are kernels in both backends.
*Neutral: efficiency-only, output-identical.* See
`docs/architecture/list-ops-lower-wiring.md`.

### A15 — Model/Msg admissibility gates #91/#94/#95 (Rust-static-bounds–forced)
The reference Go backend types Model and Msg through HM (`("Live","app")` record
constraint in `Expression.hs:2674`) but performs **no static admissibility
check**: Go's runtime reflects/gob-encodes Model dynamically and tolerates
functions, `Html`, `Cmd`, and `Task` values in Model or Msg at compile time (they
fail to round-trip or are carried as `any`, but the compiler accepts them
silently).

ipê's Rust runtime imposes **static trait bounds** on both type parameters:
`live_app<Model, Msg>` requires
`Model: serde::Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync`
and `Msg: Clone + Send + Sync + Debug` (`runtime/src/sky_runtime/live/mod.rs`).
A Model or Msg that carries a function, `Cmd`, `Sub`, `Task`, or `Decoder` does
not satisfy these bounds, so the emitted Rust fails `cargo build`. Because
`skyc` exit-0 MUST imply `cargo` exit-0 (the seal), the backend adds explicit
admissibility gates:

- **#91 (shipped):** `check_admissible_model` in `emit_model_gate.rs:62` — gates
  Model at `skyc`, emits `SKY-L0120` on a non-serde/non-Clone leaf. Verified:
  `code.rs:198-200`, `emit_model_gate.rs`.
- **#94 (designed, not yet in code.rs):** `check_admissible_msg` — gates Msg
  at `skyc` using `ir_type_is_derivable` for all three app shapes (NOT serde —
  Html is derivable and thus admissible as a Live Msg payload, unlike Live Model).
  Emits the planned `SKY-L0121`. Designed in
  `docs/architecture/seal-gates-msg-lambda-view-design.md §2`.
- **#95 (designed, not yet committed):** Lambda-aware `fn_param_ty(e, idx)` in
  `emit_model_gate.rs:38` — closes the fail-open gap where `view = \m -> …`
  (an `Expr::Lambda`) bypassed the `FuncValue`-only model recovery and silently
  skipped the gate. Designed in §3 of the same doc.

*Rationale:* seal-forced divergence. The Go backend's dynamic path is correct for
Go; the Rust backend's static bounds make the Go-dynamic path a `cargo`-fail.
Gates at `skyc` convert the `cargo`-fail class into a clear user diagnostic.
See `docs/architecture/seal-gates-msg-lambda-view-design.md §4`.
*Note:* `SKY-L0121` (InadmissibleAppMsg) is **designed but not yet in
`code.rs`** — mark as pending-implementation.

### A16 — App cfg must be an inline record literal (SKY-L0119)
The reference Go backend accepts any expression as the `Live.app` / `Tui.app` /
`Webview.app` cfg argument, including a let-bound variable
(`let cfg = { … } in Live.app cfg`). ipê's backend reads the cfg's fields
(`init`, `update`, `view`, `subscriptions`, …) directly from the structural
record at the call site; a non-literal argument (a `Var`, a pipe result, a
function call) cannot be field-indexed at lower time, so it is rejected with
`SKY-L0119` ("app entry cfg must be an inline record literal").

*Rationale:* Rust-lowering constraint — the backend must structurally decompose
the cfg record at lower time to emit the correct `live_app` call; a variable
reference loses the field structure. The reference's Go backend reconstructs the
cfg at runtime via reflection; ipê does not have that escape hatch.
*Verified:* `code.rs:196`, `explain/SKY-L0119.md`, `emit_live.rs` (lookup_field).
*Note:* let-bound-cfg support (`[feature: let-bound-app-cfg]` in the explain
page) is a tracked future item — not a permanent limitation.

### A17 — `Float` rejected as `Set` element or `Dict` key (SKY-L0117)
Sky's type system treats `Float` as a `comparable` value, so the Sky type checker
accepts `Set Float` / `Dict Float v` — the reference Go backend uses `interface{}`
comparison and tolerates these at runtime. ipê backs `Set a` with
`BTreeSet<a>` and `Dict k v` with `HashMap<k, v>`; Rust's `f64` implements
neither `Ord` (NaN has no place in a total order) nor `Hash`/`Eq` (NaN != NaN).
Emitting `BTreeSet<f64>` / `HashMap<f64, _>` would produce Rust that does not
compile, so the case is rejected at lower with `SKY-L0117`.

*Rationale:* Rust-substrate constraint, permanent. The NaN/ordering issue is a
semantic property of IEEE 754 floating point that does not arise in Go's
`interface{}`-keyed maps. The diagnostic is deliberate and named a divergence from
Sky in its own explain page. *Verified:* `code.rs:192-194`,
`explain/SKY-L0117.md`. Total-order `Float` set/dict (e.g. via an
ordered-float wrapper) is a tracked future enhancement.

### A18 — `WsServerCfg` phantom `msg` type var dropped (D2)
Sky's `Sky.Http.Server.WebSocket` stdlib source declares
`WebSocketServerCfg msg` with a phantom `msg` type variable reserved for
hypothetical future `Sub` integration (the phantom never reaches the runtime).
ipê types the cfg as a **nullary** opaque constructor: `IrType::WebSocketServerCfg`
renders `WsServerCfg<SkyError>` directly, with `E = SkyError = String` pinned at
the emit site. The runtime struct `WsServerCfg<E>` remains generic over `E`;
ipê merely instantiates it monomorphically.

Effect: a type annotation `Ws.WebSocketServerCfg Msg` compiles on the reference
(phantom var accepted) but fails arity on ipê (`WebSocketServerCfg` is declared
with 0 type args). Example 33 and all known callers never annotate the cfg type
directly, so this is annotation-only in practice.

*Rationale:* the phantom `msg` var is an artefact of the upstream Go TEA
architecture where `WebSocketServerCfg msg` was future-proofed for a Sub-based
WS subscription tier. ipê's kernel-only module has no Sub-tier for the server-side
WS surface; a phantom var would widen the type to parametric with nothing to
unify against (a soundness hazard). Dropping it matches `IrType::Db`,
`IrType::StreamWriter`, and the other nullary opaque handles.
*Verified:* `crates/sky_ir/src/ir.rs` (`WebSocketServerCfg` variant, no type params);
`crates/sky_backend_rust/src/emit_types.rs` (`WsServerCfg<SkyError>` render).
*Sanctioned:* yes (`divergence:`).

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
- **Numeric literals in `{{...}}` interpolation** — ipê's interpolation
  mini-parser (`resolve_simple_interp_ref`) recognises an integer/float literal
  argument, e.g. `{{String.fromInt 54}}` lowers to `String.fromInt 54` and
  prints `54`. Sky's `resolveInterpolationRef`
  (`Sky/Canonicalise/Expression.hs`) has no literal case: a digit-leading body
  becomes `Can.VarLocal "54"`, which surfaces downstream as a naming error (the
  interpolation grammar there is names-only). ipê's `constrain` treats an
  unresolved local as a violated invariant (SKY-I0001 ICE), so without this the
  same program ICE'd rather than compiling. A Sky identifier can never start
  with a digit, so recognising the literal is unambiguous and strictly better
  (a well-typed program compiles instead of failing). **Sanctioned:** yes.
  Reference: `resolve.rs::resolve_simple_interp_ref`; regression
  `crates/skyc/tests/interp_literal.rs` + golden `m_interp_int_literal`.
  Found by the no-panic fuzzer (`multilineinterp` template).

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
| Clone strategy (non-`Copy` bindings) | use-count ≥ 2 → clone ALL reads (including last) | true last-use: clone all-but-last owned reads, last moves; borrow reads exempt | Rust move semantics; N−1 clones vs N |
| As-pattern alias in match arm | drops alias name → E0425 on use (latent bug) | binds whole by move, reconstructs inner from clone | Correctness; reference latent bug fixed |
| Model admissibility | dynamic (Go reflects at runtime; no compile-time gate) | static `SKY-L0120` gate at `skyc` | Rust static trait bounds (seal) |
| Msg admissibility | dynamic (no compile-time gate) | static `SKY-L0121` gate (designed; pending impl) | Rust static trait bounds (seal) |
| App cfg argument | any expression (let-bound variable OK) | must be an inline record literal (`SKY-L0119`) | Backend reads fields at lower time; no runtime reflection |
| `Float` as `Set`/`Dict` key | accepted (Go `interface{}` comparison) | rejected `SKY-L0117` | `f64` lacks `Ord`/`Hash` in Rust |
| WS `sendBinaryToClient` arg type | `String` (Bytes alias) | `Vec<u8>` (distinct `Bytes` primitive) | B2 consequence; lossless binary frames |
| WS send semantics | blocks ~30 s on full write buffer | bounded `try_send`; `Err` on full queue (B19) | Bounded fail-fast; no handler-task pileup |
| WS Ping heartbeat | 30 s Ping + 10 s timeout | 30 s Ping (B20 closed #135); `SKY_WS_HEARTBEAT` override | Parity restored; axum auto-replies Pong |
| `WsServerCfg` type params | `WebSocketServerCfg msg` (phantom var) | nullary opaque — `WsServerCfg<SkyError>` (A18) | Sub-tier phantom not needed; nullary is sounder |

---

## Counts

- **Behavioral divergences:** 21 classes (B1–B21). B16 (#104 true last-use) and
  B17 (#99 alias bind) are pending fixture goldens. B3 RETIRED (task #55a) per
  inline note. B18–B20 are WS-server entries added with task #127. B20 CLOSED
  (#135) — Ping heartbeat ported. B21 is the #138 total-resolution gate
  (unknown-type → SKY-N0002 not ICE). Sanctioned/recorded goldens: 42 carry a
  marker (`Math` 4, `Bytes` 5, `Encoding` 1, `Jwt` 5, `Db` 11, `Ui` 6,
  `Cmd`/`Sub` 3, `Uuid` 2, plus Go-failure kind-1 shapes and Money/case/toFloat
  sanctioned entries). B16/B17 goldens pending.
- **Architectural divergences:** 18 (A1–A18). A8 and A13 are reference-ahead on
  completeness. A15–A17 are seal-gate entries. A18 is the WS phantom-`msg`
  type-var entry added with task #127.
- **Stdlib/surface divergences:** 4 API-shape + 4 front-end capability gaps +
  2 new gate-forced surface constraints (SKY-L0119, SKY-L0117).

## Could not confirm / verify

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

- **SKY-L0121 (InadmissibleAppMsg) — PENDING IMPLEMENTATION.** The Msg-gate
  design is complete (`docs/architecture/seal-gates-msg-lambda-view-design.md §2`)
  and the diagnostic code constant is reserved in the design, but as of this pass
  `SKY_L0121` does not yet appear in `crates/sky_diagnostics/src/code.rs` (the
  file has 79 taxonomy codes; `SKY-L0121` is not among them). A15 captures the
  designed behaviour; mark as asserted-pending-impl until the code lands.

- **"Nominal (home, name) identity types #100" — NOT VERIFIED AS SKY DIVERGENCE.**
  Session memory referenced this as a divergence, but in-repo search finds no
  file or commit that records it as a Sky-specific divergence. The principled-
  decisions-audit (#12) confirms ipê already keys on `(home, name)` canonical
  naming — following `elm/compiler` — and the audit verdict is REJECT (already
  better). Sky's own Haskell compiler also uses name-qualified lookup internally,
  so this appears to be a PORT (convergence with elm/compiler), not a divergence
  from Sky. If a specific Sky-runtime divergence exists under this label, re-file
  with a concrete file:line cite.

- **Live.route non-String payload (#106) — PORT, not a divergence.**
  `routed-live-app-design.md` classifies `Live.route : String -> page ->
  LiveRoute` typing (#106) as "✅ done (Port — matches `../sky`)." The latent
  E0308 for non-String page constructor payloads (`emit_live.rs:135`) is a
  known bug to fix, not a sanctioned divergence. Not added to the ledger.

### B-route-param — Routed page-constructor payload typing (#108)

- **Differs:** `emit_live_call::LiveRoute`'s partial-ctor branch emits a
  type-directed `params.get(i)` conversion expression per constructor payload
  field. Sky's Haskell reference (`ExprEmitter.hs:1823`) and the Go backend
  both unconditionally emit `params.get(i).cloned().unwrap_or_default()` (a
  `String`) for every payload slot, relying on reflect-coercion at the Go
  runtime to coerce the captured string to the expected field type.
- **Go-oracle relationship:** for `String`-payload constructors the output is
  byte-identical. For `Int`/`Float`/`Bool` payloads the Go oracle coerces at
  runtime (opaque to the type checker); ipê emits explicit `.parse::<i64>()`/
  `.parse::<f64>()`/`s == "true"` expressions that decode at the call site. For
  payloads of any other type, the reference emits a String and relies on the
  Go runtime to coerce; ipê rejects at compile time with a `Diagnostic::CompilerBug`
  (to be upgraded to the reserved diagnostic code `SKY-L0123` in a follow-up
  task — NOT `SKY-L0121`, which is owned by the #94 `InadmissibleAppMsg` gate;
  see `docs/architecture/design-coherence-review.md` §C1). The same follow-up
  covers the sibling fail-closed arm added in #108 round 4: a route page
  builder that is neither a page constructor, an inline lambda, nor a named
  function (a let-bound or computed builder value) is rejected at emit with
  the same interim `CompilerBug` shape — pre-round-4 that arm silently
  emitted an untyped `(builder)(params)` call that cargo-failed
  (E0308/E0618) for every realistic shape.
- **Rationale:** parse, don't validate — a `:param` segment is inherently a URL
  string; feeding it to a constructor payload without an explicit decode is a
  type contract violation. Catching it at emit time gives the user a Sky error
  instead of an opaque downstream `rustc` E0308 or a runtime coercion panic.
  The `unwrap_or_default` fallback on parse failure keeps the "never panic"
  spirit of the reference's `unwrap_or_default` for missing captures.
- **Sanctioned:** yes (`sanctioned:`). Reference: `emit_live.rs::route_param_get`.
  Residual: malformed numeric captures silently degrade to `0`/`0.0`/`false`
  (same as reference's missing-capture behavior); routing to `not_found` on
  bad parse is a future refinement (not yet designed in the reference either).


### B-AnyCtorPayload — `any` ctor payload field → `Dict String String` (pub/sub wire carrier)
- **Differs:** Sky's `any` wildcard as a union-constructor payload field (e.g.
  `| MessageReceived any`, `| CartTopicReceived any`) is carried at the Go
  runtime as a dynamic `interface{}` value — universally polymorphic, no static
  type constraint. The Rust backend cannot emit a `dyn Any` field (banned by the
  concrete-over-generic contract: no `dyn Any`/`.downcast`/type-erasure) and
  cannot emit an unconstrained Rust generic (the union's `Clone + Debug +
  PartialEq + Serialize + DeserializeOwned` derives must hold for every field).
  ipê pins the `any` wildcard to `Dict String String` (`HashMap<String, String>`
  in the emitted Rust) — the sole concrete carrier that satisfies all derives and
  the `Broker` type parameter.  The pub/sub broker is typed per concrete payload
  (see A18-adjacent: `Broker<HashMap<String,String>>` for `any`-ctor programs);
  publisher and subscriber must agree on the concrete type at compile time.
- **Go-oracle relationship:** Go succeeds and carries the payload as `any` /
  `interface{}`; ipê carries it as `Dict String String`.  For real-world pub/sub
  programs (examples 27 and 37) the publisher encodes a record into
  `payloadDict : Dict String String` and the subscriber decodes with
  `Db.getString`; the round-trip is semantically equivalent.  Programs that
  use the payload directly as a non-Dict type (e.g. passing it to
  `String.length`) are now rejected at type-check with SKY-T0001 — a
  correctness gain over Go's silent runtime failure.
- **Rationale:** concrete-over-generic contract + `Clone/Debug/PartialEq` seal.
  The `any` wildcard has exactly one concrete lowering in pub/sub payload
  position; `Dict String String` is that carrier.  A Rust generic would need the
  publisher and subscriber to agree on `TypeId` at runtime (silent non-delivery
  risk); `dyn Any` is a hard ban.
- **Sanctioned:** yes (`divergence:`). Reference: `constrain.rs::pin_any_in_ty`,
  `lower.rs::lower_enum` Gate 1, `lower.rs::ir_type_from_canon` Var arm.
  Regression: `golden_l0102_any_ctor_payload`.

### B-ErrorToString — `errorToString : Stringify a => a -> String` (bounded polymorphic vs. universal)
- **Differs:** Sky's Go runtime implements `errorToString` as `fmt.Sprintf("%v", v)`,
  which accepts any value at runtime (universally polymorphic, no type-level bound).
  ipê types `errorToString` as `Stringify a => a -> String`, routing all
  stringification through the single `SkyStringify` trait chokepoint — the same
  chokepoint used by `Basics.toString`.
- **Go-oracle relationship:** Output is identical for all scalar, record, and ADT
  values. A future `Secret` newtype that omits `SkyStringify` would fail closed
  at type-check in ipê but would be silently rendered by Go's `fmt.Sprintf`.
- **Rationale:** The bounded form is strictly sounder for typed-secrets safety.
  A type withheld from the `SkyStringify` impl set (e.g. a future opaque `Secret`)
  fails closed at type-check rather than reaching the runtime `fmt` fallback.
  This is a deliberate divergence in the direction of greater security.
- **Sanctioned:** yes (`sanctioned:`). Reference: `basics.rs::basics_error_to_string`.


### B-JwtDecode — `Jwt.decode` now matches reference: `Algorithm -> Int -> String -> Result Error String`
- **Converged:** The port's `Jwt.decode` previously diverged from the reference by
  dropping the `now : Int` parameter and delegating expiry validation to
  `jsonwebtoken`'s wall-clock `SystemTime::now()`. The reference
  (`sky-stdlib/Sky/Core/Jwt.sky`) declares `decode : Algorithm -> Int -> String ->
  Result Error String`, where `now` is a caller-supplied Unix-epoch second and
  validation is deterministic. The port now matches this signature exactly.
- **Go-oracle relationship:** Semantics are reference-exact: `now >= exp` → expired,
  `now < nbf` → not yet valid, absent claims → accept. The `Algorithm` descriptor
  opaque type and token byte format are unchanged; HMAC/RSA signature verification
  remains constant-time.
- **Rationale:** The old 2-arg form was a security defect (auth-bypass class):
  an expired token could be accepted or rejected depending on when the process
  called the function, rather than on the caller's explicit, auditable `now` value.
  The 3-arg reference form is deterministic, testable, and correct.
- **Interim flat kernels:** `Jwt.encodeHs256` / `Jwt.decodeHs256` / `Jwt.encodeRs256` /
  `Jwt.decodeRs256` remain a Rust-backend M5b wall-clock interim surface
  (separate item, pre-existing, unrelated to this fix).
- **Reference:** `crates/sky_types/src/constrain.rs::K::JwtDecode`,
  `crates/sky_kernels/src/lib.rs::JwtDecode`, `crates/sky_lower/src/lower.rs`,
  `runtime/src/sky_runtime/jwt.rs::sky_jwt_decode`. Regression:
  `tests/golden/m_jwt_decode_now/`.


### B-GridTracksRaw — `Ui.gridTracksRaw` native kernel vs reference sentinel `AttrStyle "__gridTracks"`
- **Reference:** `../sky/sky-stdlib/Std/Ui/Grid.sky` implements `tracks`/`columns`/`rows`
  as `AttrStyle "__gridTracks" (cols ++ "|" ++ rows)` — a pure-Sky sentinel consumed
  by the reference renderer's `findGridTemplate` (Ui.sky:2539) before raw-style emission.
- **Port:** Uses a native `Ui.gridTracksRaw : String -> String -> Attribute msg` kernel
  (`KernelFn::UiGridTracksRaw`) that constructs a typed `Attribute::AttrGridTracks(cols, rows)`
  variant. The web renderer emits `grid-template-columns:{cols}` / `grid-template-rows:{rows}`
  directly; the TUI layout parser reads `AttrGridTracks` directly (no `split('|')`).
- **Why strictly better:** The Rust web renderer's `SafeCssPropertyName` gate rejects
  underscore keys (`[A-Za-z0-9-]` charset) BY DESIGN — the sentinel `__gridTracks` was
  silently dropped, so grid-template CSS never reached web HTML. The typed carrier bypasses
  the property-name gate entirely (fixed literal property names; values still gated via
  `SafeCssValue`). Growing the sentinel allowlist would be the wrong fix.
- **Sanctioned:** yes. Reference: `crates/sky_kernels/src/lib.rs::UiGridTracksRaw`,
  `runtime/src/sky_runtime/ui/element.rs::AttrGridTracks`,
  `runtime/src/sky_runtime/ui/render.rs`. Regression: `crates/skyc/tests/golden_stdui_grid_seal.rs`.
