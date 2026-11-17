// The controlled browser fixture: load once, verify against the repo pin, then
// serve those exact in-memory bytes. Nothing re-reads the directory after the
// fingerprint is taken, so the server can never serve bytes the receipt did not
// hash.
//
// The pin is `fixtures/SHA256SUMS`. A missing or malformed pin, a hash mismatch,
// a missing entry, or a label set other than exactly Control 001..Control 100
// makes the fixture unusable and the scenario blocked.

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { basename, join } from "node:path";

import { canonicalHash } from "./fingerprint.mjs";

export const FIXTURE_FILES = ["index.html", "actions.html", "manifest.json"];
const SERVED = new Map([
  ["/", { file: "index.html", type: "text/html; charset=utf-8" }],
  ["/index.html", { file: "index.html", type: "text/html; charset=utf-8" }],
  ["/actions.html", { file: "actions.html", type: "text/html; charset=utf-8" }],
  ["/manifest.json", { file: "manifest.json", type: "application/json; charset=utf-8" }],
]);
const NOT_FOUND = Buffer.from("404 not found\n", "utf8");
const METHOD_NOT_ALLOWED = Buffer.from("405 method not allowed\n", "utf8");

function sha256(buffer) {
  return `sha256:${createHash("sha256").update(buffer).digest("hex")}`;
}

export function parseSha256Sums(text) {
  const entries = new Map();
  const reasons = [];
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line) continue;
    const match = /^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/.exec(line);
    if (!match) {
      reasons.push(`malformed SHA256SUMS line: ${line.slice(0, 120)}`);
      continue;
    }
    const name = match[2];
    if (name !== basename(name) || name.includes("..") || name.includes("/") || name.includes("\\")) {
      reasons.push(`SHA256SUMS names a non-local fixture path: ${name}`);
      continue;
    }
    if (entries.has(name)) {
      reasons.push(`SHA256SUMS lists ${name} more than once`);
      continue;
    }
    entries.set(name, `sha256:${match[1].toLowerCase()}`);
  }
  return { entries, reasons };
}

