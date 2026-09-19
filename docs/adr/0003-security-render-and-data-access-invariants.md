Status: Living (consolidated)
Date: 2026-09-18
Archive: misc/docs/archive/ADR/

# 0003. Security, render & data-access invariants

The invariants that keep the web/HTTP surface, the UI/HTML render path, the
`Secret` and `SqlFragment` newtypes, the multi-driver data layer, the WebSocket
server, the per-capability web boundary, and doc-side Markdown rendering safe by
construction. The code is the source of truth for the *how*; this ADR records
the durable *why* and the structural properties each decision must keep holding.

## Decisions

### Headless CSRF is a separate, feature-safe double-submit impl
`Ipe.Web`'s blanket-layer CSRF (`web/csrf.rs`, `HttpOnly` + page-embedded token)
is gated by the `live` feature and its `aes-gcm` dependency; the headless
`Ipe.Http.Server` API builds under `--features server` alone, so
`Ipe.Http.Middleware.withCsrf` carries its own self-contained implementation
using only crates unconditional in the runtime (`subtle` constant-time compare,
`uuid::Uuid::new_v4()` CSPRNG token). It is a per-route wrapper-combinator (the
shape of `withCors`/`withBasicAuth`), opt-in because `Ipe.Http.Server` routes are
100% user-defined; the cookie is non-`HttpOnly` so same-origin client JS can echo
it (double-submit), which the Same-Origin Policy keeps safe against a forging
cross-origin page. The two CSRF implementations stay feature-partitioned; the
headless path may only use crates unconditional under `--features server`.

