// Catalogue model: the spec's required surface, the normalization applied to a
// captured MCP `tools/list` result, and the hand-namespaced catalogue built
// from the two pinned legacy catalogues.
//
// Normalization is deliberately lossless where the MCP schema allows it: every
// field a server sends is kept, and only the comparison *projection* is narrow
// (name, title, description, inputSchema, outputSchema, annotations). Adding a
// field to a legacy tool is therefore visible as a difference instead of being
// dropped on the floor.

import { canonicalHash, deepEqual, diffPaths } from "./canonical.mjs";

/** Required for the first release by `hand-spec.md` (25 + 7 = 32). */
export const SPEC_REQUIRED = {
  browser: [
    "navigate", "snapshot", "evaluate", "console", "network",
    "fetch_text", "click", "type", "hover", "select", "key", "upload",
    "wait_for", "assert_layout", "layout", "screenshot", "visual", "viewport",
    "tabs", "targets", "close", "storage", "dialog", "history", "cdp",
  ],
  desktop: [
    "screenshot", "diff", "list_windows", "window", "do", "cursor", "elements",
  ],
};

export const PREFIX = { browser: "browser_", desktop: "desktop_" };

/** Metadata keys compared as "documentation" rather than as contract. */
const METADATA_KEYS = ["title", "description"];

/**
 * Output-schema properties hand adds on purpose, per tool. Everything else must
 * stay byte-comparable to the legacy body: a new field cannot ship unnoticed, and
 * a declared one still has to match its own promise (the property is removed from
 * the candidate before the deep comparison, together with its `required` entry).
 *
 * Justify each entry in docs/COMPATIBILITY.md under "Intentional differences".
 */
const DECLARED_OUTPUT_ADDITIONS = {
  browser_click: ["resolved"],
  browser_hover: ["resolved"],
  browser_select: ["resolved"],
  browser_type: ["resolved"],
  browser_upload: ["resolved"],
  // hand adds attribution beside the raw before/after booleans: the import
  // audit and the samples that say whether a pointer/foreground change was the
  // owner's input or this process's. The legacy fields stay unchanged.
  desktop_screenshot: ["noSteal"],
  desktop_elements: ["noSteal"],
};

/** Remove declared additions from a candidate schema so the rest must match exactly. */
function withoutDeclaredProperties(schema, names) {
  if (!schema || typeof schema !== "object" || names.length === 0) return schema;
  const clone = JSON.parse(JSON.stringify(schema));
  if (clone.properties) for (const name of names) delete clone.properties[name];
  if (Array.isArray(clone.required)) {
    clone.required = clone.required.filter((key) => !names.includes(key));
    if (clone.required.length === 0) delete clone.required;
  }
  return clone;
}

/** The contract projection of one tool. Absent means absent, not undefined. */
export function project(tool) {
  const out = { name: tool.name };
  for (const key of ["title", "description", "inputSchema", "outputSchema", "annotations"]) {
    if (key in tool) out[key] = tool[key];
  }
  return out;
}

export function schemaHash(tool) {
  return {
    input: tool.inputSchema ? canonicalHash(tool.inputSchema) : null,
    output: tool.outputSchema ? canonicalHash(tool.outputSchema) : null,
  };
}

/** Index a `tools/list` result by name, rejecting duplicates. */
export function indexTools(tools) {
  const index = new Map();
  const duplicates = [];
  for (const tool of tools) {
    if (typeof tool?.name !== "string" || !tool.name) {
      throw new Error("a tool has no string name");
    }
    if (index.has(tool.name)) duplicates.push(tool.name);
    index.set(tool.name, tool);
  }
  return { index, duplicates };
}

/** Accept a full JSON-RPC envelope, a `result` object, or a bare array. */
export function toolsFromPayload(payload) {
  if (Array.isArray(payload)) return payload;
  if (Array.isArray(payload?.tools)) return payload.tools;
  if (Array.isArray(payload?.result?.tools)) return payload.result.tools;
  throw new Error("payload has no tools[] array (expected a tools/list result or JSON-RPC envelope)");
}

export function handName(domain, legacyName) {
  const prefix = PREFIX[domain];
  if (!prefix) throw new Error(`unknown domain: ${domain}`);
  return `${prefix}${legacyName}`;
}

export function splitHandName(name) {
  for (const domain of Object.keys(PREFIX)) {
    if (name.startsWith(PREFIX[domain])) return { domain, legacyName: name.slice(PREFIX[domain].length) };
  }
  return null;
}

/**
 * Build the hand catalogue: both legacy catalogues with the documented
 * one-to-one prefix mapping. Nothing else is renamed, merged or reordered.
 */
export function buildHandCatalog({ gladius, puppet, provenance }) {
  const groups = [
    { domain: "browser", source: "gladius", captured: gladius },
    { domain: "desktop", source: "puppet", captured: puppet },
  ];
  const tools = [];
  for (const { domain, source, captured } of groups) {
    for (const tool of captured.tools) {
      tools.push({
        ...project(tool),
        name: handName(domain, tool.name),
        legacyName: tool.name,
        domain,
        source,
        schemaHash: schemaHash(tool),
      });
    }
  }
  return {
    $comment:
      "Derived baseline for hand. Tools are the pinned legacy catalogues with the documented browser_/desktop_ prefix; no schema, description, annotation or ordering was changed. Regenerate with: node baseline/scripts/build-hand-catalog.mjs",
    spec: "docs/SPEC.md: the 32-tool floor is in 'Initial public surface and lifecycle'; tools beyond it are additions, not violations.",
    provenance,
    counts: {
      total: tools.length,
      browser: tools.filter((tool) => tool.domain === "browser").length,
      desktop: tools.filter((tool) => tool.domain === "desktop").length,
    },
    tools,
  };
}

