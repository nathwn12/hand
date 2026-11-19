#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { canonicalHash, prettyCanonical } from "./lib/canonical.mjs";
import { indexTools, schemaHash } from "./lib/catalog.mjs";
import { StdioClient } from "./lib/mcp-stdio.mjs";

const PROTOCOL = "2025-11-25";
const MODERN = "2026-07-28";

export async function captureCatalog({
  command,
  args = [],
  label,
  outPrefix,
  cwd,
  timeoutMs = 30_000,
  versionArgs = ["--version"],
  write = true,
}) {
  const executable = resolve(command);
  const started = new Date();

  const version = versionArgs.length
    ? runOnce(executable, versionArgs, cwd, 15_000)
    : { skipped: true };

  const client = new StdioClient({ command: executable, args, cwd, timeoutMs });
  const timings = {};
  let init;
  let ping;
  let tools;
  let toolsModern;
  let discover;
  let exit;
  try {
    init = await client.request("initialize", {
      protocolVersion: PROTOCOL,
      capabilities: {},
      clientInfo: { name: "hand-baseline-capture", version: "1.0.0" },
    });
    timings.initializeMs = init.elapsedMs;
    client.notify("notifications/initialized", {});

    ping = await client.request("ping", {});
    timings.pingMs = ping.elapsedMs;

    tools = await client.request("tools/list", {});
    timings.toolsListMs = tools.elapsedMs;

    toolsModern = await client.request("tools/list", {
      _meta: { "io.modelcontextprotocol/protocolVersion": MODERN },
    });
    timings.toolsListModernMs = toolsModern.elapsedMs;

    discover = await client.request("server/discover", {
      _meta: { "io.modelcontextprotocol/protocolVersion": MODERN },
    });
    timings.discoverMs = discover.elapsedMs;
  } finally {
    exit = await client.close();
  }

  const toolsList = tools.result?.tools;
  if (!Array.isArray(toolsList)) throw new Error("tools/list returned no tools[] array");
  const { duplicates } = indexTools(toolsList);

  if (write) mkdirSync(dirname(outPrefix), { recursive: true });
  const files = {};
  const emitJson = (suffix, value) => {
    const path = `${outPrefix}.${suffix}`;
    const text = prettyCanonical(value);
    if (write) writeFileSync(path, text);
    files[suffix] = { path, bytes: Buffer.byteLength(text), sha256: `sha256:${sha256(text)}`, canonical: canonicalHash(value) };
  };
  emitJson("initialize.json", init.result);
  emitJson("discover.json", discover.result);
  emitJson("ping.json", ping.result);
  emitJson("tools.json", tools.result);
  emitJson("tools.modern.json", toolsModern.result);

  const transcriptText = client.transcript.map((frame) => `${JSON.stringify(frame)}\n`).join("");
  if (write) writeFileSync(`${outPrefix}.transcript.jsonl`, transcriptText);
  files["transcript.jsonl"] = {
    path: `${outPrefix}.transcript.jsonl`,
    bytes: Buffer.byteLength(transcriptText),
    sha256: `sha256:${sha256(transcriptText)}`,
  };

  const meta = {    label: label ?? executable,
    capturedAtUtc: started.toISOString(),
    capturedAtLocal: started.toString(),
    platform: `${process.platform} ${process.arch}`,
    node: process.version,
    command: executable,
    args,
    cwd: cwd ?? process.cwd(),
    executable: {
      path: executable,
      bytes: statSync(executable).size,
      sha256: `sha256:${sha256File(executable)}`,
    },
    version,
    server: {
      stderr: client.stderr,
      exit,
      initialize: {
        protocolVersion: init.result?.protocolVersion,
        serverInfo: init.result?.serverInfo,
        capabilities: init.result?.capabilities,
      },
    },
    catalog: {
      toolCount: toolsList.length,
      toolNames: toolsList.map((tool) => tool.name),
      duplicateNames: duplicates,
      canonicalHash: canonicalHash(tools.result),
      schemaHashes: Object.fromEntries(toolsList.map((tool) => [tool.name, schemaHash(tool)])),
      legacyVsModernResultDiff: diffKeys(tools.result, toolsModern.result),
    },
    requests: { protocol: PROTOCOL, modernProtocol: MODERN, timingsMs: timings },
    unsolicitedFrames: client.unsolicited,
    files,
  };
  if (write) writeFileSync(`${outPrefix}.meta.json`, prettyCanonical(meta));
  return { meta, tools: tools.result, outPrefix, files };
}

function runOnce(command, args, cwd, timeout) {
  const out = spawnSync(command, args, { cwd, timeout, encoding: "utf8", windowsHide: true, env: cleanEnv() });
  return {
    command: [command, ...args].join(" "),
    status: out.status,
    stdout: (out.stdout ?? "").trim(),
    stderr: (out.stderr ?? "").trim(),
    error: out.error ? String(out.error) : undefined,
  };
}

function cleanEnv() {
  const env = { ...process.env };
  delete env.GLADIUS_EXPECTED_VERSION;
  return env;
}

function diffKeys(left, right) {
  const keys = new Set([...Object.keys(left ?? {}), ...Object.keys(right ?? {})]);
  return [...keys].filter((key) => canonicalHash(left?.[key] ?? null) !== canonicalHash(right?.[key] ?? null)).sort();
}

function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function parseArgs(argv) {
  const options = { args: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (token === "--cmd") options.command = argv[++i];
    else if (token === "--arg") options.args.push(argv[++i]);
    else if (token === "--out") options.outPrefix = argv[++i];
    else if (token === "--label") options.label = argv[++i];
    else if (token === "--cwd") options.cwd = argv[++i];
    else if (token === "--timeout") options.timeoutMs = Number(argv[++i]);
    else if (token === "--no-version") options.versionArgs = [];
    else if (token === "--help" || token === "-h") {
      console.log("usage: capture-mcp-catalog.mjs --cmd <exe> [--arg <a>] --out <prefix> [--label <text>] [--cwd <dir>] [--timeout <ms>] [--no-version]");
      process.exit(0);
    } else throw new Error(`unknown argument: ${token}`);
  }
  if (!options.command) throw new Error("missing --cmd");
  if (!options.outPrefix) throw new Error("missing --out");
  return options;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const options = parseArgs(process.argv.slice(2));
  captureCatalog(options)
    .then(({ meta }) => {
      const catalog = meta.catalog;
      console.log(meta.label);
      console.log(`  executable ${meta.executable.path}`);
      console.log(`  sha256     ${meta.executable.sha256}`);
      console.log(`  version    ${meta.version.stdout || meta.version.stderr || "<none>"}`);
      console.log(`  server     ${meta.server.initialize.serverInfo?.name} ${meta.server.initialize.serverInfo?.version} proto ${meta.server.initialize.protocolVersion}`);
      console.log(`  tools      ${catalog.toolCount}${catalog.duplicateNames.length ? ` (duplicates: ${catalog.duplicateNames.join(", ")})` : ""}`);
      console.log(`  hash       ${catalog.canonicalHash}`);
      console.log(`  exit       ${JSON.stringify(meta.server.exit)}`);
      if (catalog.legacyVsModernResultDiff.length) {
        console.log(`  modern diff keys: ${catalog.legacyVsModernResultDiff.join(", ")}`);
      }
    })
    .catch((error) => {
      console.error(`capture failed: ${error.message}`);
      process.exit(2);
    });
}
