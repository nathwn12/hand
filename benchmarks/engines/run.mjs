#!/usr/bin/env node
// Standalone engine-equivalence benchmark. Node 22+ (global fetch + WebSocket).
// Own seam: benchmarks/engines/ only. No npm dependencies, no repo edits elsewhere.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { inflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPS_DEFAULT = 3;
const START_TIMEOUT_MS = 20000;
const NAV_TIMEOUT_MS = 20000;
const CDP_TIMEOUT_MS = 30000;
const VIEWPORT = { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false };
const delay = (ms) => new Promise((r) => setTimeout(r, ms));
const round = (n) => Math.round(n * 100) / 100;
const now = () => performance.now();

function median(values) {
  const a = values.filter((v) => typeof v === 'number' && Number.isFinite(v)).sort((x, y) => x - y);
  if (!a.length) return null;
  const m = Math.floor(a.length / 2);
  return round(a.length % 2 ? a[m] : (a[m - 1] + a[m]) / 2);
}

function within25(reference, got) {
  if (typeof reference !== 'number' || typeof got !== 'number') return false;
  if (!Number.isFinite(reference) || !Number.isFinite(got)) return false;
  if (reference === 0) return got === 0;
  return Math.abs(got - reference) / Math.abs(reference) <= 0.25;
}

function parseArgs(argv) {
  let urls = null;
  let reps = REPS_DEFAULT;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--urls' && argv[i + 1]) {
      urls = argv[i + 1].split(',').map((s) => s.trim()).filter(Boolean);
      i++;
    } else if (a.indexOf('--urls=') === 0) {
      urls = a.slice(7).split(',').map((s) => s.trim()).filter(Boolean);
    } else if (a === '--reps' && argv[i + 1]) {
      const n = Number(argv[i + 1]);
      if (Number.isInteger(n) && n > 0) reps = n;
      i++;
    }
  }
  return { urls, reps };
}

function freePort() {
  return new Promise((res, rej) => {
    const s = createNetServer();
    s.once('error', rej);
    s.listen(0, '127.0.0.1', () => {
      const addr = s.address();
      const p = addr && typeof addr === 'object' ? addr.port : 0;
      s.close(() => res(p));
    });
  });
}

async function isFile(p) {
  try { return (await stat(p)).isFile(); } catch { return false; }
}

// Same resolution order as crates/browser-engine/src/browser.rs find_chromium:
// explicit GLADIUS_CHROMIUM first, else the versioned cache under LOCALAPPDATA
// (or GLADIUS_CHROMIUM_CACHE). Prefer a chrome-headless-shell in the newest
// directory; fall back to the pinned full Chrome binary if no shell is present.
async function resolveChromium() {
  const explicit = (process.env.GLADIUS_CHROMIUM || '').trim();
  if (explicit) {
    if (await isFile(explicit)) return { path: explicit, source: 'GLADIUS_CHROMIUM', note: null };
    return { error: 'GLADIUS_CHROMIUM is set but is not a file: ' + explicit };
  }
  const cache = (process.env.GLADIUS_CHROMIUM_CACHE || '').trim() ||
    ((process.env.LOCALAPPDATA || '').trim() ? join(process.env.LOCALAPPDATA.trim(), 'gladius', 'chromium') : '');
  if (!cache) return { error: 'chromium cache root unavailable (LOCALAPPDATA and GLADIUS_CHROMIUM_CACHE both unset)' };
  let dirs = [];
  try {
    dirs = (await readdir(cache, { withFileTypes: true })).filter((d) => d.isDirectory()).map((d) => d.name).sort().reverse();
  } catch (e) {
    return { error: 'cannot list chromium cache ' + cache + ': ' + e.message };
  }
  const rels = ['chrome-headless-shell.exe', join('chrome-headless-shell-win64', 'chrome-headless-shell.exe'), join('chrome-win64', 'chrome.exe')];
  for (const d of dirs) {
    for (const rel of rels) {
      const p = join(cache, d, rel);
      if (await isFile(p)) {
        return {
          path: p,
          source: 'cache:' + d,
          note: rel.indexOf('chrome.exe') >= 0 ? 'headless-shell absent in this cache entry; pinned full Chrome from browser.rs used' : null,
        };
      }
    }
  }
  return { error: 'no chrome-headless-shell.exe under ' + cache + ' (entries: ' + dirs.join(', ') + ')' };
}

