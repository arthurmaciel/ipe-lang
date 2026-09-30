#!/usr/bin/env node
// Headless verification of the in-browser Ipê playground in Chromium.
//
// Static mode (the GitHub Pages shape): serves the playground directory under
// a sub-path with no run server behind it, then checks the WASM compiler boots
// and emits Rust for the sample, Run is disabled with the local-server notice
// (never a raw fetch error), the ACE theme re-themes the UI, a type error
// surfaces as a diagnostic, and the GitHub link target. It also checks every
// vendored ACE file against `static/vendor/ace/SHA256SUMS`, and that the
// policy the run server sends (`pageContentSecurityPolicy` in
// `server/src/Gate.ipe`, and the `X-Frame-Options` in `server/src/Main.ipe`)
// is the page's `<meta>` policy plus `frame-ancestors 'none'`, compared as
// parsed directives. It then serves the page with each of those framing
// headers alone and proves a cross-origin frame is refused it by that header:
// a control frame of the same origin loads, the page's response is delivered,
// and Chromium blocks it naming the header.
//
// Live mode (`--live <url>`): checks `GET /` carries `X-Frame-Options: DENY`
// and the page's Content-Security-Policy (the `<meta>` policy plus
// `frame-ancestors 'none'`); that Run stays disabled without a well-formed
// `#t=` fragment and inside a same-origin frame; that a cross-origin frame is
// refused the page (proved as in static mode); then opens the launch URL the playground
// server printed (with its `#t=<token>` fragment), presses Run, and waits for
// the sample's jailed program output.
//
// In both modes every request the page makes is intercepted, and any to
// another origin is aborted and fails the check: the page holds the launch
// token, so it may load nothing from a third party.
//
// Usage: node playground-verify.mjs <playground-dir> [port]   (needs `pkg/`)
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
const FRAME_ANCESTORS_NONE = "frame-ancestors 'none'";
// Chromium's console text for a frame it refused on each framing header.
const REFUSED_BY_CSP = /frame-ancestors 'none'/;
const REFUSED_BY_XFO = /X-Frame-Options/i;
const REFUSED_BY_EITHER = /frame-ancestors 'none'|X-Frame-Options/i;

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
// `?frame=csp` / `?frame=xfo` serve the page with exactly one of the run
// server's framing headers, for the cross-origin frame checks.
let framingHeaders = { csp: {}, xfo: {} };
const server = live ? null : createServer((req, res) => {
  const [rawPath, query = ''] = req.url.split('?');
  const p = decodeURIComponent(rawPath);
  const extra = framingHeaders[new URLSearchParams(query).get('frame')] ?? {};
  const rel = p.startsWith(SUB_PATH) ? p.slice(SUB_PATH.length) : null;
  const fp = rel === null ? null : path.join(dir, rel === '' ? 'index.html' : rel);
  if (fp === null || !fp.startsWith(dir + path.sep)) { res.writeHead(404); res.end('nf'); return; }
  fs.readFile(fp, (err, data) => {
    if (err) { res.writeHead(404); res.end('nf'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(fp)] || 'application/octet-stream', ...extra });
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

// A Content-Security-Policy as parsed directives: name -> sorted source list.
// Directive order, source order and whitespace carry no meaning, so two
// policies are compared as these maps, never as strings. A repeated
// directive (a browser ignores every copy after the first), a repeated source
// or a malformed name is refused rather than normalised.
function parseCsp(text, label) {
  const policy = new Map();
  for (const raw of text.split(';')) {
    const [name, ...sources] = raw.trim().split(/\s+/).filter(Boolean);
    if (name === undefined) continue;
    const key = name.toLowerCase();
    if (!/^[a-z][a-z-]*$/.test(key)) throw new Error(`${label}: malformed CSP directive ${JSON.stringify(name)}`);
    if (policy.has(key)) throw new Error(`${label}: CSP directive ${key} appears twice`);
    if (new Set(sources).size !== sources.length) throw new Error(`${label}: CSP directive ${key} repeats a source`);
    policy.set(key, [...sources].sort());
  }
  if (policy.size === 0) throw new Error(`${label}: empty CSP`);
  return policy;
}

// Every directive on which two parsed policies disagree (empty when equal).
function cspDiff(expected, actual) {
  const names = [...new Set([...expected.keys(), ...actual.keys()])].sort();
  return names.flatMap((name) => {
    const e = expected.get(name)?.join(' ');
    const a = actual.get(name)?.join(' ');
    return e === a ? [] : [`${name}: expected ${e ?? '(absent)'}, got ${a ?? '(absent)'}`];
  });
}

// The policy the run server must send: the page's <meta> policy (which may not
// carry `frame-ancestors`, as a <meta> policy ignores it) plus that directive.
function servedPolicy(meta) {
  const policy = parseCsp(meta, 'the page <meta> CSP');
  if (policy.has('frame-ancestors')) throw new Error('the page <meta> CSP carries frame-ancestors, which a <meta> policy ignores');
  return new Map([...policy, ...parseCsp(FRAME_ANCESTORS_NONE, 'frame-ancestors')]);
}

// The comparison itself refuses a drifted policy: a dropped source, an added
// directive, a repeated directive. A comparer that accepts these proves nothing.
function cspSelfChecks() {
  const base = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'";
  const refusedParse = (text) => { try { parseCsp(text, 'probe'); return false; } catch { return true; } };
  check(cspDiff(parseCsp(base, 'a'), parseCsp("script-src 'wasm-unsafe-eval'  'self' ;default-src 'self';", 'b')).length === 0
      && cspDiff(parseCsp(base, 'a'), parseCsp("default-src 'self'; script-src 'self'", 'b')).length === 1
      && cspDiff(parseCsp(base, 'a'), parseCsp(`${base}; img-src *`, 'b')).length === 1
      && refusedParse(`${base}; default-src *`) && refusedParse("script-src 'self' 'self'") && refusedParse(''),
    'the CSP comparison ignores order and refuses a dropped source, an added directive, a repeat',
    'the CSP comparison does not refuse a drifted policy');
}

// The string-literal list `name` joins with "; " in an Ipê source: the policy
// as the run server builds it. Any other shape is refused, so a policy the
// server computes some other way cannot slip past as "not found".
function ipeJoinedLiterals(file, name) {
  const src = fs.readFileSync(file, 'utf8');
  const m = new RegExp(`^${name} =\\s*\\n\\s*String\\.join "; "\\s*\\[([^\\]]*)\\]`, 'm').exec(src);
  const literal = /"((?:[^"\\]|\\.)*)"/g;
  if (!m || m[1].replace(literal, '').replace(/[\s,]/g, '') !== '') {
    throw new Error(`${file}: ${name} is no longer a String.join "; " of string literals`);
  }
  return [...m[1].matchAll(literal)].map((x) => x[1]).join('; ');
}