export async function loadFixture(dir) {
  const reasons = [];
  let sumsText = null;
  try {
    sumsText = await readFile(join(dir, "SHA256SUMS"), "utf8");
  } catch (error) {
    reasons.push(`fixture pin is unreadable at ${join(dir, "SHA256SUMS")}: ${error.message}`);
  }
  const parsed = sumsText === null ? { entries: new Map(), reasons: [] } : parseSha256Sums(sumsText);
  reasons.push(...parsed.reasons);

  const bodies = new Map();
  const files = [];
  for (const [name, expected] of parsed.entries) {
    const path = join(dir, name);
    let body;
    try {
      body = await readFile(path);
    } catch (error) {
      reasons.push(`pinned fixture file is unreadable: ${name} (${error.message})`);
      continue;
    }
    const actual = sha256(body);
    if (actual !== expected) {
      reasons.push(`fixture pin mismatch for ${name}: SHA256SUMS has ${expected}, actual ${actual}`);
      continue;
    }
    if (FIXTURE_FILES.includes(name)) {
      bodies.set(name, body);
      files.push({ name, path, bytes: body.length, sha256: actual });
    }
  }

  for (const name of FIXTURE_FILES) {
    if (!parsed.entries.has(name)) reasons.push(`SHA256SUMS has no entry for required fixture file ${name}`);
    if (!bodies.has(name) && parsed.entries.has(name)) {
      reasons.push(`required fixture file failed its pin: ${name}`);
    }
  }

  let manifest = null;
  let manifestText = null;
  let controlLabels = [];
  if (bodies.has("manifest.json")) {
    try {
      manifestText = bodies.get("manifest.json").toString("utf8");
      manifest = JSON.parse(manifestText);
    } catch (error) {
      reasons.push(`fixture manifest.json is not JSON: ${error.message}`);
    }
  }
  if (bodies.has("index.html")) {
    const index = bodies.get("index.html").toString("utf8");
    controlLabels = [...index.matchAll(/data-label="([^"]*)"/g)].map((match) => match[1]);
    const expectedLabels = Array.from(
      { length: 100 },
      (_, indexNumber) => `Control ${String(indexNumber + 1).padStart(3, "0")}`,
    );
    const actualSorted = [...controlLabels].sort();
    const expectedSorted = [...expectedLabels].sort();
    if (
      actualSorted.length !== expectedSorted.length ||
      actualSorted.some((label, indexNumber) => label !== expectedSorted[indexNumber])
    ) {
      reasons.push(
        `fixture control labels are not exactly Control 001..Control 100 (found ${controlLabels.length} labels, ${new Set(controlLabels).size} unique)`,
      );
    }
    if (!index.includes('data-control-count="100"')) {
      reasons.push('fixture index.html is missing data-control-count="100"');
    }
  }

  if (manifest) {
    if (manifest?.benchmark?.count !== 100) {
      reasons.push(`fixture manifest declares ${manifest?.benchmark?.count} controls; required 100`);
    }
    if (manifest?.benchmark?.page !== "/index.html") {
      reasons.push(`fixture benchmark page is ${manifest?.benchmark?.page}; required /index.html`);
    }
    if (manifest?.frozen?.viewport?.width !== 1280 || manifest?.frozen?.viewport?.height !== 720) {
      reasons.push("fixture viewport is not frozen at 1280x720");
    }
    if (manifest?.frozen?.deviceScaleFactor !== 1) {
      reasons.push("fixture deviceScaleFactor is not frozen at 1");
    }
    if (manifest?.benchmark?.firstLabel !== "Control 001" || manifest?.benchmark?.lastLabel !== "Control 100") {
      reasons.push("fixture control label bounds are not Control 001 .. Control 100");
    }
  }

  const hash = canonicalHash(files.map(({ name, bytes, sha256: digest }) => ({ name, bytes, sha256: digest })));
  return {
    ok: reasons.length === 0,
    reasons,
    dir,
    pin: { file: join(dir, "SHA256SUMS"), entries: Object.fromEntries(parsed.entries) },
    hash,
    files,
    bodies,
    manifest,
    manifestText,
    controlLabels,
  };
}

export function startFixtureServer(fixture, { host = "127.0.0.1", port = 0 } = {}) {
  if (!fixture?.ok) throw new Error("cannot serve an unverified fixture");
  for (const { file } of SERVED.values()) {
    if (!fixture.bodies.has(file)) throw new Error(`fixture is missing served file: ${file}`);
  }
  const server = createServer((req, res) => {
    const headOnly = req.method === "HEAD";
    if (req.method !== "GET" && !headOnly) {
      send(res, 405, "text/plain; charset=utf-8", METHOD_NOT_ALLOWED, false);
      return;
    }
    let pathname;
    try {
      pathname = new URL(req.url ?? "/", `http://${host}`).pathname;
    } catch {
      send(res, 404, "text/plain; charset=utf-8", NOT_FOUND, headOnly);
      return;
    }
    const route = SERVED.get(pathname);
    if (!route) {
      send(res, 404, "text/plain; charset=utf-8", NOT_FOUND, headOnly);
      return;
    }
    const body = fixture.bodies.get(route.file);
    send(res, 200, route.type, body, headOnly);
  });

  return new Promise((resolveServer, reject) => {
    const onError = (error) => {
      server.off("listening", onListening);
      reject(error);
    };
    const onListening = () => {
      server.off("error", onError);
      const address = server.address();
      resolveServer({
        server,
        host,
        port: typeof address === "object" && address ? address.port : port,
        origin: `http://${host}:${typeof address === "object" && address ? address.port : port}`,
        served: fixture.files.filter(({ name }) => [...SERVED.values()].some((route) => route.file === name)),
      });
    };
    server.once("error", onError);
    server.once("listening", onListening);
    server.listen(port, host);
  });
}

function send(res, status, type, body, headOnly) {
  res.writeHead(status, {
    "Content-Type": type,
    "Content-Length": body.length,
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  });
  res.end(headOnly ? undefined : body);
}
