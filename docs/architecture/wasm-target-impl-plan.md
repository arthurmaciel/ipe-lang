# WASM / browser target — phased implementation plan

> Executable ordering for the design locked in
> `docs/architecture/wasm-target.md` (Q1–Q7). That document owns every design
> decision; this one owns the build order, per-milestone scope/files/gates,
> and the dependency graph. Backlog rows: `#234`–`#241` in
> `scripts/progressive-development/backlog.jsonl`.

## Probe findings (already verified on this tree)

`wasm32-unknown-unknown` is installed. A scoped probe build of the runtime:

| Build | Result |
|---|---|
| `cargo build -p ipe-runtime-rust --target wasm32-unknown-unknown` (default features) | **fails only in `uuid`** — entropy backend needs uuid's `js` feature (the getrandom-js shim). No other dep and no runtime module fails. |
| same + uuid `features = ["js"]` | **exit 0** — the entire default feature set compiles: `core`/`string`/`list`/`dict`/`set`/`maybe`/`result`, `decimal`, `money`, `regex_kernel`, `bytes`, `encoding`, `jwt`, `secret`, **and the whole `Ipe.Ui` render surface (`ui/element.rs`, `ui/render.rs`, `ui/input.rs`, `ui/keyed.rs`, `ui/lazy.rs`, `html.rs`, `css`, `css_safety`)**. |
| same + `--features json` (`serde_json` + `jsonwebtoken`) | **exit 0** — the floor's JSON leg and the JWT crate are wasm-clean (settles the spec's `jsonwebtoken`-maturity open decision for the verify path). |
| same + `--features crypto` (RustCrypto: rsa/aes-gcm/chacha20/pbkdf2/bcrypt) | fails in `getrandom` ("enable the `js` feature") — the second predicted shim. Additive fix; not needed for the floor. |

Two consequences that simplify the spec's floor prerequisites:

1. **The `IpeTask` `Send` split is NOT a floor blocker.** `task.rs`/`tea.rs`
   sit behind the `tokio` feature and `default = []`, so the pure floor never
   compiles them. The `cfg(target_arch = "wasm32")` relaxation of the `Send`
   bound on `IpeTask` (`core.rs:17`) is needed only when the Cmd/Sub browser
   bridge starts driving non-`Send` JS futures (M3/M4). The
   `wasm_floor_scope.rs` native `Send` assertion stays true throughout.
2. **The floor gate can be a one-line CI row today** once the uuid `js` shim
   lands: no `cfg` surgery, no manifest branch required for the *floor* (the
   emitted-crate manifest branch is still required for Target A, M2).

Structural discovery for M3: `live/diff.rs` / `live/dispatch.rs` /
`live/form.rs` are pure but live behind the tokio-linked `live` feature, and
the `IpeCmd`/`IpeSub` types behind `tokio` — the wasm client needs them
re-homed (or `cfg`-split) into a tokio-free scope.

## Dependency graph

```
M0 (floor)
 ├─→ M1 (Layer-1 kernel gate) ─┐
 └─→ M2 (wasm emission branch) ─┼─→ M3 (DOM sink + scheduler) → M4 (Cmd/Sub bridge)
                                │                                   │
                                └─→ M5 (Layer-2 closure + config) ──┴─→ M6 (Target A SPA MVP)
                                                                          │
                                                    M7 (SSR + hydration) ←┘ → M8 (playground B1 + docs)
```

M1 and M2 are independent of each other (parallelisable) once M0 lands; M5
needs M1's target plumbing but not M3/M4; M6 is the integration gate that
needs everything before it except M7.

---

## M0 — Pure-kernel wasm floor (first landable slice)

**Status: LANDED** — uuid `js` shim (wasm32-scoped), `wasm-floor` CI job
(default + `--features json` builds), floor-guard test re-pointed. The crypto
leg stays excluded (getrandom js untested for the RustCrypto stack) — a
stated exclusion, revisited with the crypto substitutes.

**Scope.** Make the documented floor (`List`/`String`/`Dict`/`Maybe`/`Result`
+ JSON, no Task I/O — including the `Ipe.Ui` render surface, which the probe
shows is already part of it) an *enforced* build target instead of an
expectation.

- Add `"js"` to the runtime's uuid features (probe-verified sufficient for
  default + `json`); add `getrandom = { features = ["js"] }` scoped to
  `cfg(target_arch = "wasm32")` for the `crypto` feature leg (or defer the
  crypto leg with a stated exclusion — decide at impl time against bundle
  size).
