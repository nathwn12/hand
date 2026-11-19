
import { buildConfiguration } from "./build.mjs";
import { canonicalHash, stableStringify } from "./fingerprint.mjs";
import { SCENARIOS } from "./scenarios.mjs";
import { percentile, roundMs } from "./stats.mjs";

export const EXPECTED_PRODUCTS = {
  "warm-ping": ["hand", "gladius", "puppet"],
  "browser-snapshot": ["hand", "gladius"],
  "desktop-enumerate": ["hand", "puppet"],
  "desktop-capture": ["hand", "puppet"],
};

const HASH_RE = /^sha256:[0-9a-f]{64}$/;

export function isFingerprint(value) {
  return typeof value === "string" && HASH_RE.test(value);
}

export function describeReceipt({ file, sha256, receipt, currentBinarySha, releaseEvidence }) {
  const executable = receipt.product?.executable ?? null;
  const attempts = Array.isArray(receipt.samples?.measured?.attempts)
    ? receipt.samples.measured.attempts
    : [];
  const warmupAttempts = Array.isArray(receipt.samples?.warmup?.attempts)
    ? receipt.samples.warmup.attempts
    : [];
  const fixtureFiles = Array.isArray(receipt.fingerprint?.fixture?.files)
    ? receipt.fingerprint.fixture.files
    : [];
  const fixturePinEntries = receipt.fingerprint?.fixture?.pin?.entries ?? null;
  const recomputedFixtureHash = fixtureFiles.length
    ? canonicalHash(
        fixtureFiles.map((file) => ({ name: file.name, bytes: file.bytes, sha256: file.sha256 })),
      )
    : null;
  const durations = attempts
    .map((attempt) => attempt.elapsedMs)
    .filter((value) => typeof value === "number" && Number.isFinite(value));
  return {
    file,
    sha256,
    product: receipt.product?.id ?? "unknown",
    scenario: receipt.scenario?.id ?? "unknown",
    status: receipt.verdict?.status ?? "unknown",
    testOnly: receipt.testOnly === true,
    p50: receipt.stats?.p50Ms ?? null,
    p95: receipt.stats?.p95Ms ?? null,
    max: receipt.stats?.maxMs ?? null,
    failed: receipt.stats?.failed ?? 0,
    attempts: receipt.stats?.attempted ?? 0,
    observed: receipt.stats?.observed ?? null,
    recordedAttempts: attempts.length,
    recordedFailures: attempts.filter((attempt) => attempt.ok !== true).length,
    recordedObserved: durations.length,
    declaredMeasuredFailures: receipt.samples?.measured?.failures ?? null,
    recomputedP50Ms: durations.length ? roundMs(percentile(durations, 50)) : null,
    recomputedP95Ms: durations.length ? roundMs(percentile(durations, 95)) : null,
    recomputedMaxMs: durations.length ? roundMs(Math.max(...durations)) : null,
    targetP95Ms: receipt.verdict?.targetP95Ms ?? null,
    warmupFailures: receipt.samples?.warmup?.failures ?? null,
    recordedWarmups: warmupAttempts.length,
    recordedWarmupFailures: warmupAttempts.filter((attempt) => attempt.ok !== true).length,
    machineHash: isFingerprint(receipt.fingerprint?.machine?.hash) ? receipt.fingerprint.machine.hash : null,
    fixtureHash: isFingerprint(receipt.fingerprint?.fixture?.hash) ? receipt.fingerprint.fixture.hash : null,
    fixtureFiles,
    fixturePinEntries,
    recomputedFixtureHash,
    browserHash: isFingerprint(receipt.fingerprint?.browser?.sha256) ? receipt.fingerprint.browser.sha256 : null,
    browserLaunchVerified: receipt.fingerprint?.browser?.launch?.verified === true,
    browserLaunchPath: receipt.fingerprint?.browser?.launch?.path ?? null,
    browserLaunchSha256: isFingerprint(receipt.fingerprint?.browser?.launch?.sha256)
      ? receipt.fingerprint.browser.launch.sha256
      : null,
    targetHash: isFingerprint(receipt.fingerprint?.desktopTarget?.hash)
      ? receipt.fingerprint.desktopTarget.hash
      : null,
    binaryPath: executable?.path ?? null,
    binarySha256: isFingerprint(executable?.sha256) ? executable.sha256 : null,
    binaryStillMatches:
      isFingerprint(executable?.sha256) && typeof currentBinarySha === "string"
        ? executable.sha256 === currentBinarySha
        : null,
    buildConfiguration: buildConfiguration(receipt.product?.version),
    releaseEvidenceVerified: releaseEvidence?.verified === true,
    releaseEvidenceKind: releaseEvidence?.kind ?? null,
    releaseEvidenceReference: releaseEvidence?.reference ?? null,
    releaseEvidenceReason: releaseEvidence?.reason ?? null,
    claimedReleaseIdentityVerified: receipt.product?.releaseIdentity?.verified === true,
    claimedReleaseIdentityBasis: receipt.product?.releaseIdentity?.basis ?? null,
    paramsHash: isFingerprint(receipt.scenario?.paramsHash) ? receipt.scenario.paramsHash : null,
    protocol: receipt.protocol ?? null,
    protocolKey: receipt.protocol ? stableStringify(receipt.protocol) : null,
    version: receipt.product?.version?.stdout ?? null,
    reasons: receipt.verdict?.reasons ?? [],
    warnings: receipt.verdict?.warnings ?? [],
  };
}

