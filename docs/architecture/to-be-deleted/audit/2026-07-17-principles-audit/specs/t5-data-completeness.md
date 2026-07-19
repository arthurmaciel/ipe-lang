# T5 — data/decode + completeness

Design + implementation plan for the six T5 findings:

| ID | axis | one-line |
|---|---|---|
| RT-DATA-001 | correctness | Postgres BOOL/BYTEA/NUMERIC/TIMESTAMP columns silently collapse to NULL/"" |
| RT-DATA-003 | security (low) | `findByConditions` empty conditions → `SELECT *` fail-open (cross-tenant read) |
| CO-INCR-005 | completeness | `ipe watch` / `ipe lsp` never wire the FFI catalog → FFI projects red-loop |
| RT-NET-001 | completeness | every production `wss://` client dial fails-closed (no TLS backend) |
| RT-UI-002 | completeness | `Keyed.column`/`Keyed.row` drop the key instead of attaching `ipe-key` |
| CO-BACKEND-003 | correctness | routed Live `:param` decode `unwrap_or_default` → bad URL becomes `id 0`, not `notFound` |

## Theme root cause

There is no single generative defect; the theme is **the boundary decode /
capability-wiring surface accepts more inputs than it can faithfully carry, and
resolves the shortfall with a silent default instead of failing closed or
carrying the value with fidelity.** Five of the six are one instance each of
that pattern:

- a driver-typed cell that the untyped bridge can't read → `Null`/`""` (RT-DATA-001);
- an empty condition set → an unscoped `SELECT *` (RT-DATA-003);
- a missing FFI-injection call on two of the three drivers → unresolved imports
  (CO-INCR-005);
- an unparseable numeric `:param` → the type's `Default` (CO-BACKEND-003);
- a key dropped rather than attached (RT-UI-002).

RT-NET-001 is the inverse — a capability that fails **closed** correctly but is
un-usable in the config that needs it (production TLS), so the fix is either to
supply the missing capability (a rustls dialer) or to record the limitation.

Each fix below chooses the principled move for its case: carry the value
faithfully, fail closed, wire the capability, or (RT-NET-001) supply it and
record the residual. Two divergence-ledger defects are also closed: the phantom
`§B-Keyed` citation (RT-UI-002) and the stale `§B-route-param` residual note
(CO-BACKEND-003).

---

## RT-DATA-001 — Postgres column decode fidelity

### Root cause
`row_to_json` (`src/runtime/rust/src/db.rs:220-241`) and `row_to_map`
(`db.rs:186-205`) probe every column through exactly three `try_get`
attempts — `Option<String>` → `Option<i64>` → `Option<f64>` — and on the
fall-through arm return `JsonVal::Null` / `String::new()`. `db.rs` is
byte-identical across the sqlite and postgres builds (only `config.rs`/
`config_postgres.rs` supply the `DbRow` alias, per
`project.rs:668-678`), so these two bridges must decode driver-generically.
On sqlite's dynamic typing a value almost always round-trips through one of the
three probes; on the strict-decode postgres driver a non-NULL
`BOOLEAN`/`BYTEA`/`NUMERIC`/`TIMESTAMP` cell decodes as none of the three and
collapses to `Null`. Composed with `db_decode_nullable` (`db.rs:469`) this turns
a present value into a trusted `Nothing` — a correctness/data-integrity bug, not
a panic. The runtime can itself *write* a `BYTEA` column (`SqlParam::Bytes`,
`db.rs:1759`) that it then cannot read back.

### Design — widen the typed-probe chain to the SQL scalar set
The decoders downstream (`db_decode_bool` at `db.rs:355`, `db_decode_money`,
`db_decode_int`, …) already accept the string/`Bool`/`Number` `JsonVal` forms,
so the fix is confined to the two row bridges: extend the probe chain so every
SQL scalar type has a faithful `JsonVal` (or map-string) representation, and
make the *ordering* correct so a broad type never steals a narrower one.

Add one shared helper that both bridges call, replacing the ad-hoc nested match:

```rust
/// Decode column `i` into a `JsonVal`, trying the SQL scalar types in an order
/// that never lets a broad reader shadow a narrower one. `Ok(None)` at any arm
/// is SQL NULL → `JsonVal::Null`. The final arm is the honest failure: a driver
/// type none of the readers cover returns `Err`, surfaced by the caller as a
/// decode error rather than a silent Null.
fn column_to_json(row: &DbRow, i: usize) -> Result<JsonVal, sqlx::Error>
```

Probe order (first `Ok` wins; each arm distinguishes `Ok(None)` = NULL):

1. `Option<bool>`     → `JsonVal::Bool`
2. `Option<i64>`      → `JsonVal::Number`
3. `Option<f64>`      → `JsonVal::Number`
4. `Option<String>`   → `JsonVal::String`
5. `Option<Vec<u8>>`  → `JsonVal::String(hex_encode(bytes))` (BYTEA/BLOB; hex is
   the lossless, driver-neutral text form — pairs with a new `Db.Decode.bytes`)
