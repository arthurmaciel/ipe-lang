# Class 6 fix spec — `Secret` (#44) + `SqlFragment` (#61)

> Synthesis of 3 independent guardian-design reasoners (2026-07-09), reconciling
> `docs/architecture/class6-secret-sqlfragment-questions-2026-07-09.md`. This
> spec is the implementable target. Base documents: `docs/superpowers/plans/2026-07-02-secret-type.md`
> (Secret, patched below) and `docs/architecture/additive-stdlib-features.md:365-460`
> (SqlFragment sketch, patched below).

## Decision record — where the reasoners agreed

All three: **Auth re-typing is in scope** (the plan's deferral premise is
dead — `AuthSignToken`/`AuthVerifyToken` kernels already exist,
`crates/sky_kernels/src/lib.rs:983-984,2119-2120`; zero fixtures call them
today, so migration cost is zero). **Zeroize ships now**, not deferred
(security tier is pre-push; hardening never defers). **`lit` and `group` are
dropped** from SqlFragment (every whitelisted operator is a typed combinator;
every combinator emits unconditional parens, making precedence bugs
unrepresentable). **Empty `inList` emits `(1 = 0)`.** **No new diagnostic
codes** — everything routes through existing SKY-T0001/T0014/L0120. **Secret
is Model-inadmissible by design** (never serializes — SKY-L0120 catches it
for free once Secret is marked non-serde). **No shared capability-table
mechanism** for n=2 opaque types — a documented checklist, not a framework.

## Decision record — where they disagreed, and the resolution

### 1. Secret's equality/stringify semantics — adopt reasoner C's design, with a further simplification

