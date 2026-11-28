#!/usr/bin/env node
// benchmarks/modern-web/run.mjs
// Conformance harness: drive the hand MCP stdio server against static fixtures.
// One case = navigate, snapshot, assert expected role/name, act, verify.
// No npm dependencies. Node 22+.
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";

const HERE = import.meta.dirname;
const REPO = resolve(HERE, "..", "..");
const FIXTURES = join(HERE, "fixtures");
const CASES = join(HERE, "cases");
const NEGATIVE = join(HERE, "negative");
const REQUEST_TIMEOUT_MS = 30_000;

function parseArgs(argv) {
  const out = { exe: null, filter: null, limit: null, negative: false, help: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--help" || a === "-h") out.help = true;
    else if (a === "--negative") out.negative = true;
    else if (a === "--exe") out.exe = argv[++i] ?? null;
    else if (a === "--filter") out.filter = argv[++i] ?? null;
    else if (a === "--limit") { const n = Number(argv[++i]); out.limit = Number.isFinite(n) && n >= 0 ? Math.floor(n) : null; }
    else { console.error("Unknown argument: " + a); out.help = true; }
  }
  return out;
}

function usage() {
  console.log([
    "Usage: node benchmarks/modern-web/run.mjs [options]",
    "  --exe <path>       hand executable (default: target/release/hand.exe, then target/debug/hand.exe)",
    "  --filter <substr>  only run case ids containing this substring",
    "  --limit N          only run the first N selected cases",
    "  --negative         run the deliberately-broken negative fixtures (all must fail as intended)",
    "  --help             show this help",
  ].join("\n"));
}

function resolveExe(requested) {
  const candidates = [];
  if (requested) candidates.push(resolve(requested));
  else if (process.env.HAND_MCP_BIN) candidates.push(resolve(process.env.HAND_MCP_BIN));
  else {
    candidates.push(join(REPO, "target", "release", "hand.exe"));
    candidates.push(join(REPO, "target", "debug", "hand.exe"));
  }
  const found = candidates.find((p) => existsSync(p));
  return { exe: found ?? null, tried: candidates };
}

function startFixtureServer() {
  const server = createServer((req, res) => {
    let pathname;
    try { pathname = decodeURIComponent(new URL(req.url, "http://127.0.0.1").pathname); }
    catch { res.writeHead(400).end("bad url"); return; }
    if (pathname === "/") pathname = "/index.html";
    const rel = pathname.replace(/^\/+/, "").replace(/^fixtures\//, "");
    if (rel.includes("..") || rel.includes("\\")) { res.writeHead(403).end("forbidden"); return; }
    const file = join(FIXTURES, rel);
    if (!file.startsWith(FIXTURES) || !existsSync(file)) { res.writeHead(404).end("not found"); return; }
    const type = file.toLowerCase().endsWith(".html") ? "text/html; charset=utf-8" : "application/octet-stream";
    res.writeHead(200, { "Content-Type": type, "Cache-Control": "no-store" });
    res.end(readFileSync(file));
  });
  return new Promise((resolveReady, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => resolveReady({ server, origin: "http://127.0.0.1:" + server.address().port }));
  });
}

