//! Head-identity pairing of two constructor applications' children.
//!
//! Every structural walk that runs two type trees side by side pairs a
//! constructor's arguments through [`HeadIdentity::paired_args`] (any two
//! representations) or [`paired_ty_children`] (two [`Ty`]s), so no walk can
//! pair two distinct constructors that merely share an arity.

use core::array;
use core::iter::Zip;
use core::slice;

use ipe_intern::{Interner, Symbol};

use crate::ty::Ty;
use crate::unify::con_heads_compatible;

/// The rule that decides whether two constructor heads name one type constructor.
#[derive(Clone, Copy, Debug)]
pub enum HeadIdentity<'i> {
    /// The rule unification applies ([`con_heads_compatible`]).
    ///
    /// Names agree and homes agree, or one side is a builtin's empty home and
    /// the other its stdlib or reserved spelling. A walk over two types that
    /// inference unified uses this rule.
    Unified(&'i Interner),
    /// Exact home and name, the identity a lowered enum carries in the emitted program.
    ///
    /// A walk that predicts what the backend will match (a struct template
    /// against a use site) uses this rule, since the backend keys an enum by
    /// its exact home.
    Emitted,
}

/// A constructor head and its argument list, in any type representation.
#[derive(Clone, Copy, Debug)]
pub struct ConHead<'a, T> {
    /// The defining module path; empty for a builtin.
    pub home: &'a [Symbol],
    /// The type constructor's name.
    pub name: Symbol,
    /// The applied arguments.
    pub args: &'a [T],
}

/// The pairwise arguments of two constructor applications of one head.
pub type ArgPairs<'x, 'y, X, Y> = Zip<slice::Iter<'x, X>, slice::Iter<'y, Y>>;

impl HeadIdentity<'_> {
    /// Whether heads `a_home.a_name` and `b_home.b_name` name one constructor under this rule.
    #[must_use]
    pub fn same_head(
        self,
        a_home: &[Symbol],
        a_name: Symbol,
        b_home: &[Symbol],
        b_name: Symbol,
    ) -> bool {
        match self {
            Self::Unified(interner) => {
                con_heads_compatible(a_home, a_name, b_home, b_name, interner)
            }
            Self::Emitted => a_name == b_name && a_home == b_home,
        }
    }

    /// The argument pairs of `a` and `b` when they are one constructor at one arity, else `None`.
    #[must_use]
    pub fn paired_args<'x, 'y, X, Y>(
        self,
        a: ConHead<'x, X>,
        b: ConHead<'y, Y>,
    ) -> Option<ArgPairs<'x, 'y, X, Y>> {
        (self.same_head(a.home, a.name, b.home, b.name) && a.args.len() == b.args.len())
            .then(|| a.args.iter().zip(b.args))
    }
}

/// One pair of mirrored [`Ty`] children.
type TyPair<'a> = (&'a Ty, &'a Ty);

/// The positional child pairs of two same-headed [`Ty`] nodes.
///
/// Yields tuple elements or constructor arguments pairwise, or an arrow's
/// argument then result.
#[derive(Clone, Debug)]
pub struct TyPairs<'a> {
    seq: ArgPairs<'a, 'a, Ty, Ty>,
    slots: array::IntoIter<Option<TyPair<'a>>, 2>,
}

impl<'a> Iterator for TyPairs<'a> {
    type Item = TyPair<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.seq
            .next()
            .or_else(|| self.slots.by_ref().flatten().next())
    }
}

