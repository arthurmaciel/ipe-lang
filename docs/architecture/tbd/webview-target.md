# Ipe.Webview — app-entry wiring target

Status: implemented — runtime (`src/runtime/rust/src/webview.rs`), IR kernel, lower
resolution, and emission (`src/compiler/backend/rust/src/emit_webview.rs`) all landed;
a blocking example under `examples/` is the remaining gap.
Scope: wire `Webview.app { init, update, view, subscriptions, window }` through Ipê
so a desktop app opens a native window via the system webview, reusing the proven
Ipe.Live (Phase-1b) and Ipe.Tui (Phase-1c) app-entry mechanics.
Ordering of concerns: security > correctness > soundness > efficiency > completeness > readability.
Two invariants: PARSE, DON'T VALIDATE; MAKE INVALID STATES UNREPRESENTABLE. No runtime
panic from well-typed Ipê. No exit-0-then-cargo-fail.

---

## Executive summary (12 lines)

1. The Webview runtime is fully ported (`src/runtime/rust/src/webview.rs`, `webview_app`, `WebviewWindowCfg`) and feature-gated (`webview = ["wry","tao","live"]`, `src/runtime/rust/Cargo.toml`; wry 0.55 + tao 0.35).
2. The IR kernel (`KernelFn::WebviewApp`), `is_webview()`, lower callee-resolution `("Webview","app")→WebviewApp` (`lower.rs:4049`), arity-1, and `uses_webview` propagation already exist.
3. Four wiring holes remain: (a) constrain scheme, (b) lower L0107 exemption arm, (c) `emit_webview.rs`, (d) `project.rs` manifest + mod.rs injection — plus a fifth, genuinely-new element: the main-thread entry switch.
4. Cfg is a closed 5-field record: `init : () -> (Model, Cmd Msg)` (unit, not `LiveReq`), `update`, `view : Model -> Html Msg` (Html, not Element), `subscriptions`, `window`.
5. `window` is a nested closed record `{ title : String, size : (Int, Int) }`, fully concrete, type-checked by existing structural record + tuple unification with zero new machinery.
6. `view` returns `Html`; the user wraps `Ui.layout [] (...)` to convert `Element → Html`. Forgetting the wrap is a compile error (Element ≠ Html), not Go's silent blank window.
7. The bridge is in-process: initial `render` → `with_html`; DOM events → `window.ipc.postMessage` → `parse_ipc` → reused Live `HandlerIndex::resolve` → `update` → re-render → `evaluate_script("__ipeApply(...)")` full-body `innerHTML` swap. No HTTP, SSE, or session store.
8. The single genuinely-new soundness-critical deliverable (hard Phase-1d requirement): under `uses_webview`, the emitted `fn main` must call `block_on_current_thread(sky_main())` (not the thread-spawning `block_on`), applied as an anchor-asserted `replacen`-once that emits `CompilerBug` on anchor drift (fail-loud, never a silent no-op). Omission = exit-0 at compile, death on first paint; its runtime regression is the Tier-B xvfb paint test.
9. `Cmd.perform` / `Sub.every` are dropped with a one-time stderr warning in v0.1 — documented limitation, observable, never a panic; `Cmd.none` works.
10. Feature gating is structural, not a toggle: `--features webview` unconditionally needs the system webview dev libs; `ipe` preflights `pkg-config webkit2gtk-4.1 libsoup-3.0` and emits an actionable diagnostic.
11. Security posture is inherited from Ipe.Live verbatim: `render_html` escapes every text/attr node; the single `evaluate_script` slot is safe because it is a JS-*execution* context (not an HTML parse) and `json_str` (= `serde_json::to_string`) escapes the literal delimiters + control bytes — NOT because it escapes U+2028/U+2029 (it does not; see Q6). No `data-ipe-eval`/`new Function()`, local-content-only (`with_html`, never `with_url`), fail-closed IPC. No new sink.
12. Golden: three tiers — build+link (real + stub), xvfb spawn/render/no-crash, and a round-trip coverage tier that never pollutes the shipped runtime.

## Testing-golden verdict

- Tier A — build + link (BLOCKING, everywhere): `ipe` exit-0 on a Webview example, then `cargo build --features webview` links the real webkit2gtk-4.1/libsoup-3.0, AND `--no-default-features` links the stub (graceful `Err`). Forecloses exit-0-then-cargo-fail and proves graceful degradation.
- Tier B — xvfb spawn/render/no-crash (BLOCKING on this host, loud-skip on displayless CI): `timeout 20 xvfb-run -a ./app`; assert window realizes, initial view paints, process alive-at-timeout / clean SIGTERM. Never silent-green: degrade to a `smoke skipped: no display surface` log.
- Tier C — round-trip coverage (mandatory floor + optional driven smoke): a `#[cfg(feature="webview")]` runtime unit test driving `render()` → `HandlerIndex::resolve(sky_id,ev,args)` → `update()` → re-`render()` and asserting the model advances (the click-is-a-no-op guard, deterministic, no display). Optionally, a compile-time-gated (`#[cfg(feature="webview_smoke")]`, NOT env-var-gated) driven synthetic-IPC smoke through the real event loop under xvfb — never a production-runtime branch.

