// Deterministic MCP stdio fixture for driver tests only.
//
// THIS IS NOT A PRODUCT AND ITS RESULTS ARE NOT EVIDENCE OF LIVE BEHAVIOR.
// It answers a fixed script, touches no browser and no desktop, and every reply
// carries a test-only marker. bench.mjs marks any run against it testOnly:true
// and refuses to treat it as measured product behavior.
//
// Modes (all deterministic):
//   --mode ok               valid, complete output
//   --mode incomplete       structurally valid snapshot that fails completeness
//   --mode fail-every=<n>   every Nth ping/tools-call returns a JSON-RPC error
//   --mode exit-after=<n>   exit(17) before replying to request N+1
//   --mode hang-on=<n>      never reply to request N (driver timeout test)
//   --delay-ms=<n>          wait n ms before replying
//   --bad-json              emit one non-JSON frame before the first reply
//   --window-title=<s>      controlled window title (default hand-bench-fixture)
//   --image-size=WxH        capture image/source size (default 1280x720)

import { createInterface } from "node:readline";

import { MODE_B_NOTE } from "../scripts/lib/scenarios.mjs";

const args = new Map();
for (let index = 2; index < process.argv.length; index += 1) {
  const raw = process.argv[index];
  if (!raw.startsWith("--")) continue;
  const body = raw.slice(2);
  const equals = body.indexOf("=");
  const key = equals === -1 ? body : body.slice(0, equals);
  const inline = equals === -1 ? undefined : body.slice(equals + 1);
  const value = inline !== undefined ? inline : process.argv[index + 1]?.startsWith("--") ? "" : process.argv[++index];
  args.set(key, value ?? "");
}

const mode = args.get("mode") ?? "ok";
const failEvery = Number((/^fail-every=(\d+)$/.exec(mode) ?? [])[1] ?? args.get("fail-every") ?? 0);
const exitAfter = Number((/^exit-after=(\d+)$/.exec(mode) ?? [])[1] ?? args.get("exit-after") ?? 0);
const hangOn = Number((/^hang-on=(\d+)$/.exec(mode) ?? [])[1] ?? args.get("hang-on") ?? 0);
const delayMs = Number((/^delay=(\d+)$/.exec(mode) ?? [])[1] ?? args.get("delay-ms") ?? 0);
const incomplete = mode === "incomplete";
const incompleteAfter = Number((/^incomplete-after=(\d+)$/.exec(mode) ?? [])[1] ?? -1);
let snapshotCalls = 0;
const badJson = mode === "bad-json" || args.has("bad-json");
const windowTitle = args.get("window-title") || process.env.FAKE_WINDOW_TITLE || "hand-bench-fixture";
const imageSize = /^(\d+)x(\d+)$/.exec(args.get("image-size") || "1280x720");
const imageWidth = Number(imageSize?.[1] ?? 1280);
const imageHeight = Number(imageSize?.[2] ?? 720);
const browserLog = args.get("browser-log") || "actual";
const captureDefect = args.get("capture-defect") || null;
const captureDefectAfter = Number(args.get("capture-defect-after") || 0);
let captureCalls = 0;

let requestCount = 0;
let toolCalls = 0;
let emittedBadJson = false;
let pending = Promise.resolve();
let exiting = false;

process.stderr.write(
  `[hand-benchmark-fake-mcp] TEST FIXTURE ONLY — mode=${mode} failEvery=${failEvery} exitAfter=${exitAfter} hangOn=${hangOn} delayMs=${delayMs}\n`,
);
if (browserLog !== "none") {
  const logged = browserLog === "other" ? process.execPath : process.env.GLADIUS_CHROMIUM;
  if (logged) process.stderr.write(`[gladius] backend: chromium ${logged} (port 9222)\n`);
}

const reader = createInterface({ input: process.stdin, crlfDelay: Infinity });
reader.on("line", (line) => {
  if (!line.trim()) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } });
    return;
  }
  pending = pending.then(() => handle(message)).catch((error) => {
    process.stderr.write(`[fake] handler error: ${error?.stack ?? error}\n`);
  });
});

function send(payload) {
  if (exiting) return;
  process.stdout.write(`${JSON.stringify(payload)}\n`);
}

