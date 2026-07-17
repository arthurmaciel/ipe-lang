# RT-DATA findings

4 findings: 0 critical, 0 high, 1 medium, 3 low.

Audited: `src/runtime/rust/src/db.rs` (full), `src/runtime/rust/src/json.rs` (full),
`src/runtime/rust/src/config_decode.rs` (full), `src/runtime/rust/src/config.rs`,
plus the `Db.Decode` kernel routing (`src/compiler/kernels/src/lib.rs`,
`src/compiler/backend/rust/src/naming.rs`) to establish surface reachability.

Prior-audit items in this partition, verified FIXED (not re-filed):
- `decode_pipeline_optional` present-but-malformed swallow → now propagates the
  decode error (`json.rs:1364-1380`) with regression tests covering all four
  cases (`json.rs:1484-1534`).
- `json_enc_encode` unbounded/panicking indent → clamped to 16 spaces
  (`json.rs:74-76`) with a regression test (`json.rs:1461`).
- `db_update_fields` empty-WHERE unscoped UPDATE → fails closed
  (`db.rs:2299-2305`) with a regression test (`db.rs:3737`).
- Raw-`environ` env reads in `config_decode`/`db` → routed through
  `crate::system::read_env_var` (`config_decode.rs:72,187`; `db.rs:639,652,969`).
- YAML billion-laughs: `config_decode_yaml` now enforces a 4 MiB source cap
  (`config_decode.rs:69-97`) and relies on serde_yaml 0.9's built-in alias
  repetition limit for expansion — residual gap filed as RT-DATA-004.
- SQL parameter binding across the whole module is sound: every value binds
  positionally (`q.bind` / `bind_sql_param`), identifiers pass `SqlIdent::parse`
  or `valid_sql_ident` before interpolation, `unsafeFindWhere` is replaced by
  the closed `SqlFragment` builder (poison-on-invalid, binds/placeholders in
  lockstep), the `RETURNING` projection is identifier-validated, and injection
  round-trip tests exist (`db.rs:3701`, `db.rs:3617`). No injection path found.

