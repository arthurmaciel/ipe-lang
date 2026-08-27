# Bounded revocation store design

## Problem

The runtime revocation store is the session-layer fail-closed gate that keeps a
revoked principal or session denied for as long as its token could still be
live. It answers one boolean question — is this `subject`/`jti` revoked? — from
two in-memory sets. Those sets grow with every revoke and never shrink except
by an explicit restore. They carry no per-entry expiry and no size ceiling.

A growable buffer whose size a (necessarily authenticated) party dictates,
without a declared limit, violates the **bounded-by-construction** clause of the
soundness principle: every growable buffer in the emitted runtime must have a
declared ceiling. A hostile or compromised admin — the only party who can insert,
since a write requires a minted `Principal` — can grow the store without limit
and exhaust process memory. That is a denial-of-service vector even though it is
NOT a dropped-revocation hole: today every insert succeeds, so the
security-critical direction (a revoked principal stays denied) already holds.

The fix must add a ceiling **without** ever converting the DoS into the far
worse failure of silently re-admitting a revoked principal. That tension — a
bound that must never evict a live revocation — is the whole of this design.

## The collections, enumerated

Source: `src/runtime/rust/src/revocation.rs`.

| # | Collection | Holds | Growth driver | Lifetime / eviction today |
|---|-----------|-------|---------------|---------------------------|
| 1 | `RevocationStore.subjects: HashSet<String>` (line 43) | Revoked subjects — every session of that user is denied | One entry per `revokeUser` call (per user) | None. Removed only by an explicit `restoreUser`. No expiry, no cap. |
| 2 | `RevocationStore.sessions: HashSet<String>` (line 45) | Revoked session ids (`jti`) — one specific session denied | One entry per `revokeSession` call (per session) | None. Never removed by any path (there is no session-restore). No expiry, no cap. |

Both live behind a single `Mutex<RevocationStore>` in a process-global
`OnceLock` (`store()`, lines 57–60). Reads (`is_revoked`, line 77) and writes
(`revoke_subject` line 89, `revoke_session` line 98, `restore_subject` line 110)
all take that one lock; the critical sections are pure in-memory set operations
with no `.await` inside the guard.

### There is no second unbounded store

The sliding-session mechanism is **stateless**. `auth_reissue_token`
(`auth.rs` line 395) mints a fresh signed JWT whose `iat`, `cap`, and `jti` are
carried verbatim from the verified prior token; there is no server-side
active-session table. The `HashMap<String, String>` values in `auth.rs` are
per-call claim maps — transient locals, freed when the request returns, never
retained. So the revocation store's two sets are the only unbounded collections
in the auth surface. This design touches nothing else.

## The crux security decision

**A revocation store must never silently drop a live revocation.** If it did,
the dropped `subject`/`jti` would return `Verdict::Active` at the next request
and the revoked token would validate again — a privilege-escalation / auth-bypass.
Therefore the bound **cannot** be a naive LRU or capacity-eviction over
revocations: evicting a revocation is never acceptable.

This is the opposite of a cache. In a cache, eviction costs a recompute. Here,
eviction costs an authorization bypass. The two directions of failure are not
symmetric, so the two collections in this design are treated by the same
asymmetric rule:

- **Revocation entries (both sets) — eviction is FORBIDDEN except by proof of
  redundancy.** The only entry that may be removed is one that can no longer
  change any verdict: a revocation for a token whose absolute lifetime cap has
  already passed. Such a token is denied by the JWT `cap` gate
  (`auth_verify_token`, `auth.rs` lines 260–312) regardless of the revocation
  set, so dropping its revocation changes no verdict. This is redundancy-driven
  reclamation, not capacity eviction: it removes only entries that are provably
  dead, and it is the *sole* form of removal the store performs on its own.

For contrast — and to state explicitly what this design does **not** apply the
forbidden-eviction rule to:

- **A hypothetical active-session store** (were one ever introduced) MAY evict
  its oldest/idle entry under capacity pressure, because evicting an active
  session merely forces the user to re-authenticate — an availability cost, not
  a security bypass. That is capacity eviction and it is safe there precisely
  because it is unsafe here. This project has no such store today (sessions are
  stateless JWTs); the contrast is recorded so a future active-session store is
  designed with the correct — opposite — eviction policy, and never by copying
  the revocation store's forbidden-eviction rule where it does not belong.

## Bounded semantics

### Entries gain an expiry

Each revocation entry becomes a pair: the id string and the absolute Unix-second
timestamp past which the underlying token can no longer be valid. That timestamp
is the token's `cap` claim (`iat + AuthMaxLifetime`), the same value the JWT
absolute-lifetime gate enforces. Once `now >= expiry`, the entry is redundant
(the `cap` gate denies the token anyway) and is reclaimable.

- **Session revocation** (`jti`): the `cap` is known exactly — it is a claim of
  the very token being revoked. The `revokeSession` kernel must thread it in.
- **Subject revocation**: a subject revocation must outlive *every* currently
  live session of that subject. The safe expiry is `now + AuthMaxLifetime`
  (the longest any token minted now could remain valid). Any session that
  existed when the subject was revoked has `cap <= now + AuthMaxLifetime`, so
  this bound covers them all. A subject re-revoked later refreshes the expiry to
  the later `now + AuthMaxLifetime` (take the max), never shortening it.

Making entries carry expiry is what turns "unbounded and unsweepable" into
"bounded with a principled reclamation rule". Without it there is no proof of
redundancy and thus no safe removal — only the forbidden capacity eviction.

### The typed ceiling

A declared per-set ceiling, `REVOCATION_STORE_CAPACITY`, a `usize` constant with
a config override (`app_config`, resolved once like `resolve_auth_slide_window`).
The store is a struct that cannot exceed its bound by construction: every insert
path goes through one method that checks the count and returns a typed outcome;
there is no public field a caller can push into directly.

**Default: 1,048,576 (2^20) entries per set.** Rationale: at roughly 64 bytes
per entry (a short id `String` plus an `i64` expiry plus map overhead) each set
caps near ~64 MB, ~128 MB for both — a bound a server can hold without strain,
yet far above any plausible legitimate concurrent-revocation count. A deployment
with more than a million *simultaneously live* revoked sessions is past the point
where per-session revocation is the right tool; the correct response there is
signing-key rotation (see fail-closed escalation), which the ceiling nudges the
operator toward rather than letting memory grow without limit. The default is
config-overridable for deployments with a justified higher (or lower) bound.

### Reclamation sweep — bounded work, off the hot path

Redundant (`now >= expiry`) entries are reclaimed by a sweep. Two rules keep it
bounded and off the per-request path:

1. **The hot path never scans.** `is_revoked` does one lookup per set and
   returns; it must not walk the store. Expired-but-not-yet-swept entries are
   harmless — they still say "revoked", and the `cap` gate independently denies
   the token, so a stale entry can only over-deny (fail-closed), never
   under-deny.
2. **The sweep runs at bounded moments:** lazily on an insert that finds the set
   at capacity (reclaim-then-insert), and optionally on a coarse timer. Each
   sweep does bounded work — one pass whose cost is the current entry count,
   amortized against the inserts that filled the set. No unbounded scan is ever
   triggered by an attacker-controlled request, because the sweep is gated on an
   insert, and inserts require a `Principal`.

### Fail-closed behavior at the ceiling

When an insert finds the set at `REVOCATION_STORE_CAPACITY`, the store first runs
the reclamation sweep. Then:

- **If the sweep freed room**, the insert proceeds normally.
- **If no entry was redundant** (all live), the insert **fails closed**: the
  store returns a typed error — a new revocation-store variant such as
  `RevocationError::AtCapacity` — WITHOUT dropping any existing entry. The kernel
  (`revokeUser`/`revokeSession`) surfaces this as a `Task Error ()` failure so
  the calling app learns the revoke did not take and can react (alert, retry,
  or escalate to signing-key rotation, which invalidates every session at once
  and is the correct blunt instrument when granular revocation is saturated).

