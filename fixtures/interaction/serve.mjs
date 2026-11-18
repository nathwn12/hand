#!/usr/bin/env node
// Two-origin loopback fixture server for the interaction surface.
//
//   node fixtures/interaction/serve.mjs
//
// Binds 127.0.0.1 on one port and `localhost` on another, and serves the same
// three pages from both. A page requested from one origin has its
// `__CROSS_ORIGIN__` placeholder replaced with the OTHER origin, so
// /oopif-parent.html always frames a genuinely cross-site document (different
// host, different site, different renderer once site isolation is on).
//
// Everything is loopback; no external host is ever contacted.

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';

const PAGES = new Set(['/pointer.html', '/oopif-parent.html', '/oopif-child.html']);
const NOT_FOUND = Buffer.from('404 not found\n', 'utf8');

/// `otherOrigin` is read at REQUEST time, so a page served before its sibling
/// listener finished still substitutes the final origin.
function start(host, otherOrigin) {
  return new Promise((resolve, reject) => {
    const server = createServer((req, res) => {
      const pathname = new URL(req.url ?? '/', `http://${host}`).pathname;
      if (!PAGES.has(pathname)) {
        res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
        res.end(NOT_FOUND);
        return;
      }
      readFile(fileURLToPath(new URL(`.${pathname}`, import.meta.url)), 'utf8')
        .then((body) => {
          res.writeHead(200, {
            'Content-Type': 'text/html; charset=utf-8',
            'Cache-Control': 'no-store',
          });
          res.end(body.split('__CROSS_ORIGIN__').join(otherOrigin()));
        })
        .catch(() => {
          res.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
          res.end('500 fixture unreadable\n');
        });
    });
    server.once('error', reject);
    server.listen(0, host, () => resolve({ server, port: server.address().port }));
  });
}

export async function startInteractionFixture() {
  const state = { a: '', b: '' };
  const a = await start('127.0.0.1', () => state.b);
  const b = await start('localhost', () => state.a);
  state.a = `http://127.0.0.1:${a.port}`;
  state.b = `http://localhost:${b.port}`;
  return {
    origins: { ...state },
    async close() {
      await Promise.all(
        [a, b].map((s) => new Promise((r) => s.server.close(() => r()))),
      );
    },
  };
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  const { origins, close } = await startInteractionFixture();
  console.log(`interaction fixture: A=${origins.a} B=${origins.b}`);
  console.log(`  pointer page: ${origins.a}/pointer.html`);
  console.log(`  cross-site iframe page: ${origins.a}/oopif-parent.html`);
  const stop = async () => {
    await close();
    process.exit(0);
  };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  setInterval(() => {}, 1 << 30);
}
