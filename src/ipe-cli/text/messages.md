# ipe messages

Every user-facing message the `ipe` CLI prints that is not a help page. Each
`## <key>` section holds one message; `{name}` marks a value filled in when the
message is shown. Edit the text here, never a Rust string: the catalog tests pin
every section to the Rust declaration that shows it, placeholder for
placeholder.

# Command-line misuse

## command-refusal

ipe {command}: {reason}

## unknown-flag

ipe {command}: unknown flag `{flag}`

## unknown-subcommand

ipe {command}: unknown subcommand `{sub}` (expected {expected})

## unexpected-argument

ipe {command}: unexpected argument `{arg}`

## flag-repeated

ipe {command}: {flag} given more than once

## plain-json-exclusive

ipe {command}: --plain and --json are mutually exclusive

## flag-needs-value

ipe {command}: {flag} needs a value

## unsupported-target

unsupported target `{target}` — supported: wasm, wasi, {supported}

## static-flags-with-wasm

--static / --allocator are native-target flags; they do not compose with --target {target}

## cfree-with-wasm

--cfree is a native-target flag; it does not compose with --target {target}

## emit-ir-with-out

--emit-ir does not compose with --out

## emit-ir-with-static

--emit-ir does not compose with --static

## emit-ir-with-target

--emit-ir does not compose with --target

## emit-ir-with-allocator

--emit-ir does not compose with --allocator

## emit-ir-with-cfree

--emit-ir does not compose with --cfree

## run-wasm-target

ipe run builds and executes a native binary; --target wasm has no native artifact to run — use `ipe build --target wasm` to produce a browser bundle

## run-wasi-native-flags

--static / --allocator / --cfree are native-target flags; they do not compose with --target wasi

## eject-out-required

ipe eject: --out <dir> is required (the directory to write the standalone project to)

## release-no-wasi

ipe release produces a browser bundle (`--target wasm`) or a native binary; it does not produce a WASI module — build one with `ipe build --target wasi`

## release-embed-bundle-exclusive

ipe release: --embed and --bundle are mutually exclusive (embed is the default single self-jailing binary; --bundle is the multi-file opt-out)

## port-zero

ipe {command}: --port 0 is not a real port; omit --port to auto-select a free one

## port-invalid

ipe {command}: --port `{value}` is not a port number (1-65535)

## fix-usage

usage: ipe fix <path> [--yes]

## lsp-takes-no-arguments

ipe lsp takes no arguments

## lint-single-path

ipe lint takes at most one path

## audit-advisory-db-exclusive

ipe package audit: --advisory-db and --no-advisory-db are mutually exclusive

## audit-single-path

ipe package audit: expected a single <path> argument

## audit-format-repeated

ipe package audit: an output-format flag was given more than once

## doc-type-exclusive

ipe doc: --type is mutually exclusive with --check-examples and --list

## doc-type-unexpected-positional

ipe doc --type: unexpected positional argument; use `ipe doc --type "<type expr>"`

## doc-serve-port-needs-number

ipe doc serve: --port needs a number

## doc-single-key

ipe doc: expected a single <key> argument

## doc-single-path

ipe doc: expected a single <path> argument

## ffi-inspector-not-found

ipe add: `ipe-ffi-inspector` not found beside the `ipe` binary or on PATH

## ffi-add-home-unset

ipe add: HOME is not set; cannot create a safe scratch directory

## ffi-no-bubblewrap

ipe add: no bubblewrap isolation available — install `bwrap`, or set IPE_FFI_ALLOW_UNSANDBOXED=1 to accept running the crate's build scripts UNSANDBOXED (dangerous)

## ffi-add-no-payload

ipe add

## install-aborted

ipe install: aborted

## ffi-legacy-define-removed

[[rust.define.*]] is no longer supported — declare FFI types via `foreign` in `src/Ffi/<Crate>.ipe`

## rust-usage

usage: ipe rust <add|remove|install> <crate>[@<version>] [flags]

## rust-add-usage

usage: ipe rust add <crate>[@<version>] [--features a,b] [--yes] [--verbose]

## rust-add-aborted

ipe rust add: aborted

## rust-remove-usage

usage: ipe rust remove <crate>

## rust-install-usage

usage: ipe rust install [--yes] [--allow-build-scripts] [--verbose]

## rust-install-package-ipe-unsupported

ipe rust install: reading `[rust.dependencies]` / `[rust.wrapper]` bindings out of a package.ipe is not yet wired (part of the outstanding ergonomic Rust-FFI work) — the text inspector reads only a legacy ipe.toml

