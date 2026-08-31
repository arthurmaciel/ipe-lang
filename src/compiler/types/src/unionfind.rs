//! `Vec`-backed union-find with weighted union + path compression.
//!
//! Rust port of the the compiler compiler's `Ipe.Type.UnionFind` (itself a
//! derivative of elm/compiler's `Type.UnionFind`, BSD-3-Clause). The the compiler
//! original threads `IORef`-backed pointers; this port replaces them with a
//! single arena `Vec` indexed by [`VarId`]. That choice is deliberate: it keeps
//! the whole structure inside safe Rust (no `unsafe`, no aliasing of raw
//! pointers), so the "riskiest aliasing surface" of inference is provably
//! sound under Miri.
//!
//! Every arena access goes through `.get()` / `.get_mut()`. A dangling
//! [`VarId`] is impossible for ids minted by this arena, so it is treated as a
//! [`Diagnostic::CompilerBug`] (a violated internal invariant) rather than a
//! panic — the no-panic gate forbids `[]`-indexing here.

use ipe_diagnostics::{DResult, Diagnostic};

/// An index into the union-find arena. Minted only by [`UnionFind::fresh`].
pub type VarId = u32;

/// `where_` tag stamped onto every [`Diagnostic::CompilerBug`] this module
/// raises.
const STAGE: &str = "ipe_types::unionfind";

/// One arena slot: either a root carrying a descriptor + union-by-rank weight,
/// or a link to another slot.
enum Node<T> {
    Root { content: T, rank: u32 },
    Link(VarId),
}

/// A weighted-union / path-compressed union-find over descriptors of type `T`.
///
/// `T` mirrors the the compiler `Descriptor`'s payload — here a `Content` (see
/// `ty.rs`). The structure is generic so the union-find logic stays decoupled
/// from the type lattice it carries, exactly as `UF.Point a` is in the
/// reference compiler.
pub struct UnionFind<T> {
    nodes: Vec<Node<T>>,
}

impl<T: Clone> UnionFind<T> {
    /// An empty arena.
    #[must_use]
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Mint a fresh root carrying `content`, returning its [`VarId`].
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] if the arena has grown past `u32::MAX`
    /// slots (unreachable for any realistic program — bounds the id space).
    pub fn fresh(&mut self, content: T) -> DResult<VarId> {
        let id = u32::try_from(self.nodes.len()).map_err(|_| Diagnostic::CompilerBug {
            where_: STAGE,
            detail: "union-find arena exceeded u32::MAX variables".to_owned(),
        })?;
        self.nodes.push(Node::Root { content, rank: 0 });
        Ok(id)
    }

    /// Read a slot, mapping a dangling id to a `CompilerBug`.
    fn node(&self, v: VarId) -> DResult<&Node<T>> {
        self.nodes.get(v as usize).ok_or_else(|| dangling(v))
    }

    /// Find the representative root of `v`, compressing the path on the way.
    ///
    /// Two passes, both iterative (no recursion → bounded stack on adversarial
    /// chains): walk links to the root, then repoint every link directly at it.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on a dangling id (impossible for arena ids).
    pub fn find(&mut self, v: VarId) -> DResult<VarId> {
        let mut root = v;
        loop {
            match self.node(root)? {
                Node::Root { .. } => break,
                Node::Link(next) => root = *next,
            }
        }
        // Path compression: repoint the whole chain at `root`.
        let mut cur = v;
        loop {
            match self.node(cur)? {
                Node::Root { .. } => break,
                Node::Link(next) => {
                    let next = *next;
                    if let Some(slot) = self.nodes.get_mut(cur as usize) {
                        *slot = Node::Link(root);
                    }
                    cur = next;
                }
            }
        }
        Ok(root)
    }

    /// Read-only view of the descriptor at an ALREADY-RESOLVED root, without
    /// cloning it. The caller must have run [`Self::find`] first (so path
    /// compression is preserved); passing a non-root is the same error
    /// contract as [`Self::content`]. Lets post-solve passes peek a large
    /// record descriptor's field map without deep-copying it
    /// (efficiency-audit §2 medium).
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] when `root` is dangling or not a root.
    pub fn root_content(&self, root: VarId) -> DResult<&T> {
        match self.node(root)? {
            Node::Root { content, .. } => Ok(content),
            Node::Link(_) => Err(not_a_root()),
        }
    }

    /// Clone of the descriptor stored at `v`'s representative root.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on a dangling id, or if the resolved root is
    /// not actually a root (a structural invariant violation).
    pub fn content(&mut self, v: VarId) -> DResult<T> {
        let root = self.find(v)?;
        match self.node(root)? {
            Node::Root { content, .. } => Ok(content.clone()),
            Node::Link(_) => Err(not_a_root()),
        }
    }

    /// Overwrite the descriptor at `v`'s representative root.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on a dangling id / non-root root.
    pub fn set_content(&mut self, v: VarId, value: T) -> DResult<()> {
        let root = self.find(v)?;
        match self.nodes.get_mut(root as usize) {
            Some(Node::Root { content, .. }) => {
                *content = value;
                Ok(())
            }
            Some(Node::Link(_)) => Err(not_a_root()),
            None => Err(dangling(root)),
        }
    }

    /// Whether `a` and `b` already share a representative.
    ///
    /// Test-only since the `unify` hot path inlined it (two reused `find`s —
    /// efficiency-audit §2 low); the semantics tests below still assert
    /// class-merge behaviour through it.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on a dangling id.
    #[cfg(test)]
    pub fn equivalent(&mut self, a: VarId, b: VarId) -> DResult<bool> {
        Ok(self.find(a)? == self.find(b)?)
    }

    /// Merge the classes of `a` and `b`, storing `keep` as the surviving root's
    /// descriptor. Weighted by rank so the shallower tree hangs off the deeper
    /// one (near-constant amortised cost).
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] on a dangling id / non-root root.
    pub fn union(&mut self, a: VarId, b: VarId, keep: T) -> DResult<()> {
        let ra = self.find(a)?;
        let rb = self.find(b)?;
        if ra == rb {
            // Already one class; just refresh the descriptor.
            return self.set_content(ra, keep);
        }
        let rank_a = self.rank(ra)?;
        let rank_b = self.rank(rb)?;
        // Hang the lower-rank root under the higher-rank one.
        let (winner, loser) = if rank_a >= rank_b { (ra, rb) } else { (rb, ra) };
        match self.nodes.get_mut(winner as usize) {
            Some(Node::Root { content, rank }) => {
                *content = keep;
                if rank_a == rank_b {
                    *rank = rank.saturating_add(1);
                }
            }
            Some(Node::Link(_)) => return Err(not_a_root()),
            None => return Err(dangling(winner)),
        }
        self.nodes.get_mut(loser as usize).map_or_else(
            || Err(dangling(loser)),
            |slot| {
                *slot = Node::Link(winner);
                Ok(())
            },
        )
    }

    /// The rank of a (must-be) root slot.
    fn rank(&self, v: VarId) -> DResult<u32> {
        match self.node(v)? {
            Node::Root { rank, .. } => Ok(*rank),
            Node::Link(_) => Err(not_a_root()),
        }
    }
}