## RT-DATA-001 · row-to-value bridge silently coerces undecodable driver columns to NULL/empty
- severity: medium
- axis: correctness
- principle: "Parse, don't validate" / P2 correctness — a present-but-unreadable value must not be represented as a trusted absent/NULL value
- location: `src/runtime/rust/src/db.rs:220-241` (`row_to_json`, fallback arm `Err(_) => JsonVal::Null`), `src/runtime/rust/src/db.rs:186-205` (`row_to_map`, fallback `String::new()`)
- reachability: every untyped `Db.query`/`findOneByField`/… result and every typed `Db.queryDecode`/`getByIdDecode`/`insertFieldsReturning` row passes through these bridges. They try only `Option<String>` → `Option<i64>` → `Option<f64>` per column. On the Postgres driver template (strictly typed decode in sqlx: BOOLEAN, BYTEA, TIMESTAMP/DATE, NUMERIC do not decode as any of those three), such a column falls to the fallback: `row_to_json` yields `JsonVal::Null`, `row_to_map` yields `""`. The runtime itself can produce such columns: `SqlParam::Bytes` (`SqlBytes`, `db.rs:1760`) is bindable but there is no `Db.Decode.bytes` — a written BLOB/BYTEA is unreadable through the decode surface.
- problem: a present, non-NULL column value is reported as NULL/empty with no error. Consequences compose badly with the decoders: on Postgres a `BOOLEAN true` cell makes `Db.Decode.bool` fail with the misleading "expected Bool, got NULL", and `Db.Decode.nullable (Db.Decode.bool …)` returns `Ok Nothing` — a real stored value silently read as absent (exactly the "present-but-wrong defaulted to a trusted value" class). The doc comment on `row_to_json` acknowledges the fallback but nothing surfaces it to the caller; NULL and "unreadable" are indistinguishable.
- fix direction: make the fallback arm an explicit decode error (or extend the probe chain with `bool`/`Vec<u8>`/time types per driver) instead of collapsing to Null/"".
- prior: new (adjacent to the prior audit's NULL-preservation work, which fixed NULL-vs-empty but left the unreadable-type arm).

## RT-DATA-002 · Decoder `run`/`fields` invariant unenforced (pub fields, combinators drop metadata)
- severity: low
- axis: soundness
- principle: "Make invalid states unrepresentable" — the fields-mirror-run invariant is asserted in comments, not by types
- location: `src/runtime/rust/src/json.rs:21-30` (`pub run`, `pub fields`, unvalidating `Decoder::new`); concrete in-tree drift: `decode_list` (`json.rs:510` — `fields: vec![]` discards element-decoder fields), `decode_one_of` (`json.rs:766` — drops all branch fields), `decode_at` (`json.rs:471` — propagates inner fields instead of the path head); consumer: `db_decode_nullable`'s NULL gate (`src/runtime/rust/src/db.rs:469-500`)
- reachability: `db_decode_nullable` gates NULL detection on `inner.fields`. A drifted `fields` silently flips its behaviour (Err instead of `Nothing`, or vice versa). Today the typed `Db.Decode` surface (string/int/float/bool/nullable/map/andThen/succeed/fail/map2-4/required/optional/money — `kernels/src/lib.rs:2066-2080`) only composes field-preserving combinators under `nullable`, so no current Ipê program hits the drifted decoders through it — hence low, a structural smell, not a live bug.
- problem: the struct is openly constructible with any `(run, fields)` pair, and three shipped combinators already violate the documented invariant; the first future addition of `oneOf`/`list` to the `Db.Decode` kernel surface (or any direct runtime composition) turns the drift into a silent NULL-gating bug with no compile-time or test-time signal.
- fix direction: privatise `run`/`fields` behind accessor + smart constructors (or derive `fields` structurally in one place) so a fields/run mismatch is unrepresentable.
- prior: runtime-audit-verdict.md issue (3) "Decoder's pub run/fields can drift, silently mis-gating db_decode_nullable NULL detection" — still present, unchanged.

## RT-DATA-003 · `db_find_by_conditions` with empty conditions is a fail-open full-table SELECT
- severity: low
- axis: security
- principle: P1 — on untrusted input the safe outcome is the only reachable outcome (same class as the fixed unscoped-UPDATE)
- location: `src/runtime/rust/src/db.rs:1475-1484` (`if keys.is_empty() { format!("SELECT * FROM {}", …) }`)
- reachability: `Db.findByConditions conn table dict` where the conditions `Dict` is built from request-derived filters; when the derived list comes back empty (the exact scenario the `db_update_fields` fix at `db.rs:2295-2305` cites), the query silently returns EVERY row of the table instead of nothing/erroring — in a multi-tenant app, a cross-tenant read.
- problem: asymmetric hardening within the same module: an empty WHERE-set on UPDATE fails closed, but on SELECT fails open to a full-table scan. Read exposure is less destructive than a mass-write but is the same defect class (unscoped statement from an attacker-emptiable condition set).
- fix direction: refuse an empty conditions set (mirror the `db_update_fields` fail-closed error), documenting the divergence if Go returns all rows.
- prior: new (the prior audit's item 7 covered only the UPDATE side).

## RT-DATA-004 · YAML expansion-bomb guard's second leg is an untested claim about a deprecated dependency
- severity: low
- axis: completeness
- principle: P1 no unbounded resource / an invariant asserted in comments but not enforced
- location: `src/runtime/rust/src/config_decode.rs:84-97` (comment: serde_yaml's built-in repetition limit "(verified)"), `src/runtime/rust/Cargo.toml:25` (`serde_yaml = "0.9"`, an archived/unmaintained crate)
- reachability: `Config.decodeYaml` / `Config.loadFromFile` on attacker-supplied YAML. The 4 MiB source cap bounds raw input, but the anti-expansion property (a small input cannot expand exponentially) rests entirely on serde_yaml 0.9's internal alias-repetition limit — asserted "(verified)" in a comment, with no in-repo regression test pinning it (no bomb fixture anywhere under `src/runtime/rust/`).
- problem: the load-bearing half of the billion-laughs defence is an unpinned property of a deprecated dependency; a future crate swap (likely, given serde_yaml's archived status) or version change silently drops it, and nothing in the test suite fails. Within the 4 MiB cap an alias-heavy document could still expand to a large multiple of the source size even with the upstream limit — the actual bound is untested.
- fix direction: add a small anchor/alias-bomb fixture test asserting `Err` (pins the property against any future YAML-crate swap); note the serde_yaml succession plan.
- prior: runtime-audit-verdict.md MEDIUM `config_decode_yaml` billion-laughs — substantially fixed (source cap + upstream limit); this files precisely what remains.
