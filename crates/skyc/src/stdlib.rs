//! The embedded Sky standard-library source (`Sky.Core.*`).
//!
//! `skyc` is self-contained: the foundational `Sky.Core` modules are compiled
//! into the binary as their original Sky source (a port of the Haskell
//! compiler's Template-Haskell embedding of `sky-stdlib/`). The checked-in copies
//! under `crates/skyc/stdlib/Sky/Core/` are byte-identical to the upstream
//! `sky-stdlib` sources; embedding a copy (rather than `include_str!`-ing an
//! out-of-repo path) keeps the build portable and the toolchain hermetic.
//!
//! M4a embeds the foundational set — `Basics`, `Maybe`, `Result`, `List` — and
//! resolves `Sky.Core.Prelude` to `Basics` (the Prelude re-exports the
//! non-numeric basics, exactly as the reference compiler maps it). M4b adds
//! `Sky.Core.String` and `Sky.Core.Char`. The source is ordinary Sky: the same
//! parser that reads user code reads it (the `parses` test proves it), so it is
//! the substrate the import resolver compiles once whole-program
//! let-generalisation lands.

/// One embedded standard-library module: its dotted name and its Sky source.
pub struct StdModule {
    /// The dotted module name as written in an `import`, e.g. `Sky.Core.Maybe`.
    pub name: &'static str,
    /// The module's Sky source, embedded at compile time.
    pub source: &'static str,
}

/// `Sky.Core.Basics` — `identity` / `always` / `not` / `fst` / `snd` / `clamp`.
const BASICS: &str = include_str!("../stdlib/Sky/Core/Basics.sky");
/// `Sky.Core.Maybe` — combinators over the `Maybe` ADT.
const MAYBE: &str = include_str!("../stdlib/Sky/Core/Maybe.sky");
/// `Sky.Core.Result` — combinators over the `Result` ADT.
const RESULT: &str = include_str!("../stdlib/Sky/Core/Result.sky");
/// `Sky.Core.List` — list combinators.
const LIST: &str = include_str!("../stdlib/Sky/Core/List.sky");
/// `Sky.Core.String` — string combinators (M4b).
const STRING: &str = include_str!("../stdlib/Sky/Core/String.sky");
/// `Sky.Core.Char` — single-character helpers (M4b).
const CHAR: &str = include_str!("../stdlib/Sky/Core/Char.sky");
/// `Sky.Core.Dict` — string-keyed associative map (M4d).
const DICT: &str = include_str!("../stdlib/Sky/Core/Dict.sky");
/// `Sky.Core.Set` — unordered set of unique elements (M4d).
const SET: &str = include_str!("../stdlib/Sky/Core/Set.sky");
/// `Sky.Core.Bytes` — arbitrary byte buffer, distinct from `String` (M4e).
///
/// Divergence from Sky: Sky defines `type alias Bytes = String`; Sky-Rust
/// makes `Bytes` a distinct primitive lowering to `Vec<u8>` (lossless for
/// non-UTF-8 binary). See `docs/architecture/divergence-policy.md`.
const BYTES: &str = include_str!("../stdlib/Sky/Core/Bytes.sky");
/// `Sky.Core.Crypto` — hashes / HMAC / RSA / AEAD / key-derivation / random (M5a).
const CRYPTO: &str = include_str!("../stdlib/Sky/Core/Crypto.sky");
/// `Sky.Core.Task` — Task combinator surface (M5a).
const TASK: &str = include_str!("../stdlib/Sky/Core/Task.sky");
/// `Sky.Core.Io` — standard-I/O effect kernels (M5a).
const IO: &str = include_str!("../stdlib/Sky/Core/Io.sky");
/// `Sky.Core.Time` — time effect kernels (M5a).
const TIME: &str = include_str!("../stdlib/Sky/Core/Time.sky");
/// `Sky.Core.System` — process / environment effect kernels (M5a).
const SYSTEM: &str = include_str!("../stdlib/Sky/Core/System.sky");
/// `Sky.Core.Random` — entropy-backed randomness effect kernels (M5a).
const RANDOM: &str = include_str!("../stdlib/Sky/Core/Random.sky");
/// `Sky.Core.File` — file-system effect kernels (M5a).
const FILE: &str = include_str!("../stdlib/Sky/Core/File.sky");
/// `Sky.Core.Http` — outbound HTTP client kernels + pure builders (M5b).
const HTTP: &str = include_str!("../stdlib/Sky/Core/Http.sky");