async function resolveObscura() {
  const p = join(process.env.USERPROFILE || '', '.local', 'bin', 'obscura.exe');
  if (await isFile(p)) return { path: p, source: 'USERPROFILE/.local/bin', note: null };
  return { error: 'obscura.exe not found at ' + p };
}

async function launch(spec) {
  const rec = {
    name: spec.name, exe: spec.exe || null, source: spec.source || null, note: spec.note || null,
    port: spec.port, args: spec.args.slice(), profile: null, child: null,
    stdout: '', stderr: '', coldStartMs: null, version: null, wsUrl: null, error: null,
    identity: null, results: [], errors: [], cdp: null,
  };
  if (spec.name === 'chromium') {
    try { rec.profile = await mkdtemp(join(tmpdir(), 'hand-engines-chrome-')); }
    catch (e) { rec.error = 'create temp profile: ' + e.message; return rec; }
    rec.args = rec.args.map((a) => a === '--user-data-dir=' ? '--user-data-dir=' + rec.profile : a);
  }
  try {
    rec.child = spawn(rec.exe, rec.args, { stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
  } catch (e) { rec.error = 'spawn: ' + e.message; return rec; }
  rec.child.stdout.on('data', (d) => { rec.stdout = (rec.stdout + d.toString()).slice(-8000); });
  rec.child.stderr.on('data', (d) => { rec.stderr = (rec.stderr + d.toString()).slice(-8000); });
  rec.child.on('error', (e) => { rec.error = (rec.error ? rec.error + '; ' : '') + 'child error: ' + e.message; });
  const base = 'http://127.0.0.1:' + spec.port;
  const t0 = now();
  let lastErr = '';
  while (now() - t0 < START_TIMEOUT_MS) {
    if (rec.child.exitCode !== null) {
      rec.error = (rec.error ? rec.error + '; ' : '') + 'engine exited with code ' + rec.child.exitCode;
      break;
    }
    try {
      const resp = await fetch(base + '/json/version', { signal: AbortSignal.timeout(1000) });
      if (resp.ok) {
        const v = await resp.json();
        if (v && v.webSocketDebuggerUrl) {
          rec.version = v;
          rec.wsUrl = v.webSocketDebuggerUrl;
          rec.coldStartMs = round(now() - t0);
          break;
        }
        lastErr = 'no webSocketDebuggerUrl in /json/version';
      } else lastErr = 'HTTP ' + resp.status;
    } catch (e) { lastErr = e.message; }
    await delay(100);
  }
  if (rec.coldStartMs === null && !rec.error) {
    rec.error = 'readiness timeout after ' + START_TIMEOUT_MS + 'ms: ' + lastErr;
  }
  return rec;
}

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 0;
    this.pending = new Map();
    this.waiters = [];
    ws.addEventListener('message', (ev) => this.onMessage(ev));
    ws.addEventListener('close', () => this.failAll('CDP socket closed'));
    ws.addEventListener('error', () => this.failAll('CDP socket error'));
  }

  static connect(url, timeoutMs) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      const timer = setTimeout(() => {
        try { ws.close(); } catch {}
        reject(new Error('CDP WebSocket connect timeout: ' + url));
      }, timeoutMs || 10000);
      ws.addEventListener('open', () => { clearTimeout(timer); resolve(new Cdp(ws)); }, { once: true });
      ws.addEventListener('error', () => {
        clearTimeout(timer);
        reject(new Error('CDP WebSocket connect failed: ' + url));
      }, { once: true });
    });
  }

  onMessage(ev) {
    let m;
    try { m = JSON.parse(typeof ev.data === 'string' ? ev.data : Buffer.from(ev.data).toString('utf8')); }
    catch { return; }
    if (m.id !== undefined) {
      const p = this.pending.get(m.id);
      if (!p) return;
      this.pending.delete(m.id);
      clearTimeout(p.timer);
      if (m.error) p.reject(new Error(m.error.message || JSON.stringify(m.error)));
      else p.resolve(m.result);
      return;
    }
    if (m.method) {
      for (const w of this.waiters.slice()) {
        if (w.method !== m.method) continue;
        if (w.sessionId && m.sessionId && w.sessionId !== m.sessionId) continue;
        w.resolve(m);
        const i = this.waiters.indexOf(w);
        if (i >= 0) this.waiters.splice(i, 1);
      }
    }
  }

  failAll(msg) {
    for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(new Error(msg)); }
    this.pending.clear();
    for (const w of this.waiters) { clearTimeout(w.timer); w.reject(new Error(msg)); }
    this.waiters = [];
  }

  send(method, params, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error('CDP timeout: ' + method));
      }, CDP_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
      const msg = { id: id, method: method, params: params || {} };
      if (sessionId) msg.sessionId = sessionId;
      try { this.ws.send(JSON.stringify(msg)); }
      catch (e) { clearTimeout(timer); this.pending.delete(id); reject(e); }
    });
  }

  waitFor(method, sessionId, timeoutMs) {
    return new Promise((resolve, reject) => {
      const w = { method, sessionId, resolve, reject, timer: null };
      w.timer = setTimeout(() => {
        const i = this.waiters.indexOf(w);
        if (i >= 0) this.waiters.splice(i, 1);
        reject(new Error('wait timeout: ' + method));
      }, timeoutMs || CDP_TIMEOUT_MS);
      this.waiters.push(w);
    });
  }

  close() { try { this.ws.close(); } catch {} }
}

