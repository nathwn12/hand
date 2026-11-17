#!/usr/bin/env node
// Derive the hand catalogue from the two pinned legacy catalogues.
//
//   node baseline/scripts/build-hand-catalog.mjs
//
// Reads the captured `tools/list` results and their receipts, checks them
// against `pins.json` and the spec's required surface, applies only the
// documented `browser_` / `desktop_` prefix, and writes `hand-catalog.json`.
//
// Exit codes: 0 = catalogue written with no blocker; 1 = catalogue written but
// a blocker was recorded; 2 = capture missing or unreadable.
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
// the exit code.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { prettyCanonical } from "./lib/canonical.mjs";
import { SPEC_REQUIRED, buildHandCatalog, indexTools, validateHandCatalog } from "./lib/catalog.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");

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

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { hand, blockers, notes } = buildFromDisk();
    const issues = validateHandCatalog(hand);
    const out = resolve(ROOT, "hand-catalog.json");
    writeFileSync(out, prettyCanonical(hand));

    console.log(`wrote ${out}`);
    console.log(`  tools      ${hand.counts.total} (browser ${hand.counts.browser}, desktop ${hand.counts.desktop})`);
    for (const [key, source] of Object.entries(hand.provenance.sources)) {
      console.log(`  ${key.padEnd(10)} ${source.toolCount} tools from ${source.version} @ ${source.pin} (${source.capturedFrom})`);
    }
    for (const issue of issues) console.log(`  ISSUE ${issue.code} ${issue.tool ?? ""} ${issue.detail ?? ""}`.trimEnd());
    for (const note of notes) console.log(`  NOTE ${note}`);
    for (const blocker of blockers) console.log(`  BLOCKER ${blocker}`);
    process.exit(issues.length || blockers.length ? 1 : 0);
  } catch (error) {
    console.error(`build-hand-catalog failed: ${error.message}`);
    process.exit(2);
  }
}
