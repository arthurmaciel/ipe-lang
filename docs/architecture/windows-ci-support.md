# Windows support for the ipê example-sweep CI

Status: design (spec only — no code, no build)
Scope: add `windows-latest` to `.github/workflows/examples-sweep.yml` now (not deferred)
Supersedes: the Windows-deferral note in `docs/architecture/examples-sweep-port.md`

---

## Executive summary

1. Add `windows-latest` to the existing sweep matrix now, informational-first. The
   deferral in the port doc ("Git Bash noise while most rows fail") is retired: Windows
   is in-scope.
2. Reuse the existing bash harness under Git Bash (`shell: bash`). A native PowerShell
   reimplementation is rejected — a second harness is a second source of truth and a
   silent-divergence vector. `../sky` proves Git-Bash reuse viable on `windows-latest`.
3. The ported `scripts/lib/checks.sh` is already ~80% Windows-aware (host detection at
   `checks.sh:33-45`, tui SKIP `checks.sh:214-217`, webview RUN `checks.sh:241-242`,
   `pkill`-guarded `reap` `checks.sh:76`). The real work is what the port DROPPED: the
   shared-target `.exe` handle-lock reap and a `python3` fallback.
4. The one genuinely Windows-only NEW failure class is a spurious-RED cascade: all
   examples share one `CARGO_TARGET_DIR/…/sky-app.exe`; a webview/server RUN that lingers
   holds the handle, so the next example's `cargo build` dies `Access is denied (os error
   5)` and every downstream row falsely reads `cargo-fail`. Fix: `taskkill //F //T` in
   `build_rust` pre-build + an os-error-5 retry arm.
5. CRLF is a VCS/text-mode artifact, not a program-output divergence — Rust and Go both
   emit `\n` to stdout/sockets on Windows. Close it at the source (`.gitattributes
   eol=lf`) and at the sink (line-ending-scoped normalization), and keep the Go≡Rust
   oracle ubuntu-only so CRLF never touches the gating path.
6. Webview RUNS on Windows (no xvfb): the runner has a real interactive desktop + a
   preinstalled WebView2 runtime. This is the one shape where Windows is strictly better
   than headless Linux — subject to the OPEN feature-emission check (D4 below).
7. Tui SKIPs in the sweep (the example binary enters crossterm raw mode → needs a
   console). Genuine Windows Tui coverage comes from `element_to_cells` in the `cargo
   test` unit lane, not from faking a sweep RUN.
8. Build is default MSVC dynamic-CRT only. `+crt-static` is the `ipe build --static`
   concern (`static-compilation.md` `StaticWindows`), explicitly out of sweep scope.
9. Gating: windows-informational-first, decoupled from ubuntu/macOS via a per-OS
   `experimental` matrix flag, with a filed flip-trigger (one independent all-green
   Windows BUILD+RUN sweep). Never a calendar timebox; never informational-forever.
10. Supply chain unchanged: vendored examples + `Cargo.lock`-pinned crates only. No `ipe
    add`, no `playwright --with-deps`, no Go/Haskell reference on the Windows PR path.
    WebView2 is a Microsoft-preinstalled OS component, not a fetched artifact.
11. Fail-loud is invariant: every missing-tool preflight is a hard `exit 2`; a
    built-but-unlocatable binary is a counted RED (`binmiss`), never a SKIP; the
    macOS-only server SKIP stays `IPE_HOST_OS=macos`-gated so Windows cannot borrow it to
    mask a real `noserve`.
12. First-run expectation: most rows RED on all three hosts (ipe implements only
    `Ipe.*`). Windows adds no new red class — it re-runs the same examples on a third
    target; the informational table honestly surfaces the pre-parity state.

## Webview-on-Windows verdict

RUN, gated on no-panic (boot + survive 8 s), with NO xvfb — adopt `../sky` verbatim.
`windows-latest` GitHub runners ship an interactive desktop session and the WebView2
Evergreen runtime preinstalled, so a `wry`/`tao` app constructs a real native window with
no X server. The ported `checks.sh:241-242` already carries the correct arm
(`timeout -k 5 8 "$abin"`; `-k 5` escalates to SIGKILL so a window ignoring SIGTERM is
still reaped; verdict = no PANIC string in 8 s). Two conditions gate the claim:

- The `taskkill //F //T` reap (D1) is the essential partner — the WebView2 process
  (`msedgewebview2.exe` + children) is exactly what lingers and locks the next build.
  The reap list is a DELIBERATE SUPERSET of `../sky`'s proven kill set
  (`sky-app.exe` / `winpty.exe` / `winpty-agent.exe`): it adds `msedgewebview2.exe //IM`.
  This is SOUNDER — WebView2 child processes can reparent OUTSIDE the app's process tree,
  so `//T` (tree-kill) alone may miss them and leave the shared-target handle locked — but
  the extra `//IM` is UNPROVEN on any runner (no real webview build has been observed on
  Windows). It stays behind the same UNPROVEN gate as the webview RUN below: neither is
  trusted until a real (non-stub, `--features webview`) webview build is observed on
  Windows.
- OPEN (D4): the emitted `sky-out/rust/Cargo.toml` must actually enable `--features
  webview`. The runtime gates webview behind `webview = ["wry","tao","live"]`, EXCLUDED
  from `full` (`runtime/Cargo.toml:98,119`), and a stub compiles for `!webview`
  (`webview_stub.go` analogue). If ipe does not emit the feature, the Windows webview RUN
  exercises the stub (exits 0, passes the no-panic gate) — a fake green. This must be
  verified before the webview row is trusted; until verified, treat a webview "pass" on
  Windows as UNPROVEN, not green. The webview RUN is UNPROVEN until a real (non-stub,
  `--features webview`) webview build is observed on Windows — the same gate that covers
  the `msedgewebview2.exe //IM` reap superset above.

## Top open decisions

- D-A (MSYS path-conversion vs the `//F` reap — must resolve before the reap lands).
  Blanket-disabling MSYS argument conversion (`MSYS_NO_PATHCONV=1` /
  `MSYS2_ARG_CONV_EXCL='*'`) is INCOMPATIBLE with the `taskkill //F //T //IM` idiom: `//F`
  survives as `/F` only because conversion collapses `//`→`/`. Disable conversion and
  `taskkill` receives literal `//F`, rejects it, and the reap silently no-ops — bringing
  back the exact handle-lock cascade the reap exists to fix. Recommendation: keep MSYS
  conversion ON (audited: the harness passes only relative `--out sky-out/rust` /
  `--manifest-path …` and schemed `http://…` URLs to native exes — no leading-slash argv
  reaches a native exe today), use `//F` for the reap, and document
  `MSYS2_ARG_CONV_EXCL` as a per-call escape hatch only if a future native call needs a
  literal `/flag`. OPEN: confirm no future native-exe call needs a literal leading-slash
  path.
- D-B (gating flip-trigger). Decoupled per-OS `experimental` flag is the agreed shape.
  OPEN: the informational→gating flip for Windows must be a filed gate with a concrete
  criterion ("one independent all-green Windows BUILD+RUN sweep"), not open-ended —
  otherwise it decays into permanently-unenforced Windows (a silent-skip of gating,
  violating the no-deferral principle).
- D-C (webview feature emission). See the webview verdict / D4 — whether ipe emits
  `--features webview` for a Webview example on `x86_64-pc-windows-msvc`, and whether that
  crate links WebView2. Verification-required before trusting the webview row.
- D-D (headless Tui RUN, filed follow-up). A `IPE_TUI_HEADLESS` one-shot render mode
  (`init → view → element_to_cells → stdout → exit 0`, no crossterm/TTY) would flip the
  sweep's tui RUN column green on ALL hosts and unlock a real cell-grid EQUIVALENCE via
  `equivalence_tui_grid.py`. This is a runtime change, out of scope for "add Windows CI" — filed
  as a tracked task, not smuggled into this job.

---

## Q1 — the `windows-latest` job: matrix, toolchain, shell

### Decision

Add `windows-latest` as a matrix entry (not a separate job); pin the MSVC toolchain via
the existing `rust-toolchain@stable` host default; run the bash harness under Git Bash
(`shell: bash`).

### Matrix

Move from a bare `os:` list to an `include:` with a per-OS `experimental` flag so gating
decouples per host (Q6):

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - { os: ubuntu-latest,  experimental: true }   # dev-reference host
      - { os: macos-latest,   experimental: true }
      - { os: windows-latest, experimental: true }    # stays true until independently green
