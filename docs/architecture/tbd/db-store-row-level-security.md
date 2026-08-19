# Design: Row-level security for `Ipe.Db.Store` (data-policy, fail-closed)

Status: proposal (tbd). Not yet ratified or implemented. Prior art considered:
Acadia (`acadia.engineering`) — see `misc/acadia-inspection.md`.

> **All fenced blocks below are ILLUSTRATIVE proposed API, not runnable.** None of
> `Policy`, `Secured`, `secured`, `ownerColumn`, or the `…As` operations exists
> yet; the `.ipe` snippets describe the surface to build. The only pre-existing
> code shown is §3 (current `Ipe.Db.Store` signatures, for context). Do not
> copy-run these blocks — implement them per §10–§11.

## 1. Goal

Give `Ipe.Db.Store` **row-level authorization**: a table declares, once, who may
read / insert / delete which rows, and the safe surface **cannot** perform an
out-of-policy operation. Security is the top principle; today `Store` has no
authorization concept, so any code with a `Db` reads and writes every row.

The invariant to deliver, structurally:

> A read or write against a secured store is filtered by that store's policy,
> keyed on an *authenticated* principal, and the policy is enforced at more than
> one independent layer — so no single missed or bypassed check (a wrong
> principal, a raw-SQL escape) opens the hole.

**Defense in depth — v1 ships all three layers, none deferred:**

1. **Authenticated principal (not a raw value).** The policy is keyed on a
   `Principal` that can be minted ONLY by authentication (§4a), so a caller
   cannot fabricate or swap in another subject's id (closes the confused-deputy
   gap). RLS is only as strong as the authenticity of the principal it filters
   on; this makes that authenticity structural.
2. **In-query filter (universal floor).** Every secured read/write pushes the
   policy into SQL (`WHERE … = $principal`) via the store's existing
   validated-identifier + bound-parameter barrier. Works on every dialect
   (SQLite + Postgres); injection-safe by construction.
3. **DB-native enforcement where the dialect supports it (Postgres).** `create`
   also emits `CREATE POLICY` DDL and the connection binds the principal to a
   session setting, so the database enforces row security itself — even against
   a raw `Ipe.Db.Unsafe` statement that bypasses the safe surface. SQLite (no
   dialect RLS) relies on layers 1–2; the store records which layers are active
   so the reduced guarantee is explicit, never silent.

## 2. Why NOT functions-in-records (the encoding decision)

Acadia encodes a policy as a record of closures
(`Security.policy { access = \user row -> … }`). Ipê **forbids
functions-in-records** (IPE-L0107 / TEA-only-state); relaxing it re-opens the
first-class-function closure-in-aggregate lowering the FCF campaign deliberately
stopped, and cannot be scoped to "everywhere except Model" (Model is a naming
convention, not a marked type — the carve-out would need whole-program flow
analysis and the hard lowering is identical either way).

Instead a **policy is DATA** — a small algebra over *validated column names* plus
the principal value. This is strictly better on our own principles:

- **Security:** it compiles into the same injection-safe `SqlFragment` the store
  already builds (`Sql.column` validated + `Sql.param` bound). Out-of-policy
  access is unrepresentable, not merely unchecked.
- **Efficiency + correctness:** because it is data, it **pushes into SQL**
  (`WHERE owner = $principal`) — the database filters; the app never fetches rows
  it may not see.
- **MISU / inspectable:** a policy is a value — testable, comparable, showable,
  reportable by diagnostics.
- **No closures in aggregates:** IPE-L0107 stays intact.

Escape hatch for the rare policy the algebra cannot express: a **call-site
predicate applied after fetch** (a function *argument*, which L0107 permits) or a
disclosed `Ipe.Db.Unsafe` capability. On Postgres the DB-native layer (§7a) still
enforces the policy even under `Ipe.Db.Unsafe`, so the escape hatch is not an
unconditional bypass there.

## 3. Current surface this builds on (do not change its guarantees)

`src/stdlib/Ipe/Db/Store.ipe`:

```ipe
type Store = Store { table : String, columns : List Column, specs : List ColumnSpec, pk : Maybe String }

fromColumns : String -> List Column -> Result Error Store
insert    : Db -> Store -> List ( String, SqlValue ) -> Task Error Int
all       : Db -> Store -> (Row -> Result Error a) -> Task Error (List a)
findWhere : Db -> Store -> (Row -> Result Error a) -> SqlFragment -> Task Error (List a)
get       : Db -> Store -> (Row -> Result Error a) -> SqlValue -> Task Error (Maybe a)
delete    : Db -> Store -> SqlValue -> Task Error Int
create    : Db -> Store -> Task Error (List String)
allOn/getOn/findWhereOn : Connection a -> …   -- external read-only variants
```

