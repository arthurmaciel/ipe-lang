Status: Accepted

# 0004. Web / HTTP web-security invariants (CSRF, trusted-proxy, CSWSH, floors)

## Context

Several web-security gaps across the Ipe.Web and Ipe.Http.Server runtime
surfaces needed closing. The code is the source of truth for the *how*; this
ADR preserves the security *why* and the invariants that must keep holding,
since a stale procedural spec would mislead but the decisions below are
durable.

Note: Ipe.Web's own CSRF (`web/csrf.rs`) was already complete and wired
end-to-end (`__Host-` prefixed double-submit cookie, constant-time compare,
axum middleware layer); Ipe.Web's own CSRF half was already complete and wired end-to-end. What these decisions
cover is the surrounding surface.

## Decision

### 1. Headless `Ipe.Http.Middleware.withCsrf` is a separate, feature-safe impl

Ipe.Web's `csrf.rs` **cannot be reused verbatim** for the headless
`Ipe.Http.Server` API. `csrf.rs` is gated by the `live` Cargo feature, which
pulls in `aes-gcm`; the `server` feature alone does not. `Ipe.Http.Middleware`
kernels register under the `Server` region and must build standalone under
`--features server`. So `withCsrf` has its own self-contained implementation in
`server.rs` using only crates unconditional in the runtime: `subtle`
(constant-time compare) and `uuid::Uuid::new_v4()` (CSPRNG token source, an
approved security-bearing randomness source per the runtime's own SECURITY
INVARIANT convention).

Intentional design differences from Ipe.Web's CSRF (not shortcuts):

- **How the client learns the token.** No page render exists for a headless API,
  so the cookie is **non-`HttpOnly`** — same-origin client JS reads it and
  echoes it back (classic double-submit). This stays safe against a forging
  cross-origin page because the Same-Origin Policy blocks that page from reading
  the victim-origin cookie.
- **Opt-in, per-route** via `Middleware.withCsrf handler` (the wrapper-combinator
  shape of `withCors`/`withBasicAuth`), not a blanket layer — `Ipe.Http.Server`
  routes are 100% user-defined, so the user simply doesn't wrap routes that
  shouldn't require CSRF; no exempt-path list is needed.

### 2. Spoofable proxy headers are honoured only behind a trusted-proxy opt-in

The session cookie's `Secure` attribute was ENV-gated (snapshot once at process
start), so a dev process behind a TLS proxy emitted a non-Secure cookie. The fix
makes the SESSION cookie's `Secure` attribute **request-scoped** via
`X-Forwarded-Proto` — but only when the operator opts in through
`IPE_TRUSTED_PROXY` (unset/`0`/`false` = don't trust), the **same env-var and
same rationale** `server.rs` already established for `X-Forwarded-For`. A
client-supplied header must never be trusted by default.

Scope invariant: the CSRF cookie's `__Host-` **name** decision stays
process-global — the cookie identity must be stable across a browser session or
the double-submit compare would spuriously fail whenever proxy-scheme detection
flips between requests. Only the session cookie's `Secure` *attribute* (no name
implications) becomes request-scoped.

### 3. Same-origin floor for the otherwise-unauthenticated dev ingest

`/_ipe/observability/ingest` is CSRF-exempt and open in dev. When it is
otherwise unauthenticated (token unset, dev mode), it gets an `Origin`-vs-`Host`
same-origin floor — this is the ONLY defense available in the no-token case
(production already fails closed; token-configured mode has real auth).
**Absent** `Origin` is NOT flagged: same-origin fetch/XHR, curl, and
server-to-server pushes never send a hostile cross-origin request by
construction, and requiring `Origin` would break legitimate non-browser ingest.

### 4. WebSocket upgrade requires an Origin allowlist (CSWSH)

`server_web_socket_upgrade` previously skipped the Origin check entirely outside
production when `originPatterns` was empty (dev allow-all fall-through). That is
a Cross-Site WebSocket Hijacking gap; the upgrade must validate `Origin` against
the configured allowlist.

### 5. Environment byte-count floors must reject `0`

`web_max_body_bytes()` read `IPE_WEB_MAX_BODY_BYTES` without a `> 0` filter, so setting it to `0`
made every `/_ipe/event` POST 413. It must
`.filter(|&n| n > 0)` before falling back to the default, matching
`server.rs::max_body()`. The env var is shared between the Ipe.Web event
endpoint and the Ipe.Http.Server default, so the floor behaviour must be
identical on both sides.

## Consequences

- The two CSRF implementations (Ipe.Web blanket-layer with HttpOnly +
  page-embedded token; headless per-route double-submit with non-HttpOnly
  cookie) are deliberately different and must stay feature-partitioned: the
  headless path may only use crates unconditional under `--features server`.
- `IPE_TRUSTED_PROXY` is the single opt-in gate for *all* spoofable proxy-header
  trust (X-Forwarded-For and X-Forwarded-Proto). Any future header that a
  reverse proxy sets must route through the same gate — never trusted by
  default.
- Any new unauthenticated dev endpoint that accepts browser requests needs the
  same `Origin`-present-and-mismatched floor (not `Origin`-required).
- Any new env-var byte/size limit must apply the `> 0` floor before defaulting.
