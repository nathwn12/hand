#!/usr/bin/env node
// Compare benchmark receipts without hiding anything.
//
// Reads every *.json receipt under the given directories (or explicit files),
// re-hashes the binary at each recorded path, then applies the release-gate
// rules in lib/compare-core.mjs: expected products present, machine/fixture/
// browser/target/binary hashes present and re-verifiable, release builds only,
// identical protocol and params, no test-only receipts.
//
//   node benchmarks/scripts/compare.mjs --receipts benchmarks/receipts
//   node benchmarks/scripts/compare.mjs --receipts a.json,b.json --require-pass
//
// Exit: 0 report written; 1 --require-pass not satisfied; 3 usage; 4 unreadable input.

import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

import { buildComparison } from "./lib/compare-core.mjs";
import { hashFile } from "./lib/fingerprint.mjs";
import { loadBuildReceipts, verifyReleaseEvidence } from "./lib/release-evidence.mjs";
import { RECEIPT_SCHEMA } from "./lib/receipt.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const DEFAULT_OUT_DIR = join(REPO_ROOT, "benchmarks", "receipts", "comparisons");
const binaryHashes = new Map();

async function main() {
  const options = parse(process.argv.slice(2));
  if (options.help) {
    process.stdout.write(
      "usage: node benchmarks/scripts/compare.mjs --receipts <dir-or-json-list> [--build-receipts <dir>] [--out <file>] [--require-pass]\n",
    );
    return 0;
  }
  if (options.errors.length) {
    process.stderr.write(`${options.errors.join("\n")}\n`);
    return 3;
  }
  if (!options.inputs.length) {
    process.stderr.write("--receipts is required\n");
    return 3;
  }

  const files = [];
  for (const input of options.inputs) {
    const absolute = resolve(input);
    let entries;
    try {
      entries = await readdir(absolute, { withFileTypes: true });
    } catch {
      files.push(absolute);
      continue;
    }
    for (const entry of entries) {
      if (entry.isFile() && entry.name.endsWith(".json")) files.push(join(absolute, entry.name));
    }
  }
  files.sort();

  const entries = [];
  for (const file of files) {
    let text;
    try {
      text = await readFile(file, "utf8");
    } catch (error) {
      process.stderr.write(`cannot read ${file}: ${error.message}\n`);
      return 4;
    }
    let receipt;
    try {
      receipt = JSON.parse(text);
    } catch (error) {
      process.stderr.write(`cannot parse ${file}: ${error.message}\n`);
      return 4;
    }
    if (receipt.schema !== RECEIPT_SCHEMA) {
      process.stderr.write(`not a benchmark receipt: ${file} (schema=${receipt.schema})\n`);
      return 4;
    }
    entries.push({
      file,
      sha256: `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`,
      receipt,
      currentBinarySha: await currentBinaryHash(receipt.product?.executable?.path),
    });
  }

  if (entries.length === 0) {
    process.stderr.write("no benchmark receipts found\n");
    return 4;
  }

  let pins = null;
  try {
    pins = JSON.parse(await readFile(join(REPO_ROOT, "baseline", "pins.json"), "utf8"));
  } catch {
    pins = null;
  }
  const builds = await loadBuildReceipts(options.buildReceipts ?? join(REPO_ROOT, "benchmarks", "receipts", "builds"));
  for (const entry of entries) {
    entry.releaseEvidence = await verifyReleaseEvidence({
      receipt: entry.receipt,
      currentBinarySha: entry.currentBinarySha,
      pins,
      builds,
    });
  }

  const comparison = buildComparison(entries);
  const outPath = options.out
    ? resolve(options.out)
    : join(DEFAULT_OUT_DIR, `comparison-${new Date().toISOString().replace(/[-:]/g, "").replace(/\.(\d{3})Z$/, "$1Z")}.json`);
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, `${JSON.stringify(comparison, null, 2)}\n`, "utf8");

  const header = ["scenario", "product", "status", "p50 ms", "p95 ms", "max ms", "failed", "warm-fail", "build", "release evidence", "test", "receipt"];
  const table = comparison.receipts.map((row) => [
    row.scenario,
    row.product,
    row.status,
    row.p50 ?? "n/a",
    row.p95 ?? "n/a",
    row.max ?? "n/a",
    row.failed,
    row.warmupFailures ?? "n/a",
    row.buildConfiguration ?? "n/a",
    row.releaseEvidenceVerified ? row.releaseEvidenceKind : "unverified",
    row.testOnly ? "YES" : "no",
    row.file,
  ]);
  process.stdout.write(`${formatTable([header, ...table])}\n`);
  for (const group of comparison.groups) {
    process.stdout.write(
      `${group.scenario}: ${group.members.length} receipt(s), comparable=${group.comparable}${group.differences.length ? ` (${group.differences.join("; ")})` : ""}\n`,
    );
  }
  process.stdout.write(`gate: ${comparison.gate.pass ? "pass" : "fail"}\ncomparison: ${outPath}\n`);

  if (options.requirePass && !comparison.gate.pass) {
    process.stderr.write(
      `release gate not satisfied for: ${comparison.gate.failures
        .map((failure) => `${failure.scenario} [${failure.differences.join("; ")}]`)
        .join(" | ")}\n`,
    );
    return 1;
  }
  return 0;
}

async function currentBinaryHash(path) {
  if (!path) return null;
  if (!binaryHashes.has(path)) {
    binaryHashes.set(
      path,
      hashFile(path)
        .then((file) => file.sha256)
        .catch(() => null),
    );
  }
  return binaryHashes.get(path);
}

function parse(argv) {
  const options = { inputs: [], out: null, requirePass: false, help: false, errors: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--help") options.help = true;
    else if (arg === "--require-pass") options.requirePass = true;
    else if (arg === "--out") {
      options.out = argv[++index];
      if (!options.out) options.errors.push("--out requires a path");
    } else if (arg === "--receipts") {
      const value = argv[++index];
      if (!value) options.errors.push("--receipts requires a path");
      else options.inputs.push(...value.split(",").filter(Boolean));
    } else if (arg === "--build-receipts") {
      options.buildReceipts = argv[++index];
      if (!options.buildReceipts) options.errors.push("--build-receipts requires a path");
    } else {
      options.errors.push(`unknown argument: ${arg}`);
    }
  }
  return options;
}

function formatTable(rows) {
  const widths = rows[0].map((_, column) => Math.max(...rows.map((row) => String(row[column]).length)));
  return rows
    .map((row) => row.map((cell, column) => String(cell).padEnd(widths[column])).join("  ").trimEnd())
    .join("\n");
}

export { main };

if (process.argv[1] && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  main()
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      process.stderr.write(`compare error: ${error?.stack ?? error}\n`);
      process.exitCode = 4;
    });
}
