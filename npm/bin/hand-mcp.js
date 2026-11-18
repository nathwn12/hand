#!/usr/bin/env node
/**
 * hand-mcp — the npm launcher for the bundled Windows binary.
 *
 * Why a launcher and not a postinstall download: the executable ships inside the
 * tarball, so `npx hand-mcp` works with no network, no release asset to resolve,
 * and nothing to verify at run time. 14 MB is the honest cost of that.
 *
 * It forwards argv, stdio and the exit code, so `hand-mcp doctor`, `--version`
 * and a plain MCP session behave exactly like running hand.exe directly.
 */
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const exe = join(here, '..', 'hand.exe');

if (!existsSync(exe)) {
  console.error(`hand-mcp: bundled binary missing at ${exe} — reinstall the package`);
  process.exit(1);
}

const args = process.argv.slice(2);
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

// A client that closes the pipe should stop the server too, the same way hand
// stops on stdin close when it is spawned directly.
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal));
}