The fragment vocabulary it already uses (from the native `Ipe.Db.Sql`):

```ipe
Sql.column : String -> SqlFragment          -- identifier, gated by valid_sql_ident in the kernel
Sql.param  : SqlValue -> SqlFragment         -- value, bound as a parameter (never interpolated)
Sql.eq     : SqlFragment -> SqlFragment -> SqlFragment
-- keyEquals c v = Sql.eq (Sql.column c) (Sql.param v)   -- the exact template a policy fragment follows
```

Reads route through `Db.findWhere conn table fragment`, deletes through
`Db.deleteWhere conn table fragment`. A policy is one more `SqlFragment` `AND`-ed
into that same `WHERE`.

**Prerequisite check for the implementer:** confirm `Ipe.Db.Sql` exposes a
conjunction combinator `Sql.and : SqlFragment -> SqlFragment -> SqlFragment` (and
an `alwaysTrue`/`Sql.true` `1=1` fragment — `Store` already uses `alwaysTrue`
internally). If `Sql.and` is absent, add it to the `Ipe.Db.Sql` kernel module
(a thin wrapper emitting `(<lhs>) AND (<rhs>)`), with the same validated-column /
bound-param discipline; this is the only possible native/kernel change and it is
additive.

## 4. New types (`Ipe.Db.Store`)

```ipe
-- A policy is DATA: a conjunction of authorization rules over the store's
-- columns and the principal. Opaque; built only through the combinators below.
type Policy
    = Policy (List Rule)          -- internal; NOT exposed with (..)

-- One authorization rule. Each maps to a SQL fragment (reads/deletes) and/or an
-- in-code check (inserts). Closed set — a new prim forces a compile error in the
-- compiler, never a silent fall-through.
type Rule
    = OwnerColumn String          -- <col> = $principal   (col validated at build)
    | Immutable String            -- reserved for a future update op; see §9
    | PublicRead                  -- reads unrestricted; writes still gated by other rules

-- A store carrying a policy. The ONLY value the `…As` operations (§7) accept;
-- reached only through `secured` (§6). Its sibling handle is a plain `Store` from
-- `unrestrictedStore`. Both start from an `UnsecuredStore`, so an undecided table
-- cannot reach any operation.
type Secured
    = Secured Store Policy

-- A defined table that has NOT yet made an authorization decision. `fromColumns`
-- returns this; it accepts no read/write op. `secured` or `unrestrictedStore`
-- turns it into a usable handle (§6). This is the fail-closed gate.
type UnsecuredStore
    = UnsecuredStore Store
```

Exposure: add `Policy` (opaque — NO `(..)`), `Secured` (opaque), `UnsecuredStore`
(opaque), `Principal` (opaque), `fromColumns` (return type changed), `secured`,
`unrestrictedStore`, the combinators, and the secured operations to the module's
`exposing` list. Keep `Rule` internal (not exposed).

### 4a. `Principal` — the authenticated subject (defense layer 1)

```ipe
-- The identity a secured operation filters on. OPAQUE, and mintable ONLY by
-- authentication — there is no `Principal` constructor from a bare String or
-- SqlValue on the safe surface, so a caller cannot fabricate or swap in another
-- subject's id. Parse-don't-validate: holding a `Principal` is proof the request
-- was authenticated as this subject.
type Principal
    = Principal SqlValue          -- internal; NOT exposed with (..)
```

The ONLY safe way to obtain a `Principal` is from an authentication result — e.g.
a verified session/JWT subject claim:

```ipe
-- Mints a Principal from a verified auth subject. The auth layer is the sole
-- producer; it runs AFTER signature/expiry verification, so the subject is
-- attested, not caller-supplied.
principalFromAuth : Auth.VerifiedSubject -> Principal
```

If the auth surface does not yet expose a typed verified-subject, this feature
adds the minimal one (a `VerifiedSubject` returned only by token/session
verification) as part of the work — the point of the layer is that a `Principal`
cannot exist without an authentication having happened. A deliberate,
audited `Ipe.Db.Unsafe.principalUnchecked : SqlValue -> Principal` MAY exist for
tests/tools, capability-disclosed like the rest of `Unsafe` — never on the safe
surface.

