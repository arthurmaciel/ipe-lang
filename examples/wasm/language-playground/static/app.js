// The playground page's script. It runs in the origin that holds the launch
// token, so it and every script it loads are same-origin files from this
// directory; the page's Content-Security-Policy admits no other script source.
import init, { compile } from '../pkg/ipe_wasm.js';

// ACE loads mode and theme chunks lazily as `<script>`s under `basePath`: the
// vendored copy next to this file (see `vendor/ace/README.md`).
ace.config.set('basePath', new URL('vendor/ace/', import.meta.url).href);

const SAMPLE = `module Main exposing (main)

import Ipe.Error exposing (Error)
import Ipe.Io as Io
import Ipe.List as List
import Ipe.String as String
import Ipe.Task exposing (Task)


-- Edit this program: it compiles in your browser as you type.
-- The Ipê frontend (parse -> typecheck -> lower -> emit) runs as WebAssembly
-- and shows the Rust it emits. Press Run to send that emitted Rust to the
-- playground server, which builds and runs it (sandboxed) and shows the output.
double : Int -> Int
double n =
    n * 2


main : Task Error ()
main =
    do
        Io.println ("double 21 = " ++ String.fromInt (double 21))
        [ 1, 2, 3 ] |> List.map (double >> String.fromInt) |> String.join ", " |> Io.println
`;

const statusBar = document.getElementById('status-bar');
const output = document.getElementById('output');
const outputTitle = document.getElementById('output-title');
const runOutput = document.getElementById('run-output');
const runTitle = document.getElementById('run-title');
const themeSelect = document.getElementById('theme-select');
const runBtn = document.getElementById('run-btn');

// ---- ACE editor ----
const editor = ace.edit('editor');
// Ipê is close to Elm/Haskell syntactically; Haskell highlighting is the
// best-fit bundled mode until a dedicated Ipê mode is authored.
editor.session.setMode('ace/mode/haskell');
editor.setOptions({
  fontFamily: '"JetBrains Mono", "Fira Code", ui-monospace, monospace',
  fontSize: '14px',
  showPrintMargin: false,
  tabSize: 4,
  useSoftTabs: true,
});
editor.setValue(SAMPLE, -1);

// ---- Theme switcher: re-themes editor AND the surrounding UI ----
const themelist = ace.require('ace/ext/themelist');
for (const theme of themelist.themes) {
  const opt = document.createElement('option');
  opt.value = theme.theme;       // e.g. "ace/theme/monokai"
  opt.textContent = theme.caption;
  themeSelect.appendChild(opt);
}

// Parse an "rgb(r, g, b)" / "rgba(...)" string into [r,g,b].
function parseRgb(str) {
  const m = str && str.match(/(\d+(?:\.\d+)?)/g);
  if (!m || m.length < 3) return null;
  return [Number(m[0]), Number(m[1]), Number(m[2])];
}
function luminance([r, g, b]) {
  return (0.299 * r + 0.587 * g + 0.114 * b) / 255;
}
function mix([r, g, b], [r2, g2, b2], t) {
  const c = (a, d) => Math.round(a + (d - a) * t);
  return `rgb(${c(r, r2)}, ${c(g, g2)}, ${c(b, b2)})`;
}
function rgbStr([r, g, b]) { return `rgb(${r}, ${g}, ${b})`; }

// Derive UI CSS variables from the ACE theme's editor colours so the whole
// interface (header, panels, status bar, output) matches the editor.
function applyUiFromEditor() {
  const editorEl = editor.container.querySelector('.ace_editor')
    || editor.container;
  const gutterEl = editor.container.querySelector('.ace_gutter');
  const cs = getComputedStyle(editorEl);
  const bg = parseRgb(cs.backgroundColor);
  const fg = parseRgb(cs.color);
  if (!bg || !fg) return;

  const dark = luminance(bg) < 0.5;
  const towards = dark ? [255, 255, 255] : [0, 0, 0];
  const gcs = gutterEl ? getComputedStyle(gutterEl) : null;
  const panel = gcs ? parseRgb(gcs.backgroundColor) : null;

  const root = document.documentElement.style;
  root.setProperty('--bg', rgbStr(bg));
  root.setProperty('--fg', rgbStr(fg));
  root.setProperty('--panel', panel ? rgbStr(panel) : mix(bg, towards, 0.06));
  root.setProperty('--border', mix(bg, towards, 0.16));
  root.setProperty('--muted', mix(fg, bg, 0.35));
  // Keep a readable accent regardless of theme brightness.
  root.setProperty('--accent', dark ? '#3b82f6' : '#2563eb');
  root.setProperty('--accent-fg', '#ffffff');
}

