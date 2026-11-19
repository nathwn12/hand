
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { canonicalHash } from "../scripts/lib/canonical.mjs";
import { DECLARED_ADDITIONS, SPEC_REQUIRED, compareCatalogs, validateHandCatalog } from "../scripts/lib/catalog.mjs";
import { buildFromDisk, validateLive } from "../scripts/build-hand-catalog.mjs";
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

function withoutLive(catalog) {
  const clone = JSON.parse(JSON.stringify(catalog));
  delete clone.live;
  delete clone.counts.live;
  clone.notes = clone.notes.filter((note) => !note.startsWith("live: "));
  return clone;
}

function setPath(root, dotted, value) {
  const segments = dotted.split(".");
  const last = segments.pop();
  const parent = segments.reduce((node, segment) => {
    if (!node[segment] || typeof node[segment] !== "object") node[segment] = {};
    return node[segment];
  }, root);
  parent[last] = value;
}

function prefixed() {
  const candidate = {
    tools: [
      ...gladiusTools.map((tool) => ({ ...clone(tool), name: `browser_${tool.name}` })),
      ...puppetTools.map((tool) => ({ ...clone(tool), name: `desktop_${tool.name}` })),
    ],
  };
  for (const [key, tools] of Object.entries({ inputSchema: DECLARED_ADDITIONS.inputSchema, outputSchema: DECLARED_ADDITIONS.outputSchema })) {
    for (const [name, paths] of Object.entries(tools)) {
      const tool = candidate.tools.find((entry) => entry.name === name);
      for (const path of paths) setPath(tool[key], path, { type: "object", description: "declared addition" });
    }
  }
  for (const name of DECLARED_ADDITIONS.metadata.description) {
    const tool = candidate.tools.find((entry) => entry.name === name);
    tool.description = `${tool.description} Declared rewrite: names the parameters this tree added.`;
  }
  return candidate;
}

test("committed catalogues match the pinned canonical hashes", () => {
  for (const [key, source] of Object.entries(pins.sources)) {
    const captured = readTools(source.capture.tools);
    assert.equal(captured.length, source.expectedToolCount, `${key} tool count`);
    const result = JSON.parse(readFileSync(resolve(ROOT, source.capture.tools), "utf8"));
    assert.equal(canonicalHash(result), source.capture.expectCatalogCanonicalHash, `${key} canonical hash`);
  }
});

test("captured names are exactly the spec's 25 browser and 7 desktop tools", () => {
  assert.deepEqual(gladiusTools.map((tool) => tool.name).sort(), [...SPEC_REQUIRED.browser].sort());
  assert.deepEqual(puppetTools.map((tool) => tool.name).sort(), [...SPEC_REQUIRED.desktop].sort());
});

test("hand-catalog.json is exactly what the generator derives, not a hand edit", () => {
  const derived = buildFromDisk({ root: ROOT }).hand;
  assert.deepEqual(withoutLive(handCatalog), derived);
  assert.equal(handCatalog.tools.length, 32);
  assert.deepEqual(handCatalog.counts, { total: 32, browser: 25, desktop: 7, live: 35 });
});

test("the live block records the server's own tools/list beside the 32-name floor", () => {
  const live = handCatalog.live;
  assert.equal(live.toolCount, 35);
  assert.equal(live.toolCount, live.names.length);
  assert.equal(handCatalog.counts.live, live.toolCount);
  assert.equal(new Set(live.names).size, live.names.length, "live names are unique");
  assert.deepEqual(live.names, [...live.names].sort(), "live names are sorted");

  const floor = handCatalog.tools.map((tool) => tool.name);
  assert.deepEqual(live.names.filter((name) => !floor.includes(name)), [...live.extra]);

  assert.deepEqual(live.extra, ["browser_context", "browser_drag", "browser_wheel"]);

  assert.deepEqual(Object.keys(live.capture).sort(), ["executable", "executableSha256", "reportedVersion"]);
  assert.match(live.capture.executableSha256, /^sha256:[0-9a-f]{64}$/);
  assert.ok(live.capture.reportedVersion.includes("hand"), live.capture.reportedVersion);
  assert.match(live.protocolVersion, /^\d{4}-\d{2}-\d{2}$/);

  assert.deepEqual(validateLive(handCatalog), []);
});

