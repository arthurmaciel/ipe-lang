# Kernel registry — Phase D (per-family scheme fill)

> Implementation plan (superpowers *writing-plans* grade). Read-only design
> artifact; the tasks below are executed later, in order, by an implementer.
> Source spec (follow, do not redesign):
> `docs/architecture/kernel-registry-design.md` §Q3, §Q5 "Phase D", §Q6,
> and the "Phase C re-review" (adjustments 3 & 4 — the obligation trap and
> the parity tripwire). Umbrella task: **#45** ("Make constrain kernel-scheme
> table exhaustive over canon lists").
> Downstream consumer: `docs/superpowers/plans/2026-07-02-registry-phase-E.md`
> (its Task **E0** gate `stdlib_scheme_is_total` cannot go green until this
> plan's exit gate — Task **D-Z** — lands).

---

## Goal

Phase C stood up the dual-lookup seam: `stdlib_scheme(k) -> Option<Ty>` serves
the three migrated families (String's `fromInt`/`fromFloat`, all of List, all
of Math except `min`/`max`) and delegates every other kernel to the legacy
symbol-keyed `kernel_ty`. Phase D **fills the seam family by family** until
`stdlib_scheme` returns `Some` for **every** `StdlibKernel::ALL` variant — the
exact precondition Phase E's E0 gate asserts.

Two distinct kinds of work, both landing in `sky_types/src/constrain.rs`:

1. **Relocation** — families that already have a byte-faithful scheme in legacy
   `kernel_ty` (Maybe, Result, Bytes, Task, System, File, Http, Time, Random,
   Io, Cmd, Sub, Server, Middleware, RateLimit, Db, Db.Decode, and the
   obligation-carrying Dict / Set / Math.min-max). The arm body moves verbatim;
   the per-kernel structural-parity tripwire (`stdlib_scheme(k) ≡
   kernel_ty(decl(k).qualifier, decl(k).name)`) proves the move changed no
   type — Go-parity by construction.
2. **First-scheming** — the ~14 holed families that `kernel_ty` never typed
   (they fall to `_ => Ty::Var(u32::MAX)`: Uuid, Encoding, Jwt, JsonEnc,
   JsonDec, JsonDecP, Char, Crypto, PubSub, the 33 un-schemed String kernels,
   and the Std.Ui / Std.Html / app-entry families). Each gets its **first
   correct scheme**, authored against the runtime signature
   (`runtime/src/sky_runtime/*`) and the Go oracle. For these the parity
   tripwire is inapplicable (there is no legacy `Ty` to match) and is replaced
   by a *was-a-hole* assertion plus a green skyc→cargo build.

The constraint **obligations** (bounded super-vars that live OUTSIDE the scheme
tables in `constrain_var_kernel`) are re-keyed off the resolved `id` as their
families migrate, and pinned by reject-probes so migration cannot silently drop
a bound. This is the M4d twin of the Math `Ord` gate: Dict/Set key obligations
(`Hash + Eq + Ord`) and Math.min/max `Ord`.

Done-state: `stdlib_scheme` returns `Some` for all of `ALL`; `migrated_set_burndown`'s
`MIGRATED` set equals `ALL`; every obligation family has a green reject-probe;
the seam is ready for Phase E to delete the legacy tail and drop the `Option`.

---

## Architecture

Identity lives in the leaf crate `sky_kernels` (`StdlibKernel` + `decl() ->
StdlibDecl`; `decl()` is **already complete** — all 555 variants carry
qualifier/name/arity/class/emit, verified `no_colliding_qualifier_name_pairs`).
Canon already resolves every family to an id via `stdlib_index` built from `ALL`.
**Phase D therefore edits exactly one production file** — the HM-scheme
projection that owns `Ty`:

| Axis | Home crate | Phase D action |
|---|---|---|
| identity / arity / class / emit | `sky_kernels/src/lib.rs` | **none** — `decl()` already total |
| canon resolution (`stdlib_index`) | `sky_canon/src/env.rs` | **none** — already built from `ALL` |
| **HM scheme (`Ty`)** | **`sky_types/src/constrain.rs`** | **fill `stdlib_scheme` family by family; re-key obligations** |
| callee dispatch / native_ir_type / backend emit | `sky_lower`, `sky_ir`, `sky_backend_rust` | **none** — those projections are Phase-separate; scheme fill does not touch them |

Data-flow at the constrain site (unchanged shape, filled coverage):

```
canon Expr_::VarKernel { id: Option<StdlibKernel>, module, name }
  → constrain_var_kernel(id, module, name, span)
      ├─ obligation pre-checks (Math.min/max Ord; Dict/Set key)   ← re-keyed off id in Phase D
      └─ dual-lookup: id.and_then(stdlib_scheme)  ‖  legacy_kernel_ty(module,name)
                       ↑ Phase D grows this        ↑ Phase D shrinks reliance on this
```

---

## Tech Stack

- Rust (workspace at repo root; `edition.workspace`, `version.workspace`).
- Crate touched by every task: **`sky_types`** only. No `Cargo.toml` edits
  (the `sky_types → sky_kernels` down-edge already exists from Phase C — verify
  with `rg 'sky_kernels' crates/sky_types/Cargo.toml`).
- Test runner: `cargo test -p sky_types <filter>`. No new deps.
- `Ty` constructors: the local closures in `stdlib_scheme`'s preamble
  (`constrain.rs` ~`:1959`): `int() float() string() bool_ty() var fun list
  maybe`. Relocating Result/Dict/Set/Bytes/Task/Char families **requires
  porting the matching closures** (`result`, `dict`, `set`, `bytes`, `task`,
  `char`, tuple/record helpers) from `kernel_ty`'s preamble (`constrain.rs`
  ~`:2099`) into `stdlib_scheme`'s preamble — a per-batch prerequisite step,
  called out in each task.
- Diagnostics: fail-closed `Diagnostic::Lower { span, msg:
  LowerError::Unsupported(Feature::Kernels) }` = **SKY-L0108**. Phase D never
  introduces a `panic!`, `unreachable!()`, or a silent `Ty::Var(u32::MAX)`.
  A holed kernel getting its first scheme is an *unimplemented-feature* gap
  being closed, never a `CompilerBug`.

---

## Global Constraints

- **PRINCIPLES order (highest wins):** security > correctness > soundness >
  efficiency > completeness > readability. Phase D is a correctness/soundness
  fill: it closes the exit-0-then-cargo-fail hole class one family at a time.
  Where a holed kernel's *correct* scheme is uncertain, prefer the
  conservative, fail-closed scheme (reject more) over a permissive one that
  re-opens a silent hole.
- **PARSE, DON'T VALIDATE.** `(qualifier, name)` is parsed once, at canon, into
  a `StdlibKernel` id. Phase D consumes that id in `stdlib_scheme(k)` and never
  re-inspects the raw `&str` pair for scheme selection. The obligation re-key
  moves the Dict/Set/min-max trigger from a `self.interner.resolve(module)`
  string match to an `id`/`decl().qualifier` check — deleting the last
  scheme-path string re-inspection at the constrain site.