This is the fail-closed rule stated precisely: at the ceiling the store denies
the *write* (returns an error), never the *revocation invariant* (never drops a
live entry) — and never deny-all-by-default (a saturated store still answers
`is_revoked` correctly for existing entries; only new inserts are refused).
The three outcomes — inserted, reclaimed-then-inserted, at-capacity-error — are
a closed set encoded in the return type; there is no fourth, silent-drop outcome.

### Concurrency

The store stays behind one `std::sync::Mutex` (not tokio's): all critical
sections are synchronous in-memory operations with no `.await` inside the guard,
so a blocking mutex is correct and cheapest, and the "never hold a lock across
await" rule is satisfied by construction — the guard is dropped before the async
kernel wrapper returns. `is_revoked` keeps returning `Verdict::Unknown` on lock
poison (fail-closed: the middleware denies on `Unknown`). The sweep and the
capacity check run inside the same single critical section as the insert, so
the count and the ceiling are checked atomically — no torn read can let two
concurrent inserts both pass a near-full check and overshoot the bound.

### Defense-in-depth composition

A request is admitted only if it clears three independent gates, each a full
denial on its own:

1. **JWT signature + `exp` + absolute `cap`** (`auth_verify_token`) — a
   tampered, expired, or past-cap token never reaches the revocation gate.
2. **Revocation** (`is_revoked`) — an explicitly revoked live token is denied.
3. **The ceiling + fail-closed insert** — the store cannot grow without bound;
   at saturation new revokes fail loudly rather than silently succeeding into an
   ever-growing heap.

Gate 1 is exactly what makes redundancy-driven reclamation sound: an entry the
sweep drops is one gate 1 already denies, so gates 1 and 2 together still deny
every token that either gate would. Expiry AND revocation AND bound — no single
gate carries the whole weight.

## Approaches

### A — Expiry-indexed sweep over a `HashMap<String, i64>` (recommended)

Each set becomes `HashMap<id, expiry_unix_secs>`. Lookup is `contains_key` (same
O(1) hot path as today). The sweep, run only on an at-capacity insert (and
optionally a coarse timer), does one `retain(|_, &exp| now < exp)` pass.

- **+** Minimal change from the current shape; hot path stays O(1) with no scan.
- **+** Reclamation is provably redundancy-only (`retain` drops solely
  past-cap entries); the forbidden-eviction rule holds by construction.
- **+** The ceiling check is a single `len()` compare in the same critical
  section — trivially atomic, invalid overshoot unrepresentable.
- **−** The at-capacity sweep is O(n) in the set size. Acceptable: it is
  amortized against the n inserts that filled the set, and never triggered by an
  unauthenticated request.

### B — Expiry-ordered `BTreeMap<expiry, HashSet<id>>` with a companion index

Order entries by expiry so the sweep pops only the expired prefix (`range(..now)`),
touching just the entries it reclaims rather than the whole set. A companion
`HashMap<id, expiry>` preserves the O(1) hot-path lookup.

- **+** Sweep cost is proportional to what it reclaims, not to set size.
- **−** Two structures to keep consistent (insert, restore, and sweep must
  update both) — more surface for a consistency bug, and a consistency bug in a
  revocation store is a security bug. Higher complexity buys a sweep-cost win
  that approach A only pays at capacity, rarely.
- **−** More code for a gain that does not bind in practice (the sweep is rare).

### C — Fixed-capacity generational buckets (segmented by expiry window)

Partition entries into a small fixed ring of buckets by coarse expiry window;
drop a whole bucket once its window is wholly past. Reclamation is O(1) per
bucket.

- **+** Cheapest reclamation; naturally bounded bucket count.
- **−** Bucket granularity forces a coarse expiry, over-retaining entries until
  their bucket's window closes — acceptable (over-retention only over-denies)
  but it complicates the capacity accounting, and per-id restore
  (`restoreUser`) must find the id across buckets. Most machinery for the least
  practical benefit here.

### Recommendation

**Approach A.** It is the smallest, clearest change that satisfies every
requirement: O(1) hot path unchanged, redundancy-only reclamation that cannot
drop a live revocation, an atomic ceiling check, and fail-closed-at-capacity —
all in one already-existing critical section. B and C optimize a sweep that runs
only at capacity, a case a well-tuned ceiling makes rare; that optimization is
YAGNI here and each adds a second structure whose desync would be a security
defect. Revisit B only if profiling ever shows the at-capacity sweep is a real
latency source (it will not be on the intended entry counts).

## Implementation checklist (for the build lane)

Files to touch: `src/runtime/rust/src/revocation.rs` (core), `src/runtime/rust/src/app_config.rs` (capacity config + resolver), `src/runtime/rust/src/server.rs` (thread the token `cap` into the session-revoke call site), and the `Ipe.Auth.Revocation` kernel signatures where `revokeSession` gains its expiry input.

Types to add:
- A typed error, e.g. `enum RevocationError { Unavailable, AtCapacity }`, replacing the current `String` error channel of the write kernels. Map it to the kernel `Task Error ()` at the boundary (parse-don't-validate: typed error, not `String`).
- Change each set from `HashSet<String>` to `HashMap<String, i64>` (id → absolute-cap expiry). Keep `Verdict` as-is.
- `REVOCATION_STORE_CAPACITY: usize` constant in `app_config` with a `resolve_revocation_capacity()` resolver (mirror `resolve_auth_slide_window`), default `1 << 20`.

Core behavior:
- One private `insert_bounded(&mut self, set, id, expiry, now) -> Result<(), RevocationError>` that: checks `len() < CAPACITY`; if not, runs `retain(|_, &e| now < e)` and re-checks; inserts on success, returns `AtCapacity` on failure. All inserts route through it — no other write path to the maps. This is the make-invalid-states-unrepresentable point: the maps are private, the only growth path enforces the bound.
- `revoke_session(jti, cap)` and `revoke_subject(subject, expiry = now + AuthMaxLifetime)` call `insert_bounded`. A re-revoke of an existing id takes the max of the old and new expiry (never shorten).
- `is_revoked` unchanged except `contains` → `contains_key`; still no scan, still `Unknown` on poison.
- `restore_subject` unchanged (`remove`).

Tests to add (in the existing `#[cfg(test)] mod tests`, unique ids per test):
1. **Ceiling reached → live revoke still denied.** Fill a set to a small
   test-configured capacity with non-expired entries, attempt one more insert,
   assert it returns `AtCapacity`, then assert every previously inserted id
   still returns `Verdict::Revoked` — the store denied the write, not the
   invariant.
2. **Does-not-drop-a-live-revoke property test** (`proptest`/quickcheck): over
   an arbitrary interleaving of inserts (with future expiries) and one
   at-capacity insert, assert that no id inserted with a future expiry ever
   subsequently returns `Verdict::Active`. The invariant "a live revocation is
   never dropped" must hold across every sequence.
3. **Reclamation frees room for a real insert.** Insert an entry with an
   already-past expiry, fill to capacity, then insert a new entry: assert the
   sweep reclaimed the expired one and the new insert succeeded, and that the
   expired id is now `Active` (correctly, its token is past `cap`) while the new
   id is `Revoked`.
4. **Sweep drops only expired.** Mix past-expiry and future-expiry entries,
   trigger a sweep, assert exactly the past-expiry ids became `Active` and every
   future-expiry id stayed `Revoked`.
5. **Kernel surfaces `AtCapacity` as a `Task Error`.** Drive a `revokeUser`/
   `revokeSession` kernel against a saturated store and assert it returns
   `IpeResult::Err`, so the app learns the revoke did not take.
6. **Subject expiry covers all live sessions.** Assert a subject revoked at
   `now` stays `Revoked` for a `jti` whose `cap` is `now + AuthMaxLifetime`.

Config/doc: update the module doc-comment in `revocation.rs` — it currently
states the store is unbounded; after the change it must describe the ceiling,
the redundancy-driven sweep, and the fail-closed-at-capacity behavior (timeless
prose, no history narration).
