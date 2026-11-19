
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFile, link, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import { join, resolve } from "node:path";

import { MODE_B_NOTE } from "../scripts/lib/scenarios.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const BENCH = join(REPO_ROOT, "benchmarks", "scripts", "bench.mjs");
const FAKE = join(REPO_ROOT, "benchmarks", "fixtures", "fake-mcp-server.mjs");
const DISPLAY = "--no-display-probe";

async function withTemp(fn) {
  const dir = await mkdtemp(join(os.tmpdir(), "hand-bench-test-"));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function runDriver(args, { timeoutMs = 120_000 } = {}) {
  return new Promise((resolveRun) => {
    const child = spawn(process.execPath, [BENCH, ...args], {
      cwd: REPO_ROOT,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
    }, timeoutMs);
    child.on("close", (code) => {
      clearTimeout(timer);
      resolveRun({ code, stdout, stderr, timedOut });
    });
  });
}

function fakeArgs(extra = []) {
  const args = ["--command", process.execPath, "--arg", FAKE, "--test-fixture"];
  for (const value of extra) args.push("--arg", value);
  return args;
}

async function readReceipt(dir) {
  const names = (await readdir(dir)).filter((name) => name.endsWith(".json"));
  assert.equal(names.length, 1, `expected one receipt, found ${names.join(", ")}`);
  return JSON.parse(await readFile(join(dir, names[0]), "utf8"));
}

async function fakeBrowser(dir) {
  const path = join(dir, "chrome-headless-shell.exe");
  await writeFile(path, "driver test placeholder; never executed\n", "utf8");
  return path;
}

test("hand uses prefixed desktop_list_windows while Puppet uses unprefixed list_windows", async () => {
  await withTemp(async (dir) => {
    const handOut = join(dir, "hand");
    const hand = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(),
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      handOut,
      DISPLAY,
    ]);
    assert.equal(hand.code, 0, hand.stderr);
    const handReceipt = await readReceipt(handOut);
    assert.equal(handReceipt.testOnly, true);
    assert.match(handReceipt.evidence, /driver-test-only/);
    assert.equal(handReceipt.verdict.status, "pass");
    assert.equal(handReceipt.scenario.tool, "desktop_list_windows");
    assert.equal(handReceipt.stats.attempted, 100);
    assert.equal(handReceipt.samples.warmup.attempts.length, 10);
    assert.equal(handReceipt.samples.measured.attempts.length, 100);
    assert.equal(handReceipt.load.maxConcurrentRequests, 1);
    assert.ok(handReceipt.fingerprint.desktopTarget.hash);

    const puppetOut = join(dir, "puppet");
    const puppet = await runDriver([
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
      "--out",
      puppetOut,
      DISPLAY,
    ]);
    assert.equal(puppet.code, 0, puppet.stderr);
    const puppetReceipt = await readReceipt(puppetOut);
    assert.equal(puppetReceipt.scenario.tool, "list_windows");
    assert.equal(puppetReceipt.verdict.status, "pass");
  });
});

test("hand prefixed browser snapshot validates 100 fixture controls and freezes digests", async () => {
  await withTemp(async (dir) => {
    const browser = await fakeBrowser(dir);
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "browser-snapshot",
      "--browser-exe",
      browser,
      ...fakeArgs(),
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 0, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.scenario.tool, "browser_snapshot");
    assert.equal(receipt.verdict.status, "pass");
    assert.equal(receipt.setup.ok, true);
    assert.deepEqual(
      receipt.setup.steps.map((step) => step.tool),
      ["browser_navigate", "browser_viewport", "browser_snapshot"],
    );
    assert.equal(receipt.fingerprint.fixture.ok, true);
    assert.equal(receipt.fingerprint.fixture.controlCount, 100);
    assert.match(receipt.fingerprint.fixture.pin.file, /fixtures[\\/]SHA256SUMS$/);
    assert.equal(receipt.fingerprint.browser.role, "chrome-headless-shell");
    assert.match(receipt.fingerprint.browser.sha256, /^sha256:[0-9a-f]{64}$/);
    assert.equal(receipt.fingerprint.browser.launch.verified, true);
    assert.equal(receipt.fingerprint.browser.launch.path.toLowerCase(), browser.toLowerCase());
    assert.equal(receipt.fingerprint.browser.launch.sha256, receipt.fingerprint.browser.sha256);
    assert.equal(receipt.stats.attempted, 100);
    assert.equal(receipt.samples.measured.attempts.every((attempt) => attempt.ok), true);
    assert.equal(receipt.verdict.status, "pass");
    assert.equal(receipt.verdict.releaseBlocked, true);
    assert.equal(receipt.verdict.releaseIdentityUnverified, true);
    assert.equal(receipt.product.releaseIdentity.verified, false);
  });
});

