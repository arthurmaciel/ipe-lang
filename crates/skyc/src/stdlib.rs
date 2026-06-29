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
}