test("a live block that contradicts the floor is rejected, so the check is not vacuous", () => {
  const omitFloorTool = JSON.parse(JSON.stringify(handCatalog));
  omitFloorTool.live.names = omitFloorTool.live.names.filter((name) => name !== "desktop_do");
  omitFloorTool.live.toolCount = omitFloorTool.live.names.length;
  omitFloorTool.counts.live = omitFloorTool.live.toolCount;
  const omitted = validateLive(omitFloorTool).map((issue) => issue.code);
  assert.ok(omitted.includes("live-omits-floor-tool"), omitted.join(", "));

  const misfiledExtra = JSON.parse(JSON.stringify(handCatalog));
  misfiledExtra.live.extra = [];
  const extra = validateLive(misfiledExtra).map((issue) => issue.code);
  assert.deepEqual(extra, ["live-extra-mismatch"]);

  const timestamped = JSON.parse(JSON.stringify(handCatalog));
  timestamped.live.capture.capturedAt = "2026-11-18T00:00:00Z";
  const stamped = validateLive(timestamped).map((issue) => issue.code);
  assert.deepEqual(stamped, ["live-capture-unexpected-field"]);

  const counted = JSON.parse(JSON.stringify(handCatalog));
  counted.live.toolCount = 34;
  const miscounted = validateLive(counted).map((issue) => issue.code);
  assert.ok(miscounted.includes("live-count-mismatch"), miscounted.join(", "));
  assert.ok(miscounted.includes("live-count-not-in-counts"), miscounted.join(", "));
});

test("hand catalogue validates against the required surface", () => {
  assert.deepEqual(validateHandCatalog(handCatalog), []);
});

test("the floor plus its declared allowances passes the hand comparison with no metadata drift", () => {
  const report = compareCatalogs({ baseline: handCatalog, candidate: prefixed(), profile: "hand" });
  assert.equal(report.ok, true, JSON.stringify({ mismatches: report.schemaMismatches, missing: report.declaredMissing }, null, 2));
  assert.equal(report.counts.compared, 32);
  assert.deepEqual(report.missing, []);
  assert.deepEqual(report.extra, []);
  assert.deepEqual(report.metadataDrift, []);
  assert.deepEqual(report.declaredMissing, []);
  assert.equal(report.declaredAdditions.length, 17, "every declared allowance is exercised: 10 schema groups and 7 descriptions");
  assert.equal(
    report.declaredAdditions.filter((entry) => entry.kind === "metadata.description").length,
    7,
    "the seven rewritten descriptions are allowances, not drift",
  );
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

test("a rewritten description is a failure, not drift, unless the tool is declared", () => {
  const candidate = prefixed();
  const navigate = candidate.tools.find((tool) => tool.name === "browser_navigate");
  navigate.description = "Go somewhere";
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "a description is pinned exactly like a schema");
  assert.equal(report.metadataDrift.length, 1);
  assert.equal(report.metadataDrift[0].tool, "browser_navigate");
  assert.equal(report.metadataDrift[0].kind, "description");

  const declared = prefixed();
  const click = declared.tools.find((tool) => tool.name === "browser_click");
  assert.notEqual(
    click.description,
    handCatalog.tools.find((tool) => tool.name === "browser_click").description,
  );
  const allowed = compareCatalogs({ baseline: handCatalog, candidate: declared, profile: "hand" });
  assert.equal(allowed.ok, true);
  assert.deepEqual(allowed.metadataDrift, []);
  assert.equal(
    allowed.declaredAdditions.filter((entry) => entry.kind === "metadata.description" && entry.tool === "browser_click").length,
    1,
  );
});

test("a title difference is always a failure", () => {
  const candidate = prefixed();
  const navigate = candidate.tools.find((tool) => tool.name === "browser_navigate");
  navigate.title = "Browser navigate";
  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  assert.deepEqual(report.metadataDrift.map((entry) => `${entry.tool} ${entry.kind}`), ["browser_navigate title"]);
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
