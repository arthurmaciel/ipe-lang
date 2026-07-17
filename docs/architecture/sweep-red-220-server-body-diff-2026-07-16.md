# #220 — server body-diff cluster (15/30/32/33): root cause + fix design

**Verdict up front: REAL shared-class divergence, NOT harness fuzz.** The
normalized-HTML fallback is not even in play: none of the compared bodies
contains `id="sky-root"`, so `_norm_body_for_equiv` (checks for that marker
before invoking `equivalence_normalize_html.py`) passes every body through RAW and
the DIFFER is a plain byte diff. The row-note wording "normalized-HTML
fallback" describes the *mode* the sweep fell back to for live-shaped
examples, not what actually ran here — all four are `Server.listen`
(Sky.Http.Server) apps on the `body` equivalence mode, raw compare.

## Per-example verdict + evidence

Method: fresh `skyc` (`master-gate-target/debug/skyc`, built at HEAD
`1ba31988`) → `skyc build sky.toml --out sky-out/rust` → `cargo build` →
Go reference via pinned `tools/oracle/bin/sky` (v0.17.3, `SKY_RUNTIME_DIR`
unset) → boot each binary, `curl` every comparable GET route, full-body
`diff`.

| Example | Routes compared | Verdict | Evidence |
|---|---|---|---|
| 15-http-server | `/`, `/api/status`, `/cookie-demo`, `/redirect` | REAL — banner-only | `/api/status` (JSON) + `/redirect` byte-IDENTICAL. `/` and `/cookie-demo` differ by exactly one appended element: `<a id="__sky-dev-console" href="/_sky/console" …>&#128269; Console</a>` present in Go, absent in Rust. |
| 30-sse-server-demo | `/` (`/events` skipped: SSE) | REAL — banner-only | Sole diff = same `__sky-dev-console` anchor appended after `</pre>`. |
| 32-sse-relay | `/` (`/upstream` `/relay` skipped: streaming) | REAL — banner-only | Sole diff = same anchor appended. |
| 33-websocket-echo | `/` (`/ws` skipped: ws) | REAL — banner-only | Full-document page: Go injects the anchor before `</body>`; Rust body otherwise byte-identical incl. `</body></html>`. |

One class, four instances: **Go's Sky.Http.Server injects the dev-console
banner into every `text/html` response in dev mode; our Rust
Sky.Http.Server never does.** Every other byte of every compared route is
identical — codegen, HTML escaping, headers-to-body behaviour, JSON and
redirect routes all match.

## Root cause

Go reference (`../sky/runtime-go/rt/rt.go` ~8140–8210, server dispatch
tail): after a handler returns, the runtime post-processes every buffered
response:

1. `setSecurityHeaders` — **ported** (`server.rs::to_axum_response`,
   `telemetry::security_headers()` if-unset policy).
2. For `Content-Type` prefix `text/html`:
   a. `injectCsrfIntoForms(body, CurrentCsrfToken(req))` — hidden
      `__sky_csrf` input into every POST form (only when the csrf cookie is
      already present) — **not ported**.
   b. `injectDevBanner(body, devBannerHTML())` — the `__sky-dev-console`
      anchor before the last case-insensitive `</body>`, else appended;
      `devBannerHTML` returns `""` in production (`productionFromEnv`) or
      when `SKY_DEV_BANNER=off|0`; href = `SKY_CONSOLE_URL` default
      `/_sky/console`, attribute-escaped — **not ported**. ← the observed diff.
3. `MountEmbeddedConsole(mux)` + `MountObservabilityEndpoints(mux)` before
   user routes (the surface the banner links to) — **not ported** for
   Sky.Http.Server; our `server_listen` (server.rs:670) mounts only user
   routes + CatchPanicLayer.