## 5. Policy combinators (full API + semantics + SQL)

```ipe
-- Deny by default is the ABSENCE of a policy (you cannot reach a Secured without
-- one). `unrestricted` is the EXPLICIT opt-out.
unrestricted : Policy
unrestricted = Policy [ PublicRead ]        -- reads → 1=1 ; inserts/deletes ungated

-- The common case: a column that must equal the principal.
--   read/delete  → WHERE "<col>" = $principal
--   insert       → in-code: the row's <col> value must equal the principal
ownerColumn : String -> Policy
ownerColumn col = Policy [ OwnerColumn col ]   -- `col` re-validated by validSqlIdent at build (§6)

-- Conjunction: every rule of both policies applies.
and : Policy -> Policy -> Policy
and (Policy a) (Policy b) = Policy (a ++ b)
```

`Immutable col` is declared now (so the type is stable) but has **no effect until
an update operation exists** (§9). Do not wire it to any current op.

**Compilation `Policy -> SqlFragment` (reads/deletes), given a `principal :
SqlValue`:**

```
policyFragment principal (Policy rules) =
    -- AND together every read-gating rule; PublicRead / Immutable contribute 1=1
    rules
      |> List.filterMap (ruleReadFragment principal)   -- OwnerColumn c -> Just (Sql.eq (Sql.column c) (Sql.param principal))
      |> conjoin                                        -- fold with Sql.and, empty -> alwaysTrue
```

Re-validate every `OwnerColumn` name through `validSqlIdent` at **build** time
(`secured`), not at compile time — an invalid column returns `Err` from
`secured`, before any store is usable (parse-don't-validate; identical to how
`fromColumns` validates column names).

## 6. Construction (default-deny, fail-closed)

**`fromColumns` alone yields nothing you can run a query on.** It returns an
`UnsecuredStore` — a table definition that has NOT yet made an authorization
decision. To reach a handle the read/write ops accept, you must make that
decision explicitly, one of two ways:

```ipe
-- fromColumns changes its RETURN TYPE: a defined-but-undecided table.
fromColumns : String -> List Column -> Result Error UnsecuredStore

-- Door 1 — policy: attach a real Policy → a Secured (the `…As` ops, §7).
-- Re-validates every column the policy names via validSqlIdent AND checks each
-- is a column of the store; returns Err on the first failure (fail-closed,
-- parse-don't-validate) — no Secured with an unvalidated/absent policy column
-- can exist.
secured : Policy -> UnsecuredStore -> Result Error Secured

-- Door 2 — explicit opt-out: a plain `Store`, accepted by the existing full-access
-- ops (all/get/findWhere/insert/delete/create, §3, signatures UNCHANGED). This is
-- the ONLY way to a `Store`, so every unrestricted table is a grep-able
-- `unrestrictedStore` call site — an enumerable attack surface.
unrestrictedStore : UnsecuredStore -> Store
```

Because the ops accept only `Store` or `Secured`, and the ONLY paths to either run
through `unrestrictedStore` or `secured`, **"forgot to make an authorization
decision" cannot type-check** (MISU + fail-closed by construction — the point of
the design). There is no silent allow-all default.

**This is a breaking stdlib change** — `fromColumns` no longer returns a `Store`.
Every existing caller must insert `|> Result.andThen (secured p)` or
`|> Result.map unrestrictedStore` (a mechanical sweep; §10). The break is the
feature: it forces each table's stance to be stated at the use site. The golden
re-bless this triggers is cheap and automated — not a cost to weigh.

## 7. Secured operations (enforcement points — defense layer 2)

One secured counterpart per current op. Each threads a `principal : Principal`
(§4a — an *authenticated* subject, not a raw value the caller conjures) and
enforces the policy. `policyFragment` binds the principal's inner `SqlValue` as a
parameter via a private accessor; the `Principal` type never leaks its wrapped
value to the safe surface.

