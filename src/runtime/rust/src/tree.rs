//! `Ipe.Tree` — a minimal recursive, payload-carrying stdlib ADT proving the
//! kernel type-bridge for recursive runtime-backed types (issue #2493).
//!
//! `Tree` is the Rust SSOT for the Ipê `type Tree = Leaf Int | Node (List Tree)`
//! veneer. Recursion is expressed through `Vec<Tree>` (the `Node` payload lowers
//! to the existing `List` lowering), so no self-edge `Box` is needed at the
//! bridge boundary. Variant names match the Ipê constructors verbatim so emitted
//! construction (`Tree::Leaf(n)` / `Tree::Node(children)`) and `case`-match arms
//! resolve through the `pub use tree::*` glob in the generated `mod.rs`.
//!
//! The compiler-side variant + field shape is asserted equal to this enum by a
//! build-time tripwire (`tree_bridge_shape_matches_runtime` in `ipe_lower`), so
//! any drift breaks the build rather than surfacing as an `ipe`-exit-0-then-cargo
//! failure — the load-bearing SEAL guarantee for the bridge.

use super::IpeResult;
use crate::error::IpeError;

/// The Ipê `Tree` ADT — a recursive, payload-carrying sum. `Leaf` carries an
/// `Int`; `Node` carries a `List Tree` (its children), the recursive edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tree {
    Leaf(i64),
    Node(Vec<Tree>),
}

/// B3 SEAL tripwire (runtime half): a compile-time pin of the `Tree` variant +
/// field shape the bridge depends on. The emitted Rust constructs `Tree::Leaf(i64)`
/// and `Tree::Node(Vec<Tree>)` and matches them positionally, and the compiler's
/// `Ipe.Tree` bridge (`is_tree_*` / `builtin_runtime_enum` / the `Con(Tree)`
/// scheme) assumes exactly this shape. Renaming a variant, reordering, or changing
/// a field type here (drift) fails to compile THIS function — breaking the build,
/// not deferring to an `ipe`-exit-0-then-cargo-fail. The compiler-side half lives
/// in `ipe_stdlib`'s `tree_bridge_shape_matches_runtime`, which pins the stdlib
/// `type Tree` ctor+field shape against the same contract.
#[cfg(test)]
const fn _tree_shape_pin(t: &Tree) -> i64 {
    match t {
        // `Leaf` carries exactly one `i64`.
        Tree::Leaf(n) => *n,
        // `Node` carries exactly one `Vec<Tree>` (the recursive `List Tree` edge).
        Tree::Node(_children) => 0,
    }
}

/// `Tree.demoTree : Int -> Tree` — build a small fixed tree whose leaf payloads
/// derive from `n`. Total and allocation-bounded: the shape is fixed (a root
/// `Node` with two `Leaf` children and one nested single-leaf `Node`), so no
/// input can make it grow without bound.
#[must_use]
pub fn tree_demo_tree(n: i64) -> Tree {
    Tree::Node(vec![
        Tree::Leaf(n),
        Tree::Node(vec![Tree::Leaf(n.wrapping_add(1))]),
        Tree::Leaf(n.wrapping_mul(2)),
    ])
}

/// The nesting-depth ceiling `parse_tree` enforces. A parse boundary consumes
/// untrusted input, so the depth is bounded *by construction* (principle 1's
/// exhaustion clause + soundness's bounded-by-construction clause): input nested
/// past this is turned back with a typed limit error, never a stack overflow.
const PARSE_MAX_DEPTH: usize = 128;