/// Every embedded `Sky.Core` module, keyed by its dotted import name.
///
/// `Sky.Core.Prelude` is intentionally absent here: it is not a source file but
/// an alias for `Basics` (the Prelude re-exports the non-numeric basics), so
/// [`source`] maps it onto `Basics` rather than a distinct entry.
pub const MODULES: &[StdModule] = &[
    StdModule {
        name: "Sky.Core.Basics",
        source: BASICS,
    },
    StdModule {
        name: "Sky.Core.Maybe",
        source: MAYBE,
    },
    StdModule {
        name: "Sky.Core.Result",
        source: RESULT,
    },
    StdModule {
        name: "Sky.Core.List",
        source: LIST,
    },
    StdModule {
        name: "Sky.Core.String",
        source: STRING,
    },
    StdModule {
        name: "Sky.Core.Char",
        source: CHAR,
    },
    StdModule {
        name: "Sky.Core.Dict",
        source: DICT,
    },
    StdModule {
        name: "Sky.Core.Set",
        source: SET,
    },
    StdModule {
        name: "Sky.Core.Bytes",
        source: BYTES,
    },
    StdModule {
        name: "Sky.Core.Crypto",
        source: CRYPTO,
    },
    StdModule {
        name: "Sky.Core.Task",
        source: TASK,
    },
    StdModule {
        name: "Sky.Core.Io",
        source: IO,
    },
    StdModule {
        name: "Sky.Core.Time",
        source: TIME,
    },
    StdModule {
        name: "Sky.Core.System",
        source: SYSTEM,
    },
    StdModule {
        name: "Sky.Core.Random",
        source: RANDOM,
    },
    StdModule {
        name: "Sky.Core.File",
        source: FILE,
    },
    StdModule {
        name: "Sky.Core.Http",
        source: HTTP,
    },
];

/// The embedded Sky source for a dotted `Sky.Core` module name, or `None` when
/// the name is not one of the embedded modules.
///
/// `Sky.Core.Prelude` resolves to the `Basics` source (the Prelude is an alias
/// re-export of the non-numeric basics, matching the reference compiler's
/// `("Sky.Core.Prelude", "Basics")` mapping).
#[must_use]
pub fn source(module_name: &str) -> Option<&'static str> {
    if module_name == "Sky.Core.Prelude" {
        return Some(BASICS);
    }
    MODULES
        .iter()
        .find(|m| m.name == module_name)
        .map(|m| m.source)
}

// ===========================================================================
// Compiled-source stdlib modules (#98) — DISJOINT from `MODULES` above.
// ===========================================================================
//
// `MODULES` above is a PARSE-TEST fixture: those `Sky.Core.*` files are shadow
// copies whose real implementations are Rust kernels resolved by qualifier.
// `COMPILED_STD_MODULES` is the opposite: modules that are ACTUALLY compiled
// from Sky source through the ordinary parse → canon → infer → lower → emit
// pipeline (a Std-source module that defines AND pattern-matches its own data
// type — the exact thing a kernel cannot express).
//
// A module is EITHER kernel-qualified (a member of `STDLIB_MODULE_QUALIFIERS`)
// OR compiled-source (here) — never both. `compiled_vs_kernel_qualifier_disjoint`
// enforces that invariant; a name in both would be pre-installed as a kernel
// qualifier AND injected as a source dep, giving ambiguous resolution.

/// One compiled-from-source standard-library module: its dotted name and its
/// embedded Sky source.
pub struct CompiledStdModule {
    /// The dotted module name as written in an `import`, e.g. `Std.Palette`.
    pub dotted: &'static str,
    /// The module's Sky source, embedded at compile time.
    pub source: &'static str,
}

/// `Std.Palette` — the #98 spike: a Std-namespace module that defines `Shade`
/// and pattern-matches its own constructors in `toHex`.
const PALETTE: &str = include_str!("../stdlib/Std/Palette.sky");

/// `Std.Css` (#47) — the typed stylesheet DSL, compiled pure Sky source: it
/// defines AND pattern-matches its own `CssProp` / `CssRule` / `Length` /
/// `Color` / keyword-enum ADTs and folds them to a CSS string.  Its only Rust
/// surface is the four leaf security kernels under the `Sky.Core.CssSafety`
/// kernel qualifier (NOT under `Std.Css`, so the disjointness invariant holds).
const CSS: &str = include_str!("../stdlib/Std/Css.sky");

