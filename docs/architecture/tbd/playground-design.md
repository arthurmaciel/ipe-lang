# Playground design — `Ipe.Process` + hybrid playground

> All fenced blocks in this document are **illustrative design sketches** (Ipê
> module shapes, wire payloads, wiring tables, a CI matrix outline) — not runnable
> commands to copy. They specify the intended shape for the implementing round;
> the exact emitted text and signatures are pinned by that round's tests.

Implementation-ready design for two coupled deliverables:

- **`Ipe.Process`** — a typed, sound stdlib surface over `std::process::Command`
  with a wait-with-timeout, capability-denied under the browser (WASM) target.
- **the playground** — replace the axum backend (`src/playground/`) with a hybrid:
  the browser compiles Ipê→Rust with the in-browser WASM compiler already shipped
  (`examples/wasm/language-playground/`), and an **Ipê** server (written in Ipê,
  using `Ipe.Http.Server`) builds and runs the emitted Rust in a sandbox and
  streams the program output back.

This is a design document. It stops at the spec; a later swarm round implements it.

---

## A. Architecture — the hybrid

```
┌──────────────────────────── BROWSER (static SPA, ipe-wasm) ─────────────────────────────┐
│                                                                                          │
│   ACE editor  ──(debounced, on change)──▶  compile(source)   [wasm-bindgen export]       │
│        │                                        │                                        │
│        │                                        ▼                                        │
│        │                          CompileOutcome { ok, diagnostics, emitted_rust }       │
│        │                                        │                                        │
│        │                    ┌───────────────────┴───────────────────┐                    │
│        │             ok == true                              ok == false                  │
│        │                    ▼                                       ▼                      │
│        │        right pane: emitted Rust                right pane: diagnostics            │
│        │        Run button: ENABLED                     Run button: DISABLED              │
│        │                    │                                                              │
│        │             (user clicks Run)                                                     │
│        │                    ▼                                                              │
│        │   POST /run  { rust: "<emitted Rust project text>" }   ── fetch ──▶               │
│        │                                                                    │              │
│        └────────────────────────────────────────────────────────────────  │              │
│                                                                             │              │
│   bottom pane (below the Rust pane): program stdout / stderr  ◀── JSON ──── │              │
└─────────────────────────────────────────────────────────────────────────  │  ───────────┘
                                                                              │
┌──────────────────────────── SERVER (Ipê, Ipe.Http.Server) ──────────────── ▼ ────────────┐
│  POST /run                                                                                │
│    1. parse body → { rust : String }          (parse, don't validate; size-cap first)     │
│    2. token <- Crypto.randomToken 8                                                        │
│    3. dir = temp project root for this token                                              │
│    4. write Cargo project files (split the banner-delimited text back into files)          │
│    5. cargo build   in a SANDBOX with net-off + fs-jail + rlimits + wall clock             │
│    6. run the built binary in the SAME sandbox class, wall-clock capped                    │
│    7. format { build stdout/stderr, run stdout/stderr, exitCode } → response text          │
│    8. cleanup temp dir (best-effort; never masks the real result)                          │
│  GET  /            → the SPA index.html (static)                                          │
│  GET  /pkg/*, /static/* → the wasm bundle + assets (static)                               │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

### Data shapes on the wire

Request (`POST /run`), JSON body:

```
{ "rust": "<the full emitted-Rust project text, banner-delimited exactly as
            CompileOutcome.emitted_rust renders it>" }
```

Response, JSON body:

```
{ "ok": Bool,          -- did the build succeed AND the run finish within limits
  "unsandboxed": Bool, -- true when no OS jail was available (drives the warning box)
  "output": String }   -- the formatted build+run transcript (below)
```

Formatted `output` (server-owned single source of truth for the transcript):

```
── Build ─────────────
<trimmed cargo build stderr/stdout, or "ok">
── Run ──────────────
<trimmed program stdout>
<if nonzero exit or timeout: a trailing status line, e.g. "[exited 101]" / "[timed out after 10s]">
```

### Why the client sends **Rust**, not Ipê

The browser already ran the whole frontend (`compile()` in `src/wasm/src/lib.rs`),
so it holds the emitted Rust and a proven-`ok` verdict. Sending Rust:

- keeps the server off the Ipê frontend entirely — the server is a build+run
  harness, not a second compiler, so there is one compiler (the WASM one), one
  source of truth for diagnostics;
- means the server never accepts Ipê it would have to re-typecheck to trust; it
  receives Rust it will build under a sandbox regardless of provenance (the
  sandbox, not a trust check, is the security boundary — fail closed);
- makes the Run button gating exact: it is enabled **iff** `ok`, i.e. iff there
  *is* emitted Rust to send.

### What replaces the old Python `http.server` step

Nothing external. The Ipê server (`Ipe.Http.Server`) serves the static SPA (index,
wasm bundle, assets) on `GET /` and the compile-and-run endpoint on `POST /run`.
`python3 -m http.server` is removed from `build.sh` and from all docs.

---

## B. `Ipe.Process`

### Typed surface (`src/stdlib/Ipe/Process.ipe`)

Two entry points: a terse `run` and a rich `exec` that captures everything. The
following module is the illustrative target shape.

```elm
-- | Ipe.Process — spawn an external program and capture its result
-- (Layer 3 Ipe source).
--
-- Every function is an effect (`Task Error _`). The runtime kernel is the
-- PARSE boundary: it builds the argv with NO shell (each argument is a
-- separate list element, so the shell-injection class does not exist), waits
-- with a wall-clock cap, and KILLS the child on expiry. A nonzero exit or a
-- timeout is a typed `Error`, never a panic.
--
-- SECURITY: this module is a server-effect surface. It is DENIED under the
-- browser (WASM) target and listed among the sandbox-blocked modules — a
-- program importing it cannot compile for `--target wasm`.
module Ipe.Process exposing
    ( ProcessResult
    , ExecOptions
    , run
    , exec
    , defaultOptions
    , withCwd
    , withTimeoutMs
    , withEnv
    )


import Ipe.Ffi as Ffi
import Ipe.Dict exposing (Dict)
import Ipe.Error exposing (Error)


-- ── Types ─────────────────────────────────────────────────────────────────────

-- | The captured outcome of a finished process. `exitCode` is `Nothing` when
-- the process was killed by a signal (no code) — make-invalid-states-
-- unrepresentable: "killed" and "exited N" are distinct, not `-1`.
type alias ProcessResult =
    { stdout : String
    , stderr : String
    , exitCode : Maybe Int
    }


-- | Options for `exec`. Built with the `with*` combinators from
-- `defaultOptions` so adding a field never breaks a call site.
type alias ExecOptions =
    { cwd : Maybe String
    , timeoutMs : Int
    , env : Dict String String
    }


-- ── Simple form ────────────────────────────────────────────────────────────────

-- | `run program args` — spawn `program` with `args` (NO shell), wait with the
-- default wall-clock cap, and return captured stdout. A nonzero exit or a
-- timeout fails the Task with a typed `Error`.
run : String -> List String -> Task Error String
run = Ffi.kernel "Process_run"


-- ── Rich form ───────────────────────────────────────────────────────────────────

-- | `exec program args options` — like `run` but returns the full
-- `ProcessResult` (stdout, stderr, exit/kill) and honours `cwd`, `timeoutMs`,
-- and `env`. A nonzero exit is NOT a Task failure here — it is reported in
-- `exitCode`, so the caller decides. A spawn failure or a timeout IS a Task
-- failure (there is no result to report).
exec : String -> List String -> ExecOptions -> Task Error ProcessResult
exec = Ffi.kernel "Process_exec"


-- ── Option builders (pure) ──────────────────────────────────────────────────────

defaultOptions : ExecOptions
defaultOptions =
    { cwd = Nothing, timeoutMs = 10000, env = Ipe.Dict.empty }


withCwd : String -> ExecOptions -> ExecOptions
withCwd d opts = { opts | cwd = Just d }


withTimeoutMs : Int -> ExecOptions -> ExecOptions
withTimeoutMs ms opts = { opts | timeoutMs = ms }


withEnv : String -> String -> ExecOptions -> ExecOptions
withEnv k v opts = { opts | env = Ipe.Dict.insert k v opts.env }
```

Design notes:

- **No shell, ever.** `run`/`exec` take `(program, argsList)`; the runtime spawns
  `Command::new(program).args(argsList)` directly. There is no `sh -c "…"` form,
  so the quoting/interpolation injection class the Sky `shellIn` helper carried
  does not exist. (Callers that want `sh -c` must pass `"sh"` and
  `["-c", "…"]` explicitly and own that risk; the default surface never
  concatenates a command string.)
- **`exitCode : Maybe Int`** encodes killed-by-signal vs exited-with-code as
  distinct states (make-invalid-states-unrepresentable). `run` collapses both
  nonzero and killed into a typed `Error`; `exec` surfaces them.
- **Typed error channel** — `Task Error _`, never `Task String _` (non-regression
  rule §7). The runtime maps spawn failure, timeout, and (for `run`) nonzero exit
  to `IpeError` variants; the child's own stderr rides along in the error `info`.

### Runtime kernel (`src/runtime/rust/src/process.rs`, new)

Over `std::process::Command`. Requirements, in principle order:

1. **Security.** Direct argv; no shell. `env` is *set*, not merged blindly — the
   child inherits a scrubbed environment plus the caller's `env` entries (the
   playground server passes exactly what a build needs). Output is capped
   (a byte ceiling) so a runaway child cannot exhaust server memory through the
   pipe; over-cap is a typed `Error`, matching the sandbox crate's
   `OutputCapExceeded` shape.
3. **Soundness.** No `unwrap`/`expect`/`panic`/indexing. `spawn()`,
   `wait`/`try_wait`, and every pipe read map their `io::Result` into `IpeError`.
   The runtime file carries the runtime crate's panic-denying clippy posture (see
   `runtime/src/lib.rs`).
2. **Wait-with-timeout that kills on expiry.** Spawn with piped stdout/stderr;
   drive `wait`/output collection against a deadline (`timeoutMs`); on expiry
   `child.kill()` + reap, and return a `Timeout` `Error`. No busy-wait; no
   unbounded `wait`. This mirrors the existing `run_jail` wall-clock discipline —
   share that timeout helper rather than re-inventing it.

### The single-construction-point wiring (all anti-drift sites)

Registering `Process_run` and `Process_exec` updates every site enumerated in
the root `AGENTS.md` "Registering a kernel", each fail-closed at ipe time:

| Site | Change |
|---|---|
| `src/compiler/kernels/src/lib.rs` | new enum variants `ProcessRun`, `ProcessExec`; `decl()` rows `d("Process","run",2,Server,"process_run")` and `d("Process","exec",3,Server,"process_exec")` (effect family **Server**, i.e. server-effect ⇒ WASM-denied); add to `ALL` |
| `src/compiler/types/src/constrain.rs` | type schemes for both (`run : String -> List String -> Task Error String`; `exec : String -> List String -> ExecOptions -> Task Error ProcessResult`); out of the `KNOWN_UNBACKED` bucket, into `FIRST_SCHEMED` coverage; register the `ProcessResult`/`ExecOptions` record aliases so field access type-checks |
| `src/compiler/canon/src/env.rs` | module alias row so `import Ipe.Process as Process` and `Process.run`/`Process.exec` resolve |
| `src/compiler/lower/src/lower.rs` | arity table rows (2 and 3); lower to the runtime call names |
| `src/compiler/backend/rust/src/naming.rs` | kernel → runtime fn names (`process_run`, `process_exec`) |
| `src/compiler/ir/src/pretty.rs` | pretty-print arms for the two kernels |
| `src/stdlib/src/lib.rs` (embedded-stdlib registration: `include_str!` + module registry) | register `Ipe.Process` so the `.ipe` module is embedded and injected |

Seal the new module with the standard template
(`src/ipe-cli/tests/golden_stdlib_module_seal.rs`).

### Capability model — denied under the browser target

`Process` is a **server-effect** family, exactly like `Ipe.Http.Server` /
`Ipe.File` / `Ipe.System`. The mechanism already exists and is reused verbatim:

- The `Target::WasmClient` allowlist is **default-deny** (`kernels/src/lib.rs`:
  every server-effect kernel returns `false` from `available_on(Target::WasmClient)`;
  a red-team test pins that the WASM allowlist is default-deny). `ProcessRun`
  and `ProcessExec`, tagged `Server`, are denied there for free.
- Importing `Ipe.Process` in a `--target wasm` build is a canon-time error
  (`NameError::ServerModuleReachableFromWasmClient`, **IPE-N0030**) — the same gate
  that already blocks `Ipe.Http.Server`/`Ipe.File` from the browser bundle. Add
  `Ipe.Process` to that module-classification denylist
  (`src/compiler/canon/src/module_classify.rs`).
- The in-browser compiler (`src/wasm/src/lib.rs`) compiles at
  `Target::WasmClient`, so the playground's own client path exercises exactly
  this gate: a user who pastes `import Ipe.Process` gets a diagnostic in the
  right pane, never a foothold.

This is the SSOT security story: `Process` is dangerous **and** it is on the
server-effect list **and** the browser bundle denies that whole list — three
statements, one list.

---

## C. Server design (Ipê, `Ipe.Http.Server` + `do`/`parallelDo`)

The Sky reference was a `Sky.Live` routed app (server-rendered TEA). Ipê's
equivalent surface for a plain compile-and-run endpoint is **`Ipe.Http.Server`**
(non-routed: `Server.listen port routes`, handlers `Request -> Task Error Response`).
This is the right fit — the client is now the SPA, so the server needs no TEA loop,
no `Model`/`Msg`/`view`. The Sky `State`/`Update`/`View` modules therefore do **not**
port as-is; their responsibilities move as follows:

| Sky module | Ipê equivalent | Fate |
|---|---|---|
| `Runner.sky` | `Runner.ipe` | ports directly (temp dir, write, build, run, format, cleanup) |
| `Security.sky` | `Security.ipe` | mostly retired — the security boundary is now the OS sandbox on the *Rust* build, not an Ipê-source module denylist. Keep a small request-shape guard (size cap, UTF-8) |
| `State/Update/View/*.sky` | — | dropped; the SPA (client) owns all UI state |
| `Examples.sky` | client-side | examples ship in the SPA (`index.html`), not the server |
| `Main.sky` | `Main.ipe` | becomes `Server.listen` with two routes (`/`, `/run`) + static |

### `Runner.ipe` — the build+run pipeline, `do`-notation (not an `andThen` pyramid)

Per ADR 0050, `do` desugars to `Task.andThen`/`let`; `parallelDo` to `Task.parallel`.
The pipeline is inherently sequential (each step needs the previous step's dir),
so it is a `do` block. `parallelDo` is used only where steps are genuinely
independent — writing the several project files at once. Illustrative shape:

```elm
module Runner exposing (runEmittedRust)

import Ipe.Crypto as Crypto
import Ipe.Error as Error
import Ipe.File as File
import Ipe.Process as Process
import Ipe.String as String
import Ipe.Task as Task


-- | Build and run a full emitted-Rust project text, returning the formatted
-- transcript. A cleanup failure never masks the real result.
runEmittedRust : String -> Task Error String
runEmittedRust rustProjectText =
    do
        token <- Crypto.randomToken 8
        dir = tempRoot ++ "/ipe-run-" ++ token
        File.mkdirAll (dir ++ "/src")

        -- The emitted text is banner-delimited (`// ==== path ====`); split it
        -- back into files. Independent writes run together.
        _ <- parallelDo
            writeSplitFiles dir rustProjectText   -- expands to Task.parallel [ … ]

        build <- Process.exec "cargo"
            [ "build", "--manifest-path", dir ++ "/Cargo.toml" ]
            (buildOptions dir)

        result <- runIfBuilt dir build

        _ <- cleanup dir          -- best-effort; onError-swallowed inside cleanup
        Task.succeed result
    |> Task.onError (\err ->
           -- On any failure, still attempt cleanup, then re-report the error.
           cleanup dir |> Task.andThen (\_ -> Task.fail err))
```

- `runIfBuilt` inspects `build.exitCode`: on `Just 0` it runs the binary
  (`Process.exec <dir>/target/debug/<bin> [] (runOptions dir)` with a short
  `timeoutMs`) and formats build+run; on nonzero it formats the build failure and
  skips the run.
- `writeSplitFiles` returns a `List (Task Error ())` (one `File.writeFile` per
  emitted file); `parallelDo` over it is `Task.parallel`.
- `cleanup dir = File.remove dir |> Task.onError (\_ -> Task.succeed ())` — a fresh
  Task per call site (a Task is one-shot; do not share the binding), matching the
  Sky `removeDir` note.
- `formatOutput` owns the `── Build ──` / `── Run ──` transcript (SSOT for the
  wire format in §A).

### `Main.ipe` (illustrative shape)

```elm
module Main exposing (main)

import Ipe.Http.Server as Server
import Ipe.Http.Server exposing (Request, Response)
import Ipe.Maybe as Maybe
import Ipe.String as String
import Ipe.System as System
import Ipe.Task as Task
import Runner


main =
    let port = Maybe.withDefault 8080
                   (String.toInt (System.getenvOr "IPE_PLAYGROUND_PORT" "8080"))
    in
    Server.listen port
        [ Server.get  "/"     serveIndex        -- static SPA
        , Server.get  "/pkg/:file"   servePkg    -- wasm bundle (static)
        , Server.post "/run"  handleRun
        ]


handleRun : Request -> Task Error Response
handleRun req =
    do
        payload = Server.body req            -- POST body (String)
        rust    = decodeRustField payload    -- parse, don't validate (typed decode)
        transcript <- Runner.runEmittedRust rust
        Task.succeed (Server.json (encodeResult True transcript))
    |> Task.onError (\err ->
           Task.succeed
               (Server.withStatus 200
                   (Server.json (encodeResult False (Error.toString err)))))
```

- `Server.body req` reads the POST body (confirmed accessor). `decodeRustField`
  parses the JSON body into a typed `{ rust : String }` at the boundary
  (`Ipe.Json.Decode`), rejecting oversize / malformed input as a typed error —
  never re-validated downstream.
- Response builders confirmed present: `Server.text`/`json`/`html`/`withStatus`/
  `withHeader`/`redirect`/`withCookie`.

### Static serving — gap / decision

`Ipe.Http.Server` has **no** dedicated static-directory kernel today (no
`Server.static`/`serveDir`). Two in-boundary options; the design picks (1):

1. **Serve the few known static files through ordinary handlers** that read the
   file with `Ipe.File.readFile` and return `Server.html`/`Server.text` with the
   right content type (`Server.withHeader "content-type" …`). The bundle is a
   fixed, small set (`index.html`, `pkg/ipe_wasm.js`, `pkg/ipe_wasm_bg.wasm`), so
   an explicit route per asset is honest and needs no new kernel. Note: the wasm
   file is binary — this needs `Ipe.File.readFileBytes` plus a bytes-bodied
   response; if the server response builders cannot carry a raw byte body with a
   content type, that is a **prerequisite gap** (see below) and option (2) is taken.
2. **Add a `Server.static : String -> Route` kernel** (a new server-effect kernel,
   full anti-drift wiring). Larger; defer unless (1)'s binary-body path is blocked.

---

## D. Cross-platform sandbox strategy (security-critical)

The server builds and runs **arbitrary emitted Rust**. `cargo build` executes
foreign code (`build.rs`, proc-macros) and the built binary is fully arbitrary —
this is remote code execution by construction. It MUST run jailed: network off,
filesystem jailed to the temp project, memory/CPU + wall-clock capped.

**This capability already exists in-repo and is reused, not rebuilt.** The
`ipe_sandbox` crate (`src/compiler/sandbox/`) was built to jail exactly this — it
jails `ipe add <crate>` compiles, which are the same RCE surface. It provides:

- `probe()` → `Capabilities { bwrap, prlimit, timeout }` and `missing_caps()`.
- `build_jail::build_in_jail(...)` with per-platform arms
  (`cfg(target_os = …)`) returning a `JailOutcome`.
- `run_jail::probe_run_jail_tools()` + `run_jail::exec_in_run_jail(...)`, again
  per-platform, plus `SandboxProfile` (fs scope, `RunResourceLimits`, network
  axis) and `run_jail_argv`.
- **Fail-closed default (IPE-F4410):** if the jail mechanism or a mandatory cap
  helper (`timeout`/`prlimit`) is absent, the jail is refused, not run uncapped.
  The one override is an env flag that the driver must surface with a printed
  trust warning.

The playground server does not call this Rust crate directly (the server is Ipê).
Instead, the emitted-Rust build+run is driven through **`Ipe.Process`**, and the
`Process` runtime kernel is wired to go **through `ipe_sandbox`** when building/
running untrusted code. Concretely: the `Process` kernel gains a "jailed" path the
playground opts into (an `ExecOptions` sandbox flag, or a dedicated
`Process.execJailed`); on the server, `cargo build` and the program run take that
path. Reusing `ipe_sandbox` means the playground inherits the audited fail-closed
posture instead of forking a second, weaker sandbox.

### Per-platform mechanism, detection, and fallback

| Platform | Mechanism (already in `ipe_sandbox`) | Detected at runtime by |
|---|---|---|
| **Linux** | bubblewrap (`bwrap`): `/` read-only, one scoped writable tempdir, scrubbed env, fresh PID/UTS/IPC/cgroup + **empty net** namespaces (no egress); `prlimit` rlimits (AS/CPU/NOFILE); `timeout` wall clock; optional seccomp (`seccomp.rs`) | `probe()` finds `bwrap`+`prlimit`+`timeout` on `PATH`; `missing_caps()` empty |
| **macOS** | `sandbox-exec` with an SBPL profile (`sbpl_from_profile`), net denied, fs scoped to the temp dir; `macos_scrubbed_env`; wall clock via the run-jail tools | `run_jail::probe_run_jail_tools()` (macOS arm) |
| **FreeBSD** | Capsicum / `jail` build arm (`build_jail_freebsd_e2e.rs` exercises it) | build-jail FreeBSD arm probe |
| **Windows** | Job Objects + restricted token (`windows_scrubbed_env`, `run_windows_jailed_for_test`; `run_jail_windows_e2e.rs` / `build_jail_windows_e2e.rs`) | `run_jail::probe_run_jail_tools()` (Windows arm) |

**Fallback when NO sandbox is available.** Do not block — the playground is a
local dev tool as often as a hosted one, and a developer running it on their own
box is entitled to. When `probe()`/`probe_run_jail_tools()` reports no usable jail,
the server:

1. still serves `/run`, but marks each response as **unsandboxed** (the
   `unsandboxed` flag in the JSON so the client can render the warning banner), and
2. the client renders a large light-red warning box above the run output. Exact
   copy:

> **⚠ Running unsandboxed on this machine.**
> No OS sandbox (bubblewrap / `sandbox-exec` / Capsicum / Job Object) was found,
> so the emitted Rust is compiled and executed here with only a time limit — not
> isolated from your files or network. Emitted Rust is only as trustworthy as the
> Ipê you wrote: if this page or the code it compiles were tampered with, running
> it could let an attacker execute code on this machine (remote code execution)
> and reach your files. **If you do not fully understand this risk, run the
> playground only on your own computer, never expose it to the internet.**

The server still applies the `Process` wall-clock + output cap in the fallback —
"no jail" degrades isolation, never the timeout. This is fail-closed on
information (the user is told), open on availability (the tool still works
locally), which is the correct trade for a dev tool whose refusal-by-default path
(`IPE-F4410`) is the *hosted* deployment's posture.

### Temporary CI workflow — sandbox matrix

A dedicated, temporary workflow (`.github/workflows/playground-sandbox.yml`,
removed once the platform arms are covered by the standard e2e shards) exercises
the jail on each OS. Illustrative outline:

```
strategy:
  matrix:
    os: [ubuntu-latest, macos-latest, windows-latest]   # + FreeBSD via a VM action
job (per os):
  - install jail deps (Linux: bubblewrap util-linux coreutils; others: builtin)
  - build ipe + the Ipê playground server
  - POST a benign program → assert it builds, runs, returns expected stdout
  - POST a program that tries to open a socket / read /etc → assert the jail
    DENIES it (network-off + fs-jail proven, not assumed)
  - simulate "no jail": hide the jail tools from PATH → assert the response is
    flagged unsandboxed AND the timeout/output-cap still fire
```

The deny-path assertions reuse the existing `*_jail_*_e2e.rs` red-team patterns
(e.g. `build_jail_windows_e2e.rs`'s "network-withholding jail must deny the
socket and decode to Denied { network }").

---

## E. Client design (SPA)

Builds on the shipped `examples/wasm/language-playground/index.html`, which
already has: ACE editor, a full theme switcher that re-themes the whole UI, a
debounced live compile via `compile()`, and a Run button. Changes:

- **Two output panes, stacked in the right column.** Top: emitted Rust (or
  diagnostics), as today. **New** bottom pane: program stdout/stderr from
  `POST /run`. A splitter or fixed proportion divides them.
- **Run button gating.** `runBtn.disabled = !lastCompile.ok`. On every debounced
  compile, store `ok`; when `ok` is false the bottom pane shows "Fix the errors
  above to run." The button, when enabled, POSTs `lastCompile.emitted_rust`.
- **`POST /run` flow.** On click: disable the button, show "Building and running…"
  in the bottom pane, `fetch('/run', { method:'POST', body: JSON.stringify({ rust })})`,
  render `output` (monospace, `err` class if `!ok`), and if the response is flagged
  `unsandboxed`, render the light-red warning box (copy in §D) above it. Re-enable
  the button.
- **ACE mode/highlighting.** Keep the current pragmatic choice — Haskell mode is
  the closest bundled highlighter for Ipê's Elm/Haskell-family syntax — until a
  dedicated Ipê mode is authored (a follow-up, not this scope). Note it in a code
  comment, not a promise.
- **Theme selector.** The existing mechanism stays: ACE `ext-themelist` populates
  the `<select>`; `setTheme` applies the ACE theme AND derives the UI CSS
  variables (`--bg/--fg/--panel/--border/--muted/--accent`) from the editor's
  computed colours, so the whole app restyles. A few representative themes to keep
  in the shortlist: `tomorrow_night` (default), `github`, `monokai`, `dracula`,
  `solarized_light`, `gruvbox`. (All bundled themes remain selectable.)
- **Strip all Sky references.** No "Sky", no `☁` logo, no cloud copy. Title "Ipê
  playground". GitHub link → **https://github.com/arthurmaciel/ipe-lang**.

---

## F. Integration & migration

Fold the playground into `examples/wasm/language-playground/` and delete the axum
crate.

- **Move / create** under `examples/wasm/language-playground/`:
  - `server/` — the Ipê server project (`ipe.toml`, `src/Main.ipe`,
    `src/Runner.ipe`, `src/Security.ipe`), an Ipê example that builds and runs like
    any other server example.
  - `index.html` — extended with the second output pane + `/run` wiring (§E).
  - `build.sh` — kept, still builds `ipe-wasm` → `pkg/`. Its trailing
    `python3 -m http.server` instruction is **removed**; replaced by "serve via the
    Ipê playground server: `cd server && ipe run`".
- **Delete** `src/playground/` (the axum crate: `main.rs`, `Cargo.toml`,
  `www/index.html`).
- **Remove** `"src/playground"` from the workspace `members` in the root
  `Cargo.toml` (around line 29).

References to update (search-and-fix list):

| Location | Change |
|---|---|
| root `Cargo.toml` `members` | drop `"src/playground"` |
| `examples/wasm/language-playground/build.sh` | drop the `python3 -m http.server` line; add the `ipe run` server instruction |
| any doc mentioning `ipe-playground` / `IPE_PLAYGROUND_*` env vars / `/compile` endpoint / axum | rewrite for the Ipê server + `/run` |
| `docs/internals/wasm.md` (referenced from `src/wasm/src/lib.rs`) | note the run step is now an Ipê server, not axum |
| `.github/workflows/*` referencing `ipe-playground` | remove; add the temporary sandbox matrix (§D) |
| examples sweep (`tools/scripts/lib/examples.sh`) | the new `server/` is a normal server-shape example — verify it is auto-included (disk-derived `build_set`) or add its dir |

Note (write-boundary): `examples/*/target` is git-tracked in places (a known bug,
per project memory) — ensure the new `server/` does not commit a `target/`, and
the temp build/run dirs live under `~/.cache/ipe/` per the write-boundary, never
`/tmp`.

---

## G. Phased implementation plan

Dependency-ordered; each phase an independently-testable increment. **P1 and P4
can start immediately** (no cross-dependency); P2 is largely reuse; P3 blocks on
P1; P5 blocks on P2+P3+P4.

### P1 — `Ipe.Process` · *start immediately*
Stdlib module + runtime kernel + all anti-drift sites + WASM-deny wiring (§B).
- **SEAL:** `src/ipe-cli/tests/golden_stdlib_module_seal.rs` extended for
  `Ipe.Process`; a golden that a program
  using `Process.run`/`exec` emits Rust that `cargo build`s (`IPE_E2E=1`); a
  runtime soundness test (`runtime/tests/`) that spawn-failure, nonzero exit, and
  timeout each return a typed `Error` and never panic, and that a timeout actually
  **kills** the child. Red-team: `import Ipe.Process` under `--target wasm` yields
  IPE-N0030, and `available_on(Target::WasmClient)` is `false` for both kernels.

### P2 — sandbox integration · *mostly reuse; starts with P1*
Expose the existing `ipe_sandbox` jail to the `Process` kernel (jailed exec path),
with fail-closed detection + the unsandboxed flag (§D). No new sandbox mechanism —
wiring + a flag.
- **SEAL:** a runtime test that a jailed `Process.exec` of a network-touching /
  fs-escaping program is denied on the host platform; that absent jail tools →
  the unsandboxed flag is set AND timeout/output-cap still fire; `IPE-F4410`
  refusal path unchanged for the hosted posture.

### P3 — Ipê server (`Main.ipe` + `Runner.ipe`) · *blocks on P1*
The `Ipe.Http.Server` app using `do`/`parallelDo` (§C). Resolve the static-serving
decision (§C) — confirm the binary-body path or take `Server.static`.
- **SEAL:** the example builds and runs via the examples sweep (server shape,
  headless); an e2e that `POST /run` with a known-good emitted-Rust project returns
  the expected stdout in the transcript, and with a build-error project returns the
  `── Build ──` failure and no run.

### P4 — client SPA · *start immediately*
Second output pane, Run gating, `/run` fetch, unsandboxed warning box, Sky→Ipê
strip, GitHub link (§E). Pure static HTML/JS — no compiler dependency.
- **SEAL:** the wasm example's headless driver (`tools/scripts/lib/wasm-verify.mjs`)
  loads the page, edits to a good program (Run enabled), to a bad program (Run
  disabled + diagnostics shown), and — against a running P3 server — clicks Run and
  asserts the bottom pane fills. (Verify real interaction, not boot-only.)

### P5 — integration + CI · *blocks on P2+P3+P4*
Delete `src/playground/`, drop the workspace member, fold everything into
`examples/wasm/language-playground/`, fix all references (§F), add the temporary
sandbox CI matrix (§D).
- **SEAL:** `cargo build` the workspace with `src/playground` gone (member removed,
  no dangling ref); the examples sweep is green including the new server example;
  the sandbox matrix workflow passes on Linux/macOS/Windows(/FreeBSD) with both the
  benign-runs and the deny-path assertions.

---

## Prerequisite gaps found (surfaces that do not exist yet)

1. **`Ipe.Process` — does not exist.** Greenfield; this is the module the plan
   builds. No `src/stdlib/Ipe/Process.ipe`, no `Process_*` kernels. (Confirmed: no
   `Process` `.ipe` module anywhere in `src/stdlib/`.)
2. **No `Ipe.Tea.Web` / `Ipe.Web` server surface.** The task brief assumed a
   `Sky.Live` port. Ipê has no `Live` app surface; the correct and fully-shipped
   surface is **`Ipe.Http.Server`** (`Server.listen`, `Server.get/post`,
   `Server.body`, `Server.text/json/html/withStatus/withHeader/redirect`). This
   design uses it. The Sky `State`/`Update`/`View` TEA modules do not port — the
   client SPA owns UI state. Not a blocker; a design correction.
3. **No static-directory kernel** in `Ipe.Http.Server` (no `Server.static`/
   `serveDir`). Handled by serving the fixed asset set through ordinary handlers
   (§C option 1). **Sub-gap:** if the server response builders cannot carry a raw
   **byte** body (the wasm `.wasm` asset is binary) with an explicit content type,
   a small addition is needed (either a bytes-response builder or the
   `Server.static` kernel of §C option 2). Confirm in P3 before committing to
   option 1.
4. **`Crypto.randomToken` — exists.** `Ipe.Crypto.randomToken : Int -> Task Error
   String`. No gap.
5. **`Ipe.File` temp/dir surface — exists** (`mkdirAll`, `writeFile`,
   `readFile`/`readFileBytes`, `remove`, `tempDir`). No gap.
6. **`do`/`parallelDo` — exists** (ADR 0050, parser desugar to
   `Task.andThen`/`Task.parallel`). No gap.
7. **Sandbox crate — exists** (`ipe_sandbox`, per-platform build+run jails,
   fail-closed IPE-F4410). Reused. The only new work is wiring it to the `Process`
   kernel's jailed path (P2), not building a sandbox.
