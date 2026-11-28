
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  DECLARED_ADDITIONS,
  DECLARED_ADDITIONS_PATH,
  compareCatalogs,
  hasDeclaredPath,
} from "../scripts/lib/catalog.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = resolve(ROOT, "..");
const handCatalog = JSON.parse(readFileSync(resolve(ROOT, "hand-catalog.json"), "utf8"));
const rawFile = JSON.parse(readFileSync(DECLARED_ADDITIONS_PATH, "utf8"));
const clone = (value) => JSON.parse(JSON.stringify(value));

const ALLOWANCE_KEYS = ["inputSchema", "outputSchema"];
const DECLARED_DESCRIPTIONS = [
  "browser_assert_layout",
  "browser_click",
  "browser_hover",
  "browser_layout",
  "browser_screenshot",
  "browser_select",
  "browser_snapshot",
  "browser_type",
  "browser_upload",
  "browser_visual",
  "desktop_diff",
  "desktop_do",
  "desktop_elements",
  "desktop_screenshot",
  "desktop_window",
];

function setPath(root, dotted, value) {
  const segments = dotted.split(".");
  const last = segments.pop();
  const parent = segments.reduce((node, segment) => {
    if (!node[segment] || typeof node[segment] !== "object") node[segment] = {};
    return node[segment];
  }, root);
  parent[last] = value;
}

function getPath(root, dotted) {
  let node = root;
  for (const segment of dotted.split(".")) {
    if (!node || typeof node !== "object") return undefined;
    node = node[segment];
  }
  return node;
}

function deletePath(root, dotted) {
  const segments = dotted.split(".");
  const last = segments.pop();
  const parent = segments.reduce((node, segment) => (node && typeof node === "object" ? node[segment] : undefined), root);
  if (parent && typeof parent === "object") delete parent[last];
}

function additiveCandidate() {
  const candidate = { tools: clone(handCatalog.tools) };
  for (const key of ALLOWANCE_KEYS) {
    for (const [name, paths] of Object.entries(DECLARED_ADDITIONS[key])) {
      const tool = candidate.tools.find((entry) => entry.name === name);
      assert.ok(tool, `${key} declares an addition for ${name}, which is not in the floor`);
      for (const path of paths) setPath(tool[key], path, { type: "object", description: "declared addition" });
    }
  }
  for (const name of DECLARED_ADDITIONS.metadata.description) {
    const tool = candidate.tools.find((entry) => entry.name === name);
    assert.ok(tool, `metadata.description declares ${name}, which is not in the floor`);
    tool.description = `${tool.description} Declared rewrite: names the parameters this tree added.`;
  }
  for (const rewrite of DECLARED_ADDITIONS.metadata.rewrites ?? []) {
    const tool = candidate.tools.find((entry) => entry.name === rewrite.tool);
    assert.ok(tool, `metadata.rewrites declares ${rewrite.tool}, which is not in the floor`);
    const original = getPath(tool[rewrite.schema], rewrite.path);
    setPath(tool[rewrite.schema], rewrite.path, `${original ?? ""} Declared rewrite: names the real opt-in.`);
  }
  return candidate;
}

