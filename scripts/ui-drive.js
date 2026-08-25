#!/usr/bin/env node
//
// ui-drive.js — drive the share-tracker web UI in headless Chrome over CDP.
//
// scripts/ui-check.sh renders a route and prints the DOM; it cannot *interact*.
// This does: real clicks and hovers, typing into form fields, native
// confirm()/alert() dialogs, print-media emulation and PDF, plus capture of
// every console message and uncaught exception the page produced. It exists for
// the same reason ui-check.sh does — to automate a manual spot-check — and is
// likewise NOT a browser test harness: nothing here runs in CI, and the
// automated UI tests stay bundle-assertion based (see CLAUDE.md).
//
// Zero dependencies: Node 22+ has a global WebSocket and fetch, which is the
// whole of a CDP client. Chrome is located the way ui-check.sh locates it.
//
// Usage:
//   node scripts/ui-drive.js --url http://127.0.0.1:3971 steps.js [arg...]
//
// steps.js default-exports `async (page, args) => {}`. The process exits 0 when
// it returns, 1 if it throws. Anything the scenario prints goes to stdout; the
// driver's own diagnostics go to stderr.
//
//   // steps.js
//   export default async function (page) {
//     await page.route('#/e/trades');
//     await page.click('button.new');
//     await page.fill('[name=quantity]', '10');
//     await page.click('button[type=submit]');
//     console.log(await page.text('#toast'));
//   }
//
// Options:
//   --url URL        server to drive (required; start it yourself)
//   --headed         run a visible browser instead of --headless=new
//   --keep           leave the browser open after the script returns
//   --timeout MS     default wait ceiling (default 10000)
//
// Env: CHROME (browser binary), CHROME_FLAGS (extra whitespace-separated flags).

import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

// --- locating Chrome ---------------------------------------------------------

const CHROME_CANDIDATES = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
];

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  for (const c of CHROME_CANDIDATES) if (existsSync(c)) return c;
  throw new Error('Chrome not found — set CHROME=/path/to/chrome');
}

// --- the CDP connection ------------------------------------------------------