## Buildable / testable on THIS Linux host?

Yes. Verified present: webkit2gtk-4.1 2.50.4, libsoup-3.0 3.0.7, `xvfb-run` at `/usr/bin/xvfb-run`. Tiers A + B + C are all runnable here. The runtime already builds the Linux GTK path (`build_gtk`, Wayland + X11), so v0.1 ships Linux-now — a sanctioned divergence from the Go runtime's macOS-only v0.1 note (see OPEN DECISION 4). Interactive click-driving through the real window is NOT provable (wry 0.55 exposes no CDP/WebDriver hook, and the IPC channel is inbound-only from the app's own bridge JS); Tier C covers that class instead.

---

## Q1 — closed-record cfg + nested `window` scheme + constrain

DECISION. `Webview.app` takes a single closed 5-field record; all five fields required:

```
Webview.app :
  { init          : () -> (Model, Cmd Msg)
  , update        : Msg -> Model -> (Model, Cmd Msg)
  , view          : Model -> Html Msg
  , subscriptions : Model -> Sub Msg
  , window        : { title : String, size : (Int, Int) }
  } -> Task Error ()
```

Shared type vars `var(0) = Model`, `var(1) = Msg`, exactly the Live/Tui idiom. Distinctions
that MUST be honored (each is a soundness trap if copied wrong from a sibling):