- **MAKE INVALID STATES UNREPRESENTABLE.** The end-state Phase D delivers —
  `stdlib_scheme` total over `ALL` — is the precondition that lets Phase E make
  "kernel id without a scheme" unrepresentable by deleting the `Option` and the
  `Ty::Var(u32::MAX)` fallback. Phase D does not itself delete the wildcard (the
  match stays `Option`-returning with a shrinking `_ => None` tail until the
  burndown hits zero); it makes the deletion safe.
- **Go-parity is golden-pinned per family.** Relocation families: the arm body
  is copied verbatim and pinned equal to `kernel_ty` by
  `stdlib_scheme_matches_legacy` (structural `Ty` equality). First-scheme
  families: the scheme is authored against the runtime `fn` signature and the
  Go oracle, and the parity proof is the green skyc→cargo build of a fixture
  that exercises the kernel.
- **Obligations are re-keyed, not dropped.** An obligation (bounded super-var)
  is not expressible in a bare `Ty`; it is attached at instantiation in
  `constrain_var_kernel`. Migrating an obligation family means (a) relocate its
  base scheme into `stdlib_scheme`, (b) switch its obligation path to consult
  `stdlib_scheme(k)` instead of `kernel_ty`, (c) re-key the trigger off `id`,
  (d) add reject-probes proving the bound still fires. Never relocate the base
  scheme without doing (b)–(d) in the same commit.
- **../sky reference note.** The Haskell compiler at `../sky` re-matches
  `(qualifier, name)` strings at each stage (`Compile.hs` kernel dispatch);
  the Rust backend replaces the type-scheme string table with an exhaustive
  `match StdlibKernel`. Stated as a capability difference, no value judgment;
  `../sky` is a read-only parity reference, never a contribution target.

---

## Preconditions & parallel-safety (READ BEFORE STARTING)

**Phase D is strictly downstream of Phase C and upstream of Phase E.** Verify all
four before starting:

1. **Phase C landed.** `constrain.rs` has `fn stdlib_scheme(&self, k:
   StdlibKernel) -> Option<Ty>` with a `_ => return None` tail (~`:2052`), the
   dual-lookup in `constrain_var_kernel` (~`:1482`), `legacy_kernel_ty`
   (~`:1514`), `kernel_scheme_or_unsupported` (~`:1494`), and the
   `registry_phase_c_tests` module (~`:4450`) with `MIGRATED`,
   `stdlib_scheme_matches_legacy`, `migrated_set_burndown`,
   `both_miss_is_fail_closed`. Confirm:
   ```
   cargo test -p sky_types registry_phase_c_tests
   ```
   Expected `test result: ok. 3 passed`. If red, **stop** — Phase C is not
   settled.

2. **HEAD quiescent on `constrain.rs`.** The Phase C agent's working tree
   oscillated (a +516-line diff that vanished; HEAD stayed at `691e275`). All
   anchors below are `file:fn` + an *as-of-read* line; **re-grep each anchor
   immediately before editing** (`rg -n 'fn stdlib_scheme' crates/sky_types/src/constrain.rs`).

3. **`decl()` is complete and canon resolves every family.** Read-only sanity:
   ```
   cargo test -p sky_kernels no_colliding_qualifier_name_pairs
   cargo test -p sky_canon canon_equals_registry
   ```
   Both green ⇒ every family already has an id path; Phase D adds only schemes.