/// Every compiled-source stdlib module, keyed by its dotted import name.
///
/// Disjoint from [`MODULES`] (parse fixtures) and from `sky_canon`'s
/// `STDLIB_MODULE_QUALIFIERS` (kernel qualifiers) — see the module comment.
pub const COMPILED_STD_MODULES: &[CompiledStdModule] = &[
    CompiledStdModule {
        dotted: "Std.Palette",
        source: PALETTE,
    },
    CompiledStdModule {
        dotted: "Std.Css",
        source: CSS,
    },
];

/// The embedded Sky source for a compiled-source stdlib module named by its path
/// SEGMENTS (e.g. `["Std", "Palette"]`), or `None` when the segments name no
/// compiled-source module.
///
/// Segment-based (rather than `Symbol`-based) so it composes directly with the
/// build driver's `Vec<String>` module paths without an interner round-trip.
#[must_use]
pub fn compiled_std_source_segments(segments: &[String]) -> Option<&'static str> {
    let dotted = segments.join(".");
    COMPILED_STD_MODULES
        .iter()
        .find(|m| m.dotted == dotted)
        .map(|m| m.source)
}

/// Whether `segments` name a compiled-source stdlib module.
#[must_use]
pub fn is_compiled_source_segments(segments: &[String]) -> bool {
    compiled_std_source_segments(segments).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_intern::Interner;

    /// Every embedded `Sky.Core` module must PARSE with the same front end that
    /// reads user code — the proof that the compiler can read its own embedded
    /// standard library (the foundation the import resolver builds on).
    #[test]
    fn every_embedded_module_parses() {
        for m in MODULES {
            let mut interner = Interner::new();
            let parsed = sky_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "embedded module {} must parse: {:?}",
                m.name,
                parsed.err()
            );
        }
    }

    /// The `Sky.Core.Prelude` alias resolves to the `Basics` source.
    #[test]
    fn prelude_aliases_basics() {
        assert_eq!(source("Sky.Core.Prelude"), Some(BASICS));
        assert_eq!(source("Sky.Core.Basics"), Some(BASICS));
    }

    /// An unknown `Sky.Core` module is not embedded.
    #[test]
    fn unknown_module_is_absent() {
        assert_eq!(source("Sky.Core.Nope"), None);
    }

    /// Every compiled-source module must PARSE with the real front end — the
    /// PARSE-DON'T-VALIDATE floor: a module cannot enter any build graph until it
    /// is proven to parse with the same parser that reads user code.
    #[test]
    fn every_compiled_source_module_parses() {
        for m in COMPILED_STD_MODULES {
            let mut interner = Interner::new();
            let parsed = sky_parse::parse_module(m.source, &mut interner);
            assert!(
                parsed.is_ok(),
                "compiled-source module {} must parse: {:?}",
                m.dotted,
                parsed.err()
            );
        }
    }

    /// Load-bearing invariant (design §2.1): a module is EITHER a kernel
    /// qualifier OR a compiled-source module, never both. A name in both would be
    /// pre-installed as a kernel qualifier AND injected as a source dep — an
    /// ambiguous resolution / silent miscompile.
    #[test]
    fn compiled_vs_kernel_qualifier_disjoint() {
        for m in COMPILED_STD_MODULES {
            let segments: Vec<&str> = m.dotted.split('.').collect();
            let clash = sky_canon::STDLIB_MODULE_QUALIFIERS
                .iter()
                .any(|(path, _)| *path == segments.as_slice());
            assert!(
                !clash,
                "{} is BOTH a compiled-source module and a kernel qualifier — \
                 the two tables must be disjoint",
                m.dotted
            );
        }
    }

    /// Segment lookup resolves a compiled-source module and rejects a non-member.
    #[test]
    fn compiled_source_segment_lookup() {
        let palette = vec!["Std".to_owned(), "Palette".to_owned()];
        assert!(is_compiled_source_segments(&palette));
        assert!(compiled_std_source_segments(&palette).is_some());

        let log = vec!["Std".to_owned(), "Log".to_owned()];
        assert!(!is_compiled_source_segments(&log), "Std.Log is a kernel");

        let nope = vec!["Std".to_owned(), "Nope".to_owned()];
        assert!(!is_compiled_source_segments(&nope));
    }
}
