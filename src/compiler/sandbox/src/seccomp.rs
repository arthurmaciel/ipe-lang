//! The seccomp-bpf syscall filter the run jail installs, hand-assembled as a
//! classic-BPF program.
//!
//! The run jail closes one axis a namespace alone cannot: syscalls. A fresh
//! network namespace stops egress, a mount scope stops file access — but a
//! shared PID namespace still lets a process `fork`, and no namespace stops
//! `ptrace`, `io_uring`, or `process_vm_readv`. This filter denies those.
//!
//! ## Why hand-assembled classic BPF, not a crate
//!
//! The program is small, fixed, and security-critical, so its exact bytes are
//! asserted in-crate ([`tests`]); a dependency would put the load-bearing bytes
//! behind a version we do not pin at the instruction level. Bubblewrap loads
//! the compiled program from a file descriptor (`--seccomp <fd>`), so this
//! module only has to *emit the bytes*; the kernel install is bubblewrap's.
//!
//! ## Fail-closed architecture guard
//!
//! The first thing the program does is check the audit architecture. A seccomp
//! filter that ignores the arch is a silent no-op on a mismatched ABI (the
//! syscall *numbers* differ), which would be fail-**open**. This filter instead
//! kills the process on any architecture other than the one it was built for —
//! an unexpected ABI refuses, it never runs unfiltered.
//!
//! The first cut supports `x86_64` (the primary Linux target). On any other
//! build target [`subprocess_deny_program`] returns `None` and the run-jail
//! wiring refuses (fail-closed) rather than install an filter that does not
//! match the running kernel's syscall numbers.

// ── classic-BPF instruction encoding ────────────────────────────────────────

/// One classic-BPF instruction (`struct sock_filter`): a 16-bit opcode, two
/// 8-bit jump offsets, and a 32-bit operand. The kernel ABI is fixed, so the
/// field order and widths are load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SockFilter {
    /// Opcode.
    pub code: u16,
    /// Jump-if-true offset (in instructions).
    pub jt: u8,
    /// Jump-if-false offset (in instructions).
    pub jf: u8,
    /// 32-bit operand (a constant, a syscall number, or an absolute offset).
    pub k: u32,
}

impl SockFilter {
    /// The 8-byte little-endian wire encoding the kernel reads.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 8] {
        let [c0, c1] = self.code.to_ne_bytes();
        let [k0, k1, k2, k3] = self.k.to_ne_bytes();
        [c0, c1, self.jt, self.jf, k0, k1, k2, k3]
    }
}

// BPF opcode constants (from <linux/bpf_common.h>). Only the ones this program
// uses are defined; each is the exact value the kernel's classic-BPF verifier
// expects.
const BPF_LD: u16 = 0x00;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JEQ: u16 = 0x10;
const BPF_JSET: u16 = 0x40;
const BPF_K: u16 = 0x00;

/// `seccomp_data` field offsets (from <linux/seccomp.h>): the filter reads the
/// syscall number, the audit arch, and — for the clone thread/process
/// discriminator — the low 32 bits of the first argument.
const OFF_NR: u32 = 0;
const OFF_ARCH: u32 = 4;
/// `args[0]` is a `u64` at offset 16; its low 32 bits (little-endian) hold the
/// `clone` flags, which is where `CLONE_VM`/`CLONE_THREAD` live.
const OFF_ARG0_LO: u32 = 16;

/// The audit architecture this filter is built for. `AUDIT_ARCH_X86_64` =
/// `EM_X86_64 (62) | __AUDIT_ARCH_64BIT (0x8000_0000) | __AUDIT_ARCH_LE
/// (0x4000_0000)`.
const AUDIT_ARCH_X86_64: u32 = 0x3E | 0x8000_0000 | 0x4000_0000;

/// seccomp return actions (from <linux/seccomp.h>).
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
/// `EPERM` — the errno a denied syscall returns (so a caller sees a permission
/// error, the same as any other blocked operation, rather than a crash).
const EPERM: u32 = 1;

