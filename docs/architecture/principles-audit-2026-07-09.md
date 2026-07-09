# Principles Audit — 2026-07-09 (Fable hardening swarm)

Whole-codebase audit of the **Ipê Rust project only** (`crates/`, `runtime/`,
`tools/` — `../sky` excluded) against `PRINCIPLES.md`: the 6 ranked principles
(1 Security · 2 Correctness · 3 Soundness · 4 Efficiency · 5 Completeness ·
6 Readability), the two fundamental rules (PARSE-DON'T-VALIDATE ·
MAKE-INVALID-STATES-UNREPRESENTABLE), and **the seal** (skyc exit 0 ⟹ cargo
build exit 0; concrete-over-generic; no `dyn Any`).

Method: 12 Fable auditors, one per subsystem partition, read-only, each reporting
reachable holes with `file:line` evidence; a Fable adversarial-verify pass on
every high/critical finding.

> **Run status — PARTIAL.** The weekly token limit tripped mid-run (resets
> 2026-07-11 17:00 America/Sao_Paulo). **7 of 12 audit partitions completed**;
> the **verify pass mostly did not run** (only 1 finding carries a completed
> verdict). Findings below without a ✓verdict are auditor-reported at the stated
> confidence and still need adversarial verification. Raw run:
> `wf_3f451707-748` (result JSON archived in the session task output).
>
> **Not yet audited (rerun after limit reset):** `types-seal`, `canon-rules`,
> `kernels-diagnostics`, `skyc-tools`, `parse-input`. Verify pass owed for:
> `rt-auth-crypto`, `backend-emit`, `lower-ir-seal`, and the 2nd `rt-db-sql`
> finding.

Severity legend: 🔴 critical · 🟠 high · 🟡 medium · ⚪ low. `✓verdict` = passed
adversarial verify; `⧗verify-owed` = auditor-confident, verify blocked by limit.

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
so `f : any -> any -> Int; f _ _ = 0` called `f "x" 3` is well-typed → skyc exit 0.
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
no Sky diagnostic). **Fix:** replace with a shape-preserving `map_bodies` combinator
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
`_` is a legal Sky ident char, so `fooBar` and `foo_bar` are distinct values →
both snake-case to `main_foo_bar` → two `fn main_foo_bar` → **E0428**. The enum
path guards this (`:300-314`) and records dedup with suffixes; the func path
inserts blind. **Fix:** mirror the enum guard — dedup with numeric suffix or
fail-closed naming both Sky sources.

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
Locals emitted verbatim + `pub use sky_runtime::*` → a Sky local named `task_run`/
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
Scheme types claims as `var(0)` (unifies with anything → skyc exit 0) but the wrapper
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
  allowlist, or a `SKY_LIVE_FORCE_SECURE_COOKIES` override; thread into `__Host-` choice.
- ⚪ security — **`/_sky/observability/ingest` CSRF-exempt AND open in dev**
  (`live/csrf.rs:141`): cross-site POST folds forged telemetry into the console (dev
  only; prod fails closed). **Fix:** remove from `is_exempt_path`, or bind dev-open to
  loopback + missing-Origin.
- ⚪ security — **Sky.Http.Server WS upgrade does no Origin check outside production**
  (`server.rs:974`): empty `originPatterns` + non-prod ENV → any Origin upgraded (CSWSH).
  **Fix:** default-deny cross-origin when patterns empty, independent of the ENV gate.
- ⚪ correctness — **`live_max_body_bytes()` lacks the `>0` floor** `server::max_body` has
  (`live/mod.rs:864`): `SKY_LIVE_MAX_BODY_BYTES=0` → every `/_sky/event` 413s. **Fix:**
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
  `config.rs` `SKY_DB_URL` from sky.toml (never `:memory:`), or read `DATABASE_URL`
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

Posture very strong — **no reachable panic/unwrap/OOB/downcast from well-typed Sky**;
fallible paths Maybe/Result-typed, casts clamp-then-narrow, sorts `catch_unwind`-guarded,
integer overflow wraps (Go parity via emitted `overflow-checks=false`). Residual:

- ⚪ correctness — **`List.range` silently truncates to a 10M prefix** (`list.rs:128`):
  large-but-finite range → wrong short list vs Go's full list; not in the divergence
  ledger. **Fix:** record as sanctioned DoS-guard divergence, or return `Result` on over-cap.
- ⚪ correctness — **`Math.abs` (saturating) vs `Basics.abs` (wrapping) disagree at
  `i64::MIN`** and `Math.abs` diverges from Go (`math.rs:43`). **Fix:** one i64::MIN
  semantics for both, matched to the Go oracle; fixture at i64::MIN.

---

## Top actionable (hardening order — security/soundness/seal first)

1. 🔴 `lower.rs:4331` — per-occurrence `any` → concrete/alpha-renamed generic (seal).
2. 🟠 `auth.rs:184` — JWT `leeway=0` + `reject_tokens_expiring_in_less_than=1` (security).
3. 🟠 `db.rs:103` — pool-identity in txn routing (correctness, cross-tenant) ✓verified.
4. 🟠 `emit_expr.rs:4849`/`4659` — kill textual surgery, do IR-level clone/inline (seal).
5. 🟠 `lower.rs:3548` — re-key `bounds` by `(home, name)` (seal).
6. 🟠 `constrain.rs:4859` — pin Auth claims scheme to `dict(string,string)` (seal).
7. 🟠 `config.rs:10`/`project.rs:284` — wire real `SKY_DB_URL` (correctness).

Confirmed-real + high-confidence items are mirrored into
`docs/architecture/backlog.md` (Security/hardening tier) per the no-deferral rule.

**Owed:** rerun `types-seal`, `canon-rules`, `kernels-diagnostics`, `skyc-tools`,
`parse-input` audits + the verify pass after the limit resets (2026-07-11 17:00).
Resume: `Workflow({scriptPath: "…/ipe-hardening-audit-wf_3f451707-748.js",
resumeFromRunId: "wf_3f451707-748"})` — completed lanes return cached.
