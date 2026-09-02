/**
 * Playwright configuration for the geo-clipboard browser E2E suite.
 *
 * A single Chromium project — the only browser target for these tests.
 * The geo/clipboard browser APIs (navigator.geolocation,
 * navigator.clipboard) behave identically across Chromium, Firefox, and
 * WebKit from Playwright's permission-grant model, but Chromium ships with
 * every runner (CI installs chromium; local machines typically have it).
 *
 * Screenshots go to tools/scripts/browser-e2e/artifacts/ (.gitignore'd).
 */

/** @type {import('@playwright/test').PlaywrightTestConfig} */
export default {
  testMatch: "geo-clipboard.spec.mjs",
  timeout: 30_000,
  retries: 0,
  reporter: "list",
  use: {
    headless: true,
    // `navigator.clipboard` is only available in a secure context.  The
    // Ipe.Web server listens on loopback (127.0.0.1), which browsers treat
    // as a secure context, so no HTTPS setup is needed.
    baseURL: `http://127.0.0.1:${process.env.IPE_GEO_CLIPBOARD_PORT ?? "18080"}`,
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
};
