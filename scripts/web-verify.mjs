#!/usr/bin/env node
// scripts/web-verify.mjs — the `scenario`-mode Go≡Rust equivalence driver for
// `live`-shape examples (scripts/lib/checks.sh's exercise_live / examples-sweep.sh's
// equiv_for "scenario" branch). Boots an ALREADY-BUILT binary, drives a real
// headless Chromium against it via the scenario in verify-scenarios.mjs, and
// exits 0/1 — the SAME driver runs against both the skyc-emitted Rust binary
// AND the Go-oracle reference binary (see checks.sh's two exercise_live calls
// in examples-sweep.sh's "scenario" equiv branch), so a real browser round-trip
// is the equivalence signal instead of the boot-floor fallback (both processes
// merely listening).
//
// ADAPTED from ../sky/scripts/verify-live-app.mjs. Two adaptations for this
// repo's two-binary (Go-oracle + Rust) shape, per checks.sh:88-90's ALREADY-
// EXPECTED CLI contract (exercise_live calls `node "$DRIVER" "$ex" "$port"
// "$scen" "$abin"`):
//   1. Binary path is the caller's 4th positional arg (checks.sh's resolve_bin /
//      _abs_bin already resolved it — Go oracle's `sky-out/app` or the Rust
//      `sky-app`), NOT derived from `examples/<name>/sky-out/app` — this driver
//      has no opinion on which backend built it.
//   2. Spawns from a FRESH TMPDIR scratch cwd (not the example dir) — matches
//      this repo's exercise_server/_boot_server_at convention (lib/checks.sh)
//      so cwd-relative app state (sqlite files, static dirs) never leaks into
//      the repo tree across repeated sweep runs.
// Otherwise the pipeline is the same: spawn → wait for the port → Playwright →
// run the named scenario → screenshot + console/panic checks → kill → report.
//
// Usage:
//   node scripts/web-verify.mjs <example-name> <port> <scenario> <abin>
//
// Exits 0 on pass, non-zero on any failure. See
// docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md §3.3.

import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname  = path.dirname(__filename);
const repoRoot   = path.resolve(__dirname, '..');

const exampleName  = process.argv[2];
const port         = parseInt(process.argv[3] || '8000', 10);
const scenarioName = process.argv[4] || 'smoke';
const binary        = process.argv[5];

if (!exampleName || !binary) {
    console.error('usage: node web-verify.mjs <example-name> <port> <scenario> <abin>');
    process.exit(2);
}

if (!fs.existsSync(binary)) {
    console.error(`binary missing: ${binary}`);
    process.exit(2);
}

// Fresh scratch run dir — cwd for the spawned binary AND the artefact output
// dir. mkdtemp guarantees no collision between the Go-oracle invocation and
// the Rust invocation of the SAME example that examples-sweep.sh runs
// back-to-back (equiv_for's "scenario" branch calls exercise_live twice).
const runDir = fs.mkdtempSync(path.join(os.tmpdir(), `sky-webverify-${exampleName}-`));

// ─── Helpers ────────────────────────────────────────────────────────

function waitForPort(p, timeoutMs) {
    const deadline = Date.now() + timeoutMs;
    return new Promise((resolve, reject) => {
        const tick = () => {
            if (Date.now() > deadline) {
                reject(new Error(`port ${p} never accepted within ${timeoutMs}ms`));
                return;
            }
            const sock = net.connect(p, '127.0.0.1');
            sock.on('connect', () => { sock.end(); resolve(); });
            sock.on('error', () => { setTimeout(tick, 200); });
        };
        tick();
    });
}