- `init` takes `()` — use `Ty::Unit`, NOT an empty `Ty::Tuple` (empty tuple prints `()` but won't unify with the `Ty::Unit` a `() -> …` annotation produces). Matches runtime bound `FInit: Fn(()) -> (Model, SkyCmd<Msg>)`. This is Tui-shaped, not Live's `LiveReq`.
- `view` returns `Html Msg` via `self.builtins.html_con`, NOT `element`. Matches runtime bound `FView: Fn(Model) -> Html<Msg>` (`webview.rs:199`). This is Live-shaped, not Tui's `Element`.
- `window` is a nested closed `Ty::Record { title: String, size: tuple2(Int, Int) }`, fully concrete (no Model/Msg vars).

Rationale: the cfg is a hybrid of Tui (`init` unit) and Live (`view` Html) with a new fifth
field (`window`, a nested record, not Tui's `onKey` function).

New interned `Builtins` symbols: `webview_f_window`, `webview_f_title`, `webview_f_size`.
Reuse existing `live_f_init/update/view/subscriptions`, `int`, `string`, `cmd`, `sub`,
`html_con`, `tuple2`.

Constrain arm (add to the `(qualifier, name)` table in `constrain.rs`, beside the Tui arm):

```
(Some("Webview"), Some("app")) => {
    let init_ret  = tuple2(var(0), cmd(var(1)));                       // (Model, Cmd Msg)
    let init_ty   = fun(Ty::Unit, init_ret.clone());                  // () -> (Model, Cmd Msg)
    let update_ty = fun(var(1), fun(var(0), init_ret));               // Msg -> Model -> (Model, Cmd Msg)
    let view_ty   = fun(var(0), html(var(1)));                        // Model -> Html Msg
    let subs_ty   = fun(var(0), sub(var(1)));                         // Model -> Sub Msg
    let window_ty = Ty::Record{ title: string, size: tuple2(int, int) }; // closed, concrete
    let cfg_rec   = Ty::Record{ init, update, view, subscriptions, window };
    fun(cfg_rec, task_unit)
}
```

Nested-record type-check — zero new machinery. The unifier already does structural closed-record
unification (`unify.rs:259`: field-name sets must match, then field-by-field) and tuple
unification (`unify.rs:247`: same arity, elementwise recurse). `window` is fully concrete and does
not participate in Model/Msg unification; the cfg-record literal unifies field-by-field, recursing
into `window` and the `size` tuple2 automatically. A missing/extra/mistyped window field, or a
non-2-tuple `size`, is a clean IPE-T structural mismatch at the call site.

FIREWALL (the exit-0-then-cargo-fail gate). The constrain qualifier set `(Some("Webview"),
Some("app"))` MUST byte-match lower's resolved set `("Webview","app") → WebviewApp` (`lower.rs:4049`).
Any mismatch leaves the kernel resolved-but-unschemed → the `_ => Ty::Var(u32::MAX)` fallback
(`constrain.rs`), which type-checks anything and then fails in cargo. This is the exact class the
two fundamental rules forbid; a resolved-but-unschemed Webview kernel is a compile error, not a
`Ty::Var` fallback.

The byte-match cannot be proven by inspection or by a unit test — it is only PROVEN by a Tier-A
build+link running against a REAL Webview example: `ipe` exit-0 followed by `cargo build --features
webview` linking clean. A constrain-arm typo (a mistyped qualifier, a wrong `var()` index, a
dropped field) still type-checks via the `Ty::Var(u32::MAX)` catch-all and reopens the
exit-0-then-cargo-fail class SILENTLY — there is no earlier signal. Therefore the Webview example
that drives Tier A MUST be added to the BLOCKING sweep the moment the constrain+lower arm lands, in
the same change. Shipping the arm without its example in the blocking sweep leaves the firewall
unguarded and the `Ty::Var(u32::MAX)` exit-0-then-cargo-fail regression class silently reopenable.

Required-fields rule: closed record ⇒ every field is required; a missing field is a clean IPE-T
error (no row var to absorb it), an extra field is likewise rejected.

Non-literal `window` — rejected at LOWER (parse, don't validate). The lower gate must fail-closed on
exactly TWO structural conditions, and no more:

1. `window` is an inline `Expr::Record` literal (not a var, not a builder-pipe, not a let-bound
   name) — emit field-extracts `title` and `size` by name.
2. `size` is an inline 2-tuple literal (`Expr::Tuple` of length 2) — emit destructures it to `(w, h)`
   for `WebviewWindowCfg { size: (w, h) }`.

`title` need NOT be a literal: it is a plain `String`-typed slot, so ANY `String`-typed expression
(a `let`-bound name, a `String.append`, a function call) lowers fine — emit passes the lowered
expression straight into the `title:` field. The gate predicate must therefore be precise on both
edges: NOT over-broad (it must not reject a computed `title`, e.g. `title = "Ipê — " ++ appName`),
and NOT under-broad (a non-literal `size` — `size = dims` or `size = computeSize model` — must be
caught HERE, at lower, not allowed to reach emit's tuple-destructure where it would surface as a
deep `CompilerBug`). Emit the reject with a clean user-facing diagnostic
("`window.size` must be an inline `(Int, Int)` tuple literal") at the earliest boundary that can name
it clearly. Emit keeps its `require Expr::Record` (and `require Expr::Tuple` on `size`) assertion as
defense-in-depth only, because lower has already foreclosed the case. Same discipline applies to the
whole cfg record (inline-literal required; a let-bound cfg stays fail-closed via
`reject_function_through_type_var`).

---

## Q2 — view type + Element→Html wrapping

DECISION. The scheme pins `view : Model -> Html Msg`. The user writes
`view model = Ui.layout [] ( Ui.column … )`; `Ui.layout : List Attr -> Element -> Html` performs the
`Element → Html` conversion. A bare `Ui.column` body has type `Element Msg`, does not unify with
`Html Msg`, and is a clean compile error — the "blank window" failure mode is caught at type-check,
strictly better than Go's silent blank window.

Rationale: confirmed against the emitted renderer. `render()` (`webview.rs:157-174`) takes
`FView: Fn(Model) -> Html<Msg>` and calls `assign_sky_ids` + `apply_style_injections` +
`build_index` + `render_html` — byte-identical to the Live pipeline. The same `view` function paints
across Live (web), Tui (terminal), and Webview (desktop).

Emit passes `view` (and the other three function fields) by raw `fn` item name. `emit_webview_fn`
mirrors `emit_tui_fn` / `emit_live_fn`: `Expr::FuncValue → callee_name` (a named `fn` satisfies
`Fn(Model)->Html<Msg> + Send + 'static` via the blanket impl), falling back to `emit_expr_at` for
lambdas. Note the runtime bounds are `Send`-only (no `Sync`); the raw-name-vs-box decision is
unaffected.

---

## Q3 — the wry/tao in-process TEA bridge

Runtime entry (`src/runtime/rust/src/webview.rs`, real + `!webview` stub):

```rust
pub fn webview_app<Model, Msg, E, FInit, FUpdate, FView, FSubs>(
    init: FInit, update: FUpdate, view: FView,
    _subscriptions: FSubs, window: WebviewWindowCfg,
) -> SkyTask<E, ()>
where E: Send + From<String> + 'static,
      Model: Clone + Send + 'static, Msg: Clone + Send + 'static,
      FInit:   Fn(()) -> (Model, SkyCmd<Msg>) + Send + 'static,
      FUpdate: Fn(Msg, Model) -> (Model, SkyCmd<Msg>) + Send + 'static,
      FView:   Fn(Model) -> Html<Msg> + Send + 'static,
      FSubs:   Fn(Model) -> SkySub<Msg> + Send + 'static,
```

`WebviewWindowCfg { title: String, size: (i64, i64) }` (`webview.rs:38`).

DECISION — the loop (all in-process; no HTTP, no SSE, no session store):

1. Build `EventLoopBuilder::<UserEvent>::with_user_event().build()` on the true main thread; a `WindowBuilder` with `title` + `LogicalSize(w, h)`.
2. `init(())` → `(model, cmd0)`; initial view → HTML: `render(&view, &model)` → `(body0, index)`; wrap in `<!doctype html>…<body>{body0}</body><script>{BRIDGE_JS}</script>`; `WebViewBuilder::new().with_html(html)` loads it directly — local content only, no remote URL. Per-OS webview build: `build(&win)` off Linux, `build_gtk(default_vbox | gtk_window)` on Linux (`webview.rs:266`).
3. Msg dispatch (JS→Rust): `BRIDGE_JS` installs delegated `document` listeners on `[ipe-id]` elements; each event posts `{skyId, event, args}` over `window.ipc.postMessage`. The `with_ipc_handler` closure forwards the body into the loop as `UserEvent::Ipc` via the event-loop proxy. On `Event::UserEvent(Ipc(body))`: `parse_ipc` (serde → `Option`) → reused Live `HandlerIndex::resolve(sky_id, ev, args)` → `Option<Msg>`.
4. update → re-render → DOM patch (Rust→JS): `update(msg, model)` → next model → `render` again → `webview.evaluate_script("window.__ipeApply(<json_str(nbody)>)")`, which sets `document.body.innerHTML = nbody`. This is a full-body `innerHTML` swap, NOT a VNode diff. Event delegation means the swap needs no re-bind. `__ipeApply` wraps the assignment in the pinned focus/caret save/restore (OPEN DECISION 1 → option (a)): capture `document.activeElement`'s `ipe-id` + `value` + selection before the swap, re-apply them by PROPERTY assignment after — never by concatenation into the HTML string.
5. `WindowEvent::CloseRequested → ControlFlow::Exit`. `event_loop.run` is synchronous and diverging, so no `.await` crosses the `!Send` webview and the `SkyTask` future stays `Send`.

Rationale on the innerHTML swap. AGENTS.md/spec text mentions `diffTrees`; the ported runtime
deliberately uses a full-body `innerHTML` replace in v0.1 (`webview.rs:93,110`). This is
security-positive: every patch re-flows through `render_html` (the single audited escaping sink),
so there is structurally no partial-patch path that could bypass escaping. The diff patcher is a
later optimization, not a correctness gap.

Cmd/Sub mapping (v0.1). The synchronous `event_loop.run` does not pump tokio, so `Cmd.perform` /
`Sub.every` cannot fire. A non-`Cmd.none` return is dropped with a one-time stderr warning
(`warn_dropped_cmd_if_real`, `webview.rs:125`); `_subscriptions` is accepted but unused
(`webview.rs:190`). The constrain scheme still types `init`/`update` returns as `(Model, Cmd Msg)`
and requires the `subscriptions` field, so app code stays portable across Live/Tui/Webview
unchanged — only the runtime execution of the effect is deferred. This is a documented, observable
v0.1 limitation, not a soundness hole and not a panic.

THE MAIN-THREAD ENTRY SWITCH — HARD Phase-1d REQUIREMENT, the single genuinely-new
soundness-critical deliverable of this whole wiring (no Live/Tui analog; every other hole is a clone
of a proven sibling mechanic). The golden epilogue emits `block_on(sky_main())`
(`tests/golden/basics/main.rs:287`), which drives the future on a `std::thread::spawn`ed OS thread
(`task.rs:14`). This is fatal for Webview: tao/winit's `EventLoop` + Cocoa's `NSApplication` require
the event loop on the process's TRUE main thread (hard on macOS, expected on Windows, cleanest on
Linux GTK). The runtime provides `block_on_current_thread` (`task.rs:47`, a current-thread tokio
runtime, no spawn) for exactly this. Under `uses_webview`, the emitted `fn main` MUST call
`block_on_current_thread(sky_main())`. Both return `SkyResult<E,()>`, so the surrounding `match` is
unchanged.

The switch MUST be implemented as an anchor-asserted `replacen(anchor, replacement, 1)` — replace
the single `block_on(sky_main())` call site exactly ONCE — that FAILS LOUD (emits a `CompilerBug`
diagnostic and aborts codegen) if the golden anchor string is not found, i.e. if the epilogue
template drifts. Never a silent no-op: a `replacen` that matches zero sites must be a hard compiler
error, because a missed switch produces a well-typed app that compiles cleanly (exit 0) and then
dies at runtime on first paint — precisely the exit-0-then-death class the two fundamental rules
forbid. Its runtime regression is the Tier-B xvfb paint test: `timeout 20 xvfb-run -a ./app` must
realize the window and paint the initial view; a reverted/missed entry switch turns that test red
(off-main-thread event loop → crash before paint), which is the only test in the suite that
exercises the switch end-to-end.

---

## Q4 — cross-platform + feature gating + native-lib requirements

DECISION — `project.rs` additions (four, mirroring the Live/Tui injectors, fail-loud on every
anchor):

1. `webview_cargo_toml(base)` — promote `"webview"` into the emitted manifest's `default = [...]` list. Because `webview = ["wry","tao","live"]` transitively pulls `live`, this injector composes ON TOP of `live_cargo_toml` (and `server_cargo_toml`), which are guaranteed to run because `uses_webview ⇒ uses_live`. Run `webview_cargo_toml` LAST, after the live/server surgery, matching the tui_cargo_toml "called after server/live" contract. Idempotent via `contains`/`replacen`-once. See OPEN DECISION 2 for the dep-emission mechanism.
2. `RUNTIME_MOD_RS_WEBVIEW_APPEND` = `#[cfg(feature="webview")] pub mod webview;\n#[cfg(feature="webview")] pub use webview::{webview_app, WebviewWindowCfg};`, pushed when `ctx.uses_webview`. The vendored `webview.rs` is always physically present (`copy_dir` copies the whole runtime tree); this `pub mod` declaration is what compiles it. Gating on `uses_webview` keeps non-webview builds from compiling `webview.rs` at all.
3. Ensure `uses_webview` implies `uses_live` (per `ir.rs:104`) at the flag-computation site, so the live module + its serde/tokio-signal deps ship whenever webview is used.
4. The main-thread entry switch (Q3) — the HARD Phase-1d requirement, the single genuinely-new soundness-critical deliverable. A `uses_webview`-gated, anchor-asserted `replacen(anchor, replacement, 1)` of the single `block_on(sky_main())` call site → `block_on_current_thread(sky_main())`, that emits a `CompilerBug` and aborts (fail-loud, never a silent zero-match no-op) if the golden anchor drifts. Runtime regression: the Tier-B xvfb paint test (a missed switch → off-main-thread loop → crash before first paint → red Tier B).

The cgo-detect analog. Go flips to `CGO_ENABLED=1` when `rt.Webview_app` is present. The Rust
analog is STRUCTURAL, not a toggle: `--features webview` (via the emitted manifest's `default` list)
unconditionally links the system webview libraries; there is no static-binary fallback for a native
window. `ctx.uses_webview` ⇒ manifest promotes `webview` to `default` ⇒ `cargo build` links
webkit2gtk/libsoup on the first attempt. A machine without the libs fails at cargo link. The
feature-OFF stub (`webview.rs:47`) compiles cleanly and returns a graceful `Err` at call time; the
Ipê build emits feature-ON whenever `uses_webview`.

Dev-experience preflight. When `ctx.uses_webview`, `ipe` runs `pkg-config --exists webkit2gtk-4.1
libsoup-3.0` on Linux and emits an actionable diagnostic naming the apt/brew package, rather than
letting cargo vomit a raw linker error.

Platform scope. The ported runtime already builds the Linux GTK path (`build_gtk`, Wayland + X11)
with a uniform main-thread event loop across OSes (no `with_any_thread` branch). v0.1 ships
Linux-now (that is where the sweep runs), macOS/Windows following from the same loop. This is a
sanctioned divergence from the Go runtime's macOS-only v0.1 note (OPEN DECISION 4). Native libs on
this host are verified present at the versions wry 0.55 expects.

---

## Q5 — the honest headless-testing golden

DECISION — three tiers (verdict restated from the top of this doc):

- Tier A — build + link, BLOCKING everywhere. `ipe` compiles a `webview-smoke` example clean (exit-0); `cargo build --features webview` links the real webkit2gtk-4.1/libsoup-3.0; `cargo build --no-default-features` links the stub (graceful `Err`). Exercises the whole chain (constrain → L0107 exemption → lower → emit_webview → manifest → native link → main-thread entry) and proves graceful degradation.
- Tier B — xvfb spawn/render/no-crash, BLOCKING on this (verified-capable) host, loud-skip on displayless CI. `timeout 20 xvfb-run -a ./app`; assert the window realizes, the initial view paints, and the process is alive at the timeout / exits cleanly on SIGTERM. Never silent-green: a display-surface failure degrades to a clear skip log.
- Tier C — round-trip coverage without a production seam. A `#[cfg(feature="webview")]` runtime unit test drives `render()` → `HandlerIndex::resolve(sky_id, ev, args)` → `update()` → re-`render()` and asserts the model advances (counter 0→1). This is the click-is-a-no-op guard (AGENTS.md §9: `--build-only` cannot catch it), deterministic, no display, no production pollution. Optionally, a compile-time-gated (`#[cfg(feature="webview_smoke")]`, NOT env-var-gated) driven synthetic-IPC smoke through the real event loop under xvfb (OPEN DECISION 3).

Rationale on rejecting an env-gated seam. Injecting a synthetic `UserEvent::Ipc` through the real
event loop via an `IPE_WEBVIEW_SMOKE` branch inside the shipped `webview_app` puts test scaffolding
in the production hot path; a deployed binary that changes behavior on a stray env var is a
security/correctness smell that violates MAKE INVALID STATES UNREPRESENTABLE. The round-trip is the
composition of three near-pure pieces (`render`, `resolve`, `update`); test the pieces the loop
composes (Tier C floor), or gate the driven smoke at COMPILE time — never widen the production
surface. Interactive click-driving through the real window is honestly infeasible in v0.1 (no
CDP/WebDriver in wry 0.55; the IPC channel is inbound-only). Do not claim end-to-end click
simulation; that would be a dishonest golden.

---

## Q6 — security posture

DECISION. The webview loads only app-rendered, local HTML — the entire attack surface is
app-controlled content, defended by reusing Ipe.Live's sanitizer unchanged.

- render_html sanitization parity. The body comes exclusively from `render_html`, which HTML-escapes every text + attribute node (INVARIANT comment, `webview.rs:106`). The `document.body.innerHTML = html` in `__ipeApply` is therefore not an XSS sink for user data. Any FUTURE raw-HTML renderer node becomes the XSS boundary and must be audited in `render_html`, not here — the Webview port adds ZERO new sink.
- No eval of app content. `BRIDGE_JS` is a fixed, audited constant with no `data-ipe-eval` / `new Function()`. The only outbound `evaluate_script` call is `window.__ipeApply(<json_str(body)>)`, where `json_str` = `serde_json::to_string(body)`. The re-render payload cannot break out of the JS string literal — but note the precise reason, because the obvious one is WRONG. Stock `serde_json::to_string` does NOT escape U+2028/U+2029 (serde emits those raw, unlike Go's `encoding/json`); do not claim a U+2028 escaping guarantee that the encoder does not provide. The no-breakout conclusion holds for two independent reasons: (1) the sink is a JS-*execution* context (`evaluate_script("window.__ipeApply(...)")`), NOT an HTML-parse context, so `</script>` / `<` breakouts are structurally inapplicable — there is no HTML parser between the string and the engine; and (2) on wry 0.55's ES2019+ engines (WebKitGTK 2.50 / modern WKWebView / WebView2), U+2028/U+2029 are legal string-literal characters and do not terminate the literal. serde's `\"` / `\\` / control-char escaping closes the remaining literal-delimiter and control-byte breakout. WARNING to future maintainers: this reasoning is sink-specific. A different sink — e.g. splicing `json_str(body)` into an inline `<script>` HTML block that goes through an HTML parser — would face BOTH the `</script>` breakout AND (on a legacy/HTML-context engine) the U+2028 line-terminator hazard, neither of which `json_str` guards against. Do NOT route a new sink through `json_str` trusting a U+2028 guarantee that does not exist.
- Local content only. `with_html(...)` loads inline app HTML in-process; there is no `.with_url`, no remote navigation, no network fetch.
- Fail-closed IPC. `parse_ipc` is `serde_json::from_str(...).ok()?` — a malformed/hostile IPC body yields `None` (no panic, no dispatch). `HandlerIndex::resolve` returns `None` for an unknown ipe-id/event (no fabricated Msg). The only IPC producer is the fixed `BRIDGE_JS` in a no-remote-load webview, so there is no external injector; the parse path is fail-closed by construction. IPC-arg indexing uses `args.get(i).cloned().unwrap_or_default()`-style access — no `.unwrap`, no index-panic.
- No new secrets surface. No session store, no cookies, no HTTP server — the multi-tenant/console/auth attack surfaces of Ipe.Live do not exist for Webview.
- No panic from well-typed Ipê. Every fallible step routes through `Err` (window build `:235`, webview build `:271`, stub `:72`); `event_loop.run` diverges; `json_str`'s unreachable Err arm has a total fallback. On a machine without libs (feature-ON, libs-missing) the failure is at cargo link, not at runtime — consistent with "if it compiles, it works". Preserve this by never adding an `.unwrap()` in the emitted call path.

---

## Reused Phase-1b / 1c app-entry mechanics (the base this extends)

1. L0107 exemption (`lower.rs`, `lower_call` intercept). Add a `WebviewApp` arm beside the `LiveApp` / `TuiApp|TuiProgram` arms:

```
Callee::Kernel(KernelFn::WebviewApp) if args.len() == 1 => {
    let lowered_cfg = match &arg0.value {
        canon::Expr_::Record(fields) => self.lower_app_cfg_record(fields)?, // exemption: omits reject_function_valued_field
        _ => self.lower_expr(arg0)?,                                        // let-bound → fail-closed
    };
    return Ok(Expr::Call { callee: peek, args: vec![lowered_cfg] });
}
```

A direct record-literal cfg routes through `lower_app_cfg_record` (function-typed fields do not trip
IPE-L0107); the nested `window` record (no function fields) passes the per-field `reject_function_
valued_field` cleanly. A let-bound / builder-piped cfg stays fail-closed. Add the inline-`window`
gate here (Q1).

2. Closed-record constrain scheme (`constrain.rs`) — Q1. Qualifier set == lower resolved set.
3. `emit_webview.rs` (modelled on `emit_tui.rs`) — field-extract the four function fields via `emit_webview_fn`; extract `title`/`size` from the inline `window` record and construct the NOMINAL `ipe_runtime::webview::WebviewWindowCfg { title, size: (w, h) }` by name. Do NOT route `window` through the generic anonymous-record emitter — that produces a distinct `Rec…`-named struct and cargo-fails. Dispatch from `emit_expr.rs` on `k.is_webview()`.
4. `project.rs` manifest injection — Q4.

Emitted call shape:

```rust
ipe_runtime::webview::webview_app(
    <init>, <update>, <view>, <subscriptions>,
    ipe_runtime::webview::WebviewWindowCfg { title: <title>, size: (<w>, <h>) },
)
```

---

## Trap ledger (every foreclosed failure mode)

| Trap | Where it bites | Foreclosure |
|---|---|---|
| exit-0-then-cargo-fail (unschemed kernel → `Ty::Var(u32::MAX)`) | constrain `_` fallback | Add `(Some("Webview"),Some("app"))` arm; qualifier set byte-equal to lower's `("Webview","app")`. Proven ONLY by Tier-A build+link on a real Webview example — that example MUST enter the blocking sweep the moment the arm lands (a constrain-arm typo silently reopens the class otherwise). |
| L0107 false-reject (function-valued cfg fields) | lower `Record` arm | Add `WebviewApp` intercept arm → `lower_app_cfg_record`; let-bound cfg fail-closed. |
| Element vs Html view | view type | Scheme uses `html_con`; forgotten `Ui.layout` wrap is a compile error, not a blank window. |
| `Ty::Unit` vs empty `Ty::Tuple` in `init` | init arg | Use `Ty::Unit`. |
| nested-record / tuple unify | `window {title,size:(Int,Int)}` | None needed — `unify.rs:259` (record) + `:247` (tuple) already recurse; `window` fully concrete. |
| main-thread violation (`block_on` spawns a thread) | golden `fn main` | `uses_webview`-gated `block_on_current_thread` switch: anchor-asserted `replacen`-once, `CompilerBug` on zero-match drift (fail-loud). The single genuinely-new soundness-critical deliverable; Tier-B xvfb paint test is its runtime regression. |
| non-literal `window` / non-literal `size` → emit can't field-extract / can't destructure | lower/emit | Lower gate fails-closed on BOTH `window` inline `Expr::Record` AND `size` inline 2-tuple literal; `title` stays any `String`-typed expr. Precise predicate: not over-broad (computed `title` allowed) nor under-broad (non-literal `size` caught at lower, never reaching emit's `CompilerBug`). Emit keeps `require Expr::Record` + `require Expr::Tuple` as defense-in-depth. |
| generic record emitter for `window` → distinct Rust type | emit_webview | Construct nominal `WebviewWindowCfg` by name. |
| manifest compose order / duplicate `live` | project.rs | `uses_webview ⇒ uses_live` runs server+live first; `webview_cargo_toml` last; fail-loud anchors. |
| missing native libs | cargo link | `ipe` pkg-config preflight with actionable diagnostic; stub path links + `Err`. |
| Cmd/Sub silently dropped | v0.1 sync loop | `warn_dropped_cmd_if_real` (one-time stderr); typed uniformly; no panic. |
| click-is-a-no-op regression | headless test | Tier-C runtime round-trip unit test (build-only can't catch it). |
| new XSS / eval hole | DOM patch | Reuse `render_html` (escapes all) + `__ipeApply` sink is a JS-execution context (not HTML-parse) with `json_str` literal-escaping — NOT a U+2028 guarantee (serde does not escape U+2028/U+2029; see Q6); no `data-ipe-eval`; local-content-only; fail-closed IPC. |

---

## OPEN DECISIONS

1. Input / focus preservation parity (v0.1) — PINNED to option (a). Ipe.Live hardens against re-render destroying uncontrolled input state (focus-preserving DOM replacer; password fields carry no `value`; open-`<select>` defence). A naive full-body `innerHTML = nbody` blows focus, caret position, in-flight input text, and open dropdowns on every Msg. RESOLUTION (a): add a small, bounded save/restore of `document.activeElement`'s value + selection (`selectionStart`/`selectionEnd`) around the innerHTML assignment inside the fixed `BRIDGE_JS` constant, keyed by the element's `ipe-id`. HARD CONSTRAINT: the restore MUST use property assignment (`el.value = savedValue; el.selectionStart = …`), NEVER concatenation of the saved value into an HTML string. Property assignment sets a live DOM node's property directly — it never re-enters the HTML parser and never re-enters `eval`, so it opens ZERO new injection path (in contrast, splicing the saved value back into `innerHTML` would resurrect an XSS sink). The saved value stays entirely client-side and is never round-tripped to Rust. IMPORTANT — the doc must NOT claim Ipe.Live input-preservation parity: this save/restore covers the focused element's value + caret only, not Live's full uncontrolled-field matrix (every uncontrolled INPUT/TEXTAREA/SELECT, password-field-carries-no-`value`, open-`<select>` defence). It is a UX/correctness gap, not a security gap — the in-flight input remains client-side and is therefore never a secret leak. State the residual gap explicitly rather than implying parity.

2. Manifest dependency-emission mechanism. Whether `webview_cargo_toml` DIRECT-INJECTS `wry = "0.55"` + `tao = "0.35"` into the emitted project manifest, or relies on FEATURE-FORWARDING from the vendored runtime's `webview = ["wry","tao","live"]` mapping, depends on whether the vendored runtime is an in-project module (single Cargo.toml → direct-inject) or a path sub-crate (features forward). RESOLVE by reading `project.rs` and mirroring `live_cargo_toml`'s proven mechanism exactly — do not invent a new one. Tier-A build+link catches any drift.

3. Driven synthetic-IPC smoke (Tier C, optional half). Whether to build the compile-time-gated (`#[cfg(feature="webview_smoke")]`) driven-IPC smoke through the real event loop under xvfb for v0.1, or ship with only the pure-function round-trip unit test as the floor. The env-var-gated production-runtime seam is REJECTED regardless. Recommendation: unit-test floor mandatory; compile-time driven smoke as a fast-follow if the unit test proves insufficient against a real regression.

4. AGENTS.md doc consistency (REQUIRED follow-up, out of scope for THIS doc-only edit). The AGENTS.md / user-doc line "Ipe.Webview v0.1 (desktop, macOS)" and "macOS in v0.1; Linux/Windows in v0.2" contradicts the Rust runtime's shipping Linux-now GTK path (this spec's Q4 platform scope: Linux-now, cross-platform by construction). That AGENTS.md text MUST be corrected in the SAME change that ships Webview wiring — not before (the arm doesn't exist yet) and not after (a shipped feature with a doc that contradicts it is a stale-claim trap for the next AI-authored app). NOTE: do not edit AGENTS.md as part of the present doc-only pass; this is recorded here purely as the tracked follow-up to bundle with the implementation change.

---

## Load-bearing files

- Runtime: `src/runtime/rust/src/webview.rs` (`webview_app`, `WebviewWindowCfg`, `BRIDGE_JS` / `__ipeApply`, `render`, `warn_dropped_cmd_if_real`); `src/runtime/rust/src/task.rs` (`block_on`, `block_on_current_thread`); `src/runtime/rust/Cargo.toml` (`webview` feature).
- Wiring sites: `src/compiler/types/src/constrain.rs` (new arm + interned symbols); `src/compiler/lower/src/lower.rs` (`L0107` intercept + inline-window gate + `uses_webview⇒uses_live`; callee resolve); `src/compiler/backend/rust/src/emit_webview.rs` + `emit_expr.rs` (dispatch); `src/compiler/backend/rust/src/project.rs` (`webview_cargo_toml` + `mod.rs` append + entry switch); `src/compiler/backend/rust/src/preamble.rs` (entry rewrite).
- Reference templates: `src/compiler/backend/rust/src/emit_tui.rs`, `emit_live.rs`, and the `live_cargo_toml` / `tui_cargo_toml` injectors in `project.rs`.