Reasoners A and B proposed: Secret is non-derivable (like `Task`/`Db`),
`==`/`toString` are **type errors** (SKY-T0014) via a new
`ty_is_equatable` denylist, with `Secret.redacted`/`Secret.constantTimeEq`
as explicit escapes. Reasoner C found a real defect in this: marking a leaf
type non-derivable makes **every record/enum containing it** lose ALL
derives, not just the missing ones — `ir_type_is_derivable` /
`emit_types.rs:408-440`'s gate is all-or-nothing. A record
`{ signingKey : Secret, issuer : String }` would silently lose `Clone` too,
an exit-0-then-cargo-fail (the exact class #45/#70 exist to close). A and
B's own fix for this was to introduce a new per-trait `IrTypeCaps`
mechanism. C's fix is simpler: give `Secret` a **hand-written,
constant-time `PartialEq`** (via `subtle::ConstantTimeEq`, already a
runtime dep) so it stays fully derivable — `Clone, Debug (redacting),
PartialEq (constant-time)` — with no `Hash`/`Ord`/serde. **`==` is then
always safe by construction: there is no other `PartialEq` impl to
accidentally call, so allowing it costs nothing security-wise and avoids
inventing a new capability-table mechanism.**

**This spec adopts C's mechanism, extended one step further for
consistency: `toString`/interpolation is ALSO allowed, always redacting**
(the hand-written `SkyStringify`/`Debug` impls return `"<redacted>"`
unconditionally). Rationale: once equality is "safe by construction, no
escape needed," the same philosophy should apply to stringify — a
user who forgets to call `Secret.redacted` gets the safe redacted output
automatically, rather than a compile error whose only fix is remembering
the same escape hatch. This is strictly friendlier AND no less secure (the
only path to plaintext remains the single `reveal` call). `constantTimeEq`
as a *named kernel* is dropped — `==` already **is** the constant-time
compare, one way to do the thing.

**Net result: no `ty_is_equatable` denylist entry is needed for Secret at
all.** `Secret` is `IrType::Secret`, `ir_type_is_derivable = true`,
`ir_type_is_serde = false` (this is also the free transitive-containment
predicate any future WASM `HydrationState` gate needs).

### 2. SqlFragment's equality/printing — same simplification applies, verify one precondition first

None of the three reasoners noticed that `SqlParam` (the runtime's 6-variant
SQL bind-value enum, `runtime/src/sky_runtime/db.rs:1707-1721`) likely
**already has every constituent type implementing `PartialEq`** (String,
i64, f64, bool, `Vec<u8>`, and the Decimal/Time/Money wrapper types are all
comparable in this codebase already). **Implementation must verify this
first** (grep the concrete `SqlParam` variant field types for existing
`PartialEq`), then: add `#[derive(PartialEq)]` to `SqlParam` if missing, and
give `SqlFragment` a plain `#[derive(Clone, PartialEq)]` (comparing `sql`
text + `binds` + `invalid` state structurally — a meaningful, no-security-
concern equality, unlike Secret's). **`Debug` for `SqlFragment` stays
hand-written regardless** — SQL text + bind **count** only, never bind
*values* (a bind may be a revealed Secret; this is the one place the two
items intersect, and it resolves the same way both items resolve elsewhere:
safe-by-construction, no reliance on a caller remembering an escape). If the
precondition fails (some `SqlParam` variant's field type genuinely isn't
`PartialEq`), fall back to reasoner A/B's `ty_is_equatable`-denylist design
for `SqlFragment` only (`Secret` is unaffected either way since it never
needed the denylist).

**Net result: this design needs zero new type-checker equatable/showable
machinery for either type** — a further simplification beyond what any
single reasoner reached. `frag == frag` and `toString frag` are both
**allowed**, both safe by construction. Raw-`String`-where-`SqlFragment`-
expected is the existing ordinary SKY-T0001 (`String` vs `SqlFragment` Con
mismatch) — no new code either way.

### 3. `unsafeFindWhere`'s fate — remove now (majority, with C's dissent recorded)

A and B: remove it in the same change (the security-tier's whole point is
closing representable SQL injection; "eventually deny-flagged" is the
forbidden deferral shape; zero fixtures call it today, verified by grep, so
migration cost is zero). C: keep it, citing the sanctioned-divergence
policy's "Go parity is default" framing and the existing `unsafe`-prefix
naming convention as an eyes-open escape hatch.

**Resolution: remove it now.** This backlog section is explicitly headed
"Security tier — BEFORE the compiler push," and CLAUDE.md's no-deferral
principle treats "known broken edge case, fix later" as a forbidden shipping
excuse regardless of upstream-parity framing — Go-parity is the default for
*language semantics*, not for *carrying forward a raw-SQL escape hatch a
brand-new safe API supersedes in the same commit, at zero migration cost*.
Delete `DbUnsafeFindWhere` (kernel variant + decl + `ALL`), `db_unsafe_find_where`
(`db.rs:1438-1456`), its canon/constrain/lower rows, and its one golden
fixture (rewritten to `Db.findWhere`, see §Tests). Ledger entry in
`docs/divergences-from-sky.md` (upstream keeps the raw-WHERE API; sanctioned
divergence, strictly-better-security class).

### 4. Sequencing — #61 first, then #44

A and B wanted #44 first because it introduces shared machinery #61 would
otherwise duplicate. That argument dissolves under this spec's
simplification (§1/§2): **neither item needs the other's machinery** — no
shared denylist helper exists to sequence around. Given that, prioritize by
security urgency: **#61 closes a live, representable SQL-injection surface
today** (`unsafeFindWhere`'s verbatim string interpolation); **#44 is
defense-in-depth for a type with zero current plaintext-secret fixtures**.
Land #61 first. Both still append to the tail of `StdlibKernel`/`ALL`/
`decl`/`kernel_name`/`callee_arity` — land sequentially on one branch (not
concurrent worktrees) so the second lands as a clean tail-append rebase, per
the plan's own discriminant-shift warning.

## Full design — #61 `SqlFragment` (land first)

**Surface** (`Std.Db.Sql`, qualifier `Sql`; consumers under `Db`):

```elm
column : String -> SqlFragment   -- valid_sql_ident-gated (db.rs:1768-1773, the DOT-ACCEPTING
                                  -- validator — required for `users.id`; invalid input poisons
                                  -- the fragment, propagated by every combinator, surfaced as
                                  -- a Task Err at the consumer, never a panic)
param  : SqlValue -> SqlFragment  -- "?" + bind
int, string, float, bool : ... -> SqlFragment   -- pure-Sky sugar over `param`, zero new kernels

eq, ne, gt, lt, gte, lte : SqlFragment -> SqlFragment -> SqlFragment
and, or : SqlFragment -> SqlFragment -> SqlFragment      -- Sql.and / Sql.or / Sql.not: plain
not     : SqlFragment -> SqlFragment                     -- words, no trailing underscores (not
isNull, isNotNull : SqlFragment -> SqlFragment            -- lexer keywords; qualified usage only)
inList  : SqlFragment -> List SqlValue -> SqlFragment    -- [] -> "(1 = 0)"
like    : SqlFragment -> String -> SqlFragment           -- pattern is a bound param

-- consumers, v1 scope line: WHERE-position predicates only
Db.findWhere   : Db -> String -> SqlFragment -> Task Error (List (Dict String String))
Db.deleteWhere : Db -> String -> SqlFragment -> Task Error Int
-- REMOVED: Db.unsafeFindWhere
```

Explicitly OUT of v1, filed as named backlog follow-ups (completeness, not
security — none reopen the injection surface): `Db.updateWhere`, typed
RETURNING projections, `orderBy`/`limit` fragments.

**Runtime** (`runtime/src/sky_runtime/db.rs`, un-gated like `SqlParam`; only
the two consumers are `#[cfg(feature = "db")]`):

```rust
#[derive(Clone, PartialEq)]   // see precondition note above
pub struct SqlFragment {
    sql: String,              // "?"-placeholder text; every combinator's output is
                               // unconditionally parenthesized
    binds: Vec<SqlParam>,     // NOT the lowerer-synthesized SqlValue enum — see wiring below
    invalid: Option<String>,  // poison: first invalid-column-reference reason; propagates
}
// Hand-written Debug: sql text + bind COUNT — never bind values (constant across both items'
// intersection: a bind may be a revealed Secret).
```

- `sql_column`: `valid_sql_ident` (dot-accepting) → poison on failure, never
  panics. `sql_param`/`sql_in_list`: generic `Into<SqlParam>` bound —
  `impl From<StdDbSqlValue> for SqlParam` is already emitted project-wide
  whenever `uses_db` (`project.rs:1033-1090`), so the collapse from the
  lowerer-synthesized `SqlValue` enum to the runtime's `SqlParam` happens at
  this generic boundary with no new emit special-casing for the *value*
  path. **The one genuinely new lowering touch:** the synthetic `SqlValue`
  enum is currently injected into `types_ir` only when `expr_uses_db_kernel`
  fires (`lower.rs:3096`); extend that trigger to also fire on any `Sql.*`
  fragment kernel, else a program using `Sql.param (SqlInt 5)` with no
  `Db.*` call compiles clean but cargo-fails on the missing enum definition
  (an exit-0-then-cargo-fail the sketch's kernel-count estimate missed).
- Combinators: string-concatenate with unconditional parens, merge binds,
  propagate first-poison-wins.
- Consumers: check `invalid` → `Task::Err`; validate the table name via
  `SqlIdent::parse` (no dot — tables are never dotted here); assemble
  `SELECT * FROM {t} WHERE {frag.sql}` / `DELETE FROM {t} WHERE {frag.sql}`;
  route through `db_format_sql` (the existing per-driver dialect seam,
  `config.rs:32`) so a future Postgres-reachability fix (AUD-09) picks up
  fragment queries for free — nothing dialect-specific is stored in the
  type; bind via the existing `bind_sql_param` loop (`db.rs:1787`).

**Compiler wiring** (kernel-builtin path, mirrors `Decimal`/`Db`):
`IrType::SqlFragment` leaf, derivable (per §2), non-serde; `"SqlFragment"`
into `RESERVED_BUILTIN_TYPES` (not `EXTRA_BUILTIN_TYPE_NAMES` — user
shadowing of a security-tier type must be a hard canon error, matching the
`SqlValue`/`SqlField` precedent); ~16 kernels appended at the tail of
`StdlibKernel`/`ALL`/`decl`/`naming.rs`; schemes in `constrain.rs` reusing
the existing `sqlvalue_ty` builder; `("Db","unsafeFindWhere")` rows deleted
everywhere. Add both consumers to `is_db()` so the sqlx dep-injection gate
still triggers correctly.

**Tests:** positive self-oracle golden exercising every combinator at least
once (a per-family drift tripwire, doubling as the #45/#70 completeness gate
for this new kernel family) + empty-`inList` + dotted-`column` + poisoned-
column-caught-as-Task-Err + `deleteWhere` row count; rewrite the existing
`unsafeFindWhere` golden to `findWhere`; negative golden for raw-String-where-
fragment-expected (SKY-T0001). All divergence-marked (`oracle_divergence =
true`) per the `m5b_db_*` convention — no Go counterpart exists.

## Full design — #44 `Secret` (land second)

**Surface** (`Sky.Core.Secret`, 3 kernels):

```elm
fromString : String -> Secret     -- the seal; construction boundary
reveal     : Secret -> String     -- THE single greppable un-parse
redacted   : Secret -> String     -- explicit "<redacted>" (also what toString gives automatically)
-- plus, re-typed (not new):
Auth.signToken   : Secret -> Dict String String -> Int -> Result Error String
Auth.verifyToken : Secret -> String -> Result Error (Dict String String)
```

`==`/`toString`/interpolation are all **allowed**, safe by construction (§1).
Dict-key/Set-element/ordering are **already rejected** with zero new code —
verified: the comparable-key and ord gates in `sky_types` are scalar
*allowlists* (`Int|Float|Char|String|Bool`), so a bare `Ty::Con` like Secret
is already outside them, surfacing the existing SKY-T0014 naming
"Comparable" with no denylist needed.

**Runtime** (`runtime/src/sky_runtime/secret.rs`):

```rust
#[derive(Clone)]
pub struct Secret(String);
impl PartialEq for Secret {                    // the ONLY equality; constant-time
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (self.0.as_bytes(), other.0.as_bytes());
        a.len() == b.len() && bool::from(a.ct_eq(b))   // length is metadata, not payload
    }
}
impl Drop for Secret { fn drop(&mut self) { self.0.zeroize(); } }   // ships now, not deferred
// redacting Debug; NO Display; redacting SkyStringify; NO Hash/Ord/serde.
pub fn secret_reveal(mut s: Secret) -> String { std::mem::take(&mut s.0) }  // avoids E0509 under Drop
```

`zeroize = "1"` new runtime dep (alongside the already-present `subtle`).
Payload stays `String` in v1 — every current consumer (Auth, env vars) is
string-shaped; a `Bytes`-payload variant is an additive follow-up, filed, not
built speculatively.

**Compiler wiring:** `IrType::Secret` leaf, **derivable = true** (per §1),
non-serde; `"Secret"` into `RESERVED_BUILTIN_TYPES`; 3 kernels appended at
the tail; Auth's two kernel schemes re-typed (first argument `String` →
`Secret`) in `constrain.rs`; `AUTH_WRAPPERS` (`project.rs`) signatures
follow; `Std/Auth.sky` / `Sky/Core/Secret.sky` stdlib source.

**Explicitly filed as follow-ups, not built now** (all three reasoners
agreed these are additive, no current consumer needs them): `System.getenvSecret`
companion kernel (construction ergonomics; `Task.map Secret.fromString` over
`System.getenv` is the v1 pattern); a committed-secret-literal lint;
`Secret`-accepting `Log.*`/`Trace.*` overloads (rejected as a *design*, not
just deferred — normalizes routing secrets toward the logging subsystem;
`Secret.redacted` already covers the use case); WASM `HydrationState`
containment gate (nothing to gate — the target doesn't exist yet; the
non-serde classification already IS the future predicate).

**Tests:** positive self-oracle golden — seal, `==` match/mismatch/length-
mismatch, `toString`/interpolation (asserts `"<redacted>"`), record
containing a Secret (proves the derive-blast-radius fix: the record still
gets `Clone`/`Debug`/`==`), `reveal`, with a plaintext grep-guard (the secret
value appears exactly once, on the `reveal` line). Auth round-trip golden
(`Secret.fromString` at the boundary, divergence-marked). Model-gate negative
test: a Live Model field of type `Secret` → SKY-L0120, naming the field.

## Documentation updates required in the same change(s)

- `docs/divergences-from-sky.md`: Secret sealed-newtype + constant-time-`==`
  semantics; Auth re-typing; `unsafeFindWhere` removal; SqlFragment feature
  (no Go counterpart).
- `docs/stdlib.md`: both new surfaces; the "hold secrets in a top-level
  binding, not the Model" pattern for Live apps.
- CLAUDE.md §8 wording: "Auth.signToken/verifyToken take `Secret`, not
  `String` or `any`."
- Backlog follow-up entries: `getenvSecret`, committed-secret lint, `Bytes`
  payload variant, `updateWhere`/typed-projection/`orderBy`+`limit` fragment
  families, `SqlParam`-derive-precondition fallback note if it was needed.
