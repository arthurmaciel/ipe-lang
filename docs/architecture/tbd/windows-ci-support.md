# Windows support for the example-sweep CI

Status: design. Adds `windows-latest` to `.github/workflows/examples-sweep.yml`,
informational-first, reusing the existing bash harness under Git Bash. No native
PowerShell reimplementation (a second harness is a second source of truth and a
silent-divergence vector).

## Verdict per shape

| Shape | Windows | Why |
|---|---|---|
| cli | RUN | Pure console; `println!` emits `\n`, CRLF-clean at source. |
| server | RUN | tokio/axum/sqlx are Windows-portable; rustls (pure-Rust TLS) avoids the OpenSSL cross-compile hazard; loopback + curl portable. |
| live | RUN as boot-check, browser round-trip SKIP | axum boots identically; no Playwright browsers on Windows → degrades to the server boot check. |
| tui | SKIP in the sweep | The emitted `Tui.app` binary enters crossterm raw mode → needs a real console; Git-Bash CI has no ConPTY bridge. Real render coverage comes from the unit lane (below), not a faked sweep RUN. |
| webview | RUN, no xvfb, no-panic gate — UNPROVEN until a real webview build is seen | `windows-latest` ships an interactive desktop + preinstalled WebView2, so a `wry`/`tao` app opens a real window with no X server. Gated on the harness actually emitting `--features webview` (otherwise the stub passes — a fake green). |

Tui render coverage lives in the `cargo test` lane: `element_to_cells`
(`src/runtime/rust/src/tui/layout.rs`) is a pure `fn(&Element, cols, rows) ->
String` — pty-free, deterministic across OSes. Add `windows-latest` to the
unit-test matrix so it is exercised there.

## Windows-only failure class: shared-`.exe` handle lock

All examples share one `CARGO_TARGET_DIR/.../app.exe`. A webview/server RUN that
leaves a process (or WebView2 children) alive holds the handle, so the next
`cargo build` dies with `Access is denied (os error 5)` and every downstream row
falsely reads `cargo-fail`. Fix: `taskkill //F //T` in `build_rust` **pre-build**,
plus an os-error-5 retry arm. `//T` (tree-kill) is load-bearing for WebView2
children, which can reparent outside the app's process tree.

This requires MSYS argument conversion left **ON**: `//F` reaches `taskkill` as
`/F` only because conversion collapses `//`→`/`. Disabling conversion
(`MSYS_NO_PATHCONV`) breaks the reap silently. The harness passes only relative
forward-slash paths and schemed URLs to native exes, so no leading-slash argv is
mangled today; keep conversion on and use `MSYS2_ARG_CONV_EXCL` as a scoped
per-call hatch if a future native call needs a literal `/flag`.

## Harness portability under Git Bash

| Breakage | Fix |
|---|---|
| CRLF-corrupted scripts (autocrlf checkout) | Root `.gitattributes` pinning `eol=lf` on `*.sh`/`*.py`/`*.ipe`/`*.mjs`; belt: `core.autocrlf false` before checkout. |
| `.exe` resolution miss — every no-extension probe misses; falls through to a freshest-file `find` in the shared dir → wrong-binary run | In both `resolve_bin` and the binary-probe loop, try the OS-gated `.exe` candidate first; a built-but-unlocatable binary is a counted RED (`binmiss`), never a SKIP or the `ls -t` guess. |
| `python3` name (Windows exposes `python`) | Resolve once: `IPE_PYTHON="$(command -v python3 || command -v python)"`; belt: `actions/setup-python`. Empty → `exit 2` (fail-closed preserved). |
| `CARGO_TARGET_DIR` backslashes choke bash builtins | `cygpath -u` normalize once on Windows, or set a forward-slash value in the job env. |
| `timeout` (needs GNU `-k`) | Git for Windows bundles coreutils `timeout.exe`; add a fail-loud preflight rather than a degrade-to-SKIP. |
| `pkill`/`script`/`xvfb-run` absent | Already handled (guarded no-ops / SKIP arms). Note: `reap` being a Windows no-op is *why* the kill must live in `build_rust`. |
| ripgrep, Node/Playwright | `rg` preinstalled (prefer it; unpinned `choco` fallback only on image regression). Run `npm ci` without Playwright browsers; `WEB_OK=0` degrades live to the server boot check. |

## Build: MSVC dynamic CRT only

Default `x86_64-pc-windows-msvc` dynamic-CRT `cargo build` — no `RUSTFLAGS`, no
`+crt-static`, no `rustup target add`. `+crt-static` belongs to `ipe build
--static` (`StaticWindows`; see `static-compilation.md`) and is out of sweep
scope. Sharpest first-run risk: native C-toolchain deps (`libsqlite3-sys`, the
rustls crypto provider). A link failure surfaces as a loud `cargo-fail` RED, not
a silent green; provision nasm/cmake or pin a pure-Rust provider if it recurs.

## Line-ending normalization (EQUIVALENCE path)

Both Rust and Go emit `\n` on stdout/sockets on Windows, so a CRLF delta is a
VCS/text-mode artifact. Normalize line-ending-scoped only (`sed 's/\r$//'` /
`\r\n`→`\n`) — never `tr -d '\r'`, which also deletes payload CR and can make two
different outputs compare equal. Defense in depth: `.gitattributes eol=lf` at the
source, `sed 's/\r$//'` at the harness sink, `\r\n`→`\n` in the Python
normalizers. Keep the reference oracle ubuntu-only — reference bytes are
OS-invariant, so a second EQUIVALENCE host adds cost, not coverage, and removes
CRLF from the gating path entirely.

## Gating

Windows-informational-first, decoupled per-OS via an `experimental` matrix flag
(`continue-on-error: ${{ matrix.experimental }}`). When parity flips the reference
hosts to gating, Windows keeps `experimental: true` until it independently reaches
one all-green BUILD+RUN sweep — Windows carries novel spurious-RED vectors (the
handle-lock cascade, MSYS signal→native-child gaps, loopback timing) the other
hosts never exercise. The flip is a filed criterion, never a calendar timebox and
never informational-forever. Green-neutral SKIPs (tui, live-browser) do not block
the gate.

## Security / supply chain

No untrusted-code build on the PR path: vendored first-party examples +
`Cargo.lock`-pinned crates only, no `ipe add`/FFI-inspector step. WebView2 is a
Microsoft-preinstalled OS component, not a fetched artifact. All path/CRLF
handling lives in the bash/python harness; the panic regex still fails loud on any
abort in RUN output.
