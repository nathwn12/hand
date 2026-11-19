
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { basename, join } from "node:path";

import { parseBuildIdentity } from "./build.mjs";
import { hashFile } from "./fingerprint.mjs";

export const BUILD_RECEIPT_SCHEMA = "hand-mcp.benchmark.build-receipt/v1";
const HASH_RE = /^sha256:[0-9a-f]{64}$/;

function sha256Text(text) {
  return `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
}

function cleanLines(text) {
  return text
    .split(/\r?\n/)
    .map((line) => line.trimEnd())
    .filter((line) => line.length > 0);
}

export function currentSourceState(root) {
  const git = (args) =>
    execFileSync("git", args, {
      cwd: root,
      encoding: "utf8",
      timeout: 30_000,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
  try {
    const head = git(["rev-parse", "HEAD"]).trim();
    const branch = git(["rev-parse", "--abbrev-ref", "HEAD"]).trim();
    const status = cleanLines(git(["status", "--porcelain=v1", "--untracked-files=no"]));
    const diff = git(["diff", "--binary", "HEAD"]);
    return {
      ok: true,
      root,
      head,
      branch,
      dirty: status.length > 0,
      statusLines: status,
      statusSha256: sha256Text(status.join("\n")),
      trackedDiffSha256: sha256Text(diff),
    };
  } catch (error) {
    return {
      ok: false,
      root,
      reason: `source tree is not a committed git work tree: ${String(error.stderr ?? error.message).trim()}`,
    };
  }
}

export function sameSourceState(recorded, current) {
  if (!recorded?.gitHead || !current?.ok) return false;
  return (
    recorded.gitHead === current.head &&
    recorded.statusSha256 === current.statusSha256 &&
    recorded.trackedDiffSha256 === current.trackedDiffSha256
  );
}

export function verifyBaselinePin({ receipt, pins }) {
  const id = receipt.product?.id;
  const source = pins?.sources?.[id];
  if (!source) return { verified: false, reason: `${id ?? "unknown"} has no baseline pin` };
  if (id !== "gladius" && id !== "puppet") {
    return { verified: false, reason: `${id} is not a pinned baseline product` };
  }
  const recorded = receipt.product?.executable?.sha256;
  const pinned = source.capture?.capturedExecutableSha256;
  if (!HASH_RE.test(recorded ?? "") || recorded !== pinned) {
    return {
      verified: false,
      reason: `${id} binary ${recorded ?? "(missing)"} does not match the pinned ${pinned ?? "(missing)"}`,
    };
  }
  const version = String(receipt.product?.version?.stdout ?? "");
  if (!version.toLowerCase().includes(`${id} ${source.version}`.toLowerCase())) {
    return {
      verified: false,
      reason: `${id} version ${JSON.stringify(version)} does not match pinned ${source.version}`,
    };
  }
  return {
    verified: true,
    kind: "baseline-pin",
    reference: `baseline/pins.json#sources.${id}.capture.capturedExecutableSha256`,
    detail: { pin: source.pin, version: source.version, executableSha256: pinned },
  };
}

export async function loadBuildReceipt(file) {
  const errors = [];
  let text;
  let sidecar;
  try {
    text = await readFile(file, "utf8");
    sidecar = await readFile(`${file}.sha256`, "utf8");
  } catch (error) {
    return { file, ok: false, errors: [`build receipt or sidecar unreadable: ${error.message}`] };
  }
  const sidecarMatch = /^(?:sha256:)?([0-9a-f]{64})\s+\*?(.+?)\s*$/i.exec(sidecar.trim());
  if (!sidecarMatch) {
    errors.push("build receipt sidecar is malformed");
  } else {
    const actual = createHash("sha256").update(text, "utf8").digest("hex");
    if (actual !== sidecarMatch[1].toLowerCase()) errors.push("build receipt sidecar hash mismatch");
    if (sidecarMatch[2] !== basename(file)) errors.push("build receipt sidecar names another file");
  }
  let build;
  try {
    build = JSON.parse(text);
  } catch (error) {
    return { file, ok: false, errors: [...errors, `build receipt JSON is invalid: ${error.message}`] };
  }
  if (build.schema !== BUILD_RECEIPT_SCHEMA) errors.push(`unexpected build receipt schema: ${build.schema}`);
  if (build.ok !== true) errors.push("build receipt is not a successful build");
  if (build.profile !== "release") errors.push("build receipt profile is not release");
  if (!Array.isArray(build.command?.args) || !build.command.args.includes("build") || !build.command.args.includes("--release")) {
    errors.push("build receipt command is not a cargo build --release");
  }
  if (typeof build.command?.program !== "string" || !basename(build.command.program).toLowerCase().includes("cargo")) {
    errors.push("build receipt command program is not cargo");
  }
  if (build.command?.exitCode !== 0) errors.push("build receipt records a non-zero cargo exit");
  if (!HASH_RE.test(build.output?.sha256 ?? "")) errors.push("build receipt output hash is missing or malformed");
  if (!build.source?.gitHead) errors.push("build receipt source HEAD is missing");
  if (!build.toolchain?.target) errors.push("build receipt target is missing");
  return { file, ok: errors.length === 0, errors, build, sha256: `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}` };
}