runs-on: ${{ matrix.os }}
continue-on-error: ${{ matrix.experimental }}
```

The whole job is `continue-on-error: true` today because ipe is pre-parity; the flag
preserves that and makes the eventual per-host flip representable.

### Toolchain

`dtolnay/rust-toolchain@stable` resolves the host default triple, which on
`windows-latest` is `x86_64-pc-windows-msvc`. The runner ships VS Build Tools + the
Windows SDK (MSVC `link.exe`). No `rustup target add`, no `-gnu`, no `+crt-static`
(Q5). Zero toolchain additions — the same step covers all three hosts.

### Shell — Git Bash (`shell: bash`), recommended

| Option | Verdict |
|---|---|
| Git Bash reuse (`shell: bash`) | ADOPT |
| Native PowerShell / batch reimplementation | Reject — a second SSOT; every example-shape change made twice; drift between them is itself a silent-skip vector; doubles the soundness/audit surface. |
| Thin PowerShell wrapper around bash | Reject — `shell: bash` already IS the wrapper GitHub provides; a PS layer adds quoting/exit-code translation for zero benefit. |

Git-Bash reuse keeps ONE behavioural contract across three OSes (a correctness property: a
shape cannot pass on Linux and silently mean something different on Windows). The harness
is already Windows-aware. `windows-latest` ships Git for Windows; GitHub maps
`shell: bash` to `C:\Program Files\Git\bin\bash.exe`.

Git-Bash gotchas to foreclose (each fixed in Q2): (a) CRLF-corrupted script execution on
autocrlf checkout; (b) MSYS argv path-mangling of leading-slash args to native `.exe`s;
(c) `.exe` suffix defeating `resolve_bin`/`SKYC_BIN` probes; (d) `python3` not on PATH;
(e) `.exe` file-handle locks in the shared `CARGO_TARGET_DIR`; (f) `CARGO_TARGET_DIR`
backslashes choking bash builtins; (g) `pkill`/`script`/`xvfb-run` absent.

---

## Q2 — harness portability under Git Bash: every breakage → fix

| # | Sev | Breakage (cited) | Fix |
|---|---|---|---|
| B1 | CRITICAL (fail-loud) | CRLF-corrupted scripts. autocrlf checkout rewrites `scripts/*.sh` to CRLF → bash: `$'\r': command not found`, or a script that half-runs then dies mid-sweep. No `.gitattributes` exists (verified absent). | Commit `.gitattributes` pinning LF (B7). Belt: `git config --global core.autocrlf false` in a Windows-only step BEFORE checkout. `.gitattributes` is authoritative; the config line is the ordering/contributor-config floor. |
| B2 | HIGH (silent mis-run) | `.exe` resolution miss. On Windows EVERY explicit no-extension probe misses (`sky-app` vs `sky-app.exe`): `resolve_bin` (`checks.sh:115`) probes `sky-app`; the `SKYC_BIN` loop (`env.sh:73-85`) probes `ipe`. cargo-msvc emits `sky-app.exe` / `ipe.exe`. Every explicit candidate misses → both fall through to a `find … | xargs ls -t | head -1` freshest-file heuristic in the SHARED target dir → an `ls -t` race can pick a stray/other-example executable → WRONG binary run = silent false-green. | In BOTH sites, the OS-gated `.exe` candidates MUST precede the `find` fallback: `ext=""; [ "${IPE_HOST_OS:-}" = windows ] && ext=".exe"`; probe `…/sky-app$ext` (`resolve_bin`, `checks.sh:115`) and `…/ipe$ext` (the `SKYC_BIN` loop, `env.sh:73-85`) FIRST, reaching `find` only after every explicit candidate misses. A built-but-unlocatable binary must then surface as a counted RED `binmiss`, never a SKIP and never the freshest-file guess. |
| B3 | HIGH (spurious-RED cascade) | `.exe` handle-lock. Single shared `CARGO_TARGET_DIR` holds one `sky-app.exe`; a webview/server RUN leaving the process (or WebView2 children) alive holds the handle → next `cargo build` fails `Access is denied (os error 5)` at file-remove. `reap()` (`checks.sh:76`) is a `command -v pkill`-guarded no-op on Windows AND runs only BETWEEN examples — wrong locus. GNU `timeout` does not tree-kill a native GUI process. THIS IS THE GAP THE PORT DROPPED. | Port `_win_reap_app` into `examples-sweep.sh` `build_rust`: `taskkill //F //T //IM sky-app.exe`, `msedgewebview2.exe`, `winpty.exe`, `winpty-agent.exe`. (`../sky`'s proven list is `sky-app.exe`/`winpty.exe`/`winpty-agent.exe`; `msedgewebview2.exe //IM` is a deliberate superset — sound because WebView2 children can reparent outside the tree, but UNPROVEN until a real webview build runs, see the webview verdict.) Call PRE-build; add an os-error-5 retry arm (`grep -qiE 'Access is denied \(os error 5\)|failed to remove file' … && _win_reap_app && sleep 3 && continue`). NOTE: the alternation `|` is UNESCAPED — under `grep -E` (ERE) `\|` is a LITERAL pipe, matches neither real message, and the retry never fires; port `../sky` `examples-sweep.sh:147` verbatim. `//T` (tree-kill) is load-bearing for WebView2 children. Requires MSYS conversion ON (D-A). |
| B4 | MED (fail-closed) | `python3` name. `free_port` (`checks.sh:72`) and preflight (`examples-sweep.sh:77`) call `python3`; Windows exposes `python`. → preflight `exit 2` aborts the whole sweep. Loud but wrong. | Resolve once in `env.sh`: `export IPE_PYTHON="${IPE_PYTHON:-$(command -v python3 || command -v python)}"`; replace bare `python3` at both sites with `"$IPE_PYTHON"`. Belt: `actions/setup-python@v5` guarantees a `python`. Fail-closed preserved (empty `IPE_PYTHON` → exit 2). |
| B5 | MED (bash-builtin break) | `CARGO_TARGET_DIR` backslashes. The yml sets it to `${{ github.workspace }}/.cache/…` → `D:\a\…` under bash; native `cargo.exe` tolerates mixed separators but `mkdir -p` (`env.sh:30`) and `[ -x … ]` builtins choke, and B2's `.exe` probe strings become malformed. | Normalize once at the top of `env.sh` when `IPE_HOST_OS=windows`: `CARGO_TARGET_DIR="$(cygpath -u "$CARGO_TARGET_DIR")"` (or set a forward-slash value in the Windows job env). |
| B6 | LOW→verify | `timeout` availability. Harness uses bare `timeout` pervasively incl. `timeout -k 5 8` (needs GNU `-k`). Git for Windows BUNDLES coreutils `timeout.exe` (`../sky` relies on it, no install). | No install step. Add a fail-loud preflight: `command -v timeout >/dev/null || { echo "ERROR: timeout(1) missing"; exit 2; }` so a runner-image change fails loud rather than degrading a whole column to SKIP. |
| B7 | (repo) | `.gitattributes` absent. | Commit at repo root: `*.sh text eol=lf`, `*.py text eol=lf`, `*.ipe text eol=lf`, `*.mjs text eol=lf`, `scripts/equivalence-checks/equivalence-classification.tsv text eol=lf`, and (phase-2) the oracle `*.txt`/`*.expected` → `text eol=lf`. Also pins the staleness-hash input cross-OS (Q4). |
| B8 | LOW (present but no-op) | MSYS argv path-mangling. Audit: skyc/cargo receive only RELATIVE forward-slash paths (`--out sky-out/rust`, `--manifest-path sky-out/rust/Cargo.toml`); curl uses schemed URLs. No leading-slash argv reaches a native exe today → no live bug. | Do NOT blanket-disable conversion (D-A: it breaks the `//F` reap). Keep conversion ON; document `MSYS2_ARG_CONV_EXCL` as a scoped per-call hatch. |
| B9 | LOW (handled) | `pkill`/`script`/`xvfb-run` absent. Already: `reap` guards on `pkill` (no-op), `exercise_tui` Windows SKIP, `exercise_webview` Windows arm omits xvfb. | No fix — but note `reap` being a Windows no-op is WHY B3's kill must live in `build_rust`. |
| B10 | LOW (guard) | ripgrep. `is_out_of_scope` needs `rg`; its absence hard-exits (loud). rg is preinstalled on `windows-latest`. | Rely on the preinstalled `rg` (`command -v rg`); the `choco install ripgrep -y` fallback exists only for a runner-image regression and fetches LATEST (NOT version-pinned — do not claim "pinned"). Supply-chain: prefer the trusted preinstalled binary; the fetch path is a fail-loud last resort, not the norm. |
| B11 | LOW | Node deps. Live browser round-trip needs chromium; `playwright install --with-deps` is unsupported on the Git-Bash runner (and a supply-chain fetch). | On Windows run `npm ci`/`npm install` WITHOUT Playwright browsers. `WEB_OK=0` (`checks.sh:82-95`, already forced when `/usr/bin/chromium` absent) → `exercise_live` degrades to `exercise_server` boot check. Mirror `../sky`. |

Subprocess/socket model (server & live RUN): `exercise_server` binds a python-allocated
ephemeral `127.0.0.1` port, backgrounds the binary, polls `curl http://127.0.0.1:$port/`,
tears down with `kill -TERM`/`kill -KILL`. On Windows: `curl.exe` ships on the runner;
loopback bind+connect works without a Defender prompt on hosted runners; MSYS `kill`
terminates a plain console server. The only weakness is GUI/detached trees (webview),
covered by B3's `taskkill //T`. Portable as-is.

Timeouts need no portable-guard rewrite (B6). Fractional `sleep`, `df -Pk`, `seq`,
`mktemp -d`, process substitution all resolve under Git-Bash coreutils.

---

## Q3 — per-shape Windows behaviour

| Shape | Windows verdict | Rationale |
|---|---|---|
| CLI | RUN (+ stdout-equiv when EQUIV on) | Pure console; `exercise_cli` under MSYS `timeout` after B2. Rust `println!` emits `\n` — stdout CRLF-clean at source. Mirror `../sky` cli-stdout-equiv. |
| server | RUN (+ body-equiv when EQUIV on) | tokio/axum/sqlx Windows-portable; rustls (pure-Rust TLS) avoids the OpenSSL cross-compile hazard; loopback + curl portable. The macOS-only SKIP (`checks.sh:184`) is `IPE_HOST_OS=macos`-gated, so Windows genuinely connects and must produce a real `ok`/`noserve` — never a borrowed SKIP. Mirror `../sky` server-body-equiv-RUN. |
| live | RUN as boot-check; browser round-trip SKIP | axum boots identically; no `--with-deps`/chromium on Windows → `WEB_OK=0` → `exercise_live` degrades to `exercise_server` (`checks.sh:194-199`). Honest SKIP of the browser layer, real RUN of the serve layer. Mirror `../sky` live-browser-deps-skip. |
| Tui | SKIP in the sweep; real coverage via unit lane | The built `Tui.app` binary enters crossterm raw-mode/alt-screen → needs a real console/pty. Git-Bash CI has no ConPTY/node-pty bridge (`winpty` needs an interactive console it lacks); Ipe.Tui REFUSES a non-TTY. `checks.sh:214-217` already SKIPs (green-neutral, printed reason). Do NOT fake a pass. |
| webview | RUN, no xvfb, no-panic gate — UNPROVEN until a real webview build is seen | WebView2 preinstalled + interactive desktop → real window, no X server. `checks.sh:241-242` arm present. Subject to D-C (feature emission) and B3 (reap — whose `msedgewebview2.exe //IM` entry is a deliberate superset of `../sky`'s kill list, sound but UNPROVEN, under the same gate). The one shape where Windows beats headless Linux — once a non-stub `--features webview` build is observed on Windows. |
| fyne | SKIP (Go-FFI shape) | Never enters the Rust set (`examples-sweep.sh:264-266`; `examples-sweep-port.md`). OS-independent. |

### Tui: the `element_to_cells` question, resolved honestly

Two distinct "tui tests" must not be conflated:

- The sweep's tui RUN drives the actual compiled `Tui.app` binary → needs a console → SKIP
  on a headless Windows runner. Forcing it green would be a fake pass.
- `element_to_cells` (`runtime/src/sky_runtime/tui/layout.rs:2426`, `pub fn
  element_to_cells<M: Clone>(view, cols, rows) -> String`) is a pure, pty-free, headless
  cell renderer — but it is a library function reached by `cargo test`, NOT by the example
  binary's entry point.

Resolution (both adopted):
- Now: the sweep SKIPs tui on Windows (honest); the `cargo test` unit-test matrix includes
  `windows-latest` (e.g. `cargo test -p sky-runtime-rust --features tui`) so
  `element_to_cells` is genuinely exercised on Windows — real cell-render coverage where
  `../sky`'s pty-based tui had to SKIP entirely. This lives in the test lane, not the
  sweep step (keeps the harness's single responsibility intact).
- Filed (D-D): a runtime `IPE_TUI_HEADLESS` one-shot mode so the example binary renders one
  frame via `element_to_cells` and exits 0 without crossterm — flips the sweep tui RUN
  column green on all hosts and unlocks `equivalence_tui_grid.py` cell-EQUIVALENCE. Runtime change,
  out of scope here; tracked, not faked.

---

## Q4 — oracle / EQUIVALENCE line-ending normalization

EQUIVALENCE is dormant today (`IPE_SWEEP_NO_EQUIV=1`, `examples-sweep.yml:66`; this repo has no
Haskell-`sky` Go reference). The normalization is designed airtight now so turning EQUIVALENCE
on in phase-2 cannot introduce a spurious-DIFFER.

Root fact: Rust `println!`/`print!` and Go `fmt.Print*` both emit `\n` (no text-mode
translation on stdout) on Windows; HTTP bodies cross the socket byte-identically both
backends; Ipe.Live templates are LF string constants in the runtime `.rs` sources. So the
program output is CRLF-clean on both sides — a CRLF delta is provably a VCS/text-mode
artifact, and normalizing it RESTORES the true comparison (a correctness requirement, not
a masking hack). It is the parse-don't-validate move applied to the diff input:
canonicalize the line-terminator once, then compare.

Normalization must be line-ending-scoped, NOT a blanket CR strip. `tr -d '\r'` also
deletes a deliberate mid-line `\r` (progress bars, `\r`-overwrite output) → can make two
genuinely-different outputs compare EQUAL (a false-equal, worse than spurious-DIFFER
because it masks a real inequivalence). Use `sed 's/\r$//'` / `\r\n`→`\n` — payload CR
preserved.

Defense in depth, three layers:
1. Source — `.gitattributes eol=lf` (B7) on `*.ipe`, scripts, and the phase-2 oracle
   `*.txt`/`*.expected`. The checked-out oracle stays LF on Windows; also closes B1 and
   stabilizes the `sha256(source)` staleness key cross-OS.
2. Harness sink — change `norm()` (`examples-sweep.sh:151`) from
   `grep -v '^[[:space:]]*$'` to `grep -v '^[[:space:]]*$' | sed 's/\r$//'`. Lands NOW
   even with EQUIVALENCE dormant (cheap; guarantees a future Windows-emitted stream can't
   spuriously DIFFER regardless of how a fixture got its endings). Apply the same
   `\r`-strip to the server-body compare (`checks.sh:342`) as defense-in-depth.
3. Python normalizers — `equivalence_normalize_html.py:223` reads text mode `encoding='utf-8'`;
   add `.replace('\r\n','\n')` immediately after read and pin stdout to `\n`.
   `equivalence_tui_grid.py` reads `'rb'` + `rstrip()` (already CRLF-tolerant per row); add an
   explicit `\r\n`→`\n` at ingest for interior CR. Moot while tui SKIPs; cheap correctness
   for D-D.

Architectural rule: keep the Go≡Rust oracle ubuntu-only (`if: matrix.os ==
'ubuntu-latest'`, as `../sky` does). Reference bytes are OS-invariant for deterministic
programs, so a second EQUIVALENCE host adds cost, not coverage — and it removes the entire
Windows-CRLF-in-oracle surface from the GATING path. Windows' distinct value is
cross-platform MSVC build+run soundness, not re-checking Go parity. The layer-2/3
normalizers are therefore defense-in-depth for Windows, not load-bearing — keep them
anyway (phase-2 may add a deterministic-stdout oracle a Windows-native tool could touch).

---

## Q5 — MSVC / CRT build

Confirmed: the sweep needs only the default MSVC dynamic-CRT `cargo build` + run. Emit NO
`RUSTFLAGS`, NO `.cargo/config.toml` `+crt-static`, NO `rustup target add`. The
`windows-latest` runner ships the MSVC toolchain + dynamic UCRT/vcruntime; the emitted
crate builds byte-for-byte with the same `cargo build --manifest-path sky-out/rust/
Cargo.toml` invocation as Linux/macOS.

`+crt-static` belongs exclusively to `ipe build --static` — the `StaticWindows` BuildPlan
variant in `docs/architecture/static-compilation.md`, which IS the `+crt-static` case by
construction (the doc removes a `crt_static: bool` to make the non-static-"static" state
unrepresentable). Adding it to the sweep would (a) diverge from the real default build
users get and (b) risk link failures against dynamically-linked deps. Static-CRT is
validated separately in the static-compilation design's Windows fixtures, non-gating.

Sharpest first-run build risk (surface, don't pre-solve): native C-toolchain deps on
windows-msvc — `libsqlite3-sys` (bundled `cc`, MSVC `cl.exe` present on runner) and the
rustls crypto provider (`aws-lc-rs`/`ring` historically need nasm/cmake on windows-msvc).
If a dep fails to link it surfaces as a LOUD `cargo-fail` RED row, not a silent green —
file it if it fires (no-deferral). If it recurs, either provision nasm/cmake in the
Windows job or pin a pure-Rust rustls provider in the emitted crate.

---

## Q6 — gating posture + honest first-run

Windows-informational-first, decoupled per-OS. Add `windows-latest` under the existing
`continue-on-error` mechanism via the per-OS `experimental` flag (Q1). During pre-parity
the entire job is non-gating on every host; Windows enters on identical footing (prints
its table, uploads its artifact, surfaces its verdict, never fails the workflow).

When parity flips the reference hosts to gating (`experimental: false` after ubuntu/macOS
reach all-green BUILD+RUN — the tracked event in `examples-sweep-port.md`), Windows KEEPS
`experimental: true` until it independently reaches one all-green Windows BUILD+RUN sweep.
Rationale: Windows carries novel spurious-RED vectors Linux/macOS CI has never exercised
(the `.exe` handle-lock cascade, MSYS signal→native-child gaps, loopback/ephemeral-port
timing, backslash target-dir handling). A per-OS flag makes "a Windows-only flake blocks a
reference-host-green PR" UNREPRESENTABLE by construction — strictly safer than a shared
flag or a calendar timebox.

Green-neutral SKIPs do NOT block the gate: per the verdict logic
(`examples-sweep.sh:375-377`), a `skip` row is neither RED nor counted GREEN, and `n/a`
EQUIVALENCE doesn't fail. So Windows tui-SKIP and live-browser-SKIP will not spuriously fail the
gate once it flips — Windows fails only on a genuine `ipe-fail`/`cargo-fail`/`panic`/
`noserve`/`DIFFER`.

Flip-trigger is a filed gate (D-B), not open-ended: informational-forever would itself be
an unenforced-gating silent-skip.

Honest first-run: most rows RED on all three hosts (ipe implements only `Ipe.*`;
`Ipe.*`/server/live/tui/webview → ipe-fail or cargo-fail everywhere). Windows adds no new
red CLASS — it re-runs the same examples on a third target. What shows green early on
Windows: `Ipe.*`-only cli/server build+run and (subject to D-C) webview boot. Every
Windows-specific defect the first run surfaces (e.g. a runtime `cfg(unix)`-only call
reached unconditionally on the server path) is filed and fixed — that surfacing is the
value of adding Windows now.

---

## Security / supply-chain posture

- No untrusted-code build in the PR critical path. The Windows sweep builds only vendored
  first-party examples + `Cargo.lock`-pinned crates (same set as Linux, actions/cache
  restored). No `ipe add`/FFI-inspector step (that would `cargo build` an untrusted crate
  with build.rs/proc-macros) — consistent with the FFI-sandbox gate still being open and
  the nightly-only live-Go boundary in `sweep-and-parity-plan.md`.
- WebView2 runtime is Microsoft-shipped and preinstalled — a trusted, non-downloaded OS
  component. No third-party binary is fetched to enable the webview RUN.
- No `sky add`, no Go toolchain, no Haskell `sky` on the Windows job (EQUIVALENCE stays phased
  off; the live-Go reference stays nightly-only, ubuntu-only). The only possible new fetch is a
  ripgrep fallback (`choco install ripgrep`, unpinned/LATEST) that fires ONLY if the
  preinstalled `rg` ever disappears from the runner image; the preinstalled binary is the
  norm. Windows adds ZERO new untrusted-code-build surface over the existing Linux job.
- No new panic vector touches the emitted-code path: all path/CRLF handling lives in the
  harness (bash/python); `PANIC_RE` still fails loud on any Rust/Go abort in RUN output.

---

## Trap ledger — foreclosed by construction

| Trap | Foreclosed by |
|---|---|
| Spurious-DIFFER (CRLF) | Source `.gitattributes eol=lf` + sink line-ending-scoped `sed 's/\r$//'`/`\r\n`→`\n`; Go oracle ubuntu-only removes it from the gating path. Both backends already emit LF. |
| False-EQUAL (mid-line CR sledgehammer) | Line-ending-scoped normalization ONLY; never `tr -d '\r'`. Payload CR preserved. |
| Silent-skip reported green | Missing-tool preflights hard `exit 2` (python/timeout); tui SKIP prints a reason and is green-neutral (not counted GREEN); built-but-unlocatable binary → counted RED `binmiss`, never SKIP; macOS server-SKIP stays `IPE_HOST_OS=macos`-gated so Windows can't borrow it. |
| Spurious-RED cascade (`.exe` handle-lock) | `_win_reap_app` (`taskkill //F //T`) pre-build in `build_rust` + os-error-5 retry arm. |
| CRLF-corrupted script half-running | `.gitattributes` LF on `*.sh`/`*.py` + `core.autocrlf false` pre-checkout. |
| Path-mangling | Audited: no leading-slash argv to native exes; conversion kept ON (required by the `//F` reap); `MSYS2_ARG_CONV_EXCL` documented as a scoped hatch, not set blindly. |
| Wrong-binary-run (freshest-file race) | Explicit OS-gated `.exe` probes AHEAD of the `find` fallback in BOTH `resolve_bin` (`checks.sh:115`) and the `SKYC_BIN` loop (`env.sh:73-85`); a miss is a counted RED `binmiss`, never the `ls -t` guess and never a SKIP. |
| no-xvfb confusion | Windows never installs/requires xvfb; webview boots the real desktop + preinstalled WebView2; the xvfb step stays `IPE_HOST_OS=linux`-gated. |
| Faked tui pass | Sweep tui SKIPs honestly; real coverage in the `cargo test` windows-latest lane; RUN-column flip gated behind the filed `IPE_TUI_HEADLESS` runtime change. |
| Webview stub fake-green | D-C: verify ipe emits `--features webview` and the crate links WebView2 before trusting the webview row. |
| Supply-chain | Vendored examples + pinned crates only; no `ipe add`, no `playwright --with-deps`, no Go/Haskell reference; WebView2 preinstalled; ripgrep preinstalled-preferred (unpinned `choco` fallback only on image regression). |
| `+crt-static` mis-application | Excluded — dynamic MSVC CRT only; static-CRT is `ipe build --static`'s `StaticWindows` plan. |

---

## Concrete change-set (spec, no code)

Workflow — `.github/workflows/examples-sweep.yml`:
1. `matrix.include` with per-OS `experimental`; `continue-on-error: ${{ matrix.experimental }}`.
2. Windows-only step, BEFORE checkout: `git config --global core.autocrlf false`.
3. `actions/setup-python@v5` (guarantees `python`).
4. Windows ripgrep guard: `command -v rg || choco install ripgrep -y` (preinstalled `rg` preferred; `choco` fallback is unpinned/LATEST, fires only on image regression — not "pinned").
5. Windows Node install WITHOUT Playwright `--with-deps`.
6. Windows job env: forward-slash `CARGO_TARGET_DIR`; MSYS conversion left ON (D-A).
7. Keep the Go≡Rust EQUIVALENCE/corpus step `if: matrix.os == 'ubuntu-latest'`.
8. The Linux-only "Free disk space" step stays Linux-only; add a Windows reclaim/`df`
   check if the shared target + full dep tree pressures the runner.
9. Run-sweep step is already `shell: bash` — no change.

Harness:
10. `scripts/equivalence-checks/examples-sweep.sh` `build_rust`: add `_win_reap_app` + pre-build call +
    os-error-5 retry arm (port from `../sky`).
11. `scripts/equivalence-checks/examples-sweep.sh` `norm()`: append `| sed 's/\r$//'`.
12. `scripts/lib/env.sh`: `IPE_PYTHON` resolution; `cygpath -u` `CARGO_TARGET_DIR`
    normalization on Windows; `.exe` candidates AHEAD of the `find` fallback in the
    `SKYC_BIN` probe loop (`env.sh:73-85`), miss → `binmiss` RED not the freshest-file
    guess; a `timeout` preflight assertion.
13. `scripts/lib/checks.sh`: `resolve_bin` (`checks.sh:115`) `.exe` candidates ahead of the
    `find` fallback (same miss → `binmiss` RED rule as env.sh:73-85);
    `free_port` uses `"$IPE_PYTHON"`; server-body compare `\r`-strip. (tui SKIP + webview
    RUN arms already present — keep.)
14. `scripts/lib/equivalence_normalize_html.py` + `equivalence_tui_grid.py`: `\r\n`→`\n` at ingest;
    stdout pinned to `\n`.

Repo:
15. Root `.gitattributes` (B7).

Filed follow-ups (no-deferral, tracked — not in this job):
16. `IPE_TUI_HEADLESS` runtime one-shot render mode (D-D).
17. `cargo test` unit-matrix `windows-latest` entry for `element_to_cells` coverage.
18. D-C verification: ipe emits `--features webview` on windows-msvc + WebView2 link.
