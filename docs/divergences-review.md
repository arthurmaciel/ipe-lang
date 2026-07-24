# Divergences Review — Guardian Synthesis

> Read-only evaluation of the two filed departure ledgers
> (`docs/divergences-from-elm.md`, `docs/divergences-from-sky.md`). This
> document does **not** edit the ledgers — a separate verification pass owns
> them. It merges three independent reasoner evaluations (product/README lens,
> soundness/correctness lens, roadmap lens) into one PRINCIPLES-ordered verdict.
>
> **Principle order used throughout:** security > correctness > soundness >
> efficiency > completeness > readability. Two governing rules: *parse, don't
> validate*; *make invalid states unrepresentable*.
>
> **Public-artifact rule (enforced in every line below):** neither Elm nor Ipê
> is characterized as buggy, wrong, or limited. Every departure is stated as
> *what differs* plus *the technical rationale*. Where Ipê matches Go or Elm
> more closely than the upstream reference does, the framing is "Ipê follows
> Elm-conformance / Go `%v` semantics; the reference's fork differs here" —
> never "the reference is broken."

---

## 1. Classification table

Verdicts: **FEATURE** (strength to surface) · **NEUTRAL** (substrate
consequence, no claim) · **INTERIM** (tracked convergence item) · **RECONSIDER**
(revisit — may be a latent parity/soundness issue). Where the three reasoners
diverged, the row records it and gives the reconciled call.

### Elm ledger

