//! TCO IR nodes (`TailLoop` / `TailRecur`) construct, clone, and compare — the
//! parse-don't-validate jump transport is a typed value, never a stringly
//! sentinel.

use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{Expr, IrType};

#[test]
fn tail_nodes_construct_and_clone() -> DResult<()> {
    let mut interner = Interner::new();
    let p = interner.intern("acc")?;
    let loop_ = Expr::TailLoop {
        params: vec![(p, IrType::Int)],
        body: Box::new(Expr::TailRecur {
            args: vec![Expr::Int(1)],
        }),
    };
    assert_eq!(loop_.clone(), loop_);
    Ok(())
}
