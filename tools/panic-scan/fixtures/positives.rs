// Every construct below is a real production abrupt-failure and MUST be flagged.
// The `//@HIT` marker sits on the line where the construct's token starts.
fn f01() { panic!("x"); } //@HIT
fn f02() { unreachable!(); } //@HIT
fn f03() { todo!(); } //@HIT
fn f04() { unimplemented!(); } //@HIT
fn f05() { assert!(c); } //@HIT
fn f06() { assert_eq!(a, b); } //@HIT
fn f07() { assert_ne!(a, b); } //@HIT
fn f08() { debug_assert!(x); } //@HIT
fn f09() { debug_assert_eq!(a, b); } //@HIT
fn f10() { debug_assert_ne!(a, b); } //@HIT
fn f11() { assert![c]; } //@HIT
fn f12() { assert! {true} } //@HIT
fn f13() { let _ = o.unwrap(); } //@HIT
fn f14() { let _ = r.expect("m"); } //@HIT
fn f15() { let _ = r.unwrap_err(); } //@HIT
fn f16() { let _ = r.expect_err("m"); } //@HIT
fn f17() { let _ = unsafe { o.unwrap_unchecked() }; } //@HIT
fn f18() { let _ = o.unwrap::<T>(); } //@HIT
fn f19() { std::process::abort(); } //@HIT
fn f20() { panic_any(5); } //@HIT
fn f21() { std::process::exit(1); } //@HIT
fn f22() { let s = "http://z"; let _ = o.unwrap(); } //@HIT
fn f23() { o . unwrap (); } //@HIT
fn f24() { assert !( c ); } //@HIT
fn f25() {
    panic! //@HIT
    ("split across lines");
}
fn f26() {
    o.
    unwrap(); //@HIT
}
