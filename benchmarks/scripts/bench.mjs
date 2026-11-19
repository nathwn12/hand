#!/usr/bin/env node

import os from "node:os";
import { join, resolve } from "node:path";

import { parseArgs } from "./lib/cli.mjs";
import { releaseIdentityFor } from "./lib/build.mjs";
import { loadFixture, startFixtureServer } from "./lib/fixture.mjs";
import {
  browserFingerprint,
  canonicalHash,
  cpuBusyPercent,
  fingerprintExecutable,
  hashFile,
  machineFingerprint,
  normalizeDesktopTarget,
  sampleCpu,
} from "./lib/fingerprint.mjs";
import { resolveProduct } from "./lib/products.mjs";
import { startInputProbe } from "./lib/input-probe.mjs";
import { EXIT_CODES, finalizeReceipt, formatSummary, writeReceipt } from "./lib/receipt.mjs";
import { PRODUCT_DEFINITIONS, SCENARIOS, resolveScenarioPlan, extractPayload } from "./lib/scenarios.mjs";
import { summarize } from "./lib/stats.mjs";
import { BenchClient } from "./lib/stdio.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const DEFAULT_OUT = join(REPO_ROOT, "benchmarks", "receipts");
const FIXTURE_DIR = join(REPO_ROOT, "fixtures");
const INPUT_PROBE = join(REPO_ROOT, "benchmarks", "fixtures", "input-probe.ps1");
const DEFAULT_WARMUP = 10;
const DEFAULT_MEASURED = 100;
const DEFAULT_TIMEOUT_MS = 60_000;

const HELP = `hand benchmark driver (opt-in; no release claim without a passing receipt)

Usage:
  node benchmarks/scripts/bench.mjs --product <hand|gladius|puppet> --scenario <id> [options]

Scenarios and spec p95 targets:
  warm-ping           50 ms   MCP ping, warm no-op round trip
  browser-snapshot   250 ms   snapshot of the loopback 1280x720 fixture
  desktop-enumerate  250 ms   window enumeration including the controlled fixture
  desktop-capture    500 ms   native 1280x720 fixture window capture

Options:
  --warmup <n>                 default ${DEFAULT_WARMUP}; live runs require >=10
  --measured <n>               default ${DEFAULT_MEASURED}; live runs require >=100
  --timeout-ms <n>             per-request timeout, default ${DEFAULT_TIMEOUT_MS}
  --command <path>             override the product executable
  --arg <value>                repeatable child argument (test fixture uses --arg <fake server>)
  --browser-exe <path>         explicit chrome-headless-shell; personal browsers are refused
  --desktop-target <json>      controlled window matcher, e.g. {"titleContains":"hand-bench-fixture"}
  --desktop-title-contains <s> shorthand for the same matcher
  --out <dir>                  receipt directory, default <repo>/benchmarks/receipts
  --json                       print the receipt JSON on stdout
  --no-display-probe           skip the Windows display probe (receipt records that)
  --test-fixture               drive the deterministic fake stdio fixture; marked test-only
  --list                       print scenarios, tools and candidate paths
  --help

Exit codes: 0 pass, 1 fail/blocked, 2 unsupported, 3 usage, 4 driver error.
`;