## rust-install-no-manifest

ipe rust install: no manifest with `[rust.dependencies]` in the current directory

## pkg-no-manifest

ipe add/remove: no `package.ipe` in the current directory (run inside an Ipê project)

## publish-single-path

ipe package publish: expected a single <path> argument

## watch-dir-no-manifest

directory supplied but no package.ipe found inside it

## legacy-toml-hint

no package.ipe in this directory (found a legacy ipe.toml — package.ipe is the project manifest the toolchain reads)

## no-entry

nothing to build here — pass a source file or run inside a project (a package.ipe, or a src/Main.ipe)

## internal-entry-not-in-source-map

internal: entry module not in source map

## internal-module-not-in-source-map

internal: module in topo order not in source map

## library-package-no-entry

this is a library package (it declares `exposedModules` and no runnable program) — there is no entry to build. Use `ipe type-check` to verify its public surface, or add a `Package.programs [ … ]` stage to declare a runnable entry

## pkg-not-found-in-dir

no package.ipe found — run inside a project or pass its path

## package-usage

usage: ipe package <audit|audit-entry|publish|validate-entry> [<path>]

## package-validate-entry-usage

usage: ipe package validate-entry <packages/<name>.toml>

## package-audit-entry-single-path

ipe package audit-entry: expected a single entry-file path

## package-audit-entry-usage

usage: ipe package audit-entry <packages/<name>.toml> [--index <root>]

## package-capability-inference-failed

package capability inference: no module in the package could be lowered

## package-manifest-name-required

package.ipe: missing a `name = "…"` field — a package must be named

## package-manifest-src-root-missing

package.ipe: the source root directory does not exist

## package-manifest-no-package-binding

package.ipe: no top-level `package = …` binding found

## package-manifest-no-package-binding-edit

package.ipe: no top-level `package = …` binding to edit

## package-manifest-package-not-record

package.ipe: the `package` value must be a record literal `{ … }` for `ipe add` to edit it

## package-manifest-deps-not-list

package.ipe: `dependencies` must be a list literal `[ … ]` for `ipe add` to edit it

## package-manifest-deps-brace-not-found

package.ipe: could not locate the `package` record's closing `}` to add a dependency

## package-manifest-deps-brace-out-of-range

package.ipe: the `package` record's closing `}` is out of range

## init-shape-needs-value

ipe init: `--shape` requires a value: script, tui, cli, worker, server, web

## diff-usage

usage: ipe diff <old-path> <new-path>
   or: ipe diff check <old-path> <new-path> <old-version> <new-version>

## health-yes-with-format

ipe health: --yes does not compose with --plain / --json (a data form never mutates)

## fmt-single-path

fmt: expected a single <path> argument

## fmt-stdin-and-path

fmt: --stdin and a <path> argument are mutually exclusive

## fmt-format-with-stdin

fmt: --plain / --json do not compose with --stdin (it already writes to stdout)

## fmt-format-needs-check

fmt: --plain / --json report the unformatted files of a --check scan; pass --check

## delivery-word-shadows-path

note: a path `{word}` exists but bare `{word}` selects the delivery; write `./{word}` to build that path

# Help layout

## verbs-label

Verbs:

## help-arguments-label

Arguments

## help-options-label

Options

## help-output-label

Output

# Static-build refusals

## unknown-allocator

unknown allocator {allocator} — expected one of: auto, system, dlmalloc, talc, mimalloc

## unknown-static-target

{target} is not a supported static target — supported: {supported}

## target-requires-static

--target {target} requires --static (cross-compiling a dynamic build is not supported)

## allocator-requires-static

--allocator {allocator} requires --static (allocator selection applies to static builds)

## talc-requires-arena-design

the talc allocator is not wired yet: a hosted talc #[global_allocator] needs a static arena design that has not landed. Use the dlmalloc default instead

## webview-static

an Ipe.WebView app cannot be built --static: it links the system webview (WebKit/WebView2), which has no static form

## target-not-installed

the target {triple} is not installed — run: rustup target add {triple}

## musl-c-compiler-missing

