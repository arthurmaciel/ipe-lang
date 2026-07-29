# Divergences from Ipê

## Framing

Ipê's runtime is a Rust port of Sky's. Sky is the parity and capability **reference**
Ipê was ported from: for the same well-typed program and the same input, Go /
behavioral parity is Ipê's default contract, ideally byte-for-byte. This
document is the durable ledger of the places where Ipê nonetheless **differs**
from that reference — some deliberate (recorded as sanctioned divergences with a
neutral rationale), some emergent from the host-language change (Haskell→Rust)
and the type system Rust brings. Every entry states only *what differs* and
*why*. Where Ipê matches Sky, that is not a divergence and is omitted. Where Ipê
follows a different target (e.g. Elm-conformance, a lossless
byte model, a closed typed registry), the technical fact is stated 
as a difference and its reason.

The lens throughout is the PRINCIPLES.md content.

This document is also the ledger of **planned** divergences: §6 files the
intentional future departures from the reference language (accepted or
designed, not yet implemented), so shipped and planned divergences live in
one place.

Every divergence in §2 is recorded in-repo and non-silent: the oracle framework
(`tools/oracle`) pins each one with an `oracle_divergence = true` marker and a
tagged reason in `tests/golden/<name>/oracle.meta` + `sanctioned.divergence`
(policy: `docs/architecture/divergence-policy.md`).

---

## 2. Behavioral divergences

Tag key (from the divergence policy): **`divergence:`** = Ipê's current behavior
differs and it follows a different target; **`sanctioned:`** = the reference
succeeds correctly and Ipê is deliberately more correct still; **Go-failure** =
the reference cannot build/run the exact shape, so Ipê's own output is the
recorded reference.

### B-Lazy — `Ipe.Ui.Lazy`: no memoisation in v1 (eager evaluation)
- **Differs:** `Lazy.lazy f a` / `lazy2` / `lazy3` / `lazy4` / `lazy5` evaluate
  eagerly in Ipê v1 — calling `f(a)` (etc.) directly without caching. Ipê's Go
  runtime memoises the subtree using an LRU keyed on the function pointer and
  shallow argument equality (`reflect.DeepEqual`); re-renders with identical
  arguments short-circuit the diff layer by reusing the last `Element` value.
- **Go-oracle relationship:** Output is byte-identical for any *first* render.
  Repeated renders with the same arguments that would be short-circuited by the
  Go LRU *could* differ if `f` is impure (side-effecting view functions are not
  a supported pattern in Ipê; memoisation is purely a performance optimisation
  in the reference). In practice, for well-typed Ipê code the rendered HTML is
  always the same value regardless of caching, so observable output is
  byte-identical.
- **Rationale:** The TEA diff layer that would make a keyed memoisation cache
  reachable at render time does not exist in the Ipê Rust backend yet. The
  `Ipe.Ui.Lazy` *module and kernels* are registered so `import Ipe.Ui.Lazy as
  Lazy` compiles and `Lazy.lazy viewItem item` lowers correctly; the caching
  optimisation is a v2 follow-on.
- **Sanctioned:** `sanctioned:` (deliberate deferral; observable semantics
  identical for pure view functions). Reference:
  `runtime/src/sky_runtime/ui/lazy.rs`.

### B1 — `Math.min` / `Math.max`: Elm polymorphic comparable
- **Differs:** Ipê compares `min`/`max` arguments at the argument type (Elm's
  `a -> a -> a` comparable). `Math.min 0.4 1.3 = 0.4`, `Math.max 0.4 1.3 = 1.3`,
  `Math.min "b" "a" = "a"`. The reference routes both arguments through `AsInt`
  before comparing (`Math.min 0.4 1.3 = 0`, and a non-meaningful compare on
  `String`).
- **Go-oracle relationship:** Go succeeds; outputs differ by design.
- **Rationale:** Elm-conformance. (`Math.abs` stays `Int -> Int` and is *not* a
  divergence.)
- **Sanctioned:** yes (`divergence:`). Goldens `m4c_math_{min,max}_{float,string}`.

### B2 — `Bytes` is a distinct `Vec<u8>` primitive
- **Differs:** Ipê defines `type alias Bytes = String`; Go's `string` is an
  arbitrary byte sequence so the alias is cost-free there. Ipê makes `Bytes` a
  distinct primitive lowering to `Vec<u8>`; `String ↔ Bytes` conversions are
  always explicit (`Bytes.fromString` UTF-8-encodes, `Bytes.toString` UTF-8-
  decodes → `Maybe String`).
- **Go-oracle relationship:** programs using `Ipe.Bytes` produce different
  output under the Go oracle.
- **Rationale:** Rust's `String` is UTF-8-constrained; a transparent alias would
  silently corrupt non-UTF-8 binary payloads. A lossless byte buffer makes the
  invalid state (non-UTF-8 in a `String`) unrepresentable.
- **Sanctioned:** yes (`divergence:`). Goldens `m4e_bytes_*`.
- **Downstream:** `Ipe.Compression` (`gzip`/`gunzip`/`zstdCompress`/
  `zstdDecompress`) takes + returns `Bytes` rather than the Go surface's
  `String`, to line up with the Rust runtime `compression_*(Vec<u8>) ->
  SkyTask<_, Vec<u8>>`. Same B2 rationale (a compressed payload is arbitrary
  binary, not UTF-8). Seal probe `compression_builds_and_runs`.

### B3 — `Encoding.base64Encode` / `hexEncode` over non-ASCII text — ~~divergence~~ RETIRED (task #55a)
- **Was:** Ipê's runtime used a Latin-1 char-as-byte model that silently
  truncated codepoints > 255 (`c as u8`), so `hexEncode "café" → "636166e9"` vs
  Go's UTF-8 `"636166c3a9"`.