4. **File overlap.** Phase D touches **only `crates/sky_types/src/constrain.rs`**
   (production) — specifically `stdlib_scheme`, `constrain_var_kernel`, the
   `stdlib_scheme` preamble closures, and the `registry_phase_c_tests` module.
   - **vs #49 (Port TailCallOpt):** #49 edits `sky_ir`, `sky_lower/src/lower.rs`,
     `emit_expr.rs`. **Zero overlap** with Phase D — the two can run fully in
     parallel. (Phase E's E3, which *does* edit `lower.rs`, is the one that
     serialises against #49; Phase D does not.)
   - **vs Phase E:** Phase E takes exclusive ownership of `constrain.rs` after
     Phase D completes. Do not start Phase E tasks while any Phase D family task
     is in flight — both edit `stdlib_scheme` / `constrain_var_kernel`.
   - **vs the in-flight registry migration (#45):** Phase D *is* the #45 D-slice;
     it co-owns `constrain.rs` with C (done) and E (next). Land Phase D
     end-to-end before opening Phase E.

**Ground-truth anchors verified against HEAD `691e275` (2026-07-02):**

| Symbol | File:anchor (as-of-read line) | Current shape |
|---|---|---|
| `stdlib_scheme` | `constrain.rs` `fn stdlib_scheme` (`:1955`) | `-> Option<Ty>`; preamble closures `int/float/string/bool_ty/var/fun/list/maybe` (`:1959–1990`); `Some(match k { … _ => return None })` tail (`:2052`) |
| `constrain_var_kernel` | `constrain.rs` `fn constrain_var_kernel` (`:1436`) | Math.min/max Ord early-return (`:1451–1457`); Dict/Set `key_obligation` block calling `self.kernel_ty` (`:1461–1469`); dual-lookup (`:1482–1485`) |
| `key_obligation` | `constrain.rs` `fn key_obligation` (`:546`) | `match interner.resolve(module) { Some("Set") => set_elem, Some("Dict") => dict_key, _ => None }` |
| legacy `kernel_ty` | `constrain.rs` `fn kernel_ty` (`:2061`); preamble closures (`:2099–2120`); min/max body `fun(var(0),fun(var(0),var(0)))` (`:2203`); tail `_ => Ty::Var(u32::MAX)` (`:4249`) | total; the relocation oracle |
| `legacy_kernel_ty` / `kernel_scheme_or_unsupported` | `constrain.rs` (`:1514` / `:1494`) | dual-lookup composition; untouched by Phase D |
| tests | `constrain.rs` `mod registry_phase_c_tests` (`:4450`) | `MIGRATED` (`:4462`), `stdlib_scheme_matches_legacy` (`:4534`), `migrated_set_burndown` (`:4591`), `both_miss_is_fail_closed` (`:4618`) |
| scheme-test Builder | `constrain.rs` `fn for_scheme_test` (`:4426`) | pure-scheme `Builder`; used by all registry tests |
| bounds | `ty.rs` `TyBounds::{ord,eq,set_elem,dict_key}` (`:94/99/108/119`) | obligation bound constructors |
| decl / ALL | `sky_kernels/src/lib.rs` `decl` (`:550`), `ALL` (`:1134`), `KernelClass` (`:25`) | complete; no Phase D edit |

---

## Family ledger — safest-first order (obligation-free before obligation-carrying)

Counts are decl-variant counts (`rg -c 'd\("<Qual>"' sky_kernels/src/lib.rs`).
"Holed?" = does legacy `kernel_ty` return a real scheme (No = relocation) or
`Ty::Var(u32::MAX)` (Yes = first-scheme). "Oblig." = an obligation lives in
`constrain_var_kernel` for this family.

| # | Task | Family(ies) | Kernels | Class | Holed? | Oblig. | Verify |
|---|---|---|---|---|---|---|---|
| — | done (Phase C) | String(2 of 35), List(10), Math(35 of 37) | 47 | reloc | No | — | in `MIGRATED` |
| **D1** | reloc batch 1 | Maybe(3), Result(2), Bytes(11) | 16 | reloc | No | No | parity tripwire |
| **D2** | reloc batch 2 | Io(3), Time(4), Random(3) | 10 | reloc | No | No | parity tripwire |
| **D3** | reloc batch 3 | Task(11), System(11), File(15), Http(9) | 46 | reloc | No | No | parity tripwire |
| **D4** | reloc batch 4 | Cmd(3), Sub(4), Middleware(4), RateLimit(1), Server(23) | 35 | reloc | No | No | parity tripwire |
| **D5** | reloc batch 5 | Db(23), Db.Decode(14) | 37 | reloc | No | No | parity tripwire |
| **D6** | **obligation families** | Set(10), Dict(14), Math.min/max(2) | 26 | reloc | No | **Yes** | parity tripwire **+ reject probes** |
| **D7** | **tripwire partition** | — (infra) | 0 | — | — | — | split RELOCATED vs FIRST_SCHEMED |
| **D8** | holed no-module A | Uuid(3), Encoding(6), Jwt(4) | 13 | first | Yes | No | was-a-hole + build fixture |
| **D9** | holed no-module B | JsonEnc(8), JsonDecP(3), JsonDec(16) | 27 | first | Yes | No | was-a-hole + build fixture |
| **D10** | holed scalar | Char(8), Crypto(12), PubSub(2) | 22 | first | Yes | No | was-a-hole + build fixture |
| **D11** | complete String | String remaining(33) | 33 | first | Yes | No | was-a-hole + build fixture |
| **D12** | UI leaves | Font(5), Border(3), Background(2), Html(14) | 24 | first | Yes | No | was-a-hole + build fixture |
| **D13** | UI + app-entry | Ui(46), Live(1), Tui(0), Webview(0) | 47 | first | Yes | No | was-a-hole + build fixture |
| **D-Z** | exit gate | — | 0 | — | — | — | burndown == ALL; hand off to E0 |

> **Completeness caveat (robustness).** The "Holed? No" families are marked
> relocation from a line-scan of `kernel_ty`. Any individual variant of a
> relocation family whose `kernel_ty(qual,name)` actually returns
> `Ty::Var(u32::MAX)` is a *hidden hole*: the per-family task's **Step 1
> classify** (below) catches it — such a variant is moved to the FIRST_SCHEMED
> path and authored fresh, never relocated against a `u32::MAX` oracle. So the
> ledger's Holed flags are a starting hypothesis the first step of each task
> verifies, not a trusted input.

D1–D6 are pure relocation and can be authored in any order among themselves;
the batching is for review-sized commits. D7 **must** precede D8. D8–D13 are
first-scheme families ordered highest-value-first (the no-module families have
no import escape hatch, so they are holed under every user action) and
structurally-simplest-first (scalars before record-cfg UI). Each task is an
independent commit; the build stays green throughout because every kernel is
registry-backed **or** legacy-backed, never neither.

---

## The two per-family recipes

Every family task instantiates one of two recipes. The worked exemplars (D1,
D6, D8) give the real code; the remaining tasks reference the recipe + the
ledger row.

### Recipe R (relocation — Holed? No)

1. **Classify.** For each variant `k` in the family, evaluate whether
   `kernel_ty(decl(k).qualifier, decl(k).name)` returns a real scheme or
   `Ty::Var(u32::MAX)`. The `stdlib_scheme_matches_legacy` tripwire does this
   for you once `k` is added: a `u32::MAX` variant fails the equality assert
   loudly. If any variant is a hidden hole, split it out to Recipe F.
2. **Port closures.** Ensure `stdlib_scheme`'s preamble has every `Ty`
   constructor the family's arms use (`result`/`dict`/`set`/`bytes`/`task`/…).
   Copy the closure definitions verbatim from `kernel_ty`'s preamble
   (`constrain.rs` ~`:2099`). Idempotent — skip closures already present.
3. **Move arms.** Copy each family arm from `kernel_ty` into `stdlib_scheme`'s
   `match k`, re-keyed from `(Some("Qual"), Some("name"))` to the
   `StdlibKernel` variant (`decl()` gives the variant↔(qual,name) mapping).
   Body **byte-identical**. Do NOT delete the `kernel_ty` arm (Phase E deletes
   the whole legacy fn; keeping it makes the parity tripwire meaningful).
4. **Append to `MIGRATED`.** Add the family's variants to the `MIGRATED` const
   (`:4462`) under a `// <Family> (<n>)` comment.
5. **Run the gates** (Step-by-step commands in D1). `stdlib_scheme_matches_legacy`
   proves byte-faithfulness; `migrated_set_burndown` proves the count tracks.
6. **Commit** one family-batch per commit.

### Recipe F (first-scheme — Holed? Yes)

1. **Author the scheme.** For each variant, read the runtime `fn` signature
   (`runtime/src/sky_runtime/<module>.rs`) AND the Sky stdlib declared type
   (`sky-stdlib/**/<Module>.sky`) AND the Go oracle. Encode the HM scheme in
   `stdlib_scheme`'s `match k`. Conservative-fail-closed on ambiguity.
2. **Port closures** as in Recipe R (fresh families often need new constructors
   — e.g. Json `Value`, Crypto `Bytes`).
3. **Append to `FIRST_SCHEMED`** (the D7 partition), NOT `MIGRATED`'s
   parity-checked list — there is no legacy `Ty` to match.
4. **Was-a-hole assert.** The partitioned tripwire asserts
   `kernel_ty(qual,name) == Ty::Var(u32::MAX)` for every `FIRST_SCHEMED`
   kernel — proving the scheme closes a genuine hole (not silently shadowing an
   existing legacy scheme with a divergent one).
5. **Build fixture.** Add a minimal `.sky` fixture exercising the kernel to the
   skyc example/oracle harness; the previously-failing skyc→cargo build now
   succeeds, and that green build is the Go-parity proof (Recipe F has no
   structural oracle).
6. **Commit** one family per commit.

---

## Task D1 — Relocation batch 1: Maybe, Result, Bytes (worked exemplar for Recipe R)

**Files:** `crates/sky_types/src/constrain.rs`

**Interfaces**
- Consumes: `StdlibKernel::{MaybeWithDefault, MaybeMap, MaybeAndThen,
  ResultWithDefault, ResultMap, BytesEmpty, …}`; `self.builtins.{maybe, result,
  set, dict, bytes}`; the `stdlib_scheme` preamble closures.
- Produces: `stdlib_scheme` arms for the 16 kernels; extended `MIGRATED`;
  green `stdlib_scheme_matches_legacy` + `migrated_set_burndown`.

**Steps**

1. **Classify (Step R1).** Confirm each target is a real legacy scheme:
   ```
   rg -n '\(Some\("Maybe"\)|\(Some\("Result"\)|\(Some\("Bytes"\)' crates/sky_types/src/constrain.rs
   ```
   Expected: the Maybe(3)/Result(2)/Bytes(9-lines-covering-11) arms at
   `constrain.rs:2167`, `:2180`, and the Bytes block. None are `Ty::Var`.

2. **Port closures (Step R2).** `stdlib_scheme`'s preamble (`:1959–1990`) has
   `maybe` but lacks `result`, `bytes`. Add them immediately after the `maybe`
   closure, copied verbatim from `kernel_ty`'s preamble (`:2104`, `:2115`):
   ```rust
   let result = |e: Ty, a: Ty| Ty::Con {
       module: Vec::new(),
       name: self.builtins.result,
       args: vec![e, a],
   };
   // `bytes` is a zero-argument constructor: `Bytes`.
   let bytes = Ty::Con {
       module: Vec::new(),
       name: self.builtins.bytes,
       args: Vec::new(),
   };
   ```
   > `self.builtins.bytes` must exist (it backs `kernel_ty`'s `bytes` closure).
   > Confirm: `rg -n 'bytes:' crates/sky_types/src/constrain.rs | rg 'intern'`.

3. **Move arms (Step R3).** Into `stdlib_scheme`'s `match k`, above the
   `_ => return None` tail, add (byte-identical bodies, re-keyed to variants):
   ```rust
   // ── Maybe ──
   K::MaybeWithDefault => fun(var(0), fun(maybe(var(0)), var(0))),
   K::MaybeMap => fun(fun(var(0), var(1)), fun(maybe(var(0)), maybe(var(1)))),
   K::MaybeAndThen => fun(
       fun(var(0), maybe(var(1))),
       fun(maybe(var(0)), maybe(var(1))),
   ),
   // ── Result ──
   K::ResultWithDefault => fun(var(0), fun(result(var(1), var(0)), var(0))),
   K::ResultMap => fun(
       fun(var(0), var(1)),
       fun(result(var(2), var(0)), result(var(2), var(1))),
   ),
   // ── Bytes ── (copy every Bytes arm body verbatim from kernel_ty:~2320)
   K::BytesEmpty => bytes.clone(),
   K::BytesLength => fun(bytes.clone(), int()),
   K::BytesIsEmpty => fun(bytes.clone(), bool_ty()),
   // …the remaining Bytes arms: fromString/toString/fromHex/toHex/
   //   fromBase64/toBase64/append/slice — bodies copied from kernel_ty.
   ```
   > **Do not paraphrase.** Open the exact `kernel_ty` Bytes arms and copy each
   > body character-for-character; the parity tripwire fails on any divergence.
   > `bytes` is a value (not a closure) so arms clone it — matching `kernel_ty`.

4. **Append to `MIGRATED` (Step R4).** In `registry_phase_c_tests` (`:4462`),
   after the Math block:
   ```rust
   // Maybe (3)
   K::MaybeWithDefault, K::MaybeMap, K::MaybeAndThen,
   // Result (2)
   K::ResultWithDefault, K::ResultMap,
   // Bytes (11)
   K::BytesEmpty, K::BytesLength, K::BytesIsEmpty, K::BytesFromString,
   K::BytesToString, K::BytesFromHex, K::BytesToHex, K::BytesFromBase64,
   K::BytesToBase64, K::BytesAppend, K::BytesSlice,
   ```

5. **Run the gates.** The parity tripwire recompares every migrated kernel:
   ```
   cargo test -p sky_types registry_phase_c_tests
   ```
   Expected:
   ```
   test constrain::registry_phase_c_tests::stdlib_scheme_matches_legacy ... ok
   test constrain::registry_phase_c_tests::migrated_set_burndown ... ok
   test constrain::registry_phase_c_tests::both_miss_is_fail_closed ... ok
   test result: ok. 3 passed; 0 failed
   ```
   If `stdlib_scheme_matches_legacy` fails with `stdlib_scheme(BytesSlice) is
   NOT byte-faithful to kernel_ty("Bytes","slice")`, Step R3 diverged — fix the
   arm body, do not weaken the assert.

6. **Commit:**
   ```
   git add crates/sky_types/src/constrain.rs
   git commit -m "Registry Phase D: relocate Maybe/Result/Bytes schemes into stdlib_scheme"
   ```

---

## Task D2 — Relocation batch 2: Io, Time, Random

Recipe R. Ledger row D2. **Closure prerequisite:** these families' arms use the
`task` constructor (`Task Error a`) and Random uses `int`/`float` (present).
Port the `task` closure from `kernel_ty`'s preamble into `stdlib_scheme` if not
already present (`rg -n 'let task =' constrain.rs`):
```rust
let task = |e: Ty, a: Ty| Ty::Con {
    module: Vec::new(),
    name: self.builtins.task,   // verified field: `builtins.task` (constrain.rs:2079)
    args: vec![e, a],
};
```
> Copy `kernel_ty`'s exact `task` closure body verbatim (`self.builtins.task`,
> the 2-arg `Task Error a`); the parity tripwire guards any divergence.

Move the Io(3: writeStdout/writeStderr/readLine), Time(4:
now/unixMillis/sleep/every), Random(3: int/float/choice) arms verbatim; append
to `MIGRATED`; run `registry_phase_c_tests`. Commit
`"Registry Phase D: relocate Io/Time/Random schemes"`.

> **Note — Time.every is `Tea`-class, but scheme-wise a plain relocation.**
> `decl(TimeEvery).class == Tea` affects backend routing, not the HM scheme;
> Phase D moves only the scheme. No obligation.

---

## Task D3 — Relocation batch 3: Task, System, File, Http

Recipe R. Ledger row D3 (46 kernels — the largest relocation batch; consider
splitting into two commits: {Task, System} then {File, Http} for reviewability).

- **Closures:** `task` (from D2), plus any Http record-type constructors
  (`HttpRequest`/`HttpResponse` — these are nominal `Ty::Con` over
  `self.builtins.*`; copy the exact closures/inline `Ty::Con`s from
  `kernel_ty`'s Http block at `constrain.rs:~3050`). File/System use
  `task`/`string`/`bytes`/`list`.
- Move all arms verbatim; append to `MIGRATED`; run `registry_phase_c_tests`.
- Commit(s) `"Registry Phase D: relocate Task/System schemes"` /
  `"…relocate File/Http schemes"`.

> **Watch:** `Task.sequence`/`Task.parallel` (`kernel_ty:~2850`) and
> `System.exit` (diverging `Int -> a`) have subtle bodies — copy exactly, do
> not reconstruct. The parity tripwire is the guard.

---

## Task D4 — Relocation batch 4: Cmd, Sub, Middleware, RateLimit, Server

Recipe R. Ledger row D4 (35 kernels). Cmd/Sub are `Tea`-class; Server/Middleware/
RateLimit are `Server`-class — scheme relocation is class-agnostic. Server(23)
is fully schemed in `kernel_ty` (verified: `get|post|put|delete|any|api`,
`static`, `listen`, `text|json|html|redirect`, `withStatus`, `withHeader`,
`param|queryParam|header|getCookie`, `body|path|method`, `cookie`, `withCookie`).
Closures: request/response/handler nominal constructors — copy from
`kernel_ty`'s Server block (`constrain.rs:~3900`). Append; run gates; commit(s).

---

## Task D5 — Relocation batch 5: Db, Db.Decode

Recipe R. Ledger row D5 (37 kernels). Db(23) + Db.Decode(14) are fully schemed
in `kernel_ty` (Db block `constrain.rs:~2900`, Db.Decode `~3700`). Closures:
`Db`/`Decoder`/`SqlValue`/`SqlField` nominal constructors + the `Db.Decode`
combinator shapes (`map`/`andThen`/`map2..4`/`required`/`optional`) — copy
verbatim. Append; run gates; commit.

> **Note:** Db.Decode `nullable : Decoder a -> Decoder (Maybe a)` (post-#577
> single-arg shape) — copy the exact `kernel_ty` body; do not re-derive.

---

## Task D6 — Obligation families: Set, Dict, Math.min/max (worked exemplar for the obligation re-key)

This is the M4d twin of the Math `Ord` gate and the load-bearing correctness
task of Phase D. The **base scheme** relocates like Recipe R, but the
**obligation** (bounded super-var) must be re-keyed off the id and re-pointed at
`stdlib_scheme`, with reject-probes proving the bound still fires.

**Files:** `crates/sky_types/src/constrain.rs`
(`stdlib_scheme`, `constrain_var_kernel`, `key_obligation`, tests)

**Interfaces**
- Consumes: `StdlibKernel::{SetEmpty, …, DictEmpty, …, MathMin, MathMax}`;
  `TyBounds::{set_elem, dict_key, ord}`; `self.super_var`, `self.instantiate_tracked`,
  `self.eq`; `decl().qualifier`.
- Produces: relocated Set/Dict/Math.min-max base schemes in `stdlib_scheme`;
  `constrain_var_kernel` obligation blocks re-keyed off `id` and consulting
  `stdlib_scheme(k)`; `key_obligation` accepts a `StdlibKernel`; reject-probe
  tests; extended `MIGRATED`.

**Steps**

1. **Relocate the base schemes (Recipe R Steps 2–4).** Port the `set`/`dict`
   closures into `stdlib_scheme` (`kernel_ty:2114`/`2109`). Move every Set(10) +
   Dict(14) arm verbatim, plus the Math.min/max base scheme (its `kernel_ty`
   body is `fun(var(0), fun(var(0), var(0)))` at `:2203` — the *unbounded* base;
   the bound is layered separately):
   ```rust
   // ── Set (base schemes; the set_elem obligation is layered in
   //    constrain_var_kernel, keyed off the id) ──
   K::SetEmpty => set(var(0)),
   K::SetSize => fun(set(var(0)), int()),
   K::SetInsert | K::SetRemove => fun(var(0), fun(set(var(0)), set(var(0)))),
   K::SetMember => fun(var(0), fun(set(var(0)), bool_ty())),
   K::SetToList => fun(set(var(0)), list(var(0))),
   K::SetFromList => fun(list(var(0)), set(var(0))),
   K::SetUnion | K::SetIntersect | K::SetDiff =>
       fun(set(var(0)), fun(set(var(0)), set(var(0)))),
   // ── Dict (base schemes) ── copy all 14 arms verbatim from kernel_ty:2236
   K::DictEmpty => dict(var(0), var(1)),
   // …insert/get/remove/member/keys/values/toList/fromList/map/foldl/union/
   //   isEmpty/size — bodies byte-identical to kernel_ty.
   // ── Math.min/max (base scheme; the Ord obligation is layered below) ──
   K::MathMin | K::MathMax => fun(var(0), fun(var(0), var(0))),
   ```

2. **Re-key `key_obligation` off the id.** Change its signature from a module
   symbol to a `StdlibKernel` (parse-don't-validate — no more `interner.resolve`
   string match on the scheme path):
   ```rust
   /// The element/key obligation for Set/Dict kernels, keyed off the resolved
   /// id (its decl().qualifier), not a re-inspected module string.
   fn key_obligation_for(k: StdlibKernel) -> Option<TyBounds> {
       match k.decl().qualifier {
           "Set" => Some(TyBounds::set_elem()),
           "Dict" => Some(TyBounds::dict_key()),
           _ => None,
       }
   }
   ```
   > Keep the old `key_obligation(interner, module)` only until every caller is
   > switched; delete it in this same commit once unused (Phase D goal: no
   > scheme-path string re-inspection).

3. **Re-point the obligation block at `stdlib_scheme`.** In
   `constrain_var_kernel` (`:1436`), the min/max and Dict/Set blocks now consult
   the registry via `id`. Rewrite the block, guarding on the resolved `id`:
   ```rust
   // Obligation families: layer the bounded super-var on top of the
   // *registry* base scheme (post-D6 these are all in stdlib_scheme).
   if let Some(k) = id {
       // Math.min/max: Comparable a => a -> a -> a  (M4c gate).
       if matches!(k, StdlibKernel::MathMin | StdlibKernel::MathMax) {
           let s = self.super_var(TyBounds::ord(), span)?;
           let inner = self.structure(FlatType::Fun(s, s))?;
           return self.structure(FlatType::Fun(s, inner));
       }
       // Set/Dict key: Hash + Eq + Ord on the element/key (raw var 0).
       if let Some(bound) = Self::key_obligation_for(k) {
           let ty = self
               .stdlib_scheme(k)
               .ok_or(Diagnostic::Lower { span, msg: LowerError::Unsupported(Feature::Kernels) })?;
           let (v, vars) = self.instantiate_tracked(&ty)?;
           if let Some(&key_var) = vars.get(&0) {
               let s = self.super_var(bound, span)?;
               self.eq(span, key_var, s);
           }
           return Ok(v);
       }
   }
   // …then the dual-lookup for every non-obligation kernel (unchanged).
   ```
   > **Subtlety preserved:** min/max keep the *direct-build* bounded scheme
   > (super-var `s` reused across both arrow positions so the two arguments and
   > the result unify to one bounded var) — NOT `stdlib_scheme` + a tie, because
   > their base scheme has three *independent* `var(0)`s and the M4c gate needs
   > all three tied to one bounded var. Set/Dict use `stdlib_scheme` + tie
   > because only key-position `var(0)` carries the bound. Do not unify these
   > two shapes.
   > **Deleted:** the old `matches!(self.interner.resolve(module), Some("Math"))
   > && … "min"|"max"` string guard and the `key_obligation(self.interner,
   > module)` call. The scheme path no longer re-inspects strings.

4. **Reject-probe tests (regressions — the bound must still fire post-migration).**
   The existing M4c coverage is the **skyc golden**
   `crates/skyc/tests/golden_m4c_math_gate.rs` (cases `math_min_fn_gate`,
   `math_min_rec_gate`, `…_emit` — it builds `.sky` fixtures that must be
   REJECTED at type-check). Extend that golden with Set/Dict analogues so the
   obligation coverage lives in one place, in the same shape as the existing
   min/max gate:
   ```
   // crates/skyc/tests/golden_m4c_math_gate.rs — add cases:
   //   set_insert_fn_gate   :  Set.insert (\x -> x) Set.empty   → must reject
   //   dict_insert_fn_gate  :  Dict.insert (\x->x) 0 Dict.empty → must reject
   // (each fixture asserts skyc build FAILS at type-check with an
   //  unsatisfied Ord / Hash+Eq+Ord diagnostic — the set_elem / dict_key bound)
   ```
   Additionally add a **constrain-level unit probe** in a new `mod
   obligation_probes` in `constrain.rs`, driving `constrain_var_kernel` + solve
   directly (no skyc build), asserting a function element/key is rejected:
   ```rust
   /// Set.insert on a function element must be rejected (BTreeSet key needs
   /// Ord; functions are not Ord). Reopening this = the M4d gate regressed.
   /// Build the constrain graph for `Set.insert` applied to a lambda element,
   /// solve, and expect Err — model the graph-building on an existing
   /// constrain unit test in this module (find one: `rg -n 'fn .*\(\) \{'
   /// constrain.rs` inside `mod .*tests`; reuse its Builder + solve helpers).
   #[test]
   fn set_insert_rejects_function_element() { /* … */ }

   /// Dict.insert with a function key must be rejected (HashMap key needs
   /// Hash + Eq + Ord).
   #[test]
   fn dict_insert_rejects_function_key() { /* symmetric */ }
   ```
   > If no constrain-unit solve harness exists in the module, rely on the skyc
   > golden alone (it is the authoritative M4c/M4d gate) and skip the unit
   > probe — do NOT invent an unsound bespoke harness. The skyc golden is the
   > load-bearing regression; the unit probe is a fast-feedback convenience.

5. **Append to `MIGRATED`.** Set(10) + Dict(14) + `K::MathMin, K::MathMax`
   (26). **Update the `stdlib_scheme_matches_legacy` invariant:** its current
   tail asserts `!MIGRATED.contains(MathMin) && !MathMax` (`:4579`). That
   invariant is now WRONG — min/max ARE in `stdlib_scheme` (base scheme) post-D6.
   Replace that assert with a comment documenting that min/max carry their bound
   in `constrain_var_kernel`, and that their *base* scheme is parity-checked
   like any other relocation (`kernel_ty(Math,min) == fun(var0,fun(var0,var0))
   == stdlib_scheme(MathMin)`).

6. **Run all gates + probes:**
   ```
   cargo test -p sky_types registry_phase_c_tests obligation_probes
   cargo test -p skyc  --test golden_m4c_math_gate       # skyc golden (M4c + new M4d cases)
   ```
   Expected all green. The parity tripwire now covers Set/Dict/min-max base
   schemes; the skyc golden proves the obligations still reject bad keys/
   comparands (no M4c/M4d regression).

7. **Commit:**
   ```
   git add crates/sky_types/src/constrain.rs
   git commit -m "Registry Phase D: relocate Set/Dict/Math.min-max; re-key obligations off id + reject probes"
   ```

---

## Task D7 — Partition the tripwire: RELOCATED (parity) vs FIRST_SCHEMED (was-a-hole)

Infrastructure. Before any holed family enters `stdlib_scheme`, the parity
tripwire must stop asserting `stdlib_scheme(k) == kernel_ty(k)` for kernels that
have no legacy scheme — for a holed kernel `kernel_ty` returns
`Ty::Var(u32::MAX)`, so the assert would compare the new correct scheme against
the sentinel and fail. Split the migrated set in two.

**Files:** `crates/sky_types/src/constrain.rs` (`registry_phase_c_tests`)

**Interfaces**
- Consumes: `MIGRATED` (rename → `RELOCATED`), `kernel_ty`, `stdlib_scheme`,
  `StdlibKernel::ALL`, `Ty::Var(u32::MAX)`.
- Produces: `RELOCATED` (the D1–D6 set), a new empty `FIRST_SCHEMED` const,
  `stdlib_scheme_matches_legacy` restricted to `RELOCATED`, a new
  `first_schemed_were_holes` test, `migrated_set_burndown` covering
  `RELOCATED ∪ FIRST_SCHEMED`.

**Steps**

1. **Rename `MIGRATED` → `RELOCATED`** (it holds only byte-faithful relocations
   through D6). Add a sibling:
   ```rust
   /// Families that had NO legacy scheme (kernel_ty → Ty::Var(u32::MAX)) and
   /// receive their FIRST correct scheme in Phase D (D8–D13). No parity oracle
   /// exists; correctness is pinned by `first_schemed_were_holes` (the scheme
   /// closes a genuine hole) plus the skyc→cargo build fixtures. GROWS per
   /// family task; never shrinks.
   const FIRST_SCHEMED: &[StdlibKernel] = &[ /* filled by D8–D13 */ ];
   ```

2. **Restrict the parity tripwire.** `stdlib_scheme_matches_legacy` iterates
   `ALL` and compares every `Some`; change it to compare only `RELOCATED`
   members, and assert the migrated count equals `RELOCATED.len()`:
   ```rust
   for &(k, qual, name) in &syms {
       if let Some(scheme) = builder.stdlib_scheme(k) {
           if RELOCATED.contains(&k) {
               assert_eq!(scheme, builder.kernel_ty(qual, name),
                   "stdlib_scheme({k:?}) not byte-faithful to kernel_ty");
               relocated_count += 1;
           } else {
               // FIRST_SCHEMED: must be a real hole in the legacy table.
               assert!(FIRST_SCHEMED.contains(&k),
                   "stdlib_scheme({k:?}) is Some but k is in neither RELOCATED \
                    nor FIRST_SCHEMED — classify it");
           }
       }
   }
   ```

3. **Add `first_schemed_were_holes`** (the was-a-hole proof, Recipe F Step 4):
   ```rust
   /// Every FIRST_SCHEMED kernel had NO legacy scheme (kernel_ty →
   /// Ty::Var(u32::MAX)). Proves the new scheme closes a genuine exit-0 hole
   /// rather than silently diverging from an existing legacy type.
   #[test]
   fn first_schemed_were_holes() {
       let mut interner = Interner::new();
       let builtins = make_builder(&mut interner);
       let syms: Vec<_> = FIRST_SCHEMED.iter().map(|&k| {
           let d = k.decl();
           (k, interner.intern(d.qualifier).unwrap(), interner.intern(d.name).unwrap())
       }).collect();
       let mut uf = UnionFind::<Content>::new();
       let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);
       for (k, q, n) in syms {
           assert_eq!(builder.kernel_ty(q, n), Ty::Var(u32::MAX),
               "FIRST_SCHEMED {k:?} had a legacy scheme — it is a relocation, \
                move it to RELOCATED so its parity is checked");
       }
   }
   ```

4. **Generalise `migrated_set_burndown`** to `RELOCATED ∪ FIRST_SCHEMED`:
   ```rust
   let expected = RELOCATED.contains(&k) || FIRST_SCHEMED.contains(&k);
   assert_eq!(builder.stdlib_scheme(k).is_some(), expected, "…");
   ```

5. **Run:**
   ```
   cargo test -p sky_types registry_phase_c_tests
   ```
   Expected green with `FIRST_SCHEMED` empty (all three tests trivially cover
   the D1–D6 relocations). Commit
   `"Registry Phase D: split parity tripwire — RELOCATED vs FIRST_SCHEMED"`.

---

## Task D8 — First-scheme no-module A: Uuid, Encoding, Jwt (worked exemplar for Recipe F)

**Files:** `crates/sky_types/src/constrain.rs`; a fixture under the skyc
example/oracle harness (path per Task #35 sweep layout — confirm before adding).

**Interfaces**
- Consumes: runtime sigs `runtime/src/sky_runtime/{uuid_kernel,encoding,jwt}.rs`;
  Sky stdlib types `sky-stdlib/Sky/Core/{Uuid,Encoding,Jwt}.sky`;
  `StdlibKernel::{UuidV4, UuidV7, UuidParse, EncodingBase64Encode, …,
  JwtEncodeHs256, …}`.
- Produces: 13 first-scheme arms; extended `FIRST_SCHEMED`; green
  `first_schemed_were_holes`; a passing build fixture.

**Steps**

1. **Read the runtime signatures (Recipe F Step 1).** Ground truth
   (`runtime/src/sky_runtime/uuid_kernel.rs`):
   ```
   pub fn uuid_v4() -> String
   pub fn uuid_v7() -> String
   pub fn uuid_parse(s: String) -> SkyMaybe<String>
   ```
   `encoding.rs`: `base64_encode(String)->String`,
   `base64_decode(String)->SkyResult<E,String>`, `url_encode(String)->String`,
   `url_decode`, `hex_encode(String)->String`, `hex_decode(String)->SkyResult`.
   `jwt`: `encodeHs256(claims,secret,…)->String`, `decodeHs256(token,secret)->Result`.

2. **Author the schemes.** `Uuid.v4`/`v7` are the **arity-0** case — their decl
   arity is 0 and their scheme head is a *bare* `String`, NOT `() -> …` (this is
   Limitation #7's zero-arity classifier; the scheme head shape *is* the
   classification, pinned later by Phase E's `arity_matches_scheme`):
   ```rust
   // ── Uuid ──
   K::UuidV4 | K::UuidV7 => string(),            // bare String (arity 0)
   K::UuidParse => fun(string(), maybe(string())),
   // ── Encoding ──  (encode : String -> String; decode : String -> Result Error String)
   K::EncodingBase64Encode | K::EncodingUrlEncode | K::EncodingHexEncode =>
       fun(string(), string()),
   K::EncodingBase64Decode | K::EncodingUrlDecode | K::EncodingHexDecode =>
       fun(string(), result(error_ty(), string())),   // port `error_ty`/Error con from kernel_ty
   // ── Jwt ──  (author against runtime: encode arity 3, decode arity 2)
   K::JwtEncodeHs256 | K::JwtEncodeRs256 => /* claims -> secret -> ... -> String */,
   K::JwtDecodeHs256 | K::JwtDecodeRs256 => /* token -> secret -> Result Error Claims */,
   ```
   > **Conservative-fail-closed:** if the Jwt claims/return shape is uncertain
   > from the runtime sig alone, cross-check the `.sky` declared type; encode
   > exactly that. Do not widen to `var(_)` to make it compile — a too-loose
   > scheme re-opens a silent hole (violates correctness > completeness).
   > Port the `Error` constructor closure (`error_ty`) from `kernel_ty` if the
   > Result-returning arms need it.

3. **Append to `FIRST_SCHEMED`** (D7's const), under `// Uuid (3) / Encoding (6)
   / Jwt (4)` comments.

4. **Run the was-a-hole gate:**
   ```
   cargo test -p sky_types registry_phase_c_tests
   ```
   `first_schemed_were_holes` proves `kernel_ty(Uuid,v4) == Ty::Var(u32::MAX)`
   (it was a genuine hole). `migrated_set_burndown` confirms coverage grew.

5. **Build fixture (the Go-parity proof for Recipe F).** Add a minimal `.sky`
   exercising each kernel, e.g.:
   ```elm
   module Main exposing (main)
   import Sky.Core.Uuid as Uuid
   import Sky.Core.Encoding as Encoding
   import Std.Log exposing (println)
   main =
       let _ = println (Encoding.base64Encode "hi")
       in  println (Uuid.parse "not-a-uuid" |> Maybe.withDefault "invalid")
   ```
   Before D8 this failed skyc→cargo (`Uuid.parse` typed `Ty::Var(u32::MAX)` →
   arg-count never checked → cargo error). After D8 it builds and runs. Wire it
   into the sweep (Task #35) and run:
   ```
   cargo run -p skyc -- build <fixture>/src/Main.sky   # expected: builds + runs clean
   ```

6. **Commit:** `"Registry Phase D: first schemes for Uuid/Encoding/Jwt (close no-module holes)"`.

---

## Task D9 — First-scheme no-module B: JsonEnc, JsonDecP, JsonDec

Recipe F. Ledger row D9 (27 kernels). Highest-value holed family group after
D8. **New closures:** the Json `Value` nominal con and `Decoder a`
(`json_dec`) con — construct from `self.builtins.*` (add the `builtins` fields
if absent; check `rg -n 'json_value\|decoder' crates/sky_types/src/constrain.rs`).
Author each scheme against `runtime/src/sky_runtime/json*.rs` +
`sky-stdlib/Sky/Core/Json/**`:
- `JsonEnc.{string,int,float,bool}` : `<prim> -> Value`; `null : Value` (arity
  0, bare); `list : (a -> Value) -> List a -> Value`; `object`, `encode`.
- `JsonDec` combinators (`field`/`at`/`index`/`list`/`map`/`andThen`/`succeed`/
  `fail`/`oneOf`/`map2..4`/`decodeString`) — mirror the Elm shapes.
- `JsonDecP.{required,optional,custom}` pipeline combinators.
Append to `FIRST_SCHEMED`; run gates; build a Json round-trip fixture; commit.

---

## Task D10 — First-scheme scalar: Char, Crypto, PubSub

Recipe F. Ledger row D10 (22 kernels). Char(8): `isAlpha`/`isDigit`/`isLower`/
`isUpper` : `Char -> Bool`; `toLower`/`toUpper` : `Char -> Char`; `toCode` :
`Char -> Int`; `fromCode` : `Int -> Char` (port the `char` closure). Crypto(12):
hashing `String -> String` (sha256/…), `hmac* : String -> String -> String`,
symmetric-encryption Result-returning, `randomBytes`/`randomToken` : `Int ->
Task Error Bytes/String`. PubSub(2): `publish`/`publishNoEcho` :
`Task`-shaped. Author against runtime; append to `FIRST_SCHEMED`; gates;
fixture; commit.

---

## Task D11 — Complete String (33 remaining first-scheme kernels)

Recipe F. Ledger row D11. Only `StringFromInt`/`StringFromFloat` were schemed in
Phase C; the other 33 String kernels are holes. Author schemes from
`runtime/src/sky_runtime/string_kernel.rs` (or equivalent) + `Sky.Core.String`
declared types:
- `length`/`toInt`(→`Maybe Int`)/`toFloat`(→`Maybe Float`) etc.
- unary `String -> String`: reverse/toUpper/toLower/casefold/trim/trimStart/
  trimEnd; `isEmpty`/`isEmail`/`isUrl` : `String -> Bool`; `fromChar` :
  `Char -> String`; `fromList` : `List Char -> String`; `toList` :
  `String -> List Char`; `words`/`lines` : `String -> List String`; `concat` :
  `List String -> String`.
- binary: `append`/`join`/`split`/`repeat`/`dropLeft`/`dropRight`/`contains`/
  `startsWith`/`endsWith`/`equalFold`.
- ternary: `replace`/`slice`/`padLeft`/`padRight`.
Append to `FIRST_SCHEMED`; gates; a String-ops fixture; commit.

> **Cross-check the two already-migrated String kernels are in `RELOCATED`, not
> `FIRST_SCHEMED`** — `fromInt`/`fromFloat` HAD legacy schemes, so
> `first_schemed_were_holes` would fail if they were mis-filed.

---

## Task D12 — First-scheme UI leaves: Font, Border, Background, Html

Recipe F. Ledger row D12 (24 kernels). These are `Ui`-class attribute/element
builders. Structurally unlike scalars: they return `Attribute msg` / `Element
msg` — polymorphic over the message type `msg`. Author schemes with a free
`var(_)` for `msg` per the `Std.Ui`/`Std.Html` declared types
(`sky-stdlib/Std/Ui/**`, `Std/Html.sky`). Font/Border/Background helpers
(`Font.color`/`Border.rounded`/`Background.color`) : `<arg> -> Attribute msg`.
Html nodes (`Html.text`/`Html.node`/`Html.a`) : element shapes. Append to
`FIRST_SCHEMED`; gates; a Std.Ui render fixture; commit.

> **Interaction note:** the M-propagation work (Task #43) and the plain-HTML-
> attribute wiring (#46) touch the same Ui/Html kernels at the *lowering /
> emit* layer. Phase D touches only their **scheme** here. Coordinate: land the
> scheme (D12) independent of #46's emit wiring, but verify the fixture builds
> only once both the scheme and the emit path exist — if #46 is not yet landed,
> the fixture may type-check (scheme present) yet fail to emit (no lowering).
> In that case pin correctness with a **type-check-only** assertion (constrain
> the fixture, assert `Ok`) and defer the full build fixture until #46 lands.

---

## Task D13 — First-scheme UI + app-entry: Ui, Live, Tui, Webview (migrate last)

Recipe F. Ledger row D13 (47 kernels — the bulk of Std.Ui plus the app-entry
kernels). Per spec migration-order point 5, app-entry families migrate last:
they are already hand-schemed in lockstep and are the closed-record-cfg proving
ground. `Ui.{layout,layoutWith,el}` (3) are ALREADY schemed in `kernel_ty` →
they belong in `RELOCATED` (verify via `first_schemed_were_holes`); the other
46 Ui builders are holes. `Live.{app,route,renderStatic}` schemed (3, RELOCATED);
`Live`'s 4th decl variant + any holed one → FIRST_SCHEMED. `Tui.{app,program}`
+ `Webview.app` already schemed (RELOCATED).

- For the holed Ui builders (`row`/`column`/`text`/`button`/`input`/`spacing`/
  `padding`/`Background`/`Border`/`Font` attrs on `Ui`, etc.), author
  `Attribute msg` / `Element msg` schemes as in D12.
- For app-entry, the scheme is the closed-record-cfg `Cfg model msg -> …`
  shape — copy the RELOCATED `Live.app` body pattern; the config record is a
  nominal `Ty::Con`.
Append; gates; the largest UI fixture (`examples/26-ui-showcase` analogue);
commit. Heed the same #46/#43 interaction note as D12.

---

## Task D-Z — Exit gate: burndown covers ALL; hand off to Phase E

Verification + handoff. Phase D is done when `stdlib_scheme` returns `Some` for
every `StdlibKernel::ALL` variant — i.e. `RELOCATED ∪ FIRST_SCHEMED == ALL`.

**Files:** `crates/sky_types/src/constrain.rs` (test only)

**Steps**

1. **Add the coverage gate** (mirrors Phase E's E0 but stays in the Phase C/D
   test module, so E0 is a rename-and-relocate, not new logic):
   ```rust
   /// Phase D exit gate: RELOCATED ∪ FIRST_SCHEMED == StdlibKernel::ALL, and
   /// stdlib_scheme returns Some for every variant. Green ⇒ Phase E may start.
   #[test]
   fn stdlib_scheme_covers_all() {
       let mut interner = Interner::new();
       let builtins = make_builder(&mut interner);
       let mut uf = UnionFind::<Content>::new();
       let builder = Builder::for_scheme_test(&mut uf, &interner, builtins);
       let missing: Vec<_> = StdlibKernel::ALL.iter().copied()
           .filter(|&k| builder.stdlib_scheme(k).is_none())
           .collect();
       assert!(missing.is_empty(),
           "{} kernels still unschemed (Phase D incomplete): {missing:?}",
           missing.len());
   }
   ```

2. **Run the full crate + the obligation golden:**
   ```
   cargo test -p sky_types
   cargo test -p skyc --test golden_m4c_math_gate
   ```
   Expected: `stdlib_scheme_covers_all ... ok`, all `registry_phase_c_tests`
   green, all `obligation_probes` green, and the skyc M4c/M4d gate green.

3. **Confirm the `_ => return None` tail is now dead** (every variant has an
   explicit arm). Do NOT delete it here — that deletion, plus dropping the
   `Option` and the `Ty::Var(u32::MAX)` legacy fallback, is Phase E's E2. Phase
   D leaves the wildcard in place (still `Option`-returning) so the diff to E
   is a clean, isolated seal.

4. **Workspace regression** (Phase D changed only scheme *coverage*, not any
   existing scheme value; examples that already built still build):
   ```
   cargo test --workspace
   ```
   Expected green across crates.

5. **Hand off.** Update the Phase E plan's precondition #1 ("Phase D complete —
   burndown = 0") as satisfied; Phase E's Task E0 (`stdlib_scheme_is_total`) is
   now the same assertion as `stdlib_scheme_covers_all` and will pass. Commit
   `"Registry Phase D exit gate: stdlib_scheme covers StdlibKernel::ALL"`.

---

## Definition of done

- `stdlib_scheme` returns `Some` for every `StdlibKernel::ALL` variant
  (`stdlib_scheme_covers_all` green); the `_ => return None` tail is dead but
  retained for Phase E to delete.
- Every relocation family's scheme is byte-faithful to its legacy `kernel_ty`
  arm (`stdlib_scheme_matches_legacy` over `RELOCATED`).
- Every first-scheme family closed a genuine hole (`first_schemed_were_holes`:
  each `FIRST_SCHEMED` kernel had `kernel_ty == Ty::Var(u32::MAX)`) and builds
  a green skyc→cargo fixture.
- Obligations survived migration: Set/Dict key (`Hash+Eq+Ord`) and Math.min/max
  (`Ord`) re-keyed off the resolved id, consulting `stdlib_scheme`, pinned by
  `obligation_probes` + `golden_m4c_math_gate`. No scheme-path `interner.resolve`
  string re-inspection remains in `constrain_var_kernel`.
- `cargo test --workspace` green; no `panic!`/`unreachable!()` added; no new
  `Ty::Var(u32::MAX)` site.
- Phase E's E0 precondition is satisfied; `constrain.rs` is handed to Phase E.
