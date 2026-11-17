// Canonical JSON helpers shared by capture, comparison and tests.
//
// A catalogue hash has to survive reformatting by an editor or a different
// serializer, so hashes are taken over a canonical form (object keys sorted at
// every level, no insignificant whitespace) and never over file bytes alone.
// File bytes are still recorded separately, because "the server sent these
// exact bytes" is a different fact from "the schema is this value".

import { createHash } from "node:crypto";

/** Deterministic JSON: object keys sorted recursively, arrays keep order. */
export function stableStringify(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) {
    return `[${value.map((v) => stableStringify(v ?? null)).join(",")}]`;
  }
  const keys = Object.keys(value).sort();
  const parts = [];
  for (const key of keys) {
    if (value[key] === undefined) continue;
    parts.push(`${JSON.stringify(key)}:${stableStringify(value[key])}`);
  }
  return `{${parts.join(",")}}`;
}

export function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** Canonical hash of any JSON value. */
export function canonicalHash(value) {
  return `sha256:${sha256(stableStringify(value))}`;
}

/** Pretty printed, key-sorted, so diffs are stable across captures. */
export function prettyCanonical(value) {
  return `${JSON.stringify(JSON.parse(stableStringify(value)), null, 2)}\n`;
}

/** Deep structural equality on parsed JSON values. */
export function deepEqual(a, b) {
  return stableStringify(a) === stableStringify(b);
}

/**
 * Recursively collect the paths at which two JSON values differ, so a failure
 * names the exact schema location instead of dumping two large objects.
 * `limit` keeps a single broken tool from producing thousands of lines.
 */
export function diffPaths(a, b, { path = "", out = [], limit = 40 } = {}) {
  if (out.length >= limit) return out;
  const ta = kind(a);
  const tb = kind(b);
  if (ta !== tb) {
    out.push({ path: path || "$", left: summarize(a), right: summarize(b), kind: "type" });
    return out;
  }
  if (ta === "object") {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const key of [...keys].sort()) {
      if (out.length >= limit) break;
      const next = path ? `${path}.${key}` : key;
      if (!(key in a)) out.push({ path: next, left: "<absent>", right: summarize(b[key]), kind: "missing-left" });
      else if (!(key in b)) out.push({ path: next, left: summarize(a[key]), right: "<absent>", kind: "missing-right" });
      else diffPaths(a[key], b[key], { path: next, out, limit });
    }
    return out;
  }
  if (ta === "array") {
    if (a.length !== b.length) {
      out.push({ path: path || "$", left: `array(${a.length})`, right: `array(${b.length})`, kind: "length" });
      return out;
    }
    for (let i = 0; i < a.length; i += 1) {
      if (out.length >= limit) break;
      diffPaths(a[i], b[i], { path: `${path}[${i}]`, out, limit });
    }
    return out;
  }
  if (a !== b) out.push({ path: path || "$", left: summarize(a), right: summarize(b), kind: "value" });
  return out;
}

function kind(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  return typeof value;
}

function summarize(value) {
  const text = stableStringify(value);
  return text.length > 160 ? `${text.slice(0, 157)}...` : text;
}