async function main() {
  const { options, errors } = parseArgs(process.argv.slice(2));
  if (errors.length) return usage(errors.join("\n"));
  if (options.help) {
    process.stdout.write(HELP);
    return 0;
  }
  if (options.list) {
    printList();
    return 0;
  }

  const productId = options.product;
  const scenarioId = options.scenario;
  if (!productId) return usage("--product is required");
  if (!scenarioId) return usage("--scenario is required");
  const definition = PRODUCT_DEFINITIONS[productId];
  const scenario = SCENARIOS[scenarioId];
  if (!definition) return usage(`unknown product: ${productId} (expected ${Object.keys(PRODUCT_DEFINITIONS).join(", ")})`);
  if (!scenario) return usage(`unknown scenario: ${scenarioId} (expected ${Object.keys(SCENARIOS).join(", ")})`);

  const testFixture = Boolean(options.testFixture);
  const warmup = options.warmup ?? DEFAULT_WARMUP;
  const measured = options.measured ?? DEFAULT_MEASURED;
  if (!testFixture && (warmup < 10 || measured < 100)) {
    return usage(
      `docs/SPEC.md requires >=10 warmups and >=100 measured requests; got ${warmup}/${measured}. Use --test-fixture only for driver tests.`,
    );
  }
  if (testFixture && (warmup < 1 || measured < 1)) {
    return usage("a driver test still requires at least one warmup and one measured request");
  }
  if (testFixture && !options.command) {
    return usage("--test-fixture requires --command and --arg <fake MCP server script>");
  }

  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  if (!Number.isInteger(timeoutMs) || timeoutMs <= 0) return usage("--timeout-ms must be a positive integer");

  const fixturesDir = FIXTURE_DIR;
  const outDir = resolve(options.out ?? DEFAULT_OUT);

  let target = null;
  if (options.desktopTarget && options.desktopTitleContains) {
    return usage("use either --desktop-target or --desktop-title-contains, not both");
  }
  if (options.desktopTarget) {
    let parsed;
    try {
      parsed = JSON.parse(options.desktopTarget);
    } catch (error) {
      return usage(`--desktop-target is not valid JSON: ${error.message}`);
    }
    const normalized = normalizeDesktopTarget(parsed);
    if (!normalized.ok) return usage(normalized.reason);
    target = normalized.target;
  } else if (options.desktopTitleContains) {
    const normalized = normalizeDesktopTarget({ titleContains: options.desktopTitleContains });
    if (!normalized.ok) return usage(normalized.reason);
    target = normalized.target;
  }

  const scenarioMeta = {
    id: scenario.id,
    title: scenario.title,
    domain: scenario.domain,
    specTargetP95Ms: scenario.specTargetP95Ms,
  };
  const protocol = {
    warmup,
    measured,
    sequential: true,
    timeoutMs,
    framing: "newline-delimited JSON-RPC",
    latencyBasis: "stdin write to complete response frame; validation happens after the measurement",
  };

  const machine = await machineFingerprint({ displayProbeEnabled: !options.noDisplayProbe });
  const fingerprint = { machine, browser: null, fixture: null, desktopTarget: null };

  const finish = async (receipt, name) => {
    const finalized = finalizeReceipt({ testOnly: testFixture, ...receipt });
    const written = await writeReceipt({ dir: outDir, receipt: finalized, name });
    if (options.json) {
      process.stdout.write(`${JSON.stringify(finalized, null, 2)}\n`);
    }
    process.stderr.write(`${formatSummary(finalized)}\nreceipt: ${written.path}\nreceipt sha256: ${written.sha256}\n`);
    return EXIT_CODES[finalized.verdict.status] ?? EXIT_CODES.error;
  };

  const base = () => ({
    product: { id: productId, label: definition.label, resolved: false },
    scenario: scenarioMeta,
    protocol,
    fingerprint,
    samples: { warmup: { attempts: [], failures: 0 }, measured: { attempts: [], failures: 0 } },
    setup: null,
    coldStart: null,
    catalog: null,
    load: null,
    stats: null,
    phases: null,
    process: null,
  });

  const finishUnavailable = async (status, { stage, reason, tried, product, extra = {} }) => {
    const receipt = base();
    if (product) receipt.product = product;
    Object.assign(receipt, extra);
    receipt.verdict = {
      status,
      targetP95Ms: scenario.specTargetP95Ms,
      failures: 0,
      reasons: [reason],
      releaseBlocked: true,
    };
    receipt.reason = reason;
    receipt[status === "unsupported" ? "unsupported" : "blocked"] = { stage, reason, tried: tried ?? null };
    return finish(receipt);
  };

  if (scenario.domain !== "protocol" && !definition.domains.includes(scenario.domain)) {
    return finishUnavailable("unsupported", {
      stage: "scenario-domain",
      reason: `${definition.label} has no ${scenario.domain} capability in the pinned surface`,
    });
  }

  const resolved = await resolveProduct({
    productId,
    command: options.command,
    args: options.args,
    cwd: options.cwd ? resolve(options.cwd) : undefined,
    testFixture,
  });
  if (!resolved.ok) {
    if (resolved.usage) return usage(resolved.reason);
    return finishUnavailable("unsupported", {
      stage: "product-resolution",
      reason: resolved.reason,
      tried: resolved.tried,
      product: { id: productId, label: definition.label, resolved: false },
    });
  }

  const product = {
    ...resolved.product,
    resolved: true,
    executable: null,
    version: null,
    entrypoint: null,
  };
  const executableFingerprint = await fingerprintExecutable({
    command: product.command,
    runVersion: !product.testFixture,
  });
  if (!executableFingerprint.ok) {
    return finishUnavailable("blocked", {
      stage: "binary-fingerprint",
      reason: executableFingerprint.reason,
      product,
    });
  }
  product.executable = executableFingerprint.executable;
  product.version = executableFingerprint.version;
  product.releaseIdentity = releaseIdentityFor(product.version);
  if (product.testFixture) {
    const scriptPath = resolve(product.args[0]);
    const script = await hashFile(scriptPath);
    product.entrypoint = { path: scriptPath, bytes: script.bytes, sha256: script.sha256 };
  }

  let fixture = null;
  let fixtureServer = null;
  let origin = null;
  if (scenario.domain === "browser") {
    const browser = await browserFingerprint({ explicitPath: options.browserExe ? resolve(options.browserExe) : undefined });
    if (!browser.ok) {
      return finishUnavailable("blocked", {
        stage: "browser-fingerprint",
        reason: browser.reason,
        product,
      });
    }
    fingerprint.browser = browser.browser;
    fixture = await loadFixture(fixturesDir);
    if (!fixture.ok) {
      return finishUnavailable("blocked", {
        stage: "fixture-pin",
        reason: `fixture is not the pinned benchmark fixture: ${fixture.reasons.join("; ")}`,
        product,
      });
    }
    fingerprint.fixture = fixtureEvidence(fixture);
    fixtureServer = await startFixtureServer(fixture);
    origin = fixtureServer.origin;
  }

  try {
    const planResult = resolveScenarioPlan({ productId, scenarioId, origin, target });
    if (!planResult.ok) {
      if (planResult.unsupported) {
        return finishUnavailable("unsupported", {
          stage: "scenario-plan",
          reason: planResult.reason,
          product,
        });
      }
      return finishUnavailable("blocked", { stage: "scenario-plan", reason: planResult.reason, product });
    }
    const plan = planResult.plan;

    const requested = plan.request();
    const scenarioRecord = {
      ...scenarioMeta,
      tool: requested.tool ?? null,
      method: requested.method ?? null,
      params: requested.params ?? null,
      paramsHash: plan.paramsHash ?? canonicalHash(requested),
    };
    const loadBefore = sampleCpu();
    const freeMemoryBytesStart = os.freemem();
    const client = new BenchClient({
      command: product.command,
      args: product.args,
      cwd: product.cwd,
      timeoutMs,
      clientName: "hand-benchmark",
      env:
        scenario.domain === "browser"
          ? { GLADIUS_CHROMIUM: fingerprint.browser.path, GLADIUS_CDP_URL: "" }
          : {},
    });
    let closed = false;
    let inputProbe = null;

    const closeClient = async () => {
      if (closed) return;
      closed = true;
      await client.close();
    };

    try {
      const init = await client.initialize();
      const coldStart = {
        ok: init.ok,
        spawnToInitializeMs: round(client.msSinceSpawn()),
        initializeRoundTripMs: init.elapsedMs === null ? null : round(init.elapsedMs),
        error: init.ok ? null : init.error,
      };
      if (!init.ok) {
        return finishUnavailable("blocked", {
          stage: "initialize",
          reason: `server did not complete initialize: ${init.error}`,
          product,
          extra: { coldStart },
        });
      }

      const toolsList = await client.listTools();
      if (!toolsList.ok) {
        return finishUnavailable("blocked", {
          stage: "tools/list",
          reason: `server did not return a tool catalogue: ${toolsList.error}`,
          product,
          extra: { coldStart },
        });
      }
      const catalog = {
        toolCount: toolsList.names.length,
        sha256: toolsList.catalogSha256,
        names: toolsList.names,
      };
      const missing = plan.requiredTools.filter((name) => !toolsList.names.includes(name));
      if (missing.length) {
        return finishUnavailable("unsupported", {
          stage: "tools/list",
          reason: `scenario tool(s) missing from the ${product.label} surface: ${missing.join(", ")}`,
          product,
          extra: { coldStart, catalog },
        });
      }

      const ctx = { origin, target, product: productId };
      if (scenario.domain === "desktop") {
        inputProbe = await startInputProbe(INPUT_PROBE);
        ctx.inputProbe = inputProbe;
        ctx.inputProbeError = inputProbe ? null : "the driver's input probe did not start";
      }
      const setupSteps = [];
      let setupFailure = null;
      for (const step of plan.prepare) {
        const probeBefore = inputProbe ? await inputProbe.sample() : null;
        const call = await callOnce(client, { tool: step.tool, params: step.params }, timeoutMs);
        const probeAfter = inputProbe ? await inputProbe.sample() : null;
        let validation = null;
        if (call.ok) {
          ctx.probe = { before: probeBefore, after: probeAfter };
          validation = step.validate(call.payload, ctx);
        }
        const stepRecord = {
          name: step.name,
          tool: step.tool,
          params: step.params,
          ok: Boolean(call.ok && validation?.ok),
          elapsedMs: call.elapsedMs === null ? null : round(call.elapsedMs),
          error: call.ok ? (validation?.ok ? null : validation.reason) : call.error,
        };
        if (!stepRecord.ok) {
          stepRecord.stage = call.ok ? "validation" : "transport";
          setupSteps.push(stepRecord);
          setupFailure = stepRecord;
          break;
        }
        stepRecord.detail = validation.detail ?? null;
        setupSteps.push(stepRecord);
      }

      const setup = {
        ok: !setupFailure,
        steps: setupSteps,
        failure: setupFailure,
        controlledTarget: ctx.targetIdentity
          ? { identity: ctx.targetIdentity, hwnd: ctx.hwnd ?? null, hash: ctx.targetHash ?? null }
          : null,
      };
      if (ctx.targetIdentity) {
        fingerprint.desktopTarget = {
          matcher: target,
          identity: ctx.targetIdentity,
          hwnd: ctx.hwnd ?? null,
          hash: ctx.targetHash ?? null,
        };
      }

      if (!setupFailure && scenario.domain === "browser") {
        const launch = await verifyBrowserLaunch(client.stderrText, fingerprint.browser);
        fingerprint.browser = { ...fingerprint.browser, launch };
        if (!launch.ok) {
          return finishUnavailable("blocked", {
            stage: "browser-binding",
            reason: launch.reason,
            product,
            extra: { coldStart, catalog, setup },
          });
        }
      }

      if (setupFailure) {
        const receipt = base();
        Object.assign(receipt, {
          product,
          scenario: scenarioRecord,
          setup,
          catalog,
          coldStart,
          load: loadRecord(loadBefore, sampleCpu(), freeMemoryBytesStart, os.freemem(), client),
          process: processRecord(client),
          stats: null,
        });
        receipt.verdict = {
          status: "blocked",
          targetP95Ms: scenario.specTargetP95Ms,
          failures: 0,
          reasons: [`setup precondition failed: ${setupFailure.name}: ${setupFailure.error}`],
          releaseBlocked: true,
        };
        receipt.blocked = { stage: "setup", reason: setupFailure.error, step: setupFailure.name };
        return finish(receipt);
      }

      const warmupAttempts = [];
      for (let index = 1; index <= warmup; index += 1) {
        warmupAttempts.push(await measureOnce(client, plan, ctx, timeoutMs, index));
      }
      const warmupFailures = warmupAttempts.filter((attempt) => !attempt.ok).length;

      const measuredAttempts = [];
      for (let index = 1; index <= measured; index += 1) {
        measuredAttempts.push(await measureOnce(client, plan, ctx, timeoutMs, index));
      }
      await closeClient();
      const loadAfter = sampleCpu();
      const { stats, verdict } = summarize(measuredAttempts, { targetP95Ms: scenario.specTargetP95Ms });
      const reasons = [...verdict.reasons];
      if (client.protocolViolations.length) {
        verdict.status = "fail";
        verdict.releaseBlocked = true;
        reasons.push(`${client.protocolViolations.length} protocol violation(s) on stdout`);
      }
      verdict.reasons = reasons;
      verdict.failures = stats.failed;
      verdict.releaseIdentity = product.releaseIdentity;
      if (product.releaseIdentity.claimed !== "release") {
        verdict.releaseBlocked = true;
        reasons.push(
          `binary embeds ${product.releaseIdentity.claimed ?? "no build profile"}, not a release build; measurement may stand but release is blocked`,
        );
      }
      if (product.releaseIdentity.verified !== true) {
        verdict.releaseBlocked = true;
        verdict.releaseIdentityUnverified = true;
        verdict.warnings = [
          ...(verdict.warnings ?? []),
          `release identity is unverified (${product.releaseIdentity.basis}): ${product.releaseIdentity.reason}`,
        ];
      }
      if (warmupFailures > 0) {
        verdict.warmupAttempts = warmup;
        verdict.warmupFailures = warmupFailures;
        verdict.warnings = [
          ...(verdict.warnings ?? []),
          `${warmupFailures} of ${warmup} warmup attempts failed; warmups are outside the 100 measured samples and do not by themselves fail the gate, but the failures are listed in samples.warmup`,
        ];
      }

      const receipt = base();
      Object.assign(receipt, {
        product,
        scenario: scenarioRecord,
        setup,
        catalog,
        coldStart,
        samples: {
          warmup: { attempts: warmupAttempts, failures: warmupFailures },
          measured: { attempts: measuredAttempts, failures: stats.failed },
        },
        stats,
        phases: phasesFor({ coldStart, setup, scenarioRecord, warmup, measured, warmupFailures, stats }),
        load: loadRecord(loadBefore, loadAfter, freeMemoryBytesStart, os.freemem(), client),
        process: processRecord(client),
      });
      receipt.verdict = verdict;
      return finish(receipt);
    } finally {
      if (inputProbe) await inputProbe.close();
      await closeClient();
    }
  } finally {
    if (fixtureServer) {
      fixtureServer.server.closeAllConnections?.();
      await new Promise((resolveClose) => fixtureServer.server.close(() => resolveClose()));
    }
  }
}

