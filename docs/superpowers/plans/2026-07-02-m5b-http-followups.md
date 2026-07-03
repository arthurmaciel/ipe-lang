# M5b-http follow-ups — header-case parity + extra Http builders

> Status: PLAN (doc-only). Written 2026-07-02 against HEAD `691e275`
> ("Thread the resolved kernel id through the parse-once seam").
> Guardian planner item #33. `|>` / `<|` are already shipped (task #36,
> completed) — **out of scope here**; this plan covers the two remaining
> M5b-http follow-ups only.

## Goal

Close the two open M5b-http parity gaps against the Go reference
(`../sky`, a capability reference — statements below say *what differs
and why*, nothing more):

1. **Header-case parity for `Sky.Http.Server`.** Inbound request headers
   reach Sky through `Server.header name req`. Go stores request headers
   in canonical MIME case (`Content-Type`) and `r.Header.Get(name)`
   canonicalises the lookup key, so `Server.header "content-type"` and
   `Server.header "Content-Type"` both resolve. The Rust runtime today
   stores hyper's lower-cased keys verbatim and does a **case-sensitive**
   `HashMap::get`, so a caller who asks for `"Content-Type"` gets
   `Nothing`. (Sky.Live already canonicalises — `live/req.rs::canonical_header`
   — so this gap is Server-only; the fix also unifies the two paths on one
   canonicaliser.)

2. **The three missing `Http` builders.** The Rust `Sky.Core.Http`
   surface exposes `defaultRequest / withMethod / withHeader / withTimeout
   / withBody`. The Go reference additionally exposes `withUrl`,
   `withFollowRedirects`, and `withMaxRedirects`. The `HttpRequest` record
   already carries `url` / `followRedirects` / `maxRedirects` fields and
   the runtime `HttpRequest` struct already has them (`http_client.rs:65`),
   so the three builders are pure record-update setters with zero runtime
   change — they are just not wired through the kernel-resolution chain.

## Architecture

### How an `Http.with*` call is resolved and emitted (the wiring chain)

`Sky.Core.Http`'s builders are **not** ordinary cross-module Sky
functions — `crates/sky_canon/src/env.rs:526-535` documents that skyc
does not resolve cross-module *pure-Sky* stdlib calls, so each builder is
registered as a **kernel** and lands as `Callee::Kernel(_)`. The `.sky`
record-update bodies (`crates/skyc/stdlib/Sky/Core/Http.sky:93-116`) are
therefore non-authoritative for codegen — they exist for `sky doc` and
signature discovery; the emitter synthesises the record update directly.
(Resolved ambiguity — see Global Constraints.)

Adding one builder touches this exact chain (grep-verified against HEAD —
every site where `HttpWithHeader` appears):

| # | File:anchor | Role |
|---|---|---|
| 1 | `crates/sky_canon/src/env.rs:537-548` | `("Http", &[…])` QUALIFIERS list → makes `Http.foo` a resolvable var |
| 2 | `crates/sky_kernels/src/lib.rs:351-359` | `KernelFn` enum variants |
| 3 | `crates/sky_kernels/src/lib.rs:893-903` | `decl()` arm → `StdlibDecl { module, name, arity, effect, emit_symbol }` |
| 4 | `crates/sky_kernels/src/lib.rs:1396-1404` | `ALL` slice (canon invariant checks iterate it) |
| 5 | `crates/sky_lower/src/lower.rs:3845-3853` | `("Http", name) => Ok(Callee::Kernel(_))` resolution |
| 6 | `crates/sky_types/src/constrain.rs:2716-2820` | per-name type scheme (the `HttpRequest` record type) |
| 7 | `crates/sky_backend_rust/src/naming.rs:513-521` | `kernel_name()` total match (emit symbol) |
| 8 | `crates/sky_ir/src/pretty.rs:469-477` | IR pretty-printer total match |
| 9 | `crates/sky_backend_rust/src/emit_expr.rs:308-435` | `emit_http_builder_call` — the actual struct-update emission |
| 10 | `crates/skyc/stdlib/Sky/Core/Http.sky:18-30, 93-116` | `exposing` + `.sky` decl (doc/signature parity) |

