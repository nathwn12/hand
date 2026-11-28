#!/usr/bin/env node
// Usage:
//   node npm/release.mjs              # all three, in order: initial, publish, trust
//   node npm/release.mjs --initial    # npm login only (visible window; passkey)
//   node npm/release.mjs --publish    # gate, build, pack, handshake, publish, registry read-back
//                                     # --publish runs npm login itself when the session is absent
//   node npm/release.mjs --trust      # bind GitHub Actions as npm trusted publisher
//   node npm/release.mjs --check      # read-only artifact proof (preflight, build, pack, version, handshake, data: navigation; loopback advisory)
//   node npm/release.mjs --status     # read-only npm user, registry, tag and trust state
//   node npm/release.mjs <mode> --visible  # visible passkey window; parent waits for its exit marker
// publish is idempotent: a version already on the registry is a skip only when
// its gitHead names this source commit, never a second publish.
import { execFileSync, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..');
const pkg = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8'));
const WORKFLOW = 'publish.yml';
// Today's value is the fallback; a copied lane reads owner/repo from its own
// package.json.repository, so porting this script needs no edit here.
const GITHUB_REPO = (() => {
  const url = typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url;
  const match = /(?:github\.com\/|github:)([^/]+)\/([^/#]+)/i.exec(url ?? '');
  return match ? `${match[1]}/${match[2].replace(/\.git$/i, '')}` : 'nathwn12/hand';
})();

// npm's own registry client uses `?write=true` for read-after-write: the public
// registry revalidates its cache before replying. `npm view --prefer-online`
// reads a cached packument and can miss a just-published version for minutes.
const REGISTRY = 'https://registry.npmjs.org';
// Public run 45744131332: the publish succeeded (hand-mcp@0.3.4, gitHead
// 84af90f, provenance signed) but npm CDN replicas were still 404 after
// 18 x 5s = 90s, so read-back exited PENDING/code 2 and held the tag until a
// rerun. Widen the window to 36 x 5s = 180s so a version visible in ~2-3 min
// verifies in one run. Identity, conflict, pending and exit semantics are unchanged.
const READ_ATTEMPTS = 36;
const READ_DELAY_MS = 5000;

// Closed history, not a rule. hand-mcp@0.3.0 was published from 2065697...
// while the immutable v0.3.0 tag already dereferenced 8ae5914... . Both stay
// where they are. No other version or commit pair matches this exception.
export const HISTORICAL_RELEASE_IDENTITY = Object.freeze({
  '0.3.0': Object.freeze({
    tagCommit: '8ae5914f249d500f498255d070b704dfffab7daa',
    sourceCommit: '2065697ad7231f3a4604e3568eb58c1e35b7b7b8',
  }),
});

const argv = process.argv.slice(2);
const KNOWN = new Set(['--initial', '--publish', '--trust', '--check', '--status', '--visible', '--ci', '--skip-build']);
const unknown = argv.filter((a) => !KNOWN.has(a));
if (unknown.length) usage(`unknown flag: ${unknown.join(' ')}`);
const ci = argv.includes('--ci');
const skipBuild = argv.includes('--skip-build');
const visible = argv.includes('--visible');
const check = argv.includes('--check');
const status = argv.includes('--status');
const chosen = ['--initial', '--publish', '--trust'].filter((f) => argv.includes(f));
if ((check || status) && (chosen.length || ci)) usage('choose one mode: --check and --status are read-only');
if (ci && !(chosen.length === 1 && chosen[0] === '--publish')) usage('--ci belongs to --publish only');
if (visible && (check || status || ci)) usage('--visible belongs to --initial, --publish or --trust only');
const modes = check || status ? [] : ['--initial', '--publish', '--trust'].filter((f) => !chosen.length || chosen.includes(f));

function usage(line) {
  console.error(`FAIL usage: ${line}`);
  console.error('  node npm/release.mjs [--initial | --publish | --trust | --check | --status] [--skip-build] [--visible]');
  console.error('  no flags = initial, publish, trust in that order');
  console.error('  --visible opens a passkey window for initial, publish or trust');
  process.exit(64);
}
function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { cwd: repo, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], ...opts }).trim();
}
function npmInvocation(args) {
  return process.platform === 'win32'
    ? [process.env.ComSpec || 'cmd.exe', ['/d', '/c', 'npm', ...args]]
    : ['npm', args];
}
function npmOut(args, opts = {}) {
  const [cmd, full] = npmInvocation(args);
  return run(cmd, full, opts);
}
function npmExec(args, opts = {}) {
  const [cmd, full] = npmInvocation(args);
  return execFileSync(cmd, full, opts);
}
function fail(step, message, code = 1) {
  console.error(`FAIL ${step}: ${message}`);
  process.exit(code);
}
function step(name, fn) {
  try { return fn(); } catch (e) { fail(name, (e.stderr || e.message || String(e)).trim()); }
}
function npmUser() {
  try { return npmOut(['whoami'], { cwd: here }).split(/\r?\n/)[0].trim(); } catch { return null; }
}
function interpretVersionDocument(text, name, version) {
  let doc;
  try { doc = JSON.parse(text); } catch { return { status: 'unavailable', reason: 'registry JSON is unreadable' }; }
  const record = Array.isArray(doc) ? doc[0] : doc;
  if (!record || record.version !== version) return { status: 'absent', reason: `no record for ${name}@${version}` };
  const gitHead = typeof record.gitHead === 'string' ? record.gitHead.toLowerCase() : null;
  if (!gitHead || !/^[0-9a-f]{7,40}$/.test(gitHead)) {
    return { status: 'unavailable', reason: `${name}@${version} has no usable gitHead` };
  }
  return { status: 'present', gitHead, version: record.version };
}