6. NUMERIC / TIMESTAMP: sqlx exposes these behind driver-specific types
   (`sqlx::types::Decimal`, `time`/`chrono`). Decode them to their
   canonical string via `try_get::<Option<String>, _>` is NOT reliable on
   postgres (postgres refuses the TEXT cast at decode). Use the sqlx feature
   types already enabled for the driver and `to_string()` them into
   `JsonVal::String` so `db_decode_money` / a future `db_decode_time` can parse.

**bool before i64** is load-bearing: on postgres a `BOOLEAN` will NOT decode as
`i64`, but on sqlite a boolean is stored as `0`/`1` INTEGER and WOULD decode as
`i64` first — which is fine because `db_decode_bool` already accepts a numeric
`0`/`1` (`db.rs:361`). Probing `bool` first keeps postgres faithful without
regressing sqlite. **string is demoted below the numeric probes** on the JSON
bridge to match postgres's stricter decode (a postgres INTEGER does not decode
as String); the current code has String first, which is why it never reaches the
numeric arms on sqlite — acceptable there because sqlite lets String read an
INTEGER, but the reorder makes both drivers agree.

The final fallback becomes `Err(sqlx::Error::ColumnDecode{...})` for `row_to_json`
(the typed-decoder path — the decoder run then surfaces a real decode error
instead of a phantom NULL). `row_to_map` (the untyped `db_query` path returning
`HashMap<String,String>`) keeps a total contract but uses the widened chain, and
its final fallback stays `String::new()` (that path has no typed consumer to
distinguish NULL from empty — documented at `db.rs:209`).

Add the missing reader kernel `Db.Decode.bytes : String -> Decoder Bytes` (a
`db_decode_bytes` mirroring `db_decode_string`, hex-decoding the column string
to `Vec<u8>`), closing the write-without-read asymmetry the verdict flags.

**Divergence:** none — this is a correctness fix toward the Go oracle (which
reads every column faithfully). No ledger entry.

### Impl plan
1. `db.rs`: add `fn column_to_json(row, i) -> Result<JsonVal, sqlx::Error>` and
   `fn column_to_string(row, i) -> String` (the widened `row_to_map` arm);
   rewrite `row_to_json`/`row_to_map` to loop calling them. `row_to_json`
   propagates the `Err` (change its signature to `Result<JsonVal, sqlx::Error>`
   and its two call sites `db_query_decode`/`db_get_by_id_decode` to `?` it into
   `ipe_err`). **Test:** `row_to_json` unit test over a sqlite in-memory row with
   a `BLOB` column bound via `SqlParam::Bytes` → asserts hex `JsonVal::String`,
   not `Null`; a `bool`-typed column asserts `JsonVal::Bool`.
2. `db.rs`: add `db_decode_bytes` + register the `Db.Decode.bytes` kernel
   (kernel registry + `ipe doc`). **Test:** round-trip — `SqlBytes`-write then
   `db_decode_bytes`-read yields the original bytes (sqlite).
3. Kernel-registry + backend routing entry for `Db.Decode.bytes`; **negative
   test** in `src/ipe-cli/tests/negative_suite.rs` is not applicable (this is an
   additive kernel, not a rejection) — instead add a positive SEAL example under
   `examples/` (or extend an existing db example) that writes+reads a bytes
   column and asserts the value survives.
4. **Postgres-tier caveat (document in the spec-linked task, not code):** the
   runtime unit-test config compiles `DbRow = SqliteRow`, so the postgres decode
   path is only exercisable behind a live postgres. Add a `#[cfg(feature =
   "pg-integration")]`-gated (ignored-by-default) integration test that, given
   `IPE_TEST_PG_URL`, writes a `BOOLEAN`/`BYTEA`/`NUMERIC`/`TIMESTAMP` row and
   asserts every column decodes non-Null. This is the only way to pin the
   driver-specific bite; keep it out of the default gate.

### Risk / blast radius
`row_to_json`'s signature change touches its two callers only. Reordering the
probe chain could change an *untyped* `db_query` result for a column that used
to read as String but now reads as Number on sqlite — audit: numbers stringify
identically (`n.to_string()`), so `HashMap` values are unchanged; JSON bridge
values change type (String→Number) which is what the typed decoders want. Re-gate:
`cargo nextest -p ipe-runtime-rust --features full` (db tests), the db examples
in the sweep. No golden impact (runtime-only).

---

## RT-DATA-003 — `findByConditions` empty → fail closed

### Root cause
`db_find_by_conditions` (`db.rs:1475-1476`) emits `SELECT * FROM {table}` when
`keys.is_empty()`, returning every row. Reachable when an app builds `conditions`
from request-derived filters that come back empty → a cross-tenant read in a
multi-tenant app. This is the exact asymmetric twin of the already-fixed
fail-closed `db_update_fields` (`db.rs:2299-2305`), which refuses an unscoped
UPDATE.

### Design — mirror the UPDATE fail-closed guard
Replace the empty-conditions `SELECT *` branch with an early `IpeResult::Err`,
mirroring `db_update_fields` verbatim in shape:

```rust
if keys.is_empty() {
    return IpeResult::Err(
        "db.findByConditions: refusing unscoped SELECT (no conditions); \
         pass at least one condition"
            .to_string()
            .into(),
    );
}
```