function reply(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function replyError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

async function handle(message) {
  if (message.id === undefined) {
    if (message.method === "notifications/initialized") return;
    return;
  }
  requestCount += 1;

  if (requestCount === hangOn) {
    // Deliberately never replies: the driver must time out and count a failure.
    return;
  }
  if (exitAfter > 0 && requestCount > exitAfter) {
    exiting = true;
    setTimeout(() => process.exit(17), 0);
    return;
  }
  if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
  if (badJson && !emittedBadJson && (message.method === "ping" || message.method === "tools/call")) {
    emittedBadJson = true;
    process.stdout.write("this is not a json-rpc frame\n");
  }

  if (message.method === "initialize") {
    reply(message.id, {
      protocolVersion: "2025-11-25",
      capabilities: { tools: { listChanged: false } },
      serverInfo: {
        name: "hand-benchmark-fake-mcp",
        version: "0.0.0-test",
        title: "TEST FIXTURE ONLY",
      },
      instructions:
        "Deterministic driver-test fixture. Nothing here proves live browser or desktop behavior.",
    });
    return;
  }

  if (message.method === "ping") {
    if (failEvery > 0 && requestCount % failEvery === 0) {
      replyError(message.id, -32000, `fixture failure at request ${requestCount}`);
      return;
    }
    reply(message.id, {});
    return;
  }

  if (message.method === "tools/list") {
    reply(message.id, { tools: toolDefinitions() });
    return;
  }

  if (message.method === "tools/call") {
    toolCalls += 1;
    if (failEvery > 0 && toolCalls % failEvery === 0) {
      replyError(message.id, -32000, `fixture failure at tool call ${toolCalls}`);
      return;
    }
    const name = message.params?.name;
    const toolArgs = message.params?.arguments ?? {};
    handleTool(message.id, name, toolArgs);
    return;
  }

  replyError(message.id, -32601, `method not found: ${message.method}`);
}

function toolResult(payload) {
  return {
    content: [{ type: "text", text: JSON.stringify(payload) }],
    structuredContent: payload,
  };
}

function handleTool(id, name, toolArgs) {
  switch (name) {
    case "navigate":
    case "browser_navigate": {
      const url = String(toolArgs.url ?? "http://127.0.0.1:1/index.html");
      reply(id, toolResult({ url, title: "hand benchmark fixture — 100 controls" }));
      return;
    }
    case "viewport":
    case "browser_viewport": {
      reply(
        id,
        toolResult({
          applied: { width: 1280, height: 720, deviceScaleFactor: 1 },
          matched: true,
          readback: { innerWidth: 1280, innerHeight: 720, devicePixelRatio: 1 },
        }),
      );
      return;
    }
    case "snapshot":
    case "browser_snapshot": {
      snapshotCalls += 1;
      const isIncomplete = incomplete || (incompleteAfter >= 0 && snapshotCalls > incompleteAfter);
      const count = isIncomplete ? 3 : 100;
      const nodes = Array.from({ length: count }, (_, index) => ({
        ref: `@e${index + 1}`,
        role: "button",
        name: `Control ${String(index + 1).padStart(3, "0")}`,
        tag: "button",
        disabled: false,
        value: "",
      }));
      reply(
        id,
        toolResult({
          url: "http://127.0.0.1:1/index.html",
          title: "hand benchmark fixture — 100 controls",
          truncated: incomplete,
          nodes,
          snapshotId: "snap-1",
          totalLines: nodes.length,
          nextOffset: incomplete ? 3 : null,
          text: nodes.map((node) => `${node.ref} ${node.role} "${node.name}"`).join("\n"),
        }),
      );
      return;
    }
    case "list_windows":
    case "desktop_list_windows": {
      reply(
        id,
        toolResult({
          windows: [
            {
              hwnd: 4242,
              pid: 4242,
              exe: "fake-fixture.exe",
              title: windowTitle,
              class: "FakeFixtureWindow",
              rect: [100, 100, 100 + imageWidth + 16, 100 + imageHeight + 39],
              dwm_rect: [100, 100, 100 + imageWidth, 100 + imageHeight],
              client_rect: [100, 100, 100 + imageWidth, 100 + imageHeight],
              minimized: false,
              maximized: false,
              foreground: false,
              cloaked: false,
              visible: true,
              dpi: 96,
            },
            {
              hwnd: 4243,
              pid: 4243,
              exe: "other.exe",
              title: "unrelated window",
              class: "Other",
              rect: [0, 0, 10, 10],
              client_rect: [0, 0, 10, 10],
              minimized: false,
              visible: true,
              dpi: 96,
            },
          ],
        }),
      );
      return;
    }
    case "screenshot":
    case "desktop_screenshot": {
      captureCalls += 1;
      const defect = captureDefect && captureCalls > captureDefectAfter ? captureDefect : null;
      const payload = {
        ok: true,
        image: { w: imageWidth, h: imageHeight, format: "png", quality: null },
        source: { x: 100, y: 100, w: imageWidth, h: imageHeight, kind: "window" },
        scale: 1,
        monitors: { count: 1 },
        cursor: { x: 0, y: 0 },
        formula: "screen_x = source.x + round((img_x + 0.5) * source.w / image.w)",
        capture: {
          backend: "printwindow",
          moved_during_capture: false,
          stale: false,
          blank_check: { verdict: "content", max_channel: 255, threshold: 8 },
          note: MODE_B_NOTE,
          method: "PrintWindow",
        },
        foregroundUnchanged: true,
        cursorUnchanged: true,
        window: {
          hwnd: 4242,
          pid: 4242,
          exe: "fake-fixture.exe",
          title: windowTitle,
          minimized: false,
          cloaked: 0,
          dwm_rect: [100, 100, 100 + imageWidth, 100 + imageHeight],
        },
      };
      if (defect === "blank") payload.capture.blank_check = { verdict: "full_blank", max_channel: 0, threshold: 8 };
      if (defect === "moved") payload.capture.moved_during_capture = true;
      if (defect === "stale") payload.capture.stale = true;
      if (defect === "crop") payload.capture.backend = "desktop-crop:gdi";
      if (defect === "note") payload.capture.note = "PrintWindow refused; desktop crop used";
      if (defect === "unsafe") payload.foregroundUnchanged = false;
      if (defect === "cursor") payload.cursorUnchanged = false;
      if (defect === "no-window") delete payload.window;
      if (defect === "warnings") payload.warnings = [{ code: "quality_ignored", field: "quality" }];
      if (defect === "minimized") payload.window.minimized = true;
      reply(id, toolResult(payload));
      return;
    }
    default:
      replyError(id, -32602, `unknown tool: ${name}`);
  }
}

function toolDefinitions() {
  const names = [
    "navigate",
    "viewport",
    "snapshot",
    "list_windows",
    "screenshot",
    "browser_navigate",
    "browser_viewport",
    "browser_snapshot",
    "desktop_list_windows",
    "desktop_screenshot",
  ];
  return names.map((name) => ({
    name,
    description: "TEST FIXTURE ONLY — deterministic response, no live behavior",
    inputSchema: { type: "object", additionalProperties: true },
    annotations: { testOnly: true },
  }));
}
