#![forbid(unsafe_code)]
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Symbol(u32);

impl Symbol {
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

    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let id = u32::try_from(self.strings.len()).unwrap_or(u32::MAX);
        let sym = Symbol(id);
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), sym);
        sym
    }

    #[must_use]
    pub fn resolve(&self, sym: Symbol) -> &str {
        self.strings.get(sym.0 as usize).map_or("", String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intern_dedups_and_resolves() {
        let mut i = Interner::new();
        let a = i.intern("Increment");
        let b = i.intern("Increment");
        let c = i.intern("Decrement");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "Increment");
        assert_eq!(i.resolve(c), "Decrement");
    }
    #[test]
    fn resolve_unknown_is_empty_not_panic() {
        let i = Interner::new();
        assert_eq!(i.resolve(Symbol::from_raw(999)), "");
    }
}