/** Structural checks on a hand catalogue, independent of any server. */
export function validateHandCatalog(catalog) {
  const issues = [];
  const { index, duplicates } = indexTools(catalog.tools);
  for (const name of duplicates) issues.push({ level: "error", code: "duplicate-tool-name", tool: name });
  for (const domain of Object.keys(SPEC_REQUIRED)) {
    for (const legacyName of SPEC_REQUIRED[domain]) {
      const name = handName(domain, legacyName);
      const tool = index.get(name);
      if (!tool) {
        issues.push({ level: "error", code: "missing-required-tool", tool: name });
        continue;
      }
      if (tool.legacyName !== legacyName) {
        issues.push({ level: "error", code: "legacy-name-mismatch", tool: name, detail: tool.legacyName });
      }
      if (tool.domain !== domain) issues.push({ level: "error", code: "domain-mismatch", tool: name, detail: tool.domain });
      if (!tool.inputSchema) issues.push({ level: "error", code: "missing-input-schema", tool: name });
    }
  }
  for (const tool of catalog.tools) {
    const split = splitHandName(tool.name);
    if (!split) issues.push({ level: "error", code: "unprefixed-tool-name", tool: tool.name });
    if (tool.inputSchema && tool.inputSchema.type !== "object") {
      issues.push({ level: "error", code: "input-schema-not-object", tool: tool.name, detail: tool.inputSchema.type });
    }
  }
  return issues;
}

/**
 * Compare a candidate catalogue against a baseline catalogue.
 *
 * `profile: "legacy"` requires the exact legacy names and treats every field,
 * including title and description, as contract: it is the re-capture check
 * that proves a pinned revision still emits what is committed here.
 *
 * `profile: "hand"` requires the 32 prefixed names, and treats inputSchema,
 * outputSchema and annotations as contract. Title/description differences are
 * reported as drift, because the spec allows them to change with the prefix or
 * the server identity; `strictMetadata` turns that drift into a failure.
 */
export function compareCatalogs({ baseline, candidate, profile = "hand", strictMetadata = false }) {
  // In the legacy profile title and description are part of the contract: the
  // whole point of that profile is to prove a pinned binary emits exactly what
  // is committed. Only the hand profile may tolerate metadata drift.
  const strict = strictMetadata || profile === "legacy";
  const base = indexTools(baseline.tools);
  const cand = indexTools(candidate.tools);
  const report = {
    profile,
    strictMetadata: strict,
    ok: true,
    counts: { baseline: baseline.tools.length, candidate: candidate.tools.length, compared: 0 },
    duplicateNames: { baseline: base.duplicates, candidate: cand.duplicates },
    missing: [],
    extra: [],
    schemaMismatches: [],
    declaredAdditions: [],
    metadataDrift: [],
    warnings: [],
  };

  const expected = new Map(); // candidate name -> baseline tool + expected legacy name
  for (const tool of baseline.tools) {
    const name = profile === "hand" ? handName(tool.domain, tool.legacyName ?? tool.name) : tool.name;
    expected.set(name, tool);
  }

  for (const [name, baseTool] of expected) {
    const candTool = cand.index.get(name);
    if (!candTool) {
      report.missing.push(name);
      continue;
    }
    report.counts.compared += 1;
    if (profile === "hand") {
      const split = splitHandName(name);
      const legacyName = baseTool.legacyName ?? baseTool.name;
      if (!split || split.legacyName !== legacyName || split.domain !== baseTool.domain) {
        report.schemaMismatches.push({
          tool: name,
          kind: "identity",
          expected: `${baseTool.domain}/${legacyName}`,
          actual: split ? `${split.domain}/${split.legacyName}` : "unprefixed",
          diffs: [{ path: "name", left: name, right: candTool.name }],
        });
      }
    }
    for (const key of ["inputSchema", "outputSchema", "annotations"]) {
      const declared = key === "outputSchema" && profile === "hand" ? (DECLARED_OUTPUT_ADDITIONS[name] ?? []) : [];
      const candidate = declared.length ? withoutDeclaredProperties(candTool[key], declared) : candTool[key];
      if (!deepEqual(baseTool[key], candidate)) {
        report.schemaMismatches.push({
          tool: name,
          kind: key,
          diffs: diffOf(baseTool[key], candidate),
        });
      } else if (declared.length) {
        report.declaredAdditions.push({ tool: name, properties: declared });
      }
    }
    for (const key of METADATA_KEYS) {
      if (!deepEqual(baseTool[key], candTool[key])) {
        report.metadataDrift.push({ tool: name, kind: key, baseline: brief(baseTool[key]), candidate: brief(candTool[key]) });
      }
    }
  }

  for (const name of cand.index.keys()) {
    if (!expected.has(name)) report.extra.push(name);
  }

  if (report.duplicateNames.candidate.length) {
    report.warnings.push(`candidate repeats tool names: ${report.duplicateNames.candidate.join(", ")}`);
  }
  report.ok = report.missing.length === 0
    && (profile === "hand" || report.extra.length === 0)
    && report.schemaMismatches.length === 0
    && report.duplicateNames.candidate.length === 0
    && (!strict || report.metadataDrift.length === 0);
  return report;
}

function diffOf(left, right) {
  return diffPaths(left ?? null, right ?? null);
}

function brief(value) {
  if (value === undefined) return "<absent>";
  const text = typeof value === "string" ? value : JSON.stringify(value);
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}