// ── x86_64 syscall numbers (from <asm/unistd_64.h>) ─────────────────────────

const NR_CLONE: u32 = 56;
const NR_FORK: u32 = 57;
const NR_VFORK: u32 = 58;
// `execve`/`execveat` are intentionally NOT denied (see `CREATE_DENIED_NON_CLONE`);
// these numbers exist to pin their ABI value in the removal-guard test.
#[cfg_attr(not(test), allow(dead_code))]
const NR_EXECVE: u32 = 59;
const NR_PTRACE: u32 = 101;
const NR_PIVOT_ROOT: u32 = 155;
const NR_MOUNT: u32 = 165;
const NR_UMOUNT2: u32 = 166;
const NR_UNSHARE: u32 = 272;
const NR_SETNS: u32 = 308;
const NR_PROCESS_VM_READV: u32 = 310;
const NR_PROCESS_VM_WRITEV: u32 = 311;
#[cfg_attr(not(test), allow(dead_code))]
const NR_EXECVEAT: u32 = 322;
const NR_IO_URING_SETUP: u32 = 425;
const NR_IO_URING_ENTER: u32 = 426;
const NR_IO_URING_REGISTER: u32 = 427;
const NR_OPEN_TREE: u32 = 428;
const NR_MOVE_MOUNT: u32 = 429;
const NR_FSOPEN: u32 = 430;
const NR_FSCONFIG: u32 = 431;
const NR_FSMOUNT: u32 = 432;
// `clone3` is intentionally allowed unconditionally (see `CREATE_DENIED_NON_CLONE`);
// this number exists to pin its ABI value in the allow-guard test.
#[cfg_attr(not(test), allow(dead_code))]
const NR_CLONE3: u32 = 435;
const NR_MOUNT_SETATTR: u32 = 442;

/// `clone` flags (from <linux/sched.h>): `CLONE_VM | CLONE_THREAD` is exactly
/// the thread/process discriminator. `pthread_create` (and thus tokio's worker
/// threads and the runtime's `block_on` thread) sets both; a `fork`-style
/// process clone sets neither.
const CLONE_VM: u32 = 0x0000_0100;
const CLONE_THREAD: u32 = 0x0001_0000;

/// The syscalls denied **unconditionally**, independent of the capability set —
/// escape / exfiltration primitives no legitimate declared effect needs.
///
/// - `ptrace`, `process_vm_readv`/`process_vm_writev`: cross-process memory read
///   / code injection against jail siblings.
/// - `io_uring_setup`/`enter`/`register`: file and network I/O *without* the
///   `openat`/`connect` syscalls a naive filter watches — a direct-I/O bypass of
///   the mount-scope and (belt-and-braces to the empty netns) the network view.
/// - the mount family (`mount`/`umount2`/`pivot_root`/`setns`/`unshare`/
///   `move_mount`/`open_tree`/`fsopen`/`fsconfig`/`fsmount`/`mount_setattr`):
///   remount / namespace-re-enter escape levers.
const BASELINE_DENIED: &[u32] = &[
    NR_PTRACE,
    NR_PROCESS_VM_READV,
    NR_PROCESS_VM_WRITEV,
    NR_IO_URING_SETUP,
    NR_IO_URING_ENTER,
    NR_IO_URING_REGISTER,
    NR_MOUNT,
    NR_UMOUNT2,
    NR_PIVOT_ROOT,
    NR_SETNS,
    NR_UNSHARE,
    NR_MOVE_MOUNT,
    NR_OPEN_TREE,
    NR_FSOPEN,
    NR_FSCONFIG,
    NR_FSMOUNT,
    NR_MOUNT_SETATTR,
];

