# Sliding sessions, absolute lifetime cap, and revocation

Status: proposed (tbd)

## Goal

Three properties for authenticated sessions:

1. **Sliding (rolling) re-issue** — an active session's expiry extends on use, so an
   engaged user is not logged out mid-session.
2. **Absolute-lifetime cap** — regardless of activity, a session cannot outlive a hard
   maximum age measured from its original issue. This bounds the value of a stolen,
   still-usable token.
3. **Revocation / suspension** — an application can declare a session (or its subject)
   revoked, and the session layer honors that on every authenticated request.

Precedence throughout is Security > Correctness > Soundness > Efficiency: a request is
authenticated iff `now < sliding_expiry` **and** `now < absolute_expiry` **and** the
subject/session is not revoked. Any uncertainty (store error, unknown state, malformed
token) denies — fail closed.

## Current state

Sessions today are a fixed-lifetime signed cookie. The session token is minted at login,
carries the subject, and is validated on each request against a single expiry derived
from a TTL setting (`session_ttl_from` in `app_config`, precedence env > setting >
fallback). There is no rolling extension, no separate absolute cap, and no revocation
path — a minted token is valid until its one expiry passes. The per-app CSRF cookie work
(`web/csrf.rs::csrf_cookie_name_for`) and the session cookie share the same per-app
`web_base_path()` scoping; this design keeps that coupling.

## Token model — stateless, with a bounded revocation store

The session token stays **stateless**: an HS256-signed value carrying tamper-proof time
anchors as signed claims:

- `iat` — issued-at (the immovable absolute anchor).
- `exp` — the sliding expiry (extends on use).
- `cap` — the absolute expiry, fixed at `iat + max_lifetime` and never rewritten.
- `sub` — subject; `jti` — a per-session id (for session-scoped revocation and eviction).

All four are inside the signed segment, so a client cannot move any expiry. Validation is
`verify_signature(token)` then `now < exp && now < cap && revocation != Revoked`.

Why stateless over a full server-side session table: it keeps the time anchors
self-contained (no server row to desync from the cookie), and avoids a session-table read
on the happy path. The one thing statelessness cannot do by itself — *instant* revocation
— is restored by a **separate, small, server-side revocation store** (membership only),
checked per authenticated request. That store holds revoked subjects/session-ids, not
full session state, so it stays small and its only question is boolean.

## Revocation mechanism — a runtime-owned store the app writes to

An Ipê app cannot store a closure in a record (IPE-L0107) and its state lives in the TEA
loop, so "app-provided revocation predicate" cannot be a raw callback. Instead the runtime
owns a revocation store and the app **writes** to it; the session layer **reads** it.

Considered alternatives:

- *A TEA message the app answers per request.* Rejected: couples auth latency to the
  update loop, and cannot answer for a session being torn down out-of-band.
- *A capability/hook registered in config, resolved to a runtime check.* Folded in as the
  on-switch (below), but insufficient alone as the storage mechanism.
- **A runtime-owned revocation store the app writes to (chosen).** Stores no closure
  anywhere (respects no-FCF-in-record by construction), keeps the authorization check off
  the TEA hot path, is trivially fail-closed, and composes with `Principal` — a revoked
  subject yields no `Principal`, hence no secured-row access (closes the typed-store path).

### App-facing surface

Module `Ipe.Auth.Revocation`:

- `revokeUser : Principal -> Subject -> Task Error ()` — mark every session of a subject revoked.
- `revokeSession : Principal -> SessionId -> Task Error ()` — revoke one session (`jti`).
- `restoreUser : Principal -> Subject -> Task Error ()` — clear a subject's revocation.
- `isRevoked : Subject -> Task Error Bool` — query (for the app's own UI/admin flows).

Writes require a `Principal` (an authenticated caller), so revocation is itself an
authorized action. Enabled declaratively on `AuthConfig`:

- `withRevocation : RevocationMode -> AuthConfig -> AuthConfig`, where `RevocationMode` is
  a closed sum (`Off | Store`) — `Off` keeps today's zero-overhead path; `Store` arms the
  membership check.

### Middleware check

The middleware calls a crate-internal `revocation::is_revoked(subject, jti) -> Verdict`
where `Verdict = Active | Revoked | Unknown`, wired into `authed_route` before
`principal_mint`. The gate denies on `Revoked`, on `Unknown`, and on any store error —
fail closed on all three. Propagation is next-request (a revoke takes effect on the
subject's next authenticated request); session-scoped revocation additionally evicts any
live server-held session for that `jti`.

## Configuration

Two new `app_config::Setting` variants, resolved with the existing `session_ttl_from`
pattern (env > setting-in-code > fallback; a non-positive value is dropped fail-closed):

- `AuthMaxLifetime` — the absolute cap. Default **8h**.
- `AuthSlideWindow` — the rolling window. Default **30m**. Clamped so
  `slide_window < max_lifetime`.

These live in the existing config front door (`app_config`, `Ipe.App`); no new mechanism.

## Sliding re-issue rule (throttled)

Re-issuing a cookie on *every* request is wasteful. Rule: re-issue only once the token is
past `exp - slide_window/2`, setting the new `exp = min(now + slide_window, cap)`. `cap`
and `iat` never change. The re-issued Set-Cookie preserves every security attribute of the
original (`__Host-`/Secure/`Path=/`/SameSite and the per-app name). Once `now >= cap`, no
re-issue is possible and the session ends regardless of activity.

## Security review points

- **Session fixation** — re-mint the session id on privilege change / login, never carry a
  pre-auth id into the authed session.
- **Theft window** — bounded by `cap`; sliding cannot extend past it.
- **Tamper-proofing** — `iat`/`exp`/`cap`/`sub`/`jti` are inside the HS256 signature; a
  client-mutated expiry fails verification.
- **Revocation fail-closed** — deny on `Revoked`, `Unknown`, and store error.
- **CSRF interplay** — the per-app CSRF cookie (`csrf_cookie_name_for`) is unaffected; a
  re-issued session cookie keeps the same per-app scoping.
- **Cookie attributes** — `__Host-` + Secure + `SameSite` preserved on every re-issue.

## Phased implementation plan (each slice independently shippable, fail-closed)

- **P1 — absolute cap.** Add the `cap` signed claim + a `now < cap` gate + the
  `AuthMaxLifetime` setting (default 8h). No behavior change for a session shorter than the
  cap. Refusal tests.
- **P2 — sliding re-issue.** The throttled re-issue rule above + the `AuthSlideWindow`
  setting (default 30m), `exp` clamped to `cap`, cookie security attributes preserved.
- **P3 — revocation store.** The `Ipe.Auth.Revocation` surface + `RevocationMode` +
  `withRevocation` + the `is_revoked` middleware gate + live-session eviction on
  session-scoped revoke. **Mandatory security-soundness-guardian review** — this is the
  auth/language boundary.

If a later phase does not land, earlier phases remain correct and fail-closed.
