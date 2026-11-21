#!/usr/bin/env node
// Usage:
//   node npm/release.mjs              # all three, in order: initial, publish, trust
//   node npm/release.mjs --initial    # npm login only (visible window; passkey)
//   node npm/release.mjs --publish    # gate, build, pack, handshake, publish, registry read-back
//                                     # --publish runs npm login itself when the session is absent
//   node npm/release.mjs --trust      # bind GitHub Actions as npm trusted publisher
// publish is idempotent: a version already on the registry is a skip, never a second publish.
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..');
const pkg = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8'));
const WORKFLOW = 'publish.yml';
const GITHUB_REPO = 'nathwn12/hand';

const argv = process.argv.slice(2);
const KNOWN = new Set(['--initial', '--publish', '--trust', '--ci', '--skip-build']);
const unknown = argv.filter((a) => !KNOWN.has(a));
if (unknown.length) usage(`unknown flag: ${unknown.join(' ')}`);
const ci = argv.includes('--ci');
const skipBuild = argv.includes('--skip-build');
const chosen = ['--initial', '--publish', '--trust'].filter((f) => argv.includes(f));
if (ci && !(chosen.length === 1 && chosen[0] === '--publish')) usage('--ci belongs to --publish only');
const modes = ['--initial', '--publish', '--trust'].filter((f) => !chosen.length || chosen.includes(f));

function usage(line) {
  console.error(`FAIL usage: ${line}`);
  console.error('  node npm/release.mjs [--initial] [--publish] [--trust] [--skip-build]');
  console.error('  no flags = initial, publish, trust in that order');
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
function registryHas(name, version) {
  try { return npmOut(['view', `${name}@${version}`, 'version', '--prefer-online'], { cwd: here }) === version; } catch { return false; }
}
function visibleWindow(mode) {
  return `Start-Process powershell.exe -NoExit -Command "cd Q:\\PROJECTS\\PERSONAL\\hand; node npm/release.mjs ${mode}"`;
}

async function initial() {
  const user = npmUser();
  if (user) {
    console.log(`initial: already logged in as ${user} (skip)`);
    return;
  }
  if (!process.stdin.isTTY) fail('initial', `npm login needs a visible terminal for the passkey. Run: ${visibleWindow('--initial')}`);
  console.log('initial: npm login (complete the browser passkey)');
  step('initial', () => npmExec(['login', '--auth-type=web'], { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));
  const after = npmUser();
  if (!after) fail('initial', 'npm login finished but npm whoami still fails');
  console.log(`initial: logged in as ${after}`);
}

async function publishMode() {
  const head = step('preflight', () => run('git', ['rev-parse', 'HEAD']));
  if (!ci) {
    const branch = step('preflight', () => run('git', ['rev-parse', '--abbrev-ref', 'HEAD']));
    if (branch !== 'main') fail('preflight', `branch is ${branch}, not main. Check out main first: git checkout main`);
    const dirty = step('preflight', () => run('git', ['status', '--porcelain']));
    if (dirty) fail('preflight', `working tree is dirty. Commit it first, generated receipts included:\n${dirty}\n  git add -A; git commit -m "chore: record release receipts" -m "[skip ci]"`);
    const origin = step('preflight', () => run('git', ['rev-parse', 'origin/main']));
    if (head !== origin) fail('preflight', `HEAD ${head.slice(0, 8)} is not origin/main ${origin.slice(0, 8)}. Push or pull main first`);
  }

  const cargo = readFileSync(join(repo, 'Cargo.toml'), 'utf8');
  const cargoVersion = (cargo.match(/^version\s*=\s*"([^"]+)"/m) || [])[1];
  if (cargoVersion !== pkg.version) fail('preflight', `package ${pkg.version} and Cargo ${cargoVersion} disagree`);

  if (registryHas(pkg.name, pkg.version)) {
    console.log(`publish: ${pkg.name}@${pkg.version} is already on the registry (skip, nothing to publish)`);
    return;
  }

  if (ci) {
    console.log('publish: OIDC trusted publishing (no user token)');
  } else {
    let user = npmUser();
    if (!user) {
      if (!process.stdin.isTTY) fail('publish', `not logged in. Run: ${visibleWindow('--publish')}`);
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

  const handshake = await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(process.execPath, [launcher], { cwd: work, stdio: ['pipe', 'pipe', 'pipe'] });
    let buf = '';
    const seen = [];
    const timer = setTimeout(() => { child.kill(); rejectPromise(new Error('handshake timed out')); }, 15000);
    child.stdout.on('data', (d) => {
      buf += d.toString('utf8');
      let i;
      while ((i = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
        if (line) { try { seen.push(JSON.parse(line)); } catch { /* non-JSON banner */ } }
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      const init = seen.find((m) => m.id === 1);
      const list = seen.find((m) => m.id === 2);
      resolvePromise({
        code,
        protocol: init?.result?.protocolVersion,
        tools: list?.result?.tools?.length,
        names: (list?.result?.tools ?? []).map((t) => t.name),
      });
    });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'release-check', version: '0.0.1' } } }) + '\n');
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n'), 200);
    setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n'), 400);
    setTimeout(() => child.stdin.end(), 1600);
  });
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
  if (handshake.tools < BASELINE_TOOLS.length) fail('handshake', `expected at least ${BASELINE_TOOLS.length} tools, got ${handshake.tools}`);
  const missing = BASELINE_TOOLS.filter((name) => !handshake.names.includes(name));
  if (missing.length) fail('handshake', `baseline tools missing: ${missing.join(', ')}`);

  console.log(`check: ${tarballName} ${pkg.version} ${head.slice(0, 8)} tools=${handshake.tools} protocol=${handshake.protocol} exit=${handshake.code}`);

  console.log('publish: npm publish --access public (a passkey URL may appear)');
  const publishArgs = ['publish', '--access', 'public'];
  if (ci) publishArgs.push('--provenance');
  step('publish', () => npmExec(publishArgs, { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));

  // The publish command accepting is not the registry showing it. Propagate, then verify.
  let live = null;
  for (let attempt = 0; attempt < 18; attempt += 1) {
    try { live = npmOut(['view', `${pkg.name}@${pkg.version}`, 'version', '--prefer-online'], { cwd: here }); } catch { live = null; }
    if (live === pkg.version) break;
    if (attempt < 17) await new Promise((r) => setTimeout(r, 5000));
  }
  if (live !== pkg.version) {
    console.error(`PENDING registry: ${pkg.name}@${pkg.version} was accepted, but the registry has not shown it after 90s.`);
    console.error(`check https://registry.npmjs.org/${pkg.name}/${pkg.version} before announcing anything`);
    process.exitCode = 2;
  } else {
    console.log(`published: ${pkg.name}@${live}`);
  }
  rmSync(work, { recursive: true, force: true });
}

async function trustMode() {
  const user = npmUser();
  if (!user) fail('trust', `not logged in. Run: ${visibleWindow('--initial')}`);
  if (!process.stdin.isTTY) fail('trust', `npm trust needs a visible terminal for the passkey. Run: ${visibleWindow('--trust')}`);
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

for (const mode of modes) {
  if (mode === '--initial') await initial();
  if (mode === '--publish') await publishMode();
  if (mode === '--trust') await trustMode();
}
