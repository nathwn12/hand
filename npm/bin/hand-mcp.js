#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { createHash } from 'node:crypto';
import { createWriteStream, existsSync, mkdirSync, renameSync, rmSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const exe = join(here, '..', 'hand.exe');
const manifestPath = join(here, '..', 'chromium-manifest.json');

if (!existsSync(exe)) {
  console.error(`hand-mcp: bundled binary missing at ${exe} — reinstall the package`);
  process.exit(1);
}

function fail(message) {
  console.error(`hand-mcp: ${message}`);
  process.exit(1);
}

function cacheRoot() {
  const override = process.env.GLADIUS_CHROMIUM_CACHE;
  if (override && override.trim()) return override.trim();
  const local = process.env.LOCALAPPDATA;
  if (local && local.trim()) return join(local.trim(), 'gladius', 'chromium');
  return join(homedir(), 'AppData', 'Local', 'gladius', 'chromium');
}

// The zip holds chrome-win64/ at its root, so it must be extracted INTO the
// versioned directory: that is the path find_chromium() checks. All output
// below is stderr — stdout belongs to the MCP protocol.
async function provision() {
  let manifest;
  try {
    manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  } catch (error) {
    fail(`chromium manifest missing or unreadable at ${manifestPath}: ${error.message}`);
  }
  const browser = manifest && manifest.browser;
  const asset = browser && browser.asset;
  if (!browser || !browser.version || !asset || !asset.url || !asset.sha256 || !Number.isFinite(Number(asset.size))) {
    fail(`chromium manifest at ${manifestPath} lacks browser.version or asset.url/sha256/size`);
  }
  const expectedSize = Number(asset.size);
  const expectedHash = String(asset.sha256).toLowerCase();
  const sha8 = expectedHash.slice(0, 8);
  const root = cacheRoot();
  const versioned = join(root, `${browser.version}-${sha8}`);
  const target = join(versioned, 'chrome-win64', 'chrome.exe');
  if (existsSync(target)) {
    console.error(`hand-mcp: chromium already provisioned at ${target}`);
    return;
  }

  const zip = join(root, `.tmp-${sha8}.zip`);
  const staging = join(root, `.stage-${sha8}`);
  mkdirSync(root, { recursive: true });
  rmSync(zip, { force: true });
  rmSync(staging, { recursive: true, force: true });

  try {
    console.error(`hand-mcp: downloading Chrome for Testing ${browser.version} from ${asset.url} (${expectedSize} bytes)`);
    const response = await fetch(asset.url);
    if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`);
    if (!response.body) throw new Error('response has no body');
    const declared = Number(response.headers.get('content-length'));
    if (declared && declared !== expectedSize) {
      throw new Error(`size mismatch: server declared ${declared} bytes, manifest expects ${expectedSize} bytes`);
    }

    const hash = createHash('sha256');
    const writer = createWriteStream(zip);
    let writeError = null;
    writer.on('error', (error) => { writeError = error; });
    let received = 0;
    let nextPercent = 10;
    for await (const chunk of response.body) {
      if (writeError) throw writeError;
      received += chunk.length;
      if (received > expectedSize) {
        throw new Error(`size mismatch: expected ${expectedSize} bytes, received more than ${expectedSize} bytes`);
      }
      hash.update(chunk);
      if (!writer.write(chunk)) await once(writer, 'drain');
      const percent = Math.floor((received * 100) / expectedSize);
      if (percent >= nextPercent) {
        console.error(`hand-mcp: download ${percent}% (${received}/${expectedSize} bytes)`);
        nextPercent = Math.floor(percent / 10) * 10 + 10;
      }
    }
    await new Promise((resolvePromise, rejectPromise) => writer.end((error) => error ? rejectPromise(error) : resolvePromise()));
    if (writeError) throw writeError;
    if (received !== expectedSize) {
      throw new Error(`size mismatch: expected ${expectedSize} bytes, received ${received} bytes`);
    }
    const actual = hash.digest('hex');
    if (actual !== expectedHash) {
      throw new Error(`sha256 mismatch: expected ${asset.sha256}, got ${actual}`);
    }
    console.error(`hand-mcp: sha256 verified ${actual} (${received} bytes)`);

    const tar = join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe');
    mkdirSync(staging, { recursive: true });
    const extraction = spawnSync(tar, ['-xf', zip, '-C', staging], { stdio: ['ignore', 'inherit', 'inherit'] });
    if (extraction.error || extraction.status !== 0) {
      throw new Error(`tar extraction failed: ${extraction.error ? extraction.error.message : 'exit ' + extraction.status}`);
    }
    if (!existsSync(join(staging, 'chrome-win64', 'chrome.exe'))) {
      throw new Error(`extraction did not produce chrome-win64/chrome.exe under ${staging}`);
    }
    if (existsSync(versioned)) {
      rmSync(staging, { recursive: true, force: true });
      if (!existsSync(target)) throw new Error(`cache directory ${versioned} exists but has no chrome-win64/chrome.exe`);
    } else {
      renameSync(staging, versioned);
    }
    rmSync(zip, { force: true });
    console.error(`hand-mcp: extracted Chrome for Testing ${browser.version} into ${versioned}`);
  } catch (error) {
    rmSync(zip, { force: true });
    rmSync(staging, { recursive: true, force: true });
    fail(`chromium provisioning failed from ${asset.url}: ${error.message}. The engine can also use an existing binary via GLADIUS_CHROMIUM.`);
  }
}

const args = process.argv.slice(2);
if (args[0] === 'provision') {
  await provision();
  process.exit(0);
}

const mcpMode = !args.length || args[0] === 'mcp';
if (mcpMode && !(process.env.GLADIUS_CHROMIUM || '').trim() && process.env.HAND_SKIP_PROVISION !== '1') {
  await provision();
}

const child = spawn(exe, args.length ? args : ['mcp'], { stdio: 'inherit' });

child.on('error', (error) => {
  console.error(`hand-mcp: could not start the bundled binary: ${error.message}`);
  process.exit(1);
});
child.on('exit', (code, signal) => {
  if (signal) {
    console.error(`hand-mcp: terminated by ${signal}`);
    process.exit(1);
  }
  process.exit(code ?? 1);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal));
}
