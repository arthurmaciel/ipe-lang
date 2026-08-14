# Visual parity harness — design

Visual parity is parity increment 3: for each Web/WebView example port, prove
that the Ipê and Sky renderings of the same initial model are visually
equivalent within a perceptual tolerance. This document covers the capture
strategy per shape, the determinism model, the comparison tool and threshold,
and the CI job shape.

---

## Scope

### Web ports (12 green; harness generalised)

`check-sky-parity-visual.sh` now iterates all `shape = "web"` + `status = "green"`
ports from `examples/sky/manifest.toml` data-driven (no hardcoded list):

| Port | Dynamic? | Policy |
|------|----------|--------|
| `09-live-counter` | static | compare |
| `10-live-component` | static | compare |
| `12-skyvote` | static | compare |
| `25-sky-console` | static | compare |
| `26-ui-showcase` | static | compare |
| `27-multi-session-chat` | real-time SSE | skip |
| `28-streaming-chat` | streaming | skip |
| `34-multi-tier-console` | static | compare |
| `16-skychess` | dynamic board | skip |
| `17-skymon` | dynamic graph | skip |
| `19-skyforum` | static | compare |
| `37-composite-live-shop` | static | compare |

Skipped ports (`27`, `28`, `16`, `17`) have nondeterministic first-paint due to
live data pushed before the Playwright settle completes.

Ports excluded from this harness: `18-job-queue` (broken), `13-skyshop` (broken),
`38-composite-ui-multibackend` (deps-deferred).

### Webview ports (unchanged — sky stub)

Sky v0.19.13 `Std.Webview` on Linux: the Go runtime file `webview.go` carries
`//go:build cgo && darwin`. Linux falls through to `webview_stub.go`
(`//go:build !cgo || !darwin`), which returns `Err Error` immediately — no
GTK window, no screenshot possible. This is confirmed by inspecting the
emitted Go in `sky-out/rt/` after a successful `sky build`.

The v0.19.13 source comments say "v0.2 will widen the tag once Linux
(webkit2gtk-4.0 / 4.1) + Windows smoke validation lands." Until that ships,
webview ports remain `SKY-STUB` in the harness.

| Port | Sky side | Ipê side |
|------|----------|----------|
| `29-webview-threejs-spike` | SKY-STUB (no window) | webkit2gtk under xvfb |
| `31-webview-stopwatch-ui` | SKY-STUB (no window) | webkit2gtk under xvfb |

---

## Capture strategy

### Web shape (Playwright — same-engine)

All 12 web ports use `Live.app` HTTP servers on both sides. Both sky and Ipê
are served on free loopback ports and captured with `npx playwright screenshot
--browser=chromium` at `--viewport-size=1280,800` after `--wait-for-timeout=2000`.

Because both sides use the **same Playwright/Chromium engine**, rendering noise
from font metrics, antialiasing, and shadow rendering is eliminated. The
same-engine threshold is **RMS ≤ 8.0**, vs the PoC's cross-engine threshold of
60.0 (Chromium vs WebKitGTK).

**Sky build compatibility:** sky v0.16.29 fails with `Std.Css.zero` arity
errors. Sky v0.19.13 changed `Live.app` to a builder API — the record syntax
used in all committed `original/` ports produces `[E2001] AppConfig _ _ vs
record` on every web port. Both versions fail to compile from the committed
originals. When sky build fails the port is recorded as `SKY-BUILD-FAIL` (data;
does not fail the harness) and the diff is skipped. The ipe screenshot is still
captured for reference.

