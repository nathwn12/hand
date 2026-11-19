
import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export const RECEIPT_SCHEMA = "hand-mcp.benchmark.receipt/v1";
export const DRIVER_VERSION = "1.0.0";

export const EXIT_CODES = {
  pass: 0,
  fail: 1,
  blocked: 1,
  unsupported: 2,
  usage: 3,
  error: 4,
};

export function exitCodeFor(status) {
  return EXIT_CODES[status] ?? EXIT_CODES.error;
}

export function finalizeReceipt({ testOnly = false, ...receipt }) {
  return {
    schema: RECEIPT_SCHEMA,
    driverVersion: DRIVER_VERSION,
    generatedAtUtc: new Date().toISOString(),
    testOnly,
    evidence: testOnly
      ? "driver-test-only: deterministic fake MCP stdio fixture; this proves the driver, not live browser or desktop behavior"
      : "measured against the resolved product binary on this machine",
    ...receipt,
  };
}

function stamp(date = new Date()) {
  return date.toISOString().replace(/[-:]/g, "").replace(/\.(\d{3})Z$/, "$1Z");
}

function safeName(value) {
  return String(value).replace(/[^A-Za-z0-9._-]+/g, "-");
}

export async function writeReceipt({ dir, receipt, name }) {
  await mkdir(dir, { recursive: true });
  const filename = `${safeName(name ?? `${receipt.product?.id ?? "unknown"}-${receipt.scenario?.id ?? "unknown"}-${stamp()}`)}.json`;
  const path = join(dir, filename);
  const text = `${JSON.stringify(receipt, null, 2)}\n`;
  await writeFile(path, text, "utf8");
  const sha256 = `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
  await writeFile(`${path}.sha256`, `${sha256}  ${filename}\n`, "utf8");
  return { path, sha256 };
}

export function formatSummary(receipt) {
  const lines = [];
  const status = receipt.verdict?.status ?? receipt.status ?? "unknown";
  lines.push(`status: ${status}${receipt.testOnly ? "   [DRIVER TEST ONLY — not live evidence]" : ""}`);
  lines.push(`product: ${receipt.product?.id ?? "unknown"} (${receipt.product?.label ?? "unresolved"})`);
  lines.push(`scenario: ${receipt.scenario?.id ?? "unknown"}${receipt.scenario?.tool ? ` → ${receipt.scenario.tool}` : receipt.scenario?.method ? ` → ${receipt.scenario.method}` : ""}`);
  if (receipt.product?.executable?.path) {
    lines.push(`binary: ${receipt.product.executable.path} ${receipt.product.executable.sha256}`);
  }
  if (receipt.reason) lines.push(`reason: ${receipt.reason}`);
  if (receipt.verdict?.reasons?.length) {
    for (const reason of receipt.verdict.reasons) lines.push(`- ${reason}`);
  }
  if (receipt.verdict?.warnings?.length) {
    for (const warning of receipt.verdict.warnings) lines.push(`warning: ${warning}`);
  }
  if (receipt.stats) {
    lines.push(
      `measured: ${receipt.stats.attempted} attempts, ${receipt.stats.passed} passed, ${receipt.stats.failed} failed`,
    );
    lines.push(
      `percentiles: p50 ${fmt(receipt.stats.p50Ms)} ms, p95 ${fmt(receipt.stats.p95Ms)} ms, max ${fmt(receipt.stats.maxMs)} ms ` +
        `(nearest-rank, ${receipt.stats.percentileBase.observed}/${receipt.stats.percentileBase.attempted} with a duration)`,
    );
    if (receipt.verdict?.targetP95Ms !== null && receipt.verdict?.targetP95Ms !== undefined) {
      lines.push(`target: p95 ≤ ${receipt.verdict.targetP95Ms} ms`);
    }
    lines.push(`release blocked: ${receipt.verdict?.releaseBlocked === true ? "yes" : "no"}`);
  }
  if (receipt.samples?.warmup) {
    lines.push(`warmup: ${receipt.samples.warmup.attempts.length} attempts, ${receipt.samples.warmup.failures} failed`);
  }
  const failures = receipt.samples?.measured?.attempts?.filter((attempt) => !attempt.ok) ?? [];
  if (failures.length) {
    lines.push(`failures (${failures.length}):`);
    for (const failure of failures.slice(0, 5)) {
      lines.push(`- #${failure.index}: ${failure.error ?? "failed"}`);
    }
    if (failures.length > 5) lines.push(`- … and ${failures.length - 5} more, all present in the receipt`);
  }
  const fingerprints = receipt.fingerprint ?? {};
  if (fingerprints.machine?.hash) lines.push(`machine: ${fingerprints.machine.hash}`);
  if (fingerprints.fixture?.hash) lines.push(`fixture: ${fingerprints.fixture.hash}`);
  if (fingerprints.browser?.sha256) lines.push(`browser: ${fingerprints.browser.sha256}`);
  if (fingerprints.desktopTarget?.hash) lines.push(`desktop target: ${fingerprints.desktopTarget.hash}`);
  return lines.join("\n");
}

function fmt(value) {
  return value === null || value === undefined ? "n/a" : String(value);
}
