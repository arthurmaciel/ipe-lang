# WASM / browser target for Ipê

> **Status:** implemented. This document is the design of record for the
> client-WASM target and its security model; the shipped code lives under
> `src/runtime/rust/src/wasm/`, `src/compiler/backend/rust/` (emission branch),
> and `src/compiler/canon/src/target_gate.rs` (the `WasmClient` effect gate),
> with browser examples under `examples/wasm-*`. (`wasm-backend.md` proposes a
> distinct, still-rejected direct IR→WASM backend that reuses this document's
> effect gate.)
> **Scope:** running Ipê in the browser — client-side apps (SPA + Ipe.Live
> hydration) and an online playground.
> **Principle ordering (binding for every decision below):** security >
> correctness > soundness > efficiency > completeness > readability.
> **Two rules applied at the target boundary:** *parse, don't validate* and
> *make invalid states unrepresentable*. Concretely: a server-only effect must
> be **unrepresentable** — not linted — in a client-WASM module, so no secret
> or credential can ever compile into a public bundle.

This spec is the synthesis of a three-reasoner design panel and two rounds of
cross-critique. Where the panel converged, the decision is stated flatly. Where
a fork survived critique, it is listed under **Open decisions** for the user.

---

## Executive summary

1. **Two targets, staged.** *(A)* compile Ipê **programs** to WASM so a TEA app
   runs client-side; *(B)* compile the **toolchain** to WASM for an in-browser
   playground. **Ship A first**; B is A plus a compile stage.
2. **Q1 — priority:** A first. B splits into **B1** (server-compile-then-ship-
   WASM, nearly free once A exists) and **B2** (interpreter-in-WASM, deferred to
   the interpreter tier). A, B1, B2 share the front-end, the ported runtime, the
   Cmd/Sub browser bridge, and the effect gate.
3. **Q2 — route:** Ipê → Rust → `wasm32-unknown-unknown`, **reusing the existing
   Rust backend**. No direct Ipê→WASM backend (it would fork emission and the
   no-panic contract, and abandon the ported runtime).
4. **Q3 — capability matrix:** exhaustive table below. Pure + fallible-pure tiers
   compile wholesale; the effects tier compiles iff a browser analogue exists,
   otherwise it is unrepresentable client-side.
5. **Q4 — client runtime:** reuse `Html<M>` + `diff() -> Vec<Patch>` + `render`;
   a new WASM sink applies the **same** `Vec<Patch>` to the real DOM via typed
   `web-sys`, with **delegated event listeners** and one update+diff+patch per
   `requestAnimationFrame`.
6. **Q5 — security:** a **three-layer gate** — (1) target-keyed kernel registry
   (server effects have no denotation at canonicalisation), (2) module partition
   + reachability closure, (3) the emitted `Cargo.toml` dep-floor. Reject the
   `Task`-capability-row as a v1 mechanism.
7. **Q6 — Ipe.Live:** **both** pure-SPA and isomorphic SSR + hydration. Pure-SPA
   ships MVP; hydration is design-locked, built MVP+1. Opt-in via a `[wasm]`
   `sky.toml` section + `ipe build --target wasm`.
8. **Q7 — playground:** B1 (server-compile) now; B2 (front-end + IR interpreter
   in WASM) with the interpreter tier. `rustc`-in-WASM rejected. Trust gated on
   the H12 interpreter≡AOT differential-conformance invariant.
9. **CSP:** WASM is eval-free; the app runs under `script-src 'self'
   'wasm-unsafe-eval'` with **no** JS `'unsafe-eval'` — strictly tighter than a
   JS SPA. The no-`data-sky-eval` invariant holds; `__skyReviveScripts` is not
   ported.
10. **No-panic ⇒ no-trap (with one honest residual):** the no-runtime-panic
    contract extends to WASM traps; guarded *kernels* keep the kernel trap class
    unreachable, but **stack exhaustion from non-TCO list ops is a reachable
    structural residual** (smaller WASM stack) — caught, not prevented. Because
    `panic = "abort"` poisons the instance, the posture is **log-and-die**:
    `console_error_panic_hook` emits a classified diagnostic (`console.error` +
    errId, incl. `StackOverflow`) before the instance dies — never a *silent*
    white-screen, but not a recovered UI either.
11. **Top 5 CANNOT-compile:** `File.*`; `Process.run`; `Ipe.Db.*` (server SQL +
    connection strings); `Auth.signToken` / HS256 `verifyToken` / `register` /
    `login` / `setRole`; `Ipe.Http.Server.*` + Ipe.Live session stores. Also
    `System.getenv`/`exit`, `Email.send`.
12. **Open decisions** (§Open): reqwest-wasm vs raw `web-sys` fetch; JWT/bcrypt
    WASM crate maturity; the IndexedDB substitute module shape (v2); the
    `Task`-capability-row endgame.

---

## Q1 — What "run Ipê in the browser" means; priority; shared machinery

**Decision.** Two genuinely distinct targets, and we build **both**, staged:

- **Target A — compile Ipê PROGRAMS to WASM.** A TEA app (`init`/`update`/
  `view`) runs client-side and drives the real DOM: a SPA, or the client half of
  a Ipe.Live SSR + hydration page. This is the product — the "real online
  experience." **Priority 1.** ~85% reuse (front-end unchanged; runtime
  VNode/diff/render already ported; the backend already emits a cargo crate that
  targets `wasm32` cleanly for WASM-compatible deps).
- **Target B — compile the COMPILER/toolchain to WASM** for an in-browser
  playground (Elm-style). **Priority 2**, split in two:
  - **B1 — server-compile-then-ship-WASM.** A playground backend runs
    `ipe build --target wasm` and returns the bundle; the browser runs it. Pure
    reuse of Target A. Ships alongside A.
  - **B2 — fully-client compile via the interpreter tier.** Front-end + IR
    interpreter compiled to WASM; instant, offline. Lands with the interpreter
    tier (§Q7).

*Rationale.* A is the substrate B stands on — the playground still needs A's
client runtime to *run* whatever it compiles. Shipping A first directly de-risks
B.

**Shared machinery across A and B:** the whole front-end (`ipe_parse →
ipe_canon → ipe_types → ipe_ir`); the ported runtime (`Html<M>`, `diff`,
`render`, `tea`); the **target-keyed kernel gate** (§Q5); and the **Cmd/Sub →
browser bridge** (§Q4). B2's interpreter running a TEA app in the browser needs
the *same* browser-substitute effect bridge and the *same* gate that A needs.

---

## Q2 — Compilation route for programs

**Decision.** Ipê → Rust → `wasm32-unknown-unknown`, **reusing
`ipe_backend_rust` (`src/compiler/backend/rust`) verbatim**. Reject a direct
Ipê→WASM backend.

*Rationale (principle-ordered).* Reuse maximises correctness/soundness: one
emission path, one runtime, one no-panic contract, one security gate to audit. A
direct backend would double the golden-oracle surface, fork the no-panic
contract, and throw away the ported runtime (VNode/diff/render, Decimal, Json,
Regex, chrono) — a completeness *and* soundness regression for control we do not
need. WASM is a **cargo target of the single Rust backend, not a second codegen
path.**

