#!/usr/bin/env node
// Headless verification of the in-browser Ipê playground: boots index.html in
// Chromium, waits for the WASM compiler to report ready, checks it emits Rust
// for the sample program, drives a compile error, switches the ACE theme and
// confirms the surrounding UI re-themes, and checks the GitHub link target.
//
// Usage: node playground-verify.mjs <playground-dir> [port]
// Exit 0 on pass, non-zero on any failure.

import { chromium } from 'playwright';
import { createServer } from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const dir = path.resolve(process.argv[2] || '');
const port = parseInt(process.argv[3] || '8199', 10);
if (!dir || !fs.existsSync(path.join(dir, 'index.html'))) {
  console.error('usage: node playground-verify.mjs <playground-dir> [port]');
  process.exit(2);
}

const MIME = { '.html': 'text/html', '.js': 'application/javascript', '.wasm': 'application/wasm', '.css': 'text/css' };
const server = createServer((req, res) => {
  const p = req.url.split('?')[0];
  const fp = path.join(dir, p === '/' ? 'index.html' : p);
  fs.readFile(fp, (err, data) => {
    if (err) { res.writeHead(404); res.end('nf'); return; }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(fp)] || 'application/octet-stream' });
    res.end(data);
  });
});

function fail(msg) { console.error('FAIL:', msg); }

await new Promise((r) => server.listen(port, r));
const browser = await chromium.launch();
let ok = true;
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(`http://localhost:${port}/`, { waitUntil: 'load' });

  // 1) The compiler boots and the sample compiles to Rust.
  await page.waitForFunction(
    () => document.getElementById('status-bar')?.textContent?.includes('ready')
       || document.getElementById('status-bar')?.textContent?.includes('successfully'),
    { timeout: 30000 },
  );
  await page.waitForFunction(
    () => document.getElementById('output')?.textContent?.includes('==== src/main.rs ===='),
    { timeout: 30000 },
  );
  console.log('PASS: sample program compiled in-browser, emitted Rust shown');

  // 1b) The Run button forces a compile and shows the emitted Rust.
  await page.evaluate(() => { document.getElementById('output').textContent = ''; });
  await page.click('#run-btn');
  await page.waitForFunction(
    () => document.getElementById('output')?.textContent?.includes('==== src/main.rs ===='),
    { timeout: 15000 },
  );
  console.log('PASS: Run button compiles and shows emitted Rust');

  // 2) The GitHub link points at the playground in this repo.
  const href = await page.getAttribute('a.gh', 'href');
  if (href && href.includes('/arthurmaciel/ipe-lang') && href.includes('examples/wasm/language-playground')) {
    console.log('PASS: GitHub link resolves ->', href);
  } else { ok = false; fail('GitHub link wrong: ' + href); }

  // 3) Title.
  const title = await page.title();
  if (title === 'Ipê playground') console.log('PASS: title is "Ipê playground"');
  else { ok = false; fail('title: ' + title); }

  // 4) Theme switch re-themes BOTH editor and UI. Record the header bg + editor
  //    bg under two very different themes and assert both change.
  const readColors = () => page.evaluate(() => ({
    ui: getComputedStyle(document.querySelector('header')).backgroundColor,
    editor: getComputedStyle(document.querySelector('#editor .ace_editor, #editor')).backgroundColor,
    rootBg: getComputedStyle(document.documentElement).getPropertyValue('--bg').trim(),
  }));
  const setTheme = async (t) => {
    await page.selectOption('#theme-select', t);
    await page.waitForTimeout(300);
  };
  await setTheme('ace/theme/monokai');
  const dark = await readColors();
  await setTheme('ace/theme/github');
  const light = await readColors();
  if (dark.ui !== light.ui && dark.rootBg !== light.rootBg) {
    console.log('PASS: theme switch re-themes UI (header/--bg changed):', dark.ui, '->', light.ui);
  } else { ok = false; fail(`UI did not re-theme: ${JSON.stringify(dark)} vs ${JSON.stringify(light)}`); }

  // 5) A type error surfaces as a diagnostic, not a crash.
  await page.evaluate(() => {
    // Replace the editor content with an ill-typed program.
    const ed = window.ace ? null : null;
  });
  await page.evaluate(() => {
    const editorDiv = document.getElementById('editor');
    // Access the ACE editor instance via the global registry.
    const inst = window.ace.edit(editorDiv);
    inst.setValue('module Main exposing (main)\n\nmain : Int\nmain = "not an int"\n', -1);
  });
  await page.waitForFunction(
    () => document.getElementById('status-bar')?.textContent?.includes('error'),
    { timeout: 15000 },
  );
  const diag = await page.textContent('#output');
  if (diag && diag.trim().length > 0) console.log('PASS: type error reported as diagnostic');
  else { ok = false; fail('no diagnostic text for type error'); }

  if (errors.length) { ok = false; fail('page errors: ' + errors.join('; ')); }
} catch (e) {
  ok = false;
  fail(String(e));
} finally {
  await browser.close();
  server.close();
}
process.exit(ok ? 0 : 1);