`kernel_name()` (site 7) and `pretty.rs::n` (site 8) are **exhaustive
matches over `KernelFn` with no `_` catch-all** — a new enum variant that
misses either fails to compile. That is the fail-closed floor for sites
2/7/8. Canon self-tests (`crates/sky_canon/src/lib.rs:1401-1478`) enforce
`qual_vars ⟺ stdlib_index` and id-agreement, so a builder added to
QUALIFIERS (site 1) but missing from `ALL`/`decl()` (sites 3/4) trips a
canon test, not a silent miscompile. A `decl()` injectivity test
(`crates/sky_kernels/src/lib.rs:1819-1852`) rejects a duplicated
`(module, name)`.

### Header canonicalisation

`live/req.rs::canonical_header` (`runtime/src/sky_runtime/live/req.rs:61-72`)
title-cases a `-`-separated name (`content-type` → `Content-Type`). It is
private to `live`. This plan **hoists it to a shared runtime module**
(`runtime/src/sky_runtime/http_header.rs`, `pub(crate)`), so Live and
Server share one canonicaliser — a single source of truth (two divergent
canonicalisers would be an unrepresentable-invalid state made
representable). Server then:

* canonicalises on **insert** in `build_request`
  (`runtime/src/sky_runtime/server.rs:453`), aligning the internal map with
  Go's storage and the Live path; and
* canonicalises the **lookup key** in `server_header`
  (`runtime/src/sky_runtime/server.rs:298-303`), matching Go's
  `Header.Get` case-folding — the load-bearing Sky-visible fix.

Internal `eq_ignore_ascii_case` scans (`content-length`, `x-forwarded-for`,
`x-real-ip` at `server.rs:504-506, 480, 486`) keep working under either
casing — no regression. Cookies are **not** canonicalised: RFC 6265
cookie names are case-sensitive and Go treats them so (`server.rs:423-441`
untouched).

Parity nuance recorded (not a blocker): Go's
`textproto.CanonicalMIMEHeaderKey` returns the key **unchanged** when it
contains a byte outside the header-token set (e.g. a space); the current
`canonical_header` always title-cases. For a well-formed header this is
byte-identical to Go; the malformed-name case is an accepted divergence
(hyper has already validated the name, and canonicalising a would-be
invalid name is harmless — no injection surface). Task 2's parity table
documents it.

## Tech Stack

- **Rust** (workspace crates: `sky_kernels`, `sky_canon`, `sky_lower`,
  `sky_types`, `sky_backend_rust`, `sky_ir`, `skyc`; runtime crate
  `sky_runtime`).
- **Golden parity harness**: `crates/skyc/tests/golden_m5b_http.rs` +
  `crates/skyc/tests/support/mod.rs` + `tools/oracle` +
  `tools/refresh-oracle`. Go oracle binary:
  `/home/arthur/Documentos/comp/sky/sky-out/sky` (override with
  `SKY_GO_ORACLE`). E2E goldens gated on `SKY_E2E=1`; shared cargo target
  `~/.cache/sky-rust-target`.
- **Runtime tests**: in-crate `#[cfg(test)]` unit modules
  (`server.rs`, the new `http_header.rs`).

## Global Constraints

- **PRINCIPLES order (strict tie-break):** security > correctness >
  soundness > efficiency > completeness > readability. When two options
  trade off, the higher-ranked principle wins.
- **PARSE, DON'T VALIDATE.** The runtime `http_client`/`server` boundary
  is the parse point: it turns wire bytes into typed records
  (`HttpRequest` / `ServerRequest`) once. Header canonicalisation happens
  at that boundary so downstream Sky code reads a single canonical form
  and never re-checks casing.
- **MAKE INVALID STATES UNREPRESENTABLE.** One canonicaliser shared by
  Live + Server (not two that can drift). The `HttpRequest` type scheme is
  produced by **one** helper (Task 1) so all eight builders return a record
  of identical shape — a builder whose scheme omits a field would make
  well-typed programs unrepresentable/rejected.
