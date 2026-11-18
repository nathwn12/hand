#!/usr/bin/env node
/**
 * hand-mcp release: build, pack, prove, publish, verify.
 *
 *   node npm/release.mjs --check     build, pack, extract, --version, handshake
 *   node npm/release.mjs --publish   the same, then npm publish and registry check
 *
 * The check is the gate. Publish only runs after every check passed in the same
 * process, so a green publish cannot skip a red proof. No secret is printed.
 */
import { execFileSync, spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..');
const publish = process.argv.includes('--publish');
const skipBuild = process.argv.includes('--skip-build');

function run(cmd, args, opts = {}) {
  return execFileSync(cmd, args, { cwd: repo, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], ...opts }).trim();
}
function fail(step, message, code = 1) {
  console.error(`FAIL ${step}: ${message}`);
  process.exit(code);
}
function step(name, fn) {
  try { return fn(); } catch (e) { fail(name, (e.stderr || e.message || String(e)).trim()); }
}

// 1. Preflight: the release comes from main, and main is what the registry will see.
//    --ci skips the branch and cleanliness checks, because a workflow runs on main
//    by definition and checks out a merge ref; the identity check still runs.
const ci = process.argv.includes('--ci');
const head = step('preflight', () => run('git', ['rev-parse', 'HEAD']));
if (!ci) {
  const branch = step('preflight', () => run('git', ['rev-parse', '--abbrev-ref', 'HEAD']));
  if (branch !== 'main') fail('preflight', `branch is ${branch}, not main`);
  const dirty = step('preflight', () => run('git', ['status', '--porcelain']));
  if (dirty) fail('preflight', `working tree is dirty:\n${dirty}`);
  const origin = step('preflight', () => run('git', ['rev-parse', 'origin/main']));
  if (head !== origin) fail('preflight', `HEAD ${head.slice(0, 8)} is not origin/main ${origin.slice(0, 8)}`);
}

const pkg = JSON.parse(readFileSync(join(here, 'package.json'), 'utf8'));
const cargo = readFileSync(join(repo, 'Cargo.toml'), 'utf8');
const cargoVersion = (cargo.match(/^version\s*=\s*"([^"]+)"/m) || [])[1];
if (!cargoVersion || cargoVersion !== pkg.version) fail('preflight', `package ${pkg.version} and Cargo ${cargoVersion} disagree`);

// 2. Build the exact tree main points at.
if (!skipBuild) {
  console.log(`build: cargo build --release (${pkg.version} at ${head.slice(0, 8)})`);
  step('build', () => execFileSync('cargo', ['build', '--release'], { cwd: repo, stdio: ['ignore', 'inherit', 'inherit'] }));
}

// 3. Pack and extract, then prove the artifact from its own bytes.
const work = mkdtempSync(join(tmpdir(), 'hand-release-'));
const tarballName = step('pack', () => {
  const out = run('npm', ['pack', '--pack-destination', work], { cwd: here });
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

// 4. A real MCP handshake from the extracted package, then a clean exit.
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
    resolvePromise({ code, protocol: init?.result?.protocolVersion, tools: list?.result?.tools?.length });
  });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'release-check', version: '0.0.1' } } }) + '\n');
  setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n'), 200);
  setTimeout(() => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }) + '\n'), 400);
  setTimeout(() => child.stdin.end(), 1600);
});
if (handshake.code !== 0) fail('handshake', `exit ${handshake.code}`);
if (handshake.tools !== 35) fail('handshake', `expected 35 tools, got ${handshake.tools}`);

console.log(`check: ${tarballName} ${pkg.version} ${head.slice(0, 8)} tools=${handshake.tools} protocol=${handshake.protocol} exit=${handshake.code}`);

// 5. Publish only after the checks above, then make the registry prove it.
if (publish) {
  step('whoami', () => run('npm', ['whoami'], { cwd: here }));
  console.log('publish: npm publish --access public (a passkey URL may appear)');
  const publishArgs = ['publish', '--access', 'public'];
  if (ci) publishArgs.push('--provenance');
  step('publish', () => execFileSync('npm', publishArgs, { cwd: here, stdio: ['inherit', 'inherit', 'inherit'] }));
  const live = step('registry', () => run('npm', ['view', `${pkg.name}@${pkg.version}`, 'version'], { cwd: here }));
  if (live !== pkg.version) fail('registry', `registry reports ${live}, manifest says ${pkg.version}`);
  console.log(`published: ${pkg.name}@${live}`);
}

rmSync(work, { recursive: true, force: true });
