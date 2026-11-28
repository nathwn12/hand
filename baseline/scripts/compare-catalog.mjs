#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { captureCatalog } from "./capture-mcp-catalog.mjs";
import { prettyCanonical } from "./lib/canonical.mjs";
import { compareCatalogs, toolsFromPayload } from "./lib/catalog.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");

export async function runComparison(options) {
  const baselineName = options.baseline ?? "hand";
  const pins = JSON.parse(readFileSync(resolve(ROOT, "pins.json"), "utf8"));
  const pin = options.pin ?? pins.sources[baselineName]?.pin;
  if (baselineName !== "hand" && !pin) throw new Error(`unknown legacy baseline: ${baselineName} (expected gladius or puppet)`);
  const baselinePath = baselineName === "hand"
    ? resolve(ROOT, "hand-catalog.json")
    : resolve(ROOT, "catalog", `${baselineName}-${pin}.tools.json`);
  const baseline = JSON.parse(readFileSync(baselinePath, "utf8"));
  const profile = baselineName === "hand" ? "hand" : "legacy";

  let candidate;
  let captureMeta;
  if (options.serve) {
    const captured = await captureCatalog({
      command: options.serve,
      args: options.serveArgs ?? [],
      label: `candidate: ${options.serve}`,
      outPrefix: "<memory>",
      write: false,
    });
    candidate = toolsFromPayload(captured.tools);
    captureMeta = captured.meta;
  } else if (options.candidate) {
    candidate = toolsFromPayload(JSON.parse(readFileSync(resolve(options.candidate), "utf8")));
  } else {
    throw new Error("one of --serve or --candidate is required");
  }

  const report = compareCatalogs({
    baseline,
    candidate: { tools: candidate },
    profile,
  });
  report.baseline = { name: baselineName, path: baselinePath, tools: baseline.tools.length };
  report.candidate = {
    source: options.serve ? options.serve : options.candidate,
    tools: candidate.length,
    server: captureMeta?.server.initialize ?? null,
    executableSha256: captureMeta?.executable.sha256 ?? null,
  };
  report.comparedAtUtc = new Date().toISOString();
  return report;
}

function printReport(report) {
  const status = report.ok ? "PASS" : "FAIL";
  console.log(`${status} ${report.profile} baseline: ${report.baseline.tools} baseline tools vs ${report.candidate.tools} candidate tools`);
  console.log(`  compared   ${report.counts.compared} tools`);
  if (report.missing.length) console.log(`  missing    ${report.missing.length}: ${report.missing.join(", ")}`);
  if (report.extra.length) console.log(`  extra      ${report.extra.length}: ${report.extra.join(", ")}`);
  for (const mismatch of report.schemaMismatches) {
    console.log(`  MISMATCH   ${mismatch.tool} ${mismatch.kind}`);
    for (const diff of mismatch.diffs.slice(0, 8)) {
      console.log(`             ${diff.path}: ${diff.kind} left=${diff.left} right=${diff.right}`);
    }
  }
  console.log(`  metadata   ${report.metadataDrift.length} title/description difference(s) (strict: on)`);
  for (const entry of report.metadataDrift.slice(0, 10)) {
    console.log(`             ${entry.tool} ${entry.kind}`);
  }
  console.log(`  rewrites   ${report.rewrites?.length ?? 0} property description rewrite(s) applied`);
  for (const entry of report.rewrites ?? []) {
    console.log(`             ${entry.tool} ${entry.schema}.${entry.path}`);
  }
  for (const issue of report.rewriteIssues ?? []) console.log(`  REWRITE-ISSUE ${issue}`);
  if (report.declaredMissing?.length) {
    for (const entry of report.declaredMissing) {
      console.log(`  MISSING-ADDITION ${entry.tool} ${entry.kind}: ${entry.paths.join(", ")} declared in baseline/declared-additions.json but absent from the candidate`);
    }
  }
  if (report.declaredAdditions?.length) {
    console.log(`  declared   ${report.declaredAdditions.length} addition group(s) allowed by baseline/declared-additions.json:`);
    for (const entry of report.declaredAdditions) {
      console.log(`             ${entry.tool} ${entry.kind} += ${entry.paths.join(", ")}`);
    }
  }
  for (const warning of report.warnings) console.log(`  WARN       ${warning}`);
}

function parseArgs(argv) {
  const options = { serveArgs: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const token = argv[i];
    if (token === "--serve") options.serve = argv[++i];
    else if (token === "--serve-arg") options.serveArgs.push(argv[++i]);
    else if (token === "--candidate") options.candidate = argv[++i];
    else if (token === "--baseline") options.baseline = argv[++i];
    else if (token === "--pin") options.pin = argv[++i];
    else if (token === "--out") options.out = argv[++i];
    else if (token === "--json") options.json = true;
    else if (token === "--strict-metadata") options.strictMetadata = true;
    else if (token === "--help" || token === "-h") {
      console.log("usage: compare-catalog.mjs (--serve <exe> [--serve-arg <a>] | --candidate <json>) [--baseline hand|gladius|puppet] [--pin <commit>] [--json] [--out <file>]");
      console.log("       metadata is strict by default in both profiles: any title difference fails, and a");
      console.log("       description difference fails unless the tool is declared as rewritten under");
      console.log("       metadata.description in baseline/declared-additions.json. A property-level");
      console.log("       description change must be declared under metadata.rewrites as a");
      console.log("       {tool, schema, path} descriptor; its published value must differ from the");
      console.log("       pin, and the pin value is substituted before the equality check.");
      console.log("       --strict-metadata is still accepted and changes nothing: it is a no-op kept for");
      console.log("       CLI compatibility, not an extra failure mode.");
      process.exit(0);
    } else throw new Error(`unknown argument: ${token}`);
  }
  return options;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let report;
  try {
    report = await runComparison(parseArgs(process.argv.slice(2)));
  } catch (error) {
    console.error(`compare-catalog failed: ${error.message}`);
    process.exit(2);
  }
  if (process.argv.includes("--json")) console.log(JSON.stringify(report, null, 2));
  else printReport(report);
  const outIndex = process.argv.indexOf("--out");
  if (outIndex > -1) writeFileSync(process.argv[outIndex + 1], prettyCanonical(report));
  process.exit(report.ok ? 0 : 1);
}
