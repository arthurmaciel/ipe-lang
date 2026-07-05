# Sky.Http.Server.WebSocket — server-side WebSocket design (unblocks 33-websocket-echo)

> Design Lane output, 2026-07-05. Supersedes §2.5 of
> `effect-modules-kernel-plan.md`, whose status row is **stale**: the runtime
> is NOT missing. Read §1 before estimating anything.

---

## 0. Verdict up front

- **The runtime is already shipped** in `runtime/src/sky_runtime/server.rs:711-1057`
  (WS section: `WsHandle`, `WsServerCfg<E>` with `Arc<dyn Fn>` callbacks,
  `ws_loop`, registry, origin glob gate with CSWSH hardening, task-local
  upgrade smuggling, send/sendBinary/broadcast/close). No new `ws_server.rs`
  module is needed. What's missing is ~100 lines of **additive runtime
  adapters** (cfg constructor + builders + handle-taking wrappers) and the
  **entire compiler wiring** (canon → kernels → constrain → lower → emit),
  which is zero today (`rg -i websocket crates/` matches only a parser-test
  comment, `sky_parse/src/lib.rs:1843`).
- **Kernel count is 12, not 5.** The plan's "5 kernels" assumed the upstream
  split where `defaultCfg`/`with*` are compiled Sky stdlib source
  (`sky-stdlib/Sky/Http/Server/WebSocket.sky`). skyc has **no embedded
  `Sky.Http.Server[.X]` stdlib modules** — the whole Server surface is
  kernel-registered (`crates/skyc/stdlib/` has no `Sky/Http/` tree; see
  `d("Server", …)` arms, `sky_kernels/src/lib.rs:1466-1488`). We follow that
  precedent: builders become kernels too. Effort drops from XL to **L**
  (runtime exists; wiring is mechanical and compiler-exhaustiveness-guided).
- **No sentinel needed for the upgrade response.** The prompt asked whether to
  prefer a typed variant over Stream's magic-prefix sentinel: the vendored
  runtime already implements the *typed* mechanism — a `tokio::task_local!`
  pair (`WS_UPGRADER` / `WS_RESPONSE`, server.rs:782-787). `method_router`
  (server.rs:609-631) scopes both around every handler call and **prefers
  `WS_RESPONSE`** over the handler's returned `ServerResponse`. The 101
  `ServerResponse` the kernel returns is a placebo that is discarded. Nothing
  to design; it is wired and works for any route registered via
  `Server.get`/`post`/`any`.

---

## 1. Inventory — what exists today (verified at HEAD)

### 1.1 Runtime (`runtime/src/sky_runtime/server.rs`) — COMPLETE core

| Piece | Lines | Notes |
|---|---|---|
| `WsHandle::WebSocketServer(i64)` | 726-728 | opaque per-peer handle enum, variant name matches the Sky ctor |
| `WsServerCfg<E>` | 740-755 | `onConnect/onMessage/onClose/onError: Arc<dyn Fn(..) -> SkyTask<E, ()> + Send + Sync>` + `maxMessageBytes: i64` + `originPatterns: Vec<String>`. Generic over `E: From<String>` |
| outbound queue | 757-773 | `mpsc::channel<WsOut>(256)`, `SKY_WS_SEND_BUFFER` override; `try_send` drop-on-full = bounded memory |
| registry | 775-780 | `OnceLock<Mutex<HashMap<i64, Sender<WsOut>>>>` + `AtomicI64` id counter |
| task-locals | 782-787 | `WS_UPGRADER` (axum upgrader stashed by `build_request`, 508-513) + `WS_RESPONSE` (101 response, preferred by `method_router` 617-619) |
| `ws_loop` | 789-857 | register → `onConnect` → `select!{recv, rx}` → `onClose` → deregister. 1 MiB default max-message enforced in-loop (axum 0.7 lacks builder caps); binary frames funnel to `onMessage` via lossy UTF-8; Ping/Pong auto-handled |
| `ws_production()` | 859-864 | `ENV` → `SKY_ENV` fallback, same gate as the rest of the runtime |
| `ws_origin_matches` | 875-935 | glob with `*`; **CSWSH-hardened**: a `*`-covered span before a literal anchor must be host-safe chars only (`evil.com/.example.com`, `evil.com@x.example.com` rejected). `"*"` = explicit allow-all |
| `server_web_socket_upgrade(req, cfg)` | 937-983 | production + empty patterns → 403 fail-closed; non-empty patterns enforced in every mode; missing upgrader → 400 |
| `server_web_socket_send_to_client(i64, String)` | 998-1010 | Err when peer unknown or queue full |
| `server_web_socket_send_binary_to_client(i64, Vec<u8>)` | 1012-1024 | |
| `server_web_socket_broadcast(Vec<i64>, String)` | 1026-1049 | best-effort; Err only when every send failed and list non-empty |
| `server_web_socket_close_client(i64)` | 1051-1057 | idempotent |

