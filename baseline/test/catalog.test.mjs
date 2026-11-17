// Executable checks on the baseline itself. Run from the repository root:
//
//   node --test baseline/test/
//
// Two things are proven here. First, that the committed files are what the
// pinned revisions emitted and what the generator derives (an integrity check,
// so an edited baseline cannot pass silently). Second, that the comparison used
// as the compatibility gate is not vacuous: each test below breaks a copy of
// the baseline in one specific way and requires the comparator to reject it.
//
// Live tests re-spawn the pinned binaries named in `pins.json`; they skip (not
// pass) when the binary is absent, and are disabled entirely by
// HAND_BASELINE_SKIP_LIVE=1.

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { canonicalHash } from "../scripts/lib/canonical.mjs";
import { SPEC_REQUIRED, compareCatalogs, validateHandCatalog } from "../scripts/lib/catalog.mjs";
import { buildFromDisk } from "../scripts/build-hand-catalog.mjs";
import { captureCatalog } from "../scripts/capture-mcp-catalog.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pins = JSON.parse(readFileSync(resolve(ROOT, "pins.json"), "utf8"));
const gladiusTools = readTools(pins.sources.gladius.capture.tools);
const puppetTools = readTools(pins.sources.puppet.capture.tools);
const handCatalog = JSON.parse(readFileSync(resolve(ROOT, "hand-catalog.json"), "utf8"));
const clone = (value) => JSON.parse(JSON.stringify(value));

function readTools(relativePath) {
  return JSON.parse(readFileSync(resolve(ROOT, relativePath), "utf8")).tools;
}

function prefixed() {
  // Deep clone: a shallow spread would share the nested schema objects with the
  // fixtures, so a test that mutates one candidate would corrupt later tests.
  return {
    tools: [
      ...gladiusTools.map((tool) => ({ ...clone(tool), name: `browser_${tool.name}` })),
      ...puppetTools.map((tool) => ({ ...clone(tool), name: `desktop_${tool.name}` })),
    ],
  };
}

test("committed catalogues match the pinned canonical hashes", () => {
  for (const [key, source] of Object.entries(pins.sources)) {
    const captured = readTools(source.capture.tools);
    assert.equal(captured.length, source.expectedToolCount, `${key} tool count`);
    // The hash is over the tools/list *result*, not the array, so rebuild it.
    const result = JSON.parse(readFileSync(resolve(ROOT, source.capture.tools), "utf8"));
    assert.equal(canonicalHash(result), source.capture.expectCatalogCanonicalHash, `${key} canonical hash`);
  }
});

test("captured names are exactly the spec's 25 browser and 7 desktop tools", () => {
  assert.deepEqual(gladiusTools.map((tool) => tool.name).sort(), [...SPEC_REQUIRED.browser].sort());
  assert.deepEqual(puppetTools.map((tool) => tool.name).sort(), [...SPEC_REQUIRED.desktop].sort());
});

test("hand-catalog.json is exactly what the generator derives, not a hand edit", () => {
  // Whole-file equality, including the provenance block: a stale executable
  // hash, pin or tool body all show up here rather than only a changed schema.
  const derived = buildFromDisk({ root: ROOT }).hand;
  assert.deepEqual(handCatalog, derived);
  assert.equal(handCatalog.tools.length, 32);
  assert.deepEqual(handCatalog.counts, { total: 32, browser: 25, desktop: 7 });
});

test("hand catalogue validates against the required surface", () => {
  assert.deepEqual(validateHandCatalog(handCatalog), []);
});

test("prefixed baseline passes the hand comparison with no metadata drift", () => {
  const report = compareCatalogs({ baseline: handCatalog, candidate: prefixed(), profile: "hand" });
  assert.equal(report.ok, true, JSON.stringify(report.schemaMismatches, null, 2));
  assert.equal(report.counts.compared, 32);
  assert.deepEqual(report.missing, []);
  assert.deepEqual(report.extra, []);
  assert.deepEqual(report.metadataDrift, []);
});

test("legacy profile compares the raw catalogue with every field as contract", () => {
  const report = compareCatalogs({
    baseline: { tools: gladiusTools },
    candidate: { tools: clone(gladiusTools) },
    profile: "legacy",
  });
  assert.equal(report.ok, true);
  assert.equal(report.counts.compared, 25);
});

test("a relaxed input schema is rejected", () => {
  const candidate = prefixed();
  const click = candidate.tools.find((tool) => tool.name === "browser_click");
  delete click.inputSchema.required;
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  const mismatch = report.schemaMismatches.find((entry) => entry.tool === "browser_click");
  assert.ok(mismatch, "browser_click must be named");
  assert.equal(mismatch.kind, "inputSchema");
  assert.ok(mismatch.diffs.some((diff) => diff.path === "required" && diff.kind === "missing-right"));
});

