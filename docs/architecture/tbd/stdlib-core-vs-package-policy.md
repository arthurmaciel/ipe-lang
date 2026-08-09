# Stdlib core-vs-package policy — a lean core, an opt-in package ecosystem

> Status: design proposal, no implementation. This note sits **above**
> `docs/architecture/tbd/stdlib-placement-policy.md` and its ADR
> `docs/adr/0057-stdlib-placement.md`. Those partition the stdlib by *what a
> function is* (capability / security-defense / perf primitive / cold
> computation) to decide native-vs-Ipê. This note partitions the stdlib by
> *packaging readiness* — what has to ship before a module can leave the central
> tree — and records the maintainer's fixed per-module decisions. The two axes
> compose: a module the placement policy calls "Ipê package" is assigned a
> *when* here (package-now vs package-later); a module it calls capability,
> defense, or core stays **core** here.

## Why a lean core

A smaller central library is not an aesthetic preference; it follows directly
from the precedence order (Security first).

- **Smaller trusted computing base.** Every module that ships in the default
  install is code the user runs without asking. A lean core is a smaller audited
  and attack surface — the same argument the placement policy makes for kernels,
  applied to whole modules.
- **Installation is explicit consent.** `ipe add <pkg>` is a user act that
  mirrors the capability model: the user chooses to include the code, exactly as
  they consent to a package's disclosed capabilities at add time (see
  `docs/adr/0044-package-coordination-manifest-index-gate.md`). Code inclusion
  gated on installation is the natural analogue of effect inclusion gated on
  capability disclosure.
- **Faster compile, smaller emitted binary.** Core the user never imports still
  costs parse/resolve/DCE work; a package costs nothing until added.
- **Examples become teaching material.** When the first-party libraries ship as
  packages and the examples `ipe add` them, the example *is* a worked
  package-consumer — the best documentation for authoring and consuming
  packages. The architecture teaches its own extension model.

The central project stays lean; we show users abundantly how to produce external
packages by making our own first-party libraries packages.

## The decision rule — three tiers by what a module NEEDS

A module's tier is set by what must exist before it can leave the core, not by
its subject matter.

1. **Core** — the module is one of:
   - a **security / capability primitive** (a parse-don't-validate barrier, or a
     world-touching capability the trust model gates), or
   - a **language primitive** the compiler/runtime itself depends on, or
   - a **battery the language's primary story needs out of the box** (the
     default `ipe new` app compiles and runs without an `ipe add`).

   Core is non-negotiable residency: a security primitive must never be opt-in
   (a user who forgets to add it loses the defense), and the primary story must
   work on first run.

2. **Package-now** — pure Ipê source that composes only **existing public
   kernels / stdlib**. It can move the moment the package ecosystem ships; it
   needs no new native code. Its safety, where relevant, comes from the barriers
   it routes through (e.g. the native HTML escaper), not from core residency.

3. **Package-later** — needs a **new native kernel or native crate** that is not
   yet bindable from package source. It can move only after the native-package /
   FFI-to-Rust tier lands (`docs/architecture/ffi-to-rust.md`,
   `docs/adr/0033-ipe-rust-ffi-subsystem.md`), which lets a package declare and
   bind native code under the disclosed-and-consented capability model.

The tier boundary between package-now and package-later is precisely **kernel
bindability from package source** (see the open question at the end): a module
whose native kernels are already registered is package-now *iff* a package may
bind them; otherwise it waits with package-later.

## Fixed maintainer decisions

Each is stated verbatim, then justified against the rule.

### Html (+ `Ipe.Html.Attributes` / `Ipe.Html.Events` / `Ipe.Html.Unsafe`) → PACKAGE

`Ipe.Ui` does **not** depend on `Ipe.Html`, and the language emphasizes
`Ipe.Ui`. Making Html a package makes the Ui-first path the easy, default one —
**the architecture becomes the incentive**: the terse, safe road (`Ipe.Ui`
builders) is in core, and the raw-HTML road costs an explicit `ipe add`.

**Kernel coupling (verified).** `Ipe.Html` is already a hybrid
(`src/stdlib/Ipe/Html.ipe`): every element builder (`div` / `table` / `br` / …)
is pure Ipê over five retained native constructors — `Html_node`,
`Html_voidNode`, `Html_text`, `Html_doctype`, `Html_titleNode`,
`Html_styleNode` — reached through point-free `Ffi.kernel "Html_*"` aliases. The
serialiser and escapers (`Html_render`, `Html_renderStatic`, `Html_escapeHtml`,
`Html_escapeAttr`, `Html_attrToString`) stay **native** — they are the XSS
barrier and render sink, per the placement policy's worked example. Crucially,
**all of these kernels are already registered**; the package would bind existing
kernels, minting no new native code.

