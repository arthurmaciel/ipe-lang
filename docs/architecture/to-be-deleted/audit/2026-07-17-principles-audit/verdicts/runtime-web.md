# runtime-web verdicts

Adversarial verification of rt-net (5), rt-live (5), rt-ui (5) — 15 findings.
Read-only; every verdict traced against the cited `file:line`.

## RT-NET-001 · CONFIRMED
- final severity: medium (completeness)
- reachability: production default. `ssrf_deny_private_enabled()` returns true
  whenever `ENV`/`IPE_ENV` is set (`ssrf.rs:36-40`). `ssrf_pinned_ws_addr`
  (`ssrf.rs:216-233`) then resolves ANY host — public OR private — to a
  `SocketAddr` and returns `Some(addr)` (only genuinely-private hosts error out
  at resolution). So EVERY `wss://` dial hits the `Some(addr)` arm in
  `do_connect` (`ws_client.rs:259-268`) and returns `Err`. Not attacker-set: the
  URL is app-code, the guard is operator-env.
- reasoning: the client crate builds no TLS backend (`MaybeTlsStream` with no TLS
  feature), so a pinned dial can only be plaintext `ws://`; the code refuses
  `wss://` rather than dial plaintext to a TLS endpoint. Net: ALL production
  `wss://` client connections fail-closed — a broken core capability in exactly
  the config where TLS is mandatory. It is an `Err`, not a silent no-op or a
  plaintext downgrade, so no secret leaks — completeness, not security.
- dup-of: —

## RT-NET-002 · CONFIRMED
- final severity: low (security / unbounded resource)
- reachability: app-author cooperation, NOT remote-attacker-driven. A handler
  calls `Server.Stream.stream` (inserts `token → Arc<closure>` into
  `pending_handlers`, `server_stream.rs:116-119`) but returns a DIFFERENT response
  (early-error branch, or middleware that rewrites/drops the sentinel body). The
  entry is removed ONLY when that exact sentinel body reaches
  `serve_streaming_sentinel` (`server_stream.rs:199`). A remote attacker cannot
  force this handler shape — the handler code is fixed by the author.
- reasoning: `pending_handlers` has no TTL, no size cap, no reaping. Each leaked
  token retains its captured closure (app state) for the process lifetime;
  repeated over many requests to such a handler it grows unbounded. Real, but
  gated behind a non-idiomatic author-written shape → low is correct.
- dup-of: —

## RT-NET-003 · CONFIRMED
- final severity: low (bounded-but-large resource)
- reachability: an origin-allowed peer (production requires passing
  `server_web_socket_upgrade`'s origin gate — empty `originPatterns` → 403; dev
  = same-origin) sends a large Text/Binary frame.
- reasoning: `ws_loop` (`server.rs:842`) checks `t.len() > max_bytes` at line 877/
  884 — AFTER `socket.recv()` returns a fully-buffered `Message`. The code comment
  (857-859) confirms axum 0.7 exposes no `max_message_size`/`max_frame_size`, so
  tungstenite buffers up to its own default (~16 MiB frame / 64 MiB message)
  before the app check runs. A configured `maxMessageBytes=1 KiB` does NOT bound
  transport buffering. Attacker-influenceable but bounded per connection and gated
  behind the origin allowlist → low correct. Client side already sets the
  tungstenite config; server is the asymmetric gap.
- dup-of: —

## RT-NET-004 · CONFIRMED
- final severity: low (soundness / unreclaimable resource)
- reachability: a subscribe racing a socket close/deregister; bounded by reconnect
  churn, not remotely amplifiable.
- reasoning: `ws_registered` (registry lock) and `ws_mark_subscribed` (separate
  `ws_subscribed` lock) are not one critical section (`ws_client.rs:565-582,607`
  vs `deregister` at 115-124). A `deregister` retain in the window leaves a
  never-reclaimed `(id,kind)` marker (ids monotonic, never reused). Narrowed by
  the `ws_registered` gate, not closed. Real residual leak, low correct.
- dup-of: —