### Spoofable proxy headers are honoured only behind a trusted-proxy opt-in
A client-supplied header is never trusted by default. `X-Forwarded-For` and
`X-Forwarded-Proto` are honoured only when the operator sets `IPE_TRUSTED_PROXY`
(unset/`0`/`false` = don't trust) — one gate for all spoofable proxy-header
trust, on both the `Ipe.Web` (`web/mod.rs`) and `Ipe.Http.Server` (`server.rs`)
sides. The session cookie's `Secure` *attribute* is request-scoped through
`X-Forwarded-Proto` behind that gate, so a dev process behind a TLS proxy emits a
`Secure` cookie; the CSRF cookie's `__Host-` *name* stays process-global, because
a name that flips between requests would spuriously fail the double-submit
compare. Any future proxy-set header routes through the same gate.

### Fail-closed floors on the dev ingest and env byte limits
`/_ipe/observability/ingest` is CSRF-exempt and open in dev; when otherwise
unauthenticated it gets an `Origin`-vs-`Host` same-origin floor (the only defence
in the no-token case — production fails closed, token mode has real auth). Absent
`Origin` is not flagged: same-origin fetch, curl, and server-to-server pushes
never send a hostile cross-origin request by construction, and requiring `Origin`
would break legitimate non-browser ingest. Every env-var byte/size limit applies a
`> 0` floor before defaulting, so setting a limit to `0` cannot make every request
`413`; the floor is identical on both endpoints that share the env var.

### UI pseudo-class rules travel as one marker with a stable wire tag
`Ipe.Ui`'s pseudo-class sugar builds `AttrPseudoRule(PseudoClass, css)`; the
render pipeline harvests every such attribute on an element into ONE
`data-ipe-pc-rules` marker that the `live::style_inject` pass converts to a
`<style>` block. The wire format is a fixed encoder/decoder contract: tag mapping
`Hover→"h"`, `Focus→"f"`, `FocusVisible→"v"`, `Active→"a"`, `Disabled→"d"`; one
entry `"<tag>|<css>"`; entries joined with `"||"`; empty-`css` entries dropped.
`PseudoClass::wire_tag()` (colocated with the type, the SSOT for encode) must
move in lock-step with `style_inject::pseudo_selector_for_tag` (decode). Ipe.Tui
has no pseudo-class concept and never runs the injection pass — the marker simply
does not leak there.

### CSS value safety decodes escapes before scanning
CSS Syntax Level 3 defines a general `\`+hex escape that decodes to a code point
anywhere a token is lexed, so a raw substring scan for breakout chars and
script-sink keywords misses `\65 xpression(...)` → `expression(...)` and kin.
`SafeCssValue::parse` therefore decodes CSS escapes and re-runs the *same*
dangerous-pattern scan against the decoded string, with the pattern list in ONE
shared helper so the raw and decoded paths cannot drift. `SafeCssPropertyName` and
`SafeCssSelector` reject `\` outright via charset allowlists. Any new evasion
vector is hardened by extending the *decode* step, never by adding a second
scanner.

### `Ui.mediaQuery` reuses the shared CSS collector behind a typed boundary
The media-query producer wraps its child in a node carrying the
`data-ipe-mq-q` / `data-ipe-mq-rules` markers; the rules string comes from the
*same* `build_style_string` collector used for inline `style=""` and
`Ui.onPseudo`, so every CSS value inherits the escape-decode hardening above. The
only new piece is a thin `SafeCssMediaQuery::parse` newtype for the *query*
string, delegating to the shared danger-pattern + escape-decode pair — same
policy, its own proof type (the selector gate is unusable here because Media
Queries Level 4 range syntax legitimately uses `<`/`<=`). Fail mode is
fail-closed drop: a poisoned query silently omits both markers, leaving the child
intact, so the DOM shape stays stable (always a wrapper element) and the Web diff
never sees a gate-dependent structural change.

### `onSubmit` carries a typed generic closure — no `Arc<dyn Any>`
`Ui.onSubmit` / `Ipe.Html.Events.onSubmit` take a properly-typed generic closure
`F: Fn(T) -> M` rather than routing the form payload through `Arc<dyn Any>`. A
form payload of the wrong type is thus unrepresentable rather than a runtime
downcast — the make-invalid-states-unrepresentable choice over parse-at-runtime.
These typed-payload event sinks stay generic over the Msg type; reintroducing
`Arc<dyn Any>` anywhere in the event path is a regression.

### `Secret` is a sealed newtype that cannot leak or be `==`-compared
`Secret` is an opaque built-in primitive (`IrType::Secret`, four `Secret.*`
kernels, typed in `ipe_types::constrain`, dispatched in `ipe_lower`) whose runtime
representation is a sealed `struct Secret(String)` — never a transparent alias,
because an alias would inherit `String`'s `Display`/`Debug`/`PartialEq` and leak.
The newtype has a redacting `Debug`, no `Display`, and no `PartialEq`; the only
equality is the explicit constant-time `secret_constant_time_eq`. `==` on a
`Secret` fails closed at type-check with IPE-T0014: `ty_is_equatable(&Secret)` is
`false`, so an equality obligation is rejected rather than deferred to a `cargo`
error or — worse — a silent variable-time compare. `ty_is_equatable` is
security-load-bearing; opaque non-equatable primitives are added through the named
helper so the check is appended, never re-derived. Do not derive
`Display`/`PartialEq` on `Secret` and do not make it a transparent alias.

### `SqlFragment` is a fully-derivable, redacting type — no capability denylist
The typed SQL-WHERE builder (`SqlFragment` / `Sql.*`, replacing the raw-string
injection surface) is fully derivable (`Clone`, `PartialEq`) with a hand-written
`Debug` that shows SQL text plus a bind *count* only, never bind *values*.
Equality is safe because every `SqlParam` field type already implements
`PartialEq`, and `Debug` redacts by design, so no second disclosure path exists;
the only route to a bind value is the single `reveal` call on a contained
`Secret`. A per-trait denylist (as for `Secret`) is deliberately *not* used here:
it would strip **all** derives from any record merely containing a `SqlFragment`,
the derived-blast-radius form of the exit-0-then-cargo-fail class. Invariant:
every `SqlParam` variant field type must implement `PartialEq`; if a future
variant adds a non-`PartialEq` field, the fallback is the denylist *for
`SqlFragment` only*, never a silent loss of derives on containing records.

### Multi-driver DB: compile-time selection, typed NULL binds, hub-local tenant gate
The parsed `DbDriver` (Sqlite | Postgres) threads from manifest parsing into
codegen, which `include_str!`-selects between two config templates exporting
*identical* symbol names with driver-specific bodies (`DbPool`, `DbRow`,
`ipe_db_url`, `db_last_insert_id`, `db_format_sql`, `DB_USES_RETURNING_ID`); `db.rs`
never branches on driver and compiles once per project. There is no runtime
negotiation — the driver is frozen per binary. `SqlParam::Null(Box<SqlParam>)`
carries a witness for its variant tag (not a value) so Postgres's per-parameter
type OID matches the target column at prepare time; a degenerate nested-null falls
back to TEXT rather than panicking. Tenant scope is a hub-specific concern
(task-local `TENANT_PREFIX`, an optional `AND service_name LIKE ?` on hub's
`read_*`), not a generic `Db.*` feature; when a prefix is in scope every
`hub_read_filtered_*` short-circuits to enforce the gate *before* SQL is built, and
`escape_like_prefix` strips `%`/`_` so a tenant name like `"%"` cannot match all
services.

### WebSocket server is kernel-only with typed opaque handles and bounded send
The `Ipe.Http.Server.WebSocket` module is kernel-only (a `Ws` qualifier + 12
kernels, no `.ipe` port). It introduces two opaque monomorphic IR types —
`IrType::WebSocketServer` (renders `WsHandle`) and `IrType::WebSocketServerCfg` —
so kernels take a typed `WsHandle` rather than a raw `Int`, leveraging
exhaustiveness checks and preventing handle/integer confusion. Send is bounded
non-blocking `try_send` (`IPE_WS_SEND_BUFFER`, default 256, drop-on-full): the
sound default for effect kernels, since fail-fast prevents handler-task pileup
behind one slow peer. Origin glob gating with CSWSH hardening is mandatory in
production — empty `originPatterns` fails closed with 403, and the WebSocket
upgrade validates `Origin` against the allowlist even outside production (closing
the former dev allow-all fall-through). Reusing a stale handle after close yields
a clean `Err` (registry miss).

### Per-capability web disclosure with a fail-closed app-boundary gate
The coarse "this program talks to page JS" port axis is replaced by a closed
per-capability vocabulary the compiler owns — a `WebCapability` enum
(`Geolocation`, `Clipboard`, …) modelled as a sub-axis of the port capability,
not flat siblings, so a wrapper cannot invent a capability outside the vocabulary.
Each capability-bearing browser module is bound to its sub-axis via
`WebCapability::for_browser_module`, keyed on the canonical module path (so a
low-level submodule import cannot dodge classification); the capability scan
infers the *precise* capability from reachable code, threading
`imported_web_capabilities` through canon into codegen and telemetry
registration. Only the top-level application may grant a web capability — absence
denies, no grant is implicit or inherited, and a coarse grant for one capability
covers no other; a dependency reaching an un-granted capability is a compile error
that names the dependency. Introducing a web-reaching wrapper without binding it to
a vocabulary capability would re-open the invisible-reach hole.

### Kernel robustness: blocking-work offload, single-pass read-limit, per-frame CLI newline
Blocking work is offloaded off the async reactor: the compression kernels and the
blocking `std::fs` file kernels each split into a private `*_sync` helper routed
through `tokio::task::spawn_blocking` (`compression.rs` unconditionally;
`file.rs` under `#[cfg(feature = "tokio")]` with an inline fallback reachable only
from test-only narrow-feature builds). The `readFileLimit` TOCTOU is closed by
dropping the `metadata()` stat and reading `cap + 1` bytes in one pass, checking
the actual count post-read (boundary `read as u64 > cap`: exactly `cap` succeeds,
one over errors; the message omits an exact byte count to keep the bounded read
bounded). Every CLI render frame is terminated with a newline at its call site so
consecutive frames land on separate lines. A single-threaded executor must still
make progress on concurrent work during a blocking op.

