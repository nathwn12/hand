
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { canonicalHash, deepEqual, diffPaths } from "./canonical.mjs";

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

export const DECLARED_ADDITIONS_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "declared-additions.json",
);

const ALLOWANCE_KEYS = ["inputSchema", "outputSchema"];
export const DECLARABLE_METADATA_KEYS = ["description"];
const METADATA_KEY = "metadata";
const TOP_LEVEL_KEYS = ["$comment", "version", METADATA_KEY, ...ALLOWANCE_KEYS];
const PATH_SEGMENT = /^[A-Za-z0-9_$-]+$/;
const TOOL_NAME = /^[A-Za-z0-9_$-]+$/;

function loadDeclaredAdditions(path) {
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  if (parsed.version !== 1) {
    throw new Error(`${path}: unsupported declared-additions version ${JSON.stringify(parsed.version)} (expected 1)`);
  }
  const unknown = Object.keys(parsed).filter((key) => !TOP_LEVEL_KEYS.includes(key));
  if (unknown.length) throw new Error(`${path}: unknown top-level key(s): ${unknown.join(", ")}`);
  const declared = { version: parsed.version };
  for (const key of ALLOWANCE_KEYS) {
    const tools = parsed[key] ?? {};
    if (typeof tools !== "object" || tools === null || Array.isArray(tools)) {
      throw new Error(`${path}: ${key} must be an object of tool name -> path array`);
    }
    declared[key] = {};
    for (const [tool, paths] of Object.entries(tools)) {
      if (!Array.isArray(paths) || paths.length === 0) {
        throw new Error(`${path}: ${key}.${tool} must be a non-empty array of dotted paths`);
      }
      for (const dotted of paths) {
        const segments = typeof dotted === "string" ? dotted.split(".") : [];
        if (segments.length < 2 || !segments.every((segment) => PATH_SEGMENT.test(segment))) {
          throw new Error(`${path}: ${key}.${tool} has a path that is not a dotted schema path: ${JSON.stringify(dotted)}`);
        }
      }
      declared[key][tool] = [...paths];
    }
  }
  const metadata = parsed[METADATA_KEY] ?? {};
  if (typeof metadata !== "object" || metadata === null || Array.isArray(metadata)) {
    throw new Error(`${path}: ${METADATA_KEY} must be an object of metadata key -> tool name array`);
  }
  const undeclarable = Object.keys(metadata).filter((key) => !DECLARABLE_METADATA_KEYS.includes(key));
  if (undeclarable.length) {
    throw new Error(
      `${path}: ${METADATA_KEY} may only declare ${DECLARABLE_METADATA_KEYS.join(", ")}, not ${undeclarable.join(", ")}: title has no declared exceptions`,
    );
  }
  declared[METADATA_KEY] = {};
  for (const key of DECLARABLE_METADATA_KEYS) {
    const names = metadata[key];
    if (names === undefined) {
      declared[METADATA_KEY][key] = [];
      continue;
    }
    if (!Array.isArray(names) || names.length === 0) {
      throw new Error(`${path}: ${METADATA_KEY}.${key} must be a non-empty array of tool names`);
    }
    for (const name of names) {
      if (typeof name !== "string" || !TOOL_NAME.test(name)) {
        throw new Error(`${path}: ${METADATA_KEY}.${key} names a value that is not a bare tool name: ${JSON.stringify(name)}`);
      }
    }
    if (new Set(names).size !== names.length) {
      throw new Error(`${path}: ${METADATA_KEY}.${key} repeats a tool name`);
    }
    declared[METADATA_KEY][key] = [...names];
  }
  return declared;
}

export const DECLARED_ADDITIONS = loadDeclaredAdditions(DECLARED_ADDITIONS_PATH);

export function declaredAdditionsFor(name, key, profile = "hand") {
  if (profile !== "hand" || !ALLOWANCE_KEYS.includes(key)) return [];
  return DECLARED_ADDITIONS[key]?.[name] ?? [];
}

export function declaredMetadataFor(name, key, profile = "hand") {
  if (profile !== "hand" || !DECLARABLE_METADATA_KEYS.includes(key)) return false;
  return DECLARED_ADDITIONS[METADATA_KEY][key].includes(name);
}

export function hasDeclaredPath(schema, path) {
  if (!schema || typeof schema !== "object") return false;
  let node = schema;
  for (const segment of path.split(".")) {
    if (!node || typeof node !== "object" || Array.isArray(node)) return false;
    if (!Object.hasOwn(node, segment)) return false;
    node = node[segment];
  }
  return true;
}