// The X-Frame-Options value `page` sets in the run server's `Main.ipe`.
function ipeFrameOptions(file) {
  const found = [...fs.readFileSync(file, 'utf8').matchAll(/Server\.withHeader "X-Frame-Options" "([^"]*)"/g)];
  if (found.length !== 1) throw new Error(`${file}: expected one X-Frame-Options header, found ${found.length}`);
  return found[0][1];
}

// The run server's page policy, as its source defines it, is the page's
// <meta> policy plus frame-ancestors, and its X-Frame-Options is DENY.
function serverPolicyChecks(meta) {
  const gateFile = path.join(dir, 'server', 'src', 'Gate.ipe');
  const gate = ipeJoinedLiterals(gateFile, 'pageContentSecurityPolicy');
  const diff = cspDiff(servedPolicy(meta), parseCsp(gate, 'Gate.pageContentSecurityPolicy'));
  check(diff.length === 0, 'Gate.pageContentSecurityPolicy is the page <meta> policy plus frame-ancestors',
    `Gate.pageContentSecurityPolicy drifted from the page <meta> policy: ${diff.join('; ')}`);
  const xfo = ipeFrameOptions(path.join(dir, 'server', 'src', 'Main.ipe'));
  check(xfo === 'DENY', 'the run server sends X-Frame-Options: DENY', `the run server sends X-Frame-Options: ${xfo}`);
  return { csp: { 'Content-Security-Policy': gate }, xfo: { 'X-Frame-Options': xfo } };
}