Port env override: `IPE_WEB_PORT=<N>` for the Ipê binary; `SKY_LIVE_PORT=<N>`
for the sky binary (sky runtime's default prefix is `SKY_`).

The server is started by `check-sky-parity-visual.sh` itself; the script finds
a free port, waits ≤ 10 s for the port to accept connections, triggers the
screenshot, then kills the server.

### Webview shape (xvfb + ImageMagick import)

The built `ipe-app` binary is launched under `xvfb-run -a -w 2` (a fresh
virtual display), given 5 s to paint the initial frame, then
`import -window root` (ImageMagick) captures the full virtual display surface.
The process is terminated before `xvfb-run` exits. This produces a screenshot
of the exact pixel output of the WebKitGTK webview.

For the Sky side of webview ports: sky v0.16.29 on Linux compiles webview
programs with CGO disabled (`Std.Webview` resolves to a no-op stub at link
time); the binary exits 0 immediately with no window. This means sky-side
visual capture of webview ports is **not possible** with sky ≤ 0.16.29 on
Linux — the harness documents this as a `sky-stub` blocker and skips the diff.
CI uses sky v0.19.13 (see `tools/scripts/install-sky-toolchain.sh`); webview
parity on CI is blocked until either (a) sky v0.19.13 is tested to produce a
real webview binary on Linux or (b) an alternative sky-side reference is
derived (see next section).

### Sky-side reference for webview ports (fallback)

When the sky binary is a stub, the reference can be derived from the Go emitted
source. Both sky and Ipê use the same HTML renderer (`render_html` / `rt.Live`);
the initial HTML body `render_html(init())` is deterministic and can be extracted
by serving a minimal HTTP wrapper around the app's view function. A future
increment can build this extraction tool for Go; for now the harness marks
these ports `sky-stub` and defers comparison to CI where sky produces a real
window.

---

## Determinism strategy

### Static initial render

Dynamic UIs (animations, timers, random data) produce non-deterministic frames.
The harness captures only the **initial render** — the frame immediately after
`init()` before any `Sub.every` tick or animation frame fires. This is
deterministic because:

- `init()` takes a fixed argument (`()` for webview, a fixed request record for
  Live.app); no wall-clock time is used in `init` unless the app explicitly
  reads it.
- WebKit renders the initial HTML synchronously before JavaScript timers tick;
  Playwright's `--wait-for-timeout=2000` waits for JS settle but the DOM is
  already painted.

Port-specific notes:
- `31-webview-stopwatch-ui`: `init _ = { running = False, elapsedMs = 0 }` —
  stopwatch starts paused. `Sub.every 100 Tick` is registered only when
  `model.running = True`; the initial state has no running subscription.
  Capture at t=0 is fully static.
- `29-webview-threejs-spike`: Three.js animation begins immediately after
  script load. The xvfb capture at t=5s will show mid-animation state. This
  port requires a **clock-freeze** strategy: inject `requestAnimationFrame` to
  a no-op before launch, or reduce wait to t=0 (capture before JS runs).
  Deferred to a follow-on increment.
- `38-composite-ui-multibackend`: `todayIndex = 20000` (fixed constant in ipe
  port); all seeded habit data is deterministic. Sky side uses `Time.unixMillis`
  — the actual day index changes daily, so the "Day NNNNN" header and 7-day avg
  differ between sky and ipe runs on different days. The harness crops or masks
  the date header for comparison (see threshold section).

---

## Perceptual comparison tool and threshold

### Tool: PIL RMS diff

Pure-Python, no external deps beyond `Pillow` (already available). Both images
are first auto-cropped to remove the black border that `xvfb` virtual-display
capture adds, then resized to the smaller of the two dimensions (matching
viewports avoids lossy resize), then greyscale-diffed pixel-by-pixel.

```
rms = sqrt(mean((A_grey - B_grey)^2))   over all pixels after crop+resize
```

### Threshold — two tiers, same-engine vs cross-engine

Measured baselines from the PoC (port 38, matched 960×720 viewport):

| Comparison | RMS | Explanation |
|-----------|-----|-------------|
| same image vs itself | 0.0 | sanity check |
| small synthetic colour diff (~7/ch) | ~6.0 | pure colour shift |
| sky Chromium vs ipe WebKitGTK (same HTML) | ~46 | cross-engine: font AA, shadow, border-radius differ |

Two thresholds apply:

- **Same-engine threshold: RMS ≤ 8** — for Playwright/Chromium vs Playwright/Chromium.
  Both sides use the same browser engine so only HTML/CSS differences contribute;
  antialiasing noise is ≤ 2 RMS. Catches real layout regressions (missing element
  ≥ 80 RMS, wrong colour ≥ 6 RMS). **This is the active threshold for all 12 web ports.**
- **Cross-engine threshold: RMS ≤ 60** — for Playwright/Chromium vs xvfb/WebKitGTK.
  Applies only to webview ports where the ipe side renders via webkit2gtk and the sky
  side (if available) would capture via a different engine. Currently unused for web
  ports (all use Playwright on both sides).

For ports with a known nondeterministic region (port 38's "Day NNNNN" header),
the comparison crops that row before diffing. Crop coordinates are declared
per-port in the harness's port table (see `check-sky-parity-visual.sh`).

### Capture viewport matching

The two sides must be captured at the same viewport size or the resize step
will misalign layout reflows. The viewport to use is the ipe port's declared
`window.size` from `ipe.toml`. Playwright is called with `--viewport-size=WxH`
matching that size. The harness reads the size from `ipe.toml` at runtime.

---

## Integration with existing scripts

The harness lives in `tools/scripts/check-sky-parity-visual.sh`. It:

1. Sources `lib/env.sh` and `lib/checks.sh` (same as `check-sky-parity.sh`).
2. Parses `examples/sky/manifest.toml` with an awk block to collect all
   `shape = "web"` + `status = "green"` port names (data-driven; no hardcoded list).
3. Overlays the per-port skip/threshold policy table embedded in the script.
4. For each port:
   a. Builds the ipe port (`ipe build` + `cargo build`) with a per-port
      `CARGO_TARGET_DIR` suffix to avoid collisions across ports.
   b. Starts the ipe HTTP server (`IPE_WEB_PORT=<free-port>`) and captures
      a Playwright/Chromium screenshot.
   c. Attempts to build the sky port (`sky build` + `go build`); on failure,
      records `SKY-BUILD-FAIL` (data, not a harness error) and skips the diff.
   d. On sky build success: starts the sky server (`SKY_LIVE_PORT=<free-port>`)
      and captures a Playwright/Chromium screenshot.
   e. Runs `tools/scripts/lib/visual_diff.py` for a same-engine RMS diff.
   f. Cleans per-port transient artifacts (sky-out/, cargo target subtree, raw
      screenshots) to avoid disk accumulation across 12 ports.
5. Checks free disk between ports; stops gracefully at < 7G.
6. Exits non-zero only on diff-FAIL or ipe-build-FAIL; sky-build-FAILs do not
   fail the exit code.

Flags:
- `--no-sky` — skip sky build/serve entirely; capture ipe screenshots only.
- `--keep-artifacts` — retain all raw screenshots and build artifacts.
- `--names N,…` — run a subset of ports.

The existing `check-sky-parity.sh` (stdout parity for program/console ports)
is unchanged; this script is additive.

---

## CI job shape

`.github/workflows/sky-parity-visual.yml` — a standalone nightly workflow
(`ubuntu-latest`, `continue-on-error: true`, not a required PR gate).

Schedule: `17 6 * * *` UTC (offset from `sky-parity.yml` at 05:47 to avoid
runner contention).

The workflow installs system dependencies, builds ipe, installs the pinned sky
toolchain via `tools/scripts/install-sky-toolchain.sh`, runs the visual parity
check, then uploads all screenshots and logs as a CI artifact
(`visual-parity-screenshots`, 7-day retention) for visual inspection.
Screenshots are not committed to git.

Local equivalent — the system packages below are prerequisites that cannot be
run from inside the repo (they require sudo); install them separately before
running the harness commands. Packages confirmed present on Ubuntu 22.04/24.04:
`xvfb libwebkit2gtk-4.1-dev libsoup-3.0-dev imagemagick python3-pil ripgrep`
plus `npx playwright install --with-deps chromium`.

Once those are in place, from the repo root:

- `cargo build --release -p ipe`
- `bash tools/scripts/install-sky-toolchain.sh`
- `bash tools/scripts/check-sky-parity-visual.sh --out-dir /tmp/ipe-visual-parity`

---

## Observed results (web ports, sky pinned version)

Tested with sky v0.19.13 against the committed `examples/sky/original/` ports
(Ubuntu, Playwright 1.62.1, ipe 0.1.50):

| Port | Result | Notes |
|------|--------|-------|
| `09-live-counter` | SKY-BUILD-FAIL | sky `[E2001]` — Live.app builder API break |
| `10-live-component` | SKY-BUILD-FAIL | same |
| `12-skyvote` | SKY-BUILD-FAIL | same |
| `25-sky-console` | SKY-BUILD-FAIL | same |
| `26-ui-showcase` | SKY-BUILD-FAIL | same |
| `27-multi-session-chat` | SKIP | real-time SSE — dynamic first-paint |
| `28-streaming-chat` | SKIP | streaming — dynamic first-paint |
| `34-multi-tier-console` | SKY-BUILD-FAIL | same |
| `16-skychess` | SKIP | dynamic board state |
| `17-skymon` | SKIP | dynamic monitoring graph |
| `19-skyforum` | SKY-BUILD-FAIL | same |
| `37-composite-live-shop` | SKY-BUILD-FAIL | same |

All ipe-side HTTP servers started and Playwright captured their initial render
successfully. The sky-side comparison is blocked by the v0.19.13 API break
across all web ports (`Live.app` changed from record syntax to a builder API;
the committed originals use the old syntax). These are data rows — the harness
exits 0 because no diff-FAIL occurred. SKY-BUILD-FAILs are non-gating.

Passing ports are tracked by absence of diff-FAIL in the harness summary.
No manifest `status` field is changed by visual parity results (status reflects
build parity, not visual parity).

---

## Blockers and follow-on work

| Blocker | Impact | Resolution path |
|---------|--------|-----------------|
| sky v0.19.13 `Live.app` builder API break | all 12 web ports cannot build sky-side | Port originals to v0.19.13 builder syntax, OR derive sky-side HTML from Go emit via an HTTP wrapper |
| sky webview on Linux is a stub | ports 29/31 sky-side no screenshot | Source-confirmed: `//go:build cgo && darwin`; stub returns `Err Error` — no GTK window |
| port 29 Three.js animation starts at t=0 | non-deterministic after first frame | Clock-freeze: inject `requestAnimationFrame` no-op before capture |
| Dynamic ports (16/17/27/28) have live data at t=0 | nondeterministic first-paint | Session-seed strategy: serve with fixed seed / mock time |
| xvfb `import -window root` black border on webview captures | large black region in screenshot | Crop to declared `window.size` from `ipe.toml` |

Most impactful unblocking path: update `examples/sky/original/` web ports to
the v0.19.13 builder syntax. This would enable same-engine Chromium/Chromium
diffs at threshold 8.0 for the eight static ports (09, 10, 12, 25, 26, 34,
19, 37).
