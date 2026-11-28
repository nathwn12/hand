#!/usr/bin/env node
// Live acceptance harness: prove the shipped hand.exe over real MCP stdio.
// Node 22+, no npm deps. Usage: node scripts/acceptance/live.mjs [--exe <path>] [--keep]
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";

const CALL_TIMEOUT_MS = 60000;
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const argv = process.argv.slice(2);
const keep = argv.includes("--keep");
const valueOf = (flag) => { const i = argv.indexOf(flag); return i < 0 ? undefined : argv[i + 1]; };

// Fixed order so a broken binary still prints a complete table.
const CHECKS = [
  "binary",
  "initialize",
  "tools_list",
  "browser_navigate",
  "browser_snapshot",
  "browser_click_precision",
  "browser_type_fast",
  "browser_screenshot",
  "browser_assert_network",
  "browser_metrics",
  "browser_trace",
  "browser_tabs_roundtrip",
  "browser_clipboard_roundtrip",
  "browser_clipboard_write_requires_opt_in",
  "browser_assert_false_verdict",
  "browser_assert_rejects_unknown_expectation",
  "browser_run_assert_failure",
  "browser_trace_dir_required",
  "browser_audit_contrast_fails",
  "browser_audit_contrast_passes",
  "browser_close",
  "desktop_list_windows",
  "desktop_cursor",
  "desktop_do_focus_guard",
  "desktop_clipboard_roundtrip",
  "desktop_window_resize",
  "desktop_record",
  "desktop_do_empty_actions",
  "desktop_window_noop_move_with_allow_foreground",
];

const results = new Map();
const tidy = (s) => String(s).replace(/\s+/g, " ").trim().slice(0, 260);
const message = (e) => tidy((e && e.message) || e);
function record(name, ok, detail) { results.set(name, { ok: !!ok, detail: tidy(detail) }); }
async function run(name, fn) {
  try { const v = await fn(); record(name, true, v); return v; }
  catch (e) { record(name, false, message(e)); return undefined; }
}
function need(v, what) { if (v === undefined || v === null) throw new Error("prerequisite failed: " + what); return v; }
function requireThat(cond, msg) { if (!cond) throw new Error(msg); }

// hand speaks newline-delimited JSON-RPC 2.0 over stdio; one id per call.
class McpClient {
  constructor(exe) {
    this.child = spawn(exe, ["mcp"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    this.buffer = "";
    this.pending = new Map();
    this.nextId = 1;
    this.exited = null;
    this.stderr = "";
    this.stderrLine = "";
    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");
    this.child.stdout.on("data", (d) => this.onData(d));
    this.child.stderr.on("data", (d) => {
      this.stderr = (this.stderr + d).slice(-4000);
      const m = this.stderr.match(/\[hand\][^\r\n]+/);
      if (m && !this.stderrLine) this.stderrLine = m[0].slice(0, 200);
    });
    const die = (why) => {
      if (this.exited) return;
      this.exited = why;
      for (const p of this.pending.values()) p.reject(new Error(why));
      this.pending.clear();
    };
    this.child.on("error", (e) => die("cannot spawn " + exe + ": " + e.message));
    this.child.on("exit", (code, signal) => {
      die("hand exited before replying (code " + code + ", signal " + (signal || "none") + ")" +
          (this.stderrLine ? " | " + this.stderrLine : ""));
    });
  }
  onData(chunk) {
    this.buffer += chunk;
    let i;
    while ((i = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, i).trim();
      this.buffer = this.buffer.slice(i + 1);
      if (!line) continue;
      let msg;
      try { msg = JSON.parse(line); } catch { continue; }
      const p = this.pending.get(msg.id);
      if (p) { this.pending.delete(msg.id); p.resolve(msg); }
    }
  }
  call(method, params) {
    if (this.exited) return Promise.reject(new Error(this.exited + " (calling " + method + ")"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(method + " timed out after " + CALL_TIMEOUT_MS + " ms"));
      }, CALL_TIMEOUT_MS);
      this.pending.set(id, {
        resolve: (m) => { clearTimeout(timer); resolve(m); },
        reject: (e) => { clearTimeout(timer); reject(e); },
      });
      try {
        this.child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
      } catch (e) {
        this.pending.delete(id); clearTimeout(timer); reject(e);
      }
    });
  }
  close() {
    try { this.child.stdin.end(); } catch {}
    setTimeout(() => {
      if (this.child.exitCode === null && this.child.signalCode === null) {
        try { this.child.kill(); } catch {}
      }
    }, 250);
  }
}

const FIXTURE_HTML = [
  "<!doctype html><html><head><meta charset=\"utf-8\"><title>hand acceptance fixture</title></head><body>",
  "<button id=\"hit\" aria-label=\"hit target\" style=\"display:block;width:20px;height:20px;padding:0;border:0;background:#06f\">hit</button>",
  "<input id=\"name\" aria-label=\"name\" style=\"width:260px\">",
  "<form action=\"/submit\" method=\"post\"><button id=\"submit\" type=\"submit\" aria-label=\"submit\">Submit</button></form>",
  "<script>window.__clicks=[];",
  "document.getElementById(\"hit\").addEventListener(\"click\",function(e){var r=e.currentTarget.getBoundingClientRect();",
  "window.__clicks.push({x:e.clientX,y:e.clientY,rect:{left:r.left,top:r.top,width:r.width,height:r.height}});});",
  "requestAnimationFrame(function(){setTimeout(function(){var d=document.createElement(\"div\");",
  "d.style.height=\"120px\";d.style.background=\"#ccc\";document.body.insertBefore(d,document.body.firstChild);},30);});<\/script>",
  "</body></html>",
].join("");

