// Percentiles and verdicts for the MCP stdio benchmark.
//
// Rules that must not drift (docs/SPEC.md 309-336):
// - Every measured attempt is in `attempts`, including failures.
// - p95 is nearest-rank over every attempt that produced a duration. A failed
//   attempt's duration stays in the distribution; it is never omitted.
// - Any failed attempt fails the scenario, whatever the percentile says.
// - An attempt that could not be dispatched has no duration (null); it stays in
//   the attempt list, fails the verdict, and the receipt reports the smaller
//   percentile base rather than inventing a fast sample.

export function roundMs(value) {
  return Math.round(value * 1000) / 1000;
}

export function percentile(values, p) {
  if (!Array.isArray(values) || values.length === 0) return null;
  if (!(p > 0 && p <= 100)) throw new Error(`percentile must be in (0, 100], got ${p}`);
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil((p / 100) * sorted.length);
  const index = Math.min(Math.max(rank, 1), sorted.length) - 1;
  return sorted[index];
}

export function summarize(attempts, { targetP95Ms } = {}) {
  if (!Array.isArray(attempts)) throw new Error("attempts must be an array");
  const durations = attempts
    .map((attempt) => attempt.elapsedMs)
    .filter((value) => typeof value === "number" && Number.isFinite(value));
  const failures = attempts.filter((attempt) => attempt.ok !== true);
  const p50 = percentile(durations, 50);
  const p95 = percentile(durations, 95);
  const reasons = [];

  if (failures.length > 0) {
    reasons.push(`${failures.length} of ${attempts.length} measured attempts failed`);
  }
  if (durations.length < attempts.length) {
    reasons.push(
      `${attempts.length - durations.length} measured attempts produced no duration and are counted as failures, not samples`,
    );
  }
  if (p95 !== null && typeof targetP95Ms === "number" && p95 > targetP95Ms) {
    reasons.push(`p95 ${roundMs(p95)} ms exceeds target ${targetP95Ms} ms`);
  }
  if (p95 === null) {
    reasons.push("no measured attempt produced a duration");
  }

  const stats = {
    attempted: attempts.length,
    observed: durations.length,
    passed: attempts.length - failures.length,
    failed: failures.length,
    percentileMethod: "nearest-rank",
    percentileBase: { observed: durations.length, attempted: attempts.length },
    minMs: durations.length ? roundMs(Math.min(...durations)) : null,
    p50Ms: p50 === null ? null : roundMs(p50),
    p95Ms: p95 === null ? null : roundMs(p95),
    maxMs: durations.length ? roundMs(Math.max(...durations)) : null,
    meanMs: durations.length ? roundMs(durations.reduce((sum, value) => sum + value, 0) / durations.length) : null,
    sumMs: durations.length ? roundMs(durations.reduce((sum, value) => sum + value, 0)) : null,
  };

  const verdict = {
    status: reasons.length === 0 ? "pass" : "fail",
    targetP95Ms: typeof targetP95Ms === "number" ? targetP95Ms : null,
    failures: failures.length,
    reasons,
    releaseBlocked: reasons.length > 0,
  };

  return { stats, verdict };
}
