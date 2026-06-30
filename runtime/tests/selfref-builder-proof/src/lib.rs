//! WALL 6 (#68) — self-referential FFI builder bundle: MIRI soundness PROOF.
//!
//! GUARDIAN-GATED EXISTENCE PROOF (design gate 2026-06-27, see
//! `runtime-rust/docs/superpowers/specs/2026-06-27-rust-ffi-wall6-selfref-builder-design.md`).
//!
//! This file is the concrete, MIRI-checked proof that the *mechanism* which would
//! bind a self-borrowing builder chain as an owned `'static` Sky value is SOUND
//! for a COVARIANT builder. It is **deliberately NOT wired into codegen**: the
//! guardian ruled auto-emission REJECT-FOR-NOW because the soundness precondition
//! (the foreign builder `B<'a>` must be COVARIANT in `'a`) is UNVERIFIABLE at
//! codegen — a covariance compile-probe failure would break the
//! `sky build ⇒ cargo build` floor, and the inspector runs rustdoc without
//! `--document-private-items` so it cannot compute variance itself. The owned
//! params-struct path (fixture 104) is the SHIPPED, zero-unsafe circumvention;
//! this proof keeps the harder mechanism honest + picked-up-able for the day
//! covariance can be proven from the inspector.
//!
//! ## The shape (mirrors firestore `db.fluent().select()…query()`)
//!   entry:    `Db::fluent(&self) -> Sel<'_>`         &self -> Builder<'_>
//!   chain:    `Sel::limit(self, n) -> Sel<'a>`       self-by-value, same builder
//!             `Sel::rows(self)     -> Selected<'a>`  self-by-value, NEXT builder
//!   terminal: `Selected::run(self) -> Vec<String>`  self-by-value, OWNED return
//!
//! ## Soundness invariants proven here (guardian Q1/Q2 + a MIRI-found 5th)
//!   1. STABLE OWNER  — the owner lives on the heap at a stable address; moving
//!      the bundle never moves the pointee while it is borrowed.
//!   2. DROP ORDER    — `dep` declared BEFORE `owner` ⇒ dropped first ⇒ the
//!      builder releases its borrow before the owner is freed (RFC 1857).
//!   3. NO ESCAPE     — every chain step re-bundles `dep` with the SAME `owner`;
//!      the terminal returns a fully-OWNED `Vec<String>`. The `'static`-laundered
//!      borrow is never handed to the caller.
//!   4. COVARIANCE    — `Sel<'a>`/`Selected<'a>` use `'a` ONLY as a shared
//!      `&'a Db` ⇒ covariant ⇒ lengthening `'a → 'static` via transmute is benign
//!      given (1)-(3). THIS is the precondition the inspector cannot verify for an
//!      arbitrary foreign type → why auto-emit stays blocked.
//!   5. ALIASABLE OWNER (MIRI-FOUND, 2026-06-27) — the owner MUST NOT be a plain
//!      `Box<Db>`. Under Stacked Borrows, moving a `Box` asserts UNIQUE access to
//!      its pointee, which INVALIDATES the dependent's outstanding shared borrow —
//!      UB, even with invariants 1-4 satisfied. (MIRI flagged the naive `Box`
//!      version: "<tag> was later invalidated by a Unique retag" at the bundle
//!      move.) The fix is an *aliasable* owner (a raw-pointer-backed box that does
//!      NOT carry `noalias`, like `self_cell`/`ouroboros`'s `AliasableBox`), so
//!      moving it performs no unique retag. This is a FIFTH precondition the spec's
//!      original invariants 1-4 missed — a further reason auto-emit is REJECT-FOR-NOW:
//!      the generated code must be the aliasable-owner form, not the obvious one.
//!
//! ## Shapes that MUST stay dropped (guardian Q2 — NOT bindable by this mechanism)
//!   (a) a builder method whose return carries `'a` and is NOT a re-bundled chain
//!       successor (e.g. `fn name(&self) -> &'a str`) — leaks the borrow;
//!   (b) a terminal returning a borrowing type (`BoxStream<'a, _>`) — leaks;
//!   (c) a non-`Clone`/non-ownable receiver — the cart cannot be owned;
//!   (d) an `!Send` receiver under an async terminal — unsound to spawn;
//!   (e) ANY builder with interior mutability over `'a` (`Cell<&'a Db>`,
//!       `&'a mut Db`, `fn(&'a …)`) — INVARIANT/contravariant ⇒ the launder is UB
//!       even though the return-position escape gate would pass it (guardian's
//!       "interior-mutability WRITE channel"). This is the covariance precondition.
//!
//! Run: `cargo +nightly miri test` in this directory (Stacked Borrows), and
//! `MIRIFLAGS=-Zmiri-tree-borrows cargo +nightly miri test` (Tree Borrows).
//! VERIFIED 2026-06-27: **3/3 pass under BOTH models** — but only after MIRI
//! forced out invariants 5 and 6 below, which the design spec's original
//! invariants 1-4 MISSED. That is the proof's real payload: the sound
//! construction is materially subtler than "box the owner + order the fields",
//! which is precisely why auto-EMITTING it is REJECT-FOR-NOW (guardian gate) —
//! a codegen that emits the obvious form ships UB.