// Three states, never two: "npm is down" must not become "version is free".
// `view` is injected so offline fixtures can drive the parse without a network.
export function registryVersion(view, name, version) {
  let text;
  try {
    text = view(['view', `${name}@${version}`, '--json', '--prefer-online']);
  } catch (error) {
    const detail = `${error?.stderr ?? ''}\n${error?.message ?? ''}`;
    const absent = /E404|No matching version found|is not in this registry|version not found/i.test(detail);
    return absent
      ? { status: 'absent', reason: detail.trim() }
      : { status: 'unavailable', reason: detail.trim() };
  }
  return interpretVersionDocument(text, name, version);
}

export function versionReadUrl(name, version, registry = REGISTRY) {
  return `${registry.replace(/\/+$/, '')}/${encodeURIComponent(name)}/${encodeURIComponent(version)}?write=true`;
}

// Authoritative read of one version. `?write=true` is the registry's
// read-after-write control; `cache-control: no-cache` stops a local or edge
// cache serving a stale copy. `fetchImpl` is injected so the read-back can be
// tested without a publish or a network.
export async function authoritativeVersion(fetchImpl, name, version, registry = REGISTRY) {
  let response;
  try {
    response = await fetchImpl(versionReadUrl(name, version, registry), {
      headers: { accept: 'application/json', 'cache-control': 'no-cache' },
    });
  } catch (error) {
    return { status: 'unavailable', reason: String(error?.message ?? error).trim() };
  }
  if (response.status === 404) return { status: 'absent', reason: 'registry returned 404' };
  if (!response.ok) return { status: 'unavailable', reason: `registry HTTP ${response.status}` };
  let text;
  try { text = await response.text(); } catch (error) {
    return { status: 'unavailable', reason: `registry body unreadable: ${String(error?.message ?? error).trim()}` };
  }
  return interpretVersionDocument(text, name, version);
}

const short = (sha) => (typeof sha === 'string' ? sha.slice(0, 8) : '(missing)');

// After publish there are three outcomes, not two. `verified` alone may tag;
// `conflict` is a real identity failure and `pending` (not visible or unreadable)
// is a resumable state: never republish, never tag an unverified SHA.
export function classifyPublishedVersion({ observed, expected }) {
  if (observed?.status === 'present') {
    return observed.gitHead === expected
      ? { kind: 'verified', gitHead: observed.gitHead }
      : { kind: 'conflict', gitHead: observed.gitHead, expected,
          reason: `registry shows ${observed.version} from ${short(observed.gitHead)}, not ${short(expected)}; refusing to tag` };
  }
  return { kind: 'pending', status: observed?.status ?? 'unavailable', reason: observed?.reason ?? 'no read-back' };
}