// Tiny inline PNG reader: supports 8-bit RGB/RGBA, non-interlaced. It reports
// the number of distinct reconstructed pixel sample values, which is what the
// non-blank check needs. Unsupported PNG forms fall back to a size heuristic.
function pngUniformity(buf) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  if (buf.length < 24 || !sig.equals(buf.subarray(0, 8))) return { supported: false, reason: 'not a PNG' };
  let off = 8;
  let ihdr = null;
  const idat = [];
  while (off + 12 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString('ascii', off + 4, off + 8);
    if (off + 12 + len > buf.length) break;
    const data = buf.subarray(off + 8, off + 8 + len);
    if (type === 'IHDR') {
      ihdr = { width: data.readUInt32BE(0), height: data.readUInt32BE(4), depth: data[8], color: data[9], interlace: data[12] };
    } else if (type === 'IDAT') idat.push(data);
    else if (type === 'IEND') break;
    off += 12 + len;
  }
  if (!ihdr) return { supported: false, reason: 'no IHDR chunk' };
  if (ihdr.depth !== 8 || (ihdr.color !== 2 && ihdr.color !== 6) || ihdr.interlace !== 0) {
    return { supported: false, reason: 'PNG form unsupported (depth ' + ihdr.depth + ', color ' + ihdr.color + ', interlace ' + ihdr.interlace + ')' };
  }
  let raw;
  try { raw = inflateSync(Buffer.concat(idat)); }
  catch (e) { return { supported: false, reason: 'IDAT inflate failed: ' + e.message }; }
  const bpp = ihdr.color === 6 ? 4 : 3;
  const stride = ihdr.width * bpp;
  if (raw.length < (stride + 1) * ihdr.height) return { supported: false, reason: 'short pixel data' };
  const prev = Buffer.alloc(stride);
  const cur = Buffer.alloc(stride);
  const distinct = new Set();
  let p = 0;
  for (let y = 0; y < ihdr.height; y++) {
    const filter = raw[p++];
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? cur[x - bpp] : 0;
      const b = prev[x];
      const c = x >= bpp ? prev[x - bpp] : 0;
      let v = raw[p++];
      if (filter === 1) v = (v + a) & 255;
      else if (filter === 2) v = (v + b) & 255;
      else if (filter === 3) v = (v + ((a + b) >> 1)) & 255;
      else if (filter === 4) {
        const pa = Math.abs(b - c), pb = Math.abs(a - c), pc = Math.abs(a + b - 2 * c);
        const pred = (pa <= pb && pa <= pc) ? a : (pb <= pc ? b : c);
        v = (v + pred) & 255;
      } else if (filter !== 0) return { supported: false, reason: 'bad PNG filter ' + filter };
      cur[x] = v;
      if (distinct.size < 4096) distinct.add(v);
    }
    cur.copy(prev);
  }
  return { supported: true, distinctValues: distinct.size, uniform: distinct.size < 2, width: ihdr.width, height: ihdr.height };
}