## RT-NET-005 · CONFIRMED (smell)
- final severity: low (soundness / invalid-states-representable — smell)
- reachability: none live — defended in depth at every use site.
- reasoning: `ServerResponse.status: i64` / `method: String` (`server.rs:51-52`),
  same on `HttpResponse`/`HttpRequest` (`http_client.rs:54-70`). Nonsense values
  are representable, but `status.clamp(100,599)` before `from_u16` and
  `Method::from_bytes` (Err → 500) defend every reachable sink; no panic/injection
  today. Type-hole smell, not a live hole — low correct.
- dup-of: —

## RT-LIVE-001 · CONFIRMED
- final severity: low (security / secret-in-transit)
- reachability: operator misconfiguration (URL is operator-set, not attacker-set).
  `IPE_PARENT_URL=http://<non-loopback>` + `IPE_INGEST_TOKEN` ships the shared
  secret in cleartext.
- reasoning: verified the divergence directly. `hub_exporter::enable_from_env`
  parses the URL and refuses non-https/non-loopback, disabling the exporter
  (`hub_exporter.rs:102-119`). `push_exporter::enable_from_env` just
  `format!("{}/_ipe/observability/ingest", parent…)` with NO scheme/host check
  (`push_exporter.rs:75`) before the token is attached. Sibling exporters diverge;
  same secret-in-transit class the hub already guards. Low correct.
- dup-of: —

## RT-LIVE-002 · CONFIRMED (smell)
- final severity: low (soundness / invalid-states-representable — smell)
- reachability: input is the console operator's own UI filter, not untrusted
  external data; fallback is total (no-filter).
- reasoning: `HubLogFilter` = 4 bools → 16 states, only 5 defined;
  `pick_single_level` treats 0 or 2+ actives as "no filter". Illegal combos
  constructible but reach only a degraded-but-safe query. Type smell, not a hole —
  low correct.
- dup-of: —

