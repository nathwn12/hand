
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import { join, resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const BENCH = join(REPO_ROOT, "benchmarks", "scripts", "bench.mjs");
const COMPARE = join(REPO_ROOT, "benchmarks", "scripts", "compare.mjs");
const FAKE = join(REPO_ROOT, "benchmarks", "fixtures", "fake-mcp-server.mjs");
const DISPLAY = "--no-display-probe";

async function withTemp(fn) {
  const dir = await mkdtemp(join(os.tmpdir(), "hand-bench-compare-"));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function run(script, args) {
  return new Promise((resolveRun) => {
    const child = spawn(process.execPath, [script, ...args], {
      cwd: REPO_ROOT,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("close", (code) => resolveRun({ code, stdout, stderr }));
  });
}

function fakeArgs(extra = []) {
  const args = ["--command", process.execPath, "--arg", FAKE, "--test-fixture"];
  for (const value of extra) args.push("--arg", value);
  return args;
}

test("comparison keeps failures and unsupported rows visible and refuses a test-only gate", async () => {
  await withTemp(async (dir) => {
    const receipts = join(dir, "receipts");
    const common = ["--out", receipts, DISPLAY];

    const handEnumerate = await run(BENCH, [
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(),
      "--warmup",
      "2",
      "--measured",
      "6",
      ...common,
    ]);
    assert.equal(handEnumerate.code, 0, handEnumerate.stderr);

    const puppetEnumerate = await run(BENCH, [
      "--product",
      "puppet",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(),
      "--warmup",
      "2",
      "--measured",
      "6",
      ...common,
    ]);
    assert.equal(puppetEnumerate.code, 0, puppetEnumerate.stderr);

    const gladiusPing = await run(BENCH, [
      "--product",
      "gladius",
      "--scenario",
      "warm-ping",
      ...fakeArgs(["--mode=fail-every=3"]),
      "--warmup",
      "2",
      "--measured",
      "10",
      ...common,
    ]);
    assert.equal(gladiusPing.code, 1, gladiusPing.stderr);

    const handMissing = await run(BENCH, [
      "--product",
      "hand",
      "--scenario",
      "warm-ping",
      "--command",
      join(dir, "missing.exe"),
      ...common,
    ]);
    assert.equal(handMissing.code, 2, handMissing.stderr);

    const comparisonPath = join(dir, "comparison.json");
    const report = await run(COMPARE, ["--receipts", receipts, "--out", comparisonPath]);
    assert.equal(report.code, 0, report.stderr);
    assert.match(report.stdout, /desktop-enumerate/);
    assert.match(report.stdout, /warm-ping/);
    assert.match(report.stdout, /unsupported/);
    assert.match(report.stdout, /fail/);
    assert.match(report.stdout, /YES/);

    const comparison = JSON.parse(await readFile(comparisonPath, "utf8"));
    assert.equal(comparison.receipts.length, 4);
    const desktop = comparison.groups.find((group) => group.scenario === "desktop-enumerate");
    const ping = comparison.groups.find((group) => group.scenario === "warm-ping");
    assert.equal(desktop.comparable, false, JSON.stringify(desktop.differences));
    assert.ok(desktop.differences.some((difference) => difference.includes("test-only")));
    assert.equal(desktop.allPass, false, "test-only receipts cannot satisfy the release gate");
    assert.ok(desktop.members.every((member) => member.status === "pass" && member.testOnly));
    assert.equal(ping.allPass, false);
    assert.ok(ping.members.some((member) => member.status === "unsupported"));
    assert.ok(ping.members.some((member) => member.status === "fail"));
    assert.ok(ping.differences.some((difference) => difference.includes("missing product receipt")));
    assert.equal(comparison.receipts.filter((row) => row.testOnly === true).length, 3);
    const unsupportedRow = ping.members.find((member) => member.status === "unsupported");
    assert.equal(unsupportedRow.testOnly, false);
    assert.ok(ping.differences.some((difference) => difference.includes("test-only receipt cannot satisfy")));

    const gate = await run(COMPARE, [
      "--receipts",
      receipts,
      "--out",
      join(dir, "comparison-gate.json"),
      "--require-pass",
    ]);
    assert.equal(gate.code, 1);
    assert.match(gate.stderr, /release gate not satisfied/);
  });
});

test("an unreadable receipt is a hard error, not a skipped row", async () => {
  await withTemp(async (dir) => {
    await writeFile(join(dir, "broken.json"), "{ not json", "utf8");
    const runResult = await run(COMPARE, ["--receipts", dir, "--out", join(dir, "comparison.json")]);
    assert.equal(runResult.code, 4);
    assert.match(runResult.stderr, /cannot parse/);
  });
});