- **Fail-closed diagnostics, not panics/wildcards.** Every new
  `emit_http_builder_call` arm returns `Err(Diagnostic::CompilerBug{…})` on
  arity mismatch (mirroring the existing arms), never `panic!`/`unwrap`.
  No `_ =>` catch-alls added to the exhaustive `KernelFn` matches.
- **Parallel-safety / file-overlap (READ THIS BEFORE SCHEDULING):**
  - **Tasks 2 & 3 are runtime-only** (`live/req.rs`, `server.rs`, new
    `http_header.rs`) and are **disjoint** from every in-flight compiler
    migration — safe to run fully in parallel with the registry migration
    and with #49 TCO.
  - **Tasks 1 & 4 collide with the kernel-registry migration.** Sites
    `constrain.rs` (Task 1 + 4 site 6), `sky_kernels/src/lib.rs` (enum +
    `decl()` + `ALL`, site 2/3/4), and `sky_lower/src/lower.rs` callee
    (site 5) are the *same files* the registry migration is actively
    editing (`691e275` touched the parse-once seam through
    `lower.rs`/`constrain.rs`/`env.rs`/`sky_kernels`). **Serialise Task 4
    after the current registry-migration phase lands**, or rebase Task 4
    onto it — do not run them concurrently on those four files.
  - **#49 TCO overlap:** #49 adds two `sky_ir` variants + edits
    `lower.rs` and `emit_expr.rs`. This plan edits **different regions** of
    `lower.rs` (the `("Http", _)` match block, ~3845) and `emit_expr.rs`
    (the `emit_http_builder_call` fn, ~308) and adds **no** `sky_ir` enum
    variants (only a `pretty.rs` match arm). Overlap is line-adjacent, not
    semantic — resolve by rebase; no design conflict.
- **No `sky build` from repo root** (overwrites the compiler binary). All
  builds run via `cargo`/`cargo test` from the workspace root, which is
  safe.
- **Commit discipline:** one commit per task; branch off `main` (current
  branch is `master` per repo, but PR target is `main`). No AI/co-author
  trailer.

---

## Task 1 — Extract a single `http_request_ty()` scheme helper (soundness/DRY, no behavior change)

**Why first:** `constrain.rs:2716-2820` rebuilds the identical seven-field
`HttpRequest` record inline in every builder arm. Task 4 would add three
more copies. Divergence risk is real: if one copy omits `http_f_url`, that
builder's result type unifies wrongly and rejects valid programs. Collapse
to one helper so all eight builders are provably the same shape
(make-invalid-states-unrepresentable). Pure refactor — the emitted schemes
are byte-identical, so existing goldens stay green.

**Files:**
- `crates/sky_types/src/constrain.rs`

**Interfaces:**
- Consumes: `self.builtins.http_f_{body,follow_redirects,headers,max_redirects,method,timeout,url}` (existing `Symbol` fields), the local `string`/`int`/`bool_ty` builders, `list`, `tuple2`, `Ty::Record`.
- Produces:
  ```rust
  /// The `Sky.Core.Http.HttpRequest` record type — the single source of
  /// truth for the field set shared by `defaultRequest` and every `with*`
  /// builder. `bool_ty` is consumed by value, so callers pass a fresh clone.
  fn http_request_ty(&self, string: &Ty, int: &Ty, bool_ty: Ty) -> Ty;
  ```
  (Signature mirrors how the existing arms hold `string`/`int` by clone and
  move `bool_ty` once. If the surrounding `constrain` method already owns
  reusable `string`/`int`, adapt to `&self`-only and clone internally —
  verify the exact ownership at implementation time against the live arms.)

