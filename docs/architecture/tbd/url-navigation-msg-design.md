# URL navigation as a Msg — `onNavigate` + `page`-field demotion (#155)

> Backlog item #155 (Post-completion): "Route URL changes to a Msg (Elm
> `Browser.application` parity), demote the magic `page` field to sugar."
> Spec+plan written 2026-07-10. Design-only; no code has changed.
>
> **One-line decision:** add the reference's optional
> `onNavigate : page -> msg` cfg field to `Web.app` (row-poly absorbed,
> deferred-check typed), have the runtime dispatch the resulting Msg
> through `update` on every URL-driven route change, and **redefine** the
> existing magic `page`-field mutation as the *desugaring of the absent
> field* — i.e. `onNavigate` missing ≡ an implicit
> `\p -> __SetPage p` whose implicit update arm is
> `({ model | page = p }, Cmd.none)`. Absent-field apps stay
> byte-identical; present-field apps own navigation in `update`.

## Problem statement

Today the Rust port's routed Live app applies a URL change entirely
runtime-internally: match the path against `routes`, build the `Page`
value, and mutate `Model.page` via the generated `set_page` closure —
the app's `update` never sees a navigation event.

- Runtime application site: `src/runtime/rust/src/live/mod.rs:1173-1174`
  — `route_resolver: Arc<dyn Fn(Model, &str) -> Model>` applies
  `(set_page)(route::match_routes(&routes, &not_found, path), m)` directly.
- Client paths that trigger it: `src/runtime/rust/src/live/client.js`
  — `sky-nav` click handler (≈1182–1206), `popstate` listener
  (≈1210–1306), `data-sky-path` history sync (≈1005–1012).
- The "magic" detection: `src/compiler/types/src/lib.rs:916-920`
  (`resolve_routed_live_checks`) inspects the settled Model for a field
  literally named `page`; `src/compiler/backend/rust/src/emit_live.rs:361-417`
  (`emit_live_app_inner`) branches on it and synthesises `set_page`.

Consequences: an app cannot react to navigation (fetch data for the new
page, deny navigation, record analytics) without polling; and the `page`
field is an undocumented magic name with no in-language account of *why*
it changes. Elm solved this shape in `Browser.application`: **every** URL
change becomes a Msg (`onUrlChange : Url -> msg`), and the model's page
field is ordinary state the app updates itself.

