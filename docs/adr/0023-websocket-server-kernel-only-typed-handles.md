Status: Accepted

# 0023. Ipe.Http.Server.WebSocket is kernel-only with typed opaque handles and bounded fail-fast send

## Context

The runtime WebSocket-server section shipped from upstream's runtime-rust with
local hardening (origin glob gating, canonical headers, task-local upgrade
smuggling). What was missing was the compiler wiring (canon → kernels →
constrain → lower → emit). The original plan assumed builders like
`defaultCfg`/`with*` would be compiled Ipê stdlib source, but `ipe` embeds no
`Ipe.Http.*` stdlib modules, so every piece must be a kernel.

## Decision

Kernel-only module: register a `Ws` qualifier + 12 kernels (7 builders,
`upgrade`, 4 send/broadcast/close ops), with no stdlib `.ipe` port — mirroring
`Server`/`Stream`. Introduce two new opaque, monomorphic IR types (no phantom
var): `IrType::WebSocketServer` (renders `WsHandle`, a Copy i64) and
`IrType::WebSocketServerCfg` (renders `WsServerCfg<IpeError>`). Kernels take the
typed `WsHandle` directly, not a raw `Int`. Keep the runtime's bounded
non-blocking `try_send` (256-frame queue, drop-on-full) — the sound default for
effect kernels (fail-fast prevents handler-task pileup behind one slow peer),
matching the HTTP response-writing family.

Rejected alternatives:

- **Pure-Ipê routing** — would require embedded `Ipê/Http/Server/WebSocket.ipe`,
  `Ffi.kernel` resolution, and the `runtimeOpaqueTypes` machinery `ipe` replaced
  with dedicated `IrType` variants.
- **Untyped `Int` handles** — typed handles leverage the compiler's
  exhaustiveness checks and prevent handle/integer confusion.
- **Long-blocking send (~30 s)** — bounded fail-fast is the architectural
  default; if overruled it is a 3-line adapter change.

## Consequences

- The `Ws` qualifier + 12 kernels are the stable compiler↔runtime contract
  (example 33-websocket-echo builds through them). Send is bounded
  (`IPE_WS_SEND_BUFFER=256`); reusing a stale handle after close yields a clean
  `Err` (registry miss).
- **Invariant that must keep holding:** origin glob gating with CSWSH hardening
  is mandatory in production — empty `originPatterns` → 403 fail-closed. The
  bounded-send and the missing 30 s heartbeat (dead peers linger until TCP gives
  up) are recorded oracle divergences (both Rust runtimes share them); the
  heartbeat is filed as follow-on hardening H1, not a blocker.