**Steps:**
1. Write a failing assertion first. Add a unit test in `constrain.rs`'s
   `#[cfg(test)]` module that constructs the scheme for `Http.defaultRequest`
   and `Http.withMethod` and asserts both request-record types are
   structurally equal (same `BTreeMap` field set + field types):
   ```rust
   #[test]
   fn http_builder_schemes_share_one_record_shape() {
       let c = /* construct the Constrain ctx as the sibling tests do */;
       let d = c.kernel_scheme(Some("Http"), Some("defaultRequest")).unwrap();
       let m = c.kernel_scheme(Some("Http"), Some("withMethod")).unwrap();
       // result of defaultRequest == arg2/result of withMethod (the HttpRequest record)
       assert_eq!(record_of_result(&d), record_of_arg2(&m));
   }
   ```
   Use the exact scheme-lookup fn name found at the `(Some("Http"), …)`
   match site (read the enclosing `fn` header near `constrain.rs:2646`
   before writing — correct the helper names in the test to match HEAD).
2. Run it — it should compile-fail or fail only if a helper name is wrong;
   the point is to lock the invariant. `cargo test -p sky_types
   http_builder_schemes_share_one_record_shape`. Expected first run: **fails
   to compile** (helper `http_request_ty` does not yet exist if the test
   calls it) or **passes trivially** (if it only compares existing arms).
   Prefer the version that fails until step 3 — have the test call
   `c.http_request_ty(...)` and compare to the existing arms' output.
3. Add `http_request_ty(&self, …)` populating the seven fields exactly as
   the current `defaultRequest` arm does (`constrain.rs:2718-2731`). Rewrite
   all five existing arms (`defaultRequest`, `withMethod`, `withTimeout`,
   `withBody`, `withHeader`) to call it, preserving each arm's outer
   `fun(...)` arrow shape.
4. Run: `cargo test -p sky_types` — expected: the new test passes, all
   existing `sky_types` tests pass.
5. Run the golden regression to prove byte-identical schemes:
   `SKY_E2E=1 cargo test -p skyc --test golden_m5b_http` — expected: the
   three existing http goldens (`http_parse_query`, `http_builders`,
   `http_response_fields`) still pass.
6. `cargo fmt` + `cargo clippy -p sky_types -- -D warnings`.
7. Commit: `refactor(types): single http_request_ty() scheme helper for Http builders`.

---

## Task 2 — Hoist `canonical_header` to a shared runtime module + parity table

**Parallel-safe** (runtime-only; no compiler-crate overlap).

**Files:**
- `runtime/src/sky_runtime/http_header.rs` (new)
- `runtime/src/sky_runtime/mod.rs` (add `pub mod http_header;` near the
  other http mods at `mod.rs:127-131`)
- `runtime/src/sky_runtime/live/req.rs` (replace the private
  `canonical_header` with a re-use of the shared fn)

**Interfaces:**
- Produces:
  ```rust
  // runtime/src/sky_runtime/http_header.rs
  /// Canonical MIME header-name casing (`content-type` -> `Content-Type`),
  /// matching Go's `textproto.CanonicalMIMEHeaderKey` for well-formed names.
  /// Shared by the Sky.Live request builder and the Sky.Http.Server request
  /// builder + `Server.header` lookup so both paths agree on one casing.
  pub(crate) fn canonical_header(k: &str) -> String;
  ```
- Consumes (in `live/req.rs`): the new `canonical_header` in place of the
  local copy at `req.rs:61-72`.

**Steps:**
1. Create `http_header.rs` with the fn body moved verbatim from
   `live/req.rs:61-72`. Add a `#[cfg(test)]` **parity table** test:
   ```rust
   #[test]
   fn canonical_header_matches_go_canonical_mime_key() {
       for (input, want) in [
           ("content-type", "Content-Type"),
           ("CONTENT-TYPE", "Content-Type"),
           ("x-forwarded-for", "X-Forwarded-For"),
           ("etag", "Etag"),                       // Go: Etag, not ETag
           ("www-authenticate", "Www-Authenticate"),
           ("host", "Host"),
           ("a", "A"),
       ] {
           assert_eq!(canonical_header(input), want, "input {input:?}");
       }
   }
   ```
   These `want` values are `textproto.CanonicalMIMEHeaderKey` outputs
   (simple upper-after-hyphen; no `commonHeader` special-case changes them).
