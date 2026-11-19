#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { prettyCanonical } from "./lib/canonical.mjs";
import { SPEC_REQUIRED, buildHandCatalog, indexTools, validateHandCatalog } from "./lib/catalog.mjs";
import { StdioClient } from "./lib/mcp-stdio.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const PROTOCOL = "2025-11-25";

const LIVE_CAPTURE_KEYS = ["executable", "executableSha256", "reportedVersion"];

export function buildFromDisk({ root = ROOT } = {}) {
  const pins = JSON.parse(readFileSync(resolve(root, "pins.json"), "utf8"));
  const blockers = [];
  const notes = [];
  const sources = {};

  for (const [key, pin] of Object.entries(pins.sources)) {
    const catalogPath = resolve(root, pin.capture.tools);
    const metaPath = resolve(root, pin.capture.meta);
    let tools;
    let meta;
    try {
      tools = JSON.parse(readFileSync(catalogPath, "utf8")).tools;
      meta = JSON.parse(readFileSync(metaPath, "utf8"));
    } catch (error) {
      throw new Error(`cannot read capture for ${key}: ${error.message}`);
    }
    const { index, duplicates } = indexTools(tools);
    if (duplicates.length) blockers.push(`${key}: duplicate tool names ${duplicates.join(", ")}`);

    if (tools.length !== pin.expectedToolCount) {
      blockers.push(`${key}: ${tools.length} tools captured, pins.json expects ${pin.expectedToolCount}`);
    }
    if (!meta.version.stdout.includes(pin.version)) {
      blockers.push(`${key}: binary reports "${meta.version.stdout}", pins.json expects version ${pin.version}`);
    }
    const required = SPEC_REQUIRED[pin.domain];
    const captured = [...index.keys()];
    const missing = required.filter((name) => !index.has(name));
    const unexpected = captured.filter((name) => !required.includes(name));
    if (missing.length) blockers.push(`${key}: required tools not in the captured catalogue: ${missing.join(", ")}`);
    if (unexpected.length) {
      notes.push(`${key}: captured tools beyond the required floor: ${unexpected.join(", ")}`);
    }

    sources[key] = {
      pin: pin.pin,
      pinSubject: pin.pinSubject,
      repo: pin.repo,
      version: pin.version,
      domain: pin.domain,
      specPrefix: pin.specPrefix,
      capturedFrom: meta.command,
      executableSha256: meta.executable.sha256,
      reportedVersion: meta.version.stdout,
      server: meta.server.initialize.serverInfo,
      protocolVersion: meta.server.initialize.protocolVersion,
      toolCount: tools.length,
      canonicalHash: meta.catalog.canonicalHash,
      capture: pin.capture,
      names: captured,
      duplicates,
    };

    if (meta.catalog.canonicalHash !== pin.capture.expectCatalogCanonicalHash) {
      blockers.push(
        `${key}: capture hash ${meta.catalog.canonicalHash} does not match the pinned expectation ${pin.capture.expectCatalogCanonicalHash}`,
      );
    }
  }

  const hand = buildHandCatalog({
    gladius: { tools: JSON.parse(readFileSync(resolve(root, pins.sources.gladius.capture.tools), "utf8")).tools },
    puppet: { tools: JSON.parse(readFileSync(resolve(root, pins.sources.puppet.capture.tools), "utf8")).tools },
    provenance: {
      generator: "baseline/scripts/build-hand-catalog.mjs",
      migration: "one-to-one name mapping only: browser_<legacy> / desktop_<legacy>; inputSchema, outputSchema, annotations, title and description are carried over unchanged",
      sources,
    },
  });
  hand.blockers = blockers;
  hand.notes = notes;
  return { hand, blockers, notes, pins, sources };
}

export async function captureLiveTools({ executable, timeoutMs = 30_000 }) {
  const path = resolve(executable);
  if (!existsSync(path)) throw new Error(`no executable at ${path}`);
  const version = spawnSync(path, ["--version"], { timeout: 15_000, encoding: "utf8", windowsHide: true, env: cleanEnv() });
  const client = new StdioClient({ command: path, timeoutMs });
  let init;
  let tools;
  try {
    init = await client.request("initialize", {
      protocolVersion: PROTOCOL,
      capabilities: {},
      clientInfo: { name: "hand-baseline-live", version: "1.0.0" },
    });
    client.notify("notifications/initialized", {});
    tools = await client.request("tools/list", {});
  } finally {
    await client.close();
  }

  const list = tools.result?.tools;
  if (!Array.isArray(list) || list.length === 0) throw new Error(`live tools/list from ${path} returned no tools[] array`);
  const serverInfo = init.result?.serverInfo;
  return {
    capture: {
      executable: path,
      executableSha256: `sha256:${sha256File(path)}`,
      reportedVersion:
        (version.stdout ?? "").trim() || [serverInfo?.name, serverInfo?.version].filter(Boolean).join(" ") || null,
    },
    protocolVersion: init.result?.protocolVersion ?? PROTOCOL,
    toolCount: list.length,
    names: list.map((tool) => tool.name),
  };
}

export function liveBlock({ capture, protocolVersion, toolCount, names, floor }) {
  const floorSet = new Set(floor);
  return {
    capture,
    protocolVersion,
    toolCount,
    names: [...names].sort(),
    extra: [...new Set(names)].filter((name) => !floorSet.has(name)).sort(),
  };
}

