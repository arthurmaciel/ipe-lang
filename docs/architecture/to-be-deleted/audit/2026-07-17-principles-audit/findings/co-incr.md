# CO-INCR findings

9 findings: 0 critical, 2 high, 2 medium, 5 low.

Scope audited: `src/compiler/db/**` (salsa database, metadata, sync/cancellation
seams), `src/compiler/watch/**` (scope, coalesce, process, signal), `src/lsp/**`
(server main loop, loader seam, features: offset/diagnostics/hover/links/
folding/symbols), `src/stdlib/**/*.ipe` (all 43 embedded stdlib modules;
deep-read: Task, Random, List, Money, String, Result, Maybe, Cache, Email,
Http, Crypto, System, File, Io, Time, WebSocket, Config, Pure). Driver seams
read for reachability only: `src/ipe-cli/src/{lib,watch,cache,stdlib}.rs`,
`src/runtime/rust/src/{math,basics,money}.rs`.

## CO-INCR-001 · `Money.allocate 0` aborts the whole process (divide-by-zero panic)
- severity: high
- axis: soundness
- principle: P3 soundness — "a well-typed Ipê program can never trigger a runtime failure"; §0 no shortcuts (guard dropped in port)
- location: `src/stdlib/Ipe/Money.ipe:432-441` (`base = totalMinor // parts`); panic site `src/runtime/rust/src/math.rs:78-83` (`ipe_int_div` panics on `b == 0`)
- reachability: `Money.ipe` is embedded stdlib injected into every build (`src/ipe-cli/src/stdlib.rs`). Any app computing `parts` from data — e.g. `Money.allocate (List.length participants) total` with an empty list, where the list derives from a request — calls `totalMinor // 0`, which is the intentional DivisionByZero abort (exit 101). In an Ipe.Live/Http.Server app this kills the entire server process from one request.
- problem: the pure-Ipê port of `allocate` dropped the `parts <= 0` guard that the runtime kernel it replaced enforces (`money_allocate`, `src/runtime/rust/src/money.rs:272-274`, returns `[]`). Upstream `../ipe` routes `allocate` to that guarded kernel (`Money_allocate` via `Ffi.callPure`); the port silently regressed the guard away.
- fix direction: restore the guard in the `.ipe` body — `if parts <= 0 then [] else …` — matching the kernel and upstream behaviour.
- prior: related surface to runtime-audit-verdict's "guard … allocate residue" (money.rs) — that runtime fix landed; this is a NEW regression in the pure-source port.

## CO-INCR-002 · `Money.allocate` mints money on negative amounts (sum ≠ input)
- severity: high
- axis: correctness
- principle: P2 correctness; three-rules "make invalid states unrepresentable" (documented invariant not upheld)
- location: `src/stdlib/Ipe/Money.ipe:437-453` (`base = totalMinor // parts`, `extra = modBy parts totalMinor`, `allocateHelp`)
- reachability: any refund/credit flow — `Money.allocate 3 (Money -100 minor USD)`.
- problem: `//` truncates toward zero (`ipe_int_div`, `src/runtime/rust/src/math.rs:70-82`) while `modBy` is Euclidean-adjusted non-negative for a positive divisor (`basics_mod_by`, `src/runtime/rust/src/basics.rs:16-23`). The pairing is inconsistent: for `totalMinor = -100, parts = 3`, `base = -33`, `extra = 2` → parts are `[-32, -32, -33]`, summing to **-97**, violating the module's own contract ("The sum of returned parts equals the input exactly", line 429) and creating 3 cents. The replaced runtime kernel handles this correctly by distributing the residue toward zero by sign (`src/runtime/rust/src/money.rs:310-322`).
- fix direction: compute `extra = totalMinor - base * parts` and step each of the first `abs extra` parts by `sign extra` (mirror the kernel), instead of pairing trunc-div with Euclidean `modBy`.
- prior: new (same port regression family as CO-INCR-001).