```ipe
-- READ all rows the principal may see.
--   emits: SELECT … FROM <table> WHERE <policyFragment principal>
allAs : Db -> Secured -> Principal -> (Row -> Result Error a) -> Task Error (List a)
allAs conn (Secured store policy) principal read =
    Store.findWhere conn store read (policyFragment principal policy)

-- READ rows matching the caller condition, further constrained by policy.
--   emits: … WHERE (<policyFragment>) AND (<cond>)
findWhereAs : Db -> Secured -> Principal -> (Row -> Result Error a) -> SqlFragment -> Task Error (List a)
findWhereAs conn (Secured store policy) principal read cond =
    Store.findWhere conn store read (Sql.and (policyFragment principal policy) cond)

-- READ by primary key, constrained by policy (returns Nothing if the row exists
-- but the policy excludes it — indistinguishable from absent, by design).
--   emits: … WHERE "<pk>" = $key AND (<policyFragment>)
getAs : Db -> Secured -> Principal -> (Row -> Result Error a) -> SqlValue -> Task Error (Maybe a)

-- DELETE by primary key, constrained by policy (a non-owned row is not deleted).
--   emits: DELETE FROM <table> WHERE "<pk>" = $key AND (<policyFragment>)
deleteAs : Db -> Secured -> Principal -> SqlValue -> Task Error Int

-- INSERT with an in-code policy check (SQL has no WHERE on INSERT).
--   For each OwnerColumn c: look up c in `values`; require its SqlValue == the
--   principal's inner value. On mismatch (or missing owner column) → Task.fail
--   (typed error), no SQL issued.
insertAs : Db -> Secured -> Principal -> List ( String, SqlValue ) -> Task Error Int

-- CREATE is unchanged in effect (DDL is not row-scoped); provide a passthrough
-- for symmetry so a caller holding a Secured need not unwrap it.
createSecured : Db -> Secured -> Task Error (List String)
createSecured conn (Secured store _) = Store.create conn store
```

Provide the external read-only counterparts `allAsOn` / `findWhereAsOn` /
`getAsOn` over `Connection a` (mirroring the existing `…On` reads), using
`Db.findWhereOn` with the policy fragment. There is no external write path (a
foreign write is already unrepresentable by `Connection ReadOnly`).

**Insert check detail:** compare `SqlValue`s for equality. If `SqlValue` has no
`==`, add a private `sqlValueEq : SqlValue -> SqlValue -> Bool` (structural over
the `SqlValue` variants), or reuse an existing equality in `Ipe.Db.Sql`. The
check must fail-closed: a missing owner column, a `null` principal, or a mismatch
→ typed `Err`.

## 7a. DB-native enforcement (defense layer 3 — Postgres)

On a Postgres connection, the policy is ALSO enforced by the database, so a
policy-violating operation is blocked even when it does not go through the safe
`…As` surface (a raw `Ipe.Db.Unsafe` statement). Two mechanical parts:

- **`create` emits `CREATE POLICY`.** For a `Secured` store on Postgres,
  `createSecured` additionally emits, through the migrate ledger, `ALTER TABLE
  <t> ENABLE ROW LEVEL SECURITY` + a `CREATE POLICY` per `OwnerColumn` rule whose
  `USING`/`WITH CHECK` clause is `"<col>" = current_setting('ipe.principal')`.
  The clause is built from the SAME `validSqlIdent`-gated column name — no raw
  interpolation. Idempotent + additive, like every other ledger entry.
- **The connection binds the principal.** Each secured operation issues
  `SET LOCAL "ipe.principal" = $principal` (bound parameter) in the same
  transaction before the query, so the DB policy resolves `current_setting`
  against the authenticated `Principal`. `SET LOCAL` is transaction-scoped —
  it cannot leak the principal across requests on a pooled connection.

**Dialect fallback (fail-closed, explicit).** SQLite has no row-level security,
so on SQLite only layers 1–2 apply. `Secured` records which layers are active
(`dbNative : Bool`), and `createSecured` on a dialect without native RLS returns
a typed `Warning`-level note in its result — the weaker guarantee is surfaced,
never silent. It NEVER silently downgrades to "no policy": the in-query filter
(layer 2) always runs regardless of dialect.

`Ipe.Db.Sql`/`Ipe.Db` gains the minimal native support this needs (the
`SET LOCAL` bound-parameter form and the `CREATE POLICY` DDL emit); both go
through the existing validated-identifier + bound-parameter discipline. This is
the second sanctioned kernel touch (alongside `Sql.and`, §3).

## 8. Errors / diagnostics

Add typed constructors to the store's `Error` type (do not stringly-type):

- `PolicyColumnInvalid String` — a policy names a column that fails `validSqlIdent`
  (raised by `secured`).
