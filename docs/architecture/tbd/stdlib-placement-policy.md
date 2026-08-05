# Stdlib placement policy — per-module application

> This is the per-module application of the decision in
> `docs/adr/0057-stdlib-placement.md`. The ADR fixes the four-way line
> (core intrinsic / native / Ipê package) and the security-defense carve-out;
> this note walks the concrete stdlib module by module against it.

## The problem, in one example

Adding a currency to `Ipe.Money` today requires **rebuilding the `ipe`
binary**. The `Money`/`Currency` ADTs and the pure combinators live in Ipê
source (`src/stdlib/Ipe/Money.ipe`), but the `Currency` enum is a hardcoded
closed ADT and the currency **property table** (minor units, symbol, name,
formatting) is a Rust `match` in `money.rs::lookup_currency`. Both are baked in
— the `.ipe` via `include_str!`, the table compiled into the runtime. A
currency is *data*; nothing about it touches the OS or needs a crate. Baking it
into the compiler is debt from prioritising byte-exact runtime parity and raw
performance over modularity.

The same pattern repeats across the stdlib: computation and data that could be
ordinary Ipê are wired as native kernels, so the audited native surface (and the
attack surface, and the recompile cost) is larger than it needs to be.

## The principle

Draw the line at **capability / security-defense / perf vs. cold computation**:

- A module is a **capability** if it touches the outside world (OS, network,
  filesystem, clock, entropy, a foreign library) or needs a vetted native crate
  or genuine performance. Capabilities are native Rust, feature-gated, and are
  exactly the surface the capability/trust model must gate.
- A function is a **security defense** if its *correctness is itself a security
  property* — escapers/sanitizers, injection/traversal validators, crypto.
  Even though it is pure computation, its failure mode is a vulnerability, not a
  wrong value, so it stays **native, vetted, and non-overridable**. The
  idiomatic shape is a native validator as the *sole constructor of an opaque
  safe type*: the validator carries the safety outward, so downstream Ipê code
  that consumes the safe value is safe by construction without itself being
  security code. This carve-out **overrides** the "cold computation → package"
  pull below — a defense is a trusted component to keep small and directly
  auditable, not attack surface to be shrunk by moving it up.
- Everything else is **cold computation or data** — value transformations
  expressible in Ipê. It should be a pure-Ipê library, ideally a
  distributable/overridable package, changeable without rebuilding `ipe`.

**Rule for new modules:** author a module in pure Ipê by default. Reach for a
native kernel only when it is a capability, a security defense, or a measured
performance primitive, and keep that kernel as small as the concern itself —
push data and logic up into the Ipê layer. (Money's kernel should be *Decimal
arithmetic*; the currency set, formatting, and `allocate` belong in Ipê.)

## Three destinations

| Destination | What it is | Cost to change/add |
|---|---|---|
| **Core** | Compiler/runtime intrinsics everything depends on | Rebuild `ipe` (rare, by nature) |
| **Native** | A capability, a security defense, a vetted parser, or a perf primitive; feature-gated, capability-gated; extractable into its own module tree | Rebuild the native tree (`ipe` rebuild today; a standalone runtime tree under the S3 embed→materialize model tomorrow) |
| **Ipê package** | Pure cold computation/data in Ipê source | Recompile only the *user's* project — and with auto-import + DCE + a materialised/packaged stdlib, no special build; a third party can publish or override it |

The reduction goal is served twice: a pure-Ipê module pulls **no native code**
beyond whatever kernel it already calls, and every kernel we *don't* write
shrinks the compiler and the audited/attack surface (Security first).

## Worked example — `Html` splits within one module

The surface-reduction argument inverts *inside* a single module:

- The `Html` / `Html.Attributes` / `Html.Events` **constructors** only assemble
  `Element`/`Attribute` values → **Ipê package**. Moving them up shrinks the
  native surface.
- The **serialiser** (`Html.render` + the `escapeText`/`escapeAttr` escapers) is
  the XSS injection barrier → **native**. Moving the escaper up would enlarge the
  trusted computing base for the escaper and make it harder to audit and fuzz.

Escaping is concentrated in the one native serialiser, so the assembled tree is
just data and only rendering is a defense — the same module holds both a package
half and a native half.

## Per-module classification

"Today" = `kernel` (native, no `.ipe`), `hybrid` (`.ipe` wrapper over kernels),
or `pure` (`.ipe`, kernel-free). "→" = target destination.

### Core — stays intrinsic