// ACE loads a theme chunk lazily and calls back once its style is applied;
// the UI colours are read from the editor only then. The frame wait lets
// the injected style reach the computed style.
function setTheme(theme) {
  editor.setTheme(theme, () => requestAnimationFrame(applyUiFromEditor));
}
themeSelect.addEventListener('change', () => setTheme(themeSelect.value));

const DEFAULT_THEME = 'ace/theme/tomorrow_night';
themeSelect.value = DEFAULT_THEME;
setTheme(DEFAULT_THEME);

// ---- Live, debounced compile ----
let wasmReady = false;
let timer = null;
// Run is enabled only while the last compile succeeded: the server runs the
// emitted Rust, so the button mirrors `compile()`'s ok state.
let compileOk = false;
let lastEmittedRust = '';
// Whether a run server answered GET /health: 'probing' | 'up' | 'down', or
// 'notoken' when it did but the page holds no launch token.
// Run is enabled only when the compile succeeded AND the server is up.
let serverState = 'probing';
const NO_SERVER = 'Run needs the local server — see README';

function syncRunButton() {
  runBtn.disabled = running || !compileOk || serverState !== 'up';
  runBtn.title = serverState === 'down'
    ? NO_SERVER
    : serverState === 'notoken'
      ? NO_TOKEN
      : 'Build and run on the server, sandboxed (Ctrl/Cmd+Enter)';
}

function compiledStatus() {
  if (serverState === 'up') return 'Compiled successfully — emitted Rust shown; Run is enabled.';
  if (serverState === 'down') return 'Compiled successfully — emitted Rust shown. ' + NO_SERVER + '.';
  if (serverState === 'notoken') return 'Compiled successfully — emitted Rust shown. ' + NO_TOKEN + '.';
  return 'Compiled successfully — emitted Rust shown; looking for the run server…';
}

function runCompile() {
  if (!wasmReady) return;
  statusBar.className = 'compiling';
  statusBar.textContent = 'Compiling…';
  // Yield so the status paints before the (synchronous) compile runs.
  requestAnimationFrame(() => {
    let res;
    try {
      res = compile(editor.getValue());
    } catch (e) {
      statusBar.className = 'error';
      statusBar.textContent = 'Compiler error: ' + e;
      return;
    }
    if (res.ok) {
      compileOk = true;
      lastEmittedRust = res.emitted_rust;
      output.className = '';
      outputTitle.textContent = 'Emitted Rust';
      output.textContent = res.emitted_rust;
      syncRunButton();
      statusBar.className = 'ok';
      statusBar.textContent = compiledStatus();
    } else {
      compileOk = false;
      output.className = 'err';
      outputTitle.textContent = 'Diagnostics';
      output.textContent = res.diagnostics || '(no diagnostics)';
      syncRunButton();
      statusBar.className = 'error';
      statusBar.textContent = 'Compile error — see diagnostics.';
    }
  });
}

function scheduleCompile() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(runCompile, 300);
}
editor.session.on('change', scheduleCompile);

// ---- Run button: server build + run of the EMITTED RUST ----
// The client-side WASM `compile()` shows the emitted Rust live as you type.
// Turning that Rust into a running binary needs cargo, which cannot run in
// the browser — so Run POSTs the emitted Rust (not the Ipê source) to the
// playground server, which builds AND executes it inside a hardened sandbox
// (no network, jailed filesystem, memory/CPU/fork/time caps) and returns a
// {ok, unsandboxed, output} transcript.
//
// The run server is separate from a static host (e.g. GitHub Pages). The
// page talks only to the directory it was served from: the server admits
// /run from its own origin alone, with this launch's token. With no server
// answering /health, Run stays disabled and the page is a pure in-browser
// emit preview.
const RUN_BASE = new URL('.', location.href);
const endpoint = (name) => new URL(name, RUN_BASE.href).href;
const HEALTH_URL = endpoint('health');
const RUN_URL = endpoint('run');

let running = false;
// The launch token POST /run carries. The server never serves it: it
// prints the launch URL `.../#t=<token>` to its operator's terminal, and
// the token arrives only in that fragment. It is taken once, removed from
// the address bar and the history entry, and kept in memory only. A framed
// page never takes one, so a framing site cannot drive Run.
const TOKEN_SHAPE = /^[A-Za-z0-9_-]{43}$/;
const NO_TOKEN = 'Run needs the launch URL the server printed (…/#t=…); open it to enable Run';
const runToken = (() => {
  const match = /^#t=([A-Za-z0-9_-]*)$/.exec(location.hash);
  if (location.hash) history.replaceState(null, '', location.pathname + location.search);
  if (window.top !== window.self) return null;
  return match && TOKEN_SHAPE.test(match[1]) ? match[1] : null;
})();

