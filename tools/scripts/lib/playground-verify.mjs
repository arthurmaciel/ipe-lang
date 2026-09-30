#!/usr/bin/env node
// Headless verification of the in-browser Ipê playground in Chromium.
//
// Static mode (the GitHub Pages shape): serves the playground directory under
// a sub-path with no run server behind it, then checks the WASM compiler boots
// and emits Rust for the sample, Run is disabled with the local-server notice
// (never a raw fetch error), the ACE theme re-themes the UI, a type error
// surfaces as a diagnostic, and the GitHub link target.
//
// Live mode (`--live <url>`): opens the page the playground server serves at
// <url>, presses Run, and waits for the sample's jailed program output.
//
// Usage: node playground-verify.mjs <playground-dir> [port]
//        node playground-verify.mjs --live http://localhost:8000/
// Exit 0 on pass, non-zero on any failure.

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const SUB_PATH = '/compiler/playground/';
const NO_SERVER = 'Run needs the local server — see README';
const SAMPLE_OUTPUT = 'double 21 = 42';

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
const text = (page, id) => page.evaluate((i) => document.getElementById(i)?.textContent ?? '', id);

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
  // A theme chunk loads lazily from the CDN: wait for `--bg` to move off its
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
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(live || `http://localhost:${port}${SUB_PATH}`, { waitUntil: 'load' });
  await bootChecks(page);
  if (live) await liveChecks(page);
  else await staticChecks(page);
  if (errors.length) fail('page errors: ' + errors.join('; '));
} catch (e) {
  fail(String(e));
} finally {
  await browser.close();
  if (server) server.close();
}
process.exit(ok ? 0 : 1);