- **Now (task #55a):** the `Encoding.*` text codecs encode a `String`'s UTF-8
  bytes — **byte-identical to Go for BOTH ASCII and non-ASCII**. The
  silent-truncation hole (a real security bug: two Basic-auth passwords differing
  only above 0xFF collided) is closed; codepoints > 255 no longer collapse.
  Golden `encoding_nonascii` now carries `oracle_divergence = false`.
- **Related behavior change (recorded):** `base64Decode` / `hexDecode` now require
  the decoded bytes to be valid UTF-8 and return `Err` otherwise (previously a
  never-erroring lossy Latin-1 reinterpretation). This keeps
  `decode (encode s) == Ok s` for every `String s`; raw-byte round-tripping moved
  to `Ipe.Bytes` (`Vec<u8>`). No reachable caller depended on the old behavior
  (the ASCII goldens round-trip identically; `jwt.rs` owns its own base64/hex).
- **Deferred (#55b):** the runtime-internal binary pipelines (`compression.rs`,
  `email.rs`, `ws_client.rs`) still use the Latin-1 `sky_bytes`/`bytes_to_sky`
  helpers because they have no Ipê-facing module in the ipe port yet; a
  follow-up migrates them onto `Bytes`(`Vec<u8>`) and deletes the helpers
  (tracked as a GitHub issue).

### B4 — `Ipe.Money.allocate` over a negative total
- **Differs:** Ipê distributes the residue toward zero by sign so the shares sum
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
- **Differs:** Ipê accepts the standard float grammar and rejects Go's hex-float
  and underscore-separated literals.
- **Go-oracle relationship:** stricter, not looser.
- **Rationale:** parse-don't-validate at the numeric boundary.
- **Sanctioned:** yes (`sanctioned:`, stricter).

### B7 — Bare arity-0 `Uuid.v4` / `Uuid.v7` evaluate
- **Differs:** the import-less bare reference `Uuid.v4` / `Uuid.v7` evaluates to a
  fresh `String` on Ipê (the documented bare-reference form). The Go reference
  leaves the bare reference as a kernel function value (AGENTS.md Limitation #7 —
  arity-0 kernel codegen), so its length/version-nibble checks differ.
- **Go-oracle relationship:** Go succeeds; checks differ on this shape.
- **Rationale:** arity-0 kernel codegen. **Sanctioned:** yes (`sanctioned:`).
  Golden `uuid_format`.

### B8 — `Uuid.parse` accepts a canonical UUID
- **Differs:** Ipê's `Uuid.parse` returns `Just` for a canonical hyphenated UUID
  and `Nothing` for malformed input. The Go reference returns `Nothing` for the
  same canonical UUID on this shape.
- **Go-oracle relationship:** Go succeeds; Ipê is semantically correct.
- **Rationale:** correctness. **Sanctioned:** yes (`sanctioned:`). Golden
  `uuid_parse`.

### B9 — `Ipe.Jwt` flat + builder surfaces (✅ both shipped, corrected 2026-07-09)
- **No longer differs — closed.** Ipê surfaces BOTH the four flat kernels
  (`encodeHs256`/`decodeHs256`/`encodeRs256`/`decodeRs256`, claims as a JSON
  string) AND the full builder API (`Jwt.encode`/`Jwt.hs256`/`Jwt.rs256`/
  `Jwt.claims`/`subject`/`issuer`/`audience`/`expiresAt`/`notBefore`/
  `issuedAt`/`jwtId`/`withClaim`/`decode`, `Algorithm`/`Claims` types — D-00,
  #152), wired end to end (kernel registry, type schemes, lowering, runtime
  `jwt.rs`). **The emitted token bytes are byte-identical to Go** (same
  Go-parity primitives; byte equality asserted in `golden_m5b_uuid_jwt.rs`'s
  `jwt_decode_now` test, which exercises the builder syntax end to end;
  RS256/PKCS#1 v1.5 is deterministic).
- **`withClaim` value type converged (#217).** The builder API originally
  authored `Jwt.withClaim : String -> String -> Claims -> Claims`, an
  UNSANCTIONED narrowing of the reference `withClaim : String -> JsonEnc.Value
  -> Claims -> Claims` (`Ipê/Core/Jwt.ipe:79`). This both rejected valid
  reference programs (`Jwt.withClaim "email" (JsonEnc.string e)` → IPE-T0001)
  and lost expressiveness (an `Int`/`Bool`/nested-object claim was
  inexpressible, and even the string case stored the value as a JSON *string* —
  wrong token bytes). Now converged: the scheme, kernel signature
  (`sky_jwt_with_claim(key, value: JsonValue, claims)`), and runtime insert all
  take the encoded JSON value directly (`Value` and `Claims` are both
  `serde_json::Value`). Regression: `golden_i217_stdlib_contract_drift.rs`.
- **Go-oracle relationship:** byte-identical, both call surfaces.
- **Sanctioned:** yes (`divergence:` — offering both surfaces is strictly
  additive over Go). Goldens `m5b_jwt_*`, `m_jwt_decode_now`,
  `jwt_withclaim_value`.

### B10 — `Ipe.Db` emits Rust + `sqlx` (vs Go + SQLite/cgo)
- **Differs:** the full `Ipe.Db` surface is shared, but Go emits Go+SQLite (cgo)
  binaries while Ipê emits Rust+`sqlx`. The in-memory SQLite connection-pool
  behavior and row-type representation differ enough that one `Main.ipe` cannot
  run identically on both backends.
- **Go-oracle relationship:** both build; runtime representation differs, so Ipê's
  output is the recorded reference.
- **Rationale:** backend runtime substrate. The parameterised-args channel
  (`?`-placeholder binding on `unsafeFindWhere` / `findByConditions`) is exercised
  to prove injection-safe operation on the sole sanctioned raw-SQL path.
- **Sanctioned:** yes (`sanctioned:`). Goldens `m5b_db_*`.

### B11 — `Ipe.Ui` HTML skeleton
- **Differs:** Ipê emits compact inline CSS with no separate `<style>` reset
  block; the Go backend emits a different HTML skeleton (separate CSS reset tag,
  trailing spaces). Both render semantically-correct Flexbox layouts.
- **Go-oracle relationship:** Go succeeds; HTML bytes differ.
- **Rationale:** the two are separate renderers; strict byte-parity for HTML is a
  later goal. **Sanctioned:** yes (`divergence:`). Goldens `stdui*`.

### B12 — `Cmd` / `Sub` are construct-only on Ipê
- **Differs:** Ipê provides TEA `Cmd`/`Sub` constructors; the Go backend has no
  equivalent constructors, so these goldens record Ipê's output as the
  authoritative reference.
- **Go-oracle relationship:** Go has no equivalent surface.
- **Rationale:** TEA-everywhere surface. **Sanctioned:** yes (`sanctioned:`).
  Goldens `cmd_ctors` / `sub_ctors` / `perform_ctor`.

### B13 — Shapes the Go reference cannot build (Ipê compiles + runs)
- **Differs:** several well-typed Ipê programs are rejected by the Go reference's
  front-end, so Ipê's output is recorded as the reference:
  - Recursive enum through a **tuple** payload — `type Chain = ChainEnd | ChainNode (Chain, Int)` (Go parse error; Ipê boxes the cyclic edge so the Rust enum stays finite-sized). Golden `tuple_self_edge`.
  - Recursive enum through a **record** payload — `type RChain = REnd | RNode { rest : RChain, val : Int }` (Go parse error). Golden `record_self_edge`.
  - `Ipe.Ui` with `Html.htmlRender` — not exposed by the Go oracle (`sky dev`), which exits 1; Ipê compiles and runs. Goldens `stdui_onclick` / `stdui_oninput_closure`.
  - `Set` generic / member on shapes the Go oracle exits 1 on. Goldens `set_generic` / `set_member`.
  - Invalid-encoding decode input where the Go oracle exits 1; Ipê returns `Err`. Golden `encoding_invalid`.
  - Partial application of a sibling **let-bound** function value (`wrap f x = f x + 1; guarded f = wrap (inc f)`) — Go emits `wrap(oneArg)` against the flattened 2-arity local and fails `go build` (`not enough arguments in call to wrap`); Ipê eta-expands the residual and Arc-promotes the captured value. Golden `fn_capture_eta_promoted` (output `4`, hand-computed language semantics).
- **Go-oracle relationship:** Go-failure (auto kind-1); Ipê handles the shape.
- **Rationale:** capability/coverage; Ipê's output is correct on these shapes.
- **Sanctioned:** yes (auto Go-failure).

### B14 — Runtime-fork behavioral hardening (vs the reference's Rust runtime)
The 48 `sky_runtime` modules are a vendored fork shared by name with the
reference's `feat/runtime-rust` runtime; the divergence is within-module. Ipê is
uniformly at-or-ahead — several behavioral differences vs the reference's Rust
runtime (each either matches Go or is more correct):
- **auth** — Ipê fail-closes on an id-column decode error; the reference's Rust
  runtime `unwrap_or(0)` (authenticating as user 0). *Security.*
- **jwt** — Ipê rejects `now == exp` (Go parity) and makes `exp`/`nbf` optional;
  the reference's Rust runtime accepts one instant past expiry and rejects
  legitimately exp-less tokens. *Correctness/security.*
- **http/ws/http_stream** — Ipê redacts URL userinfo/query in errors, defaults
  SSRF-deny ON in production, and returns `Err` on an invalid HTTP method; the
  reference's Rust runtime echoes the URL, is SSRF-opt-in, and silently downgrades
  an invalid method to GET. *Security/correctness.*
- **decimal/money** — Ipê rounds `toStringFixed`/`formatWith` half-away-from-zero
  (Go `StringFixed` parity), caps division at 16 dp (Go `DivisionPrecision = 16`),
  and `saturating_abs` in `allocate`; the reference's Rust runtime banker's-rounds
  `toStringFixed`, leaves division uncapped, and wraps at `i64::MIN`.
- **cache** — Ipê uses saturating counters; the reference's Rust runtime uses raw
  `+=`/`-=` (debug overflow panic). *Soundness.*
- **regex split** — Ipê drops the trailing zero-width empty (Go `regexp.Split`
  parity); the reference's Rust runtime keeps boundary empties.
- **env** — Ipê routes env access through a process-global lock; the reference's
  Rust runtime uses raw `std::env`. *Soundness (env data-race).*
- **telemetry/trace** — Ipê strips CRLF from CSP frame-ancestors and scrubs log
  controls + U+2028/9. *Security.*
- **render/diff depth cap (RT-UI-001)** — `ui/render.rs::render_element` and
  `dom/diff.rs::diff_node` now cap descent at `MAX_HTML_DEPTH = 1024` (matching
  the sibling `html.rs` walkers already bounded there); a tree deeper than 1024
  is silently truncated rather than stack-overflowing the process. The reference's
  runtime recurses without a cap. *Soundness (no stack abort on deep Model-derived
  UI trees).*
- **TUI fillPortion area cap (RT-TUI-001)** — `tui/layout.rs::fill_spec` clamps
  each `Length::Fill(p)` portion at `MAX_CELLS = 100_000` at construction; the
  distribution folds (`distribute_row_fill`, `distribute_col_fill`) use
  `saturating_add`; `Block::set_width` clamps the repeat count. A program-Int
  `fillPortion i64::MAX` row now renders bounded rather than wrapping the sum
  and allocating ~9e18 bytes. The reference's runtime is unclamped. *Soundness
  (no OOM/panic on adversarial fill weights).*
- **TUI padding area cap (RT-TUI-002)** — `tui/layout.rs` applies a
  terminal-proportional row cap (`clamp_pad_rows(rows, canvas) = rows.min(
  canvas.rows × PAD_ROW_SLACK)`, `PAD_ROW_SLACK = 4`) at every pad/gap
  row-allocation site (`apply_padding` top/bottom, `vstack` gap,
  `apply_self_height`). Per-axis caps alone could still produce a
  `rows × width ≈ 10 GB` product; the row cap bounds the area to roughly
  `(canvas.rows × 4) × MAX_CELLS ≤ 96 × 100_000 ≈ 10 MB` worst case.
  The reference's runtime clamps per-axis only. *Soundness (no OOM on huge
  paddingEach values).*
- **Sanctioned:** these are runtime-fork differences, not source-program
  divergences; they are captured in `docs/architecture/sky-rust-backend-reference-audit.md` §Runtime.

### B16 — True last-use clone analysis vs reference's use-count≥2 blanket (#104)
- **Differs:** For a local binding of non-`Copy` type that appears in multiple
  owned-consume positions, the reference clones **all** occurrences — including
  the last one — when the binding is in `ecCloneVars` (the set of locals used
  ≥ 2 times; `ExprEmitter.hs` `collectVarLocalsMulti`, `varLocalRead:781-787`).
  Ipê performs true last-use analysis: borrow-position reads (comparison operands,
  `++`, interpolation) are emitted bare; among owned-consume reads the **last** is
  emitted as a move (zero clones), and every earlier one is `x.clone()`. Result:
  N uses → N−1 clones instead of N; borrow positions are excluded from the count.
- **Go-oracle relationship:** Go succeeds; output is identical (clone count is an
  internal concern). The divergence is in emitted-Rust efficiency only.
- **Rationale:** soundness/efficiency — Rust's move semantics let the last use
  move; over-cloning the last use is incorrect by Rust's standards (wastes an
  allocation on a path where the original is never touched again). Strictly better
  than the reference. See `docs/adr/0002-seal-noncopy-move-clone-escape-hatch.md` (§4.1).
- **Verified:** design doc §4.1; reference `ExprEmitter.hs:294,781-787`.
- **Sanctioned:** yes (`sanctioned:`). Pending fixture goldens for #104.

### B17 — Refutable as-pattern alias bind vs reference's drop-the-alias bug (#99)
- **Differs:** For a match-arm alias pattern `name @ inner` over an owned
  scrutinee, the reference drops the alias name: `patternToMatchString` renders
  `((a,b) as w)` as just `(a, b)` and never binds `w`
  (`ExprEmitter.hs:4206`). A body that uses `w` would fail with E0425 "cannot
  find value `w`" — the alias whole is lost. Ipê correctly binds the whole by
  move (`name @ skeleton`) and reconstructs the inner bindings from
  `name.clone()` in the arm prelude, routing through `emit_binding_stmts` for
  nested aliases. The reference's `let-else` + `patternIsIrrefutable` discipline
  (`Pattern.hs:113-171`) is ported for the refutable reconstruction branch.
- **Go-oracle relationship:** Go backend does not generate Rust, so the reference
  pattern is the Haskell-emitting-Rust path. Ipê's output is correct on this
  shape; the reference's output is wrong when `name` is used in the arm body.
- **Rationale:** correctness — the reference has a latent bug (alias name dropped
  → E0425 on use). Ipê fixes it. See `docs/adr/0002-seal-noncopy-move-clone-escape-hatch.md` (§4.2).
- **Verified:** design doc §4.2; `ExprEmitter.hs:4206`; existing #96 clone-split
  machinery (`emit_expr.rs:3237`) extended into `emit_arm_head`.
- **Sanctioned:** yes (`sanctioned:` — correctness improvement over a reference
  latent bug). Pending fixture goldens for #99.

### B15 — Float scientific-notation threshold — **RESOLVED (Ipê-correct)**
- **Differs:** Ipê's `stringify.rs` switches to scientific notation at exponent ≥ 6
  (`!(-4..6)`); the reference's Rust backend switches at exponent ≥ 21 (`!(-4..21)`).
- **Go-oracle relationship:** RESOLVED by a direct probe of Go 1.26.2 (task #52,
  commit `1903654`): `fmt %v` ≡ `strconv 'g',-1` cuts to scientific notation at
  decimal exponent ≥ 6 (and < −4) for every input — there is no exp-21 behaviour.
  `1000000 → "1e+06"`, `1e15 → "1e+15"`, `999999 → "999999"`. Ipê matches Go
  byte-for-byte; the reference's exp ≥ 21 is the value that diverges from Go.
- **Rationale:** Go `%v` parity, now oracle-confirmed and pinned by discriminating
  regression tests (`float_go_v_parity` / `ff_go_g_threshold_is_six_not_twentyone`,
  proven RED under a scratch `!(-4..21)` flip).
- **Sanctioned:** N/A — Ipê matches the Go oracle exactly, so this is a difference
  from the *reference's Rust fork* only, not from Go. No sanctioned-divergence marker
  needed.

### Note — Decimal rounding modes are parity, not a divergence
`Decimal.round` uses banker's rounding (Go `RoundBank`) and
`Decimal.toStringFixed`/`formatWith` use half-away-from-zero (Go `StringFixed`);
both match Go exactly and are therefore *not* divergences from Ipê. Recorded here
only to pre-empt mis-listing (see AGENTS.md "Agent learnings").

### B18 — WS `sendBinary` (server + client) takes `Vec<u8>`, not `String`
- **Differs:** Ipê defines `type alias Bytes = String`, so the Go reference's
  server `sendBinaryToClient` AND the client `Ipe.WebSocket.sendBinary`
  both take a `String` (raw bytes in a Go string; cost-free alias). Ipê's
  `Bytes` is a distinct `Vec<u8>` primitive (B2), so both `Ws.sendBinaryToClient`
  and `WebSocket.sendBinary` take `Bytes` (`Vec<u8>`) — no lossy UTF-8 hop. The
  client stdlib's `sendBinaryRaw` is annotated `Int -> Bytes -> Task Error ()`
  accordingly (the reference declares `Int -> String`). Programs that pass binary
  data through either work correctly on Ipê; the same program on the Go reference
  relies on Go's transparent `string` ↔ `[]byte` relationship.
- **Go-oracle relationship:** Go succeeds; binary frames are representationally
  different (`String` vs `Vec<u8>`). For ASCII-range payloads, output is
  identical. For non-UTF-8 binary payloads, Ipê is the correct implementation
  (no silent corruption).
- **Rationale:** B2 consequence (`Bytes` = distinct `Vec<u8>` primitive — lossless
  byte model). **Sanctioned:** yes (`divergence:`).

### B19 — WS server `sendToClient` / `broadcast` are bounded fail-fast (D4)
- **Differs:** Go's reference WS server (`runtime-go/rt/server_websocket.go`)
  blocks up to ~30 s on a full write buffer before returning an error. Ipê's
  `ws_loop` uses a `tokio::sync::mpsc::channel` of capacity
  `IPE_WS_SEND_BUFFER` (default 256 frames) with `try_send`: when the queue is
  full the send returns `Err` immediately without blocking. Frames from a slow
  or dead consumer are dropped rather than causing handler-task pileup.
- **Go-oracle relationship:** Go succeeds; error timing and behavior on a slow
  peer differ.
- **Rationale:** security/soundness — a blocking send behind one slow peer can
  pile up goroutines/tasks (memory exhaustion), while a bounded fail-fast
  channel keeps back-pressure explicit and configurable. The 256-frame default
  (overridable via `IPE_WS_SEND_BUFFER`) is sufficient for all non-streaming
  uses. If Go's 30 s blocking semantic is required, change 3 lines in the
  adapters to `tx.send_timeout(out, Duration::from_secs(30))`.
  **Sanctioned:** yes (`divergence:`).

### ~~B20 — `ws_loop` does not send Ping heartbeat frames~~ — **CLOSED (#135)**
- ~~**Differs:** Go's reference WS server sends a Ping frame every 30 s with a
  10 s timeout (`runtime-go/rt/server_websocket.go`, `wsDefaultPingInterval
  = 30s`). Ipê's `ws_loop` had no Ping `select!` arm — dead peers lingered in
  the registry until TCP gave up.~~
- **RESOLVED (#135):** `ws_loop` now has a third `select!` arm driven by
  `tokio::time::interval(Duration::from_secs(ws_heartbeat_secs()))`.  Default
  interval 30 s (Go parity).  Override via `IPE_WS_HEARTBEAT` (seconds, > 0).
  axum auto-replies to incoming Pong frames.  Confirmed green in
  `ws_adapter_tests` unit-tests.
- **Go-oracle relationship:** parity restored — both send a Ping every 30 s.

### B21 — Unknown type names fail-closed at canon (IPE-N0002) vs deferred ICE (#138)
- **Differs:** Sky's Haskell canonicaliser resolves an unqualified uppercase type
  name by calling `Map.findWithDefault []` on `type_home_map`, silently supplying
  an empty home `[]` for any name absent from the map. An empty home downstream
  in the Go code-gen is a silent runtime error (or, in the Rust backend, a
  `IPE-I0001` ICE via the `ir_type_from_canon` unique-match heuristic). Ipê's
  `canonicalise_type` now classifies every unqualified upper-case type name
  explicitly at canon time:
  - **Known builtins** (`RESERVED_BUILTIN_TYPES` + `EXTRA_BUILTIN_TYPE_NAMES`) →
    empty-home sentinel `home = []` as before; the lowerer resolves them via
    explicit named arms.
  - **User-defined / unknown names** → `TypeNotFound` / `IPE-N0002` with a
    did-you-mean suggestion list from `type_home_map` + `ctx.aliases`. The ICE
    path and the unique-match `enum_variants` heuristic in `ir_type_from_canon`
    and `ir_type_from_ty` are removed.
- **Go-oracle relationship:** the Go backend accepts a program where a type name
  is referenced without importing its home module (the empty-home fallback is
  harmless in Go's stringly-typed codegen). Ipê rejects such programs with a
  clear user error instead.
- **Rationale:** correctness / robustness — a reference to a type that is not in
  scope is a genuine user error; silently giving it an empty home and deferring
  the failure to codegen (or crashing with an ICE) violates "make invalid states
  unrepresentable". The Sky Haskell compiler's `findWithDefault ""` is a known
  deferred-failure hole; this is the stricter-is-better class.
- **Sanctioned:** yes (`sanctioned:`). Regression gate: `golden_i138_total_resolution`
  (error fixtures `empty_home_bridge` / `optbridge` → must emit
  IPE-N0002 not IPE-I0001; positive control `kernel_implicit_positive` →
  must compile clean).

### B22 — Function value in a `Maybe`/`Result`/user-union constructor payload (#90 Stage 1)
- **Differs:** Ipê lifted the blanket `IPE-L0114` rejection of a function
  value in a constructor payload for ENUM-LIKE heads (`Maybe`/`Result`, or a
  user union) — `Ok (\x -> x + 1)`, `Just f`, and a DECLARED function-typed
  payload (`type Retryish e = RetryWhen (e -> Bool)`) all lower and run.
  `Maybe.andMap`/`Result.andMap` are usable with a function payload (arity 1);
  a CURRIED (2+-argument) payload through `andMap`, and reuse of a fn-carrying
  binding in more than one non-call position, stay fail-closed. Upstream's
  Haskell→Rust codegen instead renders a function-typed field as a bare `fn`
  pointer (`TypeRenderer.hs`) — `Clone`/`Debug`/`PartialEq`-preserving, but
  restricted to NON-CAPTURING closures (documented there explicitly). Ipê's
  `Box<dyn Fn>` payload is strictly more general (Ipê closures capture
  freely) at the cost of losing those three derives on the carrier — absorbed
  by the #87 derive-demotion fixpoint and the type checker's
  `ty_is_equatable`/serde/#91-Model use-site gates, so no unsound use reaches
  `cargo`.
- **The curried-callback gate is now a genuine TYPE-LEVEL obligation, not an
  AST-shape match (2026-07-10, re-landed a FIFTH time — the 4th attempt was
  reverted the same day for a fail-open `Ty::Var` arm; see the history note
  below).** `sky_types::ty::TyBounds::hof_kernel_result()` (a bit alongside
  `SetElem`/`DictKey`/`Show`) is tied to the callback-RESULT scheme variable
  of EVERY `Maybe`/`Result` higher-order kernel — `map`, `map2..5`,
  `mapError`, `andMap` (13 kernels; slot table
  `constrain::Builder::hof_result_slot_for`, drift-pinned by the
  `hof_result_slots_match_scheme_shapes` unit test) — at
  `constrain_var_kernel`, exactly mirroring the pre-existing `Math.min`/
  `Set`/`Dict`-key obligation mechanism INCLUDING its fail-closed treatment
  of a bare variable the obligation escaped into (the 4th attempt's
  `andMap`-only, fail-open-on-`Ty::Var` version let an ANNOTATED DOUBLE
  FORWARDER reach `cargo build` as E0308, and let the whole
  `map`/`map2..5`/`mapError` family through entirely — a user applicative
  `map2` over `Result.map`+`Result.andMap` cargo-failed even at a safe
  arity). Because the obligation is minted once per kernel REFERENCE (not
  per call-node), it survives arbitrary Ipê-level aliasing by construction —
  direct call, piped, `let`-bound, bare-value top-level re-export,
  higher-order argument, record-field extraction, and every forwarder
  nesting depth — with no AST-shape enumeration anywhere.
  `Maybe.andThen`/`Result.andThen`/`Result.traverse` need no obligation:
  their callback results are `Con`-headed in the scheme itself, so a curried
  callback is already a plain type mismatch (pinned by the
  `and_then_fn_payload_accepted` fixture, which also proves a callback
  legitimately returning `Ok fn` stays accepted and computes correctly). A lowering-time
  backstop (`reject_curried_andmap_payload`, re-anchored inside
  `lower_callee` itself — the single funnel EVERY kernel/top-level reference
  resolves through, not just the `Call`-node arm the three reverted attempts
  used) stays wired as defense-in-depth but was never observed firing in
  this pass's fixture matrix (Tier 2 always catches the hazard first). See
  `docs/architecture/ctor-payload-andmap-arity-gate-design.md` for the full
  design and `crates/ipe/tests/golden_l0114_ctor_payload_function.rs` for
  the aliasing-shape fixture matrix (direct call, `let`-bound, bare
  top-level re-export, higher-order argument, record-field extraction, and a
  cross-module ANNOTATED forwarder reused at two different arity-1-safe
  types — confirmed ACCEPTED, closing the one precision-loss case the design
  flagged as needing empirical confirmation before declaring done). Import
  aliasing (`import Result as R`) is NOT constructible in Ipê today —
  `Result`/`Maybe` are fixed compiler-kernel qualifiers
  (`crates/sky_canon/src/resolve.rs`), not backed by an importable
  Ipê-source module in this milestone, so there is no module to alias.
- **Diagnostic code depends on HOW the obligation is violated, mirroring the
  pre-existing `Math.min` gate's own documented split
  (`crates/ipe/tests/golden_m4c_math_gate.rs`)**: a DIRECT `andMap` call
  pins the obligated payload-result variable straight to a concrete `Fun`
  structure at the unifier's own head-pin check (the "eager pin" case),
  surfacing a plain `IPE-T0001` (`TypeMismatch`) — every aliasing shape in
  the fixture matrix hits this path, confirmed empirically. An ANNOTATED
  GENERIC FORWARDER around `andMap` instead lifts the obligation onto its
  own annotation skolem, re-verified per external call site, surfacing the
  friendlier `IPE-T0014` (`SuperTypeUnsatisfied`, "non-function callback
  result (Maybe/Result higher-order kernel)"). Both are clean Ipê
  diagnostics; the pipeline never emits Rust that `cargo` rejects either
  way. One documented conservatism (5th attempt): when the obligated result
  escapes into ANOTHER binding's generic variable — an annotated forwarder
  OF a forwarder, or a cross-module unannotated forwarder's promoted scheme
  — the check fails CLOSED and rejects the inner reference even when every
  eventual payload would have been arity-safe, exactly as `Math.min`'s
  Comparable bound already does on the identical shape. Cross-binding
  obligation propagation (for ALL bounds at once, with matching trait-bound
  emission) is the filed follow-up that would recover those programs.
- **Go-oracle relationship:** for a NON-CAPTURING payload closure and a plain
  `Ok f |> Result.andMap x` / `Just f |> Maybe.andMap x` chain, the reference
  Go compiler (`sky` v0.16.29) has an existing codegen bug (the same
  `interface{}`-boxed-value class as B-below): a case-arm-extracted function
  value sometimes fails `go build` outright (`invalid operation: cannot call
  f (variable of interface type any): any is not a function` —
  `function_payload_gate`, `ctor_decl_fn_payload`,
  `fn_extracted_called_twice`) and sometimes builds but computes the
  WRONG value (`result_and_map_fn_payload`, `maybe_and_map_fn_payload`
  — Go silently returns the untransformed `ra` operand instead of applying the
  boxed function, verified against an unambiguous named-function probe:
  `Ok addTen |> Result.andMap (Ok 5)` prints `5` under Go, not the correct
  `15`). Ipê's `Box<dyn Fn>` kernel call computes the semantically correct
  value in every case. A minority of shapes (a genuinely non-capturing payload
  with no `andMap`/case-extraction involved) DO match — real parity there.
- **Rationale:** Ipê closures capture freely; a bare-`fn`-pointer restriction
  (upstream's choice) would silently forbid that. `Box<dyn Fn>` is the sound
  direction, and the machinery to keep it seal-safe (#87/#93/#91/type-checker
  equatable gate) already ships. See
  `docs/adr/0015-constructor-payload-functions-narrowed-gates.md` for the full
  hazard analysis and `docs/adr/0016-andmap-arity-gate-type-obligation.md`
  for the T3 two-tier design.
- **Sanctioned:** yes (`sanctioned:` for the Go-succeeds-but-differs shapes;
  Go-failure divergence for the shapes that don't `go build`). Goldens
  `result_and_map_fn_payload`, `maybe_and_map_fn_payload`,
  `ctor_decl_fn_payload`, `fn_extracted_called_twice`,
  `function_payload_gate` (flipped from its reject branch to its
  build-and-run branch). Negative controls (aliasing-shape matrix, must stay
  a clean diagnostic — IPE-T0001 / IPE-T0014 / IPE-L0114, never a cargo-fail;
  no oracle needed): `and_map_curried_stays_gated`,
  `and_map_let_bound_alias_stays_gated`,
  `and_map_bare_alias_stays_gated`,
  `and_map_higher_order_arg_stays_gated`,
  `and_map_record_field_stays_gated`,
  `and_map_forwarder_curried_is_t0014`, `fn_carrier_reuse_gated`,
  `lambda_param_reuse_gated`. Positive cross-module control (must stay
  ACCEPTED): `and_map_cross_module_wrapper_accepted`.
- **Revert-incident history (2026-07-10, THREE reverts before this landing).**
  (1) `f80f05a` landed, reverted (`dbd876b`): the IPE-L0127 reuse gate was
  wired at 4 call sites (Def params, `let`-bindings, match-arm bindings) but
  not at `lower_lambda`'s own parameters — `\mf -> consume mf + consume mf`
  with `mf : Maybe (Int -> Int)` reused the boxed closure twice and reached
  `cargo build` as E0382; the IPE-L0114 curried-`andMap` gate also matched
  only two syntactic call shapes and was bypassed by a `let`-bound partial
  application (`let g = Result.andMap (Ok 1) in g (Ok add3)`), reaching
  `cargo build` as E0277. (2) `39d9a57` re-landed with both bugs fixed —
  the reuse gate now also runs over `lower_lambda`'s `ir_params`, and the
  curried-payload check moved into `lower_call_uniform`'s
  `VarKernel | VarTopLevel` arm, keyed on the resolved `Callee` — but was
  reverted AGAIN (`73f33bc`) after independent review found a THIRD bypass:
  a bare, point-free top-level re-export (`myAndMap = Result.andMap`, then
  `myAndMap (Ok 1) (Ok add3)`) resolves to `Callee::Func`, not
  `Callee::Kernel`, at the OUTER call site — the check, still living inside
  `lower_call_uniform`'s Call-node arm, never saw the kernel reference at
  all, because that reference is a bare VALUE inside `myAndMap`'s own body,
  lowered through a DIFFERENT `lower_expr` arm that never calls
  `lower_call_uniform`. (3) This landing (the one this entry documents)
  replaces the AST-shape approach entirely with the two-tier design summarized
  above: Tier 2 is a genuine type-level obligation minted once per kernel
  reference (immune to AST shape by construction), and Tier 1 is re-anchored
  inside `lower_callee` — the actual single funnel, proven by inspection to
  be the only path any kernel/top-level reference can lower through. Every
  exact failing shape from all three incidents is now a permanent fixture:
  `lambda_param_reuse_gated` (Bug 1), `and_map_let_bound_alias_stays_gated`
  (Bug 2), `and_map_bare_alias_stays_gated` (Bug 3) — plus new fixtures
  for higher-order-argument and record-field-extraction aliasing (neither
  incident found these bypassed, but the design doc named them as unexplored
  shapes to verify rather than assume closed) and the cross-module annotated-
  forwarder case the design flagged as needing empirical confirmation.

### B23 — Boundary Scheme Promotion: phase-1 under-acceptance for untyped bindings (class-1 inference fix #2)
- **Context:** an unannotated top-level binding is monomorphic *within its
  home module* (unchanged); at its module's boundary it is now generalized
  into a scheme, and each cross-module reference instantiates it fresh — see
  `docs/adr/0008-untyped-binding-module-boundary-generalization.md`. Empirically
  verified against the reference `sky v0.16.29`: a cross-module untyped
  helper used at two different concrete types from two different importers,
  and an untyped zero-param value binding used at two different element
  types cross-module, are both **accepted** by the reference. Ipê now accepts
  both too (test matrix items 1 and 3 in the spec).
- **D1 — ambiguous instantiation fails closed.** Where the reference accepts
  via Go's `[]any` erasure (a use-site region still carrying a free type
  variable not covered by the enclosing def's own generics), Ipê rejects with
  IPE-L0102-ambiguous at the use span. **Sanctioned:** yes — matches the
  repo's "prefer concrete over generic codegen" rule; strictly the safer
  direction (under-acceptance, never a soundness hole).
- **D2 — `Super`-bounded residual vars stay program-monomorphic in phase 1.**
  The reference generalizes `number`-bounded untyped bindings (e.g. `plus a b
  = a + b` used at `Int` in one module and `Float` in another); Ipê phase 1
  defers this — `Super`-bounded roots are excluded from quantification and
  stay shared program-wide, so such a program is still rejected. **Sanctioned:**
  yes — known under-acceptance; phase 2 (quantify `Super{flex}` too, populate
  `bounds` keyed by synthesized symbols) is additive-only when it lands;
  `#66`/`#110`'s oracle differential must whitelist this gap until then.
- **D3 — rigid-contaminated untyped defs stay unquantified.** A def whose
  body unifies with a typed sibling's skolem (`f : a -> a; f x = ident x`)
  leaves `ident`'s shared var rigid; phase 1 conservatively excludes rigid
  roots from generalization. **Sanctioned:** yes — known under-acceptance,
  phase-2 item after a skolem-escape review.
- **Rationale:** all three (D1/D2/D3) are under-acceptance (Ipê rejects
  programs the reference accepts) — the safe direction. At the HM-solver
  level, an instantiated scheme var is always plain `Flex` (the same shape
  typed instantiation already produces), so no new `Super`-flex / `Super`-
  rigid meeting points exist — no HM-soundness hole is introduced by this
  fix.
- **Incident note (do not re-drop this caveat):** this fix first landed as
  commit `29bab0d` and was same-day reverted (`5e870b4`) after independent
  adversarial review found a real SEAL violation that the doc's own earlier
  draft had missed: `ipe` exit-0, but the emitted Rust failed `cargo build`
  with E0283 ("cannot infer type of the type parameter T1 ... cannot satisfy
  `_: Clone`") on a 3-module cross-module field-access getter
  (`getName r = r.name`). Root cause: `promote_untyped_boundaries`'s
  `obligation_roots` excluded the record var (`fa.record`) from
  quantification but not the field-access's own result var (`fa.result`),
  so a getter's return-type var could be quantified before `resolve_deferred`
  pinned it concrete — a codegen-level defect (an unused Rust generic), not
  an HM-soundness hole, but a genuine SEAL violation (exit-0 on a program
  whose emitted output does not compile). Re-fixed by (1) inserting
  `fa.result` into `obligation_roots` alongside `fa.record`, and (2) porting
  the Typed lowering arm's `used_generics` structural-appearance filter into
  the Untyped arm as defense-in-depth, so a stale quantified var can never be
  declared as a Rust generic absent from the resolved `params`/`ret` again.
  Both re-verified against the EXACT failing shape via a real `cargo build`
  + `cargo run` golden (`crates/ipe/tests/golden_class1_boundary_scheme_field_result.rs`,
  `IPE_E2E=1`), not just a `sky_types` unit test — the original attempt's
  full `sky_types` unit-test matrix (including the obligation-gated
  single-record-type-cross-module-use case) passed despite the bug, because
  the defect is invisible to HM-level checks. Given that history, this
  entry deliberately does **not** claim an exhaustive "zero over-acceptance"
  guarantee across the whole `obligation_roots` surface — only that the
  specific bug class independent review found is closed and re-verified,
  and that the two fixes are structurally defense-in-depth (a gap in one
  does not silently defeat the other).
- **#201 follow-up — a promoted element-polymorphic recursion was rejected at
  LOWERING, not inference.** A cross-module untyped recursive function
  polymorphic in its LIST-ELEMENT type (`evenLen`/`oddLen`/`listLen : List a ->
  Bool`, fuzzer seed 31348 `mmrecpair`) was correctly generalized by
  `sky_types` (`untyped_type_params` listed the element var), yet `ipe` FAILED
  with IPE-L0102 at the `[] ->` arm. Root cause was NOT the boundary scheme but
  a stale gate in `sky_lower::lower_case`: it rejected ANY list `case` binding a
  value (`_ :: rest`) whose element lowered to `IrType::Generic(_)`, on the now-
  false premise that "function generics emit bound-free" so the owned-rebind
  (`rest.to_vec()` / `x.clone()`) would `cargo`-fail. Every emitted function
  type parameter in fact carries a `Clone` bound (`render_fn_generics`'s
  `bounds.with_clone()`), and `list_elem_ir` returns `IrType::Generic(sym)` ONLY
  for a var that IS one of the enclosing function's declared type parameters (a
  free var maps to `IrType::Json`), so the emitted
  `fn f<T1: Clone>(xs: Vec<T1>) -> …` with `rest.to_vec()` builds. The gate
  rejected sound programs (an exit-1 where the backend would have built cleanly)
  — removed. This turns a spurious under-acceptance into acceptance; the emitted
  Rust is a `Clone`-bounded generic monomorphized per use site. Re-verified via
  a real `cargo build` + `cargo run` golden
  (`crates/ipe/tests/golden_i201_cross_module_poly_recursion.rs`, `IPE_E2E=1`,
  prints `EO`) and the fuzzer (`scripts/fuzz-well-typed.sh --seed 31348`, now
  green).

### B24 — Prescriptive TEA `init` signature (Live → `LiveReq`; Tui/Webview → `()`) (#180)
- **Reference:** `upstream:src/Sky/Type/Constrain/Expression.hs:2665-2695` leaves
  `Live.app`'s `init` argument a **free type var** (`req`), and models the
  request as a heterogeneous `map[string]any` accessed via `Dict.get "path"
  req`. It does this for a Go-runtime reason (keep the untyped map compatible
  with any inferred shape; return-only TVar defaulting collapses it to
  `rt.SkyValue` for examples that never touch `req`).
- **Ipê is prescriptive:** the `init` field is **pinned per app shape** — 
  `Live.app` requires `init : LiveReq -> (Model, Cmd Msg)`, and
  `Tui.app`/`Tui.program`/`Webview.app` require `init : () -> (Model, Cmd Msg)`.
  A mismatch (`init : {} ->` on Live, or `init : LiveReq ->` on Tui) is a clear
  compile-time IPE-T0001 (`expected LiveReq, found {}` / `expected (), found
  LiveReq`) at the `init` cfg field, not a raw unification failure and not a
  deferred `cargo` break. A `Live.app` init that declares polymorphic `init : a
  -> …` unifies `a` to `LiveReq` automatically (unchanged for the canonical
  corpus examples 09/10/37).
- **`LiveReq` is a typed opaque record, not a heterogeneous map.** Ipê's runtime
  carries `sky_runtime::live::LiveReq` as a concrete struct (`path`/`query`/
  `method : String`; `params`/`headers`/`cookies : Dict String String`).
  `req.path` is ordinary field access: `LiveReq` stays an opaque nullary `Con`
  at the type level (so no bare record literal can masquerade as the runtime
  struct — the same make-invalid-states-unrepresentable posture as the opaque
  server `Request`), but its fixed field set is READABLE via the deferred
  `FieldAccess` pass (`LiveReqFields`, mirroring `RequestFields`). A field access
  lowers to `(req).<field>.clone()` reading the struct directly — no synthesised
  record. Record UPDATE on a `LiveReq` is rejected (IPE-T0017), exactly like
  `Request`.
- **Rationale:** ambient input (env/args/cwd) is reached through `System.*` from
  anywhere; `init`'s argument carries ONLY genuine per-invocation context with
  no ambient accessor — `LiveReq` for Live (a session is born from one specific
  HTTP request; there is no `System.currentRequest`), nothing for Tui/Webview.
  Elm needs `flags` as an init arg only because a browser sandboxes JS; Ipê runs
  natively with a real `System` API, so `flags`-as-init-arg is redundant. Being
  prescriptive is both more Elm-`Browser.application`-faithful and
  make-invalid-states-unrepresentable — the reference's free-tvar rationale
  (untyped `map[string]any` compat) simply does not transfer to a typed
  `LiveReq`. **Sanctioned:** yes — stricter direction (rejects only ill-shaped
  `init`s the reference would silently default), no soundness hole. Full design:
  `docs/architecture/tea-shape-matrix-and-init-design-2026-07-13.md`.

---

### B25 — `Jwt.encode`/`decode` (HS256) reject a signing secret under 32 bytes
- **Ipê:** the HS256 kernel (`src/runtime/rust/src/jwt.rs`) fails closed with
  `jwt-encode: HS256 secret must be at least 32 bytes (RFC 7518 §3.2)` when the
  key is shorter than 32 bytes (256 bits). The reference accepted any-length
  key. *Rationale:* security — an HMAC key below the hash-output size is
  low-entropy and forgeable; a 1-byte key mints a token anyone can re-sign. This
  is the same 32-byte floor `auth.rs` / `Ipe.Auth` enforce, closed for a direct
  `Jwt.*` caller that bypasses `Ipe.Auth`. A short-key example must adopt a
  >=32-byte secret (e.g. the `00-standard-libs` port's `ipe-edits`).

---

## 3. Architectural divergences (compiler + runtime structure)

These are structural consequences of porting a Haskell compiler that emits
Go/Rust into a Rust compiler that emits Rust. Confirmed against upstream Sky
(`feat/runtime-rust`) and the Ipê tree.

### A1 — Rust-all-the-way `skyc` vs a Haskell compiler
The reference is a Haskell compiler (`src/Sky/…`) emitting Go and Rust. Ipê is a
single Rust pipeline — parse → canon → types → lower → IR → Rust-emit — split
across `crates/sky_canon`, `crates/sky_types`, `crates/sky_lower`,
`crates/sky_ir`, `crates/sky_backend_rust`. Strategies and invariants are ported,
never literal code.

### A2 — Typed IR checkpoint vs AST→string emitters
Ipê lowers `canon → typed sky_ir::Expr → Rust` (two stage). The reference walks
`Can.Expr_` → Rust string in one pass
(`src/Ipê/Generate/Rust/Builder/ExprEmitter.hs`). *Rationale:* a malformed shape
is unrepresentable in the typed IR rather than caught only at `rustc` —
make-invalid-states-unrepresentable.

### A3 — Typed `TailRecur`/`TailLoop` IR + self-authored Rust `loop` emission
Ipê represents tail recursion as typed IR nodes (`Expr::TailRecur` /
`Expr::TailLoop`, `sky_lower/src/lower.rs`) and emits a Rust `loop { … }`. The
reference transports the jump as a stringly kernel-name sentinel
(`tcoMarker = "__tco_jump__"`, `src/Ipê/Build/TailCallOpt.hs:140`) and emits TCO
**Go-only** — its Rust backend has no TCO. Ipê ports the reference's
backend-agnostic `isTailRecursive` analysis but authors the Rust loop emission
itself. *Rationale:* soundness — constant stack vs an uncatchable stack-overflow
trap; and a typed jump vs a stringly sentinel.

### A4 — Closed typed kernel registry + fail-closed default
Ipê dispatches through a closed **424-variant** `KernelFn` enum with a typed
`StdlibKernel` registry (`crates/sky_kernels`), indexed anti-drift from
`StdlibKernel::ALL`; an unknown kernel fails **closed** with `IPE-L0108`. The
reference dispatches `(mod,name)` via a string `case` and falls **open** to a
`toSnakeCase` default (`src/Ipê/Generate/Rust/Builder/Kernel.hs:801-802`).
*Rationale:* security → correctness — a fail-open snake_case default is the exact
"ipe exits 0 then the emitted code fails to build" class; the enum *is* the
registry.

### A5 — `render_type : IrType → DResult`, no `"String"` default
Ipê's type renderer is a closed function returning `DResult<String>`
(`emit_types.rs:73`) with no catch-all. The reference's `TypeRenderer.hs` falls
back to a `"String"` default on an unmatched type. *Rationale:* soundness floor —
the renderer is total by construction.

### A6 — First-class opaque `IrType` variants vs `{M}`-placeholder strings
Ipê models opaque/parametric types as first-class closed `IrType` variants with a
structural `Box<IrType>` message parameter. The reference uses a stringly-keyed
`Map (String,String) String` with `{M}`-placeholder substitution and
re-derivation. *Rationale:* invalid-states.

### A7 — Exact-key record resolution, fail-loud
Ipê resolves record aliases by exact sorted-key match and raises a `CompilerBug`
on a miss. The reference widens to the best superset row and falls back to
`"String"` on a miss. *Rationale:* soundness > completeness (a superset fallback
would be added only if a real example trips the guard).

### A8 — Uniform `Box<dyn Fn>` callbacks (reference is more complete here)
Ipê renders effectful callbacks uniformly as `Box<dyn Fn>` (with a handler
special-case). The reference uses a 3-way classification (stored
`Arc<dyn Fn>+Send+Sync` / passed `impl Fn` / ADT-embedded bare `fn`). This is one
axis where the reference is more complete; Ipê adopts the 3-way split when a
`derive`/`Clone` callback subsystem lands. *Neutral: reference-ahead on
completeness.*

**#195 refinement (2026-07-14) — decoder-payload function values are Send-only,
matching the runtime.** The uniform `IrType::Fun` → `Box<dyn Fn + Send + Sync>`
rendering is retained for callback PARAMETERS (which may forward into a shared
`Arc<dyn Fn + Send + Sync>` UI/Live event slot via `arc_callback_wrap`, the
load-bearing #184 path), BUT a function value that is the PAYLOAD of a decoder
(`Decoder (a -> b)` — the accumulator of a `succeed Ctor |> required …` pipeline,
or a `succeed (partiallyApplied x)`) now renders as the Send-ONLY curry chain
`Box<dyn FnOnce(a) -> b + Send>` the runtime actually constructs (`curryN` +
`decode_succeed`, `runtime/src/sky_runtime/json.rs`). The blanket `+ Sync` on a
decoder payload was over-constrained — a `FnOnce` curry chain is `Send` but not
`Sync`, and a decoder payload is owned/linear and never flows into an `Arc`
slot — so it caused a ipe-0-then-cargo-fail (E0308 wrong-trait + E0277
`Sync`-unsatisfiable). This mirrors the reference's ownership split (owned/linear
→ Send-only) at the decoder-payload position rather than diverging from it.
Regression: `golden_i195_json_decode_pipeline`; the `+ Sync` forwarding path is
pinned by `golden_i190_static_bound` / `golden_i191_input_arc_capture`.

**#198 refinement — decode-combinator mapper payload parameters are Send-only
too.** #195 renders the Send-only `FnOnce` chain at the `Decoder<Fun>` TYPE
position (the producer). When that payload flows OUT of the decoder into a
mapper's PARAMETER — `JsonDec.map (\f -> f x) d`, `andThen (\f -> …) d`, and the
`map2`/`map3`/`map4` + `Db.Decode` equivalents — the parameter's inferred type is
a bare `Ty::Fun`, so `lower_lambda` stamped it as `IrType::Fun`, which
`render_type` emits as the shared `Box<dyn Fn + Send + Sync>`: wrong trait (`Fn`
vs the producer's `FnOnce`) plus an unsatisfiable `+ Sync` → ipe-0-then-cargo-
fail (E0308 / E0277), the same class as #195 one surface deeper. In every
`map`/`mapN`/`andThen` combinator the mapper's parameters ARE, by construction,
the decoded payload value(s), so `sky_lower::retype_decoder_payload_mapper`
retypes any single-parameter function-typed mapper param from `IrType::Fun` to
the owned `IrType::FnOnceChain` at the combinator call site — matching the
producer shape rather than diverging. Single-parameter only: a `FnOnceChain` is a
nested curry chain the flat body application (`(f)(a, b)`) does not match, so a
multi-parameter payload stays a distinct surface. Regression:
`golden_i198_decoder_payload_mapper` (ipe-0 render assertion + cargo-0 E2E).

### A9 — Crate-version SSOT as a typed `const` table + drift test
Ipê holds crate name+version in a typed `const CrateSpec` table
(`crates/sky_backend_rust/src/crate_specs.rs`) read by every manifest-emitting
function, with a co-located drift test asserting the SSOT ≡ `runtime/Cargo.toml`
(all crates) **and** ≡ the golden base manifest. The reference holds the same SSOT
as an embedded `crate-specs.toml` re-parsed at build
(`src/Ipê/Generate/Rust/Builder/crate-specs.toml` + `CrateSpecs.hs`) with a sync
test. *Rationale:* compiler-checked structured data over a string re-parse; Ipê's
drift test additionally covers the golden base manifest.

### A10 — Kernel-registry drift tripwires
Ipê builds the canon `stdlib_index` anti-drift from `StdlibKernel::ALL`
(`sky_canon/src/env.rs`), so a registered kernel and its call-site resolution
cannot skew silently. *Rationale:* parse-don't-validate at the registry boundary.

### A11 — Runtime as a vendored fork that has since diverged
The 48-module `sky_runtime` is a vendored fork shared by name with the
reference's Rust runtime; Ipê's copy is a strict superset — every module is
equal-or-larger with the reference's logic plus the security/correctness/soundness
hardening enumerated in B14. Structurally: runtime divergence is *within-module*,
not a different module layout. *Rationale:* the reference's Rust runtime is not
cargo-culted back in.

### A12 — Fail-closed refutable function-argument patterns
For a refutable function-arg pattern (`f (Just x) = …`) Ipê refuses at lower
(IPE-L0115/0116) and closes the gap via a front-end desugar to `case`. The
reference synthesises a `let … else { panic! }` (a reachable `panic!`).
*Rationale:* soundness — "no panic from well-typed Ipê" outranks the completeness
the reference gains. (Front-end desugar is the completeness close.)

### A13 — Fail-closed nested-constructor-payload patterns (Ipê currently less complete)
A nested list/cons/record inside a constructor payload (`Just (h :: t)`,
`Ok {name}`) is rejected fail-closed in Ipê
(`Err(NestedCtorDiscrimination/NestedPayloadPatterns)`); the reference recurses
and compiles it. *Rationale:* soundness over completeness for now; the
completeness gap is a tracked front-end item. *Neutral: reference-ahead on
completeness.*

### A14 — Non-HOF `List` ops as iterative kernels vs pure-Ipê recursion (efficiency-only, output-identical)
Ipê wires the non-HOF `List` combinators
(`append`/`concat`/`take`/`drop`/`zip`/`cons`/`isEmpty`) as **iterative Rust
kernels** (constant native stack), whereas the Go "Ipê" backend classifies them
as non-tail-recursive pure-Ipê (O(N) call-stack). Output is byte-identical
across all Elm edges (negative/over-length `take`/`drop`, shorter-truncating
`zip`, empty `concat`); Ipê additionally has a strictly better stack profile (no
200k+-element stack-depth risk). *Rationale:* `List.*` is anchored to
`VarHome::Kernel` in Ipê canonicalisation (task #68), so the kernel path is the
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

Ipê's Rust runtime imposes **static trait bounds** on both type parameters:
`live_app<Model, Msg>` requires
`Model: serde::Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync`
and `Msg: Clone + Send + Sync + Debug` (`runtime/src/sky_runtime/live/mod.rs`).
A Model or Msg that carries a function, `Cmd`, `Sub`, `Task`, or `Decoder` does
not satisfy these bounds, so the emitted Rust fails `cargo build`. Because
`ipe` exit-0 MUST imply `cargo` exit-0 (the seal), the backend adds explicit
admissibility gates:

- **#91 (shipped):** `check_admissible_model` in `emit_model_gate.rs:62` — gates
  Model at `ipe`, emits `IPE-L0120` on a non-serde/non-Clone leaf. Verified:
  `code.rs:198-200`, `emit_model_gate.rs`.
- **#94 ✅ shipped (landed, corrected 2026-07-09):** `check_admissible_msg` in
  `emit_model_gate.rs:105` — gates Msg at `ipe` using `ir_type_is_derivable`
  for all three app shapes (NOT serde — Html is derivable and thus admissible
  as a Live Msg payload, unlike Live Model), called from `emit_live.rs:349` /
  `emit_tui.rs:182` / `emit_webview.rs:137`. Emits `IPE-L0125`
  (`InadmissibleAppMsg`) — NOT the originally-planned `IPE-L0121`, which was
  reassigned in the interim to the unrelated `JsonDec.succeed` curry-arity
  gate. Regression: `crates/ipe/tests/msg_admissibility.rs` (7/7 green).
  Originally designed in `docs/architecture/seal-gates-msg-lambda-view-design.md §2`.
- **#95 ✅ shipped (landed, corrected 2026-07-09):** Lambda-aware
  `fn_param_ty(e, idx)` in `emit_model_gate.rs:46` — closes the fail-open gap
  where `view = \m -> …` (an `Expr::Lambda`) bypassed the `FuncValue`-only
  model recovery and silently skipped the gate. Originally designed in §3 of
  the same doc.

*Rationale:* seal-forced divergence. The Go backend's dynamic path is correct for
Go; the Rust backend's static bounds make the Go-dynamic path a `cargo`-fail.
Gates at `ipe` convert the `cargo`-fail class into a clear user diagnostic.
See `docs/architecture/seal-gates-msg-lambda-view-design.md §4`.

### A16 — App cfg must be an inline record literal (IPE-L0119)
The reference Go backend accepts any expression as the `Live.app` / `Tui.app` /
`Webview.app` cfg argument, including a let-bound variable
(`let cfg = { … } in Live.app cfg`). Ipê's backend reads the cfg's fields
(`init`, `update`, `view`, `subscriptions`, …) directly from the structural
record at the call site; a non-literal argument (a `Var`, a pipe result, a
function call) cannot be field-indexed at lower time, so it is rejected with
`IPE-L0119` ("app entry cfg must be an inline record literal").

*Rationale:* Rust-lowering constraint — the backend must structurally decompose
the cfg record at lower time to emit the correct `live_app` call; a variable
reference loses the field structure. The reference's Go backend reconstructs the
cfg at runtime via reflection; Ipê does not have that escape hatch.
*Verified:* `code.rs:196`, `explain/IPE-L0119.md`, `emit_live.rs` (lookup_field).
*Note:* let-bound-cfg support (`[feature: let-bound-app-cfg]` in the explain
page) is a tracked future item — not a permanent limitation.

### A17 — `Float` rejected as `Set` element or `Dict` key (IPE-L0117)
Ipê's type system treats `Float` as a `comparable` value, so the Ipê type checker
accepts `Set Float` / `Dict Float v` — the reference Go backend uses `interface{}`
comparison and tolerates these at runtime. Ipê backs `Set a` with
`BTreeSet<a>` and `Dict k v` with `HashMap<k, v>`; Rust's `f64` implements
neither `Ord` (NaN has no place in a total order) nor `Hash`/`Eq` (NaN != NaN).
Emitting `BTreeSet<f64>` / `HashMap<f64, _>` would produce Rust that does not
compile, so the case is rejected at lower with `IPE-L0117`.

*Rationale:* Rust-substrate constraint, permanent. The NaN/ordering issue is a
semantic property of IEEE 754 floating point that does not arise in Go's
`interface{}`-keyed maps. The diagnostic is deliberate and named a divergence from
Ipê in its own explain page. *Verified:* `code.rs:192-194`,
`explain/IPE-L0117.md`. Total-order `Float` set/dict (e.g. via an
ordered-float wrapper) is a tracked future enhancement.

### A18 — `WsServerCfg` phantom `msg` type var dropped (D2)
Ipê's `Ipe.Http.Server.WebSocket` stdlib source declares
`WebSocketServerCfg msg` with a phantom `msg` type variable reserved for
hypothetical future `Sub` integration (the phantom never reaches the runtime).
Ipê types the cfg as a **nullary** opaque constructor: `IrType::WebSocketServerCfg`
renders `WsServerCfg<SkyError>` directly, with `E = SkyError = String` pinned at
the emit site. The runtime struct `WsServerCfg<E>` remains generic over `E`;
Ipê merely instantiates it monomorphically.

Effect: a type annotation `Ws.WebSocketServerCfg Msg` compiles on the reference
(phantom var accepted) but fails arity on Ipê (`WebSocketServerCfg` is declared
with 0 type args). Example 33 and all known callers never annotate the cfg type
directly, so this is annotation-only in practice.

*Rationale:* the phantom `msg` var is an artefact of the upstream Go TEA
architecture where `WebSocketServerCfg msg` was future-proofed for a Sub-based
WS subscription tier. Ipê's kernel-only module has no Sub-tier for the server-side
WS surface; a phantom var would widen the type to parametric with nothing to
unify against (a soundness hazard). Dropping it matches `IrType::Db`,
`IrType::StreamWriter`, and the other nullary opaque handles.
*Verified:* `crates/sky_ir/src/ir.rs` (`WebSocketServerCfg` variant, no type params);
`crates/sky_backend_rust/src/emit_types.rs` (`WsServerCfg<SkyError>` render).
*Sanctioned:* yes (`divergence:`).

### A19 — Emitted project targets Rust edition 2024
Every generated project (`Cargo.toml` `edition = "2024"`) compiles under the same
Rust edition as the compiler and runtime crates, rather than the reference's
Go output (which has no Rust-edition analogue). The emitted project vendors the
`sky_runtime` source tree verbatim (`build_emit_manifest` copies
`runtime/src/sky_runtime/` into `src/sky_runtime/`), so the emitted edition must
match the edition that source is written for: the runtime is edition 2024, and
its `db.rs` uses the 2024-only `expr_2021` macro-fragment specifier — vendoring
it into an edition-2021 project would be a parse error the moment a program uses
Db kernels (a seal violation: `ipe` accepts, `cargo build` fails). Pinning the
emitted edition to 2024 keeps vendored-source acceptance and downstream
`cargo build` structurally in agreement. *Rationale:* the seal — the emitted
project and the runtime source it embeds must share one edition; there is no
Go-parity oracle for a Rust edition, so this is a Rust-only property with no
reference behaviour to match. *Verified:* the four checked-in golden manifests
`tests/golden/{basics,mm_diamond,mm_local_pkg,multi_mod_split_pilot}/Cargo.toml`
(`edition = "2024"`), byte-compared against emitted output by
`crates/ipe/tests/support/mod.rs`. *Sanctioned:* yes (`divergence:`).

### A20 — Static-build allocator default: pure-Rust dlmalloc, not mimalloc
The reference's musl-static path defaults to the C `mimalloc` allocator
(`static_alloc = ["mimalloc"]` in its emitted manifest). Ipê's static path
defaults to pure-Rust `dlmalloc`, with `mimalloc` demoted to an explicit,
noticed opt-in (`--allocator mimalloc`) — the full trade study and AUTO table
live in `docs/architecture/static-compilation.md`. *Rationale:* security
principle #1 over efficiency #4 — the pure-Rust default removes the C
toolchain, `build.rs`, unsafe C-FFI boundary, and frozen vendored-C
supply-chain surface from every static build, while still clearing the musl
malloc throughput cliff; dlmalloc is Rust std's wasm allocator (one audited
allocator across static-native and wasm). The concurrent-churn delta versus
mimalloc on the Ipê runtime is not yet measured (the measure-before-finalize
bench sizes the opt-in recommendation; it does not decide the default — the
principle order does). Three reference warn-paths tighten into typed
refusals/gates for the same reason: unknown allocator names are a parse-time
rejection (closed enum, no string fallthrough), `system` malloc on musl needs
a two-key acknowledgment (`--allow-slow-allocator` / `allowSlowAllocator`)
instead of warn-and-proceed, and an unbuildable static request (macOS,
webview) is refused instead of silently degraded to a dynamic artifact.
*Verified:* `src/ipe-cli/src/build_plan.rs` + `src/ipe-cli/tests/static_emit.rs`
(refusals, AUTO rows, ldd-asserted musl e2e); the static examples sweep
(`IPE_SWEEP_STATIC=1`) and the `static` CI workflow keep both allocator arms
green. *Sanctioned:* yes (`divergence:`).

---

## 4. Stdlib / surface divergences

Surface-shape differences (several overlap the behavioral entries; listed here for
API-shape review):

- **`Bytes` conversion API** — because `Bytes` is a distinct `Vec<u8>` primitive
  (B2), Ipê exposes explicit `Bytes.fromString` / `Bytes.toString : Maybe String`
  where Ipê's `Bytes = String` alias needs none.
- **`Ipe.Jwt` call surface** — flat kernels vs the Go builder API (B9); token
  bytes identical, call surface differs until the builder API lands.
- **`Cmd` / `Sub` constructors** — present on Ipê, absent on the Go backend (B12).
- **`Ipe.Db` substrate** — `sqlx`/Rust vs SQLite/cgo/Go (B10); identical Ipê
  surface.
- **Front-end capability gaps (Ipê not-yet, reference-ahead)** — neutral coverage
  differences, each a tracked front-end item, none a principle divergence:
  - Bare `.field` accessor-as-function (no canon AST variant yet).
  - Refutable function-argument patterns (A12 — closes via desugar).
  - Nested constructor-payload patterns (A13).
  - Mutual / let-rec tail-call optimization is out of scope for the current TCO
    (self-recursion only).
- **`Task` error-channel scheme is monomorphic (`fail`/`mapError`/`onError`)**
  — Sky declares `fail : e -> Task e a` (`upstream:sky-stdlib/Sky/Core/Task.sky:51`),
  polymorphic in the error type. Ipê pins all three combinators to the
  concrete `Error` type: `fail : Error -> Task Error a`,
  `mapError : (Error -> Error) -> Task Error a -> Task Error a`,
  `onError : (Error -> Task Error a) -> Task Error a -> Task Error a`
  (`crates/sky_types/src/constrain.rs`, `K::TaskFail`/`K::TaskMapError`/
  `K::TaskOnError`; `crates/ipe/stdlib/Ipê/Core/Task.ipe:33`). Rationale: the
  Rust runtime's task error channel is monomorphic end-to-end — every emitted
  wrapper is `SkyTask<A> = SkyTask<SkyError, A>` (`SkyError` is a real 11-kind
  enum since backlog #85/#160, not a type alias), and the project's own house
  rules forbid `Task String a` in public surfaces. Before this pin,
  `K::TaskFail`'s scheme was `fun(var(1), task(var(0)))` — over-polymorphic
  relative to `task`'s implicit single fixed error slot — so `Task.fail
  "plain string"` HM-checked and then failed the emitted `cargo build` with
  E0308 (`expected SkyError, found String`): a "compilation successful, then
  `cargo build` fails" class violation. `mapError`/`onError` were already
  pinned; `fail`'s pin closes the family. Regression:
  `crates/ipe/tests/golden_m5b_db.rs::db_transaction`,
  `crates/ipe/tests/golden_m5a_task.rs::{error_channel,
  task_map_error_lambda}`, plus a negative `IPE-T0001` check-only test
  (`Task.fail "oops"` must be rejected). **Sanctioned:** yes — the polymorphic
  reading was unimplementable on this backend; it only ever produced
  ill-typed Rust, never a working program.

- **Numeric literals in `{{...}}` interpolation** — Ipê's interpolation
  mini-parser (`resolve_simple_interp_ref`) recognises an integer/float literal
  argument, e.g. `{{String.fromInt 54}}` lowers to `String.fromInt 54` and
  prints `54`. Ipê's `resolveInterpolationRef`
  (`Ipê/Canonicalise/Expression.hs`) has no literal case: a digit-leading body
  becomes `Can.VarLocal "54"`, which surfaces downstream as a naming error (the
  interpolation grammar there is names-only). Ipê's `constrain` treats an
  unresolved local as a violated invariant (IPE-I0001 ICE), so without this the
  same program ICE'd rather than compiling. A Ipê identifier can never start
  with a digit, so recognising the literal is unambiguous and strictly better
  (a well-typed program compiles instead of failing). **Sanctioned:** yes.
  Reference: `resolve.rs::resolve_simple_interp_ref`; regression
  `crates/ipe/tests/interp_literal.rs` + golden `m_interp_int_literal`.
  Found by the no-panic fuzzer (`multilineinterp` template).

- **`Money.add`/`sub`/`sumOf` return `Result Error Money`; comparisons ignore
  currency** — `Money.add`/`sub` return `Err` when the two values have
  different currencies; `Money.sumOf` returns `Err` if any list element has the
  wrong currency. This is a **sanctioned divergence** from upstream
  (`upstream:sky-stdlib/Std/Money.sky:304-317`), which returns the left operand
  unchanged on mismatch. The principled change makes invalid states
  unrepresentable: a currency mismatch is a typed `Err`, not a silently wrong
  money value. `compare`/`lt`/`lte`/`gt`/`gte` still compare amounts only and
  disregard currency — a separate follow-up. Call sites must handle the `Result`;
  convert values to a common currency first (via `Money.convert`) if needed.

---

## 5. README-liftable summary table

| Aspect | Ipê (reference) | Ipê | Why |
|---|---|---|---|
| Compiler host | Haskell, emits Go + Rust | Rust, emits Rust (`skyc`) | Single-language port |
| IR | AST → string emitters | Typed `sky_ir::Expr` (two-stage) | Malformed shapes unrepresentable |
| Tail-call jump | Stringly `__tco_jump__` sentinel; Rust backend has no TCO | Typed `TailRecur`/`TailLoop` IR → Rust `loop` | Constant stack; typed jump |
| Kernel dispatch | `(mod,name)` case; fail-open `toSnakeCase` default | Closed 424-variant `KernelFn`; fail-closed `IPE-L0108` | No exit-0-then-cargo-fail class |
| Type render | `_ -> "String"` fallback | `IrType → DResult`, closed, no default | Total by construction |
| Record alias | superset-widen or `"String"` | exact sorted-key, `CompilerBug` on miss | Soundness > completeness |
| Refutable arg pattern | synthesised `panic!` | fail-closed + desugar to `case` | No panic from well-typed code |
| `Bytes` | `type alias Bytes = String` | distinct `Vec<u8>` primitive | Rust `String` is UTF-8-constrained; lossless bytes |
| `Math.min`/`max` | `AsInt`-coerced compare | Elm polymorphic comparable | Elm-conformance |
| Case mapping | simple per-rune | full-Unicode `SpecialCasing` | Correctness; Unicode in core |
| `Money.allocate` (negative) | residue clamped at zero | residue distributed; shares sum to input | Fair split must sum to input |
| `Uuid.parse` | `Nothing` on canonical UUID (this shape) | `Just` on canonical, `Nothing` on malformed | Correctness |
| JWT | builder API | flat kernels (interim); **token bytes identical** | Interim surface; codec unchanged |
| `Ipe.Db` | Go + SQLite (cgo) | Rust + `sqlx` | Backend substrate |
| `Ipe.Ui` HTML | skeleton + `<style>` reset | compact inline CSS, no reset block | Separate renderer; byte-parity later |
| Runtime | shared fork baseline | strict superset (auth/jwt/SSRF/decimal/cache/env/telemetry hardening) | Security/correctness/soundness |
| Float sci-notation | exp ≥ 21 (reference Rust) | exp ≥ 6 (Go `%v` parity) — **confirmed vs Go 1.26.2 (#52)** | Ipê matches Go; the reference's Rust fork diverges |
| Static-build allocator | mimalloc (C) default | pure-Rust dlmalloc default; mimalloc = noticed opt-in; warn-paths → typed refusals | Security #1 > efficiency #4; C-free static path |
| Clone strategy (non-`Copy` bindings) | use-count ≥ 2 → clone ALL reads (including last) | true last-use: clone all-but-last owned reads, last moves; borrow reads exempt | Rust move semantics; N−1 clones vs N |
| As-pattern alias in match arm | drops alias name → E0425 on use (latent bug) | binds whole by move, reconstructs inner from clone | Correctness; reference latent bug fixed |
| Model admissibility | dynamic (Go reflects at runtime; no compile-time gate) | static `IPE-L0120` gate at `ipe` | Rust static trait bounds (seal) |
| Msg admissibility | dynamic (no compile-time gate) | static `IPE-L0121` gate (designed; pending impl) | Rust static trait bounds (seal) |
| App cfg argument | any expression (let-bound variable OK) | must be an inline record literal (`IPE-L0119`) | Backend reads fields at lower time; no runtime reflection |
| `Float` as `Set`/`Dict` key | accepted (Go `interface{}` comparison) | rejected `IPE-L0117` | `f64` lacks `Ord`/`Hash` in Rust |
| WS `sendBinaryToClient` arg type | `String` (Bytes alias) | `Vec<u8>` (distinct `Bytes` primitive) | B2 consequence; lossless binary frames |
| WS send semantics | blocks ~30 s on full write buffer | bounded `try_send`; `Err` on full queue (B19) | Bounded fail-fast; no handler-task pileup |
| WS Ping heartbeat | 30 s Ping + 10 s timeout | 30 s Ping (B20 closed #135); `IPE_WS_HEARTBEAT` override | Parity restored; axum auto-replies Pong |
| `WsServerCfg` type params | `WebSocketServerCfg msg` (phantom var) | nullary opaque — `WsServerCfg<SkyError>` (A18) | Sub-tier phantom not needed; nullary is sounder |

---

## Counts

- **Behavioral divergences:** 22 classes (B1–B23). B16 (#104 true
  last-use) and B17 (#99 alias bind) are pending fixture goldens. B3 RETIRED
  (task #55a) per inline note. B18–B20 are WS-server entries added with task
  #127. B20 CLOSED (#135) — Ping heartbeat ported. B21 is the #138
  total-resolution gate (unknown-type → IPE-N0002 not ICE). **B22
  RE-LANDED 2026-07-10** — the #90 ctor-payload-function lift was landed,
  reverted, re-landed, then reverted AGAIN the same day after a second
  independent review reproduced a THIRD seal violation in the curried-
  `andMap` gate (a bare/re-exported alias to `Maybe.andMap`/`Result.andMap`,
  e.g. `myAndMap = Result.andMap`, bypassed the AST-shape-keyed check
  entirely). The fourth attempt replaces the AST-shape approach with a
  two-tier design: a genuine type-level `TyBounds` obligation minted once
  per kernel reference (Tier 2, primary — survives arbitrary aliasing by
  construction) plus a lowering-time backstop re-anchored inside
  `lower_callee` itself, the actual single funnel every kernel/top-level
  reference resolves through (Tier 1). See B22's own entry above and
  `docs/architecture/ctor-payload-andmap-arity-gate-design.md` for the full
  design and incident history. B23 is class-1 inference fix #2 (Boundary
  Scheme Promotion,
  D1/D2/D3 under-acceptance), re-landed 2026-07-10 after a same-day revert;
  regression golden:
  `crates/ipe/tests/golden_class1_boundary_scheme_field_result.rs`.
  Sanctioned/recorded goldens: 43 carry a marker (authoritative count:
  `find tests/golden -name sanctioned.divergence | wc -l` — the
  hand-maintained per-family sub-counts drifted twice in one day and are
  retired; families span `Math`/`Bytes`/`Encoding`/`Jwt`/`Db` (incl.
  B-DbDecMoney's `db_decode_money`, backlog #34, and the
  `db_find_by_field` coverage golden)/`Ui`/`Cmd`/`Sub`/`Uuid`,
  Go-failure kind-1 shapes, Money/case/toFloat sanctioned entries, and the
  B22 ctor-payload-function set (`maybe_and_map_fn_payload`,
  `result_and_map_fn_payload`,
  `and_map_untyped_double_forwarder_arity1`). B23 is pure
  under-acceptance, not a Go-sanctioned divergence, so it adds no entries
  to this count. B16/B17 goldens pending.
- **Architectural divergences:** 18 (A1–A18). A8 and A13 are reference-ahead on
  completeness. A15–A17 are seal-gate entries. A18 is the WS phantom-`msg`
  type-var entry added with task #127.
- **Stdlib/surface divergences:** 5 API-shape (added `Task` error-channel
  monomorphism, class-7 fix 2026-07-10) + 4 front-end capability gaps +
  2 new gate-forced surface constraints (IPE-L0119, IPE-L0117).

## Could not confirm / verify

- ~~**B15 float sci-notation threshold.**~~ RESOLVED (task #52, commit `1903654`):
  probed Go 1.26.2 directly — `%v` cuts to scientific at exp ≥ 6, no exp-21
  behaviour; Ipê matches Go byte-for-byte. No longer an open item.
- ~~The reference `TypeRenderer.hs` `"String"` default and `ExprEmitter.hs`
  single-pass shape line-cites.~~ CONFIRMED against the reference tree at
  `src/Ipê/Generate/Rust/Builder/`:
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

- **IPE-L0121 (InadmissibleAppMsg) — PENDING IMPLEMENTATION.** The Msg-gate
  design is complete (`docs/architecture/seal-gates-msg-lambda-view-design.md §2`)
  and the diagnostic code constant is reserved in the design, but as of this pass
  `IPE_L0121` does not yet appear in `crates/sky_diagnostics/src/code.rs` (the
  file has 79 taxonomy codes; `IPE-L0121` is not among them). A15 captures the
  designed behaviour; mark as asserted-pending-impl until the code lands.

- **"Nominal (home, name) identity types #100" — NOT VERIFIED AS SKY DIVERGENCE.**
  Session memory referenced this as a divergence, but in-repo search finds no
  file or commit that records it as a Ipê-specific divergence. The principled-
  decisions-audit (#12) confirms Ipê already keys on `(home, name)` canonical
  naming — following `elm/compiler` — and the audit verdict is REJECT (already
  better). Sky's own Haskell compiler also uses name-qualified lookup internally,
  so this appears to be a PORT (convergence with elm/compiler), not a divergence
  from Ipê. If a specific Ipê-runtime divergence exists under this label, re-file
  with a concrete file:line cite.

- **Live.route non-String payload (#106) — PORT, not a divergence.**
  `routed-live-app-design.md` classifies `Live.route : String -> page ->
  LiveRoute` typing (#106) as "✅ done (Port — matches upstream Sky)." The latent
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
  runtime (opaque to the type checker); Ipê emits explicit `.parse::<i64>()`/
  `.parse::<f64>()`/`parse::<bool>()` expressions that decode at the call site.
  A malformed capture (e.g. `"abc"` for an `Int` slot) causes Ipê to route to
  `not_found` — the Go reference silently substitutes `0` via reflect-coercion.
  For payloads of any other type, the reference emits a String and relies on the
  Go runtime to coerce; Ipê rejects at compile time (to be upgraded to diagnostic
  code `IPE-L0123` — NOT `IPE-L0121`, which is owned by the #94
  `InadmissibleAppMsg` gate; see `docs/architecture/design-coherence-review.md`
  §C1). A route page builder that is neither a page constructor, an inline
  lambda, nor a named function is rejected at emit — pre-round-4 that arm
  silently emitted an untyped `(builder)(params)` call that cargo-failed
  (E0308/E0618) for every realistic shape.
- **Rationale:** parse, don't validate — a `:param` segment is inherently a URL
  string; feeding it to a constructor payload without an explicit decode is a
  type contract violation. The `Route.build` closure now returns `Option<Page>`
  so a decode failure falls through to `not_found` rather than silently landing
  on a zero-value page. `match_routes` calls the builder and treats `None` as a
  pattern-level miss — identical to how an arity mismatch is handled.
- **Sanctioned:** yes (`sanctioned:`). References: `emit_live.rs::route_param_get`,
  `live/route.rs::Route::build`, `live/route.rs::match_routes`.


### B-AnyCtorPayload — `any` ctor payload field → `Dict String String` (pub/sub wire carrier)
- **Differs:** Ipê's `any` wildcard as a union-constructor payload field (e.g.
  `| MessageReceived any`, `| CartTopicReceived any`) is carried at the Go
  runtime as a dynamic `interface{}` value — universally polymorphic, no static
  type constraint. The Rust backend cannot emit a `dyn Any` field (banned by the
  concrete-over-generic contract: no `dyn Any`/`.downcast`/type-erasure) and
  cannot emit an unconstrained Rust generic (the union's `Clone + Debug +
  PartialEq + Serialize + DeserializeOwned` derives must hold for every field).
  Ipê pins the `any` wildcard to `Dict String String` (`HashMap<String, String>`
  in the emitted Rust) — the sole concrete carrier that satisfies all derives and
  the `Broker` type parameter.  The pub/sub broker is typed per concrete payload
  (see A18-adjacent: `Broker<HashMap<String,String>>` for `any`-ctor programs);
  publisher and subscriber must agree on the concrete type at compile time.
- **Go-oracle relationship:** Go succeeds and carries the payload as `any` /
  `interface{}`; Ipê carries it as `Dict String String`.  For real-world pub/sub
  programs (examples 27 and 37) the publisher encodes a record into
  `payloadDict : Dict String String` and the subscriber decodes with
  `Db.getString`; the round-trip is semantically equivalent.  Programs that
  use the payload directly as a non-Dict type (e.g. passing it to
  `String.length`) are now rejected at type-check with IPE-T0001 — a
  correctness gain over Go's silent runtime failure.
- **Rationale:** concrete-over-generic contract + `Clone/Debug/PartialEq` seal.
  The `any` wildcard has exactly one concrete lowering in pub/sub payload
  position; `Dict String String` is that carrier.  A Rust generic would need the
  publisher and subscriber to agree on `TypeId` at runtime (silent non-delivery
  risk); `dyn Any` is a hard ban.
- **Sanctioned:** yes (`divergence:`). Reference: `constrain.rs::pin_any_in_ty`,
  `lower.rs::lower_enum` Gate 1, `lower.rs::ir_type_from_canon` Var arm.
  Regression: `golden_l0102_any_ctor_payload`.

### B-AuthClaims — `Ipe.Auth.signToken`/`verifyToken` claims pinned to `Dict String String`
- **Differs:** Go's `Auth.signToken : String -> a -> Int -> Result Error String`
  / `verifyToken : String -> String -> Result Error a` type the claims argument
  and return as a fully polymorphic `a` — any Go value marshals through
  `interface{}`/`encoding/json`. Ipê pins `a` to `Dict String String`
  (`HashMap<String, String>` in the emitted Rust).
- **Go-oracle relationship:** Go succeeds for any claims shape (record, map,
  scalar); Ipê accepts only a `Dict String String` claims argument (and returns
  the same on verify). A well-typed Ipê program passing a record literal or
  other non-Dict shape as claims is now REJECTED at type-check (IPE-T0001-class)
  instead of silently miscompiling — the AUD-06 seal fix this entry documents
  closed an exit-0-then-cargo-fail hole where `var(0)` unified with anything
  while the emitted runtime wrapper (`AUTH_WRAPPERS` in
  `sky_backend_rust::project`, `runtime/src/sky_runtime/auth.rs`) was already
  hard-pinned to `HashMap<String, String>` with no coercion inserted at
  lowering.
- **Rationale:** concrete-over-generic contract — `var(0)` here was never
  genuine polymorphism (no per-call-site re-instantiation is exercised; the
  runtime wrapper has exactly one concrete shape), so pinning it concretely at
  the type-scheme level is the correct fix, matching the same pattern as
  `B-AnyCtorPayload` above.
- **Sanctioned:** yes (`divergence:`). Reference:
  `constrain.rs::K::AuthSignToken` / `K::AuthVerifyToken` kernel schemes.

### B-ErrorToString — `errorToString : Stringify a => a -> String` (bounded polymorphic vs. universal)
- **Differs:** Ipê's Go runtime implements `errorToString` as `fmt.Sprintf("%v", v)`,
  which accepts any value at runtime (universally polymorphic, no type-level bound).
  Ipê types `errorToString` as `Stringify a => a -> String`, routing all
  stringification through the single `SkyStringify` trait chokepoint — the same
  chokepoint used by `Basics.toString`.
- **Go-oracle relationship:** Output is identical for all scalar, record, and ADT
  values. A future `Secret` newtype that omits `SkyStringify` would fail closed
  at type-check in Ipê but would be silently rendered by Go's `fmt.Sprintf`.
- **Rationale:** The bounded form is strictly sounder for typed-secrets safety.
  A type withheld from the `SkyStringify` impl set (e.g. a future opaque `Secret`)
  fails closed at type-check rather than reaching the runtime `fmt` fallback.
  This is a deliberate divergence in the direction of greater security.
- **Sanctioned:** yes (`sanctioned:`). Reference: `basics.rs::basics_error_to_string`.


### B-JwtDecode — `Jwt.decode` now matches reference: `Algorithm -> Int -> String -> Result Error String`
- **Converged:** The port's `Jwt.decode` previously diverged from the reference by
  dropping the `now : Int` parameter and delegating expiry validation to
  `jsonwebtoken`'s wall-clock `SystemTime::now()`. The reference
  (`sky-stdlib/Ipê/Core/Jwt.ipe`) declares `decode : Algorithm -> Int -> String ->
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
- **Reference:** `upstream:sky-stdlib/Std/Ui/Grid.sky` implements `tracks`/`columns`/`rows`
  as `AttrStyle "__gridTracks" (cols ++ "|" ++ rows)` — a pure-Ipê sentinel consumed
  by the reference renderer's `findGridTemplate` (Ui.ipe:2539) before raw-style emission.
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
  `runtime/src/sky_runtime/ui/render.rs`. Regression: `crates/ipe/tests/golden_stdui_grid_seal.rs`.


### B-ErrorADT — real `Error ErrorKind ErrorInfo` (backlog #85/#160)
- **Reference:** `upstream:sky-stdlib/Sky/Core/Error.sky` defines `Error = Error
  ErrorKind ErrorInfo` (11-variant `ErrorKind`, `ErrorInfo = { message, details :
  Maybe ErrorDetails }`, `ErrorDetails` a 5-variant union with `FfiPanic`/
  `TypeMismatch`/`HttpStatus`/`JsonDecode`/`Custom` payloads), constructed via Go
  runtime builders (`ErrIo`/`ErrNetwork`/etc, `runtime-go/rt/rt.go`).
- **Port:** Ported the full surface: `Error ErrorKind ErrorInfo` is a REAL,
  pattern-matchable ADT (closing the canon/lowerer ctor-scheme gap that was
  #160's blocker), backed by `sky_runtime::error::SkyError` (`SkyErrorKind`
  mirrors the reference's 11 kinds). `Error.toString`/`isRetryable`/
  `withMessage` all work end-to-end (E2E-verified: `crates/ipe/tests/
  golden_error_adt_roundtrip.rs`). **Backlog #85 follow-up (same session):**
  `ErrorInfo.details : Maybe ErrorDetails` + the 5-variant `ErrorDetails`
  union (`FfiPanic PanicInfo | TypeMismatch TypeInfo | HttpStatus Int |
  JsonDecode String | Custom String`) are now registered the same way as
  `ErrorKind` (canon ctor registration, `IrType::ErrorDetails` leaf,
  `builtin_runtime_enum`, constrain ctor schemes for all 5 variants).
  `Error.withDetails : ErrorDetails -> Error -> Error` is the sanctioned way
  to attach `ErrorDetails` to a live `Error` from Ipê source. **SEAL fix
  2026-07-11** (`docs/architecture/
  error-record-literal-seal-fix-2026-07-11.md`): `PanicInfo` / `TypeInfo` /
  `ErrorInfo` are NOMINAL opaque builtin types (backed by the runtime's
  `SkyPanicInfo`/`SkyTypeInfo`/`SkyErrorInfo` structs), not anonymous
  structural records. Raw record-literal construction is a LOUD `ipe`-time
  IPE-T0001 rejection — before the fix it was silently accepted and the
  emitted project failed `cargo build` (E0308: the literal lowered to a
  project-local synthesized struct), an exit-0-then-cargo-fail that was
  temporarily mis-filed here as a sanctioned divergence. Field access on the
  three types (`p.message`/`p.stack`/`t.expected`/`t.actual`/`info.message`/
  `info.details`) resolves via fixed builtin field tables (`sky_types`'
  `ErrorRecordFields`, the same recipe as the opaque server `Request`), the
  names are annotatable (`describePanic : PanicInfo -> String`), and a
  pattern-bound payload agrees with every use site on one Rust type.
  E2E-verified for 3 of the 5 variants
  (`HttpStatus`/`JsonDecode`/`Custom` — the non-record-payload ones)
  round-tripping through `ErrorInfo.details`, plus exhaustive compile-time
  coverage of all 5 (`FfiPanic`/`TypeMismatch` as unreached `case` arms):
  `crates/ipe/tests/golden_error_details_roundtrip.rs`; nominal-payload
  coherence: `crates/ipe/tests/golden_error_nominal_payload.rs`; the
  record-literal rejections: `crates/ipe/tests/
  error_record_literal_gates.rs`.
- **Sanctioned:** yes (matches the reference design exactly; construction
  goes through the smart constructors + `withDetails`, and the former
  record-literal codegen gap is CLOSED as a compile-time rejection, no
  longer a divergence). Reference:
  `crates/sky_types/src/constrain.rs` (`Error`/`ErrorKind`/`ErrorDetails`
  ctor schemes + the nominal payload Cons), `crates/sky_lower/src/lower.rs`
  (`IrType::Error`/`IrType::ErrorKind`/`IrType::ErrorDetails`/
  `IrType::ErrorInfo`/`IrType::PanicInfo`/`IrType::TypeInfo`),
  `crates/sky_backend_rust/src/lib.rs::builtin_runtime_enum`,
  `runtime/src/sky_runtime/error.rs`. Every project's generated `SkyError`
  alias (`tests/golden/*/main.rs`'s boilerplate slice, `sky_backend_rust::
  project::runtime_bindings`) flipped atomically from `type SkyError =
  String` to `pub use sky_runtime::error::SkyError` in the same change — the
  "69-golden flip" #85 had originally deferred this work for.

### B-SqlFragment — `Ipe.Db.Sql` typed WHERE-fragment builder; `Db.unsafeFindWhere` removed (backlog #61)
- **Reference:** the Go backend's `Db.unsafeFindWhere : Db -> String -> String
  -> List String -> Task Error (List Row)` accepts a raw, hand-built WHERE
  clause string with `?`-parameterized args — the values are bind-safe, but
  the WHERE clause's STRUCTURE (column names, operators, boolean composition)
  is caller-authored text with no typed guard against a mistaken
  string-interpolated fragment sneaking in alongside the intentional
  parameterization.
- **Ipê design:** a new opaque `SqlFragment` type (`Ipe.Db.Sql`, qualifier
  `Sql`) whose ONLY constructors are typed combinators — `column`, `param`
  (plus the `int`/`string`/`float`/`bool` sugar), `eq`/`ne`/`gt`/`lt`/`gte`/
  `lte`, `and`/`or`/`not`, `isNull`/`isNotNull`, `inList`, `like`. Every
  combinator unconditionally parenthesizes its output and merges binds, so
  precedence bugs are unrepresentable and a `SqlFragment`'s `sql` text is
  always `?`-placeholder text with a matching `binds` list. `Db.findWhere` /
  `Db.deleteWhere` take `SqlFragment`, not `String` — a naive
  string-concatenated WHERE clause is now a `ipe` compile-time `IPE-T0001`
  type mismatch, never a runtime value. `Db.unsafeFindWhere` (and its runtime
  `db_unsafe_find_where`) is REMOVED, not deprecated — the security-tier
  no-deferral rule (`AGENTS.md` / `PRINCIPLES.md`) treats "keep the raw-SQL
  escape hatch a brand-new safe API supersedes, at zero migration cost" as a
  forbidden shipping excuse. Zero fixtures called `unsafeFindWhere` outside
  its own golden, which was rewritten to `Db.findWhere`.
- **Sanctioned:** yes — strictly-better-security class (parse-don't-validate
  applied to SQL WHERE-clause construction); no Go counterpart exists for
  `Sql.*` / `Db.findWhere` / `Db.deleteWhere` (`oracle_divergence = true` on
  every new golden). `SqlFragment` is `Clone + PartialEq` (derivable) but
  deliberately NOT `serde` (never persisted to a Live session store); its
  hand-written `Debug` shows SQL text + bind COUNT only, never bind VALUES.
  Reference: `runtime/src/sky_runtime/db.rs` (`SqlFragment` + `sql_*`
  combinators + `db_find_where`/`db_delete_where`), `crates/sky_kernels/src/
  lib.rs` (`SqlColumn..DbDeleteWhere` tail-appended kernels),
  `crates/sky_types/src/constrain.rs` (`stdlib_scheme` `FIRST_SCHEMED`
  entries), `crates/ipe/tests/golden_m5b_db.rs` (`db_find_where` /
  `db_delete_where` / `db_sql_combinators`) + `golden_m5b_db_gates.rs`
  (`db_findwhere_string_is_t0001` — the negative "parse, don't validate"
  proof). Spec: `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md`.

### B-Secret — `Ipe.Secret` opaque secret-string type; `Auth.signToken`/`verifyToken` re-typed (backlog #44)
- **Reference:** the Go backend's `Ipe.Auth.signToken` / `verifyToken` take a
  plain `String` signing key, and every other Go/Haskell surface handles API
  keys / passwords / tokens as bare `String` — no typed boundary stops one
  from landing in a log line, a `fmt.Sprintf("%v", …)`, or an error message.
- **Ipê design:** a new opaque `Secret` type (`Ipe.Secret`, 3 kernels:
  `fromString` — the seal; `reveal` — the single greppable un-parse;
  `redacted` — the explicit `"<redacted>"` accessor). `Auth.signToken` /
  `Auth.verifyToken`'s signing-key argument is re-typed `String -> Secret`
  (zero migration cost — no fixture called either kernel before this change).
  `==` and `toString`/string-interpolation/`Log.*With` stringification are
  ALL **allowed** on `Secret` and are **safe by construction**: `Secret` has
  exactly one hand-written `PartialEq` (constant-time, via
  `subtle::ConstantTimeEq`) and exactly one hand-written `SkyStringify` /
  `Debug` (both ALWAYS render the fixed `"<redacted>"` placeholder,
  regardless of the wrapped value) — there is no OTHER impl a caller could
  reach instead, so no `ty_is_equatable`/`has_show` denylist is needed at
  all. `Secret` has NO `Display`, NO `Hash`, NO `Ord`, NO `serde`: a bare
  `Basics.toString`/`Debug.toString` call and any `Dict`-key/`Set`-element/
  ordering (`<`/`>`) use are Rust-or-Ipê-level compile errors, never a
  runtime concern (Dict-key/ordering rejection needs zero new type-checker
  code — `Secret` is a bare `Ty::Con` outside the 4-5-scalar
  `comparable`/`Ord` allowlist already). The backing buffer is zeroized on
  `Drop` (`zeroize::Zeroize`), shipped in the same change as the type itself
  (security-tier hardening is pre-push, never deferred).
- **Sanctioned:** yes — strictly-better-security class ("secrets are typed,
  never `fmt`-stringified", `PRINCIPLES.md`'s Security & soundness
  enforcement section); no Go counterpart type exists (`oracle_divergence =
  true` on every new golden). `Secret` is `Clone + PartialEq` (derivable) but
  deliberately NOT `serde` — this is ALSO the mechanism that makes a
  `Ipe.Live` Model field of type `Secret` a compile-time `IPE-L0120`
  (never a session-store leak): `ir_type_is_serde(Secret) = false` gates the
  Live Model exactly like it gates `SqlFragment`. A record containing a
  `Secret` field stays fully `Clone`/`Debug`/`==` (the #45/#70
  derive-blast-radius class `SqlFragment` already closed — marking a leaf
  merely non-serde, never non-derivable). Reference:
  `runtime/src/sky_runtime/secret.rs` (`Secret` + `secret_from_string` /
  `secret_reveal` / `secret_redacted`), `crates/sky_kernels/src/lib.rs`
  (`SecretFromString`/`SecretReveal`/`SecretRedacted` tail-appended kernels),
  `crates/sky_types/src/constrain.rs` (`stdlib_scheme` `FIRST_SCHEMED`
  entries + the `AuthSignToken`/`AuthVerifyToken` re-typing),
  `crates/sky_backend_rust/src/project.rs` (`AUTH_WRAPPERS` reveals the
  `Secret` at the Ipê-facing boundary before delegating to the runtime's
  unchanged `String`-typed `auth_sign_token`/`auth_verify_token`),
  `crates/ipe/tests/golden_secret.rs` (seal/reveal/redacted round trip,
  `==` match/mismatch/length-mismatch, record-containing-Secret,
  Log.infoWith redaction, Auth sign/verify round trip) +
  `crates/ipe/tests/secret_gates.rs` (`secret_concat_is_rejected` — the
  negative "parse, don't validate" proof) +
  `crates/ipe/tests/model_admissibility.rs`
  (`live_model_with_secret_field_is_rejected`). Spec:
  `docs/architecture/class6-secret-sqlfragment-fix-spec-2026-07-09.md`.
  **Out of scope, explicitly deferred** (per the spec's own filed-follow-ups
  list, not a gap introduced here): a `System.getenvSecret` construction
  convenience (`Task.map Secret.fromString (System.getenv …)` is the v1
  pattern); a committed-secret-literal lint; `Secret`-accepting `Log.*`/
  `Trace.*` overloads (rejected as a design, not deferred — normalizes
  routing secrets toward logging; `Secret.redacted`/the automatic
  `SkyStringify` redaction already cover the use case); the WASM
  `HydrationState` field-type containment gate (the client-WASM target and its
  three-layer effect gate are recorded in `docs/adr/0042-wasm-client-target.md`)
  — `Secret`'s `ir_type_is_serde = false` classification IS the predicate a
  hydration-state field-type gate consults.

### B-DbDecMoney — `Db.Decode.money` returns `Decoder (Decimal, String)`, not `Decoder Money` (backlog #34)

- **Reference:** the Go backend's `DbDec_money` (`upstream:runtime-go/rt/
  db_decoder.go:202-244`) returns a full `Decoder Money`, constructing the
  Ipê `Money` ADT directly at the runtime layer (including resolving the
  3-letter ISO code to a `Currency` ADT variant via a hand-rolled
  `sqlCodeToCurrency` switch) — Go's runtime is dynamically-typed
  (`any`-based `SkyADT`), so it can construct an arbitrary user ADT value at
  runtime with no compile-time dependency on the project's generated types.
- **Ipê design:** `db_decode_money` (`runtime/src/sky_runtime/db.rs`) returns
  the structural pair `Decoder (Decimal, String)` instead. `Money`/`Currency`
  are project-generated Rust types (`StdMoneyMoney`/`StdMoneyCurrency`,
  named per the project's module prefix) unnameable from the shared
  `sky-runtime-rust` crate — there is no equivalent to Go's `any`-typed
  runtime ADT construction available here. `Db.Decode.money "col"` decodes
  the `"ISO_CODE AMOUNT"` TEXT column `SqlMoney` writes on INSERT (v0.16.26
  lossless serialisation) back into its `(amount, currency_code)` pair;
  callers compose `Decode.map (\(amount, code) -> …) (Decode.money "col")`
  to build a `Money` value at the call site.
- **Sanctioned:** yes, tagged `divergence` (not `sanctioned`) — a real
  API-shape difference, not a strictly-better-security/correctness class;
  Go's `Decoder Money` shape cannot be replicated without a
  `Currency`-construction codegen wrapper that reimplements Go's 50+-code
  `sqlCodeToCurrency` table (or calls into a Ipê-level
  `Ipe.Money.parseCurrency`-equivalent) — filed as a separate follow-up, not
  bundled into this mechanical kernel-registration fix. Before this fix,
  `db_decode_money` was fully implemented and unit-tested in the runtime
  (`test_db_decode_money_roundtrip`) but had NO `StdlibKernel` variant, NO
  constrain scheme, and NO lower/emit arm — unreachable from Ipê source
  (`ipe-index parity --gaps` flagged `DbDec.money go=1 rust=0`). Reference:
  `crates/sky_canon/src/env.rs` (`Db.Decode` allowlist), `crates/sky_kernels/
  src/lib.rs` (`DbDecMoney` decl + `is_db()` classification),
  `crates/sky_types/src/constrain.rs` (`K::DbDecMoney` scheme +
  exhaustiveness list), `crates/sky_lower/src/lower.rs` (arity-1 dispatch +
  callee resolution), `crates/sky_backend_rust/src/{naming,emit_expr}.rs`,
  `crates/sky_ir/src/pretty.rs`, `crates/ipe/tests/golden_m5b_db.rs`
  (`db_decode_money`). Spec:
  `docs/architecture/class7-sql-db-fix-spec-2026-07-09.md` §6.

---

### B-FfiKernelAliasSealed — `Ffi.kernel "Name"` alias resolves in the compiled path, fail-closed on an unregistered kernel (backlog #196)

- **Reference:** the Haskell backend compiles a compiled-source stdlib module's
  point-free binding `f = Ffi.kernel "Module_function"` and rewrites `f`'s call
  sites to the kernel at LOWERING (`upstream:src/Sky/Build/Compile.hs`
  `collectKernelAliases` → `_lc_kernelAlias`), typing the alias against the
  binding's own Ipê annotation. An `Ffi.kernel` string that names no kernel is
  not structurally rejected at that boundary — Go's dynamically-typed runtime
  routes it to the `ffi_kernel_polyfill` (`Kernel.hs:780`), which can panic at
  run time.
- **Ipê design:** the split-at-first-`_` `(Module, function)` pair is resolved
  against the closed kernel registry (`Env::stdlib_index`) at **canonicalisation**
  (`crates/sky_canon/src/resolve.rs`, `detect_kernel_alias`). A binding whose
  body is exactly `Ffi.kernel "Module_function"` and whose pair IS registered is
  registered as a `VarHome::Kernel` (so every in-module `f` and cross-module
  `Alias.f` reference lowers straight to the kernel dispatch); its def body emits
  no top-level function. Cross-module resolution flows through a new
  `ModuleExports.kernel_aliases` map.
- **Fail-closed (SEAL):** a pair the registry does NOT cover — or a malformed
  string with no usable `_` split — is rejected at compile time with
  `NameError::UnknownKernelAlias` (**IPE-N0028**), never emitted as a call to a
  non-existent kernel that would type-check in `ipe` yet fail the downstream
  `cargo build`. This is the "make invalid states unrepresentable" rule applied
  to the kernel-alias path: `ipe` acceptance is a structural proof the kernel
  exists. Regression: `crates/ipe/tests/golden_ffi_kernel_alias_seal.rs`
  (unknown → IPE-N0028; malformed → IPE-N0028; registered `String_toUpper` →
  ipe-0 AND cargo-0).
- **Layered fail-closed for arity/lowering gaps:** because Ipê types the alias's
  *body* via the kernel's HM scheme (not a flexible var — a flexible var would be
  the exact exit-0-then-cargo-fail hole the SEAL forbids), an alias whose declared
  annotation arity differs from the kernel's scheme is rejected with **IPE-T0001**
  at type-check, and a kernel with no lowering arm is rejected with **IPE-L0108**
  at lowering. Both are clean `ipe`-time rejections — no cargo-fail. Consequence:
  some upstream compiled-source modules stay kernel-blocked on Ipê until their
  Rust kernels (and, where the annotation diverges from the kernel scheme, the
  matching lowering) exist:
  - **Registry-blocked (no `StdlibKernel` variant, IPE-N0028):** `Ipe.Trace`,
    `Ipe.Cache`, `Ipe.Csv`, `Ipe.Email`, `Ipe.Compression`, `Ipe.Config`,
    `Ipe.WebSocket` (incl. its `Sub_subscribeWebSocket`).
  - **Lowering-blocked (kernel in the registry but no lower/emit arm, IPE-L0108
    / emit `CompilerBug`):** *(none — backlog #215 resolved `Ipe.PubSub`: it now
    emits `pubsub_publish::<_, SkyError>(topic, payload)` with scheme
    `String -> a -> Task Error Int`; ipe-0 AND cargo-0 guaranteed.  The payload
    `a` is a genuine monomorphized type var, never erased.)*
  - **Arity-blocked (IPE-T0001):** `Ipe.Pure`'s internal `uuidV4Kernel` /
    `uuidV7Kernel` helpers annotate an arity-0 `Task Error String` value over the
    arity-1 `Uuid_v4`/`Uuid_v7` kernels (`() -> Task Error String`). Go's
    `func() any` runtime boundary absorbs the arity difference; Rust's
    monomorphized `Box<dyn Fn(()) -> …>` cannot, so the shape is rejected until an
    arity-0-alias-of-nullary-effect-kernel lowering is built.
  These are documented completeness gaps (PRINCIPLES §5), not silent workarounds.
- **Sanctioned:** yes, tagged `sanctioned` — Ipê is deliberately more sound: the
  closed registry turns a Go-runtime-panic-class (unknown kernel routed to a
  polyfill) into a compile-time rejection. Reference: `crates/sky_canon/src/
  resolve.rs` (`detect_kernel_alias`, `KernelAlias`, in-module + dep injection),
  `crates/sky_canon/src/lib.rs` (`ModuleExports.kernel_aliases`,
  `ExportedKernelAlias`), `crates/sky_diagnostics/src/{code,diagnostic,render}.rs`
  + `explain/IPE-N0028.md`.

### B-UiEventsFnArg — `Ipe.Ui.Events.onSubmit`/`onInput` take a handler function, not a bare Msg

- **Reference:** upstream `Ipe.Ui.Events` (`upstream:sky-stdlib/Std/Ui/Events.sky`)
  re-exports `onSubmit : a -> Attribute b` and `onInput : msg -> Attribute msg`
  — the arg is a bare value, matching upstream `Ipe.Ui`'s permissive event
  kernels.
- **Ipê design:** the Rust `Ui.onSubmit` kernel is `(a -> msg) -> Attribute msg`
  and `Ui.onInput` is `(String -> msg) -> Attribute msg` (function-arg handlers
  — `crates/sky_types/src/constrain.rs`, `K::UiOnSubmit`/`K::UiOnInput`). The
  `Ipe.Ui.Events` re-exports mirror those kernel schemes
  (`onSubmit : (a -> msg) -> Ui.Attribute msg`,
  `onInput : (String -> msg) -> Ui.Attribute msg`); the upstream bare-value
  signatures would not type-check against the Ipê kernel. `onClick` is unchanged
  (`msg -> Attribute msg` on both). Resolves the module fully (ipe-0 AND
  cargo-0). Reference: `crates/ipe/stdlib/Std/Ui/Events.ipe`.
- **Sanctioned:** yes, tagged `divergence` — an API-shape difference emergent
  from Ipê's function-arg event-kernel schemes; the re-export must match the
  kernel it forwards to.

### B-Keyed — `Ipe.Ui.Keyed`: key attached as `sky-key` DOM attribute (stamp approach)

- **Differs:** `Keyed.column` / `Keyed.row` attach the supplied key as a
  `sky-key` HTML attribute on each child element (or wrap `Text`/`Empty`/`Raw`
  children in a keyed `el`). The sky-id stamper (`assign_sky_ids_depth` /
  `ipe_id_key` in `html.rs`) reads that attribute to derive a stable `sky-id`
  for the child regardless of its list position. The Go reference runtime uses
  VNode-level key tracking inside the diff/patch layer — the key is a first-class
  field on the VNode struct, never a DOM attribute.
- **Go-oracle relationship:** rendered HTML byte output is identical for any
  single render (both produce the same element tree). On subsequent *patch*
  renders, the Go reference diff uses the key to pair old and new children and
  issue minimal DOM moves; Ipê v1 uses positional sky-ids but the `sky-key`
  attribute lets a future diff upgrade adopt stable identity without an API
  change.
- **Rationale:** Ipê v1 ships no VNode struct — the render surface works with
  plain `Element<M>` trees serialised as HTML strings. Carrying the key as a
  `sky-key` DOM attribute costs zero abstraction overhead and keeps the public
  API (key ≠ discarded) correct. A VNode-level key differ is a v2 upgrade,
  not an API change.
- **Sanctioned:** yes, tagged `divergence`. Reference: `ui/keyed.rs::attach_key`,
  `ui/keyed.rs::keyed_column_`, `ui/keyed.rs::keyed_row_`.

### B-WS-TLS — `Ipe.WebSocket` client: rustls backend for `wss://` (no native-TLS)

- **Differs:** `WebSocket.connect` / `WebSocket.connectWith` dial `wss://` URLs
  using the rustls backend (`tokio-tungstenite` feature
  `rustls-tls-webpki-roots`) rather than the platform native-TLS stack (OpenSSL
  / Schannel / SecureTransport). The Go reference runtime uses Go's
  `crypto/tls`, which routes to the platform TLS stack on each OS.
- **Go-oracle relationship:** observable wire output is byte-identical for a
  successful `wss://` handshake to a public CA-signed endpoint. Certificate
  validation policy differs for self-signed or private-CA certs: rustls enforces
  WebPKI trust roots (no `InsecureSkipVerify`); Go trusts the OS cert store and
  accepts `InsecureSkipVerify = true`.
- **Rationale:** Ipê already uses rustls for `reqwest` (HTTP client), `sqlx`
  (database), and `lettre` (email). A uniform TLS backend means one root-store,
  one audit surface, and no OpenSSL linking — aligned with the no-native-deps
  goal of the runtime. The `native-tls` feature of `tokio-tungstenite` is
  deliberately NOT enabled. When `IPE_HTTP_DENY_PRIVATE` (SSRF guard) is active,
  `wss://` is refused in the pinned-dial arm because a raw TCP socket carries no
  TLS context; the user must disable the guard or use `ws://` on that endpoint.
- **Sanctioned:** yes, tagged `divergence`. Reference: `ws_client.rs::do_connect`,
  `Cargo.toml` (`tokio-tungstenite` features).

---

### B-FfiAsyncBridge — async-FFI wrapper hardening: JoinError redaction funnel, abort-on-drop cancel guard, process-global runtime

Three upgrades over the reference's async wrapper body
(`upstream:src/Sky/Build/Rust/Ffi.hs`, the `Box::pin(async move {
tokio::task::spawn(…).await })` three-arm match):

- **JoinError through the redaction funnel.** The reference's panic arm emits
  a bare `str_err("foreign async call panicked")` — no correlation id, no
  server-side detail. Ipê routes the `JoinError` through
  `ipe_error_from_foreign` (same funnel as every foreign `Err(e)`): the raw
  `Debug` — which carries the panic payload — is logged server-side under a
  fresh correlation id, and Ipê observes the generic
  `external operation failed (ref <id>)` message. Strictly better on operator
  traceability with the same secret-redaction posture. The sync-fallible arm
  likewise funnels `Err(e)` through `ipe_error_from_foreign` where the
  reference still embeds `format!("{:?}", e)` verbatim (SDK errors echo URLs /
  bearer tokens / API keys in their `Debug` output — a secret channel).
- **Abort-on-drop cancel guard.** The reference's inner spawned task detaches
  when the outer wrapper future is dropped (`Task.parallel` early-cancel),
  leaking side effects after failure. Ipê arms an `AbortOnDrop` guard
  (runtime `task.rs`) around the spawn and defuses it after a normal join, so
  a cancelled foreign call is aborted, preserving the no-side-effect-after-
  failure contract. Regression:
  `src/runtime/rust/tests/ffi_async_bridge.rs`.
- **Process-global tokio runtime.** Both runtimes historically built a fresh
  `Runtime::new()` per `block_on`, so a reactor-registered handle (FFI
  client, listener) constructed in one entry died with that entry's reactor.
  Ipê drives every `block_on` on one `OnceLock`-held global runtime
  (`task.rs::global_runtime`); `block_on_current_thread` (the webview
  main-thread driver) is unchanged. Behavior-compatible — a shared reactor is
  strictly more available than a fresh one. Regression:
  `ffi_async_bridge.rs::reactor_handle_survives_across_two_block_on_entries`.
- **Sanctioned:** yes, tagged `divergence`. Reference:
  `src/compiler/ffi/src/bindings.rs` (async arms),
  `src/compiler/ffi/src/instance.rs` (generic async arm),
  `src/runtime/rust/src/task.rs` (`GLOBAL_RUNTIME`, `AbortOnDrop`).

---

<a id="planned-future-divergences"></a>
## 6. Planned future divergences (filed, not yet implemented)

Intentional departures from the reference language, filed and (where noted)
designed, sequenced for the post-completion program.
Governing rules: every divergence here, if/when adopted, becomes a documented
ledger entry above (a divergence is *documented*, never silent) and flips the
relevant parity row from "mirrors Go" → "intentional design + rationale + own
tests". Until then Ipê still **mirrors** upstream behaviour. **Divergences go
last, on a verified-complete base** (rule filed 2026-06-28): a grammar
superset can't be checked against the Go oracle (its parser rejects the new
form), so adding one early would muddy every parity sweep.

### 6.1 Hot-reload / Ui-as-IR / standalone TEA (research, post-core)

Three runtime-side directions that compose into one vision — "edit code → UI
updates live, render any UI to any target" (the graphical-and-programmatic
site-builder):

- **Hotloading a running app** (preserve `Model` state across code change).
  Key fact: **salsa ≠ hotloading** — salsa only makes the *rebuild* fast;
  getting new code into a live process is a separate runtime mechanism.
  Mechanisms ranked by principle-fit: (1) dev-mode IR interpreter (safe, no
  `unsafe`, reuses the IR — best fit); (2) WASM module reload (sandboxed);
  (3) native dylib reload — **discouraged** (`dlopen` + fn-pointer transmute
  violates the no-`unsafe` spirit); (4) baseline: fast watch-rebuild →
  restart → session-store persist → SSE reconnect. Open: `Model`-shape
  migration on type changes; interpreter eval path must stay sandboxed.
- **`Ipe.Ui` as a backend-agnostic UI IR** — target chosen by a function call
  (`Ui.toHtml : Element -> String`, `Ui.toAnsi : Element -> AnsiBuffer`),
  decoupling *what UI* from *how rendered*. Pure functions — excellent
  principle fit. **Security is load-bearing:** HTML *generation* must keep
  the HTML-escaping / no-eval / no-XSS guarantees even when
  producing-not-serving; ANSI generation must sanitize control bytes.
- **Standalone TEA engine** — make the TEA runtime a backend-agnostic engine;
  Live / TUI / Webview become transports/drivers plugged into it. Enables the
  hot-reload host (which owns the `Model` and swaps `update`/`view`).

All three are research/deferred until the mirror/parity machinery is proven;
each then becomes a real ledger entry with its own tests.

### 6.2 Deep nested-record-update sugar `{ r | a.b.c = v }`

Elm has no deep-update syntax; this is a deliberate grammar superset.
**Static path only** — the LHS must be a literal field chain (no computed or
conditional paths). **Desugars in canon** into the existing nested
`update`+`access` form, so types/lower/backend see only supported primitives
— zero new IR, zero new codegen; runtime behaviour is identical to the manual
form, the only divergence is the accepted grammar. Filed 2026-06-28
(user-approved); post-M6 per the divergences-go-last rule.

### 6.3 Or-patterns (alternative patterns in one case arm)

`A | B -> arm` — one arm matching several patterns. **Syntax decided
2026-06-28: `|` (NOT `||`)** — `|` already means "one of these" via ADT sums,
matches Rust/OCaml/F#/Python, and maps 1:1 to Rust's native or-pattern; `||`
would overload one token with two structurally different meanings.
**Correctness gate (load-bearing):** every alternative MUST bind the same set
of variables at the same types — reject mismatched-binding or-patterns
fail-fast in canon/types (before Rust would). Exhaustiveness: the Maranget
check expands `p|q` into two rows (the algorithm already supports this).
Filed 2026-06-28 (user-approved); post-M6.

### 6.4 Pattern guards

`pattern if cond -> body`. **Syntax decided 2026-06-28: `if` (NOT Haskell's
`|`)** — `|` is taken by or-patterns (6.3); Rust's spelling composes with
them (`A | B if cond -> …`, guard applies to the whole or-pattern) and maps
1:1 to Rust match guards. **Soundness floor (load-bearing):** a guarded arm
does NOT contribute to exhaustiveness (the guard may be false) — the Maranget
check must treat guarded rows as non-covering, requiring an unguarded/
wildcard fallback else **IPE-T0010**, caught BEFORE emit (Rust would
otherwise reject the guard-only match as E0004 = exit-0-then-cargo-fail).
Guards also affect redundancy (a guard can make a shadowed later arm
reachable). Implement together with 6.3. Filed 2026-06-28 (user-approved);
post-M6.

### 6.5 Effect `do` block (effect-sequencing sugar)

The nested-`Task.andThen`-lambda pyramid gets a scoped effect block. A long
spelling debate (Gleam `use`, `let x <- e`, `Task.chain`, free-floating Roc
`!`, `Task.block`-fenced `!`) converged on the outcome recorded in the design
doc: effect visibility is a first-class criterion (the block boundary marks
the effect REGION, a per-line marker marks each effect), bind is built-in for
the fixed effect types only (no user-facing Monad class), a bare effect line
= run/discard (kills the `let _ = TaskExpr` auto-force wart). Shipped: `do`
and `parallelDo` desugar in the parser to `Task.andThen` / `Task.parallel` —
decision recorded in `docs/adr/0050-do-block-task-sequencing-sugar.md`.

### 6.6 Record field-punning on construction

`{ name, age, email }` ⇒ `{ name = name, age = age, email = email }` — dual
of the record-pattern punning already supported. Pure **canon-level desugar**
resolving each bare field to the in-scope local of the same name (error if
none); zero new IR / codegen / runtime. Filed 2026-06-28; post-M6, low
effort, low risk.

### 6.7 Time-travel debugger for live apps (dev-only)

Elm-style debugger for TEA/live apps (Live + Webview + Tui): record the Msg
history, step back/forward, inspect the Model at each step, import/export
sessions. **Dev mode only** (off in production — no overhead, no surface).
Future: simulate Model-value edits + replay forward, Msg injection,
time-scrubbing, adjacent-state diff. Filed 2026-06-29; post-M6 (needs the
live runtime); same dev-loop family as 6.1.

### 6.8 Language-level ADT⇄string ergonomics (undecided route)

A sum type used as an option set (`type Theme = Light | Dark | Purple`)
almost always needs a hand-written `toString`/`fromString` pair that drifts.
Two candidate routes, **undecided**: (1) **LSP code-action** generating both
functions + a round-trip property test from the variant list — zero language
magic, ordinary visible code; (2) **syntax-SSOT** — declare the wire name
alongside each variant (`type Theme = Light "light" | …`) and derive both
functions. The "is this magic good?" judgement is the crux — lean LSP-route
if the magic proves surprising. Filed 2026-06-30; brainstorm post-core.

### 6.9 CI patch queue over the upstream examples corpus — ACCEPTED

Every surface departure (dropping `Task.run`/`Task.perform` #128, margin
stripping #133, the rename) breaks the pristine upstream examples corpus —
exactly the regression net we most need. **Accepted 2026-07-05; execute at
Tier-3 start** (first consumer is #128/#133; wire `--patched` mode into the
sweep + CI with #37). Keep examples byte-identical to upstream and carry an
in-repo patch queue (`tests/example-patches/…`) that CI applies before
build/run/perf. Patch-apply failure is a feature (fires exactly when upstream
changed the lines we diverge on). Go-only examples get adaptation patches,
widening the corpus. **Oracle policy per patch class:** output-neutral
departures keep byte-equivalence against the Go oracle running the UNpatched
source; output-changing departures record an `oracle_divergence + reason`
(carried in the patch-file header) per the sanctioned-divergence policy.
**Codemod synergy:** mechanical departures ship with a `ipe fix`
auto-migration; CI can generate the patches by running the migrator over
pristine sources — the queue doubles as an end-to-end test of the user-facing
migration tool.

### 6.10 `ipe lint` — source-level lint tool (design complete)

The reference ships no lint subsystem — no lint pass, no lint CLI command, no
per-site suppression (its `Ipê/Lsp/Diag.hs` republishes compiler diagnostics
only). Ipê adds `ipe lint`, an elm-review/clippy-class static-analysis tool
over the compiler's own artifacts (parse AST + canon AST + `SolvedTypes` —
never a second analyzer): visitor-schema rules in a new `sky_lint` crate,
dual `IPE-W####`+kebab-name identity, `allow`/`warn`/`deny` levels via
`sky.toml [lint]`, per-site `-- @allow(<rule>) <reason>` directives with a
mandatory reason (an unused directive is itself a lint), autofix via the
existing `Suggestion`/`Applicability` machinery with a verify-then-write
gate, and LSP surfacing as diagnostics + quick-fix code actions. Rule
catalogue v1: dead code, case hygiene, Elm-family pitfalls, and security
smells (`Task String` errors, Float-money, password-`onInput`,
`data-sky-eval`). Capability-only divergence: lint never changes what the
compiler accepts, so Go-oracle parity is unaffected. Tracked as a GitHub
issue; implementation not started.