function runCli(candidate) {
  const dir = mkdtempSync(join(tmpdir(), "hand-declared-"));
  const file = join(dir, "candidate.tools.json");
  try {
    writeFileSync(file, `${JSON.stringify(candidate, null, 2)}\n`);
    const run = spawnSync(process.execPath, [resolve(ROOT, "scripts", "compare-catalog.mjs"), "--candidate", file], {
      cwd: REPO,
      encoding: "utf8",
    });
    return { status: run.status, stdout: run.stdout, stderr: run.stderr };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("the allowance file is versioned, dotted and read by the comparator itself", () => {
  assert.equal(rawFile.version, 1, "the file is versioned");
  const unknown = Object.keys(rawFile).filter((key) => !["$comment", "version", "metadata", ...ALLOWANCE_KEYS].includes(key));
  assert.deepEqual(unknown, [], "only declared schema keys and metadata may appear at the top level");

  assert.deepEqual(DECLARED_ADDITIONS, {
    version: rawFile.version,
    inputSchema: rawFile.inputSchema,
    outputSchema: rawFile.outputSchema,
    metadata: rawFile.metadata,
  });

  const floorNames = new Set(handCatalog.tools.map((tool) => tool.name));
  for (const key of ALLOWANCE_KEYS) {
    for (const [name, paths] of Object.entries(rawFile[key] ?? {})) {
      assert.ok(floorNames.has(name), `${key}.${name} is not a floor tool`);
      assert.ok(paths.length > 0, `${key}.${name} declares no path`);
      assert.equal(new Set(paths).size, paths.length, `${key}.${name} repeats a path`);
      const tool = handCatalog.tools.find((entry) => entry.name === name);
      for (const path of paths) {
        assert.match(path, /^[A-Za-z0-9_$-]+(\.[A-Za-z0-9_$-]+)*$/, `${key}.${name}: ${path} is not a dotted path or a top-level key`);
        assert.equal(
          hasDeclaredPath(tool[key], path),
          false,
          `${key}.${name} declares ${path}, which the pinned body already carries`,
        );
      }
    }
  }

  assert.deepEqual(Object.keys(rawFile.metadata), ["description", "rewrites"], "description and rewrites may be declared");
  assert.deepEqual(
    [...rawFile.metadata.description].sort(),
    DECLARED_DESCRIPTIONS,
    "exactly the fifteen rewritten descriptions are declared, nothing more and nothing less",
  );
  for (const name of rawFile.metadata.description) {
    assert.ok(floorNames.has(name), `metadata.description.${name} is not a floor tool`);
  }
  for (const rewrite of rawFile.metadata.rewrites ?? []) {
    assert.ok(floorNames.has(rewrite.tool), `metadata.rewrites.${rewrite.tool} is not a floor tool`);
    assert.ok([
      "inputSchema",
      "outputSchema",
    ].includes(rewrite.schema), `${rewrite.tool} has an unknown schema: ${rewrite.schema}`);
    const floorTool = handCatalog.tools.find((entry) => entry.name === rewrite.tool);
    assert.equal(
      hasDeclaredPath(floorTool[rewrite.schema], rewrite.path),
      true,
      `${rewrite.tool} ${rewrite.schema}.${rewrite.path} is absent from the floor and cannot be a rewrite`,
    );
  }
});

test("a candidate that is the floor plus the declared allowances passes, and the report names them", () => {
  const report = compareCatalogs({ baseline: handCatalog, candidate: additiveCandidate(), profile: "hand" });
  assert.equal(report.ok, true, JSON.stringify(report.schemaMismatches, null, 2));
  assert.deepEqual(report.declaredMissing, []);
  assert.deepEqual(report.metadataDrift, [], "a declared description is an allowance, not drift");

  const input = report.declaredAdditions.filter((entry) => entry.kind === "inputSchema");
  const output = report.declaredAdditions.filter((entry) => entry.kind === "outputSchema");
  const descriptions = report.declaredAdditions.filter((entry) => entry.kind === "metadata.description");
  const expectedInput = Object.entries(DECLARED_ADDITIONS.inputSchema)
    .map(([tool, paths]) => `${tool} ${[...paths].sort().join(",")}`)
    .sort();
  assert.deepEqual(
    input.map((entry) => `${entry.tool} ${[...entry.paths].sort().join(",")}`).sort(),
    expectedInput,
    "every declared input addition is reported exactly once",
  );
  assert.equal(
    output.reduce((count, entry) => count + entry.paths.length, 0),
    Object.values(DECLARED_ADDITIONS.outputSchema).reduce((count, paths) => count + paths.length, 0),
    "every declared output addition is reported",
  );
  assert.deepEqual(
    descriptions.map((entry) => entry.tool).sort(),
    DECLARED_DESCRIPTIONS,
    "one place lists every allowance, so all fifteen rewritten descriptions are named here",
  );
});

test("an addition that is not declared fails: red on properties.bogus", () => {
  const candidate = additiveCandidate();
  const click = candidate.tools.find((tool) => tool.name === "browser_click");
  setPath(click.inputSchema, "properties.bogus", { type: "string" });

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "an undeclared addition must fail the gate");
  const mismatch = report.schemaMismatches.find((entry) => entry.tool === "browser_click");
  assert.ok(mismatch, "browser_click must be named");
  assert.equal(mismatch.kind, "inputSchema");
  assert.ok(
    mismatch.diffs.some((diff) => diff.path === "properties.bogus" && diff.kind === "missing-left"),
    JSON.stringify(mismatch.diffs),
  );

  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /^FAIL hand baseline: 32 baseline tools vs 32 candidate tools/m);
  assert.match(cli.stdout, /MISMATCH {3}browser_click inputSchema/);
  assert.match(cli.stdout, /properties\.bogus: missing-left/);
});

