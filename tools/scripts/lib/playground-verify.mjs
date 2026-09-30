#!/usr/bin/env node
// Headless verification of the in-browser Ipê playground in Chromium.
//
// Static mode (the GitHub Pages shape): serves the playground directory under
// a sub-path with no run server behind it, then checks the WASM compiler boots
// and emits Rust for the sample, Run is disabled with the local-server notice
// (never a raw fetch error), the ACE theme re-themes the UI, a type error
// surfaces as a diagnostic, and the GitHub link target. It also checks every
// vendored ACE file against `static/vendor/ace/SHA256SUMS`.
//
// Live mode (`--live <url>`): checks `GET /` carries `X-Frame-Options: DENY`
// and the page's Content-Security-Policy (the `<meta>` policy plus
// `frame-ancestors 'none'`); that Run stays disabled without a well-formed
// `#t=` fragment and inside a frame; then opens the launch URL the playground
// server printed (with its `#t=<token>` fragment), presses Run, and waits for
// the sample's jailed program output.
//
// In both modes every request the page makes is intercepted, and any to
// another origin is aborted and fails the check: the page holds the launch
// token, so it may load nothing from a third party.
//
// Usage: node playground-verify.mjs <playground-dir> [port]
//        node playground-verify.mjs --live 'http://127.0.0.1:8000/#t=<token>'
// Exit 0 on pass, non-zero on any failure.

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const SUB_PATH = '/compiler/playground/';
const NO_SERVER = 'Run needs the local server — see README';
const SAMPLE_OUTPUT = 'double 21 = 42';
const NO_TOKEN = 'Run needs the launch URL the server printed';
const SCRIPT_SRC = "script-src 'self' 'wasm-unsafe-eval'";
const VENDOR_DIR = path.join('static', 'vendor', 'ace');

const live = process.argv[2] === '--live' ? process.argv[3] : null;
const dir = live ? null : path.resolve(process.argv[2] || '');
const port = parseInt(process.argv[3] || '8199', 10);
if (!live && (!dir || !fs.existsSync(path.join(dir, 'index.html')))) {
  console.error('usage: node playground-verify.mjs <playground-dir> [port] | --live <url>');
  process.exit(2);
}

const MIME = { '.html': 'text/html', '.js': 'application/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
// A static host: files under SUB_PATH only, 404 for everything else (so
// `health` and `run` are absent, as on GitHub Pages).
const server = live ? null : createServer((req, res) => {
  const p = decodeURIComponent(req.url.split('?')[0]);
  const rel = p.startsWith(SUB_PATH) ? p.slice(SUB_PATH.length) : null;
  const fp = rel === null ? null : path.join(dir, rel === '' ? 'index.html' : rel);
  if (fp === null || !fp.startsWith(dir + path.sep)) { res.writeHead(404); res.end('nf'); return; }
  fs.readFile(fp, (err, data) => {
    if (err) { res.writeHead(404); res.end('nf'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(fp)] || 'application/octet-stream' });
    res.end(data);
  });
});

let ok = true;
function fail(msg) { ok = false; console.error('FAIL:', msg); }
function check(cond, pass, failure) { if (cond) console.log('PASS:', pass); else fail(failure); }
const text = (page, id) => page.evaluate((i) => document.getElementById(i)?.textContent ?? '', id);

// Abort and record every request the context makes to an origin other than
// `origin`, and record every failed same-origin load (a missing vendored file).
async function sameOriginOnly(context, origin, missing) {
  const foreign = [];
  await context.route('**/*', (route) => {
    const url = route.request().url();
    if (new URL(url).origin === origin) return route.continue();
    foreign.push(url);
    return route.abort();
  });
  context.on('response', (resp) => {
    const u = new URL(resp.url());
    const probe = u.pathname.endsWith('/health') || u.pathname.endsWith('/run')
      || u.pathname.endsWith('/favicon.ico');
    if (resp.status() >= 400 && !probe) missing.push(`${resp.status()} ${resp.url()}`);
  });
  return foreign;
}

// Every vendored file is the one `SHA256SUMS` records, and every one is recorded.
function vendorChecks() {
  const vendor = path.join(dir, VENDOR_DIR);
  const listed = new Map(fs.readFileSync(path.join(vendor, 'SHA256SUMS'), 'utf8')
    .trim().split('\n').map((line) => { const [sum, name] = line.split(/\s+/); return [name, sum]; }));
  const present = fs.readdirSync(vendor).filter((f) => f.endsWith('.js'));
  const unlisted = present.filter((f) => !listed.has(f));
  const drifted = [...listed].filter(([name, sum]) => !fs.existsSync(path.join(vendor, name))
    || createHash('sha256').update(fs.readFileSync(path.join(vendor, name))).digest('hex') !== sum);
  check(unlisted.length === 0 && drifted.length === 0 && listed.size > 0,
    `${listed.size} vendored ACE files match SHA256SUMS`,
    `vendored ACE drift: unlisted=${unlisted.join(',')} drifted=${drifted.map(([n]) => n).join(',')}`);
}

