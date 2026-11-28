#!/usr/bin/env node
// benchmarks/perf/run.mjs -- reproducible performance measurements for hand.
// Dependency-free Node 22+. Run from anywhere: node benchmarks/perf/run.mjs [options]
//
// Every timing is a wall-clock measurement taken in this process with
// process.hrtime.bigint(); every reported size is read from a real file or a
// real protocol frame. No number is estimated or cached. The only files this
// script writes are inside one mkdtemp directory, removed in the finally block.

import os from "node:os";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { createInterface } from "node:readline";
import { gunzipSync } from "node:zlib";
import { existsSync, statSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const DEFAULT_EXE = join(REPO, "target", "release", "hand.exe");
const DEFAULT_COMPARE = "C:/Users/nathan/.local/share/hand-mcp-0.3.3/node_modules/hand-mcp/hand.exe";
const LAUNCHER = join(REPO, "npm", "bin", "hand-mcp.js");
const REQUEST_TIMEOUT_MS = 90_000;
const CLIENT_INFO = { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "hand-perf", version: "1.0.0" } };

// ---------------------------------------------------------------- cli

function parseArgs(argv) {
  const out = { exe: DEFAULT_EXE, compare: DEFAULT_COMPARE, reps: 10, json: false, help: false };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--help" || a === "-h") out.help = true;
    else if (a === "--json") out.json = true;
    else if (a === "--exe" || a === "--compare" || a === "--reps") {
      const v = argv[i + 1];
      if (!v || v.startsWith("--")) throw new Error(a + " requires a value");
      i += 1;
      if (a === "--exe") out.exe = resolve(v);
      else if (a === "--compare") out.compare = resolve(v);
      else {
        if (!/^\d+$/.test(v) || Number(v) < 1) throw new Error("--reps must be a positive integer");
        out.reps = Number(v);
      }
    } else throw new Error("unknown argument: " + a);
  }
  return out;
}

const USAGE = [
  "Usage: node benchmarks/perf/run.mjs [options]",
  "  --exe <path>       hand executable to measure (default target/release/hand.exe)",
  "  --compare <path>   older executable for the startup/handshake comparison",
  "  --reps N           repetitions per measurement, median of N (default 10)",
  "  --json             emit one JSON object instead of the aligned table",
  "  --help             show this help",
].join("\n");

// ---------------------------------------------------------------- timing

const now = () => process.hrtime.bigint();
const elapsedMs = (a) => Number(now() - a) / 1e6;
const round = (n, places = 2) => (Number.isFinite(n) ? Number(n.toFixed(places)) : null);

function summarize(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  const median = sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
  return { median: round(median), min: round(sorted[0]), max: round(sorted[sorted.length - 1]), reps: sorted.length, values: sorted.map((v) => round(v)) };
}

// ---------------------------------------------------------------- stdio client

class McpClient {
  constructor(exe) {
    this.seq = 0;
    this.pending = new Map();
    this.exited = false;
    this.stderr = "";
    this.child = spawn(exe, ["mcp"], {
      cwd: REPO, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, GLADIUS_CDP_URL: "" },
    });
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (c) => { this.stderr = (this.stderr + c).slice(-8192); });
    this.child.on("error", () => { this.exited = true; });
    this.child.on("exit", () => {
      this.exited = true;
      for (const w of this.pending.values()) { clearTimeout(w.timer); w.reject(new Error("server exited before replying")); }
      this.pending.clear();
    });
    const rl = createInterface({ input: this.child.stdout, crlfDelay: Infinity });
    rl.on("line", (line) => {
      if (!line.trim()) return;
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      const id = msg.id;
      if (id === undefined || !this.pending.has(id)) return;
      const w = this.pending.get(id);
      this.pending.delete(id);
      clearTimeout(w.timer);
      w.resolve({ msg, bytes: Buffer.byteLength(line, "utf8") + 1, elapsedMs: round(elapsedMs(w.started)) });
    });
  }

  request(method, params) {
    if (this.exited) return Promise.reject(new Error("server already exited"));
    const id = ++this.seq;
    const started = now();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(method + " timed out after " + REQUEST_TIMEOUT_MS + " ms"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer, started });
      this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  }

  notify(method, params) {
    this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
  }

  async initialize() {
    await this.request("initialize", CLIENT_INFO);
    this.notify("notifications/initialized", {});
  }

  async close() {
    if (this.child.exitCode === null && !this.exited) {
      try { this.child.stdin.end(); } catch { /* already gone */ }
      const exited = new Promise((r) => this.child.once("exit", r));
      const timeout = new Promise((r) => setTimeout(() => r("timeout"), 2000));
      const outcome = await Promise.race([exited, timeout]);
      if (outcome === "timeout" && this.child.exitCode === null) this.child.kill();
    }
  }
}

