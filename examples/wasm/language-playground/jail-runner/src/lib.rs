#![allow(clippy::missing_errors_doc)]

pub mod budget;
pub mod run_jailed;
pub mod staging;

/// Reads the server's Ipê constants, so a limit both sides enforce is compared.
#[cfg(test)]
mod ipe_source {
    /// The value of the top-level Ipê constant `name`, laid out as
    /// `name =` then its value on the next line.
    pub fn constant<T: std::str::FromStr>(source: &str, name: &str) -> Option<T> {
        let header = format!("\n{name} =\n");
        let (_, rest) = source.split_once(&header)?;
        rest.lines().next()?.trim().parse().ok()
    }
}