This is a runtime check, not a type-level fix, and that is correct here: the
condition dict is a genuinely-dynamic `Dict String String` whose emptiness is a
runtime property of request data — there is no compile-time type that could
carry "non-empty dict" without changing the public `findByConditions` signature
(which takes an ordinary `Dict`), and the sibling UPDATE fix already established
this runtime-guard shape as the sanctioned pattern. Consistency with the shipped
UPDATE guard outweighs a bespoke `NonEmptyDict` newtype that would touch the
kernel signature and every call site.

**Divergence:** the reference returns all rows on an empty filter; this diverges
toward the safe outcome. Record a short entry (or fold into an existing DB
divergence) noting the fail-closed refusal, matching how the UPDATE guard is
treated.

### Impl plan
1. `db.rs`: replace the `keys.is_empty()` `SELECT *` arm with the `Err` guard.
   Delete the now-dead `if keys.is_empty()` branch of the `db_format_sql`
   ternary (the `else` WHERE-building arm becomes unconditional).
   **Test:** `db_find_by_conditions` with an empty `IpeDict` → `IpeResult::Err`
   (sqlite in-memory; mirror the existing `db_update_fields` empty-WHERE test).
2. **Test:** non-empty conditions still return the filtered rows (regression that
   the guard did not break the happy path).
3. Divergence ledger: add/extend the DB section in
   `docs/divergences-from-sky.md`.

### Risk / blast radius
Any app *relying* on empty-conditions-means-all-rows breaks (returns `Err`).
That is the intended behavioural change; call it out in the divergence record.
Re-gate: runtime db tests, db examples sweep.

---

## CO-INCR-005 — wire FFI catalog into `watch` + `lsp`

### Root cause
The one-shot `run_build` path loads the FFI catalog and injects interfaces
(`src/ipe-cli/src/lib.rs:429-446`: `ffi::load_catalog_for` →
`ffi::inject_interfaces` → `ffi::assemble_emit`, then
`create_source_root(&db, &sources, &injected, &ffi_injected)` at 520 and
`BuildConfig::new(&db, driver, ffi_emit, target)` at 526). Neither incremental
driver does:

- **watch** (`src/ipe-cli/src/watch.rs:718-765`): after
  `inject_compiled_std_closure` it passes an empty `BTreeSet` for `ffi_injected`
  to `create_source_root` (`:744`) and builds `BuildConfig::new(&db_main,
  driver, None, Target::Native)` (`:757-762`) — `ffi = None`, no interface
  injection. Every `import Rust.Foo` resolves Unresolved → IPE-N0020 red-loop
  while `ipe build` succeeds.
- **lsp** (`src/ipe-cli/src/lsp.rs:37`, `DriverLoader::load`): injects only the
  std closure; every non-std module is tagged `User`, so FFI imports are
  unresolved in the editor.

The generative defect: FFI wiring is **duplicated inline in `run_build`** rather
than factored into a shared step the three drivers all call. The two incremental
drivers were written against the pre-FFI shape and never grew the step.

### Design — factor the FFI-injection step, call it from all three drivers
Extract the catalog-load + interface-injection into one reusable helper in the
`ffi` module (or `lib.rs`), returning the pieces each driver needs:

```rust
/// Load the project's installed-crate FFI catalog and inject one
/// `Rust.<Crate>` interface module per crate into `sources`. Returns the set of
/// injected module paths (for `ModuleOrigin::FfiInterface` tagging) and the
/// backend emit inputs (for `BuildConfig`). Empty catalog → empty set + `None`,
/// so a non-FFI project is unaffected.
pub fn prepare_ffi(
    sources: &mut BTreeMap<Vec<String>, (PathBuf, String)>,
    blame_path: &Path,
) -> Result<FfiPrep, CliError>;

pub struct FfiPrep {
    pub injected: BTreeSet<Vec<String>>,
    pub emit: Option<ipe_backend_rust::FfiEmit>,
}
```

- **watch** calls `prepare_ffi` right after `inject_compiled_std_closure`,
  threads `prep.injected` into `create_source_root` (replacing the empty
  `BTreeSet`), tags those module origins `FfiInterface` in the `desired` map, and
  passes `prep.emit` (not `None`) into `BuildConfig::new`. `resolved.blame_path`
  is already available (`watch.rs:776`). **Cache note:** watch does not use the
  disk build cache, so the FFI-disables-cache concern (`lib.rs:446`) does not
  apply. **Config-mutation note:** the warm `db_main` reuses one `BuildConfig`
  across generations (`watch.rs:750-765`); if the FFI catalog changes between
  rebuilds (`ipe add`/`remove` while watching), `prep.emit` must be re-set on the
  existing config via `salsa::Setter` (mirror the existing `set_db_driver` at
  `:752`). Add a `set_ffi_emit` setter on `BuildConfig` and call it when the emit
  inputs differ. Because a warm salsa DB memoises on the FFI input, changing it
  correctly invalidates downstream queries.
- **lsp** (`DriverLoader::load`): the LSP never emits (no backend run) — it only
  type-checks for diagnostics/hover. So it needs **only** the interface
  injection, not `assemble_emit`. Call `prepare_ffi` (ignoring `prep.emit`), tag
  the injected modules `ModuleOrigin::FfiInterface` (already supported by
  `LoadedFile.origin`, `loader.rs:24`), so `import Rust.Foo` resolves in the
  editor. The blame path is `entry` (`lsp.rs:28-30`).