// Bounded eventual consistency. Terminal verdicts stop the loop; only `pending`
// waits, and the caller controls both delay and clock so fixtures stay instant.
export async function waitForPublishedVersion({
  read, expected, attempts = READ_ATTEMPTS, delayMs = READ_DELAY_MS,
  sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms)),
}) {
  let last;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    last = await read();
    const verdict = classifyPublishedVersion({ observed: last, expected });
    if (verdict.kind !== 'pending') return { ...verdict, reads: attempt };
    if (attempt < attempts) await sleep(delayMs);
  }
  return {
    kind: 'pending', reads: attempts, status: last?.status,
    reason: `not visible after ${attempts} authoritative reads (${(attempts * delayMs) / 1000}s): ${last?.reason ?? 'no read-back'}`,
  };
}

// An annotated tag needs its ^{} dereference, not the tag object id. An exact
// ls-remote pattern hides the ^{} line, so ask for the ref family and filter.
export function remoteTagCommit(lsRemote, tag) {
  let text;
  try {
    text = lsRemote(['ls-remote', '--tags', 'origin', `refs/tags/${tag}*`]);
  } catch (error) {
    return { status: 'unavailable', reason: String(error?.stderr ?? error?.message ?? error).trim() };
  }
  const cols = text.split(/\r?\n/).filter(Boolean).map((line) => line.split(/\s+/));
  const ref = `refs/tags/${tag}`;
  const deref = cols.find((c) => c[1] === `${ref}^{}`) ?? cols.find((c) => c[1] === ref);
  return deref && /^[0-9a-f]{40}$/i.test(deref[0])
    ? { status: 'present', commit: deref[0].toLowerCase() }
    : { status: 'absent', reason: `no remote tag ${tag}` };
}

// The registry's gitHead is the authority for a skip and for the commit a tag
// must name. A conflicting immutable tag is refused before any publish.
export function releaseIdentityDecision({ version, head, registry, tag, historical = HISTORICAL_RELEASE_IDENTITY }) {
  const fail = (reason) => ({ action: 'fail', tagMode: 'none', reason });
  if (registry.status === 'unavailable') return fail(`registry lookup failed: ${registry.reason}`);
  if (tag.status === 'unavailable') return fail(`tag lookup failed: ${tag.reason}`);

  const pinned = historical[version];
  if (pinned
    && registry.status === 'present' && registry.gitHead === pinned.sourceCommit
    && tag.status === 'present' && tag.commit === pinned.tagCommit) {
    return {
      action: 'skip', tagMode: 'leave', historical: true,
      sourceCommit: pinned.sourceCommit, tagCommit: pinned.tagCommit,
      reason: `v${version} is the closed historical identity exception: tag ${short(pinned.tagCommit)}, publish ${short(pinned.sourceCommit)}; neither moves`,
    };
  }

  if (registry.status === 'present') {
    // The published gitHead owns this version. A later push HEAD is never a
    // reason to republish or to move the tag onto an unrelated commit.
    const source = registry.gitHead;
    if (tag.status === 'present' && tag.commit !== source) {
      return fail(`immutable tag v${version} points at ${short(tag.commit)}, not published source ${short(source)}; refusing to retag`);
    }
    return { action: 'skip', tagMode: 'tag', sourceCommit: source, tagCommit: source,
      reason: `v${version} is already published from ${short(source)}${source !== head ? `; current HEAD ${short(head)} is not tagged` : ''}` };
  }
  if (tag.status === 'present' && tag.commit !== head) {
    return fail(`immutable tag v${version} points at ${short(tag.commit)}, not source ${short(head)}; refusing to publish or retag`);
  }
  return { action: 'publish', tagMode: 'tag', sourceCommit: head, tagCommit: head,
    reason: `v${version} is absent from the registry` };
}

function emitReleaseIdentity({ version, sourceCommit, tagCommit, tagMode }) {
  const fields = {
    releaseVersion: version,
    releaseSourceCommit: sourceCommit,
    releaseTagCommit: tagCommit,
    releaseTagMode: tagMode,
  };
  const out = process.env.GITHUB_OUTPUT;
  for (const [key, value] of Object.entries(fields)) {
    if (out) appendFileSync(out, `${key}=${value}\n`);
    else console.log(`${key}=${value}`);
  }
}
// The old helper hard-coded a directory that does not exist and hid the
// child's output. --visible runs this file in a real window, tees every stream
// to a log so the npm URL stays on screen, and waits for an exit marker.
function psQuote(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}
const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));