no musl-capable C compiler found for {triple} (the emitted project's zstd/ring dependencies compile C). Install one (Debian/Ubuntu: apt install musl-tools) or set CC_{triple_env}

## mimalloc-requires-c

--allocator {allocator} cannot combine with --cfree: mimalloc vendors and links C. Drop --cfree, or use the pure-Rust dlmalloc default

## libc-allocator-requires-c

--allocator {allocator} cannot combine with --cfree: the target libc's malloc links C. Drop --cfree, or use the pure-Rust dlmalloc default

## cfree-not-yet-wired

--cfree is not wired yet: the pure-Rust dependency swaps that make the default emitted graph link no C (flate2/zstd codecs, a ring-free rustls provider) have not landed, so the build would still pull C. Drop --cfree

## invalid-bool

{source}: expected true/false/1/0, got {value}

# Delivery refusals

## delivery-shape-mismatch

you asked for `{stated}`, but `main` is a `{pinned}` app. A program's shape is fixed by the head of `main` (what `view` renders) — the CLI word only double-checks it. Drop the `{stated}` word, or change `main` to a `{stated}` entry.

## delivery-served-not-a-word

`served` is the default runtime, so it is never written. The web shape runs served (a co-located server loop) unless you opt into `solo` (a self-contained client). Write `web` for served, or `web desktop` for served on the desktop.

## delivery-runtime-on-non-web

`solo` is a web runtime, but this is a `{shape}` app. Only the `web` shape has a runtime choice (served vs solo) — every other shape runs one way. Drop the runtime word.

## delivery-host-on-non-web

`{host}` is a web host, but this is a `{shape}` app. Hosts (desktop/ios/android) belong to the `web` shape's delivery axis; a `{shape}` app has one host. Drop the host word.

## delivery-served-host-not-mobile

`{host}` is a `solo` host, not a served host. Mobile ships a self-contained client (`web solo {host}`); served is the co-located server loop (served or `web desktop`). Write `web solo {host}` for mobile.

## delivery-static-not-allowed-webview

`web desktop` links the system webview at runtime, so it has no static binary. Use `web` (served), `tui`, `cli`, or `script` for a static musl binary, or ship the desktop app bundle.

## delivery-static-not-allowed

`{delivery}` targets wasm or a native bundle, so `--static` (a musl binary) does not apply. `--static` is for the co-located, no-webview shapes: `script`, `tui`, `cli`, or served `web`.

## delivery-unknown-token

`{got}` is not a runtime or host word. The web runtime word is `solo` (served is the default). Hosts are `desktop`, `ios`, `android`. Use `--static` for a musl binary or `--target` for a cross-compile triple.

## delivery-duplicate-token

`{got}` repeats the {kind} — each axis takes exactly one value. Write the {kind} once: e.g. `web solo` (not `web solo solo`) or `web desktop` (not `web desktop ios`). Drop the duplicate `{got}`.

## delivery-solo-requires-wasm-target

a `solo` delivery is a self-contained client that must compile to wasm, but the target resolved to native. The sandbox's native-deny guards are keyed to the wasm target, so a native `solo` build would ship native effects into the sandbox. Build for wasm — pass `--target wasm`, set `IPE_TARGET=wasm`, or set `[wasm] mode` in `package.ipe` — or drop `solo` for a co-located served delivery.

## delivery-wasm-target-requires-solo

a wasm compile target was requested, but the delivery is not `solo`. The wasm client target exists only to carry a self-contained `solo` app; every other shape has no wasm form. Deliver `web solo` to build for wasm, or drop the wasm target (`--target`/`IPE_TARGET`/`[wasm] mode`) for a native build.

## delivery-native-engine-refuses-wasm-triple

`{triple}` is a WebAssembly triple, but this build targets the native binary, which has no WASM form. The browser client compiles to `wasm32-unknown-unknown` (deliver `web solo`); the co-located WASI target compiles to `wasm32-wasip1`. Drop the WASM triple for a native build, or pick the delivery that carries it.

## delivery-solo-requires-browser-triple

a `web solo` client compiles only to `wasm32-unknown-unknown`, but `{triple}` was requested. The sandboxed browser client has exactly one triple — its wasm sandbox. Drop the triple (it is implied by `solo`), or drop `solo` for the delivery that carries `{triple}`.

## delivery-solo-refuses-wasi-triple

a `web solo` client cannot target `wasm32-wasip1`. The browser sandbox denies native effects and reaches the world only through Web-API capabilities; WASI is the co-located, native-ish target for a `tui`/`cli`/`script`/served-`web` program, never the browser sandbox. Deliver `web solo` to `wasm32-unknown-unknown`, or use a co-located shape for a WASI build.

## delivery-webview-has-no-static-triple

`{delivery}` links the system webview at runtime, so it has no static (musl) triple. Use `web` (served-live), `tui`, `cli`, or `script` for a static musl binary, or ship the desktop app bundle.

## delivery-wasi-refuses-solo-delivery

a co-located `wasm32-wasip1` build cannot carry a `solo` delivery. `solo` is the browser sandbox (`wasm32-unknown-unknown`), which denies native effects; WASI is the co-located, native-ish target that runs a script's own effect floor. Drop `solo` for a WASI build, or deliver `web solo` to the browser triple.

## delivery-wasi-requires-direct-shape

a co-located `wasm32-wasip1` build carries only a `Direct` script (a plain `Task Error ()` `main`), but this is a `{shape}` app. A `tui`/`cli`/`web` TEA loop needs the reactor spine, which does not build on WASI. Build the `{shape}` app natively, or ship a `Direct` script to `wasm32-wasip1`.

## delivery-wasi-requires-wasi-triple

a co-located WASI build compiles only to `wasm32-wasip1`, but `{triple}` was requested. The WASI engine has exactly one triple — its portable target. Drop the triple (it is implied by the WASI build), or pick the delivery that carries `{triple}`.

# Driver errors

## cli-static-refusal

static build refused: {refusal}

## cli-runtime-not-found

could not locate the Ipe runtime; set IPE_RUNTIME_DIR to an explicit path or pass --runtime <dir>

## cli-runtime-dir-invalid

IPE_RUNTIME_DIR points at {path}, which is not an Ipe runtime crate root (its Cargo.toml must declare `name = "ipe-runtime-rust"`)

## cli-runtime-dir-invalid-inner-hint

  = help: this looks like the inner runtime module directory; point IPE_RUNTIME_DIR at the crate root that holds Cargo.toml (e.g. `src/runtime/rust`), not the `src/ipe_runtime` inside it

## cli-runtime-home-unknown

could not determine where to install the Ipe runtime: none of IPE_HOME, XDG_DATA_HOME, or HOME is set; set IPE_HOME to a writable directory

## cli-runtime-materialize-failed

could not install the Ipe runtime: {detail}
  the build was stopped rather than link an incomplete runtime

## cli-runtime-version-mismatch

the Ipe runtime at {path} is version {found}, but this compiler is {expected}; a program emitted by this compiler cannot link a different runtime.
  = help: this runtime is out of date. Remove the stale copy (the project's `out/` directory, or whatever `IPE_RUNTIME_DIR` points at) and rebuild — the matching runtime re-materializes automatically.

## cli-emitted-build-feature-missing

building {what} failed: it needs the runtime feature `{feature}`

## cli-emitted-build-feature-context

, but the runtime at {root} (version {version}) does not provide it

## cli-emitted-build-stale-runtime-hint

  = help: the runtime is out of date. Remove the stale copy (the project's `out/` directory, or whatever `IPE_RUNTIME_DIR` points at) and rebuild — the matching runtime re-materializes automatically.

## cli-cargo-fetch-failed

cargo exited {code} while fetching crates for {what}

## cli-cargo-fetch-failed-detail

cargo exited {code} while fetching crates for {what}:
{trimmed}

## cli-cargo-compile-failed

cargo exited {code} with no output while compiling {what}

## cli-cargo-compile-failed-detail

cargo exited {code} while compiling {what}:
{trimmed}

## cli-capability-mismatch-header

declared capabilities do not match the program's inferred set

## cli-capability-mismatch-missing

  used but not declared: {list}

## cli-capability-mismatch-extra

  declared but not used: {list}

## cli-hash-mismatch

package `{package}`: content hash mismatch — the fetched source does not match the hash the index pinned.
  expected: {expected}
  actual:   {actual}
the source was NOT trusted; nothing was written.

## cli-doc-not-found

no documentation entry is named `{query}`

## cli-doc-suggestions-header

closest matches:

## cli-doc-suggestion-line

  ipe doc {key}  — {title} ({kind})

## cli-unknown-code

unknown error code `{input}`

## cli-unknown-code-did-you-mean

  did you mean: {first}

## cli-semver-rejected

version {proposed} does not clear the required {required} bump — the new version must be at least {floor}.

## cli-publish-refused

ipe package publish refused: {refusal}

## cli-unknown-group-verb

unknown `ipe {group}` verb `{attempted}`

## cli-unknown-group-suggestion

= help: maybe `ipe {group} {sugg}`?

## cli-verify-failed

verify: the {stage} stage failed

## cli-test-failed-suffix

one or more tests failed (runner exited {code})

## cli-upgrade-no-prebuilt

{glyph} No prebuilt binary for {version} on {platform}.
    Possibly the binaries for that version are still being generated.
    If you prefer, build from source:
        cargo install --git https://github.com/arthurmaciel/ipe-lang ipe

## cli-health-critical

health: a required prerequisite is missing (see the report above)

## cli-eject-unsupported

eject: {reason}

## cli-lint-gate-failed

lint: findings remain at or above the gate severity (see above)

## cli-file-too-large

{path}: file exceeds the {max}-byte read ceiling — refusing to allocate an unbounded buffer

## cli-path-escape

manifest path {raw} was rejected: {reason}

## cli-output-refused

output directory refused: {refusal}

## cli-discovery-limit-reached

module-discovery walk aborted: {detail}

## cli-advisory-vulnerable

dependency `{package}` v{version} is affected by {severity}-severity advisory {id}:
  {description}{fixed_in}

## cli-advisory-fixed-in

  Fixed in: {v}

## cli-advisory-db-unreachable

advisory database is unreachable — refusing to treat the dep as safe:
  {detail}

## cli-advisory-db-malformed

advisory file {path} is malformed — refusing to treat the dep as safe:
  {detail}

## cli-wasi-run-feature-disabled

ipe run --target wasi needs the embedded wasmtime engine, but this `ipe` binary was built without the `wasi_run` feature.
  = help: build the module with `ipe build --target wasi` and run it under a WASI runtime, or reinstall an `ipe` compiled with `--features wasi_run` (the default in release packaging).

## cli-wasi-run-failed

ipe run --target wasi: the emitted wasm32-wasip1 module could not be run under the embedded wasmtime engine — {detail}

## cli-wasi-run-exited

the wasm32-wasip1 module exited with code {code}

## cli-unknown-command-line

unknown command `{attempted}`

## cli-unknown-command-suggestion

= help: maybe `{sugg}`?

## cli-io-not-found

no such file `{path}` — pass a source file, or run inside an Ipê project (a directory with a package.ipe, or a src/Main.ipe)

## cli-io-other

could not access `{path}` — {kind}

# Publish refusals

## publish-dirty-tree

the working tree at {source_root} has uncommitted changes — publish pins the exact committed revision, so commit (or stash) every change first; otherwise the pinned `sha256`/`rev` would not name the bytes you publish.

## publish-unpushed-head

HEAD ({rev}) is not reachable from any remote branch — a published version pins an immutable, fetchable revision, so push this commit to its remote before publishing.

## publish-duplicate-version

`{name}` {version} is already published in the index — a published version is immutable and must never be rewritten. Bump the version in `package.ipe` and publish the new one.

## publish-no-source

could not determine the package's source URL — the index needs a public git URL the resolver can fetch. Pass `--source <url>`, or set an `origin` remote on the package's git repository.

## publish-unsigned-commit

no commit-signing key is configured, so the publish commit could only be pushed unsigned — the curated index requires signed commits and would never merge it, so nothing was published. Set `IPE_PUBLISH_SIGNING_KEY` to the path of an SSH signing key (the private key file; its `.pub` must be registered as a signing key on your GitHub account) and publish again.

## publish-unresolvable-identity

could not resolve your GitHub identity for the index-PR commit — the curated index requires signed commits marked "Verified", which is only possible when the commit's committer is your authenticated GitHub account's verified noreply identity. Run `ipe login` so publish can sign the index PR under your verified GitHub identity, then publish again. Nothing was published.

# Documentation site

The labels of the generated documentation site (`ipe doc`). A label is plain
text, escaped where it lands; an entry that holds HTML tags is inserted as written.

## site-skip-link

Skip to content

## site-nav-label

Site

## site-title-full

Ipê language documentation

## site-title-short

Ipê language docs

## site-menu-label

Menu

## site-guides

Guides

## site-topics

Topics

## site-idioms

Idioms

## site-constructs

Constructs

## site-reference

Reference

## site-diagnostics

Diagnostics

## site-cli

CLI

## site-documentation

Documentation

## site-search-placeholder

Search…

## site-search-label

Search documentation

## site-search-results-label

Search results

## site-theme-toggle-label

Toggle light and dark theme

## site-scroll-top-label

Scroll to top

## site-filter-modules

Filter modules…

## site-filter-modules-label

Filter modules

## site-project-modules

Project modules

## site-standard-library

Standard library

## site-types

Types

## site-values

Values

## site-no-documentation

No further documentation yet.

## site-reference-fallback

See <a href="module/index.html">Reference</a> for the full API.

## site-code-families-intro

Every code reads <code>IPE-</code>, a family letter, and four digits. The letter names the part of the compiler that reports it:
