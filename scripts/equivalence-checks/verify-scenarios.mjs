// Per-app end-to-end interaction scenarios used by scripts/equivalence-checks/web-verify.mjs.
// Each scenario is async (page, opts) => void. Throw to fail.
// `opts.baseUrl` available. `opts.pause(ms)` helps with video pacing.
// `opts.skyEventPosts` is a live array the driver pushes to whenever a
// POST /_sky/event fires — use `expectSkyEventAfter(opts, page, fn, label, sel)`
// to assert a click really round-tripped to the server.  Pre-v0.13.2 a
// regression silently dropped Ipe.Ui events at render time, so the
// click became a no-op; the page still rendered and Playwright still
// "succeeded", masking the bug. The expectSkyEvent assertion forces
// the test to fail loudly on that class.
//
// `opts.log(msg)` records a line the driver surfaces on stdout — use it
// to mark any interaction a scenario could NOT truly verify (a degraded
// fallback path) so an un-run assertion is never mistaken for a pass.
//
// ASSERTION DISCIPLINE (why these helpers throw instead of swallowing):
// a scenario is a BEHAVIOURAL check, not a boot check. A step that errors,
// a target input/button that is absent, or a form submit that changed
// nothing observable must FAIL the row — never pass silently. The plain
// `clickIfPresent` / `fillByName` helpers stay for genuinely-optional
// steps but no longer swallow the underlying action's errors; the
// `*OrThrow` variants and the `expect*` assertions are for the load-bearing
// steps a scenario's meaning depends on.
//
// SHAPE-AGNOSTIC: this file drives a browser against whatever is listening
// on `baseUrl`, so it applies unchanged to ipe-emitted Rust binaries AND
// the Go-oracle reference binary alike. See docs/architecture/
// class2-tier1-sweep-fix-spec-2026-07-09.md §3.3.

const wait = (page, ms) => page.waitForTimeout(ms);

// Record a scenario diagnostic on the driver's stdout. Safe when the driver
// did not supply a logger (older driver) — falls back to console.error so the
// line is never lost.
function note(opts, msg) {
    if (opts && typeof opts.log === 'function') opts.log(msg);
    else console.error(`[scenario] ${msg}`);
}

// Optional step: click the FIRST match if it exists. Returns whether it
// existed. Does NOT swallow the click's own failure — an element that is
// present but un-clickable (detached, covered, disabled) is a real defect
// and must surface, not be masked. Absence is the only tolerated outcome,
// and the caller decides whether that is acceptable.
async function clickIfPresent(page, sel, pauseMs = 400) {
    const el = page.locator(sel).first();
    if (await el.count() > 0) {
        await el.click({ timeout: 5_000 });
        await wait(page, pauseMs);
        return true;
    }
    return false;
}

// Required click: FAIL when the target is absent. Use for a control the
// scenario's meaning depends on (a form's submit button, the one action
// under test).
async function clickOrThrow(page, sel, label, pauseMs = 400) {
    const el = page.locator(sel).first();
    if (await el.count() === 0) {
        throw new Error(`required control absent: ${label} (selector ${sel}) — `
            + `page did not render it, so the interaction could not run`);
    }
    await el.click({ timeout: 5_000 });
    await wait(page, pauseMs);
}

// Optional fill: fill the FIRST [name=…] if it exists. Returns whether it
// existed. Does NOT swallow fill errors (a present-but-unfillable field is a
// real defect). A name-less / absent input returns false; the caller decides.
async function fillByName(page, name, value, pauseMs = 200) {
    const el = page.locator(`[name="${name}"]`).first();
    if (await el.count() > 0) {
        await el.fill(value, { timeout: 5_000 });
        await wait(page, pauseMs);
        return true;
    }
    return false;
}

// Required fill: FAIL when no input with that name exists. Use for a field
// the scenario must populate for the flow to be meaningful (login creds, the
// record body under test). Silent no-op on a missing field is exactly the
// class this replaces.
async function fillOrThrow(page, name, value, pauseMs = 200) {
    const el = page.locator(`[name="${name}"]`).first();
    if (await el.count() === 0) {
        throw new Error(`required input absent: name="${name}" — `
            + `the form is missing this field, so the flow cannot be verified`);
    }
    await el.fill(value, { timeout: 5_000 });
    await wait(page, pauseMs);
}