2. Run: `cargo test -p sky_runtime canonical_header_matches_go` — expected:
   **fails to compile** (module not yet wired into `mod.rs`).
3. Add `pub mod http_header;` to `mod.rs`. Re-run — expected: **passes**.
4. In `live/req.rs`: delete the private `canonical_header` (lines 61-72) and
   its now-unused status; call
   `crate::sky_runtime::http_header::canonical_header(k.as_str())` at
   `req.rs:34`. Keep the existing `live_req_parses_headers_and_cookies` test.
5. Run: `cargo test -p sky_runtime live_req_parses_headers_and_cookies
   canonical_header_matches_go` — expected: both pass (Live behaviour
   unchanged; the `X-Custom` assertion at `req.rs:98` still holds).
6. `cargo fmt` + `cargo clippy -p sky_runtime -- -D warnings`.
7. Commit: `refactor(runtime): share canonical_header between Live and Server`.

---

## Task 3 — Canonicalise `Sky.Http.Server` request headers (parity)

**Parallel-safe** (runtime-only). Depends on Task 2 (the shared fn).

**Files:**
- `runtime/src/sky_runtime/server.rs`

**Interfaces:**
- Consumes: `crate::sky_runtime::http_header::canonical_header`.
- Changes (no signature change):
  - `build_request` (`server.rs:448-455`): store
    `canonical_header(k.as_str())` as the map key instead of
    `k.as_str().to_string()`.
  - `server_header` (`server.rs:298-303`): look up
    `req.headers.get(&canonical_header(&name))` instead of `.get(&name)`.

**Steps:**
1. Write failing tests first. Add a `#[cfg(test)]` module in `server.rs`:
   ```rust
   #[test]
   fn server_header_is_case_insensitive_go_parity() {
       let mut headers = std::collections::HashMap::new();
       // simulate what build_request now stores (canonical key):
       headers.insert("Content-Type".to_string(), "application/json".to_string());
       let req = ServerRequest { /* minimal: headers, empty others */ };
       // Go's r.Header.Get canonicalises the lookup key:
       assert!(matches!(server_header("content-type".into(), req.clone()),
                        SkyMaybe::Just(ref v) if v == "application/json"));
       assert!(matches!(server_header("Content-Type".into(), req.clone()),
                        SkyMaybe::Just(ref v) if v == "application/json"));
       assert!(matches!(server_header("CONTENT-TYPE".into(), req),
                        SkyMaybe::Just(ref v) if v == "application/json"));
   }
   ```
   Construct `ServerRequest` via whatever constructor/field-init the struct
   at `server.rs:34-56` allows (read it; if fields are `pub` use a literal,
   else add a `#[cfg(test)]` builder). Also add
   `build_request_canonicalises_keys` if `build_request` is unit-reachable;
   otherwise cover storage casing via the e2e in step 5.
2. Run: `cargo test -p sky_runtime server_header_is_case_insensitive`
   — expected: **fails** (current `.get(&name)` returns `Nothing` for
   `"content-type"` when the map holds `"Content-Type"`).
3. Apply the two edits (build_request insert key; server_header lookup key).
4. Run the same test — expected: **passes**. Run the full server module
   tests: `cargo test -p sky_runtime server` — expected: green (the
   internal `eq_ignore_ascii_case` scans are casing-agnostic; no regression).
5. If `crates/skyc/tests/server_e2e.rs` spins a real server (verify — no
   header-parity test exists there today), add an e2e that sends a request
   with a lower-cased custom header and asserts a handler calling
   `Server.header "X-Trace-Id"` (canonical) sees the value. Gate on
   `SKY_E2E=1`. If the harness cannot inject arbitrary request headers,
   the unit test in step 1 is the mechanical floor — note that in the test
   doc-comment and stop here (do not build header injection into the
   harness in this task).
6. `cargo fmt` + `cargo clippy -p sky_runtime -- -D warnings`.
7. Commit: `fix(server): canonical-case header parity for Server.header (Go r.Header.Get)`.

---