async function startFixture() {
  const serve = (res) => {
    const body = Buffer.from(FIXTURE_HTML, "utf8");
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Content-Length": body.length, "Cache-Control": "no-store" });
    res.end(body);
  };
  const server = createServer((req, res) => {
    if (req.method === "POST" && String(req.url).startsWith("/submit")) { req.resume(); req.on("end", () => serve(res)); return; }
    serve(res);
  });
  await new Promise((ok, bad) => { server.once("error", bad); server.listen(0, "127.0.0.1", ok); });
  return { server, url: "http://127.0.0.1:" + server.address().port + "/" };
}

function structured(body) {
  let sc = body && body.structuredContent;
  if (typeof sc === "string") { try { sc = JSON.parse(sc); } catch { sc = {}; } }
  return sc && typeof sc === "object" ? sc : {};
}
function textOf(body) {
  const c = Array.isArray(body && body.content) ? body.content.find((x) => x && x.type === "text") : null;
  return typeof (c && c.text) === "string" ? c.text : tidy(JSON.stringify(body));
}
async function tool(client, name, args) {
  const t0 = performance.now();
  const msg = await client.call("tools/call", { name, arguments: args || {} });
  const ms = Math.round(performance.now() - t0);
  if (msg.error) throw new Error(name + ": JSON-RPC " + msg.error.code + ": " + msg.error.message);
  const body = msg.result || {};
  if (body.isError === true) {
    const sc = structured(body);
    throw new Error(name + ": tool error " + (sc.code || sc.reason || "unknown") + " " + textOf(body));
  }
  return { body, sc: structured(body), ms };
}

// Negative checks need the reply's own code/reason/arg. tool() above throws on
// isError and hides that shape, so these checks inspect the raw MCP frame.
async function callRaw(client, name, args) {
  const t0 = performance.now();
  const msg = await client.call("tools/call", { name, arguments: args || {} });
  const ms = Math.round(performance.now() - t0);
  if (msg.error) throw new Error(name + ": JSON-RPC " + msg.error.code + ": " + msg.error.message);
  const body = msg.result || {};
  const sc = structured(body);
  const text = textOf(body);
  let parsed = sc;
  if (!Object.keys(sc).length) {
    try { const v = JSON.parse(text); if (v && typeof v === "object") parsed = v; } catch {}
  }
  return { body, sc, parsed, text, isError: body.isError === true, ms };
}
function errFields(reply) {
  const p = reply.parsed || {};
  return { code: p.code, reason: p.reason, arg: p.arg };
}
function errLine(label, reply) {
  const f = errFields(reply);
  return label + ": isError=" + reply.isError + " code=" + JSON.stringify(f.code) +
    " reason=" + JSON.stringify(f.reason) + " arg=" + JSON.stringify(f.arg) +
    " text=" + JSON.stringify(reply.text.slice(0, 160));
}
function requireTypedInvalidArgs(label, reply, argName) {
  requireThat(reply.sc.verdict !== true && reply.parsed.verdict !== true,
    label + " returned verdict:true on an invalid call");
  requireThat(reply.sc.ok !== true && reply.parsed.ok !== true,
    label + " returned ok:true on an invalid call");
  requireThat(reply.isError === true, errLine(label + " expected isError:true", reply));
  const f = errFields(reply);
  requireThat(f.code === "invalid_args", errLine(label + " expected typed invalid_args", reply));
  if (argName) {
    const named = f.arg === argName || String(f.arg || "").includes(argName) || reply.text.includes(argName);
    requireThat(named, errLine(label + " did not name " + argName, reply));
  }
}

const NO_CAPTURABLE = "no capturable window: need minimized:false, rect >=200x100 and a non-empty title";
function windowLabel(w) {
  const width = Math.round(w.rect[2] - w.rect[0]);
  const height = Math.round(w.rect[3] - w.rect[1]);
  return "hwnd " + w.hwnd + " " + JSON.stringify(tidy(String(w.title)).slice(0, 80)) +
    " " + width + "x" + height;
}
// The first listed window can be a 1x1 cloaked helper, so capability checks
// work from one bounded list, smallest qualifying window first.
async function capturableCandidates() {
  const t = await tool(client, "desktop_list_windows", {});
  const windows = Array.isArray(t.sc.windows) ? t.sc.windows : [];
  const ok = (w) => w && Number.isFinite(w.hwnd) && w.minimized !== true &&
    Array.isArray(w.rect) && w.rect.length === 4 && w.rect.every(Number.isFinite) &&
    (w.rect[2] - w.rect[0]) >= 200 && (w.rect[3] - w.rect[1]) >= 100 &&
    typeof w.title === "string" && w.title.trim().length > 0;
  const area = (w) => (w.rect[2] - w.rect[0]) * (w.rect[3] - w.rect[1]);
  return windows.filter(ok).sort((a, b) => area(a) - area(b) || a.hwnd - b.hwnd).slice(0, 5);
}
async function capturableWindow() {
  const candidates = await capturableCandidates();
  return candidates[0] || null;
}
function codeOf(e, sc) {
  if (sc && (sc.code || sc.error)) return String(sc.code || sc.error);
  const m = String((e && e.message) || e || "").match(/tool error ([a-z_]+)/);
  return m ? m[1] : "unknown";
}

// Shared fixture for the two contrast checks. data: is an explicitly allowed
// navigation target; the title makes the load observable without a network.
async function auditContrast(client, color) {
  const html = '<!doctype html><html><head><meta charset="utf-8"><title>audit probe</title></head>' +
    '<body style="background:#ffffff"><p id="probe" style="color:' + color +
    ';background:#ffffff">low contrast text here</p></body></html>';
  const url = "data:text/html," + encodeURIComponent(html);
  const nav = await tool(client, "browser_navigate", { url });
  requireThat(typeof nav.sc.url === "string" && nav.sc.url.startsWith("data:text/html,"),
    "audit page did not load: url=" + JSON.stringify(nav.sc.url));
  const a = await tool(client, "browser_audit", { viewports: [800], checks: ["contrast"] });
  const record = Array.isArray(a.sc.viewports) ? a.sc.viewports[0] : null;
  requireThat(record, "no viewports[0] in browser_audit reply: " + tidy(JSON.stringify(a.sc)));
  const contrast = record.contrast || {};
  return {
    top: a.sc.verdict, measured: a.sc.measured, vp: record.verdict, pass: contrast.pass,
    ratio: contrast.observed && contrast.observed.ratio, ms: nav.ms + a.ms,
  };
}