#![allow(dead_code)]

// ───────────────────────── the foreign-like crate under test ─────────────────
// A COVARIANT borrowing builder chain (the firestore fluent shape, distilled).

#[derive(Clone)]
pub struct Db {
    rows: Vec<String>,
}

impl Db {
    pub fn new() -> Db {
        Db {
            rows: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        }
    }

    /// ENTRY: `&self -> Builder<'_>`. `Sel<'a>` borrows `&'a self`.
    pub fn fluent(&self) -> Sel<'_> {
        Sel {
            db: self,
            limit: None,
        }
    }
}

/// Borrowing builder — covariant in `'a` (only use of `'a` is `&'a Db`).
pub struct Sel<'a> {
    db: &'a Db,
    limit: Option<usize>,
}

impl<'a> Sel<'a> {
    /// CHAIN (self-returning): consumes self, returns the same builder.
    pub fn limit(mut self, n: usize) -> Sel<'a> {
        self.limit = Some(n);
        self
    }

    /// CHAIN (type-changing): consumes self, returns the NEXT builder, same `'a`.
    pub fn rows(self) -> Selected<'a> {
        Selected {
            db: self.db,
            limit: self.limit,
        }
    }
}

/// The next borrowing builder — also covariant in `'a`.
pub struct Selected<'a> {
    db: &'a Db,
    limit: Option<usize>,
}

impl<'a> Selected<'a> {
    /// TERMINAL: consumes self, returns a fully-OWNED value (no borrow escapes).
    pub fn run(self) -> Vec<String> {
        let mut v = self.db.rows.clone();
        if let Some(n) = self.limit {
            v.truncate(n);
        }
        v
    }
}

// ─────────────────── the generated bundle (what codegen WOULD emit) ───────────

/// Minimal ALIASABLE owner (invariant 5, MIRI-found): a heap box whose MOVE does
/// NOT assert unique access to the pointee (unlike `Box`, which carries `noalias`
/// and triggers a Unique retag on move). A dependent borrowing `*self` therefore
/// stays valid under Stacked Borrows when the bundle is moved/destructured.
/// Mirrors `self_cell`/`ouroboros`'s `AliasableBox`. Frees the pointee on drop.
struct AliasableBox<T> {
    ptr: core::ptr::NonNull<T>,
}

impl<T> AliasableBox<T> {
    fn new(v: T) -> AliasableBox<T> {
        // Box::into_raw keeps the heap allocation alive; NonNull is a raw pointer,
        // so MOVING this struct performs NO unique retag of the pointee.
        let raw = Box::into_raw(Box::new(v));
        // SAFETY: Box::into_raw never returns null.
        AliasableBox {
            ptr: unsafe { core::ptr::NonNull::new_unchecked(raw) },
        }
    }