test("a declared addition that is missing fails: red on a deleted properties.clickCount", () => {
  const candidate = additiveCandidate();
  const click = candidate.tools.find((tool) => tool.name === "browser_click");
  deletePath(click.inputSchema, "properties.clickCount");

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "a declared addition that vanished must fail the gate");
  const missing = report.declaredMissing.find((entry) => entry.tool === "browser_click");
  assert.ok(missing, JSON.stringify(report.declaredMissing));
  assert.equal(missing.kind, "inputSchema");
  assert.deepEqual(missing.paths, ["properties.clickCount"]);
  assert.deepEqual(report.schemaMismatches, []);

  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /MISSING-ADDITION browser_click inputSchema: properties\.clickCount/);
});

test("a listed addition under a renamed property fails, because presence is checked first", () => {
  const candidate = additiveCandidate();
  const screenshot = candidate.tools.find((tool) => tool.name === "browser_screenshot");
  const value = screenshot.outputSchema.properties.format;
  delete screenshot.outputSchema.properties.format;
  screenshot.outputSchema.properties.mime = value;

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false);
  const missing = report.declaredMissing.find((entry) => entry.tool === "browser_screenshot");
  assert.ok(missing, JSON.stringify(report.declaredMissing));
  assert.deepEqual(missing.paths, ["properties.format"]);
});

test("an eighth tool's rewritten description fails: red on an undeclared description", () => {
  const candidate = additiveCandidate();
  const navigate = candidate.tools.find((tool) => tool.name === "browser_navigate");
  navigate.description = "Go somewhere";

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "an undeclared description change must fail the gate");
  const drift = report.metadataDrift.find((entry) => entry.tool === "browser_navigate");
  assert.ok(drift, JSON.stringify(report.metadataDrift));
  assert.equal(drift.kind, "description");
  assert.equal(drift.candidate, "Go somewhere");
  assert.deepEqual(report.schemaMismatches, [], "the description alone is what failed");

  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /^FAIL hand baseline: 32 baseline tools vs 32 candidate tools/m);
  assert.match(cli.stdout, /metadata {3}1 title\/description difference\(s\) \(strict: on\)/);
  assert.match(cli.stdout, /\n {13}browser_navigate description/);
});

test("a declared description left at the pin fails as a stale declaration", () => {
  const candidate = additiveCandidate();
  const pin = handCatalog.tools.find((tool) => tool.name === "browser_click");
  const click = candidate.tools.find((tool) => tool.name === "browser_click");
  click.description = pin.description;

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "an unapplied declaration must fail the gate");
  const stale = report.declaredMissing.find((entry) => entry.tool === "browser_click");
  assert.ok(stale, JSON.stringify(report.declaredMissing));
  assert.equal(stale.kind, "metadata.description");
  assert.deepEqual(stale.paths, ["description"]);
  assert.deepEqual(report.metadataDrift, []);

  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /^FAIL hand baseline: 32 baseline tools vs 32 candidate tools/m);
  assert.match(cli.stdout, /MISSING-ADDITION browser_click metadata\.description: description declared in baseline\/declared-additions\.json but absent from the candidate/);
});

test("a title difference fails even on a tool whose description is declared", () => {
  const candidate = additiveCandidate();
  const click = candidate.tools.find((tool) => tool.name === "browser_click");
  const pin = handCatalog.tools.find((tool) => tool.name === "browser_click");
  click.title = pin.title ? `${pin.title} (hand)` : "browser_click";

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "a title change must fail the gate");
  const drift = report.metadataDrift.find((entry) => entry.tool === "browser_click");
  assert.ok(drift, JSON.stringify(report.metadataDrift));
  assert.equal(drift.kind, "title");

  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /browser_click title/);
});

