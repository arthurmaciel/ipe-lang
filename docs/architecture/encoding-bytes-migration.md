# Encoding / Bytes migration — authoritative spec

Status: APPROVED (guardian-synthesised). Task #55. Not yet implemented.
Verified against HEAD 2026-07-03 (encoding.rs, jwt.rs, email.rs, compression.rs,
ws_client.rs, server.rs, crates/sky_types/src/constrain.rs).

> **SCOPE CORRECTION (2026-07-03, gap-1 enumeration before impl).** The panel
> reasoned partly against the UPSTREAM `sky-stdlib/` tree. In the skyc PORT
> (`crates/skyc/stdlib/`) there is **no `Compression.sky`, `Email.sky`, or
> `WebSocket.sky`** — those modules are NOT ported yet, so they have no Sky-facing
> binary surface to re-type. The swarm's "atomic step-3" (re-type
> Compression/Email/WebSocket binary payloads `String→Bytes`) is therefore MOOT
> in this tree, and `sky_bytes`/`bytes_to_sky` CANNOT be deleted — the
> runtime-internal `compression.rs`/`email.rs`/`ws_client.rs` still consume them
> and are the only consumers. Per the spec's own milestone-delay guard ("if step 3
> can't land atomically → DELAY"), #55 SPLITS:
>
> - **#55a (lands now — reachable surface, exit-0 critical path):** (1) fix the
>   ENCODE truncation — `base64_encode` + `encoding_hex_encode` use `s.as_bytes()`
>   (UTF-8, Go parity) instead of `sky_bytes` (`url_encode` already UTF-8 via
>   `utf8_percent_encode`); this also fixes the one Sky-adjacent security caller,
>   `server.rs:1210` HTTP Basic-auth. (2) SCHEME the 6 Encoding kernels in
>   `constrain.rs` (String→String encoders; String→Result Error String decoders),
>   moving Encoding off the `Ty::Var(u32::MAX)` fallback → advances the exit-0
>   seal. (3) flip the `m4f_encoding_nonascii_divergence` golden from the
>   Latin-1 bug-recording (`café → Y2Fm6Q==`) to Go-parity (`Y2Fmw6k=`); ASCII
>   goldens stay byte-identical. Add a non-ASCII red-discovery golden proving the
>   truncation fails pre-fix. Decoder Latin-1→fallible (Shape A D5) is OPTIONAL in
>   55a — keep current behavior if the golden set stays green; the type (`Result
>   Error String`) is unchanged either way, so scheming is orthogonal.
> - **#55b (deferred until Compression/Email/WebSocket are ported as Sky modules):**
>   delete `sky_bytes`/`bytes_to_sky`, migrate the byte pipelines to the real
>   `Bytes`(`Vec<u8>`) newtype, make decoders fallible, add the security
>   non-collision + decode-back-compat goldens. Tracked as a follow-up; NOT a
>   workaround — the reachable truncation IS closed by 55a; 55b only migrates
>   currently-unreachable runtime-internal plumbing once its Sky surfaces exist.
>
> Everything below is the full Shape-A design; read it as the 55a+55b union.

Principles order in force: security > correctness > soundness > efficiency >
completeness > readability. Two rules: (1) parse, don't validate; (2) make
invalid states unrepresentable.

---

## 0. Problem (one paragraph)

`encoding.rs:38 sky_bytes(s) = s.chars().map(|c| c as u8).collect()` silently
truncates any codepoint > 0xFF (`€` U+20AC → `0xAC`). It is called by
`base64_encode` (:88) and `encoding_hex_encode` (:123). Go computes
`[]byte(goString)` = UTF-8, so `base64Encode("€")` is Go `4oKs` (E2 82 AC) but
Rust `rOk=`… no — Rust `AC` → `rA==`. This is a live correctness + Go-parity
bug AND, for HTTP Basic auth (server.rs:1210), a security bug (two distinct
> 0xFF passwords collide to the same truncated bytes; non-ASCII credentials
fail against every RFC 7617 client). `url_encode`/`url_decode` are ALREADY
UTF-8 (`utf8_percent_encode` / `decode_utf8`) — they are the internal precedent
for the correct model; `base64`/`hex` are the outliers being aligned.

---

## 1. Locked decisions (each with a one-line rationale)