async function tool(client, name, args) {
  const r = await client.request("tools/call", { name, arguments: args ?? {} });
  if (r.error) throw new Error(name + ": rpc error " + JSON.stringify(r.error));
  if (r.msg.result?.isError === true) {
    const p = r.msg.result.structuredContent ?? {};
    throw new Error(name + ": " + (p.code ?? p.error ?? p.reason ?? "isError"));
  }
  return { ...r, payload: r.msg.result?.structuredContent ?? {} };
}

// ---------------------------------------------------------------- one-shot processes

function spawnWall(command, args, options = {}) {
  return new Promise((resolve) => {
    const started = now();
    const child = spawn(command, args, {
      cwd: options.cwd ?? REPO, windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"], env: options.env, shell: options.shell ?? false,
    });
    let output = "";
    let settled = false;
    const finish = (code) => {
      if (settled) return;
      settled = true;
      resolve({ ms: round(elapsedMs(started)), code, output: output.trim() });
    };
    child.stdout.on("data", (c) => { output += c; });
    child.stderr.on("data", (c) => { output += c; });
    child.on("close", finish);
    child.on("error", () => finish(-1));
  });
}

async function version(exe) {
  const r = await spawnWall(exe, ["--version"]);
  if (r.code !== 0) throw new Error(exe + " --version exit " + r.code + ": " + r.output);
  return r;
}

async function handshake(exe) {
  const started = now();
  const client = new McpClient(exe);
  try {
    const init = await client.request("initialize", CLIENT_INFO);
    if (init.error) throw new Error("initialize: " + JSON.stringify(init.error));
    client.notify("notifications/initialized", {});
    const list = await client.request("tools/list", {});
    if (list.error) throw new Error("tools/list: " + JSON.stringify(list.error));
    return {
      ms: round(elapsedMs(started)), initMs: init.elapsedMs, listMs: list.elapsedMs,
      listBytes: list.bytes, tools: (list.msg.result?.tools ?? []).length,
    };
  } finally {
    await client.close();
  }
}

// The engine's first page load can race the freshly launched Chrome and answer
// blank_load once (the same race the test suite retries). Time from the first
// attempt through the first successful load, and report how many tries it took.
async function navigate(client, url, maxAttempts = 3) {
  let total = 0;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const r = await client.request("tools/call", { name: "browser_navigate", arguments: { url } });
    total += r.elapsedMs;
    if (!r.error && r.msg.result?.isError !== true) {
      return { ...r, payload: r.msg.result?.structuredContent ?? {}, attempts: attempt, elapsedMs: round(total) };
    }
    if (attempt === maxAttempts) {
      const p = r.msg.result?.structuredContent ?? {};
      throw new Error("browser_navigate: " + (p.code ?? p.reason ?? JSON.stringify(r.error ?? {})));
    }
  }
  throw new Error("unreachable");
}

// ---------------------------------------------------------------- loopback fixture

function startFixture() {
  const html = [
    "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">",
    "<title>hand perf fixture</title></head><body>",
    "<main><h1 id=\"title\">hand performance fixture</h1>",
    "<p>A stable loopback page used by every browser measurement.</p>",
    "<button id=\"action\" type=\"button\" onclick=\"document.getElementById('title').textContent='Clicked'\">Click me</button>",
    "</main></body></html>",
  ].join("\n");
  const server = createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" });
    res.end(html);
  });
  return new Promise((resolveReady) => server.listen(0, "127.0.0.1", () =>
    resolveReady({ server, origin: "http://127.0.0.1:" + server.address().port })));
}

