# Principles Audit — 2026-07-09 (Fable hardening swarm)

Whole-codebase audit of the **Ipê Rust project only** (`crates/`, `runtime/`,
`tools/` — `../sky` excluded) against `PRINCIPLES.md`: the 6 ranked principles
(1 Security · 2 Correctness · 3 Soundness · 4 Efficiency · 5 Completeness ·
6 Readability), the two fundamental rules (PARSE-DON'T-VALIDATE ·
MAKE-INVALID-STATES-UNREPRESENTABLE), and **the seal** (ipe exit 0 ⟹ cargo
build exit 0; concrete-over-generic; no `dyn Any`).

Method: 12 Fable auditors, one per subsystem partition, read-only, each reporting
reachable holes with `file:line` evidence; a Fable adversarial-verify pass on
every high/critical finding.

> **Run status — COMPLETE.** All **12 of 12 partitions audited** and the full
> **adversarial-verify pass ran**. Run 1 (2026-07-08→09) tripped the weekly limit
> after 7 partitions; the resume (run 2) completed the remaining 5 (`types-seal`,
> `canon-rules`, `parse-input`, `kernels-diagnostics`, `ipe-tools`) plus every
> owed verify. **14 high/critical findings confirmed real** by adversarial verify;
> **1 refuted** — types-seal `unify.rs:373-389` open-record (the verifier found the
> closed-record guard already covers the case: a caught false positive, recorded
> below for the trail). Every `⧗verify-owed` tag in the sections below is now
> upgraded — all high/critical passed verification (isReal=true). Raw:
> `wf_3f451707-748` (result JSON in the session task output).

Severity legend: 🔴 critical · 🟠 high · 🟡 medium · ⚪ low. `✓verdict` = passed
adversarial verify (isReal); the previously-`⧗verify-owed` high/critical items
all verified real except the one noted refuted above.

---

## Confirmed (adversarial-verified)

### 🟠 correctness — Transaction routing ignores the pool argument ✓verdict
`runtime/src/sky_runtime/db.rs:103-114` (+ nesting gate `:1574`)
`exec_routed`/`fetch_*_routed` route to the task-local `TXN_CONN` whenever set,
**discarding the `pool` parameter**. A program with two DBs running `Db.query dbB`
inside `Db.withTransaction dbA (…)` executes dbB's statement on dbA's connection
— silent wrong-DB read/write; cross-tenant data-mixing in per-tenant-DB apps.
Go reference passes the handle through (diverges). **Fix:** tag each pool with an
identity; the routed helpers + nesting gate use `TXN_CONN` only on id match, else
fall through to `pool`. Two-DB regression test.

---

## lower-ir-seal (audit complete · ⧗verify-owed)

### 🔴 seal — Multiple `any` occurrences collapse to ONE shared Rust generic
`crates/sky_lower/src/lower.rs:4331-4343` (+ `:3540-3553`)
The checker gives every `any` a fresh flex UV per occurrence (`constrain.rs:1576`),
so `f : any -> any -> Int; f _ _ = 0` called `f "x" 3` is well-typed → ipe exit 0.
But the lowerer maps each param `any` to `IrType::Generic(v)` keyed by the single
interned `"any"` Symbol → backend emits `fn main_f<T1>(a: T1, b: T1)` → call
`main_f("x".into(), 3)` fails cargo **E0308**. Exactly the named seal class; the
codebase already pins union-ctor `any` payloads to `Dict String String` but
annotation-position `any` diverges to a generic (and `ir.rs:493` doc still claims
`any` is rejected at lowering). **Fix:** alpha-rename each `any` to a fresh
synthetic Symbol before collecting `type_params` (per-occurrence generic), **or**
resolve each param `any` from its solved region type as the return-position fix
already does (`lower.rs:3483`); fail-closed when unresolved. Relates memory
`prefer-concrete-over-generic-codegen`. Regression: `f : any -> any -> Int` called
with two concrete types must build.