async function measureOnce(client, plan, ctx, timeoutMs, index) {
  const request = plan.request();
  const probeBefore = ctx.inputProbe ? await ctx.inputProbe.sample() : null;
  const call = await callOnce(client, request, timeoutMs);
  const probeAfter = ctx.inputProbe ? await ctx.inputProbe.sample() : null;
  if (!call.ok) {
    return {
      index,
      ok: false,
      elapsedMs: call.elapsedMs === null ? null : round(call.elapsedMs),
      dispatched: call.dispatched,
      error: call.error,
    };
  }
  ctx.probe = { before: probeBefore, after: probeAfter };
  const validation = plan.validate(call.payload, ctx);
  if (!validation.ok) {
    return {
      index,
      ok: false,
      elapsedMs: round(call.elapsedMs),
      dispatched: true,
      outputIncomplete: !validation.unsafe,
      unsafeEvidence: Boolean(validation.unsafe),
      error: `${validation.unsafe ? "unsafe-evidence" : "incomplete-output"}: ${validation.reason}`,
    };
  }
  return {
    index,
    ok: true,
    elapsedMs: round(call.elapsedMs),
    dispatched: true,
    detail: validation.detail ?? null,
  };
}

async function callOnce(client, request, timeoutMs) {
  if (request.method) {
    const response = await client.request(request.method, request.params, { timeoutMs });
    if (!response.ok) return response;
    return { ok: true, elapsedMs: response.elapsedMs, payload: response.result, source: "method", dispatched: true };
  }
  const response = await client.request(
    "tools/call",
    { name: request.tool, arguments: request.params },
    { timeoutMs },
  );
  if (!response.ok) return response;
  const extracted = extractPayload(response.result);
  if (!extracted.ok) {
    return {
      ok: false,
      elapsedMs: response.elapsedMs,
      dispatched: true,
      error: `incomplete-output: ${extracted.reason}`,
    };
  }
  return {
    ok: true,
    elapsedMs: response.elapsedMs,
    payload: extracted.payload,
    source: extracted.source,
    dispatched: true,
  };
}