**Tier: package-now — conditional on kernel bindability from package source.**
The Html package needs to bind already-registered kernels
(`Html_node`/`Html_voidNode`/`Html_text` to build, `Html_render`/`Html_escape*`
to render), not new native infrastructure. So it is package-now **iff** a
package (a `User`-origin Ipê module) may write `Ffi.kernel "Html_*"` against a
registered kernel. If kernel-alias binding is reserved to `EmbeddedStdlib`-origin
source, Html cannot leave until that privilege is extended to package source —
which is a bindability-policy change, not new native code, so it is still
"package-now-once-bindable", never package-later. The escaper stays native and
non-overridable regardless of where the constructors live; the package binds it,
it does not re-implement it.

**`Html.Unsafe` in a package is safe by consent, not by residency.**
`Ipe.Html.Unsafe.unsafeRaw` is an XSS sink. Its safety when packaged comes from
the **unsafe-import acknowledgment** (`docs/architecture/tbd/unsafe-import-acknowledgment.md`)
and the inferred `unsafe` capability
(`docs/architecture/tbd/unsafe-escape-convention-design.md`): importing the
`.Unsafe` submodule discloses `unsafe`, and consuming user code must acknowledge
it. A packaged `Html.Unsafe` is exactly as visible and consented-to as a core
one — the boundary is the capability, not the shelf it sits on.

### `Ipe.Bitwise` → PACKAGE

Niche integer bit-ops, pure over runtime kernels
(`src/stdlib/Ipe/Bitwise.ipe`, eight `Bitwise_*` aliases). It is not on any
program's primary path, so it fails the battery test for core.

*Correction to the brief's rationale:* `Ipe.Bitwise` **mirrors elm/core
`Bitwise`** (it is included in Elm, not excluded) — its own module comment says
so. The PACKAGE decision stands on niche-and-pure grounds alone. Its tier is
package-now-once-bindable for the same reason as Html: it binds already-registered
`Bitwise_*` kernels, so it moves once package source may bind kernels.

### `Ipe.Path` → KEEP CORE

`Ipe.Path` is a **parse-don't-validate security primitive**
(`src/stdlib/Ipe/Path.ipe`): the type is opaque, and `fromString` is the sole
constructor — it normalises and **rejects a NUL byte or a `..` traversal
escape**. It is the barrier that keeps an unvalidated string from reaching a
`File` / `System` syscall (every `Ipe.File` entry point takes a `Path`, not a
`String`). A security primitive must not be opt-in: a user who forgot to
`ipe add` it would lose path-traversal protection. Rule 1 (security primitive)
pins it to core.

### Package-now (pure source, existing kernels only)

| Module | Coupling | Note |
|---|---|---|
| `Ipe.Palette` | pure (0 kernels) | colour data; pure computation |
| `Ipe.Money` | hybrid (12 `Decimal`/currency kernels) | arithmetic/format over `Decimal`; the currency table is data (see churn) |
| `Ipe.Csv` | hybrid (5 kernels) | parse+format over `String` |
| `Ipe.Markdown` | pure (0 kernels) | `String → Html`; **output must route through the native Html escaper** — the hidden-defense trap the placement policy flags. Package-now, but never emits raw HTML |
| `Ipe.Uuid` | native (kernel-qualifier, no `.ipe`) | v4/v7 layout is pure over the `Random`/`Time` kernels; **currently native** — becomes package-now once its layout is re-expressed as pure Ipê over those kernels |
| `Ipe.Locale` | hybrid (2 kernels) | data tables |
| `Ipe.Ui.Chart` | pure (0 kernels) | typed SVG chart builder; routes labels/ticks through the Html escaper — package-now, output stays escaped |

### Package-later (needs native, waits on the FFI / native-package tier)

| Module | Coupling (verified) | Note |
|---|---|---|
| `Ipe.Compression` | native kernels exist (`src/runtime/rust/src/compression.rs`, 4 aliases) | gzip/zstd algorithms are a vetted native crate |
| `Ipe.Config` | native kernels exist (`config_decode.rs`, `config_postgres.rs`, 28 aliases) | native TOML/YAML/env parsers — **verified native, not pure** |
| `Ipe.Email` | native (3 aliases, lettre SMTP) | network capability — a world-touching send |
| `Ipe.Regex` | native (6 aliases, `regex` crate) | a native engine; a security-sensitive parser |

These four are **capability / vetted-native / security-parser** modules under
the placement policy: even as packages they carry native code. They can leave
the central tree only once a package may declare-and-bind native crates under
the disclosed-capability model (the FFI-to-Rust / native-package tier). Note the
native kernels for Compression and Config already exist in the runtime, so the
blocker is the *packaging* of native code, not writing new kernels — but the
gate is still the native-package tier, so they remain package-later.

## Full classification