### 🟠 seal — Generic bounds looked up by BARE def name (cross-module collision)
`crates/sky_lower/src/lower.rs:3548` (+ `sky_types/src/lib.rs:82,306-323`)
`SolvedTypes::env`/`regions` are keyed by `(home, name)` because bare-name lookup
is unsound cross-module — but `bounds` is keyed by bare `Symbol` and populated
with a plain overwrite while iterating `(home, def_name)`. Two modules each with a
same-named generic fn under different obligations (`a: Add` vs `a: Ord`) → the
later-iterated wins → `lib_scale<T1>(x,y){x+y}` emitted **without `T1: Add`** →
cargo E0369. Same bare key feeds `check_scheme_applications` → use-site soundness
gate checks the WRONG def (both false-accept and false-reject). **Fix:** re-key
`bounds` as `(Vec<Symbol>, Symbol)`; thread home into `SchemeApp`; lookup by
`(def.home(), name)`.

### 🟡 seal — Bug-29 `any`-return UI injection matches ANY single-arg `Ty::Con`
`crates/sky_lower/src/lower.rs:3405-3419`
The `view : Model -> any` → `Html<T1>` heuristic checks only `Ty::Con{args}` +
`args.first() == Ty::Var`, never the Con **name** — so it also fires for
`Maybe`/`List`/`Set`/`Decoder`. `w : Int -> any; w n = []` → `fn main_w<T1>() -> Vec<T1>`
(generic only in return); a call that never pins the element → cargo **E0282**.
**Fix:** gate injection on the Con being a UI msg-parametric ctor (Html/Element/
Attribute); otherwise pin to the concrete `any` carrier or fail-closed.

### ⚪ invalid-states — `Match::from_parts_unchecked` is `pub`
`crates/sky_ir/src/ir.rs:1626-1628`
Doc claims Match's only constructor validates arm exhaustiveness, but this `pub`
escape hatch builds a Match from arbitrary arms (empty vec → `match x {}` → E0004,
no Ipê diagnostic). **Fix:** replace with a shape-preserving `map_bodies` combinator
that cannot change patterns; or `pub(crate)`-seal + debug-assert arm shapes.

---

## backend-emit (audit complete · ⧗verify-owed)

### 🟠 correctness — TaskSeq clone-capture does textual surgery on emitted Rust
`crates/sky_backend_rust/src/emit_expr.rs:4849` (mechanism `:238-283`)
`clone_captured_vars` token-rewrites already-emitted Rust text; `add_clone_to_bare_ident`
only skips an ident when the immediately-preceding byte is `"`/`'` — no string-literal
state. `let count=3; _ = println "the count is" in …count…` →
`log_println("the count.clone() is"…)` (silent output corruption); a record field
matching a captured var → `RecCount { count.clone(): n }` → **invalid Rust, seal
breach**; match binders hit the same. **Fix:** stop textual surgery — compute
`rest`'s free-var set on the **IR** and emit `Expr::CloneVar` (already exists) for
shared non-Copy bindings before emission.

### 🟠 seal — Let-inlining `replace_word_all` rewrites inside string literals
`crates/sky_backend_rust/src/emit_expr.rs:4659-4664` (mechanism `:39-97`)
Multi-use task-list `let` inlines via `replace_word_all` over emitted body text, no
literal-state tracking → `let tasks=[…] in Log.info "run tasks" … tasks` splices the
value into the `"run tasks"` literal → embedded quotes terminate it → cargo fail
(seal); quote-free values still silently diverge from Go. **Fix:** inline at the IR
level (Symbol-keyed substitution), or emit a zero-arg closure per use. Delete the
textual pass.

### 🟡 seal — No emitted-name collision guard for functions (non-injective snake_case)
`crates/sky_backend_rust/src/lib.rs:341-344`
`_` is a legal Ipê ident char, so `fooBar` and `foo_bar` are distinct values →
both snake-case to `main_foo_bar` → two `fn main_foo_bar` → **E0428**. The enum
path guards this (`:300-314`) and records dedup with suffixes; the func path
inserts blind. **Fix:** mirror the enum guard — dedup with numeric suffix or
fail-closed naming both Ipê sources.

### 🟡 seal — TaskSeqSync has no clone-capture handling (use-after-move)
`crates/sky_backend_rust/src/emit_expr.rs:4860-4865`
`let _ = Io.writeStdout msg in msg` → `{ let _ = task_run(io_write_stdout(msg)); msg }`
→ `msg` moved then used → **E0382**. Async `TaskSeq` handles it; sync sibling does
nothing. **Fix:** same IR-level free-var + `CloneVar` fix, covering both paths.