class McpClient {
  constructor(exe) {
    this.seq = 0;
    this.pending = new Map();
    this.stderr = "";
    this.child = spawn(exe, ["mcp"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, cwd: REPO });
    this.child.stderr.setEncoding("utf8");
    this.child.stderr.on("data", (c) => { this.stderr = (this.stderr + c).slice(-65536); });
    this.reader = createInterface({ input: this.child.stdout, crlfDelay: Infinity });
    this.reader.on("line", (line) => {
      if (!line.trim()) return;
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      if (msg.id === undefined || !this.pending.has(msg.id)) return;
      const waiter = this.pending.get(msg.id);
      this.pending.delete(msg.id);
      clearTimeout(waiter.timer);
      waiter.resolve(msg);
    });
    this.exited = null;
    this.child.on("exit", (code, signal) => {
      this.exited = { code, signal };
      for (const w of this.pending.values()) { clearTimeout(w.timer); w.resolve({ error: { code: -1, message: "server exited" } }); }
      this.pending.clear();
    });
  }
  request(method, params) {
    return new Promise((resolveP) => {
      if (this.exited) { resolveP({ error: { code: -1, message: "server already exited" } }); return; }
      const id = ++this.seq;
      const timer = setTimeout(() => { if (this.pending.delete(id)) resolveP({ error: { code: -2, message: method + " timed out" } }); }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve: resolveP, timer });
      this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    });
  }
  notify(method, params) { this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n"); }
  async initialize() {
    const r = await this.request("initialize", { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "modern-web-conformance", version: "1.0.0" } });
    if (r.error) return { ok: false, reason: "initialize: " + JSON.stringify(r.error) };
    this.notify("notifications/initialized", {});
    return { ok: true };
  }
  async call(tool, args) {
    const msg = await this.request("tools/call", { name: tool, arguments: args ?? {} });
    if (msg.error) return { ok: false, reason: "rpc error: " + JSON.stringify(msg.error), tool };
    const result = msg.result ?? {};
    const isError = result.isError === true;
    let payload = result.structuredContent;
    if (payload === undefined || payload === null) {
      const text = Array.isArray(result.content) ? result.content.find((x) => x && x.type === "text" && typeof x.text === "string")?.text : null;
      if (typeof text === "string") { try { payload = JSON.parse(text); } catch { payload = { text }; } }
      else payload = {};
    }
    if (isError) {
      const reason = payload?.reason ?? payload?.message ?? payload?.code ?? "tool reported isError";
      return { ok: false, reason: String(reason), tool, payload };
    }
    return { ok: true, payload, tool };
  }
  async close() {
    if (this.exited) return;
    try { this.request("shutdown", {}).catch(() => {}); this.notify("exit", {}); this.child.stdin.end(); } catch (e) {}
    await new Promise((r) => setTimeout(r, 300));
    if (!this.exited) this.child.kill();
  }
}

function norm(s) { return String(s ?? "").replace(/\s+/g, " ").trim().toLowerCase(); }
function deepEqual(a, b) { return JSON.stringify(a) === JSON.stringify(b); }

function loadCases(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const f of readdirSync(dir).filter((n) => n.endsWith(".json")).sort()) {
    const parsed = JSON.parse(readFileSync(join(dir, f), "utf8"));
    const list = Array.isArray(parsed) ? parsed : (Array.isArray(parsed.cases) ? parsed.cases : [parsed]);
    for (const c of list) out.push({ ...c, _file: f });
  }
  return out;
}

// Resolve every at-ref string against a snapshot node. roleHint disambiguates
// and is removed before the tool call. Never a hard-coded at-eN.
function resolveRefs(value, nodes, ctx) {
  if (typeof value === "string") {
    if (!value.startsWith("@ref:")) return value;
    const name = norm(value.slice(5));
    const matches = nodes.filter((n) => norm(n.name) === name && n.role !== "rootwebarea" && n.role !== "generic");
    if (!matches.length) throw new Error("no snapshot node with accessible name " + JSON.stringify(name) + " (saw " + JSON.stringify(nodes.map(n => ({role: n.role, name: n.name}))) + ")");
    const node = matches[0];
    ctx.refs.push({ name, role: node.role, ref: node.ref });
    return "@" + String(node.ref).replace(/^@/, "");
  }
  if (Array.isArray(value)) return value.map((v) => resolveRefs(v, nodes, ctx));
  if (value && typeof value === "object") {
    const copy = {};
    for (const [k, v] of Object.entries(value)) { if (k !== "roleHint") copy[k] = resolveRefs(v, nodes, ctx); }
    return copy;
  }
  return value;
}