test("a changed output schema is rejected", () => {
  const candidate = prefixed();
  const navigate = candidate.tools.find((tool) => tool.name === "browser_navigate");
  navigate.outputSchema.additionalProperties = true;
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  assert.equal(report.schemaMismatches[0].tool, "browser_navigate");
});

test("a missing required tool is rejected by name", () => {
  const candidate = prefixed();
  candidate.tools = candidate.tools.filter((tool) => tool.name !== "desktop_do");
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  assert.deepEqual(report.missing, ["desktop_do"]);
});

test("an unprefixed legacy name is a missing tool, not a match", () => {
  const candidate = { tools: [...gladiusTools, ...puppetTools] };
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  assert.equal(report.missing.length, 32);
  // 31, not 32: the two legacy suites both name a tool `screenshot`, so the
  // unprefixed union has one repeated name. That collision is why the prefix
  // exists; it is asserted directly in the next test.
  assert.equal(report.extra.length, 31);
  assert.deepEqual(report.duplicateNames.candidate, ["screenshot"]);
});

test("the legacy suites collide on one tool name, so the prefix is not cosmetic", () => {
  const gladiusNames = new Set(gladiusTools.map((tool) => tool.name));
  const shared = puppetTools.map((tool) => tool.name).filter((name) => gladiusNames.has(name));
  assert.deepEqual(shared, ["screenshot"]);
  const handNames = handCatalog.tools.map((tool) => tool.name);
  assert.equal(new Set(handNames).size, handNames.length, "prefixed names are unique");
  assert.equal(handNames.filter((name) => name.endsWith("_screenshot")).length, 2);
});

test("hand permits additional tools while reporting them", () => {
  const candidate = prefixed();
  candidate.tools.push({ name: "browser_pilot", inputSchema: { type: "object" } });
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, true);
  assert.deepEqual(report.extra, ["browser_pilot"]);
});

test("legacy recapture rejects additional tools", () => {
  const candidate = { tools: clone(gladiusTools) };
  candidate.tools.push({ name: "new_tool", inputSchema: { type: "object" } });
  const report = compareCatalogs({ baseline: { tools: gladiusTools }, candidate, profile: "legacy" });
  assert.equal(report.ok, false);
  assert.deepEqual(report.extra, ["new_tool"]);
});

test("a duplicate tool name is rejected", () => {
  const candidate = prefixed();
  candidate.tools.push(clone(candidate.tools[0]));
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  assert.deepEqual(report.duplicateNames.candidate, [candidate.tools[0].name]);
});

test("a rewritten description is drift, and only --strict-metadata fails it", () => {
  const candidate = prefixed();
  const navigate = candidate.tools.find((tool) => tool.name === "browser_navigate");
  navigate.description = "Go somewhere";
  const relaxed = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(relaxed.ok, true, "schema equality is the contract; description drift is reported");
  assert.equal(relaxed.metadataDrift.length, 1);
  assert.equal(relaxed.metadataDrift[0].tool, "browser_navigate");
  const strict = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand", strictMetadata: true });
  assert.equal(strict.ok, false);
});

test("the legacy profile treats a description change as a failure", () => {
  const candidate = { tools: clone(gladiusTools) };
  candidate.tools[0].description = `${candidate.tools[0].description} `;
  const report = compareCatalogs({ baseline: { tools: gladiusTools }, candidate, profile: "legacy" });
  assert.equal(report.ok, false);
  assert.equal(report.metadataDrift[0].tool, gladiusTools[0].name);
});

for (const [key, source] of Object.entries(pins.sources)) {
  test(`live: ${key} at ${source.pin} still emits the committed catalogue`, async (t) => {
    if (process.env.HAND_BASELINE_SKIP_LIVE === "1") return t.skip("HAND_BASELINE_SKIP_LIVE=1");
    const binary = resolve(source.repo, "target", "release", key === "gladius" ? "gladius.exe" : "puppet.exe");
    if (!existsSync(binary)) return t.skip(`binary not present: ${binary}`);
    const captured = await captureCatalog({ command: binary, label: key, outPrefix: "<memory>", write: false, timeoutMs: 60_000 });
    const committed = JSON.parse(readFileSync(resolve(ROOT, source.capture.tools), "utf8"));
    assert.equal(captured.meta.catalog.canonicalHash, canonicalHash(committed), `${key} canonical hash drifted`);
    assert.equal(captured.meta.executable.sha256, source.capture.capturedExecutableSha256, `${key} binary changed since capture`);
    assert.equal(captured.meta.version.stdout.includes(source.version), true, `${key} version line`);
  });
}