const EXPECTED_BROWSER = [
  "navigate", "snapshot", "screenshot", "click", "type", "evaluate", "wait_for", "targets",
  "dialog", "history", "network", "console", "storage", "tabs", "viewport", "select", "hover",
  "key", "upload", "layout", "assert_layout", "visual", "cdp", "close", "fetch_text", "context",
  "wheel", "drag", "form", "download", "assert", "audit", "trace", "run", "metrics", "clipboard",
];
const EXPECTED_DESKTOP = ["screenshot", "diff", "list_windows", "window", "do", "cursor", "elements", "record"];

let client = null;
let fixture = null;
let tempRoot = null;
let traceDir = null;
let setupError = null;
let keptPath = "";
let fixtureUrl = null;
let windowHwnd = null;

try {
  tempRoot = mkdtempSync(join(tmpdir(), "hand-accept-"));
  traceDir = resolve(join(tempRoot, "trace"));
  mkdirSync(traceDir, { recursive: true });

  const explicit = valueOf("--exe");
  const tried = explicit ? [explicit] : [join(root, "target", "release", "hand.exe"), join(root, "target", "debug", "hand.exe")];
  const exe = explicit ? (existsSync(explicit) ? explicit : null) : tried.find((p) => existsSync(p)) || null;
  await run("binary", () => {
    requireThat(exe, "no binary found; tried " + tried.join(", "));
    return exe + " (" + statSync(exe).size + " bytes)";
  });
  if (!exe) throw new Error("no binary found; tried " + tried.join(", "));

  try { fixture = await startFixture(); fixtureUrl = fixture.url; }
  catch (e) { fixtureUrl = null; console.error("fixture setup: " + message(e)); }
  try { client = new McpClient(exe); } catch (e) { console.error("spawn: " + message(e)); }

  await run("initialize", async () => {
    need(client, "MCP child process");
    const r = await client.call("initialize", {
      protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "hand-acceptance", version: "1" },
    });
    if (r.error) throw new Error("JSON-RPC " + r.error.code + ": " + r.error.message);
    const got = r.result && r.result.protocolVersion;
    requireThat(got === "2025-06-18", "protocolVersion echo was " + JSON.stringify(got));
    return "protocol 2025-06-18 echoed; tools capability " + !!((r.result && r.result.capabilities) || {}).tools;
  });

  await run("tools_list", async () => {
    need(client, "MCP child process");
    const r = await client.call("tools/list", {});
    if (r.error) throw new Error("JSON-RPC " + r.error.code + ": " + r.error.message);
    const names = ((r.result && r.result.tools) || []).map((t) => t && t.name).filter(Boolean);
    const missing = ["browser_", "desktop_"].flatMap((prefix, i) =>
      (i === 0 ? EXPECTED_BROWSER : EXPECTED_DESKTOP).filter((n) => !names.includes(prefix + n)).map((n) => prefix + n));
    const bad = names.filter((n) => !n.startsWith("browser_") && !n.startsWith("desktop_"));
    const b = names.filter((n) => n.startsWith("browser_")).length;
    const d = names.filter((n) => n.startsWith("desktop_")).length;
    const seen = names.length + " tools (" + b + " browser_, " + d + " desktop_)";
    requireThat(names.length === 44 && b === 36 && d === 8,
      "saw " + seen + ", expected 44 (36 browser_, 8 desktop_)");
    requireThat(names.includes("browser_metrics"), "browser_metrics missing from tools/list (" + seen + ")");
    requireThat(missing.length === 0, "missing tools: " + missing.join(", ") + " (" + seen + ")");
    requireThat(bad.length === 0, "unprefixed tools: " + bad.join(", ") + " (" + seen + ")");
    return seen + "; exact 44/36/8; all advertised names present incl browser_metrics";
  });

  await run("browser_navigate", async () => {
    need(fixtureUrl, "fixture server"); need(client, "MCP child process");
    const t = await tool(client, "browser_navigate", { url: fixtureUrl });
    return t.sc.url + " in " + t.ms + " ms";
  });

  let hitRef = null;
  await run("browser_snapshot", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_snapshot", {});
    const nodes = Array.isArray(t.sc.nodes) ? t.sc.nodes : [];
    const node = nodes.find((n) => n && n.tag === "button" && typeof n.name === "string" && /hit/i.test(n.name));
    requireThat(node && node.ref, "no @eN ref for #hit in snapshot (" + nodes.length + " nodes)");
    hitRef = "@" + String(node.ref).replace(/^@/, "");
    return hitRef + " for #hit (" + nodes.length + " nodes, " + t.ms + " ms)";
  });

  await run("browser_click_precision", async () => {
    const ref = need(hitRef, "browser_snapshot ref for #hit"); need(client, "MCP child process");
    const c = await tool(client, "browser_click", { element: ref });
    requireThat(c.sc.ok === true, "browser_click ok was " + JSON.stringify(c.sc.ok));
    const e = await tool(client, "browser_evaluate", { expression: "window.__clicks" });
    const clicks = e.sc.value;
    requireThat(Array.isArray(clicks) && clicks.length > 0, "window.__clicks empty: " + JSON.stringify(clicks));
    const last = clicks[clicks.length - 1];
    const r = last && last.rect;
    requireThat(r && [r.left, r.top, r.width, r.height, last.x, last.y].every(Number.isFinite),
      "malformed click record: " + JSON.stringify(last));
    requireThat(last.x >= r.left && last.x <= r.left + r.width && last.y >= r.top && last.y <= r.top + r.height,
      "clicked point outside button rect: " + JSON.stringify(last));
    return "click " + ref + " at (" + last.x + "," + last.y + ") inside 20x20 rect [" + r.left + "," + r.top + " " + r.width + "x" + r.height + "]";
  });

  await run("browser_type_fast", async () => {
    need(client, "MCP child process");
    const typed = "abcdefghij0123456789".repeat(10);
    const t0 = performance.now();
    const t = await tool(client, "browser_type", { element: "#name", text: typed });
    const ms = Math.round(performance.now() - t0);
    requireThat(t.sc.ok === true, "browser_type ok was " + JSON.stringify(t.sc.ok));
    const e = await tool(client, "browser_evaluate", { expression: "document.querySelector(\"#name\").value" });
    requireThat(e.sc.value === typed, "typed value mismatch: got " + JSON.stringify(e.sc.value));
    return typed.length + " chars in " + ms + " ms; exact value read back";
  });

  await run("browser_screenshot", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_screenshot", {});
    const image = Array.isArray(t.body.content) ? t.body.content.find((c) => c && c.type === "image") : null;
    requireThat(image && typeof image.data === "string", "no image content block in screenshot reply");
    const png = Buffer.from(image.data, "base64");
    requireThat(png.subarray(0, 8).toString("hex") === "89504e470d0a1a0a", "PNG magic missing");
    requireThat(png.length > 1024, "PNG too small: " + png.length + " bytes");
    return png.length + " bytes, PNG magic ok, mime " + (image.mimeType || "image/png") + ", " + t.ms + " ms";
  });

  await run("browser_assert_network", async () => {
    need(client, "MCP child process");
    await tool(client, "browser_network", { action: "get" });
    const click = await tool(client, "browser_click", { element: "#submit" });
    requireThat(click.sc.ok === true, "submit click ok was " + JSON.stringify(click.sc.ok));
    try { await tool(client, "browser_wait_for", { networkIdle: true, idleMs: 500 }); } catch {}
    let a = await tool(client, "browser_assert", { kind: "network", expected: { url: "/submit", status: 200 } });
    let form = "exact /submit";
    if (a.sc.verdict !== true) {
      // The CDP ring stores absolute URLs; a bare path is not an exact match.
      a = await tool(client, "browser_assert", { kind: "network", expected: { url: { contains: "/submit" }, status: 200 } });
      form = "contains /submit";
    }
    requireThat(a.sc.verdict === true, "verdict " + JSON.stringify(a.sc.verdict) + ", observed " + tidy(JSON.stringify(a.sc.observed)));
    return "verdict:true (" + form + ", POST /submit -> 200) in " + a.ms + " ms";
  });

  await run("browser_metrics", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_metrics", {});
    const s = t.sc;
    requireThat(Number.isFinite(s.ttfbMs) && s.ttfbMs >= 0, "ttfbMs not finite >= 0: " + JSON.stringify(s.ttfbMs));
    requireThat(Number.isFinite(s.domContentLoadedMs) && s.domContentLoadedMs >= 0,
      "domContentLoadedMs not finite >= 0: " + JSON.stringify(s.domContentLoadedMs));
    requireThat(typeof s.cls === "number", "cls not a number: " + JSON.stringify(s.cls));
    return "ttfbMs=" + s.ttfbMs + " domContentLoadedMs=" + s.domContentLoadedMs + " cls=" + s.cls + " in " + t.ms + " ms";
  });

  await run("browser_trace", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_trace", { dir: traceDir, video: true, videoDurationMs: 1200 });
    const avi = join(traceDir, "trace.avi");
    const bytes = readFileSync(avi);
    requireThat(bytes.subarray(0, 4).toString("latin1") === "RIFF", "trace.avi has no RIFF magic");
    const manifest = JSON.parse(readFileSync(join(traceDir, "manifest.json"), "utf8"));
    const frames = manifest.video && manifest.video.frames;
    const declared = manifest.video && manifest.video.bytes;
    requireThat(Number.isInteger(frames) && frames > 0, "manifest video.frames invalid: " + JSON.stringify(frames));
    return "trace.avi " + bytes.length + " bytes (manifest " + declared + "), " + frames + " frames, RIFF ok, " + t.ms + " ms";
  });

  // New capability: real tab lifecycle over the same engine instance.
  await run("browser_tabs_roundtrip", async () => {
    need(fixtureUrl, "fixture server"); need(client, "MCP child process");
    const list = async () => {
      const t = await tool(client, "browser_tabs", { action: "list" });
      requireThat(t.sc.ok === true, "browser_tabs list ok was " + JSON.stringify(t.sc.ok));
      requireThat(Array.isArray(t.sc.tabs), "browser_tabs list tabs not an array: " + tidy(JSON.stringify(t.sc)));
      return t;
    };
    const first = await list();
    const n0 = first.sc.tabs.length;
    requireThat(n0 >= 1, "initial tab count " + n0 + " < 1");
    const before = await tool(client, "browser_evaluate", { expression: "document.title" });
    const firstTitle = before.sc.value;
    requireThat(typeof firstTitle === "string", "document.title not a string: " + JSON.stringify(firstTitle));
    const tab2 = new URL("tab2", fixtureUrl).href;
    const opened = await tool(client, "browser_tabs", { action: "new", url: tab2 });
    requireThat(opened.sc.ok === true, "browser_tabs new ok was " + JSON.stringify(opened.sc.ok));
    let newIndex = Array.isArray(opened.sc.tabs) ? opened.sc.tabs.length - 1 : -1;
    let closed = false;
    try {
      const afterNew = await list();
      const n1 = afterNew.sc.tabs.length;
      requireThat(n1 === n0 + 1, "after new: " + n1 + " tabs, expected " + (n0 + 1));
      newIndex = n1 - 1;
      const sel = await tool(client, "browser_tabs", { action: "select", index: 0 });
      requireThat(sel.sc.ok === true, "browser_tabs select ok was " + JSON.stringify(sel.sc.ok));
      const titleT = await tool(client, "browser_evaluate", { expression: "document.title" });
      requireThat(titleT.sc.value === firstTitle,
        "title at index 0 was " + JSON.stringify(titleT.sc.value) + ", first page " + JSON.stringify(firstTitle));
      const close = await tool(client, "browser_tabs", { action: "close", index: newIndex });
      closed = true;
      requireThat(close.sc.ok === true, "browser_tabs close ok was " + JSON.stringify(close.sc.ok));
      const afterClose = await list();
      const n2 = afterClose.sc.tabs.length;
      requireThat(n2 === n0, "after close: " + n2 + " tabs, expected " + n0);
      return "list=" + n0 + " -> new " + tab2 + " -> list=" + n1 +
        " -> select index 0 title=" + JSON.stringify(firstTitle) + " -> close (index " + newIndex +
        ") -> list=" + n2 + " in " + (first.ms + afterNew.ms + sel.ms + close.ms + afterClose.ms) + " ms";
    } finally {
      // A failed assertion must not leave a scratch tab behind.
      if (newIndex >= 0 && !closed && client && !client.exited) {
        try { await tool(client, "browser_tabs", { action: "close", index: newIndex }); } catch {}
      }
    }
  });

  // PAGE clipboard via navigator.clipboard IS the Windows clipboard in
  // Chromium. The write is destructive and opted in here; no desktop tool
  // reads, writes or restores the OS clipboard in this check.
  await run("browser_clipboard_roundtrip", async () => {
    need(client, "MCP child process");
    const marker = "hand-clip-" + randomBytes(6).toString("hex");
    const wrote = await tool(client, "browser_clipboard", {
      action: "write", text: marker, allow_clipboard: true,
    });
    requireThat(wrote.sc.ok !== false, "browser_clipboard write ok=" + JSON.stringify(wrote.sc.ok));
    requireThat(wrote.sc.systemClipboard === true,
      "browser_clipboard write reply systemClipboard=" + JSON.stringify(wrote.sc.systemClipboard) +
      ", expected true (field absent)");
    const read = await tool(client, "browser_clipboard", { action: "read" });
    const got = read.sc && read.sc.text !== undefined ? read.sc.text : read.sc.value;
    requireThat(got === marker, "page clipboard read back " + JSON.stringify(got) + ", wrote " + JSON.stringify(marker));
    return "wrote " + marker + " via navigator.clipboard, read back exact in " + read.ms +
      " ms; page clipboard via navigator.clipboard = Windows clipboard; write opted in";
  });

  // The destructive page-side write needs its explicit opt-in. Without it the
  // tool must refuse before touching the page or the clipboard.
  await run("browser_clipboard_write_requires_opt_in", async () => {
    need(client, "MCP child process");
    const r = await callRaw(client, "browser_clipboard", {
      action: "write", text: "hand-must-not-write",
    });
    const sc = r.sc || {};
    requireThat(sc.ok !== true && r.body.isError !== false,
      "browser_clipboard write without opt-in returned ok:true; the clipboard was written");
    requireThat(r.isError === true || sc.ok === false,
      "browser_clipboard write without opt-in was not refused: isError=" + JSON.stringify(r.body.isError) +
      ", ok=" + JSON.stringify(sc.ok));
    requireThat(sc.code === "no_clipboard_policy" || /no_clipboard_policy/.test(r.text || ""),
      "browser_clipboard write without opt-in carried no no_clipboard_policy: " + errLine("reply", r));
    return "typed no_clipboard_policy (isError:true); refused write left the clipboard alone in " +
      r.ms + " ms";
  });

  // Adversarial pass: absent text must yield an honest strict-false verdict.
  await run("browser_assert_false_verdict", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_assert", {
      kind: "text", expected: { contains: "zzz-definitely-absent-from-page" },
    });
    requireThat(t.sc.verdict !== true, "assertion passed on absent text: verdict:true");
    requireThat(t.sc.verdict === false, "verdict not strictly false: " + JSON.stringify(t.sc.verdict));
    // The engine omits isError on a clean reply; false and absent both mean
    // "not a tool error". A true (or any other value) is a FAIL.
    requireThat(t.body.isError === false || t.body.isError === undefined,
      "browser_assert isError was " + JSON.stringify(t.body.isError) + " (expected false)");
    return "verdict:false (strict boolean), isError not true (" + JSON.stringify(t.body.isError) +
      ") for absent needle in " + t.ms + " ms";
  });

  // An unsupported expectation key must be typed, never downgraded into an
  // empty needle that would match every page.
  await run("browser_assert_rejects_unknown_expectation", async () => {
    need(client, "MCP child process");
    const r = await callRaw(client, "browser_assert", { kind: "text", expected: { text: "x" } });
    requireTypedInvalidArgs("browser_assert expected.text", r, "expected.text");
    return "typed invalid_args naming expected.text, verdict not true, isError:true in " + r.ms + " ms";
  });

  // A false verdict inside browser_run is still a failed step: the batch
  // cannot report ok:true/failed:0 when its assertion did not hold.
  await run("browser_run_assert_failure", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_run", {
      steps: [{ tool: "assert", args: { kind: "text", expected: { contains: "zzz-definitely-absent-from-page" } } }],
    });
    requireThat(t.sc.ok !== true, "browser_run ok:true after a failing assertion");
    requireThat(t.sc.ok === false, "browser_run ok was not strictly false: " + JSON.stringify(t.sc.ok));
    requireThat(Number.isInteger(t.sc.failed) && t.sc.failed >= 1,
      "browser_run failed was " + JSON.stringify(t.sc.failed) + ", expected >= 1");
    const steps = Array.isArray(t.sc.steps) ? t.sc.steps : [];
    const step = steps.find((s) => s && s.tool === "assert" && s.status !== "ok");
    requireThat(step, "no failed assert step in browser_run steps: " + tidy(JSON.stringify(steps)));
    const evidence = step.error || step.result || {};
    requireThat(evidence.verdict !== true, "failed assert step carries verdict:true");
    return "ok:false, failed=" + t.sc.failed + ", step " + step.index + " status=" + step.status +
      " (assert verdict=" + JSON.stringify(evidence.verdict) + ") in " + t.ms + " ms";
  });

  // browser_trace's runtime contract requires dir; a schema change that
  // silently defaults it must be visible here.
  await run("browser_trace_dir_required", async () => {
    need(client, "MCP child process");
    const r = await callRaw(client, "browser_trace", {});
    requireTypedInvalidArgs("browser_trace dir", r, "dir");
    return "typed invalid_args naming dir, verdict not true, isError:true in " + r.ms + " ms";
  });

  // The top-level verdict must follow the requested check result, not merely
  // report that a measurement happened. measured stays a separate signal.
  await run("browser_audit_contrast_fails", async () => {
    need(client, "MCP child process");
    const a = await auditContrast(client, "#eeeeee");
    requireThat(a.top === "fail",
      "top-level verdict " + JSON.stringify(a.top) + " contradicts the expected fail (contrast.pass=" +
      JSON.stringify(a.pass) + ", viewports[0].verdict=" + JSON.stringify(a.vp) +
      ", measured=" + JSON.stringify(a.measured) + ")");
    requireThat(a.measured === "pass", "audit measured was " + JSON.stringify(a.measured) + ", expected pass");
    requireThat(a.pass === false, "contrast.pass was " + JSON.stringify(a.pass) + ", expected false");
    requireThat(a.vp === "fail", "viewports[0].verdict was " + JSON.stringify(a.vp) + ", expected fail");
    return "verdict:fail, measured:pass, contrast.pass:false (ratio=" + JSON.stringify(a.ratio) + ") in " + a.ms + " ms";
  });

  // Positive control: an audit that always fails must not satisfy the check above.
  await run("browser_audit_contrast_passes", async () => {
    need(client, "MCP child process");
    const a = await auditContrast(client, "#111111");
    requireThat(a.top === "pass", "top-level verdict was " + JSON.stringify(a.top) + ", expected pass");
    requireThat(a.pass === true, "contrast.pass was " + JSON.stringify(a.pass) + ", expected true");
    return "verdict:pass, contrast.pass:true (ratio=" + JSON.stringify(a.ratio) + ") in " + a.ms + " ms";
  });

  await run("browser_close", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "browser_close", {});
    requireThat(t.sc.ok === true, "browser_close ok was " + JSON.stringify(t.sc.ok));
    return "browser closed in " + t.ms + " ms";
  });

  await run("desktop_list_windows", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "desktop_list_windows", {});
    const windows = Array.isArray(t.sc.windows) ? t.sc.windows : [];
    requireThat(windows.length > 0, "no visible top-level windows");
    const withHwnd = windows.find((w) => w && Number.isFinite(w.hwnd));
    if (withHwnd) windowHwnd = withHwnd.hwnd;
    return windows.length + " windows; first hwnd=" + (withHwnd ? withHwnd.hwnd : "n/a") + " in " + t.ms + " ms";
  });

  await run("desktop_cursor", async () => {
    need(client, "MCP child process");
    const t = await tool(client, "desktop_cursor", {});
    requireThat(Number.isFinite(t.sc.x) && Number.isFinite(t.sc.y),
      "cursor x/y not numeric: " + tidy(JSON.stringify(t.sc)));
    return "x=" + t.sc.x + " y=" + t.sc.y + " (read only, pointer untouched) in " + t.ms + " ms";
  });

  await run("desktop_do_focus_guard", async () => {
    need(client, "MCP child process");
    const w = await capturableWindow();
    requireThat(w, NO_CAPTURABLE);
    const t = await tool(client, "desktop_do", {
      actions: [{ type: "wait", ms: 50 }], target: { hwnd: w.hwnd }, focus_guard: true,
    });
    requireThat(t.sc.foregroundUnchanged === true, windowLabel(w) + " foregroundUnchanged was " +
      JSON.stringify(t.sc.foregroundUnchanged));
    return windowLabel(w) + " foregroundUnchanged:true in " + t.ms + " ms";
  });

  // Self-sufficient clipboard proof. An owner clipboard with no text is a
  // valid starting state ("original: none"), not a failure: the marker is
  // written and read inside one allow_clipboard/restore_clipboard burst and
  // the ledger plus a post-burst read prove the restore.
  await run("desktop_clipboard_roundtrip", async () => {
    need(client, "MCP child process");
    const readText = async () => {
      const r = await callRaw(client, "desktop_do", { actions: [{ type: "clipboard_get" }] });
      const a = (r.sc.results || [])[0] || {};
      if (a.ok === true && typeof a.text === "string") return a.text;
      const why = String(a.error || a.code || r.text || "");
      // All three engines' "no text" signals are a valid starting state. A
      // real failure (OpenClipboard/GlobalLock or a retry that never landed)
      // stays a FAIL and names its stage.
      const hardStage = /stage=(OpenClipboard|GlobalLock)/i.test(why);
      const noText = !hardStage && /no CF_UNICODETEXT|no_text_format|non.?text|stage=IsClipboardFormatAvailable/i.test(why);
      if (noText) return null;
      throw new Error("clipboard_get failed: " + tidy(why));
    };
    // Evidence never carries clipboard content: only lengths and verdicts.
    const shape = (v) => v === null ? "none" : "len=" + String(v).length;
    const same = (a, b) => a !== null && b !== null && a === b;
    let original = null, originalWasText = false, wrote = false, inBurst = null, after = null;
    const observed = () => "in-burst=" + shape(inBurst) + ", post-burst=" + shape(after) +
      ", original=" + (originalWasText ? shape(original) : "none");
    try {
      original = await readText();
      originalWasText = original !== null;
      const marker = "hand-accept-" + randomBytes(6).toString("hex");
      wrote = true;
      const burst = await callRaw(client, "desktop_do", {
        allow_clipboard: true, restore_clipboard: true,
        actions: [{ type: "clipboard_set", text: marker }, { type: "clipboard_get" }],
      });
      const got = (burst.sc.results || []).find((r) => r && r.type === "clipboard_get");
      if (got && got.ok === true && typeof got.text === "string") inBurst = got.text;
      const ledger = burst.sc.clipboard || {};
      requireThat(got && got.ok === true, "clipboard_get in burst failed: " +
        tidy(String((got && got.error) || burst.text)));
      requireThat(same(inBurst, marker), "in-burst marker mismatch: got " + shape(inBurst) +
        ", wrote len=" + String(marker).length);
      requireThat(ledger.writes === 1 && ledger.restores === 1,
        "clipboard ledger writes=" + ledger.writes + " restores=" + ledger.restores);
      // restore_clipboard restores at the END of the burst, so read after it returns.
      after = await readText();
      const restoreMatched = originalWasText ? same(after, original) : after === null;
      requireThat(restoreMatched, "post-burst clipboard " + shape(after) + " differs from original " +
        (originalWasText ? shape(original) : "none"));
      return "original=" + (originalWasText ? "text len=" + String(original).length : "none") +
        ", marker len=" + String(marker).length + " read in burst, post-burst matched, clipboard writes=1 restores=1";
    } catch (e) {
      throw new Error(message(e) + " | observed " + observed());
    } finally {
      // Only a text original can be put back exactly. An empty or non-text
      // clipboard is restored by the burst's restore_clipboard; guessing at
      // it here would leave the owner's clipboard changed.
      if (wrote && originalWasText && client && !client.exited) {
        try {
          const now = await readText();
          if (now !== original) {
            await tool(client, "desktop_do", { allow_clipboard: true, actions: [{ type: "clipboard_set", text: original }] });
          }
        } catch { /* best effort; the burst itself already restores on failure */ }
      }
    }
  });

  // Resize a real window by -40/-40, trying candidates until one actually
  // changes size. Every changed window is restored in a finally; never moved.
  await run("desktop_window_resize", async () => {
    need(client, "MCP child process");
    const rectOk = (r) => Array.isArray(r) && r.length === 4 && r.every(Number.isFinite);
    const candidates = await capturableCandidates();
    requireThat(candidates.length > 0, NO_CAPTURABLE);
    let tried = 0, lastCode = "unknown", lastLabel = "";
    for (const target of candidates) {
      tried++;
      const hwnd = target.hwnd;
      const label = windowLabel(target);
      lastLabel = label;
      const origW = Math.round(target.rect[2] - target.rect[0]);
      const origH = Math.round(target.rect[3] - target.rect[1]);
      const wantW = origW - 40, wantH = origH - 40;
      const findRect = async () => {
        const t = await tool(client, "desktop_list_windows", {});
        const ws = Array.isArray(t.sc.windows) ? t.sc.windows : [];
        const w = ws.find((x) => x && x.hwnd === hwnd);
        return w && rectOk(w.rect) ? w.rect : null;
      };
      let sent = false, ms = 0;
      try {
        let r;
        try {
          r = await tool(client, "desktop_window", { action: "resize", hwnd, width: wantW, height: wantH });
        } catch (e) { lastCode = codeOf(e); continue; }
        ms = r.ms;
        if (r.sc.ok === false) { lastCode = codeOf(null, r.sc); continue; }
        sent = true;
        const rect1 = await findRect();
        if (!rect1) { lastCode = "no_rect_after_resize"; continue; }
        const gotW = Math.round(rect1[2] - rect1[0]);
        const gotH = Math.round(rect1[3] - rect1[1]);
        if (Math.abs(gotW - wantW) > 8 || Math.abs(gotH - wantH) > 8) {
          lastCode = "resize_no_change (got " + gotW + "x" + gotH + ")";
          continue;
        }
        return label + " " + origW + "x" + origH + " -> " + gotW + "x" + gotH +
          " (want " + wantW + "x" + wantH + ", within 8 px) in " + ms +
          " ms; tried " + tried + " candidates; position untouched, original size restored in finally";
      } finally {
        if (sent && client && !client.exited) {
          let rr;
          try {
            rr = await tool(client, "desktop_window", { action: "resize", hwnd, width: origW, height: origH });
          } catch (e) { throw new Error(label + " restore failed code=" + codeOf(e)); }
          requireThat(rr.sc.ok !== false, label + " restore refused code=" + codeOf(null, rr.sc));
          const rect2 = await findRect();
          requireThat(rect2, label + " no rect after restore");
          const rw = Math.round(rect2[2] - rect2[0]);
          const rh = Math.round(rect2[3] - rect2[1]);
          requireThat(Math.abs(rw - origW) <= 8 && Math.abs(rh - origH) <= 8,
            label + " restore mismatch: got " + rw + "x" + rh + ", wanted " + origW + "x" + origH);
        }
      }
    }
    throw new Error("all " + tried + " candidates refused resize; last code=" + lastCode +
      " (" + lastLabel + ")");
  });

  // Record the first candidate that really yields frames. Every .avi this
  // check creates is deleted in finally, including partial failed attempts.
  await run("desktop_record", async () => {
    need(client, "MCP child process");
    const candidates = await capturableCandidates();
    requireThat(candidates.length > 0, NO_CAPTURABLE);
    const outs = [];
    const codes = [];
    try {
      for (const w of candidates) {
        const hwnd = w.hwnd;
        const where = windowLabel(w) + " rect " + JSON.stringify(w.rect);
        const out = resolve(join(tempRoot, "desktop-record-" + hwnd + ".avi"));
        outs.push(out);
        let t;
        try {
          t = await tool(client, "desktop_record", {
            target: { hwnd }, duration_ms: 800, fps: 6, max_px: 480, out,
          });
        } catch (e) { codes.push(hwnd + ":" + codeOf(e)); continue; }
        if (t.sc.ok !== true) { codes.push(hwnd + ":" + codeOf(null, t.sc)); continue; }
        if (!existsSync(out)) { codes.push(hwnd + ":missing_avi"); continue; }
        const bytes = readFileSync(out);
        if (bytes.subarray(0, 4).toString("latin1") !== "RIFF") { codes.push(hwnd + ":no_riff"); continue; }
        const frames = Number.isFinite(t.sc.frames) ? t.sc.frames : (t.sc.video && t.sc.video.frames);
        if (!Number.isInteger(frames) || frames < 2) { codes.push(hwnd + ":frames=" + JSON.stringify(frames)); continue; }
        if (t.sc.foregroundUnchanged !== true) { codes.push(hwnd + ":foreground_changed"); continue; }
        if (t.sc.cursorUnchanged !== true) { codes.push(hwnd + ":cursor_changed"); continue; }
        const fps = Number.isFinite(t.sc.fps) ? t.sc.fps : (t.sc.video && t.sc.video.fps);
        return where + " record.avi " + bytes.length + " bytes, RIFF ok, " + frames + " frames, fps=" +
          JSON.stringify(fps) + ", foregroundUnchanged=true, cursorUnchanged=true in " + t.ms +
          " ms; tried " + codes.length + " candidates";
      }
      throw new Error("all " + candidates.length + " candidates failed record: " + codes.join(", "));
    } finally {
      for (const p of outs) { try { rmSync(p, { force: true }); } catch {} }
    }
  });

  // Empty actions must be a typed invalid_args; ok:true would present a
  // no-op as a delivered burst.
  await run("desktop_do_empty_actions", async () => {
    need(client, "MCP child process");
    const r = await callRaw(client, "desktop_do", { actions: [] });
    requireTypedInvalidArgs("desktop_do actions:[]", r);
    return "typed invalid_args, ok not true, isError:true in " + r.ms + " ms";
  });

  // A no-op move to the window's own rect with allow_foreground:true must be
  // accepted without taking focus or moving the window. Nothing here selects,
  // focuses, minimizes, screenshots or moves the pointer.
  await run("desktop_window_noop_move_with_allow_foreground", async () => {
    need(client, "MCP child process");
    const rectOk = (v) => Array.isArray(v) && v.length === 4 && v.every(Number.isFinite);
    const findRect = async (hwnd) => {
      const list = await tool(client, "desktop_list_windows", {});
      const ws = Array.isArray(list.sc.windows) ? list.sc.windows : [];
      const w = ws.find((x) => x && x.hwnd === hwnd);
      return w && rectOk(w.rect) ? w.rect : null;
    };
    const target = await capturableWindow();
    requireThat(target, NO_CAPTURABLE);
    const hwnd = target.hwnd;
    const before = target.rect;
    const label = windowLabel(target);
    const r = await callRaw(client, "desktop_window", {
      action: "move", hwnd, x: before[0], y: before[1], allow_foreground: true,
    });
    requireThat(r.sc.code !== "no_foreground_policy", label + " " + errLine("desktop_window no-op move", r));
    requireThat(r.sc.ok === true, label + " " + errLine("desktop_window no-op move ok was not true", r));
    const after = await findRect(hwnd);
    requireThat(after, label + " no rect after no-op move");
    const moved = Math.max(Math.abs(after[0] - before[0]), Math.abs(after[1] - before[1]));
    requireThat(moved <= 8, label + " window moved by " + moved + " px: " +
      JSON.stringify(before) + " -> " + JSON.stringify(after));
    return label + " ok:true, rect " + JSON.stringify(before) + " unchanged (max delta " +
      moved + " px), no_foreground_policy absent in " + r.ms + " ms";
  });
} catch (e) {
  if (!setupError) setupError = message(e);
} finally {
  for (const name of CHECKS) {
    if (!results.has(name)) record(name, false, "not run: " + (setupError || "setup incomplete"));
  }
  if (client) client.close();
  if (fixture && fixture.server) {
    try { fixture.server.close(); if (fixture.server.closeAllConnections) fixture.server.closeAllConnections(); } catch {}
  }
  if (tempRoot) {
    if (keep) keptPath = tempRoot;
    else { try { rmSync(tempRoot, { recursive: true, force: true }); } catch {} }
  }
}

let passed = 0;
for (const name of CHECKS) {
  const r = results.get(name) || { ok: false, detail: "not run" };
  if (r.ok) passed++;
  console.log((r.ok ? "PASS" : "FAIL") + "  " + name + "  " + tidy(r.detail));
}
if (keptPath) console.log("KEPT: " + keptPath);
console.log(passed === CHECKS.length ? "RESULT: " + passed + "/" + CHECKS.length + " PASS" : "RESULT: FAILED");
process.exitCode = passed === CHECKS.length ? 0 : 1;