async function gotoAndSettle(page, url, pauseMs = 800) {
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 15_000 });
    await page.waitForLoadState('networkidle', { timeout: 5_000 }).catch(() => {});
    await wait(page, pauseMs);
}

// Assert the page's visible text contains `needle` after `fn` runs (the
// observable effect of an interaction: a new row, a flash message, a changed
// counter). Throws when it does not — proving the Msg actually reshaped the
// view, not merely that the page still boots.
async function expectTextAfter(page, fn, needle, label) {
    await fn();
    const body = await page.locator('body').innerText();
    if (!body.includes(needle)) {
        throw new Error(`expected "${needle}" in the page after "${label}" but it was absent `
            + `— the interaction did not produce its observable effect `
            + `(view unchanged: probable no-op Msg / dropped event)`);
    }
}

// Run fn, then assert AT LEAST ONE new POST /_sky/event fired during fn or in
// the 500ms after. Throws when none fired — the silent-event-drop regression
// class. `opts.skyEventPosts` is the driver's live event log.
//
// When the driver did NOT wire the event log (older driver), do NOT silently
// skip: fall back to a structural DOM assertion that the interactive element
// still carries a sky-* event marker, so a stripped-event regression fails
// even on the degraded path. The degraded mode is logged so an operator can
// see the assertion ran in fallback form.
async function expectSkyEventAfter(opts, page, fn, label, sel) {
    if (opts && Array.isArray(opts.skyEventPosts)) {
        const before = opts.skyEventPosts.length;
        await fn();
        await (opts.pause ? opts.pause(500) : new Promise(r => setTimeout(r, 500)));
        const fired = opts.skyEventPosts.length - before;
        if (fired === 0) {
            throw new Error(`expected /_sky/event POST after "${label}" but none fired `
                + `(event-emission pipeline broken? button/form may be missing sky-* attrs)`);
        }
        return;
    }
    // Degraded path — assert structurally instead of skipping.
    note(opts, `event log not wired; asserting sky-* attr on "${label}" target instead of POST round-trip`);
    if (sel) {
        const el = page.locator(sel).first();
        if (await el.count() > 0) {
            const marked = await el.evaluate(node =>
                [...node.attributes].some(a => /^sky-(click|input|change|submit)$/.test(a.name))
                || !!node.closest('[sky-click],[sky-input],[sky-change],[sky-submit]'));
            if (!marked) {
                throw new Error(`degraded check: target for "${label}" (${sel}) carries no `
                    + `sky-* event attribute — event stripped at render time`);
            }
        }
    }
    await fn();
}

// ─── Scenario definitions ───────────────────────────────────────────