## CO-INCR-003 · `Money.add`/`sub` silently return the left operand on currency mismatch
- severity: medium
- axis: correctness
- principle: P2 correctness — "swallowed errors (present-but-wrong defaulted to a trusted value)"; pinned default "Errors — `Result Error a` … never"
- location: `src/stdlib/Ipe/Money.ipe:395-408` (`add`, `sub`); amplified by `sumOf` (457-469) and by `compare`/`lt`/`lte`/`gt`/`gte` (474-506) which ignore currency entirely
- reachability: any app mixing currencies: `Money.add (usd 10) (eur 5)` returns `$10` with no error; `Money.sumOf USD mixedList` silently drops every non-USD entry; `Money.lt (usd 5) (eur 6)` compares raw decimals across currencies.
- problem: a wrong monetary result is produced silently instead of an error value. This matches upstream (`../ipe/ipe-stdlib/Std/Money.ipe:304-317`), so it is parity — but it is an unflagged silent-wrong-money default in the flagship "never raw Float for currency" module, exactly the swallowed-error class the correctness axis names. No divergence record sanctions keeping it, and no doc warns the caller.
- fix direction: `Result Error Money` arithmetic (or a same-currency witness type); if Go parity is retained short-term, record it in the divergence/limitation ledger and document the mismatch behaviour in the module doc.
- prior: new.

## CO-INCR-004 · wasm security gate (IPE-N0029) is convention-attached, not a dependency of the emit queries
- severity: low
- axis: completeness
- principle: THE SEAL — "every new acceptance path … fails closed at ipe time"; make-invalid-states-unrepresentable
- location: gate at `src/ipe-cli/src/lib.rs:800-816` (inside `compile_prepared`); ungated queries `src/compiler/db/src/lib.rs:829` (`emit_project`), `:993` (`emit_manifest`), `:916`/`:950` (`emit_spine_file`/`emit_rust_file`)
- reachability: no end-user bypass today — the CLI (one-shot and watch) always routes through `compile_prepared`, and both on-disk cache tiers are target-keyed (`src/ipe-cli/src/cache.rs:161,222`, deliberately, per the comment "keeps the wasm Layer-1 gate … unskippable") with FFI builds disabling the cache (`lib.rs:446`). The bypass is structural: any direct demand of `emit_project`/`emit_manifest` with a `WasmClient` `BuildConfig` (tests do this; a future warm driver, LSP code-action, or alternate front-end would) emits a complete wasm bundle containing server-only kernels with no diagnostic.
- problem: the target-keyed security gate is not part of the query graph it protects; the fail-closed property holds by call-site convention across three separate mechanisms (driver ordering, cache key fields, cache-disable flag) rather than by construction at the one place emission is decided.
- fix direction: demand a memoized `wasm_gate(db, root, entry)`-style query (or run `check_wasm_client`) inside `emit_project`/`emit_spine_file` when `config.target == WasmClient`, so every emission path fails closed.
- prior: new.