Target triple: **`wasm32-unknown-unknown` + wasm-bindgen** (browser/DOM). **Not**
`wasm32-wasip1` — that is the future *edge/server* WASM target with no DOM
(out of scope here; see Open decisions).

### Emitted-crate changes under `--target wasm`

The emitter (`emit_expr.rs` / `emit_types.rs`) is **target-agnostic** — Ipê
values lower to the same Rust. Only two files branch on target:

**`project.rs` — a WASM manifest template** (a fourth manifest alongside
base/db/server, produced by the same anchor-substitution the backend already
uses). It is the point at which the "computed-from-used-kernels" manifest work
(previously scheduled independently) becomes load-bearing, because the fixed
golden manifest is native-hostile:

```toml
[lib]
crate-type = ["cdylib"]              # WASM module, not [[bin]]

[dependencies]
wasm-bindgen         = "0.2"
wasm-bindgen-futures = "0.4"         # spawn_local — replaces the tokio runtime
js-sys               = "0.3"
web-sys              = { version = "0.3", features = [   # explicit allowlist
  "Window","Document","Element","HtmlElement","Node","Text",
  "Event","EventTarget","console","Request","RequestInit",
  "Response","Headers","Crypto","Performance","WebSocket" ] }
gloo-timers          = { version = "0.3", features = ["futures"] }  # Sub.every / Time.sleep
getrandom            = { version = "0.2", features = ["js"] }        # entropy → crypto.getRandomValues
console_error_panic_hook = "0.1"     # classify residual traps

[profile.release]
opt-level = "z"
lto       = true
panic     = "abort"
strip     = true
```

**Dropped vs the native/server manifest (this omission IS part of the security
gate, §Q5):** `tokio` (no OS threads on `wasm32-unknown-unknown`), `axum`,
`tower-http`, `hyper`, `sqlx`/DB drivers, native-TLS (`rustls-tls`). The WASM
crate declares **no** `server`/`db`/`live`(SSE)/`webview`/`tui` feature; those
are mutually exclusive with `wasm`. Cargo fails on an undeclared feature — a
build-time floor beneath the language gate.

**Entry shape** (`preamble.rs`, WASM branch). The native `fn main()` + tokio
epilogue is replaced by a wasm-bindgen entry:

```rust
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    ipe_runtime_rust::wasm::mount("#app", Main_init, Main_update, Main_view, Main_subscriptions);
}
```

Hydration mode additionally exports `#[wasm_bindgen] pub fn hydrate(model_json: &str)`
(§Q6). `hydrate` **parses `model_json` into `Result<HydrationState, _>` and falls
back to a clean client `init` on `Err`** (malformed / truncated / tampered
island) — never `unwrap`/`expect`/trap on the public, user-editable island (§Q6
"Fault-tolerant hydrate"). Async: `Cmd.perform` uses
`wasm_bindgen_futures::spawn_local` instead of
a tokio task — the `IpeCmd::Perform` boxed thunk in `tea.rs` is already
runtime-agnostic, so only the *driver* changes.

**Toolchain orchestration:** `ipe build --target wasm` runs
`cargo build --target wasm32-unknown-unknown --release`, then the `wasm-bindgen`
CLI to emit `.wasm` + JS glue + a minimal `index.html` shell, then `wasm-opt -Oz`
for size. This mirrors the existing cgo auto-detect the driver uses for
Ipe.Webview.

**Size posture.** DCE prunes Ipê-side dead code; whole-program DCE + `opt-level=z`
+ `lto` + `wasm-opt -Oz` cap the Rust dep graph. `chrono-tz` bundles the full
IANA database — feature-gate it so apps not using `Ipe.Time` zones do not pay
for it. A client-bundle size budget gate is an Open decision.

---

## Q3 — Capability matrix (the core deliverable)

Cited against the **effect-boundary tiers**: Pure `a` / Fallible-pure
`Result e a` · `Maybe a` / Effects `Task Error a` / Diverging.

Legend:
- **COMPILES** — the same Rust runs in WASM unchanged.
- **SUBSTITUTE** — the `Task Error a` / `Result` shape is preserved; a client
  kernel swaps the implementation to a browser API.
- **DOES-NOT** — unrepresentable in a client target: the kernel is absent from
  the client registry, so naming it is an unbound-name error at canonicalisation
  (§Q5). Reason is either "no browser analogue" or "would ship a server secret."

### Pure tier — bare `a` (all COMPILE; no host dependency)

| Stdlib area | Status | Browser substitute | Notes |
|---|---|---|---|
| Basics, List, Dict, Set, Maybe, Result | COMPILES | — | Pure. Non-TCO list ops (`map`/`filter`/`foldr`/`concat`/`take`/`zip`/…) recurse on the stack; the WASM stack is smaller, so the Limitation-#8 "prefer `foldl` past ~200k elements" guidance **tightens** client-side. |
| String, Char, Regex, Path | COMPILES | — | `regex` crate → WASM clean. |
| Math | COMPILES | — | `f64` intrinsics. |
| Crypto **pure** (sha256/512/1, md5, hmac*, rsaSha256Verify, constantTimeEqual) | COMPILES | — | Pure hashing/verify crates → WASM. Distinct from secret token-signing (`Auth.signToken`, DOES-NOT). |
| Crypto symmetric AEAD (aesGcm*/chacha20*/`*KeyFromPassword`) `Result Error String` | COMPILES | — | RustCrypto → WASM; entropy via `getrandom(js)`. **Key is caller-supplied at runtime — no server secret baked in.** |
| Bytes, Encoding (base64/url/hex) | COMPILES | — | Pure. |
| Json (Encode/Decode/Pipeline) | COMPILES | — | `serde_json` → WASM. |
| Decimal, Money | COMPILES | — | `rust_decimal` → WASM. Money math client-side ships no secret. |
| Csv (`parse`/`encode` from String) | COMPILES | — | `csv` crate pure. `parseStreamFromFile` → DOES-NOT (file read). |
| Config (`decodeToml`/`Yaml`/`Json` from String) | COMPILES | — | Pure decoders. `loadFromFile` → DOES-NOT. |
| Compression `gzip`/`gunzip` | COMPILES | — | `flate2` + `miniz_oxide` (pure Rust). |
| Compression `zstdDecompress` | SUBSTITUTE | `ruzstd` (pure-Rust, decode-only) | — |
| Compression `zstdCompress` | **DOES-NOT (v1)** | — | `zstd-sys` is a C dep; no pure-Rust encoder. Asymmetric with decode — flag it. |
| Jwt.encode/decode (verify path) | COMPILES* | — | Pure over a supplied key. *Crate-maturity caveat: `jsonwebtoken`/`ring` have historically thin `wasm32-unknown-unknown` support; may need `jwt-simple` or a hand-rolled HS256 substitute — Open decision. **Sign only with a caller-supplied key, never a server secret.** |
| Uuid v4/v7 | SUBSTITUTE | `crypto.getRandomValues` (getrandom `js`) | `parse` is pure COMPILES. |
| ToString, Pure | COMPILES | — | Pure aliases. |

