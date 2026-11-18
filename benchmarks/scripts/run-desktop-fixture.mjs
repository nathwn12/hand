#!/usr/bin/env node
// Controlled 1280x720 desktop fixture launcher and benchmark coordinator.
//
// The fixture is shown with WS_EX_NOACTIVATE and ShowWithoutActivation, after a
// short wait for a quiet desktop so it does not cover an active window
// unannounced. Desktop changes during measurement are attributed, not hidden: a
// pointer or foreground delta with real input behind it is the owner's, a delta
// with no input is unattributable, and the fixture taking the foreground always
// fails. `GetLastInputInfo` supplies the input tick; the before/after samples
// stay in the output.
//
// On success it runs desktop-enumerate and desktop-capture for hand and Puppet
// against the fixture title, then terminates the fixture, confirms its window is
// gone and classifies the final desktop state again.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { access, mkdir, readFile, readdir, rm, stat } from "node:fs/promises";
import { join, resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const BENCH = join(REPO_ROOT, "benchmarks", "scripts", "bench.mjs");
const FIXTURE_SOURCE = join(REPO_ROOT, "benchmarks", "fixtures", "desktop-fixture", "hand-desktop-fixture.cs");
const STATE_SCRIPT = join(REPO_ROOT, "benchmarks", "fixtures", "win-state.ps1");
const CACHE_DIR = join(REPO_ROOT, "benchmarks", ".cache", "desktop-fixture");
const FIXTURE_EXE = join(CACHE_DIR, "hand-desktop-fixture.exe");
const RECEIPTS = join(REPO_ROOT, "benchmarks", "receipts");
const ARTIFACTS = join(REPO_ROOT, "benchmarks", "receipts", "builds", "artifacts");

function parse(argv) {
  const options = {
    startupOnly: false,
    title: null,
    handBinary: null,
    out: RECEIPTS,
    product: null,
    scenario: null,
    quietWaitMs: 5_000,
  };
  const errors = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--startup-only") options.startupOnly = true;
    else if (arg === "--title") options.title = argv[++index];
    else if (arg === "--hand-binary") options.handBinary = argv[++index];
    else if (arg === "--out") options.out = argv[++index];
    else if (arg === "--product") options.product = argv[++index];
    else if (arg === "--scenario") options.scenario = argv[++index];
    else if (arg === "--quiet-wait-ms") {
      const value = Number(argv[++index]);
      if (Number.isInteger(value) && value >= 0) options.quietWaitMs = value;
      else errors.push("--quiet-wait-ms requires a non-negative integer");
    } else errors.push(`unknown argument: ${arg}`);
  }
  if (!options.title) options.title = `hand-bench-desktop-1280x720-${process.pid}`;
  return { options, errors };
}

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

function findCsc() {
  const roots = [
    "C:\\Windows\\Microsoft.NET\\Framework64\\v4.0.30319",
    "C:\\Windows\\Microsoft.NET\\Framework\\v4.0.30319",
  ];
  for (const root of roots) {
    const path = join(root, "csc.exe");
    if (existsSync(path)) return { path, root };
  }
  return null;
}

async function compileFixture() {
  await mkdir(CACHE_DIR, { recursive: true });
  // Content-addressed: the fixture binary is named by its source hash and reused,
  // so hand and Puppet measure the identical bytes. A per-run rebuild would give
  // every receipt a different target hash and no two products could be compared.
  const source = await readFile(FIXTURE_SOURCE);
  const hash = createHash("sha256").update(source).digest("hex").slice(0, 16);
  const exe = join(CACHE_DIR, `hand-desktop-fixture-${hash}.exe`);
  if (await exists(exe)) return { ok: true, exe, compiled: false, sourceHash: hash };
  const csc = findCsc();
  if (!csc) return { ok: false, reason: "no .NET Framework csc.exe is available to compile the fixture" };
  const result = spawnSync(
    csc.path,
    [
      "/nologo",
      "/target:winexe",
      "/platform:x64",
      "/optimize+",
      `/out:${exe}`,
      `/r:${join(csc.root, "System.Windows.Forms.dll")}`,
      `/r:${join(csc.root, "System.Drawing.dll")}`,
      FIXTURE_SOURCE,
    ],
    { encoding: "utf8", windowsHide: true, timeout: 60_000 },
  );
  if (result.status !== 0) {
    return {
      ok: false,
      reason: `fixture compilation failed: ${(result.stderr || result.stdout || "").trim().slice(0, 2000)}`,
    };
  }
  return { ok: true, exe, compiled: true, sourceHash: hash };
}

function readState(hwnd = 0) {
  const result = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", STATE_SCRIPT, "-Hwnd", String(hwnd)],
    { encoding: "utf8", windowsHide: true, timeout: 30_000 },
  );
  if (result.status !== 0) throw new Error(`window-state probe failed: ${(result.stderr || result.stdout).trim()}`);
  return JSON.parse(result.stdout);
}

function desktopChanged(before, after) {
  return (
    before.foreground !== after.foreground ||
    before.cursorX !== after.cursorX ||
    before.cursorY !== after.cursorY
  );
}