### Impl plan
1. `ffi.rs`: add `prepare_ffi` + `FfiPrep`, factored out of the inline
   `lib.rs:429-441` sequence; rewrite `run_build`'s inline block to call it (no
   behaviour change — regression-guarded by the existing FFI build examples).
2. `ipe_db::BuildConfig`: add a `set_ffi_emit` salsa setter (mirror
   `set_db_driver`).
3. `watch.rs` FsBatch arm: call `prepare_ffi`, thread `injected` into
   `create_source_root`, tag origins, pass `emit` into `BuildConfig::new`, and
   re-set on the warm config when the catalog changed.
4. `lsp.rs` `DriverLoader::load`: call `prepare_ffi`, tag injected modules
   `FfiInterface`, merge into `files`.
5. **Tests:**
   - watch: an integration/driver test that resolves a fixture project with an
     `.ipe/cache/ffi/rust` catalog and asserts the FsBatch source set contains
     the injected `Rust.<Crate>` module with `FfiInterface` origin (mirror an
     existing watch resolution test); assert the produced `BuildConfig` carries
     `Some(emit)`.
   - lsp: a `ProjectLoader` test (the server tests already substitute a fixture
     loader) asserting `DriverLoader::load` over an FFI-cataloged project returns
     the `Rust.<Crate>` module tagged `FfiInterface`.
   - **Negative/red-loop regression:** a test that the *pre-fix* shape produced
     an unresolved-import diagnostic and the post-fix shape does not (drives the
     seam end to end).

### Risk / blast radius
Factoring `run_build`'s inline block risks a behaviour change on the one-shot
path — guard with the existing FFI examples (byte-for-byte emit unchanged). The
warm-config `set_ffi_emit` interacts with salsa memoisation; the `ipe
add`-while-watching path is the sharp edge — test it. `create_source_root`
already handles `FfiInterface` origin (`lib.rs:584-586`), so no origin-tag
plumbing is new. Re-gate: `cargo check -p ipe`, watch + lsp crate tests, the FFI
examples sweep.

---

## RT-NET-001 — production `wss://` client dials fail-closed

### Root cause
Under the SSRF production default (`ssrf_deny_private_enabled()` → true whenever
`ENV`/`IPE_ENV` is set, `ssrf.rs:36-40`), `ssrf_pinned_ws_addr`
(`ssrf.rs:216-233`) resolves EVERY host to a `SocketAddr` and returns
`Some(addr)`; `do_connect` (`ws_client.rs:259-268`) then hits the `Some(addr)`
arm, and because the client crate builds **no TLS backend**
(`tokio-tungstenite = "0.24"` with no TLS feature — runtime `Cargo.toml:33`;
emitted via `project.rs:2053-2057` `ws_dep`), a pinned dial can only produce a
plaintext `MaybeTlsStream::Plain`. The code correctly refuses to dial plaintext
to a TLS endpoint (`ws_client.rs:263-268`) — so ALL production `wss://` client
connections return `Err`. It fails **closed** (no secret leaks → completeness,
not security), but a core capability is unusable in exactly the config where TLS
is mandatory.

### Design — supply the rustls dialer (preferred); the fix IS feasible
`reqwest` (`rustls-tls`), `sqlx` (`runtime-tokio-rustls`), and `lettre`
(`tokio1-rustls-tls`) already pull `rustls` into the dependency graph, so adding
a rustls TLS backend to `tokio-tungstenite` adds no new TLS stack. The
fail-closed-under-guard branch exists **only** because no TLS feature was
enabled. Fix by enabling `tokio-tungstenite`'s rustls connector and performing a
**TLS-pinned dial**: connect the raw `TcpStream` to the vetted
`SocketAddr` (preserving the DNS-rebind-close TOCTOU guard), then hand-shake TLS
over it using the URL's original hostname for SNI/cert-verification.

Concretely:

1. **Cargo:** enable the rustls connect feature on `tokio-tungstenite` in both
   the runtime `Cargo.toml` and the emitted `ws_dep` (`project.rs:2053`). In
   0.24 that is `features = ["rustls-tls-webpki-roots"]` (or
   `"...-native-roots"`), which also brings `tokio-rustls` /
   `tokio_tungstenite::Connector`.
2. **ws_client.rs `do_connect`:** in the `Some(addr)` (pinned) arm, branch on
   scheme:
   - `ws://` → keep the current `MaybeTlsStream::Plain(tcp)` path.
   - `wss://` → build a `tokio_rustls::TlsConnector` (webpki/native roots),
     resolve `ServerName` from the URL **host** (never the IP — cert
     verification must use the name), TLS-handshake the pinned `TcpStream`, wrap
     as `MaybeTlsStream::Rustls(tls_stream)`, and pass to
     `client_async_with_config`. The single handshake timeout (`ws_client.rs:297`)
     still bounds the whole pinned+TLS handshake because the dial+TLS runs inside
     `connect_fut`.
   The un-pinned (`None`, guard-off) arm should also use
   `connect_async_tls_with_config` so dev `wss://` works too, replacing the bare
   `connect_async_with_config` (`ws_client.rs:288`).

This closes the capability gap outright — the divergence record then documents
only the *rustls-roots choice* (webpki vs native), not a missing feature.