// A minimal request/response + event client over one WebSocket. CDP multiplexes
// on a monotonic `id`; every reply carries the id it answers, so a Map of
// pending promises is the whole protocol.
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.handlers = new Map();
    ws.addEventListener('message', (ev) => this.#onMessage(JSON.parse(ev.data)));
  }

  static async attach(wsUrl) {
    const ws = new WebSocket(wsUrl);
    await new Promise((ok, fail) => {
      ws.addEventListener('open', ok, { once: true });
      ws.addEventListener('error', () => fail(new Error('CDP websocket failed')), { once: true });
    });
    return new Cdp(ws);
  }

  #onMessage(msg) {
    if (msg.id !== undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.fail(new Error(`${msg.error.message}${msg.error.data ? ': ' + msg.error.data : ''}`));
      else p.ok(msg.result);
      return;
    }
    for (const h of this.handlers.get(msg.method) || []) h(msg.params);
  }

  on(method, handler) {
    if (!this.handlers.has(method)) this.handlers.set(method, []);
    this.handlers.get(method).push(handler);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((ok, fail) => {
      this.pending.set(id, { ok, fail });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    try { this.ws.close(); } catch { /* already gone */ }
  }
}

// --- the page API the scenario script drives ---------------------------------

// Every wait in here polls a JS expression rather than a CDP event, because the
// thing worth waiting for in this SPA is always a DOM fact ("the table has
// rows", "the toast is visible") and never a navigation. `page.settle()` is the
// exception: it waits on the in-flight fetch counter installed below, so a
// scenario does not have to guess how long an API round-trip takes.
const INFLIGHT_SHIM = `
  window.__inflight = 0;
  window.__uiDriveErrors = [];
  addEventListener('error', (e) => window.__uiDriveErrors.push(String(e.message)));
  addEventListener('unhandledrejection', (e) => window.__uiDriveErrors.push('unhandled rejection: ' + e.reason));
  const __fetch = window.fetch;
  window.fetch = function (...a) {
    window.__inflight++;
    return __fetch.apply(this, a).finally(() => { window.__inflight--; });
  };
`;

// A JavaScript *source literal* for `value`, for the expression strings below.
// JSON.stringify is not one on its own: it escapes for JSON, and leaves `<`,
// `>`, U+2028 and U+2029 as themselves — so a selector or field value carrying
// `</script>` ends the script it is interpolated into, and a line separator can
// change where the surrounding statement ends. Escaping those four as `\uXXXX`
// leaves the string's value identical while making the characters inert.
// (CodeQL's js/bad-code-sanitization is the check that names this.)
const JS_UNSAFE = { '<': '\\u003C', '>': '\\u003E', '\u2028': '\\u2028', '\u2029': '\\u2029' };

function jsLiteral(value) {
  const json = JSON.stringify(value);
  // `undefined` has no JSON form; keep the interpolation it used to produce.
  return json === undefined ? 'undefined' : json.replace(/[<>\u2028\u2029]/g, (c) => JS_UNSAFE[c]);
}

class Page {
  constructor(cdp, base, defaultTimeout) {
    this.cdp = cdp;
    this.base = base.replace(/\/$/, '');
    this.timeout = defaultTimeout;
    /** Console messages and page errors, newest last: `{kind, text}`. */
    this.logs = [];
    /** Native dialogs seen, in order: `{type, message, accepted}`. */
    this.dialogs = [];
    /** How the next native dialog is answered: 'accept' | 'dismiss'. */
    this.dialogAnswer = 'accept';
  }

  static async open(cdp, base, timeout) {
    const page = new Page(cdp, base, timeout);
    cdp.on('Runtime.consoleAPICalled', (p) => {
      const text = (p.args || []).map((a) => a.value ?? a.description ?? a.type).join(' ');
      page.logs.push({ kind: p.type, text });
    });
    cdp.on('Runtime.exceptionThrown', (p) => {
      const d = p.exceptionDetails;
      page.logs.push({ kind: 'exception', text: d.exception?.description || d.text });
    });
    // Native confirm()/alert() block the renderer until answered, so this must
    // be handled or the very first delete button hangs the whole run.
    cdp.on('Page.javascriptDialogOpening', async (p) => {
      const accept = page.dialogAnswer === 'accept';
      page.dialogs.push({ type: p.type, message: p.message, accepted: accept });
      await cdp.send('Page.handleJavaScriptDialog', { accept });
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('DOM.enable');
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: INFLIGHT_SHIM });
    return page;
  }

  // --- evaluation ------------------------------------------------------------

  /**
   * Evaluate in the page: either an expression, or statements ending in a
   * `return`. The wrapper is async so a scenario can `await` a dynamic import
   * of the app's own modules (which is how a scenario reads `config.js`
   * instead of re-stating what it configures).
   */
  async eval(expression) {
    const r = await this.cdp.send('Runtime.evaluate', {
      expression: `(async function () { ${expression.includes('return') ? expression : 'return (' + expression + ')'} })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (r.exceptionDetails) {
      const d = r.exceptionDetails;
      throw new Error('page eval failed: ' + (d.exception?.description || d.text));
    }
    return r.result.value;
  }

  /** Poll `expression` until truthy. Returns its value; throws on timeout. */
  async waitFor(expression, opts = {}) {
    const ceiling = Date.now() + (opts.timeout ?? this.timeout);
    let last;
    for (;;) {
      try {
        last = await this.eval(expression);
        if (last) return last;
      } catch (e) {
        last = e.message;
      }
      if (Date.now() > ceiling) {
        throw new Error(`timed out waiting for: ${opts.msg || expression}${last ? ` (last: ${JSON.stringify(last).slice(0, 200)})` : ''}`);
      }
      await sleep(50);
    }
  }

  /** Wait until no fetch the page started is still in flight, then a frame. */
  async settle(quietMs = 120) {
    await this.waitFor('window.__inflight === 0');
    await sleep(quietMs);
    // A second look: the first response often starts the next request.
    await this.waitFor('window.__inflight === 0');
  }

  // --- navigation ------------------------------------------------------------

  /** Load `base + path` fresh (full document load). */
  async goto(path = '/') {
    const url = this.base + (path.startsWith('/') || path.startsWith('#') ? path : '/' + path);
    const loaded = this.#once('Page.loadEventFired');
    await this.cdp.send('Page.navigate', { url });
    await loaded;
    await this.waitFor('typeof window.__inflight === "number"');
    await this.settle();
  }

  /**
   * Go to a hash route. Within a loaded document this is a hashchange, not a
   * navigation — assigning location.hash and waiting on the app mount is what
   * the user's own click does, and Page.navigate would not fire a load event
   * for it anyway.
   */
  async route(hash) {
    if (!hash.startsWith('#')) hash = '#' + hash;
    const current = await this.eval('location.href').catch(() => '');
    if (!current.startsWith(this.base)) return this.goto('/' + hash);
    await this.eval(`location.hash = ${jsLiteral(hash)}`);
    await this.settle();
  }

  // --- reading ---------------------------------------------------------------

  /** Does the selector match anything? */
  exists(sel) {
    return this.eval(`!!document.querySelector(${jsLiteral(sel)})`);
  }

  /** innerText of the first match (null when absent). */
  text(sel) {
    return this.eval(`(document.querySelector(${jsLiteral(sel)}) || {}).innerText ?? null`);
  }

  /** innerText of every match. */
  texts(sel) {
    return this.eval(`Array.from(document.querySelectorAll(${jsLiteral(sel)}), (e) => e.innerText)`);
  }

  /** outerHTML of the first match (the whole document when sel is omitted). */
  html(sel) {
    if (!sel) return this.eval('document.documentElement.outerHTML');
    return this.eval(`(document.querySelector(${jsLiteral(sel)}) || {}).outerHTML ?? null`);
  }

  /**
   * The visible toast text, or null when no toast is showing. Toasts stack
   * (error ones persist until dismissed), so this is every showing message
   * joined by newlines — and the message text only, without the close glyph.
   */
  async toast() {
    return this.eval(`
      const t = document.getElementById('toast');
      if (!t || t.hidden) return null;
      return Array.from(t.querySelectorAll('.toast-msg'), (m) => m.innerText).join('\\n') || null;
    `);
  }

  /** Wait for a toast to appear and return its text. */
  async waitForToast(opts = {}) {
    return this.waitFor(`
      const t = document.getElementById('toast');
      if (!t || t.hidden) return null;
      return Array.from(t.querySelectorAll('.toast-msg'), (m) => m.innerText).join('\\n') || null;
    `, { msg: 'a toast to appear', ...opts });
  }

  /** Dismiss every showing toast (an error one never goes away on its own). */
  async dismissToasts() {
    return this.eval(`
      const t = document.getElementById('toast');
      if (!t) return 0;
      const items = Array.from(t.querySelectorAll('.toast-item'));
      items.forEach((i) => i.dispatchEvent(new MouseEvent('click', { bubbles: true })));
      return items.length;
    `);
  }

  /** Uncaught errors the page recorded, including ones thrown before attach. */
  pageErrors() {
    return this.eval('window.__uiDriveErrors || []');
  }

  // --- interaction -----------------------------------------------------------

  /** Viewport centre of an element, scrolling it into view first. */
  async #centre(sel) {
    const box = await this.eval(`
      const e = document.querySelector(${jsLiteral(sel)});
      if (!e) return null;
      e.scrollIntoView({ block: 'center', inline: 'center' });
      const r = e.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
    `);
    if (!box) throw new Error(`no element matches ${sel}`);
    if (box.w === 0 || box.h === 0) throw new Error(`${sel} has no layout box (hidden?)`);
    return box;
  }

  /** A real mouse click at the element's centre. */
  async click(sel, opts = {}) {
    const { x, y } = await this.#centre(sel);
    const common = { x, y, button: 'left', clickCount: 1, buttons: 1 };
    await this.cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, buttons: 0 });
    await this.cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...common });
    await this.cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...common });
    if (opts.settle !== false) await this.settle();
  }

  /** Move the pointer over an element (what a CSS :hover menu needs). */
  async hover(sel) {
    const { x, y } = await this.#centre(sel);
    await this.cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, buttons: 0 });
    await sleep(80);
  }

  /**
   * Set a form field's value and fire `input` + `change`, which is what the
   * app's own listeners react to. Use `type()` when the keystrokes themselves
   * matter (a keydown handler, an input mask).
   */
  async fill(sel, value) {
    await this.eval(`
      const e = document.querySelector(${jsLiteral(sel)});
      if (!e) throw new Error('no element matches ' + ${jsLiteral(sel)});
      const proto = e instanceof HTMLSelectElement ? HTMLSelectElement.prototype
        : e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, ${jsLiteral(String(value))});
      e.dispatchEvent(new Event('input', { bubbles: true }));
      e.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    `);
    await this.settle();
  }

  /** Focus an element and type into it as a keyboard would. */
  async type(sel, value) {
    await this.click(sel, { settle: false });
    await this.cdp.send('Input.insertText', { text: String(value) });
    await this.eval(`
      const e = document.querySelector(${jsLiteral(sel)});
      e.dispatchEvent(new Event('input', { bubbles: true }));
      e.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    `);
    await this.settle();
  }

  /** Press a named key (Enter, Escape, Tab, …) at the current focus. */
  async press(key) {
    const codes = { Enter: 13, Escape: 27, Tab: 9, ArrowDown: 40, ArrowUp: 38 };
    const params = { key, code: key, windowsVirtualKeyCode: codes[key] ?? 0, nativeVirtualKeyCode: codes[key] ?? 0 };
    await this.cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', ...params });
    await this.cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...params });
    await this.settle();
  }

  // --- rendering -------------------------------------------------------------

  /** Emulate a CSS media type ('print', 'screen', or '' to stop emulating). */
  async media(type) {
    await this.cdp.send('Emulation.setEmulatedMedia', { media: type });
  }

  /** Resize the viewport (0,0 restores the window's own size). */
  async viewport(width, height) {
    await this.cdp.send('Emulation.setDeviceMetricsOverride', {
      width, height, deviceScaleFactor: 1, mobile: false,
    });
  }

  /** Computed style of the first match, for the properties named. */
  async style(sel, props) {
    return this.eval(`
      const e = document.querySelector(${jsLiteral(sel)});
      if (!e) return null;
      const cs = getComputedStyle(e);
      const out = {};
      for (const p of ${jsLiteral(props)}) out[p] = cs.getPropertyValue(p);
      return out;
    `);
  }

  /** Save a full-page PNG. */
  async screenshot(path) {
    const r = await this.cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
    await writeFile(path, Buffer.from(r.data, 'base64'));
    return path;
  }

  /** Save the page as the browser's print pipeline renders it. */
  async pdf(path, opts = {}) {
    const r = await this.cdp.send('Page.printToPDF', {
      printBackground: true,
      preferCSSPageSize: true,
      ...opts,
    });
    await writeFile(path, Buffer.from(r.data, 'base64'));
    return path;
  }

  #once(method) {
    return new Promise((ok) => {
      const h = (p) => ok(p);
      this.cdp.on(method, h);
    });
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// --- launching ---------------------------------------------------------------

async function launchChrome(headed) {
  const dir = await mkdtemp(join(tmpdir(), 'ui-drive-'));
  const flags = [
    headed ? '--new-window' : '--headless=new',
    '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--disable-features=Translate,MediaRouter',
    '--remote-debugging-port=0',
    `--user-data-dir=${dir}`,
    'about:blank',
  ];
  if (process.env.CHROME_FLAGS) flags.push(...process.env.CHROME_FLAGS.split(/\s+/).filter(Boolean));
  const child = spawn(findChrome(), flags, { stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (d) => { stderr += d; });

  // Chrome writes the port it actually bound to into the profile once it is up.
  const portFile = join(dir, 'DevToolsActivePort');
  const ceiling = Date.now() + 20000;
  for (;;) {
    if (existsSync(portFile)) {
      const [port] = (await readFile(portFile, 'utf8')).split('\n');
      if (port) return { child, dir, port: Number(port) };
    }
    if (child.exitCode !== null) throw new Error(`Chrome exited (${child.exitCode}): ${stderr.slice(-500)}`);
    if (Date.now() > ceiling) throw new Error(`Chrome never reported a debugging port: ${stderr.slice(-500)}`);
    await sleep(50);
  }
}

async function pageTarget(port) {
  const ceiling = Date.now() + 10000;
  for (;;) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const target = list.find((t) => t.type === 'page');
      if (target?.webSocketDebuggerUrl) return target.webSocketDebuggerUrl;
    } catch { /* not listening yet */ }
    if (Date.now() > ceiling) throw new Error('no page target appeared');
    await sleep(50);
  }
}

// --- CLI ---------------------------------------------------------------------

function parseArgs(argv) {
  const opts = { url: null, headed: false, keep: false, timeout: 10000, script: null, rest: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--url') opts.url = argv[++i];
    else if (a === '--headed') opts.headed = true;
    else if (a === '--keep') opts.keep = true;
    else if (a === '--timeout') opts.timeout = Number(argv[++i]);
    else if (a === '-h' || a === '--help') opts.help = true;
    else if (!opts.script) opts.script = a;
    else opts.rest.push(a);
  }
  return opts;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help || !opts.script || !opts.url) {
    process.stderr.write('usage: node scripts/ui-drive.js --url URL steps.js [arg...]\n');
    process.exit(opts.help ? 0 : 2);
  }

  const mod = await import(pathToFileURL(resolve(opts.script)).href);
  const run = mod.default;
  if (typeof run !== 'function') throw new Error(`${opts.script} must default-export an async function`);

  const { child, dir, port } = await launchChrome(opts.headed);
  const cdp = await Cdp.attach(await pageTarget(port));
  const page = await Page.open(cdp, opts.url, opts.timeout);

  let failure = null;
  try {
    await page.goto('/');
    await run(page, opts.rest);
  } catch (e) {
    failure = e;
  }

  // Console output is diagnostic for both outcomes: a scenario that "passed"
  // while the page threw is not a pass, and that is invisible from the DOM.
  const noisy = page.logs.filter((l) => l.kind === 'error' || l.kind === 'exception');
  if (noisy.length) {
    process.stderr.write(`ui-drive: ${noisy.length} console error(s)/exception(s):\n`);
    for (const l of noisy) process.stderr.write(`  [${l.kind}] ${l.text}\n`);
  }

  if (!opts.keep) {
    cdp.close();
    child.kill('SIGKILL');
    // Chrome's helper processes keep writing to the profile for a moment after
    // the launcher dies, so removing it immediately loses the race with an
    // ENOTEMPTY. Wait for the launcher, then retry briefly; a leftover temp
    // profile is not worth failing a scenario over.
    await new Promise((ok) => (child.exitCode !== null ? ok() : child.once('exit', ok)));
    for (let i = 0; i < 20; i++) {
      try { await rm(dir, { recursive: true, force: true }); break; } catch { await sleep(100); }
    }
  } else {
    process.stderr.write(`ui-drive: --keep, browser left running (profile ${dir})\n`);
  }

  if (failure) {
    process.stderr.write(`ui-drive: ${failure.stack || failure.message}\n`);
    process.exit(1);
  }
}

main().catch((e) => {
  process.stderr.write(`ui-drive: ${e.stack || e.message}\n`);
  process.exit(1);
});
