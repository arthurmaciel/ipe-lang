# rt-live findings

5 findings: 0 critical, 0 high, 1 medium, 4 low

Partition: the runtime Ipe.Live surface — `src/runtime/rust/src/live/**`
(hub.rs, hub_exporter.rs, push_exporter.rs, pubsub.rs, mod.rs, store.rs,
csrf.rs, console.rs, observability.rs, req.rs, route.rs, sse.rs,
style_inject.rs).

Prior-audit status (runtime-audit-verdict.md) for this partition — all the
critical/high items are FIXED and were NOT re-filed:
- `count_table` latent SQLi (HIGH) → FIXED: `table: &str` replaced by a closed
  `TelemetryTable` enum whose `name()` is a compile-time literal
  (`hub.rs:649-676`). The only `format!`-built SQL in the partition can now only
  interpolate a hardcoded name.
- diff/patch stored-XSS (CRITICAL) → FIXED in `dom/diff.rs`: `diff_attrs`/
  `diff_events` route every key/value through `insert_safe_attr` →
  `crate::html::safe_patch_attr` (name gate + URL-scheme gate); tests
  `diff_gates_out_dangerous_attr` / `diff_neutralises_javascript_url_in_patch`.
- `client.js` `__skyRunEvals` eval sink (CRITICAL) → FIXED: removed; only the
  CSP-safe `__skyRunPaths` remains; no `new Function(`/`eval(` survives.
- `sendBeacon` CSRF omission (MEDIUM) → FIXED: `client.js` now flushes via a
  `keepalive` fetch carrying `X-Sky-Csrf`.

## rt-live-001 · push_exporter sends the ingest token over cleartext HTTP
- severity: low
- axis: security
- principle: P1 no secret leakage into transit; "safe outcome is the only reachable one"
- location: `src/runtime/rust/src/live/push_exporter.rs:66-77,103-111,216-228`
- reachability: `enable_from_env` reads `IPE_PARENT_URL` and, when
  `IPE_INGEST_TOKEN` is set, `flush` attaches it as the `x-sky-ingest-token`
  header on every POST. Unlike the sibling `hub_exporter::enable_from_env`
  (which parses the URL and refuses a non-`https`, non-loopback host —
  `hub_exporter.rs:102-119`), `push_exporter` appends `/_sky/observability/ingest`
  to the raw parent URL with no scheme/host validation.
- problem: an operator who sets `IPE_PARENT_URL=http://<non-loopback-host>`
  plus an ingest token ships the shared secret in cleartext to that host. Same
  secret-in-transit class the hub exporter already guards; the two exporters
  diverge. Bounded to a misconfiguration (the URL is operator-set, not
  attacker-set), hence low, but cross-host federation is in scope.
- fix direction: extract `hub_exporter`'s `reqwest::Url::parse` + https-or-loopback
  guard into a shared helper and apply it in `push_exporter::enable_from_env`
  before the SENDER is claimed; refuse + log when a token is present over a
  non-loopback `http://` parent.
- prior: runtime-audit-verdict.md live-hub-obs `[low]` push_exporter — STILL PRESENT.

## rt-live-002 · HubLogFilter encodes a one-of-N level as four independent bools
- severity: low
- axis: soundness
- principle: make-invalid-states-unrepresentable
- location: `src/runtime/rust/src/live/hub.rs:137-181`
- reachability: `decode_filter` deserializes the console's forwarded filter into
  `HubLogFilter { showDebug, showInfo, showWarn, showError }`; `pick_single_level`
  reads it on the log-read path (`read_logs_value`).
- problem: four bools represent 16 states but only 5 have defined semantics; the
  store applies an `=` level filter so "exactly one toggled" is the only
  expressible query. `pick_single_level` silently treats 0 or 2+ actives as
  "no filter" — the illegal combinations are constructible and reach the SQL
  path with degraded-but-undiscoverable behaviour. No live exploit (input is the
  console operator's own UI filter, not untrusted external data; the fallback is
  total), so it is a type-hole smell, not a security hole.
- fix direction: replace the four bools with an `enum LevelFilter { All, Debug,
  Info, Warn, Error }` parsed once at the decode boundary.
- prior: runtime-audit-verdict.md live-hub-obs `[low]` HubLogFilter — STILL PRESENT.

## rt-live-003 · session id is a bare String; SSE hello frame hand-interpolates it
- severity: low
- axis: security
- principle: parse-don't-validate; P1 no injection into a serialized frame
- location: `src/runtime/rust/src/live/mod.rs:1629` (hello frame) +
  `sid_from_cookie` (`mod.rs:2051-2063`) + the Cold-hit adopt path (`mod.rs:1477-1482`)
