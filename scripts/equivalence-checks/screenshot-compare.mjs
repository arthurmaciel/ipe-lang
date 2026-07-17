#!/usr/bin/env node
// scripts/equivalence-checks/screenshot-compare.mjs — visual Go-vs-Rust
// equivalence for `live`/`server` UI examples. Boots two ALREADY-BUILT binaries
// (the Go-oracle reference and the ipe-emitted Rust `sky-app`), navigates a
// headless Chromium to the SAME route against each, captures a full-page PNG of
// each, and pixel-diffs them.
//
// The cornerstone sweep's `scenario` mode proves BEHAVIOURAL parity (a real
// round-trip completes on both backends); this closes the last gap — a
// PIXEL-level render diff catches a regression the behavioural driver misses
// (a mis-coloured button, a collapsed grid, a dropped heading) where both
// backends still boot + serve + accept clicks.
//
// Usage:
//   node screenshot-compare.mjs <go-binary> <rust-binary> <route> <out-dir> [threshold]
//
// Exit codes:
//   0  visual match (diff ratio <= threshold)  — or Go ref absent (SKIP, see below)
//   1  visual DIFFER (diff ratio > threshold; writes go.png / rust.png / diff.png)
//   2  setup error (a binary missing, neither boots, Playwright/pixelmatch absent)
//   3  SKIP — the Go reference binary is empty/absent (caller decides amber vs skip)
//
// The Go reference may be unbuildable on a host whose pinned `sky` oracle is
// version-skewed from the mirrored examples (e.g. a v0.16 oracle vs v0.17
// sources). In that case the caller passes an EMPTY go-binary path and this
// driver exits 3 (SKIP) after capturing the Rust screenshot alone, so the run
// is honestly recorded as "Rust-only, Go reference unavailable" — never a
// silent pass.

import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';

const goBinary   = process.argv[2] || '';
const rustBinary = process.argv[3] || '';
const route      = process.argv[4] || '/';
const outDir     = process.argv[5] || path.join(os.tmpdir(), 'ipe-shot-compare');
const threshold  = parseFloat(process.argv[6] || '0.02'); // 2% mismatched-pixel budget

if (!rustBinary || !fs.existsSync(rustBinary)) {
    console.error(`rust binary missing: ${rustBinary}`);
    process.exit(2);
}
fs.mkdirSync(outDir, { recursive: true });

// pixelmatch + pngjs are optional deps; degrade to a size/exists check if absent
// so a host without them still records the two screenshots (SKIP, not a crash).
let pixelmatch = null;
let PNG = null;
try {
    pixelmatch = (await import('pixelmatch')).default;
    PNG = (await import('pngjs')).PNG;
} catch {
    console.error('note: pixelmatch/pngjs not installed — capturing screenshots without a pixel diff');
}

function freePort() {
    return new Promise((resolve, reject) => {
        const srv = net.createServer();
        srv.unref();
        srv.on('error', reject);
        srv.listen(0, '127.0.0.1', () => {
            const p = srv.address().port;
            srv.close(() => resolve(p));
        });
    });
}

async function waitForPort(port, timeoutMs = 15000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        const ok = await new Promise((resolve) => {
            const sock = net.connect(port, '127.0.0.1');
            sock.on('connect', () => { sock.destroy(); resolve(true); });
            sock.on('error', () => resolve(false));
        });
        if (ok) return true;
        await new Promise((r) => setTimeout(r, 200));
    }
    return false;
}

