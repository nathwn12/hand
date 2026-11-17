#!/usr/bin/env node
// Record a genuine local release build as evidence.
//
// This tool runs cargo itself. It never accepts a caller-supplied binary hash,
// build profile or "verified" flag. The receipt binds a successful
// `cargo build --release` command to its output hash, source HEAD, tracked-diff
// state and toolchain. compare.mjs re-checks the artifact and source state.
//
//   node benchmarks/scripts/record-build.mjs --product hand \
//     --source-root . --manifest-path Cargo.toml --bin hand-mcp \
//     --target-dir C:\scratch\hand-release
//
// Build output stays wherever --target-dir points. The receipt is written under
// benchmarks/receipts/builds unless --out says otherwise.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFile, mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

import { hashFile } from "./lib/fingerprint.mjs";import {
  BUILD_RECEIPT_SCHEMA,
  currentSourceState,
  loadBuildReceipt,
  sameSourceState,
} from "./lib/release-evidence.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const DEFAULT_OUT = join(REPO_ROOT, "benchmarks", "receipts", "builds");
const MAX_TAIL = 8192;

async function main() {
  const parsed = parseBuildArgs(process.argv.slice(2));
  const options = parsed.options;
  if (parsed.errors.length) return usage(parsed.errors.join("\n"));
  if (options.help) {
    process.stdout.write(
      "usage: node benchmarks/scripts/record-build.mjs --product <id> --bin <name> [--source-root <dir>] [--manifest-path Cargo.toml] [--target-dir <dir>] [--target <triple>] [--out <dir>] [--artifact-dir <dir>] [--no-locked]\n",
    );
    return 0;
  }
  if (!options.product) return usage("--product is required");
  if (!options.bin) return usage("--bin is required");
  const sourceRoot = resolve(options.sourceRoot ?? REPO_ROOT);
  const manifestPath = resolve(sourceRoot, options.manifestPath ?? "Cargo.toml");
  const targetDir = resolve(sourceRoot, options.targetDir ?? "target");
  const outDir = resolve(options.out ?? DEFAULT_OUT);
  const artifactDir = resolve(options.artifactDir ?? join(outDir, "artifacts"));
  const target = options.target ?? null;
  const cargo = options.cargo ?? "cargo";
  const locked = options.noLocked !== true;

  const before = currentSourceState(sourceRoot);
  if (!before.ok) {
    process.stderr.write(`${before.reason}\n`);
    return 2;
  }

  const cargoVersion = runOnce(cargo, ["--version"], sourceRoot);
  const rustcVersion = runOnce("rustc", ["-vV"], sourceRoot);
  const host = /^host:\s*(.+)$/m.exec(rustcVersion.stdout)?.[1]?.trim() ?? null;
  const effectiveTarget = target ?? host;
  if (!effectiveTarget) {
    process.stderr.write("cannot determine rustc host target; pass --target\n");
    return 2;
  }

  const args = ["build", "--release"];
  if (locked) args.push("--locked");
  args.push("--manifest-path", manifestPath, "--bin", options.bin, "--target-dir", targetDir);
  if (target) args.push("--target", target);

  const startedAtUtc = new Date().toISOString();
  const started = process.hrtime.bigint();
  const result = spawnSync(cargo, args, {
    cwd: sourceRoot,
    encoding: "utf8",
    timeout: 0,
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const durationMs = Math.round(Number(process.hrtime.bigint() - started) / 1e6);
  const finishedAtUtc = new Date().toISOString();
  const after = currentSourceState(sourceRoot);
  const sourceStable = after.ok && sameSourceState(
    {
      gitHead: before.head,
      statusSha256: before.statusSha256,
      trackedDiffSha256: before.trackedDiffSha256,
    },
    after,
  );

  const outputPath = join(
    targetDir,
    ...(target ? [target] : []),
    "release",
    process.platform === "win32" ? `${options.bin}.exe` : options.bin,
  );
  const command = {
    program: cargo,
    args,
    cwd: sourceRoot,
    startedAtUtc,
    finishedAtUtc,
    durationMs,
    exitCode: result.status,
    stdoutBytes: Buffer.byteLength(result.stdout ?? "", "utf8"),
    stderrBytes: Buffer.byteLength(result.stderr ?? "", "utf8"),
    stdoutSha256: hashText(result.stdout ?? ""),
    stderrSha256: hashText(result.stderr ?? ""),
    stdoutTail: tail(result.stdout ?? ""),
    stderrTail: tail(result.stderr ?? ""),
  };
  const source = {
    root: sourceRoot,
    gitHead: before.head,
    branch: before.branch,
    dirty: before.dirty,
    statusSha256: before.statusSha256,
    trackedDiffSha256: before.trackedDiffSha256,
  };
  const toolchain = {
    cargo: { status: cargoVersion.status, stdout: cargoVersion.stdout.trim(), stderr: cargoVersion.stderr.trim() },
    rustc: { status: rustcVersion.status, stdout: rustcVersion.stdout.trim(), stderr: rustcVersion.stderr.trim() },
    host,
    target: effectiveTarget,
  };

  const errors = [];
  if (result.error) errors.push(`cargo failed to start: ${result.error.message}`);
  if (result.status !== 0) errors.push(`cargo exited ${result.status ?? "null"}`);
  if (!sourceStable) errors.push("source tree changed during the build");
  let output = null;
  if (errors.length === 0) {
    try {
      const info = await stat(outputPath);
      const hashed = await hashFile(outputPath);
      const artifactName = `${options.product}-${before.head.slice(0, 8)}-${stamp()}-${options.bin}${process.platform === "win32" ? ".exe" : ""}`;
      const artifactPath = join(artifactDir, artifactName);
      await mkdir(artifactDir, { recursive: true });
      await copyFile(outputPath, artifactPath);
      const artifact = await hashFile(artifactPath);
      if (artifact.sha256 !== hashed.sha256) {
        throw new Error(`copied artifact ${artifact.sha256} does not match build output ${hashed.sha256}`);
      }
      output = {
        path: artifactPath,
        buildPath: outputPath,
        bytes: artifact.bytes,
        sha256: artifact.sha256,
        modifiedAtUtc: info.mtime.toISOString(),
        durable: true,
      };
      const version = runOnce(outputPath, ["--version"], sourceRoot);
      const receipt = {
        schema: BUILD_RECEIPT_SCHEMA,
        ok: true,
        product: options.product,
        profile: "release",
        source,
        command,
        toolchain,
        output,
        version: { status: version.status, stdout: version.stdout.trim(), stderr: version.stderr.trim() },
        generatedAtUtc: new Date().toISOString(),
      };
      const written = await writeBuildReceipt(outDir, receipt);
      const reloaded = await loadBuildReceipt(written.path);
      if (!reloaded.ok) {
        process.stderr.write(`recorded build receipt failed verification: ${reloaded.errors.join("; ")}\n`);
        return 1;
      }
      process.stdout.write(
        `build receipt: ${written.path}\nprofile: release\ntarget: ${effectiveTarget}\noutput: ${outputPath}\nsha256: ${output.sha256}\nsource: ${source.gitHead}${source.dirty ? " (dirty)" : " (clean)"}\n`,
      );
      return 0;
    } catch (error) {
      errors.push(`build output is missing or unreadable: ${error.message}`);
    }
  }

  const failed = {
    schema: BUILD_RECEIPT_SCHEMA,
    ok: false,
    product: options.product,
    profile: "release",
    source,
    command,
    toolchain,
    output,
    errors,
    generatedAtUtc: new Date().toISOString(),
  };
  const written = await writeBuildReceipt(outDir, failed);
  process.stderr.write(`release build failed: ${errors.join("; ")}\nfailed receipt: ${written.path}\n`);
  return 1;
}

function runOnce(program, args, cwd) {
  const result = spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    timeout: 30_000,
    windowsHide: true,
    maxBuffer: 8 * 1024 * 1024,
  });
  return {
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    error: result.error ?? null,
  };
}