test("measured JSON-RPC failures stay in the distribution and fail the verdict", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "gladius",
      "--scenario",
      "warm-ping",
      ...fakeArgs(["--mode=fail-every=3"]),
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "fail");
    assert.equal(receipt.verdict.releaseBlocked, true);
    assert.equal(receipt.stats.attempted, 100);
    assert.equal(receipt.stats.observed, 100);
    assert.ok(receipt.stats.failed > 0);
    assert.equal(receipt.samples.measured.failures, receipt.stats.failed);
    for (const attempt of receipt.samples.measured.attempts.filter((entry) => !entry.ok)) {
      assert.ok(attempt.error, `failure #${attempt.index} has no error`);
      assert.equal(attempt.dispatched, true);
    }
    assert.match(run.stderr, /failures \(/);
  });
});

test("incomplete measured output is a failed attempt, not a passing sample", async () => {
  await withTemp(async (dir) => {
    const browser = await fakeBrowser(dir);
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "browser-snapshot",
      "--browser-exe",
      browser,
      ...fakeArgs(["--mode=incomplete-after=2"]),
      "--warmup",
      "1",
      "--measured",
      "3",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.setup.ok, true, "setup must pass so the failures are measured");
    assert.equal(receipt.verdict.status, "fail");
    assert.equal(receipt.stats.failed, 3);
    for (const attempt of receipt.samples.measured.attempts) {
      assert.equal(attempt.outputIncomplete, true);
      assert.match(attempt.error, /incomplete-output/);
    }
  });
});

test("missing product binary is unsupported with no samples and exit 2", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "warm-ping",
      "--command",
      join(dir, "definitely-missing.exe"),
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 2, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "unsupported");
    assert.equal(receipt.unsupported.stage, "product-resolution");
    assert.equal(receipt.stats, null);
    assert.equal(receipt.samples.measured.attempts.length, 0);
  });
});

test("desktop scenario without a controlled target is unsupported before spawning", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "puppet",
      "--scenario",
      "desktop-enumerate",
      ...fakeArgs(),
      "--warmup",
      "2",
      "--measured",
      "6",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 2, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "unsupported");
    assert.equal(receipt.unsupported.stage, "scenario-plan");
    assert.match(receipt.reason, /controlled desktop fixture target/);
    assert.equal(receipt.catalog, null, "the product must not be spawned without a target");
    assert.equal(receipt.samples.measured.attempts.length, 0);
  });
});

test("a server that exits mid-run leaves every remaining attempt as a visible failure", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(["--mode=exit-after=5"]),
      "--warmup",
      "2",
      "--measured",
      "10",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "fail");
    assert.equal(receipt.samples.measured.attempts.length, 10);
    assert.equal(receipt.stats.failed, 10);
    assert.ok(receipt.stats.observed < 10);
    assert.ok(receipt.samples.measured.attempts.some((attempt) => attempt.dispatched === false));
    assert.equal(receipt.process.exited.code, 17);
  });
});

test("--fixtures override is refused so the fingerprint and served bytes cannot diverge", async () => {
  await withTemp(async (dir) => {
    const browser = await fakeBrowser(dir);
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "browser-snapshot",
      "--fixtures",
      join(dir, "not-served"),
      "--browser-exe",
      browser,
      ...fakeArgs(),
      "--warmup",
      "1",
      "--measured",
      "2",
      "--out",
      join(dir, "receipts"),
      DISPLAY,
    ]);
    assert.equal(run.code, 3, run.stderr);
    assert.match(run.stderr, /unknown flag: --fixtures/);
  });
});