// ---------------------------------------------------------------- measurements

async function browserRep(exe, origin) {
  const client = new McpClient(exe);
  try {
    await client.initialize();
    await client.request("tools/list", {}); // real clients discover before acting
    const cold = await navigate(client, origin + "/");
    const nav = await navigate(client, origin + "/");
    const snap = await tool(client, "browser_snapshot", {});
    const nodes = snap.payload?.nodes ?? [];
    const button = nodes.find((n) => /click me/i.test(n.name ?? "")) ?? nodes.find((n) => n.role === "button");
    if (!button) throw new Error("fixture button missing from snapshot");
    const click = await tool(client, "browser_click", { element: "@" + String(button.ref).replace(/^@/, ""), button: "left" });
    const shot = await tool(client, "browser_screenshot", { format: "png", fullPage: true });
    const evalR = await tool(client, "browser_evaluate", { expression: "document.querySelectorAll('*').length" });
    const metrics = await tool(client, "browser_metrics", {});
    const steps = [
      { tool: "navigate", args: { url: origin + "/" } },
      ...Array.from({ length: 9 }, (_, i) => ({ tool: "evaluate", args: { expression: String(i + 1) } })),
    ];
    const run = await tool(client, "browser_run", { steps });
    return {
      coldMs: cold.elapsedMs, coldAttempts: cold.attempts, navMs: nav.elapsedMs, snapMs: snap.elapsedMs,
      clickMs: click.elapsedMs, shotMs: shot.elapsedMs, evalMs: evalR.elapsedMs,
      metricsMs: metrics.elapsedMs, runMs: run.elapsedMs,
      nodes: nodes.length, shotBytes: shot.payload?.bytes ?? null,
      evalValue: evalR.payload?.value ?? null, runOk: run.payload?.ok === true,
      runMsReported: run.payload?.ms ?? null, runSteps: run.payload?.steps?.length ?? null,
      viewport: shot.payload?.size ?? null,
    };
  } finally {
    await client.close();
  }
}

async function desktopRep(exe, tmpDir, index) {
  const out = join(tmpDir, "record-" + index + ".avi");
  const client = new McpClient(exe);
  try {
    await client.initialize();
    const listed = await tool(client, "desktop_list_windows", {});
    const wins = listed.payload?.windows ?? [];
    const target = wins
      .filter((w) => w.visible && !w.cloaked && !w.minimized && Array.isArray(w.rect) && w.rect[2] - w.rect[0] > 200)
      .sort((a, b) => ((b.rect[2] - b.rect[0]) * (b.rect[3] - b.rect[1])) - ((a.rect[2] - a.rect[0]) * (a.rect[3] - a.rect[1])))[0];
    if (!target) throw new Error("no visible desktop target window");
    const shot = await tool(client, "desktop_screenshot", { target: { hwnd: target.hwnd }, fmt: "png" });
    const imageBlock = (shot.msg.result?.content ?? []).find((c) => c && typeof c.data === "string");
    const rec = await tool(client, "desktop_record", {
      target: { hwnd: target.hwnd }, out, duration_ms: 1000, fps: 8,
    });
    const p = rec.payload ?? {};
    return {
      listMs: listed.elapsedMs, windows: wins.length,
      shotMs: shot.elapsedMs,
      shotBytes: imageBlock ? Buffer.from(imageBlock.data, "base64").length : null,
      shotW: shot.payload?.image?.w ?? null, shotH: shot.payload?.image?.h ?? null,
      recordMs: rec.elapsedMs, frames: p.frames ?? null, captured: p.capturedFrames ?? null,
      dropped: p.droppedFrames ?? null, recordBytes: p.bytes ?? null,
      nominalFps: p.fps ?? null, durationMs: p.durationMs ?? null,
      achievedFps: p.capturedFrames != null && p.durationMs ? round(p.capturedFrames / (p.durationMs / 1000)) : null,
      aviFps: p.frames != null && p.durationMs ? round(p.frames / (p.durationMs / 1000)) : null,
      targetExe: target.exe ?? null, targetElevated: target.elevated === true,
    };
  } finally {
    await client.close();
    await rm(out, { force: true, maxRetries: 5, retryDelay: 100 }).catch(() => {});
  }
}

