# rt-net findings

5 findings: 0 critical, 0 high, 1 medium, 4 low

Audited (full read): `src/runtime/rust/src/server.rs`, `server_stream.rs`,
`http_client.rs`, `http_stream.rs`, `ssrf.rs`, `ws_client.rs`, `http_header.rs`.

Prior `runtime-audit-verdict.md` items for this surface are largely FIXED and
were NOT re-filed: http buffered-client secret redaction (now
`ipe_error_from_foreign` on send + body-read errors), SSRF guard default-ON in
production (`ssrf_deny_private_enabled` ties to `production_from_env`),
`http_stream_open` invalid-method now errors instead of downgrading to GET,
ws_client connect-error URL userinfo now redacted (`redact_ws_url`/`safe_url`),
streaming-sentinel builder failure now returns 500 instead of leaking the
sentinel+nonce. The residual items below are the low-severity remainders plus
one completeness regression from making the SSRF guard default-on.

## rt-net-001 · wss:// WebSocket client is unusable under the production-default SSRF guard
- severity: medium
- axis: completeness
- principle: P5 completeness (claimed capability silently/partly fails) — interacts with P1 default-secure
- location: `src/runtime/rust/src/ws_client.rs:244-268` (pinned branch), `src/runtime/rust/src/ssrf.rs:36-41` (`ssrf_deny_private_enabled` default-on in prod)
- reachability: `WebSocket.connect`/`connectWith` are wired Task-tier kernels. In production `ENV`/`IPE_ENV` is set, so `ssrf_deny_private_enabled()` returns true by default; `ssrf_pinned_ws_addr` then returns `Some(addr)`, and `do_connect` hits the `if url.starts_with("wss://")` arm.
- problem: With the guard active (the production default now), every `wss://` client connection returns `Err("wss with IPE_HTTP_DENY_PRIVATE is unsupported (no TLS feature to pin the connection)")`. The client crate builds no TLS backend, so a pinned dial can only be plaintext `ws://`. Net effect: an app that worked in dev (guard off, or `ws://`) fails all TLS WebSocket connections in the default production config — the one place TLS is mandatory. It fails closed (an `Err`, not a silent no-op), so it is not a security hole, but a core capability is broken by the secure-by-default change with no divergence record.
- fix direction: build a TLS backend for the ws client (rustls) so pinned `wss://` can dial a vetted addr over TLS; until then, record the no-TLS-client limitation in `docs/divergences-from-sky.md` and make the guard resolve-and-validate `wss://` without requiring a plaintext pin.
- prior: new (regression surfaced by the sanctioned "SSRF default-on in production" fix from runtime-audit-verdict.md http #2)

## rt-net-002 · Streaming handler registry leaks on non-sentinel response paths
- severity: low
- axis: security (unbounded resource)
- principle: P1 no unbounded resource a remote party can grow
- location: `src/runtime/rust/src/server_stream.rs:48-51,116-119,196-199`
- reachability: `Server.Stream.stream` inserts an `Arc` handler (capturing app state) into `pending_handlers` under a token and returns a `ServerResponse` whose body is the sentinel. The entry is only removed when THAT exact response reaches `serve_streaming_sentinel`. A handler that calls `stream` but then returns a different response (early-error branch, or middleware that rewrites/drops the sentinel body) never frees the token.
- problem: `pending_handlers` has no TTL, no size cap, and no reaping; a leaked token retains its captured closure for the process lifetime. Repeated over many requests this is an unbounded-memory path reachable by ordinary (if non-idiomatic) handler shapes.
- fix direction: stamp each entry with an `Instant` and reap entries older than a short TTL on the insert path (mirror the amortized `RL_SWEEP_EVERY` sweeps), and/or cap the map with oldest-eviction.
- prior: runtime-audit-verdict.md `server` security #1 (still present)

## rt-net-003 · WebSocket server per-message size cap enforced only after full transport buffering
- severity: low
- axis: security (bounded-but-large resource)
- principle: P1 no unbounded/oversized resource a peer can force
- location: `src/runtime/rust/src/server.rs:842-921` (`ws_loop`, size checks at 877/884)
- reachability: an origin-allowed (or, in dev, same-origin) peer that passed `server_web_socket_upgrade`'s gate sends a large Text/Binary frame.
- problem: axum 0.7's `WebSocketUpgrade` exposes no `max_message_size`/`max_frame_size`, so tungstenite buffers up to its own default (~16 MiB frame / 64 MiB message) BEFORE `ws_loop` can check `t.len() > max_bytes` and close. A configured `maxMessageBytes` (e.g. 1 KiB) does not bound transport buffering; many connections → memory pressure. Bounded per connection and gated behind the origin allowlist, hence low. (The client side already sets `WebSocketConfig{max_message_size,max_frame_size}` via tokio-tungstenite; the server side is the asymmetric gap.)
- fix direction: set the transport limit at upgrade time once the crate exposes it (upgrade axum and call `.max_message_size()`/`.max_frame_size()` on the `WebSocketUpgrade`); until then document the transport floor.
- prior: runtime-audit-verdict.md `server` security #2 (still present)

## rt-net-004 · ws_subscribed marker leak under a connect/close race (residual window)
- severity: low
- axis: soundness (unreclaimable resource)
- principle: P1 no unbounded resource
- location: `src/runtime/rust/src/ws_client.rs:565-582,607,636,656,694` (`ws_registered` then `ws_mark_subscribed`), `115-124` (`deregister`)
- reachability: a subscribe racing a socket close/deregister.
- problem: the `ws_registered(socket_id)` gate (registry lock) and `ws_mark_subscribed` (separate `ws_subscribed` lock) are not one atomic critical section. If `deregister` runs its `retain(sid != id)` in the window between the two, `ws_mark_subscribed` inserts `(id,kind)` for an already-gone socket; because ids are monotonic and never reused, that marker is never reclaimed. The `ws_registered` gate narrows but does not close the window. Bounded by reconnect churn (not remotely amplifiable), hence low.
- fix direction: make check-and-mark atomic — hold both locks in a fixed order across the membership check and the insert, or fold the subscribed-set into the registry value so one map lock covers both; or have the drain task self-remove its `(id,kind)` marker when `subscribe_events` returns `None`.
- prior: runtime-audit-verdict.md `pubsub-ws` security #2 (partially mitigated, residual)

## rt-net-005 · Bridge structs keep HTTP status/method stringly/int-typed at the trust boundary
- severity: low
- axis: soundness (invalid states representable — smell)
- principle: parse-don't-validate / make-invalid-states-unrepresentable
- location: `src/runtime/rust/src/server.rs:51-52` (`ServerResponse.status: i64`, `method: String`), `src/runtime/rust/src/http_client.rs:54-70` (`HttpResponse.status: i64`, `HttpRequest.method: String`)
- reachability: constructible by codegen / any handler; defended in depth at every use site (`status.clamp(100,599)` before `from_u16`; `Method::from_bytes` parse at send with an Err path; verb re-cased at routing).
- problem: an HTTP status is one-of `100..599` and a method is a closed verb set, yet both are modeled as open `i64`/`String`, so nonsense values (`99999`, `"GET\r\nHost:"`) are representable and safety rests on every call site remembering to clamp/parse. No reachable panic or injection today (builder rejects bad header bytes → 500; status clamped everywhere), so this is a smell, not a live hole.
- fix direction: introduce an `HttpStatus` newtype (`new(i64) -> clamped`) stored on the response, and parse the method once into a `Method` enum at route/request construction; keep the raw `String` only where it mirrors the Ipê record for app inspection.
- prior: runtime-audit-verdict.md `server`/`http` invalid-state items (still present, unchanged)
