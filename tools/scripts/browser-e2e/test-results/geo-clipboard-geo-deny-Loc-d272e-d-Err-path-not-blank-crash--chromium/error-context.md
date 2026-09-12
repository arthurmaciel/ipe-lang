# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: geo-clipboard.spec.mjs >> geo deny: Locate renders typed Err path (not blank/crash)
- Location: geo-clipboard.spec.mjs:98:1

# Error details

```
Error: expect(locator).toBeVisible() failed

Locator: getByText(/location: error:/)
Expected: visible
Timeout: 5000ms
Error: element(s) not found

Call log:
  - Expect "toBeVisible" getByText(/location: error:/) with timeout 5000ms
  - waiting for getByText(/location: error:/)

```

```yaml
- button "Locate"
- text: "location: unknown"
- button "Copy location"
- button "Paste"
- text: "clipboard:"
```

# Test source

```ts
  11  |  *      populate a location, "Copy location" clicked, "Paste" clicked → app
  12  |  *      renders "clipboard: <same text>" proving write+read round-trip.
  13  |  *
  14  |  * Prerequisites (all satisfied by the CI `browser-e2e` job setup):
  15  |  *   - IPE_GEO_CLIPBOARD_BIN  env var pointing at the compiled geo-clipboard binary
  16  |  *   - IPE_GEO_CLIPBOARD_PORT env var with the port the binary listens on
  17  |  *   - Playwright installed  (`npm install @playwright/test`)
  18  |  *   - Chromium browser      (`npx playwright install chromium`)
  19  |  *
  20  |  * Run locally after building:
  21  |  *   bash tools/scripts/browser-e2e/run.sh
  22  |  *
  23  |  * Screenshots are written to tools/scripts/browser-e2e/artifacts/ (never
  24  |  * committed — listed in .gitignore).
  25  |  */
  26  | 
  27  | import { test, expect } from "@playwright/test";
  28  | import path from "path";
  29  | import fs from "fs";
  30  | import { fileURLToPath } from "url";
  31  | 
  32  | const __dirname = path.dirname(fileURLToPath(import.meta.url));
  33  | const ARTIFACTS = path.join(__dirname, "artifacts");
  34  | fs.mkdirSync(ARTIFACTS, { recursive: true });
  35  | 
  36  | const PORT = process.env.IPE_GEO_CLIPBOARD_PORT ?? "18080";
  37  | const BASE = `http://127.0.0.1:${PORT}`;
  38  | 
  39  | // ── helpers ───────────────────────────────────────────────────────────────────
  40  | 
  41  | /** Screenshot name → full path under artifacts/. */
  42  | function shot(name) {
  43  |   return path.join(ARTIFACTS, `${name}.png`);
  44  | }
  45  | 
  46  | /**
  47  |  * Block until the app is interactive — <html data-ipe-live="1">.
  48  |  *
  49  |  * The runtime sets this marker exactly when the SSE handshake lands, which is
  50  |  * when the server binds THIS session's outbound Ipe.Ffi.Js port sink to the
  51  |  * connection. Geo.current / Clipboard.read ride that port; a Cmd dispatched
  52  |  * before the sink is bound has its outbound frame dropped fire-and-forget and
  53  |  * never round-trips. `page.goto` resolves on `load`, before the async
  54  |  * handshake, so clicking without this wait races the sink binding — the exact
  55  |  * startup race that flaked all three specs together. Waiting on a real
  56  |  * readiness event (not a fixed sleep) makes the gate deterministic.
  57  |  */
  58  | async function waitReady(page) {
  59  |   await page.waitForSelector('html[data-ipe-live="1"]', { timeout: 15000 });
  60  | }
  61  | 
  62  | // ── flow 1: Geolocation GRANT ─────────────────────────────────────────────────
  63  | 
  64  | test("geo grant: Locate renders Ok Coords path", async ({ browser }) => {
  65  |   // A fresh context lets us control permissions cleanly per test.
  66  |   const ctx = await browser.newContext({
  67  |     permissions: ["geolocation"],
  68  |     geolocation: { latitude: 51.5074, longitude: -0.1278 },
  69  |   });
  70  |   const page = await ctx.newPage();
  71  |   await page.goto(BASE);
  72  |   await waitReady(page);
  73  | 
  74  |   // Initial state — "location: unknown", four buttons present.
  75  |   await expect(page.getByText("location: unknown")).toBeVisible();
  76  |   await expect(page.getByRole("button", { name: "Locate" })).toBeVisible();
  77  | 
  78  |   // Click Locate — triggers Geo.current outbound Cmd.
  79  |   await page.getByRole("button", { name: "Locate" }).click();
  80  | 
  81  |   // Wait for the GotLocation reply to arrive on the positions Sub and the TEA
  82  |   // update to re-render the view.  The reply arrives asynchronously via the
  83  |   // port-glue JS → window.__ipePortSend → inbound subscription decoder.
  84  |   await expect(page.getByText(/location: 51\.\d+/)).toBeVisible({
  85  |     timeout: 5000,
  86  |   });
  87  | 
  88  |   // Coordinates must contain both lat and lng.
  89  |   const locText = await page.getByText(/location: /).textContent();
  90  |   expect(locText).toMatch(/location: 51\.\d+, -0\.\d+/);
  91  | 
  92  |   await page.screenshot({ path: shot("geo-grant") });
  93  |   await ctx.close();
  94  | });
  95  | 
  96  | // ── flow 2: Geolocation DENY ──────────────────────────────────────────────────
  97  | 
  98  | test("geo deny: Locate renders typed Err path (not blank/crash)", async ({
  99  |   browser,
  100 | }) => {
  101 |   // No `permissions: ["geolocation"]` → the browser denies the API call.
  102 |   const ctx = await browser.newContext();
  103 |   const page = await ctx.newPage();
  104 |   await page.goto(BASE);
  105 |   await waitReady(page);
  106 | 
  107 |   await page.getByRole("button", { name: "Locate" }).click();
  108 | 
  109 |   // The inbound JsMsg.Denied folds to Err Error.permissionDenied →
  110 |   // update sets model.location = "error: ..." — never a blank or crash.
> 111 |   await expect(page.getByText(/location: error:/)).toBeVisible({
      |                                                    ^ Error: expect(locator).toBeVisible() failed
  112 |     timeout: 5000,
  113 |   });
  114 | 
  115 |   // Must NOT show the raw "unknown" initial state (would mean no message arrived).
  116 |   const locText = await page.getByText(/location: /).textContent();
  117 |   expect(locText).not.toBe("location: unknown");
  118 | 
  119 |   await page.screenshot({ path: shot("geo-deny") });
  120 |   await ctx.close();
  121 | });
  122 | 
  123 | // ── flow 3: Clipboard round-trip ──────────────────────────────────────────────
  124 | 
  125 | test("clipboard: write then read round-trip renders the copied text", async ({
  126 |   browser,
  127 | }) => {
  128 |   const ctx = await browser.newContext({
  129 |     permissions: ["geolocation", "clipboard-read", "clipboard-write"],
  130 |     geolocation: { latitude: 48.8566, longitude: 2.3522 },
  131 |   });
  132 |   const page = await ctx.newPage();
  133 |   await page.goto(BASE);
  134 |   await waitReady(page);
  135 | 
  136 |   // Populate a location first so "Copy location" has something to write.
  137 |   await page.getByRole("button", { name: "Locate" }).click();
  138 |   await expect(page.getByText(/location: 48\.\d+/)).toBeVisible({
  139 |     timeout: 5000,
  140 |   });
  141 | 
  142 |   const locText = await page.getByText(/location: /).textContent();
  143 |   // Extract the coords string after "location: ".
  144 |   const coords = locText.replace(/^location: /, "").trim();
  145 |   expect(coords.length).toBeGreaterThan(0);
  146 | 
  147 |   // Copy the location text to the clipboard.
  148 |   await page.getByRole("button", { name: "Copy location" }).click();
  149 | 
  150 |   // Paste — triggers Clipboard.read outbound Cmd; the read result arrives on
  151 |   // the contents Sub and update sets model.clipboard = the text.
  152 |   await page.getByRole("button", { name: "Paste" }).click();
  153 | 
  154 |   // The pasted clipboard text must match the coords that were copied.
  155 |   await expect(page.getByText(`clipboard: ${coords}`)).toBeVisible({
  156 |     timeout: 5000,
  157 |   });
  158 | 
  159 |   await page.screenshot({ path: shot("clipboard-roundtrip") });
  160 |   await ctx.close();
  161 | });
  162 | 
```