use anyhow::{Result, anyhow};
use regex::Regex;
use std::sync::OnceLock;

/// A built-in pattern compiled once, on first use. A pattern that does not
/// compile is an error returned to the caller at every use, never a panic.
pub struct StaticRegex {
    pattern: &'static str,
    cell: OnceLock<Result<Regex, regex::Error>>,
}

impl StaticRegex {
    pub const fn new(pattern: &'static str) -> Self {
        StaticRegex {
            pattern,
            cell: OnceLock::new(),
        }
    }

    pub fn get(&self) -> Result<&Regex> {
        match self.cell.get_or_init(|| Regex::new(self.pattern)) {
            Ok(re) => Ok(re),
            Err(e) => Err(anyhow!(
                "built-in pattern {:?} does not compile: {e}",
                self.pattern
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_pattern_is_an_error_every_time() {
        static BAD: StaticRegex = StaticRegex::new("(unclosed");
        assert!(BAD.get().is_err());
        assert!(BAD.get().is_err());
    }

    #[test]
    fn valid_pattern_compiles_once() {
        static GOOD: StaticRegex = StaticRegex::new(r"^a+$");
        let (Ok(first), Ok(second)) = (GOOD.get(), GOOD.get()) else {
            panic!("a valid pattern must compile");
        };
        assert!(std::ptr::eq(first, second));
        assert!(first.is_match("aaa"));
    }
}
