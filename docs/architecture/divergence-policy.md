# Divergence policy — when Sky-Rust output may differ from the Go reference

> Status: IMPLEMENTED (M4b; extended M4c). The oracle crate (`tools/oracle`) +
> the `refresh-oracle` tool encode this policy. M4c added the tagged
> Go-succeeds-but-we-differ marker (`divergence:` / `sanctioned:`); the first
> `divergence:`-tagged divergence is Math.min/max #136 (see registry below).

## Default — byte parity with Go

Sky-Rust's contract (PRINCIPLES.md §2 Correctness) is that for the same
well-typed Sky program and the same input, the Rust output matches the Go
reference's observable behaviour, **ideally byte-for-byte**. The golden parity
suite enforces this: each runnable golden caches the Go reference's clean
program stdout (`expected_go.txt` + `oracle.meta`), and `oracle::check_parity`
diffs skyc's output against it on every run — no live Go in the hot path.

So the DEFAULT for every golden is: skyc must reproduce the Go bytes exactly.
A mismatch is a hard failure.

## The three kinds of recorded divergence

A divergence is **never silent**. PRINCIPLES.md §2 allows a deliberate
divergence only when it is *documented rather than silent*. The oracle
records exactly three kinds, all via `oracle_divergence = true` in `oracle.meta`,
distinguished by the leading TAG on the `divergence_reason`:

### 1. Auto Go-failure divergence (Go FAILS)

The Go oracle panics, exits non-zero, or fails to build on a shape skyc handles
correctly. The `refresh-oracle` tool detects the Go failure, falls back to
skyc's own (correct) output, and records it with a reason such as
`Go oracle failed: …`. This is the original "the Go oracle can fail to produce a
reference on a shape" carve-out. No marker file is needed — the failure itself
triggers the branch.

### 2. `divergence:` kind (Sky's current behaviour differs; we follow a different target)

The Go oracle *succeeds* — it builds and runs — but Sky's current behaviour
differs from what Sky-Rust implements. Because Go does not fail, the auto path
(kind 1) never fires; the golden must opt in explicitly. Sky-Rust records its
own output with a neutral rationale stating what differs and why (e.g.
Elm-conformance, fuller Unicode).

- Drop a `sanctioned.divergence` marker file whose reason begins with the
  `divergence: ` tag (e.g. `divergence: Math.min/max Elm-conformant polymorphic
  comparable. Divergence from Sky, rationale: Elm-conformance`).
- `refresh-oracle` short-circuits straight to skyc's output (it does **not**
  require, or even run, the Go oracle for the expected value) and records it with
  `oracle_divergence = true` and `divergence_reason = divergence: <reason>`.

### 3. `sanctioned:` divergence (Go SUCCEEDS, Sky-Rust is deliberately MORE correct)

The Go oracle succeeds *correctly*, but Sky-Rust is intentionally more correct
still, so the two outputs differ by design (e.g. full-Unicode case mapping).
This is a **deliberate, reviewed** choice — not a bug on either side.

- Drop a `sanctioned.divergence` marker file; its contents are the human-readable
  reason. An untagged reason defaults to the `sanctioned: ` tag (so historical
  markers keep working); you may write `sanctioned: …` explicitly.
- `refresh-oracle` short-circuits straight to skyc's output and records it with
  `oracle_divergence = true` and `divergence_reason = sanctioned: <reason>`.

In all three kinds the staleness gate (`sha256(Main.sky)`) and `check_parity`
apply unchanged — the recorded expectation is still pinned to the source and
diffed exactly. The leading tag (`Go oracle failed:` / `divergence:` /
`sanctioned:`) is what lets the read side, the refresh logs, and a human
reviewer tell the kinds apart without re-running anything. A blank marker is a
hard error: a marker divergence MUST state why.

This keeps the rigour floor intact: a sanctioned divergence is loud, reviewed,
and source-pinned — it can never silently mask a *real* parity bug, because the
expected bytes are Sky-Rust's deliberate output, captured and committed.

## The sanctioned divergences in M4b