The reference already grew the same capability: Ipê v0.16.7+ has an
optional `onNavigate : Page -> Msg` cfg field
(`upstream:runtime-go/rt/live.go:2727-2740`, dispatch sites at 2949, 3073,
3099) — when set, the framework dispatches the Msg through `update`
after every URL-driven route change; when nil, routes apply silently
(pre-v0.16.7 behaviour). So the *mechanism* half of #155 is
parity-restoring, not a divergence; only the *reframing* ("magic page is
sugar over onNavigate") is Ipê editorial.

## Decision

### D1 — Surface: optional `onNavigate : page -> msg` on `Web.app` cfg

Same field name, position, and semantics as the reference. It rides the
existing open row tail (`RowTail::Open(3)`) of the `Web.app` cfg scheme
(`src/compiler/types/src/constrain.rs:4028-4055`), so no required-field
change and no breakage for existing apps.

Typing is enforced by a **deferred post-solve check**, the same pattern
as `RoutedLiveCheck` (`constrain.rs:1148-1155`) and `RouteWitnessCheck`
(`constrain.rs:1187-1196`): if the settled cfg row carries an
`onNavigate` field, unify its type with `var(2) -> var(1)`
(page → Msg). Mismatch → IPE-T0001 at the field's span. This keeps the
required-field scheme untouched and makes the optional field fully typed
(no silent `any`).

### D2 — Semantics: Msg dispatch replaces direct mutation when present

When `onNavigate` is present, on every URL-driven route change (initial
mount, `sky-nav` click, popstate Back/Forward) the runtime:

1. matches the path exactly as today (`route::match_routes`,
   `src/runtime/rust/src/live/route.rs`);
2. does **not** call `set_page`;
3. calls `onNavigate(matchedPage)` and dispatches the resulting Msg
   through `update`, exactly like any other event (same Cmd handling,
   same SSE patch cycle).

The app is now the owner of `model.page` — it can accept the navigation
(`{ model | page = p }`), enrich it (`, Cmd.perform (fetchFor p) Loaded`),
or refuse it. This is the Elm `onUrlChange` contract, delivered at the
`Page` level rather than the raw-`Url` level (see Non-goals).

Reference-parity note: the Go runtime dispatches onNavigate *after*
applying the route (mutate-then-notify). We deliberately dispatch
*instead of* mutating (notify-only). Rationale: mutate-then-notify makes
`update` observe a model whose `page` already moved — the app cannot
refuse navigation and the "who owns page?" question keeps two answers.
Notify-only is the Elm contract and the entire point of the demotion.
This is a behavioural divergence from the reference for
`onNavigate`-present apps only — record it in
`docs/divergences-from-sky.md` (`divergence:` tag) with this rationale.
Absent-field apps remain reference-identical.

### D3 — Demotion: the magic `page` field becomes specified sugar

The pre-existing behaviour (no `onNavigate`) is redefined — with **zero
behavioural change** — as the default desugaring:

```
onNavigate = \p -> __SetPage p
-- implicit update arm:
__SetPage p -> ( { model | page = p }, Cmd.none )
```

Concretely the runtime keeps its `set_page` fast path when the field is
absent (no synthetic Msg value is actually constructed — the desugaring
is normative, not operational), but docs, the routed-live explain pages,
and `routed-live-app-design.md` are updated to present `page` as sugar
over `onNavigate`, not as magic. IPE-L0124 ("routes declared but no
`page` field", `src/compiler/types/src/lib.rs:947-950`) is extended: an
app with `routes` but neither a `page` field nor `onNavigate` warns; an
app with `onNavigate` and no `page` field is **legal** (the app may
store its route state under any name — this is the demotion made real).

### Non-goals (recorded, deliberately out of scope for #155)

- **`onUrlRequest` / `UrlRequest (Internal|External)`** — link-click
  interception stays declarative via the `sky-nav` attribute. Filing a
  typed `UrlRequest` Msg requires a client→server event for *every*
  link click; defer until a concrete need shows up.
- **`Nav.pushUrl` / `Key`** — programmatic navigation already works via
  the `data-sky-path` sentinel; an imperative typed API
  (`Nav.push : String -> Cmd msg`) is a natural follow-up but a separate
  surface addition (file under C.4 Elm-core coverage if wanted).
- **Raw-`Url` payload** — Elm's `onUrlChange` receives a `Url`; we
  deliver the already-matched `page` value. The route table is the
  parse-don't-validate boundary; handing apps a raw URL string would
  reintroduce stringly-typed routing. If query-string access is needed,
  extend the route matcher, don't widen the payload.

### Alternatives considered and rejected

1. **Mutate-then-notify (exact Go semantics).** Rejected: `update` sees
   a post-mutation model; app cannot veto; `page` stays half-magic.
2. **Require `onNavigate` on every routed app (remove the sugar).**
   Rejected: breaks every existing routed example and the upstream
   corpus for no soundness gain; the sugar is well-specified now.
3. **Full `Browser.application` (Url + Key + onUrlRequest).** Rejected
   for this item: large client-protocol surface; #155's value is the
   Msg-dispatch path and the demotion. Recorded as follow-on above.

## Implementation plan (for a cold swarm lane)

Constrain (`src/compiler/types/src/constrain.rs`):
1. Intern `live_f_on_navigate` symbol next to `live_f_routes`/
   `live_f_not_found` (≈lines 290–297, interning at ≈519–520).
2. Add an `OnNavigateCheck { cfg_row_var, page_var, msg_var, span }`
   deferred check pushed per `Web.app` call site (mirror
   `RoutedLiveCheck`, `constrain.rs:1148-1155`); resolve it in
   `src/compiler/types/src/lib.rs` next to `resolve_routed_live_checks`
   (≈864–955): if the settled row has `onNavigate`, unify with
   `Fun(page, msg)`; IPE-T0001 on mismatch.

Lower + emit (`src/compiler/lower/src/lower.rs` ≈3264–3305
`lower_app_entry_cfg`; `src/compiler/backend/rust/src/emit_live.rs`):
3. Thread the optional `onNavigate` cfg field through the lowered app
   entry; `emit_live_app_inner` (`emit_live.rs:361-417`) passes it to
   `live_app_routed` as `Option<Arc<dyn Fn(Page) -> Msg>>` (or a
   two-variant enum `NavigationMode::SetPage(set_page) | Dispatch(f)` —
   preferred: makes the absent/present states unrepresentable as a
   mixed pair).
4. `onNavigate` present + no `page` field must NOT take the non-routed
   emit branch: `routed_page_field` detection gains "or cfg has
   onNavigate" so routes are forwarded.

Runtime (`src/runtime/rust/src/live/mod.rs`):
5. Replace `route_resolver: Arc<dyn Fn(Model, &str) -> Model>`
   (≈1173–1174) with the `NavigationMode` enum: `SetPage` keeps today's
   closure; `Dispatch` runs `match_routes` then feeds
   `onNavigate(page)` into the normal update/dispatch path (same code
   path as a wire event, so Cmds, SSE patches, and seq ordering all
   behave identically).
6. No `client.js` change — the browser side is unchanged.

Docs (same commit): `docs/architecture/routed-live-app-design.md` gains
a short "superseded framing" note; `IPE-L0124` explain page updated;
`docs/divergences-from-sky.md` gains the notify-only divergence entry.

Dependency/ordering: none on Section-A work beyond routed-Live already
being landed (#108, done). Post-completion phase as filed.

## Test plan

Unit/golden (`src/ipe-cli/tests/`, fixtures under `tests/golden/`):
- `i155_on_navigate_dispatch` — routed app with `onNavigate = NavTo`;
  `update (NavTo p)` sets `page` and appends to a log list rendered in
  the view. E2E (`IPE_E2E=1`): drive a route change, assert the log
  entry appears (proves the Msg went through `update`).
- `i155_on_navigate_absent_baseline` — an existing routed golden
  rebuilt unchanged; assert emitted Rust and behaviour are byte-stable
  (the sugar path regression pin).
- `i155_on_navigate_bad_type` — `onNavigate : Int -> Msg` against
  `Page` routes → assert IPE-T0001 at the field span.
- `i155_on_navigate_no_page_field` — `onNavigate` present, model field
  named `current` instead of `page` → accepted, works E2E (demotion
  proof); and `routes` present with neither → IPE-L0124 warn still
  fires.
- `i155_on_navigate_veto` — `update (NavTo p)` returns the model
  unchanged for a "locked" page; E2E asserts the view did not change
  (app-owned navigation proof).

Runtime unit (`src/runtime/rust/src/live/` tests): `NavigationMode`
dispatch — popstate-shaped and nav-shaped requests both route through
the update queue in `Dispatch` mode; `SetPage` mode bit-identical to
current behaviour.

Reference cross-check: run the `onNavigate` fixture against upstream Sky
(which mutates-then-notifies) once, record the difference in the
divergence entry, and mark the fixture `oracle_divergence = true` with
that reason. All absent-field fixtures stay byte-equivalent to the Go
oracle.
