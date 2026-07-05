# Effect-Heavy Stdlib Modules — Execution Design (#111)

> **Scope.** Five sweep-blocking modules: `Std.Cli` (ex 20), `Std.Auth`
> (ex 12), `Sky.Http.Server.Stream` (ex 30), `Sky.Core.Http.Stream`
> (ex 32), `Sky.Http.Server.WebSocket` (ex 33).
>
> **Status snapshot** (2026-07-04, after commit `4dfc7ba`).
> `lower.rs` + `constrain.rs` + `emit_model_gate.rs` edits are in the
> working tree, entangled with #108.

---

## 1. High-level status matrix

| Module | KernelFn variants | Runtime impl | `lower_callee` arm | `callee_arity` | `get_scheme` | `naming.rs` | `emit_*` wiring | Sweep example |
|---|---|---|---|---|---|---|---|---|
| **Std.Cli** `Cli.program` | ✅ `CliProgram` (committed) | ✅ `tea.rs::cli_program` | ✅ WD | ✅ WD | ✅ WD (closed-cfg shape) | ✅ committed | ✅ `emit_cli.rs` committed | **20-cli-counter** |
| **Std.Auth** (9 kernels) | ✅ 9 variants (committed) | ✅ `auth.rs` (complete) | ✅ WD | ✅ WD | ❌ `return None` | ✅ committed | ✅ standard path via naming | **12-skyvote** |
| **ServerStream** (4 kernels) | ✅ 4 variants (committed) | ✅ `server_stream.rs` | ✅ WD | ✅ WD | ❌ `return None` | ✅ committed | ✅ standard path | **30-sse-server-demo** |
| **HttpStream** (3 kernels) | ✅ 3 variants (committed) | ✅ `http_stream.rs` | ✅ WD | ✅ WD | ❌ `return None` | ✅ committed | ✅ standard path | **32-sse-relay** |
| **ServerWebSocket** (12 kernels) | ✅ 12 `Ws*` variants (task #127) | ✅ adapters in `server.rs:1059+` (task #127) | ✅ (task #127) | ✅ (task #127) | ✅ (task #127) | ✅ (task #127) | ✅ (task #127) | **33-websocket-echo** ✅ |

**WD** = working-tree only (committed to disk, not `git add`-ed); the
lower.rs / constrain.rs / emit_model_gate.rs hunks are entangled with
#108 changes and must land in a dedicated #111 commit once #108 is
merged.

---

## 2. Per-module analysis

### 2.1 Std.Cli — `Cli.program` (example 20-cli-counter)

**Used API (from Main.sky):**
```elm
Cli.program
    { init         : () -> (Model, Cmd Msg)
    , update       : Msg -> Model -> (Model, Cmd Msg)
    , view         : Model -> String           -- prints to stdout
    , subscriptions: Model -> Sub Msg
    , onLine       : String -> Msg             -- maps each stdin line
    }
|> Task.run
```

#### API surface table

| Sky fn | Kind | `KernelFn` | Arity | Runtime fn | Impl |
|---|---|---|---|---|---|
| `Cli.program cfg` | kernel | `CliProgram` | 1 | `cli_program` | ✅ `tea.rs` |

#### Layer status

All layers are wired in the committed+WD state:

- `sky_kernels` — `CliProgram` variant, `KernelClass::Cli`, `decl` entry (`"cli_program"`, arity 1) — **committed**.
- `lower.rs` — `lower_callee` arm `("Cli", "program") => Ok(Callee::Kernel(KernelFn::CliProgram))` — **WD**.
- `constrain.rs` — full closed-cfg scheme (5 fields: `init`, `update`, `view`, `subscriptions`, `onLine`) — **WD**.
- `naming.rs` — `CliProgram => "cli_program_"` — **committed** (via standard naming table).
- `emit_cli.rs` — `emit_cli_call` / `emit_cli_inner` / `model_ty_of_view` gate — **committed**.

#### `Cli.program` entry design (reference)

`emit_cli.rs` (187 lines, committed) decomposes the cfg record into 5
named fields and emits:

```rust
sky_runtime::cli_program(
    Main_init,    // FInit : () -> SkyTuple2<Model, SkyCmd<Msg>>
    Main_update,  // FUpdate: Msg -> Model -> SkyTuple2<Model, SkyCmd<Msg>>
    Main_view,    // FView  : Model -> String
    Main_subs,    // FSubs  : Model -> SkySub<Msg>
    Main_on_line, // FOnLine: String -> Msg
)
```

The runtime (`tea.rs::cli_program`) spawns a blocking stdin reader
thread, uses `tokio::sync::mpsc::unbounded_channel` for inter-thread
event delivery, and shares a `SubManager<M>` for `Sub.every` support.
Identical TEA loop to `tui_app`/`live_app`.

#### Gap to close

Land the WD changes (lower.rs + constrain.rs) in a standalone #111
commit, separate from #108. No code to write — it already exists.

**Effort: XS** (1 commit, landing WD hunks).

---

### 2.2 Std.Auth (example 12-skyvote)

**Used API (from `src/Lib/Auth.sky`):**
```elm
Auth.hashPassword   : String -> Result Error String
Auth.verifyPassword : String -> String -> Result Error Bool
```

Example 12 does NOT call `Auth.register`, `Auth.login`, or
`Auth.signToken`/`verifyToken` — it implements its own SQLite-backed
user table via `Std.Db`. Only the two pure `Result`-returning kernels
are in the critical path for unblocking the sweep.

#### API surface table (all 9 stdlib kernels)

| Sky fn | Kind | `KernelFn` | Arity | Runtime fn | Tier | WD lower arm | WD `get_scheme` |
|---|---|---|---|---|---|---|---|
| `hashPassword : String -> Result Error String` | kernel | `AuthHashPassword` | 1 | `auth_hash_password` | Result | ✅ | ❌ `None` |
| `hashPasswordCost : String -> Int -> Result Error String` | kernel | `AuthHashPasswordCost` | 2 | `auth_hash_password_cost` | Result | ✅ | ❌ `None` |
| `verifyPassword : String -> String -> Result Error Bool` | kernel | `AuthVerifyPassword` | 2 | `auth_verify_password` | Result | ✅ | ❌ `None` |
| `passwordStrength : String -> Result Error String` | kernel | `AuthPasswordStrength` | 1 | `auth_password_strength` | Result | ✅ | ❌ `None` |
| `signToken : String -> a -> Int -> Result Error String` | kernel | `AuthSignToken` | 3 | `auth_sign_token` | Result | ✅ | ❌ `None` |
| `verifyToken : String -> String -> Result Error a` | kernel | `AuthVerifyToken` | 2 | `auth_verify_token` | Result | ✅ | ❌ `None` |
| `register : Db -> String -> String -> Task Error Int` | kernel | `AuthRegister` | 3 | `auth_register` | Task (async) | ✅ | ❌ `None` |
| `login : Db -> String -> String -> Task Error Int` | kernel | `AuthLogin` | 3 | `auth_login` | Task (async) | ✅ | ❌ `None` |
| `setRole : Db -> Int -> String -> Task Error ()` | kernel | `AuthSetRole` | 3 | `auth_set_role` | Task (async) | ✅ | ❌ `None` |

Two additional stdlib bindings (`signTokenWithClaims`,
`verifyTokenWithAlgorithm`) are **compiled Sky source** routing through
`Sky.Core.Jwt` — no kernel wiring needed for them.

#### Runtime signatures (auth.rs, all complete)

```rust
// Pure (Result-returning, no async):
pub fn auth_hash_password<E: From<String>>(pw: String) -> SkyResult<E, String>
pub fn auth_hash_password_cost<E: From<String>>(pw: String, cost: i64) -> SkyResult<E, String>
pub fn auth_verify_password<E: From<String>>(pw: String, hash: String) -> SkyResult<E, bool>
pub fn auth_password_strength<E: From<String>>(pw: String) -> SkyResult<E, String>
pub fn auth_sign_token<E: From<String>>(secret: String, claims: SkyAny, ttl: i64)
    -> SkyResult<E, String>
pub fn auth_verify_token<E: From<String>>(secret: String, token: String)
    -> SkyResult<E, SkyAny>

// Async (Task-returning, sqlx pool):
pub fn auth_register<E: Send + From<String> + 'static>(
    pool: SkyAny, username: String, password: String) -> SkyTask<E, i64>
pub fn auth_login<E: Send + From<String> + 'static>(
    pool: SkyAny, email: String, password: String) -> SkyTask<E, i64>
pub fn auth_set_role<E: Send + From<String> + 'static>(
    pool: SkyAny, user_id: i64, role: String) -> SkyTask<E, ()>
```

`auth_login` includes an anti-timing-oracle defense: on unknown email
it runs a dummy `bcrypt::verify` to keep latency constant.

#### `get_scheme` gap

`constrain.rs` currently returns `None` for all 9 Auth kernels. This
means:

- HM inference does NOT apply the kernel's declared type scheme to
  call sites.
- Call sites are typed purely by their surrounding context (explicit
  annotations, use-site inference).
- For example 12 (fully annotated), this likely does not cause a type
  error, but it means `hashPassword "pw"` in an unannotated context
  would leave the return type as `any`.
- The proper fix is to add a `get_scheme` arm for each variant.

**Seal note:** `AuthSignToken` and `AuthVerifyToken` are polymorphic
(`a` type var). Their schemes require a fresh `var(0)` — the same
pattern as `ListHead` and similar. Needs Opus review to confirm the
scheme unification is sound (particularly `verifyToken`'s return type
`Result Error a` where `a` is free).

#### Gap to close

1. Land WD `lower.rs` + `constrain.rs` hunks (same commit as Cli —
   just the #111 block).
2. **Optional for ex 12 unblocking**: add proper `get_scheme` arms for
   the 9 Auth kernels. The `return None` may be tolerated for
   fully-annotated code, but correct schemes prevent silent `any`
   widening in user code.

**Effort: S** (landing WD + writing 9 schemes ≈ 60 lines in
constrain.rs, patterns directly follow existing `Result`-returning
kernel schemes).

---

### 2.3 Sky.Http.Server.Stream (example 30-sse-server-demo)

**Used API (from example 30 Main.sky):**
```elm
Stream.stream    : String -> (StreamWriter -> Task Error ()) -> Task Error Response
Stream.emit      : String -> StreamWriter -> Task Error ()
Stream.finish    : StreamWriter -> Task Error ()
```

`withContentType` is not used by ex 30 but is in the stdlib.

#### Internal kernel signatures (what lower sees)

`Stream.emit`, `Stream.finish`, `Stream.withContentType` are wrappers
that unwrap the `StreamWriter Int` ADT before calling the raw kernel
(`emitRaw`, `finishRaw`, `withContentTypeRaw`):

| Sky kernel binding | `KernelFn` | Arity (kernel) | Runtime fn | Impl |
|---|---|---|---|---|
| `streamRaw : String -> (StreamWriter -> Task Error ()) -> Task Error Response` | `StreamStream` | 2 | `server_stream_stream` | ✅ |
| `emitRaw : String -> Int -> Task Error ()` | `StreamEmit` | 2 | `server_stream_emit` | ✅ |
| `finishRaw : Int -> Task Error ()` | `StreamFinish` | 1 | `server_stream_finish` | ✅ |
| `withContentTypeRaw : String -> Int -> Task Error ()` | `StreamWithContentType` | 2 | `server_stream_with_content_type` | ✅ |

Note: `streamRaw` receives `(StreamWriter -> Task Error ())` — the
callback still receives a `StreamWriter` ADT, not a raw Int. The
runtime constructs the ADT value and passes it to the handler.

#### `get_scheme` gap

Same situation as Auth: `return None` for all 4 variants. For
well-annotated example 30, `stream "text/event-stream" (\writer ->
...)` is typed by its return position (`Task Error Response`), so
inference likely succeeds. Adding proper schemes ensures the `writer`
parameter in the callback is typed as `StreamWriter`, not `any`.

**Effort: S** (4 schemes; `StreamStream`'s callback-argument scheme
needs the `StreamWriter` ADT referenced via a `Ty::Named` constructor —
check how `ws_client.rs` schemes reference `WebSocketMessage` for the
pattern).

---

### 2.4 Sky.Core.Http.Stream (example 32-sse-relay)

**Used API (from example 32 Main.sky):**
```elm
HttpStream.open         : HttpRequest -> Task Error StreamId
HttpStream.forEachChunk : StreamId -> (String -> Task Error ()) -> Task Error ()
```

#### Internal kernel signatures

| Sky kernel binding | `KernelFn` | Arity | Runtime fn | Impl |
|---|---|---|---|---|
| `openRaw : HttpRequest -> Task Error Int` | `HttpStreamOpen` | 1 | `http_stream_open` | ✅ |
| `forEachChunkRaw : Int -> (String -> Task Error ()) -> Task Error ()` | `HttpStreamForEachChunk` | 2 | `http_stream_for_each_chunk` | ✅ |
| `closeRaw : Int -> Task Error ()` | `HttpStreamClose` | 1 | `http_stream_close` | ✅ |

`chunks : StreamId -> (ChunkEvent -> msg) -> Sub msg` (the Sub-tier
variant) maps to `sub_subscribe_stream` — already implemented in
`http_stream.rs`. Its kernel routing follows the Sub path, not
`KernelFn`; confirm it's wired in the Sub dispatch table.

#### `get_scheme` gap

Same pattern. `openRaw` scheme: `HttpRequest -> Task Error Int`
(straightforward). `forEachChunkRaw` scheme requires a callback type
`String -> Task Error ()` embedded in the outer `Task`.

**Effort: XS** (3 schemes; all straightforward scalar types).

---

### 2.5 Sky.Http.Server.WebSocket (example 33-websocket-echo)

**Used API (from example 33 Main.sky):**
```elm
Ws.upgrade         : Request -> WebSocketServerCfg msg -> Task Error Response
Ws.defaultCfg      : WebSocketServerCfg msg          -- compiled Sky (pure record)
Ws.withOnConnect   : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg msg -> WebSocketServerCfg msg
Ws.withOnMessage   : (WebSocketServer -> String -> Task Error ()) -> WebSocketServerCfg msg -> WebSocketServerCfg msg
Ws.withOnClose     : (WebSocketServer -> Task Error ()) -> WebSocketServerCfg msg -> WebSocketServerCfg msg
Ws.withOnError     : (WebSocketServer -> Error -> Task Error ()) -> WebSocketServerCfg msg -> WebSocketServerCfg msg
Ws.withOriginPatterns : List String -> WebSocketServerCfg msg -> WebSocketServerCfg msg
Ws.sendToClient    : WebSocketServer -> String -> Task Error ()
```

`defaultCfg` and all `with*` builders are **compiled Sky source** (pure
record constructors/updates). Only `upgrade`, `sendToClient`,
`sendBinaryToClient`, `broadcast`, `closeClient` are kernels.

#### Full kernel surface

| Sky kernel binding | `KernelFn` (to add) | Arity | Proposed runtime fn | Status |
|---|---|---|---|---|
| `upgradeRaw : Request -> WebSocketServerCfg msg -> Task Error Response` | `WsServerUpgrade` | 2 | `ws_server_upgrade` | ❌ missing |
| `sendToClientRaw : Int -> String -> Task Error ()` | `WsServerSendToClient` | 2 | `ws_server_send_to_client` | ❌ missing |
| `sendBinaryToClientRaw : Int -> String -> Task Error ()` | `WsServerSendBinaryToClient` | 2 | `ws_server_send_binary_to_client` | ❌ missing |
| `broadcastRaw : List Int -> String -> Task Error ()` | `WsServerBroadcast` | 2 | `ws_server_broadcast` | ❌ missing |
| `closeClientRaw : Int -> Task Error ()` | `WsServerCloseClient` | 1 | `ws_server_close_client` | ❌ missing |

#### `WebSocketServerCfg` shape at the kernel boundary

`upgrade req cfg` receives the full `WebSocketServerCfg msg` Sky ADT.
The runtime must:

1. Destructure the ADT to extract the 6 fields
   (`onConnect`, `onMessage`, `onClose`, `onError`,
   `maxMessageBytes`, `originPatterns`).
2. Upgrade the HTTP connection to WebSocket (axum's
   `WebSocketUpgrade`).
3. Spawn a per-peer task that calls the Sky callbacks.

```rust
// Sky type: WebSocketServerCfg msg
// Runtime representation: a SkyRecord (or typed struct in ws_server.rs)
pub struct WsServerCfg {
    pub on_connect:        SkyAny,   // WebSocketServer -> Task Error ()
    pub on_message:        SkyAny,   // WebSocketServer -> String -> Task Error ()
    pub on_close:          SkyAny,   // WebSocketServer -> Task Error ()
    pub on_error:          SkyAny,   // WebSocketServer -> Error -> Task Error ()
    pub max_message_bytes: i64,
    pub origin_patterns:   Vec<String>,
}
```

The runtime constructs `WebSocketServer(id)` ADT values (Sky's opaque
`type WebSocketServer = WebSocketServer Int`) via `SkyVariant::new` and
passes them into each callback invocation.

#### `ws_server.rs` design sketch

```rust
//! Sky.Http.Server.WebSocket — server-side WebSocket upgrade (axum).
//!
//! Uses axum::extract::ws::WebSocketUpgrade.  Each accepted peer gets a
//! unique i64 id (AtomicI64 counter); the registry is a global
//! Mutex<HashMap<i64, WsServerEntry>> holding the send half of
//! an mpsc::Sender<WsServerCmd>.  Same registry pattern as ws_client.rs.

struct WsServerEntry {
    tx: mpsc::Sender<WsServerCmd>,
}

enum WsServerCmd {
    SendText(String),
    SendBinary(Vec<u8>),
    Close,
}

/// Upgrade a request to WebSocket; returns the streaming sentinel Response.
/// The dispatcher in server.rs detects the sentinel and calls `serve_ws_sentinel`.
pub fn ws_server_upgrade<E: From<String> + Send + 'static>(
    req: ServerRequest,
    cfg: SkyAny,
) -> SkyTask<E, ServerResponse>

/// Called by server.rs after detecting the WebSocket upgrade sentinel.
pub fn serve_ws_sentinel(r: &ServerResponse) -> Option<axum::response::Response>

/// Send a text frame to the connected peer.
pub fn ws_server_send_to_client<E: From<String> + Send + 'static>(
    id: i64, text: String) -> SkyTask<E, ()>

/// Send a binary frame.
pub fn ws_server_send_binary_to_client<E: From<String> + Send + 'static>(
    id: i64, data: String) -> SkyTask<E, ()>

/// Fan out a text frame to multiple peers; best-effort (skips closed peers).
pub fn ws_server_broadcast<E: From<String> + Send + 'static>(
    ids: SkyList, text: String) -> SkyTask<E, ()>

/// Disconnect a peer.
pub fn ws_server_close_client<E: From<String> + Send + 'static>(
    id: i64) -> SkyTask<E, ()>
```

**Upgrade sentinel pattern** mirrors `server_stream_stream`: the
`ws_server_upgrade` function stashes the per-peer cfg in a one-shot
registry keyed by a nonce id embedded in the sentinel Response body.
`serve_ws_sentinel` in `server.rs` extracts the nonce, pops the cfg,
and runs the WebSocket handshake.

**Origin validation**: in production mode (`ENV != dev`) reject upgrades
if `cfg.origin_patterns` is empty (mirror the Go runtime's `403` gate).
Check `sky_runtime::is_production()`.

**Callback invocation pattern**: use `sky_runtime::sky_call_1` /
`sky_call_2` to invoke the opaque `SkyAny` callback fields, constructing
the `WebSocketServer(id)` ADT value before each call.

#### Seal notes (Opus review required)

1. **Origin-pattern matching.** The Go runtime uses `nhooyr.io/websocket`'s
   glob matching; the Rust side should use the same glob syntax. Confirm
   the matching semantics (`*` = any subdomain, `*` alone = any origin)
   before shipping. A wrong match in production mode → silent DoS or
   security bypass. Needs Opus adversarial review.

2. **Callback error isolation.** The Go runtime isolates per-callback
   errors: an erroring `onMessage` on one peer does not abort others.
   The Rust implementation MUST do the same — each callback invocation
   runs in its own `tokio::spawn(async move { ... }.await.ok())` task
   rather than a sequential `?` chain.

3. **Backpressure.** `sendToClient` blocks up to 30 s when the write
   buffer fills. A 30 s timeout on the mpsc `tx.send_timeout` is the
   correct surface; unbounded send is forbidden (slow peer → unbounded
   queue growth).

4. **`msg` type parameter.** `WebSocketServerCfg msg` has a phantom
   `msg` type var (for potential future Sub integration). The Rust
   runtime receives `SkyAny` for the cfg; `msg` never appears at the
   Rust level. Confirm this causes no unsoundness in the constrain
   scheme (the scheme's `var(0)` for `msg` should unify freely).

**Effort: XL** — new 300–450 line runtime module + 5 KernelFn variants
+ lower_callee + callee_arity + get_scheme (including the complex
callback-in-cfg shape) + naming + integration test. Needs Opus guardian
design review before implementation.

---

## 3. Constrain scheme patterns

### 3.1 Result-returning kernels (Auth pure helpers)

```rust
// hashPassword : String -> Result Error String
K::AuthHashPassword =>
    fun(string(), result(error(), string())),

// verifyPassword : String -> String -> Result Error Bool
K::AuthVerifyPassword =>
    fun(string(), fun(string(), result(error(), bool()))),

// signToken : String -> a -> Int -> Result Error String
K::AuthSignToken => {
    scheme_with_vars(1, fun(string(), fun(var(0), fun(int(), result(error(), string())))))
}

// verifyToken : String -> String -> Result Error a
K::AuthVerifyToken => {
    scheme_with_vars(1, fun(string(), fun(string(), result(error(), var(0)))))
}
```

`scheme_with_vars(n, ty)` is the pattern used for polymorphic kernel
schemes (see `ListHead`, `MaybeWithDefault` in constrain.rs for the
exact builder).

### 3.2 Task-returning kernels (Auth async)

```rust
// register : Db -> String -> String -> Task Error Int
K::AuthRegister =>
    fun(db(), fun(string(), fun(string(), task(error(), int())))),

// setRole : Db -> Int -> String -> Task Error ()
K::AuthSetRole =>
    fun(db(), fun(int(), fun(string(), task_unit()))),
```

### 3.3 Stream callback schemes

`StreamStream` receives a callback whose argument is a `StreamWriter`
ADT. The scheme requires a `Ty::Named` for `StreamWriter`:

```rust
// streamRaw : String -> (StreamWriter -> Task Error ()) -> Task Error Response
K::StreamStream => {
    let writer_ty = Ty::Named {
        home: ModPath::from("Sky.Http.Server.Stream"),
        name: self.intern("StreamWriter"),
        args: vec![],
    };
    fun(string(), fun(fun(writer_ty, task_unit()), task(error(), response())))
}
```

Verify that `Ty::Named` for a stdlib ADT is sound here — see how
`Live.app`'s `Request` type is referenced in the Live constrain scheme
for the precedent.

---

## 4. Implementation dependency order

```
A1  Land WD changes as #111 commit (lower_callee + constrain CliProgram
      + callee_arity #111 block) ← unblocks ex 20 immediately
        ↓
A2  Add get_scheme arms for Auth/Stream/HttpStream (constrain.rs)
      ← unblocks ex 12, 30, 32 (combined with A1)
        ↓
A3  ws_server.rs runtime (new file, 300–450 lines)  ← SUPERSEDED: runtime
    already existed in server.rs:711-1057; task #127 added ~100-line
    adapters after line 1057 instead. See websocket-server-design.md §1.
        ↓
A4  KernelFn variants — 12 Ws* variants  ← DONE (task #127)
        ↓
A5  lower_callee + callee_arity arms for Ws*  ← DONE (task #127)
        ↓
A6  get_scheme arms for Ws* (complex cfg callback shape)  ← DONE (task #127)
        ↓
A7  naming.rs arms for Ws*  ← DONE (task #127)
        ↓
A8  server.rs sentinel — no change needed; task-local upgrade already
    wired (server.rs:782-787, method_router:617-619)  ← DONE (existing)
        ↓
DONE  ex 33 (websocket-echo) unblocked  ← DONE (task #127)
```

A1–A2 are independent of A3–A8 and can land immediately after #108
merges. A3–A8 form a sequential chain requiring Opus design review
at A3 before implementation starts.

---

## 5. Bite-sized Lane A task breakdown

| Task | Files touched | Effort | Blocks |
|---|---|---|---|
| **A1** Land #111 WD (lower.rs + constrain.rs CliProgram + callee_arity) | `lower.rs`, `constrain.rs` | XS | ex 20 |
| **A2** Add `get_scheme` for 9 Auth + 4 Stream + 3 HttpStream variants | `constrain.rs` (~90 lines) | S | ex 12, 30, 32 |
| **A3** Implement `ws_server.rs` runtime | new file `runtime/src/sky_runtime/ws_server.rs` | XL | ex 33 |
| **A4** Add 5 `WsServer*` `KernelFn` variants | `sky_kernels/src/lib.rs` | XS | — |
| **A5** Add `lower_callee` + `callee_arity` arms for WsServer* | `lower.rs` | XS | — |
| **A6** Add `get_scheme` arms for WsServer* | `constrain.rs` | S | — |
| **A7** Add `naming.rs` arms for WsServer* | `naming.rs` | XS | — |
| **A8** Wire WebSocket sentinel in server.rs | `server.rs` | S | ex 33 |

**A1 is the highest-ROI first step** — 1 commit that lands already-written
code and immediately unblocks example 20 (`Cli.program`) with zero new
implementation.

**A2 is the second step** — ~90 lines in `constrain.rs` unblocks
examples 12, 30, and 32. All runtime implementations are already
complete; this is purely type-scheme wiring.

**A3–A8** (WebSocket server, example 33) is a standalone track requiring
a new runtime module. Recommend Opus guardian design review of A3 before
implementation to confirm the sentinel/origin-gate/callback-isolation
design.

---

## 6. Effort summary

| Module | Total effort | Status after A1+A2 |
|---|---|---|
| Std.Cli (ex 20) | XS — WD landing only | ✅ unblocked |
| Std.Auth (ex 12) | S — 9 `get_scheme` arms | ✅ unblocked |
| ServerStream (ex 30) | S — 4 `get_scheme` arms | ✅ unblocked |
| HttpStream (ex 32) | XS — 3 `get_scheme` arms | ✅ unblocked |
| ServerWebSocket (ex 33) | L — runtime existed; 12-kernel wiring (task #127) | ✅ **shipped** (see `websocket-server-design.md`) |

**Examples 20, 12, 30, 32 can be unblocked in a single focused
session** (A1 + A2). Example 33 is a multi-session effort requiring
Opus review.

---

## 7. Files and locations quick reference

```
crates/sky_kernels/src/lib.rs          — KernelFn variants + decl() table
crates/sky_lower/src/lower.rs          — lower_callee() + callee_arity()
crates/sky_types/src/constrain.rs      — get_scheme() (HM type schemes)
crates/sky_backend_rust/src/naming.rs  — kernel_name() (Rust fn name string)
crates/sky_backend_rust/src/emit_cli.rs— Cli.program emit (committed)
runtime/src/sky_runtime/auth.rs        — Auth kernels (complete)
runtime/src/sky_runtime/server_stream.rs — ServerStream kernels (complete)
runtime/src/sky_runtime/http_stream.rs — HttpStream kernels (complete)
runtime/src/sky_runtime/tea.rs         — cli_program() (complete)
runtime/src/sky_runtime/ws_client.rs   — pattern reference for ws_server.rs
runtime/src/sky_runtime/ws_server.rs   — DOES NOT EXIST (task A3)
runtime/src/sky_runtime/server.rs      — sentinel dispatch (needs A8)
```

---

## 8. References

- `examples/20-cli-counter/src/Main.sky` — `Cli.program` usage
- `examples/12-skyvote/src/Lib/Auth.sky` — `Auth.hashPassword` + `Auth.verifyPassword`
- `examples/30-sse-server-demo/src/Main.sky` — `Stream.stream/emit/finish`
- `examples/32-sse-relay/src/Main.sky` — `HttpStream.open/forEachChunk` relay
- `examples/33-websocket-echo/src/Main.sky` — `Ws.upgrade/sendToClient`
- `sky-stdlib/Sky/Http/Server/WebSocket.sky` — full `WebSocketServerCfg` shape
- `docs/architecture/websocket-server-design.md` — full design for task #127 (12 kernels, D2/D4/D5 divergences, e2e spec)
- `sky-stdlib/Std/Auth.sky` — Auth kernel bindings
- `crates/sky_backend_rust/src/emit_cli.rs` — Cli.program emit design
- `runtime/src/sky_runtime/ws_client.rs` — registry pattern reference