## Task 4 — Wire the three extra `Http` builders (`withUrl`, `withFollowRedirects`, `withMaxRedirects`)

**COLLIDES with the kernel-registry migration** on `sky_kernels/src/lib.rs`,
`constrain.rs`, `sky_lower/src/lower.rs`. **Serialise after / rebase onto the
current migration phase.** Depends on Task 1 (`http_request_ty`).

Add three pure builders, each a one-field record update on `HttpRequest`:

| Builder | Sky type | Emit | KernelFn | emit_symbol |
|---|---|---|---|---|
| `withUrl` | `String -> HttpRequest -> HttpRequest` | `__sky_rec.url = <s>` | `HttpWithUrl` | `http_with_url` |
| `withFollowRedirects` | `Bool -> HttpRequest -> HttpRequest` | `__sky_rec.followRedirects = <b>` | `HttpWithFollowRedirects` | `http_with_follow_redirects` |
| `withMaxRedirects` | `Int -> HttpRequest -> HttpRequest` | `__sky_rec.maxRedirects = <n>` | `HttpWithMaxRedirects` | `http_with_max_redirects` |

Reference: `../sky/sky-stdlib/Sky/Core/Http.sky:117-134` (the Go reference
defines exactly these three with the same `{ req | field = x }` bodies and
`withMaxRedirects` negative → default-cap semantics; the Rust runtime floor
lives in `http_client.rs:189-208, 261-266`, so no runtime change needed).

**Files & exact edits (all 8 wiring sites + stdlib):**

- **`crates/sky_kernels/src/lib.rs`**
  - Enum (after `HttpWithHeader`, `:359`): add `HttpWithUrl,
    HttpWithFollowRedirects, HttpWithMaxRedirects,`.
  - `decl()` (after `:903`): add
    ```rust
    Self::HttpWithUrl => d("Http", "withUrl", 2, Pure, "http_with_url"),
    Self::HttpWithFollowRedirects =>
        d("Http", "withFollowRedirects", 2, Pure, "http_with_follow_redirects"),
    Self::HttpWithMaxRedirects =>
        d("Http", "withMaxRedirects", 2, Pure, "http_with_max_redirects"),
    ```
  - `ALL` (after `:1404`): add the three `Self::HttpWith*,` entries.
- **`crates/sky_lower/src/lower.rs`** (after `:3853`):
  ```rust
  ("Http", "withUrl") => Ok(Callee::Kernel(KernelFn::HttpWithUrl)),
  ("Http", "withFollowRedirects") => Ok(Callee::Kernel(KernelFn::HttpWithFollowRedirects)),
  ("Http", "withMaxRedirects") => Ok(Callee::Kernel(KernelFn::HttpWithMaxRedirects)),
  ```
- **`crates/sky_types/src/constrain.rs`** (after `:2820`), using Task 1's
  helper:
  ```rust
  (Some("Http"), Some("withUrl")) => {
      let req = self.http_request_ty(&string, &int, bool_ty);
      fun(string, fun(req.clone(), req))
  }
  (Some("Http"), Some("withFollowRedirects")) => {
      let req = self.http_request_ty(&string, &int, bool_ty.clone());
      fun(bool_ty, fun(req.clone(), req))
  }
  (Some("Http"), Some("withMaxRedirects")) => {
      let req = self.http_request_ty(&string, &int, bool_ty);
      fun(int, fun(req.clone(), req))
  }
  ```
  (Adjust `string`/`int`/`bool_ty` ownership to whatever the live arms use;
  the arg type is `String`/`Bool`/`Int` respectively, then two `HttpRequest`s.)
- **`crates/sky_backend_rust/src/naming.rs`** (after `:521`, exhaustive
  `kernel_name`): add
  ```rust
  KernelFn::HttpWithUrl => "http_with_url",
  KernelFn::HttpWithFollowRedirects => "http_with_follow_redirects",
  KernelFn::HttpWithMaxRedirects => "http_with_max_redirects",
  ```