export function withLive(catalog, captured) {
  const live = liveBlock({ ...captured, floor: catalog.tools.map((tool) => tool.name) });
  return { ...catalog, counts: { ...catalog.counts, live: live.toolCount }, live };
}

export function validateLive(catalog) {
  const issues = [];
  const live = catalog?.live;
  if (!live) return issues;

  if (live.toolCount !== live.names.length) {
    issues.push({
      level: "error",
      code: "live-count-mismatch",
      detail: `toolCount ${live.toolCount} but ${live.names.length} names`,
    });
  }
  if (catalog.counts?.live !== live.toolCount) {
    issues.push({
      level: "error",
      code: "live-count-not-in-counts",
      detail: `counts.live ${catalog.counts?.live} but live.toolCount ${live.toolCount}`,
    });
  }
  const sorted = [...live.names].sort();
  if (JSON.stringify(sorted) !== JSON.stringify(live.names)) {
    issues.push({ level: "error", code: "live-names-unsorted", detail: "names must be sorted" });
  }
  const repeated = [...new Set(sorted.filter((name, i) => sorted.indexOf(name) !== i))];
  if (repeated.length) issues.push({ level: "error", code: "live-duplicate-name", detail: repeated.join(", ") });

  const floor = catalog.tools.map((tool) => tool.name);
  const floorSet = new Set(floor);
  const omitted = floor.filter((name) => !live.names.includes(name));
  if (omitted.length) issues.push({ level: "error", code: "live-omits-floor-tool", detail: omitted.join(", ") });

  const expectedExtra = [...new Set(live.names)].filter((name) => !floorSet.has(name)).sort();
  if (JSON.stringify(expectedExtra) !== JSON.stringify(live.extra)) {
    issues.push({ level: "error", code: "live-extra-mismatch", detail: `expected ${expectedExtra.join(", ") || "<none>"}` });
  }

  const capture = live.capture ?? {};
  const unexpected = Object.keys(capture).filter((key) => !LIVE_CAPTURE_KEYS.includes(key));
  if (unexpected.length) {
    issues.push({ level: "error", code: "live-capture-unexpected-field", detail: unexpected.join(", ") });
  }
  if (!/^sha256:[0-9a-f]{64}$/.test(capture.executableSha256 ?? "")) {
    issues.push({ level: "error", code: "live-executable-sha256-invalid", detail: String(capture.executableSha256) });
  }
  if (!capture.executable) issues.push({ level: "error", code: "live-executable-missing" });
  return issues;
}

function cleanEnv() {
  const env = { ...process.env };
  delete env.GLADIUS_EXPECTED_VERSION;
  return env;
}

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (token === "--serve") {
      options.serve = argv[++i];
      if (!options.serve) throw new Error("--serve needs a path to a hand executable");
    } else if (token === "--help" || token === "-h") {
      console.log("usage: build-hand-catalog.mjs [--serve <path-to-hand.exe>]");
      process.exit(0);
    } else throw new Error(`unknown argument: ${token}`);
  }
  return options;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const options = parseArgs(process.argv.slice(2));
    const derived = buildFromDisk();
    const { blockers } = derived;
    let catalog = derived.hand;
    let liveCount = null;

    if (options.serve) {
      const captured = await captureLiveTools({ executable: options.serve });
      catalog = withLive(derived.hand, captured);
      liveCount = captured.toolCount;
      const notes = [
        ...derived.notes,
        `live: ${captured.toolCount} tools published by ${captured.capture.executable}; the floor is ${derived.hand.counts.total}`,
      ];
      if (catalog.live.extra.length) {
        notes.push(`live: beyond the ${derived.hand.counts.total}-tool floor: ${catalog.live.extra.join(", ")}`);
      }
      catalog = { ...catalog, notes };
    }

    const issues = [...validateHandCatalog(catalog), ...validateLive(catalog)];
    const out = resolve(ROOT, "hand-catalog.json");
    writeFileSync(out, prettyCanonical(catalog));

    console.log(`wrote ${out}`);
    console.log(`  floor      ${catalog.counts.total} (browser ${catalog.counts.browser}, desktop ${catalog.counts.desktop})`);
    if (liveCount !== null) {
      const extra = catalog.live.extra;
      console.log(`  live       ${liveCount}${extra.length ? ` (${extra.length} beyond the floor: ${extra.join(", ")})` : ""}`);
    }
    for (const [key, source] of Object.entries(catalog.provenance.sources)) {
      console.log(`  ${key.padEnd(10)} ${source.toolCount} tools from ${source.version} @ ${source.pin} (${source.capturedFrom})`);
    }
    for (const issue of issues) console.log(`  ISSUE ${issue.code} ${issue.tool ?? ""} ${issue.detail ?? ""}`.trimEnd());
    for (const note of catalog.notes) console.log(`  NOTE ${note}`);
    for (const blocker of blockers) console.log(`  BLOCKER ${blocker}`);
    process.exit(issues.length || blockers.length ? 1 : 0);
  } catch (error) {
    console.error(`build-hand-catalog failed: ${error.message}`);
    process.exit(2);
  }
}