- CI/gate row: `cargo build -p ipe-runtime-rust --target wasm32-unknown-unknown`
  (default and `--features json`) must stay green.
- Update `src/runtime/rust/tests/wasm_floor_scope.rs`: the floor is no longer
  hypothetical; keep the native `Send` assertion, re-point the prose at this
  plan.

**Files.** `src/runtime/rust/Cargo.toml`, `src/runtime/rust/tests/wasm_floor_scope.rs`,
CI workflow.
**Gate.** The two wasm builds exit 0 in CI; native workspace tests untouched.

## M1 — Layer-1 security gate: target-keyed kernel registry

**Status: LANDED** — `ipe_kernels::Target` (`Native` | `WasmClient`) +
default-deny `available_on` allowlist; `ipe_canon::target_gate` walks the
linked module (naming-based: everything linked is emitted, so reachability
pruning waits for the M5 closure); IPE-N0029 diagnostic + explain page;
`--target wasm` CLI flag threaded through `BuildOptions`/`BuildConfig` and
both build-cache keys. `Cmd.none`/`Cmd.batch`/`Cmd.perform`/`Sub.none` +
`Live.app` are tagged alongside the landed M3 sink (their substitutes exist);
`Live.route` stays untagged until the client router lands (routed apps are
IPE-L0129 under wasm).

**Scope.** The `Target` (`Native` | `WasmClient`) parameter on the kernel
registry as a **default-deny allowlist** (spec Q5 Layer 1): under
`--target wasm` a kernel resolves only if explicitly `WasmClient`-tagged;
naming a server-only kernel (`Auth.signToken`, `Db.query`, `File.readFile`,
`System.getenv`, `Server.listen`, …) is an unbound-name-for-target error at
canonicalisation with the teacher-style diagnostic + fix (route via
`Http.post`). Add the `--target wasm` CLI flag and thread the target through
resolution. Tag the Q3 COMPILES rows only (SUBSTITUTE rows get tagged in M4
when their browser kernels exist — tagging before the substitute exists would
violate the SEAL).

**Files.** `src/ipe-cli/src/stdlib.rs`, `src/compiler/kernels`,
`src/compiler/canon` (resolution error), `src/ipe-cli/src/lib.rs` (flag),
`src/compiler/diagnostics` (new `IPE-` code + explain page).
**Gate.** Unit tests: pure program resolves under both targets; a program
naming each DOES-NOT family fails at canonicalisation with the new code;
default-deny proven by an untagged-new-kernel test.

## M2 — WASM emission branch (manifest + entry)

**Status: LANDED (orchestration partial)** — fourth manifest template
(cdylib, wasm-bindgen pinned to the CLI version, no tokio/axum/sqlx/TLS,
project-local `.cargo/config.toml` shielding wasm32 from host native-linker
rustflags), wasm runtime module set + floor-filtered prelude,
`#[wasm_bindgen(start)]` entry, `www/index.html` + `boot.js` CSP shell in the
emitted project. SEAL proven: `examples/40-wasm-counter` ipe-0 ⇒ wasm
cargo-0. The driver prints the cargo/wasm-bindgen bundle commands instead of
running them — the in-driver orchestration (+`wasm-opt`) is the remaining M2
work.

**Scope.** Fourth manifest template in the backend (spec Q2): `cdylib`,
wasm-bindgen/wasm-bindgen-futures/js-sys/web-sys-allowlist/gloo-timers/
getrandom-js/console_error_panic_hook; **no** tokio/axum/sqlx/native-TLS and
no `server`/`db`/`live` feature (Layer 3 of the gate). `preamble.rs` wasm
branch: `#[wasm_bindgen(start)]` entry replacing `fn main()` + tokio.
`emit_expr.rs`/`emit_types.rs` untouched (target-agnostic).

**Files.** `src/compiler/backend/rust/src/project.rs`, `crate_specs.rs`,
`preamble.rs`; `src/ipe-cli/src/project.rs` (build orchestration:
`cargo build --target wasm32-unknown-unknown --release` → `wasm-bindgen` CLI →
`wasm-opt -Oz` → `index.html` shell + recommended CSP header).
**Gate.** A pure floor-only program emitted under `--target wasm` cargo-builds
to a `.wasm` (SEAL holds cross-target: ipe exit-0 ⇒ wasm cargo exit-0).

