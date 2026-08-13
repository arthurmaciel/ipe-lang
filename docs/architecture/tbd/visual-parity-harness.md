# Visual parity harness — design

Visual parity is parity increment 3: for each Web/WebView example port, prove
that the Ipê and Sky renderings of the same initial model are visually
equivalent within a perceptual tolerance. This document covers the capture
strategy per shape, the determinism model, the comparison tool and threshold,
and the CI job shape.

---

## Scope

Three ports are the initial target:

| Port | Shape | Sky binary shape | Ipê binary shape |
|------|-------|------------------|------------------|
| `29-webview-threejs-spike` | webview | Go stub on Linux (exits 0, no window) | webkit2gtk window under xvfb |
| `31-webview-stopwatch-ui` | webview | Go stub on Linux (exits 0, no window) | webkit2gtk window under xvfb |
| `38-composite-ui-multibackend` | web (ipe port uses WebView.app) | Live.app HTTP server on :8006 | WebView.app window under xvfb |

The ~15 already-green `web` ports are a natural extension once the harness is
proven; they are out of scope here.

---

## Capture strategy

### Web shape (Playwright)

Both the sky-side HTTP server and any Ipê `Live.app` port are served on a
loopback port. `npx playwright screenshot --browser=chromium` captures a
full-page PNG at a fixed viewport (`--viewport-size=1280x800`) after a
`--wait-for-timeout=2000` settle period. The server is started by
`check-sky-parity-visual.sh` itself (not assumed to already be running);
the script finds a free port, waits for the port to accept connections (≤ 8s),
triggers the screenshot, then kills the server.

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

Two thresholds therefore apply:

- **Same-engine threshold: RMS ≤ 8** — for Playwright-Playwright comparisons
  (future: ipe-web `Live.app` via Playwright vs sky `Live.app` via Playwright;
  same Chromium, same DPR, only HTML differences matter). This catches real
  layout regressions.
- **Cross-engine threshold: RMS ≤ 60** — for Playwright (sky/Chromium) vs
  xvfb (ipe/WebKitGTK). The threshold is calibrated above the ~46 observed
  cross-engine baseline so that identical HTML structure PASSes. A layout
  regression that removes or repositions a major element produces RMS ≥ 80.

The cross-engine threshold of 60 is deliberately conservative. A tighter bound
requires either (a) normalising both screenshots to the same engine or (b)
adding a structural element-count pre-check before the pixel diff.

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
2. Reads the manifest to find ports with `visual_parity` set and not `skip`.
3. For each port:
   a. Builds the ipe port (`ipe build` + `cargo build`) if not already built.
   b. Starts the sky or ipe server / process (shape-aware).
   c. Captures screenshot (Playwright or xvfb + import).
   d. Runs the PIL diff helper (`tools/scripts/lib/visual_diff.py`).
   e. Reports PASS / FAIL / SKIP with the RMS score.
4. Writes a summary table and exits non-zero on any FAIL.

The existing `check-sky-parity.sh` (stdout parity for program/console ports)
is unchanged; this script is additive.

---

## CI job shape

A new job `visual-parity` in `.github/workflows/examples-sweep.yml` (or a
separate `visual-parity.yml`) runs only on `ubuntu-latest`.

The install step (illustrative — adapt to the target workflow file) adds two
packages to the existing webview dep install:

```
sudo apt-get install -y xvfb libwebkit2gtk-4.1-dev libsoup-3.0-dev imagemagick
npx playwright install chromium
```

`xvfb libwebkit2gtk-4.1-dev libsoup-3.0-dev` are already present in the
`examples-sweep.yml` webview smoke install; the additions are `imagemagick`
(for `import -window root`) and `npx playwright install chromium` (for
web-shape Playwright capture). All four packages are confirmed available on
Ubuntu 22.04 (`apt-get install` names verified locally).

The check itself runs as:

```
bash tools/scripts/check-sky-parity-visual.sh
```

The job is `continue-on-error: true` during the PoC phase (same as the
examples-sweep job) and becomes a gate once all in-scope ports are green.

---

## Blockers and follow-on work

| Blocker | Impact | Resolution |
|---------|--------|------------|
| sky v0.16.29 webview is a stub on Linux | ports 29/31 sky-side has no screenshot | Use CI sky v0.19.13; or derive HTML reference from Go source |
| port 38 ipe uses WebView.app, manifest says web/serve | misclassified port | Separate PoC; compare sky Live.app screenshot vs ipe WebView screenshot using the shared HTML renderer |
| port 29 Three.js animation starts at t=0 | non-deterministic after first frame | Clock-freeze strategy (requestAnimationFrame no-op injection) |
| sky v0.16.29 type errors on web ports (09, 12, 26…) | sky-side screenshot blocked for these | Use CI sky v0.19.13 only |
| xvfb `import -window root` captures full virtual display, not just the app window | large black border in screenshot | Crop to window bounds, or use `import -window <title>` once wm is running |
| Chromium vs WebKitGTK render differences | RMS 2–5 expected between engines | Threshold at 8.0 covers this; per-port overrides available |

The single most impactful follow-on: install and test sky v0.19.13 on CI and
verify that `Std.Webview` on Linux in that version actually opens a GTK window
(not a stub). If it does, ports 29/31 become fully comparable.
