// Release-gate rules, exercised with synthetic receipts so every refusal is
// direct and independent of product binaries. These tests are the red/green
// contract for missing evidence, debug builds, mismatched protocol/params and
// unproven browser/target identities.

import test from "node:test";
import assert from "node:assert/strict";

import { buildComparison } from "../scripts/lib/compare-core.mjs";
import { canonicalHash } from "../scripts/lib/fingerprint.mjs";

function hash(char) {
  return `sha256:${char.repeat(64)}`;
}

function makeReceipt(product, scenario = "browser-snapshot", character = "a") {
  const target = { "warm-ping": 50, "browser-snapshot": 250, "desktop-enumerate": 250, "desktop-capture": 500 }[
    scenario
  ];
  const attempts = Array.from({ length: 100 }, (_, index) => ({
    index: index + 1,
    ok: true,
    elapsedMs: 1,
    dispatched: true,
  }));
  const warmups = Array.from({ length: 10 }, (_, index) => ({
    index: index + 1,
    ok: true,
    elapsedMs: 1,
    dispatched: true,
  }));
  const fixtureFiles = [
    { name: "index.html", bytes: 16374, sha256: hash("d") },
    { name: "actions.html", bytes: 8494, sha256: hash("d") },
    { name: "manifest.json", bytes: 2746, sha256: hash("d") },
  ];
  const fixtureHash = canonicalHash(
    fixtureFiles.map(({ name, bytes, sha256 }) => ({ name, bytes, sha256 })),
  );
  return {
    schema: "hand-mcp.benchmark.receipt/v1",
    testOnly: false,
    product: {
      id: product,
      executable: {
        path: `Q:\\bench\\${product}\\target\\release\\${product}.exe`,
        sha256: hash(character),
      },
      releaseIdentity: { verified: true, basis: "signed-test-fixture", claimed: "release" },
      version: { stdout: `${product} release` },
    },
    scenario: { id: scenario, paramsHash: hash("b") },
    protocol: {
      warmup: 10,
      measured: 100,
      sequential: true,
      timeoutMs: 60000,
      framing: "newline-delimited JSON-RPC",
      latencyBasis: "stdin write to complete response frame",
    },
    fingerprint: {
      machine: { hash: hash("c") },
      fixture: {
        hash: fixtureHash,
        files: fixtureFiles,
        pin: { entries: Object.fromEntries(fixtureFiles.map((file) => [file.name, file.sha256])) },
      },
      browser: {
        sha256: hash("e"),
        launch: {
          verified: true,
          path: "Q:\\browser\\chrome-headless-shell.exe",
          sha256: hash("e"),
        },
      },
      desktopTarget: { hash: hash("f") },
    },
    stats: { attempted: 100, observed: 100, failed: 0, p50Ms: 1, p95Ms: 1, maxMs: 1 },
    samples: {
      warmup: { attempts: warmups, failures: 0 },
      measured: { attempts, failures: 0 },
    },
    verdict: { status: "pass", targetP95Ms: target, reasons: [] },
  };
}

function compare(receipts, evidence = {}) {
  return buildComparison(
    receipts.map((receipt, index) => ({
      file: `receipt-${index}.json`,
      sha256: hash("1"),
      receipt,
      currentBinarySha: receipt.product.executable.sha256,
      releaseEvidence:
        evidence[receipt.product.id] ?? { verified: true, kind: "test-fixture", reference: `test-${index}` },
    })),
  );
}

function group(comparison, scenario) {
  return comparison.groups.find((entry) => entry.scenario === scenario);
}

function browserGroup(mutateHand = () => {}, mutateGladius = () => {}, evidence = {}) {
  const hand = makeReceipt("hand", "browser-snapshot", "a");
  const gladius = makeReceipt("gladius", "browser-snapshot", "2");
  mutateHand(hand);
  mutateGladius(gladius);
  return group(compare([hand, gladius], evidence), "browser-snapshot");
}

function assertFails(groupResult, pattern) {
  assert.equal(groupResult.comparable, false);
  assert.equal(groupResult.allPass, false);
  assert.ok(
    groupResult.differences.some((difference) => pattern.test(difference)),
    `expected ${pattern} in ${JSON.stringify(groupResult.differences)}`,
  );
}

test("a complete browser comparison with release binaries passes its group", () => {
  const result = browserGroup();
  assert.equal(result.comparable, true, result.differences.join("; "));
  assert.equal(result.allPass, true);
  assert.deepEqual(result.products.sort(), ["gladius", "hand"]);
});

test("the executable path is not evidence; the embedded profile is a claim and verified release evidence decides", () => {
  // A release-looking directory with no embedded identity claims nothing.
  const pathOnly = browserGroup((hand) => {
    hand.product.executable.path = "Q:\\bench\\hand\\target\\release\\hand.exe";
  });
  assert.equal(pathOnly.allPass, true, pathOnly.differences.join("; "));
  assert.equal(
    pathOnly.members.find((member) => member.product === "hand").buildConfiguration,
    null,
  );

  // The compiled-in profile travels with the bytes, wherever they are copied.
  const embedded = browserGroup((hand) => {
    hand.product.executable.path = "Q:\\bench\\arbitrary\\hand-copy.exe";
    hand.product.version.stdout = "hand 0.1.0 (abc1234; release)";
  });
  assert.equal(embedded.allPass, true, embedded.differences.join("; "));
  assert.equal(
    embedded.members.find((member) => member.product === "hand").buildConfiguration,
    "release",
  );

  const embeddedDebug = browserGroup((hand) => {
    hand.product.version.stdout = "hand 0.1.0 (abc1234; debug)";
  });
  assert.equal(
    embeddedDebug.members.find((member) => member.product === "hand").buildConfiguration,
    "debug",
  );

  const debugWithoutEvidence = browserGroup(
    (hand) => {
      hand.product.version.stdout = "hand 0.1.0 (abc1234; debug)";
    },
    () => {},
    { hand: { verified: false, reason: "no matching release build" } },
  );
  assertFails(debugWithoutEvidence, /release identity is unverified/);

  const unknownWithoutEvidence = browserGroup(
    (hand) => {
      hand.product.executable.path = "Q:\\bench\\hand\\hand.exe";
    },
    () => {},
    { hand: { verified: false, reason: "no matching release build" } },
  );
  assertFails(unknownWithoutEvidence, /release identity is unverified/);
});