/// The child pairs of `a` and `b` when both nodes have one head under `heads`, else `None`.
///
/// An arrow pairs with an arrow, a tuple with a tuple of the same length, a
/// constructor with a constructor [`HeadIdentity::paired_args`] accepts. A
/// record (keyed by field, not position), a variable, unit, and every
/// mismatched pair yield `None`, so a caller that treats `None` as "no shared
/// shape" fails closed.
#[must_use]
pub fn paired_ty_children<'a>(
    a: &'a Ty,
    b: &'a Ty,
    heads: HeadIdentity<'_>,
) -> Option<TyPairs<'a>> {
    let empty: &[Ty] = &[];
    let seq_only = |seq| TyPairs {
        seq,
        slots: [None, None].into_iter(),
    };
    match (a, b) {
        (Ty::Fun(xa, xr), Ty::Fun(ya, yr)) => Some(TyPairs {
            seq: empty.iter().zip(empty),
            slots: [Some((&**xa, &**ya)), Some((&**xr, &**yr))].into_iter(),
        }),
        (Ty::Tuple(xs), Ty::Tuple(ys)) => {
            (xs.len() == ys.len()).then(|| seq_only(xs.iter().zip(ys)))
        }
        (
            Ty::Con {
                module: xm,
                name: xn,
                args: xs,
            },
            Ty::Con {
                module: ym,
                name: yn,
                args: ys,
            },
        ) => heads
            .paired_args(
                ConHead {
                    home: xm,
                    name: *xn,
                    args: xs,
                },
                ConHead {
                    home: ym,
                    name: *yn,
                    args: ys,
                },
            )
            .map(seq_only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ConHead, HeadIdentity, paired_ty_children};
    use crate::ty::Ty;
    use ipe_intern::{Interner, Symbol};

    struct Names {
        interner: Interner,
        ipe: Symbol,
        main: Symbol,
        lib: Symbol,
        list: Symbol,
        set: Symbol,
        web_route: Symbol,
        t: Symbol,
    }

    impl Names {
        fn new() -> Self {
            let mut interner = Interner::new();
            #[allow(clippy::expect_used)] // a fresh interner accepts these names
            let mut sym = |s: &str| interner.intern(s).expect("intern");
            let (ipe, main, lib, list, set, web_route, t) = (
                sym("Ipe"),
                sym("Main"),
                sym("Lib"),
                sym("List"),
                sym("Set"),
                sym("WebRoute"),
                sym("T"),
            );
            Self {
                interner,
                ipe,
                main,
                lib,
                list,
                set,
                web_route,
                t,
            }
        }
    }

    fn con(module: Vec<Symbol>, name: Symbol, args: Vec<Ty>) -> Ty {
        Ty::Con { module, name, args }
    }

    fn paired(a: &Ty, b: &Ty, heads: HeadIdentity<'_>) -> Option<usize> {
        paired_ty_children(a, b, heads).map(Iterator::count)
    }

    /// Same-named constructors of distinct user homes never pair, under either rule.
    #[test]
    fn distinct_user_homes_refuse() {
        let n = Names::new();
        let main_t = con(vec![n.main], n.t, vec![Ty::Unit]);
        let lib_t = con(vec![n.lib], n.t, vec![Ty::Unit]);
        assert_eq!(paired(&main_t, &lib_t, HeadIdentity::Emitted), None);
        assert_eq!(
            paired(&main_t, &lib_t, HeadIdentity::Unified(&n.interner)),
            None
        );
        assert_eq!(paired(&main_t, &main_t, HeadIdentity::Emitted), Some(1));
    }

    /// A user-homed `Main.WebRoute` is not the empty-home builtin `WebRoute`; an `Ipe`-rooted spelling is.
    ///
    /// `WebRoute` is a builtin name user code may declare, so a same-named
    /// user type is a distinct constructor. A reserved name such as `List`
    /// never carries a user home: canon refuses its declaration.
    #[test]
    fn user_home_never_matches_empty_builtin_home() {
        let n = Names::new();
        let unified = HeadIdentity::Unified(&n.interner);
        let builtin = con(vec![], n.web_route, vec![Ty::Unit]);
        let user = con(vec![n.main], n.web_route, vec![Ty::Unit]);
        let stdlib = con(vec![n.ipe, n.web_route], n.web_route, vec![Ty::Unit]);
        assert_eq!(paired(&builtin, &user, unified), None);
        assert_eq!(paired(&user, &builtin, unified), None);
        assert_eq!(paired(&builtin, &stdlib, unified), Some(1));
        assert_eq!(paired(&builtin, &stdlib, HeadIdentity::Emitted), None);
    }

    /// Distinct names or arities never pair; tuples pair only at one length.
    #[test]
    fn distinct_names_arities_and_lengths_refuse() {
        let n = Names::new();
        let unified = HeadIdentity::Unified(&n.interner);
        let list = con(vec![], n.list, vec![Ty::Unit]);
        assert_eq!(
            paired(&list, &con(vec![], n.set, vec![Ty::Unit]), unified),
            None
        );
        assert_eq!(
            paired(
                &list,
                &con(vec![], n.list, vec![Ty::Unit, Ty::Unit]),
                unified
            ),
            None
        );
        let pair = Ty::Tuple(vec![Ty::Unit, Ty::Unit]);
        let triple = Ty::Tuple(vec![Ty::Unit, Ty::Unit, Ty::Unit]);
        assert_eq!(paired(&pair, &triple, unified), None);
        assert_eq!(paired(&pair, &pair, unified), Some(2));
        let args: [Ty; 1] = [Ty::Unit];
        let head = |args| ConHead {
            home: &[],
            name: n.list,
            args,
        };
        let arity_one: &[Ty] = &args;
        assert!(
            HeadIdentity::Emitted
                .paired_args(head(arity_one), head(arity_one))
                .is_some()
        );
        assert!(
            HeadIdentity::Emitted
                .paired_args(head(arity_one), head(&[]))
                .is_none()
        );
    }
}