Our runtime HAS a byte-exact port of `devBannerHTML` — but only on the
Sky.Live path: `live/mod.rs::dev_console_banner` (private, called by
`render_page_full`; test `banner_byte_matches_go_dev_banner_markup` pins
Go's exact bytes). It is unreachable from the server path twice over:
private fn, and the `live` feature is OFF in emitted server apps (example
15's emitted `Cargo.toml`: `default = ["tokio","crypto","json","server"]`).
The sibling reference Rust runtime (`../sky/runtime-rust`) has the same gap
in its `server.rs` — we faithfully mirrored a hole; the Go runtime is the
oracle, and the oracle injects.

Structural framing (fix-the-structure): `to_axum_response` is the single
choke point every buffered Sky.Http.Server response passes through. Go's
dev-mode post-processing pipeline belongs there *as a unit*; porting it
piecemeal (headers yes, CSRF-inject no, banner no) is exactly how this
class of silent divergence arises.

## Fix design

**Phase 1 — banner injection (closes the 4 REDs).** ~40 lines + tests.

- Extract `dev_console_banner` out of `live/mod.rs` into a feature-neutral
  home — `telemetry.rs` fits (unconditional module, already owns
  `production_from_env`); `live/mod.rs` re-exports/calls it (zero byte
  change to the Live path).
- During extraction, reconcile the gate drift with Go's `devBannerHTML`:
  Go suppresses on `productionFromEnv` + `SKY_DEV_BANNER=off|0`; our live
  helper suppresses on production + non-empty `base` (sub-app) +
  `SKY_CONSOLE_EMBED=off|0|false` + `SKY_CONSOLE_AUTH=off`, and does NOT
  honour `SKY_DEV_BANNER`. Union both sets in the shared helper (add
  `SKY_DEV_BANNER`, keep the live-side gates) — suppression can only make
  bodies match MORE often in odd configs, and the sweep's env (nothing
  set) hits the injecting path either way.
- In `to_axum_response`: after the effective content-type is resolved
  (`r.contentType` when no handler `content-type` header override, else
  the header value — the resolution logic already exists at lines
  577–583), if it starts with `text/html` and the banner is non-empty,
  inject before the LAST case-insensitive `</body>`, else append (exact
  `injectDevBanner` semantics). Streaming responses are untouched: the
  `serve_streaming_sentinel` early-return sits above, same as Go where
  streams bypass the buffered `fmt.Fprint` path.
- Tests: (a) unit — injection before `</body>`, append fallback,
  non-HTML untouched, production/`SKY_DEV_BANNER` suppression; (b) reuse
  the pinned Go-bytes test against the shared helper; (c) E2E golden or
  sweep re-run — the four examples flip DIFFER → `equivalence-body N`.

**Phase 2 — console + observability mounts for Sky.Http.Server
(completeness, separate backlog item).** Go mounts `MountEmbeddedConsole`
+ `MountObservabilityEndpoints` on the server mux before user routes; our
`server_listen` mounts nothing, so the injected banner's `/_sky/console`
link 404s in a server-only app until this lands. The console
implementation (`live/console.rs`, `live/observability.rs`) is gated
under `live`; the port needs either a dual gate
(`any(feature="server", feature="live")`) or extraction of the console
surface from `live/`. Deliberately NOT bundled with Phase 1: Phase 1
restores byte parity of route bodies (what the sweep gates); the mount is
an endpoint-surface gap the sweep does not currently probe. File it —
per no-deferral it enters the pipeline, not a footnote.

**Phase 3 — CSRF form injection (latent, same choke point).** No current
example reds on it (fires only when the csrf cookie is present and the
HTML contains a POST form), but any future server example with a form +
csrf middleware will DIFFER the same way. Port `injectCsrfIntoForms` into
the same `to_axum_response` HTML branch (order: CSRF first, then banner —
Go's order). Own backlog item.

## Harness recommendation

**No harness change.** The equivalence is doing precisely its job — it caught a
real missing runtime feature. Do NOT pin these four in
`equivalence-classification.tsv` (loses the byte-compare that proved everything
else identical), do NOT loosen `equivalence_normalize_html.py` (it never ran
here), and do NOT boot the Go reference with `SKY_DEV_BANNER=off` (would
mask this whole class permanently). One cosmetic nit, optional: the
`DIFFER` row note for server-shape examples says "route body differs";
only the live-shape fallback path mentions HTML normalisation — the
backlog entry's "via the normalized-HTML fallback" phrasing was a
misreading of the sweep banner NOTE line, worth remembering when triaging
future server DIFFERs.

## Confidence

HIGH. Full-body diffs on all comparable GET routes of all four examples
show the banner anchor as the ONLY divergence; two non-HTML routes are
byte-identical; the Go injection site, the Go banner builder, our missing
server-side counterpart, and our existing byte-exact live-side port are
all identified by file:line. Fix is a bounded port into a single existing
choke point with a pre-existing pinned-bytes test.