function screenshotCheck(bytes) {
  const size = bytes.length;
  const u = pngUniformity(bytes);
  return {
    bytes: size,
    sha256: createHash('sha256').update(bytes).digest('hex'),
    png: u,
    nonBlank: size > 2048 && (u.supported ? !u.uniform : true),
    caveat: u.supported ? null : 'PNG decoder could not read this form; non-blank decided by byte-size (>2048) heuristic only',
  };
}

async function evaluate(cdp, sessionId, expression) {
  const r = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, sessionId);
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.text || 'Runtime.evaluate exception');
  return r.result && r.result.value;
}

async function measureRep(cdp, url, rep) {
  const rec = { rep, errors: [] };
  const timed = async (name, fn) => {
    const t = now();
    try {
      const v = await fn();
      rec[name] = { ms: round(now() - t), ok: true, value: v };
      return v;
    } catch (e) {
      rec[name] = { ms: round(now() - t), ok: false, error: e.message, value: null };
      rec.errors.push(name + ': ' + e.message);
      return null;
    }
  };

  const attach = await timed('createAttach', async () => {
    const t = await cdp.send('Target.createTarget', { url: 'about:blank' });
    const a = await cdp.send('Target.attachToTarget', { targetId: t.targetId, flatten: true });
    if (!a.sessionId) throw new Error('attachToTarget returned no sessionId');
    return { targetId: t.targetId, sessionId: a.sessionId };
  });
  rec.targetId = attach ? attach.targetId : null;
  if (!attach) return rec;
  const sid = attach.sessionId;

  for (const [method, params] of [
    ['Page.enable', {}], ['Runtime.enable', {}], ['Accessibility.enable', {}],
    ['Emulation.setDeviceMetricsOverride', VIEWPORT],
  ]) {
    try { await cdp.send(method, params, sid); }
    catch (e) { rec.errors.push(method + ': ' + e.message); }
  }

  {
    const wait = cdp.waitFor('Page.loadEventFired', sid, NAV_TIMEOUT_MS);
    const t = now();
    let navErr = null;
    try { await cdp.send('Page.navigate', { url }, sid); }
    catch (e) { navErr = e.message; }
    try { await wait; }
    catch (e) { navErr = (navErr ? navErr + '; ' : '') + e.message; }
    rec.navigate = { ms: round(now() - t), ok: !navErr, error: navErr };
    if (navErr) rec.errors.push('navigate: ' + navErr);
  }
  await delay(100);

  rec.title = await timed('title', () => evaluate(cdp, sid, 'document.title'));

  await timed('ax', async () => {
    const r = await cdp.send('Accessibility.getFullAXTree', {}, sid);
    const nodes = r.nodes || [];
    return { total: nodes.length, nonIgnored: nodes.filter((n) => !n.ignored).length };
  });

  await timed('screenshot', async () => {
    const r = await cdp.send('Page.captureScreenshot', { format: 'png' }, sid);
    return screenshotCheck(Buffer.from(r.data || '', 'base64'));
  });

  await timed('text', async () => {
    const v = await evaluate(cdp, sid, 'document.body.innerText.length');
    return typeof v === 'number' ? v : null;
  });
  if (rec.text && rec.text.value === null) {
    rec.errors.push('text: innerText length was not a number');
    rec.text.ok = false;
  }

  // insertText before click so the click always starts from the loaded document.
  const hasInput = await timed('inputProbe', () => evaluate(cdp, sid, '!!document.querySelector("input[type=text]")'));
  if (hasInput === true) {
    try { await evaluate(cdp, sid, 'document.querySelector("input[type=text]").focus()'); }
    catch (e) { rec.errors.push('input focus: ' + e.message); }
    const token = 'hand-bench-' + rep;
    await timed('insertText', async () => {
      await cdp.send('Input.insertText', { text: token }, sid);
      return await evaluate(cdp, sid, 'document.querySelector("input[type=text]").value');
    });
    const ins = rec.insertText;
    rec.insert = {
      ms: ins.ms, ok: ins.ok, value: ins.value,
      expected: token, written: !!(typeof ins.value === 'string' && ins.value.indexOf(token) >= 0),
      error: ins.error || null,
    };
  } else {
    rec.insert = { ms: null, ok: null, skipped: true, reason: 'no input[type=text] on page' };
  }

  const target = await timed('clickTarget', () => evaluate(cdp, sid, '(function(){var el=document.querySelector("h1")||document.querySelector("a");if(!el)return null;var r=el.getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2,tag:el.tagName};})()'));
  const beforeState = await timed('clickBefore', () => evaluate(cdp, sid, 'location.href + "|" + document.title'));
  if (target) {
    await timed('click', async () => {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: target.x, y: target.y, button: 'left', clickCount: 1 }, sid);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: target.x, y: target.y, button: 'left', clickCount: 1 }, sid);
      return true;
    });
    const clk = rec.click;
    await delay(300);
    let afterState = null;
    try { afterState = await evaluate(cdp, sid, 'location.href + "|" + document.title'); }
    catch (e) { rec.errors.push('click after-state: ' + e.message); }
    rec.click = {
      ms: clk.ms, ok: clk.ok, target,
      changed: !!(afterState !== null && beforeState !== null && afterState !== beforeState),
      before: beforeState, after: afterState, error: clk.error || null,
    };
  } else {
    rec.click = { ms: null, ok: null, skipped: true, reason: 'no h1 or link on page' };
  }

  if (rec.targetId) {
    try { await cdp.send('Target.closeTarget', { targetId: rec.targetId }); } catch {}
  }
  return rec;
}