// Frame each target from another loopback origin, beside a control frame of
// the page's origin, and prove each target was refused BY ITS HEADER: the
// control renders (so the framer can frame this origin at all), the target's
// response arrived with status 200 (so nothing failed before the header was
// read), Chromium blocked it (`net::ERR_BLOCKED_BY_RESPONSE`, an error page in
// place of the document) and logged the refusal naming the header, and no
// part of the page rendered. The framer is a real loopback server: a routed
// page counts as a public origin, which Chromium's local-network checks bar
// from framing loopback at all, the control included, which would make every
// refusal vacuous.
async function crossOriginFrameChecks(browser, controlUrl, targets) {
  const body = [controlUrl, ...targets.map((t) => t.url)]
    .map((src, i) => `<iframe id="f${i}" src="${src}" width="800" height="600"></iframe>`).join('');
  const framer = createServer((_req, res) => {
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(`<!DOCTYPE html>${body}`);
  });
  await new Promise((r) => framer.listen(0, '127.0.0.1', r));
  const context = await browser.newContext();
  const statuses = new Map();
  const failures = new Map();
  const logged = [];
  const bare = (u) => u.split('#')[0];
  context.on('response', (resp) => statuses.set(bare(resp.url()), resp.status()));
  context.on('requestfailed', (req) => failures.set(bare(req.url()), req.failure()?.errorText ?? ''));
  try {
    const page = await context.newPage();
    page.on('console', (m) => logged.push(m.text()));
    await page.goto(`http://127.0.0.1:${framer.address().port}/`, { waitUntil: 'load' });
    const frameOf = async (i) => (await page.$(`#f${i}`))?.contentFrame();
    const settled = async () => {
      const control = await frameOf(0);
      const shown = control ? await control.evaluate(() => (document.body?.textContent ?? '').length > 0).catch(() => false) : false;
      return shown && targets.every((t) => failures.has(bare(t.url)));
    };
    for (let deadline = Date.now() + 15000; !(await settled()) && Date.now() < deadline;) await page.waitForTimeout(100);
    const control = await frameOf(0);
    const controlShown = control
      ? control.url() === controlUrl && await control.evaluate(() => (document.body?.textContent ?? '').length > 0).catch(() => false)
      : false;
    check(controlShown && statuses.get(controlUrl) === 200, `control: a cross-origin frame of ${controlUrl} renders`,
      `control: a cross-origin frame of ${controlUrl} did not render (status ${statuses.get(controlUrl)}), so no refusal below is proven`);
    for (const [i, t] of targets.entries()) {
      const frame = await frameOf(i + 1);
      const rendered = frame ? await frame.evaluate(() => Boolean(document.getElementById('run-btn'))).catch(() => false) : false;
      const evidence = {
        status: statuses.get(bare(t.url)),
        failure: failures.get(bare(t.url)),
        frameUrl: frame?.url(),
        logged: logged.some((m) => t.refusedBy.test(m)),
        rendered,
      };
      check(evidence.status === 200 && evidence.failure === 'net::ERR_BLOCKED_BY_RESPONSE'
          && evidence.frameUrl?.startsWith('chrome-error://') && evidence.logged && !rendered,
        `${t.label}: a cross-origin frame is refused the page (delivered 200, blocked by the header, refusal logged)`,
        `${t.label}: cross-origin frame refusal not proven: ${JSON.stringify(evidence)}`);
    }
  } finally {
    await context.close();
    framer.close();
  }
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
  const headerDiff = cspDiff(servedPolicy(meta), parseCsp(csp, 'the GET / CSP header'));
  check(headerDiff.length === 0, 'GET / CSP is the page <meta> policy plus frame-ancestors',
    `GET / CSP drifted from the page <meta> policy: ${headerDiff.join('; ')}`);
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
  await crossOriginFrameChecks(browser, `${origin}/static/app.js`,
    [{ label: 'the served page', url: `${origin}/#t=${token}`, refusedBy: REFUSED_BY_EITHER }]);
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
  } else {
    vendorChecks();
    cspSelfChecks();
  }
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
  else {
    await staticChecks(page);
    framingHeaders = serverPolicyChecks(meta);
    await crossOriginFrameChecks(browser, `${target}static/app.js`, [
      { label: "Gate's CSP alone", url: `${target}?frame=csp`, refusedBy: REFUSED_BY_CSP },
      { label: 'X-Frame-Options alone', url: `${target}?frame=xfo`, refusedBy: REFUSED_BY_XFO },
    ]);
  }
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