fn dangling(v: VarId) -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: format!("dangling union-find id {v}"),
    }
}

fn not_a_root() -> Diagnostic {
    Diagnostic::CompilerBug {
        where_: STAGE,
        detail: "union-find representative was not a root".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ids_are_sequential_and_self_representative() {
        let mut uf: UnionFind<i32> = UnionFind::new();
        let a = uf.fresh(1).ok();
        let b = uf.fresh(2).ok();
        assert_eq!(a, Some(0));
        assert_eq!(b, Some(1));
        let (Some(a), Some(b)) = (a, b) else { return };
        assert_eq!(uf.find(a).ok(), Some(a));
        assert_eq!(uf.find(b).ok(), Some(b));
        assert_eq!(uf.equivalent(a, b).ok(), Some(false));
    }

    #[test]
    fn union_merges_classes_and_keeps_descriptor() {
        let mut uf: UnionFind<i32> = UnionFind::new();
        let (Some(a), Some(b), Some(c)) = (uf.fresh(10).ok(), uf.fresh(20).ok(), uf.fresh(30).ok())
        else {
            return;
        };
        assert!(uf.union(a, b, 99).is_ok());
        assert_eq!(uf.equivalent(a, b).ok(), Some(true));
        assert_eq!(uf.content(a).ok(), Some(99));
        assert_eq!(uf.content(b).ok(), Some(99));
        // c is still its own class.
        assert_eq!(uf.equivalent(a, c).ok(), Some(false));
        // Chain a≡b≡c, descriptor flows to the surviving root.
        assert!(uf.union(b, c, 77).is_ok());
        assert_eq!(uf.content(a).ok(), Some(77));
        assert_eq!(uf.equivalent(a, c).ok(), Some(true));
    }

    #[test]
    fn path_compression_handles_long_chains() {
        let mut uf: UnionFind<u32> = UnionFind::new();
        let mut ids = Vec::new();
        for k in 0..64u32 {
            if let Ok(id) = uf.fresh(k) {
                ids.push(id);
            }
        }
        // Chain them all into one class.
        for w in ids.windows(2) {
            if let [x, y] = w {
                assert!(uf.union(*x, *y, 0).is_ok());
            }
        }
        // Every element resolves to the same representative.
        let first = ids.first().copied();
        let Some(first) = first else { return };
        let rep = uf.find(first).ok();
        for id in &ids {
            assert_eq!(uf.find(*id).ok(), rep);
        }
    }

    #[test]
    fn dangling_id_is_compiler_bug_not_panic() {
        let mut uf: UnionFind<i32> = UnionFind::new();
        let r = uf.find(999);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
    }

    #[test]
    fn set_content_updates_representative() {
        let mut uf: UnionFind<i32> = UnionFind::new();
        let (Some(a), Some(b)) = (uf.fresh(1).ok(), uf.fresh(2).ok()) else {
            return;
        };
        assert!(uf.union(a, b, 5).is_ok());
        assert!(uf.set_content(b, 42).is_ok());
        assert_eq!(uf.content(a).ok(), Some(42));
    }
}
