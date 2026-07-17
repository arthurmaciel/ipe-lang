# Principles-audit register (verified)

Whole-codebase audit for security / soundness / correctness / completeness breaks.
12 Fable auditors (per-partition) → 4 Opus adversarial judges (refute/confirm/
downgrade/dedupe). This register lists the **judge-verified** findings. Per-partition
raw findings: `findings/*.md`. Per-theme verdicts + repros: `verdicts/*.md`.

Verification coverage: the critical + every high + the security/SEAL mediums were
adversarially verified. Some low "smells" are auditor-reported only (marked ⚠︎unverified).

## PUSH-BLOCKING (verified) — 6

| ID | sev | axis | one-line | fix theme |
|---|---|---|---|---|
| **co-ffi-001** | **critical** | security | FFI decode gates identifiers but emits type/path strings VERBATIM into `<crate>_bindings.rs`, compiled UNSANDBOXED → code injection; reachable via a writable ancestor-dir cache plant (no crate publish). Also a SEAL breach. | T1 |
| **CLI-001** | high | security | `~/.cargo/credentials.toml` ro-bound past the `--tmpfs /home` mask into the network-on FFI jail → crates.io token exfil on any `cargo login`'d box. | T1 |
| **CO-TYPES-001** | high | soundness | Exhaustiveness pass seeds only Maybe/Result; a nested `case` over a Prelude builtin ADT (`ErrorKind`/`SqlValue`/`ChunkEvent`) → `ipe` exit-0 → rustc E0004. SEAL breach. | T2 |
| **CO-BACKEND-001** | high | soundness | A local shadows a bare-emitted top-level fn (`let main_update = …` in module Main) → E0618 SEAL break, or **silent wrong-call** if the local is fn-typed. `mangle_reserved` also non-injective. | T2 |
| **CO-INCR-001** | high | soundness | `Money.allocate 0` → `// 0` → process abort (exit 101); pure-`.ipe` port dropped the kernel's `parts<=0` guard; `parts` is request-derivable → one request kills the server. | T4 |
| **CO-INCR-002** | high | correctness | `Money.allocate` on negative amounts mints cents (trunc-div + Euclidean `modBy`; sum ≠ input) — flagship-module contract violation; kernel is sign-correct, port is not. | T4 |

## Confirmed high — not push-blocking

| ID | sev | note |
|---|---|---|
| CO-FRONT-001 | high | `climb_binops` recurses per operator on a flat chain (MAX_DEPTH guards nesting, not length) → stack-overflow DoS on ~300k-op `.ipe`. `ipe` CRASHES (not exit-0) → DoS, not a SEAL-contract break. Matters for the playground/CI compile path. |
| RT-TUI-001 | high | `distribute_row_fill` i64 portion-sum wraps → `str::repeat(~9e18)` panic/OOM. Real P3 hole but **local-author-only** (no remote/data blast radius). Fix, don't block. |

## Confirmed medium (advised before push)

| ID | axis | note | theme |
|---|---|---|---|
| RT-AUTH-001 | correctness | negative-int `exp` → expiry check skipped (flat decoders) → non-expiring; signer-side/Go-parity, not remote bypass. | T4 |
| RT-AUTH-002 | correctness | fractional `exp∈[0,0.5)` → `Parsed(0)` → `0u64-1` wraps under emitted `overflow-checks=false` → accepted non-expiring; any RFC-legal issuer. | T4 |
| RT-UI-001 | soundness | `render_element`/`diff_node` recurse uncapped (siblings already capped) → attacker-influenceable process abort. **One-liner fix.** | T3 |
| RT-TUI-002 | soundness | padding area = product of two clamped dims → ~10-20 GB OOM. | T3 |
| RT-NET-001 | completeness | all prod `wss://` client dials fail-closed (SSRF-default regression); fails closed, needs a divergence record. | T5 |
| RT-DATA-001 | correctness | `row_to_json`/`row_to_map` coerce undecodable Postgres columns (BOOL/BYTEA/NUMERIC/TIMESTAMP) to NULL/"" silently. | T5 |
| CO-BACKEND-002 | soundness | dead `assert_mod_idents_unique` gate (lying comment) → `Std.Ui` vs `Std_Ui` collide → E0428 + silent file overwrite. | T2 |
| CO-INCR-003 | correctness | `Money.add`/`sub` return left operand on currency mismatch; comparisons ignore currency — silent-wrong-money. | T4 |
| CO-INCR-005 | completeness | `ipe watch`/`lsp` never wire FFI → FFI projects red-loop under watch while `ipe build` succeeds. | T5 |
| RT-UI-002 | completeness | `Keyed.column/row` drop the key instead of attaching `sky-key` → wrong-element patches on reorder. | T5 |

## Downgraded / refuted (judge corrections — do NOT chase)
- **RT-LIVE-004** med→low: the O(n²) style-scrub is defended on the auditor's cited path (`SafeCssValue` rejects `</`); only reachable via uncited callers needing author cooperation — not an every-render DoS.
- **CLI-003** deferred to correctness (no security surface).

## Lows (verified + ⚠︎unverified smells)
See `findings/*.md` + `verdicts/*.md`. Notable verified lows: RT-DATA-003 (findByConditions empty→`SELECT *` fail-open, cross-tenant read), RT-NET-002/003 (handler leak, ws pre-check buffering), RT-LIVE-001 (ingest-token cleartext), CLI-002 (cache-root walk = co-ffi-001 delivery vector), CLI-007/008 (`ipe install --yes`, predictable `/tmp` jail dir), RT-AUTH-005 (stringly HS256 secret), CO-INCR-006..009 (warm-db-in-prod, LSP/watch transient-failure drops), CO-BACKEND-004..007.

## Themes → design specs (Phase 3)
- **T1 FFI trust-boundary hardening** — parse-don't-validate type/path strings at decode (newtypes); no-egress-while-executing two-phase sandbox; stop mounting `~/.cargo` secrets; cache-root stops at `sky.toml` + ownership check; hard-fail on missing timeout/prlimit caps. *(co-ffi-001, CLI-001/002/007/008, co-ffi-002/003/004)*
- **T2 SEAL-breach closure** — one shared builtin-ctor table (canon/exhaust/lower); qualify emitted top-level calls / injective mangling; revive `assert_mod_idents_unique`; ADD negative-test coverage for each (the suite missed them). *(CO-TYPES-001, CO-BACKEND-001/002)*
- **T3 Bound untrusted recursion/allocation** — iterative/depth-capped rewrites + saturating arithmetic. *(CO-FRONT-001, RT-UI-001, RT-TUI-001/002)*
- **T4 JWT-exp + Money correctness** — restore dropped guards (`parts<=0`, sign-correct allocate, currency checks); align flat/builder JWT exp parsing to Go. *(RT-AUTH-001/002/003, CO-INCR-001/002/003)*
- **T5 Data/decode + completeness** — Postgres column decode fidelity; wire FFI into watch/lsp; divergence records (wss, currency); `Keyed` key attach; route-param fail-closed. *(RT-DATA-001/003, CO-INCR-005, RT-NET-001, RT-UI-002, CO-BACKEND-003)*
