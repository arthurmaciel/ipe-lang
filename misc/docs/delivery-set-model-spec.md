# Delivery-set model and packager loop — spec + implementation plan

`ipe build` (no args) builds the default delivery. `ipe release` (no args) must
build **every** delivery the project declares — that contract is already stated
in the `release` help text and in the shape-overhaul spec (§7, §12), but nothing
in `package.ipe` today *represents* the set of deliveries a project ships, so
`release` still produces exactly one artifact. This document models the set,
the type that makes a declared-but-unbuilt delivery unrepresentable, the
packager loop, and the migration path.

## 1. What exists, and the gap

- `src/ipe-cli/src/delivery.rs` — `Delivery` (shape × web-runtime × host) is
  the validated single-invocation delivery. It is constructible only through
  `Delivery::resolve` / `resolve_checked`, which is the one validity table
  (live is the unnamed default; `spa` the only runtime word; mobile hosts are
  spa-only; `--static` gated by `allows_static`). Invalid combinations are
  unrepresentable *per invocation*.
- `package.ipe`'s `delivery` record (`src/ipe-cli/src/project.rs`
  `DeliveryConfig`, read by `package_manifest.rs::read_delivery`, schema in
  `src/stdlib/Ipe/Package.ipe`) holds **per-host configuration** —
  `desktop = { title, width, height }`, `mobile = { bundleId, orientation }`,
  `browser = { basePath }`. All sections are present and live; none of them
  says "this project ships to that host". Presence of a config section is
  *not* intent to ship (defaults exist for every section, `ipe init` fills
  them all), so config cannot double as the declaration.
- `run_release` (`src/ipe-cli/src/lib.rs`) routes one invocation to one
  artifact: wasm-target → browser bundle, native → optimised binary (jailed
  bundle when native-bearing). The desktop and mobile packagers
  (`pack/desktop.rs`, `pack/mobile.rs`) run only via the separate
  `ipe pack --target …` command; `release` never reaches them.

The gap is a missing noun: the project's **delivery set** — which
runtime × host × target combinations this project ships. The shape is never
part of it: the shape is pinned by `main`, and config never overrides it.

## 2. The manifest surface: `ships`

The `delivery` record gains one field, `ships` — a list of **ship entries**,
each naming one delivery the project releases. Entries are builder values from
the `Ipe.Package` schema (the same closed-vocabulary builder pattern as
`dep` / `rustDep`), so a typo is an unknown name at read time, never a string
compared at build time.

```elm
package =
    { name = "my-app"
    , delivery =
        { ships =
            [ binary                 -- the co-located artifact, host triple
            , staticBinary           -- + the musl static binary
            , desktop                -- web live desktop: webview-native bundle
            , spa                    -- web spa: browser wasm bundle
            , spaIos                 -- web spa ios: wasm + WKWebView shell
            ]
        , desktop = { title = "My App", width = 1024, height = 768 }
        , mobile  = { bundleId = "com.example.myapp", orientation = Portrait }
        , browser = { basePath = "/" }
        }
    }
```

### The ship vocabulary (closed)

| builder | delivery (CLI words) | artifact |
|---|---|---|
| `binary` | *(none)* — the shape's own co-located artifact; for `web`, served live | native binary on the host triple |
| `staticBinary` | `--static` | musl static binary (co-located shapes only) |
| `crossBinary "<triple>"` | `<triple>` | cross-compiled co-located binary; triple validated at read time against the curated target set |
| `desktop` | `web desktop` | webview-native desktop bundle |
| `spa` | `web spa` | browser wasm bundle |
| `spaDesktop` | `web spa desktop` | wasm bundle + native webview shell |
| `spaIos` | `web spa ios` | wasm bundle + iOS shell |
| `spaAndroid` | `web spa android` | wasm bundle + Android shell |

Design decisions, in order of the tension they resolve:

- **One vocabulary.** Every builder maps 1:1 onto the CLI delivery grammar
  (`Delivery`'s `Display` words). The manifest is a stored spelling of the
  same sentences the CLI accepts; no second naming scheme.
- **Shape-neutral entries, shape checked at resolve time.** `binary` /
  `staticBinary` / `crossBinary` are meaningful for every shape (`served`
  would misname a `cli` binary); the web-only entries (`desktop`, `spa*`) are
  meaningful only when `main` is a `web` app. The manifest reader cannot know
  the shape (it reads before any compilation), so shape compatibility is
  checked where the shape *is* known — delivery-set resolution — reusing the
  existing pedagogical `DeliveryError` voice: *"`package.ipe` declares
  `spaIos`, but `main` is a `cli` app. Mobile shells carry a sandboxed web
  client; a `cli` app ships as a binary. Remove the entry, or change `main`
  to a `Web.app` entry."*
- **Ship-set separate from host config.** A host's config (`desktop = { … }`)
  is shared by every delivery on that host (`desktop` and `spaDesktop` both
  read the window title); folding config into ship entries would either
  duplicate it or turn presence into meaning. The `ships` list declares
  *what* ships; the sections configure *how* it looks when it does. Both stay
  all-present and live, no `active` selector.
- **Duplicates are read-time errors** (as duplicate program names are): a
  delivery is shipped once; a repeated entry is a confused manifest, not a
  request for two copies.
- **The empty list is a read-time error.** A project that ships nothing has
  no release; the fix (omit the field entirely for the default) is named in
  the message. This keeps the resolved set non-empty *by parse*, so the
  packager loop never needs an "empty set" branch.
- **`build.target` / `build.static` stay what they are** — the default
  *build* configuration for the fast inner loop. `staticBinary` /
  `crossBinary` describe *release* deliveries. The two do not merge: `build`
  answers "what does `ipe build` produce", `ships` answers "what does
  `ipe release` produce".

### Schema additions (`src/stdlib/Ipe/Package.ipe`)

`Delivery` gains `ships : List Ship`; `Ship` is a new opaque type with the
eight builders above exposed (`crossBinary : String -> Ship`, the rest
nullary). Signing remains release-time environment, never manifest data.

## 3. The typed model: declared-but-unbuilt is unrepresentable

Three layers, each a parse boundary in the parse-don't-validate sense — the
next layer never re-checks what the previous one established.

### 3.1 `ShipEntry` — the parsed manifest entry

```rust
/// One declared delivery, as read from `package.ipe` — shape not yet known.
pub enum ShipEntry {
    Binary(BinaryTarget), // Host | Static | Cross(CuratedTriple)
    Desktop,
    Spa,
    SpaDesktop,
    SpaIos,
    SpaAndroid,
}
```

Produced only by the manifest reader (`read_delivery` gains a `ships` arm
using the existing `expect_ctor_app` builder machinery). `CuratedTriple` is
the triple already parsed against the curated set (the same set the
static/build-plan layer owns — one source), so an unsupported triple is a
`package.ipe:LINE:COL` rejection, not a cargo error later.

### 3.2 `DeliverySet` — resolved against the pinned shape

```rust
/// The non-empty, duplicate-free set of deliveries this project releases,
/// every element individually admitted by `Delivery::resolve`.
pub struct DeliverySet(/* private */ Vec<PlannedDelivery>);

pub struct PlannedDelivery {
    delivery: Delivery,          // the existing validated type
    target: BinaryTarget,        // Host | Static | Cross — the third axis
}
```

`DeliverySet::resolve(pinned: Shape, entries: &[ShipEntry])` maps each entry
onto the arguments of the **existing** `Delivery::resolve` (e.g. `SpaIos` →
`(Web, Some(Spa), Host::Ios)`) and the existing `allows_static` gate for
`Binary(Static)`. The single-invocation validity table remains the only
validity table — the set resolver is a fold over it, so the two can never
disagree (single source of truth; a manifest-declared combination is exactly
as valid as the same words typed at the CLI). Web-only entries on a non-web
shape fail here with the pedagogical error of §2. An absent `ships` field
resolves to the singleton `[ binary ]` — the shape's default delivery — which
is precisely today's `release` behaviour.

`DeliverySet` is constructible only through `resolve`, and its inner vector is
private: no code path can hold a delivery set containing an invalid, duplicate,
or shape-incompatible member, and none can construct an empty one.

### 3.3 `ReleaseOutcome` — the loop's only exit

```rust
/// Proof one delivery was built and packaged. Private constructor: the only
/// producer is the release loop, fed by a packager that actually ran.
pub struct Released {
    delivery: Delivery,
    artifact: PathBuf,           // what was written, for the summary line
}

pub enum ReleaseOutcome {
    /// Every declared delivery built. Constructible only by the loop, only
    /// when the built list is element-for-element the declared set.
    AllReleased(Vec<Released>),
    /// At least one delivery failed. Carries every outcome so the report
    /// names each failure (never a bare "release failed").
    Incomplete {
        built: Vec<Released>,
        failed: Vec<(Delivery, CliError)>, // non-empty by construction
    },
}
```

The loop is the sole constructor:

```rust
impl DeliverySet {
    /// Consume the set, invoking `release_one` per delivery in declared
    /// order. There is no other way to obtain a `Released`, and no way to
    /// obtain `AllReleased` without one `Released` per declared delivery.
    pub fn release_each(
        self,
        release_one: impl FnMut(&PlannedDelivery) -> Result<PathBuf, CliError>,
    ) -> ReleaseOutcome { /* fold; private Released constructor */ }
}
```

This is where "a declared-but-unbuilt delivery is unrepresentable" lands:

- `Released` has no public constructor — it exists only as evidence a
  packager ran to completion for that delivery.
- `AllReleased` is produced only when the fold consumed every element; a
  partial success has a *different type constructor* (`Incomplete`), so no
  call site can treat it as success by accident — matching on the outcome is
  exhaustive, and `Incomplete` carries what failed.
- `DeliverySet` is consumed (`self`) by the loop: the set cannot be iterated
  halfway and dropped with the remainder silently unshipped.

Failure semantics (fail-closed, kind to the developer): the loop **continues
through failures** — each delivery is attempted, each failure recorded — and
the command exits non-zero unless the outcome is `AllReleased`. The
all-green seal line prints only on `AllReleased`; `Incomplete` prints a
per-delivery ✓/✗ table with each failure's own diagnostic. Continuing costs
nothing in safety (success is typed, partial artifacts live in per-delivery
directories) and saves the fix-one-rerun-discover-the-next cycle.

## 4. The packager loop

`run_release` becomes: resolve the pinned shape → read the manifest → resolve
the `DeliverySet` → `release_each` with a router that dispatches each
`PlannedDelivery` to the machinery that already exists:

| planned delivery | route (all existing code paths) |
|---|---|
| `binary` (any shape; web = served live) | the current native release path (optimised binary; jailed bundle when native-bearing) |
| `staticBinary` | native path with the musl `StaticPlan` |
| `crossBinary t` | native path with triple `t` |
| `desktop` | build with `webview_host` + the `pack::desktop` layout/materialise pipeline |
| `spa` | the current wasm/browser-bundle path (`bundle_wasm`) |
| `spaDesktop` | wasm bundle + `pack::desktop` (spa shell) |
| `spaIos` / `spaAndroid` | wasm bundle + `pack::mobile` |

The router is one exhaustive `match` — a new ship variant that lacks a route
is a compile error, not a silently skipped delivery (no wildcard arm).

- **`ipe pack` folds in.** The desktop/mobile packagers stop being a separate
  user-facing step for the declared set: `release` invokes them. `ipe pack`
  remains as the low-level single-step tool (and `--emit-permissions`
  inspection) — it is a subset view, not a second owner.
- **Output layout.** Every released delivery writes to
  `release/<slug>/…`, the slug being the delivery's CLI words joined by `-`
  with the target suffix when non-host (`release/web/`,
  `release/web-desktop/`, `release/web-spa-ios/`, `release/cli-static/`,
  `release/cli-aarch64-unknown-linux-gnu/`). Deterministic, collision-free
  by construction (slugs are injective over the set since the set is
  duplicate-free). `--out <dir>` replaces the `release/` root, never the
  per-slug nesting. A manifest with *no* `ships` field (the implicit
  singleton) keeps today's flat `release/` layout, so existing projects and
  scripts observe no change until they declare a set.
- **CLI subset/override.** Delivery positionals on `release` keep their
  §7-spec meaning: `ipe release spa ios` releases exactly that one delivery
  for this invocation — resolved through the same `ShipEntry → DeliverySet`
  path as a singleton (so the identical validity and shape checks run),
  using the manifest's host config when present and built-in defaults when
  not. Positionals never edit `package.ipe` and never mean "add to the set".
- **Shared work is computed once.** Every `spa*` delivery needs the same wasm
  bundle; the loop builds it on first need and reuses it for the shells
  (efficiency inside the loop, invisible in the model).
- **Capability/consent resolution runs once per release**, before the loop:
  the accepted-capability set is a property of the program, not of a
  delivery; per-delivery permission *derivation* (`pack::permissions`) stays
  inside each packager as today.

## 5. Goldens and gates — per released delivery

Following the coverage-matrix rule (no combination ships untested; the tier is
run-E2E where CI can execute the artifact, build-golden where it cannot):

- **Manifest reader goldens/tests:** `ships` parsing — each builder, the
  cross triple, unknown-builder rejection, duplicate rejection, empty-list
  rejection, absent-field default. Refusals are pinned (prove the refusals).
- **Set-resolution tests:** web-only entries on each non-web shape refuse
  pedagogically; `staticBinary` refuses on `desktop`-style deliveries via the
  existing `allows_static` wording; absent field = singleton default.
- **Loop tests:** `AllReleased` only when every delivery built (a stub router
  failing the k-th delivery yields `Incomplete` naming it); exit code
  non-zero on `Incomplete`; declared order preserved; slug layout goldened.
- **Emit goldens per delivery:** each routed delivery's emitted project is
  byte-goldened (the multi-delivery release of the worked `web` example locks
  one golden per declared delivery). Run-E2E where the host can execute
  (binary, served web, spa browser); build-golden for shells, cross, musl.
- **Scaffold golden:** `ipe init`'s written `ships` list per wizard answer
  set, so the wizard and this spec cannot drift.

## 6. Migration

Existing single-delivery projects require **no edit**:

- `ships` absent → the implicit `[ binary ]` singleton → `ipe release`
  builds one artifact into flat `release/`, exactly the current behaviour.
- `render_manifest_record` (used by `ipe migrate config`) emits `ships` only
  when it differs from the implicit default, so minimal manifests stay
  minimal and round-trip unchanged.
- `ipe init` starts writing an explicit `ships` reflecting the wizard's
  shape/runtime/host answers (a `web` project that answered "desktop too"
  gets `[ binary, desktop ]`), making the declaration visible from the first
  scaffold.
- The `release` help text already promises the every-declared-delivery
  contract; this work makes the promise true rather than changing it.

## 7. Implementation plan (each task lands green on its own)

1. **Schema + reader.** `Ship` type and eight builders in
   `src/stdlib/Ipe/Package.ipe`; `ShipEntry`/`BinaryTarget` in the CLI;
   `read_delivery` gains the `ships` arm (builder dispatch via
   `expect_ctor_app`, duplicate/empty/unknown rejections, curated-triple
   parse); `render_manifest_record` renders it; reader tests + refusal pins.
   No behaviour change — the field is read and carried, unused.
2. **`DeliverySet`.** `PlannedDelivery`, `DeliverySet::resolve` folding over
   `Delivery::resolve` + `allows_static`, shape-compat pedagogy, the
   absent-field singleton default; unit tests across all shapes × entries.
3. **`ReleaseOutcome` + loop skeleton.** `Released` (private ctor),
   `release_each`, slug + layout computation; loop tests with a stub router.
4. **Router: co-located deliveries.** Wire `binary` / `staticBinary` /
   `crossBinary` through the existing native paths under the loop; flat-vs-
   slug layout switch; `run_release` now consumes the set end to end for
   non-web projects. E2E: a `cli` project with `[ binary, staticBinary ]`
   releases both.
5. **Router: web deliveries.** `spa` via the wasm path, `desktop` via
   `pack::desktop`, `spaDesktop`/`spaIos`/`spaAndroid` via the shared wasm
   bundle + shells; once-per-release consent resolution. E2E/goldens per §5.
6. **CLI subset override.** Release delivery positionals resolved through
   the same singleton path; refusal when the positional names a delivery the
   shape cannot carry (same errors as the set resolver).
7. **`ipe init` + docs.** Wizard writes explicit `ships`; scaffold goldens;
   guide + `ipe doc` surface updates; help text audit (promise ⇔ behaviour).

Dependencies: 1 → 2 → 3 → {4, 5} → 6 → 7; tasks 4 and 5 are independent of
each other after 3.

## Boundaries

- No delivery-specific *signing* configuration in `package.ipe` — signing
  stays release-time environment.
- No `active`/selector field, ever: the CLI selects, the manifest declares.
- No per-ship overrides of host config (a second `title` per entry): one
  host, one config. If a real need appears, it arrives as a new typed field,
  not an open record merge.
- `ipe build` semantics are untouched: default delivery only, fast inner
  loop.