| id | Verdict | One-line rationale | README treatment |
|---|---|---|---|
| E1 | FEATURE | Direct `Task`-returning effect stdlib — the root native/server-target capability. | Lead |
| E2 | FEATURE | Four-tier effect taxonomy makes failure/effect mode legible in the type (parse-don't-validate at the signature). | Lead |
| E3 | NEUTRAL | `Task.run` at a real process entry follows from the `func main` target. | Omit |
| E4 | NEUTRAL | Auto-forced `let _ = TaskExpr` is ergonomic; mild implicit-effect smell, bounded + documented. | Omit |
| E5 | FEATURE | Task/Result bridges + retry as a named surface; absence of `Result.fromTask` is a principled asymmetry. | Support |
| E6 | FEATURE | Server-side pub/sub broker — structurally impossible in a browser runtime. | Support |
| E7 | NEUTRAL | `System.exit : Int -> a` diverging tier — sound, but a substrate necessity. | Omit |
| ER1 | FEATURE | Typed-`Error` mandate, test-enforced; strongest error-model claim. | Lead |
| ER2 | NEUTRAL | Deleted `IoError`/`RemoteData`; expressible via `Maybe (Result Error a)`. | Omit |
| ER3 | FEATURE | Two-level error pattern (correlation id + structured log + no leak) = production operability. | Support |
| S1 | FEATURE | `Ffi.kernel` + HM-typed auto crate-binding at SDK scale. | Lead |
| S2 | FEATURE | `{{expr}}` interpolation with `\{{` passthrough escape. | Support |
| S3 | NEUTRAL | Reserved-name rewriting — invisible codegen hygiene. | Omit entirely |
| S4 | INTERIM `[pre-push]` | Continuation-inside-type-body unsupported; honestly flagged as a parser limitation, not a choice. Elm accepts it. | Converging |
| S5 | NEUTRAL | Already parity (closed v0.16.4); kept for grep only. | Omit |
| P1 | FEATURE | Typed Go/Rust native/server/desktop/WASM target — the root fact. | Lead (headline) |
| P2 | FEATURE | Server-driven TEA over SSE; state + secrets stay server-side. | Lead |
| P3 | FEATURE | One pure `view`, three backends (web/TUI/desktop). | Lead |
| U1 | NEUTRAL→FEATURE | Server-rendered `Ipe.Ui`; the sellable angle is "no client build." | Support |
| U2 | FEATURE | Typed pseudo/media/animation/grid surface elm-ui lacks, keeping "never write CSS." | Support |
| U3 | NEUTRAL | Asymmetric `Ui.fill` lowering — a Flexbox §9.8 indefinite-height fix. | Omit (footnote) |
| U4 | FEATURE | No `data-ipe-eval`/eval sink, HTML-escape-everything, CSP-strict. | Lead (security) |
| R1 | NEUTRAL | Haystack-first `*In` companions; both forms ship. | Omit |
| R2 | NEUTRAL | `ToString.*` discoverability namespace. | Omit |
| R3 | INTERIM `[with arity-0 fix]` | `Ipe.Pure` arity-0 companions — an admitted workaround for Limitation #7. Couples to B7. | Converging |
| R4 | FEATURE | Task error slot fixed to `Error` (no `Never`-task) — mandate at the type level. Caveat: loses Elm's `Task Never a` infallible expressiveness. | Support |
| R5 | NEUTRAL (mostly) | Browser/ports/`Debug`/`Tuple` omission is sound; **`Array`/`Bitwise` absence is a capability gap, not a clean omission** — see §6. | Support w/ caveat |
| STR1 | FEATURE | Code-point String semantics; no UTF-16 surrogate splitting. Caveat: code points ≠ graphemes — say "rune-correct," not "grapheme-correct." | Lead (w/ caveat) |
| STR2 | NEUTRAL→FEATURE | Extra typed String surface (casefold/isEmail/words/lines). | Support |

### Ipê ledger — behavioral

| id | Verdict | One-line rationale | README treatment |
|---|---|---|---|
| B1 | FEATURE | `Math.min/max` Elm-comparable (polymorphic) where the reference's `AsInt`-coerce path differs. | Support |
| B2 | FEATURE | `Bytes` = distinct `Vec<u8>`; makes non-UTF-8-in-String unrepresentable; lossless binary. | Lead |
| **B3** | **RECONSIDER** `[pre-push]` | Latin-1 char-as-byte over the **text** path diverges from Go for code points ≥ 0x80; sanctioned framing masks an interim correctness regression. **All three reasoners flag as top wrongly-diverged.** See §5. | **Keep out of README until fixed** |
| B4 | FEATURE | `Money.allocate` shares sum to input on negative totals — strictly more correct. | Support |
| B5 | FEATURE | Full-Unicode `SpecialCasing` (`ß→SS`); mainstream-aligned (Rust/Python/Swift). | Support |
| B6 | FEATURE | Stricter `toFloat` grammar (rejects hex-float/underscore) — parse-don't-validate + Elm-conformant. | Support |
| **B7** | **RECONSIDER / INTERIM** `[with arity-0 fix]` | Bare `Uuid.v4 : String` evaluates — but typing entropy as the **pure** tier contradicts E2 (entropy lives in `Task`). **Disagreement:** R1 filed it INTERIM-as-feature; R2/R3 flag it as a soundness inconsistency. **Reconciled:** present once, consistently, as interim — never as a strength. See §5. | Converging (not a strength) |
| B8 | FEATURE (verify) | `Uuid.parse` `Just` on a canonical UUID. **R3 caveat:** confirm this is genuine parse semantics, not the same arity-0 codegen artifact as B7. See §5. | Support (pending verify) |
| B9 | INTERIM `[pre-push]` | Jwt flat-kernel; builder-API programs don't compile yet, though token bytes are byte-identical to Go. | Converging |
| B10 | NEUTRAL | `Ipe.Db` on sqlx vs cgo/SQLite — substrate. | Omit |
| B11 | INTERIM `[post-DONE, low]` | `Ipe.Ui` HTML skeleton; semantically correct now, byte-parity later. | Converging |
| B12 | NEUTRAL | `Cmd`/`Sub` construct-only; the reference has no equivalent surface. | Omit |
| B13 | FEATURE | Recursive enums through tuple/record boxing + Set shapes the reference can't build — Ipê handles strictly more. | Support |
| B14 | FEATURE | Runtime-fork hardening: auth fail-close, JWT `now==exp` reject (RFC-correct), SSRF-deny-default, saturating counters, env lock, telemetry CRLF/control scrub. | **Lead (flagship security)** |
| B15 | FEATURE | Float sci-notation exp≥6 = Go `%v` parity (confirmed vs Go 1.26.2). Package as "Ipê matches Go byte-for-byte; the reference's fork differs." | Support (trust signal) |

### Ipê ledger — architectural

| id | Verdict | One-line rationale | README treatment |
|---|---|---|---|
| A1 | NEUTRAL | Rust-all-the-way single-language port structure. | Omit |
| A2 | FEATURE | Typed IR checkpoint — malformed shapes unrepresentable vs AST→string. | Lead |
| A3 | FEATURE | Typed `TailRecur`/`TailLoop` → Rust `loop`; constant stack where the reference's Rust backend has no TCO. Soundness win. | Lead |
| A4 | FEATURE | Closed 424-variant `KernelFn`, fail-closed `IPE-L0108` vs a fail-open snake_case path; kills the "ipe exits 0, cargo fails" class. | Lead (security) |
| A5 | FEATURE | `render_type → DResult`, no `"String"` default — total by construction. | Support |
| A6 | NEUTRAL→FEATURE | First-class opaque `IrType` variants vs `{M}`-placeholder strings. | Support |
| A7 | FEATURE (**watch**) | Exact-key record resolution, fail-loud > superset-widen + `"String"`. **All three reasoners: watch-item** — could reject a valid row-polymorphic subset/superset program the reference accepts. See §5. | Support (pending sweep gate) |
| A8 | INTERIM `[post-DONE]` | Uniform `Box<dyn Fn>` callbacks; reference-ahead 3-way split adopted when the Clone-callback subsystem lands. | Converging |
| A9 | FEATURE | Typed `const CrateSpec` SSOT + drift test over string re-parse. | Support |
| A10 | FEATURE | Kernel-registry drift tripwires from `StdlibKernel::ALL`. | Support |
| A11 | NEUTRAL | Vendored runtime superset fork (structural framing for B14). | Omit |
| A12 | FEATURE | Fail-closed refutable arg patterns + desugar-to-`case`; no reachable panic from well-typed code. | Support |
| A13 | INTERIM `[pre-push]` | Nested ctor-payload patterns (`Just (h :: t)`, `Ok {name}`) rejected fail-closed — sound, but blocks a bread-and-butter Elm idiom. Most user-visible convergence gap. | Converging (call out) |
| §4 front-end gaps | INTERIM `[pre-push]` | `.field` accessor-as-function, refutable-arg (A12 close), nested-ctor (A13), mutual/let-rec TCO — bundle as one front-end workstream. | Converging |

---

## 2. Lead-with list (README, ordered)

A skeptic accepts these on inspection; they are the identity.

1. **Native / server / desktop target with a pure TEA core** — P1, P2, P3, U1.
   One pure `view` renders to web (server-driven SSE), terminal, and desktop;
   state and secrets never serialize to the client.
2. **Security fail-closed everywhere** — B14 (auth fail-close, RFC-correct JWT
   expiry, SSRF-deny-by-default, telemetry CRLF/control scrub), U4 (no eval
   sink, HTML-escape-everything, CSP-strict), A4 (fail-closed kernel dispatch).
   Frame: "fail-closed where the substrate would otherwise fall open."
3. **Total-by-construction, typed-IR compiler** — A2, A3, A4, A5, A9, A10.
   "Malformed shapes are unrepresentable, not caught at `rustc`"; typed TCO
   gives constant stack.
4. **Correctness where the reference's fork diverges** — STR1 (rune-correct
   strings), B5 (full-Unicode casing), B4 (money sums to input), B1
   (comparable min/max), B15 (Go `%v` float parity, confirmed vs Go 1.26.2),
   B2 (lossless typed `Bytes`), B13 (richer ADT shapes).