test("missing machine, fixture, browser, launch-proof, binary or target evidence fails", () => {
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.machine.hash = null;
    }),
    /machine fingerprint is missing or malformed/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.fixture.hash = null;
    }),
    /fixture fingerprint is missing or malformed/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.browser.sha256 = null;
    }),
    /browser fingerprint is missing or malformed/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.browser.launch.verified = false;
    }),
    /launched browser is not proven/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.product.executable.sha256 = null;
    }),
    /binary SHA-256 is missing or malformed/,
  );

  const desktopHand = makeReceipt("hand", "desktop-capture", "a");
  const desktopPuppet = makeReceipt("puppet", "desktop-capture", "2");
  desktopHand.fingerprint.desktopTarget.hash = null;
  assertFails(
    group(compare([desktopHand, desktopPuppet]), "desktop-capture"),
    /desktop target fingerprint is missing or malformed/,
  );
});

test("a binary changed or unreadable after the receipt fails", () => {
  const hand = makeReceipt("hand", "browser-snapshot", "a");
  const gladius = makeReceipt("gladius", "browser-snapshot", "2");
  const changed = buildComparison([
    { file: "hand.json", sha256: hash("1"), receipt: hand, currentBinarySha: hash("9") },
    { file: "gladius.json", sha256: hash("1"), receipt: gladius, currentBinarySha: gladius.product.executable.sha256 },
  ]);
  assertFails(group(changed, "browser-snapshot"), /binary at its recorded path changed/);

  const unreadable = buildComparison([
    { file: "hand.json", sha256: hash("1"), receipt: hand, currentBinarySha: null },
    { file: "gladius.json", sha256: hash("1"), receipt: gladius, currentBinarySha: gladius.product.executable.sha256 },
  ]);
  assertFails(group(unreadable, "browser-snapshot"), /binary at its recorded path could not be re-hashed/);
});

test("protocol and scenario params must be identical and meet the sample floor", () => {
  const short = browserGroup((hand) => {
    hand.protocol.measured = 50;
  });
  assertFails(short, /protocol has 50 measured requests; required at least 100/);

  const differentParams = browserGroup((hand) => {
    hand.scenario.paramsHash = hash("9");
  });
  assertFails(differentParams, /scenario params differ/);

  const nonSequential = browserGroup((hand) => {
    hand.protocol.sequential = false;
  });
  assertFails(nonSequential, /sequential/);
});

test("a pass receipt with failures, too few attempts or a slow p95 is refused", () => {
  assertFails(
    browserGroup((hand) => {
      hand.stats.failed = 1;
    }),
    /pass receipt reports 1 failed/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.stats.p95Ms = 999;
    }),
    /exceeds the 250 ms spec target/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.stats.attempted = 50;
    }),
    /required at least 100/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.verdict.targetP95Ms = 500;
    }),
    /receipt declares p95 target 500; spec requires 250/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.samples.measured.attempts = [];
    }),
    /stats claim 100 attempts but the receipt contains 0/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.stats.p95Ms = 2;
    }),
    /stats p95 2 does not match the attempt records 1/,
  );
});

test("missing products and test-only receipts fail the gate", () => {
  const gladiusOnly = group(compare([makeReceipt("gladius", "browser-snapshot", "2")]), "browser-snapshot");
  assertFails(gladiusOnly, /missing product receipt\(s\): hand/);

  const testOnly = browserGroup((hand) => {
    hand.testOnly = true;
  });
  assertFails(testOnly, /test-only receipt cannot satisfy the gate/);
});

test("warmup records, p50/observed accounting, fixture pins and launch hash are verified", () => {
  assertFails(
    browserGroup((hand) => {
      hand.samples.warmup.attempts = [];
    }),
    /0 warmup attempt records/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.samples.warmup.attempts.pop();
    }),
    /protocol declares 10 warmups but the receipt records 9/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.stats.p50Ms = 2;
    }),
    /stats p50 2 does not match the attempt records 1/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.stats.observed = 99;
    }),
    /stats observe 99 durations but the receipt records 100/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.samples.measured.failures = 1;
    }),
    /measured failure summary claims 1 but the records show 0/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.fixture.pin.entries["index.html"] = hash("9");
    }),
    /fixture pin for "index.html"/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.fixture.hash = hash("9");
    }),
    /fixture hash does not match the recorded fixture files/,
  );
  assertFails(
    browserGroup((hand) => {
      hand.fingerprint.browser.launch.sha256 = hash("9");
    }),
    /launched browser SHA-256/,
  );
  assertFails(
    browserGroup(() => {}, () => {}, { hand: { verified: false, reason: "no evidence" } }),
    /release identity is unverified/,
  );
});

test("warmup failures are visible but do not fail the gate", () => {
  const result = browserGroup((hand) => {
    hand.samples.warmup.attempts[0].ok = false;
    hand.samples.warmup.failures = 1;
    hand.verdict.warnings = ["1 warmup attempt failed"];
  });
  assert.equal(result.allPass, true, result.differences.join("; "));
  assert.equal(result.members.find((member) => member.product === "hand").warmupFailures, 1);
});
