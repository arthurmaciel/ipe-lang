# Go-Oracle Fixture Corpus — regression manifest + normalizer wiring plan (task #51)

Read-only, doc-only planning artifact. Prepares task #51 (Go-oracle + equivalence
harness) for drop-in implementation. It catalogues the reference parity corpus
under `../sky/runtime-rust/tests/sky/` (140 fixtures) into a concrete ipê
regression manifest, prioritizes the port by **silent-divergence danger**, and
specifies how the already-vendored render normalizers get wired, the equivalence gate
flipped on, and CRLF handled for cross-OS CI.

**PRINCIPLES order — Security > Correctness > Soundness.** Every ruling below is
ordered by that priority. Two blocking rules govern the harness itself:

- **Rule 1 (no false green).** A normalizer that collapses away a
  *behaviourally-meaningful* difference — so a real Go≠ipê divergence renders as
  an empty diff — is a **correctness defect**, not a convenience. It silently
  ships a wrong runtime. Every masking/collapsing step in
  `equivalence_normalize_html.py` / `equivalence_tui_grid.py` / the stdout `norm()` is
  audited below for this.
- **Rule 2 (no false red).** A normalizer that reports a DIFFER for two outputs
  that are behaviourally identical (implementation-freedom surface, or a
  *sanctioned* divergence recorded as `oracle_divergence`) is equally a
  correctness defect: it poisons the green gate and trains reviewers to ignore
  red. Sanctioned divergences (Rust/Unicode/modern-correct choices where ipê
  deliberately differs from Go) MUST be tagged, never "fixed" to match Go.

**Public-artifact note.** `../sky`'s parity harness is the **reference parity
oracle** for ipê and is treated as an authoritative spec throughout. This
document neither disparages it nor proposes upstream contribution; it adapts the
reference into ipê's tree.

---

## 0. Scope: what is in, what is out

| Bucket | Count | Disposition |
|---|---:|---|
| Total fixtures in `tests/sky/` | **140** | — |
| **FFI-dependent** (declare `[rust.dependencies]` OR `import Sky.Ffi`) | **75** | **OUT of scope** until the FFI phase — the whole shim-free Rust-crate binding path (`43–114 ffi-*`, the `01–16` crate demos, `40/41/42/46` struct/enum-accessor codegen, `44-wide-int`, `45/46/47/48` builder-crate tests, `58-ffi-calltask-static`) needs the inspector→emit subsystem ipê has not built yet. |
| **Non-FFI, in scope** | **65** | The corpus to author now. Each doubles as a skyc golden. |

The FFI/non-FFI split was derived mechanically: a fixture is FFI iff its
`sky.toml` has a `[rust.dependencies]` table or its `Main.sky` imports
`Sky.Ffi`. This matches reference-audit items 20/21/22
(`docs/architecture/sky-rust-backend-reference-audit.md`).

**Count of non-FFI fixtures in scope: 65.**

---

## 1. Enumerated manifest — the 65 non-FFI fixtures

Shape drives the equivalence mode (reference `equivalence_mode` in `lib/examples.sh`):
`cli→stdout`, `server→body`, `live→scenario`, `tui→pty`, `webview→none`.

**Divergence class** = the specific Go-vs-ipê behaviour each fixture guards.
`SILENT` classes pass skyc but mismatch Go with *no error* — the dangerous ones.
`ERROR`/`STRUCT` classes surface as a build/run failure or a visible structural
diff.

### 1a. CLI / stdout mode (42)