**Divergence:** record a new `§B-WS-TLS` entry in `docs/divergences-from-sky.md`:
the client uses a rustls TLS backend (matching the reqwest/sqlx rustls posture)
rather than the reference's native-TLS; note the root store chosen and that the
pinned dial verifies the cert against the URL hostname (SNI), not the pinned IP.

**Fallback if a rustls dialer proves infeasible at impl time** (e.g. a
`tokio-tungstenite` 0.24 API mismatch): DO NOT ship the silent fail-closed. Keep
the `Err` but (a) make the error message actionable, and (b) record a `§B-WS-TLS`
divergence stating production `wss://` client dials are unsupported pending the
dialer, with the tracked follow-up. The verdict already confirms this is
`Err`-not-leak, so recording is the honest minimum. The spec's PRIMARY plan is
the dialer; the record is the floor.

### Impl plan
1. runtime `Cargo.toml`: add the rustls feature to `tokio-tungstenite`; confirm
   `websocket_client` feature (`Cargo.toml:157`) pulls it.
2. `project.rs::websocket_cargo_toml` (`:2040-2109`) + `crate_specs::TOKIO_TUNGSTENITE`:
   emit the feature in `ws_dep`. Update the `project.rs` unit tests that assert
   the emitted Cargo.toml shape.
3. `ws_client.rs::do_connect`: implement the scheme-branched pinned dial (TLS for
   `wss`, plain for `ws`) and the un-pinned TLS path. Remove the
   `wss-unsupported` `Err` (`:263-268`).
4. **Tests:**
   - runtime unit: with a self-signed local `wss://` echo server (rustls
     server config in the test), assert `ws_connect` to `wss://localhost:...`
     succeeds under `IPE_HTTP_DENY_PRIVATE=1` — the exact config that fails
     today. (Localhost is private, so this also proves the pinned-private path;
     for the public path, an ignored-by-default networked test.)
   - a test that a `wss://` to a genuinely-private host is still blocked by the
     SSRF resolver (the guard is not weakened — `resolve_first_non_private_addr`
     still errors for RFC-1918).
   - `project.rs` emit test: `ws_dep` carries the rustls feature.
5. `docs/divergences-from-sky.md`: add `§B-WS-TLS`.

### Risk / blast radius
Highest-risk item in the theme — it adds a TLS handshake to the client dial path
and touches the emitted Cargo.toml (golden-adjacent). Get the `ServerName`
derivation right (URL host, IDNA) or cert verification breaks. Ensure the
`MaybeTlsStream::Rustls` variant is available (it is when a rustls feature is on).
Re-gate: `cargo nextest -p ipe-runtime-rust --features full` (ws tests), the
`project.rs` golden/emit tests, a ws example in the sweep, and manual `wss://`
smoke against a public echo (`wss://echo.websocket.org`-class) behind the
networked test.

---

## RT-UI-002 — `Keyed` attach `ipe-key`

### Root cause
`keyed_column_`/`keyed_row_` (`src/runtime/rust/src/ui/keyed.rs:21,31`) DROP the
key with `.map(|(_, e)| e)` and forward bare children to `ui_column_`/`ui_row_`.
The consuming machinery already exists and is tested: `assign_ipe_ids_depth` →
`ipe_id_key` reads a `ipe-key` attribute (`html.rs:703,718-719`) and
`keyed_items_keep_id_across_reorder` (`html.rs:1313`) proves keyed items keep
ipe-id identity across reorder **when the attr is present**. Without it,
positional ipe-ids shift on reorder → the diff patches the wrong elements and
uncontrolled-input state / focus attaches to the wrong row. The module doc's
"semantically correct" claim is FALSE for this positional-ipe-id runtime, and it
cites `docs/divergences-from-sky.md §B-Keyed` — which does not exist (only
`§B-Lazy`): a phantom ledger citation (verified: `rg` finds `§B-Lazy` at
line 43, no `§B-Keyed`).

### Design — attach the key as `AttrAttribute("ipe-key", key)` on each child
The Ui-level attribute `AttrAttribute(k, v)` lowers to `html::Attribute::Attr(k,
v)` via `collect_html_attrs` (`ui/render.rs:361-364`), which survives the render
sink because `ipe-key` passes `SafeAttrName` (`is_safe_html_name` allows the
hyphen, not `is_dangerous_attr_name` — `html.rs:494-511`). So `ipe_id_key` will
then read it exactly like the `Ipe.Html.keyed` path. The fix: instead of
dropping the key, push `AttrAttribute("ipe-key".into(), key)` onto each child
element's attribute list before forwarding.

The child is an arbitrary `Element<M>` — only `Node`/`TaggedNode` carry an
attribute slot. `Text`/`Empty`/`Raw` cannot hold a `ipe-key`. Handle with a
small helper:

