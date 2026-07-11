#![forbid(unsafe_code)]
use std::collections::{BTreeSet, HashMap};

use sky_diagnostics::{DResult, Diagnostic};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Symbol(u32);

impl Symbol {
    /// Construct a `Symbol` from a raw index.
    ///
    /// # Invariant
    ///
    /// `n` MUST be an index previously handed out by an [`Interner::intern`]
    /// call on the *same* interner. A raw value that was never interned (a
    /// forged or cross-interner symbol) resolves to `None`, NOT to a silent
    /// empty string — see [`Interner::resolve`]. Prefer `intern` for any
    /// symbol you intend to resolve later; reach for `from_raw` only for
    /// stable sentinel encodings where the invariant is locally obvious.
    #[must_use]
    pub const fn from_raw(n: u32) -> Self {
        Self(n)
    }
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

#[derive(Default)]
pub struct Interner {
    map: HashMap<String, Symbol>,
    strings: Vec<String>,
    /// When set, [`Self::fresh_symbols`] avoids exactly the names in this set
    /// instead of every already-interned string — see the incremental-build
    /// determinism note on that method. Set per build via
    /// [`Self::set_fresh_avoid`]; `None` preserves the historical
    /// whole-table behaviour for callers that own a per-build interner.
    fresh_avoid: Option<BTreeSet<String>>,
}

impl Interner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern a string, returning its stable [`Symbol`].
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when the symbol table is exhausted
    /// — i.e. when `u32::MAX` distinct identifiers have already been interned.
    /// This guards against silently aliasing a new identifier onto
    /// `u32::MAX` (the saturating bug the old `unwrap_or` hid).
    pub fn intern(&mut self, s: &str) -> DResult<Symbol> {
        if let Some(&sym) = self.map.get(s) {
            return Ok(sym);
        }
        let id = u32::try_from(self.strings.len()).map_err(|_| Diagnostic::CompilerBug {
            where_: "intern",
            detail: "symbol table exhausted (u32::MAX identifiers)".to_owned(),
        })?;
        let sym = Symbol(id);
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), sym);
        Ok(sym)
    }

    /// Mint `count` fresh symbols whose resolved strings are guaranteed to be
    /// distinct from one another *and* from every user identifier.
    ///
    /// Each name is `<prefix><n>` for the smallest run of `n` (from `0`) whose
    /// candidate does not collide, so the result can never alias a user
    /// identifier nor a previously minted fresh symbol of the same pool. The
    /// lowerer uses this for eta-expansion parameter names, which must not
    /// capture any name free in the supplied arguments.
    ///
    /// **Collision universe** — two modes:
    ///
    /// - [`Self::set_fresh_avoid`] set (the incremental driver path): a
    ///   candidate collides IFF it is in the avoid set — the identifier words
    ///   of the *current program*, a pure function of the build's source
    ///   inputs. This keeps the minted names deterministic on a **warm**
    ///   (reused) interner: names minted by a previous build's lowering are
    ///   NOT collisions (re-minting `eta_0` returns the same append-only
    ///   symbol), so a rebuild emits the same names a cold build would —
    ///   proven by the Task-18 clean-vs-incremental parity gate.
    /// - unset (per-build interners: unit tests, direct embedders): the
    ///   historical whole-table behaviour — any interned string collides.
    ///   Equivalent to the avoid-set mode on a cold interner, where every
    ///   interned string came from this build.
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when the `u32` suffix space is
    /// exhausted before `count` fresh names are found, or when [`Self::intern`]
    /// itself reports the symbol table full — both unreachable for any real
    /// program.
    pub fn fresh_symbols(&mut self, prefix: &str, count: usize) -> DResult<Vec<Symbol>> {
        let mut out = Vec::with_capacity(count);
        let mut n: u32 = 0;
        while out.len() < count {
            let candidate = format!("{prefix}{n}");
            let collides = match &self.fresh_avoid {
                Some(avoid) => avoid.contains(&candidate),
                None => self.map.contains_key(&candidate),
            };
            if !collides {
                out.push(self.intern(&candidate)?);
            }
            n = n.checked_add(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "intern.fresh_symbols",
                detail: "exhausted u32 suffix space minting fresh symbols".to_owned(),
            })?;
        }
        Ok(out)
    }

    /// Set the fresh-name collision universe for the CURRENT build: the set
    /// of identifier words appearing in the program's source text (the
    /// driver computes it as a pure function of the build inputs — see
    /// `sky_db::identifier_words`). Overwrites any previous build's set.
    ///
    /// Must over-approximate the program's user identifiers: extra entries
    /// only skip more candidate names (sound); a MISSING user identifier
    /// could let a minted name capture it (unsound) — which is why callers
    /// derive the set from a total scan of the source text rather than a
    /// per-AST-node walker that could silently under-approximate.
    pub fn set_fresh_avoid(&mut self, avoid: BTreeSet<String>) {
        self.fresh_avoid = Some(avoid);
    }

    /// Resolve a [`Symbol`] to its interned string.
    ///
    /// Returns `None` for a symbol this interner never handed out (a forged or
    /// cross-interner value) instead of a silent empty string — the caller
    /// decides whether that `None` is a genuine absence or an impossible
    /// invariant violation.
    #[must_use]
    pub fn resolve(&self, sym: Symbol) -> Option<&str> {
        self.strings.get(sym.0 as usize).map(String::as_str)
    }

    /// Whether `s` has already been interned, i.e. `intern(s)` would return an
    /// existing [`Symbol`] rather than minting a new one.
    ///
    /// Lets a caller mint a synthesized name (e.g. a generalized type
    /// variable's `"a"`, `"b"`, …) that is guaranteed not to alias a real user
    /// identifier, without provisionally interning-then-discarding candidates.
    #[must_use]
    pub fn contains(&self, s: &str) -> bool {
        self.map.contains_key(s)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    #[test]
    fn intern_dedups_and_resolves() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        let a = i.intern("Increment")?;
        let b = i.intern("Increment")?;
        let c = i.intern("Decrement")?;
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), Some("Increment"));
        assert_eq!(i.resolve(c), Some("Decrement"));
        Ok(())
    }
    #[test]
    fn resolve_unknown_is_none_not_panic() {
        let i = Interner::new();
        assert_eq!(i.resolve(Symbol::from_raw(999)), None);
    }

    #[test]
    fn fresh_symbols_are_distinct_and_named_in_order() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        let pool = i.fresh_symbols("eta_", 3)?;
        let names: Vec<Option<&str>> = pool.iter().map(|&s| i.resolve(s)).collect();
        assert_eq!(
            names,
            vec![Some("eta_0"), Some("eta_1"), Some("eta_2")],
            "named by ascending suffix"
        );
        // Distinct symbols.
        let unique: BTreeSet<Symbol> = pool.iter().copied().collect();
        assert_eq!(unique.len(), pool.len(), "every minted symbol is distinct");
        Ok(())
    }

    #[test]
    fn fresh_symbols_skip_already_interned_collisions() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        // A user identifier that would collide with the first candidate.
        let user = i.intern("eta_0")?;
        let pool = i.fresh_symbols("eta_", 2)?;
        // The minted names step over the pre-existing `eta_0`.
        let names: Vec<Option<&str>> = pool.iter().map(|&s| i.resolve(s)).collect();
        assert_eq!(names, vec![Some("eta_1"), Some("eta_2")]);
        assert!(
            pool.iter().all(|&s| s != user),
            "a fresh symbol never aliases a user name"
        );
        Ok(())
    }

    #[test]
    fn fresh_symbols_zero_count_is_empty() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        assert!(i.fresh_symbols("eta_", 0)?.is_empty());
        Ok(())
    }

    /// Avoid-set mode: re-minting a pool on a WARM interner (previous build's
    /// pool names already interned) yields the SAME names — the previous
    /// build's synthetic names are not collisions, only the program's
    /// identifier words are. This is the Task-18 warm/cold byte-parity
    /// property at the interner level.
    #[test]
    fn fresh_symbols_avoid_set_is_stable_across_rebuilds() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        i.set_fresh_avoid(BTreeSet::new());
        let run1 = i.fresh_symbols("eta_", 2)?;
        let run2 = i.fresh_symbols("eta_", 2)?;
        assert_eq!(run1, run2, "warm re-mint returns the same symbols");
        let names: Vec<Option<&str>> = run2.iter().map(|&s| i.resolve(s)).collect();
        assert_eq!(names, vec![Some("eta_0"), Some("eta_1")]);
        Ok(())
    }

    /// Avoid-set mode still dodges the program's user identifiers.
    #[test]
    fn fresh_symbols_avoid_set_dodges_user_identifiers() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        let user = i.intern("eta_0")?;
        i.set_fresh_avoid(BTreeSet::from(["eta_0".to_owned()]));
        let pool = i.fresh_symbols("eta_", 2)?;
        let names: Vec<Option<&str>> = pool.iter().map(|&s| i.resolve(s)).collect();
        assert_eq!(names, vec![Some("eta_1"), Some("eta_2")]);
        assert!(
            pool.iter().all(|&s| s != user),
            "a fresh symbol never aliases a user name"
        );
        Ok(())
    }

    /// A later build's avoid set replaces the earlier one: a user identifier
    /// REMOVED from the program frees its candidate again (cold-build
    /// equivalence — the collision universe is per build, not sticky).
    #[test]
    fn fresh_symbols_avoid_set_is_per_build_not_sticky() -> sky_diagnostics::DResult<()> {
        let mut i = Interner::new();
        i.set_fresh_avoid(BTreeSet::from(["eta_0".to_owned()]));
        let run1 = i.fresh_symbols("eta_", 1)?;
        assert_eq!(
            run1.first().and_then(|&s| i.resolve(s)),
            Some("eta_1"),
            "avoided while the program names eta_0"
        );
        // Next build: the program no longer names `eta_0`.
        i.set_fresh_avoid(BTreeSet::new());
        let run2 = i.fresh_symbols("eta_", 1)?;
        assert_eq!(
            run2.first().and_then(|&s| i.resolve(s)),
            Some("eta_0"),
            "freed once the program stops naming it (cold-build equivalence)"
        );
        Ok(())
    }
}