### Fallible-pure tier — `Result e a` / `Maybe a` (all COMPILE)

| Stdlib area | Status | Notes |
|---|---|---|
| String.toInt, JSON decoders, Encoding.base64Decode | COMPILES | Pure. |
| `Auth.hashPassword` / `verifyPassword` / `passwordStrength` | **COMPILES** | bcrypt compiles to WASM (slow but bounded) and carries **no secret**. Gating these was over-conservative — whether to hash client-side is an app-design question, not a secret-leak question. |
| `Auth.verifyToken` (RS256 / public-key path) | COMPILES | Exposed as a distinct `Jwt.verifyRs256 pubKey` kernel; public keys are safe to ship. The secret-bearing HS256 path stays DOES-NOT. |

### Effects tier — `Task Error a`

| Effect / module | Status | Browser substitute | Notes |
|---|---|---|---|
| Task (succeed/fail/map/andThen/sequence/parallel/retryWith) | COMPILES (re-targeted) | — | `parallel` = `join_all` over `spawn_local`. |
| Cmd (none/batch/perform/publish/publishNoEcho), Sub (none/every/batch/subscribeTopic), PubSub | SUBSTITUTE | §Q4 mapping | `perform`→`spawn_local`; `every`→`gloo-timers::Interval`; pub/sub → in-tab broker (optionally `BroadcastChannel` cross-tab). Echo/no-echo preserved; cross-*process* federation DOES-NOT. |
| Http.get/post/request | SUBSTITUTE | `fetch` (reqwest-wasm primary; raw `web-sys` fetch fallback — Open) | Full `HttpResponse{status,body,headers}`. **New client failure modes** (CORS, forbidden headers, timeout via `AbortController`) surface as `Task.fail (Error …)` — never a trap, never `Task String`. |
| Time.now / unixMillis | SUBSTITUTE | `Date.now()` / `performance.now()` | — |
| Time.sleep / every | SUBSTITUTE | `setTimeout` / `setInterval` (gloo-timers) | — |
| Time.format* / addMillis / diffMillis; Ipe.Time (chrono/chrono-tz) | COMPILES | — | tz data bundled; feature-gate `chrono-tz` (§Q2). |
| Random.* (entropy) | SUBSTITUTE | `crypto.getRandomValues` | Seeded splitmix64 variants are pure COMPILES. |
| Crypto.randomBytes / randomToken | SUBSTITUTE | `crypto.getRandomValues` | — |
| Log.* | SUBSTITUTE | `console.{debug,info,warn,error}` | Structured fields → console object. |
| Io.writeStdout / writeStderr | SUBSTITUTE | `console.log` / `console.error` | — |
| Io.readLine | **DOES-NOT** | — | No blocking stdin. Diagnostic points at `Ui.input` (a *different* API — not a silent substitute). |
| Ipe.Live.Head (title/meta) | COMPILES | `document.title` / meta via web-sys (SPA); server-owned in hydrate mode | — |
| Cache (Ipe.Cache LRU+TTL) | COMPILES | — | In-memory; bounded by tab memory. |
| Trace (span/event/attr) | SUBSTITUTE (degraded) | `console.group`; optional `fetch`/`sendBeacon` OTLP push | Hub-exporter-with-token DOES-NOT (bearer is a secret). |
| WebSocket **client** (`Ipe.WebSocket`) | SUBSTITUTE | `web_sys::WebSocket` | Natural client↔server channel; can replace SSE for server-pushed Msgs. |
| Http.Stream (client upstream read as Sub) | SUBSTITUTE | `fetch` + `ReadableStream` | Post-MVP-optional. |
| **File.*** (read/write/exists/mkdir/readDir/temp/copy/rename) | **DOES-NOT** | — | No filesystem. v2 *may* add an opt-in `Ipe.Browser.File` over the File System Access API / OPFS — a **distinct** module, never `Ipe.File`. |
| **Process.run** | **DOES-NOT** | — | No subprocess. |
| **System.exit** (Diverging) | **DOES-NOT** | — | No process to exit; would be a trap. |
| **System.getenv / getenvInt / getenvBool / setenv / unsetenv / args / cwd / loadEnv** | **DOES-NOT** | — | Reading process env is server-only **and** the sharpest secret-capture vector. Public build-time config flows ONLY through a distinct allowlisted kernel (§Q5), never `getenv`. |
| **Ipe.Db.*** (open/connect/exec/query/queryDecode/migrate/withTransaction, all stores) | **DOES-NOT** | — | SQL drivers are server-only; connection strings are secrets. Substitute (if ever) is a distinct opt-in `Ipe.Browser.Store` over IndexedDB — **never** a `Ipe.Db` alias (that would make the SQL surface representable client-side, re-opening the gate). |
| **Ipe.Live session stores** (memory/sqlite/redis/postgres/firestore) | **DOES-NOT** | — | Server-side persistence. A hydrated client holds only an opaque server-issued token (§Q6). |
| **Ipe.Auth** register/login/setRole | **DOES-NOT** | — | Server user tables + secret. |
| **Auth.signToken / signTokenWithClaims** | **DOES-NOT** | — | **Crown-jewel gate.** Signing needs `IPE_AUTH_TOKEN_SECRET`; the kernel is absent, so the secret has no client consumer. |
| **Auth.verifyToken** (HS256 shared-secret path) | **DOES-NOT** | — | Same secret. RS256/public-key verify is the separate COMPILES path above. |
| **Ipe.Email.send** | **DOES-NOT** | — | Provider API keys are secrets; send via a server endpoint over `Http.post`. |
| **Ipe.Http.Server** (+ Stream, Middleware, RateLimit, WebSocket **server**) | **DOES-NOT** | — | A browser tab is not a server. (Note the split: WebSocket *client* SUBSTITUTE; WebSocket *server* DOES-NOT.) |
| **Ipe.Live.Console / consoleAuth** | **DOES-NOT** | — | Server-mounted dev console. |

### TEA + Ipe.Ui — COMPILES (the headline)

`Ipe.Ui` (`row`/`column`/`el`/`grid`/`text`/`button`/`input`/`form`, the
`Background`/`Border`/`Font`/`Region`/`Input` sub-modules, pseudo-classes, media
queries, transitions/animations) renders through the ported `render.rs` to
`Html<M>`; the client driver (§Q4) turns that into real DOM via `web-sys`.
Inline-style + `<style data-sky-*>` injection compiles. Ipe.Tui and Ipe.Webview
backends are irrelevant to WASM. The minimal first-target subset is "everything
`render.rs` already emits"; no primitive is dropped a priori (unlike Ipe.Tui,
which drops gradients/fine letter-spacing).