function printLogTail(logPath, count = 20) {
  if (!existsSync(logPath)) return console.log(`visible: no log yet at ${logPath}`);
  const lines = readFileSync(logPath, 'utf8').split(/\r?\n/).filter(Boolean);
  console.log(`visible: log tail (${lines.length} line${lines.length === 1 ? '' : 's'}) from ${logPath}:`);
  for (const line of lines.slice(-count)) console.log(`  ${line}`);
}

async function visibleRun() {
  const label = chosen.length === 1 ? chosen[0].slice(2) : 'all';
  const dir = join(tmpdir(), `hand-release-${label}-${process.pid}`);
  mkdirSync(dir, { recursive: true });
  const log = join(dir, 'child.log');
  const done = join(dir, 'done.txt');
  const child = join(dir, 'child.ps1');
  const childFlags = argv.filter((a) => a !== '--visible').join(' ');
  const script = [
    `Set-Location -LiteralPath ${psQuote(repo)}`,
    `& node ${psQuote(join(here, 'release.mjs'))} ${childFlags} *>&1 | Tee-Object -FilePath ${psQuote(log)}`,
    '$code = if ($null -eq $LASTEXITCODE) { 1 } else { $LASTEXITCODE }',
    `$code | Set-Content -LiteralPath ${psQuote(done)}`,
    '',
  ].join('\r\n');
  writeFileSync(child, script, 'utf8');
  console.log(`visible: window log ${log}`);
  console.log(`visible: exit marker ${done}`);
  console.log('visible: in that window the npm URL appears — click it, or press Enter if it asks to open the browser; approve with your passkey in 1Password. Nothing needs typing.');
  const launch = `Start-Process -FilePath powershell.exe -ArgumentList @('-NoExit','-NoProfile','-File',${psQuote(child)}) -WindowStyle Normal`;
  step('visible', () => execFileSync('powershell.exe', ['-NoProfile', '-Command', launch], { cwd: repo, stdio: 'inherit' }));
  const deadline = Date.now() + 20 * 60 * 1000;
  while (!existsSync(done) && Date.now() < deadline) await sleep(2000);
  printLogTail(log);
  if (!existsSync(done)) {
    console.error(`visible: 20-minute deadline passed; ${done} is still absent. The window is still open at ${log} and was NOT killed. Finish the passkey step there, then read the log or rerun.`);
    return 1;
  }
  const code = Number.parseInt(readFileSync(done, 'utf8').trim(), 10);
  return Number.isFinite(code) ? code : 1;
}
async function initial() {
  const user = npmUser();
  if (user) {
    console.log(`initial: already logged in as ${user} (skip)`);
    return;
  }
  if (!process.stdin.isTTY) fail('initial', 'npm login needs a visible terminal for the passkey: run this mode with --visible');
  console.log('initial: npm login (complete the browser passkey)');
  step('initial', () => npmExec(['login', '--auth-type=web'], { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));
  const after = npmUser();
  if (!after) fail('initial', 'npm login finished but npm whoami still fails');
  console.log(`initial: logged in as ${after}`);
}

function preflight({ allowDirty = false } = {}) {
  const head = step('preflight', () => run('git', ['rev-parse', 'HEAD'])).toLowerCase();
  if (!ci) {
    const branch = step('preflight', () => run('git', ['rev-parse', '--abbrev-ref', 'HEAD']));
    if (branch !== 'main') fail('preflight', `branch is ${branch}, not main. Check out main first: git checkout main`);
    const dirty = step('preflight', () => run('git', ['status', '--porcelain']));
    if (dirty) {
      if (!allowDirty) fail('preflight', `working tree is dirty. Commit it first, generated receipts included:\n${dirty}\n  git add -A; git commit -m "chore: record release receipts" -m "[skip ci]"`);
      console.log(`preflight: ${dirty.split(/\r?\n/).length} local changes; read-only check builds from the current files`);
    }
    const origin = step('preflight', () => run('git', ['rev-parse', 'origin/main']));
    if (head !== origin) fail('preflight', `HEAD ${head.slice(0, 8)} is not origin/main ${origin.slice(0, 8)}. Push or pull main first`);
  }

  const cargo = readFileSync(join(repo, 'Cargo.toml'), 'utf8');
  const cargoVersion = (cargo.match(/^version\s*=\s*"([^"]+)"/m) || [])[1];
  if (cargoVersion !== pkg.version) fail('preflight', `package ${pkg.version} and Cargo ${cargoVersion} disagree`);
  return head;
}

// One pack-and-prove path shared by --publish and --check. Nothing here
// logs in, publishes, tags or writes to the registry.
async function packAndVerify(head, noBuild = false) {
  if (!noBuild) {
    console.log(`build: cargo build --release (${pkg.version} at ${head.slice(0, 8)})`);
    step('build', () => execFileSync('cargo', ['build', '--release'], { cwd: repo, stdio: ['ignore', 'inherit', 'inherit'] }));
  }
  const work = mkdtempSync(join(tmpdir(), 'hand-release-'));
  const tarballName = step('pack', () => {
    const out = npmOut(['pack', '--pack-destination', work], { cwd: here });
    return out.split(/\r?\n/).filter(Boolean).pop();
  });
  const tarball = join(work, tarballName);
  if (!existsSync(tarball)) fail('pack', `no tarball at ${tarball}`);

  const extract = join(work, 'package-extract');
  step('extract', () => execFileSync('tar', ['-xzf', tarball, '-C', work], { stdio: 'ignore' }));
  const launcher = join(work, 'package', 'bin', 'hand-mcp.js');
  if (!existsSync(launcher)) fail('extract', 'launcher missing after extract');
  const version = step('version', () => run(process.execPath, [launcher, '--version'], { cwd: work }));
  if (!version.includes(head.slice(0, 8))) fail('version', `artifact identity does not match HEAD: ${version}`);

  // Prove the packed launcher can navigate, not only answer initialize/tools-list.
  // The loopback fixture is advisory: a VM network policy may abort loopback
  // navigation on this host, which says nothing about the shipped artifact.
  // The data: navigation is blocking: it needs no host networking and proves
  // the packaged browser really navigates. Both run inside the packed launcher.
  const fixtureBody = '<!doctype html><html><head><title>launcher http check</title></head><body><p id="probe">packaged launcher HTTP ok</p></body></html>';
  const fixture = createServer((req, res) => {
    res.writeHead(200, {
      'Content-Type': 'text/html; charset=utf-8',
      'Content-Length': Buffer.byteLength(fixtureBody),
      'Cache-Control': 'no-store',
    });
    res.end(fixtureBody);
  });
  await new Promise((ok, bad) => { fixture.once('error', bad); fixture.listen(0, '127.0.0.1', ok); });
  const fixtureUrl = 'http://127.0.0.1:' + fixture.address().port + '/probe';
  // Plain ASCII survives percent-encoding, so the returned URL can be checked
  // without depending on Chrome's canonicalisation of the rest of the payload.
  const DATA_MARKER = 'handlauncherdataok';
  const dataUrl = 'data:text/html,' + encodeURIComponent('<!doctype html><title>' + DATA_MARKER + '</title><p id="probe">packaged launcher data ok</p>');
  const navUrl = (body) => {
    const sc = body?.structuredContent;
    if (sc && typeof sc === 'object') return sc.url;
    try { return JSON.parse(typeof sc === 'string' ? sc : (body?.content?.[0]?.text ?? 'null'))?.url; } catch { return undefined; }
  };
  const navText = (body) => {
    const blocks = body?.content;
    if (Array.isArray(blocks)) {
      const text = blocks.map((b) => b?.text ?? '').filter(Boolean).join(' ').trim();
      if (text) return text;
    }
    const sc = body?.structuredContent;
    return typeof sc === 'string' ? sc.trim() : (sc && typeof sc === 'object' ? JSON.stringify(sc) : '');
  };

  const handshake = await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(process.execPath, [launcher], { cwd: work, stdio: ['pipe', 'pipe', 'pipe'] });
    let buf = '';
    const seen = [];
    // Chrome's first launch on a fresh runner takes longer than the old 15s
    // MCP-only bound; the timer covers a real navigation, not just a reply.
    const timer = setTimeout(() => { child.kill(); rejectPromise(new Error('handshake timed out')); }, 120000);
    child.stdout.on('data', (d) => {
      buf += d.toString('utf8');
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
        if (line) {
          try {
            const msg = JSON.parse(line);
            seen.push(msg);
            if (msg.id === 4) child.stdin.end();
          } catch { /* non-JSON banner */ }
        }
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      const init = seen.find((m) => m.id === 1);
      const list = seen.find((m) => m.id === 2);
      const loop = seen.find((m) => m.id === 3);
      const data = seen.find((m) => m.id === 4);
      resolvePromise({
        code,
        protocol: init?.result?.protocolVersion,
        tools: list?.result?.tools?.length,
        names: (list?.result?.tools ?? []).map((t) => t.name),
        loopbackError: loop?.result?.isError === true,
        loopbackUrl: navUrl(loop?.result),
        loopbackText: navText(loop?.result),
        dataError: data?.result?.isError === true,
        dataUrl: navUrl(data?.result),
        dataText: navText(data?.result),
      });
    });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'release-check', version: '0.0.1' } } }) + '\n');
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n'), 200);
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n'), 400);
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'browser_navigate', arguments: { url: fixtureUrl } } }) + '\n'), 800);
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'browser_navigate', arguments: { url: dataUrl } } }) + '\n'), 1600);
  });
  await new Promise((ok) => fixture.close(ok));
  // The 35-name compatibility floor, in catalog order. Additions raise the
  // count; nothing in this list may be dropped.
  const BASELINE_TOOLS = [
    'browser_navigate', 'browser_snapshot', 'browser_screenshot', 'browser_click',
    'browser_type', 'browser_evaluate', 'browser_wait_for', 'browser_targets',
    'browser_dialog', 'browser_history', 'browser_network', 'browser_console',
    'browser_storage', 'browser_tabs', 'browser_viewport', 'browser_select',
    'browser_hover', 'browser_key', 'browser_upload', 'browser_layout',
    'browser_assert_layout', 'browser_visual', 'browser_cdp', 'browser_close',
    'browser_fetch_text', 'browser_context', 'browser_wheel', 'browser_drag',
    'desktop_screenshot', 'desktop_diff', 'desktop_list_windows', 'desktop_window',
    'desktop_do', 'desktop_cursor', 'desktop_elements',
  ];
  if (handshake.code !== 0) fail('handshake', `exit ${handshake.code}`);
  if (!handshake.protocol) fail('initialize', 'packaged launcher did not answer initialize');
  if (handshake.tools < BASELINE_TOOLS.length) fail('tools/list', `expected at least ${BASELINE_TOOLS.length} tools, got ${handshake.tools}`);
  const missing = BASELINE_TOOLS.filter((name) => !handshake.names.includes(name));
  if (missing.length) fail('tools/list', `baseline tools missing: ${missing.join(', ')}`);
  if (handshake.dataError || typeof handshake.dataUrl !== 'string'
    || !handshake.dataUrl.startsWith('data:text/html,') || !handshake.dataUrl.includes(DATA_MARKER)) {
    fail('data', 'packaged launcher did not navigate the data: URL: isError=' + handshake.dataError
      + ' url=' + JSON.stringify(handshake.dataUrl) + ' detail=' + JSON.stringify(handshake.dataText));
  }
  // Loopback is recorded but never blocks a release: only a data: URL is a
  // host-independent proof of the packaged browser's navigation.
  const loopbackOk = !handshake.loopbackError && handshake.loopbackUrl === fixtureUrl;
  const loopbackDetail = loopbackOk ? null
    : (handshake.loopbackText || (handshake.loopbackError ? 'isError=true' : 'url mismatch'));
  console.log(`loopback: url=${JSON.stringify(handshake.loopbackUrl)} error=${JSON.stringify(loopbackDetail)}`);
  if (!loopbackOk) console.log('environmental: loopback navigation aborted on this host');

  console.log(`check: ${tarballName} ${pkg.version} ${head.slice(0, 8)} tools=${handshake.tools} protocol=${handshake.protocol} data=${handshake.dataUrl} loopback=${loopbackOk ? 'ok' : 'environmental'} exit=${handshake.code}`);
  const bytes = statSync(tarball).size;
  const sha256 = createHash('sha256').update(readFileSync(tarball)).digest('hex');
  const files = step('pack-list', () => run('tar', ['-tzf', tarball], { cwd: work })).split(/\r?\n/).filter(Boolean);
  return { work, tarballName, tarball, version, handshake, bytes, sha256, files };
}