function fixtureEvidence(fixture) {
  return {
    ok: fixture.ok,
    hash: fixture.hash,
    pin: fixture.pin,
    files: fixture.files.map(({ name, path, bytes, sha256 }) => ({ name, path, bytes, sha256 })),
    manifest: fixture.manifest
      ? {
          version: fixture.manifest.version,
          manifestVersion: fixture.manifest.manifestVersion,
          page: fixture.manifest.benchmark?.page,
          viewport: fixture.manifest.frozen?.viewport,
          deviceScaleFactor: fixture.manifest.frozen?.deviceScaleFactor,
          selector: fixture.manifest.benchmark?.selector,
          declaredControls: fixture.manifest.benchmark?.count,
          firstLabel: fixture.manifest.benchmark?.firstLabel,
          lastLabel: fixture.manifest.benchmark?.lastLabel,
        }
      : null,
    labels: {
      count: fixture.controlLabels.length,
      unique: new Set(fixture.controlLabels).size,
      first: fixture.controlLabels[0] ?? null,
      last: fixture.controlLabels.at(-1) ?? null,
      exact: true,
    },
    controlCount: fixture.controlLabels.length,
    served: "in-memory bytes from this load; the HTTP server re-reads nothing",
  };
}

async function verifyBrowserLaunch(stderrText, expected) {
  const matches = [...(stderrText ?? "").matchAll(/\[gladius\] backend:\s+chromium\s+(.+?)\s+\(port\s+\d+\)/gi)];
  if (matches.length === 0) {
    return {
      ok: false,
      verified: false,
      reason:
        "the product did not log the launched chromium path; the fingerprinted browser cannot be tied to the browser that actually ran",
    };
  }
  const loggedPaths = [...new Set(matches.map((match) => resolve(match[1].trim())))];
  const unexpected = loggedPaths.filter((path) => path.toLowerCase() !== resolve(expected.path).toLowerCase());
  if (unexpected.length) {
    return {
      ok: false,
      verified: false,
      reason: `the product launched ${unexpected.join(", ")}, not the fingerprinted browser ${expected.path}`,
      loggedPaths,
    };
  }
  const loggedPath = loggedPaths[0];
  const actual = await hashFile(loggedPath);
  if (actual.sha256 !== expected.sha256) {
    return {
      ok: false,
      verified: false,
      reason: `the launched browser ${loggedPath} has ${actual.sha256}, not the fingerprinted ${expected.sha256}`,
      loggedPath,
      actualSha256: actual.sha256,
    };
  }
  return {
    ok: true,
    verified: true,
    path: loggedPath,
    sha256: actual.sha256,
    logLines: matches.map((match) => match[0]),
  };
}