### Doc-side Markdown renders through the `Ipe.Markdown` parse tree
`Ipe.Markdown`'s `Block` / `Span` / `HeadingLevel` tree is the single Markdown
parse model. Because the `ipe` doc tooling cannot run Ipê at doc-time and must not
carry the runtime dependency, the parser is hand-ported into a std-only,
deny-set-clean leaf module (`src/ipe-docs/src/markdown/parse.rs`) written with
`Result` and iterators — no indexing, unwrap, or panic. A semantic-parity gate
(`markdown/parity.rs`) keeps `Ipe.Markdown` authoritative: for a shared corpus the
ported tree must equal a tree snapshotted from a real `ipe` run, diffed in CI, so
any drift reddens the build — this is what preserves the single source of truth
despite the second implementation. The doc-side `Block`→HTML walker
(`markdown/walker.rs`) is the sole security-critical component: it HTML-escapes
every text byte by default (`& < > " '`), routes every `href`/`src` through one
shared `is_safe_href` scheme allowlist (`http`/`https`/`mailto` + scheme-less
relative; reject `javascript:`/`vbscript:`/dangerous `data:`) enforced at BOTH
parse and emit (defend in depth) and failing closed to escaped plain text, carries
an exhaustive arm per constructor so a new one forces a compile error, and bounds
blockquote nesting depth. `is_safe_href` is the only home for URL-scheme policy;
the heading-level offset a page applies lives in the doc-side caller, never in the
shared parser.