**Effect-tier summary.** Pure + fallible-pure → 100% compile. Effects → compile
iff a browser analogue exists (fetch/timers/console/WebSocket/crypto RNG/DOM),
else DOES-NOT. The DOES-NOT set is precisely {no browser analogue} ∪ {would ship
a server secret}, and §Q5 makes every DOES-NOT unrepresentable at compile time.

---

## Q4 — Client TEA runtime + Cmd/Sub mapping

**Decision.** Reuse `src/runtime/rust/src/ui/render.rs` (`Element<M> → Html<M>`, with its existing
`SafeCssPropertyName`/`sanitise_css_url`/`SafeAttrName` sanitisers) and
`live/diff.rs` (`diff<M>(old, new) -> Vec<Patch>`) **unchanged**. Swap only the
*sink*: today `Vec<Patch>` is serialised to JSON → SSE → `client.js`; on the
client the transport becomes an **in-process function call** applying the *same*
`Patch` structs to the real DOM via `web-sys`. This gives byte-for-byte
behavioural parity between the SSE wire and the client driver (one diff
algorithm, two consumers). `client.js` is **not** shipped; its patch-apply logic
is ported into Rust/`web-sys`.

New module `src/runtime/rust/src/wasm/` (feature `wasm-client`,
`cfg(target_arch = "wasm32")`). The scheduler holds
`Rc<RefCell<{ model: M, tree: Html<M>, queue: VecDeque<M> }>>`.

**Loop** (reused pieces in *italics*):

1. *`init(req)`* → `(Model, Cmd)`. In the browser `req` is synthesised from
   `location` + `document.cookie` — the v0.16.7 row-poly `req` shape
   (path/query/params/method/headers/cookies) reconstructed client-side.
   Absent-field apps are unaffected.
2. First render: *`view`* → *`Html<M>`* → *`assign_sky_ids`*, then a new
   `wasm::mount(&html, root)` walks the tree once and builds real DOM nodes
   (`create_element`/`create_text_node`/`set_attribute`), stamping the same
   sky-ids. (In hydrate mode the DOM already exists — attach listeners only,
   §Q6.)
3. **Event delivery — delegated listeners.** One delegated listener per event
   type on the root, keyed by `data-sky-hid` (the same handler-id scheme
   `dispatch.rs` already defines). *This is the chosen design over per-node
   closures:* a node inserted by a later `Patch` needs no re-wiring (delegation
   sees it automatically), and there are no per-node `Closure`s to clean up on
   `remove` — closing the manual-listener-lifecycle leak class that per-node
   wiring re-introduces on WASM. Wire-event arg shapes (checkbox→Bool,
   number/range→Float, text/textarea/select→String, form→typed record via
   *`form.rs`*, key→String) are decoded from the DOM `Event` exactly as the SSE
   path does.
4. Update cycle: `Msg` → *`update`* → `(Model', Cmd)` → *`diff(old_tree,
   new_view)`* → *`Vec<Patch>`* → `wasm::apply(&patches)`. `apply` uses typed
   web-sys calls: `Patch.text` → `set_text_content`; `Patch.attrs` →
   `set_attribute` / `remove_attribute` (empty value = remove, per the existing
   Go convention); `Patch.html` (subtree replace) → `set_inner_html` **only**,
   fed by the sanitiser-gated `render_html` output; `Patch.remove` → `.remove()`.
   Replace the retained tree.
5. **Frame batching.** Coalesce Msgs enqueued within a tick and run **one**
   update+view+diff+patch per `requestAnimationFrame` — the browser analogue of
   the server's SSE seq-ordered batching; avoids layout thrash and handles a Cmd
   resolving mid-tick without a race.

### Cmd / Sub → browser mapping

`IpeCmd<M>` / `IpeSub<M>` in `tea.rs` are already generic over `M` (not `any`),
so only the driver is new.

| Ipê | Browser mechanism |
|---|---|
| `Cmd.none` / `Cmd.batch` | no-op / iterate (microtask fan-out) |
| `Cmd.perform task toMsg` | `spawn_local(async { let m = toMsg(task.await); dispatch(m) })` |
| `Http.get`/`post` inside a Cmd | `fetch` → `JsFuture` inside that `spawn_local` |
| `Cmd.publish` / `PubSub.publish` (+ NoEcho) | in-tab broker (`Rc<RefCell<HashMap<topic, Vec<Sub>>>>`); optional `BroadcastChannel` cross-tab; echo/no-echo preserved |
| `Sub.every ms` / `Time.every` | `gloo-timers::Interval` → `dispatch(tick)`; teardown+respawn via the reused `SubManager` |
| `Time.sleep` | `gloo-timers::Timeout` future |
| `Sub.subscribeTopic` | register in the in-tab broker |
| stream Subs (`EventSource` / `WebSocket`) | `web_sys::EventSource` / `web_sys::WebSocket` → `dispatch` |
| event listeners (`onClick`/`onInput`/`onSubmit`/`onKeyDown`) | delegated root listener; closure decodes wire-arg shape → `Msg` |

**No-panic ⇒ no-trap (with one honest residual class).** A WASM `unreachable`
trap is the panic analogue (worse than a native panic — it poisons the module).
The contract "no runtime panic from well-typed Ipê" becomes "no WASM trap from
well-typed Ipê." For the **kernel-originated** trap class it holds as on native:
every trap-capable kernel (`IntDiv`/`Rem` checked, `Coerce` fallible, index
checks, `render.rs` `saturating_add`) returns `Result`/`Task.fail`, so those
paths compile to WASM with traps unreachable.

