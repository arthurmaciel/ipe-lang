# 2026-07-09 hardening campaign — item classification

> Produced by a Fable-model classifier agent from `BACKLOG.md`
> (in-scope: Sweep front, Tier-1, Security tier, AUD-08/09, SEAL follow-ups,
> Rename, Non-blocking hardening) cross-referenced against
> `docs/architecture/prior-art-runtime-rust-2026-07-09.md`. AUD-03 (landed) and
> AUD-13 (separately owned, in progress) excluded. Tier-2/Tier-3 out of scope.
>
> This doc feeds spec-writing: each class below gets either a direct
> implementation spec (MECHANICAL) or a brainstorm→design→spec pipeline
> (GUARDIAN-DESIGN-REQUIRED) before any sonnet-5 implementation lane starts.

## Classes

1. **Type-system inference & soundness oracles** — GUARDIAN-DESIGN-REQUIRED.
   Inference cluster #2 (blocks Tier-1 sweep-green), ex27 SKY-L0102, AUD-09's
   Bug-29 any-return-matches-any-Con, #56, #66-T2, #66-N second half.
2. **Tier-1 sweep/CI/push infrastructure** — MECHANICAL. #35, #110, #37.
3. **Kernel-registry & emitted-name integrity** — MECHANICAL. AUD-08, #45,
   #70, #71, AUD-09's `Match::from_parts_unchecked` pub.
4. **Pattern & lowering completeness bugs** — MECHANICAL (diagnosis-first).
   SKY-I0001 interp ICE, #90, #158, #102, #32.
5. **Emitter clone/borrow discipline + typed-token backend** — split:
   #99/#125/#142/AUD-09's O(n²) clone are MECHANICAL; #53 is
   GUARDIAN-DESIGN-REQUIRED (whole-backend rewrite), scheduled last.
6. **Typed security primitives** — GUARDIAN-DESIGN-REQUIRED. #44 (opaque
   `Secret`), #61 (`SqlFragment` newtype).
7. **SQL/DB runtime correctness & security** — MECHANICAL. `url_is_cacheable`
   DoS, `SqlNull`/Postgres, Postgres reachability, `db_insert_row` fabricated
   id, tenant-prefix gap in `db.rs`, #34 (incl. `db_decode_money` wiring).
8. **Live/HTTP web security** — MECHANICAL. #63 (CSRF port), cookie
   Secure-vs-TLS, observability-ingest CSRF exemption, WebSocket CSWSH,
   `live_max_body_bytes` floor, #33.
9. **Runtime kernel robustness + stdlib surface completeness** — MECHANICAL.
   `Io.readLine`, gunzip multistream, `File.readFileLimit` TOCTOU, time
   lossy casts, i64::MIN divergences, #129, #122, #157.
10. **UI/HTML rendering + event sinks** — MECHANICAL (one bounded
    either/or inside #156). `escape_text` quote gap, #113, #105, #109, #156.
11. **Rename + documentation accuracy** — MECHANICAL, but #59 runs strictly
    solo (touches every file). #75, #59, #159.

Unclassified meta-item: #31 (make-invalid-states-unrepresentable) is a lens,
not a work item — apply it (plus Part 2 §4's "real-type check, never
usage-heuristic" lens) across every class's spec rather than speccing it
standalone.

## Recommended processing order

1. Class 1 + Class 6 first, in parallel (guardian design, disjoint files).
   Class 1 blocks Tier-1's entire sweep-green→seal→#110→#37→#59→push chain.
2. Mechanical wave, parallel-safe: Class 7, 8, 9, 2, and Class 11's #75+#159.
3. Sequential (shared-file conflict risk): Class 3 then Class 4 (both touch
   `sky_kernels`/`constrain.rs`/`lower.rs`, and Class 4 overlaps Class 1's fix
   surface — run after Class 1 lands). Class 5's mechanical items and Class
   10's #156 both touch `emit_expr.rs` — sequence relative to each other.
4. #53 (Class 5's guardian tail) schedules last among code work — every
   other `emit_expr.rs` change must land first; non-blocking for push.
5. Class 11's #59 dead last, strictly solo — touches every file, nothing
   concurrent with it. Push per #37 follows.