M4b ships the `Sky.Core.String` / `Sky.Core.Char` kernels. Char **predicates**
(`Char.isDigit` / `isLower` / `isUpper` / `isAlpha`) are NOT divergences — they
match Go exactly (`unicode.IsDigit`/`Nd`, `IsLower`/`Ll`, `IsUpper`/`Lu`,
`IsLetter`/`L*`); see FIX-3. The only sanctioned divergences are:

1. **Full-Unicode default case mapping** —
   `String.toUpper` / `toLower` / `casefold` and `Char.toUpper` / `toLower`.
   Rust applies the full Unicode `SpecialCasing` rules (e.g. `ß → SS`,
   `İ → i̇`), where Go's `strings`/`unicode` simple per-rune mapping does not.
   Sky-Rust is deliberately more correct here. Reason recorded:
   `sanctioned: full-Unicode default case mapping (Rust SpecialCasing — ß→SS, İ→i̇ — vs Go simple per-rune)`.

2. **`String.toFloat` standard grammar** — a minor sanctioned-stricter
   divergence: Sky-Rust accepts the standard float grammar and *rejects* Go's
   hex-float and underscore-separated literals. Stricter, not looser; recorded as
   sanctioned where it surfaces.

## Recorded `divergence:` entries (Sky's current behaviour differs)

- **Math.min / Math.max — AsInt coercion vs Elm-conformant polymorphic compare
  (Sky PR #136).** Sky's `Math.min`/`Math.max` currently route both arguments
  through `AsInt` before the compare, coercing `Float` to `Int` (`Math.min 0.4
  1.3 → 0`) and yielding a meaningless compare for `String`. Sky-Rust follows
  Elm's `Basics` polymorphic comparable (`min`/`max : a -> a -> a`) preserving
  each argument's type + value. Divergence from Sky, rationale: Elm-conformance.
  Recorded as `divergence: Math.min/max Elm-conformant polymorphic comparable
  (Divergence from Sky, rationale: Elm-conformance)`. (`Math.abs` stays
  `Int -> Int` — the `AsInt` path is correct there and is not a divergence.)

- **`Sky.Core.Bytes` — `Vec<u8>` vs Sky's `type alias Bytes = String` (M4e).**
  Sky/Go defines `type alias Bytes = String`. Go's `string` is an arbitrary byte
  sequence (no UTF-8 constraint), so the alias is cost-free and correct in Go.
  Rust's `String` is UTF-8-constrained: mapping `Bytes → String` would silently
  corrupt non-UTF-8 binary payloads (e.g. image/audio/crypto buffers). Sky-Rust
  makes `Bytes` a DISTINCT primitive lowering to `Vec<u8>`, providing lossless
  handling of arbitrary binary data. String ↔ Bytes conversions are always
  explicit: `Bytes.fromString` UTF-8-encodes; `Bytes.toString` UTF-8-decodes
  returning `Maybe String`. This differs from Sky's surface (where `Bytes` is
  `String`, so no conversion is needed), hence `oracle_divergence = true` for all
  `Sky.Core.Bytes` golden tests. Rationale: Rust type-system correctness — a
  lossless byte buffer is strictly more correct than a transparent alias whose
  semantics only hold in Go. Recorded as `divergence: Bytes is Vec<u8> in
  Sky-Rust; Sky/Go aliases Bytes = String — programs using Sky.Core.Bytes produce
  different output under the Go oracle`.