`axum = { version = "0.7", features = ["ws"] }` already in
`runtime/Cargo.toml:21` under the `server` feature (line 100). The emitted
project's Cargo surgery already emits `axum … features ["ws"]`
(`sky_backend_rust/src/project.rs`, the `server_cargo_toml` dep lines), and
`project.rs:77-78` already appends `pub mod server; pub use server::*;` — so
`WsHandle`/`WsServerCfg` are re-exported into emitted crates **today**.

Provenance: this WS section arrived via runtime sync from upstream
`../sky/runtime-rust` (design spec:
`../sky/runtime-rust/superpowers/specs/2026-06-02-sub-D2-websocket-server-design.md`,
shipped there against the *Haskell* compiler's Rust backend). Our copy has
since diverged with local hardening (canonical headers, `SKY_TRUSTED_PROXY`,
accessor-kernel doc); treat `server.rs` as fork-maintained, not byte-vendored.

### 1.2 Client side (`runtime/src/sky_runtime/ws_client.rs`, 706 lines)

Exists (tokio-tungstenite, SSRF-validated + DNS-pinned connect at 161/228,
close-code ADT at 43, 1 MiB `max_message_size` at 219) but `Sky.Core.WebSocket`
has **zero compiler wiring** too — out of scope here; its Sub-tier
(`onMessage`/`onClose` as subscriptions) is a separate effort.

### 1.3 Example (`examples/33-websocket-echo/src/Main.sky`, byte-identical to upstream)

Uses exactly: `Ws.upgrade req cfg`, `Ws.defaultCfg`, `Ws.withOnConnect`,
`Ws.withOnMessage`, `Ws.withOnClose`, `Ws.withOnError`,
`Ws.withOriginPatterns ["*"]`, `Ws.sendToClient sock ("echo: " ++ msg)`,
plus **qualified type annotations** `Ws.WebSocketServer` on the four
callbacks and `Server.get "/ws" handleWs` on port 8033. It does NOT use
`sendBinaryToClient` / `broadcast` / `closeClient` / `withMaxMessageBytes`
(we wire them anyway — full surface, no-deferral).

### 1.4 Sub-tier for 33? **Not needed.**

All of example 33's callbacks are direct server-side `Task` callbacks invoked
by `ws_loop`; nothing routes through TEA `Sub`. Deliverable (3) is a no-op for
33-green. (Client-side `Sky.Core.WebSocket.onMessage : … -> Sub msg` is the
Sub-tier surface; different module, different effort.)

---

## 2. Design decisions

### D1 — Kernel-only module (no stdlib `.sky` port)

Mirror `Server`/`Stream`/`HttpStream`: register qualifier `Ws` + 12 kernels.
Rationale: skyc embeds no `Sky.Http.*` Sky source; porting the upstream
stdlib file would additionally require (a) `Ffi.kernel` string→KernelFn
resolution for `ServerWebSocket_*` names, (b) codegen for a user-visible
record with function-typed fields bridged onto the runtime `WsServerCfg<E>`
struct — the exact `runtimeOpaqueTypes` machinery skyc deliberately replaced
with dedicated `IrType` opaque variants (`ServerRequest`, `StreamWriter`, …).
Kernel-only is the shorter, precedented, fail-closed path.

### D2 — Two new opaque IR types, both monomorphic

- `IrType::WebSocketServer` → renders `WsHandle` (Copy, i64 inside).
- `IrType::WebSocketServerCfg` → renders `WsServerCfg<SkyError>` (emitted
  code always pins `SkyError = String`; `E: From<String>` is satisfied).

**Phantom `msg` is dropped.** Upstream's `WebSocketServerCfg msg` carries a
phantom var for hypothetical future Sub integration; it never reaches the
runtime. skyc types the cfg as a nullary constructor. Soundness: no type var
exists to mis-unify (this *dissolves* plan seal-note 4 rather than answering
it). User impact: an annotation `Ws.WebSocketServerCfg Msg` fails arity while
upstream accepts it — record in `docs/divergences-from-sky.md` (example 33
never names the cfg type; the risk is annotation-only).

### D3 — Kernels take `WsHandle`, not `Int`

Upstream's Sky source unwraps `WebSocketServer raw -> raw` before calling the
i64 kernels. With no Sky layer, our kernels accept the handle directly and
thin runtime adapters unwrap. The existing i64 functions stay untouched
(they are the registry API and upstream-sync surface).

### D4 — Send semantics: keep bounded non-blocking `try_send`; ledger the divergence

Go blocks up to ~30 s on a full write buffer; our vendored send path drops
and returns `Err` when the 256-frame queue is full. Bounded fail-fast is the
sounder default (no handler-task pileup behind one slow peer) and is already
shipped behavior for the i64 family. Adapters delegate 1:1. Add a
`docs/divergences-from-sky.md` entry: *"WS server send is bounded
fail-fast (SKY_WS_SEND_BUFFER=256) instead of Go's 30 s blocking send"*. If
the guardian overrules, the change is 3 lines inside the adapters
(`tx.send_timeout(out, Duration::from_secs(30))`) — decision point flagged
in §7.

### D5 — Heartbeat gap: filed as follow-on hardening, not 33-blocking

Go pings every 30 s with a 10 s timeout
(`../sky/runtime-go/rt/server_websocket.go:392-401`,
`wsDefaultPingInterval = 30s` in `websocket.go:92`). Neither our `ws_loop`
nor upstream's runtime-rust `ws_loop` sends pings — dead peers linger in the
registry until TCP gives up. Both Rust runtimes share this oracle divergence.
Fix (H1, ~15 lines, do in the same PR if time allows, else file a task —
no-deferral): add a third `select!` arm
`_ = ping_interval.tick() => { if socket.send(Message::Ping(vec![])).await.is_err() { break; } }`
with `let mut ping_interval = tokio::time::interval(Duration::from_secs(30));`
(first tick fires immediately — call `.tick().await` once before the loop or
use `interval_at`). Pong frames already fall into the ignore arm; a dead peer
surfaces as a send error → loop break → `onClose` + deregister. That is
Go-equivalent liveness without pong bookkeeping.

---

## 3. Runtime additions (server.rs, insert after line 1057, inside the WS section)

~100 lines, all additive, generic over `E: From<String> + Send + 'static`
(matching the section's convention). No existing line changes.

```rust
/// ServerWebSocket defaultCfg — no-op callbacks, 0 => 1 MiB default cap,
/// empty origin allowlist (dev: allow-all; production: upgrade returns 403).
pub fn ws_server_default_cfg<E: From<String> + Send + 'static>() -> WsServerCfg<E> {
    WsServerCfg {
        onConnect: Arc::new(|_| Box::pin(async { ok_res(()) })),
        onMessage: Arc::new(|_, _| Box::pin(async { ok_res(()) })),
        onClose: Arc::new(|_| Box::pin(async { ok_res(()) })),
        onError: Arc::new(|_, _| Box::pin(async { ok_res(()) })),
        maxMessageBytes: 0,
        originPatterns: Vec::new(),
    }
}

pub fn ws_server_with_on_connect<E, F>(cb: F, mut cfg: WsServerCfg<E>) -> WsServerCfg<E>
where E: From<String> + Send + 'static,
      F: Fn(WsHandle) -> SkyTask<E, ()> + Send + Sync + 'static {
    cfg.onConnect = Arc::new(cb);
    cfg
}
// ws_server_with_on_message  — F: Fn(WsHandle, String) -> SkyTask<E, ()>  (UNCURRIED,
//                              matching dict_foldl's Fn(K, V, A) precedent, dict.rs:128-131)
// ws_server_with_on_close    — F: Fn(WsHandle) -> SkyTask<E, ()>
// ws_server_with_on_error    — F: Fn(WsHandle, E) -> SkyTask<E, ()>
// ws_server_with_max_message_bytes(n: i64, cfg) — cfg.maxMessageBytes = n
// ws_server_with_origin_patterns(ps: Vec<String>, cfg) — cfg.originPatterns = ps

/// Handle-taking adapters over the i64 registry family (D3).
pub fn ws_server_send_to_client<E: From<String> + Send + 'static>(
    h: WsHandle, msg: String,
) -> SkyTask<E, ()> {
    let WsHandle::WebSocketServer(id) = h;
    server_web_socket_send_to_client(id, msg)
}
// ws_server_send_binary_to_client(h, data: Vec<u8>)   -> delegates ..._send_binary_to_client
// ws_server_broadcast(hs: Vec<WsHandle>, msg: String) -> unwrap ids, delegate ..._broadcast
// ws_server_close_client(h)                            -> delegates ..._close_client
```

Notes:
- `Ws.upgrade` needs **no adapter** — `server_web_socket_upgrade(req, cfg)`
  (937) already has the kernel shape `Request -> Cfg -> Task Error Response`.
- `sendBinaryToClient`: skyc's `Bytes` is a distinct `Vec<u8>` primitive
  (divergence-policy'd), so the scheme uses `Bytes` and the adapter takes
  `Vec<u8>` — no lossy String hop. (Upstream Sky sig says `Bytes = String`;
  same ledger entry family.)
- Unit-test the pure parts in-file (`#[cfg(test)]`): default-cfg field
  values; adapter unwrap; `ws_server_broadcast` empty-list Ok.

---

## 4. Compiler wiring — 12 kernels across 6 crates

Naming: `KernelFn::Ws*` (qual + name, per `StreamStream` convention).

| Sky (qual `Ws`) | KernelFn | Arity | Scheme (constrain helpers) | Runtime fn (emit name) |
|---|---|---|---|---|
| `defaultCfg` | `WsDefaultCfg` | 0 | `wscfg()` | `ws_server_default_cfg` |
| `withOnConnect` | `WsWithOnConnect` | 2 | `fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg()))` | `ws_server_with_on_connect` |
| `withOnMessage` | `WsWithOnMessage` | 2 | `fun(fun(wsh(), fun(string(), task_unit())), fun(wscfg(), wscfg()))` | `ws_server_with_on_message` |
| `withOnClose` | `WsWithOnClose` | 2 | `fun(fun(wsh(), task_unit()), fun(wscfg(), wscfg()))` | `ws_server_with_on_close` |
| `withOnError` | `WsWithOnError` | 2 | `fun(fun(wsh(), fun(error_ty(), task_unit())), fun(wscfg(), wscfg()))` | `ws_server_with_on_error` |
| `withMaxMessageBytes` | `WsWithMaxMessageBytes` | 2 | `fun(int(), fun(wscfg(), wscfg()))` | `ws_server_with_max_message_bytes` |
| `withOriginPatterns` | `WsWithOriginPatterns` | 2 | `fun(list(string()), fun(wscfg(), wscfg()))` | `ws_server_with_origin_patterns` |
| `upgrade` | `WsUpgrade` | 2 | `fun(req(), fun(wscfg(), task(resp())))` | `server_web_socket_upgrade` |
| `sendToClient` | `WsSendToClient` | 2 | `fun(wsh(), fun(string(), task_unit()))` | `ws_server_send_to_client` |
| `sendBinaryToClient` | `WsSendBinaryToClient` | 2 | `fun(wsh(), fun(bytes(), task_unit()))` | `ws_server_send_binary_to_client` |
| `broadcast` | `WsBroadcast` | 2 | `fun(list(wsh()), fun(string(), task_unit()))` | `ws_server_broadcast` |
| `closeClient` | `WsCloseClient` | 1 | `fun(wsh(), task_unit())` | `ws_server_close_client` |

Schemes are written curried (HM); the IR `Fun` flattening that already makes
`dict_foldl`'s 3-arg callback uncurried handles the 2-arg `onMessage`/`onError`
callbacks identically.

Type-annotation resolution (`Ws.WebSocketServer` in example 33's sigs) is
free once the qualifier registers: `canonicalise_type`'s `TType` arm
(`sky_canon/src/resolve.rs:2454-2478`) only validates the qualifier against
`env.qual_vars` and then canonicalises the bare name as `Type::Con { home: [], name }`
(resolve.rs:2510-2518) — exactly how `Stream.StreamWriter` in example 30
reaches the checker; the interned bare name then matches the new builtins
symbol. Do **not** add the names to `RESERVED_BUILTIN_TYPES`
(resolve.rs:64-102) — `StreamWriter` isn't there either; the #100/#101
home-aware guard keeps a user `type WebSocketServer` winning by identity, and
the string arms added in `ir_type_from_ty`/`ir_type_from_canon` sit below the
`enum_variants` guard, mirroring `"StreamWriter"`.

---

## 5. Sonnet-executable steps (file:line, in dependency order)

Line numbers verified at HEAD (2026-07-05); re-anchor with the quoted
neighbours if the file has drifted. After step 2, `cargo check` drives the
rest: every `match` over `KernelFn`/`IrType` is exhaustive and fail-closed —
compile errors are the todo list.

1. **Runtime adapters** — `runtime/src/sky_runtime/server.rs`, insert after
   `server_web_socket_close_client` (line 1057), before the
   `── Sky.Http.Middleware` banner (1059): the 11 functions of §3 + in-file
   tests. `cargo test -p sky_runtime --features server`.

2. **IR type variants** — `crates/sky_ir/src/ir.rs`: add
   `WebSocketServer` + `WebSocketServerCfg` variants next to `StreamWriter`
   (ir.rs:587-594), doc-commented with their runtime renders (`WsHandle`,
   `WsServerCfg<SkyError>`). `crates/sky_ir/src/pretty.rs:119`: add both to
   `ir_type_name` (`"WebSocketServer"` / `"WebSocketServerCfg"`).

3. **Kernel registry** — `crates/sky_kernels/src/lib.rs`:
   - enum: 12 variants after `HttpStreamClose` (line 874), doc-commented.
   - `decl()`: 12 `d("Ws", …, KernelClass::Server, "<emit name>")` arms after
     `HttpStreamClose`'s (line 1807), arities per §4 table.
   - `ALL` (line 1871 block; Stream entries at 2552-2558): append the 12.
   - `is_server()` (fn at 2670; Stream entries at 2701-2709): append the 12
     (this is what flips `uses_server` → axum dep + `server` feature in the
     emitted project, `sky_backend_rust/src/lib.rs:564`).
   - Run `cargo test -p sky_kernels` — the decl/qual drift tripwires will
     point at anything missed.

4. **Canon qualifier** — `crates/sky_canon/src/env.rs`:
   - `STDLIB_MODULE_QUALIFIERS` table: `(&["Sky", "Http", "Server", "WebSocket"], "Ws")`
     next to the Stream entry (env.rs:105).
   - qual-functions table (env.rs:1277-1280): `("Ws", &["upgrade",
     "defaultCfg", "withOnConnect", "withOnMessage", "withOnClose",
     "withOnError", "withMaxMessageBytes", "withOriginPatterns",
     "sendToClient", "sendBinaryToClient", "broadcast", "closeClient"])`.
   - dotted-name list (env.rs:1333-1334): `("Sky.Http.Server.WebSocket", "Ws")`.
   - `cargo test -p sky_canon` — `lib.rs` drift tests enforce three-way sync
     with the kernel registry.

5. **Constrain** — `crates/sky_types/src/constrain.rs`:
   - builtins struct + interner: `ws_server: Symbol` (`"WebSocketServer"`),
     `ws_server_cfg: Symbol` (`"WebSocketServerCfg"`) next to `stream_writer`
     (fields ~152, interns ~330).
   - helper closures next to `sw()` (2534-2540): `wsh()` / `wscfg()` as
     nullary `Ty::Con`.
   - schemes in `kernel_scheme_or_unsupported` (fn at 1935; Stream arms at
     4106-4116): 12 arms per §4, each with a `-- Sky sig` comment line like
     the Stream block.

6. **Lower** — `crates/sky_lower/src/lower.rs`:
   - `callee_arity` (fn at 4234): `WsDefaultCfg` into the arity-0 group
     (MathPi/DictEmpty block, 4240-4246); `WsCloseClient` into the Ok(1)
     group (5254-5258); the other 10 into the Ok(2) group (5259-5266).
   - `lower_callee` qual match: 12 arms after `("HttpStream", "close")`
     (6149-6152), pattern `("Ws", "upgrade") => Ok(Callee::Kernel(KernelFn::WsUpgrade))` etc.
   - `ir_type_from_ty` / `ir_type_from_canon` string arms:
     `"WebSocketServer" => Ok(IrType::WebSocketServer)` +
     `"WebSocketServerCfg" => Ok(IrType::WebSocketServerCfg)` at BOTH 2508
     and 2938, in the same below-`enum_variants`-guard position as
     `"StreamWriter"`.
   - The value-reference arm that lets bare `Dict.empty` lower as a value
     covers `Ws.defaultCfg` once `callee_arity` says 0 — no extra arm; the
     example's `|>` chains desugar to full application at parse (no partial
     kernel application arises in 33).

7. **Backend** — `crates/sky_backend_rust/src/`:
   - `naming.rs`: 12 `KernelFn::Ws* => "<emit name>"` arms after the Stream
     block (998-1001), §4 last column.
   - `emit_expr.rs`: add all 12 to the standard N-arg list in
     `emit_server_call` (1030-1053) — none needs boxing/projection (the
     callback args emit as closures/fn items exactly like `StreamStream`'s).
   - `emit_types.rs`: `IrType::WebSocketServer => "WsHandle"`,
     `IrType::WebSocketServerCfg => "WsServerCfg<SkyError>"` next to
     `StreamWriter` (145).
   - `lib.rs` exhaustive IrType matches (962, 1075, 1126, 1204, 1472):
     both variants join the opaque-handle arms (no record shape, pointer-ish
     size leaf, monomorphic, no generics). NOTE `WsServerCfg` is *not* Copy
     and holds Arcs — it still "carries no record shape" for these
     classifiers; if any site distinguishes Copy-ness, follow what
     `IrType::Db` (Arc-backed) does, not `StreamWriter`.
   - `emit_model_gate.rs` (200-201): both variants are NOT valid Model
     leaves (a live handle must never be session-persisted).
   - `emit_live.rs` (611): add to the IrType name table.
   - `cargo check` until exhaustiveness is clean; any site not listed here
     that the compiler flags: classify by the `StreamWriter`-vs-`Db` rule
     above and document the choice in the arm comment.

8. **Docs/ledger** — `docs/divergences-from-sky.md`: (a) phantom `msg`
   dropped (D2); (b) `sendBinaryToClient` takes real `Bytes = Vec<u8>` (§3);
   (c) bounded fail-fast send vs Go 30 s block (D4); (d) heartbeat gap until
   H1 lands (D5). Update `docs/architecture/effect-modules-kernel-plan.md`
   §1 row + §2.5 with a pointer to this doc. Refresh
   `docs/architecture/parity-matrix.tsv` if it carries a ServerWebSocket row.

9. **H1 heartbeat** (same PR if green, else file the task immediately):
   §D5 recipe in `ws_loop` (runtime server.rs:812, the `select!`).

## 6. Red → green for 33-websocket-echo

**Red (today).** `skyc build examples/33-websocket-echo/src/Main.sky` fails in
canon: `Ws` is not a known qualifier (`UnknownModule`, resolve.rs:2468-2477
for the annotations; the import/qual table for the calls).

**Green gates, in order:**

1. `cargo test -p sky_kernels -p sky_canon -p sky_types -p sky_lower -p sky_backend_rust`
   — registry/qual drift tripwires + goldens all pass.
2. `skyc build examples/33-websocket-echo/src/Main.sky` → skyc-0, then
   `cargo build` of the emitted project → cargo-0. (Sweep does both:
   `SKYC_BIN=target/release/skyc ./scripts/examples-sweep.sh` — 33 is a
   server shape, RUN probes the bound port.)
3. **Echo round-trip e2e** — new `crates/skyc/tests/ws_server_e2e.rs`,
   cloned from `server_e2e.rs`'s harness (SKY_E2E=1 gate, temp-dir compile,
   `oracle::build_rust_binary`, ephemeral port via bind-then-drop,
   `SKY_SERVER_PORT` + `SKY_HTTP_BIND=127.0.0.1`, stderr-readiness poll,
   `ProcessGuard`). Inline Sky program = example 33's shape with
   `Server.listen port` reading `SKY_SERVER_PORT`. Client: **raw
   `TcpStream`, no new dev-deps** (server_e2e doctrine):
   - Handshake: `GET /ws HTTP/1.1` + `Host` + `Upgrade: websocket` +
     `Connection: Upgrade` + `Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==` +
     `Sec-WebSocket-Version: 13`, no Origin header (cfg `["*"]` matches the
     empty origin — `ws_origin_matches` returns true for the bare-`*`
     pattern). Assert status line contains `101`.
   - Send one masked text frame `hello` with an all-zero mask key (valid per
     RFC 6455 §5.3, leaves the payload bytes unchanged):
     `[0x81, 0x85, 0x00, 0x00, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o']`.
   - Read the reply frame; assert it starts `[0x81, 0x0B]` and the next 11
     bytes are `echo: hello`. Read with a 10 s socket timeout — never
     unbounded (timeout-gate rule).
   - Negative case in the same file: `ENV=production` + a program whose cfg
     omits `withOriginPatterns` → handshake response is `403`.
   Run: `SKY_E2E=1 cargo test -p skyc --test ws_server_e2e`.
4. Full sweep green including 33 (`BUILD ok · RUN ok`; EQUIV per current
   phase default).

## 7. Seal-touching + adversarial shapes

**Seal flag: LOW but non-zero.** No existing golden output changes (purely
additive kernels; no shared-path emission edits). Touched sealed-adjacent
surfaces: the canon qualifier drift tests (`sky_canon/src/lib.rs`), the
kernel `ALL`/decl tripwires, and `emit_model_gate.rs` (gate list growth —
strictly more rejecting, never less). Anything requiring a change to an
existing `golden_*_seal.rs` expectation = STOP, escalate to guardian review
(it would mean the additive claim is false).

**Guardian (Opus) review points:**
1. **Origin gate** — verify prod-empty→403 ordering stays BEFORE the
   upgrader take (server.rs:945-965: it does — cfg is checked before
   `WS_UPGRADER.try_with`); verify the new `ws_server_with_origin_patterns`
   can't be bypassed by calling `upgrade` with a cfg built in another task
   (it can't — the gate reads the cfg argument, not ambient state).
2. **D4 decision** — bounded fail-fast vs Go 30 s blocking send (3-line
   change if overruled).
3. **`with*` builder cloning** — `WsServerCfg` is Clone (Arc fields);
   builders take cfg by value and return it; no double-registration hazard
   because ids are minted only inside `upgrade`.

**Adversarial shapes to test or reject cleanly:**
- `Ws.upgrade` called from a NON-handler context (e.g. inside
  `Cmd.perform`): no task-local in scope → `WS_UPGRADER.try_with` errs →
  400 "expected an Upgrade request". Safe; add a spec if cheap.
- Reusing a stale `WebSocketServer` handle after close: registry miss →
  `Err("ws: no client N")` — already the runtime contract; the e2e can
  assert it via `closeClient` then `sendToClient`.
- `sendToClient` from *inside* `onMessage` (the echo shape itself): goes
  through the mpsc queue, never the socket directly — no re-entrancy on the
  socket. This is exactly example 33; the e2e covers it.
- Partial application of a `Ws.*` kernel (`let f = Ws.withOnMessage cb`):
  whatever the current kernel partial-application story is (Stream kernels
  share it), the behavior must be a clean diagnostic, not a miscompile —
  add one negative fixture mirroring the existing Stream/Auth gates if such
  a fixture family exists; otherwise note it as shared-with-Stream.
- Oversize frame (> maxMessageBytes): in-loop check closes the socket
  (server.rs:816-826). H1's ping arm must not resurrect closed peers
  (registry removal happens once, after loop exit).
- Two simultaneous peers: ids are `AtomicI64`; echo isolation is per-`ws_loop`
  state. Cheap 2-client extension of the e2e — worth the ~10 lines.

## 8. Estimate

Runtime adapters S (~100 lines + tests). Compiler wiring M (12 kernels ×
7 mechanical sites, compiler-exhaustiveness-guided). E2E S (~150 lines,
mostly cloned). H1 heartbeat XS. Total **L**, one build-lane pass — the
plan's XL assumed a from-scratch 300-450-line runtime module that turned out
to already exist.