function aggregate(urlRec, coldStartMs) {
  const reps = urlRec.reps;
  const vals = (fn) => reps.map(fn).filter((v) => typeof v === 'number' && Number.isFinite(v));
  const firstValue = (fn) => {
    for (const r of reps) { const v = fn(r); if (v !== undefined && v !== null) return v; }
    return null;
  };
  return {
    repetitions: reps.length,
    coldStartMs,
    createAttachMs: median(vals((r) => r.createAttach && r.createAttach.ok ? r.createAttach.ms : null)),
    navigateMs: median(vals((r) => r.navigate && r.navigate.ok ? r.navigate.ms : null)),
    axMs: median(vals((r) => r.ax && r.ax.ok ? r.ax.ms : null)),
    axNodes: median(vals((r) => r.ax && r.ax.ok && r.ax.value ? r.ax.value.total : null)),
    axNonIgnoredNodes: median(vals((r) => r.ax && r.ax.ok && r.ax.value ? r.ax.value.nonIgnored : null)),
    navAxMs: median(vals((r) => r.navigate && r.navigate.ok && r.ax && r.ax.ok ? r.navigate.ms + r.ax.ms : null)),
    screenshotMs: median(vals((r) => r.screenshot && r.screenshot.ok ? r.screenshot.ms : null)),
    screenshotBytes: median(vals((r) => r.screenshot && r.screenshot.ok && r.screenshot.value ? r.screenshot.value.bytes : null)),
    screenshotNonBlankAll: reps.length > 0 && reps.every((r) => !!(r.screenshot && r.screenshot.ok && r.screenshot.value && r.screenshot.value.nonBlank)),
    textMs: median(vals((r) => r.text && r.text.ok ? r.text.ms : null)),
    textLength: median(vals((r) => r.text && r.text.ok ? r.text.value : null)),
    clickMs: median(vals((r) => r.click && r.click.ok ? r.click.ms : null)),
    clickChangedAny: reps.some((r) => !!(r.click && r.click.changed)),
    clickSkipped: reps.length > 0 && reps.every((r) => !!(r.click && r.click.skipped)),
    insertMs: median(vals((r) => r.insert && r.insert.ok ? r.insert.ms : null)),
    insertWrittenCount: reps.filter((r) => !!(r.insert && r.insert.written)).length,
    insertSkipped: reps.length > 0 && reps.every((r) => !!(r.insert && r.insert.skipped)),
    title: firstValue((r) => r.title),
    errors: reps.reduce((acc, r) => acc.concat(r.errors || []), []),
  };
}