/// The task-creation syscalls denied outright when `subprocess` is **absent**:
/// `fork` and `vfork`. Legacy `clone` is handled separately (allowed only for a
/// thread — `CLONE_VM|CLONE_THREAD` — via the flag split below).
///
/// **`clone3` is deliberately NOT here — it is allowed unconditionally.** On
/// glibc >= 2.34 (verified: `pthread_create` on glibc 2.35 issues exactly one
/// `clone3({flags=CLONE_VM|…|CLONE_THREAD})`), thread creation routes through
/// `clone3`, and the emitted runtime spawns an OS thread + a multi-threaded
/// tokio runtime on *every* entry. seccomp cannot inspect `clone3`'s argument
/// (the flags live in a `struct clone_args` behind a pointer classic BPF cannot
/// dereference), so a thread/process split is not expressible for it — it is
/// all-or-nothing, and denying it is a universal false-deny (nothing runs).
///
/// Allowing `clone3` means a subprocess-absent program *can* raw-`clone3` a new
/// process. That does not breach the capability boundary: the child is born
/// inside the *same* jail — same PID/net/mount/IPC namespaces, same scrubbed
/// env and fresh `/proc`, and the same seccomp filter (a child inherits the
/// parent's filter, and `no_new_privs` makes it unremovable across `execve`). So
/// the child is confined **identically to the parent and cannot exceed its
/// capability set**; it can reach no network/file/env the parent could not.
/// What the `subprocess` axis still controls is the common, portable spawn paths
/// (`fork`/`vfork`/legacy-process-`clone` — the syscalls `posix_spawn`,
/// `Command::new`, `system`, and `fork()+exec()` use); a determined wrapper
/// using raw `clone3` can make an equally-confined sibling. That residual (a
/// process *count*, not a capability axis) is the coarse first cut.
///
/// **`execve`/`execveat` are deliberately NOT here** (replace-not-spawn model):
/// `execve` *replaces* the current process image, it does not create a child. A
/// subprocess needs a `fork`/`vfork`/`clone`(process) FIRST, then an `execve` in
/// the child — and those fork steps are denied above, so the common subprocess
/// paths are contained without touching `execve`. Denying `execve` would also
/// kill the jail's own launch: bubblewrap installs the filter and then `execve`s
/// the confined payload (`prlimit`, then the app), so an `execve` deny is a
/// universal false-deny. A lone `execve` grants no new authority: the seccomp
/// filter survives it (kernel design), and `no_new_privs` + the read-only root
/// leave the exec'd image equally confined — this holds for a raw-`clone3` child
/// that then execs, too.
const CREATE_DENIED_NON_CLONE: &[u32] = &[NR_FORK, NR_VFORK];

/// Build the seccomp program for the run jail.
///
/// `allow_subprocess` = whether the resolved capability set grants
/// `subprocess`; when `false`, the task-creation family is denied (threads
/// excepted, per [`CREATE_DENIED_NON_CLONE`] and the `clone` flag check).
///
/// Returns `None` on any architecture other than `x86_64` — the syscall numbers
/// above are `x86_64`-specific, so emitting them for another ABI would be a
/// mismatched, fail-open filter. The caller treats `None` as "no filter can be
/// built here" and refuses (fail-closed).
#[must_use]
pub fn subprocess_deny_program(allow_subprocess: bool) -> Option<Vec<SockFilter>> {
    if !cfg!(target_arch = "x86_64") {
        return None;
    }
    Some(build_program(allow_subprocess))
}

