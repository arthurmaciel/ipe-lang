# Ipê language playground (in-browser Ipê → Rust, sandboxed run)

A split-pane playground. You type Ipê on the left, and the parse → resolve →
typecheck → lower → emit pipeline runs **in your browser** as a WebAssembly
module (the `ipe-wasm` crate). The right pane shows the emitted Rust or the
compiler diagnostics almost at once. **Run** sends the emitted Rust to a local
Ipê server, which builds and runs it inside a bubblewrap jail and sends the
program output back.

## Layout

| Path | What it is |
|---|---|
| `index.html` | the three-pane UI (Ipê source \| emitted Rust \| program output) |
| `static/app.js` | the page logic |
| `static/vendor/ace/` | the vendored ACE editor (version, digests, and license in its `README.md`) |
| `pkg/` | git-ignored wasm-bindgen output the page loads |
| `setup/` | the setup program: wasm bundle, jail-runner install, offline cache warm |
| `server/` | an `Ipe.Server.Http` app: `GET /`, `/pkg`, `/static`, `GET /health`, `POST /run` |
| `jail-runner/` | a Rust workspace member: the sandboxed build+run harness |

## Prerequisites

- The `ipe` compiler binary (build it from this repo: `cargo build -p ipe`).
- The `wasm32-unknown-unknown` rustup target and the `wasm-bindgen` CLI
  (`cargo install wasm-bindgen-cli --version 0.2.126`).
- For Run only: `bwrap` (bubblewrap).

The setup program checks for each tool. If one is missing, it stops with an
install hint and a non-zero exit.

## Setup

```sh
cd examples/wasm/language-playground/setup
ipe run              # full setup
ipe run -- bundle    # only rebuild the wasm bundle (pkg/)
```

The full setup:

1. builds `ipe-wasm` for `wasm32-unknown-unknown` (release) and runs
   `wasm-bindgen` into `../pkg/`;
2. installs `playground-jail-runner` to
   `~/.cache/ipe/playground/bin/jail-runner` with `cargo install --locked`;
3. warms the offline dependency cache (`jail-runner prewarm`, into
   `~/.cache/ipe/playground-warm`, or `$IPE_PLAYGROUND_WARM_DIR` if set).
   Warming needs the network once and takes about 2.4 GB. Every jailed build
   after that runs offline.

Every step runs as a direct argv vector, never through a shell.

## Run the server

```sh
cd examples/wasm/language-playground/server
IPE_HTTP_BIND=127.0.0.1 ipe run
```

The server listens on `127.0.0.1:8000` and prints its launch URL on stdout:

```text
Ipê playground: open http://127.0.0.1:8000/#t=<launch token>
```

That URL is a per-launch secret: anyone who holds it can run code on this
machine through the jail. Keep stdout on your terminal; do not pipe it into a
shared log (journald, a CI log, a terminal-sharing session).

Open that exact URL. The page takes the token from the fragment, removes it
from the address bar, and keeps it in memory only, so reloading needs the
printed URL again. The browser can still keep the full URL, token included,
in its history and address-bar suggestions; that token is good only until the
server stops, since each launch mints a new one. Without the token the page still compiles, but Run stays
disabled. The server refuses to start unless `IPE_HTTP_BIND` is exactly
`127.0.0.1` and `IPE_SERVER_PORT` is unset or `8000` (so a relocated port, as
under `ipe watch`, is refused rather than guessed).

The server serves the page from the playground root, the parent of the
directory it starts in, so start it from `server/`. Before it listens it checks
that `index.html`, `static/app.js`, `pkg/ipe_wasm.js` and `pkg/ipe_wasm_bg.wasm`
exist there, and refuses to start otherwise, naming the missing file (for
`pkg/`, run `cd ../setup && ipe run -- bundle` first).

`GET /` serves `index.html` with `X-Frame-Options: DENY` and a
`Content-Security-Policy` (`Gate.pageContentSecurityPolicy`) whose
`frame-ancestors 'none'` stops any other page framing it, and whose
`script-src 'self' 'wasm-unsafe-eval'` admits only same-origin scripts. The
page holds the launch token, so it loads no third-party script: ACE is
vendored under `static/vendor/ace/`, and `index.html` has no inline script.
`/pkg` and `/static` are served statically. Nothing else under the playground
root is served.

| Route | Answers |
|---|---|
| `GET /health` | `{"ok":true}`, `Cache-Control: no-store`: the page enables Run only after this |
| `POST /run` | `{"rust": <emitted crate>}` → `{"ok", "unsandboxed", "output"}` |

Run executes code, so `POST /run` admits only the playground page served by
this server. `server/src/Gate.ipe` requires, before the body is read:

- `Host` is exactly `127.0.0.1:8000` or `localhost:8000` (refuses DNS rebinding);
- `Origin` is `http://` followed by that `Host` (refuses cross-site pages);
- `Sec-Fetch-Site` is absent or `same-origin`;
- `Content-Type` is `application/json` (a form post cannot send it);
- `X-Ipe-Playground-Token` equals the random token minted at startup, compared
  in constant time. No route serves the token: it exists only on the server's
  stdout and in the launch URL's fragment, which the browser never sends. A
  framed page refuses the token.