## CO-INCR-005 · `ipe watch` never wires FFI: FFI-using projects cannot watch
- severity: medium
- axis: completeness
- principle: P5 completeness — "a claimed capability that … partially works"; P2 (watch diverges from `ipe build` on the same project)
- location: `src/ipe-cli/src/watch.rs:718-765` (FsBatch arm injects only `inject_compiled_std_closure` and builds `BuildConfig::new(&db_main, driver, None, Target::Native)`); contrast one-shot driver `src/ipe-cli/src/lib.rs:429-446` (FFI catalog load + `inject_interfaces` + `assemble_emit`)
- reachability: any project with installed FFI crates (`.ipe/cache/ffi/rust` present): `ipe build` succeeds, `ipe watch` red-loops — the `Rust.<Crate>` interface modules are never injected, so every `import Rust.Foo` resolves Unresolved and canonicalisation emits IPE-N0020; even if sources somehow compiled, `ffi = None` would emit a project missing the FFI dep lines.
- problem: watch is silently a non-FFI-only feature; the failure mode (missing-module diagnostic on a module the batch build accepts) misleads the user toward their own code. INV-3 keeps the last-good binary up, so no soundness impact.
- fix direction: run the same FFI catalog/injection/assemble step per resolve cycle in `resolve_project_sources` (or once at watch start with `.ipei`-change re-resolution — the scope already watches `.ipei`/`kernel.json` per `scope.rs`'s H13 comment, so the observation half exists; only the injection half is missing).
- prior: new.

## CO-INCR-006 · warm-db reuse ships in production against a test-only precondition (byte-identity unguarded)
- severity: low
- axis: correctness
- principle: P2 correctness (SEAL byte-identity pin); documentation-vs-code invariant drift
- location: `src/compiler/db/src/lib.rs:30-35` ("Warm-db reuse stays confined to tests until the clean-vs-incremental parity gate exists"); production warm reuse at `src/ipe-cli/src/watch.rs:667-798` (one `db_main` across all generations) and `src/lsp/server/src/main_loop.rs:33` (long-lived `State.db`)
- reachability: every `ipe watch` rebuild after the first; every LSP session.
- problem: symbol numbering depends on query demand order; warm re-execution interns newly-introduced identifiers at the tail of an already-populated interner, so a warm rebuild's emitted bytes can differ from a clean `ipe build` of the same sources wherever emission ordering is `Symbol`-id-keyed. No clean-vs-incremental parity gate exists to bound this, yet watch writes warm-emitted output to disk and runs it. No concrete wrong-behaviour output was demonstrated (resolved strings, not ids, dominate emission) — filed as a smell: the stated invariant is not enforced, and the doc claim is false as written. The LSP never emits, so it is unaffected beyond diagnostics.
- fix direction: either land the parity gate the doc reserves, or correct the doc and add a watch-mode test pinning warm-vs-cold emitted-byte equality for an identifier-adding edit.
- prior: new.

## CO-INCR-007 · LSP: transient project-load failure collapses the session to single-file and clears real diagnostics
- severity: low
- axis: correctness
- principle: P2 — transient failure must not present as healed state
- location: `src/lsp/server/src/main_loop.rs:367-400` (`ensure_project_fresh` error arm), `:422-454` (`sync_inputs` rebuilds `desired` from the shrunken layout), `:564-593` (`publish` sends clearing pushes for URIs that left the layout)
- reachability: any `DidSave`/`DidClose`/`DidChangeWatchedFiles` during a transient loader failure (file mid-rename, momentary I/O error, git branch switch).
- problem: on a load error the state degrades to a one-module fallback; `sync_source_root` then REMOVES every other module from the salsa root, and the next publish clears their diagnostics (they "left the project") while the fallback file gains bogus unresolved-import diagnostics. Recovery requires a subsequent edit. A transient error thus paints a false clean/false red picture across the workspace.
- fix direction: on load failure with a previously-good layout, keep the prior layout (retry later) instead of adopting the single-file fallback; reserve the fallback for the never-loaded case.
- prior: new.

## CO-INCR-008 · watch: a transient resolve failure kills the in-flight cargo build and drops the rebuild
- severity: low
- axis: correctness
- principle: P2 — a superseding batch that fails to start must not discard the cycle it superseded
- location: `src/ipe-cli/src/watch.rs:687-716` (generation bumped and `cargo_child` killed BEFORE `resolve_project_sources`; error arm `continue`s)
- reachability: an FS batch landing while a source file is mid-write in no-manifest mode (entry parse needed for module-path discovery) or during a momentary read failure.
- problem: the previous generation's in-flight compile/cargo results now fail the `g != generation` staleness check and are silently dropped, the almost-finished cargo build was already killed, and no new cycle is scheduled — the save is lost until the next filesystem event. INV-3 holds (last-good binary stays up), but the pipeline wedges idle on a red note that never self-retries.
- fix direction: on resolve failure, either don't bump/kill until resolution succeeds, or schedule a retry `FsBatch` after a short delay.
- prior: new.

## CO-INCR-009 · watch scope: any path with a `tests` component is watch-relevant — app-written test artifacts can self-trigger a rebuild loop
- severity: low
- axis: correctness
- principle: P1/P2 — the watcher must not observe its own (or its child's) output; INV-4 confined scope
- location: `src/compiler/watch/src/scope.rs:283-292` (`is_watchable_leaf` accepts ANY component named `tests`, any extension, anywhere under root), `:254-275` (`is_relevant`)
- reachability: a supervised app (or test run) that writes files under any in-root `tests/` directory — e.g. golden outputs, `tests/output.log` — while `ipe watch` runs: write → relevant event → rebuild → restart → app writes again → loop. The exclusion list (`target`, `out`, `.ipe`, …) does not cover it.
- problem: the "tests/ fixture assets count" intent (root-level `tests/`) is implemented as a global any-component match with no extension filter, so a churning non-source artifact inside any `tests` directory keeps the rebuild loop hot; combined with `count_source_files` counting every file (not just `.ipe`, `scope.rs:294-334`), the H18 bound also miscounts assets as "source files" in its refusal message.
- fix direction: restrict the `tests` rule to the root-level `tests/` watch root and/or to source extensions; count only `.ipe` files toward `MAX_WATCHED_FILES`.
- prior: new.