function snapshotMatches(nodes, expected) {
  return nodes.some((n) => norm(n.role) === norm(expected.role) && norm(n.name) === norm(expected.name));
}

function verdictOf(entry, payload) {
  const tool = entry.tool;
  const want = entry.expected;
  if (tool === "browser_assert") {
    const v = payload?.verdict;
    return { pass: v === true || v === "pass", detail: "verdict=" + JSON.stringify(v) + " observed=" + JSON.stringify(payload?.observed) };
  }
  if (tool === "browser_evaluate") {
    if (want !== undefined) {
      const got = payload?.value;
      return { pass: deepEqual(got, want), detail: "value=" + JSON.stringify(got) + " expected=" + JSON.stringify(want) };
    }
    return { pass: payload?.value === true, detail: "value=" + JSON.stringify(payload?.value) };
  }
  if (tool === "browser_assert_layout") return { pass: payload?.pass === true, detail: "pass=" + JSON.stringify(payload?.pass) };
  if (tool === "browser_screenshot") {
    const ok = Number(payload?.bytes) > 0;
    return { pass: ok, detail: "bytes=" + JSON.stringify(payload?.bytes) };
  }
  if (tool === "browser_snapshot") {
    if (Array.isArray(entry.expectedNodes)) {
      const nodes = payload?.nodes ?? [];
      const missing = entry.expectedNodes.filter((e) => !snapshotMatches(nodes, e));
      return { pass: missing.length === 0, detail: "missing=" + JSON.stringify(missing) };
    }
    return { pass: Array.isArray(payload?.nodes), detail: "nodes=" + Array.isArray(payload?.nodes) };
  }
  return { pass: payload?.ok !== false, detail: "ok=" + JSON.stringify(payload?.ok) };
}

async function runCase(client, c, origin) {
  const ctx = { refs: [] };
  const fail = (stage, detail) => ({ status: "FAIL", stage, detail, ctx, expectSnapshot: c.expectSnapshot ?? [] });
  const url = c.url.startsWith("http") ? c.url : origin + (c.url.startsWith("/") ? c.url : "/" + c.url);
  let r = await client.call("browser_navigate", { url });
  if (!r.ok) return fail("navigate", "navigate: " + r.reason);
  let snap = await client.call("browser_snapshot", {});
  if (!snap.ok) return fail("snapshot", "snapshot: " + snap.reason);
  let nodes = snap.payload?.nodes ?? [];
  const missing = (c.expectSnapshot ?? []).filter((e) => !snapshotMatches(nodes, e));
  if (missing.length) return fail("snapshot", "expectSnapshot missing " + JSON.stringify(missing));
  const acts = [];
  for (const act of (c.act ?? [])) {
    if (JSON.stringify(act).includes("@ref:")) {
      const s2 = await client.call("browser_snapshot", {});
      if (!s2.ok) return fail("snapshot", "pre-act snapshot: " + s2.reason);
      nodes = s2.payload?.nodes ?? [];
    }
    let args;
    try { args = resolveRefs(act.args ?? {}, nodes, ctx); } catch (e) { return fail("resolve", e.message); }
    const step = await client.call(act.tool, args);
    acts.push({ tool: act.tool, args, ok: step.ok, reason: step.ok ? null : step.reason });
    if (!step.ok) return fail("act", act.tool + ": " + step.reason);
  }
  const expects = [];
  for (const exp of (c.expect ?? [])) {
    let args;
    try { args = resolveRefs(exp.args ?? {}, nodes, ctx); } catch (e) { return fail("resolve", e.message); }
    const step = await client.call(exp.tool, args);
    if (!step.ok) { expects.push({ tool: exp.tool, pass: false, detail: step.reason }); return fail("expect", exp.tool + ": " + step.reason); }
    const v = verdictOf(exp, step.payload);
    expects.push({ tool: exp.tool, pass: v.pass, detail: v.detail });
    if (!v.pass) return fail("expect", exp.tool + " " + v.detail);
  }
  return { status: "PASS", stage: null, detail: "ok", ctx, acts, expects, expectSnapshot: c.expectSnapshot ?? [] };
}

