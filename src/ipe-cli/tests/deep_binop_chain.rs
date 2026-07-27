//! Regression test for CO-FRONT-001: a long right-associative operator chain
//! must parse and canon-compile without a stack overflow.
//!
//! The old recursive `climb_binops` consumed one native stack frame per
//! right-associative operator: `++` is right-assoc at precedence 5, so a chain
//! of N operators causes N recursive calls (~200 B/frame × 50k = ~10 MB).
//! This overflows the default 8 MB thread stack → SIGSEGV before producing a
//! result.
//!
//! The iterative explicit-stack rewrite makes `climb_binops` O(1) in call-stack
//! depth. The test proves this: it calls `ipe_canon::canonicalise` on a 50k-
//! operator chain and asserts it returns. The resulting deeply-nested canon tree
//! is leaked (`std::mem::forget`) so that its recursive drop does not itself
//! overflow the stack — the property under test is that the compile succeeds,
//! not the cleanup.

use ipe_intern::Interner;

#[test]
#[allow(clippy::expect_used)] // test setup: a failed parse IS the failure signal
#[allow(clippy::panic)] // test assertion: a canon Err IS the failure signal
fn deep_right_assoc_chain_does_not_crash() {
    // 50_000 `++` operators (right-assoc at precedence 5).
    // Old code: climb_binops recurses 50k deep → ~10 MB of stack → SIGSEGV.
    // New code: climb_binops uses an explicit heap stack, O(1) native depth.
    const N: usize = 50_000;

    let mut src = String::with_capacity(N * 10 + 300);
    src.push_str(
        "module Main exposing (main)\nimport Ipe.Prelude exposing (..)\n\
         import Ipe.Io as Io\n\nmain =\n    Io.println (",
    );
    src.push_str(r#""a""#);
    for _ in 0..N {
        src.push_str(r#" ++ "a""#);
    }
    src.push_str(")\n");

    let mut interner = Interner::new();
    let parsed = ipe_parse::parse_module(&src, &mut interner).expect("parse must succeed");

    // This call SIGSEGV'd before the fix: climb_binops recursed 50k deep.
    // After the fix it returns a 50k-deep canon tree in O(1) stack depth.
    let result = ipe_canon::canonicalise(&parsed, &mut interner);

    // Leak the deeply-nested canon tree so that the recursive drop does not
    // itself overflow the stack. The property under test is that canonicalise
    // RETURNS without a stack overflow — cleanup is irrelevant to the fix.
    //
    // `forget` is safe and deterministic: the heap allocation is freed by the
    // OS at process exit; there are no destructors with side effects in the
    // canon AST.
    if let Ok(module) = result {
        std::mem::forget(module);
    } else {
        // Canonicalise returned an Err.  The important thing is that it
        // returned at all (no crash) — but we also assert Ok to catch
        // correctness regressions.
        panic!(
            "deep right-assoc chain ({N} operators) must canon-compile successfully; \
             error indicates a regression: {:?}",
            result.err()
        );
    }
}