async function runEngine(eng, urls, reps) {
  eng.results = [];
  if (!eng.wsUrl) {
    for (const url of urls) eng.results.push({ url, reps: [], aggregate: null, error: eng.error || 'engine unavailable' });
    return;
  }
  try {
    eng.cdp = await Cdp.connect(eng.wsUrl, 10000);
    eng.identity = await eng.cdp.send('Browser.getVersion');
  } catch (e) {
    eng.errors.push('cdp connect/identity: ' + e.message);
    eng.cdp = null;
  }
  if (!eng.cdp) {
    for (const url of urls) eng.results.push({ url, reps: [], aggregate: null, error: eng.errors.join('; ') });
    return;
  }
  for (const url of urls) {
    const rec = { url, reps: [], aggregate: null, error: null };
    for (let i = 1; i <= reps; i++) {
      try { rec.reps.push(await measureRep(eng.cdp, url, i)); }
      catch (e) { rec.reps.push({ rep: i, errors: ['fatal: ' + e.message] }); }
    }
    rec.aggregate = aggregate(rec, eng.coldStartMs);
    eng.results.push(rec);
  }
}

function compare(chrome, obscura, urls) {
  const perUrl = [];
  const failing = [];
  for (let i = 0; i < urls.length; i++) {
    const c = chrome.results[i];
    const o = obscura.results[i];
    const ca = c && c.aggregate;
    const oa = o && o.aggregate;
    const checks = [];
    if (!ca || !oa || !chrome.wsUrl || !obscura.wsUrl) {
      checks.push({ name: 'engine-available', pass: false, detail: 'one engine produced no measurements' });
    } else {
      checks.push({ name: 'ax-node-count', pass: within25(ca.axNodes, oa.axNodes), detail: 'chromium ' + ca.axNodes + ' vs obscura ' + oa.axNodes });
      checks.push({ name: 'innerText-length', pass: within25(ca.textLength, oa.textLength), detail: 'chromium ' + ca.textLength + ' vs obscura ' + oa.textLength });
      checks.push({ name: 'screenshot-nonblank', pass: !!oa.screenshotNonBlankAll, detail: 'obscura all reps non-blank: ' + oa.screenshotNonBlankAll });
      checks.push({ name: 'page-title', pass: ca.title !== null && ca.title === oa.title, detail: 'chromium ' + JSON.stringify(ca.title) + ' vs obscura ' + JSON.stringify(oa.title) });
    }
    const speed = (ca && oa && ca.navAxMs && oa.navAxMs) ? round(ca.navAxMs / oa.navAxMs) : null;
    checks.push({
      name: 'speed-2x',
      pass: speed !== null && speed >= 2,
      detail: 'nav+AX chromium ' + (ca ? ca.navAxMs : null) + 'ms vs obscura ' + (oa ? oa.navAxMs : null) + 'ms (' + speed + 'x)',
    });
    perUrl.push({ url: urls[i], checks, speedup: speed, chrome: ca, obscura: oa });
    for (const ck of checks) if (!ck.pass) failing.push(urls[i] + ': ' + ck.name + ' [' + ck.detail + ']');
  }
  return { perUrl, failing, eligible: failing.length === 0 && !!chrome.wsUrl && !!obscura.wsUrl };
}

function fmt(v, suffix) {
  if (v === null || v === undefined) return '-';
  return String(v) + (suffix || '');
}

function pad(s, n) {
  const t = String(s === null || s === undefined ? '-' : s);
  return t.length > n ? t.slice(0, n - 1) + '~' : t.padEnd(n);
}