Enumerated from `ipe doc --list` (99 stdlib modules) and verified against
`src/stdlib/Ipe/**` and the runtime kernel sources. Coupling drives the tier:
**pure** = `.ipe`, kernel-free; **hybrid** = `.ipe` over kernels; **native** =
kernel-qualifier (no `.ipe`) or a vetted native crate. A module marked
package-now-once-bindable is pure/hybrid over *already-registered* kernels and
moves the instant package source may bind kernels; package-later needs the
native-package tier.

| Module | Coupling | Tier | One-line reason |
|---|---|---|---|
| `Ipe.Basics` | hybrid | **core** | language primitive: operators + core types |
| `Ipe.Maybe` | pure/kernel | **core** | core ADT; happy-path combinators (native until benchmarked) |
| `Ipe.Result` | pure/kernel | **core** | core ADT; happy-path combinators |
| `Ipe.Tuple` | pure/kernel | **core** | core ADT + combinators |
| `Ipe.Error` | native | **core** | core error type threaded through every `Task`/`Result` surface |
| `Ipe.List` | hybrid | **core** | hot throughput primitive |
| `Ipe.Dict` | hybrid | **core** | hot throughput primitive |
| `Ipe.Set` | hybrid | **core** | hot throughput primitive |
| `Ipe.String` | hybrid | **core** | hot throughput primitive |
| `Ipe.Math` | hybrid | **core** | hot throughput primitive |
| `Ipe.Char` | hybrid | **core** | Unicode category half is a vetted native table; ASCII half pure |
| `Ipe.ToString` | pure | **core** | ubiquitous formatting on the display path |
| `Ipe.Task` | hybrid | **core** | language primitive: the effect type |
| `Ipe.Tea.Terminal` / `.Web` / `.WebView` (+ `.Cmd` / `.Sub` / `.PubSub`) | hybrid/kernel | **core** | the reactor engine is the primary app story; the `Cmd`/`Sub` sugar rides it |
| `Ipe.Ui` (+ `.Animation` `.Background` `.Border` `.Events` `.Font` `.Grid` `.Input` `.Keyed` `.Lazy` `.Region` `.Responsive` `.Transform` `.Transition`) | hybrid/kernel | **core** | the emphasised primary UI story — a battery the default app needs |
| `Ipe.Ui.Chart` | pure | **package-now** | typed SVG builder; escaped output; not on the primary path |
| `Ipe.Path` | hybrid | **core** | **security primitive** (parse-don't-validate, rejects `..`/NUL) |
| `Ipe.Secret` (+ `.Unsafe`) | native | **core** | capability + secret typing; `zeroize` is native; `.Unsafe` is a disclosed sink |
| `Ipe.Css` | pure | **core** | on the `Ipe.Ui` styling path (primary story) |
| `Ipe.CssSafety` | native | **core** | **security defense**: CSS injection validator, sole constructor of the safe type |
| `Ipe.Io` | hybrid | **core** | capability: stdin/stdout, `readSecret` |
| `Ipe.File` | hybrid | **core** | capability: filesystem (guarded by `Path`) |
| `Ipe.Env` | hybrid | **core** | capability: environment |
| `Ipe.System` | hybrid | **core** | capability: process/system |
| `Ipe.Process` | hybrid | **core** | capability: subprocess |
| `Ipe.Time` | hybrid | **core** | capability: the clock; calendar math is a package candidate |
| `Ipe.Random` (+ `.Generator`) | hybrid | **core** | capability: entropy source |
| `Ipe.Crypto` | native | **core** | capability + defense: RustCrypto primitives (29 aliases) |
| `Ipe.Jwt` | native | **core** | defense: signs/verifies, rides `Crypto` |
| `Ipe.Auth` | native | **core** | capability + defense: auth primitives |
| `Ipe.Http` (+ `.Stream`) | hybrid | **core** | capability: HTTP client |
| `Ipe.Http.Server` (+ `.Stream` `.WebSocket`) | native | **core** | capability: listener |
| `Ipe.Http.Middleware` / `.RateLimit` | native/kernel | **core** | policy over the server capability (RateLimit needs a clock/store) |
| `Ipe.WebSocket` | hybrid | **core** | capability: socket |
| `Ipe.Db` (+ `.Sql` `.Decode` `.Unsafe`) | native/kernel | **core** | capability: connection/exec + `SqlFragment` defense; `.Unsafe` is a disclosed sink |
| `Ipe.Cache` | hybrid | **core** | capability: store |
| `Ipe.PubSub` | native | **core** | capability: the reserved topic-handle + broker seam |
| `Ipe.Web.Console` / `.Head` (+ `.Head.Unsafe`) | hybrid | **core** | capability: browser sinks; `.Unsafe` disclosed |
| `Ipe.Bytes` | native | **core** | vetted codec primitives (11 aliases) — security-sensitive |
| `Ipe.Encoding` | native | **core** | vetted base64/hex/percent codecs — security-sensitive |
| `Ipe.Decimal` | native | **core** | the arbitrary-precision arithmetic primitive `Money` needs |
| `Ipe.Json.Decode` / `.Encode` / `.Decode.Pipeline` | native/kernel | **core** | serde_json is a security-sensitive parser; combinators ride it |
| `Ipe.Codec` | pure | **core** | auto-derive codec combinators on the serialization path |
| `Ipe.Debug` | hybrid | **core** | language dev battery (the log sink is a capability) |
| `Ipe.Trace` | hybrid | **core** | language dev battery |
| `Ipe.Log` | native | **core** | capability: the log sink |
| `Ipe.Test` | hybrid | **core** | language battery: `ipe test` |
| `Ipe.Url` (+ `.Parser`) | hybrid | **core** | host-parsing is a security-sensitive parse on the primary web path |
| `Ipe.Html` (+ `.Attributes` `.Events` `.Unsafe`) | hybrid | **package-now** | Ui-first incentive; binds existing kernels; escaper stays native |
| `Ipe.Money` | hybrid | **package-now** | arithmetic over `Decimal`; currency set is data |
| `Ipe.Csv` | hybrid | **package-now** | parse+format over `String` |
| `Ipe.Markdown` | pure | **package-now** | `String → Html`; output routes through the native escaper |
| `Ipe.Palette` | pure | **package-now** | colour data |
| `Ipe.Locale` | hybrid | **package-now** | data tables |
| `Ipe.Uuid` | native | **package-now** | pure layout over `Random`/`Time`; currently native, re-express as Ipê |
| `Ipe.Bitwise` | hybrid | **package-now** | niche pure bit-ops over existing kernels |
| `Ipe.Compression` | native | **package-later** | gzip/zstd native crate |
| `Ipe.Config` | native | **package-later** | native TOML/YAML/env parsers |
| `Ipe.Email` | native | **package-later** | SMTP send (network capability) |
| `Ipe.Regex` | native | **package-later** | native regex engine |

**Counts:** core ≈ 80 (counting the `Tea.*` and `Ui.*` submodule families and
the `Db`/`Http`/`Json` submodules individually), package-now 8, package-later 4.
The exact core count depends on whether grouped submodule families are counted
as one module or many; by the 99-line `ipe doc --list`, 12 leaves are non-core
(8 now + 4 later) and the remaining 87 are core.

## Migration & sequencing

- **Package-now leaves** move when the packaging ecosystem is usable end-to-end.
  The coordination machinery already exists
  (`docs/adr/0044-package-coordination-manifest-index-gate.md`: manifest, index,
  resolver, lockfile, gate); what gates the move is (a) a hosted index able to
  serve first-party packages and (b) the kernel-bindability decision below.
  *(The brief referenced `namespace-imports-and-packaging-spec.md` and
  `package-coordination-and-capabilities-design.md`; neither exists — ADR 0044
  and this note's links are the real sources. Flagged as an open naming
  question.)*
- **Package-later leaves** wait on the native-package / FFI-to-Rust tier
  (`docs/architecture/ffi-to-rust.md`, `docs/adr/0033-ipe-rust-ffi-subsystem.md`),
  which lets a package declare-and-bind native code under disclosed capabilities.
- **Dogfood as teaching.** Convert examples to `ipe add` the first-party
  packages. That migration *is* the package-authoring and package-consuming
  documentation — the strongest reason to make our own libraries packages.
- **Example / golden churn.** Moving `Ipe.Money` alone touches its call sites
  across the shop example (verified ~18 `Money.*` sites under the tracked example
  tree, concentrated in `13-skyshop`), each of which gains an `ipe add money`
  and an import adjustment. Byte-exact golden re-bless follows mechanically.
  **Golden re-bless cost is not a con** — it is cheap and automated (`ipe`'s
  regen path); weigh only the usefulness of the move.

## Open questions for the maintainer

1. **Kernel bindability from package source.** May a `User`-origin package write
   `Ffi.kernel "Html_render"` against a registered kernel, or is kernel-alias
   binding reserved to `EmbeddedStdlib` origin? This single decision sets whether
   Html / Money / Bitwise / Uuid are movable the day the index ships, or need a
   bindability extension first. (`detect_kernel_alias` in
   `src/compiler/canon/src/resolve.rs` is not itself origin-gated, but `Ipe.Ffi`
   is not exposed in `ipe doc --list`, so the effective gate is import
   visibility — unverified for package source.)
2. The two brief-named packaging spec docs do not exist; confirm ADR 0044 +
   this note are the intended references.
3. Submodule-family counting for "core count" — one module or many?
