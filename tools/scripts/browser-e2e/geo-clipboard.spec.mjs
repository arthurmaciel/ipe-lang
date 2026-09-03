/**
 * Playwright behaviour spec for the geo-clipboard example.
 *
 * Exercises three flows that require a real headless browser:
 *
 *   1. Geolocation GRANT — browser permission granted, fixed position injected,
 *      "Locate" clicked → app renders "location: <lat>, <lng>" (Ok path).
 *   2. Geolocation DENY  — browser permission denied, "Locate" clicked →
 *      app renders "location: error: ..." (Err path, not a blank or crash).
 *   3. Clipboard round-trip — clipboard permission granted, "Locate" clicked to
 *      populate a location, "Copy location" clicked, "Paste" clicked → app
 *      renders "clipboard: <same text>" proving write+read round-trip.
 *
 * Prerequisites (all satisfied by the CI `browser-e2e` job setup):
 *   - IPE_GEO_CLIPBOARD_BIN  env var pointing at the compiled geo-clipboard binary
 *   - IPE_GEO_CLIPBOARD_PORT env var with the port the binary listens on
 *   - Playwright installed  (`npm install @playwright/test`)
 *   - Chromium browser      (`npx playwright install chromium`)
 *
 * Run locally after building:
 *   bash tools/scripts/browser-e2e/run.sh
 *
 * Screenshots are written to tools/scripts/browser-e2e/artifacts/ (never
 * committed — listed in .gitignore).
 */

import { test, expect } from "@playwright/test";
import path from "path";
import fs from "fs";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ARTIFACTS = path.join(__dirname, "artifacts");
fs.mkdirSync(ARTIFACTS, { recursive: true });

const PORT = process.env.IPE_GEO_CLIPBOARD_PORT ?? "18080";
const BASE = `http://127.0.0.1:${PORT}`;

// ── helpers ───────────────────────────────────────────────────────────────────

/** Screenshot name → full path under artifacts/. */
function shot(name) {
  return path.join(ARTIFACTS, `${name}.png`);
}

// ── flow 1: Geolocation GRANT ─────────────────────────────────────────────────

test("geo grant: Locate renders Ok Coords path", async ({ browser }) => {
  // A fresh context lets us control permissions cleanly per test.
  const ctx = await browser.newContext({
    permissions: ["geolocation"],
    geolocation: { latitude: 51.5074, longitude: -0.1278 },
  });
  const page = await ctx.newPage();
  await page.goto(BASE);

  // Initial state — "location: unknown", four buttons present.
  await expect(page.getByText("location: unknown")).toBeVisible();
  await expect(page.getByRole("button", { name: "Locate" })).toBeVisible();

  // Click Locate — triggers Geo.current outbound Cmd.
  await page.getByRole("button", { name: "Locate" }).click();

  // Wait for the GotLocation reply to arrive on the positions Sub and the TEA
  // update to re-render the view.  The reply arrives asynchronously via the
  // port-glue JS → window.__ipePortSend → inbound subscription decoder.
  await expect(page.getByText(/location: 51\.\d+/)).toBeVisible({
    timeout: 5000,
  });

  // Coordinates must contain both lat and lng.
  const locText = await page.getByText(/location: /).textContent();
  expect(locText).toMatch(/location: 51\.\d+, -0\.\d+/);

  await page.screenshot({ path: shot("geo-grant") });
  await ctx.close();
});

// ── flow 2: Geolocation DENY ──────────────────────────────────────────────────

test("geo deny: Locate renders typed Err path (not blank/crash)", async ({
  browser,
}) => {
  // No `permissions: ["geolocation"]` → the browser denies the API call.
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.goto(BASE);

  await page.getByRole("button", { name: "Locate" }).click();

  // The inbound JsMsg.Denied folds to Err Error.permissionDenied →
  // update sets model.location = "error: ..." — never a blank or crash.
  await expect(page.getByText(/location: error:/)).toBeVisible({
    timeout: 5000,
  });

  // Must NOT show the raw "unknown" initial state (would mean no message arrived).
  const locText = await page.getByText(/location: /).textContent();
  expect(locText).not.toBe("location: unknown");

  await page.screenshot({ path: shot("geo-deny") });
  await ctx.close();
});

// ── flow 3: Clipboard round-trip ──────────────────────────────────────────────

test("clipboard: write then read round-trip renders the copied text", async ({
  browser,
}) => {
  const ctx = await browser.newContext({
    permissions: ["geolocation", "clipboard-read", "clipboard-write"],
    geolocation: { latitude: 48.8566, longitude: 2.3522 },
  });
  const page = await ctx.newPage();
  await page.goto(BASE);

  // Populate a location first so "Copy location" has something to write.
  await page.getByRole("button", { name: "Locate" }).click();
  await expect(page.getByText(/location: 48\.\d+/)).toBeVisible({
    timeout: 5000,
  });

  const locText = await page.getByText(/location: /).textContent();
  // Extract the coords string after "location: ".
  const coords = locText.replace(/^location: /, "").trim();
  expect(coords.length).toBeGreaterThan(0);

  // Copy the location text to the clipboard.
  await page.getByRole("button", { name: "Copy location" }).click();

  // Paste — triggers Clipboard.read outbound Cmd; the read result arrives on
  // the contents Sub and update sets model.clipboard = the text.
  await page.getByRole("button", { name: "Paste" }).click();

  // The pasted clipboard text must match the coords that were copied.
  await expect(page.getByText(`clipboard: ${coords}`)).toBeVisible({
    timeout: 5000,
  });

  await page.screenshot({ path: shot("clipboard-roundtrip") });
  await ctx.close();
});