function printTable(engines, urls, comparison, reps) {
  console.log('');
  console.log('Engine identity (Browser.getVersion):');
  for (const e of engines) {
    const id = e.identity ? (e.identity.product || '?') + ' rev ' + (e.identity.revision || '?') : 'unavailable';
    console.log('  ' + pad(e.name, 10) + id + '  exe=' + (e.exe || '-') + '  source=' + (e.source || '-'));
  }
  console.log('');
  const head = 'URL'.padEnd(42) + ' | engine     | cold   | attach | nav    | AX ms/n        | PNG ms/B        | text ms/len    | click ms/chg   | insert ms/val';
  console.log(head);
  console.log('-'.repeat(head.length));
  for (const e of engines) {
    for (const r of e.results) {
      const a = r.aggregate;
      if (!a) {
        console.log(pad(r.url, 42) + ' | ' + pad(e.name, 10) + ' | unavailable: ' + (e.error || 'no result'));
        continue;
      }
      console.log(
        pad(r.url, 42) + ' | ' + pad(e.name, 10) +
        ' | ' + pad(fmt(a.coldStartMs), 6) +
        ' | ' + pad(fmt(a.createAttachMs), 6) +
        ' | ' + pad(fmt(a.navigateMs), 6) +
        ' | ' + pad(fmt(a.axMs) + '/' + fmt(a.axNodes), 14) +
        ' | ' + pad(fmt(a.screenshotMs) + '/' + fmt(a.screenshotBytes), 15) +
        ' | ' + pad(fmt(a.textMs) + '/' + fmt(a.textLength), 14) +
        ' | ' + pad(fmt(a.clickMs) + '/' + (a.clickSkipped ? 'skip' : (a.clickChangedAny ? 'yes' : 'no')), 14) +
        ' | ' + pad(fmt(a.insertMs) + '/' + (a.insertSkipped ? 'skip' : (a.insertWrittenCount + '/' + reps + ' ok')), 14)
      );
    }
  }
  console.log('');
  console.log('Answer equality vs chromium (25% tolerance; AX = all nodes returned by getFullAXTree):');
  const ehead = 'URL'.padEnd(42) + ' | AX pass | text pass | PNG ok | title ok | nav+AX c/o (ms)      | speed';
  console.log(ehead);
  console.log('-'.repeat(ehead.length));
  for (const row of comparison.perUrl) {
    const by = {};
    for (const ck of row.checks) by[ck.name] = ck;
    const mark = (k) => by[k] ? (by[k].pass ? 'yes' : 'NO') : '-';
    const cnav = row.chrome ? row.chrome.navAxMs : null;
    const onav = row.obscura ? row.obscura.navAxMs : null;
    console.log(
      pad(row.url, 42) + ' | ' + pad(mark('ax-node-count'), 7) + ' | ' + pad(mark('innerText-length'), 9) +
      ' | ' + pad(mark('screenshot-nonblank'), 6) + ' | ' + pad(mark('page-title'), 8) +
      ' | ' + pad(fmt(cnav) + ' / ' + fmt(onav), 20) + ' | ' + pad(fmt(row.speedup, 'x'), 6)
    );
  }
  console.log('');
  console.log('VERDICT: ' + (comparison.eligible ? 'obscura-eligible' : 'obscura-not-eligible' + (comparison.failing.length ? ' - failing: ' + comparison.failing.join('; ') : '')));
}

async function killTree(child) {
  if (!child || child.exitCode !== null) return;
  if (process.platform === 'win32') {
    await new Promise((res) => {
      const k = spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true });
      k.once('exit', res);
      k.once('error', res);
    });
  } else {
    try { process.kill(-child.pid, 'SIGKILL'); }
    catch { try { child.kill('SIGKILL'); } catch {} }
  }
}

