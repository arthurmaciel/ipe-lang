/**
 * Playwright layout spec for the layout-fill example.
 *
 * Proves, in a real browser, that `fill` sizes only along the axis its parent
 * controls and that the runtime wrappers are part of the height chain:
 *
 *   1. The `height fill` main area fills the viewport below the 48px header,
 *      and its button is hit-testable and clickable.
 *   2. `fillPortion` 0 / 1 / 3 splits a row's width 0 / 25 / 75 %, and each
 *      segment's `height fill` stretches to the row's 12px cross axis.
 *   3. A `width fill` row inside a column spans the width without growing
 *      vertically.
 *
 * Prerequisites (all satisfied by the CI `browser-e2e` job setup):
 *   - IPE_LAYOUT_FILL_PORT env var with the port the layout-fill binary
 *     listens on
 *   - Playwright installed  (`npm install @playwright/test`)
 *   - Chromium browser      (`npx playwright install chromium`)
 *
 * Run locally after building:
 *   bash tools/scripts/browser-e2e/run.sh
 */

import { test, expect } from "@playwright/test";
import path from "path";
import fs from "fs";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ARTIFACTS = path.join(__dirname, "artifacts");
fs.mkdirSync(ARTIFACTS, { recursive: true });

const PORT = process.env.IPE_LAYOUT_FILL_PORT ?? "18081";
const BASE = `http://127.0.0.1:${PORT}`;
const VIEWPORT = { width: 1000, height: 700 };
const HEADER_PX = 48;
const BAR_PX = 12;

/** Block until the app is interactive — `<html data-ipe-live="1">`. */
async function waitReady(page) {
  await page.waitForSelector('html[data-ipe-live="1"]', { timeout: 15000 });
}

/**
 * The client rect of `selector`, waiting for it to be attached.
 *
 * Read through `getBoundingClientRect`, not `boundingBox()`: a zero-width
 * element (the `fillPortion 0` segment) is a legal layout box that Playwright
 * would report as invisible.
 */
async function box(page, selector) {
  const loc = page.locator(selector);
  await loc.waitFor({ state: "attached", timeout: 5000 });
  return loc.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }, undefined, { timeout: 5000 });
}

test.beforeEach(async ({ page }) => {
  await page.setViewportSize(VIEWPORT);
  await page.goto(BASE, { timeout: 15000 });
  await waitReady(page);
});

test("height fill: main area fills the viewport below the header", async ({
  page,
}) => {
  const main = await box(page, "#main");
  expect(main.height).toBeGreaterThan(0);
  expect(Math.abs(main.height - (VIEWPORT.height - HEADER_PX))).toBeLessThanOrEqual(2);

  const inc = await box(page, "#inc");
  const hit = await page.evaluate(
    ([x, y]) => {
      const target = document.getElementById("inc");
      const at = document.elementFromPoint(x, y);
      return target !== null && at !== null && target.contains(at);
    },
    [inc.x + inc.width / 2, inc.y + inc.height / 2],
  );
  expect(hit, "#inc must be the element at its own centre").toBe(true);

  await expect(page.locator("#count")).toHaveText("0", { timeout: 5000 });
  await page.click("#inc", { timeout: 5000 });
  await expect(page.locator("#count")).toHaveText("1", { timeout: 5000 });

  await page.screenshot({ path: path.join(ARTIFACTS, "layout-fill.png") });
});

test("fillPortion: row width splits 0 / 25 / 75 and heights stretch", async ({
  page,
}) => {
  const bar = await box(page, "#bar");
  const seg0 = await box(page, "#seg0");
  const seg25 = await box(page, "#seg25");
  const seg75 = await box(page, "#seg75");

  expect(bar.height).toBeCloseTo(BAR_PX, 0);
  expect(seg0.width).toBeLessThanOrEqual(0.5);
  expect(Math.abs(seg25.width - bar.width * 0.25)).toBeLessThanOrEqual(1);
  expect(Math.abs(seg75.width - bar.width * 0.75)).toBeLessThanOrEqual(1);
  for (const seg of [seg0, seg25, seg75]) {
    expect(Math.abs(seg.height - bar.height)).toBeLessThanOrEqual(0.5);
  }
});

test("width fill in a column spans the width and does not grow vertically", async ({
  page,
}) => {
  const rows = await box(page, "#rows");
  const wrow = await box(page, "#wrow");

  expect(wrow.height).toBeGreaterThan(0);
  expect(wrow.height).toBeLessThan(rows.height / 2);
  expect(Math.abs(wrow.width - rows.width)).toBeLessThanOrEqual(1);
});