    /// Borrow the pointee via the RAW pointer (SharedReadOnly under SB — NOT
    /// derived from a uniquely-retagged `Box`).
    fn borrow(&self) -> &T {
        // SAFETY: `ptr` is valid until Drop; we only ever hand out shared refs.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> Drop for AliasableBox<T> {
    fn drop(&mut self) {
        // SAFETY: reconstruct the owning Box exactly once to free the pointee.
        // Runs AFTER `dep` is dropped (bundle field order: invariant 2), so no
        // live borrow remains when this Unique retag + dealloc happens.
        unsafe {
            drop(Box::from_raw(self.ptr.as_ptr()));
        }
    }
}

// Field order is LOAD-BEARING (invariant 2): `dep` before `owner`.
struct SkyFluentSel {
    dep: Sel<'static>,       // field 0 → dropped FIRST (releases borrow)
    owner: AliasableBox<Db>, // field 1 → dropped SECOND (frees pointee)
}

struct SkyFluentSelected {
    dep: Selected<'static>,
    owner: AliasableBox<Db>,
}

impl SkyFluentSel {
    /// The ONE lifetime-launder unsafe site (composed with the aliasable owner).
    fn new(db: Db) -> SkyFluentSel {
        let owner = AliasableBox::new(db);
        // SAFETY (invariants 1-5): `dep` borrows `*owner` via the aliasable raw
        // pointer (moving `owner` performs no unique retag → the borrow survives
        // the bundle move). `owner` is heap-stable, in the same struct, dropped
        // AFTER `dep`. `Sel<'a>` is covariant in `'a` (only `&'a Db`), so
        // lengthening `'a → 'static` is benign; the borrow is never observed past
        // `*owner` (every step re-bundles; the terminal returns owned).
        let dep: Sel<'static> =
            unsafe { core::mem::transmute::<Sel<'_>, Sel<'static>>(owner.borrow().fluent()) };
        SkyFluentSel { dep, owner }
    }
}

/// CHAIN step (SAFE — no new unsafe): move the aliasable owner along, transform dep.
fn limit_step(b: SkyFluentSel, n: usize) -> SkyFluentSel {
    let SkyFluentSel { dep, owner } = b; // aliasable move = no unique retag
    SkyFluentSel {
        dep: dep.limit(n),
        owner,
    }
}

/// CHAIN step that changes the builder type (SAFE): carry `owner` to the next bundle.
fn rows_step(b: SkyFluentSel) -> SkyFluentSelected {
    let SkyFluentSel { dep, owner } = b;
    SkyFluentSelected {
        dep: dep.rows(),
        owner,
    }
}

/// TERMINAL — the SUBTLE one (invariant 6, MIRI-found 2026-06-27). The terminal
/// READS the pointee (`dep.run()`, a shared access — fine under a protector) and
/// returns the owned result, but it MUST NOT free `owner` inside its own frame:
/// `b` is a by-value argument, so the borrow `dep` holds of `*owner` is STRONGLY
/// PROTECTED until this function returns, and freeing protected memory in-frame
/// is UB under BOTH Stacked AND Tree Borrows. So the terminal HANDS `owner` BACK;
/// the caller drops it in a frame where no protector is active. (For the real
/// async terminal this is automatic: the owner lives alongside the spawned future
/// and is dropped when the future completes — outside this synchronous builder
/// fn's frame.) Returning owner is a move, not a free → no protected write.
fn run_terminal(b: SkyFluentSelected) -> (Vec<String>, AliasableBox<Db>) {
    let SkyFluentSelected { dep, owner } = b;
    let out = dep.run(); // shared READ of *owner — allowed under the protector
    (out, owner) // hand owner back; caller frees it out-of-frame
}

#[cfg(test)]
mod tests {
    use super::*;

    /// POSITIVE: the full owned bundle chain `db.fluent().limit(2).rows().run()`
    /// is UB-free under MIRI (transmute + drop order + chained moves all sound).
    #[test]
    fn bundle_chain_is_ub_free() {
        let b = SkyFluentSel::new(Db::new());
        let b = limit_step(b, 2);
        let b = rows_step(b);
        let (out, owner) = run_terminal(b);
        drop(owner); // freed in THIS (caller) frame — no by-value-arg protector
        assert_eq!(out, vec!["a".to_string(), "b".to_string()]);
    }

    /// POSITIVE: a bundle constructed + chained but NOT terminated must drop
    /// cleanly via SCOPE-END glue (dep released, then owner freed — field order).
    /// NOTE (invariant 6): we must NOT use `drop(b)` here — `std::mem::drop` is a
    /// by-value function, so it would PROTECT `dep`'s borrow and the owner-free in
    /// `AliasableBox::drop` would then violate the protector (UB under both MIRI
    /// models). Scope-end drop glue runs in the owning frame with no such
    /// protector. (Generated terminal/discard code must obey the same rule.)
    #[test]
    fn bundle_dropped_without_terminal_is_ub_free() {
        let b = SkyFluentSel::new(Db::new());
        let _b = limit_step(b, 1);
        // `_b` falls out of scope HERE → drop glue, no by-value-arg protector.
    }

    /// POSITIVE: many independent bundles interleaved — proves each owner's heap
    /// allocation is independent and no cross-bundle aliasing occurs.
    #[test]
    fn many_bundles_no_cross_aliasing() {
        let mut outs = Vec::new();
        for i in 0..8usize {
            let b = SkyFluentSel::new(Db::new());
            let b = limit_step(b, i % 4);
            let b = rows_step(b);
            let (rows, owner) = run_terminal(b);
            drop(owner);
            outs.push(rows.len());
        }
        assert_eq!(outs, vec![0, 1, 2, 3, 0, 1, 2, 3]);
    }
}