5. **Typed effects + typed errors** — E1, E2, ER1, ER3, R4. Task-everywhere,
   four-tier taxonomy, mandated typed `Error`, correlation-id operability.
6. **HM-typed FFI at SDK scale** — S1, S2. Auto crate-binding at 76k-symbol
   scale; first-class server templating.

---

## 3. Converge-later list (interim departures + target milestone)

Group these under one "Known convergence backlog" heading so a reviewer sees
they are filed and sequenced, not hidden.

| Item | What differs today | Target milestone |
|---|---|---|
| B3 — `Encoding.*` text path | Latin-1 char-as-byte over `String`; differs from Go for code points ≥ 0x80. | **Pre-push.** `Encoding.* → Bytes` primitive migration (unblocked — B2 exists). See §5. |
| B7 + R3 — arity-0 entropy/kernel | Bare `Uuid.v4 : String`; entropy typed pure; `Ipe.Pure` workaround. | **Pre-push.** Arity-0 kernel codegen fix (Limitation #7) — one fix closes R3 + B7 (+ possibly B8). See §5. |
| B9 — Jwt builder API | Flat kernels; builder-API programs don't compile (token bytes already identical to Go). | Pre-push. `Algorithm`/`Claims` ADT + record-alias emit. |
| A13 + §4 — nested-ctor / refutable-arg / `.field` accessor | Rejected fail-closed; blocks idiomatic Elm patterns. | **Pre-push.** Shared front-end desugar workstream (closes A12 + A13 + accessor + S4). |
| S4 — continuation-in-type-body | Parser rejects continuation inside the type body; extract a `type alias`. | Pre-push (folds into the front-end workstream). |
| A8 — callback lowering | Uniform `Box<dyn Fn>`; reference-ahead 3-way split. | Post-DONE — when the derive/Clone callback subsystem lands. |
| B11 — `Ipe.Ui` byte-parity | HTML skeleton differs byte-wise (semantically correct). | Post-DONE, low priority. |

---

## 4. Reconsider list (revisit, with argument)

These are not headline strengths and warrant a design revisit even where they
are not outright bugs. (The two that are latent bugs are escalated to §5.)

- **R4 caveat — no `Task Never a`.** Fixing the error slot to `Error` enforces
  the typed-error mandate but removes Elm's ability to type an infallible
  effect as `Task Never a`. An infallible effect still nominally carries
  `Error`. Revisit whether a `Never`-parameterized escape hatch is worth
  reintroducing for provably-total effects, or document the deliberate trade.
- **R5 — `Array`/`Bitwise` omission.** Framed as a clean omission, but on a
  native/efficiency-targeted backend `List`-only indexing is O(N) where Elm
  ships `Array`. This is a capability gap, not a divergence; see §6.
- **A7 — exact-key record resolution.** Sound direction (soundness >
  completeness), but "add the superset fallback only if a real example trips
  it" means the first legitimate superset-row call site surfaces as a
  `CompilerBug` to the user. Gate on the example sweep before public push; see
  §5.

---

## 5. Wrongly-diverged (highest priority)

Filed "choices" that may be latent parity/soundness issues. All three reasoners
independently converged on the same three. Each carries a concrete
verify/fix action — candidates to file as tasks.

### WD-1 — B3: `Encoding.base64Encode` / `hexEncode` Latin-1 over text

**Consensus: all three reasoners rank this the top item.**

- **Symptom.** `hexEncode "café"` → `636166e9` (Latin-1 char-as-byte), where Go
  yields `636166c3a9` (UTF-8). The Latin-1 model is correct for the *binary*
  `Bytes` pipeline (lossless) but produces different output than Go on the
  *text* path for any code point ≥ 0x80.
- **Why it inverts the principle order.** Efficiency/binary-convenience is
  currently placed above text correctness. Unlike B5/STR1 (defensible
  full-Unicode stances), this is neither Go-parity nor a principled Unicode
  position — it is an interim answer for code points ≥ 0x80.
- **Open question the ledger does not answer** (raised by R2/R3): what happens
  for code points **> U+00FF** (e.g. `"€"`, U+20AC)? If the path panics or
  silently truncates, it is a soundness violation ("no runtime panic / no
  silent corruption from well-typed code"), not merely a text-parity gap.
- **Security cross-check** (raised by R1): the B3 rationale names
  `base64(hexDecode(hmac))` inside JWT. Verify HMAC/signature bytes always
  route through the `Bytes` overload (or are provably < 0x80), else there is a
  signature-divergence-vs-Go risk on the security path.
- **Verify/fix actions.**
  1. Determine the behavior of the text overload for code points > U+00FF
     (panic / truncate / replacement). If not a clean `Err`, treat as a bug.
  2. Confirm no security primitive (JWT signing, HMAC) routes text-overload
     `Encoding.*` output.
  3. Land the already-tracked `Encoding.* → Bytes` migration: UTF-8 (Go-parity)
     on the `String`-text overload, lossless on the `Bytes` overload.
- **README:** keep out until fixed, or scope any mention strictly to the binary
  pipeline. Leading with it invites the `café` counter-example.

### WD-2 — B7 / R3 / Limitation #7: entropy typed as pure

- **Symptom.** `Uuid.v4 : String` — a non-deterministic, entropy-backed value
  carries the **pure** tier, directly contradicting E2's own taxonomy (which
  places `Crypto.randomBytes` / `randomToken` in `Task`). Internal
  inconsistency: `Crypto.randomToken` is `Task` while `Uuid.v4` is pure.
- **Soundness risk.** An effect masquerading as pure is unsound under any
  optimization that assumes referential transparency (CSE, memoization,
  reordering) — such passes could dedupe or reorder UUID generation.
- **Root cause.** This is a symptom of the arity-0 kernel codegen limitation
  (#7), surfaced across two ledgers with **opposite valence**: the Elm ledger
  (R3) calls it an honest workaround; the Ipê ledger (B7) frames the same
  limitation as Ipê being "more useful." **Reconciled call:** present it once,
  consistently, as an interim codegen limitation converging — never as a
  strength.
- **Verify/fix actions.**
  1. Fix arity-0 kernel codegen (Limitation #7) — the single highest-leverage
     item; one fix closes R3 + B7.
  2. Until then, either reclassify UUID generation to the effect tier or
     document that the binding is effectful despite its surface type.

### WD-3 — B8: verify it is genuine parse semantics, not the arity-0 artifact

- **Symptom.** B8 is filed as a correctness *strength* (`Uuid.parse` returns
  `Just` on a canonical UUID where the reference returns `Nothing`). R3 notes
  this "reference returns Nothing on this shape" smells like the **same**
  arity-0 codegen artifact as B7, not a genuine reference semantics difference.
- **Verify/fix action.** Confirm B8 is real parse behavior. If it is the
  identical arity-0 leakage, B7 + B8 collapse into "fix arity-0 codegen" and B8
  stops being a divergence at all — do not ship it as a standalone strength
  until confirmed.

### WD-watch — A7: exact-key record resolution

- Not confirmed wrong, but the fail-loud stance could reject a valid
  row-polymorphic program the reference accepts (optional cfg fields — `head`,
  `consoleAuth` — pass subset/superset records; this is the very row-poly
  pattern the design blesses). A rejection would surface as a `CompilerBug` and
  would be a parity regression mislabeled as a soundness choice.
- **Verify/fix action.** Add an explicit test proving a row-poly subset-record
  call site still resolves; gate A7 on the example sweep before public push. If
  a real example trips the guard, the superset path is *needed*, not optional.

---

## 6. Missing divergences worth filing

Departures Ipê arguably should make (or additive surfaces it should add) that
are not yet filed. Reasoners converged on the first four.

1. **Grapheme-cluster String surface** (additive `String.graphemes` /
   segmented length/truncation). STR1 is rune-correct but still splits ZWJ
   emoji and combining sequences; `Ipe.Tui` already vendors `uniseg`, so the
   machinery exists. File additive — do **not** change `length`. *Post-DONE.*
2. **Unicode normalization (NFC/NFD)** for `String` equality / `equalFold` /
   `isEmail`. Ipê is full-Unicode on casing but code-point-literal on equality
   (`"é"` composed ≠ decomposed). A normalizing comparison out-correctness both
   Elm and Go; mainstream (Swift/Python) makes this move. *Post-DONE.*
3. **`Array` (dense typed vector)** with O(1) indexed access. Elm ships it;
   `List`-only forces O(N) indexing on a native/data-workload backend where a
   Rust `Vec` makes O(1) free. File as an ADD. *Post-DONE.*
4. **`Bytes`-based `Encoding.*`** — this is the *fix* for WD-1, not an
   optimization. File as its own convergence task (UTF-8 text path + explicit
   `Bytes` binary path). *Pre-push.*
5. **`Bitwise` module** — omitted; crypto/protocol/framing code wants it.
   *Post-DONE.*
6. **Parameterized-query newtype (`SqlFragment`)** making
   string-concatenated raw SQL unrepresentable on the
   `unsafeFindWhere`/`findByConditions` escape path — a parse-don't-validate
   divergence that extends ER1/B10. *Evaluate.*
7. **Sub-domain typed errors** (`ParseError`/`DecodeError`/`Db`/`Http`/`Auth`
   closed taxonomy) refining the single `Error` slot — pushes
   make-invalid-states-unrepresentable further. *Post-DONE.*
8. **Decimal division precision** — documented in the ledger notes + CLAUDE
   learnings but not filed as a numbered divergence, though it is a real
   Go-parity boundary. Pin it or fold it explicitly into the B-series.
9. **General ADT-emit backend** (per roadmap) — richer enum/builder emission of
   which the B9 JWT gap is one instance. Name the general item so B9 is not
   seen as a one-off. *Post-DONE.*

---

## 7. README narrative outline — "How Ipê relates to Elm and Ipê"

Suggested shape for the public section. Neutral voice throughout.

- **Opening frame.** Ipê is an Elm-family functional language whose target is
  native/server/desktop binaries via a typed backend, rather than a browser
  sandbox. Most departures from Elm and from the upstream Ipe reference follow
  directly from that target. State each as *what differs + why*.
- **Lead paragraph — reach + identity.** One pure `view` → web (server-driven
  TEA over SSE), terminal, and desktop; state and secrets stay server-side;
  compiles to native binaries; HM-typed FFI at SDK scale. (P1-3, E1, S1.)
- **Security paragraph.** Fail-closed by default: no client-side eval sink,
  HTML-escape-everything, CSP-strict rendering; runtime hardening for auth,
  JWT expiry, SSRF, and telemetry; a closed, fail-closed compiler kernel
  registry. (U4, B14, A4.) Keep the internal mechanism detail (e.g. the
  reference's `unwrap_or(0)` authenticate-as-user-0 path) in the audit doc —
  public copy says "Ipê's runtime fork adds fail-closed auth hardening."
- **Correctness paragraph.** "Go-parity by default; more correct where the
  reference's fork has a defect." Rune-correct strings (no surrogate
  splitting), full-Unicode case mapping, money splits that sum to input,
  comparable min/max, Go `%v` float parity confirmed against Go 1.26.2. Phrase
  B1/B8/B15 as "Ipê follows Elm-conformance / matches Go `%v`" — never "the
  reference is broken."
- **Compiler paragraph.** Typed IR, total-by-construction type rendering, typed
  tail-call loops (constant stack), compiler-checked crate-version and
  kernel-registry drift. "Code that would not build is not emitted."
- **Honesty paragraph — "Known convergence backlog."** One line: "the following
  are tracked, filed, and sequenced convergence items, not open questions,"
  then list B9 (note token bytes already identical to Go), B11, the front-end
  pattern-completeness workstream (nested-ctor / refutable-arg / `.field`
  accessor / continuation-in-type-body), and the `Encoding.* → Bytes` text-path
  migration.
- **Do-not-over-claim guardrails.**
  - Say "rune-correct," not "grapheme-correct."
  - Do not imply `Array`/`Bitwise` parity with Elm — they are omitted.
  - Keep B3 (text-path encoding) and B7 (entropy typing) out of the public copy
    until resolved — both are trivially counter-exampled and would undercut the
    "Unicode-correct" and "typed-effects" headlines.
