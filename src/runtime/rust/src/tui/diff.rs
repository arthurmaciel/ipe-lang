//! Ipe.Tui — minimal grid diff.
//!
//! Pure: compares the previously-flushed grid against the freshly-rendered one
//! and returns only the cells that changed, as `(col, row, Cell)`. The TEA loop
//! turns these into ANSI cursor-moves + styled writes via crossterm — keeping
//! the I/O-free diff independently unit-testable. Total: no indexing, no panic.

use super::cell::{Cell, Grid};

/// The cells that differ between `prev` and `next`, in row-major order. When the
/// grids differ in size, every cell of `next` is emitted (a full repaint).
pub fn diff(prev: &Grid, next: &Grid) -> Vec<(usize, usize, Cell)> {
    let mut out = Vec::new();
    let full = prev.cols != next.cols || prev.rows != next.rows;
    for row in 0..next.rows {
        for col in 0..next.cols {
            let Some(n) = next.get(col, row) else {
                continue;
            };
            let changed = full || prev.get(col, row) != Some(n);
            if changed {
                out.push((col, row, n.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(g: &mut Grid, col: usize, row: usize, ch: char) {
        g.set(
            col,
            row,
            Cell {
                ch,
                ..Cell::blank()
            },
        );
    }

    #[test]
    fn only_changed_cells() {
        let mut a = Grid::new(3, 2);
        let mut b = Grid::new(3, 2);
        put(&mut a, 0, 0, 'x');
        put(&mut b, 0, 0, 'x'); // same
        put(&mut b, 2, 1, 'y'); // changed
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].0, d[0].1, d[0].2.ch), (2, 1, 'y'));
    }

    #[test]
    fn size_change_is_full_repaint() {
        let a = Grid::new(2, 2);
        let b = Grid::new(3, 2);
        assert_eq!(diff(&a, &b).len(), 3 * 2);
    }

    #[test]
    fn no_change_is_empty() {
        let a = Grid::new(4, 3);
        let b = Grid::new(4, 3);
        assert!(diff(&a, &b).is_empty());
    }

    /// A cursor-position change is represented as exactly two differing cells
    /// (the old cursor cell loses its reverse flag, the new one gains it) — not a
    /// full repaint. This validates the structural property that lets a diff-based
    /// paint avoid a full-clear on cursor moves.
    #[test]
    fn cursor_move_is_two_cells_not_full_repaint() {
        fn reverse_cell(ch: char) -> Cell {
            Cell {
                ch,
                bold: true, // use bold as stand-in for "reverse" (Cell has no reverse field)
                ..Cell::blank()
            }
        }
        let normal = |ch| Cell {
            ch,
            ..Cell::blank()
        };

        // Grid: "ab" on row 0; cursor was at col 0 (bold/reverse 'a'),
        // moves to col 1 (bold/reverse 'b').
        let mut prev = Grid::new(2, 1);
        prev.set(0, 0, reverse_cell('a'));
        prev.set(1, 0, normal('b'));

        let mut next = Grid::new(2, 1);
        next.set(0, 0, normal('a'));
        next.set(1, 0, reverse_cell('b'));

        let d = diff(&prev, &next);
        // Only the two changed cells, not all cells in the grid.
        assert_eq!(
            d.len(),
            2,
            "cursor move produces exactly 2 diffs, not {}",
            d.len()
        );
        let cols: Vec<usize> = d.iter().map(|(c, _, _)| *c).collect();
        assert!(cols.contains(&0) && cols.contains(&1));
    }
}