```rust
/// Attach `ipe-key` to a child so the ipe-id stamper can stabilise it across
/// reorder. Text/Empty/Raw children (no attribute slot) are wrapped in a keyed
/// `el` so the key still lands on a real HElement — mirrors how `Ui.el` renders
/// a single-child <div>, matching the Go runtime's keyed-wrapper behaviour.
fn attach_key<M: Clone>(key: String, child: Element<M>) -> Element<M> {
    match child {
        Element::Node(desc, mut attrs, kids) => {
            attrs.insert(0, Attribute::AttrAttribute("ipe-key".into(), key));
            Element::Node(desc, attrs, kids)
        }
        Element::TaggedNode(tag, desc, mut attrs, kids) => {
            attrs.insert(0, Attribute::AttrAttribute("ipe-key".into(), key));
            Element::TaggedNode(tag, desc, attrs, kids)
        }
        other => ui_el_(vec![Attribute::AttrAttribute("ipe-key".into(), key)], other),
    }
}
```

`keyed_column_`/`keyed_row_` map `attach_key` over the `(key, child)` pairs, then
forward to `ui_column_`/`ui_row_`. This is the minimal, correct fix — it reuses
the proven stamper path rather than inventing a key-aware differ (which the
runtime does not need for correctness: stable ipe-ids are the whole mechanism).

