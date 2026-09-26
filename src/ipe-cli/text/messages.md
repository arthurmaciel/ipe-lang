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
