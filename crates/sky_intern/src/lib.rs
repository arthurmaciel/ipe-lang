#![forbid(unsafe_code)]
use std::collections::HashMap;

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
}

#[cfg(test)]
mod tests {
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
}