test("the green candidate is green through the real CLI, exit 0", () => {
  const cli = runCli(additiveCandidate());
  assert.equal(cli.status, 0, cli.stdout + cli.stderr);
  assert.match(cli.stdout, /^PASS hand baseline: 32 baseline tools vs 32 candidate tools/m);
  assert.match(cli.stdout, /compared {3}32 tools/);
  assert.match(cli.stdout, /metadata {3}0 title\/description difference\(s\) \(strict: on\)/);
  assert.doesNotMatch(cli.stdout, /failure/i, "a passing run must not print the word failure");
  assert.doesNotMatch(cli.stdout, /--strict-metadata/, "the reported policy is not stated as a flag");
  assert.match(cli.stdout, /screenshot inputSchema \+= properties\.clip, properties\.element, properties\.format, properties\.quality/);
  assert.match(cli.stdout, /browser_click metadata\.description \+= description/);
  const declaredGroups =
    Object.keys(DECLARED_ADDITIONS.inputSchema).length +
    Object.keys(DECLARED_ADDITIONS.outputSchema).length +
    DECLARED_ADDITIONS.metadata.description.length;
  assert.match(cli.stdout, new RegExp(`declared {3}${declaredGroups} addition group\\(s\\)`));
});

test("a declared rewrite left at the pin fails as a stale declaration through the real CLI", () => {
  const candidate = additiveCandidate();
  const rewrite = DECLARED_ADDITIONS.metadata.rewrites[0];
  assert.ok(rewrite, "there is a declared property rewrite");
  const tool = candidate.tools.find((entry) => entry.name === rewrite.tool);
  const pin = handCatalog.tools.find((entry) => entry.name === rewrite.tool);
  setPath(tool[rewrite.schema], rewrite.path, getPath(pin[rewrite.schema], rewrite.path));

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "a stale rewrite must fail the comparator");
  assert.ok(
    report.rewriteIssues.some((issue) => issue.includes("identical to the pin: a stale declaration")),
    JSON.stringify(report.rewriteIssues),
  );
  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.ok(
    cli.stdout.includes(
      `REWRITE-ISSUE rewrite ${rewrite.tool} ${rewrite.schema}.${rewrite.path}: identical to the pin: a stale declaration`,
    ),
    cli.stdout,
  );
});

test("a declared rewrite whose path vanished from the published body fails", () => {
  const candidate = additiveCandidate();
  const rewrite = DECLARED_ADDITIONS.metadata.rewrites[0];
  const tool = candidate.tools.find((entry) => entry.name === rewrite.tool);
  deletePath(tool[rewrite.schema], rewrite.path);

  const report = compareCatalogs({ baseline: handCatalog, candidate, profile: "hand" });
  assert.equal(report.ok, false, "a rewrite path missing from the published body must fail");
  assert.ok(
    report.rewriteIssues.some((issue) => issue.includes("path is absent from the published body")),
    JSON.stringify(report.rewriteIssues),
  );
  const cli = runCli(candidate);
  assert.equal(cli.status, 1, cli.stdout + cli.stderr);
  assert.ok(
    cli.stdout.includes(
      `REWRITE-ISSUE rewrite ${rewrite.tool} ${rewrite.schema}.${rewrite.path}: path is absent from the published body`,
    ),
    cli.stdout,
  );
});

test("the legacy profile is never allowed to grow: a declared path does not soften a recapture", () => {
  const gladius = JSON.parse(readFileSync(resolve(ROOT, "catalog", "gladius-59c6d77.tools.json"), "utf8")).tools;
  const candidate = { tools: clone(gladius) };
  const click = candidate.tools.find((tool) => tool.name === "click");
  setPath(click.inputSchema, "properties.button", { type: "string" });
  const report = compareCatalogs({ baseline: { tools: gladius }, candidate, profile: "legacy" });
  assert.equal(report.ok, false);
  assert.deepEqual(report.declaredMissing, []);
  assert.equal(report.schemaMismatches[0].tool, "click");
});