// Reachable-from-either-backend abort signatures — Go panics AND Rust panics
// both surface as a hard equivalence failure. Superset of checks.sh's
// PANIC_RE (shared with exercise_cli/exercise_server) so this driver flags
// the same abort classes the rest of the sweep does.
const PANIC_PATTERNS = [
    /panic:/i,
    /panicked/i,
    /runtime error:/i,
    /goroutine \d+ \[/,             // Go stack trace
    /interface conversion:/,
    /CompilerBug/,
    /RUST_BACKTRACE/,
    /index out of bounds/,
    /unwrap\(\) on/,
    /called `Result::unwrap/,
];

async function main() {
    const env = { ...process.env, PORT: String(port), SKY_LIVE_PORT: String(port) };
    const serverLogPath = path.join(runDir, 'server.log');
    const serverLog = fs.createWriteStream(serverLogPath);
    const child = spawn(binary, [], { env, cwd: runDir });
    child.stdout.pipe(serverLog);
    child.stderr.pipe(serverLog);

    let serverExitedEarly = null;
    child.on('exit', (code, signal) => {
        if (signal !== 'SIGTERM' && signal !== 'SIGKILL') {
            serverExitedEarly = { code, signal };
        }
    });

    try {
        await waitForPort(port, 15_000);
    } catch (err) {
        child.kill('SIGKILL');
        await new Promise(r => setTimeout(r, 200));
        const log = fs.existsSync(serverLogPath) ? fs.readFileSync(serverLogPath, 'utf8') : '';
        console.error(`FAIL ${exampleName} — server failed to listen: ${err.message}`);
        console.error('--- server log ---');
        console.error(log.split('\n').slice(0, 40).join('\n'));
        process.exit(1);
    }

    if (serverExitedEarly) {
        console.error(`FAIL ${exampleName} — server exited early: code=${serverExitedEarly.code} signal=${serverExitedEarly.signal}`);
        console.error(fs.existsSync(serverLogPath) ? fs.readFileSync(serverLogPath, 'utf8') : '');
        process.exit(1);
    }

    // Playwright — reuse the system/OS chromium checks.sh already gates on
    // (SKY_CHROMIUM, default /usr/bin/chromium) rather than requiring a
    // separate `npx playwright install chromium` browser-binary download.
    const executablePath = process.env.SKY_CHROMIUM && fs.existsSync(process.env.SKY_CHROMIUM)
        ? process.env.SKY_CHROMIUM
        : undefined;
    const browser = await chromium.launch({ headless: true, executablePath });
    const context = await browser.newContext({
        viewport: { width: 1280, height: 720 },
        recordVideo: process.env.SKY_RECORD ? { dir: runDir } : undefined,
    });
    if (process.env.SKY_TRACE) {
        await context.tracing.start({ screenshots: true, snapshots: true });
    }
    const page = await context.newPage();

    const consoleErrors = [];
    page.on('console', msg => {
        if (msg.type() === 'error') {
            const loc = msg.location();
            const where = loc && loc.url ? ` (${loc.url})` : '';
            consoleErrors.push(msg.text() + where);
        }
    });
    page.on('pageerror', err => consoleErrors.push(`pageerror: ${err.message}`));

    // Track failed network requests to distinguish benign 404s (favicon,
    // service-worker probes) from real app failures.
    const networkFailures = [];
    page.on('response', res => {
        const status = res.status();
        if (status >= 400) {
            networkFailures.push(`${status} ${res.url()}`);
        }
    });

    // Hard-fail probe to catch the "click is a no-op" class (events stripped
    // at render time → button has no sky-click attr → DOM click never POSTs
    // /_sky/event). Watch every /_sky/event POST; scenarios assert at least
    // one round-trip via expectSkyEventAfter so a silent regression in the
    // event-emission pipeline can't ship.
    const skyEventPosts = [];
    page.on('request', req => {
        if (req.method() === 'POST' && req.url().includes('/_sky/event')) {
            skyEventPosts.push({
                url: req.url(),
                postData: req.postData() || '',
                ts: Date.now(),
            });
        }
    });

    let outcome = 'PASS';
    let detail = '';
    try {
        const baseUrl = `http://127.0.0.1:${port}`;

        await page.goto(baseUrl, { waitUntil: 'domcontentloaded', timeout: 10_000 });
        await page.waitForLoadState('networkidle', { timeout: 5_000 }).catch(() => {});

        const pause = (ms) => page.waitForTimeout(ms);

        // Scenario diagnostic log — every line a scenario records via
        // opts.log (degraded fallbacks, un-verifiable steps) is surfaced on
        // this driver's stdout so a step a scenario could NOT truly verify is
        // visible in the sweep report, never a silent pass.
        const scenarioLog = [];
        const log = (msg) => scenarioLog.push(String(msg));

        // Load the per-app scenario module.
        const scenarioMod = await import(pathToFileURL(path.join(__dirname, 'verify-scenarios.mjs')).href);
        const scenarios = scenarioMod.scenarios || scenarioMod.default;
        const scenarioFn = scenarios[scenarioName];
        if (typeof scenarioFn !== 'function') {
            throw new Error(`unknown scenario: ${scenarioName} (known: ${Object.keys(scenarios).join(', ')})`);
        }
        await scenarioFn(page, { baseUrl, pause, skyEventPosts, log });

        if (scenarioLog.length > 0) {
            console.log(`--- scenario ${scenarioName}: ${scenarioLog.length} unverified/degraded step(s) ---`);
            for (const line of scenarioLog) console.log(`  UNVERIFIED: ${line}`);
        }

        await page.screenshot({ path: path.join(runDir, 'home.png'), fullPage: false }).catch(() => {});

        // Structural HTML assertion — if the rendered home page has
        // <button> or <form>, it MUST have at least one
        // `sky-(click|input|change|submit)=` attribute. A silent
        // event-dropping regression would render buttons without any
        // sky-* event marker. Scenario-runner probes confirm the round
        // trip; this check catches even scenarios that don't exercise a
        // click but still have an event-bearing UI.
        const homeHtml = await page.content();
        const hasInteractive = /<button|<form/i.test(homeHtml);
        const hasSkyEvent = /sky-(click|input|change|submit)="/i.test(homeHtml);
        if (hasInteractive && !hasSkyEvent && outcome === 'PASS') {
            outcome = 'FAIL';
            detail = 'rendered HTML has <button>/<form> but ZERO sky-event '
                + 'attributes — events stripped at render time (probable '
                + 'Std.Ui → []any coercion regression)';
        }

        // Console-error filter — treat 404s on common static-asset paths
        // (favicon.ico, robots.txt, apple-touch-icon, manifest.json) as
        // BENIGN. They're browser auto-probes, not app failures. Any
        // other console error counts.
        const benignAssetRe = /(favicon\.ico|robots\.txt|apple-touch-icon|manifest\.json|sitemap\.xml)/i;
        const realConsoleErrors = consoleErrors.filter(e => !benignAssetRe.test(e));
        if (realConsoleErrors.length > 0 && outcome === 'PASS') {
            outcome = 'FAIL';
            detail = `console errors: ${realConsoleErrors.slice(0, 5).join('; ')}`;
        }

        if (networkFailures.length > 0) {
            fs.writeFileSync(
                path.join(runDir, 'network-failures.log'),
                networkFailures.join('\n')
            );
        }
    } catch (err) {
        outcome = 'FAIL';
        detail = `playwright: ${err.message}`;
        await page.screenshot({ path: path.join(runDir, 'error.png'), fullPage: false }).catch(() => {});
    }

    if (process.env.SKY_TRACE) {
        await context.tracing.stop({ path: path.join(runDir, 'trace.zip') }).catch(() => {});
    }
    await browser.close();

    // Kill server gracefully
    child.kill('SIGTERM');
    await new Promise(r => setTimeout(r, 500));
    if (!child.killed) child.kill('SIGKILL');

    // Tail server log for panic / runtime error — this is the equivalence
    // driver's OWN abort detection (in addition to whatever exit-code /
    // grep the caller applies to this process's own stdout/stderr).
    const log = fs.existsSync(serverLogPath) ? fs.readFileSync(serverLogPath, 'utf8') : '';
    const panics = PANIC_PATTERNS.flatMap(re => {
        const m = log.match(re);
        return m ? [m[0]] : [];
    });
    if (panics.length > 0) {
        outcome = 'FAIL';
        detail = (detail ? detail + '; ' : '') + 'server panics: ' + panics.join(', ');
    }

    // Best-effort cleanup of the scratch dir on pass — keep it around on
    // fail so a human/CI artefact step can inspect screenshots + logs.
    if (outcome === 'PASS') {
        fs.rmSync(runDir, { recursive: true, force: true });
    }

    if (outcome === 'PASS') {
        console.log(`PASS ${exampleName} (port ${port}, scenario ${scenarioName}, bin ${binary})`);
        process.exit(0);
    } else {
        console.error(`FAIL ${exampleName} — ${detail}`);
        console.error('artefacts in ' + runDir);
        console.error('--- last 30 lines of server.log ---');
        console.error(log.split('\n').slice(-30).join('\n'));
        process.exit(1);
    }
}

main().catch(err => {
    console.error('FAIL ' + exampleName + ' — driver error: ' + err.message);
    console.error(err.stack);
    process.exit(1);
});