`GET /health` applies the same `Host` rule, and an `Origin`, when sent, must
match it. A restarted server mints a new token: open the new launch URL.

`POST /run` fails with one of these statuses:

| Status | Meaning |
|---|---|
| 403 | `Gate` refused the request's `Host`, `Origin`, `Sec-Fetch-Site`, or token |
| 415 | the `Content-Type` is not `application/json` |
| 413 | the body is over 1 MiB |
| 400 | the body is not `{"rust": String}` |
| 422 | the staging allowlist refused the crate |
| 500 | the jail itself failed |

Environment:

| Variable | Effect |
|---|---|
| `IPE_PLAYGROUND_JAIL_RUNNER` | absolute path of another jail runner. A relative name is refused, never looked up on `PATH`. |
| `IPE_PLAYGROUND_WARM_DIR` | the warm cache directory |
| `IPE_HTTP_REQUEST_TIMEOUT` | the per-request deadline in whole seconds above zero (default 30 s when unset or blank). The jail's wall-clock limit is 5 s under it, capped at the jail runner's 600 s. The server refuses to start on a value that is not digits, or on one below 15 s, since the jail wall must stay 5 s under the deadline (measured from handler start; the deadline runs from arrival, body read included). |

`HOME` must be an absolute path: runs are staged under it.

## How Run works

1. The in-browser compiler emits a Rust project: each file under a
   `// ==== path ====` banner, with the emitted `Cargo.toml` last.
   `jail-runner` builds the `Cargo.toml` only when it is exactly a manifest
   the compiler renders; the client picks only the runtime features, and any
   other manifest is refused.
2. The server checks the body's size before it decodes it. `server/src/Staging.ipe`
   then parses the banners into a staging plan. It accepts only `Cargo.toml` and
   `src/<segment>/…/<name>.rs`, where each segment is non-empty
   `[A-Za-z0-9_]`, with at most 64 files and 8 levels. It refuses a path
   outside that set, text before the first banner, a duplicate path, or a
   missing `Cargo.toml` / `src/main.rs`, and the whole request is rejected
   before any file is written.
3. `server/src/Runner.ipe` writes the plan under
   `~/.cache/ipe/playground-runs/<random token>/`. Each staged path is
   joined beneath that directory with `Path.under` (`server/src/Paths.ipe`),
   so a path the allowlist missed is still refused rather than written
   elsewhere. It then runs
   `jail-runner run <dir> --wall <secs> --warm <warm>` as a direct argv
   vector, with no shell.
4. `jail-runner` checks the layout again. It then builds the crate offline and
   runs the program inside a bubblewrap jail: network denied, host filesystem
   read-only, `prlimit` caps, a wall-clock kill, and (for the run phase) a
   seccomp filter that denies creating subprocesses. The jail is the
   `ipe_sandbox` crate the compiler SEAL uses.
5. The page shows the `── Build ──` / `── Run ──` / `── Error ──` transcript.

The security proofs live in `jail-runner/tests/sandbox_security.rs`. The
staging, gate, deadline, `/health`, and layout refusals live in
`server/tests/Main.ipe`.

## Static hosting (GitHub Pages)

The compiler pane needs no server. Deploy these files, keeping their relative
layout:

```
index.html
static/          (app.js and vendor/ace/)
pkg/ipe_wasm.js
pkg/ipe_wasm_bg.wasm
```

`index.html` carries the same policy in a `<meta>` tag (without
`frame-ancestors`, which only a header can carry), so a static host runs only
same-origin scripts too.

All asset and endpoint URLs are relative to the page, so it works under any
sub-path (e.g. `/compiler/playground/`). With no run server behind it, Run is
disabled with "Run needs the local server — see README". The page runs only
against the server it was loaded from. A CI job builds `pkg/` with:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
cargo build -p ipe-wasm --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir pkg --out-name ipe_wasm \
  "$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/wasm32-unknown-unknown/release/ipe_wasm.wasm"
```

(`cd setup && ipe run -- bundle` runs the same steps.)

## Browser check

`tools/scripts/lib/playground-verify.mjs` drives the page in headless Chromium
(Playwright). Install its pinned dependencies once:

```sh
(cd tools/scripts/lib && npm ci && npx playwright install chromium)
node tools/scripts/lib/playground-verify.mjs examples/wasm/language-playground --ipe "$(command -v ipe)"  # Pages shape, plus the real server's headers
node tools/scripts/lib/playground-verify.mjs --live 'http://127.0.0.1:8000/#t=<token>'                 # the printed launch URL; Run through the jail
```

The static mode needs `pkg/` built and `127.0.0.1:8000` free. It serves the
page with no run server behind it (the GitHub Pages shape) and checks the
compiler boots and Run degrades to the local-server notice. It also starts the
real run server (`<ipe> run` in `server/`) and requires its `GET /` to deliver
`index.html` with exactly one `X-Frame-Options: DENY` and exactly one
Content-Security-Policy equal to the `<meta>` policy plus
`frame-ancestors 'none'` (compared directive by directive); then that each
delivered framing header alone makes Chromium refuse the page to a
cross-origin frame, while a control frame of the same origin loads. The
`playground-page` CI job runs it on every relevant change. The live mode
checks the same served headers, and that a frame, or a missing or malformed
token, keeps Run disabled.