| Module | Today | Note |
|---|---|---|
| `Basics` | hybrid | operators + core types; the irreducible base |
| TEA reactor engine | kernel | the event loop itself (the `Cmd`/`Sub` *sugar* is a package — below) |

### Native — capability (world-touching, security-gated)

| Module | Today | → | Note |
|---|---|---|---|
| `Io` | hybrid | native | stdin/stdout, `readSecret` (termios) |
| `File` | hybrid | native | filesystem |
| `Process`, `System`, `Env` | hybrid | native | subprocess, env, `.env` |
| `Http` (client) | hybrid | native | reqwest |
| `Http.Server` | kernel | native | axum listener |
| `Db` (open/exec/tx) | kernel | native | sqlx connection + execution |
| `Crypto` | kernel | native | RustCrypto primitives |
| `Random` (entropy) | hybrid | native | `getrandom` — the *entropy source* only |
| `Time` (clock) | hybrid | native | `Time.now` is the capability; calendar math is a package |
| `WebSocket` | hybrid | native | tokio-tungstenite |
| `Email` | hybrid | native | lettre SMTP |
| `Cache` | hybrid | native | redis / store |
| `Config` (file read) | hybrid | native | reads files/env; the *decoders* are a package |
| `Web.Console`, `Web.Head` | hybrid | native | browser sinks |
| `Jwt` | kernel | native | signs/verifies — rides Crypto |
| `Secret` | kernel | native | thin, but `zeroize` is a native concern; borderline |
| `Ffi` | kernel | core/native | the foreign boundary itself |

### Native — security defense / perf / vetted parser (kept native for correctness-as-security or speed)

| Module | Today | → | Note |
|---|---|---|---|
| `CssSafety` | kernel | native | **security defense**: a CSS injection/traversal validator; its correctness is a security property. The sole constructor of the opaque safe CSS type — non-overridable, vetted, fuzzable. Stays native even though it is pure computation |
| `Decimal` | kernel | native | `rust_decimal` arbitrary precision — the one primitive `Money` needs |
| `Regex` | hybrid | native | `regex` crate |
| `Json` (parse/serialise core) | kernel | native | serde_json — a security-sensitive parser; the *combinators* are a package |
| `Encoding`, `Bytes` | kernel | native | base64/hex/percent codecs |
| `Char` (category half) | hybrid | native | `unicode-general-category`; the ASCII/std half is pure |
| `Compression` | hybrid | native | flate/zstd algorithms |
| `Csv` (parse) | hybrid | native | the `csv` parser; *formatting* is pure |
| `String`, `List`, `Dict`, `Set`, `Math` (hot core) | hybrid | native | native for throughput; many ops are pure-able if perf allows |
| `Html.render` + escapers | kernel | native | **security defense**: the XSS escape barrier; see the worked example above |

### Ipê package — pure cold computation/data (no recompile; distributable/overridable)

| Module | Today | → | Note |
|---|---|---|---|
| `Maybe`, `Result`, `Tuple` | pure/kernel | see note | the ADTs stay **core**; the combinators are hot/ubiquitous (they thread through every success/happy path) and stay **native until benchmarked**. Only *cold* formatting/display helpers are package candidates |
| `Error` | kernel | see note | the data type stays core; construction is happy-path and stays native until benchmarked; only cold formatting/display helpers are package candidates |
| **`Money`** | hybrid | **package** | **worked example**: currency set + property table as Ipê *data*, arithmetic/format/`allocate` pure over the `Decimal` kernel |
| `Uuid` | kernel | package | v4/v7 = format over the `Random`/`Time` kernels; the layout is pure |
| `Json.Decode`, `Json.Encode`, `Json.Decode.Pipeline` | kernel | package | decoder/encoder *combinators* over the Json parse kernel |
| `Db.Decode`, `Db.Sql` | kernel | package | row-decoder combinators + the SQL-fragment builder — pure over the `Db` capability |
| `Url`, `Url.Parser` | hybrid | package | parsing/printing over `String` |
| `Ui`, `Ui.*` | hybrid/kernel | package | view builders that produce `Html` values |
| `Html`, `Html.Attributes`, `Html.Events` (constructors) | kernel | package | data constructors; the serialiser/escaper is native (above) |
| `Markdown` | hybrid | package (serialiser native) | `String → Html`. **Hidden-defense trap**: its *output* is HTML, so it must go through the native `Html` escaper. A Markdown renderer that emits raw HTML is a defense concern — the combinator/parse API is package, but the HTML serialiser stays native and non-raw |
| `Locale` | hybrid | package | data tables |
| `Palette` | pure | package | colour data |
| `ToString` | pure | package | pure formatting |
| `Bitwise` | pure | package | pure int ops (small; native only if measured) |
| `Csv` (format) | hybrid | package | printing half |
| `Tea.*.Cmd`, `Tea.*.Sub`, `PubSub` | kernel | package | the *sugar* over the reactor capability |
| `Http.Middleware`, `Http.RateLimit` | kernel | package | policy combinators (RateLimit needs a clock/store — thin capability dep) |
| `Debug`, `Trace` | hybrid | package | dev-only; the logging *sink* is a capability, the API is pure |
| `Path` | hybrid | package | path string manipulation (existence checks are the `File` capability). **Hidden-defense check**: traversal safety (`../` escapes) is a security property — if a validator constructs a safe-path type, that validator stays native per the carve-out; the plain string manipulation is package |