- reachability: `sid_from_cookie` returns the raw cookie value unvalidated. The
  page Cold-hit arm adopts it verbatim as the canonical sid (store key +
  `Set-Cookie` value); the SSE handler then builds the hello frame by manual
  string interpolation `format!("{{\"v\":1,\"sid\":\"{hello_sid}\",\"ts\":{hello_ts}}}")`.
- problem: the "sid is 32 ASCII-hex" invariant is a runtime property (produced by
  `new_sid`, and a Cold hit only fires when the sid keys a real persisted
  checkpoint, which was itself written under a hex sid), never encoded in a type.
  If that invariant were ever broken, a `"`/`\` in the sid corrupts the hello
  JSON / SSE `data` field. The adjacent resync frame already uses
  `serde_json::json!` (`mod.rs:1644-1649`) — the hello frame is the one hand-built
  holdout. Exploitability is low because the store-keying invariant holds today.
- fix direction: introduce a `SessionId(String)` smart constructor (32 ASCII-hex)
  parsed in `sid_from_cookie`; build the hello payload with `serde_json::json!`
  exactly as resync does, so any sid is escaped regardless of the upstream invariant.
- prior: runtime-audit-verdict.md live-core `[medium]` SessionId + `[low]` hello
  frame — PARTIALLY FIXED (resync now uses `json!`); hello frame + bare-String sid
  remain. Downgraded to low: reachability analysis shows every hit-path sid is
  hex-keyed, so no untrusted value currently reaches the interpolation.

## rt-live-004 · strip_style_close is O(n²) on attacker-influenceable CSS marker values
- severity: medium
- axis: security
- principle: P1 no unbounded resource a remote party can exhaust (CPU DoS)
- location: `src/runtime/rust/src/css_safety.rs:418-432` (impl), reached from
  `src/runtime/rust/src/live/style_inject.rs:219,249,289,330,334`
- reachability: `apply_style_injections` runs on every Live render
  (`mod.rs:600,1469,1518`) and calls `strip_style_close` on the `Ipe.Ui`
  style-marker values (Transition/Animation/hover-rule strings), which can carry
  app/user-derived data (theme colours, custom rules).
- problem: `strip_style_close` loops `out.to_ascii_lowercase()` (a full O(n) copy)
  + `find` + `replace_range` (O(n) tail shift) once per `</style` occurrence, so a
  ~1 MB value built largely of `</style` fragments forces ~n/7 passes ≈ O(n²/7) —
  seconds-to-minutes of server CPU per render. The fixpoint loop was added
  deliberately (seam-reconstruction safety) but the implementation is quadratic.
- fix direction: rewrite as a single linear left-to-right scan that lowercases
  per-byte and backs up at most `len("</style")-1` chars after each removal;
  optionally cap marker-value length up front. Preserves the total /
  case-insensitive / seam-safe guarantee at O(n).
- prior: runtime-audit-verdict.md html-render `[medium]` style_inject
  strip_style_close — STILL PRESENT (function relocated to `css_safety.rs`, impl
  unchanged and still quadratic).

## rt-live-005 · EventBody + Route carry unreconciled dual fields / untyped arity
- severity: low
- axis: completeness
- principle: make-invalid-states-unrepresentable
- location: `src/runtime/rust/src/live/mod.rs:287-311` (`EventBody`) +
  `src/runtime/rust/src/live/route.rs:14-27` (`Route.build`)
- reachability: `EventBody` deserializes from the `/_sky/event` POST body;
  `Route.build` closures index the captured-params `Vec` (`p[0]`, `p[1]` — the
  codegen-emitted shape mirrored in the route.rs test helpers).
- problem: `EventBody` keeps two fields for one concept (`handler_id` + fallback
  `id`) and two for another (`event` + `msg`), reconciled by ad-hoc
  `if !x.is_empty()` ladders (`mod.rs:1766-1779`); both-empty and both-set-disagree
  are representable post-deserialize. Separately, `Route`'s "pattern `:param`
  count == Vec slots the build closure indexes" is a cross-field invariant not
  carried in a type — `match_route` returns exactly the pattern's param count so
  it holds today (no panic reachable from the fixed compile-time route table),
  but a codegen arity drift would raw-`[]`-index-panic. Both are typed-boundary
  smells, not live holes.
- fix direction: parse `EventBody` once into a typed `ResolvedEvent { handler,
  event }`; store `param_names` on `Route` and have `build` take `.get(i)` /
  a named dict so arity mismatch cannot index out of bounds.
- prior: runtime-audit-verdict.md live-core `[low]` EventBody + `[low]` Route
  arity — STILL PRESENT.