function protocolProblem(protocol) {
  if (!protocol || typeof protocol !== "object") return "protocol block missing";
  if (protocol.sequential !== true) return "protocol does not prove sequential requests";
  if (!Number.isInteger(protocol.warmup) || protocol.warmup < 10) {
    return `protocol has ${JSON.stringify(protocol.warmup)} warmups; required at least 10`;
  }
  if (!Number.isInteger(protocol.measured) || protocol.measured < 100) {
    return `protocol has ${JSON.stringify(protocol.measured)} measured requests; required at least 100`;
  }
  if (!Number.isFinite(protocol.timeoutMs) || protocol.timeoutMs <= 0) {
    return "protocol timeout is missing";
  }
  return null;
}

function pushUnique(list, value) {
  if (!list.includes(value)) list.push(value);
}

export function buildComparison(entries) {
  const rows = entries.map(describeReceipt);
  const scenarioIds = new Set([...Object.keys(EXPECTED_PRODUCTS), ...rows.map((row) => row.scenario)]);
  const groups = [];

  for (const scenario of scenarioIds) {
    const members = rows.filter((row) => row.scenario === scenario);
    const differences = [];
    const measured = members.filter((row) => row.status === "pass" || row.status === "fail");
    const expected = EXPECTED_PRODUCTS[scenario] ?? [];

    const missingProducts = expected.filter((product) => !members.some((member) => member.product === product));
    if (missingProducts.length) pushUnique(differences, `missing product receipt(s): ${missingProducts.join(", ")}`);
    if (measured.length === 0) pushUnique(differences, "no measured receipt in group");

    for (const row of measured) {
      const label = row.product;
      if (!row.machineHash) pushUnique(differences, `${label}: machine fingerprint is missing or malformed`);
      if (!row.binarySha256) pushUnique(differences, `${label}: binary SHA-256 is missing or malformed`);
      if (row.binaryStillMatches !== true) {
        pushUnique(
          differences,
          row.binaryStillMatches === false
            ? `${label}: binary at its recorded path changed after the receipt`
            : `${label}: binary at its recorded path could not be re-hashed`,
        );
      }
      if (row.releaseEvidenceVerified !== true) {
        pushUnique(
          differences,
          `${label}: release identity is unverified${row.releaseEvidenceReason ? ` (${row.releaseEvidenceReason})` : ""}`,
        );
      }
      if (row.observed !== row.recordedObserved) {
        pushUnique(
          differences,
          `${label}: stats observe ${JSON.stringify(row.observed)} durations but the receipt records ${row.recordedObserved}`,
        );
      }
      if (row.recordedWarmups < 10) {
        pushUnique(differences, `${label}: ${row.recordedWarmups} warmup attempt records; required at least 10`);
      }
      if (row.recordedWarmups !== row.protocol?.warmup) {
        pushUnique(
          differences,
          `${label}: protocol declares ${JSON.stringify(row.protocol?.warmup)} warmups but the receipt records ${row.recordedWarmups}`,
        );
      }
      if (row.warmupFailures !== row.recordedWarmupFailures) {
        pushUnique(
          differences,
          `${label}: warmup summary claims ${JSON.stringify(row.warmupFailures)} failures but the records show ${row.recordedWarmupFailures}`,
        );
      }
      if (row.declaredMeasuredFailures !== row.recordedFailures) {
        pushUnique(
          differences,
          `${label}: measured failure summary claims ${JSON.stringify(row.declaredMeasuredFailures)} but the records show ${row.recordedFailures}`,
        );
      }
      const target = SCENARIOS[scenario]?.specTargetP95Ms;
      if (target === undefined) {
        pushUnique(differences, `${label}: no spec p95 target for scenario ${scenario}`);
      } else if (row.targetP95Ms !== target) {
        pushUnique(
          differences,
          `${label}: receipt declares p95 target ${JSON.stringify(row.targetP95Ms)}; spec requires ${target}`,
        );
      }
      if (target !== undefined) {
        if (!Number.isFinite(row.p95)) {
          pushUnique(differences, `${label}: p95 is missing or malformed`);
        } else if (row.p95 > target) {
          pushUnique(differences, `${label}: p95 ${row.p95} ms exceeds the ${target} ms spec target`);
        }
      }
      if (!Number.isInteger(row.attempts) || row.attempts < 100) {
        pushUnique(differences, `${label}: ${JSON.stringify(row.attempts)} measured attempts recorded; required at least 100`);
      }
      if (row.recordedAttempts !== row.attempts) {
        pushUnique(
          differences,
          `${label}: stats claim ${row.attempts} attempts but the receipt contains ${row.recordedAttempts}`,
        );
      }
      if (row.recordedAttempts < 100) {
        pushUnique(differences, `${label}: ${row.recordedAttempts} measured attempt records; required at least 100`);
      }
      if (row.recordedFailures !== row.failed) {
        pushUnique(
          differences,
          `${label}: stats claim ${row.failed} failures but the receipt records ${row.recordedFailures}`,
        );
      }
      if (row.p95 !== row.recomputedP95Ms) {
        pushUnique(
          differences,
          `${label}: stats p95 ${JSON.stringify(row.p95)} does not match the attempt records ${JSON.stringify(row.recomputedP95Ms)}`,
        );
      }
      if (row.p50 !== row.recomputedP50Ms) {
        pushUnique(
          differences,
          `${label}: stats p50 ${JSON.stringify(row.p50)} does not match the attempt records ${JSON.stringify(row.recomputedP50Ms)}`,
        );
      }
      if (row.max !== row.recomputedMaxMs) {
        pushUnique(
          differences,
          `${label}: stats max ${JSON.stringify(row.max)} does not match the attempt records ${JSON.stringify(row.recomputedMaxMs)}`,
        );
      }
      if (row.status === "pass" && row.failed !== 0) {
        pushUnique(differences, `${label}: pass receipt reports ${row.failed} failed attempt(s)`);
      }
      const protocolIssue = protocolProblem(row.protocol);
      if (protocolIssue) pushUnique(differences, `${label}: ${protocolIssue}`);
      if (scenario !== "warm-ping" && !row.paramsHash) {
        pushUnique(differences, `${label}: scenario params hash is missing or malformed`);
      }
      if (row.testOnly) pushUnique(differences, `${label}: test-only receipt cannot satisfy the gate`);
      if (scenario === "browser-snapshot") {
        if (!row.fixtureHash) pushUnique(differences, `${label}: fixture fingerprint is missing or malformed`);
        if (row.recomputedFixtureHash !== row.fixtureHash) {
          pushUnique(
            differences,
            `${label}: fixture hash does not match the recorded fixture files (${JSON.stringify(row.recomputedFixtureHash)})`,
          );
        }
        const files = Array.isArray(row.fixtureFiles) ? row.fixtureFiles : [];
        const pins = row.fixturePinEntries;
        if (!pins || typeof pins !== "object" || files.length === 0) {
          pushUnique(differences, `${label}: fixture pin entries or file records are missing`);
        } else {
          const fileNames = files.map((file) => file.name).sort();
          const pinNames = Object.keys(pins).sort();
          if (JSON.stringify(fileNames) !== JSON.stringify(pinNames)) {
            pushUnique(
              differences,
              `${label}: fixture pin entries ${JSON.stringify(pinNames)} do not match files ${JSON.stringify(fileNames)}`,
            );
          }
          for (const file of files) {
            if (pins[file.name] !== file.sha256) {
              pushUnique(
                differences,
                `${label}: fixture pin for ${JSON.stringify(file.name)} is ${JSON.stringify(pins[file.name])}, file record is ${JSON.stringify(file.sha256)}`,
              );
            }
          }
        }
        if (!row.browserHash) pushUnique(differences, `${label}: browser fingerprint is missing or malformed`);
        if (row.browserLaunchVerified !== true) {
          pushUnique(differences, `${label}: launched browser is not proven to be the fingerprinted binary`);
        }
        if (row.browserLaunchSha256 !== row.browserHash) {
          pushUnique(
            differences,
            `${label}: launched browser SHA-256 ${JSON.stringify(row.browserLaunchSha256)} does not match the browser fingerprint ${JSON.stringify(row.browserHash)}`,
          );
        }
      }
      if (scenario === "desktop-enumerate" || scenario === "desktop-capture") {
        if (!row.targetHash) pushUnique(differences, `${label}: desktop target fingerprint is missing or malformed`);
      }
    }

    for (const field of ["machineHash", "fixtureHash", "browserHash", "targetHash"]) {
      const values = [...new Set(measured.map((row) => row[field]).filter(Boolean))];
      if (values.length > 1) pushUnique(differences, `${field} differs: ${values.join(", ")}`);
    }
    const protocols = [...new Set(measured.map((row) => row.protocolKey).filter(Boolean))];
    if (protocols.length > 1) pushUnique(differences, "protocol differs across receipts");
    const paramHashes = [...new Set(measured.map((row) => row.paramsHash))];
    if (paramHashes.length > 1) pushUnique(differences, "scenario params differ across receipts");

    for (const product of new Set(measured.map((row) => row.product))) {
      const binaryHashes = [
        ...new Set(measured.filter((row) => row.product === product && row.binarySha256).map((row) => row.binarySha256)),
      ];
      if (binaryHashes.length > 1) pushUnique(differences, `${product} binary differs across receipts`);
    }

    const comparable = measured.length > 0 && differences.length === 0;
    groups.push({
      scenario,
      products: members.map((row) => row.product),
      comparable,
      differences,
      allPass: comparable && members.every((row) => row.status === "pass" && !row.testOnly),
      members,
    });
  }

  const gate = {
    pass: groups.every((group) => group.allPass),
    failures: groups
      .filter((group) => !group.allPass)
      .map((group) => ({ scenario: group.scenario, differences: group.differences })),
  };

  return {
    schema: "hand-mcp.benchmark.comparison/v1",
    generatedAtUtc: new Date().toISOString(),
    receipts: rows,
    groups,
    gate,
    notes: [
      "Rows are receipts, not reruns; a missing product is named, not green.",
      "A release gate needs release binaries, re-verifiable hashes, identical protocol and params, and no test-only receipts.",
      "Warmup failures are reported and do not fail the gate; every measured failure does.",
    ],
  };
}