/// The pure program builder, independent of the host arch so its bytes are
/// unit-testable on any developer machine. [`subprocess_deny_program`] is the
/// arch-gated entry point.
#[must_use]
pub fn build_program(allow_subprocess: bool) -> Vec<SockFilter> {
    // 1. Load the audit arch and refuse (kill) if it is not the one we built
    //    for. A mismatched ABI has different syscall numbers, so an unchecked
    //    filter would be a silent no-op — fail-closed here instead. The JEQ
    //    skips the kill (jump +1) when the arch matches; else it falls through.
    let mut prog: Vec<SockFilter> = vec![
        stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARCH),
        jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0),
        ret(SECCOMP_RET_KILL_PROCESS),
    ];

    // 2. Load the syscall number for the per-syscall decisions below.
    prog.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFF_NR));

    // 3. Baseline denials (unconditional). Each: if nr == denied → EPERM.
    for &nr in BASELINE_DENIED {
        prog.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1));
        prog.push(ret(SECCOMP_RET_ERRNO | EPERM));
    }

    // 4. Task-creation denials when subprocess is absent.
    if !allow_subprocess {
        for &nr in CREATE_DENIED_NON_CLONE {
            prog.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1));
            prog.push(ret(SECCOMP_RET_ERRNO | EPERM));
        }
        // Legacy `clone`: allow ONLY a thread create (`CLONE_VM` AND
        // `CLONE_THREAD` both set); deny a process create. The discriminator
        // lives in the low 32 bits of arg0. `JSET` tests "any bit in the mask
        // set", so requiring BOTH bits is a chain: test `CLONE_THREAD`, then (on
        // its true path) test `CLONE_VM`; only the path where both are set
        // reaches ALLOW, every other clone falls through to EPERM.
        //
        // Six instructions, indices c0..c5 relative to the first push; the
        // seventh (c6) is the default-allow return below. Offsets are computed
        // to land on c5 (EPERM) or c6 (default ALLOW):
        //   c0 JEQ clone  jf=5 → not-clone skips to c6 (default allow)
        //   c1 LD arg0_lo
        //   c2 JSET THREAD jf=2 → thread-bit clear → c5 (EPERM)
        //   c3 JSET VM     jf=1 → vm-bit clear    → c5 (EPERM)
        //   c4 ret ALLOW          (both bits set: a thread)
        //   c5 ret EPERM
        prog.push(jump(BPF_JMP | BPF_JEQ | BPF_K, NR_CLONE, 0, 5));
        prog.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARG0_LO));
        prog.push(jump(BPF_JMP | BPF_JSET | BPF_K, CLONE_THREAD, 0, 2));
        prog.push(jump(BPF_JMP | BPF_JSET | BPF_K, CLONE_VM, 0, 1));
        prog.push(ret(SECCOMP_RET_ALLOW));
        prog.push(ret(SECCOMP_RET_ERRNO | EPERM));
    }

    // 5. Default: allow everything not denied above.
    prog.push(ret(SECCOMP_RET_ALLOW));
    prog
}

/// A statement (no jump): `jt`/`jf` are 0.
const fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

/// A conditional jump.
const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// A return (the low 16 bits of `code` are `BPF_RET`; `k` is the action).
const fn ret(k: u32) -> SockFilter {
    SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k,
    }
}

/// The whole program as the flat little-endian byte stream bubblewrap loads
/// from the seccomp fd.
#[must_use]
pub fn program_bytes(prog: &[SockFilter]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(prog.len() * 8);
    for insn in prog {
        bytes.extend_from_slice(&insn.to_bytes());
    }
    bytes
}

