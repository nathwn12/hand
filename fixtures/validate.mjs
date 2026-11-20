import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { startFixtureServer } from './serve.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const PAYLOAD_FILES = ['index.html', 'actions.html', 'manifest.json'];
const SUMS_FILE = 'SHA256SUMS';
const argv = process.argv.slice(2);

const staticOnly = argv.includes('--static-only');
const writeSums = argv.includes('--write-sums');
const browserArg = optionValue('--browser') ?? process.env.HAND_FIXTURE_BROWSER;

const failures = [];
let passes = 0;

function optionValue(name) {
  const index = argv.indexOf(name);
  if (index === -1) return undefined;
  const value = argv[index + 1];
  if (!value || value.startsWith('--')) {
    console.error(`${name} requires a value`);
    process.exit(2);
  }
  return value;
}

for (let i = 0; i < argv.length; i += 1) {
  const arg = argv[i];
  if (arg === '--browser') {
    i += 1;
    continue;
  }
  if (arg === '--static-only' || arg === '--write-sums' || arg === '--help') continue;
  console.error(`unknown argument: ${arg}`);
  console.error('usage: node validate.mjs [--static-only] [--write-sums] [--browser <path>]');
  process.exit(2);
}

if (argv.includes('--help')) {
  console.log('usage: node validate.mjs [--static-only] [--write-sums] [--browser <path>]');
  console.log('');
  console.log('  (no flags)     verify SHA256SUMS, then run headless browser checks');
  console.log('  --static-only  verify structure and SHA256SUMS without a browser');
  console.log('  --write-sums   verify structure, then write SHA256SUMS for the payload files');
  console.log('  --browser      Chromium-family executable to use for headless checks');
  process.exit(0);
}

function check(name, ok, detail = '') {
  if (ok) {
    passes += 1;
    console.log(`PASS  ${name}`);
  } else {
    failures.push({ name, detail });
    console.log(`FAIL  ${name}${detail ? ` - ${detail}` : ''}`);
  }
}

function mismatch(actual, expected, subset = false) {
  const keys = subset
    ? new Set(Object.keys(expected ?? {}))
    : new Set([...Object.keys(actual ?? {}), ...Object.keys(expected ?? {})]);
  const parts = [];
  for (const key of keys) {
    if (!Object.is(actual?.[key], expected?.[key])) {
      parts.push(`${key}: expected ${JSON.stringify(expected?.[key])}, got ${JSON.stringify(actual?.[key])}`);
    }
  }
  return parts.join('; ');
}

function sequence(prefix, count, pad) {
  return Array.from({ length: count }, (_, i) => `${prefix}${String(i + 1).padStart(pad, '0')}`);
}

function stripScriptsAndComments(html) {
  return html
    .replace(/<script\b[\s\S]*?<\/script>/gi, '')
    .replace(/<!--[\s\S]*?-->/g, '');
}

async function loadPayload() {
  const payload = new Map();
  for (const file of PAYLOAD_FILES) {
    payload.set(file, await readFile(join(root, file)));
  }
  return payload;
}

function computeHashes(payload) {
  const hashes = new Map();
  for (const [file, bytes] of payload) {
    hashes.set(file, createHash('sha256').update(bytes).digest('hex'));
  }
  return hashes;
}