- **D1 — Chosen shape: A, ATOMIC.** `Encoding.*` text kernels adopt UTF-8
  (Go parity); the byte pipelines (compression / email-attachment / WebSocket
  binary) move to the existing `Bytes(Vec<u8>)` newtype (bytes.rs, M4e);
  `sky_bytes` and `bytes_to_sky` are DELETED. *Rationale: Rust `String` cannot
  faithfully model Go's "string = arbitrary bytes"; the moment the text codec
  is UTF-8, any byte-producer returning a Latin-1 `String` disagrees with the
  encoder on what a `String` means, so `textEncoder(byteProducer(x))` corrupts
  — only a distinct byte type removes the disagreement by construction.*

- **D2 — Shape B (keep Latin-1, `sky_bytes` fail-closed on > 0xFF) is
  ELIMINATED as the primary plan.** *Rationale: it neither converges to Go nor
  removes the divergence, breaks the `String -> String` Sky sig, and — fatally
  — guards only > 0xFF while the corrupting range is 0x80–0xFF where Latin-1 and
  UTF-8 BOTH succeed and DISAGREE (e.g. `gzip("café")` → Latin-1 `…66 E9` vs Go
  UTF-8 `…66 C3 A9`, silent, no error fires). Admissible ONLY as an explicitly
  worse fallback — see §7.*