async function cleanup(engines, fixture) {
  for (const e of engines) {
    if (e.cdp) e.cdp.close();
    await killTree(e.child);
  }
  await delay(300);
  for (const e of engines) {
    if (!e.profile) continue;
    try { await rm(e.profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); }
    catch (err) { console.log('WARN: temp profile cleanup failed for ' + e.name + ': ' + err.message); }
  }
  if (fixture) {
    try { await new Promise((res) => fixture.server.close(res)); } catch {}
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  let fixture = null;
  let fixtureError = null;
  let urls = args.urls && args.urls.length ? args.urls.slice() : [];
  if (!urls.length) {
    try {
      const mod = await import(new URL('../../fixtures/serve.mjs', import.meta.url).href);
      fixture = await mod.startFixtureServer({ port: 0 });
      urls.push(fixture.origin + '/index.html');
    } catch (e) { fixtureError = e.message; }
    urls.push('https://example.com', 'https://news.ycombinator.com');
  }

  const chromeRes = await resolveChromium();
  const obscuraRes = await resolveObscura();
  const chromePort = await freePort();
  const obscuraPort = await freePort();
  const specs = [
    {
      name: 'chromium', exe: chromeRes.path || null, source: chromeRes.source || null, note: chromeRes.note || null,
      error: chromeRes.error || null, port: chromePort,
      args: [
        '--headless', '--window-size=1280,720', '--remote-debugging-port=' + chromePort,
        '--user-data-dir=', '--no-first-run', '--no-default-browser-check', '--disable-gpu',
        '--remote-allow-origins=http://127.0.0.1:' + chromePort, 'about:blank',
      ],
    },
    {
      name: 'obscura', exe: obscuraRes.path || null, source: obscuraRes.source || null, note: obscuraRes.note || null,
      error: obscuraRes.error || null, port: obscuraPort,
      args: ['serve', '--port', String(obscuraPort), '--allow-private-network'],
    },
  ];

  const engines = [];
  try {
    for (const spec of specs) {
      if (spec.error || !spec.exe) {
        engines.push({
          name: spec.name, exe: spec.exe || null, source: spec.source || null, note: spec.note || null,
          port: spec.port, args: spec.args, profile: null, child: null, stdout: '', stderr: '',
          coldStartMs: null, version: null, wsUrl: null, error: spec.error || 'no executable resolved',
          identity: null, results: [], errors: [], cdp: null,
        });
        continue;
      }
      engines.push(await launch(spec));
    }
    for (const e of engines) await runEngine(e, urls, args.reps);
    const comparison = compare(engines[0], engines[1], urls);
    printTable(engines, urls, comparison, args.reps);

    if (!engines[1].wsUrl) {
      console.log('');
      console.log('obscura stderr (verbatim):');
      console.log(engines[1].stderr || '(empty)');
    }
    if (fixtureError) console.log('fixture server error: ' + fixtureError);

    const iso = new Date().toISOString();
    const stamp = iso.slice(0, 10).replace(/-/g, '') + 'T' + iso.slice(11, 19).replace(/:/g, '') + 'Z';
    const reportPath = join(HERE, 'report-' + stamp + '.json');
    const report = {
      schema: 1,
      generatedAt: iso,
      command: 'node benchmarks/engines/run.mjs ' + process.argv.slice(2).join(' '),
      urls,
      repetitions: args.reps,
      viewport: VIEWPORT,
      method: {
        equality: 'medians over repetitions; within-25% is |obscura-chromium|/chromium <= 0.25; AX count is all nodes returned by Accessibility.getFullAXTree (non-ignored count also recorded)',
        screenshot: 'non-blank requires PNG > 2048 bytes and >= 2 distinct reconstructed pixel sample values; unsupported PNG forms fall back to byte-size heuristic with a per-shot caveat',
        speed: '2x means chromium median (navigate + AX) / obscura median (navigate + AX) >= 2',
        note: 'Both engines use the same 1280x720 DPR1 viewport override. insertText runs before the click so the click starts from the loaded document.',
      },
      fixtureServer: fixture ? fixture.origin + '/index.html' : null,
      fixtureError,
      engines: engines.map((e) => ({
        name: e.name, executable: e.exe, resolution: e.source, note: e.note, port: e.port,
        args: e.args, identity: e.identity, coldStartMs: e.coldStartMs, wsUrl: e.wsUrl,
        startupError: e.error, engineErrors: e.errors, stdoutTail: e.stdout, stderrTail: e.stderr,
        results: e.results,
      })),
      comparison,
      verdict: comparison.eligible ? 'obscura-eligible' : 'obscura-not-eligible',
    };
    try {
      await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n');
      console.log('report: ' + reportPath);
    } catch (e) { console.log('ERROR: report write failed: ' + e.message); }
  } finally {
    await cleanup(engines, fixture);
  }
}

main().catch((e) => {
  console.log('ERROR: ' + (e && e.stack ? e.stack : String(e)));
  console.log('VERDICT: obscura-not-eligible - benchmark error');
  process.exitCode = 0;
});