- **`crates/sky_ir/src/pretty.rs`** (after `:477`, exhaustive `n`): add
  ```rust
  KernelFn::HttpWithUrl => "Http.withUrl",
  KernelFn::HttpWithFollowRedirects => "Http.withFollowRedirects",
  KernelFn::HttpWithMaxRedirects => "Http.withMaxRedirects",
  ```
- **`crates/sky_backend_rust/src/emit_expr.rs`**
  - Guard (`:316-322`): extend the `k @ ( … )` pattern with the three new
    variants.
  - Arms (after `:416`, before `HttpWithHeader`): add clone-and-reassign
    arms mirroring `HttpWithMethod` (`:366-382`), e.g.
    ```rust
    KernelFn::HttpWithUrl => {
        let u = args.first().ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_http_builder_call",
            detail: "HttpWithUrl expects 2 arguments (url, req)".to_owned(),
        })?;
        let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_http_builder_call",
            detail: "HttpWithUrl expects 2 arguments (url, req)".to_owned(),
        })?;
        let u_s = emit_expr_at(ctx, u, indent, child, generics)?;
        let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
        Ok(Some(format!(
            "{{ let mut __sky_rec = ({req_s}).clone(); __sky_rec.url = {u_s}; __sky_rec }}"
        )))
    }
    ```
    `HttpWithFollowRedirects` → `.followRedirects = {b_s}`;
    `HttpWithMaxRedirects` → `.maxRedirects = {n_s}`. No `panic!`/`unwrap`.
- **`crates/skyc/stdlib/Sky/Core/Http.sky`**
  - `exposing` (`:18-30`): add `withUrl`, `withFollowRedirects`,
    `withMaxRedirects`.
  - Append three builders after `withHeader` (`:116`), copying the Go
    reference bodies (`{ req | url = url }` etc.) with doc comments. These
    `.sky` bodies are non-authoritative for codegen (kernel path wins) but
    provide the doc/signature surface; keep them consistent with the
    existing five (record-update form).

**Interfaces (net new):**
- `KernelFn::{HttpWithUrl, HttpWithFollowRedirects, HttpWithMaxRedirects}` —
  `Pure`, arity 2, emit symbols `http_with_{url,follow_redirects,max_redirects}`.
- Type schemes as in the `constrain.rs` block above.
- No runtime symbol added (the emitter synthesises the struct update inline;
  `http_client.rs` `HttpRequest` already has the fields).

**Steps:**
1. Write the failing golden first (this is the discovery artefact). Create
   `tests/golden/m5b_http_builders_extra/Main.sky`:
   ```elm
   module Main exposing (main)

   import Sky.Core.Http as Http


   printReq : { body : String, followRedirects : Bool, headers : List ( String, String ), maxRedirects : Int, method : String, timeout : Int, url : String } -> String
   printReq req =
       String.join "\n"
           [ req.url
           , req.method
           , if req.followRedirects then "follow" else "nofollow"
           , String.fromInt req.maxRedirects
           ]


   main =
       println
           (printReq
               (Http.withMaxRedirects 3
                   (Http.withFollowRedirects False
                       (Http.withUrl "http://redirected.example"
                           (Http.defaultRequest "http://example.com")))))
   ```
   Expected program output:
   ```
   http://redirected.example
   GET
   nofollow
   3
   ```
2. Register a test fn in `crates/skyc/tests/golden_m5b_http.rs` (mirror
   `http_builders`, `:108-117`):
   ```rust
   #[test]
   fn http_builders_extra() {
       assert_runs_and_matches_oracle("m5b_http_builders_extra");
   }
   ```
3. Run without the wiring: `SKY_E2E=1 cargo test -p skyc --test
   golden_m5b_http http_builders_extra` — expected: **fails** at
   `skyc::build` (unresolved `Http.withUrl` / non-exhaustive match won't
   even compile the compiler until sites 7/8 are done — so before wiring,
   the failure is a skyc *name-resolution* error on `Http.withUrl`).
4. Apply all edits above. Build the compiler: `cargo build -p skyc` —
   expected: compiles (exhaustive `kernel_name` + `pretty::n` now cover the
   three variants; canon `ALL`/QUALIFIERS agree so self-tests pass).