export const scenarios = {
    // Default — just verify body renders something.
    async smoke(page, { baseUrl }) {
        const body = await page.locator('body').innerText();
        if (!body || body.trim().length === 0) {
            throw new Error('home page rendered empty body');
        }
        await wait(page, 800);
    },

    // 09-live-counter — Increment / Decrement / Reset + About nav.
    async 'live-counter'(page, opts) {
        const incBtn = page.locator('button:has-text("+")').first();
        if (await incBtn.count() === 0) throw new Error('+ button not found');
        // First click MUST round-trip through /_sky/event — proves the
        // event-emission pipeline is intact end-to-end. Pre-v0.13.2 Ipe.Ui
        // apps would silently skip this (no sky-click attr).
        await expectSkyEventAfter(opts, page, async () => {
            await incBtn.click();
        }, 'Increment click', 'button:has-text("+")');
        // The counter value MUST change — proves the Msg reshaped the view,
        // not merely that the event POSTed. A no-op update handler would
        // pass the event assertion but leave the view frozen.
        await wait(page, 300);
        await expectTextAfter(page, async () => {
            await incBtn.click();
            await wait(page, 300);
        }, '2', 'second Increment click');
        await incBtn.click();
        await wait(page, 400);
        await clickIfPresent(page, 'button:has-text("-")', 400);
        await clickIfPresent(page, 'button:has-text("Reset")', 600);
        // Nav to About
        await clickIfPresent(page, 'button:has-text("About"), a:has-text("About")', 800);
        // Back to Counter
        await clickIfPresent(page, 'button:has-text("Counter"), a:has-text("Counter")', 600);
    },

    // 10-live-component — click any rendered buttons + check page.
    async 'live-component'(page, opts) {
        // Component demo — must render at least one interactive control, and
        // the first click must round-trip (or, degraded, carry a sky-* attr).
        const buttons = page.locator('button');
        const count = await buttons.count();
        if (count === 0) throw new Error('live-component rendered no buttons');
        await expectSkyEventAfter(opts, page, async () => {
            await buttons.first().click();
        }, 'first component button', 'button');
        for (let i = 1; i < Math.min(count, 4); i++) {
            await buttons.nth(i).click();
            await wait(page, 300);
        }
        // Type into any input (optional — not every component has one).
        const input = page.locator('input').first();
        if (await input.count() > 0) {
            await input.fill('Hello v0.13');
            await wait(page, 400);
        }
    },

    // 12-skyvote — full auth + CRUD + voting flow.
    async skyvote(page, { baseUrl }) {
        const stamp = Date.now();
        const username = 'verify_' + stamp;
        const email = `verify+${stamp}@v013.test`;
        const password = 'verify-pass-12345';

        // Visit each public page
        await gotoAndSettle(page, baseUrl + '/about', 600);
        await gotoAndSettle(page, baseUrl + '/roadmap', 600);

        // Sign up — the form fields and submit button MUST exist, else the
        // whole CRUD flow below is meaningless.
        await gotoAndSettle(page, baseUrl + '/auth/signup', 600);
        await fillOrThrow(page, 'username', username, 200);
        await fillOrThrow(page, 'email', email, 200);
        await fillOrThrow(page, 'password', password, 200);
        await clickOrThrow(page, 'button[type="submit"], button:has-text("Sign up"), button:has-text("Create")', 'sign-up submit', 1500);

        // Submit an idea (auth required). The submit form must exist; the
        // new idea's title must then appear on the board.
        const ideaTitle = 'v0.13 verification idea ' + stamp;
        await gotoAndSettle(page, baseUrl + '/submit', 600);
        await fillOrThrow(page, 'title', ideaTitle, 200);
        await fillOrThrow(page, 'description', 'Auto-generated by web-verify.mjs to exercise full Ipe.Live CRUD path.', 200);
        await clickOrThrow(page, 'button[type="submit"], button:has-text("Submit")', 'idea submit', 1500);

        // Back to board — the idea we just created MUST be visible (the CRUD
        // write actually persisted + rendered), then try to upvote it.
        await expectTextAfter(page, async () => {
            await gotoAndSettle(page, baseUrl + '/', 800);
        }, ideaTitle, 'idea appears on board after submit');
        await clickIfPresent(page, '.vote-btn, button:has-text("▲"), button:has-text("Upvote")', 800);

        // Sign out
        await clickIfPresent(page, 'a:has-text("Sign Out"), button:has-text("Sign Out"), a:has-text("Logout")', 800);
    },

    // 13-skyshop — navigate public pages only (Google OAuth blocked).
    async skyshop(page, { baseUrl }) {
        const pages = [
            '/',
            '/products',
            '/cart',
            '/privacy-policy',
            '/terms',
            '/auth/signin',  // Will render the Google sign-in button page
        ];
        for (const p of pages) {
            await gotoAndSettle(page, baseUrl + p, 800);
        }
        // Try clicking the first product if products page rendered any.
        await gotoAndSettle(page, baseUrl + '/products', 600);
        await clickIfPresent(page, 'a[href^="/product/"]', 1200);
    },

    // 16-ipehess — start new game + click squares.
    async ipehess(page, { baseUrl }) {
        // Try start new game from home
        await clickIfPresent(page, 'button:has-text("Start New Game"), button:has-text("New Game")', 1200);
        // Click some board squares (e2 to e4 attempt by pixel-position)
        const squares = page.locator('td.square, td[class*="sq"], [data-square]');
        const sqCount = await squares.count();
        if (sqCount > 0) {
            // Try a sequence — click two squares to attempt a move. These
            // squares exist (sqCount>0), so a click failure is a real defect
            // and must surface.
            await squares.nth(52).click(); // ~e2
            await wait(page, 500);
            await squares.nth(36).click(); // ~e4
            await wait(page, 600);
            await squares.nth(12).click(); // black e7
            await wait(page, 500);
            await squares.nth(28).click();
            await wait(page, 800);
        }
        // Try resign
        await clickIfPresent(page, 'button:has-text("Resign")', 800);
    },

    // 17-skymon — navigate every page (auth is GitHub OAuth, external).
    async skymon(page, { baseUrl }) {
        for (const p of ['/', '/status', '/settings', '/alerts', '/auth']) {
            await gotoAndSettle(page, baseUrl + p, 1000);
        }
    },

    // 18-job-queue — every job-type button + history controls.
    async 'job-queue'(page, opts) {
        const { baseUrl } = opts;
        // The primary "Fast Job" button MUST exist and its click MUST
        // round-trip — this is the app's core action under test.
        await expectSkyEventAfter(opts, page, async () => {
            await clickOrThrow(page, 'button:has-text("Fast Job")', 'Fast Job button', 800);
        }, 'Fast Job click', 'button:has-text("Fast Job")');
        await clickIfPresent(page, 'button:has-text("Slow Job")', 800);
        await clickIfPresent(page, 'button:has-text("Failing Job")', 800);
        await clickIfPresent(page, 'button:has-text("Batch")', 1000);
        await clickIfPresent(page, 'button:has-text("Save Snapshot")', 600);
        await clickIfPresent(page, 'button:has-text("Load History")', 600);
        await clickIfPresent(page, 'button:has-text("Clear Finished")', 600);
    },

    // 19-skyforum — login + new post + upvote + comment + logout.
    async skyforum(page, { baseUrl }) {
        const stamp = Date.now();
        // The forum is Navigate-Msg driven; look for a sign-in trigger
        // (form is shown on LoginPage, reached via clicking "Sign in").
        await clickIfPresent(page, 'a:has-text("Sign in"), button:has-text("Sign in")', 600);
        await fillOrThrow(page, 'username', 'verify_' + stamp, 200);
        // Submit login — the login control MUST exist.
        await clickOrThrow(page, 'button[type="submit"]', 'login submit', 1000);

        // Try create post — click "New Post" or compose link
        await clickIfPresent(page, 'a:has-text("New Post"), button:has-text("New Post"), a:has-text("Compose")', 800);
        const postTitle = 'v0.13 verification post ' + stamp;
        // If the compose form rendered, its fields must exist; assert the
        // post then appears. Guard on the title field's presence so forums
        // that gate posting behind email/verification don't false-fail.
        if (await page.locator('[name="title"]').count() > 0) {
            await fillOrThrow(page, 'title', postTitle, 200);
            await fillOrThrow(page, 'body', 'Body text auto-generated for v0.13 verification.', 200);
            await clickOrThrow(page, 'button[type="submit"], button:has-text("Post"), button:has-text("Submit")', 'post submit', 1200);
            await expectTextAfter(page, async () => {
                await wait(page, 400);
            }, postTitle, 'new post appears after submit');
        } else {
            note({ log: (m) => console.error(`[scenario] ${m}`) },
                'skyforum: compose form not reachable (posting likely gated) — post-create effect NOT verified');
        }

        // Upvote any visible post
        await clickIfPresent(page, 'button:has-text("▲"), .upvote, .vote-up, [aria-label*="upvote" i]', 600);

        // Click any post link to view detail
        await clickIfPresent(page, 'a[class*="post"], a[href*="post"]', 800);

        // Logout
        await clickIfPresent(page, 'a:has-text("Sign out"), button:has-text("Sign out"), a:has-text("Logout"), button:has-text("Logout")', 600);
    },

    // 08-notes-app — sign-up → sign-in → CRUD note → sign-out.
    // (Ipe.Http.Server — traditional form-POST flow.)
    async 'notes-crud'(page, { baseUrl }) {
        const stamp = Date.now();
        const email = `verify+${stamp}@v013.test`;
        const password = 'verify-pass-12345';

        // 1. Landing
        await gotoAndSettle(page, baseUrl + '/', 600);

        // 2. Sign up — form fields + submit MUST exist.
        await gotoAndSettle(page, baseUrl + '/auth/sign-up', 600);
        await fillOrThrow(page, 'email', email, 200);
        await fillOrThrow(page, 'password', password, 200);
        await fillOrThrow(page, 'confirm_password', password, 200);
        await clickOrThrow(page, 'button[type="submit"]', 'sign-up submit', 1500);

        // 3. Sign-up flow may need email verification — try sign-in regardless.
        await gotoAndSettle(page, baseUrl + '/auth/sign-in', 600);
        await fillOrThrow(page, 'email', email, 200);
        await fillOrThrow(page, 'password', password, 200);
        await clickOrThrow(page, 'button[type="submit"]', 'sign-in submit', 1500);

        // 4. Notes list (may redirect back to sign-in if email verification
        // required — so New-Note reachability is treated as optional below).
        await gotoAndSettle(page, baseUrl + '/notes', 800);

        // 5. Try New Note. If the compose form is reachable, its fields must
        // exist and the note must then appear; else surface that the CRUD
        // write could not be exercised (auth gate) rather than pass silently.
        const noteTitle = 'v0.13 verify note ' + stamp;
        if (await clickIfPresent(page, 'a:has-text("New Note"), button:has-text("New Note")', 800)
            && await page.locator('[name="title"]').count() > 0) {
            await fillOrThrow(page, 'title', noteTitle, 200);
            await fillOrThrow(page, 'body', 'auto-generated body for end-to-end notes verification.', 200);
            await clickOrThrow(page, 'button[type="submit"]', 'note submit', 1500);
            await gotoAndSettle(page, baseUrl + '/notes', 800);
            await expectTextAfter(page, async () => {}, noteTitle, 'new note appears in list after submit');
        } else {
            note({ log: (m) => console.error(`[scenario] ${m}`) },
                'notes-crud: New Note not reachable (email verification gate?) — note-create effect NOT verified');
        }

        // 6. Sign out
        await gotoAndSettle(page, baseUrl + '/auth/sign-out', 600);
    },

    // 05-mux-server — gorilla/mux example has /, /echo, /ping routes.
    async 'mux-routes'(page, { baseUrl }) {
        await gotoAndSettle(page, baseUrl + '/', 500);
        await gotoAndSettle(page, baseUrl + '/ping', 500);
        await gotoAndSettle(page, baseUrl + '/echo', 500);
    },

    // 15-http-server — Ipe.Http.Server with home, hello, api/status,
    // cookie-demo, redirect routes.
    async 'http-routes'(page, { baseUrl }) {
        await gotoAndSettle(page, baseUrl + '/', 500);
        await gotoAndSettle(page, baseUrl + '/hello/sky', 500);
        await gotoAndSettle(page, baseUrl + '/api/status', 500);
        await gotoAndSettle(page, baseUrl + '/cookie-demo', 500);
    },

    // 45-wasm-spa — WASM SPA counter (TEA: Increment / Decrement / Reset).
    // The actual browser interaction is implemented as a built-in case in
    // wasm-verify.mjs (not web-verify.mjs) because wasm examples need a static
    // file server, not a server-binary boot. This entry exists so `scenario_for`
    // resolves the key and wasm-verify.mjs routes to its runWasmSpa handler.
    async 'wasm-spa'(_page, _opts) {
        // Handled by wasm-verify.mjs's runWasmSpa (increment/decrement/reset).
        // This entry is never called directly by web-verify.mjs.
    },
};

export default scenarios;
