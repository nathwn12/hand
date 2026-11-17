import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));

const ALLOWED = new Map([
  ['/', { file: 'index.html', type: 'text/html; charset=utf-8' }],
  ['/index.html', { file: 'index.html', type: 'text/html; charset=utf-8' }],
  ['/actions.html', { file: 'actions.html', type: 'text/html; charset=utf-8' }],
  ['/manifest.json', { file: 'manifest.json', type: 'application/json; charset=utf-8' }],
]);

const NOT_FOUND = Buffer.from('404 not found\n', 'utf8');
const METHOD_NOT_ALLOWED = Buffer.from('405 method not allowed\n', 'utf8');

function send(res, status, type, body, headOnly) {
  res.writeHead(status, {
    'Content-Type': type,
    'Content-Length': body.length,
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
  });
  res.end(headOnly ? undefined : body);
}

async function handle(req, res) {
  const headOnly = req.method === 'HEAD';
  if (req.method !== 'GET' && !headOnly) {
    send(res, 405, 'text/plain; charset=utf-8', METHOD_NOT_ALLOWED, false);
    return;
  }

  let pathname;
  try {
    pathname = new URL(req.url ?? '/', 'http://127.0.0.1').pathname;
  } catch {
    send(res, 404, 'text/plain; charset=utf-8', NOT_FOUND, headOnly);
    return;
  }

  const entry = ALLOWED.get(pathname);
  if (!entry) {
    send(res, 404, 'text/plain; charset=utf-8', NOT_FOUND, headOnly);
    return;
  }

  let body;
  try {
    body = await readFile(new URL(entry.file, import.meta.url));
  } catch {
    send(res, 500, 'text/plain; charset=utf-8', Buffer.from('500 fixture unreadable\n', 'utf8'), headOnly);
    return;
  }
  send(res, 200, entry.type, body, headOnly);
}

export function startFixtureServer({ port = 0, host = '127.0.0.1' } = {}) {
  const server = createServer((req, res) => {
    handle(req, res).catch(() => {
      if (!res.headersSent) {
        send(res, 500, 'text/plain; charset=utf-8', Buffer.from('500 fixture error\n', 'utf8'), false);
      } else {
        res.destroy();
      }
    });
  });

  return new Promise((resolve, reject) => {
    const onError = (error) => {
      server.off('listening', onListening);
      reject(error);
    };
    const onListening = () => {
      server.off('error', onError);
      const address = server.address();
      const actualPort = typeof address === 'object' && address ? address.port : port;
      resolve({
        server,
        port: actualPort,
        origin: `http://${host}:${actualPort}`,
        root,
      });
    };
    server.once('error', onError);
    server.once('listening', onListening);
    server.listen(port, host);
  });
}

function parsePort(argv) {
  const index = argv.indexOf('--port');
  if (index === -1) return 4173;
  const raw = argv[index + 1];
  if (!/^\d+$/.test(raw ?? '')) throw new Error('--port must be an integer from 0 to 65535');
  const port = Number(raw);
  if (port < 0 || port > 65535) throw new Error('--port must be an integer from 0 to 65535');
  return port;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  try {
    const port = parsePort(process.argv.slice(2));
    const { origin, server } = await startFixtureServer({ port });
    console.log(`hand-mcp fixtures listening on ${origin}/`);
    const stop = () => server.close(() => process.exit(0));
    process.once('SIGINT', stop);
    process.once('SIGTERM', stop);
  } catch (error) {
    console.error(`fixture server failed: ${error.message}`);
    process.exit(1);
  }
}
