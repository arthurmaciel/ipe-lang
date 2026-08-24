# `package.ipe` — the manifest in Ipê, read syntactically

Status: design proposal. Every fenced Ipê block below illustrates the **proposed
surface, not shipped API** — none of it is runnable today. This detail-designs
Concern 1 of `docs/architecture/tbd/config-design.md` (the manifest slice):
replace the `ipe.toml` manifest with a `package.ipe` written in Ipê and **read
syntactically, never evaluated**.

The governing constraint is the **bootstrap**: the toolchain must learn a
project's dependencies *before* it can compile anything, so it cannot evaluate
Ipê — which would require those dependencies — to read them. The resolution is
the discipline the compiler already applies to a reserved builder literal: the
manifest declares one top-level `package` binding built from a blessed
vocabulary, and the toolchain extracts each field by **walking the AST of that
binding**, refusing anything that is not a literal argument to a blessed builder.

---

## 1. The complete current manifest surface to preserve

Every field the `ipe.toml` reader (`src/ipe-cli/src/project.rs`) parses today,
its typed home, who consumes it, and the parse-time validation or security check
it carries. `package.ipe` must reproduce **every column** — the reader changes,
the downstream contract does not.

| Manifest field (`ipe.toml`) | Typed field on `ProjectManifest` | Type | Reader(s) | Parse-time validation / security check |
|---|---|---|---|---|
| `[project] name` (or top-level `name`) | `name` | `String` | `watch` (cargo name via `sanitize_cargo_name`), `audit`, `publish`, `index`, `resolve` | Required — absent `name` is a hard `Usage` error |
| `version` (top-level or `[project]`) | `version` | `Option<semver::Version>` | `audit` (enforced-semver gate), `publish` (rejects versionless) | Parsed to typed `semver::Version`; a malformed value is a hard parse error (parse-don't-validate) |
| `[source] root` | `src_root` (`root.join(root|"src")`) | `PathBuf` | project discovery (module walk) | The source root directory must exist, else a hard `Usage` error |
| `[database] driver` | `driver` | `ipe_backend_rust::DbDriver` | `watch` (emit config), build/emit | `parse_db_driver`: only `sqlite` / `postgres` / `postgresql`; **any other value is a named hard error** (a silent sqlite fallback would build the wrong DB) |
| `[rust] static` | `static_request.static_build` | `Option<bool>` | `build_plan::resolve` (lowest layer under CLI > env) | `parse_bool` — malformed bool is a hard error |
| `[rust] target` | `static_request.target` | `Option<String>` | `build_plan::resolve` | Passed through; layered under CLI/env |
| `[rust] allocator` | `static_request.allocator` | `Option<AllocatorChoice>` | `build_plan::resolve` | `AllocatorChoice::parse` — an unknown allocator is a hard error |
| `[rust] allowSlowAllocator` | `static_request.allow_slow_allocator` | `Option<bool>` | `build_plan::resolve` | `parse_bool` |
| `[rust] cFree` | `static_request.c_free` | `Option<bool>` | `build_plan::resolve` | `parse_bool` |
| `[wasm] mode` | `wasm.mode` | `Option<String>` | build (`implies_wasm_target`), `watch` | (value shape validated at use; `off`/absent ⇒ no wasm target) |
| `[wasm] entry` | `wasm.entry` | `Option<String>` | build (wasm bundle entry) | — |
| `[wasm] mount` | `wasm.mount` | `Option<String>` | build (SPA mount selector) | — |
| `[wasm] publicEnv` | `wasm.public_env` | `Vec<String>` | build (env allowlist), `watch` (drift check) | **`validate_public_env` / `is_denylisted_public_env_name`**: rejects `DATABASE_URL`, `IPE_*`, `*_SECRET`, `*_TOKEN`, `*_KEY`, `*_PASSWORD` (case-insensitive) at **parse time** — a secret name in the public bundle is a build error, never a runtime refusal |
| `[wasm] optLevel` | `wasm.opt_level` | `Option<String>` | build (`wasm-opt` level) | — |
| `[dependencies] <name>` | `dependencies` | `BTreeMap<String, IpeDep>` | `resolve` (add/remove), fetch/lockfile, `publish` | `parse_ipe_dep`: bare string ⇒ typed `semver::VersionReq` (malformed = hard error naming the dep); inline table must carry exactly `git`(+opt `rev`) **or** `path` — the sum type makes "both at once" unrepresentable |
| `[rust.dependencies] <name>` | `rust_dependencies` | `BTreeMap<String, RustDep>` | `ffi` (crate binding), `audit` (asserts pinned source) | `parse_rust_dep`: version string (verbatim to cargo) + optional `features` list |
| `[rust.wrapper]` (section presence) | `has_rust_wrapper` | `bool` | `audit` (flags author-asserted wrapper it cannot regenerate) | Set on the section header (`is_rust_wrapper_header`, bare or quoted spelling) |
| `[rust.wrapper] path` / `expose` / `capabilities` | (read separately in `ffi.rs` via `WrapperManifest::parse`) | path + name list + capability list | `ffi` install/inspect | Wrapper path is **package-jailed** (must resolve back inside the project root); `enforce_wrapper_capabilities` validates the declared caps |
| `[capabilities] declared` | `capabilities` | `BTreeSet<Capability>` | `run_sandbox` (sandbox grant), `audit` (vs inferred) | `parse_capabilities`: each name via `Capability::from_str`; **an unknown capability is a named hard error** — a typo can never silently drop a capability the sandbox then fails to enforce |
| `[capabilities] accept` | `capabilities_accept` | `BTreeSet<Capability>` | `unsafe_ack` (pre-accepts `.Unsafe` import prompt) | `parse_capabilities` (same); durable pre-acceptance of a disclosed hazard |

Two structural facts to carry across unchanged:

- **`IpeDep` is a sum** (`Index(VersionReq)` | `Git { url, rev? }` | `Path`),
  not three optional fields — a dep can never be both git and path.
- The **required minimum** is `name`; every other field defaults (sqlite driver,
  empty dep maps, empty capability sets, `WasmConfig::default()` mode-off).

The manifest filename `"ipe.toml"` is currently hard-coded in
`find_manifest_for_ipe_file`, the `init` writer (`templates/ipe.toml.in`), and
the entry-discovery walk in `lib.rs`. All three become dual-name aware (§5).

---

## 2. The `Ipe.Package` vocabulary

A single blessed builder surface, one symbol per manifest field. It is a
*builder* only — a pipeline of `Package.*` / `Wasm.*` / `Rust.*` calls over
literal arguments, threaded by `|>`. The types below are the **proposed surface,
not shipped**.

```elm
-- proposed surface, not shipped

module Ipe.Package exposing (..)

type Package          -- opaque; the sole top-level `package` binding's type
type Dep              -- one Ipê dependency
type RustDep          -- one crates.io FFI dependency
type Wasm             -- the [wasm] sub-config
type Wrapper          -- a local FFI wrapper crate
type Driver           -- the DB driver enum
type Allocator        -- the allocator enum
type Capability       -- reuses the compiler's Capability vocabulary

-- construction + identity ---------------------------------------------------
Package.named        : String -> Package                      -- required root
Package.version      : String -> Package -> Package           -- semver string
Package.sourceRoot   : String -> Package -> Package           -- default "src"

-- dependencies --------------------------------------------------------------
Package.dependencies : List Dep -> Package -> Package
Package.dep          : String -> String -> Dep                -- name, semver req
Package.depGit       : String -> String -> Dep                -- name, url
Package.depGitRev    : String -> String -> String -> Dep      -- name, url, rev
Package.depPath      : String -> String -> Dep                -- name, local path

-- rust FFI crates -----------------------------------------------------------
Package.rustDependencies : List RustDep -> Package -> Package
Package.rustDep          : String -> String -> RustDep        -- name, version
Rust.features            : List String -> RustDep -> RustDep

-- rust wrapper crate --------------------------------------------------------
Package.wrapper      : Wrapper -> Package -> Package
Rust.wrapper         : String -> Wrapper                      -- local path
Rust.expose          : List String -> Wrapper -> Wrapper
Rust.wrapperCaps     : List Capability -> Wrapper -> Wrapper

-- database ------------------------------------------------------------------
Package.database     : Driver -> Package -> Package
Package.sqlite       : Driver                                 -- (default)
Package.postgres     : Driver

-- native build knobs ([rust]) ----------------------------------------------
Package.static       : Bool -> Package -> Package
Package.target       : String -> Package -> Package           -- rustc triple
Package.allocator    : Allocator -> Package -> Package
Package.system       : Allocator                              -- allocator choices
Package.dlmalloc     : Allocator
Package.autoAlloc    : Allocator
Package.allowSlowAllocator : Bool -> Package -> Package
Package.cFree        : Bool -> Package -> Package

-- capabilities --------------------------------------------------------------
Package.declares     : List Capability -> Package -> Package
Package.accepts      : List Capability -> Package -> Package
-- the Capability values (Network, Clock, Unsafe, …) come from the compiler's
-- reserved capability vocabulary, referenced by name, not re-declared here.

-- wasm ----------------------------------------------------------------------
Package.wasm         : Wasm -> Package -> Package
Wasm.spa             : Wasm                                    -- mode = "spa"
Wasm.hydrate         : Wasm                                    -- mode = "hydrate"
Wasm.entry           : String -> Wasm -> Wasm
Wasm.mount           : String -> Wasm -> Wasm
Wasm.publicEnv       : List String -> Wasm -> Wasm            -- denylist-checked
Wasm.optLevel        : String -> Wasm -> Wasm
```

Design notes:

- **Driver / Allocator / Wasm-mode are constructors, not strings.** `parse_db_driver`
  today rejects a typo'd `"postgre"`; in `package.ipe` a wrong driver is not
  expressible — `Package.postgres` is the only Postgres value. This lifts a
  parse-time validation into make-invalid-states-unrepresentable, one rank up.
  The syntactic reader still recognises these as blessed nullary constructors
  (§3), so no evaluation is needed.
- **`Package.dep` vs `depGit` / `depGitRev` / `depPath`** are distinct builders,
  mirroring the `IpeDep` sum: a dependency is exactly one of index / git / path,
  never a bag of optional keys.
- **`publicEnv` denylist is unchanged.** `Wasm.publicEnv` takes a `List String`
  of literals; the reader extracts them and runs the *identical*
  `is_denylisted_public_env_name` check (§4).

### A complete `package.ipe` exercising every field

```elm
-- proposed surface, not shipped
-- package.ipe

package =
    Package.named "my-app"
        |> Package.version "0.3.0"
        |> Package.sourceRoot "src"
        |> Package.database Package.postgres
        |> Package.dependencies
            [ Package.dep "ipe-http" "^1.2"
            , Package.depGitRev "ipe-widgets" "https://example.test/widgets.git" "a1b2c3"
            , Package.depPath "ipe-local" "../local"
            ]
        |> Package.rustDependencies
            [ Package.rustDep "uuid" "1.10"
            , Package.rustDep "image" "0.25" |> Rust.features [ "png", "jpeg" ]
            ]
        |> Package.wrapper
            (Rust.wrapper "./vendor/mycrate"
                |> Rust.expose [ "encode", "decode" ]
                |> Rust.wrapperCaps [ Capability.network ]
            )
        |> Package.static True
        |> Package.target "x86_64-unknown-linux-musl"
        |> Package.allocator Package.dlmalloc
        |> Package.allowSlowAllocator False
        |> Package.cFree True
        |> Package.declares [ Capability.network, Capability.clock ]
        |> Package.accepts [ Capability.unsafe ]
        |> Package.wasm
            (Wasm.spa
                |> Wasm.entry "src/Client.ipe"
                |> Wasm.mount "#app"
                |> Wasm.publicEnv [ "API_BASE_URL", "APP_VERSION" ]
                |> Wasm.optLevel "z"
            )
```

The minimal manifest is just `package = Package.named "my-app"` — every other
field defaults exactly as the absent-section defaults do today.

---

## 3. The syntactic reader — read, never evaluate

### Reuse the existing parser, stop before evaluation

The reader reuses the compiler's own front end and **nothing past it**. It calls
`ipe_parse::parse_module(src, interner)` (the same entry the compiler uses),
obtaining an `ipe_syntax::ast::Module`. It then operates purely on that AST —
no canonicalisation, no name resolution, no type-checking, no lowering, no
emit, and above all **no evaluation**. The parser is total and effect-free by
construction, so parsing an untrusted `package.ipe` runs no project code.

### What the reader accepts

Reading `package.ipe` is a walk over the AST of the single `package` binding:

1. **The module shape.** The reader requires exactly one top-level `Value` named
   `package` with an empty `patterns` list (a value binding, not a function).
   The module may carry a `module Package exposing (package)` header. **No
   `import` declarations are permitted** — a `package.ipe` that imports anything
   is rejected. (It cannot import: dependencies are not yet resolved; and the
   blessed vocabulary is recognised by name, never imported.) A second top-level
   binding, a `type`, or an `import` is a clean diagnostic.

2. **The pipeline spine.** `package`'s body is expected to be a `|>` pipeline —
   in the AST an `Expr_::Binops` whose operators are all `|>` — or a bare
   `Package.named "…"` head with no pipeline. The reader linearises the spine
   into an ordered list of **stages**: the head builder call plus each
   right-hand `Package.*` / `Wasm.*` / `Rust.*` call the value is piped into.

3. **Each stage is a blessed builder applied to literal arguments.** A stage is
   an `Expr_::Call(callee, args)` where `callee` is an `Expr_::VarQual(module,
   fn)` naming a symbol from the blessed set (`Package.*`, `Wasm.*`, `Rust.*`,
   and the `Capability.*` / `Package.postgres` / `Package.dlmalloc` nullary
   constructors). Each `arg` must be a **literal**:
   - `Expr_::Str` → a string field (name, version, url, path, mount, publicEnv
     entry, feature, target, opt level);
   - `Expr_::Int` → not used by any field (rejected if it appears);
   - `Expr_::List` of blessed sub-calls → a dependency list, rust-dep list,
     publicEnv list, feature list, capability list;
   - a nested blessed builder call → a `Dep` / `RustDep` / `Wrapper` / `Wasm`
     sub-value (recursion into 3 above);
   - a blessed nullary constructor `VarQual` (`Package.postgres`,
     `Package.dlmalloc`, `Capability.network`, `Wasm.spa`, …) → the enum value.
   - Booleans (`Package.static True`) are the AST's `True`/`False` constructor
     references, recognised as the two blessed nullary bools.

4. **Assemble the typed `ProjectManifest`.** Each recognised stage sets the
   corresponding typed field, running that field's existing validation on the
   extracted literal (§4). The result is the **same `ProjectManifest`** the
   `ipe.toml` path produces — one struct, two front doors.

### What the reader rejects (fail-closed, clean diagnostics)

Every non-literal, non-blessed shape is turned away with a typed diagnostic
naming the offending span — never silently ignored, never evaluated:

- an `import` in `package.ipe` (dependency bootstrap forbids it);
- a top-level binding other than `package`, or `package` as a function;
- a callee that is not a blessed `Package`/`Wasm`/`Rust`/`Capability` symbol —
  a user-defined function, a local `let`, a lambda application;
- a `VarLocal` / `VarQual` argument that is **not** one of the blessed nullary
  constructors (i.e. a computed value, a reference to another binding, an
  imported name);
- a non-literal argument: an `if`, a `case`, a `let`, a `Binops` arithmetic
  expression, a record, a string interpolation — anything a value could be
  *computed* from rather than *written* as a literal;
- an operator in the spine other than `|>`;
- an unknown builder symbol (`Package.frobnicate`) or a known builder with the
  wrong arity/argument shape.

Because the reader only ever *reads* AST nodes and pattern-matches their shape,
it is **total**: every input either yields a typed `ProjectManifest` or a typed
`Diagnostic`. There is no code path that runs a `package.ipe` expression.

### Why the AST, not a bespoke text parser

The existing `ipe.toml` reader is a hand-rolled line scanner. Reusing
`parse_module` for `package.ipe` is the SSOT win: one lexer/grammar, one set of
literal-escaping rules (string escapes, triple-quoted strings), one span/
diagnostic infrastructure. The reader adds only the AST-shape recogniser above —
it does not re-implement lexing or literal decoding.

---

## 4. Every parse-time security validation, preserved on the literals

The reader runs each check against the **extracted literal**, at read time,
exactly as the `ipe.toml` path runs it against the parsed token today. None is
deferred to a later stage.

1. **`name` required.** No `Package.named` stage ⇒ hard error (mirrors "missing
   `name`" today).
2. **`version` is valid semver.** The `Package.version "…"` literal is parsed by
   `semver::Version::parse`; a malformed string is a hard read-time error.
3. **`[dependencies]` version requirements are valid semver.** Each
   `Package.dep name req` literal `req` is parsed to `semver::VersionReq`;
   malformed = hard error naming the dep. Git/path deps keep the `IpeDep` sum's
   exactly-one-of guarantee structurally (three distinct builders).
4. **`[database] driver` is a known driver.** Unrepresentable-by-construction now
   (only `Package.sqlite` / `Package.postgres` exist), *strengthening* the
   `parse_db_driver` check into the type surface. The reader still rejects any
   other `VarQual` in driver position.
5. **`[rust]` bools and allocator are well-formed.** Bool fields take literal
   `True`/`False`; the allocator takes a blessed `Allocator` constructor — again
   strengthening `parse_bool` / `AllocatorChoice::parse` into the vocabulary.
6. **`[wasm] publicEnv` secret-name denylist.** Each `Wasm.publicEnv` string
   literal runs through the **unchanged** `is_denylisted_public_env_name`; a
   `DATABASE_URL` / `IPE_*` / `*_SECRET` / `*_TOKEN` / `*_KEY` / `*_PASSWORD`
   entry is a read-time build error. This is the single most security-load-bearing
   check and it moves across verbatim, still at parse (read) time.
7. **`[capabilities] declared` / `accept` are known capabilities.** Each
   `Capability.*` reference must be a blessed capability constructor; an unknown
   one is a named hard error (the typo-drops-a-capability footgun stays closed).
   These feed the sandbox grant and the `.Unsafe` pre-acceptance exactly as today.
8. **Wrapper path is package-jailed.** The `Rust.wrapper "…"` path literal is
   resolved and must land back inside the project root (the existing
   `ffi.rs` jail); `Rust.wrapperCaps` runs `enforce_wrapper_capabilities`.

The `is_denylisted_public_env_name`, `parse_db_driver` residual guard,
`parse_capabilities`, semver parsing, and the wrapper jail are **shared code** —
the reader calls the same functions, upholding single-source-of-truth: there is
no second copy of a validation that could drift from the `ipe.toml` path during
the coexistence window.

---

## 5. Migration and coexistence

### `ipe migrate config`

A new subcommand that reads an existing `ipe.toml` via the current
`parse_manifest` into a `ProjectManifest`, then **emits an equivalent
`package.ipe`** by rendering the typed struct through the `Ipe.Package`
vocabulary, field for field. Because it renders from the already-typed
`ProjectManifest`, the emitted `package.ipe` is byte-reproducible from the
struct and round-trips: reading it back yields the same `ProjectManifest`. The
command is mechanical (no prompts, no inference), mirroring the store-migration
tooling posture. It leaves `ipe.toml` in place (the author deletes it once
satisfied), and refuses to overwrite an existing `package.ipe` without `--force`.

A round-trip property test pins the guarantee: for any `ipe.toml` the current
reader accepts, `parse_manifest(toml)` == `read_package_ipe(migrate(toml))`.

### Coexistence: prefer `package.ipe`, fall back to `ipe.toml`, warn

**Recommendation: a deprecation window, not a hard switch.** During the window,
manifest discovery (in `find_manifest_for_ipe_file`, the entry-discovery walk,
and every `parse_manifest` call site) resolves in this order:

1. If `package.ipe` exists → read it syntactically; if `ipe.toml` *also* exists,
   emit a one-line warning that `ipe.toml` is ignored (the new manifest wins,
   deterministically — never a merge, which would be an ambiguous-precedence
   footgun).
2. Else if `ipe.toml` exists → read it the old way, and emit a one-line
   deprecation notice pointing at `ipe migrate config`.
3. Else → the existing "no manifest" error.

Both front doors produce the identical `ProjectManifest`, so **no downstream
reader changes** — `resolve`, `publish`, `ffi`, `build_plan`, `audit`, `watch`,
`run_sandbox`, `index` all consume the struct, not the file. This is the whole
reason the surface in §1 is enumerated as a struct contract: the file format is
swappable behind it.

Reasoning for the window over a hard switch, under the precedence order:

- **Correctness (2).** A hard switch would break every existing project's build
  the instant the toolchain updates — a silent correctness regression for users
  who have not migrated. A window lets old projects keep building while the
  migration path is available and advertised.
- **A window costs nothing on Security.** The `package.ipe` path is read-not-run
  from day one; the `ipe.toml` path is exactly as (in)secure as it is today (it
  was never evaluated either — it is a line scanner). The window does not weaken
  any invariant; it only delays *removing* the old reader.
- **The window is bounded and loud.** Every `ipe.toml`-only build warns, so the
  ecosystem is pushed toward `package.ipe` without a flag-day break. `init`
  writes `package.ipe` (not `ipe.toml`) from the start of the window, so new
  projects never adopt the deprecated format.

`init` and its `templates/ipe.toml.in` are replaced by a `package.ipe` template
in the same phase that turns on the new reader (§7 P1), so the default new-project
manifest is the new format immediately.

---

## 6. Security invariants (enforced, not merely documented)

- **Read-not-evaluated ⇒ no build-time code execution.** Cloning an untrusted
  project and running any `ipe` command that reads the manifest executes **none**
  of `package.ipe` — only `parse_module` (total, effect-free) plus an AST-shape
  walk. There is no eval, no import resolution, no I/O, no effect surface a
  hostile manifest could reach. This is the decisive reason to read syntactically
  rather than evaluate.
- **Total.** The reader is a total function `&str -> Result<ProjectManifest,
  Diagnostic>`: every input yields a typed manifest or a typed diagnostic, never
  a panic and never an evaluated expression.
- **Every validation preserved.** The eight checks in §4 run on the extracted
  literals via the *same* shared functions the `ipe.toml` path uses — defence in
  depth is maintained (e.g. the publicEnv denylist still also cannot be bypassed
  downstream), and SSOT prevents drift during coexistence.
- **Dependencies are statically extractable.** The bootstrap holds: the
  `dependencies` / `rustDependencies` / wrapper are literal arguments to blessed
  builders, so the resolver reads them from the AST before any third-party code
  exists — no chicken-and-egg.
- **No import surface.** A `package.ipe` may not `import`, closing the door on a
  manifest that pulls (and thereby names, or on a naive evaluator runs) foreign
  code to describe itself.
- **Invalid states unrepresentable.** Driver, allocator, and wasm-mode become
  blessed constructors, so a typo that today reaches a runtime `parse_db_driver`
  rejection is instead not a writable manifest at all.

---

## 7. Phased implementation plan

Each phase is independently landable and guardian-reviewable. Breaking phases
are called out; the plan is ordered so the breaking removal is last and the
window between is loud.

- **P1 — `Ipe.Package` vocabulary + syntactic reader, dual-name discovery.**
  *(non-breaking; additive.)*
  Add the blessed `Ipe.Package` / `Wasm` / `Rust` vocabulary (recognised
  by-name by the reader; no runtime code), the AST-walking reader producing a
  `ProjectManifest`, and the shared-validation wiring (§4). Make manifest
  discovery dual-name with the §5 precedence (`package.ipe` preferred, `ipe.toml`
  fallback + deprecation warning). Switch `init` to write `package.ipe`. Existing
  `ipe.toml` projects keep building unchanged. **Security-guardian review
  required** (the reader's totality, the read-not-eval guarantee, and that every
  §4 validation runs on the literals).

- **P2 — `ipe migrate config`.** *(non-breaking; additive.)*
  Add the subcommand that renders an `ipe.toml`-derived `ProjectManifest` to an
  equivalent `package.ipe`, with the round-trip property test. No existing
  behaviour changes.

- **P3 — flip internal defaults + docs to `package.ipe`.** *(non-breaking.)*
  Point examples, templates, docs, and the AGENTS authoring reference at
  `package.ipe`; migrate the repo's own example/fixture manifests via P2. The
  `ipe.toml` reader still exists and still warns; nothing breaks, but the
  ecosystem's centre of gravity moves.

- **P4 — retire `ipe.toml` as a project manifest.** ***(BREAKING.)***
  `package.ipe` is the sole manifest the toolchain discovers, builds, watches,
  publishes, audits, and cleans. A directory with only a legacy `ipe.toml` and no
  `package.ipe` no longer builds — it errors with a clear `run ipe migrate config`
  diagnostic. The line-scanner survives ONLY as `ipe migrate config`'s input
  reader (the one path that still reads an `ipe.toml`, to convert it). This is the
  only breaking phase.

  Two authoring surfaces read TOML sections directly and are not yet ported to a
  `package.ipe` AST rewrite: `ipe add`/`ipe remove` (rewriting `Package.dependencies`)
  and `ipe rust install` (reading `[rust.dependencies]` / `[rust.wrapper]`). Both
  fail closed with a clear pointer to the manual step / the outstanding ergonomic
  Rust-FFI work rather than editing a manifest they cannot yet round-trip.

Only **P4 is breaking.** P1–P3 are strictly additive and coexist with `ipe.toml`.

---

## 8. Open questions / risks for the maintainer

1. **Coexistence window length / trigger for P4.** How long (or on which
   release boundary) does the `ipe.toml` fallback live before P4 removes it?
   The design recommends a bounded, loud window but does not pick the boundary.
2. **External-tooling export.** Registry indexers and third-party tools cannot
   parse Ipê. Do we ship a `package.ipe` → JSON (or lockfile-adjacent) **export**
   for those consumers, and if so is it a stable, documented schema? The manifest
   itself stays Ipê; this is a read-only projection for outsiders.
3. **Booleans as blessed constructors.** `Package.static True` relies on the
   reader recognising `True`/`False` as blessed nullary bools. Confirm this is
   the preferred surface over dedicated builders (`Package.staticOn` /
   `Package.staticOff`) that avoid recognising any bare constructor.
4. **Capability vocabulary reference.** `package.ipe` names capabilities as
   `Capability.network` etc. without importing anything. Confirm the reader
   should special-case the `Capability.*` qualifier as blessed (consistent with
   `Package.*` / `Wasm.*`), versus some other spelling.
5. **`ipe package add` AST-rewrite.** `add`/`remove` today textually edit the
   `[dependencies]` section preserving comments. On `package.ipe` they must
   rewrite the `Package.dependencies [ … ]` list in the AST while preserving the
   author's formatting and comments. Is a comment-preserving AST rewrite in
   scope for the manifest slice, or does it get its own follow-up (the
   config-design doc flags this tooling cost as the accepted price)?
6. **`[source] root` necessity.** `sourceRoot` is preserved for parity, but only
   `src` is exercised in practice. Keep it in the vocabulary, or drop it and hard-
   code `src` (a smaller surface, one fewer field to validate)?