### 🟡 seal — Multi-use `let` of a directly Task-typed value emitted as plain `let`
`crates/sky_backend_rust/src/emit_expr.rs:4645-4667`
`expr_value_is_non_clone` returns true only for `Expr::List` containing a Task; a
binding whose value *is* a `SkyTask` (non-Clone) or a record/tuple carrying one
takes the shared-`let` path → second use is E0382. **Fix:** decide by the binding's
**IR type** (`ir_type_contains_task`, extended to Tuple/Record/Fun-return), inline
or thunk per use.

### ⚪ seal — `emit_ident` mangles only Rust keywords (runtime-symbol shadowing)
`crates/sky_backend_rust/src/lib.rs:818-820`
Locals emitted verbatim + `pub use sky_runtime::*` → a Ipê local named `task_run`/
`dict_get` shadows the glob import; a subsequent kernel emission resolves to the
local → E0618/type error. **Fix:** emit kernel calls fully-qualified
(`sky_runtime::task::task_run`), or extend the mangle set with the runtime's
exported-symbol list (generated so it can't drift).

### ⚪ efficiency — Unconditional `.clone()` on every record-field access
`crates/sky_backend_rust/src/emit_expr.rs:4794-4811`
Comment claims rustc elides the clone — it does not for String/Vec/struct. Every
heap-field read is an O(n) deep copy → O(n²) on list-of-records renders. **Fix:**
type-directed elision (no `.clone()` for Copy fields; clone heap fields only on
non-last-use), or Arc-backed persistent containers.

### ⚪ invalid-states — `Ui.onSubmit` handler erased into `Arc<dyn Any>` (undocumented)
`crates/sky_backend_rust/src/emit_expr.rs:4116-4131`
Emit-time erasure into `Arc<dyn Any>` (runtime downcasts form data) — mirrors Go's
reflect dispatch but is an unrecorded exception to the dyn-Any ban. **Fix:** record
in `divergences-from-sky.md` + ensure the runtime downcast returns typed Err (never
panics); long-term monomorphize the submit channel per Msg type.

---

## rt-auth-crypto (audit complete · ⧗verify-owed)

### 🟠 security — `auth_verify_token` inherits jsonwebtoken's 60s default leeway
`runtime/src/sky_runtime/auth.rs:184`
`Validation::new(HS256)` leaves `leeway = 60`, so `Auth.verifyToken` accepts tokens
**up to 60s past `exp`** (and 60s before `nbf`) — attacker-replayable, diverges from
Go's zero-skew. The sibling `jwt.rs:176` explicitly sets `leeway = 0` +
`reject_tokens_expiring_in_less_than = 1` with boundary tests; `auth.rs` skipped
both. **Fix:** set `leeway = 0` + `reject_tokens_expiring_in_less_than = 1` (guard
the `exp==0` underflow as jwt.rs does); add the boundary regression.

### 🟠 seal — `Auth.signToken`/`verifyToken` claims typed flex `var(0)` vs monomorphic `HashMap`
`crates/sky_types/src/constrain.rs:4859,4862`
Scheme types claims as `var(0)` (unifies with anything → ipe exit 0) but the wrapper
is pinned to `claims: HashMap<String,String>` (`project.rs:256`, `auth.rs:120`), no
coercion at lowering → any non-`Dict String String` claims → cargo fail. **Fix:** pin
both schemes to `dict(string(), string())` (concrete-over-generic); record the Go
divergence; seal test compiling `Auth.signToken s { sub="x" } 3600`.

### 🟡 correctness — `auth_verify_token` rejects any token carrying an `aud` claim
`runtime/src/sky_runtime/auth.rs:184`
`Validation::new` defaults `validate_aud=true, aud=None` → a token merely *carrying*
`aud` fails `InvalidAudience`. `signToken` accepts arbitrary claims, so sign-then-verify
of aud-bearing claims breaks. `jwt.rs:195` disables `validate_aud`; auth.rs never
does. **Fix:** `validation.validate_aud = false`; set `required_spec_claims` explicitly.

### ⚪ soundness — Unknown-algorithm error byte-slices `&s[..min(20)]` + echoes secret bytes
`runtime/src/sky_runtime/jwt.rs:438` (+ `:495`)
`&algorithm_descriptor[..min(20)]` byte-index slice panics on a non-char-boundary and
echoes up to 20 bytes of a value built as `format!("HS256:{secret}")` → secret bytes
into the String error → logs. Dead from well-typed code today (opaque `Algorithm`
nominal) but one variant-drift away. **Fix:** drop the payload echo entirely.

---

## rt-http-live-net (audit complete · ⧗verify-owed)

Posture strong (SSRF rebind-pin, CSRF double-submit + `__Host-`, `OsRng` sids, SSE
framing, body caps). Residual:

- 🟡 security — **Session cookie `Secure` is ENV-gated, not request/TLS-gated**
  (`live/mod.rs:809`): a non-prod process behind a TLS-terminating proxy emits a
  non-`Secure` `sky_sid`. **Fix:** honour `X-Forwarded-Proto=https` under a trusted-proxy
  allowlist, or a `IPE_LIVE_FORCE_SECURE_COOKIES` override; thread into `__Host-` choice.
- ⚪ security — **`/_sky/observability/ingest` CSRF-exempt AND open in dev**
  (`live/csrf.rs:141`): cross-site POST folds forged telemetry into the console (dev
  only; prod fails closed). **Fix:** remove from `is_exempt_path`, or bind dev-open to
  loopback + missing-Origin.
- ⚪ security — **Ipe.Http.Server WS upgrade does no Origin check outside production**
  (`server.rs:974`): empty `originPatterns` + non-prod ENV → any Origin upgraded (CSWSH).
  **Fix:** default-deny cross-origin when patterns empty, independent of the ENV gate.
- ⚪ correctness — **`live_max_body_bytes()` lacks the `>0` floor** `server::max_body` has
  (`live/mod.rs:864`): `IPE_LIVE_MAX_BODY_BYTES=0` → every `/_sky/event` 413s. **Fix:**
  `.filter(|&n| n > 0)`.

---

## rt-io-injection (audit complete · ⧗verify-owed)

Injection surfaces (exec-not-shell `Process.run`, temp-path sanitisation, CSS/HTML
smart constructors, decompress/read/CSV caps) well-hardened. Residual:

- 🟡 correctness — **`Time.fromParts` lossy `y as i32`/`m,d,h as u32` casts** silently
  accept wrapped out-of-range parts as a wrong-but-valid date (`time.rs:424`) — the
  same file fixed this class in `time_add_months`. **Fix:** `i32::try_from`/`u32::try_from`
  → Err on out-of-range.
- 🟡 correctness — **`gunzip` decodes only the first gzip member**; Go's reader is
  multistream (`compression.rs:42`) → silent truncation on concatenated `.gz`. **Fix:**
  `MultiGzDecoder`.
- ⚪ security — **`Io.readLine` reads an unbounded line** (`io.rs:12`): untrusted stdin
  with no newline → OOM; every sibling read path has a cap. **Fix:** `take(cap)` + Err.
- ⚪ correctness — **`File.readFileLimit` metadata-then-`take(cap)` TOCTOU** → silent
  truncation instead of the documented Err (`file.rs:100`). **Fix:** use `readFile`'s
  `take(cap+1)` + recount pattern; drop the metadata precheck.
- ⚪ correctness — **`time_is_leap_year` lossy `y as i32` wrap** (`time.rs:263`). **Fix:**
  compute on i64 directly (no cast).
- ⚪ correctness — **`escape_text` omits `"` while its doc claims Go full-set parity**
  (`html.rs:439`): text-node byte-divergence from Go's `html.EscapeString`. **Fix:** add
  `.replace('"', "&#34;")` or correct the comment + Go byte-diff fixture.

---

## rt-db-sql (audit complete · verify partial)

SQL-injection posture strong (`SqlIdent`/`valid_sql_ident` parse-don't-validate,
positional binding, RETURNING allowlist, unscoped-UPDATE refused, redacted errors).
Beyond the verified txn-routing hole above:

- 🟠 correctness — **`Db.connect` hardcoded `sqlite::memory:` per generated project**
  (`config.rs:10` + `project.rs:284` verbatim include + `db.rs:646`): sky.toml
  `[database] url` never wired; memory URLs excluded from the pool cache → each
  `Db.connect ()` a fresh empty DB → silent data loss. `⧗verify-owed`. **Fix:** emit
  `config.rs` `IPE_DB_URL` from sky.toml (never `:memory:`), or read `DATABASE_URL`
  at call time; shared-connection behaviour test.
- 🟡 security — **`url_is_cacheable` substring test `!url.contains("memory")`**
  (`db.rs:560`): legit URLs containing "memory" bypass the pool cache → connection-exhaustion
  DoS the cache exists to prevent. **Fix:** match the actual in-memory forms only.
- 🟡 parse-don't-validate — **`SqlNull` witness erased to text-typed NULL** (`db.rs:1650`):
  breaks typed NULL binding on Postgres. **Fix:** typed null variants projected from the
  `SqlNull` witness.
- 🟡 completeness — **Postgres documented but structurally unreachable** (`config.rs:6`):
  `DbPool` hard-aliased to `SqlitePool`; `db_format_sql` never rewrites `?`→`$n`. **Fix:**
  per-driver `config.rs` emission, or record the sqlite-only limitation explicitly.
- ⚪ correctness — **`db_insert_row` RETURNING-id degrades type-mismatch to `id=0`**
  (`db.rs:1166`): non-integer PK → `unwrap_or(0)` returns 0 as a real key. **Fix:** Err
  on double `try_get` failure.
- ⚪ completeness — **Tenant-prefix SQL enforcement absent from `db.rs`** (cross-partition
  note; verify `hub.rs` implements the WHERE-clause gate).

---

## rt-core-soundness (audit complete · ⧗verify-owed)

Posture very strong — **no reachable panic/unwrap/OOB/downcast from well-typed Ipê**;
fallible paths Maybe/Result-typed, casts clamp-then-narrow, sorts `catch_unwind`-guarded,
integer overflow wraps (Go parity via emitted `overflow-checks=false`). Residual:

- ⚪ correctness — **`List.range` silently truncates to a 10M prefix** (`list.rs:128`):
  large-but-finite range → wrong short list vs Go's full list; not in the divergence
  ledger. **Fix:** record as sanctioned DoS-guard divergence, or return `Result` on over-cap.
- ⚪ correctness — **`Math.abs` (saturating) vs `Basics.abs` (wrapping) disagree at
  `i64::MIN`** and `Math.abs` diverges from Go (`math.rs:43`). **Fix:** one i64::MIN
  semantics for both, matched to the Go oracle; fixture at i64::MIN.

---

## types-seal (audit complete · verified)

### 🟠 soundness — Numeric defaulting pins Super vars to Int without checking the Append obligation ✓verdict
`crates/sky_types/src/lib.rs:261-273`
The post-solve defaulting arm matches `Super { rigid:false, bounds } if bounds.has_number()`
and pins the root to `Int` **without checking the class's accumulated union bounds**.
Obligations union across a class (`unify.rs:166`); once one entry defaults the class to
Int, later entries validate against `Structure(Int)` but the defaulting entry's own
`Append` obligation is never checked. Witness (well-formed, ill-typed): `f x = (x ++ x) + 1`
— the `Append` super is created first, then `Number`; ipe accepts → cargo fail. Order-dependent
exit-0-then-cargo-fail. **Fix:** in the defaulting arm gate the pin on the class union bounds —
if `bounds.has_append()` return `super_unsatisfied` (IPE-T0014) instead of defaulting.

### 🟠 parse-don't-validate — `Ty::Var(u32)` conflates interner raws with kernel-scheme ordinals ✓verdict
`crates/sky_types/src/constrain.rs:1576-1584`
Two id spaces flow through one `Ty::Var(u32)`: annotation vars carry `Symbol::as_raw()`
(`ty.rs:327`) while kernel schemes use bare ordinals (`var(0)`, `var(1)`, `RowTail::Open(3)`).
`instantiate_in` decides wildcard-ness by resolving `Symbol::from_raw(id)` and string-comparing
to `"any"` — applied to BOTH spaces. A user program that interns `"any"` at a low raw id
matching a scheme ordinal misfires the wildcard gate, silently severing kernel type-var sharing.
**Fix:** make the invalid state unrepresentable — split `Ty::Var` into `Sym(Symbol)` /
`Ordinal(u32)`, or resolve wildcard-ness once at the boundary into a dedicated `Ty::AnyWildcard`
variant that scheme ordinals can never be.

### 🔵 REFUTED — open-record unification step 4 (false positive)
`crates/sky_types/src/unify.rs:373-389` — auditor flagged a closed→open record leak; the
verifier read the step-2 guard (`:362-366`) and found it already rejects the reachable case
(`isReal=false, not-a-defect`). Kept for the trail; no action.

### 🟡 correctness — Merge-roots-before-children can build cyclic union-find content
`crates/sky_types/src/unify.rs:302-312`
`unify_flat` unions roots with fa's content BEFORE recursing on children, no occurs-check on
Structure-Structure merges. Witness: `b : (Int,Int); b=(1,2); x = if True then b else (b,3)`
→ merged class content references itself → `occurs()`/`zonk` (assume acyclic) surface a
CompilerBug instead of a type mismatch, and `IPE_SOLVER_BUDGET=0` risks a hang. **Fix:** add
a seen-set to `occurs()`; in zonk surface `InfiniteType` on a revisited root; correct the stale
acyclicity comments.

### ⚪ correctness — `Bool` admitted as Comparable (`True < False` type-checks)
`crates/sky_types/src/unify.rs:60` (+ `lib.rs:415`) — Elm/Ipê exclude Bool from `comparable`;
Ipê's ord set includes it → acceptance divergence. **Fix:** drop `"Bool"` from both gates, or
record as a sanctioned divergence with a golden test.

---

## canon-rules (audit complete · verified)

### 🟠 correctness — Qualifier member tables merge with silent last-wins overwrite ✓verdict
`crates/sky_canon/src/resolve.rs:1586-1592`
`import App.Utils` + `import Lib.Utils` (both default qualifier `Utils`), or a user module whose
last segment collides with a stdlib qualifier (`import App.Http` → kernel `Http` table):
`qual_map.insert(v, …)` REPLACES, so `Utils.format` / `Http.get` silently resolves to whichever
import came last — semantics-changing wrong-name resolution, zero diagnostic. The unqualified
path has a hard `AmbiguousImport` (IPE-N0024) gate; qualified doesn't. **Fix:** track
qualifier→dep_path ownership; a second distinct dep claiming a qualifier → `DuplicateQualifier`
or deferred `AmbiguousImport` at the qualified use site. Never blind-overwrite.

- 🟡 parse-don't-validate — **Qualified TYPE annotations never validate the member** (`resolve.rs:2832`):
  `Counter.Typo` passes canon → lowerer `other =>` ICE (CompilerBug) or silently matches a bare-name
  builtin. **Fix:** Tier-2 per-qualifier type-member map, fail-closed `NoSuchMember`.
- 🟡 parse-don't-validate — **`{{name}}` interp with unknown bare ident** leaks an unbound `VarLocal`
  → IPE-I0001 CompilerBug ICE instead of a NameError (`resolve.rs:3179`). **Fix:** fallback fails
  closed like `resolve_var` (value-not-found / ambiguous-import).
- 🟡 invalid-states — **Two deps exposing the same-named type ALIAS merge silent first-wins**
  (`resolve.rs:1476`) — unions + values have ambiguity gates; aliases don't. **Fix:** track alias
  origin, emit `AmbiguousImport`/`DuplicateType`.
- ⚪ correctness — **A module's own `exposing (typo)` is never validated** → silently exports nothing
  (`resolve.rs:1744`). **Fix:** emit `ExposedButNotDefined` + did-you-mean.
- ⚪ completeness — **A typo'd `Ipê.*`/`Ipe.*` import is silently skipped** (`resolve.rs:492`).
  **Fix:** `ModuleNotFound` + Levenshtein over `STDLIB_MODULE_QUALIFIERS`.

---

## parse-input (audit complete · verified)

### 🟠 security — Dotted Access chains bypass `MAX_DEPTH` (stack-overflow DoS) ✓verdict
`crates/sky_parse/src/parser.rs:1109-1113` (+ `:1414-1418`, lexer `:616`)
Every recursive path checks `depth > MAX_DEPTH` (256), but `Expr_::Access` nesting is built
ITERATIVELY (`for seg in text.split('.')`) with no bound, and the lexer's dotted-continuation
loop has no segment cap. `x = y` followed by 500k `.a` segments is ONE token → an AST 500k deep
→ stack overflow on the first recursive traversal. Adversarial source input. **Fix:** count
dotted segments against `MAX_DEPTH` in `ident_expr`/`parse_atom_postfix`, or cap the dotted run
in `lex_ident` (reject once at lex time — parse-don't-validate at the boundary).

- 🟡 correctness — **Type annotations attach by name from anywhere; orphan annotations silently
  dropped** (`parser.rs:335`): a misspelled `fooo : Int` above `foo = …` is discarded, program
  compiles unconstrained. **Fix:** positional attach; `AnnotationWithoutDefinition`.
- ⚪ parse-don't-validate — **Exposing lists accept dotted / lowercase-ctor names** (`parser.rs:464`).
- ⚪ correctness — **`i64::MIN` literal unrepresentable** (magnitude lexed before sign, `lexer.rs:407`).
- ⚪ efficiency — **Lexer materialises `Vec<(usize,char)>` for the whole source** (~16× memory blowup;
  no source-size cap) (`lexer.rs:141`). **Fix:** byte-offset cursor + optional max-source gate.

---

## kernels-diagnostics (audit complete · verified)

### 🟠 completeness — Authoritative code list is test-private; `ipe explain` drifted (17 of 85 codes unresolvable) ✓verdict
`crates/sky_diagnostics/src/code.rs:451-469`
The taxonomy's `ALL` slice is under `#[cfg(test)]` and not exported, so ipe hand-mirrors it
(`ipe/src/lib.rs:41-110`, 68 codes vs 85) — while every rendered diagnostic footer tells users
to run `ipe explain <code>`. 17 actively-produced codes (IPE-L0114..L0126, IPE-T0014/15, …) are
unresolvable. **Fix:** promote `ALL` to `pub const ALL_CODES` (single source of truth), delete
ipe's mirror, iterate the one list in `run_explain`/`suggestions`.

- 🟡 invalid-states — **`StdlibKernel::ALL` is a hand-maintained 790-entry mirror with no completeness
  tripwire** (`sky_kernels/src/lib.rs:2270`) — the exact drift that caused the HtmlStyleNode id=None
  seal incident; every tripwire iterates ALL so a missing variant is invisible. **Fix:** derive enum +
  ALL from one macro, or a const-eval `assert!(ALL.len() == VARIANT_COUNT)`.
- 🟡 seal — **`decl().arity` has no mechanical parity gate** vs the constrain-scheme arrow count or
  `lower::callee_arity` (`lib.rs:1881`) — three hand-maintained arity authorities, two past drifts.
  **Fix:** tripwire asserting instantiated-scheme arrows == `decl().arity` == `callee_arity`.
- ⚪ soundness — **`HtmlEventShape::Raw` hard-codes `Arc<dyn Any>` for onSubmit** (`lib.rs:71`) — the
  registry's own dyn-Any channel; runtime downcast mismatch = runtime failure. **Fix:** monomorphise
  per concrete `a`, or make the downcast a typed Err.
- ⚪ parse-don't-validate — **`CompilerBug.where_` stringly-typed with silent `_ => IPE_I0001`**
  (`diagnostic.rs:1002`). **Fix:** `BugSite` enum.
- ⚪ readability — **Stale doc drift** (`InadmissibleAppMsg` tagged L0122 but maps L0125; stale
  "Absent from ALL" comment) (`diagnostic.rs:657`).

---

## ipe-tools (audit complete · verified)

### 🟠 security — Unsandboxed `cargo fetch` + nightly rustdoc executes untrusted crate build.rs/proc-macros (RCE) ✓verdict
`tools/sky-ffi-inspect-rs/src/main.rs:1161` (+ `:1286`)
`inspect_crate` takes an arbitrary crates.io name / `--git` URL, scaffolds a probe, and runs
`cargo fetch` + `cargo +nightly rustdoc`. rustdoc runs AFTER macro expansion → the target crate's
(and every transitive dep's) `build.rs` + proc-macros execute with full user privileges =
arbitrary code execution from the exact untrusted input the tool consumes. This is the
documented-but-open FFI-sandbox gate (relates memory `security-hardening-before-push`,
`ffi-subsystem`; backlog Tier-2 FFI). **Fix:** ship the sandbox before FFI — fetch+rustdoc inside
landlock/seccomp (no-net-after-fetch, tmpfs writes) or a rootless container; interim `cargo
metadata --offline` build-script denylist + a loud `--allow-build-scripts` gate.

- 🟡 correctness — **`build_and_run_rust` runs the emitted golden with no timeout / unbounded capture**
  (`tools/oracle/src/lib.rs:532`) — a looping golden hangs the parity harness (violates non-negotiable
  #3). **Fix:** `wait_timeout` + SIGKILL + capped capture.
- 🟡 correctness — **`scan_lower_arms` counts ANY `KernelFn::` mention as a lower arm**
  (`tools/parity-matrix/src/main.rs:915`) → masks `lower_arm_missing` MISMATCH from the CI seal gate.
  **Fix:** anchor the scan to the `fn lower_callee` body + `=> Callee::Kernel(...)` arms.
- ⚪ correctness — **`extract_imports_from_source` reads `import X` inside multiline strings**
  (`ipe/src/project.rs:457`) → phantom graph edges / bogus IPE-N0021 cycle. **Fix:** track triple-quote
  state or reuse the parsed header.
- ⚪ security — **`write_atomic` predictable temp name (symlink-follow) + drops original file mode**
  (`ipe/src/lib.rs:1154`): `ipe fix` in a shared dir → symlink clobber; 0600→0644 permission
  downgrade. **Fix:** `create_new` (O_EXCL) + random suffix + `fchmod` to original mode.
- ⚪ parse-don't-validate — **Invalid `IPE_RUNTIME_DIR` silently ignored** (falls back to the walk;
  version-skew → seal-adjacent cargo-fail) (`ipe/src/lib.rs:657`). **Fix:** typed error, honour or reject.
- ⚪ correctness — **`find_rustdoc_json` falls back to ANY `*.json`** → can bind a stale/wrong crate's
  rustdoc (`ffi-inspect main.rs:1265`). **Fix:** resolve the true lib-target name from `cargo metadata`.

---

## Top actionable (hardening order — security/soundness/seal first)

All 14 verified. Security + seal first:

1. 🔴 `lower.rs:4331` — per-occurrence `any` → concrete/alpha-renamed generic (seal).
2. 🟠 `sky-ffi-inspect-rs:1161` — sandbox untrusted crate build before FFI ships (security, RCE).
3. 🟠 `auth.rs:184` — JWT `leeway=0` + `reject_tokens_expiring_in_less_than=1` (security).
4. 🟠 `parser.rs:1109` — cap dotted-Access segments against `MAX_DEPTH` (security, DoS).
5. 🟠 `db.rs:103` — pool-identity in txn routing (correctness, cross-tenant).
6. 🟠 `lib.rs:261` (types) — gate numeric-default pin on the class Append bound (soundness).
7. 🟠 `constrain.rs:1576` — split `Ty::Var` sym/ordinal spaces (parse-don't-validate).
8. 🟠 `emit_expr.rs:4849`/`4659` — kill textual surgery, do IR-level clone/inline (seal).
9. 🟠 `lower.rs:3548` — re-key `bounds` by `(home, name)` (seal).
10. 🟠 `constrain.rs:4859` — pin Auth claims scheme to `dict(string,string)` (seal).
11. 🟠 `resolve.rs:1586` — qualifier ownership, no last-wins overwrite (correctness).
12. 🟠 `config.rs:10`/`project.rs:284` — wire real `IPE_DB_URL` (correctness).
13. 🟠 `code.rs:456` — promote `ALL_CODES` public, delete ipe's drifting mirror (completeness).

Confirmed-real items are mirrored into `BACKLOG.md`
(Security/hardening tier, AUD-01..15) per the no-deferral rule.

**Status:** audit complete (12/12 partitions, full verify pass). No lanes owed.