/** A physical event advances `GetLastInputInfo`; a synthetic pointer write does not. */
function inputAdvanced(before, after) {
  return (
    Number.isInteger(before.inputTick) &&
    Number.isInteger(after.inputTick) &&
    before.inputTick !== after.inputTick
  );
}

/**
 * Attribute a desktop change. The fixture never activates and the launcher owns
 * no pointer API, so a change to another window, or a pointer delta with real
 * input behind it, is the owner's. Only the fixture taking the foreground or a
 * pointer delta with no input at all is unattributable.
 */
function classifyDesktopChange(before, after, fixtureHwnd) {
  if (!desktopChanged(before, after)) return "same";
  if (fixtureHwnd && after.foreground === fixtureHwnd && before.foreground !== fixtureHwnd) {
    return "fixture-stole";
  }
  if (after.foreground !== before.foreground) return "external";
  return inputAdvanced(before, after) ? "external" : "unexplained";
}

async function sleep(ms) {
  await new Promise((resolveWait) => setTimeout(resolveWait, ms));
}

/**
 * Wait for a quiet second before showing the fixture. The desktop can be in use;
 * this only avoids covering an active window unannounced. It never hides a
 * change during measurement, which the per-scenario checks attribute instead.
 */
async function waitForQuietDesktop(timeoutMs) {
  const started = Date.now();
  let previous = readState(0);
  while (Date.now() - started < timeoutMs) {
    await sleep(700);
    const next = readState(0);
    if (!desktopChanged(previous, next) && !inputAdvanced(previous, next)) {
      return { ok: true, state: next, waitedMs: Date.now() - started };
    }
    previous = next;
  }
  return { ok: false, state: previous, waitedMs: Date.now() - started };
}

async function waitForReady(path, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      return JSON.parse(await readFile(path, "utf8"));
    } catch {
      await new Promise((resolveWait) => setTimeout(resolveWait, 100));
    }
  }
  return null;
}

async function waitExit(child, timeoutMs) {
  if (child.exitCode !== null) return true;
  return new Promise((resolveWait) => {
    const timer = setTimeout(() => resolveWait(false), timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolveWait(true);
    });
  });
}

async function terminateFixture(child) {
  if (child.exitCode !== null) return true;
  child.kill();
  if (await waitExit(child, 5_000)) return true;
  spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, timeout: 15_000 });
  return waitExit(child, 5_000);
}

async function findHandBinary(explicit) {
  if (explicit) return resolve(explicit);
  const names = (await readdir(ARTIFACTS)).filter((name) => name.toLowerCase().endsWith(".exe")).sort();
  if (names.length === 0) return null;
  return join(ARTIFACTS, names[names.length - 1]);
}

function runBench(args) {
  return spawnSync(process.execPath, [BENCH, ...args], {
    cwd: REPO_ROOT,
    encoding: "utf8",
    windowsHide: true,
    timeout: 300_000,
    maxBuffer: 32 * 1024 * 1024,
  });
}