async function runStaticChecks(payload, hashes) {
  const index = payload.get('index.html').toString('utf8');
  const actions = payload.get('actions.html').toString('utf8');
  const manifest = JSON.parse(payload.get('manifest.json').toString('utf8'));

  const indexNoScripts = stripScriptsAndComments(index);
  const actionsNoScripts = stripScriptsAndComments(actions);
  const expectedIds = sequence('control-', 100, 3);
  const expectedLabels = sequence('Control ', 100, 3);

  check(
    'manifest declares fixture v1',
    manifest.manifestVersion === 1 && manifest.fixture === 'hand-mcp-fixtures' && manifest.version === '1.0.0',
  );
  check(
    'manifest freezes a 1280x720 viewport at DPR 1',
    manifest.frozen?.viewport?.width === 1280 &&
      manifest.frozen?.viewport?.height === 720 &&
      manifest.frozen?.deviceScaleFactor === 1,
  );
  check(
    'manifest declares 100 controls on /index.html',
    manifest.benchmark?.page === '/index.html' &&
      manifest.benchmark?.selector === '[data-hand-control]' &&
      manifest.benchmark?.count === 100,
  );

  const buttonTags = [...indexNoScripts.matchAll(/<button\b[^>]*>/gi)].map((match) => match[0]);
  check('index.html contains exactly 100 button elements', buttonTags.length === 100, `got ${buttonTags.length}`);

  const controlTags = buttonTags.filter((tag) => /\bdata-hand-control\b/.test(tag));
  check(
    'every benchmark button carries data-hand-control and no other button exists',
    controlTags.length === 100 && controlTags.length === buttonTags.length,
    `got ${controlTags.length} marked of ${buttonTags.length}`,
  );

  const idList = buttonTags.map((tag) => /\bid="([^"]+)"/.exec(tag)?.[1] ?? '');
  check(
    'control ids are control-001 through control-100 in order',
    idList.join(',') === expectedIds.join(','),
    `got ${idList.slice(0, 3).join(',')} ... ${idList.slice(-2).join(',')}`,
  );
  check('control ids are unique', new Set(idList).size === 100, `got ${new Set(idList).size} unique`);

  const labelList = buttonTags.map((tag) => /\bdata-label="([^"]+)"/.exec(tag)?.[1] ?? '');
  check(
    'control labels are Control 001 through Control 100 in order',
    labelList.join(',') === expectedLabels.join(','),
    `got ${labelList.slice(0, 3).join(',')} ... ${labelList.slice(-2).join(',')}`,
  );
  check('control labels are unique', new Set(labelList).size === 100, `got ${new Set(labelList).size} unique`);

  const ariaList = buttonTags.map((tag) => /\baria-label="([^"]+)"/.exec(tag)?.[1] ?? '');
  check('every control has aria-label equal to its data-label', ariaList.join(',') === labelList.join(','));

  const otherInteractive = [...indexNoScripts.matchAll(/<(a|input|select|textarea|summary)\b/gi)];
  check(
    'benchmark page has no other interactive elements',
    otherInteractive.length === 0,
    `found ${otherInteractive.map((match) => match[0]).join(', ')}`,
  );

  const indexLinks = [...indexNoScripts.matchAll(/<link\b[^>]*>/gi)].map((match) => match[0]);
  const badIndexLinks = indexLinks.filter(
    (tag) => !(/\brel="icon"/i.test(tag) && /\bhref="data:,"/i.test(tag)),
  );
  check(
    'benchmark page has no anchors, images, or iframes and only the inline empty icon link',
    !/<(?:a|img|iframe)\b/i.test(indexNoScripts) &&
      badIndexLinks.length === 0 &&
      !/\bsrc\s*=/i.test(index) &&
      !/\b@import\b/i.test(index) &&
      !/url\s*\(/i.test(index) &&
      !/(?:href|src)\s*=\s*["'](?:https?:)?\/\//i.test(index),
    `unexpected links: ${badIndexLinks.join(', ')}`,
  );
  const allLinks = [...(index + actions).matchAll(/<link\b[^>]*>/gi)].map((match) => match[0]);
  const badLinks = allLinks.filter(
    (tag) => !(/\brel="icon"/i.test(tag) && /\bhref="data:,"/i.test(tag)),
  );
  check(
    'both pages request no external assets and use only the inline empty favicon',
    badLinks.length === 0 &&
      !/\b@import\b/i.test(index + actions) &&
      !/url\s*\(/i.test(index + actions) &&
      !/(?:href|src)\s*=\s*["'](?:https?:)?\/\//i.test(index + actions),
    `unexpected links: ${badLinks.join(', ')}`,
  );
  check(
    'benchmark page declares body data-control-count=100 and a fixed viewport',
    /<body[^>]*\bdata-control-count="100"/i.test(index) &&
      /<meta\s+name="viewport"\s+content="width=1280, initial-scale=1"/i.test(index),
  );

  const actionIds = [
    'apply',
    'reset',
    'noop',
    'result',
    'noop-count',
    'increment',
    'counter-value',
    'name-input',
    'apply-name',
    'greeting',
    'mode-select',
    'mode-output',
    'flag-check',
    'flag-state',
    'open-dialog',
    'confirm-dialog',
    'dialog-close',
    'dialog-state',
    'scroll-box',
    'scroll-content',
    'scroll-offset',
    'missing-link',
  ];
  const badActionIds = actionIds.filter(
    (id) => (actionsNoScripts.match(new RegExp(`\\bid="${id.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`, 'g')) ?? []).length !== 1,
  );
  check(
    'actions page defines each required fixture id exactly once',
    badActionIds.length === 0,
    `missing or duplicated: ${badActionIds.join(', ')}`,
  );
  check(
    'actions page links to the server 404 path',
    /<a[^>]*\bid="missing-link"[^>]*\bhref="\/missing\.html"/i.test(actions),
  );
  check(
    'actions page has a native dialog and a nested scroll box',
    /<dialog\b[^>]*\bid="confirm-dialog"/i.test(actions) &&
      /<div\b[^>]*\bid="scroll-box"[^>]*>/i.test(actions) &&
      /<div\b[^>]*\bid="scroll-content"/i.test(actions),
  );

  const pageScripts = [
    ...[...index.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)].map((match) => match[1]),
    ...[...actions.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)].map((match) => match[1]),
  ].join('\n');
  check(
    'fixture scripts use no timers, randomness, clock reads, storage, or network calls',
    !/\b(?:setTimeout|setInterval|requestAnimationFrame|Date|Math\.random|performance\.now|localStorage|sessionStorage|indexedDB|fetch|XMLHttpRequest|WebSocket|EventSource|sendBeacon)\b/.test(pageScripts),
  );
  check(
    'pages contain no automatic refresh or navigation',
    !/<meta[^>]+http-equiv=["']?refresh/i.test(index + actions) && !/\bwindow\.location\s*=/.test(pageScripts),
  );

  if (!writeSums) {
    let sumsText = null;
    try {
      sumsText = await readFile(join(root, SUMS_FILE), 'utf8');
    } catch {
      check(`${SUMS_FILE} exists`, false, `run node validate.mjs --write-sums to create it`);
    }
    if (sumsText !== null) {
      const parsed = new Map();
      for (const line of sumsText.split(/\r?\n/).filter(Boolean)) {
        const match = /^([0-9a-f]{64}) {2}(.+)$/.exec(line);
        if (!match) {
          parsed.set(`invalid:${line}`, null);
          continue;
        }
        parsed.set(match[2], match[1]);
      }
      check(
        `${SUMS_FILE} lists exactly the payload files`,
        parsed.size === PAYLOAD_FILES.length && PAYLOAD_FILES.every((file) => parsed.has(file)),
        `got ${[...parsed.keys()].join(', ')}`,
      );
      for (const file of PAYLOAD_FILES) {
        check(
          `${SUMS_FILE} matches ${file}`,
          parsed.get(file) === hashes.get(file),
          `recorded ${parsed.get(file) ?? 'nothing'}, actual ${hashes.get(file)}`,
        );
      }
    }

    let readme = '';
    try {
      readme = await readFile(join(root, 'README.md'), 'utf8');
    } catch {
    }
    const staleReadme = PAYLOAD_FILES.filter(
      (file) => !readme.includes(`${hashes.get(file)}  ${file}`),
    );
    check(
      'README records the current fixture SHA256 values',
      readme.length > 0 && staleReadme.length === 0,
      readme.length === 0 ? 'README.md is missing' : `stale or missing: ${staleReadme.join(', ')}`,
    );
  }

  console.log('');
  console.log('fixture SHA256:');
  for (const file of PAYLOAD_FILES) {
    console.log(`  ${hashes.get(file)}  ${file}`);
  }
  console.log('');

  return manifest;
}

function browserCandidates() {
  const candidates = [];
  if (browserArg) candidates.push(browserArg);
  if (process.env.HAND_FIXTURE_BROWSER) candidates.push(process.env.HAND_FIXTURE_BROWSER);
  if (process.platform === 'win32') {
    const programFiles = process.env.ProgramFiles ?? 'C:\\Program Files';
    const programFilesX86 = process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)';
    const localAppData = process.env.LOCALAPPDATA ?? '';
    candidates.push(
      join(programFiles, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(programFilesX86, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(localAppData, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      join(programFiles, 'BraveSoftware', 'Brave-Browser-Nightly', 'Application', 'brave.exe'),
      join(programFiles, 'BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'),
      join(programFilesX86, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
      join(programFiles, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
    );
  } else {
    candidates.push(
      '/usr/bin/google-chrome',
      '/usr/bin/chromium',
      '/usr/bin/chromium-browser',
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    );
  }
  return [...new Set(candidates.filter(Boolean))];
}

function findBrowser() {
  if (browserArg) {
    if (!existsSync(browserArg)) {
      check('requested browser exists', false, browserArg);
      return null;
    }
    return browserArg;
  }
  for (const candidate of browserCandidates()) {
    if (existsSync(candidate)) return candidate;
  }
  check(
    'a Chromium-family browser is available',
    false,
    'pass --browser <path>, set HAND_FIXTURE_BROWSER, or use --static-only',
  );
  return null;
}

async function waitForFile(file, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (existsSync(file)) return true;
    await delay(50);
  }
  return false;
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), timeoutMs);
    child.once('exit', () => {
      clearTimeout(timer);
      resolve(true);
    });
  });
}

class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.nextId = 1;
    this.pending = new Map();
    this.waiters = [];
    this.closed = false;
    ws.addEventListener('message', (event) => this.onMessage(event));
    ws.addEventListener('close', () => this.onClose());
    ws.addEventListener('error', () => this.onClose());
  }

  static connect(url, timeoutMs = 10000) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      const timer = setTimeout(() => {
        try {
          ws.close();
        } catch {
        }
        reject(new Error('CDP WebSocket connect timeout'));
      }, timeoutMs);
      ws.addEventListener(
        'open',
        () => {
          clearTimeout(timer);
          resolve(new Cdp(ws));
        },
        { once: true },
      );
      ws.addEventListener(
        'error',
        () => {
          clearTimeout(timer);
          reject(new Error('CDP WebSocket connection failed'));
        },
        { once: true },
      );
    });
  }

  onMessage(event) {
    let message;
    try {
      const text = typeof event.data === 'string' ? event.data : Buffer.from(event.data).toString('utf8');
      message = JSON.parse(text);
    } catch {
      return;
    }

    if (message.id !== undefined) {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      clearTimeout(pending.timer);
      if (message.error) pending.reject(new Error(`${pending.method}: ${message.error.message}`));
      else pending.resolve(message.result);
      return;
    }

    if (message.method) {
      for (const waiter of [...this.waiters]) {
        if (waiter.method === message.method && waiter.sessionId === message.sessionId) {
          this.waiters.splice(this.waiters.indexOf(waiter), 1);
          clearTimeout(waiter.timer);
          waiter.resolve(message.params);
        }
      }
    }
  }

  onClose() {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error('CDP socket closed'));
    }
    this.pending.clear();
    for (const waiter of this.waiters) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error('CDP socket closed'));
    }
    this.waiters = [];
  }

  send(method, params = {}, sessionId, timeoutMs = 20000) {
    return new Promise((resolve, reject) => {
      if (this.closed) {
        reject(new Error('CDP socket closed'));
        return;
      }
      const id = this.nextId;
      this.nextId += 1;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: CDP timeout`));
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer, method });
      try {
        this.ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(error);
      }
    });
  }

  waitFor(method, sessionId, timeoutMs = 15000) {
    return new Promise((resolve, reject) => {
      const waiter = { method, sessionId, resolve: null, reject, timer: null };
      waiter.timer = setTimeout(() => {
        this.waiters = this.waiters.filter((entry) => entry !== waiter);
        reject(new Error(`${method}: event timeout`));
      }, timeoutMs);
      waiter.resolve = (params) => {
        clearTimeout(waiter.timer);
        resolve(params);
      };
      this.waiters.push(waiter);
    });
  }

  close() {
    try {
      this.ws.close();
    } catch {
    }
  }
}

async function runBrowserChecks(origin, manifest, requests) {
  const browserPath = findBrowser();
  if (!browserPath) return;

  const profile = await mkdtemp(join(tmpdir(), 'hand-fixture-'));
  let stderr = '';
  let child = null;
  let cdp = null;
  let sessionId = null;

  try {
    child = spawn(
      browserPath,
      [
        '--headless=new',
        '--disable-gpu',
        '--disable-extensions',
        '--disable-background-networking',
        '--disable-component-update',
        '--disable-sync',
        '--no-first-run',
        '--no-default-browser-check',
        '--metrics-recording-only',
        '--mute-audio',
        '--hide-crash-restore-bubble',
        `--user-data-dir=${profile}`,
        '--remote-debugging-port=0',
        '--window-size=1280,720',
        'about:blank',
      ],
      { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true },
    );
    child.stderr.on('data', (chunk) => {
      stderr = (stderr + chunk.toString()).slice(-8000);
    });
    child.on('error', (error) => {
      stderr = `${stderr}\nspawn error: ${error.message}`.slice(-8000);
    });

    const portFile = join(profile, 'DevToolsActivePort');
    if (!(await waitForFile(portFile, 15000))) {
      check('headless browser started', false, stderr.trim() || 'DevToolsActivePort was not written');
      return;
    }
    const [portLine] = (await readFile(portFile, 'utf8')).split(/\r?\n/);
    const debugPort = Number(portLine);
    if (!Number.isInteger(debugPort) || debugPort <= 0) {
      check('headless browser exposed a debugging port', false, `got ${JSON.stringify(portLine)}`);
      return;
    }

    let versionInfo = null;
    for (let attempt = 0; attempt < 100 && !versionInfo; attempt += 1) {
      try {
        const response = await fetch(`http://127.0.0.1:${debugPort}/json/version`, {
          signal: AbortSignal.timeout(1000),
        });
        if (response.ok) versionInfo = await response.json();
      } catch {
      }
      if (!versionInfo) await delay(50);
    }
    if (!versionInfo?.webSocketDebuggerUrl) {
      check('headless browser exposed a CDP endpoint', false, stderr.trim() || 'no /json/version response');
      return;
    }

    cdp = await Cdp.connect(versionInfo.webSocketDebuggerUrl);
    const version = await cdp.send('Browser.getVersion');
    console.log(`browser: ${version.product} (${version.revision}), protocol ${version.protocolVersion}`);
    if (stderr.trim()) console.log(`browser stderr (startup): ${stderr.trim().split(/\r?\n/).slice(-3).join(' | ')}`);

    const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' });
    const attached = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
    sessionId = attached.sessionId;
    await cdp.send('Page.enable', {}, sessionId);
    await cdp.send('Runtime.enable', {}, sessionId);
    await cdp.send(
      'Emulation.setDeviceMetricsOverride',
      { width: 1280, height: 720, deviceScaleFactor: 1, mobile: false },
      sessionId,
    );

    const navigate = async (url) => {
      const loaded = cdp.waitFor('Page.loadEventFired', sessionId, 15000);
      const result = await cdp.send('Page.navigate', { url }, sessionId);
      if (result.errorText) {
        loaded.catch(() => {});
        throw new Error(`navigation failed: ${result.errorText}`);
      }
      await loaded;
    };

    const evaluate = async (expression) => {
      const result = await cdp.send(
        'Runtime.evaluate',
        { expression, returnByValue: true, awaitPromise: true, userGesture: true },
        sessionId,
      );
      if (result.exceptionDetails) {
        const description =
          result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'unknown page error';
        throw new Error(`page evaluation failed: ${description}`);
      }
      return result.result.value;
    };

    await navigate(`${origin}/index.html`);
    await cdp.send('Page.bringToFront', {}, sessionId);
    const bench = await evaluate(`(async () => {
      await document.fonts.ready;
      const controls = [...document.querySelectorAll('[data-hand-control]')];
      const interactive = [...document.querySelectorAll('button, input, select, textarea, a[href], [tabindex], [role="button"], [role="link"], [role="checkbox"]')];
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const rows = controls.map((element) => {
        const rect = element.getBoundingClientRect();
        const style = getComputedStyle(element);
        const labels = element.labels ? [...element.labels].map((label) => (label.textContent || '').trim()).join(' ') : '';
        const name = (element.getAttribute('aria-label') || labels || (element.textContent || '').trim()).trim();
        const visible = rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0;
        const inside = rect.left >= 0 && rect.top >= 0 && rect.right <= vw && rect.bottom <= vh;
        return {
          id: element.id,
          label: element.getAttribute('data-label'),
          name,
          x: rect.x,
          y: rect.y,
          width: rect.width,
          height: rect.height,
          visible,
          inside
        };
      });
      return {
        url: location.href,
        viewport: { width: vw, height: vh },
        dpr: window.devicePixelRatio,
        scroll: { width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight },
        controlCount: controls.length,
        interactiveCount: interactive.length,
        bodyCount: document.body.dataset.controlCount,
        controls: rows,
        statusBefore: document.getElementById('status').textContent
      };
    })()`);

    check(
      'benchmark viewport is 1280x720 at DPR 1',
      bench.viewport.width === 1280 && bench.viewport.height === 720 && bench.dpr === 1,
      `got ${bench.viewport.width}x${bench.viewport.height} at DPR ${bench.dpr}`,
    );
    check(
      'benchmark document does not overflow the viewport',
      bench.scroll.width <= 1280 && bench.scroll.height <= 720,
      `scroll size ${bench.scroll.width}x${bench.scroll.height}`,
    );
    check(
      'benchmark page has exactly 100 marked controls',
      bench.controlCount === 100 && bench.bodyCount === '100',
      `selector count ${bench.controlCount}, body count ${bench.bodyCount}`,
    );
    check(
      'benchmark page has no extra interactive elements',
      bench.interactiveCount === 100,
      `interactive count ${bench.interactiveCount}`,
    );
    check(
      'benchmark control ids are control-001 through control-100',
      bench.controls.map((control) => control.id).join(',') === sequence('control-', 100, 3).join(','),
    );
    check(
      'benchmark control labels are Control 001 through Control 100',
      bench.controls.map((control) => control.label).join(',') === sequence('Control ', 100, 3).join(','),
    );
    const labelRe = new RegExp(manifest.benchmark.labelPattern);
    const badNames = bench.controls.filter(
      (control) => control.name.length === 0 || control.name !== control.label || !labelRe.test(control.name),
    );
    check(
      'every control has a non-empty accessible label matching its data-label',
      badNames.length === 0,
      badNames.slice(0, 5).map((control) => `${control.id}:${JSON.stringify(control.name)}`).join('; '),
    );
    check(
      'control labels are unique',
      new Set(bench.controls.map((control) => control.name)).size === 100,
      `got ${new Set(bench.controls.map((control) => control.name)).size} unique names`,
    );
    const notVisible = bench.controls.filter((control) => !control.visible || !control.inside);
    check(
      'every control is visible and fully inside the 1280x720 viewport',
      notVisible.length === 0,
      notVisible.slice(0, 5).map((control) => `${control.id} at ${control.x},${control.y} ${control.width}x${control.height}`).join('; '),
    );
    const badBoxes = bench.controls.filter(
      (control) => Math.abs(control.width - 112) > 0.5 || Math.abs(control.height - 48) > 0.5,
    );
    check(
      'every control has a fixed 112x48 CSS-pixel box',
      badBoxes.length === 0,
      badBoxes.slice(0, 5).map((control) => `${control.id}:${control.width}x${control.height}`).join('; '),
    );
    check(
      'benchmark status starts at the manifest initial text',
      bench.statusBefore === manifest.benchmark.initialStatus,
      `got ${JSON.stringify(bench.statusBefore)}`,
    );

    const screenshot = await cdp.send(
      'Page.captureScreenshot',
      { format: 'png', fromSurface: true, captureBeyondViewport: false },
      sessionId,
    );
    const png = Buffer.from(screenshot.data, 'base64');
    const pngSignature = png.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    const shotWidth = pngSignature ? png.readUInt32BE(16) : 0;
    const shotHeight = pngSignature ? png.readUInt32BE(20) : 0;
    check(
      'screenshot is a 1280x720 PNG',
      pngSignature && shotWidth === 1280 && shotHeight === 720,
      `got ${shotWidth}x${shotHeight}`,
    );

    const first = bench.controls.find((control) => control.id === 'control-001');
    if (!first) {
      check('control-001 is available for a real pointer click', false);
    } else {
      const x = first.x + first.width / 2;
      const y = first.y + first.height / 2;
      await cdp.send(
        'Input.dispatchMouseEvent',
        { type: 'mouseMoved', x, y, button: 'none', buttons: 0 },
        sessionId,
      );
      await cdp.send(
        'Input.dispatchMouseEvent',
        { type: 'mousePressed', x, y, button: 'left', buttons: 1, clickCount: 1 },
        sessionId,
      );
      await cdp.send(
        'Input.dispatchMouseEvent',
        { type: 'mouseReleased', x, y, button: 'left', buttons: 0, clickCount: 1 },
        sessionId,
      );
    }

    const activation = await evaluate(`(() => {
      const observed = document.getElementById('status').textContent;
      const expected = 'last activated: Control 002 (1)';
      return { observed, expected, verdict: observed === expected };
    })()`);
    check(
      'real pointer click on control-001 is delivered and verified',
      activation.observed === manifest.benchmark.activation.statusAfter,
      `got ${JSON.stringify(activation.observed)}`,
    );
    check(
      'deliberately false assertion fails with observed evidence',
      activation.verdict === false &&
        activation.expected === manifest.benchmark.negative.expected &&
        activation.observed === manifest.benchmark.negative.observed &&
        activation.observed === manifest.benchmark.activation.statusAfter,
      `expected ${JSON.stringify(activation.expected)}, observed ${JSON.stringify(activation.observed)}, verdict ${activation.verdict}`,
    );

    await navigate(`${origin}/actions.html`);
    const actionResult = await evaluate(`(async () => {
      await document.fonts.ready;
      const byId = (id) => document.getElementById(id);
      const text = (id) => byId(id).textContent;
      const snapshot = () => ({
        'bodyState': document.body.dataset.state,
        'result': text('result'),
        'noop-count': text('noop-count'),
        'counter-value': text('counter-value'),
        'name-value': byId('name-input').value,
        'greeting': text('greeting'),
        'mode-output': text('mode-output'),
        'flag-state': text('flag-state'),
        'flag-checked': byId('flag-check').checked,
        'dialog-state': text('dialog-state'),
        'dialog-open': byId('confirm-dialog').open,
        'scroll-offset': text('scroll-offset')
      });
      const reset = () => byId('reset').click();
      const checks = {};

      checks.initial = snapshot();

      byId('apply').click();
      checks.apply = snapshot();
      reset();

      byId('noop').click();
      checks.noop = snapshot();
      const negative = {
        expected: 'applied',
        observed: text('result'),
        verdict: text('result') === 'applied'
      };
      reset();

      byId('increment').click();
      byId('increment').click();
      checks.incrementTwice = snapshot();
      reset();

      byId('name-input').value = 'Ada';
      byId('apply-name').click();
      checks.applyNameAda = snapshot();
      reset();

      const select = byId('mode-select');
      select.value = 'beta';
      select.dispatchEvent(new Event('change', { bubbles: true }));
      checks.selectBeta = snapshot();
      reset();

      byId('flag-check').click();
      checks.checkFlag = snapshot();
      reset();

      byId('open-dialog').click();
      checks.openDialog = snapshot();
      byId('dialog-close').click();
      checks.closeDialog = snapshot();
      reset();

      const box = byId('scroll-box');
      box.scrollLeft = 60;
      box.scrollTop = 120;
      box.dispatchEvent(new Event('scroll'));
      checks.scrollTo60x120 = snapshot();
      reset();

      const response = await fetch('/missing.html', { cache: 'no-store' });
      checks.missing404 = { status: response.status, body: await response.text() };
      reset();

      checks.afterReset = snapshot();
      return { checks, negative };
    })()`);

    const initialDiff = mismatch(actionResult.checks.initial, manifest.actions.initial);
    check('actions page initial state matches the manifest', initialDiff === '', initialDiff);

    for (const effect of manifest.actions.effects) {
      const actual = actionResult.checks[effect.name];
      if (effect.name === 'missing404') {
        check(
          'actions missing404 returns HTTP 404 with a deterministic body',
          actual?.status === effect.expected.status && actual?.body === effect.expected.body,
          `got ${JSON.stringify(actual)}`,
        );
      } else if (!actual) {
        check(`actions ${effect.name} matches the manifest`, false, 'state was not returned');
      } else {
        const diff = mismatch(actual, effect.expected, true);
        check(`actions ${effect.name} matches the manifest`, diff === '', diff);
      }
    }

    const resetDiff = mismatch(actionResult.checks.afterReset, manifest.actions.initial);
    check('actions reset returns to the initial state', resetDiff === '', resetDiff);
    check(
      'actions no-op leaves result unchanged and the deliberately false assertion is false',
      actionResult.negative.verdict === false &&
        actionResult.negative.observed === manifest.actions.negative.observed,
      `expected ${JSON.stringify(actionResult.negative.expected)}, observed ${JSON.stringify(actionResult.negative.observed)}, verdict ${actionResult.negative.verdict}`,
    );

    const allowedPaths = new Set(['/', '/index.html', '/actions.html', '/manifest.json', '/missing.html']);
    const unexpectedRequests = requests.filter((url) => !allowedPaths.has(url));
    check(
      'browser requested only fixture payload paths and the deliberate 404',
      unexpectedRequests.length === 0,
      `unexpected requests: ${unexpectedRequests.join(', ')}`,
    );
  } catch (error) {
    check('headless browser validation completed', false, error?.stack ?? String(error));
    if (stderr.trim()) console.error(`browser stderr (tail): ${stderr.trim().split(/\r?\n/).slice(-10).join('\n')}`);
  } finally {
    if (cdp) {
      try {
        await cdp.send('Browser.close', {}, undefined, 3000);
      } catch {
      }
      cdp.close();
    }
    if (child && child.pid) {
      const exited = await waitForExit(child, 5000);
      if (!exited) {
        child.kill('SIGKILL');
        await waitForExit(child, 2000);
        if (process.platform === 'win32' && child.exitCode === null) {
          spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
          await delay(500);
        }
      }
    }
    try {
      await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
    } catch (error) {
      check('temporary browser profile removed', false, error?.message ?? String(error));
    }
  }
}