/// `Tree.parseTree : String -> Result Error Tree` — a bounded recursive-descent
/// parser over a tiny S-expression grammar:
///
/// - a leaf is `L<int>` (e.g. `L7`, `L-3`);
/// - a node is `(child child …)` — zero or more space-separated children.
///
/// This is a *parse boundary* (parse, don't validate): the untrusted `String`
/// becomes a typed `Tree` once, and every failure — malformed syntax OR nesting
/// past [`PARSE_MAX_DEPTH`] — is a typed [`IpeError`] on the `Result` channel,
/// never a panic and never an unbounded `Vec`. The depth counter is threaded
/// explicitly and checked before each descent, so an adversarial deeply-nested
/// input is rejected before it can exhaust the stack.
#[must_use]
pub fn tree_parse_tree(input: String) -> IpeResult<IpeError, Tree> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let tree = match p.parse_node_or_leaf(0) {
        Ok(t) => t,
        Err(e) => return IpeResult::Err(e),
    };
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return IpeResult::Err(IpeError::invalid_input(
            "Ipe.Tree.parseTree: trailing input after a complete tree".to_owned(),
        ));
    }
    IpeResult::Ok(tree)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos = self.pos.saturating_add(1);
        }
    }

    fn parse_node_or_leaf(&mut self, depth: usize) -> Result<Tree, IpeError> {
        if depth > PARSE_MAX_DEPTH {
            return Err(IpeError::invalid_input(format!(
                "Ipe.Tree.parseTree: nesting deeper than the limit of {PARSE_MAX_DEPTH}"
            )));
        }
        self.skip_ws();
        match self.bytes.get(self.pos) {
            Some(b'(') => self.parse_node(depth),
            Some(b'L') => self.parse_leaf(),
            _ => Err(IpeError::invalid_input(
                "Ipe.Tree.parseTree: expected `(` or `L` at start of a tree".to_owned(),
            )),
        }
    }

    fn parse_node(&mut self, depth: usize) -> Result<Tree, IpeError> {
        // consume '('
        self.pos = self.pos.saturating_add(1);
        let mut children = Vec::new();
        loop {
            self.skip_ws();
            match self.bytes.get(self.pos) {
                Some(b')') => {
                    self.pos = self.pos.saturating_add(1);
                    return Ok(Tree::Node(children));
                }
                None => {
                    return Err(IpeError::invalid_input(
                        "Ipe.Tree.parseTree: unclosed `(`".to_owned(),
                    ));
                }
                _ => {
                    let child = self.parse_node_or_leaf(depth.saturating_add(1))?;
                    children.push(child);
                }
            }
        }
    }

    fn parse_leaf(&mut self) -> Result<Tree, IpeError> {
        // consume 'L'
        self.pos = self.pos.saturating_add(1);
        let start = self.pos;
        if self.bytes.get(self.pos) == Some(&b'-') {
            self.pos = self.pos.saturating_add(1);
        }
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos = self.pos.saturating_add(1);
        }
        let digits = self.bytes.get(start..self.pos).unwrap_or_default();
        let text = core::str::from_utf8(digits).unwrap_or_default();
        match text.parse::<i64>() {
            Ok(v) => Ok(Tree::Leaf(v)),
            Err(_) => Err(IpeError::invalid_input(
                "Ipe.Tree.parseTree: `L` must be followed by an integer".to_owned(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PARSE_MAX_DEPTH, Tree, tree_demo_tree, tree_parse_tree};
    use crate::IpeResult;

    #[test]
    fn demo_tree_is_a_fixed_recursive_shape() {
        let t = tree_demo_tree(10);
        assert_eq!(
            t,
            Tree::Node(vec![
                Tree::Leaf(10),
                Tree::Node(vec![Tree::Leaf(11)]),
                Tree::Leaf(20),
            ])
        );
    }

    #[test]
    fn parse_round_trips_a_nested_tree() {
        let r = tree_parse_tree("(L1 (L2 L3) L4)".to_owned());
        assert_eq!(
            r,
            IpeResult::Ok(Tree::Node(vec![
                Tree::Leaf(1),
                Tree::Node(vec![Tree::Leaf(2), Tree::Leaf(3)]),
                Tree::Leaf(4),
            ]))
        );
    }

    #[test]
    fn parse_a_bare_leaf() {
        assert_eq!(
            tree_parse_tree("L-5".to_owned()),
            IpeResult::Ok(Tree::Leaf(-5))
        );
    }

    #[test]
    fn parse_rejects_malformed_input() {
        assert!(matches!(
            tree_parse_tree("(L1".to_owned()),
            IpeResult::Err(_)
        ));
        assert!(matches!(
            tree_parse_tree("Lx".to_owned()),
            IpeResult::Err(_)
        ));
        assert!(matches!(
            tree_parse_tree("L1 L2".to_owned()),
            IpeResult::Err(_)
        ));
        assert!(matches!(tree_parse_tree(String::new()), IpeResult::Err(_)));
    }

    #[test]
    fn parse_rejects_over_deep_input_with_a_typed_error() {
        // One level past the ceiling: a string of `(` nested deeper than the cap.
        let deep = "(".repeat(PARSE_MAX_DEPTH + 2);
        match tree_parse_tree(deep) {
            IpeResult::Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("nesting deeper than the limit"),
                    "over-deep input must yield the typed limit error, got: {msg}"
                );
            }
            IpeResult::Ok(_) => panic!("over-deep input must be rejected, not parsed"),
        }
    }
}