async function checkMode() {
  const head = preflight({ allowDirty: true });
  const packed = await packAndVerify(head, skipBuild);
  console.log(`check: tarball ${packed.tarballName} bytes=${packed.bytes} sha256=${packed.sha256}`);
  console.log(`check: packed files (${packed.files.length}):`);
  for (const name of packed.files) console.log(`  ${name}`);
  rmSync(packed.work, { recursive: true, force: true });
  console.log('check: all proofs passed; no login, no publish, no tag, no registry write');
}

// `npm trust list` may ask for a passkey. That is a finding, never a status
// failure: --status stays read-only even when nobody is at the keyboard.
function npmTrustList() {
  let text;
  try {
    text = npmOut(['trust', 'list', pkg.name], { cwd: here, timeout: 20000 });
  } catch (error) {
    const detail = `${error?.stderr ?? ''}\n${error?.message ?? ''}`;
    const last = detail.trim().split(/\r?\n/).filter(Boolean).slice(-1)[0] ?? 'npm trust list failed';
    if (/passkey|one-time password|\botp\b|browser|sign in|log in|auth/i.test(detail)) return { status: 'needs-passkey', reason: last };
    if (/E404|not found|no trusted|no publisher/i.test(detail)) return { status: 'absent', reason: 'no trusted publisher listed' };
    return { status: 'unavailable', reason: last };
  }
  const lower = text.toLowerCase();
  if (!lower.includes(GITHUB_REPO.toLowerCase()) || !lower.includes(WORKFLOW.toLowerCase())) {
    return { status: 'absent', reason: 'listed, but not bound to this workflow and repository' };
  }
  return { status: 'bound', text };
}