function matchesFailure(result, spec) {
  if (!spec) return false;
  if (spec.stage && result.stage !== spec.stage) return false;
  if (spec.detail && !String(result.detail ?? "").toLowerCase().includes(String(spec.detail).toLowerCase())) return false;
  return true;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.help) { usage(); return 0; }
  const started = Date.now();
  const startedIso = new Date(started).toISOString();
  const { exe, tried } = resolveExe(opts.exe);
  if (!exe) { console.error("hand executable not found; tried: " + tried.join(", ")); return 2; }
  const dir = opts.negative ? NEGATIVE : CASES;
  let cases = loadCases(dir);
  if (opts.filter) cases = cases.filter((c) => String(c.id).includes(opts.filter));
  if (opts.limit !== null) cases = cases.slice(0, opts.limit);
  if (!existsSync(FIXTURES)) { console.error("fixtures directory missing: " + FIXTURES); return 2; }
  const { server, origin } = await startFixtureServer();
  const client = new McpClient(exe);
  const init = await client.initialize();
  if (!init.ok) { console.error(init.reason); server.close(); await client.close(); return 2; }
  const results = [];
  let pass = 0, fail = 0, skip = 0, expectedFail = 0;
  for (const c of cases) {
    const t0 = Date.now();
    if (c.skip === true) {
      const reason = c.skipReason ?? "no reason given";
      skip++;
      results.push({ id: c.id, status: "SKIP", reason, ms: Date.now() - t0, file: c._file });
      console.log("SKIP  " + c.id + "  " + reason);
      continue;
    }
    const result = await runCase(client, c, origin);
    const ms = Date.now() - t0;
    const row = { id: c.id, ...result, ms, file: c._file, url: c.url };
    if (opts.negative) {
      const hit = matchesFailure(result, c.expectedFailure);
      row.status = hit ? "EXPECTED-FAIL" : "FAIL";
      if (hit) { expectedFail++; console.log("EXPECTED-FAIL  " + c.id + "  " + result.detail); }
      else { fail++; console.log("FAIL  " + c.id + "  " + (result.status === "PASS" ? "unexpectedly passed" : result.stage + ": " + result.detail)); }
    } else {
      if (result.status === "PASS") { pass++; console.log("PASS  " + c.id + "  " + result.detail); }
      else { fail++; console.log("FAIL  " + c.id + "  " + result.stage + ": " + result.detail); }
    }
    results.push(row);
  }
  const finished = Date.now();
  const durationMs = finished - started;
  const nonSkipped = results.filter((r) => r.status !== "SKIP").length;
  const good = opts.negative ? expectedFail : pass;
  console.log("RESULT: " + good + "/" + nonSkipped);
  console.log("SKIPPED: " + skip);
  console.log("DURATION: " + durationMs + "ms");
  const stamp = startedIso.replace(/[:.]/g, "-");
  const reportPath = join(HERE, "report-" + stamp + ".json");
  const report = {
    startedAt: startedIso, finishedAt: new Date(finished).toISOString(), durationMs,
    mode: opts.negative ? "negative" : "conformance", exe, exeTried: tried, origin,
    command: "node " + process.argv.slice(1).join(" "), selected: cases.length,
    counts: { pass, fail, skip, expectedFail, nonSkipped, total: results.length },
    verdict: good === nonSkipped && fail === 0 ? "GREEN" : "ISSUES",
    results,
  };
  writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n", { encoding: "utf8" });
  console.log("REPORT: " + reportPath.replace(/\\/g, "/"));
  await client.close();
  server.close();
  return fail === 0 ? 0 : 1;
}

main().then((code) => { process.exit(code); }).catch((e) => {
  console.error("harness error: " + (e?.stack ?? String(e)));
  process.exit(2);
});