## M3 — Client runtime: DOM sink + TEA scheduler (Ipe.Ui in the browser)

**Status: LANDED (first slice)** — `src/runtime/rust/src/wasm/` (feature
`wasm-client`): mount, `dom::diff` `Vec<Patch>` apply via typed web-sys,
delegated root listeners keyed by sky-id, rAF-coalesced update cycle,
`Cmd.perform` via `spawn_local`, panic hook. Proven in Chromium:
`examples/40-wasm-counter` renders the Ipe.Ui view and processes onClick
(+3/−1/Reset). Divergence from the sketch: mount applies the
sanitiser-gated `render_html` output (ONE renderer — the DOM the diff
patches is byte-identical to the SSE first paint) rather than a per-node
tree walk; the typed walk remains an option behind equivalence tests.
Prereqs executed: `dom/` re-home (diff/dispatch/form/req), cfg-split
`IpeTask`/`PerformThunk` Send relaxation. Remaining: Sub bridge (timers),
pub/sub broker, `wasm-bindgen-test` harness in the runtime crate.

**Scope.** `src/runtime/rust/src/wasm/` (feature `wasm-client`,
`cfg(target_arch = "wasm32")`) per spec Q4: `mount` (tree walk →
`create_element`/`set_attribute`, sky-ids stamped), `apply` (the same
`Vec<Patch>` the SSE wire uses, applied via typed web-sys), delegated event
listeners keyed by `data-sky-hid`, one update+diff+patch per
`requestAnimationFrame`, `console_error_panic_hook` log-and-die with the
classified taxonomy. Checked `dyn_into` only (clippy deny on
`unchecked_into`/`unchecked_ref` in this module). Prerequisites executed
here: re-home the pure `live/diff.rs` + `live/dispatch.rs` (id scheme) +
`live/form.rs` and the `IpeCmd`/`IpeSub` type definitions out of the
`live`/`tokio` features into an always-compiled (or `wasm-client`-reachable)
scope, and apply the `cfg`-gated `IpeTask` `Send` relaxation (never a fork,
never a `MaybeSend` trait; native assertion in `wasm_floor_scope.rs` stays).

**Ipe.Ui is the headline and is de-risked:** `Ipe.Ui` renders through
`ui/render.rs` → `Html<M>` — probe-verified to compile to wasm today — so M3
adds only the sink, not a render port.

**Files.** `src/runtime/rust/src/wasm/` (new), `src/runtime/rust/src/mod.rs`
(re-homes), `Cargo.toml` (`wasm-client` feature: wasm-bindgen, web-sys
allowlist, gloo-timers), `core.rs` (`cfg` Send split).
**Gate.** `wasm-bindgen-test` headless-browser test: a counter app (Ui.button
onClick) mounts, dispatches, patches the real DOM; clippy deny green;
native workspace fully green (no native behaviour change).

## M4 — Cmd/Sub browser bridge (effects tier substitutes)

**Scope.** The Q4 mapping table: `Cmd.perform` → `spawn_local`; `Sub.every`/
`Time.sleep` → gloo-timers; `Http.get/post/request` → fetch (settle
reqwest-wasm vs raw web-sys fetch against the actual `http_client` kernel —
spec Open decision 1); `Log.*`/`Io.write*` → console; `Random.*`/
`Crypto.randomBytes` → `crypto.getRandomValues`; in-tab pub/sub broker
(echo/no-echo preserved); `Ipe.WebSocket` client → `web_sys::WebSocket`;
CORS/timeout failures surface as `Task.fail`, never traps. Tag each landed
substitute `WasmClient` in the M1 registry as it ships.

**Files.** `src/runtime/rust/src/wasm/` (bridge modules),
`src/ipe-cli/src/stdlib.rs` (SUBSTITUTE-row tags).
**Gate.** Per-substitute wasm-bindgen-tests; capability-matrix conformance
test asserting every Q3 SUBSTITUTE row resolves + every DOES-NOT row fails at
canonicalisation (ties M1↔M4 together).

## M5 — Layer-2 module partition + `[wasm]` config

**Scope.** `server`/`client`/`shared` module classification + client-entry
reachability closure with exact-import-path errors (spec Q5 Layer 2); the
`[wasm]` `sky.toml` section (`mode`/`entry`/`mount`/`publicEnv`/`optLevel`);
`publicEnv` default-deny allowlist + secret-name denylist (`IPE_*`,
`*_SECRET`, `DATABASE_URL`, … — allowlisting a denylisted name is a build
error) surfaced through a distinct `Ipe.Env.public` kernel.