- **`Encoding.base64Encode` / `Encoding.hexEncode` over non-ASCII text — Latin-1
  char-as-byte vs Go's UTF-8 string bytes (M4f).** Sky's `Encoding.*` operate on
  the `Bytes = String` surface. Go's `string` is an arbitrary UTF-8 byte
  sequence, so Go encodes the UTF-8 bytes of the source text (`hexEncode "café" →
  "636166c3a9"`). Rust's `String` is UTF-8-constrained, so the runtime models the
  String-as-bytes surface with a Latin-1 char-as-byte convention (one codepoint
  U+0000..U+00FF → one byte) — chosen because the binary pipeline (email
  attachments with bytes ≥ 0x80, compression, WebSocket frames, the
  `base64(hexDecode(hmac))` JWT-signature path) must round-trip raw bytes
  losslessly through a Rust `String`. Hence `hexEncode "café" → "636166e9"` in
  Sky-Rust. **ASCII input is byte-identical to Go** (every codepoint < 0x80 maps
  one byte either way); only codepoints ≥ 0x80 diverge. Recorded as `divergence:
  Encoding.base64Encode/hexEncode over non-ASCII text … Latin-1 char-as-byte …`
  (golden `m4f_encoding_nonascii_divergence`). Rationale: Rust String UTF-8
  invariant + lossless binary byte-pipeline.
  **Tracked follow-up (post-M4f, not deferred silently):** migrate the
  `Encoding.*` String-taking kernels and their runtime callers (`email.rs`,
  `compression.rs`, `ws_client.rs`, `server.rs`) onto the `Bytes`(`Vec<u8>`)
  primitive so the text path can UTF-8-encode (matching Go) while the binary path
  stays lossless via `Bytes`, converging on `base64Encode s == Bytes.toBase64
  (Bytes.fromString s)`. Doing so today would corrupt the existing Latin-1 binary
  callers, so it is its own milestone, recorded here rather than left implicit.