const metaPolicy = (page) => page.evaluate(
  () => document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content ?? '');

// Run stays disabled on a page the run server is up for but that holds no
// token: the frame's page (or the top page) shows the no-token notice once
// the sample has compiled.
async function runRefused(frame, label) {
  await frame.waitForFunction(
    (msg) => document.getElementById('run-output')?.textContent?.includes(msg)
      && document.getElementById('output')?.textContent?.includes('==== src/main.rs ===='),
    NO_TOKEN,
    { timeout: 30000 },
  );
  const disabled = await frame.evaluate(() => document.getElementById('run-btn').disabled);
  check(disabled, `${label} -> Run stays disabled`, `${label}: Run enabled`);
}

async function refusalChecks(browser, origin, token) {
  const resp = await fetch(origin + '/');
  const xfo = resp.headers.get('x-frame-options');
  const csp = resp.headers.get('content-security-policy') ?? '';
  check(xfo === 'DENY', 'GET / sends X-Frame-Options: DENY', `X-Frame-Options: ${xfo}`);
  check(csp.split(';').map((d) => d.trim()).includes(SCRIPT_SRC),
    `GET / CSP pins ${SCRIPT_SRC}`, `GET / CSP: ${csp}`);

  const fresh = async () => {
    const context = await browser.newContext();
    const missing = [];
    const foreign = await sameOriginOnly(context, origin, missing);
    return { context, page: await context.newPage(), foreign, missing };
  };
  const done = async ({ context, foreign, missing }, label) => {
    check(foreign.length === 0, `${label}: no request left the origin`, `${label}: foreign requests ${foreign.join(', ')}`);
    check(missing.length === 0, `${label}: every same-origin load succeeded`, `${label}: failed loads ${missing.join(', ')}`);
    await context.close();
  };

  const first = await fresh();
  await first.page.goto(origin + '/', { waitUntil: 'load' });
  const meta = await metaPolicy(first.page);
  check(csp === `${meta}; frame-ancestors 'none'`, 'GET / CSP is the page <meta> policy plus frame-ancestors',
    `header CSP ${JSON.stringify(csp)} vs meta ${JSON.stringify(meta)}`);
  await runRefused(first.page, 'no #t= fragment');
  await done(first, 'no fragment');

  for (const [label, hash] of [
    ['a short token', '#t=' + token.slice(0, 42)],
    ['a long token', '#t=' + token + 'a'],
    ['a standard-base64 token', '#t=' + token.slice(0, 42) + '+'],
    ['an empty token', '#t='],
    ['another fragment key', '#x=' + token],
  ]) {
    const f = await fresh();
    await f.page.goto(origin + '/' + hash, { waitUntil: 'load' });
    await runRefused(f.page, label);
    await done(f, label);
  }

  // A same-origin frame with the real token, its framing headers stripped so
  // the page's own frame guard is what is under test: it takes no token.
  const framed = await fresh();
  await framed.context.route(origin + '/', async (route) => {
    const upstream = await route.fetch();
    const headers = { ...upstream.headers() };
    delete headers['x-frame-options'];
    headers['content-security-policy'] = meta;
    await route.fulfill({ response: upstream, headers });
  });
  await framed.context.route(origin + '/framer', (route) => route.fulfill({
    contentType: 'text/html',
    body: `<!DOCTYPE html><iframe src="/#t=${token}" width="1200" height="800"></iframe>`,
  }));
  await framed.page.goto(origin + '/framer', { waitUntil: 'load' });
  const child = await (await framed.page.waitForSelector('iframe')).contentFrame();
  await runRefused(child, 'the page inside a frame, holding the real token');
  await done(framed, 'framed');

  // With the real headers, a cross-origin frame never gets the page at all.
  const framer = await browser.newContext();
  await framer.route('http://framer.invalid/', (route) => route.fulfill({
    contentType: 'text/html',
    body: `<!DOCTYPE html><iframe src="${origin}/#t=${token}"></iframe>`,
  }));
  const fp = await framer.newPage();
  await fp.goto('http://framer.invalid/', { waitUntil: 'load' });
  await fp.waitForTimeout(1000);
  const cross = fp.frames().find((fr) => fr !== fp.mainFrame());
  const rendered = cross ? await cross.evaluate(() => Boolean(document.getElementById('run-btn'))).catch(() => false) : false;
  check(!rendered, 'a cross-origin frame is refused the page', 'the page rendered inside a cross-origin frame');
  await framer.close();
}

async function bootChecks(page) {
  await page.waitForFunction(
    () => document.getElementById('output')?.textContent?.includes('==== src/main.rs ===='),
    { timeout: 30000 },
  );
  console.log('PASS: sample program compiled in-browser, emitted Rust shown');
}

