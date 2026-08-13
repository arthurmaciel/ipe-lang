# examples/sky — Sky↔Ipê E2E parity harness (design)

> Status: TBD / designed-target. This doc is the SoT for the target system.

## Goal

`examples/sky/` mirrors the upstream Sky example corpus and proves, per example,
that our converted Ipê port **behaves identically to the original Sky program on
every shape** — Program/CLI, Terminal (Console + TUI), Web, WebView. Parity is
verified by comparing real output: stdout/stderr bytes for non-visual shapes, the
rendered terminal screen for TUIs, and **screenshots** for Web/WebView — Sky and
Ipê must match. The harness's machine-setup steps double as the user-facing
"run any Ipê program on your machine" guide.

## Layout

- `examples/sky/original/<name>/` — the raw upstream Sky source we last converted
  from (verbatim). The converter's input and the drift baseline.
- `examples/sky/ipe/<name>/` — the committed converted Ipê port. The tested SoT.
- `examples/sky/upstream.lock` — per-example **content hash** of `original/<name>/`
  (sorted file list + contents). Lets CI pinpoint exactly which example drifted.
- Conversion adjustments stay as today: `rename-map.tsv`, `ipe-edits/<name>.edits`,
  `ipe-overrides/<name>/`, `manifest.toml`.
- `manifest.toml` carries a per-example **`shape`** (program | console | tui | web
  | webview | server) and a **`verify`** policy (`run` | `build` | `serve`) that
  selects how the example is exercised.
- `examples/sky/e2e/<name>.script` — the committed driving script (keys / clicks /
  requests) for an interactive example, so both toolchains get identical input.

## Two independent mechanisms

A single job that re-fetches ALL of upstream and rebuilds everything conflates
"did upstream drift?" with "do our ports still work?" — a flaky, chronically-red
run over a moving target. Keep them separate:

### Offline "our ports work" gate — every PR, deterministic, no network

`ipe build` (+ `ipe run` per the example's `verify` policy) over the **committed
`ipe/` ports**, plus the offline consistency check (committed `ipe/` must equal
re-deriving `original/` through the converter). This catches regressions in OUR
tree — a language or converter change that breaks a port — with no network and no
flake. It is the gate that gates development PRs.

### Upstream-drift detection — scheduled / manual, network

Fetch upstream, recompute each example's hash, compare to `upstream.lock`:

- **unchanged** → skip.
- **changed** → **error**: a human updates `original/` (+ edits/overrides),
  regenerates `ipe/`, re-verifies parity, and bumps the lock.
- **added** → auto-convert → `ipe build` + `ipe run` → on success open a **bot PR**
  adding `original/` + `ipe/` + the lock entry (a new upstream example may need
  ipe-edits or be unsafe, so a human reviews); on failure → **error** to fix.
- **removed upstream** → flag for a human (keep pinned vs drop).

## E2E parity — Sky vs Ipê, all shapes

For each example the harness runs BOTH the real `sky` toolchain and `ipe`, captures
output identically, and asserts a match. Dispatch by `shape`:

- **program / console / server-oneshot** — run to exit; byte-compare stdout+stderr
  and exit code. A server that stays up gets a scripted request set; compare the
  response bodies.
- **tui** — run in a pty (headless terminal), drive the `.script` key sequence,
  capture the rendered screen, and byte-compare the screen dump (fall back to a
  screenshot of the terminal when a byte dump is unstable).
- **web** — serve the built app, drive the `.script` interaction in a headless
  browser, **screenshot**, and perceptually diff the Sky vs Ipê screenshots within
  a tight threshold.
- **webview** — run under the WebView runtime (webkit2gtk) + xvfb, **screenshot**,
  and diff Sky vs Ipê.

Both sides render under the **same** harness (identical fonts, DPI, browser,
window size), so "must match" is a tight perceptual threshold rather than an exact
pixel identity across machines. Failing screenshots/diffs upload as CI artifacts.

## Machine-setup guide

The exact packages and steps the harness installs to run every shape — the `sky`
toolchain, a headless browser and its deps, webkit2gtk, xvfb, fonts, pty tooling —
are the authoritative contents of `docs/.../running-ipe-programs.md`, the "set up
your machine to run any Ipê program" guide, kept honest because CI runs it.

## Open infra questions

- **Sky in CI:** whether a prebuilt per-OS `sky` release binary exists, or CI must
  build anzellai/sky (Haskell/Stack). Prefer a prebuilt binary; a scheduled-only
  build is the fallback.
- **Screenshot conditions:** pin fonts/DPI/browser/window so Sky and Ipê render
  identically; the match threshold is tight but nonzero to absorb sub-pixel AA.
- **Interaction scripts:** each interactive example needs a committed `.script` so
  both toolchains are driven identically.

## Delivery

The parity harness (Sky toolchain, screenshots, WebView) is heavy and runs on a
schedule; the offline ports gate runs on every PR. The system lands incrementally,
each increment a reviewed PR tracked by its own issue, starting from the drift lock
+ offline gate (no heavy infra) and building up through non-visual parity, TUI
parity, and Web/WebView visual parity to the extracted setup guide.