**Divergence:** the `§B-Keyed` claim in `keyed.rs:7` becomes TRUE (keys now
attach and behave), so the correct action is to **add the real `§B-Keyed`
section** to `docs/divergences-from-sky.md` describing the ipe-key-stamp approach
(vs the reference's VNode-key differ) and correct the module doc's false
"performance hint, not a behavioural contract" line to state keys ARE
behaviourally load-bearing for reorder identity.

### Impl plan
1. `ui/keyed.rs`: add `attach_key`, rewrite `keyed_column_`/`keyed_row_` to map
   it; import `ui_el_`. Fix the module doc (`keyed.rs:1-8`) to describe the real
   attach-and-stamp semantics and cite the now-real `§B-Keyed`.
2. **Tests** (in `ui/keyed.rs` or `ui/render.rs` tests): render a
   `keyed_column_` of two `li`-like nodes with keys `"alpha"`/`"beta"`, run
   `render`+`assign_ipe_ids`, assert each rendered element carries
   `ipe-key="alpha"`/`"beta"` and (mirroring `keyed_items_keep_id_across_reorder`)
   that reordering the input preserves each item's `:key` ipe-id. Add a case for
   a `Text` child → asserts it is wrapped and the wrapper carries the key.
3. `docs/divergences-from-sky.md`: add the real `§B-Keyed`.

### Risk / blast radius
Small, runtime-only. Existing tests that assert keyed helpers *drop* keys (if
any) must be updated to the new contract — search first. The extra
`AttrAttribute` prepend changes rendered HTML (a `ipe-key` attr now appears);
any golden/snapshot of keyed output updates. Re-gate: `cargo nextest -p
ipe-runtime-rust --features full` (ui tests), a keyed Live example in the sweep.

---

## CO-BACKEND-003 — routed Live `:param` fail to `notFound`, not `id 0`

### Root cause
`route_param_get` (`src/compiler/backend/rust/src/emit_live.rs:470-493`) decodes
a captured `:param` string with `.unwrap_or_default()` on the `Int`/`Float`/
`Bool` arms: a route that MATCHES structurally but whose payload fails to parse
(`/apps/abc` where `:id : Int`) silently becomes `AppDetailPage 0` instead of
routing to `notFound`. The route table matched on segment count/shape
(`route.rs:match_route`); only the payload decode failed, and it failed to a
`Default` rather than a miss. The `build` closure's type is
`Fn(Vec<String>) -> Page` (`route.rs:17`) — it cannot signal "no match", which is
WHY the emitter had no choice but `unwrap_or_default`. The stale `§B-route-param`
residual (`docs/divergences-from-sky.md:1180-1182`) explicitly names "routing to
`not_found` on bad parse is a future refinement" — this finding is that
refinement.

### Design — make the builder fallible: `Fn(Vec<String>) -> Option<Page>`
Change the route builder's carrier type so a failed payload decode is a first-
class miss the router folds into `not_found`:

- `route.rs`: `Route<Page>.build : Arc<dyn Fn(Vec<String>) -> Option<Page> + ...>`;
  `Route::new` takes `impl Fn(Vec<String>) -> Option<Page>`. `match_routes`
  becomes: on a pattern match, `match (rt.build)(params) { Some(p) => return p,
  None => continue }` — a structural match with a bad payload falls through to
  the next route, then `not_found`. `match_params`/`matches_any` are unchanged
  (they key off `match_route`, not the builder).
- `emit_live.rs::route_param_get`: emit fallible decoders that early-return `None`
  from the builder closure on parse failure, via `?`:
  | `IrType` | emitted expression |
  |---|---|
  | `Str`   | `params.get({i}).cloned()?` (missing capture → None; can't fail to parse) |
  | `Int`   | `params.get({i}).and_then(\|s\| s.parse::<i64>().ok())?` |
  | `Float` | `params.get({i}).and_then(\|s\| s.parse::<f64>().ok())?` |
  | `Bool`  | `params.get({i}).and_then(\|s\| match s.as_str() { "true" => Some(true), "false" => Some(false), _ => None })?` |
  and the builder body becomes `Some(Ctor(field0, field1, ...))`. The `Bool`
  decode also tightens: today `s == "true"` maps any non-"true" (incl. garbage)
  to `false`; the fallible form rejects a non-`true`/`false` segment as a miss —
  matching the "bad URL → notFound" intent.
- The `builder_fn_params` / inline-lambda / named-function builder shapes
  (`emit_live.rs:495-509`, the `move |_params| __c.clone()` constant-closure at
  `:197`, and the raw-`List String` builder) all wrap their result in `Some(...)`
  so every builder is uniformly `-> Option<Page>`. The constant/String-only
  builders never fail, so they are always `Some`.

This is a type-level fix (the carrier now REPRESENTS the miss) rather than a
scattered runtime check — parse-don't-validate at the route boundary: a URL
segment that cannot be the declared payload type is not a page, so the type says
so and the router handles it uniformly. It also removes a class of silent-wrong-
page bugs beyond the numeric case.

**Divergence:** update the existing `§B-route-param` record: strike the residual
"routing to `not_found` on bad parse is a future refinement" line and replace it
with the now-shipped behaviour (bad numeric/bool capture → `not_found`, no longer
`0`/`0.0`/`false`). This is a divergence FROM the Go reference (which coerces at
runtime), sanctioned under the safe-outcome/parse-don't-validate rationale
already recorded there.

### Impl plan
1. `runtime/src/live/route.rs`: change `build` to `-> Option<Page>`, update
   `Route::new` and `match_routes`; update the in-file tests
   (`matches_static_and_param_in_order`) to the `Option` builder shape and add a
   `bad-payload → not_found` case.
2. `emit_live.rs`: rewrite `route_param_get` to emit fallible `?`-decoders; wrap
   every builder body in `Some(...)` across the ctor / lambda / named-fn /
   constant / raw-`List String` arms (`:197`, `:222-226`, `builder_fn_params`
   path). Update the doc table (`:458-466`).
3. **Negative test** in `src/ipe-cli/tests/negative_suite.rs` is NOT the right
   home (this is a runtime-behaviour change, not a compile rejection). Instead:
   - runtime unit test in `route.rs`: a `/apps/:id` (Int) route + `not_found`;
     `match_routes(.., "/apps/abc")` → `not_found`; `"/apps/42"` →
     `AppDetailPage(42)`.
   - a SEAL/emit example under `examples/` (routed Live app with an `Int`
     `:param`) that exercises `/apps/abc` → `notFound` end to end, closing the
     "boot-only sweep masked a real interaction bug" gap the MEMORY note warns
     about.
4. `docs/divergences-from-sky.md`: update `§B-route-param`.

### Risk / blast radius
`route.rs`'s builder-type change is a SEAL-sensitive codegen change — every
emitted routed Live app's `Route::new` closure must now return `Option<Page>`;
the emitter and runtime must land in ONE commit (exit-0-then-cargo-fail hazard if
they drift). Existing routed Live examples/goldens change (the emitted closure
body gains `Some(...)` and `?`) — regenerate goldens. Re-gate: `cargo nextest
run --workspace` (route tests), the routed Live examples in the sweep, and the
new bad-param SEAL example. Confirm the single-page `live_app` (no routes) path
is untouched (it does not build `Route`s).

---

## Cross-cutting notes

- **Two ledger fixes are load-bearing, not cosmetic:** RT-UI-002's phantom
  `§B-Keyed` and CO-BACKEND-003's stale `§B-route-param` residual both violate
  the "deliberate divergence is documented, never silent" rule (PRINCIPLES §2).
  Each fix's step list includes the ledger edit.
- **Test-home summary:** runtime findings (RT-DATA-001/003, RT-NET-001,
  RT-UI-002) → `#[cfg(test)]` modules in their own `.rs` under
  `src/runtime/rust/src/` + a sweep example; CO-INCR-005 → `ipe`/`lsp-server`
  crate tests + FFI example; CO-BACKEND-003 → `route.rs` unit tests + a routed
  Live SEAL example (NOT `negative_suite.rs` — no compile rejection involved).
  The audit's "the suite missed them" gap is closed by the interaction-level
  examples (bad-param routing, keyed reorder, bytes round-trip), which boot-only
  sweeps do not cover.
- **Ordering / dependencies:** the six are independent except that RT-DATA-001's
  `Db.Decode.bytes` and RT-DATA-003's guard both touch `db.rs` (same file, no
  logical dependency — sequence to avoid a merge conflict). RT-NET-001 and
  CO-BACKEND-003 are the two SEAL/emit-adjacent items; land each's runtime +
  emit changes atomically.

---

## Proposed backlog entries

```json
{"id": "TBD", "priority": "Medium", "phase": "principles-audit-fix", "task": "RT-DATA-001: widen the Postgres/sqlite row-decode probe chain in db.rs (row_to_json/row_to_map) to cover BOOL/BYTEA(hex)/NUMERIC/TIMESTAMP with a correct probe ordering (bool>i64>f64>string>bytes) so no non-NULL column collapses to Null/\"\"; make row_to_json fallible (Err on an unreadable driver type instead of a phantom Null); add the missing Db.Decode.bytes reader kernel + backend routing to close the SqlBytes write-without-read asymmetry.", "notes": "Runtime unit tests are sqlite-only (DbRow=SqliteRow); add an ignored-by-default pg-integration test gated on IPE_TEST_PG_URL to pin the driver-specific bite. Regression: sqlite BLOB round-trip + bool column decode. Add/extend a db example that writes+reads a bytes column.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
{"id": "TBD", "priority": "Low", "phase": "principles-audit-fix", "task": "RT-DATA-003: make db_find_by_conditions fail closed on empty conditions (db.rs:1475) — return IpeResult::Err refusing an unscoped SELECT, mirroring the shipped db_update_fields empty-WHERE guard (db.rs:2299), closing the cross-tenant fail-open read. Record the fail-closed divergence in docs/divergences-from-sky.md.", "notes": "Regression: empty IpeDict -> Err; non-empty conditions still return filtered rows. Mirror the existing db_update_fields empty-WHERE test.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
{"id": "TBD", "priority": "Medium", "phase": "principles-audit-fix", "task": "CO-INCR-005: wire the FFI catalog into ipe watch + ipe lsp. Factor run_build's inline catalog-load+inject (lib.rs:429-446) into a shared ffi::prepare_ffi helper; call it from watch.rs's FsBatch arm (thread injected set into create_source_root, tag FfiInterface origins, pass ffi_emit into BuildConfig, add a BuildConfig::set_ffi_emit salsa setter for warm-config catalog changes) and from lsp.rs DriverLoader::load (interface injection only — LSP never emits). Closes the FFI-project red-loop under watch/lsp while ipe build succeeds.", "notes": "watch uses no disk cache so the FFI-disables-cache concern is moot; the sharp edge is ipe add/remove while watching (re-set ffi_emit on the warm config). Tests: watch resolution asserts injected Rust.<Crate> module w/ FfiInterface origin + Some(emit); lsp loader test asserts FfiInterface tagging; a red-loop regression (unresolved-import pre-fix, clean post-fix).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
{"id": "TBD", "priority": "Medium", "phase": "principles-audit-fix", "task": "RT-NET-001: supply a rustls TLS dialer for the WebSocket client so production wss:// dials work instead of failing closed. Enable tokio-tungstenite's rustls feature (runtime Cargo.toml + emitted ws_dep in project.rs::websocket_cargo_toml); in ws_client.rs do_connect, branch the pinned-addr arm on scheme — TLS-handshake the vetted-addr TcpStream with the URL host as SNI for wss, plain for ws — and use connect_async_tls for the un-pinned path. Remove the wss-unsupported Err. Record a new §B-WS-TLS divergence (rustls backend + hostname-verified pinned dial). Fallback if the dialer proves infeasible: keep Err but record the production-wss-unsupported limitation honestly.", "notes": "reqwest/sqlx/lettre already pull rustls, so no new TLS stack. Highest-risk item (adds TLS to the dial path + touches emitted Cargo.toml goldens). Tests: local self-signed wss echo succeeds under IPE_HTTP_DENY_PRIVATE=1; genuinely-private wss still blocked; project.rs emit test asserts the rustls feature; networked public-wss smoke ignored-by-default.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
{"id": "TBD", "priority": "Medium", "phase": "principles-audit-fix", "task": "RT-UI-002: make Ipe.Ui.Keyed attach the key. In ui/keyed.rs, instead of dropping the key (.map(|(_,e)|e)), prepend AttrAttribute(\"ipe-key\", key) to each child Element (Node/TaggedNode carry attrs; Text/Empty/Raw children are wrapped in a keyed ui_el_). The ipe-key attr survives SafeAttrName and is consumed by ipe_id_key/assign_ipe_ids_depth exactly like Html.keyed, stabilising ipe-id identity across reorder. Add the real §B-Keyed section to docs/divergences-from-sky.md (the module doc currently cites a phantom one) and correct the false 'performance hint, not a behavioural contract' doc line.", "notes": "Reuses the proven stamper path (keyed_items_keep_id_across_reorder proves it works when the attr is present) — no key-aware differ needed. Tests: render keyed_column_ asserts each child carries ipe-key + reorder preserves :key ipe-id; a Text-child wrap case. Update any golden of keyed output (ipe-key attr now appears).", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
{"id": "TBD", "priority": "Medium", "phase": "principles-audit-fix", "task": "CO-BACKEND-003: route a bad Live :param to notFound instead of the type's Default. Change Route<Page>.build to Fn(Vec<String>) -> Option<Page> (route.rs) so match_routes folds a failed payload decode into not_found; rewrite emit_live.rs::route_param_get to emit fallible ?-decoders (Int/Float/Bool early-return None on parse failure; Bool rejects non-true/false) and wrap every builder body (ctor/lambda/named-fn/constant/raw-List-String) in Some(...). Land the runtime + emit change atomically (SEAL). Update the §B-route-param divergence: bad numeric/bool capture now -> notFound (was 0/0.0/false).", "notes": "Type-level fix — the carrier now represents the miss (parse-don't-validate at the route boundary). Tests: route.rs unit /apps/abc(Int)->not_found, /apps/42->AppDetailPage(42); a routed-Live SEAL example exercising /apps/abc->notFound end to end (closes the boot-only-sweep interaction gap). Regenerate routed Live goldens (closure body gains Some(...) + ?). NOT a negative_suite.rs case.", "spec": "docs/audit/2026-07-17-principles-audit/specs/t5-data-completeness.md", "blocked_by": [], "status": "pending", "deferred": false}
```