async function staticChecks(page) {
  await page.waitForFunction(
    (msg) => document.getElementById('run-output')?.textContent?.includes(msg),
    NO_SERVER,
    { timeout: 15000 },
  );
  const disabled = await page.evaluate(() => document.getElementById('run-btn').disabled);
  const title = await page.getAttribute('#run-btn', 'title');
  if (disabled && title === NO_SERVER) console.log('PASS: no run server -> Run disabled with the local-server notice');
  else fail(`Run not degraded: disabled=${disabled} title=${title}`);
  const status = await text(page, 'status-bar');
  if (/fetch|unreachable|HTTP \d/i.test(status + await text(page, 'run-output'))) fail('raw fetch error shown: ' + status);
  else console.log('PASS: no raw fetch error shown');

  const href = await page.getAttribute('a.gh', 'href');
  if (href && href.includes('/ipe-lang/compiler') && href.includes('examples/wasm/language-playground')) {
    console.log('PASS: GitHub link resolves ->', href);
  } else fail('GitHub link wrong: ' + href);

  const pageTitle = await page.title();
  if (pageTitle === 'Ipê playground') console.log('PASS: title is "Ipê playground"');
  else fail('title: ' + pageTitle);

  // The theme switch re-themes BOTH editor and UI.
  const readColors = () => page.evaluate(() => ({
    ui: getComputedStyle(document.querySelector('header')).backgroundColor,
    rootBg: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(),
  }));
  // A theme chunk loads lazily from the vendored dir: wait for `--bg` to move off its
  // previous value rather than for a fixed delay.
  const setTheme = async (t) => {
    const before = (await readColors()).rootBg;
    await page.selectOption('#theme-select', t);
    await page.waitForFunction(
      (prev) => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() !== prev,
      before,
      { timeout: 15000 },
    ).catch(() => {});
    return readColors();
  };
  const dark = await setTheme('ace/theme/monokai');
  const light = await setTheme('ace/theme/github');
  if (dark.ui !== light.ui && dark.rootBg !== light.rootBg) {
    console.log('PASS: theme switch re-themes UI (header/--bg changed):', dark.ui, '->', light.ui);
  } else fail(`UI did not re-theme: ${JSON.stringify(dark)} vs ${JSON.stringify(light)}`);

  // A type error surfaces as a diagnostic, not a crash.
  await page.evaluate(() => {
    window.ace.edit(document.getElementById('editor'))
      .setValue('module Main exposing (main)\n\nmain : Int\nmain = "not an int"\n', -1);
  });
  await page.waitForFunction(
    () => document.getElementById('status-bar')?.textContent?.includes('error'),
    { timeout: 15000 },
  );
  if ((await text(page, 'output')).trim().length > 0) console.log('PASS: type error reported as diagnostic');
  else fail('no diagnostic text for type error');
}

async function liveChecks(page) {
  await page.waitForFunction(() => !document.getElementById('run-btn').disabled, { timeout: 15000 });
  console.log('PASS: run server found -> Run enabled');
  await page.click('#run-btn');
  await page.waitForFunction(
    () => document.getElementById('run-title')?.textContent !== 'Program output'
       || document.getElementById('status-bar')?.textContent?.includes('Ran '),
    { timeout: 120000 },
  );
  const out = await text(page, 'run-output');
  const status = await text(page, 'status-bar');
  if (out.includes(SAMPLE_OUTPUT) && status.includes('sandboxed')) {
    console.log('PASS: Run built and executed the sample in the jail:', JSON.stringify(out));
  } else fail(`Run output: ${JSON.stringify(out)} status: ${JSON.stringify(status)}`);
}

if (server) await new Promise((r) => server.listen(port, r));
const browser = await chromium.launch();
try {
  const target = live || `http://localhost:${port}${SUB_PATH}`;
  const origin = new URL(target).origin;
  if (live) {
    const token = /^#t=([A-Za-z0-9_-]{43})$/.exec(new URL(live).hash)?.[1];
    if (!token) throw new Error('--live needs the printed launch URL, with its #t=<token> fragment');
    await refusalChecks(browser, origin, token);
  } else vendorChecks();
  const context = await browser.newContext();
  const missing = [];
  const foreign = await sameOriginOnly(context, origin, missing);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(target, { waitUntil: 'load' });
  const meta = await metaPolicy(page);
  check(meta.split(';').map((d) => d.trim()).includes(SCRIPT_SRC),
    `the page's <meta> CSP pins ${SCRIPT_SRC}`, `meta CSP: ${meta}`);
  await bootChecks(page);
  if (live) await liveChecks(page);
  else await staticChecks(page);
  check(foreign.length === 0, 'no request left the page origin', 'foreign requests: ' + foreign.join(', '));
  check(missing.length === 0, 'every same-origin load succeeded', 'failed loads: ' + missing.join(', '));
  if (errors.length) fail('page errors: ' + errors.join('; '));
} catch (e) {
  fail(String(e));
} finally {
  await browser.close();
  if (server) server.close();
}
process.exit(ok ? 0 : 1);