function phasesFor({ coldStart, setup, scenarioRecord, warmup, measured, warmupFailures, stats }) {
  const phases = [];
  if (coldStart) {
    phases.push({
      name: "cold server start (spawn to initialize reply)",
      kind: "setup",
      elapsedMs: coldStart.spawnToInitializeMs,
      notThresholded: true,
    });
    phases.push({
      name: "initialize round trip",
      kind: "setup",
      elapsedMs: coldStart.initializeRoundTripMs,
      notThresholded: true,
    });
  }
  for (const step of setup.steps) {
    phases.push({
      name: `setup: ${step.name}`,
      kind: "setup",
      elapsedMs: step.elapsedMs,
      ok: step.ok,
      notThresholded: true,
      note:
        step.tool && step.tool.includes("navigate")
          ? "includes lazy cold browser launch; the thresholded browser number is the warm snapshot set"
          : null,
    });
  }
  phases.push({
    name: "warm samples",
    kind: "warmup",
    attempts: warmup,
    failures: warmupFailures,
    notThresholded: true,
  });
  phases.push({
    name: `measured ${scenarioRecord.title}`,
    kind: "measured",
    attempts: measured,
    failures: stats.failed,
    targetP95Ms: scenarioRecord.specTargetP95Ms,
    p50Ms: stats.p50Ms,
    p95Ms: stats.p95Ms,
    maxMs: stats.maxMs,
  });
  return phases;
}

