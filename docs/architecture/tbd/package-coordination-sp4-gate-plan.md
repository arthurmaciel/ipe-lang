# SP4 — the package gate: implementation plan

Status: plan (tbd). The last package-coordination phase. Every other piece it
composes is built: capability inference, the `ipe rust`/manifest surface, the
index + resolver + lockfile, the `ipe diff` enforced-semver check, the
`tools/panic-scan` provenance scanner, and FFI Tier 1 + the Tier 2 wrapper
foundation. SP4 wires them into the single check a package clears before the
curated index accepts a version.

Fenced blocks are illustrative unless the prose says otherwise.

---

## 0. What the gate is, and where it runs

The gate answers one question: *is this package version safe and honest enough
for the index to serve it?* It runs in two places, same checks:

- **`ipe package audit`** — the author's local pre-flight. Runs the full gate on
  the working package and prints pass/fail, so a submission is green before it is
  opened.
- **The index-repo CI** — the authoritative gate. When a package version is
  proposed to the curated index (a git repository), its CI re-runs the same
  audit and the entry is merged only when green. The author's local run is a
  convenience; the index CI is the source of truth.

A gate result is a hard **accept** or **reject with a diagnostic** — never a
warning that lets an unsafe version through.

---

## 1. Universal Tier-1 quality gate (every package)

Applies to every package, Ipê-only or Ipê+Rust. Four checks, all buildable from
existing pieces:

### 1a. Provenance panic-scan
Build the package to its emitted Rust, then run the `tools/panic-scan` token
scanner (see the abrupt-failure ADR) with **provenance attribution**:
- a hit in the **emitted** Rust ⇒ **compiler bug** — fails *our* CI, not the
  author's (our codegen must never emit abrupt failure from pure Ipê);
- a hit in **author-supplied FFI / wrapper Rust** ⇒ **user error** — reject the
  package with a diagnostic pointing at the offending line.

### 1b. Capability consistency
Compute the inferred capability set (the call-graph union over capability-tagged
kernels, `Capability` in the kernels crate; see the capabilities doc) and compare
it to the manifest's declared `[capabilities]`:
- a capability the code **uses but did not declare** ⇒ hard reject (a hidden
  effect);
- a capability **declared but not used** ⇒ reject (an over-broad, misleading
  declaration), so the declared set is exactly the truth the user consents to.
`native-ffi` is declared, not inferred — its presence must be surfaced loudly.

### 1c. Enforced semver
Run the implemented `ipe diff` / `check_semver_bump` between this version's public
API and the previous published version fetched from the index; an **under-bump**
(a breaking change without a major/minor bump per the pre-1.0 mapping) is
rejected. A first version has no predecessor and skips this check.

### 1d. Supply chain
Run `cargo-deny` and lockfile-integrity over the package's `[rust.dependencies]`
graph (the same supply-chain posture the workspace already applies), and verify
the fetched source's content hash matches the index pin (the resolver already
does this at install; the gate re-asserts it at publish).

---

## 2. Native Tier-2 (Ipê + Rust packages)

For a package that carries native Rust (FFI bindings or a Tier 2 wrapper crate):

- **Sandboxed build** — build the native Rust inside the `ipe_sandbox` RCE jail
  (the same jail crate inspection already uses), so a malicious `build.rs` /
  proc-macro is contained at build.
- **Declared-capability fail-closed enforcement** — enforce that the native code
  cannot exercise a capability the manifest did not declare. This is the FFI
  Tier 2 capability layer (static inference *proposes*, the manifest *declares*,
  the sandbox *enforces* fail-closed). **This layer is BLOCKED on FFI Tier 2
  capability enforcement** and lands with it.

---

## 3. Fail-closed sandbox isolation

The runtime side of enforcement: isolate the declared high-value capabilities
(`network`, `filesystem`, `env`, `subprocess`) with OS primitives — network
namespace unshared unless `network` is declared, filesystem/pid/env scoped
likewise — fail-closed, so an undeclared effect is impossible rather than merely
denied. Coarse isolation (namespaces + `prlimit`) is v1; fine-grained
per-capability syscall filtering (a seccomp capability→syscall map) is a noted v2
follow-up, not required for the gate to ship.

---

## 4. Proposed CLI surface

- **`ipe package audit`** — run the full gate locally (Tier-1 always; Tier-2 when
  the package carries native code). Exit non-zero with the failing check's
  diagnostic. This is the one new command SP4 needs.
- The index CI invokes the same audit path (a library entry the command wraps),
  so author pre-flight and authoritative gate cannot diverge.

`ipe package publish` (submitting to the index) is a separate, optional surface —
the index accepts a version through its own PR flow, so publishing can stay a
thin "open the index PR" helper rather than a privileged command.

---

## 5. What is buildable now vs blocked

| Layer | Depends on | Buildable now? |
|---|---|---|
| 1a provenance panic-scan | `tools/panic-scan` (built) | **yes** |
| 1b capability consistency | `Capability` + inference (built) | **yes** |
| 1c enforced semver | `ipe diff` (built) | **yes** |
| 1d supply chain | `cargo-deny` + resolver (built) | **yes** |
| 2 native Tier-2 enforcement | FFI Tier 2 capability layer | **blocked** on that layer |
| 3 fail-closed isolation | `ipe_sandbox` (partly built) | v1 buildable; hardening incremental |

So the **entire universal Tier-1 gate + `ipe package audit` is buildable now**;
only the native-enforcement layer waits on the FFI Tier 2 capability work.

---

## 6. Phase breakdown (for the implementation lane)

1. **`ipe package audit` skeleton + Tier-1 wiring** — the command, resolving the
   package + its previous published version, and the four Tier-1 checks calling
   existing entry points (panic-scan, capability inference + manifest compare,
   `check_semver_bump`, cargo-deny). One reject diagnostic per check. Fixture: a
   clean package passes; a package with an undeclared `network`, an under-bump,
   and a `panic!` in author FFI Rust each reject with the right diagnostic.
2. **Index-CI gate** — the curated index repo runs `ipe package audit` on every
   proposed version; the entry merges only when green. The emitted-Rust panic-scan
   provenance case (compiler bug) routes to our CI, not the author's.
3. **Native Tier-2** — sandboxed native build + declared-capability fail-closed
   enforcement, landing with the FFI Tier 2 capability layer.
4. **Fail-closed isolation hardening** — namespace/prlimit scoping of declared
   capabilities at run time; seccomp is a later refinement.

Phases 1–2 deliver a working gate for pure-Ipê and hash/semver/capability-honest
packages immediately; 3–4 extend it to native packages as the FFI capability
layer matures.

---

## 7. Open questions

- **Emitted-Rust provenance boundary.** The scan must distinguish our generated
  code from author Rust precisely (the `ModuleOrigin::FfiInterface` boundary marks
  it). A misattribution turns a user error into a false compiler-bug alarm or vice
  versa — the boundary must be exact.
- **Semver baseline availability.** The enforced-semver check needs the previous
  published version's public API; decide whether the index stores an API snapshot
  per version or the gate rebuilds it from the pinned source.
- **Audit reproducibility.** The author's local audit and the index CI must reach
  the same verdict — pin the toolchain/scanner version the gate runs with.