function markServerDown() {
  serverState = 'down';
  runTitle.textContent = 'Program output';
  runOutput.className = '';
  runOutput.innerHTML = '';
  const link = document.createElement('a');
  link.href = 'https://github.com/ipe-lang/compiler/tree/main/examples/wasm/language-playground#readme';
  link.target = '_blank';
  link.rel = 'noopener';
  link.textContent = 'README';
  runOutput.append('Run needs the local server — see ', link,
    '. The emitted Rust preview above works without it.');
  syncRunButton();
}

function markNoToken() {
  serverState = 'notoken';
  runTitle.textContent = 'Program output';
  runOutput.className = '';
  runOutput.textContent = NO_TOKEN + '. The emitted Rust preview above works without it.';
  syncRunButton();
}

async function probeServer() {
  try {
    const resp = await fetch(HEALTH_URL, { cache: 'no-store', signal: AbortSignal.timeout(5000) });
    const data = resp.ok ? await resp.json() : null;
    serverState = Boolean(data) && data.ok === true ? 'up' : 'down';
  } catch {
    serverState = 'down';
  }
  if (serverState === 'up' && runToken === null) markNoToken();
  if (serverState === 'down') markServerDown();
  syncRunButton();
  if (compileOk) statusBar.textContent = compiledStatus();
}

function showRunResult(data) {
  // Wire shape: { ok, unsandboxed, output } — the only states the server
  // emits (build failure = ok:false with a Build transcript; success =
  // ok:true with the Run transcript; unsandboxed only when the operator
  // explicitly opted out with IPE_ALLOW_UNSANDBOXED).
  if (data.ok) {
    runOutput.className = data.unsandboxed ? 'warn' : '';
    runTitle.textContent = 'Program output';
    runOutput.textContent = data.output || '(no output)';
    if (data.unsandboxed) {
      statusBar.className = 'error';
      statusBar.textContent = 'Ran WITHOUT the sandbox — the server operator opted out of isolation.';
    } else {
      statusBar.className = 'ok';
      statusBar.textContent = 'Ran successfully on the server (sandboxed).';
    }
  } else {
    runOutput.className = 'err';
    runTitle.textContent = 'Run failed';
    runOutput.textContent = data.output || '(no output from the server)';
    statusBar.className = 'error';
    statusBar.textContent = 'The server could not run the program.';
  }
}

async function runNow() {
  if (running || !compileOk || serverState !== 'up') return;
  if (timer) { clearTimeout(timer); timer = null; }
  running = true;
  syncRunButton();
  statusBar.className = 'compiling';
  statusBar.textContent = 'Building and running (sandboxed)…';
  try {
    let resp;
    try {
      resp = await fetch(RUN_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Ipe-Playground-Token': runToken },
        body: JSON.stringify({ rust: lastEmittedRust }),
      });
    } catch {
      // The server went away after the health probe.
      markServerDown();
      statusBar.className = 'error';
      statusBar.textContent = NO_SERVER + '.';
      return;
    }
    // Every /run answer, refusals (413/400/500) included, carries the same
    // {ok, unsandboxed, output} JSON body.
    let data = null;
    try {
      data = await resp.json();
    } catch {
      data = null;
    }
    if (data && typeof data.ok === 'boolean') {
      showRunResult(data);
    } else {
      showRunResult({ ok: false, unsandboxed: false,
        output: 'The run server answered HTTP ' + resp.status + ' without a transcript.' });
    }
  } finally {
    running = false;
    syncRunButton();
  }
}
runBtn.addEventListener('click', runNow);
// Ctrl/Cmd+Enter runs from the editor, matching the original playground.
editor.commands.addCommand({
  name: 'ipeRun',
  bindKey: { win: 'Ctrl-Enter', mac: 'Cmd-Enter' },
  exec: runNow,
});

// ---- Boot ----
syncRunButton();
probeServer();
(async () => {
  try {
    await init();
    wasmReady = true;
    statusBar.className = 'ok';
    statusBar.textContent =
      'Ready — emitted Rust shown live; press Run to build & run it (sandboxed) on the server.';
    runCompile(); // Run needs a clean compile and a live server
  } catch (e) {
    statusBar.className = 'error';
    statusBar.textContent = 'Failed to load the WASM compiler: ' + e;
    output.className = 'err';
    output.textContent = String(e);
  }
})();