function hashText(text) {
  return `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
}

function tail(text) {
  return text.length > MAX_TAIL ? text.slice(-MAX_TAIL) : text;
}

async function writeBuildReceipt(dir, receipt) {
  await mkdir(dir, { recursive: true });
  const filename = `${receipt.product}-${(receipt.source.gitHead ?? "nogit").slice(0, 8)}-${stamp()}-release${receipt.ok ? "" : "-failed"}.json`;
  const path = join(dir, filename);
  const text = `${JSON.stringify(receipt, null, 2)}\n`;
  await writeFile(path, text, "utf8");
  const sha256 = `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
  await writeFile(`${path}.sha256`, `${sha256}  ${filename}\n`, "utf8");
  return { path, sha256 };
}

function stamp() {
  return new Date().toISOString().replace(/[-:]/g, "").replace(/\.(\d{3})Z$/, "$1Z");
}

function usage(message) {
  process.stderr.write(`${message}\n`);
  return 3;
}

function parseBuildArgs(argv) {
  const values = new Set([
    "product",
    "bin",
    "source-root",
    "manifest-path",
    "target-dir",
    "target",
    "out",
    "artifact-dir",
    "cargo",
  ]);
  const booleans = new Set(["help", "no-locked"]);
  const options = {};
  const errors = [];
  for (let index = 0; index < argv.length; index += 1) {
    const raw = argv[index];
    if (!raw.startsWith("--")) {
      errors.push(`unexpected positional argument: ${raw}`);
      continue;
    }
    const body = raw.slice(2);
    const equals = body.indexOf("=");
    const flag = equals === -1 ? body : body.slice(0, equals);
    const inline = equals === -1 ? undefined : body.slice(equals + 1);
    if (booleans.has(flag)) {
      if (inline !== undefined) errors.push(`--${flag} does not take a value`);
      if (flag === "no-locked") options.noLocked = true;
      if (flag === "help") options.help = true;
      continue;
    }
    if (!values.has(flag)) {
      errors.push(`unknown flag: --${flag}`);
      continue;
    }
    const value = inline !== undefined ? inline : argv[++index];
    if (value === undefined) {
      errors.push(`--${flag} requires a value`);
      continue;
    }
    options[flag.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
  }
  return { options, errors };
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((error) => {
    process.stderr.write(`record-build error: ${error?.stack ?? error}\n`);
    process.exitCode = 4;
  });