## RT-LIVE-003 · CONFIRMED (smell), downgrade upheld
- final severity: low (security / injection-into-frame — smell)
- reachability: none live. `hello_sid` derives from `sid_from_cookie` (raw
  cookie), but the Cold-hit arm only fires when the sid keys a real persisted
  checkpoint, itself written under a hex `new_sid`. No untrusted `"`/`\` reaches
  the interpolation today.
- reasoning: verified the asymmetry — the hello frame at `mod.rs:1629` is
  hand-interpolated (`format!("…\"sid\":\"{hello_sid}\"…")`) while the adjacent
  resync frame at `mod.rs:1648` uses `serde_json::json!`. Hello is the one hand-
  built holdout. The auditor's own downgrade (medium→low) is sound — the
  store-keying hex invariant holds, so no untrusted value currently reaches it.
- dup-of: —

## RT-LIVE-004 · DOWNGRADED (real DoS, wrong location cited, wrong reachability)
- final severity: low (was medium) — CPU DoS, but the cited path is UNREACHABLE;
  the genuinely-reachable paths require app-author cooperation.
- reachability: **The cited style-marker path is defended, not reachable.** All
  five cited call sites (`style_inject.rs:219,249,289,330,334`) feed
  `strip_style_close` ONLY the OUTPUT of `SafeCssValue::parse` /
  `sink_safe_declaration_list` / `sink_safe_keyframes_body`. Every one of those
  routes each declaration through `SafeCssValue::parse` →
  `has_dangerous_css_pattern`, which REJECTS any value containing `</`
  (`css_safety.rs:89`). Since `</style` contains `</`, a `</style`-laden payload
  is dropped fail-closed BEFORE `strip_style_close` ever runs. So the auditor's
  "runs on every Live render over app/user CSS" reachability is REFUTED for the
  style-marker path.
  The O(n²) IS reachable via three OTHER callers the auditor missed, all
  bypassing the `</` gate: (1) `html.rs:430` — the `<style>` render sink strips an
  `Ipe.Css`/`Html.raw` body directly; (2) `ui/helpers.rs:498` — `styleNode`
  construction; (3) `css.rs:69` — the `Css.stripStyleClose` kernel, app-callable.
  Triggering the quadratic blow-up therefore requires the AUTHOR to route a large
  (~1 MB) attacker-derived string of `</style` fragments into a raw `<style>`
  body or the kernel — not automatic on every render of ordinary UI.
- reasoning: `strip_style_close` (`css_safety.rs:418-432`) is genuinely
  quadratic: `out.to_ascii_lowercase()` (full O(n) copy) + `find` + `replace_range`
  (O(n) tail shift) once per `</style`, ~n/7 passes ≈ O(n²/7). The algorithm is a
  real smell worth fixing to linear. But severity drops to low because the only
  reachable trigger is author-cooperation-gated, not a remote party exhausting CPU
  on ordinary rendering as the finding claimed.
- repro (author-cooperation): an app that does
  `Html.node "style" [] [Html.raw attackerCss]` (or `Css.stripStyleClose
  attackerCss`, or a `styleNode` whose CSS is built from request data) where
  `attackerCss` is ~1 MB of repeated `</style` — each render spends seconds of
  server CPU. NOT reachable through `Ui` Transition/Animation/hover markers (those
  are `</`-rejected upstream).
- dup-of: —

## RT-LIVE-005 · CONFIRMED (smell)
- final severity: low (completeness / invalid-states-representable — smell)
- reachability: `EventBody` from `/_ipe/event` POST; `Route.build` indexes the
  captured-params Vec. No panic reachable from the fixed compile-time route table
  (`match_route` returns exactly the pattern's param count).
- reasoning: `EventBody` keeps `handler_id`+`id` and `event`+`msg` reconciled by
  `if !x.is_empty()` ladders — both-empty / both-set-disagree representable
  post-deserialize; `Route` arity is a cross-field invariant not carried in a type
  (raw-`[]` index would panic only under codegen arity drift). Typed-boundary
  smells, not live holes — low correct.
- dup-of: —

## RT-UI-001 · CONFIRMED
- final severity: medium (soundness / process-abort)
- reachability: attacker-INFLUENCEABLE. Any Live/Webview/wasm view whose tree
  depth scales with Model data. A deep tree is constructible with O(1) app stack —
  `List.foldl (\_ acc -> Ui.el [] acc) base xs` wraps once per element over an
  attacker-length list — so the native runtime walker is the first and only
  overflow point. `render_element` runs on every commit; `diff_node` on every
  update.
- reasoning: verified the asymmetry directly. `html.rs` caps descent at
  `MAX_HTML_DEPTH = 1024` with an explicit comment that uncapped recursion "would
  overflow the thread stack and ABORT the whole process — a panic the
  no-runtime-error thesis forbids" (`html.rs:160-195`), and `dom/dispatch.rs::walk`
  was made ITERATIVE for the same reason ("Walk the view tree iteratively with an
  explicit heap work-stack", `dispatch.rs:60`). The two remaining walkers in the
  SAME data path recurse natively with NO cap: `render_element` →
  `render_node_as` → `kids.into_iter().map(render_element)` (`render.rs:501`), and
  `diff_node` recurses at `diff.rs:134`. A tree deeper than the stack aborts the
  process (uncatchable stack exhaustion) BEFORE the html-render cap is reached.
  The class is already recognised and closed in two sibling walkers; this is the
  same class left open. Severity medium (not critical) because it is a
  fail-crash, not a memory-safety/RCE, and depth to overflow is large.
- repro (soundness / process abort): an Ipê Live app with
  `view model = List.foldl (\_ acc -> Ui.el [] acc) (Ui.text "x") (List.range 1
  model.n)` where `model.n` comes from attacker input (form field / query param)
  and exceeds the native stack depth (~tens of thousands of frames). `ipe build`
  exit-0; the view renders fine for small `n`; a large `n` aborts the whole server
  process on render/diff — a runtime failure a well-typed program triggers.
- dup-of: —

## RT-UI-002 · CONFIRMED
- final severity: medium (completeness / silent-no-op + phantom ledger citation)
- reachability: any app using `Keyed.column`/`Keyed.row` (advertised in the
  authoring reference) with reorderable lists carrying uncontrolled inputs / focus.
- reasoning: verified both claims. `keyed_column_`/`keyed_row_` DROP the key via
  `.map(|(_, e)| e)` (`keyed.rs:21,31`) instead of attaching `ipe-key`. The
  machinery that would consume it EXISTS and is tested:
  `assign_ipe_ids_depth` → `ipe_id_key` reads the `ipe-key` attr
  (`html.rs:703,718-719`), and test `keyed_items_keep_id_across_reorder`
  (`html.rs:1313`) proves keyed items keep ipe-id identity across reorder WHEN the
  attr is present. Without it, positional ipe-ids shift on reorder → the diff
  patches the wrong elements and uncontrolled-input state / focus attaches to the
  wrong row. The module doc's "keys are a performance hint, not a behavioural
  contract — semantically correct" is FALSE for this positional-ipe-id runtime.
  The doc cites `docs/divergences-from-sky.md §B-Keyed`; that file has §B-Lazy
  ONLY — no §B-Keyed section exists. Phantom citation → ledger requirement
  violated. Medium correct.
- dup-of: —

## RT-UI-003 · CONFIRMED
- final severity: low (correctness / sink divergence)
- reachability: a `--target wasm` app patching `selected` on an `<option>` after
  the user has interacted with the `<select>`.
- reasoning: `sync_dom_property` (`wasm/mod.rs:456-480`) has arms for
  `value`/`checked`/`disabled` but NO `selected` arm, despite the doc comment
  (line 455) claiming it mirrors "value/checked/selected/disabled sync" and
  client.js syncing `el.selected`. `setAttribute("selected", …)` only sets
  `defaultSelected`, so the live selection goes stale on wasm after dirtying. Low
  correct.
- dup-of: —

## RT-UI-004 · CONFIRMED
- final severity: low (correctness / stale state after transition)
- reachability: an `Ipe.Html` checkbox using `BoolAttr("checked", …)` toggled
  true→false after user interaction.
- reasoning: the removal branch (`wasm/mod.rs:444-445`) calls only
  `remove_attribute(k)` and never resets the DOM property; same shape at
  `client.js:920`. The `checked`/`value` IDL properties don't reflect attribute
  removal once the control is dirty, so the checkbox stays visually checked after
  the server unchecks it. `Ipe.Ui.Input` is unaffected (encodes as string attr
  "true"/"false" → set-branch). Both sinks share the shape — likely inherited from
  the Go client; divergence policy favours fixing both. Low correct.
- dup-of: —

## RT-UI-005 · CONFIRMED (smell), prior fix partially landed
- final severity: low (completeness / comment-enforced invariant)
- reachability: none today — `insert_safe_attr` is the sole `Patch.attrs` inserter
  and every writer routes through the shared policy fns (verified in `diff.rs`:
  `diff_attrs` calls `insert_safe_attr` → `safe_patch_attr` for every changed/
  removed attr).
- reasoning: the behavioural fix landed (shared policy fns + tests) but the
  TYPE-level residue remains: `Patch.attrs` is a raw `pub HashMap<String,String>`
  (`diff.rs:8-20`), the `AttrAttribute` no-bespoke-renderer invariant lives in a
  comment (`element.rs:134-143`), and `AttrStyle` still multiplexes internal
  `__col`/`__row`/`__grid` markers with user CSS (`element.rs:130`). Smell, not a
  live hole — low correct.
- dup-of: —

---

## Prior-fixed spot-verification (not rubber-stamped)
- diff-path XSS gate (rt-live + rt-ui both cite as FIXED) — VERIFIED REAL:
  `diff_attrs` routes every changed AND removed attribute through
  `insert_safe_attr` → `html::safe_patch_attr` (name gate + URL-scheme
  sanitisation) at `diff.rs:176,182,194-198`. This is genuinely THE patch-path
  gate; the "fixed" call is accurate.
- SSRF default-on in production (rt-net cites as FIXED) — VERIFIED REAL:
  `ssrf_deny_private_enabled` ties the unset default to `production_from_env`
  (`ssrf.rs:36-40`); the RT-NET-001 regression is a genuine downstream consequence
  of this real fix.

Confirmed: 14 (0 crit/0 high/2 med/12 low) · Refuted: 0 · Downgraded: 1 (RT-LIVE-004 med→low) · Dup: 0
