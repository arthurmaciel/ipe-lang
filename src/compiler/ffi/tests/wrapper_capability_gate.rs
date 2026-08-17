//! The Tier-2 wrapper capability gate, end to end over realistic wrapper source.
//!
//! Three properties the security review made load-bearing:
//!   1. a wrapper that reaches an UNDECLARED runtime capability (network) is
//!      REFUSED, and the proposed set is surfaced;
//!   2. a wrapper whose declared set matches its inferred set AND is confined to
//!      the containable axes (pure compute / clock) is ADMITTED;
//!   3. an HONESTLY declared runtime capability is STILL refused — there is no
//!      runtime sandbox around the emitted app in this release, so the capability
//!      is infeasible to enforce and must not be admitted unenforced.
//!
//! These exercise `ipe_ffi::capability_scan::{scan_sources, reconcile}` directly
//! — the same functions the `ipe install` wrapper gate calls — so the CLI wiring
//! and the library logic cannot diverge.

use std::collections::BTreeSet;

use ipe_ffi::capability_scan::{Capability, Verdict, reconcile, scan_sources};

/// A realistic wrapper that opens a socket — the canonical exfiltration surface.
const NETWORK_WRAPPER: &str = "\
pub struct Client { host: String }
pub fn connect(host: String) -> Result<Client, String> {
    let _stream = std::net::TcpStream::connect(&host).map_err(|e| e.to_string())?;
    Ok(Client { host })
}
";