async function main() {
  const { options, errors } = parse(process.argv.slice(2));
  if (errors.length) {
    process.stderr.write(`${errors.join("\n")}\n`);
    return 3;
  }
  const compiled = await compileFixture();
  if (!compiled.ok) {
    process.stdout.write(`${JSON.stringify({ status: "unsupported", reason: compiled.reason })}\n`);
    return 2;
  }

  const readyPath = join(CACHE_DIR, `ready-${process.pid}.json`);
  await rm(readyPath, { force: true });
  const quiet = await waitForQuietDesktop(options.quietWaitMs);
  const before = quiet.state;
  // A short quiet wait is a courtesy, not a precondition: the fixture never
  // activates, and every change during measurement is attributed. Proceeding
  // on an active desktop is recorded, not hidden.
  const quietDeferred = !quiet.ok;
  const child = spawn(compiled.exe, [readyPath, options.title], {
    windowsHide: true,
    stdio: "ignore",
  });

  let fixture = null;
  const results = [];
  let safety = { quiet, quietDeferred, before, afterShow: null, afterMeasurements: null, afterClose: null };
  let status = "measured";
  let reason = null;
  try {
    fixture = await waitForReady(readyPath);
    if (!fixture) {
      status = "unsupported";
      reason = "fixture did not write its ready record within 20 s";
      return 2;
    }
    if (fixture.pid !== child.pid) {
      status = "unsupported";
      reason = `fixture pid ${fixture.pid} does not match the spawned pid ${child.pid}`;
      return 2;
    }
    const rectWidth = fixture.rect?.[2] - fixture.rect?.[0];
    const rectHeight = fixture.rect?.[3] - fixture.rect?.[1];
    const clientWidth = fixture.clientRect?.[2] - fixture.clientRect?.[0];
    const clientHeight = fixture.clientRect?.[3] - fixture.clientRect?.[1];
    if (rectWidth !== 1280 || rectHeight !== 720 || clientWidth !== 1280 || clientHeight !== 720) {
      status = "unsupported";
      reason = `fixture geometry is window ${rectWidth}x${rectHeight}, client ${clientWidth}x${clientHeight}; required 1280x720`;
      return 2;
    }

    await sleep(400);
    safety.afterShow = readState(fixture.hwnd);
    const showChange = classifyDesktopChange(before, safety.afterShow, fixture.hwnd);
    const fixtureCursorSame =
      fixture.cursorBeforeShow[0] === fixture.cursorAfterShow[0] &&
      fixture.cursorBeforeShow[1] === fixture.cursorAfterShow[1];
    const fixtureInputExplains =
      Number.isInteger(fixture.inputTickBeforeShow) &&
      Number.isInteger(fixture.inputTickAfterShow) &&
      fixture.inputTickBeforeShow !== fixture.inputTickAfterShow;
    const fixtureOwnStill =
      fixture.foregroundBeforeShow === fixture.foregroundAfterShow &&
      (fixtureCursorSame || fixtureInputExplains);
    if (showChange === "fixture-stole" || showChange === "unexplained" || !fixtureOwnStill) {
      status = "unsupported";
      reason = `showing the fixture changed the desktop in a way the launcher cannot attribute (${showChange})`;
      return 2;
    }

    if (options.startupOnly) {
      status = "safe";
      return 0;
    }

    const handBinary = await findHandBinary(options.handBinary);
    if (!handBinary) {
      status = "unsupported";
      reason = "no durable hand release artifact found under benchmarks/receipts/builds/artifacts";
      return 2;
    }

    const scenarios = [
      { product: "hand", scenario: "desktop-enumerate", command: handBinary },
      { product: "puppet", scenario: "desktop-enumerate", command: null },
      { product: "hand", scenario: "desktop-capture", command: handBinary },
      { product: "puppet", scenario: "desktop-capture", command: null },
    ].filter(
      (entry) =>
        (!options.product || entry.product === options.product) &&
        (!options.scenario || entry.scenario === options.scenario),
    );
    if (scenarios.length === 0) {
      status = "unsupported";
      reason = "no scenario matched --product/--scenario";
      return 2;
    }
    for (const entry of scenarios) {
      const args = [
        "--product",
        entry.product,
        "--scenario",
        entry.scenario,
        "--desktop-title-contains",
        options.title,
        "--out",
        resolve(options.out),
      ];
      if (entry.command) args.push("--command", entry.command);
      const stateBeforeRun = readState(fixture.hwnd);
      const run = runBench(args);
      const stateAfterRun = readState(fixture.hwnd);
      const result = {
        product: entry.product,
        scenario: entry.scenario,
        exitCode: run.status,
        stateBeforeRun,
        stateAfterRun,
        stdoutTail: (run.stdout ?? "").trim().split(/\r?\n/).slice(-4).join("\n"),
        stderrTail: (run.stderr ?? "").trim().split(/\r?\n/).slice(-4).join("\n"),
      };
      results.push(result);
      const state = stateAfterRun;
      const change = classifyDesktopChange(before, state, fixture.hwnd);
      if (change === "fixture-stole" || change === "unexplained") {
        status = "safety-failed";
        reason = `${entry.product}/${entry.scenario} caused an unattributable desktop change (${change})`;
        break;
      }
      if (!state.isWindow || !state.visible) {
        status = "safety-failed";
        reason = `fixture window was not visible after ${entry.product}/${entry.scenario}`;
        break;
      }
    }
    safety.afterMeasurements = readState(fixture.hwnd);
    if (results.some((result) => result.exitCode !== 0) && status === "measured") {
      status = "measured-with-failures";
    }
    return status === "measured" ? 0 : 1;
  } finally {
    const terminated = await terminateFixture(child);
    await new Promise((resolveWait) => setTimeout(resolveWait, 200));
    let finalState = null;
    try {
      finalState = readState(fixture?.hwnd ?? 0);
    } catch {
      finalState = null;
    }
    safety.afterClose = finalState;
    const closeChange = finalState
      ? classifyDesktopChange(before, finalState, fixture?.hwnd ?? 0)
      : "unavailable";
    if (!terminated) {
      status = "unsupported";
      reason = "fixture process could not be terminated";
    } else if (finalState?.isWindow) {
      status = "unsupported";
      reason = "fixture window still exists after termination";
    } else if (closeChange === "fixture-stole" || closeChange === "unexplained") {
      status = "safety-failed";
      reason = `closing the fixture caused an unattributable desktop change (${closeChange})`;
    }
    await rm(readyPath, { force: true });
    process.stdout.write(
      `${JSON.stringify(
        {
          status,
          reason,
          fixture,
          safety,
          results,
          terminated,
          windowAfterClose: finalState?.isWindow ?? null,
        },
        null,
        2,
      )}\n`,
    );
    process.exitCode = status === "measured" ? 0 : status === "safe" ? 0 : status === "measured-with-failures" ? 1 : 2;
  }
}

main().then((code) => {
  if (process.exitCode === undefined) process.exitCode = typeof code === "number" ? code : 4;
}).catch((error) => {
  process.stderr.write(`desktop fixture error: ${error?.stack ?? error}\n`);
  process.exitCode = 4;
});