**Residual trap class — stack exhaustion (NOT closable by the kernel argument).**
The kernel-returns-`Result` argument does **not** cover stack overflow, which is
**structural, not a kernel**: the non-TCO list ops (`map`/`filter`/`foldr`/
`concat`/`take`/`zip`/`indexedMap`/`Maybe.combine`/`Result.combine` — Limitation
#8) recurse on the call stack, and the `wasm32` stack is **smaller** than the
native one, so a deep-but-well-typed list drives a genuine WASM `unreachable`
trap from correct Ipê. This is a **reachable residual trap**, and this spec
classifies it as such rather than claiming §Q4's "traps unreachable because every
trap-capable kernel returns `Result`" (which is true only for the kernel class).
It is caught — not prevented — by the panic-hook path below and surfaced as a
classified `StackOverflow` diagnostic; the *guidance* to prefer `foldl`-style
accumulators tightens client-side (§Q3), and rewriting these ops tail-recursively
is the only true close. Until then it is a stated, caught residual, not a closed
hole.

**Defence-in-depth on residual traps — log-and-die, not recover.** `panic =
"abort"` means an escaped trap **aborts the WASM instance**; the instance is
poisoned and **cannot be resumed** — there is no surviving scheduler to "render an
error banner," and any claim to do so is infeasible. What *does* survive is
`console_error_panic_hook`, which runs at abort time: it classifies the trap
(including `StackOverflow`) into the same taxonomy as `rt.LogPanicAndExit`
(DivisionByZero/TypeMismatch/CoerceFailure/StackOverflow/…) and emits a
structured `console.error` with a 4-byte errId **before** the instance dies. This
is **log-and-die**, the honest analogue of the native synchronous-panic gate — a
classified diagnostic in the console, not a recovered UI. (An app wanting true
recovery must isolate the risky computation in a **separate Worker** instance
whose abort the main instance observes and reports — an app-level pattern, out of
scope for the runtime floor.) The floor guarantee is: never a *silent*
white-screen — every abort leaves a classified errId in the console first.

---

## Q5 — Server/client security boundary (guardian-critical)

**Threat model.** The client `.wasm` bundle is fully public: shipped to every
visitor, trivially `wasm2wat`-inspectable, strings extractable.
`IPE_AUTH_TOKEN_SECRET`, DB connection strings, `Auth` signing internals, any
provider key **MUST NEVER** compile into a client-targeted bundle. This is a
**compile-time** guarantee, not a lint — *make invalid states unrepresentable at
the target boundary.*

**Decision.** A **three-layer gate**, the first two load-bearing, the third
defence-in-depth. Reject the `Task`-capability-row as a v1 mechanism (below).

### Layer 1 — Target-keyed kernel registry (the floor: the effect has no denotation)

The kernel registry (`src/ipe-cli/src/stdlib.rs` + the backend kernel table) is
**parameterised by `Target` (`Native` | `WasmClient`)**. The client table is an
**allowlist, default-deny**: under `--target wasm` a kernel has a client
denotation **only if it is explicitly tagged `WasmClient`-safe** in the registry
(the COMPILES + SUBSTITUTE rows of §Q3, each carrying an explicit `WasmClient`
tag). It is emphatically **not** "the native table minus the DOES-NOT rows" —
that default-allow shape would silently make any *future* server-only kernel that
nobody remembered to tag `DOES-NOT` client-representable, leaking it by omission.
Default-deny inverts the failure mode: a newly-added kernel is
**unrepresentable client-side until someone proves it safe and tags it**, so the
safe state is the one you fall into when you forget. Every DOES-NOT row of §Q3 is
therefore simply *absent from the allowlist* (no tag), and any new kernel is too
until audited. Consequence — *parse, don't validate*: a client module that so
much as **names** `Auth.signToken`, `Db.query`, `File.readFile`, `Process.run`,
`System.getenv`, `System.exit`, `Server.listen`, `Email.send`, a session store,
or any un-allowlisted kernel fails at **canonicalisation (name resolution)** with
an unbound-name-for-target error:

> `Auth.signToken` is a server-only effect and has no denotation for target
> `wasm`. It consumes `IPE_AUTH_TOKEN_SECRET`, which must never ship to a public
> browser bundle. Move it behind a server route and call it from the client via
> `Cmd.perform (Http.post "/api/…") ToMsg`.

This is strictly stronger than a runtime `Err` stub (which would still let the
*name* and adjacent secret-handling code exist) and stronger than a
post-typecheck lint (which a later stage could "forget"): the server effect never
becomes a well-formed client-IR node, so the secret literally has no consumer
that could compile. The teacher-style explain page names the effect and the fix.

### Layer 2 — Module partition + reachability closure (compositional, for shared code)

For SSR + hydration where `view`/`update` are shared, modules carry a capability
classification: `server` | `client` | `shared` (default `shared` for pure/UI
modules; `server` for anything importing a DOES-NOT module). Rules:

- a `client`/`shared` module may import only `client`/`shared` modules;
- a `server` module may import anything;
- the client build computes the **reachability closure** from the client entry
  point; if the closure transitively touches a `server` module or a DOES-NOT
  kernel, it is a **hard error naming the exact import path** (e.g. `Main(client)
  → View(shared) → Data(server: imports Ipe.Db)`).

This supplies the *transitive/compositional* guarantee — a helper that
transitively uses `Db` cannot be reached from a client-rooted computation — and
means a `shared` module is checked against the **intersection** of both targets'
surfaces, so shared `view` code provably compiles to both server (SSR) and client
(hydration). It also *forces the correct architecture*: an `update` that branches
into `Cmd.perform (Db.query …)` fails the client build, pushing DB/Auth work
behind an `Http` boundary.

### Layer 3 — Cargo dependency floor (defence-in-depth)

The WASM `Cargo.toml` (§Q2) omits `tokio`/`axum`/`hyper`/`sqlx`/native-TLS and
declares no `server`/`db` feature. Even if Layers 1–2 had a hole, there is no
DB/TLS/net crate *linked to carry* a credential, and `cargo` fails on an
undeclared feature. Three independent layers must all fail before a secret could
ship.

### Config: default-deny allowlist (+ layered secret denylist)

`System.getenv` is DOES-NOT (Layer 1) — there is no client env-read. Public
build-time config flows **only** through a distinct kernel (`Ipe.Env.public`)
reading names explicitly enumerated in a `[wasm].publicEnv` allowlist. The
allowlist is **authoritative and default-deny** (a denylist alone misses
`STRIPE_SK`, `MY_PRIVATE_THING`, any unanticipated name). Layered on top: a
secret-name **denylist** (`*_SECRET`, `*_TOKEN`, `*_KEY`, `*_PASSWORD`,
`DATABASE_URL`, and the internal `IPE_*` namespace) — an allowlisted name
matching it is a **build error**, forcing the author to confirm. So
`IPE_AUTH_TOKEN_SECRET` can be neither read (no `getenv` kernel) nor allowlisted
(schema rejects `IPE_*`).

### Secret-specific foreclosures (enumerated)

| Hazard | Foreclosure |
|---|---|
| `IPE_AUTH_TOKEN_SECRET` | Double-gated: `System.getenv` and `Auth.signToken`/HS256-`verifyToken` are both DOES-NOT. No reader, no consumer. A hardcoded secret String is inert (no signing kernel to consume it). Cannot be allowlisted. |
| DB credentials / connection strings | `Ipe.Db.*` + `System.getenv` DOES-NOT → no consumer. IndexedDB (v2) is a separate credential-free module. |
| JWT signing internals | `Auth.signToken`/HS256-`verifyToken` DOES-NOT; only public-key `Jwt.verifyRs256 pubKey` is representable client-side. |
| Email / provider API keys | `Ipe.Email` DOES-NOT. |
| Silent env bake-in | `publicEnv` allowlist (default-deny) + secret-name denylist. |
| Server-effect-in-client | Layer-1 unbound-name at canon or Layer-2 reachability-closure error — never a runtime stub. |

### WASM ↔ JS/DOM boundary + FFI + CSP

- **No arbitrary FFI in WASM.** The `ipe add github.com/…` FFI subsystem is a
  native-target concept; the client target's *only* host surface is the fixed,
  audited `web-sys`/`js-sys` allowlist the runtime ships. A user cannot bind
  arbitrary JS — the FFI kernel is `server`-tagged and unrepresentable
  client-side. Hard gate.
- **DOM sinks stay gated.** The applier uses typed `set_attribute` /
  `set_text_content` / `remove_attribute`; the single `set_inner_html` sink
  consumes **only** the sanitiser-gated `render_html` output (same HTML-escape
  invariant as SSR). No `eval`, no `new Function`, no inline-handler code
  strings. **`__skyReviveScripts` is NOT ported** — the client WASM runtime never
  revives or injects `<script>` tags, closing a script-injection path. The
  `data-sky-eval` ban and `data-sky-path` typed URL-sync convention hold
  unchanged.
- **Checked casts only at the JS→web-sys boundary — `unchecked_into` is
  BANNED.** Every crossing from an untyped `JsValue` / `Node` / `EventTarget`
  into a concrete web-sys type — in `mount` (create/attach), `adopt`
  (sky-id-matched node → `Element`/`HtmlInputElement`/…), `apply` (patch target
  lookup), and event decode (`Event` → `HtmlInputElement` to read
  `.checked`/`.value`) — MUST use the **checked** `dyn_into::<T>() ->
  Result<T, _>` (or `dyn_ref`), never `unchecked_into`/`unchecked_ref`. The DOM
  is mutable by extensions, other scripts, and devtools; an unchecked cast on a
  node that is not the assumed type is **undefined behaviour in the JS glue and a
  trap in WASM**, reachable from a hostile page. A failed `dyn_into` routes to the
  **classified-diagnostic path** (§Q4 log-and-die, taxonomy `CoerceFailure`) —
  the same discipline as the native no-raw-`.(T)` / `rt.Coerce[T]` rule. A lint
  (`clippy`-level deny or a codegen post-check) rejects `unchecked_into` /
  `unchecked_ref` anywhere in `src/runtime/rust/src/wasm/`.
- **CSP is *tighter* than a JS SPA.** WASM instantiation needs the narrow
  `script-src 'wasm-unsafe-eval'` token, which permits WASM compilation but
  **not** JS `eval`. The app runs under `script-src 'self' 'wasm-unsafe-eval'`
  with **no** `'unsafe-eval'` for JS — strictly stronger than any hand-written JS
  framework. The build emits this recommended CSP header. There is no path where
  the WASM client needs `'unsafe-eval'`; it must not.

### Rejected: capability-row on `Task`

The type-theoretically purest form — refine effect kernels so `Task` carries a
capability row (`Task {caps} Error a`) and unify against a `Client`-cap set at
the WASM root, so a server cap in the row is a *unification failure = type error*
— is **rejected for v1/v2**. The compositional guarantee it offers is **already
delivered** by Layer 1 (leaf unbound-name) + Layer 2 (reachability closure); the
row adds only *error locality* and *library-signature advertisement*, neither of
which matters for an app compiler with a single target per build. Its cost is
high: the language is **HM-only, no HKT, no effect rows** (Limitation #1); a
capability row is an invasive extension to the most-used stdlib type, trading
soundness risk for locality — a bad trade under security > soundness. Filed as a
speculative endgame, gated on Ipê growing row-polymorphic effects for an
independent reason (§Open).

---

## Q6 — Ipe.Live: SSR + hydration decision

**Decision.** Support **both** pure-SPA and isomorphic SSR + hydration.
**Pure-SPA ships in the MVP** (strictly less machinery — no `adopt` path, no
Model transport, no determinism invariant). **Hydration is design-locked now,
built MVP+1** — it is the strategic headline (SSR first-paint + SEO, then client
takeover), and the effect gate (§Q5) is exactly what makes isomorphic `view`
*sound* rather than aspirational.

Three modes:

1. **Server-only (today).** SSR + SSE patches, no client WASM. Unchanged.
2. **Isomorphic SSR + WASM hydration (flagship, MVP+1).** The server renders the
   initial `Html<M>` to a string via the existing `render_html` (sky-ids
   stamped, fast first paint + SEO) and emits a **typed public-payload island**:
   `<script type="application/sky-model+json">…</script>` — **data, not code**,
   preserving the no-eval invariant.

   - **Secret boundary — the island ships a typed `HydrationState`, never the
     Model (guardian-critical).** The `<script sky-model+json>` island is on the
     **fully-public page**; it is governed by the same threat model as the WASM
     bundle (§Q5). The effect gate constrains client *effects*, **not** Model
     *contents* — so serialising the raw server-produced `Model` would leak by
     construction: a `Model` field populated server-side (`Auth.signToken`,
     `Db.query`, `System.getenv` are all legal on the native SSR producer) can
     carry `IPE_AUTH_TOKEN_SECRET`, a session secret, or a password hash straight
     into the public page. The gate never sees it, because it lives in Model
     *data*, not in a client kernel call. **Therefore the island serialises ONLY
     a distinct, app-declared `HydrationState` (a.k.a. `PublicModel`) type that
     the app explicitly projects the Model into** — `toHydrationState : Model ->
     HydrationState` — never the Model itself. `HydrationState`'s fields are
     **gated to client/shared-safe field types** (the same `shared`-surface
     intersection Layer 2 enforces on `view`): a field whose type is or transitively
     contains a server-only/secret-bearing type (an `Auth` token handle, a `Db`
     row witness, an opaque `Secret String`) is a **compile error at the
     `HydrationState` declaration**, so a secret *cannot occupy the island by
     construction* — make-invalid-states-unrepresentable, not a review checklist.
     The client `hydrate` entry is typed to receive exactly this
     `HydrationState`, closing the loop: the only value that can reach the island
     is one whose type has been proven public. **If the full field-type gate is
     deferred past MVP+1, the residual risk MUST be stated in-repo and the
     serialised type MUST still be a distinct, app-declared client/shared-safe
     type — never the raw `Model`; the mechanism above is the real fix and the
     spec commits to it.**
   - **Island escaping (XSS — mandatory).** The island body is
     serde-serialised `HydrationState` and embedded inside a `<script>` element,
     so it lives in HTML script-data context, not attribute context. A string
     field containing `</script><script>evil()</script>` — or a bare `<`, `&`, or
     the JSON-legal-but-HTML-hostile line terminators `U+2028` / `U+2029` —
     would break out of the data island into executable script context (defeating
     the entire no-eval / no-`'unsafe-eval'` posture). The island serialiser
     **MUST escape, on the emitted JSON string, before it reaches the `<script>`
     body**: U+003C `<` → `\u003c` (forecloses `</script`), U+003E `>` → `\u003e`, U+0026 `&` → `\u0026`, U+2028 → `\u2028`, U+2029 → `\u2029` (JSON numeric escapes). This is
     the **same escape class as the telemetry `json_escape` U+2028/2029 gap** —
     apply it here identically. The client parses the island back with
     `serde_json` (the `\uXXXX` escapes decode transparently), so the escaping is
     lossless. No `set_inner_html` ever touches the island; the client reads it
     via `document.querySelector(...).text_content()` and parses, never evals.
   - The client WASM boots, parses the island into `Result<HydrationState, _>`
     (see the fault-tolerant `hydrate` below), re-runs `view` over the
     reconstructed initial model to build `Html<M>`, and **adopts** the existing
     DOM by matching sky-ids (attach delegated listeners, no node rebuild), then
     takes over the loop locally. Post-hydration, updates are client-local; SSE
     becomes *optional* (only for server-pushed Msgs — `Cmd.publish` broadcasts /
     shared state).
   - **Fault-tolerant hydrate — parse, don't unwrap.** `#[wasm_bindgen] pub fn
     hydrate(model_json: &str)` MUST parse the island into
     `Result<HydrationState, serde_json::Error>` and, on `Err` (malformed,
     truncated, or **tampered** island — the page is public and user-editable via
     devtools), **fall back to a clean client `init`** exactly as the pure-SPA
     mode does, plus a logged hydration-parse warning. It must **never** `unwrap`
     / `expect` / index the parsed value — a tampered island is untrusted input
     and a trap there would be a well-typed-Ipê-reachable white-screen from
     adversarial input.
   - **Soundness — the hydration-determinism invariant.** Server and client run
     the *same* `shared` source (compiled native for SSR, WASM for client) →
     structurally identical `Html<M>` → identical sky-ids → adoption is safe.
     Enforced two ways: a **dev-mode assertion** that the client's first `diff`
     against the server-rendered DOM is empty (catch mismatch early), **and** a
     **production fallback** to full diff-and-replace + a logged
     hydration-mismatch warning (a determinism violation degrades gracefully,
     never white-screens).
   - **Native-vs-WASM determinism hazards (behind the diff-and-replace
     fallback).** "Same source, two backends" is not automatically bit-identical
     output. Enumerated hazards that can make the server string and the client
     `view` diverge — each caught by the empty-first-diff assertion (dev) and the
     diff-and-replace fallback (prod), never a silent corrupt paint:
     (a) **float formatting** — `String.fromFloat` / any `f64→String` must use
     the *same* formatter (Ryū vs libc `printf` differ on shortest-round-trip and
     on `inf`/`nan`/`-0.0`); the ported runtime already fixes one formatter, so
     native and WASM share it — assert this rather than assume it.
     (b) **map/set iteration order** — any `view` that folds a `Dict`/`Set` into
     DOM order relies on iteration order; `BTreeMap`/`BTreeSet` are deterministic
     across targets, `HashMap` is **not** (per-target/per-run seed) — the runtime
     must key UI-visible collections on the ordered variants.
     (c) **integer/usize width** — `wasm32` is a 32-bit target (`usize` = 32-bit)
     vs 64-bit native; length/offset math that overflowed silently on 64-bit can
     wrap at a *lower* bound on WASM — the checked-arithmetic kernels (§Q4) close
     this, but it is a real cross-target divergence to test.
     (d) **locale/timezone** — `Ipe.Time` formatting must pin an explicit
     locale/zone (the bundled `chrono-tz` data), never read a host locale, so SSR
     and client format identically.
   - The gate does the heavy lifting: the client closure (`update`/`view`/
     `subscriptions`) is checked, so hydration can only take over code with no
     server effects; `Db`/`Auth` work stays in server `api` handlers reached via
     `Cmd.perform (Http.post "/api/…")`.
3. **Pure client SPA (MVP).** Static HTML shell + WASM bundle; `init` runs
   client-side; all effects client-side; data via `fetch` to a separate headless
   `Ipe.Http.Server` (compiled `--target native`). Deployable to any static
   host/CDN; offline-capable PWA by nature.

**Opt-in mechanism.** A `[wasm]` section in `sky.toml` (composes with `[live]`),
plus `ipe build --target wasm`:

```toml
[wasm]
mode      = "spa"              # spa (MVP) | hydrate (MVP+1) | off (default)
entry     = "src/Client.ipe"   # client entry; its reachability closure is the bundle
mount     = "#app"             # SPA mount node
publicEnv = ["API_BASE_URL"]   # default-deny allowlist; rejects IPE_* / secret patterns
optLevel  = "z"
```

The target is a **property of the build** (whole-program, driven by the client
entry's reachability closure), not a per-module toggle — modules carry only the
`server`/`client`/`shared` classification the closure consumes. For a
dual-artifact Live+WASM app, the server binary (`--target native`, SSR + optional
SSE) and the client bundle (`--target wasm`, hydrate) compile from the **same**
`shared` `view`/`update`; the gate guarantees the client build cannot pull the
server-only half. The server auto-serves the bundle + boot `<script>` + Model
island.

**Session identity.** A hydrated client has no session store; it learns "who am
I" from the SSR-embedded `HydrationState` island / a server-issued **opaque
token** — never the signing secret. Critically, the effect gate does **not**
protect this surface (it gates client effects, not Model contents — see Blocker
above); the guarantee comes from the **typed `HydrationState` field-type gate**:
the island can only carry a value whose type is proven client/shared-safe, so
"carries only what the page already renders, never secrets" is enforced *by the
type of the island*, not by the discipline of the `view` author. An opaque
session token that authenticates subsequent `Http.post` calls is a
client/shared-safe type and may travel; the HS256 signing secret is not
representable in `HydrationState` and cannot.

---

## Q7 — Playground (Target B) + interpreter tier

**Decision.** **B1 — server-compile-then-ship-WASM — ships with Target A.** A
playground backend runs `ipe build --target wasm` on submitted source and returns
the bundle + glue; the browser runs it via the §Q4 client runtime. This is the
Rust-playground model and needs no new architecture. Latency mitigations: a warm
build server + `sccache` + a **prebuilt runtime rlib** (the runtime never changes
per submission, so only user modules recompile) + salsa-incremental front-end.
Security: the *produced* WASM is safe by the gate (a playground program can't
touch a DB/secret/file); the *compile* runs untrusted source, so the build host
needs the usual container/resource/time-limit isolation.

**B2 — fully-client compile via the interpreter tier.** Shipping `rustc`+cargo to
a browser tab is **infeasible** (not a preference — even Rust's own playground
compiles server-side); a WASM `ipe` front-end gets you to typecheck + `ipe_ir`
and **no runnable program**. The gap is closed by the roadmap's **interpreter
tier** (Position A LOCKED; the interpreter is justified there by REPL +
WASM/portability). Ship the front-end (`parse → canonicalise → type → ipe_ir`)
**plus an `ipe_ir` interpreter**, both compiled to WASM. The playground then
type-checks (instant errors, salsa-incremental as the user types) and *runs*
entirely client-side, offline — Elm-parity DX. The interpreter drives the *same*
§Q4 client TEA runtime and the *same* browser-substitute Cmd/Sub bridge, so B2 is
mostly reuse of A once the interpreter exists.

**Soundness + security gates on B2:**
- **H12 differential-conformance** (interpreter output ≡ AOT output across the
  example sweep) makes "runs in the playground" ⇒ "runs when compiled with
  `ipe build`." H12 is extended to cover the WASM targets; the example sweep
  gains WASM rows (build + run under `--target wasm`).
- The interpreter runs under the **same target-keyed kernel registry** in
  `WasmClient` mode, so a playground snippet touching `Ipe.Db`/`Auth.signToken`
  gets the same unbound-name error. The playground is sandboxed **by
  construction**; the secret boundary holds identically in interpreted mode.
- The interpreter is a fixed evaluator: user Ipê source is *data it evaluates*,
  never code it `eval`s — the no-eval invariant is preserved.

**Machinery convergence (answering "can A and B share machinery").** The
playground's execution environment is exactly the §Q3 client capability matrix.
So Target A's client runtime and Target B's execution environment **converge** on
the same runtime + the same gate — that is the shared machinery. B2 is
additionally justified as the third leg (with hot-reload L1/L2 and the REPL) of
an *already-locked* interpreter tier; it does not "tip Open Decision 3," it
reinforces a planned decision.

**Sequencing edge.** B1 depends only on A. B2 depends on the interpreter tier
(post-parity per the roadmap). Target A (client apps) can therefore ship on the
AOT path well before B2.

---

## Divergences from Ipê/Elm (stated factually)

- **Elm compiles its compiler to JS and runs the playground fully client-side.**
  Ipê's AOT compiler cannot: it emits Rust and delegates final codegen to
  `rustc`/cargo, which do not ship to a browser. Ipê reaches the same
  fully-client playground via a *different* mechanism — an IR interpreter in WASM
  — not by shipping the AOT compiler. (Elm has no separate interpreter; Ipê's
  interpreter tier is an independent roadmap item that the playground reuses.)
- **Elm's client runtime is JS.** Ipê's is WASM driven by `web-sys`, which is
  **eval-free** and runs under a strictly tighter CSP (`'wasm-unsafe-eval'`, no JS
  `'unsafe-eval'`).
- **Elm has no server/client secret boundary problem** (no server-side stdlib in
  the same language). Ipê does — hence the target-keyed kernel gate, which has no
  Elm analogue.
- **Diff-as-data reuse.** Ipê already produces DOM mutations as data
  (`Vec<Patch>`) for the SSE wire; the WASM client reuses that identical data
  path against the real DOM — the SSE consumer and the WASM consumer share one
  diff algorithm. This is an Ipê-specific asset (the ported `diff.rs`), not
  inherited from Elm's virtual-DOM.

---

## Open decisions (for the user)

1. **HTTP substitute — reqwest-wasm vs raw `web-sys` fetch.** reqwest-wasm reuses
   the native `Http` kernel implementation (one code path, behavioural parity);
   raw `web-sys` fetch is smaller (bundle size ranks above completeness). The
   panel leans reqwest-wasm *unless* the native `Http` kernel shares little
   request-building/redirect/timeout logic worth reusing. **Settle against the
   actual `Http` kernel code**; streaming bodies (`ReadableStream`) may force a
   web-sys fallback regardless.
2. **JWT / bcrypt WASM crate maturity.** `jsonwebtoken`/`ring` have historically
   thin `wasm32-unknown-unknown` support; the HS256/RS256 primitives may need
   `jwt-simple` or a hand-rolled substitute. bcrypt compiles but is slow.
   Validate at build; the verify API-split (RS256 public-key only, client-side)
   narrows the surface.
3. **IndexedDB / File-System-Access substitutes (v2).** If a client storage
   surface is added, it MUST be a **distinct, namespaced module** (`Ipe.Browser.
   Store` / `Ipe.Browser.File`) with **no** SQL surface — never a `Ipe.Db` alias
   — so server DB code cannot typecheck against it. Confirm the shape when v2 is
   scoped.
4. **`Task`-capability-row endgame.** Rejected for v1/v2 (§Q5). Reconsider only
   if Ipê grows row-polymorphic effects for an independent reason.
5. **`wasm32-wasip1` (edge/server WASM).** Out of scope for the browser target;
   a possible third target for edge/serverless deploy (preserves more of
   File/System). Decide if/when edge deploy is on the roadmap.
6. **Client-bundle size budget gate.** Whether the build fails / warns past a
   size threshold, and where DCE reaches into the Rust dep graph vs the Ipê side.
7. **`Cmd.publish` cross-tab.** Whether `BroadcastChannel` cross-tab pub/sub is
   in-scope for MVP or an add-on; `publishNoEcho`'s cross-*process* bit is
   meaningless client-side.

---

## Implementation surface (the files this target owns)

- `src/compiler/backend/rust/src/project.rs` (+ `crate_specs.rs`) — WASM
  manifest template (cdylib + wasm-bindgen/web-sys/gloo/getrandom-js; no
  tokio/axum/sqlx/native-TLS); the point where the manifest becomes
  computed-from-used-kernels.
- `src/compiler/backend/rust/src/preamble.rs` — `#[wasm_bindgen(start)]` /
  `hydrate` entry + `spawn_local` driver (WASM branch).
- `src/ipe-cli/src/stdlib.rs` (+ the `ipe_kernels` table) — target-keyed kernel
  registry (`Native` | `WasmClient`) as a **default-deny allowlist** (a kernel is
  client-representable only if explicitly `WasmClient`-tagged); the Layer-1 gate.
- `src/compiler/canon` (`ipe_canon`) — module `server`/`client`/`shared`
  classification + reachability-closure check (Layer 2).
- `src/compiler/types` (`ipe_types`) — **`HydrationState` field-type gate**: the
  type serialised into the hydration island must have only client/shared-safe
  field types (reject any transitively secret/server-only field type at the
  declaration). Blocker-1 fix.
- `src/runtime/rust/src/wasm/` (new, `feature = "wasm-client"`,
  `cfg(target_arch = "wasm32")`) — `mount` / `adopt` / `apply` sink + scheduler
  + Cmd/Sub browser bridge + delegated event wiring + the **island serialiser
  (escapes `<`/`>`/`&`/U+2028/U+2029)** and the **fault-tolerant `hydrate`
  parser** (`Result` + fallback to client `init`). All JS→web-sys crossings use
  checked `dyn_into` (no `unchecked_into`; enforced by a `clippy` deny in this
  module). Reuses `src/runtime/rust/src/ui/render.rs`, `live/diff.rs`,
  `live/dispatch.rs` (id scheme), `live/form.rs`, `tea.rs` unchanged — note
  `live/*` is currently gated behind the tokio-linked `live` feature and
  `tea.rs` behind `tokio`; the pure diff/dispatch/form/TEA-type pieces must be
  re-homed (or `cfg`-split) out of those features so `wasm-client` can pull
  them without tokio.
- `emit_expr.rs` / `emit_types.rs` — **no change** (target-agnostic).
- Example sweep + H12 conformance — add WASM rows (build + run under
  `--target wasm`).
- Docs to sync on landing: `ROADMAP.md` (add a WASM/browser section),
  `docs/architecture/ui-live-tui-webview-spec.md` (add `is_client_wasm` to the
  exhaustive target discrimination), `AGENTS.md` (the `[wasm]` `sky.toml`
  section + per-target stdlib capability notes in the authoring reference).