### The eight incoming parity-gap modules — author in the right bucket from day one

| Module | Bucket | Why |
|---|---|---|
| `Codec` | package | auto-derive JSON codec *combinators* — pure |
| `Db.Store`, `Db.Table` | package | typed query/record builders over the `Db` capability |
| `Db.Schema` | package | DDL builder — pure until it executes (that's `Db`) |
| `Db.Migrate` | native/hybrid | needs `Db.exec` + ordering state — thin capability wrapper over a pure plan |
| `Time` calendar (~60 fns) | package | `addDays`/`addMonths`/`startOfMonth`/… are pure over the `Time` kernel's instant |
| `Analytics` | hybrid | a pure event/consent API over an `Http`/`Io` sink capability |
| `Jobs` | native/hybrid | a background queue needs the reactor/store capability; the job *description* is data |
| `Cli` | package | argument/command combinators over the `Program`/`Io` capability |

The lesson from Money: **don't reflexively kernel-wire these**. Most are
combinators or data; only the capability seam (execute a migration, enqueue a
job, push an event) is native, and it should be as thin as the capability.

## Modules misclassified today (native, should be data/pure)

- **`Money` currency table** — `money.rs::lookup_currency` should be Ipê data.
- **`Uuid` formatting** — layout/printing over the `Random`/`Time` kernels.
- **`Json`/`Db` decoder combinators** — the parse/exec *core* is native; the
  combinator layer (`andThen`/`map`/`pipeline`) is pure and can move up.
- **`Locale`/`Palette` data** — any hardcoded-in-Rust tables should be Ipê data.
- (A precise audit — which `Ffi.kernel` calls are true capabilities/defenses vs.
  computation dressed as kernels — is itself the first task this note motivates.)

## What makes "no recompile" real

Three mechanisms already on the roadmap turn this classification into the actual
"add a currency without rebuilding `ipe`" behaviour:

1. **Materialise the stdlib source** (the S3 runtime embed→`~/.ipe/…` model,
   instead of `include_str!`-baking): pure-Ipê stdlib compiles with the *user's*
   project, editable/overridable without a compiler build.
2. **Auto-import + DCE** (macro-roadmap): unused pure-Ipê stdlib costs nothing
   in the emitted binary, so a large library layer is free. Until DCE is
   free-for-unused, each `.ipe` move pays emitted-binary size, so gate each move
   on a size check or sequence it after DCE.
3. **Decentralised packages + capability inference**: currencies (or a whole
   `Money`) become an overridable package; the capability model gates exactly
   the native seam, which the kernel/library split makes explicit.

See also `misc/runtime-modularization-design.md`,
`misc/disentanglement-opportunities.md`, and the packaging/capability design.

## Caveats / non-goals

- The perf line is not "all computation → Ipê". `String`/`List`/`Dict` hot paths
  stay native until a benchmark says otherwise; this note moves *cold*
  computation and *data*, not throughput primitives. The `Maybe`/`Result`/
  `Tuple`/`Error` combinators are hot/happy-path and likewise stay native until
  benchmarked; their types stay core.
- Security-defense functions (escapers/sanitizers, `CssSafety`, injection/
  traversal validators, `Json`/`Url` host parsing, codecs, crypto) stay native
  and vetted even though they are "computation" — their correctness is a
  security property.
- This is a direction, not a big-bang rewrite: apply it to the eight new modules
  first (author them right), reclassify `Money`'s currency table as the first
  retrofit, and let the materialise/DCE/packaging mechanisms land underneath.
