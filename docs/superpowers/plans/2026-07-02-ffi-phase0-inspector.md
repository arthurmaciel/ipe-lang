# Implementation Plan — FFI Phase 0: Inspector Hardening (task #40)

Source design (already GO'd): `docs/architecture/ffi-subsystem-design.md`
§"Inspector-hardening slice (parallel-startable)" + §"Hard constraints on the
slice". This plan does **not** redesign; it turns that section (tasks B0.0–B0.3 +
the `--git`/`safe_crate_name` entry gate) into a mechanical, TDD, task-by-task
sequence. Every anchor below was **re-verified against HEAD** (`691e275`); where
the spec's line numbers had drifted they are corrected inline and flagged
`[anchor re-verified]`.

Reference: `../sky` is the Haskell generator that consumes the inspector's
`PkgInfo` JSON. It is a **parity/capability reference** — the inspector's wire
contract must stay byte-compatible with what `../sky` (and the in-flight M-A
decode design) expects. What differs here: this slice hardens the inspector's
*internal* parse to fail-closed and de-workspaces it for isolated builds; the
wire shape itself is **frozen** (see Global Constraints).

**Scope boundary (ship-gate honesty).** This slice hardens the inspector's
*parse* against adversarial JSON (soundness risk #3 — DoS). It does **NOT** make
`ipe add` safe against an untrusted crate — that is the `ipe_sandbox` slice
(#41, design spec §A), a separate lane. Ship B0 with a doc note to that effect
(Task 6). Distinct also from #42 (the Haskell→Rust generator port).

---

## Goal

Make the vendored Rust-crate inspector `tools/sky-ffi-inspect-rs`:

1. **Disjoint** — its own `target/`, `Cargo.lock`, and dir-scoped
   `rust-toolchain.toml`, so a `cargo build`/`cargo test` of it neither rebuilds
   against nor locks with the ~15 GB workspace `target/` (today it is a
   workspace member — `Cargo.toml:19` — so the "safe to build in isolation"
   claim is **literally false**).
2. **Reproducible** — nightly toolchain pinned (rustdoc JSON is nightly-only and
   is the byte-diff determinism anchor), `Cargo.lock` committed, deps pinned,
   CI rebuilding from the pin.
3. **Fail-closed** — every `unwrap`/`expect`/`panic` on a path touching decoded
   rustdoc JSON (**42 `unwrap` + 57 `expect` + 31 `panic` = 130 sites**
   `[anchor re-verified: counts exact at HEAD]`) becomes either a genuine
   invariant assertion or an over-drop that pushes to `errors: Vec<String>`
   (`main.rs:451`) and exits non-zero — **never aborts the process**.
4. **Adversarial-JSON proof** — a property/fuzz target feeding malformed +
   adversarial rustdoc JSON asserts *no panic, bounded memory, error-`PkgInfo`
   out*.
5. **Entry-gated** — `safe_crate_name` (`main.rs:3756`) and a git-URL
   scheme/host charset check enforced at the inspector's own entry as
   defense-in-depth (the full driver-side gate is M-F, not here).

## Architecture

Single crate, no new modules. Changes are confined to `tools/sky-ffi-inspect-rs/`
**except** B0.0, which edits the shared root `Cargo.toml` (`members` +
`exclude`). Data flow of the hardened parse is unchanged:

```
rustdoc JSON bytes
  → serde_json::from_str::<serde_json::Value>   (main.rs:848 in inspect_crate)
  → parse_rustdoc(&Value, crate, version) -> PkgInfo   (main.rs:1561)   ← the total decode
  → PkgInfo { …, errors: Vec<String> }          (main.rs:439/451)       ← fail-closed channel
  → serde_json::to_string_pretty                (main.rs:640)
  → stdout + process exit code                  (main.rs:479 fn main)    ← B0.2 adds non-zero on errors
```

`parse_rustdoc` (`main.rs:1561`) is a **pure `Value → PkgInfo`** function (no
cargo spawn), so it is the direct target for the B0.3 property test — no sandbox
or network needed to fuzz the parse.

## Tech Stack

Rust 2021 (the inspector's own edition — `Cargo.toml:4`, self-contained package
metadata, does **not** inherit `workspace.package`). Deps: `serde`, `serde_json`,
`tempfile` (`Cargo.toml:7-10`). Test runner: `cargo test` (in-crate `#[cfg(test)]`
modules — e.g. `test_safe_crate_name` at `main.rs:14133`, the MODELLABLE_5 drift
fence at `main.rs:12962-12971`). Lint gate: `cargo clippy`. Property testing:
`proptest` (dev-dependency, pinned) driving `parse_rustdoc` — deterministic and
runs inside `cargo test` (cargo-fuzz noted as an optional follow-on, not
required for acceptance).

## Global Constraints

**PRINCIPLES order — apply in this priority when any step forces a trade-off:**

1. **Security** — the inspector is a trust boundary over attacker-influenced
   rustdoc JSON. A parse that can be driven to `panic`/OOM by a hostile crate is
   a DoS surface. Entry-name/git-URL gates kill path-traversal/injection at the
   inspector's own door.
2. **Correctness** — the wire `PkgInfo` a **well-formed** crate produces must be
   **byte-identical** before and after this slice. No B0 change may alter *which
   symbols are dropped* on a well-formed crate (that perturbs the downstream
   M-A/M-B byte-diff fixtures).
3. **Soundness** — no path from decoded JSON reaches an `abort()`/`panic`. An
   ill-formed parse resolves to an over-drop (symbol omitted) + an `errors`
   entry, never a process death.
4. **Efficiency** — bounded memory on adversarial input (huge arrays, deep
   nesting). Below correctness/soundness — a slow-but-total parse beats a
   fast-but-abortable one.
5. **Completeness** — over-drop is the sanctioned failure direction; a dropped
   binding is a completeness bug, never a soundness bug.
6. **Readability** — last. The over-drop keystone comments
   (`main.rs:812,1667,1965,2950,4578,4634,4670` `[anchors re-verified]`) survive
   **verbatim**.

**Two fundamental rules (non-negotiable):**

- **Parse, don't validate.** The inspector's trusted surface is the JSON decode.
  After `parse_rustdoc` returns, a symbol is either present-and-well-formed or
  absent — there is no "present but suspect" state re-checked downstream. B0.2
  converts every `unwrap`/`expect`/`panic` reachable from decode into this
  parse-time verdict (drop → `errors.push`), so no downstream step re-validates.
- **Make invalid states unrepresentable.** A crate that names a symbol
  `"; rm -rf ~"` cannot pass `safe_crate_name` → it never reaches a
  `Command`/path. The git source is a closed `GitSource { url, rev, branch, tag }`
  (`main.rs` ~730) with a scheme/host gate at construction; an unparseable URL is
  refused, not defaulted.

**Wire-contract freeze (hard).** B0 is internal robustness + reproducibility
**only**. Any change to the `PkgInfo`/`FnInfo` wire shape (`main.rs:439-451` and
the fn/module DTOs) desyncs the M-A decode design and is **prohibited**. The
`errors: Vec<String>` field **stays `String`** — it is the tools-crate internal
fail-closed channel, exempt from the no-`Result String` rule (design spec
§"Hard constraints").

**MODELLABLE_5 fence untouched.** `MODELLABLE_5` (`main.rs:411`), `MARKER_TRAITS`
(`main.rs:7555`), and the drift-fence test (`main.rs:12962-12971`) must survive
this slice byte-for-byte, so the M-E two-way fence (#42) closes with no inspector
edit. A grep-check guards this (Task 3 step 5).

**Fail-closed, not panic.** Every converted site pushes to `errors` and drops the
symbol; `main` exits non-zero when `errors` is non-empty. No `unwrap`/`expect`/
`panic!`/`unreachable!` survives on a decode path.

## Parallel-safety / file-overlap analysis

| Lane | Files it touches | Overlap with this slice |
|---|---|---|
| **This slice (B0.1–B0.3, Task 6)** | `tools/sky-ffi-inspect-rs/**` only | — |
| **B0.0 (Task 0)** | root `Cargo.toml` (`members` L19 + new `exclude`) **+** `tools/sky-ffi-inspect-rs/Cargo.toml` | **root `Cargo.toml` is the sole contention point** |
| **Registry migration** (commit `691e275`, #41-adjacent) | `crates/sky_kernels/src/lib.rs`, `crates/sky_canon/src/resolve.rs:1116-1175`, `crates/sky_lower/src/lower.rs:271`, `crates/sky_ir/src/ir.rs` | **none** — disjoint from `tools/` |
| **#49 TCO** | `crates/sky_ir/src/ir.rs` (+2 variants), `crates/sky_lower/src/lower.rs`, `crates/sky_backend_rust/src/emit_expr.rs` | **none** — disjoint from `tools/` |

**Conclusion.** Registry and TCO lanes edit `crates/**` internals; neither edits
root `Cargo.toml`'s `members`/`exclude`. So the only cross-lane hazard is B0.0's
one-line-plus-`exclude` edit to root `Cargo.toml`. **Run B0.0 when the primary
build lane is idle** (spec directive — B0.0 is *not* worktree-isolatable because
it mutates the shared manifest). After B0.0 lands, tasks B0.1–B0.3 + Task 6 are
fully worktree-isolatable and block nothing / are blocked by nothing.

---

## Task 0 — B0.0: de-workspace the inspector (BLOCKING prerequisite)

**Files:** `Cargo.toml` (root), `tools/sky-ffi-inspect-rs/Cargo.toml`.

**Spec ambiguity resolved.** The spec says "remove from `members`". That **alone**
makes cargo error: *"current package believes it's in the workspace but is not a
member"* whenever you run `cargo` inside `tools/sky-ffi-inspect-rs/`. The
mechanical, complete de-workspacing is three coordinated edits:
1. remove `"tools/sky-ffi-inspect-rs"` from root `members` (`Cargo.toml:19`);
2. add `exclude = ["tools/sky-ffi-inspect-rs"]` to the root `[workspace]`;
3. add an **empty `[workspace]` table** to
   `tools/sky-ffi-inspect-rs/Cargo.toml` — this promotes it to its own workspace
   root, giving it its own `target/` and `Cargo.lock` automatically.

The inspector already carries self-contained package metadata (`edition`,
`version`, `description` are literals, not `.workspace = true` —
`Cargo.toml:1-5` `[anchor re-verified]`), so no metadata breaks when it leaves
the workspace.

### Interfaces

```
Consumes: root Cargo.toml [workspace] { members: [.., "tools/sky-ffi-inspect-rs"] }  (L3-20)
          inspector Cargo.toml [package] (self-contained, no .workspace keys)         (L1-10)
Produces: root Cargo.toml [workspace] { members: [.. minus inspector], exclude: ["tools/sky-ffi-inspect-rs"] }
          inspector Cargo.toml with a trailing empty `[workspace]` table
          → inspector builds into tools/sky-ffi-inspect-rs/target/ with its own Cargo.lock
```

### Steps

1. **Write failing check.** From repo root, confirm the inspector currently
   shares the workspace (baseline):
   ```
   cargo metadata --format-version 1 --no-deps --manifest-path tools/sky-ffi-inspect-rs/Cargo.toml \
     | python3 -c "import json,sys; m=json.load(sys.stdin); print(m['workspace_root'])"
   ```
   Expected **now**: prints the **repo root** `/home/arthur/Documentos/comp/sky-rust`
   (proves it is a member of the root workspace — the bug).
2. **Edit root `Cargo.toml`.** Remove line 19 (`    "tools/sky-ffi-inspect-rs",`)
   from `members`, and after the `members = [ … ]` array add:
   ```toml
   exclude = ["tools/sky-ffi-inspect-rs"]
   ```
3. **Edit `tools/sky-ffi-inspect-rs/Cargo.toml`.** Append at end of file:
   ```toml
   # Own workspace root: keeps the inspector's target/ and Cargo.lock disjoint
   # from the ~15 GB compiler workspace so it builds in isolation (design spec
   # §"Inspector-hardening slice", B0.0).
   [workspace]
   ```
4. **Run — passes.** Re-run the step-1 command; expected **now**: prints
   `/home/arthur/Documentos/comp/sky-rust/tools/sky-ffi-inspect-rs` (its own
   workspace root). Then confirm the root workspace no longer sees it:
   ```
   cargo metadata --format-version 1 --no-deps 2>/dev/null \
     | python3 -c "import json,sys; m=json.load(sys.stdin); print(any('sky-ffi-inspect-rs' in p for p in m['workspace_members']))"
   ```
   Expected: `False`.
5. **Prove isolated build uses its own target/.**
   ```
   cd tools/sky-ffi-inspect-rs && cargo build 2>&1 | tail -3 && ls target/debug/sky-ffi-inspect-rs
   ```
   Expected: `Finished` line + the binary path resolves under
   `tools/sky-ffi-inspect-rs/target/debug/` (a fresh, own lock:
   `tools/sky-ffi-inspect-rs/Cargo.lock` now exists).
6. **Commit** `B0.0: de-workspace sky-ffi-inspect-rs (own target/ + Cargo.lock + workspace root)`.

---

## Task 1 — B0.1: reproducibility pin (toolchain + lock + deps + CI)

**Files:** `tools/sky-ffi-inspect-rs/rust-toolchain.toml` (new),
`tools/sky-ffi-inspect-rs/Cargo.toml`, `tools/sky-ffi-inspect-rs/Cargo.lock`
(committed), `.github/workflows/*` (new inspector job).

**Why nightly.** rustdoc JSON output (`--output-format json`) is nightly-only;
the exact channel is both the drift-fence anchor and the byte-diff determinism
anchor for the whole FFI subsystem. The vendoring dropped the toolchain file +
lockfile — this restores that regression.

### Interfaces

```
Consumes: (none — reproducibility metadata)
Produces: rust-toolchain.toml { toolchain.channel = "nightly-YYYY-MM-DD", components = ["rustc","cargo","rust-docs"] }
          Cargo.toml deps pinned to exact "=X.Y.Z"
          Cargo.lock committed (git-tracked, previously ignored/absent)
          CI job "ffi-inspector" building + testing from the pin
```

### Steps

1. **Write failing check.** Confirm no toolchain pin exists yet:
   ```
   test -f tools/sky-ffi-inspect-rs/rust-toolchain.toml && echo PRESENT || echo MISSING
   ```
   Expected **now**: `MISSING`.
2. **Create `tools/sky-ffi-inspect-rs/rust-toolchain.toml`.** Use the nightly
   channel the inspector is known to build rustdoc JSON with (pick the current
   pinned nightly; record the exact date):
   ```toml
   [toolchain]
   channel = "nightly-2026-06-15"
   components = ["rustc", "cargo", "rust-docs"]
   profile = "minimal"
   ```
   *(Replace the date with the nightly verified in step 4; the point is an
   **exact** pin, not `nightly` floating.)*
3. **Pin deps exactly** in `tools/sky-ffi-inspect-rs/Cargo.toml`:
   ```toml
   [dependencies]
   serde = { version = "=1.0.219", features = ["derive"] }
   serde_json = "=1.0.140"
   tempfile = "=3.20.0"
   ```
   *(Use the versions already resolved in the generated `Cargo.lock` from Task 0
   step 5 — read them with `cargo tree -p serde -p serde_json -p tempfile` and
   pin to those, so the pin does not force a re-resolve.)*
4. **Run — passes.** Build under the pinned toolchain and commit the lock:
   ```
   cd tools/sky-ffi-inspect-rs && rustup toolchain install nightly-2026-06-15 --profile minimal -c rust-docs \
     && cargo +nightly-2026-06-15 build 2>&1 | tail -2 && git add -f Cargo.lock
   ```
   Expected: `Finished` + `Cargo.lock` staged (force-add in case a broad
   `.gitignore` excludes `Cargo.lock`; verify with `git status --short Cargo.lock`).
5. **Add CI job.** In the CI workflow add an `ffi-inspector` job that
   `cd tools/sky-ffi-inspect-rs`, installs the pinned nightly, and runs
   `cargo build --locked` + `cargo test --locked`. `--locked` fails if the
   committed lock drifts — that is the reproducibility gate.
6. **Commit** `B0.1: pin nightly toolchain + exact deps + commit Cargo.lock + CI job`.

---

## Task 2 — B0.2a: fail-closed the process exit (the found gap)

**Files:** `tools/sky-ffi-inspect-rs/src/main.rs` (`fn main` at `:479`, and the
inspection driver that returns `PkgInfo`).

**Spec ambiguity resolved (a real gap found).** The design's B0.2 says "on any
internal parse failure … exit non-zero". But at HEAD, `fn main` (`main.rs:479`)
returns `()` and **always exits 0** — it prints the JSON (populated `errors`
included) and falls off the end. So even today, a crate that fully fails to parse
returns exit 0 with an error-`PkgInfo`. Realizing the spec's "exit non-zero"
requires an explicit exit-code decision. This task adds it **before** the lint
flip so the flip's error-path rewrites have a correct process contract to land in.

### Interfaces

```
Consumes: results: Vec<PkgInfo>  (main.rs:630-633)  where PkgInfo.errors: Vec<String> (main.rs:451)
Produces: process exit code: 0 iff every PkgInfo.errors is empty, else 1
          (JSON still printed to stdout unchanged — the wire body is untouched)
```

### Steps

1. **Write failing test.** Add an in-crate test that runs the binary against a
   crate spec guaranteed to error (e.g. a bogus crate name that fails
   `safe_crate_name`) and asserts a non-zero exit. Since spawning the built
   binary in a unit test is heavy, instead extract the exit decision into a
   testable pure fn and test *that*:
   ```rust
   #[cfg(test)]
   mod exit_code_tests {
       use super::*;
       #[test]
       fn nonzero_when_any_errors_present() {
           let mut p = PkgInfo::default_for_test("x"); // helper: empty PkgInfo
           p.errors.push("boom".into());
           assert_eq!(exit_code_for(&[p]), 1);
       }
       #[test]
       fn zero_when_clean() {
           let p = PkgInfo::default_for_test("x");
           assert_eq!(exit_code_for(&[p]), 0);
       }
   }
   ```
   Run: `cargo test exit_code_tests` → expected **fail to compile** (`exit_code_for`
   and `default_for_test` do not exist).
2. **Minimal impl.** Add above `fn main`:
   ```rust
   /// Fail-closed process contract: any populated `errors` channel (an over-drop
   /// or a hard parse failure) exits non-zero so a caller/CI can detect it.
   fn exit_code_for(results: &[PkgInfo]) -> i32 {
       if results.iter().any(|p| !p.errors.is_empty()) { 1 } else { 0 }
   }
   ```
   Add the `default_for_test` helper under `#[cfg(test)]`. Then at the end of
   `fn main`, after the JSON is printed, call
   `std::process::exit(exit_code_for(&results));` (thread `results` — it is in
   scope at `main.rs:631`).
3. **Run — passes.** `cargo test exit_code_tests` → `test result: ok. 2 passed`.
4. **Guard the wire body is unchanged.** The JSON printed to stdout is emitted
   *before* the exit call — confirm no test in the suite that asserts stdout
   shape regresses: `cargo test 2>&1 | tail -5` → all green.
5. **Commit** `B0.2a: exit non-zero when the inspector's errors channel is populated`.

---

## Task 3 — B0.2b: flip the three lints to deny; drive 130 sites to zero

**Files:** `tools/sky-ffi-inspect-rs/Cargo.toml` (`[lints.clippy]` L12-24),
`tools/sky-ffi-inspect-rs/src/main.rs` (~130 call sites).

**Spec framing (a REVERSAL, not an addition).** The inspector's own
`[lints.clippy]` block (`Cargo.toml:12-24` `[anchor re-verified]`) **deliberately
sets** `unwrap_used = "allow"` / `expect_used = "allow"` / `panic = "allow"`
(L17-19) with a justifying comment. This block **overrides** the workspace deny
set (the package does not use `[lints] workspace = true`), so today the inspector
compiles clean *despite* the 130 sites. B0.2b **reverses that prior decision** —
flip the three to `deny` — and records why the original `allow` no longer holds
(the inspector is now a hardened trust boundary, not "just a tool"). The flip
exposes exactly **42 `unwrap` + 57 `expect` + 31 `panic` = 130** sites to drive
to zero.

### Interfaces

```
Consumes: [lints.clippy] { unwrap_used="allow", expect_used="allow", panic="allow" }  (Cargo.toml:17-19)
          ~130 call sites on decode paths in main.rs
Produces: [lints.clippy] { unwrap_used="deny", expect_used="deny", panic="deny" }
          every decode-reachable fallible op → over-drop (errors.push + skip) OR a genuine invariant
          `cargo clippy` exits 0 under the deny set
```

### Steps

1. **Write failing gate.** Flip the three lines in `Cargo.toml:17-19`:
   ```toml
   unwrap_used = "deny"
   expect_used = "deny"
   panic = "deny"
   ```
   Replace the justifying comment (L13-16) with the reversal rationale:
   ```toml
   # Hardened trust boundary over attacker-influenced rustdoc JSON (design spec
   # B0.2): unwrap/expect/panic are DENIED so no adversarial crate can abort the
   # process. Fallible decode ops push to `errors` and over-drop the symbol.
   ```
   Run: `cd tools/sky-ffi-inspect-rs && cargo clippy --all-targets 2>&1 | rg -c '^error'`
   Expected **now**: a large count (≈130 across lib+tests; test-only sites may be
   allowed per step 4).
2. **Mechanical rewrite pattern (per site, decode paths).** For each denied site
   reachable from `parse_rustdoc` (`main.rs:1561`) / `parse_fn_item`
   (`main.rs:3903`) / `rustdoc_type_to_sky` (`main.rs:6380`) / the item walkers:
   - `x.unwrap()` on an `Option` where absence means "cannot bind this symbol"
     → `let Some(x) = opt else { errors.push(format!("drop <sym>: <why>")); return None; }`
     (or `continue` in a loop that collects `FnInfo`) — an **over-drop**.
   - `x.expect("msg")` on a `Result` → `match x { Ok(v)=>v, Err(e)=>{ errors.push(format!("drop <sym>: {e}")); return None; } }`.
   - `panic!("…")` / `unreachable!()` reachable from decode → replace with the
     over-drop return + `errors.push`.
   Distinguish a **genuine invariant** (a state the parse structurally cannot
   reach — e.g. an index into a slice just `push`ed to) from a decode-driven
   failure. For a genuine invariant, prefer restructuring so the impossibility is
   type-level; only if truly unavoidable, keep it as `unreachable!` **with an
   `#[allow(clippy::unreachable)]` and a one-line proof comment** — but the
   default answer on any decode path is over-drop, not allow.
3. **Preserve the over-drop keystone comments verbatim.** The sites at
   `main.rs:812,1667,1965,2950,4578,4634,4670` already document *why* a symbol is
   dropped — do not reword them; a converted `unwrap`→drop lands *next to*, not
   *over*, these.
4. **Test-only sites.** `#[cfg(test)]` assertions (e.g. `main.rs:14133+`,
   `12962-12971`) may legitimately `unwrap`/`assert`. Scope the deny to non-test
   code by adding `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]`
   at the crate root **only if** clippy flags test modules; production paths stay
   denied. (Confirm which sites are test-only:
   `rg -n '\.unwrap\(\)|\.expect\(|panic!' src/main.rs | rg 'test'`.)
5. **MODELLABLE_5 fence integrity check.** After the rewrite, assert the fence is
   byte-untouched:
   ```
   git diff -U0 src/main.rs | rg -n 'MODELLABLE_5|MARKER_TRAITS' && echo "FENCE TOUCHED — REVERT" || echo "fence intact"
   ```
   Expected: `fence intact` (no diff lines touch those consts).
6. **Run — passes.**
   ```
   cargo clippy --all-targets 2>&1 | rg -c '^error'   # expect 0
   cargo test 2>&1 | tail -3                            # all green
   ```
7. **Commit** `B0.2b: deny unwrap/expect/panic on decode paths; convert 130 sites to fail-closed over-drop`.

---

## Task 4 — B0.3: adversarial-JSON property/fuzz target

**Files:** `tools/sky-ffi-inspect-rs/Cargo.toml` (`[dev-dependencies]` +
optional `[[test]]`), `tools/sky-ffi-inspect-rs/tests/adversarial_json.rs` (new)
or an in-crate `#[cfg(test)]` module.

**Spec ambiguity resolved.** The spec says "fuzz/property target". Resolved to a
**deterministic `proptest` target** that calls `parse_rustdoc(&Value, "advcrate",
"0.0.0")` (`main.rs:1561`) directly — that function is the pure `Value → PkgInfo`
decode with **no cargo spawn**, so it fuzzes without sandbox/network. This is the
**acceptance test for B0.2**. `cargo-fuzz` (libfuzzer, separate `fuzz/` crate,
nightly) is an optional follow-on for continuous fuzzing — not required for
sign-off, and noted as such in Task 6's doc note.

**Visibility note.** `parse_rustdoc` must be reachable from a test. If it is
private, add `pub(crate)` and use an in-crate `#[cfg(test)] mod` (not an external
`tests/` file). Confirm: `rg -n '^fn parse_rustdoc|^pub.*fn parse_rustdoc' src/main.rs`.

### Interfaces

```
Consumes: parse_rustdoc(doc: &serde_json::Value, crate_name: &str, version: &str) -> PkgInfo   (main.rs:1561)
Produces: proptest strategy → serde_json::Value (deeply-nested / huge-array / wrong-typed / cyclic-id / non-UTF-8-escaped)
          assertions: (a) no panic (proptest catches unwinds as failures),
                      (b) bounded wall-time+alloc (size caps on the strategy),
                      (c) PkgInfo returned; on garbage input, functions == [] and/or errors non-empty
```

### Steps

1. **Write failing test.** Add `proptest` dev-dep (pinned):
   ```toml
   [dev-dependencies]
   proptest = "=1.5.0"
   ```
   Create the target (in-crate module or `tests/adversarial_json.rs`):
   ```rust
   use proptest::prelude::*;
   use serde_json::{json, Value};

   // Bounded adversarial JSON: nesting depth ≤ 8, array len ≤ 64 — so a failure
   // is a real panic, not an OOM from an unbounded strategy.
   fn adversarial_value() -> impl Strategy<Value = Value> {
       let leaf = prop_oneof![
           Just(Value::Null),
           any::<bool>().prop_map(Value::Bool),
           any::<i64>().prop_map(|n| json!(n)),
           ".*".prop_map(Value::String),
       ];
       leaf.prop_recursive(8, 256, 64, |inner| {
           prop_oneof![
               prop::collection::vec(inner.clone(), 0..64).prop_map(Value::Array),
               prop::collection::hash_map("[a-z_]{1,8}", inner, 0..64)
                   .prop_map(|m| Value::Object(m.into_iter().collect())),
           ]
       })
   }

   proptest! {
       #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]
       #[test]
       fn parse_rustdoc_never_panics(v in adversarial_value()) {
           // If parse_rustdoc panics on any of these, proptest reports the
           // minimized input as a failure. Success = total function.
           let pkg = super::parse_rustdoc(&v, "advcrate", "0.0.0");
           // Garbage in ⇒ no bindable functions escape the decode.
           prop_assert!(pkg.functions.is_empty() || !pkg.errors.is_empty());
       }
   }
   ```
   Plus a fixed corpus of hand-crafted adversarial fixtures (truncated object,
   `{"index": {"0": {"id": "0"}}}` self-cyclic id, `{"functions": <huge>}`,
   a string with lone surrogate escapes) as ordinary `#[test]` cases asserting
   `parse_rustdoc(...)` returns without panic.
   Run: `cargo test parse_rustdoc_never_panics` → **fails** if any B0.2b site was
   missed (a surviving `unwrap` panics on a shrunk input) — that failing input is
   the discovery artefact; fix the site, re-run.
2. **Minimal impl.** No new production code beyond B0.2b — this task's "impl" is
   closing whatever site the property test shrinks to. Iterate step 1 ↔ Task 3
   step 2 until green.
3. **Bounded-memory assertion.** The strategy caps (depth 8, array 64) keep each
   case bounded; add one explicit large-but-bounded fixture
   (`Value::Array` of 100_000 nulls under a `functions` key) asserting
   `parse_rustdoc` returns within a wall-clock budget
   (`std::time::Instant` guard, e.g. `< 2s`) — proves no accidental O(n²) on
   array size.
4. **Run — passes.**
   ```
   cargo test 2>&1 | rg 'adversarial|parse_rustdoc_never_panics|result:' | tail
   ```
   Expected: `test result: ok.` for all adversarial cases.
5. **Commit** `B0.3: proptest adversarial-JSON target over parse_rustdoc — no panic, bounded, error-PkgInfo out`.

---

## Task 5 — `--git` gate + `safe_crate_name` entry (defense-in-depth)

**Files:** `tools/sky-ffi-inspect-rs/src/main.rs` (`--git` arg handling `:523`,
`GitSource` struct ~`:730`, `safe_crate_name` `:3756`, entry `fn main` `:479`).

**Scope (spec-bounded).** The inspector enforces `safe_crate_name`
(`[A-Za-z0-9_-]+`, `main.rs:3756` `[anchor re-verified]`) and a **git-URL
scheme/host charset** check **at its own entry** — a testable gate independent of
the absent M-F driver. The **full** https-only + host allowlist +
rev/branch/tag mutual-exclusion belongs to the ported driver (M-F, #42) — do
**not** build the allowlist here; build the charset/scheme refusal only.

### Interfaces

```
Consumes: raw --git URL string (main.rs:523 arm), positional crate names (main.rs:578+)
          existing safe_crate_name(&str) -> Option<&str>   (main.rs:3756)
Produces: fn git_url_is_safe(url: &str) -> bool  — true iff scheme ∈ {https} AND host charset ⊆ [A-Za-z0-9.-] AND no shell metachars
          entry refuses (exit non-zero, errors.push) any crate name failing safe_crate_name
          entry refuses any --git URL failing git_url_is_safe
```

### Steps

1. **Write failing test.**
   ```rust
   #[test]
   fn git_url_scheme_and_charset_gate() {
       assert!(git_url_is_safe("https://github.com/rust-lang/log"));
       assert!(!git_url_is_safe("http://github.com/x/y"));          // not https
       assert!(!git_url_is_safe("file:///etc/passwd"));             // scheme
       assert!(!git_url_is_safe("https://h;rm -rf ~/x"));           // metachar in host
       assert!(!git_url_is_safe("git@github.com:x/y"));             // ssh form
       assert!(!git_url_is_safe("https://ho st/x"));                // space
   }
   ```
   Run: `cargo test git_url_scheme_and_charset_gate` → **fails to compile**
   (`git_url_is_safe` absent).
2. **Minimal impl.** Add near `safe_crate_name`:
   ```rust
   /// Inspector-entry defense-in-depth (design spec B0 §--git gate). NOT the
   /// full M-F driver gate (host allowlist / rev-branch-tag exclusivity live
   /// there) — only scheme=https + host charset + no shell metacharacters.
   fn git_url_is_safe(url: &str) -> bool {
       let Some(rest) = url.strip_prefix("https://") else { return false; };
       let host = rest.split(['/', '?', '#']).next().unwrap_or("");
       !host.is_empty()
           && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':')
           && !url.bytes().any(|b| matches!(b, b';' | b'|' | b'&' | b'$' | b'`' | b' ' | b'\n' | b'\t' | b'\0'))
   }
   ```
   *(The `unwrap_or("")` keeps it clippy-clean under the Task 3 deny set — no bare
   `unwrap`.)*
3. **Wire into entry.** In the `--git` arm (`main.rs:523`) and where positional
   crate names are collected: reject with an `errors.push` + non-zero exit (via
   Task 2's `exit_code_for` path) when `git_url_is_safe` / `safe_crate_name`
   fails — **before** any `Command` is constructed or any path is joined.
4. **Run — passes.** `cargo test git_url_scheme_and_charset_gate test_safe_crate_name`
   → `test result: ok. 2 passed` (the existing `test_safe_crate_name` at
   `main.rs:14133` stays green — no change to `safe_crate_name` itself).
5. **Commit** `B0: git-URL scheme/charset entry gate + safe_crate_name refusal (defense-in-depth, not the M-F driver gate)`.

---

## Task 6 — ship note + deferred-rename record

**Files:** `docs/architecture/ffi-subsystem-design.md` (a one-paragraph status
note appended under the slice section), or a short
`tools/sky-ffi-inspect-rs/README.md`.

### Steps

1. Add a **ship-gate note**: "B0 hardens the inspector's *parse* (soundness risk
   #3). It does **NOT** make `ipe add` safe against an untrusted crate — that is
   the `ipe_sandbox` slice (#41). `ipe add` must not ship to users until the
   sandbox lands."
2. Record the **deferred rename**: `sky-ffi-inspect-rs → ipe-ffi-inspect` stays
   **DEFERRED** to the post-completion namespace sweep (renaming the crate, the
   `SKY_FFI_INSPECTOR_RS` probe `sky/…/Ffi.hs:307`, the `bin/` walk-up
   `Ffi.hs:319`, the `[sky-ffi]` diagnostic prefix `Ffi.hs:149`, and cargo-fuzz
   as an optional continuous-fuzzing follow-on) — cosmetic, would churn byte-diff
   anchors mid-port.
3. **Commit** `B0: ship note (sandbox is the real ipe-add gate) + deferred-rename record`.

---

## Acceptance summary (all must hold)

1. `cargo metadata` shows the inspector as its **own** `workspace_root`; the root
   workspace no longer lists it (Task 0).
2. `rust-toolchain.toml` pins an exact nightly; `Cargo.lock` committed; CI
   `--locked` job green (Task 1).
3. `fn main` exits non-zero iff any `PkgInfo.errors` is non-empty (Task 2).
4. `cargo clippy --all-targets` exits 0 under `unwrap_used/expect_used/panic =
   "deny"`; MODELLABLE_5 fence byte-untouched (Task 3).
5. proptest adversarial target green: no panic, bounded, garbage ⇒ empty
   `functions` and/or non-empty `errors` (Task 4).
6. `git_url_is_safe` + `safe_crate_name` refuse at entry before any `Command`/path
   (Task 5).
7. **Wire freeze:** a well-formed crate's `PkgInfo` JSON is byte-identical
   pre/post-slice (spot-check one real crate, e.g. `cargo run -- log`, diff
   against a pre-slice capture).