- `PolicyColumnNotInStore String` — a policy names a column not in the store's
  `columns` (catch a typo at build, not at query time). `secured` checks the
  policy's columns are a subset of the store's columns.
- `InsertPolicyViolation String` — an `insertAs` row's owner column value ≠ the
  principal (or the owner column is absent from `values`).

Render each with a factual message (obey the diagnostic-tone lint; no
history/archaeology). These are runtime `Error` values, not compiler `IPE-Lxxxx`
diagnostics, so no explain page is required.

## 9. Update policy (explicitly deferred)

`Store` has no update operation today, so `Immutable`/before-after rules (Acadia's
`update = \u before after -> before.owner == after.owner`) have nothing to attach
to. Declare `Immutable` in `Rule` for type stability but wire it to NOTHING now.
When an `updateAs` lands, `Immutable col` means "the UPDATE may not change
`<col>`" (enforced by a `WHERE` on the old value + omitting `col` from the SET, or
an in-code before/after check). Track as a follow-up.

## 10. Files to touch

- `src/stdlib/Ipe/Db/Store.ipe` — §4–§8 (types incl. `Principal`, combinators,
  `secured`, the `…As` ops taking `Principal`, errors, the layer-3 wiring). Bulk
  of the work, pure `.ipe`.
- **Auth surface** — a typed `Auth.VerifiedSubject` (or equivalent) returned ONLY
  by token/session verification, and `principalFromAuth` (§4a). If no verified-
  subject type exists yet, add the minimal one — layer 1 (authenticated principal)
  is not optional.
- **`Ipe.Db.Sql` / `Ipe.Db` native** — `Sql.and` (§3) if absent; the `SET LOCAL
  "ipe.principal" = $x` bound-parameter form and the `CREATE POLICY` / `ENABLE
  ROW LEVEL SECURITY` DDL emit for layer 3 (§7a), both through the existing
  validated-ident + bound-param discipline. The dialect probe (Postgres vs
  SQLite) to select whether layer 3 is active.
- Tests: a new `.ipe` test module (or extend the existing Store tests) — see §11.
- **Required caller sweep** (default-deny, §6): `fromColumns` now returns
  `UnsecuredStore`, so every existing caller must map its result through
  `unrestrictedStore` (opt-out) or `secured p` (policy). `rg` for `fromColumns`
  call sites across `src/stdlib`, examples, and tests; update each and re-bless
  goldens. This is part of the work, not optional.

Do NOT touch `Ipe.Db.Store`'s existing identifier-validation, param-binding, or
migrate-ledger guarantees — this feature layers on top of them.

## 11. Tests to write (behavior-parity, real DB where the harness allows)

Follow the existing Store test style. Cover:

1. **Read filtering:** two principals each insert rows; `allAs` for principal A
   returns only A's rows; `findWhereAs` intersects policy AND caller cond;
   `getAs` on B's row as principal A returns `Nothing`.
2. **Delete scoping:** `deleteAs` as A on B's pk deletes 0 rows and leaves B's row.
3. **Insert check:** `insertAs` as A with `owner = A` succeeds; with `owner = B`
   → `Err InsertPolicyViolation` and NO row written; with owner column absent →
   `Err`.
4. **Unrestricted:** `unrestrictedStore` + `allAs` returns all rows (policy → 1=1);
   confirms the explicit opt-out path.
5. **Build validation:** `secured (ownerColumn "owner; DROP TABLE")` → `Err
   PolicyColumnInvalid`; `secured (ownerColumn "nonexistent")` → `Err
   PolicyColumnNotInStore`.
6. **Injection:** a principal `SqlValue` carrying `' OR '1'='1` binds as a
   parameter — the emitted SQL still filters correctly (value never reaches SQL
   text). Assert via the query result, and (if the harness exposes emitted SQL)
   assert the identifier/param barrier.
7. **SQL shape (if inspectable):** `policyFragment` for `ownerColumn "owner"`
   produces `"owner" = $1` (validated ident + bound param), and `and` conjoins
   with `AND`.
8. **Round-trip parity** with an `unrestricted` store vs the raw `Store` ops
   (same rows).