test("a browser that cannot be tied to the fingerprinted executable blocks before measurement", async () => {
  await withTemp(async (dir) => {
    const browser = await fakeBrowser(dir);
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "browser-snapshot",
      "--browser-exe",
      browser,
      ...fakeArgs(["--browser-log=none"]),
      "--warmup",
      "1",
      "--measured",
      "2",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "blocked");
    assert.equal(receipt.blocked.stage, "browser-binding");
    assert.equal(receipt.fingerprint.browser.launch.verified, false);
    assert.match(receipt.reason, /did not log the launched chromium path/);
    assert.equal(receipt.stats, null);

    const otherOut = join(dir, "receipts-other");
    const other = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "browser-snapshot",
      "--browser-exe",
      browser,
      ...fakeArgs(["--browser-log=other"]),
      "--warmup",
      "1",
      "--measured",
      "2",
      "--out",
      otherOut,
      DISPLAY,
    ]);
    assert.equal(other.code, 1, other.stderr);
    const otherReceipt = await readReceipt(otherOut);
    assert.equal(otherReceipt.verdict.status, "blocked");
    assert.equal(otherReceipt.blocked.stage, "browser-binding");
    assert.match(otherReceipt.reason, /not the fingerprinted browser/);
    assert.equal(otherReceipt.stats, null);
  });
});

test("desktop capture requires and records target identity, PrintWindow and safety evidence", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-capture",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(),
      "--warmup",
      "2",
      "--measured",
      "6",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 0, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "pass");
    assert.deepEqual(
      receipt.setup.steps.map((step) => step.tool),
      ["desktop_list_windows", "desktop_screenshot"],
    );
    assert.equal(receipt.setup.ok, true);
    assert.equal(receipt.samples.measured.attempts.every((attempt) => attempt.ok), true);
    assert.equal(receipt.samples.measured.attempts[0].detail.backend, "printwindow");
    assert.equal(receipt.samples.measured.attempts[0].detail.hwnd, 4242);
    assert.equal(receipt.samples.measured.attempts[0].detail.blank.verdict, "content");
    assert.equal(receipt.samples.measured.attempts[0].detail.note, MODE_B_NOTE);
    assert.equal(receipt.verdict.releaseBlocked, true);
    assert.equal(receipt.product.releaseIdentity.verified, false);
  });
});

test("blank capture evidence is an unsafe measured failure, never a pass", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-capture",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(["--capture-defect=blank", "--capture-defect-after=2"]),
      "--warmup",
      "1",
      "--measured",
      "3",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.setup.ok, true, "setup must pass so the defect is measured");
    assert.equal(receipt.verdict.status, "fail");
    assert.equal(receipt.stats.failed, 3);
    for (const attempt of receipt.samples.measured.attempts) {
      assert.equal(attempt.ok, false);
      assert.equal(attempt.unsafeEvidence, true);
      assert.match(attempt.error, /^unsafe-evidence:/);
    }
  });
});

test("a warmup failure is reported as a warning and stays outside the measured p95", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(["--mode=hang-on=4"]),
      "--warmup",
      "2",
      "--measured",
      "3",
      "--timeout-ms",
      "150",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 0, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "pass");
    assert.equal(receipt.samples.warmup.failures, 1);
    assert.equal(receipt.verdict.warmupFailures, 1);
    assert.equal(receipt.verdict.warmupAttempts, 2);
    assert.ok(receipt.verdict.warnings.some((warning) => /warmup attempts failed/.test(warning)));
    assert.ok(receipt.verdict.warnings.some((warning) => /release identity is unverified/.test(warning)));
    assert.equal(receipt.stats.attempted, 3);
    assert.equal(receipt.stats.failed, 0);
    assert.equal(receipt.stats.p95Ms !== null, true);
    assert.equal(receipt.verdict.releaseBlocked, true);
    assert.match(run.stderr, /warning:/);
  });
});

