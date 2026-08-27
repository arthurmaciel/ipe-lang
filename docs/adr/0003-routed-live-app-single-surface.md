Status: Accepted

# 0003. Routed Web.app is one open-record surface with an emit-time branch

## Context

The canonical Ipe.Web entry point is:

```elm
main =
    Web.app
        { init = init, update = update, view = view, subscriptions = subscriptions
        , routes = [ route "/" HomePage, route "/apps/:slug" AppDetailPage ]
        , notFound = HomePage
        }
```

An earlier design rejected this with IPE-T0001 because `Web.app`'s cfg was
typed as a **closed 4-field record** `{ init, update, view, subscriptions }` and
the record unifier required **identical field sets** — there was no row variable
to absorb `routes` / `notFound` (let alone `head` / `consoleAuth` / `guard` /
`status`). Worse, a **separate `Web.appRouted` kernel**
(`KernelFn::WebAppRouted`, `Feature::RoutedWebApp`, gate IPE-L0118) had been
invented; the corpus never calls it, so the routed gate at `lower.rs:3305` was
effectively dead code.

The correct design has exactly one `Web.app` surface — no separate `appRouted`
at the type level. The code is the source of truth for the *how*; this ADR
records the *why*.

## Decision

**One `Web.app` surface, branched at emit time — no `appRouted` kernel.**

- The `Web.app` cfg is an **open row-polymorphic record** with six fields typed
  (`init, update, view, subscriptions, routes, notFound`) plus an `appExt` row
  variable, unified by an open-record rule (faithful port of the reference's row
  var). `routes`/`notFound` are always present (required fields) but are simply
  not emitted in single-page mode.
- The emitter branches on a single key: **does the Model record have a `page`
  field?** (recovered from the solved type of `view : Model -> Html Msg`). If
  yes → emit `live_app_routed` (routes vec, notFound, a generated `set_page`
  closure); if no → emit `live_app` (four TEA callbacks, routes/notFound
  dropped). There is **no `appRouted` kernel** anywhere in the reference — only
  `live_app` / `live_app_routed` at the Rust-emitter + runtime layer, driven by
  one `Web.app` surface. `WebAppRouted`/`RoutedWebApp`/IPE-L0118 are
  vestigial (kept as a defensive alias or deleted).

This retires the invented `appRouted` divergence.

### Round-4 seal fixes (parametric rendering)

Three adversarial holes were closed to keep `ipe` exit-0 ⇒ `cargo` exit-0:
parametric `IrType::LiveRoute(page)` renders `Route<Page>` (not a bare `Route`,
which was E0107); lambda-view routed detection goes through the shared
`fn_param_ty` (a lambda `view` was silently emitted non-routed); and a per-route
page witness replaced the shared-var `Web.route` scheme that false-blocked
`:param` routes.

## Consequences

### Typed route-`:param` payloads (sanctioned divergence, IPE-L0119)

A simpler approach emits a `String` into every page-constructor slot,
silently assuming every payload is `String`. That is correct for `AppDetailPage
String` but produces E0308 for `NumPage : Int -> Page`. The correct design uses
param-type-directed conversion at emit, driven by the variant's payload field
types (which emit already has):

| payload type | emitted expression |
|---|---|
| `String` | `params.get(i).cloned().unwrap_or_default()` |
| `Int`    | `params.get(i).and_then(\|s\| s.parse::<i64>().ok()).unwrap_or_default()` |
| `Float`  | `params.get(i).and_then(\|s\| s.parse::<f64>().ok()).unwrap_or_default()` |
| `Bool`   | `params.get(i).map(\|s\| s == "true").unwrap_or_default()` |
| other    | compile-time diagnostic **IPE-L0119** — reject, don't emit |

The `other` arm is the parse-don't-validate boundary: a `:param` segment is
inherently a URL string; feeding it to a payload the runtime cannot derive from
a string is a program error, surfaced as an Ipê diagnostic where the type is
known — never an opaque downstream `rustc` E0308. Missing captures and malformed
numerics degrade to `unwrap_or_default` (`0`/`0.0`/`false`), the never-panic
spirit of the String path. (Whether malformed numeric segments should instead
route to `notFound` is a future refinement, recorded not built.)

This is a **sanctioned divergence** (strictly safer, static typing catches the
mismatch at compile time).

### Invariant that must keep holding

There is exactly one `Web.app` surface; routed-vs-single-page is a *codegen*
decision recovered from the solved Model type (`page` field present), never a
separate kernel or type. Any future cfg field (`head`, `consoleAuth`, `guard`,
`status`) is absorbed by the `appExt` row variable — do not reintroduce a closed
cfg record or a second `app*` kernel.
