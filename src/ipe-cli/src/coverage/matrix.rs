//! The coverage-matrix runner: iterate `surface × aspects`, render the grid, and
//! fail naming every hole.
//!
//! One runner applies every registered [`AspectCheck`] to every symbol of a
//! [`Surface`], producing a `(symbol, aspect)` grid. A [`Cell::Hole`] fails the
//! run at its named coordinate; a [`Cell::Warn`] is reported as advisory debt
//! without failing; [`Cell::Ok`] and [`Cell::NotApplicable`] pass. This
//! concentrates every per-aspect check into one loop so no symbol is judged on a
//! subset of the aspects.

use crate::coverage::contract::{AspectCheck, Cell, StdlibSymbol, Surface};

/// The registered static aspect columns of the stdlib surface.
///
/// These read only registries and typed interfaces — no program build — so they
/// run in the fast (non-E2E) path.
#[must_use]
pub fn static_columns() -> Vec<Box<dyn AspectCheck>> {
    vec![
        Box::new(crate::coverage::columns_static::HomeColumn),
        Box::new(crate::coverage::columns_static::ResolvesColumn::new()),
        Box::new(crate::coverage::columns_static::ClosedSchemeColumn::new()),
        Box::new(crate::coverage::columns_static::LayerAgreementColumn::new()),
        // LANE B: register your columns here
    ]
}

/// One hole found in the grid, at a named `(symbol, aspect)` coordinate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hole {
    /// The dotted symbol path, e.g. `Ipe.List.map`.
    pub symbol: String,
    /// The aspect column that reported the hole.
    pub aspect: &'static str,
    /// The column's human message.
    pub message: String,
}

/// One advisory found in the grid, at a named `(symbol, aspect)` coordinate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Advisory {
    /// The dotted symbol path.
    pub symbol: String,
    /// The aspect column that reported the advisory.
    pub aspect: &'static str,
    /// The column's human message.
    pub message: String,
}

/// The outcome of a matrix run: every hole and every advisory, in the
/// deterministic order the surface enumerates its symbols and the columns are
/// registered.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct MatrixReport {
    /// Every failing coordinate.
    pub holes: Vec<Hole>,
    /// Every advisory coordinate.
    pub advisories: Vec<Advisory>,
    /// The number of symbols enumerated.
    pub symbols: usize,
    /// The number of aspect columns applied.
    pub columns: usize,
}

impl MatrixReport {
    /// Whether the run passed: no holes (advisories do not fail the gate).
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.holes.is_empty()
    }

    /// A rendered, human-readable failure report naming every hole coordinate,
    /// followed by any advisories.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut out = format!(
            "coverage matrix: {} symbols × {} columns\n",
            self.symbols, self.columns
        );
        if self.holes.is_empty() {
            out.push_str("no holes\n");
        } else {
            let _ = writeln!(out, "{} hole(s):", self.holes.len());
            for h in &self.holes {
                let _ = writeln!(out, "  HOLE [{}] {}: {}", h.aspect, h.symbol, h.message);
            }
        }
        for a in &self.advisories {
            let _ = writeln!(out, "  WARN [{}] {}: {}", a.aspect, a.symbol, a.message);
        }
        out
    }
}

/// The dotted display path of a symbol, e.g. `Ipe.List.map`.
fn dotted(sym: &StdlibSymbol) -> String {
    let mut path = sym.module.join(".");
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(&sym.name);
    path
}

/// Run the matrix over one surface and one column set, collecting every hole and
/// advisory coordinate.
pub fn run<S>(surface: &S, columns: &[Box<dyn AspectCheck>]) -> MatrixReport
where
    S: Surface<Item = StdlibSymbol>,
{
    let symbols = surface.all();
    let mut report = MatrixReport {
        symbols: symbols.len(),
        columns: columns.len(),
        ..MatrixReport::default()
    };
    for sym in &symbols {
        let path = dotted(sym);
        for column in columns {
            match column.check(sym) {
                Cell::Ok | Cell::NotApplicable => {}
                Cell::Hole(message) => report.holes.push(Hole {
                    symbol: path.clone(),
                    aspect: column.name(),
                    message,
                }),
                Cell::Warn(message) => report.advisories.push(Advisory {
                    symbol: path.clone(),
                    aspect: column.name(),
                    message,
                }),
            }
        }
    }
    report
}

/// Run the stdlib surface against its registered static columns.
#[must_use]
pub fn run_static() -> MatrixReport {
    run(&crate::coverage::surface::StdlibSurface, &static_columns())
}