test("a timed-out measured request is a failure and the run continues", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(["--mode=hang-on=5"]),
      "--warmup",
      "1",
      "--measured",
      "3",
      "--timeout-ms",
      "200",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "fail");
    assert.equal(receipt.samples.measured.attempts.length, 3);
    assert.equal(receipt.stats.failed, 1);
    const failure = receipt.samples.measured.attempts.find((attempt) => !attempt.ok);
    assert.match(failure.error, /timed out/);
    assert.equal(receipt.stats.observed, 3);
  });
});

test("unparseable stdout fails the verdict as a protocol violation", async () => {
  await withTemp(async (dir) => {
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "desktop-enumerate",
      "--desktop-title-contains",
      "hand-bench-fixture",
      ...fakeArgs(["--mode=bad-json"]),
      "--warmup",
      "1",
      "--measured",
      "2",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 1, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.process.protocolViolations.count, 1);
    assert.equal(receipt.verdict.status, "fail");
    assert.ok(receipt.verdict.reasons.some((reason) => reason.includes("protocol violation")));
  });
});

test("live sample sizes are enforced and a test fixture cannot masquerade as measured", async () => {
  await withTemp(async (dir) => {
    const short = await runDriver([
      "--product",
      "gladius",
      "--scenario",
      "warm-ping",
      "--command",
      process.execPath,
      "--measured",
      "5",
      DISPLAY,
    ]);
    assert.equal(short.code, 3);
    assert.match(short.stderr, /requires >=10 warmups and >=100 measured/);

    const unmarked = await runDriver([
      "--product",
      "gladius",
      "--scenario",
      "warm-ping",
      "--command",
      process.execPath,
      "--arg",
      FAKE,
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      join(dir, "receipts"),
      DISPLAY,
    ]);
    assert.equal(unmarked.code, 3);
    assert.match(unmarked.stderr, /requires --test-fixture/);
    await assert.rejects(readdir(join(dir, "receipts")));
  });
});

test("a binary with no embedded identity passes but marks releaseBlocked and unverified", async () => {
  await withTemp(async (dir) => {
    const debugDir = join(dir, "debug");
    await mkdir(debugDir);
    const debugNode = join(debugDir, "node.exe");
    try {
      await link(process.execPath, debugNode);
    } catch {
      await copyFile(process.execPath, debugNode);
    }
    const out = join(dir, "receipts");
    const run = await runDriver([
      "--product",
      "hand",
      "--scenario",
      "warm-ping",
      "--command",
      debugNode,
      "--arg",
      FAKE,
      "--test-fixture",
      "--warmup",
      "10",
      "--measured",
      "100",
      "--out",
      out,
      DISPLAY,
    ]);
    assert.equal(run.code, 0, run.stderr);
    const receipt = await readReceipt(out);
    assert.equal(receipt.verdict.status, "pass");
    assert.equal(receipt.verdict.releaseBlocked, true);
    assert.equal(receipt.verdict.releaseIdentityUnverified, true);
    assert.equal(receipt.product.releaseIdentity.claimed, null);
    assert.equal(receipt.product.releaseIdentity.basis, "embedded-build-metadata");
    assert.equal(receipt.product.releaseIdentity.verified, false);
    assert.ok(
      receipt.verdict.reasons.some((reason) => /binary embeds no build profile/.test(reason)),
      receipt.verdict.reasons.join(" | "),
    );
    assert.ok(receipt.verdict.warnings.some((warning) => /release identity is unverified/.test(warning)));
  });
});

test("--list names every scenario and both naming conventions", async () => {
  const run = await runDriver(["--list"]);
  assert.equal(run.code, 0);
  for (const id of ["warm-ping", "browser-snapshot", "desktop-enumerate", "desktop-capture"]) {
    assert.match(run.stdout, new RegExp(id));
  }
  assert.match(run.stdout, /hand=browser_snapshot/);
  assert.match(run.stdout, /gladius=snapshot/);
  assert.match(run.stdout, /puppet=list_windows/);
  assert.match(run.stdout, /browser_snapshot|desktop_list_windows/);
});