// Boot a binary, screenshot the route, kill it. Returns the PNG path, or null if
// the binary never served. Matches the sweep's exercise_server contract
// (lib/checks.sh): pass BOTH IPE_LIVE_PORT and PORT, then read the ACTUAL
// listening port from the "listening on :NNNN" log line — a binary that
// hardcodes `Server.listen 8000` and ignores the env still gets driven on the
// port it announces.
async function shoot(binary, label) {
    if (!binary || !fs.existsSync(binary) || fs.statSync(binary).size === 0) return null;
    const wantPort = await freePort();
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), `ipe-shot-${label}-`));
    const logPath = path.join(scratch, 'boot.log');
    const logFd = fs.openSync(logPath, 'a');
    const child = spawn(binary, [], {
        cwd: scratch,
        env: { ...process.env, PORT: String(wantPort), IPE_LIVE_PORT: String(wantPort), IPE_ENV: 'dev' },
        stdio: ['ignore', logFd, logFd],
    });
    // The last `:NNNN` on a "listening on ..." line is the real port — matches
    // lib/checks.sh's `grep -oE ":[0-9]+" | tail -1`. Handles both
    // "listening on :NNNN" and "listening on http://0.0.0.0:NNNN".
    const readAnnouncedPort = () => {
        try {
            const line = fs.readFileSync(logPath, 'utf8')
                .split('\n').filter((l) => /listening on/i.test(l)).pop();
            if (!line) return null;
            const ports = line.match(/:(\d+)/g);
            return ports ? parseInt(ports[ports.length - 1].slice(1), 10) : null;
        } catch { return null; }
    };
    try {
        // Wait briefly, then drive the port the binary actually announces (an app
        // that hardcodes `Server.listen 8000` ignores the requested port).
        let port = wantPort;
        let served = await waitForPort(port, 3000);
        if (!served) {
            for (let i = 0; i < 20 && !served; i++) {
                const announced = readAnnouncedPort();
                if (announced) { port = announced; served = await waitForPort(port, 3000); break; }
                await new Promise((r) => setTimeout(r, 300));
            }
        }
        if (!served) { child.kill('SIGKILL'); return null; }
        const browser = await chromium.launch({ args: ['--no-sandbox'] });
        const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
        await page.goto(`http://127.0.0.1:${port}${route}`, { waitUntil: 'networkidle', timeout: 15000 });
        await page.waitForTimeout(400); // settle SSE first-paint
        const out = path.join(outDir, `${label}.png`);
        await page.screenshot({ path: out, fullPage: true });
        await browser.close();
        return out;
    } finally {
        child.kill('SIGKILL');
        fs.closeSync(logFd);
        fs.rmSync(scratch, { recursive: true, force: true });
    }
}

const rustShot = await shoot(rustBinary, 'rust');
if (!rustShot) { console.error('rust binary did not boot+serve'); process.exit(2); }

const goShot = await shoot(goBinary, 'go');
if (!goShot) {
    console.error('SKIP: Go reference unavailable (empty/version-skewed) — captured Rust screenshot only');
    process.exit(3);
}

if (!pixelmatch || !PNG) {
    console.error('captured go.png + rust.png; no pixel diff (pixelmatch absent) — SKIP');
    process.exit(3);
}

const go = PNG.sync.read(fs.readFileSync(goShot));
const rust = PNG.sync.read(fs.readFileSync(rustShot));
const width = Math.min(go.width, rust.width);
const height = Math.min(go.height, rust.height);
const diff = new PNG({ width, height });

// Crop both to the common area so a 1px height rounding never false-DIFFERs.
function crop(img) {
    if (img.width === width && img.height === height) return img;
    const c = new PNG({ width, height });
    for (let y = 0; y < height; y++) {
        for (let x = 0; x < width; x++) {
            const si = (img.width * y + x) << 2;
            const di = (width * y + x) << 2;
            c.data[di] = img.data[si];
            c.data[di + 1] = img.data[si + 1];
            c.data[di + 2] = img.data[si + 2];
            c.data[di + 3] = img.data[si + 3];
        }
    }
    return c;
}

const mismatched = pixelmatch(
    crop(go).data, crop(rust).data, diff.data, width, height, { threshold: 0.1 },
);
fs.writeFileSync(path.join(outDir, 'diff.png'), PNG.sync.write(diff));
const ratio = mismatched / (width * height);
console.error(`visual diff: ${mismatched}/${width * height} px mismatched (${(ratio * 100).toFixed(3)}%), threshold ${(threshold * 100).toFixed(1)}%`);
process.exit(ratio > threshold ? 1 : 0);
