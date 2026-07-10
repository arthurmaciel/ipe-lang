# Class 8 — Live/HTTP web security: implementation spec (2026-07-09)

> Status: PLAN (doc-only), written against HEAD as of 2026-07-09 (last runtime
> touch: `runtime/src/sky_runtime/live/style_inject.rs`, 2026-07-03; `req.rs`,
> 2026-07-02). Feeds the Class 8 sonnet-5 implementation lane per
> `docs/architecture/campaign-classification-2026-07-09.md` ("Live/HTTP web
> security — MECHANICAL. #63 (CSRF port), cookie Secure-vs-TLS,
> observability-ingest CSRF exemption, WebSocket CSWSH, `live_max_body_bytes`
> floor, #33"). Cross-referenced against
> `docs/architecture/prior-art-runtime-rust-2026-07-09.md` Part 1 §1 (the
> ancestor `../sky/runtime-rust/src/sky_runtime/live/csrf.rs`) and
> `docs/architecture/backlog.md` (Security tier #63, AUD-09 gap-sweep, #33).
>
> **Read this first — the prior-art doc's premise does not hold in THIS
> repo.** The ancestor's `csrf.rs` was flagged as "not yet wired into
> `serve_live`, verify before assuming end-to-end coverage." In *this* repo
> (`sky-rust`/Ipê), `runtime/src/sky_runtime/live/csrf.rs` (242 lines) already
> exists, is MORE complete than the ancestor (adds `__Host-` prefix, opt-in
> `SKY_LIVE_CSRF_ORIGIN_CHECK`, `frame_ancestors` cross-iframe mode), and IS
> wired end-to-end: `axum::middleware::from_fn(csrf::csrf_middleware)` layered
> onto the Sky.Live router (`live/mod.rs:1748`), cookie minted/read in the
> `page` GET handler (`live/mod.rs:1210-1217`), constant-time double-submit
> compare in `csrf_middleware` (`csrf.rs:196-239`). **#63's Sky.Live half is
> DONE, not open.** What IS still open, and what this spec actually covers:
>
> 1. A **different, still-unbuilt surface**: `Sky.Http.Middleware.withCsrf` —
>    an opt-in CSRF middleware for the **headless** `Sky.Http.Server` API
>    surface (no session, no page-embedded JS token — Sky.Live's mechanism
>    doesn't apply). This is what backlog #63 and the upstream audit
>    (`docs/architecture/sky-v0.17-upstream-audit.md:39,67-70`) actually mean
>    by "port `Sky.Http.Middleware.withCsrf`" once you read the Go reference:
>    "Live runtind carries its own client.js CSRF path but Server-side
>    middleware absent."
> 2. Session-cookie `Secure` is ENV-gated, not TLS-gated (confirmed bug,
>    `live/mod.rs:800-813`).
> 3. `/_sky/observability/ingest` CSRF exemption + open-in-dev (confirmed
>    intentional-but-incomplete; needs a same-origin floor, not a redesign).
> 4. WebSocket upgrade CSWSH gap (confirmed bug, `server.rs:966-1012`).
> 5. `live_max_body_bytes()` missing floor (confirmed bug, one-line fix).
> 6. #33 Http header-case parity (confirmed **partially already fixed**) +
>    extra `Http` builders (confirmed **not yet done** — an existing plan
>    doc covers this, adopted here by reference).

---

## 0. Prerequisite (blocks §1): `ServerResponse` can only carry ONE `Set-Cookie`

Discovered while designing `withCsrf` (§1) — **must land first**, its own
commit, before `middleware_with_csrf` is implemented, or the new CSRF cookie
will silently clobber (or be clobbered by) any cookie a wrapped handler sets.

**The bug.** `ServerResponse.headers` is `HashMap<String, String>`
(`runtime/src/sky_runtime/server.rs:51-56`). `server_with_cookie`
(`server.rs:359-380`) does `r.headers.insert("Set-Cookie".to_string(), v)`.
`to_axum_response` (`server.rs:549-593`) iterates `r.headers` and calls
`builder.header(k, v)` per entry — but since the **source map** collapses
to one `"Set-Cookie"` key, TWO code paths that both want to set a cookie on
the same response (e.g. a handler calling `Server.withCookie` on a response
a `withCsrf`-wrapped route also wants to stamp with a fresh CSRF cookie) will
have the second `.insert()` silently overwrite the first — never two
`Set-Cookie` header lines on the wire, even though `axum`'s `builder.header`
itself supports repeated calls with the same key name (APPEND semantics,
confirmed by the comment at `server.rs:565`, "`builder.header` APPENDS").

**Fix — add a dedicated multi-valued cookie channel.**

File: `runtime/src/sky_runtime/server.rs`.

1. Add a field to `ServerResponse` (line ~51-56):
   ```rust
   #[allow(non_snake_case)]
   #[derive(Clone, Debug)]
   pub struct ServerResponse {
       pub status: i64,
       pub body: String,
       pub headers: HashMap<String, String>,
       pub contentType: String,
       /// Pre-built `Set-Cookie` header VALUES (e.g. `"sid=abc; Path=/; HttpOnly"`),
       /// one entry per cookie. Kept separate from `headers` because `headers` is a
       /// `HashMap` (one value per key) and HTTP allows/requires MULTIPLE
       /// `Set-Cookie` response headers when a response needs to set more than one
       /// cookie (RFC 6265 §4.1 — Set-Cookie is the one header that must NEVER be
       /// comma-folded). A second caller writing into `headers["Set-Cookie"]` would
       /// silently clobber the first (make-invalid-states-unrepresentable: a
       /// `HashMap` key can't hold two values, so cookie plurality needs its own
       /// container, not a workaround on the header map).
       pub cookies: Vec<String>,
   }
   ```
   This field is Rust-runtime-internal only — `ServerResponse` is an
   `IrType::ServerResponse` opaque type; no Sky-emitted code constructs or
   reads this struct's fields directly (confirmed: every codegen site treats
   it as an opaque `Response`/`ServerResponse` token, routed exclusively
   through kernel calls like `server_text`/`server_with_cookie`/etc. —
   grep `crates/sky_backend_rust/src/emit_expr.rs` /
   `crates/sky_ir/src/ir.rs` / `crates/sky_lower/src/lower.rs` for
   `IrType::ServerResponse`: every hit is a type-classification arm, never a
   field-access emission). So this is a pure additive, source-compatible
   change from Sky's point of view.
2. Update all FOUR literal-construction sites to add `cookies: Vec::new()`:
   - `resp()` helper, `server.rs:248-256` (backs `server_text` / `server_json`
     / `server_html` / `server_redirect`).
   - `ws_resp()`, `server.rs:895-902`.
   - The WS-upgrade 101 sentinel response, `server.rs:1002-1007`.
   - `plain_resp()`, `server.rs:1364-1375` (backs the CORS/basic-auth/
     rate-limit middleware's own synthetic responses).
3. Rewrite `server_with_cookie` (`server.rs:359-380`) to push instead of
   insert:
   ```rust
   pub fn server_with_cookie(c: ServerCookie, mut r: ServerResponse) -> ServerResponse {
       let name = sanitise_cookie_field(&c.name);
       let value = sanitise_cookie_field(&c.value);
       let secure = if crate::sky_runtime::telemetry::production_from_env() {
           "; Secure"
       } else {
           ""
       };
       let v = format!("{}={}; HttpOnly; Path=/; SameSite=Lax{}", name, value, secure);
       r.cookies.push(v);
       r
   }
   ```
4. Rewrite `to_axum_response` (`server.rs:549-593`) to also emit every entry
   in `r.cookies` via `builder.header("set-cookie", v)` (append — do NOT
   route through the `r.headers` loop):
   ```rust
   for cookie_v in &r.cookies {
       builder = builder.header("set-cookie", cookie_v.as_str());
   }
   ```
   Place this loop alongside the existing `for (k, v) in &r.headers` loop
   (either order is fine — they're disjoint header instances on the wire).

**Regression test** (new, `server.rs`'s existing `#[cfg(test)] mod tests`):
```rust
#[tokio::test]
async fn two_set_cookie_headers_both_survive() {
    let mut r = server_text("ok".to_string());
    r = server_with_cookie(server_cookie("a".into(), "1".into()), r);
    r = server_with_cookie(server_cookie("b".into(), "2".into()), r);
    let resp = to_axum_response(r);
    let cookies: Vec<_> = resp
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .collect();
    assert_eq!(cookies.len(), 2, "both Set-Cookie lines must survive");
}
```
Run: `cargo test -p sky-runtime-rust --features full two_set_cookie_headers_both_survive`
— expected **fails before the fix** (1 header, second clobbers first),
**passes after**.

Commit: `fix(server): ServerResponse carries multiple Set-Cookie values (no clobber)`.

---

## 1. `Sky.Http.Middleware.withCsrf` — new middleware for `Sky.Http.Server`

**Scope confirmation.** `Sky.Http.Middleware` already exists and is wired
(`withCors` / `withLogging` / `withBasicAuth` / `withRateLimit` —
`runtime/src/sky_runtime/server.rs:1377-1546`). `withCsrf` is the ONE
missing member, per the upstream audit
(`docs/architecture/sky-v0.17-upstream-audit.md:39`): *"Double-submit-cookie
CSRF middleware — `__Host-sky_csrf` 32B token, safe-methods set cookie,
unsafe-methods require cookie + `X-Csrf-Token`/`_csrf` with constant-time
compare, 403 on mismatch."*

**Why Sky.Live's `csrf.rs` can't just be reused verbatim.** `csrf.rs` lives
under `live/`, gated by the `live` Cargo feature
(`live = ["server", "http_client", "serde_json", "serde_urlencoded",
"async-trait", "aes-gcm", "sha2"]`, `runtime/Cargo.toml:118`). The `server`
feature alone (`server = ["tokio", "axum", "tower-http", "futures-util"]`,
`Cargo.toml:100`) does **not** pull in `aes-gcm` — so `csrf::gen_token()`'s
`use aes_gcm::aead::{OsRng, rand_core::RngCore};` (`csrf.rs:82`) would fail
to compile in a `server`-only (no `live`) build. `Sky.Http.Middleware` kernels
are registered under the `Server` kernel region (`sky_kernels/src/lib.rs`'s
`d(..., Server, ...)`) and must build standalone under `--features server`.
`withCsrf` therefore needs its own, self-contained implementation in
`server.rs`, using only crates already unconditional in `runtime/Cargo.toml`:
`subtle` (constant-time compare, already used by `middleware_with_basic_auth`,
`server.rs:1508-1511`) and `uuid` (CSPRNG token source — `uuid::Uuid::new_v4()`
is backed by `getrandom`, and per the runtime's own documented convention in
`runtime/src/sky_runtime/random.rs:1-16` ("SECURITY INVARIANT" comment),
`uuid::new_v4` is an approved security-bearing randomness source alongside
`OsRng`/`crypto_random_token`).

**Design differences from Sky.Live's CSRF (intentional, not a shortcut):**

| Aspect | Sky.Live (`live/csrf.rs`) | `Middleware.withCsrf` (new) |
|---|---|---|
| How the client learns the token | Page-embedded JS (`window.__SKY_CSRF_TOKEN`, injected by `render_page_full`) — cookie is `HttpOnly` | No page render exists for a headless API — cookie must be **non-`HttpOnly`** so same-origin client JS can read it and echo it back (classic double-submit; still safe against a forging cross-origin page because SOP blocks that page from reading the victim-origin cookie) |
| Applied to | Every mutating route, blanket, via an axum middleware layer over the whole router | Opt-in, per-route, via `Middleware.withCsrf handler` — the same wrapper-combinator shape as `withCors`/`withBasicAuth` |
| Exempt-path list | Yes (`/_sky/sse`, `/_sky/console`, …) — Sky.Live owns fixed internal routes | None needed — `Sky.Http.Server` routes are 100% user-defined; the user simply doesn't wrap routes that shouldn't require CSRF |
| Token source | `aes_gcm::aead::OsRng` | `uuid::Uuid::new_v4()` ×2 (feature-safe under `server`-only) |

### 1.1 Runtime implementation

File: `runtime/src/sky_runtime/server.rs`, new code adjacent to the existing
Middleware block (`server.rs:1350-1546`), after `middleware_with_rate_limit`.

```rust
/// `__Host-` prefix requires Secure + Path=/ + no Domain — mirrors
/// `live/csrf.rs::csrf_cookie_name`'s reasoning, gated on the SAME
/// process-wide production signal `server_with_cookie` already uses
/// (`telemetry::production_from_env`), so naming stays internally consistent
/// with the rest of `server.rs`'s cookie handling.
fn csrf_cookie_name() -> &'static str {
    if crate::sky_runtime::telemetry::production_from_env() {
        "__Host-sky_csrf"
    } else {
        "sky_csrf"
    }
}

/// 64 lowercase-hex chars (two concatenated UUIDv4s, ~244 combined random
/// bits — comfortably above the 128-bit CSRF-token floor). Does NOT use
/// `aes_gcm::aead::OsRng` (unlike `live/csrf.rs::gen_token`) because this
/// function must compile under `--features server` alone, which does not
/// pull in the `aes-gcm` crate (see `Cargo.toml`'s `server` vs `live`
/// feature sets). `uuid::Uuid::new_v4` is an approved CSPRNG source per
/// `random.rs`'s documented security-bearing-randomness convention.
fn csrf_gen_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn csrf_token_well_formed(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// NOT HttpOnly (see the design-differences table above — client JS must be
/// able to read this to echo it into `X-Csrf-Token`). `Secure` mirrors
/// `server_with_cookie`'s existing production gate (`server.rs:369`) — same
/// known ENV-vs-TLS limitation as the Sky.Live session cookie (§2 of this
/// doc); not re-solved here to keep this item's scope mechanical.
fn csrf_set_cookie_value(token: &str) -> String {
    let name = csrf_cookie_name();
    let secure = if crate::sky_runtime::telemetry::production_from_env() {
        "; Secure"
    } else {
        ""
    };
    format!("{name}={token}; Path=/; SameSite=Strict{secure}")
}

/// Middleware.withCsrf : Handler -> Handler. Double-submit-cookie CSRF guard
/// for `Sky.Http.Server` routes (Go/upstream-audit parity:
/// `sky-v0.17-upstream-audit.md:39` — `__Host-sky_csrf` cookie, safe methods
/// set/refresh it, unsafe methods require cookie == `X-Csrf-Token` header via
/// constant-time compare, 403 on any mismatch/missing value).
///
/// Depends on `ServerResponse.cookies` (§0 of this spec) so this middleware's
/// Set-Cookie can never clobber (or be clobbered by) one the wrapped handler
/// sets via `Server.withCookie`.
pub fn middleware_with_csrf<E, H>(h: H) -> ServerHandler<E>
where
    E: Send + 'static,
    H: IntoServerHandler<E>,
{
    let h = h.into_server_handler();
    Arc::new(move |req: ServerRequest| {
        use subtle::ConstantTimeEq;
        let safe = matches!(
            req.method.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        );
        let cookie_name = csrf_cookie_name();
        let existing = req.cookies.get(cookie_name).cloned();
        let token = existing
            .clone()
            .filter(|t| csrf_token_well_formed(t))
            .unwrap_or_else(csrf_gen_token);
        if !safe {
            let cookie_tok = existing.unwrap_or_default();
            let header_tok = header_ci(&req.headers, "x-csrf-token")
                .unwrap_or("")
                .to_string();
            let ok = !cookie_tok.is_empty()
                && !header_tok.is_empty()
                && bool::from(cookie_tok.as_bytes().ct_eq(header_tok.as_bytes()));
            if !ok {
                return Box::pin(async move {
                    ok_res(plain_resp(403, "csrf token invalid or missing", &[]))
                });
            }
        }
        let task = h(req);
        Box::pin(async move {
            match task.await {
                SkyResult::Ok(mut resp) => {
                    resp.cookies.push(csrf_set_cookie_value(&token));
                    SkyResult::Ok(resp)
                }
                other => other,
            }
        })
    })
}
```

**Explicitly out of scope for this pass (documented, not silently dropped):**
the audit's `_csrf` body-field fallback (form-encoded body carrying the
token as an alternative to the `X-Csrf-Token` header) is NOT implemented
here — it would require content-type sniffing + form-body parsing inside a
generic middleware that must otherwise stay body-agnostic (the middleware
runs BEFORE the handler and must not consume/alter `req.body` in a way that
surprises the wrapped handler). File as a fast-follow if a consumer needs
HTML-form (non-JS) CSRF protection on `Sky.Http.Server`; the header-based
mechanism is the primary mechanism per the audit text and covers the
JSON-API case, which is `Sky.Http.Server`'s primary use per CLAUDE.md's app
shape matrix ("HTTP / JSON API (no browser UI)").

### 1.2 Kernel-registry wiring (8-site chain, mirrors the existing 4 Middleware kernels)

Follow the exact pattern of `MiddlewareWithLogging` (arity 1, same
`Handler -> Handler` shape) at each site. Verify exact line numbers before
editing — cited against HEAD 2026-07-09:

| # | File:anchor | Edit |
|---|---|---|
| 1 | `crates/sky_kernels/src/lib.rs:658` (enum, after `MiddlewareWithRateLimit`) | add `MiddlewareWithCsrf,` |
| 2 | `crates/sky_kernels/src/lib.rs:1793-1799` (`decl()`, after the `MiddlewareWithRateLimit` arm) | add `Self::MiddlewareWithCsrf => d("Middleware", "withCsrf", 1, Server, "middleware_with_csrf"),` |
| 3 | `crates/sky_kernels/src/lib.rs:2734` (`ALL` slice, after `MiddlewareWithRateLimit`) | add `Self::MiddlewareWithCsrf,` |
| 4 | `crates/sky_kernels/src/lib.rs:3240` (`is_server` match, after `MiddlewareWithRateLimit`) | add `| Self::MiddlewareWithCsrf` |
| 5 | `crates/sky_kernels/src/lib.rs:5590` (self-test enumeration list, after `MiddlewareWithRateLimit`) | add `K::MiddlewareWithCsrf,` |
| 6 | `crates/sky_canon/src/env.rs:992-995` (`("Middleware", &[…])` QUALIFIERS list) | add `"withCsrf"` to the slice |
| 7 | `crates/sky_types/src/constrain.rs:3701` (scheme, after `MiddlewareWithLogging`'s arm) | add `K::MiddlewareWithCsrf => fun(fun(req(), task(resp())), fun(req(), task(resp()))),` (byte-identical shape to `MiddlewareWithLogging` — `Handler -> Handler`, no extra config args) |
| 8 | `crates/sky_lower/src/lower.rs:7131-7132` (arity-1 kernel curry-group, after `MiddlewareWithLogging`) | add `\| KernelFn::MiddlewareWithCsrf` to the same match arm (comment: `` `Middleware.withCsrf : Handler -> Handler` ``) |
| 9 | `crates/sky_lower/src/lower.rs:8672-8674` (`("Middleware", name) => …` resolution) | add `("Middleware", "withCsrf") => Ok(Callee::Kernel(KernelFn::MiddlewareWithCsrf)),` |
| 10 | `crates/sky_backend_rust/src/naming.rs:745-748` (`kernel_name()`, exhaustive match) | add `KernelFn::MiddlewareWithCsrf => "middleware_with_csrf",` |
| 11 | `crates/sky_ir/src/pretty.rs:665-668` (exhaustive `n` match) | add `KernelFn::MiddlewareWithCsrf => "Middleware.withCsrf",` |
| 12 | `crates/sky_backend_rust/src/emit_expr.rs:1984-1988` (generic N-arg call path, no special emission needed) | add `\| KernelFn::MiddlewareWithCsrf` to the existing `\| KernelFn::MiddlewareWithRateLimit` arm |

Sites 7/10/11 are exhaustive matches with **no `_` catch-all** — a missed
site fails to compile (fail-closed floor, same guarantee the m5b-http plan
relied on, see §6 below). Canon self-tests
(`crates/sky_canon/src/lib.rs` — the `qual_vars ⟺ stdlib_index` +
`decl()` injectivity invariants) will fail if site 6 is added without
2/3/4/5, or vice versa — this is the existing safety net, not new
machinery.

**No `.sky` stdlib file to touch** — confirmed no `Middleware.sky` exists
under `crates/skyc/stdlib/Sky/Http/` (grep-verified: only `Http.sky` exists
there; `Sky.Http.Middleware`'s 4 existing kernels have no doc/signature `.sky`
mirror, so `withCsrf` needs none either — consistent with the existing 4).

### 1.3 Regression tests

**Unit tests** (`server.rs`'s `#[cfg(test)] mod tests`, mirroring
`middleware_with_basic_auth`'s existing test-free precedent — write NEW
coverage since none of the 4 existing Middleware kernels have unit tests
today; this is the first):

```rust
fn mk_req(method: &str, cookies: HashMap<String, String>, headers: HashMap<String, String>) -> ServerRequest {
    ServerRequest {
        method: method.to_string(),
        path: "/".to_string(),
        body: String::new(),
        headers,
        params: HashMap::new(),
        query: HashMap::new(),
        cookies,
        remoteAddr: String::new(),
    }
}

#[tokio::test]
async fn csrf_get_mints_and_sets_cookie_no_check() {
    let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
        Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
            as SkyTask<String, ServerResponse>
    });
    let req = mk_req("GET", HashMap::new(), HashMap::new());
    let resp = h(req).await;
    match resp {
        SkyResult::Ok(r) => assert_eq!(r.cookies.len(), 1, "GET must mint a fresh cookie"),
        SkyResult::Err(_) => panic!("GET must never be rejected"),
    }
}

#[tokio::test]
async fn csrf_post_without_header_rejected() {
    let mut cookies = HashMap::new();
    cookies.insert("sky_csrf".to_string(), "a".repeat(64));
    let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
        Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
            as SkyTask<String, ServerResponse>
    });
    let req = mk_req("POST", cookies, HashMap::new());
    match h(req).await {
        SkyResult::Ok(r) => assert_eq!(r.status, 403),
        SkyResult::Err(_) => panic!("expected an Ok(403), not an Err"),
    }
}

#[tokio::test]
async fn csrf_post_with_matching_cookie_and_header_allowed() {
    let tok = "b".repeat(64);
    let mut cookies = HashMap::new();
    cookies.insert("sky_csrf".to_string(), tok.clone());
    let mut headers = HashMap::new();
    headers.insert("x-csrf-token".to_string(), tok);
    let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
        Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
            as SkyTask<String, ServerResponse>
    });
    let req = mk_req("POST", cookies, headers);
    match h(req).await {
        SkyResult::Ok(r) => assert_eq!(r.status, 200),
        SkyResult::Err(_) => panic!("expected Ok(200)"),
    }
}

#[tokio::test]
async fn csrf_post_with_mismatched_cookie_and_header_rejected() {
    let mut cookies = HashMap::new();
    cookies.insert("sky_csrf".to_string(), "c".repeat(64));
    let mut headers = HashMap::new();
    headers.insert("x-csrf-token".to_string(), "d".repeat(64));
    let h = middleware_with_csrf::<String, _>(|_req: ServerRequest| {
        Box::pin(ready(ok_res::<String, _>(server_text("ok".into()))))
            as SkyTask<String, ServerResponse>
    });
    let req = mk_req("POST", cookies, headers);
    match h(req).await {
        SkyResult::Ok(r) => assert_eq!(r.status, 403),
        SkyResult::Err(_) => panic!("expected Ok(403)"),
    }
}
```

**Golden/E2E test (the CSRF-rejects-forged-request test the task asks for).**
New fixture `tests/golden/m6_middleware_csrf/Main.sky`, mirroring the shape
of other `Sky.Http.Server` goldens (`crates/skyc/tests/golden_m6_server.rs`):

```elm
module Main exposing (main)

import Sky.Core.Prelude exposing (..)
import Sky.Http.Server as Server
import Sky.Http.Middleware as Middleware


handle : Server.Handler
handle _ =
    Task.succeed (Server.text "ok")


main =
    Server.listen 8090
        [ Server.post "/action" (Middleware.withCsrf handle) ]
```

Add an E2E test in `crates/skyc/tests/server_e2e.rs` (mirrors the existing
`#[test]` shapes there, `server_e2e.rs:370-515`): spin the compiled binary,
issue (a) a `GET /action`-equivalent probe route to capture the minted
`Set-Cookie: sky_csrf=…`, (b) a forged cross-site-style `POST /action` with
NO `X-Csrf-Token` header (simulating an attacker page that can trigger a
simple cross-origin POST but cannot read/set the victim-origin cookie or a
custom header without a CORS preflight allow) — assert **403**; (c) a
same-origin-style POST that reads the cookie value and echoes it in
`X-Csrf-Token` — assert **200**. Gate on `SKY_E2E=1` per existing convention.

### 1.4 Verification

```bash
cargo build --workspace
cargo test -p sky-runtime-rust --features full -- csrf
cargo test -p sky_kernels && cargo test -p sky_canon   # decl() injectivity + qual_vars invariants
SKY_E2E=1 timeout 900 cargo test -p skyc --test server_e2e -- csrf
cargo clippy --workspace -- -D warnings
cargo fmt --all --check
```

Commit: `feat(server): Middleware.withCsrf — double-submit CSRF for Sky.Http.Server (#63)`.

---

## 2. Session-cookie `Secure` — ENV-gated, not TLS-gated

**Confirmed bug**, `runtime/src/sky_runtime/live/mod.rs:797-833`
(`page_response`). The function's own comment already states the gap
precisely:

> "NOTE the decision is ENV-gated, NOT request-scoped:
> `csrf::cookies_secure()` snapshots `production_from_env() ||
> frame_ancestors().is_some()` once at process start; it does NOT inspect
> this request's TLS / `X-Forwarded-Proto`. A dev process fronted by a TLS
> proxy therefore emits a non-Secure session cookie."

**Fix — request-scoped detection, gated behind the SAME trusted-proxy opt-in
`server.rs` already uses for `X-Forwarded-For`.** `server.rs:478-490`
established the precedent: spoofable proxy headers are honoured ONLY when
the operator explicitly opts in via `SKY_TRUSTED_PROXY` (unset/`0`/`false` =
don't trust). Reuse that exact env var name for consistency (currently only
read in `server.rs`; extend to `live/mod.rs`).

**Scope decision:** do NOT change the CSRF cookie's `__Host-` NAME decision
(`csrf::cookies_secure()`, still process-global) — the cookie's identity
must stay stable across a browser session or the double-submit compare
would spuriously fail whenever proxy-scheme detection flips between
requests. Only the SESSION cookie's `Secure` ATTRIBUTE (which has no name
implications) becomes request-scoped. This is deliberately the minimal
closure of the exact gap the code comment names, not a redesign of the CSRF
cookie's naming scheme.

File: `runtime/src/sky_runtime/live/mod.rs`.

1. Add a small snapshot-once helper near `cookie_path()` (~line 790), same
   `OnceLock` pattern as `csrf::cookies_secure()`:
   ```rust
   /// Whether to trust `X-Forwarded-Proto` for TLS-termination detection.
   /// Mirrors `server.rs`'s `SKY_TRUSTED_PROXY` gate (same env var, same
   /// rationale: a client-supplied header must never be trusted by
   /// default — an operator opts in only when a real reverse proxy sits in
   /// front of this process).
   fn trust_proxy_headers() -> bool {
       use std::sync::OnceLock;
       static TRUST: OnceLock<bool> = OnceLock::new();
       *TRUST.get_or_init(|| {
           crate::sky_runtime::system::read_env_var("SKY_TRUSTED_PROXY")
               .map(|v| !v.is_empty() && v != "0" && v != "false")
               .unwrap_or(false)
       })
   }

   /// Request-scoped HTTPS detection: true when this SPECIFIC request arrived
   /// over TLS at the trusted proxy (`X-Forwarded-Proto: https`). Only
   /// consulted when `trust_proxy_headers()` is on — otherwise a client could
   /// forge the header to fool the Secure-cookie decision the other way
   /// (making a plain-HTTP request look HTTPS is not a downgrade risk in
   /// itself, but honouring an unvetted header at all is the same
   /// footgun `server.rs` already closed for X-Forwarded-For).
   fn request_is_https(headers: &axum::http::HeaderMap) -> bool {
       if !trust_proxy_headers() {
           return false;
       }
       headers
           .get("x-forwarded-proto")
           .and_then(|v| v.to_str().ok())
           .map(|v| v.eq_ignore_ascii_case("https"))
           .unwrap_or(false)
   }
   ```
2. Change `page_response`'s signature (`mod.rs:797`) to accept the request
   headers:
   ```rust
   fn page_response(sid: &str, body: &str, csrf_token: &str, headers: &axum::http::HeaderMap) -> axum::response::Response {
   ```
3. Change the `secure` computation (`mod.rs:809-813`) from:
   ```rust
   let secure = if csrf::cookies_secure() { "; Secure" } else { "" };
   ```
   to:
   ```rust
   let secure = if csrf::cookies_secure() || request_is_https(headers) {
       "; Secure"
   } else {
       ""
   };
   ```
   Update the surrounding comment to state the gap is now closed for the
   trusted-proxy-opt-in case, and that the untrusted-proxy case is an
   accepted residual (an operator who doesn't set `SKY_TRUSTED_PROXY` gets
   the pre-existing ENV-only behaviour, which is still SOUND — just not
   maximally precise — because it never marks a cookie Secure incorrectly,
   only potentially fails to mark one Secure that could safely have been).
4. Update both call sites (`mod.rs:1238` and `mod.rs:1326`, inside the
   `page` handler which already has `headers: axum::http::HeaderMap` in
   scope per its signature at `mod.rs:1192`) to pass `&headers`:
   ```rust
   return page_response(&sid, &body, &csrf_tok, &headers);
   ...
   page_response(&sid, &body, &csrf_tok, &headers)
   ```

**Regression tests** (new, in `live/mod.rs`'s existing `#[cfg(test)]`
module — grep the module for its name before adding, likely near the
bottom of the file):
```rust
#[test]
fn request_is_https_requires_trust_proxy_opt_in() {
    std::env::remove_var("SKY_TRUSTED_PROXY");
    let mut h = axum::http::HeaderMap::new();
    h.insert("x-forwarded-proto", "https".parse().unwrap());
    assert!(!request_is_https(&h), "must ignore XFP without opt-in");
}

#[test]
fn request_is_https_honours_trusted_proxy_header() {
    std::env::set_var("SKY_TRUSTED_PROXY", "1");
    let mut h = axum::http::HeaderMap::new();
    h.insert("x-forwarded-proto", "https".parse().unwrap());
    assert!(request_is_https(&h));
    let mut h2 = axum::http::HeaderMap::new();
    h2.insert("x-forwarded-proto", "http".parse().unwrap());
    assert!(!request_is_https(&h2));
    std::env::remove_var("SKY_TRUSTED_PROXY");
}
```

**Cookie-Secure-behind-proxy E2E test** (the task's explicit ask). Add to
`crates/skyc/tests/` (a live-app E2E harness — reuse whatever spins a
compiled Sky.Live binary today, e.g. the harness backing
`docs/architecture` Live goldens): with `SKY_TRUSTED_PROXY=1` and
`ENV` unset (dev), send a GET with header `X-Forwarded-Proto: https` and
assert the `Set-Cookie: sky_sid=…` response header contains `; Secure`;
without that header (or without `SKY_TRUSTED_PROXY` set), assert it does
NOT contain `; Secure` — pinning both the fixed case and the pre-existing
default-safe case (never regresses to "always Secure" which would break
plain-HTTP local dev).

### Verification
```bash
cargo test -p sky-runtime-rust --features full -- request_is_https
cargo test -p sky-runtime-rust --features full -- page_response
cargo clippy --workspace -- -D warnings
```

Commit: `fix(live): session cookie Secure honours trusted-proxy X-Forwarded-Proto (request-scoped)`.

---

## 3. `/_sky/observability/ingest` — CSRF-exempt + open-in-dev

**Current state is more defensible than the backlog line implies — read
`console.rs:258-386` before touching anything.** The endpoint:

- IS exempted from `csrf_middleware` (`csrf.rs:149`) — this exemption is
  correct, not a gap: the endpoint is authenticated by a **bearer-style
  shared secret** (`X-Sky-Ingest-Token`, constant-time compared,
  `console.rs:347-386`), which is a stronger, independent mechanism; CSRF
  double-submit is redundant on top of it and would only add friction for
  legitimate sub-app→parent pushes (`push_exporter.rs`).
- Fails **CLOSED in production** when no token is configured
  (`console.rs:356-367`, `401`) — already correct.
- Is **open in dev when unset** — this IS the residual gap, but the
  impact is bounded: `fold_log`/the span reader only ever feed the
  in-RAM telemetry rings (already defended against control-byte/ANSI
  injection via `sanitise_ingest`, `console.rs:318-334`) — there is no
  data exfiltration or privilege-escalation surface, only **log/metric
  forgery** (an attacker's page could POST fake entries into the
  operator's dev console). That is a real, if low-severity, CSRF-shaped
  gap: a `POST` with `Content-Type: text/plain` and no custom header is a
  CORS "simple request" — a malicious cross-origin page CAN fire it
  without a preflight.

**Fix — same-origin floor when the endpoint is otherwise unauthenticated,
not a redesign.** Add an `Origin`-vs-`Host` same-origin check to
`ingest_token_blocked`'s "token unset, dev mode" branch ONLY (production
already fails closed; token-configured mode already has real auth and
doesn't need this). Mirrors the reasoning already used by
`csrf.rs::origin_mismatch` (`csrf.rs:159-191`), but applied
unconditionally here (not opt-in) since this is the ONLY defense available
in the no-token-configured dev case.

File: `runtime/src/sky_runtime/live/console.rs`.

```rust
/// True when `Origin` is present AND does not match `Host` — i.e. this is a
/// cross-origin request. Absent `Origin` (same-origin fetch/XHR, curl,
/// server-to-server pushes from `push_exporter.rs`) is NOT flagged: those
/// callers never send a hostile cross-origin request by construction, and
/// requiring `Origin` would break legitimate non-browser ingest pushes.
fn is_cross_origin_ingest(headers: &axum::http::HeaderMap) -> bool {
    let origin = match headers
        .get(axum::http::header::ORIGIN)
        .and_then(|h| h.to_str().ok())
    {
        Some(o) => o,
        None => return false,
    };
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let origin_host = origin.split_once("://").map(|x| x.1).unwrap_or(origin);
    !host.is_empty() && origin_host != host
}
```

In `ingest_token_blocked` (`console.rs:347-386`), inside the `None => { ... }`
branch that currently does `if telemetry::production_from_env() { 401 } else
{ return None; }` (`console.rs:354-368`), add the cross-origin check to the
dev-mode `return None` path:

```rust
None => {
    if telemetry::production_from_env() {
        return Some(
            (
                StatusCode::UNAUTHORIZED,
                "observability ingest requires SKY_INGEST_TOKEN in production",
            )
                .into_response(),
        );
    }
    // Dev + no token configured: the ONLY remaining defense is same-origin.
    // A same-origin fetch/XHR, curl, or a same-process push (no Origin
    // header) is allowed; a cross-origin browser POST (the CSRF-log-
    // injection shape) is rejected.
    if is_cross_origin_ingest(headers) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "observability ingest: cross-origin request rejected (set SKY_INGEST_TOKEN to allow federated pushes)",
            )
                .into_response(),
        );
    }
    return None;
}
```

**Regression tests** (extend `console.rs`'s existing `ingest_token_gate`
test, `console.rs:394-` or add a sibling):
```rust
#[test]
fn ingest_open_dev_rejects_cross_origin_without_token() {
    std::env::remove_var("SKY_INGEST_TOKEN");
    let mut h = axum::http::HeaderMap::new();
    h.insert("origin", "https://evil.example".parse().unwrap());
    h.insert("host", "victim.example".parse().unwrap());
    assert!(
        ingest_token_blocked(&h).is_some(),
        "cross-origin POST with no token configured must be rejected"
    );

    // Same-origin (or no Origin header at all) still passes.
    let h2 = axum::http::HeaderMap::new();
    assert!(ingest_token_blocked(&h2).is_none(), "no-Origin request still open in dev");

    let mut h3 = axum::http::HeaderMap::new();
    h3.insert("origin", "https://victim.example".parse().unwrap());
    h3.insert("host", "victim.example".parse().unwrap());
    assert!(ingest_token_blocked(&h3).is_none(), "same-origin request still open in dev");
}
```

### Verification
```bash
cargo test -p sky-runtime-rust --features full -- ingest
cargo clippy --workspace -- -D warnings
```

Commit: `fix(live): observability ingest rejects cross-origin POSTs when SKY_INGEST_TOKEN is unset`.

---

## 4. WebSocket upgrade CSWSH — Origin check skipped outside production with empty `originPatterns`

**Confirmed bug**, `runtime/src/sky_runtime/server.rs:966-1012`
(`server_web_socket_upgrade`). Current logic:

```rust
if ws_production() && cfg.originPatterns.is_empty() {
    return ok_res(ws_resp(403, "websocket: origin allowlist required in production"));
}
if !cfg.originPatterns.is_empty() {
    // ... check Origin against the allowlist ...
}
// else: FALLS THROUGH — no Origin check at all.
```

Documented as intentional ("dev: allow-all", `server.rs:1099-1101`,
`ws_server_default_cfg`'s doc comment) — but this is precisely the
Cross-Site WebSocket Hijacking (CSWSH) shape: unlike a CSRF-protected form
POST, a browser `WebSocket` constructor **cannot set custom headers**, so
Origin validation is the ONLY defense a WS handshake has (no double-submit
cookie token is possible). An attacker page at `evil.example` can open
`new WebSocket("ws://victim-dev-host:8000/chat")` and the current dev-mode
code accepts it unconditionally whenever the developer hasn't explicitly
configured `Ws.withOriginPatterns`.

**Fix — default to same-origin (Origin ↔ Host) when no explicit
`originPatterns` are configured, instead of allow-all.** This preserves the
common case (a browser page served from the same host opening a WS back to
that host — `Origin` will equal `Host`) while closing the cross-origin
case, WITHOUT requiring every dev setup to configure `Ws.withOriginPatterns`
explicitly. Production behaviour (empty patterns → hard 403) is unchanged.

File: `runtime/src/sky_runtime/server.rs`.

```rust
/// True when `Origin` is present and does not match `Host` (cross-origin).
/// Absent `Origin` (same-origin browsers on older UA quirks, non-browser WS
/// clients, and legitimate same-origin pages under some proxy setups that
/// strip it) is NOT flagged — matches the equivalent CSRF/ingest same-origin
/// helpers elsewhere in this runtime (`csrf.rs::origin_mismatch`,
/// `console.rs::is_cross_origin_ingest`); duplicated locally (not shared via
/// a `live`-feature-gated module) because `server.rs` must build standalone
/// under `--features server` without `live`.
fn ws_cross_origin(req: &ServerRequest) -> bool {
    let origin = match header_ci(&req.headers, "origin") {
        Some(o) if !o.is_empty() => o,
        _ => return false,
    };
    let host = header_ci(&req.headers, "host").unwrap_or("");
    let origin_host = origin.split_once("://").map(|x| x.1).unwrap_or(origin);
    !host.is_empty() && origin_host != host
}
```

Change `server_web_socket_upgrade` (`server.rs:970-994`) from:

```rust
if ws_production() && cfg.originPatterns.is_empty() {
    return ok_res(ws_resp(403, "websocket: origin allowlist required in production"));
}
if !cfg.originPatterns.is_empty() {
    let origin = req.headers.iter().find(...).map(...).unwrap_or("");
    if !cfg.originPatterns.iter().any(|p| ws_origin_matches(p, origin)) {
        return ok_res(ws_resp(403, "websocket: origin not allowed"));
    }
}
```

to:

```rust
if ws_production() && cfg.originPatterns.is_empty() {
    return ok_res(ws_resp(403, "websocket: origin allowlist required in production"));
}
if !cfg.originPatterns.is_empty() {
    let origin = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("origin"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if !cfg.originPatterns.iter().any(|p| ws_origin_matches(p, origin)) {
        return ok_res(ws_resp(403, "websocket: origin not allowed"));
    }
} else if ws_cross_origin(&req) {
    // Dev mode, no explicit allowlist: default to same-origin rather than
    // allow-all (closes CSWSH — a WS handshake can't carry a custom header,
    // so Origin validation is the only available defense). Configure
    // `Ws.withOriginPatterns` explicitly to allow legitimate cross-origin
    // clients.
    return ok_res(ws_resp(
        403,
        "websocket: cross-origin request rejected (set Ws.withOriginPatterns to allow)",
    ));
}
```

Update the doc comments at `server.rs:1099-1101` and `:1194-1196`
(`ws_server_default_cfg` / `ws_server_with_origin_patterns`) from "dev:
allow-all" to "dev: same-origin only (Origin must match Host when Origin is
present); production: 403".

**Regression tests** (extend `server.rs`'s existing `origin_glob_matching`
test area, `server.rs:1730-`):
```rust
#[test]
fn ws_cross_origin_detection() {
    let mk = |origin: &str, host: &str| {
        let mut headers = HashMap::new();
        headers.insert("origin".to_string(), origin.to_string());
        headers.insert("host".to_string(), host.to_string());
        ServerRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            body: String::new(),
            headers,
            params: HashMap::new(),
            query: HashMap::new(),
            cookies: HashMap::new(),
            remoteAddr: String::new(),
        }
    };
    assert!(ws_cross_origin(&mk("https://evil.example", "victim.example:8000")));
    assert!(!ws_cross_origin(&mk("https://victim.example:8000", "victim.example:8000")));
    // No Origin header at all → not flagged (non-browser client).
    let mut headers = HashMap::new();
    headers.insert("host".to_string(), "victim.example".to_string());
    let req = ServerRequest { method: "GET".into(), path: "/".into(), body: String::new(), headers, params: HashMap::new(), query: HashMap::new(), cookies: HashMap::new(), remoteAddr: String::new() };
    assert!(!ws_cross_origin(&req));
}
```

**The CSWSH E2E test the task asks for.** Extend
`crates/skyc/tests/server_e2e.rs` (or a new `websocket_e2e.rs` if a WS
harness doesn't exist yet — grep first) with: spin a Sky.Http.Server binary
using `Ws.defaultCfg` (empty `originPatterns`), attempt an upgrade with
`Origin: https://evil.example` and a mismatched `Host` — assert **403**;
attempt an upgrade with `Origin` equal to the request's own `Host` — assert
**101 Switching Protocols**. Gate on `SKY_E2E=1`.

### Verification
```bash
cargo test -p sky-runtime-rust --features full -- ws_cross_origin
cargo test -p sky-runtime-rust --features full -- origin_glob_matching
SKY_E2E=1 timeout 900 cargo test -p skyc --test server_e2e -- websocket
cargo clippy --workspace -- -D warnings
```

**Docs to update in the same PR:** `docs/architecture/websocket-server-design.md`
(states the current "dev: allow-all" design decision — update to "dev:
same-origin default"); CLAUDE.md's own `Sky.Http.Server.WebSocket` stdlib
table entry describes "Server production gate: empty `originPatterns`
returns 403 when `ENV=production`" — this remains true and doesn't need
edits (it only ever documented the production case), but if a
CLAUDE.md refresh touches this section, note the dev-mode same-origin
default too so AI-authored docs stay accurate.

Commit: `fix(server): WebSocket upgrade defaults to same-origin (not allow-all) outside production (CSWSH)`.

---

## 5. `live_max_body_bytes()` missing the `>0` floor

**Confirmed bug**, one-line fix. `runtime/src/sky_runtime/live/mod.rs:860-869`:

```rust
fn live_max_body_bytes() -> usize {
    crate::sky_runtime::system::read_env_var("SKY_LIVE_MAX_BODY_BYTES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(5 << 20)
}
```

Compare `server.rs`'s already-correct `max_body()` (`server.rs:384-394`,
`DEFAULT_MAX_BODY = 32 * 1024 * 1024`):

```rust
fn max_body() -> usize {
    crate::sky_runtime::system::read_env_var("SKY_LIVE_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_BODY)
}
```

Note both read the SAME env var name — this is Go parity (`SKY_LIVE_MAX_BODY_BYTES`
is shared between the Sky.Live event endpoint and the Sky.Http.Server
default), so `SKY_LIVE_MAX_BODY_BYTES=0` currently 413s EVERY `/_sky/event`
POST on the Sky.Live side (the missing floor), while the Sky.Http.Server
side is already immune.

**Fix** — apply the identical `.filter(|&n| n > 0)`, and (bonus, matches
`server.rs`'s defensiveness at zero extra cost) `.trim()` the raw value too:

```rust
fn live_max_body_bytes() -> usize {
    crate::sky_runtime::system::read_env_var("SKY_LIVE_MAX_BODY_BYTES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(5 << 20)
}
```

**Regression test** (mirror `server.rs`'s existing `max_body_env_override`,
`server.rs:1799-1807`, add the sibling in `live/mod.rs`'s test module):
```rust
#[test]
fn live_max_body_bytes_floors_at_default_on_zero() {
    std::env::remove_var("SKY_LIVE_MAX_BODY_BYTES");
    assert_eq!(live_max_body_bytes(), 5 << 20);
    std::env::set_var("SKY_LIVE_MAX_BODY_BYTES", "1024");
    assert_eq!(live_max_body_bytes(), 1024);
    std::env::set_var("SKY_LIVE_MAX_BODY_BYTES", "0"); // invalid → default, not "reject everything"
    assert_eq!(live_max_body_bytes(), 5 << 20);
    std::env::remove_var("SKY_LIVE_MAX_BODY_BYTES");
}
```

### Verification
```bash
cargo test -p sky-runtime-rust --features full -- live_max_body_bytes
cargo test -p sky-runtime-rust --features full -- max_body_env_override   # server.rs's sibling stays green
cargo clippy --workspace -- -D warnings
```

Commit: `fix(live): live_max_body_bytes floors at >0, matching server::max_body (SKY_LIVE_MAX_BODY_BYTES=0 no longer 413s everything)`.

---

## 6. #33 — Http header-case parity + extra `Http` builders

**Status check (confirmed against HEAD): PARTIALLY DONE.** A prior plan,
`docs/superpowers/plans/2026-07-02-m5b-http-followups.md`, already
specified this exact item in 4 tasks. Tasks 1-3 are **landed**:

- `runtime/src/sky_runtime/http_header.rs` exists (`canonical_header`,
  shared MIME-case canonicaliser + parity-table test against Go's
  `textproto.CanonicalMIMEHeaderKey`).
- `live/req.rs:34` and `server.rs:305,465` both call it —
  `build_request` (server.rs:453-469) stores canonical keys,
  `server_header` (server.rs:298-308) canonicalises its lookup key.
  Tests `server_header_is_case_insensitive_go_parity`
  (`server.rs:1662-1689`) and `build_request_stores_canonical_header_keys`
  (`server.rs:1691-1715`) are present and green.

**Still open — two items:**

### 6.1 Outbound `Http.get`/`Http.post`/`Http.request` response headers are NOT canonicalised

**New finding (not in the 2026-07-02 plan, which scoped only the
Server-inbound path).** `runtime/src/sky_runtime/http_client.rs:296-302`:

```rust
let mut headers = HashMap::new();
for (k, v) in resp.headers() {
    if let Ok(s) = v.to_str() {
        headers.insert(k.as_str().to_string(), s.to_string());
    }
}
```

`reqwest`/the underlying `http` crate's `HeaderName::as_str()` ALWAYS
returns lower-case (confirmed: `prior-art-runtime-rust-2026-07-09.md:69-71`
flags the identical issue in the ancestor's `http_client.rs`). So
`HttpResponse.headers` — the `Dict String String` a Sky program reads via
`Dict.get "Content-Type" resp.headers` — is keyed `"content-type"`, not
`"Content-Type"`, breaking the exact parity `canonical_header` was written
to guarantee. Go stores response headers canonicalised
(`net/http.Header` always is); Sky programs porting from the Go backend
that do `Dict.get "Content-Type" resp.headers` would get `Nothing`.

**Fix**: reuse the ALREADY-SHARED `http_header::canonical_header` (it's
`pub(crate)`, and `http_client.rs` is in the same crate):

```rust
let mut headers = HashMap::new();
for (k, v) in resp.headers() {
    if let Ok(s) = v.to_str() {
        headers.insert(
            crate::sky_runtime::http_header::canonical_header(k.as_str()),
            s.to_string(),
        );
    }
}
```

**Regression test** (`http_client.rs`'s existing `#[cfg(test)] mod tests`,
`http_client.rs:429-445`) — since `do_request` needs a live HTTP round-trip
to test end-to-end, add a lighter unit assertion instead pinning the
canonicalisation call is present, PLUS extend the existing
`canonical_header_matches_go_canonical_mime_key` parity table
(`http_header.rs:48-69`) with one client-response-shaped case if not already
covered (it already has `content-type`/`x-forwarded-for` etc. — no new
case needed, just confirm via the wiring below). If the crate has (or the
task adds) an httpmock/wiremock-based integration test for `Http.get`, add:
```rust
// (integration-test harness, if present) assert response.headers contains
// "Content-Type" (canonical), not "content-type".
```
Otherwise the mechanical floor is: add a doc-comment note at the fix site
citing this parity requirement, plus rely on `canonical_header`'s own
already-passing unit tests (the transform itself is proven; this change
only wires an existing, tested function into a new call site).

### 6.2 The three missing `Http` builders — adopt the existing plan (Task 4) verbatim

`crates/skyc/stdlib/Sky/Core/Http.sky` exposes `defaultRequest` /
`withMethod` / `withHeader` / `withTimeout` / `withBody`. The Go reference
additionally exposes `withUrl`, `withFollowRedirects`, `withMaxRedirects`.
Confirmed via grep: none of the three exist anywhere in
`crates/sky_kernels/src/lib.rs` or `Http.sky` today. **Do not re-derive this
— `docs/superpowers/plans/2026-07-02-m5b-http-followups.md`'s "Task 4" (and
its prerequisite "Task 1", extracting `http_request_ty()` so all 8 builders
share one record scheme) already specify the full 10-site wiring chain,
golden fixture, and oracle-diff verification.** Execute that plan's Task 1 +
Task 4 as-written; re-verify line numbers before editing (the plan itself
flags this: kernel-registry migration churn may have shifted anchors since
2026-07-02 — grep `HttpWithHeader` across the 8 sites the plan lists before
starting, exactly as the plan's own "Global Constraints" section instructs).

One update to the existing plan given current HEAD: its Task 4 line-number
citations (`sky_kernels/src/lib.rs:359`, `:903`, `:1404`, etc.) predate the
WebSocket (#127) and Stream (#111) kernel additions that have since landed
(confirmed: `MiddlewareWithRateLimit` is now at `lib.rs:658`/`:1799`/`:2734`/
`:3240`/`:5590`, not the plan's cited numbers) — re-grep `HttpWithHeader`
fresh rather than trusting the plan's line numbers verbatim; the *shape* of
every edit (enum variant, `decl()` arm, `ALL` entry, `lower.rs` resolution,
`constrain.rs` scheme via the new `http_request_ty()` helper, `naming.rs`,
`pretty.rs`, `.sky` exposing list) is unchanged and still correct.

### Verification (whole item)
```bash
cargo build --workspace
cargo test -p sky-runtime-rust --features full   # http_header parity tests stay green
SKY_E2E=1 timeout 1800 cargo test -p skyc --test golden_m5b_http
cargo clippy --workspace -- -D warnings
cargo fmt --all --check
```

**Docs to update:** `docs/divergences-from-sky.md` — if `withUrl`/
`withFollowRedirects`/`withMaxRedirects` or the outbound header-case gap
were ever recorded there as accepted divergences, flip them to "closed";
if not recorded, no action (they were undocumented gaps, not documented
divergences).

Commit(s): `fix(http): canonicalise outbound Http response headers (MIME-case parity)`
then, executing the adopted plan, `refactor(types): single http_request_ty() scheme helper` +
`feat(http): withUrl / withFollowRedirects / withMaxRedirects builders (Go parity)`.

---

## Whole-class verification gate (run before declaring Class 8 done)

```bash
cd /home/arthur/Documentos/comp/sky-rust
cargo build --workspace
cargo test --workspace --features full 2>&1 | tee /tmp/class8-test.log
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
SKY_E2E=1 timeout 1800 cargo test -p skyc --test server_e2e
SKY_E2E=1 timeout 1800 cargo test -p skyc --test golden_m5b_http
SKY_E2E=1 timeout 1800 cargo test -p skyc --test golden_m6_server
```

Read `/tmp/class8-test.log` back rather than re-running with a broader
tail (per the repo's own logging convention) before declaring green.

## Regression-test summary (what "done" requires, per the task's ask)

| Requirement | Test |
|---|---|
| CSRF-rejects-forged-request | `csrf_post_without_header_rejected`, `csrf_post_with_mismatched_cookie_and_header_rejected` (§1.3) + `server_e2e.rs` forged-POST case (§1.3) |
| Cookie-Secure-behind-proxy | `request_is_https_honours_trusted_proxy_header` (§2) + the live-app E2E Secure-header assertion (§2) |
| CSWSH | `ws_cross_origin_detection` (§4) + `server_e2e.rs`/websocket E2E 403-vs-101 case (§4) |
| Set-Cookie clobber (prerequisite) | `two_set_cookie_headers_both_survive` (§0) |
| Ingest cross-origin log-injection | `ingest_open_dev_rejects_cross_origin_without_token` (§3) |
| `live_max_body_bytes` floor | `live_max_body_bytes_floors_at_default_on_zero` (§5) |
| Outbound header-case parity | wiring `canonical_header` at the `http_client.rs` response-header loop (§6.1), covered by existing `canonical_header_matches_go_canonical_mime_key` |
| Extra Http builders | the adopted plan's `http_builders_extra` golden (§6.2, `docs/superpowers/plans/2026-07-02-m5b-http-followups.md` Task 4 step 1-7) |
