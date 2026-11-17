#!/usr/bin/env node
// Relocate an already-recorded build artifact to durable storage without
// pretending a new build happened. The copied bytes must hash to the build
// receipt's output hash; the new receipt records the source receipt and the
// original artifact path under `relocation`.
//
//   node benchmarks/scripts/relocate-build-artifact.mjs \
//     --receipt benchmarks/receipts/builds/hand-....json

import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { basename, dirname, join, resolve } from "node:path";

import { hashFile } from "./lib/fingerprint.mjs";
import { loadBuildReceipt } from "./lib/release-evidence.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");

async function main() {
  const args = process.argv.slice(2);
  const options = {};
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--receipt") options.receipt = args[++index];
    else if (args[index] === "--artifact-dir") options.artifactDir = args[++index];
    else if (args[index] === "--out") options.out = args[++index];
    else if (args[index] === "--help") {
      process.stdout.write("usage: node benchmarks/scripts/relocate-build-artifact.mjs --receipt <build.json> [--artifact-dir <dir>] [--out <dir>]\n");
      return 0;
    } else {
      process.stderr.write(`unknown argument: ${args[index]}\n`);
      return 3;
    }
  }
  if (!options.receipt) {
    process.stderr.write("--receipt is required\n");
    return 3;
  }
  const receiptPath = resolve(options.receipt);
  const loaded = await loadBuildReceipt(receiptPath);
  if (!loaded.ok) {
    process.stderr.write(`receipt is not usable: ${loaded.errors.join("; ")}\n`);
    return 1;
  }
  const build = loaded.build;
  const original = await hashFile(build.output.path);
  if (original.sha256 !== build.output.sha256) {
    process.stderr.write(`original artifact hash ${original.sha256} does not match receipt ${build.output.sha256}\n`);
    return 1;
  }
  const artifactDir = resolve(options.artifactDir ?? join(REPO_ROOT, "benchmarks", "receipts", "builds", "artifacts"));
  const outDir = resolve(options.out ?? dirname(receiptPath));
  await mkdir(artifactDir, { recursive: true });
  const stamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\.(\d{3})Z$/, "$1Z");
  const ext = process.platform === "win32" ? ".exe" : "";
  const filename = `${build.product}-${build.source.gitHead.slice(0, 8)}-${stamp}-relocated-release${ext}`;
  const durablePath = join(artifactDir, filename);
  await copyFile(build.output.path, durablePath);
  const copied = await hashFile(durablePath);
  if (copied.sha256 !== build.output.sha256) {
    process.stderr.write(`copied artifact ${copied.sha256} does not match receipt ${build.output.sha256}\n`);
    return 1;
  }

  const relocated = {
    ...build,
    output: {
      ...build.output,
      path: durablePath,
      buildPath: build.output.buildPath ?? build.output.path,
      bytes: copied.bytes,
      sha256: copied.sha256,
      durable: true,
    },
    relocation: {
      fromReceipt: basename(receiptPath),
      fromReceiptSha256: loaded.sha256,
      fromArtifact: build.output.path,
      fromArtifactSha256: build.output.sha256,
      atUtc: new Date().toISOString(),
    },
  };
  const outName = `${build.product}-${build.source.gitHead.slice(0, 8)}-${stamp}-relocated-release.json`;
  const outPath = join(outDir, outName);
  const text = `${JSON.stringify(relocated, null, 2)}\n`;
  await writeFile(outPath, text, "utf8");
  const sha256 = `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
  await writeFile(`${outPath}.sha256`, `${sha256}  ${outName}\n`, "utf8");
  const verify = await loadBuildReceipt(outPath);
  if (!verify.ok) {
    process.stderr.write(`relocated receipt failed verification: ${verify.errors.join("; ")}\n`);
    return 1;
  }
  process.stdout.write(`relocated artifact: ${durablePath}\nreceipt: ${outPath}\nsha256: ${copied.sha256}\n`);
  return 0;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((error) => {
    process.stderr.write(`relocate error: ${error?.stack ?? error}\n`);
    process.exitCode = 4;
  });
