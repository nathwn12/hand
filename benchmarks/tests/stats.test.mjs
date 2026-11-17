// Percentile and verdict tests. These prove the arithmetic the receipt claims;
// they do not measure any product.

import test from "node:test";
import assert from "node:assert/strict";

import { percentile, summarize } from "../scripts/lib/stats.mjs";

function attempt(index, elapsedMs, ok = true) {
  return { index, elapsedMs, ok, error: ok ? undefined : "failed" };
}

test("nearest-rank percentiles use the sorted attempt list", () => {
  const values = Array.from({ length: 100 }, (_, index) => index + 1);
  assert.equal(percentile(values, 50), 50);
  assert.equal(percentile(values, 95), 95);
  assert.equal(percentile(values, 100), 100);
  assert.equal(percentile(Array.from({ length: 20 }, (_, index) => index + 1), 95), 19);
  assert.equal(percentile([7], 95), 7);
  assert.equal(percentile([], 95), null);
  assert.throws(() => percentile([1], 0), /percentile must be/);
});

test("p95 is computed over all attempts, failures included", () => {
  const attempts = [
    ...Array.from({ length: 94 }, (_, index) => attempt(index + 1, 10)),
    ...Array.from({ length: 6 }, (_, index) => attempt(95 + index, 1000, false)),
  ];
  const { stats, verdict } = summarize(attempts, { targetP95Ms: 50 });
  assert.equal(stats.attempted, 100);
  assert.equal(stats.observed, 100);
  assert.equal(stats.percentileBase.observed, 100);
  assert.equal(stats.p50Ms, 10);
  assert.equal(stats.p95Ms, 1000);
  assert.equal(stats.maxMs, 1000);
  assert.equal(stats.failed, 6);
  assert.equal(verdict.status, "fail");
  assert.ok(verdict.reasons.some((reason) => reason.includes("failed")));
  assert.ok(verdict.reasons.some((reason) => reason.includes("exceeds target")));
});

test("a fast failure still fails the scenario while p95 stays low", () => {
  const attempts = Array.from({ length: 100 }, (_, index) => attempt(index + 1, 10, index !== 49));
  const { stats, verdict } = summarize(attempts, { targetP95Ms: 50 });
  assert.equal(stats.p95Ms, 10);
  assert.equal(stats.failed, 1);
  assert.equal(verdict.status, "fail");
  assert.equal(verdict.releaseBlocked, true);
});

test("p95 equal to the target passes when nothing failed", () => {
  const attempts = Array.from({ length: 100 }, (_, index) => attempt(index + 1, 50));
  const { verdict } = summarize(attempts, { targetP95Ms: 50 });
  assert.equal(verdict.status, "pass");
  assert.equal(verdict.releaseBlocked, false);
});

test("an attempt with no duration is a failure, not a fast sample", () => {
  const attempts = [
    ...Array.from({ length: 99 }, (_, index) => attempt(index + 1, 10)),
    { index: 100, elapsedMs: null, ok: false, dispatched: false, error: "server already exited" },
  ];
  const { stats, verdict } = summarize(attempts, { targetP95Ms: 50 });
  assert.equal(stats.attempted, 100);
  assert.equal(stats.observed, 99);
  assert.equal(stats.percentileBase.observed, 99);
  assert.equal(stats.percentileBase.attempted, 100);
  assert.equal(stats.failed, 1);
  assert.equal(verdict.status, "fail");
  assert.ok(verdict.reasons.some((reason) => reason.includes("no duration")));
});