#[cfg(test)]
// The tests assert on the exact instruction sequence of a KNOWN-length program
// by index (`prog[c0]`, `prog[c0 + 2]`, …); a panic on an out-of-range index is
// itself a correct test failure (the program shape drifted), so raw indexing is
// the clearest, safe form here.
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn arch_guard_is_the_first_decision_and_kills_a_mismatch() {
        let prog = build_program(false);
        // insn 0 loads the arch; insn 1 branches on it; insn 2 is the kill.
        assert_eq!(prog[0], stmt(BPF_LD | BPF_W | BPF_ABS, OFF_ARCH));
        assert_eq!(prog[1].code, BPF_JMP | BPF_JEQ | BPF_K);
        assert_eq!(prog[1].k, AUDIT_ARCH_X86_64);
        assert_eq!(prog[2], ret(SECCOMP_RET_KILL_PROCESS));
    }

    #[test]
    fn baseline_denials_are_present_in_both_modes() {
        for allow in [true, false] {
            let prog = build_program(allow);
            // Every baseline syscall appears as a JEQ compare against its nr.
            for &nr in BASELINE_DENIED {
                assert!(
                    prog.iter()
                        .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                    "baseline syscall {nr} missing from filter (allow_subprocess={allow})"
                );
            }
        }
    }

    #[test]
    fn create_family_is_denied_only_when_subprocess_absent() {
        let denied = build_program(false);
        let allowed = build_program(true);
        for &nr in CREATE_DENIED_NON_CLONE {
            assert!(
                denied
                    .iter()
                    .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                "create syscall {nr} must be denied when subprocess absent"
            );
            assert!(
                !allowed
                    .iter()
                    .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                "create syscall {nr} must NOT be denied when subprocess granted"
            );
        }
    }

    #[test]
    fn clone_thread_discriminator_uses_both_clone_flags() {
        // The subprocess-absent program must test BOTH CLONE_THREAD and
        // CLONE_VM via JSET — a thread (both set) is allowed, a process (neither)
        // is EPERM'd. Two JSET compares against the two flags prove the split.
        let prog = build_program(false);
        let jset_thread = prog
            .iter()
            .any(|i| i.code == (BPF_JMP | BPF_JSET | BPF_K) && i.k == CLONE_THREAD);
        let jset_vm = prog
            .iter()
            .any(|i| i.code == (BPF_JMP | BPF_JSET | BPF_K) && i.k == CLONE_VM);
        assert!(jset_thread && jset_vm, "clone flag split not expressed");
        // And the plain `clone` nr is matched (to enter the flag check).
        assert!(
            prog.iter()
                .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == NR_CLONE),
        );
    }

    #[test]
    fn execve_is_never_denied_in_either_mode() {
        // execve/execveat must stay ALLOWED even when subprocess is absent:
        // bubblewrap execs the confined payload, and execve replaces (never
        // spawns) a process. This pins the removal so a future edit cannot
        // silently re-add them and reintroduce the universal false-deny.
        for allow in [true, false] {
            let prog = build_program(allow);
            for nr in [NR_EXECVE, NR_EXECVEAT] {
                assert!(
                    !prog
                        .iter()
                        .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                    "execve nr {nr} must not be denied (allow_subprocess={allow})"
                );
            }
        }
    }

    #[test]
    fn clone3_is_never_denied_in_either_mode() {
        // clone3 must stay ALLOWED even when subprocess is absent: on glibc
        // >= 2.34 pthread_create issues clone3(CLONE_THREAD), so denying it kills
        // every thread (a universal false-deny). seccomp cannot inspect clone3's
        // pointer arg, so it is all-or-nothing; the child it may create is
        // confined identically to the parent (same filter + namespaces). This
        // pins the allow so a future edit cannot silently re-add the deny.
        for allow in [true, false] {
            let prog = build_program(allow);
            assert!(
                !prog
                    .iter()
                    .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == NR_CLONE3),
                "clone3 must not be denied (allow_subprocess={allow})"
            );
        }
    }

    #[test]
    fn fork_and_vfork_are_denied_only_when_subprocess_absent() {
        // The portable spawn paths (posix_spawn/Command::new/system/fork+exec)
        // route through fork/vfork/legacy-process-clone; those must EPERM when
        // subprocess is absent and be allowed when granted.
        let denied = build_program(false);
        let allowed = build_program(true);
        for nr in [NR_FORK, NR_VFORK] {
            assert!(
                denied
                    .iter()
                    .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                "fork/vfork {nr} must be denied when subprocess absent"
            );
            assert!(
                !allowed
                    .iter()
                    .any(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == nr),
                "fork/vfork {nr} must be allowed when subprocess granted"
            );
        }
    }

    #[test]
    fn a_granted_subprocess_program_has_no_clone_flag_check() {
        // With subprocess granted, clone/fork/exec are simply allowed — no
        // flag discriminator, no create denials.
        let prog = build_program(true);
        assert!(
            !prog.iter().any(|i| i.code == (BPF_JMP | BPF_JSET | BPF_K)),
            "no clone-flag JSET when subprocess is granted"
        );
    }

    #[test]
    fn the_program_ends_with_a_default_allow() {
        for allow in [true, false] {
            let prog = build_program(allow);
            assert_eq!(*prog.last().expect("non-empty"), ret(SECCOMP_RET_ALLOW));
        }
    }

    #[test]
    fn instruction_encoding_is_eight_little_endian_bytes() {
        // A fixed instruction's byte encoding is pinned: the kernel ABI is
        // code(2) jt(1) jf(1) k(4), and we emit native-endian (x86_64 is LE).
        let insn = ret(SECCOMP_RET_ERRNO | EPERM);
        let bytes = insn.to_bytes();
        assert_eq!(bytes.len(), 8);
        // code = BPF_RET|BPF_K = 0x06, jt=0, jf=0, k=0x00050001.
        assert_eq!(&bytes[0..2], &0x06u16.to_ne_bytes());
        assert_eq!(bytes[2], 0);
        assert_eq!(bytes[3], 0);
        assert_eq!(&bytes[4..8], &0x0005_0001u32.to_ne_bytes());
    }

    #[test]
    fn arch_number_matches_the_kernel_constant() {
        // AUDIT_ARCH_X86_64 = 0xC000003E. A typo here silently disables the
        // whole filter (every syscall would mismatch the arch and be killed —
        // actually fail-closed — but the value must be exact so a LEGITIMATE
        // x86_64 program is not killed).
        assert_eq!(AUDIT_ARCH_X86_64, 0xC000_003E);
    }

    #[test]
    fn program_bytes_is_dense_eight_per_instruction() {
        let prog = build_program(false);
        let bytes = program_bytes(&prog);
        assert_eq!(bytes.len(), prog.len() * 8);
    }

    #[test]
    fn clone_block_jump_offsets_land_on_the_right_returns() {
        // The clone thread/process split is the most fragile part — an
        // off-by-one in a jump offset silently allows a process create or kills
        // a thread. Locate the block by its `JEQ clone` head and verify each
        // branch resolves to the intended return, by opcode at the target.
        let prog = build_program(false);
        let c0 = prog
            .iter()
            .position(|i| i.code == (BPF_JMP | BPF_JEQ | BPF_K) && i.k == NR_CLONE)
            .expect("clone JEQ present");
        // c0 jf → default ALLOW (the last instruction).
        let not_clone_target = c0 + 1 + prog[c0].jf as usize;
        assert_eq!(prog[not_clone_target], ret(SECCOMP_RET_ALLOW));
        assert_eq!(
            not_clone_target,
            prog.len() - 1,
            "not-clone → default allow"
        );
        // c2 = JSET CLONE_THREAD, its jf → EPERM.
        let c2 = c0 + 2;
        assert_eq!(prog[c2].code, BPF_JMP | BPF_JSET | BPF_K);
        assert_eq!(prog[c2].k, CLONE_THREAD);
        let thread_clear_target = c2 + 1 + prog[c2].jf as usize;
        assert_eq!(prog[thread_clear_target], ret(SECCOMP_RET_ERRNO | EPERM));
        // c3 = JSET CLONE_VM, its jf → EPERM; its fall-through → ALLOW.
        let c3 = c0 + 3;
        assert_eq!(prog[c3].k, CLONE_VM);
        let vm_clear_target = c3 + 1 + prog[c3].jf as usize;
        assert_eq!(prog[vm_clear_target], ret(SECCOMP_RET_ERRNO | EPERM));
        // Fall-through of c3 (both bits set) is the ALLOW for a thread.
        assert_eq!(prog[c3 + 1], ret(SECCOMP_RET_ALLOW));
    }

    #[test]
    fn arch_gate_precedes_every_syscall_compare() {
        // No syscall-number compare may appear before the arch guard, or a
        // wrong-ABI process could match a number before being killed.
        let prog = build_program(false);
        let first_nr_load = prog
            .iter()
            .position(|i| *i == stmt(BPF_LD | BPF_W | BPF_ABS, OFF_NR))
            .expect("NR load present");
        // The arch load+branch+kill occupy indices 0,1,2; the NR load is after.
        assert!(first_nr_load >= 3, "NR load must follow the arch guard");
    }
}
