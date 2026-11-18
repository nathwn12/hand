#!/usr/bin/env node
// Derive the hand catalogue from the two pinned legacy catalogues.
//
//   node baseline/scripts/build-hand-catalog.mjs
//   node baseline/scripts/build-hand-catalog.mjs --serve Q:\path\to\hand.exe
//
// Reads the captured `tools/list` results and their receipts, checks them
// against `pins.json` and the spec's required surface, applies only the
// documented `browser_` / `desktop_` prefix, and writes `hand-catalog.json`.
//
// `--serve <exe>` additionally asks that binary what it actually publishes:
// one stdio MCP session (initialize, notifications/initialized, tools/list, then
// stdin close) and nothing else, so no browser launches and no window is
// touched. The answer is recorded in a `live` block beside the derived floor,
// with `counts.live` beside the floor counts. Without `--serve` no `live` block
// is written, because the derived file must stay reproducible from the pinned
// captures alone; that run does not claim anything about a running server.
//
// Exit codes: 0 = catalogue written with no blocker; 1 = catalogue written but
// a blocker or a structural issue was recorded; 2 = capture missing or
// unreadable, which includes a live server that never answered `tools/list`.
//
// A blocker is a discrepancy with the pinned capture: a count, version, hash or
// required-tool mismatch, or a duplicate name. It is never silently dropped:
// the file records what was really captured, and the exit code says the capture
// is not the pinned surface.
//
// A tool captured beyond the required floor is a *note*, not a blocker. The
// floor says what must be present; extra upstream capability does not make the
// capture wrong, and the hand comparison reports such tools as `extra` without
// failing. Notes are recorded in the artefact and printed, and do not affect
// the exit code. The same rule governs the live block: a live tool beyond the
// floor is a note, while a live block that contradicts its own count, its
// ordering or the floor it is supposed to serve is a structural issue (exit 1).

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

/** Capture keys the `live` block may carry. No timestamp: none is allowed. */
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
      // Reported, not failed: the spec lists a floor, so a pinned suite that
      // carries more is extra capability, not a broken capture.
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

/**
 * Ask a running hand server what it publishes. Read-only by construction: one
 * `initialize`, one `notifications/initialized`, one `tools/list`, then stdin
 * close. No tool is called and no further request is sent.
 */
export async function captureLiveTools({ executable, timeoutMs = 30_000 }) {
  const path = resolve(executable);
  // Fail immediately and precisely: a spawn error would otherwise sit in the
  // request queue until the initialize timeout, reporting a missing path as a
  // slow server.
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
      // Same meaning as a pinned source's `reportedVersion`: the binary's own
      // `--version` line, falling back to what it announced at initialize.
      reportedVersion:
        (version.stdout ?? "").trim() || [serverInfo?.name, serverInfo?.version].filter(Boolean).join(" ") || null,
    },
    protocolVersion: init.result?.protocolVersion ?? PROTOCOL,
    toolCount: list.length,
    names: list.map((tool) => tool.name),
  };
}

/**
 * The `live` block: what the server published, and how much of it sits above
 * the derived floor. `extra` is the only derived part; everything else is the
 * server's own answer.
 */
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

/** Attach the live block and `counts.live` to a derived catalogue. */
export function withLive(catalog, captured) {
  const live = liveBlock({ ...captured, floor: catalog.tools.map((tool) => tool.name) });
  return { ...catalog, counts: { ...catalog.counts, live: live.toolCount }, live };
}

/**
 * Structural checks on the live block, independent of any server.
 *
 * A live tool beyond the floor is expected here and is never reported: that is
 * a note. What is reported is a block that contradicts itself or the floor —
 * a count that does not match its names, names the server did not send in
 * sorted order, a repeat, a floor tool the live server does not serve, an
 * `extra` list that is not exactly the difference, or a `capture` carrying a
 * field nobody asked for (a timestamp, for instance).
 */
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