/// A realistic pure-compute wrapper — no capability at all.
const PURE_WRAPPER: &str = "\
pub struct Engine { seed: i64 }
pub fn make(seed: i64) -> Engine { Engine { seed } }
pub fn describe(e: Engine) -> String { format!(\"engine<{}>\", e.seed) }
";

/// A wrapper that reads the clock — a containable (non-exfiltration) axis.
const CLOCK_WRAPPER: &str = "\
pub fn uptime_nanos(start: std::time::Instant) -> u128 { start.elapsed().as_nanos() }
";

fn declared(names: &[Capability]) -> BTreeSet<Capability> {
    names.iter().copied().collect()
}

#[test]
fn an_undeclared_network_wrapper_is_rejected_and_surfaces_the_proposed_set() {
    let scan = scan_sources([("src/lib.rs", NETWORK_WRAPPER)]);
    // The author declared nothing; the scan finds the socket.
    let verdict = reconcile(&BTreeSet::new(), &scan, &[]);
    // Refused, and the proposed set surfaces the inferred network capability.
    let refused = matches!(
        &verdict,
        Verdict::Refuse { reasons, proposed }
            if proposed.contains(&Capability::Network) && !reasons.is_empty()
    );
    assert!(
        refused,
        "an undeclared network wrapper must be refused and surface the proposed set: {verdict:?}"
    );
}

#[test]
fn a_declared_network_wrapper_is_still_rejected_no_runtime_enforcement() {
    // The author is HONEST — declares network — but Ipê cannot contain it at run.
    let scan = scan_sources([("src/lib.rs", NETWORK_WRAPPER)]);
    let verdict = reconcile(&declared(&[Capability::Network]), &scan, &[]);
    assert!(
        matches!(verdict, Verdict::Refuse { .. }),
        "an honestly declared but unenforceable capability must still be refused: {verdict:?}"
    );
}

#[test]
fn a_pure_wrapper_whose_declaration_matches_is_admitted() {
    let scan = scan_sources([("src/lib.rs", PURE_WRAPPER)]);
    // Declared nothing, inferred nothing — the sets match and are containable.
    let verdict = reconcile(&BTreeSet::new(), &scan, &[]);
    assert!(
        matches!(verdict, Verdict::Admit { .. }),
        "a pure-compute wrapper must build: {verdict:?}"
    );
}

#[test]
fn a_clock_wrapper_whose_declaration_matches_is_admitted() {
    let scan = scan_sources([("src/lib.rs", CLOCK_WRAPPER)]);
    let verdict = reconcile(&declared(&[Capability::Clock]), &scan, &[]);
    assert!(
        matches!(verdict, Verdict::Admit { .. }),
        "a clock-only wrapper is containable and must build: {verdict:?}"
    );
}

/// A wrapper that uses `asm!` — raw syscall capability, unenumerable effects.
const ASM_WRAPPER: &str = "\
pub fn raw_syscall(nr: i64) -> i64 {
    let ret: i64;
    unsafe { core::arch::asm!(\"syscall\", inlateout(\"rax\") nr => ret) };
    ret
}
";

/// A wrapper that uses `global_asm!` at crate root — same opacity class.
const GLOBAL_ASM_WRAPPER: &str = "\
global_asm!(\"
.globl _start
_start:
    xor %edi, %edi
    call exit
\");
pub fn entry() {}
";

/// A wrapper that imports via `std::arch::asm` path — same opacity class.
const STD_ARCH_WRAPPER: &str = "\
use std::arch::asm;
pub fn nop_loop(n: u64) {
    for _ in 0..n { unsafe { asm!(\"nop\") } }
}
";

#[test]
fn asm_macro_wrapper_is_refused_as_native_ffi() {
    let scan = scan_sources([("src/lib.rs", ASM_WRAPPER)]);
    assert!(
        scan.must_refuse(),
        "a wrapper containing `asm!` must refuse (NativeFfi opacity): {scan:?}"
    );
    assert!(
        scan.proposed.contains(&Capability::NativeFfi),
        "NativeFfi capability must be in the proposed set: {scan:?}"
    );
    assert!(
        !scan.opacities.is_empty(),
        "at least one NativeFfi opacity must be recorded: {scan:?}"
    );
}

#[test]
fn global_asm_wrapper_is_refused_as_native_ffi() {
    let scan = scan_sources([("src/lib.rs", GLOBAL_ASM_WRAPPER)]);
    assert!(
        scan.must_refuse(),
        "a wrapper containing `global_asm!` must refuse (NativeFfi opacity): {scan:?}"
    );
    assert!(
        scan.proposed.contains(&Capability::NativeFfi),
        "NativeFfi capability must be in the proposed set: {scan:?}"
    );
}

#[test]
fn std_arch_path_wrapper_is_refused_as_native_ffi() {
    let scan = scan_sources([("src/lib.rs", STD_ARCH_WRAPPER)]);
    assert!(
        scan.must_refuse(),
        "a wrapper naming `std::arch` must refuse (NativeFfi): {scan:?}"
    );
    assert!(
        scan.proposed.contains(&Capability::NativeFfi),
        "NativeFfi capability must be in the proposed set: {scan:?}"
    );
}

#[test]
fn a_benign_pure_wrapper_is_not_flagged_by_asm_rules() {
    // The pure-compute wrapper has no asm, no arch paths, no syscalls.
    let scan = scan_sources([("src/lib.rs", PURE_WRAPPER)]);
    let verdict = reconcile(&BTreeSet::new(), &scan, &[]);
    assert!(
        matches!(verdict, Verdict::Admit { .. }),
        "a benign pure wrapper must still be admitted after the asm rules: {verdict:?}"
    );
}

#[test]
fn a_wrapper_with_a_non_std_dependency_is_refused_even_if_its_source_looks_pure() {
    // A capability can hide entirely in a dependency (`reqwest::get` is Network
    // the wrapper's own `.rs` never names as a std path). A non-std dependency is
    // therefore opaque and refuses — the `non_std_deps` slice is the whole
    // signal, since the source scan alone would propose nothing here.
    let scan = scan_sources([("src/lib.rs", PURE_WRAPPER)]);
    let verdict = reconcile(&BTreeSet::new(), &scan, &["reqwest".to_owned()]);
    assert!(
        matches!(&verdict, Verdict::Refuse { .. }),
        "a wrapper with a non-std dependency must be refused: {verdict:?}"
    );
}