function tarSizes(gz) {
  const buf = gunzipSync(gz);
  let offset = 0, bytes = 0, files = 0;
  while (offset + 512 <= buf.length) {
    const header = buf.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    const raw = header.subarray(124, 136).toString("latin1").replace(/\0.*$/, "").trim();
    const size = parseInt(raw || "0", 8) || 0;
    const type = String.fromCharCode(header[156]);
    if (type === "0" || type === "\u0000" || type === "7") { bytes += size; files += 1; }
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return { bytes, files };
}

async function copyInto(src, dst) {
  if ((await stat(src)).isDirectory()) {
    await mkdir(dst, { recursive: true });
    for (const entry of await readdir(src, { withFileTypes: true })) {
      await copyInto(join(src, entry.name), join(dst, entry.name));
    }
  } else {
    await copyFile(src, dst);
  }
}

async function sizes(exe, tmpDir) {
  const exeBytes = (await stat(exe)).size;
  const pkg = join(tmpDir, "pkg");
  const packDir = join(tmpDir, "pack");
  await mkdir(join(pkg, "bin"), { recursive: true });
  await mkdir(packDir, { recursive: true });
  const manifest = JSON.parse(await readFileSafe(join(REPO, "npm", "package.json")));
  await copyFile(join(REPO, "npm", "package.json"), join(pkg, "package.json"));
  for (const entry of manifest.files ?? []) {
    const src = join(REPO, "npm", entry);
    if (existsSync(src)) await copyInto(src, join(pkg, entry));
  }
  await copyFile(exe, join(pkg, "hand.exe")); // stage this build, never touch npm/**
  const npmCmd = process.platform === "win32" ? "npm.cmd" : "npm";
  const packed = await spawnWall(npmCmd + " pack --ignore-scripts --pack-destination \"" + packDir + "\"", [], { cwd: pkg, shell: true });
  if (packed.code !== 0) throw new Error("npm pack exit " + packed.code + ": " + packed.output);
  const tgzName = (await readdir(packDir)).find((n) => n.endsWith(".tgz"));
  if (!tgzName) throw new Error("npm pack produced no tarball");
  const tgzPath = join(packDir, tgzName);
  const tgzBytes = (await stat(tgzPath)).size;
  const unpacked = tarSizes(await readFileSafe(tgzPath));
  return { exeBytes, tgzBytes, unpackedBytes: unpacked.bytes, unpackedFiles: unpacked.files, tarball: tgzName };
}

async function readFileSafe(p) {
  const { readFile } = await import("node:fs/promises");
  return readFile(p);
}

async function npxStatus() {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const npx = process.platform === "win32" ? "npx.cmd" : "npx";
  const cached = await spawnWall(npm + " cache ls hand-mcp", [], { env: { ...process.env }, shell: true });
  const keys = cached.output.split(/\r?\n/).filter((l) => /hand-mcp/i.test(l) && !/warn|error/i.test(l));
  if (keys.length === 0) {
    const probe = await spawnWall(npx + " -y hand-mcp --version", [], {
      env: { ...process.env, npm_config_offline: "true" }, shell: true,
    });
    return {
      measured: false,
      reason: "not measured: no cached hand-mcp package; offline npx probe exit " + probe.code +
        " (" + (/ENOTCACHED/.test(probe.output) ? "ENOTCACHED" : probe.output.slice(0, 120)) + ")",
    };
  }
  const r = await spawnWall(npx + " -y hand-mcp --version", [], {
    env: { ...process.env, npm_config_offline: "true" }, shell: true,
  });
  return { measured: r.code === 0, ms: r.ms, output: r.output, reason: r.code === 0 ? "cached package, offline" : "npx failed offline: " + r.output };
}

// ---------------------------------------------------------------- report

async function collect(exe, compareExe, reps, tmpDir) {
  const rows = [];
  const add = (metric, unit, samples, detail = "") => {
    const s = summarize(samples);
    rows.push({ metric, unit, median: s.median, min: s.min, max: s.max, reps: s.reps, detail, values: s.values });
  };
  const progress = (label) => process.stderr.write("  " + label + "\n");

  progress("startup --version (" + basename(exe) + ")");
  const starts = [], startsOld = [];
  const versionInfo = { exe: null, compare: null };
  for (let i = 0; i < reps; i += 1) {
    const v = await version(exe);
    starts.push(v.ms);
    versionInfo.exe = v.output;
  }
  add("startup --version", "ms", starts, versionInfo.exe ?? "");

  progress("handshake initialize + tools/list (" + basename(exe) + ")");
  const handshakes = [], listBytes = [], toolCounts = [];
  for (let i = 0; i < reps; i += 1) {
    const h = await handshake(exe);
    handshakes.push(h.ms); listBytes.push(h.listBytes); toolCounts.push(h.tools);
  }
  add("handshake to tools/list", "ms", handshakes, "tools=" + toolCounts[0]);
  add("tools/list response frame", "bytes", listBytes, "newline-delimited JSON");

  progress("browser cold / warm / batch (" + reps + " fresh servers)");
  const fixture = await startFixture();
  const browser = [];
  try {
    for (let i = 0; i < reps; i += 1) browser.push(await browserRep(exe, fixture.origin));
  } finally {
    fixture.server.close();
  }
  const bKeys = {
    coldMs: "browser_navigate cold", navMs: "browser_navigate warm", snapMs: "browser_snapshot",
    clickMs: "browser_click", shotMs: "browser_screenshot", evalMs: "browser_evaluate",
    metricsMs: "browser_metrics", runMs: "browser_run 10 steps",
  };
  for (const [key, metric] of Object.entries(bKeys)) add(metric, "ms", browser.map((b) => b[key]));
  add("cold navigate attempts", "tries", browser.map((b) => b.coldAttempts), "blank_load startup race is retried when it occurs");
  add("snapshot nodes", "nodes", browser.map((b) => b.nodes), "fixture accessibility tree");
  add("browser_screenshot png", "bytes", browser.map((b) => b.shotBytes), "full page, 1440x900 viewport");
  add("browser_run reported ms", "ms", browser.map((b) => b.runMsReported), "10 steps, server-side aggregate");
  add("browser_run steps", "steps", browser.map((b) => b.runSteps), "1 navigate + 9 evaluates");

  progress("desktop list / screenshot / record");
  const desktop = [];
  for (let i = 0; i < reps; i += 1) desktop.push(await desktopRep(exe, tmpDir, i));
  add("desktop_list_windows", "ms", desktop.map((d) => d.listMs));
  add("visible windows", "windows", desktop.map((d) => d.windows), "reported window count");
  add("desktop_screenshot", "ms", desktop.map((d) => d.shotMs), "target " + (desktop[0]?.targetExe ?? "?"));
  add("desktop_screenshot png", "bytes", desktop.map((d) => d.shotBytes), "decoded image/png frame");
  add("desktop_record", "ms", desktop.map((d) => d.recordMs), "1000 ms requested, 8 fps");
  add("record frames written", "frames", desktop.map((d) => d.frames), "AVI chunks (fresh + repeats)");
  add("record fresh captures", "frames", desktop.map((d) => d.captured), "capturedFrames");
  add("record dropped slots", "frames", desktop.map((d) => d.dropped), "droppedFrames");
  add("record achieved fps", "fps", desktop.map((d) => d.achievedFps), "capturedFrames / recorded second");
  add("record AVI fps", "fps", desktop.map((d) => d.aviFps), "frames / recorded second");
  add("record AVI size", "bytes", desktop.map((d) => d.recordBytes), "file on disk");

  progress("launcher + npx");
  const launcher = [];
  for (let i = 0; i < reps; i += 1) {
    const r = await spawnWall(process.execPath, [LAUNCHER, "--version"]);
    if (r.code !== 0) throw new Error("launcher exit " + r.code + ": " + r.output);
    launcher.push(r.ms);
    versionInfo.launcher = r.output;
  }
  add("launcher node --version", "ms", launcher, versionInfo.launcher ?? "");
  const npx = await npxStatus();

  progress("sizes");
  const sizeInfo = await sizes(exe, tmpDir);
  add("release exe", "bytes", [sizeInfo.exeBytes], basename(exe));
  add("npm tarball", "bytes", [sizeInfo.tgzBytes], sizeInfo.tarball + " (npm pack in a temp copy)");
  add("installed unpacked", "bytes", [sizeInfo.unpackedBytes], sizeInfo.unpackedFiles + " files in the tarball");

  progress("comparison " + basename(compareExe));
  let comparison = { exe: compareExe, present: false };
  if (existsSync(compareExe)) {
    const oldStarts = [], oldHandshakes = [], oldBytes = [], oldTools = [];
    const compareVersion = await version(compareExe);
    for (let i = 0; i < reps; i += 1) {
      oldStarts.push((await version(compareExe)).ms);
      const h = await handshake(compareExe);
      oldHandshakes.push(h.ms); oldBytes.push(h.listBytes); oldTools.push(h.tools);
    }
    add("compare startup --version", "ms", oldStarts, compareVersion.output);
    add("compare handshake to tools/list", "ms", oldHandshakes, "tools=" + oldTools[0]);
    add("compare tools/list frame", "bytes", oldBytes, basename(compareExe));
    comparison = { exe: compareExe, present: true, version: compareVersion.output };
  } else {
    comparison = { exe: compareExe, present: false, note: "not measured: executable not present" };
  }

  return {
    machine: {
      cpu: os.cpus()[0]?.model ?? "unknown", cores: os.cpus().length,
      ramBytes: os.totalmem(), platform: os.platform(), arch: os.arch(), osRelease: os.release(),
      measuredAt: new Date().toISOString(), node: process.version,
    },
    config: { exe, compareExe, reps, launcher: LAUNCHER },
    rows, sizes: sizeInfo, npx, comparison,
    browserSample: browser[0] ?? null, desktopSample: desktop[0] ?? null,
  };
}

function table(report) {
  const head = ["Metric", "Median", "Min", "Max", "Reps", "Detail"];
  const body = report.rows.map((r) => [r.metric, String(r.median), String(r.min), String(r.max), String(r.reps), r.detail]);
  const widths = head.map((h, i) => Math.max(h.length, ...body.map((b) => b[i].length)));
  const line = (cells) => cells.map((c, i) => (i === 0 || i === 5 ? c.padEnd(widths[i]) : c.padStart(widths[i]))).join("  ");
  return [
    "hand performance  " + report.machine.measuredAt,
    report.machine.cpu + "  " + report.machine.cores + " logical CPUs  " +
      (report.machine.ramBytes / (1024 ** 3)).toFixed(1) + " GiB RAM  " +
      report.machine.platform + " " + report.machine.arch + " " + report.machine.osRelease,
    "reps=" + report.config.reps + " per row; values are wall-clock ms unless the unit column says otherwise",
    "", line(head), line(widths.map((w) => "-".repeat(w))), ...body.map(line), "",
    "npx: " + report.npx.reason,
    "compare: " + (report.comparison.present ? report.comparison.exe : report.comparison.note),
  ].join("\n");
}

async function main() {
  let opts;
  try { opts = parseArgs(process.argv.slice(2)); }
  catch (e) { console.error(e.message + "\n" + USAGE); process.exit(2); }
  if (opts.help) { console.log(USAGE); return 0; }
  if (!existsSync(opts.exe)) { console.error("executable not found: " + opts.exe); return 2; }
  const tmpDir = await mkdtemp(join(os.tmpdir(), "hand-perf-"));
  try {
    const report = await collect(opts.exe, opts.compare, opts.reps, tmpDir);
    console.log(opts.json ? JSON.stringify(report, null, 2) : table(report));
    return 0;
  } finally {
    await rm(tmpDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }).catch(() => {});
  }
}

main().then((code) => process.exit(code)).catch((e) => {
  console.error("benchmark failed: " + (e?.stack ?? String(e)));
  process.exit(1);
});