- **`Sky.Core.Jwt` API surface — flat kernels vs the Go builder API (M5b
  interim).** The Go backend exposes JWT through a builder API: `Jwt.encode
  (Jwt.hs256 secret) (Jwt.claims |> Jwt.subject … |> Jwt.expiresAt …)` and
  `Jwt.decode (Jwt.hs256 secret) now token`, with `Algorithm` / `Claims` types.
  The Rust backend currently surfaces four FLAT kernels —
  `Jwt.encodeHs256` / `decodeHs256` / `encodeRs256` / `decodeRs256` — taking the
  claims as a JSON string directly. **The token BYTES are identical to Go**: the
  encode path rebuilds the compact JWS through the same Go-parity primitives the
  Go module uses (`Json.Encode.encode 0` for header + payload, `Crypto.hmacSha256`
  / `Crypto.rsaSha256Sign` for the signature), so for a fixed key + claims the
  emitted token equals the Go reference token byte-for-byte (proven by the
  captured-Go-token goldens `m5b_jwt_hs256_bytes` / `m5b_jwt_rs256_bytes` and the
  byte-equality assertions in `crates/skyc/tests/golden_m5b_uuid_jwt.rs` +
  `runtime/src/sky_runtime/jwt.rs`). Only the CALL SURFACE differs, so a
  Go-targeted program using the builder API does not yet compile on the Rust
  backend, and the flat-kernel goldens cannot run the same `Main.sky` on the Go
  oracle — hence `oracle_divergence = true` (`divergence:` reason) for every
  `m5b_jwt_*` golden, with skyc's own output cached. Recorded as `divergence:
  Sky.Core.Jwt flat encode/decode kernels are the Rust-backend M5b interim
  surface; the Go backend exposes the builder API`. **Tracked follow-up (not
  deferred silently):** add the Go-shaped builder API (`Jwt.encode` / `hs256` /
  `rs256` / `claims` / `decode` + `Algorithm` / `Claims`) on the Rust backend so
  a Go-targeted JWT program compiles unchanged and the goldens become
  shared-`Main.sky` Go-parity goldens — the byte layout is already identical, so
  this is a surface/API milestone, not a codec change.

- **`Sky.Core.Uuid` — Rust evaluates the kernels where the Go reference differs
  on these shapes (M5b).** Two recorded behavioural divergences, both with
  Sky-Rust producing the semantically-correct result (`sanctioned:` reason,
  skyc's output cached): (1) the bare arity-0 kernel value `Uuid.v4` / `Uuid.v7`
  evaluates to a fresh `String` call on the Rust backend (the documented
  bare-reference form), whereas the Go reference leaves the bare reference as a
  kernel function value (CLAUDE.md Limitation #7 — arity-0 kernel codegen), so
  `m5b_uuid_format`'s length/version-nibble checks differ on Go; (2) the Rust
  backend's `Uuid.parse` accepts a canonical hyphenated UUID (`Just`) and rejects
  malformed input (`Nothing`), whereas the Go reference returns `Nothing` for the
  same canonical UUID on this shape (`m5b_uuid_parse`). Recorded as `sanctioned:`
  markers in each golden directory.

## How to add a marker divergence (`divergence:` or `sanctioned:`)

1. Decide the tag. `sanctioned:` — Sky-Rust is genuinely more correct
   (PRINCIPLES §2). `divergence:` — Sky's current behaviour differs; Sky-Rust
   follows a different target (e.g. Elm-conformance, fuller Unicode). State
   the difference and the rationale neutrally. If in doubt, fix the parity bug
   instead.
2. Add `tests/golden/<name>/sanctioned.divergence` with a one-line reason,
   prefixed with the chosen tag (`divergence: …` / `sanctioned: …`). An untagged
   reason defaults to `sanctioned: `.
3. Run `refresh-oracle <name>` (the runtime is resolved automatically from the
   in-repo `runtime/src/sky_runtime`; set `SKY_RUNTIME_DIR` to override). It
   captures Sky-Rust's output as the expected and writes
   `oracle_divergence = true` + `divergence_reason = <tag> <reason>` — WITHOUT
   requiring Go to fail.
4. Commit `Main.sky`, `expected_go.txt`, `oracle.meta`, and
   `sanctioned.divergence` together. For a `divergence:` entry, also add a
   one-line row to the registry above documenting the difference and rationale.

## Decision: full-Unicode case mapping is intended — deferrals to post-M6

The case-mapping divergence above (`String.toUpper`/`toLower`/`casefold`,
`Char.toUpper`/`toLower`) is a **settled, deliberate decision**, not a bug to
chase down: full-Unicode case mapping is *free* in Rust, strictly more correct
than Go's simple per-rune mapping, and matches the mainstream
(Rust/Python/Swift/`elm-string`/Haskell `Text`). Correctness wins over byte
parity here (PRINCIPLES.md §2). We keep the case kernels full-Unicode and do
**not** "fix them toward Go".

The following are **explicitly DEFERRED to post-M6** (out of scope now — for
velocity, not because they are unimportant):

- **Locale-correct case mapping** (e.g. Turkish dotless-ı / dotted-İ, Greek
  final sigma) — Rust's default `to_uppercase`/`to_lowercase` are locale-
  independent; locale tailoring would need ICU-style data we are not pulling in.
- **Formal oracle-divergence recording for non-ASCII case** — the sanctioned-
  divergence machinery (marker file + recorded reason) exists, but we have NOT
  authored the non-ASCII case goldens (`ß → SS`, `İ → i̇`, …) that would
  exercise it. Doing so is post-M6 work.
- **Non-ASCII case goldens** themselves — only ASCII case parity
  (`toUpper "hi" == "HI"`) is pinned in the M4b suite.

**Unicode lives in the CORE — permanently.** A Roc-style "Unicode as an
upgradable / opt-in package" model is explicitly **rejected** (user directive):
full Unicode is a built-in commitment of the core runtime, and every divergence
and decision is documented *here, in detail*, rather than relocated behind a
separate package. The deferrals above are *velocity* deferrals — more goldens
and locale tailoring to add later, **in core** — never a move of Unicode out of
core.

Predicates (`Char.isDigit`/`isLower`/`isUpper`/`isAlpha`), `String.split`, and
`String.toInt`/`toFloat` are NOT in this carve-out: they are pure Go-parity and
are fixed to match the Go reference's observable behaviour exactly (e.g.
`String.toInt " 42 " == Nothing` — Go's emitted `String_toInt` does not trim,
which also matches Elm).

## Open questions — resolve in the Elm-conformance phase

The Elm-derived stdlib's design source is Elm (`elm/core`, `elm/json`); the current
behaviour tracks the Go reference as a bootstrap step. Where the two differ, the
Elm semantics is the intended spec. To audit + decide later:

- **Json (`Encode`/`Decode`/`Pipeline`)** — audit vs Elm `elm/json`: object key
  order (Elm preserves insertion order), `Decode.int` on non-integer numbers,
  JSON float number formatting, decoder error structure, `null`/`oneOf`/`nullable`
  behaviour.
- **General** — the Elm-conformance phase audits stdlib *behaviour* against Elm,
  not only missing-function coverage.