async function main() {
  let payload;
  try {
    payload = await loadPayload();
  } catch (error) {
    console.error(`could not read fixture payload: ${error.message}`);
    process.exitCode = 1;
    return;
  }
  const hashes = computeHashes(payload);

  let manifest;
  try {
    manifest = await runStaticChecks(payload, hashes);
  } catch (error) {
    console.error(`static validation failed: ${error.stack ?? error}`);
    process.exitCode = 1;
    return;
  }

  if (writeSums) {
    if (failures.length > 0) {
      console.error('refusing to write SHA256SUMS while static checks fail');
      process.exitCode = 1;
      return;
    }
    const body = `${PAYLOAD_FILES.map((file) => `${hashes.get(file)}  ${file}`).join('\n')}\n`;
    await writeFile(join(root, SUMS_FILE), body, 'utf8');
    console.log(`wrote ${SUMS_FILE}`);
    return;
  }

  if (failures.length > 0) {
    console.error('static checks failed; skipping headless browser checks');
    process.exitCode = 1;
    return;
  }

  if (staticOnly) {
    console.log('browser checks skipped (--static-only)');
  } else {
    const { server, origin } = await startFixtureServer({ port: 0 });
    const requests = [];
    server.on('request', (req) => requests.push(req.url ?? ''));
    try {
      await runBrowserChecks(origin, manifest, requests);
    } finally {
      await new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections?.();
      });
    }
  }

  console.log('');
  console.log(`summary: ${passes} passed, ${failures.length} failed`);
  if (failures.length > 0) {
    console.error('FAIL: fixture validation found problems');
    process.exitCode = 1;
  } else {
    console.log('PASS: fixture validation complete');
  }
}

main().catch((error) => {
  console.error(error?.stack ?? String(error));
  process.exitCode = 1;
});