async function statusMode() {
  const user = npmUser();
  console.log(`status: npm user ${user ?? 'not logged in'} (npm whoami, no passkey)`);

  const registry = await authoritativeVersion(fetch, pkg.name, pkg.version);
  if (registry.status === 'present') console.log(`status: registry ${pkg.name}@${pkg.version} present, gitHead ${registry.gitHead}`);
  else console.log(`status: registry ${pkg.name}@${pkg.version} ${registry.status}${registry.reason ? ` (${registry.reason})` : ''}`);

  const tag = step('status', () => remoteTagCommit((args) => run('git', args), `v${pkg.version}`));
  console.log(`status: remote tag v${pkg.version} ${tag.status === 'present' ? tag.commit : tag.status}${tag.reason ? ` (${tag.reason})` : ''}`);

  const trust = npmTrustList();
  if (trust.status === 'needs-passkey') console.log(`status: trust: needs-passkey — ${trust.reason}; read-only finding, not a failure`);
  else if (trust.status === 'bound') console.log(`status: trust: bound to ${GITHUB_REPO} ${WORKFLOW}`);
  else console.log(`status: trust: ${trust.status} — ${trust.reason}`);

  const conflicts = [];
  // The closed historical identity is a disagreement by design, not a conflict.
  const pinned = HISTORICAL_RELEASE_IDENTITY[pkg.version];
  const historical = pinned && registry.status === 'present' && registry.gitHead === pinned.sourceCommit
    && tag.status === 'present' && tag.commit === pinned.tagCommit;
  if (!historical && registry.status === 'present' && tag.status === 'present' && registry.gitHead !== tag.commit) {
    conflicts.push(`registry gitHead ${registry.gitHead} and tag v${pkg.version} ${tag.commit} disagree`);
  }
  if (registry.status === 'absent' && tag.status === 'present') {
    conflicts.push(`tag v${pkg.version} exists but the version is not on the registry`);
  }
  if (conflicts.length) fail('status', conflicts.join('; '));
  console.log('status: no contradictions');
}
async function publishMode() {
  const head = preflight();
  const registry = await authoritativeVersion(fetch, pkg.name, pkg.version);
  const tag = step('identity', () => remoteTagCommit((args) => run('git', args), `v${pkg.version}`));
  const decision = releaseIdentityDecision({ version: pkg.version, head, registry, tag });
  if (decision.action === 'fail') fail('identity', decision.reason);
  if (decision.action === 'skip') {
    emitReleaseIdentity({ version: pkg.version, ...decision });
    console.log(`publish: ${decision.reason}; nothing to publish, tag mode ${decision.tagMode}`);
    return;
  }

  if (ci) {
    console.log('publish: OIDC trusted publishing (no user token)');
  } else {
    let user = npmUser();
    if (!user) {
      if (!process.stdin.isTTY) fail('publish', 'not logged in: run this mode with --visible');
      console.log('publish: npm login first (complete the browser passkey)');
      step('publish', () => npmExec(['login', '--auth-type=web'], { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));
      user = npmUser();
      if (!user) fail('publish', 'npm login finished but npm whoami still fails');
    }
    console.log(`publish: authenticated as ${user}`);
  }

  if (!skipBuild) {
    console.log(`build: cargo build --release (${pkg.version} at ${head.slice(0, 8)})`);
    step('build', () => execFileSync('cargo', ['build', '--release'], { cwd: repo, stdio: ['ignore', 'inherit', 'inherit'] }));
  }

  const packed = await packAndVerify(head, skipBuild);

  console.log('publish: npm publish --access public (a passkey URL may appear)');
  const publishArgs = ['publish', '--access', 'public'];
  if (ci) publishArgs.push('--provenance');
  step('publish', () => npmExec(publishArgs, { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));

  // Publish accepting is not the registry showing it. Read through `?write=true`
  // so a cached packument cannot report a false miss, then classify: only a
  // verified gitHead may tag; a conflict fails and pending stays resumable.
  const verdict = await waitForPublishedVersion({
    read: () => authoritativeVersion(fetch, pkg.name, pkg.version),
    expected: head,
  });
  rmSync(packed.work, { recursive: true, force: true });
  if (verdict.kind === 'conflict') fail('identity', verdict.reason);
  if (verdict.kind !== 'verified') {
    console.error(`PENDING registry: ${pkg.name}@${pkg.version} was accepted, but ${verdict.reason}.`);
    console.error(`check ${versionReadUrl(pkg.name, pkg.version)} before announcing anything; a retry resumes with a skip and a verified tag`);
    process.exitCode = 2;
    return;
  }
  emitReleaseIdentity({ version: pkg.version, sourceCommit: verdict.gitHead, tagCommit: verdict.gitHead, tagMode: 'tag' });
  console.log(`published: ${pkg.name}@${pkg.version} from ${verdict.gitHead.slice(0, 8)} (authoritative read-back, ${verdict.reads} read${verdict.reads === 1 ? '' : 's'})`);
}

async function trustMode() {
  const user = npmUser();
  if (!user) fail('trust', 'not logged in: run --initial with --visible first');
  if (!process.stdin.isTTY) fail('trust', 'npm trust needs a visible terminal for the passkey: run this mode with --visible');
  console.log(`trust: npm trust github ${pkg.name} --file ${WORKFLOW} --repository ${GITHUB_REPO} --allow-publish`);
  step('trust', () => npmExec(['trust', 'github', pkg.name, '--file', WORKFLOW, '--repository', GITHUB_REPO, '--allow-publish', '-y'], { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));

  let listing;
  try {
    listing = npmOut(['trust', 'list', pkg.name], { cwd: here });
  } catch {
    console.log('trust: npm asks for a passkey to read the list; complete it here');
    step('trust', () => npmExec(['trust', 'list', pkg.name], { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));
    listing = step('trust', () => npmOut(['trust', 'list', pkg.name], { cwd: here }));
  }
  console.log(listing.trim());
  const lower = listing.toLowerCase();
  if (!lower.includes(GITHUB_REPO.toLowerCase()) || !lower.includes(WORKFLOW.toLowerCase())) {
    fail('trust', `trust list does not name ${GITHUB_REPO} ${WORKFLOW}`);
  }
  console.log(`trust: ${GITHUB_REPO} ${WORKFLOW} may publish ${pkg.name}`);
}

// A test imports this module namespace to drive the pure identity helpers.
// Only a direct invocation may run a publish, tag or login ceremony.
const invokedDirectly = process.argv[1]
  && resolve(process.argv[1]).toLowerCase() === fileURLToPath(import.meta.url).toLowerCase();
if (invokedDirectly) {
  if (visible && !process.stdin.isTTY) {
    process.exitCode = await visibleRun();
  } else {
    if (visible) console.log('visible: stdin is a TTY; running in place, no window needed');
    if (check) await checkMode();
    else if (status) await statusMode();
    else {
      for (const mode of modes) {
        if (mode === '--initial') await initial();
        if (mode === '--publish') await publishMode();
        if (mode === '--trust') await trustMode();
      }
    }
  }
}