| # | Fixture | Divergence class | Silent? |
|---|---|---|---|
| 1 | `kernel-parity-probe` | **Multi-kernel dump** — Dict.toList order, Float toString, List/Maybe/Path/Random surface. The densest silent-class net. | **SILENT** |
| 2 | `kernel-parity-probe-set` | **Set determinism** (BTreeSet ordered vs Go map-random) + Ord obligation on element type | **SILENT** |
| 3 | `kernel-parity-probe-money` | **Money rounding** (`StringFixed`/half-away vs banker's) + ISO-code TEXT round-trip | **SILENT** |
| 4 | `kernel-parity-probe-dbdec` | **Db.Decode** decimal/nullable round-trip | **SILENT** |
| 5 | `kernel-parity-probe-dbdec2` | Db.Decode composed-decoder column NULL gating | **SILENT** |
| 6 | `kernel-parity-probe-sqlfields` | SqlValue/SqlField typed param binding, OmitField SQL shape | ERROR/SILENT |
| 7 | `63-int-overflow-wrap` | **i64 overflow wrap** (Go `overflow-checks=false` two's-complement vs Rust panic/checked) | **SILENT** |
| 8 | `67-random-float-bounds` | **Float formatting** + bounded-random range invariants | **SILENT** |
| 9 | `65-crypto-random-encoding` | randomBytes hex length / randomToken base64url length + alphabet | **SILENT** |
| 10 | `64-log-with-attrs` | `Log.*With` attr flattening (SkyStringify bound) + plain/json line shape | SILENT (post-TS-norm) |
| 11 | `60-errortostring-string` | `Basics.errorToString`/SkyStringify on a String | **SILENT** |
| 12 | `23-char` | `Sky.Core.Char` classification kernels | SILENT |
| 13 | `53-cons-pattern-tuple` | cons-pattern destructure of tuple elements (length-guard) | ERROR |
| 14 | `56-list-sort` | `List.sortWith`/`sortBy` ordering | SILENT |
| 15 | `44-curried-return` | curried-function return codegen | ERROR |
| 16 | `57-record-alias-any` | parametric record alias `any` soundness | ERROR |
| 17 | `51-let-lambda-param-infer` | let-bound lambda param inference | ERROR |
| 18 | `52-task-fn-capture` | Task closure fn-capture (Send) | ERROR |
| 19 | `54-discard-task-effect` | `let _ = TaskExpr` auto-force | SILENT |
| 20 | `59-result-passthrough-nosig` | Result passthrough without sig | ERROR |
| 21 | `62-nonclone-capture` | non-Clone capture in closure | ERROR |
| 22 | `45-usermod-kernel-collision` | user module vs kernel name collision | ERROR |
| 23 | `49-bytes-core` | `Sky.Core.Bytes` fromHex/toHex/base64 (note: reference held this out — `E0282` E-pinning gap) | ERROR |
| 24 | `101-task-rethunk` | Task re-thunk lowering | ERROR |
| 25 | `102-task-rethunk-free-tvar` | Task re-thunk with free tvar | ERROR |
| 26 | `103-task-rethunk-discard` | Task re-thunk discard + File effect | ERROR |
| 27 | `codegen-generic-recursive-adt` | generic recursive ADT codegen | ERROR |
| 28 | `codegen-record-destructure-param` | record destructure in param position | ERROR |
| 29 | `71-panic-classifier` | panic→classified exit (DivByZero/Coerce/IndexOOB) + errId | SILENT |
| 30 | `25-retry` | `Task.retryWith` policy/backoff | SILENT |
| 31 | `61-retry-transient` | retry on transient File error | SILENT |
| 32 | `26-stream-cli` | `Http.Stream.forEachChunk` CLI drain | STRUCT |
| 33 | `31-system-env-chain` | `System.*` env read/set chain (`Sky.Core.Pure`) | SILENT |
| 34 | `37-cache-cli` | `Std.Cache` LRU/TTL hits/misses/evictions counters | SILENT |
| 35 | `42-ws-client-onmessage` | WebSocket client onMessage sub | STRUCT |
| 36 | `17-db-todo-cli` | `Std.Db` CRUD CLI + two-level error | STRUCT |
| 37 | `18-auth-signup` | `Std.Auth` register/login (bcrypt/JWT) — secret never stringified | SECURITY |
| 38 | `19-config` | `Std.Config` TOML/YAML/JSON typed decode | STRUCT |
| 39 | `20-email` | `Std.Email` provider ADT (dry-run) | STRUCT |
| 40 | `68-db-migrate-cli` | `Std.Db.migrate` versioned + checksum | STRUCT |
| 41 | `66-db-postgres-compile` | Db + Postgres **build-only** compile gate | ERROR |
| 42 | `67-db-sqlvalue-params` | SqlValue mixed-type param binding end-to-end | ERROR/SILENT |

### 1b. Server / body mode (6)

| # | Fixture | Divergence class | Silent? |
|---|---|---|---|
| 43 | `21-sse-server` | `Server.Stream` SSE emit/finish body | STRUCT |
| 44 | `22-sse-relay` | upstream `Http.Stream` → `Server.Stream` relay | STRUCT |
| 45 | `24-http-api` | `Server.listen` routes + middleware (CORS/rate-limit) JSON body | STRUCT |
| 46 | `43-ws-server-capturing` | server WebSocket upgrade + broadcast capturing closure | STRUCT |
| 47 | `68-server-413` | `maxBodyBytes` POST-cap → 413 status | STRUCT |
| 48 | `alloc-stress` | **Json.Encode** at volume (HTML-escape `<>&`, U+2028/9, float threshold) + alloc bound | **SILENT** |

### 1c. Live / scenario mode (14)

| # | Fixture | Divergence class | Silent? |
|---|---|---|---|
| 49 | `69-html-render-parity` | **HTML render parity** — `#sky-root` structural + escaping (#47/F7 gate) | **SILENT (render)** |
| 50 | `70-style-injection` | **CSS/style-injection** — `</style>` breakout, `expression()`, script-verbatim (#47/F7 gate) | **SECURITY-SILENT** |
| 51 | `71-style-merge` | **style-merge** ordering + dedup in emitted inline CSS (#47/F7 gate) | **SILENT (render)** |
| 52 | `28-live-counter` | TEA update/patch round-trip | STRUCT |
| 53 | `29-live-form` | form onSubmit typed-record decode | STRUCT |
| 54 | `30-live-routing` | URL routing + `:param` capture order | STRUCT |
| 55 | `31-live-req` | init `req` shape (path/params/method/headers/cookies) | STRUCT |
| 56 | `27-live-static` | static file serving | STRUCT |
| 57 | `32-live-sessions` | session store round-trip (memory/sqlite) | STRUCT |
| 58 | `33-live-pubsub` | `Cmd.publish`/`Sub.subscribeTopic` echo | STRUCT |
| 59 | `34-live-pubsub-dict` | **Dict-keyed** pubsub state (Dict determinism in render) | **SILENT** |
| 60 | `35-live-db-startup` | DB-at-startup Cmd sequencing | STRUCT |
| 61 | `40-live-ui` | `Std.Ui` layout → HTML (`fill`/flex emission) | **SILENT (render)** |
| 62 | `50-event-handler-arc` | event-handler Arc-wrap (onClick round-trip Msg identity) | STRUCT |

### 1d. TUI / pty mode (2)

| # | Fixture | Divergence class | Silent? |
|---|---|---|---|
| 63 | `38-tui-ui` | `Std.Ui`→ANSI cell grid (layout + SGR styling) | **SILENT (render)** |
| 64 | `41-tui-input` | TUI input widget cells + cursor | **SILENT (render)** |

### 1e. Webview / none mode (1)

| # | Fixture | Divergence class | Silent? |
|---|---|---|---|
| 65 | `39-webview` | build+link only (opens a window; no comparable output → `none`) | BUILD-ONLY |

---

## 2. Prioritized port order — silent-divergence classes FIRST

Silent classes (pass skyc, mismatch Go, *no error*) are the dangerous ones and
are ported first. This is a burndown against a fixed target, not a coverage
tick.

### Tier 0 — pure-stdlib deterministic-stdout silent classes (port FIRST)

These need **no** Go oracle and **no** render normalizer — just build both
backends, run, `norm()` the stdout, byte-diff. They are the `equivalence-corpus.sh`
default set and map onto ipê's existing numeric-parity unit tests. **Top
silent-divergence fixtures to port first:**

1. `kernel-parity-probe` — widest silent net: **Dict.toList order + Float
   toString threshold** in one dump. (float threshold = the OPEN item 27 /
   `stringify.rs` `!(-4..21)` vs `!(-4..6)` disagreement, #52).
2. `kernel-parity-probe-set` — **Set determinism** + Ord obligation.
3. `kernel-parity-probe-money` — **Money/decimal rounding** (banker's vs
   half-away split; memory: two distinct rounding modes).
4. `kernel-parity-probe-dbdec` / `-dbdec2` / `-sqlfields` — Decimal/Money
   round-trip through Db.Decode.
5. `63-int-overflow-wrap` — **i64 wrap** (Go two's-complement vs Rust).
6. `alloc-stress` — **Json.Encode HTML-escape** (`<>&`, U+2028/9) + float
   threshold at volume. (json-escape is the sleeper: Go escapes by default,
   serde does not — cross-backend HMAC/webhook signatures diverge.)
7. `65-crypto-random-encoding` — base64url alphabet + hex/token lengths.
8. `67-random-float-bounds` — float formatting + range invariants.
9. `64-log-with-attrs` — attr flattening (SkyStringify).
10. `60-errortostring-string`, `23-char`, `54-discard-task-effect`,
    `56-list-sort`, `31-system-env-chain`, `37-cache-cli` — remaining
    pure-stdlib silent classes.

> **Explicit note on the float threshold (#52 / item 27):** ipê's
> `stringify.rs` switches to scientific notation at a **different** exponent
> than `../sky`'s Go (`!(-4..6)` vs `!(-4..21)`). Evidence recorded in the
> math-parity work favours ipê (Go-oracle-verified, has a pinning regression
> test). When `kernel-parity-probe` / `alloc-stress` land, this fixture line
> will DIFFER against a naive Go oracle — that is a **sanctioned divergence**
> (Rule 2), to be tagged `oracle_divergence`, **not** matched to Go, pending the
> one-line re-probe before push. If the fresh oracle says `21`, ipê is wrong and
> the pinning test is the bug.

### Tier 1 — codegen-soundness fixtures (mostly ERROR-class, cheap)

`44-curried-return`, `57-record-alias-any`, `51-let-lambda-param-infer`,
`52-task-fn-capture`, `53-cons-pattern-tuple`, `59-result-passthrough-nosig`,
`62-nonclone-capture`, `45-usermod-kernel-collision`, `101/102/103-task-rethunk*`,
`codegen-generic-recursive-adt`, `codegen-record-destructure-param`,
`71-panic-classifier`, `49-bytes-core`. These fail loudly if regressed, so they
are lower danger, but they are ipê goldens today and gate lowering changes.

### Tier 2 — render-silent classes (need the render normalizers + oracle)

`69-html-render-parity`, `70-style-injection`, `71-style-merge`, `40-live-ui`,
`34-live-pubsub-dict`, `38-tui-ui`, `41-tui-input`. Silent at the render layer;
require `equivalence_normalize_html.py` / `equivalence_tui_grid.py` driven against a Go
reference (see §3). `70-style-injection` and `69/71` also serve as the #47/F7
gate (§4) and can land as **stored-HTML snapshots first**, no oracle needed.

### Tier 3 — server/live/db/auth structural (need oracle + server-body diff)

`21/22/24/43/68-server-413`, the `27–35` live scenarios, `17/18/19/20/66/68`
db/auth/config CLIs. Structural; port once the server-body and live-scenario
equivalence paths in `examples-sweep.sh` are driven.

### Tier 4 — build-only

`39-webview`, `66-db-postgres-compile` — link/compile gates only.

---

## 3. Normalizer wiring plan

**Current state (verified):** `equivalence_normalize_html.py` and `equivalence_tui_grid.py`
are **already vendored byte-identical** into `scripts/lib/`, and
`scripts/equivalence-checks/examples-sweep.sh` already carries the EQUIVALENCE column, `build_go()`,
`equivalence_for()`, and the `IPE_SWEEP_NO_EQUIV` flag (default `1` = phase-1: BUILD +
RUN, EQUIVALENCE skipped). `scripts/equivalence-checks/equivalence-classification.tsv` is also ported
byte-identical. **What is missing:** the two standalone drivers
(`equivalence-corpus.sh`, `equivalence-render.sh`), the 65 fixtures themselves, a Go oracle,
and CRLF handling.

### 3.1 Drop-in steps

1. **Author the 65 fixtures** under `crates/skyc/tests/sky/<name>/` (mirroring
   the reference layout `src/Main.sky` + `sky.toml`). They double as skyc
   goldens. Port in the Tier 0→4 order above.
2. **Port `equivalence-corpus.sh`** (pure-stdlib deterministic-stdout driver) into
   `scripts/`. Point `FIXROOT` at ipê's fixture dir. Its `CORPUS_DEFAULT` = the
   Tier-0 silent set. Build both backends via ipê's `skyc` (Go backend needs the
   oracle — §3.2).
3. **Port `equivalence-render.sh`** (live-HTML + tui-grid driver) into `scripts/`;
   it already references `lib/equivalence_normalize_html.py` / `lib/equivalence_tui_grid.py`.
   `pyte` gates the tui path (SKIP if absent — a correct skip, not a false pass).
4. **Stand up the Go oracle** — two viable modes:
   - **Live-oracle:** point `IPE_GO_BIN` at an external Haskell `sky` that emits
     the Go reference; `build_go()` runs `sky build --backend go`. Highest
     fidelity, requires the reference toolchain present in CI.
   - **Snapshot-oracle (recommended to bootstrap):** commit the Go reference
     stdout / normalized `#sky-root` / tui-grid as golden files next to each
     fixture. No live Go build; the diff is fixture-stdout vs committed golden.
     Removes the CI dependency on a Haskell toolchain and makes the corpus
     reproducible. Regenerate goldens only from a pinned reference `sky`.
5. **Flip the gate on:** once the oracle exists, drop `IPE_SWEEP_NO_EQUIV=1`
   default (or run corpus/render explicitly). Keep `IPE_SWEEP_NO_EQUIV=1` as the
   escape hatch for the no-oracle machine.
6. **Port the harness self-tests** (reference items 20/24) so a normalizer
   regression is itself caught.

### 3.2 CRLF / line-ending normalization (also required for Windows CI)

None of the three normalizers currently strips CR. On a Windows runner every
fixture would false-RED (Rule 2 violation). Add, in this exact scope:

- **stdout `norm()` (equivalence-corpus.sh):** after the timestamp `sed` and
  blank-line strip, add `| sed 's/\r$//'` (or `tr -d '\r'`) so CRLF ≡ LF.
- **`equivalence_normalize_html.py`:** on read, `html = open(...).read()
  .replace('\r\n','\n').replace('\r','\n')` before parsing, so a CR inside a
  text node or attribute value can't produce a phantom diff. (HTMLParser does
  not normalize CR.)
- **`equivalence_tui_grid.py`:** pyte already interprets `\r` as a terminal carriage
  return (cursor-to-col-0), which is correct for ANSI capture — do **not** strip
  CR there; the raw byte stream must stay intact. Only the html/stdout paths get
  CRLF folding.
- **Golden files** are stored LF-only; add a `.gitattributes`
  `*.golden -text` / `text eol=lf` so git never rewrites them on checkout.

### 3.3 False-green / false-red audit of the vendored normalizers (Rules 1 & 2)

The normalizers are behavioural-parity collapsers. Each collapsing step is a
potential Rule-1 (false-green) hole; each re-serialization a potential Rule-2
(false-red) hole. Findings:

- **[FALSE-GREEN, must address] SVG coordinate masking** —
  `equivalence_normalize_html.py` sets every SVG coord attr (`d/x/y/points/width/…`)
  to `'#'` to hide a *known Go* float→int truncation bug (reference PR #136).
  For ipê this is a hole: a genuine ipê coordinate regression is masked → empty
  diff → false green. ipê is **not** bound to reproduce Go's truncation bug.
  **Ruling:** for ipê, compare SVG coords against a **stored-correct snapshot**
  (snapshot-oracle mode) rather than masking, OR keep masking only while a
  fixture explicitly documents the Go bug and add a separate value-level
  assertion. Do not ship the blanket mask as the permanent gate.
- **[FALSE-GREEN, acceptable + covered] event-encoding collapse** — Go
  `sky-click="Dec"` (Msg name) vs ipê `sky-click="click"`+`data-sky-on` collapse
  to `data-events="click"` (the SET of event types). This drops the *Msg
  identity*, so a handler wired to the wrong Msg is invisible here. Legitimate
  (wire encodings differ), but the gap MUST stay covered by the onClick
  round-trip e2e (`live_e2e.rs` already exercises POST `/_sky/event`). Note the
  dependency; do not remove that e2e.
- **[FALSE-GREEN, note] pseudo/mq/anim/tr style-delivery drop** — both Go
  `<style>` child and ipê `data-sky-*-rules` attrs are dropped, so the *content*
  of a pseudo/media/animation rule is not diffed. A rule that renders wrong but
  is delivered is invisible. Acceptable for a structural render gate; flag that
  CSS-rule *content* correctness needs a targeted fixture, not this normalizer.
- **[FALSE-RED risk] charref vs entityref** — `convert_charrefs=False` +
  `esc_attr` re-escaping: if one backend emits `&#34;` and the other `&quot;`
  for the same character, HTMLParser routes them through different handlers and
  they diff. Behaviourally identical → false red. **Ruling:** normalize numeric
  and named char references to a single canonical form before comparison.
- **[FALSE-RED, sanctioned] json HTML-escape** — Go `encoding/json` escapes
  `<>&`+U+2028/9 by default; serde does not. Inside a `<script
  type="ld+json">` or a data attribute, the html normalizer shows this as a real
  DIFFER. If ipê adopts the sanctioned non-escape, that DIFFER is a Rule-2
  false-red against a Go oracle: it must be recorded as `oracle_divergence`, not
  "fixed" to match Go. (This is the same class as the float threshold.)
- **[CLEAN] attribute-order sort** — both sides sort alphabetically; the
  arbitrary map/HashMap order is correctly neutralized. No false red.
- **[CLEAN] tui blank-cell style collapse** — `equivalence_tui_grid.py` collapses
  fg/attrs on blank cells (invisible on a space) to default; correct — compares
  what is seen. `pyte` absence → SKIP (correct skip).

---

## 4. #47/F7 acceptance gate — fixtures 69/70/71

The CSS/HTML-attr injection-safe-emit work (#47/F7, spec
`docs/architecture/css-attr-injection-safe-emit.md`) is gated by three render
fixtures that can land as **stored-HTML snapshots with no Go oracle** (they
assert *ipê's* output is safe, an absolute property, not a Go-relative one):

- **`70-style-injection`** — the security gate. Asserts a hostile
  `Std.Css`/`Raw`/`property` value cannot break out of the `<style>` body
  (`</style><script>`, `expression()`, `js:`/`data:` URL neutralized). This is
  the fixture that catches the open `Std.Html.styleNode → HRaw <style>` hole
  (styleNode emits the body verbatim; the `Std.Ui` path is protected, styleNode
  is not).
- **`69-html-render-parity`** — asserts the `#sky-root` structural render +
  attribute escaping (`SafeAttrName` forbids `on*`/`srcdoc`; `escape_attr`
  `"`→`&#34;`).
- **`71-style-merge`** — asserts inline-style merge ordering/dedup stays
  deterministic and escaped.

Port these as SECURITY snapshots **before** the oracle stands up; they are the
executable acceptance criteria for #47/F7. (Note: the *other* `71-*` fixture,
`71-panic-classifier`, is a soundness gate, not part of #47.)

`18-auth-signup` is an adjacent SECURITY fixture: it asserts the auth secret is
never stringified (`fmt.Sprintf("%v", secret)` forbidden). Worth porting in the
same security batch.

---

## 5. Honest first-run expectation

skyc is **pre-parity** (M-phase kernels incomplete: Set/Dict obligations,
json encode/decode, decimal/money, `Std.Ui`/style emit, live/tui/webview
backends are partially or not landed). Authoring all 65 fixtures now yields a
corpus that is **mostly RED on the first run — by design.** The corpus is the
**target of the port, not a today-pass gate.**

Expected first-run shape:

- **Likely GREEN early:** the pure-M1–M4 codegen/CLI fixtures — `23-char`,
  `53-cons-pattern-tuple`, `63-int-overflow-wrap`, `60-errortostring-string`,
  `44-curried-return`, `57-record-alias-any`, `51-let-lambda-param-infer`,
  `codegen-*`.
- **Likely RED until the owning phase lands:** anything touching Set/Dict
  determinism, json escaping, decimal/money rounding (M4d/M4g/M4h/numeric),
  `Std.Ui`/style emit and the live/tui/server/webview/db/auth backends
  (M5/M6/F7 and later). `kernel-parity-probe*`, `alloc-stress`, the `27–35`
  live set, `38/41` tui, `39-webview`, `17/18/19/20/66/68` db/auth all fall
  here.

Track the corpus as a **burndown**: each fixture flips GREEN as its phase
completes; the DONE gate (memory: endgame example sweep) is all-green across the
non-FFI corpus. Do not gate CI red-fails on the corpus until a phase claims a
fixture — until then it is an aspirational manifest, run with
`IPE_SWEEP_NO_EQUIV=1` as the phase-1 default.

---

## 6. Rulings summary

- **VERDICT: proceed** with authoring the 65-fixture non-FFI corpus in Tier
  0→4 order, silent classes first.
- **HARDEN (Rule 1):** replace the blanket SVG-coord mask with a stored-correct
  snapshot comparison for ipê; keep the event/style-delivery collapses only with
  their compensating e2e / targeted fixtures noted.
- **HARDEN (Rule 2):** add CRLF folding to the stdout + html paths (not the tui
  path); canonicalize char-references; tag the float-threshold and
  json-HTML-escape divergences as `oracle_divergence`, never match-to-Go.
- **BLOCK:** do not flip `IPE_SWEEP_NO_EQUIV` off in CI as a hard gate until a
  Go oracle (live or snapshot) exists AND the owning phase claims each fixture —
  otherwise a pre-parity red poisons the gate.