- **D3 — Shape C collapses into A.** *Rationale: Sky has no overloading/HKT
  (Limitation #1); the "Bytes-taking overload" IS the existing `Std.Bytes`
  module. No new overloaded Encoding kernels.*

- **D4 — `Encoding.*Encode` take UTF-8 text via `s.as_bytes()`.** *Rationale:
  total and widening (non-ASCII → multi-byte); truncation is structurally
  impossible, and for U+0000..U+007F `as_bytes()[i] == (c as u8)` exactly, so
  all ASCII goldens stay byte-identical (NO-GO(b) preserved).*

- **D5 — `Encoding.*Decode` return UTF-8-validated `String` via
  `String::from_utf8(bytes)`, `Err` on invalid.** *Rationale: forced by the
  round-trip identity — with encode = `base64(as_bytes s)`, only
  decode = `from_utf8` makes `decode ∘ encode = id` for every String; a Latin-1
  decode breaks it for any byte ≥ 0x80. Mirrors the already-shipped
  `url_decode`. Byte-exact decode lives on `Bytes.from{Base64,Hex}`.*

- **D6 — Locked identity:** `Encoding.base64Encode s == Bytes.toBase64
  (Bytes.fromString s)` and the hex analogue, for ALL text `s`. *Rationale:
  makes `Bytes.fromString` (= `as_bytes`, bytes.rs:40) the canonical byte path
  and pins encoder/decoder representations into agreement; golden-locked (§5).*

- **D7 — Compose hazard becomes a TYPE ERROR, not a runtime check.**
  Post-migration `Compression.gzip : Bytes -> Result Error Bytes`, so
  `Encoding.base64Encode (gzip x)` fails HM (String ≠ Bytes), forcing
  `Bytes.toBase64 (gzip x)`. *Rationale: this is the actual mechanism that makes
  the invalid state unrepresentable (Rule 2) — no guard can do it.*

- **D8 — Stale JWT comment DELETED.** `encoding.rs:28-29,129` claim the Latin-1
  convention exists for the JWT signature path. VERIFIED FALSE: jwt.rs is
  self-contained (`URL_SAFE_NO_PAD.encode(bytes)` :46, `URL_SAFE_NO_PAD.decode`
  :91, own `hex::decode(&mac_hex)` :136); it never calls `encoding.rs`.
  *Rationale: a comment asserting a now-false dependency is a transparency
  violation; leaving it invites a future reader to reintroduce the coupling.*
  Pre-land gate: grep-assert no non-jwt runtime module routes signature bytes
  through `encoding.rs` before deleting.

- **D9 — Convergence, not narrowing.** Target is Go byte-parity for `*Encode`;
  Latin-1 is the divergence being removed. The `nonascii_divergence` golden
  re-baselines to true Go bytes and `sanctioned.divergence` retires.

- **D10 — Encoding is SCHEMED (FIRST_SCHEMED) in the SAME change as the runtime
  soundness fix; never before it.** *Rationale: scheming a still-truncating
  kernel would seal exit-0 over an unsound kernel (NO-GO(c) spirit). Landing
  order is fixed in §6.*

- **D11 — Phase-E fallback deletion stays DEFERRED.** `KNOWN_UNBACKED = {PubSub}`
  (constrain.rs:5818/5820) still holds `Ty::Var(u32::MAX)` open. *Rationale:
  this milestone NARROWS the exit-0 seal (Encoding closed), it does not
  complete it; the `Ty::Var(u32::MAX) → None` flip waits on PubSub.*

---

## 2. Per-caller classification

TEXT = wants Go UTF-8 bytes-of-string. BYTE = wants raw bytes (moves to `Bytes`).

| Caller | file:line | Class | Post-migration path | Consequence of mislabel |
|---|---|---|---|---|
| HTTP Basic auth | server.rs:1210 | **TEXT** | `base64_encode` UTF-8 (`as_bytes`) | **Security.** > 0xFF password truncates via `c as u8` → distinct passwords collide (auth confusion) + non-ASCII creds fail vs RFC 7617. Needs collision regression test (§5.4). |
| Email attach (Resend) | email.rs:228 | **BYTE** | `content : Bytes` → `Bytes.toBase64` | If left on now-UTF-8 `base64_encode`, Latin-1 byte 0xE9 double-encodes to C3 A9 → corrupt attachment (comment email.rs:224 documents this exact dependency). |
| Email attach (SendGrid) | email.rs:300 | **BYTE** | `Bytes.toBase64` | same |
| Email attach (SMTP) | email.rs:593 | **BYTE** | `Bytes` slice | shares content rep with 228/300 |
| gzip/gunzip/zstd | compression.rs:65,74,85,96 | **BYTE** | `gzip : Bytes -> Result Error Bytes` (+ internal audit :174,:206) | truncation corrupts compressed stream (unrecoverable); 0x80–0xFF divergence if kept String |
| WS binary out (server) | server.rs:1011 | **BYTE** | `Bytes` slice | truncated frame → protocol corruption |
| WS binary in (server) | server.rs:827 | **BYTE** | Sub-event payload = `Bytes` | Latin-1 misread; `base64Encode` on frame composes-corrupts |
| WS binary out (client) | ws_client.rs:479 | **BYTE** | `Bytes` slice | same as server-out |
| WS binary in (client) | ws_client.rs:372 | **BYTE** | Message payload = `Bytes` | same as server-in |
| User `Encoding.*Encode` on text | user Sky | **TEXT** (primary class served) | UTF-8 / Go-parity | `café → Y2Fmw6k=`: a parity FIX, acceptable behavior change |
| User `base64(hexDecode hmac)` | user Sky | **BYTE** | route to `Bytes.fromHex` + `Bytes.toBase64` | `Encoding.hexDecode` now `Err` on non-UTF-8 → typed redirect, never silent gibberish |

WS binary payload MUST be `Bytes` end-to-end (in + out); the inbound Sub-event
payload type in the Sky `WebSocket` surface changes `String → Bytes`, else a
user calling `base64Encode` on a received frame composes-corrupts.

---

## 3. Runtime changes (exact)

1. `encoding.rs`: `base64_encode`/`encoding_hex_encode` use `s.as_bytes()`
   (delete `sky_bytes`). `base64_decode`/`encoding_hex_decode` return
   `String::from_utf8(bytes).map_err(|_| Err…)` (delete `bytes_to_sky`).
   `url_encode`/`url_decode` UNCHANGED (already UTF-8). Delete the Latin-1
   convention comment (:20-34) and the stale JWT lines (:28-29,129).
2. `compression.rs`: signatures `Bytes -> Result Error Bytes`; drop the
   `use super::encoding::{bytes_to_sky, sky_bytes}` (compression.rs:16);
   operate on `&Vec<u8>` directly.
3. `email.rs`: `Attachment.content : Bytes`; `base64_encode(a.content)` →
   `Bytes.toBase64(&a.content)` at :228/:300; SMTP :593 uses the byte slice.
4. `ws_client.rs` / `server.rs`: binary in/out carry `Vec<u8>`; delete the four
   `sky_bytes`/`bytes_to_sky` call sites (server:827,1011; ws_client:372,479).
5. `bytes.rs`: canonical byte path (`fromString`=as_bytes :40, `toBase64` :85,
   `toHex`, `fromBase64`, `fromHex`). No new API needed; confirm `fromHex`
   exists for the `base64(hexDecode)` redirect.
6. Non-jwt caller audit: grep `base64_decode`/`encoding_hex_decode` for any
   non-test Rust caller relying on non-UTF-8 output (jwt uses its own codec →
   expected clean). Migrate any binary-payload reliance to `Bytes.from*`.

No total Latin-1 `bytes → String` path survives: byte sinks carry `Vec<u8>`;
Encoding decoders are fallible `from_utf8`. `sky_bytes` and `bytes_to_sky` are
both deleted.

---

## 4. constrain.rs — exact Encoding scheme entries

Add all six to `stdlib_scheme` and to the `FIRST_SCHEMED` partition
(constrain.rs:5663). Remove Encoding from the exclusion prose at :2920-2924 and
:5660-5662. Arity == arrow-count == 1. `Error = IrType::Str` (SkyError).

| Kernel | `stdlib_scheme` Ty |
|---|---|
| `Encoding.base64Encode` | `fun(Str, Str)` |
| `Encoding.urlEncode` | `fun(Str, Str)` |
| `Encoding.hexEncode` | `fun(Str, Str)` |
| `Encoding.base64Decode` | `fun(Str, Result(Str, Str))` |
| `Encoding.urlDecode` | `fun(Str, Result(Str, Str))` |
| `Encoding.hexDecode` | `fun(Str, Result(Str, Str))` |

Decoders carry the `Result` wrap matching runtime `SkyResult<String,String>`
(encoding.rs `base64_decode` etc.). Partition = **FIRST_SCHEMED** (runtime IS
backed → genuine hole, was `Ty::Var(u32::MAX)`), NOT `KNOWN_UNBACKED`. The
`first_schemed_were_holes` gate (constrain.rs:5932) will assert the
`Ty::Var(u32::MAX)` pre-state for all six — CONFIRM all six currently fall
through to `_ => Ty::Var(u32::MAX)` (:5147) before partitioning (url being
UTF-8 in the RUNTIME does not imply url* is already SCHEMED in constrain).
After this, `KNOWN_UNBACKED = {PubSubPublish, PubSubPublishNoEcho}` remains the
sole family on the fallback (D11).

---

## 5. Golden fixtures

New (lock parity + truncation-impossibility):

1. `m4f_encoding_utf8_euro` — `base64Encode "€"` → **Go** `4oKs` (E2 82 AC),
   `hexEncode "€"` → `e282ac`. FAILS today (Latin-1 → `AC`). Discovery artifact.
2. `m4f_encoding_utf8_emoji` — emoji/CJK (e.g. `"🔒"` / `"日本"`) UTF-8 bytes.
3. `m4f_encoding_identity_bytes` — locks D6:
   `Encoding.base64Encode s == Bytes.toBase64 (Bytes.fromString s)` + hex, non-ASCII `s`.
4. `m4f_encoding_decode_nonutf8` — `Encoding.base64Decode "/w=="` (0xFF) asserts
   **`Err`**; `Bytes.fromBase64 "/w=="` asserts **Ok** (byte-exact path).
   Records the sanctioned decoder divergence.
5. `m4f_encoding_compose_typeerror` — compile-fail:
   `Encoding.base64Encode (Compression.gzip x)` no longer type-checks
   (String ≠ Bytes); paired positive `gunzip (Bytes.fromBase64 (Bytes.toBase64
   (gzip x)))` round-trip. Locks D7 at the type level.

Flip / refresh:

6. `m4f_encoding_nonascii_divergence` → `expected_go.txt` = `café Y2Fmw6k=
   636166c3a9`; DELETE `sanctioned.divergence`; set `oracle_divergence=false`;
   rewrite `Main.sky`; regenerate `main_sky_sha256` + oracle.
7. Anti-mask: add non-ASCII companions to `encoding_base64`/`hex` (currently
   ASCII-only — the exact golden-mask that hid this). Add a café url case to
   `encoding_url*` to lock the already-correct UTF-8 url path.

Untouched + documented:

8. JWT goldens (`jwt_hs256_bytes`, #62) stay byte-identical. Add a one-line
   assert/comment in jwt.rs + the golden recording "JWT does not route through
   Encoding.*" so no future reader reintroduces the stale dependency.

Byte-pipeline regressions (§5.4):

9. compression round-trip of `Bytes` with 0xFF; email attachment base64 of
   high-byte `Bytes`; WS binary echo of high-byte `Bytes` — each lossless.
10. **Security:** non-ASCII password → UTF-8 base64 matching a browser fixture;
    two distinct > 0xFF passwords MUST NOT produce equal bytes (no collision).

---

## 6. Migration checklist (ordered — landing order is a soundness constraint)

Single atomic change (D1, D7, D10). Never land the `*Encode` UTF-8 flip while
any byte-producer still returns/consumes a Latin-1 `String`.

1. Runtime: encoders → `as_bytes`; decoders → `from_utf8`-fallible; migrate
   compression/email/ws to `Bytes`; delete `sky_bytes` + `bytes_to_sky`; delete
   Latin-1 + stale-JWT comments. (§3)
2. Non-jwt decoder-caller audit grep (§3.6); migrate any binary reliance to
   `Bytes.from*`.
3. Re-type the Sky surfaces: `Std.Compression`, `Std.Email` (`Attachment.content`),
   `Sky.Core.WebSocket` binary in/out → `Bytes`; update their constrain schemes
   + lowering.
4. Flip/refresh goldens (§5.6, §5.7); add new goldens (§5.1-5.5); regenerate
   oracle shas.
5. Scheme Encoding in `stdlib_scheme` + `FIRST_SCHEMED`; remove exclusion prose;
   confirm `first_schemed_were_holes` pre-state for all six. (§4)
6. Add byte-pipeline + security regression tests (§5.9, §5.10).
7. Verify: `first_schemed_were_holes`, the exhaustiveness gate
   (RELOCATED ∪ FIRST_SCHEMED), clippy-D, golden sweep, targeted E2E.
8. Phase-E `Ty::Var(u32::MAX) → None` flip: DO NOT land (PubSub open, D11).

If step 3's `Bytes` re-typing genuinely cannot land atomically (lowering gap),
DELAY the whole milestone — a half-shipped `*Encode` flip is silent corruption
(NO-GO(a)) and is strictly worse than delaying the parity fix. Under the
principles order, delay beats corruption.

---

## 7. Sanctioned divergence records

- **RETIRED:** Latin-1 char-as-byte for `Encoding.*` text (the `café/base64`
  divergence). Removed with golden #6.
- **NEW (strictly safer):** `Encoding.{base64,hex}Decode` validate UTF-8 and
  return `Err` on non-text payloads; byte-exact decode routes through
  `Std.Bytes.from{Base64,Hex}` (`Vec<u8>`). Rationale: Rust `String` UTF-8
  invariant + parse-don't-validate (refuse to fabricate text from bytes). Go
  returns raw bytes in a string; Rust routes to the typed `Bytes` API.
  `url_decode` already behaves this way — this only aligns base64/hex. ASCII /
  all-text output is byte-identical to Go. Record in
  `docs/divergences-from-{sky,go}.md`.
- **REINFORCED (pre-existing, M4e):** `Std.Bytes = Vec<u8>` (Go: `Bytes =
  String`) is now the sole home of byte-exact encoding; the byte-pipeline
  surface change (`String → Bytes`) falls under this already-recorded
  divergence — the bytes themselves REGAIN Go parity.

- **FALLBACK ONLY (explicitly worse — not the plan):** if `Bytes` re-typing is
  proven infeasible atomically, the String + fail-closed `latin1_bytes_checked`
  (> 0xFF → `Err`) bridge MAY ship, but ONLY if the residual 0x80–0xFF
  Latin-1-vs-UTF-8 byte divergence in compression/email/ws is RECORDED as
  sanctioned WITH goldens pinning it, and the `Bytes` migration filed as a
  committed (not open-ended) follow-up. This is validate-not-parse (Rule 2
  smell: > 0xFF stays representable) and trades a recorded surface divergence
  for an unrecorded silent byte divergence — net loss. Do the atomic migration.

---

## 8. NO-GO audit (all cleared under the atomic plan)

- **(a) silent truncation** — CLEARED: `sky_bytes`/`bytes_to_sky` deleted;
  text→`as_bytes` (widening, total); bytes→`Vec<u8>` (total); decode→`from_utf8`
  (fallible); compose hazard is a type error (D7). No residual `String → byte`
  reinterpretation whose input isn't provably ≤ 0xFF or fallible.
- **(b) JWT/base64/hex byte-parity** — CLEARED: ASCII byte-identical
  (`as_bytes[i] == c as u8` for U+00..7F); JWT orthogonal (own codec, D8);
  non-ASCII flips to true Go bytes.
- **(c) Encoding schemed & sound** — CLEARED: FIRST_SCHEMED in the same change
  as the soundness fix (D10); Phase-E deferred to PubSub (D11).

Files touched: `runtime/src/sky_runtime/{encoding,bytes,jwt,compression,email,
ws_client,server}.rs`; Sky surfaces `Std/{Compression,Email}`,
`Sky/Core/WebSocket` (+ constrain schemes/lowering); `crates/sky_types/src/
constrain.rs` (FIRST_SCHEMED :5663, stdlib_scheme arms, exclusion notes
:2920/:5660, Phase-E :1503 left in place); `tests/golden/m4f_encoding_*`,
`jwt_hs256_bytes`.