function loadRecord(before, after, freeMemoryBytesStart, freeMemoryBytesEnd, client) {
  return {
    sampling:
      "whole-machine os.cpus() idle/total delta across the run; free memory at start/end; no per-request sampling",
    cpuBusyPercent: cpuBusyPercent(before, after),
    freeMemoryBytesStart,
    freeMemoryBytesEnd,
    sequential: true,
    maxConcurrentRequests: client.maxInFlight,
    unrelatedLoad:
      "not externally audited; concurrent work on this machine would appear in cpuBusyPercent. The driver issues one request at a time.",
  };
}

function processRecord(client) {
  const stderr = client.stderrReceipt();
  return {
    exited: client.exited,
    requestsCompleted: client.requestCount,
    protocolViolations: {
      count: client.protocolViolations.length,
      samples: client.protocolViolations.slice(0, 3),
    },
    unsolicitedFrames: client.unsolicitedFrames,
    notifications: client.notifications ?? 0,
    stderr,
  };
}

function round(value) {
  return Math.round(value * 1000) / 1000;
}

function usage(message) {
  process.stderr.write(`${message}\n\n${HELP}`);
  return EXIT_CODES.usage;
}

function printList() {
  const lines = ["scenarios:"];
  for (const scenario of Object.values(SCENARIOS)) {
    const tools = Object.values(PRODUCT_DEFINITIONS)
      .filter((product) => scenario.domain === "protocol" || product.domains.includes(scenario.domain))
      .map((product) => {
        if (scenario.domain === "protocol") return `${product.id}=ping`;
        if (scenario.domain === "browser") return `${product.id}=${product.tools.browserSnapshot}`;
        if (scenario.id === "desktop-enumerate") return `${product.id}=${product.tools.desktopListWindows}`;
        return `${product.id}=${product.tools.desktopCapture}`;
      });
    lines.push(
      `  ${scenario.id.padEnd(18)} p95 <= ${String(scenario.specTargetP95Ms).padStart(3)} ms  ${scenario.title}`,
    );
    lines.push(`  ${"".padEnd(18)} ${tools.join("  ")}`);
  }
  lines.push("", "products:");
  for (const product of Object.values(PRODUCT_DEFINITIONS)) {
    lines.push(`  ${product.id.padEnd(9)} ${product.domains.join("+")}`);
  }
  lines.push("", "binary candidates are printed by a run that cannot find its product; see --help.");
  process.stdout.write(`${lines.join("\n")}\n`);
}

export { main };

if (process.argv[1] && resolve(process.argv[1]) === resolve(import.meta.filename)) {
  main()
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error) => {
      process.stderr.write(`bench driver error: ${error?.stack ?? error}\n`);
      process.exitCode = EXIT_CODES.error;
    });
}