9. **Principal is unforgeable (layer 1):** there is no safe-surface function that
   builds a `Principal` from a `String`/`SqlValue`; the only producer is
   `principalFromAuth` over a verified subject. (Compile-level: assert the module
   does not expose a raw `Principal` constructor; behavioural: a `Principal` from
   auth for subject A filters to A's rows.)
10. **DB-native blocks the Unsafe bypass (layer 3, Postgres only):** on a Postgres
    harness, after `createSecured`, a raw `Ipe.Db.Unsafe` `SELECT`/`UPDATE`
    without the session principal returns zero/forbidden rows for another
    subject's data — the DB policy enforces even off the safe surface. Skip on a
    SQLite harness.
11. **Dialect fallback is explicit (layer 3 absent):** `createSecured` on SQLite
    returns the `Warning`-level "no DB-native RLS on this dialect" note, and
    layer 2 (in-query filter) still filters correctly — never a silent
    downgrade to no policy.

No `panic!`/`unwrap`/bare `expect` anywhere; in tests use `assert`/`assertEqual`.
Cover the negative space: assert the operations that must be REJECTED (forged
principal path absent, cross-subject read empty, Unsafe-bypass blocked on
Postgres), not only the happy path.

## 12. Non-goals

- Update-policy enforcement (deferred, §9 — no update op yet; not a security
  deferral — there is no update op to secure).
- Cross-table / subquery policies (e.g. "visible via a share table"); a `viaJoin`
  rule is possible future work. The three defense layers apply to the rules that
  exist.
- A phantom `auth` *type parameter* on `Store`/`Secured` (§13.2) — the
  authenticated identity is a `Principal` *value* (§4a), which is what closes the
  confused-deputy gap; the phantom type param is a separate, unrelated idea.
- Any change to the raw `Store` ops' behavior.

**Explicitly NOT deferred (all in v1, §1):** the authenticated `Principal`
(layer 1) and DB-native `CREATE POLICY` enforcement on Postgres (layer 3). No
security hardening ships later — all three layers land together.

## 13. Open decisions (for the human before/at implementation)

1. **DECIDED — default-deny (§6).** `fromColumns` returns an `UnsecuredStore` that
   MUST be turned into a `Store` (via `unrestrictedStore`) or a `Secured` (via
   `secured`) before any op, so "forgot to make an authorization decision" cannot
   compile. This is the security-first form; it is a breaking stdlib change plus a
   required caller sweep (§10), both accepted. The rejected alternative — keep
   `fromColumns : … -> Result Error Store` and make security opt-in (no break, but
   a silent allow-all default) — was declined: an opt-in default contradicts
   fail-closed, the top-principle stance for an authorization feature.
2. **Principal authenticity — DECIDED via a `Principal` value (§4a), not a phantom
   type parameter.** Acadia tags tables `Table UserID Food` (a phantom type). Ipê
   instead makes the identity an opaque `Principal` *value* mintable only by auth:
   this closes the confused-deputy gap (a caller cannot forge/swap the subject)
   without needing a `auth -> SqlValue` converter stored in an aggregate (which
   L0107 forbids). The phantom *type* param remains a non-goal (§12) — it checks
   the principal's type but not its authenticity; the value approach gives the
   security property that matters.
3. **`SqlValue` equality** for the insert check (§7) — reuse if present, else add
   a small structural `sqlValueEq`.
4. **Auth verified-subject surface.** Layer 1 needs a typed
   `Auth.VerifiedSubject` produced only by verification. If absent today, this
   feature adds the minimal one (§10) — decided, since layer 1 is not optional.

## 14. Principle ledger (why this shape)

- **security > correctness > soundness > efficiency > completeness > readability:**
  security-first (RLS), fail-closed (no policy ⇒ no `Secured`; build rejects bad
  columns; insert rejects on mismatch), MISU (out-of-policy op unrepresentable on
  the safe surface), parse-don't-validate (policy columns validated once at
  `secured`; a `Principal` is proof-of-authentication, mintable only at the auth
  boundary), SSOT (one policy drives read/delete fragment AND insert check AND the
  DB-native `CREATE POLICY`), concrete-over-generic (policy is a closed `Rule`
  sum, no closures), and it keeps L0107. Efficiency is a bonus (SQL pushdown),
  never traded against the above.
- **Defense in depth:** the policy is enforced at three independent layers — an
  authenticated `Principal` (can't forge the subject), the in-query filter
  (universal, injection-safe), and DB-native `CREATE POLICY` on Postgres (holds
  even against raw `Unsafe`). No single missed check opens the hole; the reduced
  SQLite guarantee (layers 1–2) is surfaced, never silent.