**Files.** `src/compiler/canon` (classification + closure), `src/ipe-cli`
(sky.toml parse + flag wiring), `src/compiler/diagnostics`.
**Gate.** Closure test: `Main(client) → View(shared) → Data(server)` fails
naming the path; `IPE_AUTH_TOKEN_SECRET` can be neither read nor allowlisted;
shared `view` module type-checks against both targets' surfaces.

## M6 — Target A MVP: pure client SPA end-to-end

**Scope.** Integration: `ipe build --target wasm` on a real TEA app (static
shell + bundle + CSP header), a checked-in example, and the sweep/H12 rows
(build + run under `--target wasm`, behavioural interaction — not boot-only).
Record bundle size; decide the size-budget gate (spec Open decision 6).
`chrono-tz` feature-gating if the size number demands it.

**Files.** `examples/` (new wasm SPA example), sweep tooling, `ROADMAP.md` +
`docs/architecture/ui-live-tui-webview-spec.md` (`is_client_wasm`) +
`AGENTS.md` (`[wasm]` section, per-target capability notes).
**Gate.** Example builds, boots headless, and passes a real-interaction
scenario; all three gate layers each have a red-team test; sweep stays green.

## M7 — Ipe.Live SSR + hydration (MVP+1)

**Status: LANDED** — `island_escape` (XSS-safe JSON embedding for
`<`/`>`/`&`/U+2028/U+2029), `render_page_hydrate` (SSR page with island
`<script type="application/sky-model+json">`), `wasm_adopt_app` (hydration
entry: skips `set_inner_html`, attaches delegated listeners only),
fault-tolerant `hydrate` wasm-bindgen export (parse → `Result`, fallback to
clean `init` on tampered/malformed island), `HydrationState` field-type gate
(`ir_type_contains_non_serde` allowlist — compile error on secret/server-only
fields), `wasm_hydrate_mode` flag wired from `[wasm] mode = "hydrate"` in
`sky.toml` through `BuildOptions` → `BuildConfig` → `RustBackend` → `EmitCtx`,
example `46-wasm-hydration`, 6 gate tests in `wasm_hydration_gate.rs`.

**Scope.** Spec Q6 mode 2: typed `HydrationState` island (never the Model)
with the field-type gate in `ipe_types`; island serialiser escaping
(`<`/`>`/`&`/U+2028/U+2029); fault-tolerant `hydrate` (parse →
`Result`, fallback to clean client `init` on tampered/malformed island);
`adopt` path (sky-id match, listeners only); dev-mode empty-first-diff
assertion + prod diff-and-replace fallback; determinism hazards (one float
formatter, ordered collections, usize width, pinned locale/zone) each get a
cross-target test.

**Files.** `src/compiler/types` (field-type gate), `src/runtime/rust/src/wasm/`
(adopt + island parse), backend `preamble.rs` (`hydrate` export), Live server
side (island emission in `src/runtime/rust/src/live/`).
**Gate.** Hydration example: SSR paint → client takeover with empty first
diff; tampered-island test falls back cleanly (no trap); a secret-bearing
field type in `HydrationState` is a compile error.

## M8 — Playground B1 + docs closure

**Status: LANDED** — `ipe-playground` binary crate (`src/playground/`):
`POST /compile` handler accepts Ipê source, runs `ipe build --target wasm`
in a timeout-wrapped subprocess, returns the compiled WASM bundle
(`pkg_wasm_b64` + `pkg_js`) as JSON; two-pane browser UI
(`www/index.html`) with Ctrl+Enter compile; hand-rolled base64 encoder
(no extra dep); CORS via tower-http; optional static-file serving via
`IPE_PLAYGROUND_STATIC_DIR`. B2 (interpreter-in-WASM) stays gated on the
interpreter tier and is not built here.

**Scope.** Server-compile-then-ship-WASM playground (pure reuse of A; build
host isolation per Q7); final docs sync. B2 (interpreter-in-WASM) stays
gated on the interpreter tier — filed as a dependency edge, not built here.

**Gate.** Playground compiles a submitted floor program and runs it via the
M3 client runtime; docs list in the spec's Implementation surface all updated.