5. Run compiler self-tests that guard the chain:
   `cargo test -p sky_kernels && cargo test -p sky_canon` — expected: the
   `decl()` injectivity test and the `qual_vars ⟺ stdlib_index` invariant
   tests (`sky_canon/src/lib.rs:1401-1478`) pass.
6. Generate the **true Go oracle** (the Go reference has all three
   builders): from repo root,
   `cargo run -p refresh-oracle -- m5b_http_builders_extra`
   (uses `/home/arthur/Documentos/comp/sky/sky-out/sky`; override via
   `SKY_GO_ORACLE`). Expected: writes `expected_go.txt` (the four lines
   above) + `oracle.meta` with `oracle_divergence = false`. If the Go
   binary is unavailable, the tool records skyc's own output with
   `oracle_divergence = true` + reason — acceptable fallback, but prefer the
   true oracle; note which was used in the commit body.
7. Run the golden: `SKY_E2E=1 cargo test -p skyc --test golden_m5b_http`
   — expected: all four http goldens pass (three existing + `http_builders_extra`).
8. `cargo fmt --all` + `cargo clippy --workspace -- -D warnings`.
9. Commit: `feat(http): withUrl / withFollowRedirects / withMaxRedirects builders (Go parity)`.

---

## Verification (whole-plan gate, before PR)

Run from workspace root; every command timeout-bounded per project rules.

1. `cargo build --workspace` — expected: clean (exhaustive matches force
   completeness).
2. `cargo test --workspace` — expected: green; new tests
   (`http_builder_schemes_share_one_record_shape`,
   `canonical_header_matches_go_canonical_mime_key`,
   `server_header_is_case_insensitive_go_parity`) present and passing.
3. `SKY_E2E=1 timeout 1800 cargo test -p skyc --test golden_m5b_http`
   — expected: 4 http goldens pass.
4. `cargo clippy --workspace -- -D warnings` + `cargo fmt --all --check`.
5. `sky doc Http` surfaces the three new builders (they are in `Http.sky`'s
   `exposing`) — optional manual check.

## Docs to update in the same PR (template-sync rule)

- `docs/stdlib.md` — `Sky.Core.Http` builder list: add `withUrl` /
  `withFollowRedirects` / `withMaxRedirects`.
- If a Sky.Http.Server `Server.header` doc exists under `docs/`, note the
  case-insensitive (Go `Header.Get`) lookup parity.

## Resolved ambiguities (to keep the plan mechanical)

1. **Are the `Http` builders pure Sky or kernels?** Kernels. Grep-confirmed
   the `.sky` uses `Ffi.kernel` only for `get/post/request/parseQuery`
   (`Http.sky:60,65,70,75`); the builders are registered as kernels via
   `env.rs:537` QUALIFIERS + `lower.rs:3849-3853` + emitted by
   `emit_http_builder_call`. The `.sky` record-update bodies are
   doc/signature-only. New builders therefore need the full 8-site chain,
   **not** a `.sky`-only edit.
2. **`withMaxRedirects` runtime semantics.** No runtime change: the
   `HttpRequest.maxRedirects` field already flows to `ssrf_apply`
   (`http_client.rs:191`), and a negative value is floored by
   `max_redirects.max(0)` there — same effective behaviour as the Go
   "negative → default cap" note.
3. **Header-case: canonicalise storage AND lookup, or lookup only?** Both.
   Lookup canonicalisation is the load-bearing Sky-surface fix; storage
   canonicalisation aligns the Server map with Go and with the Live path
   (one casing → make-invalid-states). Internal `eq_ignore_ascii_case`
   scans are casing-agnostic, so no regression.
4. **Cookies untouched** — RFC 6265 cookie names are case-sensitive; Go
   matches them case-sensitively.
5. **`textproto` invalid-byte edge** — accepted divergence (well-formed
   headers are byte-identical to Go; malformed names are harmless to
   title-case since hyper has already validated them). Documented in Task 2's
   parity table, not gated.