function withoutDeclaredPaths(schema, paths) {
  if (!schema || typeof schema !== "object" || paths.length === 0) return schema;
  const clone = JSON.parse(JSON.stringify(schema));
  for (const path of paths) {
    const segments = path.split(".");
    const key = segments[segments.length - 1];
    const parent = segments.slice(0, -1).reduce(
      (node, segment) => (node && typeof node === "object" && !Array.isArray(node) ? node[segment] : undefined),
      clone,
    );
    if (!parent || typeof parent !== "object" || Array.isArray(parent)) continue;
    if (!Object.hasOwn(parent, key)) continue;
    delete parent[key];
    if (segments[segments.length - 2] === "properties") {
      const owner = segments.slice(0, -2).reduce(
        (node, segment) => (node && typeof node === "object" ? node[segment] : undefined),
        clone,
      );
      if (owner && Array.isArray(owner.required)) {
        owner.required = owner.required.filter((name) => name !== key);
        if (owner.required.length === 0) delete owner.required;
      }
    }
  }
  return clone;
}

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
 * 0.2.0 deliberate extension of the pinned `browser_network` floor. The five
 * interception actions are ADDED after the pinned get/drain/clear; `blockedUrls`
 * is optional. Keeping this policy in the generator means every regeneration
 * carries it, so the floor is derived and never hand-edited. Human record:
 * docs/COMPATIBILITY.md item 11.
 */
export const HAND_020_NETWORK_ACTIONS = ["route", "mock", "abort", "offline", "restore"];

export function applyHand020Extensions(catalog) {
  const tool = catalog.tools?.find((entry) => entry.name === "browser_network");
  const action = tool?.inputSchema?.properties?.action;
  if (!action || !Array.isArray(action.enum)) return catalog;
  for (const name of HAND_020_NETWORK_ACTIONS) {
    if (!action.enum.includes(name)) action.enum.push(name);
  }
  tool.inputSchema.properties.blockedUrls = { type: "array", items: { type: "string" } };
  return catalog;
}

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
  const catalog = {
    $comment:
      "Derived baseline for hand. Tools are the pinned legacy catalogues with the documented browser_/desktop_ prefix; no schema, description, annotation or ordering was changed, except the one 0.2.0 browser_network extension in applyHand020Extensions. Regenerate with: node baseline/scripts/build-hand-catalog.mjs",
    spec: "docs/SPEC.md: the 32-tool floor is in 'Initial public surface and lifecycle'; tools beyond it are additions, not violations.",
    provenance,
    counts: {
      total: tools.length,
      browser: tools.filter((tool) => tool.domain === "browser").length,
      desktop: tools.filter((tool) => tool.domain === "desktop").length,
    },
    tools,
  };
  return applyHand020Extensions(catalog);
}

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

export function compareCatalogs({ baseline, candidate, profile = "hand" }) {
  const base = indexTools(baseline.tools);
  const cand = indexTools(candidate.tools);
  const report = {
    profile,
    strictMetadata: true,
    ok: true,
    counts: { baseline: baseline.tools.length, candidate: candidate.tools.length, compared: 0 },
    duplicateNames: { baseline: base.duplicates, candidate: cand.duplicates },
    missing: [],
    extra: [],
    schemaMismatches: [],
    declaredAdditions: [],
    declaredMissing: [],
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
      const declared = declaredAdditionsFor(name, key, profile);
      if (declared.length === 0) {
        if (!deepEqual(baseTool[key], candTool[key])) {
          report.schemaMismatches.push({ tool: name, kind: key, diffs: diffOf(baseTool[key], candTool[key]) });
        }
        continue;
      }
      const absent = declared.filter((path) => !hasDeclaredPath(candTool[key], path));
      if (absent.length) report.declaredMissing.push({ tool: name, kind: key, paths: absent });
      const stripped = withoutDeclaredPaths(candTool[key], declared);
      if (!deepEqual(baseTool[key], stripped)) {
        report.schemaMismatches.push({ tool: name, kind: key, diffs: diffOf(baseTool[key], stripped) });
      } else if (absent.length === 0) {
        report.declaredAdditions.push({ tool: name, kind: key, paths: declared });
      }
    }
    if (!deepEqual(baseTool.title, candTool.title)) {
      report.metadataDrift.push({ tool: name, kind: "title", baseline: brief(baseTool.title), candidate: brief(candTool.title) });
    }
    if (declaredMetadataFor(name, "description", profile)) {
      if (deepEqual(baseTool.description, candTool.description)) {
        report.declaredMissing.push({ tool: name, kind: "metadata.description", paths: ["description"] });
      } else {
        report.declaredAdditions.push({ tool: name, kind: "metadata.description", paths: ["description"] });
      }
    } else if (!deepEqual(baseTool.description, candTool.description)) {
      report.metadataDrift.push({ tool: name, kind: "description", baseline: brief(baseTool.description), candidate: brief(candTool.description) });
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
    && report.declaredMissing.length === 0
    && report.metadataDrift.length === 0
    && report.duplicateNames.candidate.length === 0;
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
