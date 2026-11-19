#!/usr/bin/env node
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

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal));
}