export async function verifyBuildReceipt({ build, receipt, currentBinarySha }) {
  const id = receipt.product?.id;
  if (build.product && build.product !== id) {
    return { verified: false, reason: `build receipt names product ${build.product}, not ${id}` };
  }
  const measured = receipt.product?.executable?.sha256;
  if (!HASH_RE.test(measured ?? "") || measured !== build.output?.sha256) {
    return {
      verified: false,
      reason: `measured binary ${measured ?? "(missing)"} does not match build output ${build.output?.sha256 ?? "(missing)"}`,
    };
  }
  const embedded = parseBuildIdentity(receipt.product?.version);
  if (id === "hand") {
    if (!embedded) {
      return {
        verified: false,
        reason: `hand --version carries no embedded commit and profile: ${JSON.stringify(String(receipt.product?.version?.stdout ?? "").slice(0, 200))}`,
      };
    }
    if (!build.source?.gitHead || !build.source.gitHead.startsWith(embedded.sha)) {
      return {
        verified: false,
        reason: `the binary embeds commit ${embedded.sha} but the build receipt records ${build.source?.gitHead ?? "(missing)"}`,
      };
    }
    if (embedded.profile !== "release") {
      return {
        verified: false,
        reason: `the binary embeds profile ${JSON.stringify(embedded.profile)}, not release`,
      };
    }
  }
  if (currentBinarySha !== build.output.sha256) {
    return {
      verified: false,
      reason: `binary at the measured path is ${currentBinarySha ?? "(unreadable)"}, not build output ${build.output.sha256}`,
    };
  }
  let artifact;
  try {
    artifact = await hashFile(build.output.path);
  } catch (error) {
    return { verified: false, reason: `recorded build artifact is unreadable: ${error.message}` };
  }
  if (artifact.sha256 !== build.output.sha256) {
    return {
      verified: false,
      reason: `recorded build artifact hash ${artifact.sha256} does not match receipt ${build.output.sha256}`,
    };
  }
  return {
    verified: true,
    kind: "local-release-build",
    reference: build.receiptFile ?? build.output.path,
    detail: {
      head: build.source.gitHead,
      dirty: build.source.dirty,
      target: build.toolchain.target,
      command: build.command,
      output: build.output,
    },
  };
}

export async function verifyReleaseEvidence({
  receipt,
  currentBinarySha,
  pins,
  builds,
}) {
  const baseline = verifyBaselinePin({ receipt, pins });
  if (baseline.verified) return baseline;

  const reasons = baseline.reason ? [baseline.reason] : [];
  let matchedBuild = false;
  for (const entry of builds) {
    if (!entry.ok) {
      reasons.push(`${basename(entry.file)}: ${entry.errors.join("; ")}`);
      continue;
    }
    if (entry.build.output?.sha256 !== receipt.product?.executable?.sha256) continue;
    matchedBuild = true;
    const result = await verifyBuildReceipt({
      build: { ...entry.build, receiptFile: entry.file },
      receipt,
      currentBinarySha,
    });
    if (result.verified) return result;
    reasons.push(`${basename(entry.file)}: ${result.reason}`);
  }
  const suffix =
    builds.length === 0
      ? "; no local release-build receipts available"
      : matchedBuild
        ? ""
        : "; no local release-build receipt matches this binary";
  return {
    verified: false,
    reason: `${reasons.length ? reasons.join("; ") : "no baseline pin or local release-build receipt matches this binary"}${suffix}`,
  };
}

export async function loadBuildReceipts(dir) {
  let names;
  try {
    names = (await readdir(dir)).filter((name) => name.endsWith(".json")).sort();
  } catch {
    return [];
  }
  const entries = [];
  for (const name of names) entries.push(await loadBuildReceipt(join(dir, name)));
  return entries;
}
