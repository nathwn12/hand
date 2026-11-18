// Catalogue model: the spec's required surface, the normalization applied to a
// captured MCP `tools/list` result, and the hand-namespaced catalogue built
// from the two pinned legacy catalogues.
//
// Normalization is deliberately lossless where the MCP schema allows it: every
// field a server sends is kept, and only the comparison *projection* is narrow
// (name, title, description, inputSchema, outputSchema, annotations). Adding a
// field to a legacy tool is therefore visible as a difference instead of being
// dropped on the floor.
//
// The single exception is declared in `baseline/declared-additions.json`, the
// one machine-readable list of what hand changes on purpose: schema properties
// it adds, and tool descriptions it rewrites. This module is the only reader of
// that file; nothing else hardcodes an allowance.

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

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

/**
 * The declared allowance: the one file that lets a legacy tool differ by hand,
 * and the only place an allowance is written. Schema paths are dotted and
 * relative to the schema object, so a top-level property is `properties.<name>`;
 * a metadata entry is a bare tool name.
 *
 * Nothing here is a permissive mode. `compareCatalogs` requires every listed
 * schema path to be PRESENT in the candidate, removes exactly those paths, and
 * then demands structural equality with the pin for everything else — so an
 * unlisted addition fails, a listed-but-missing (or renamed) addition fails,
 * and a change to a pinned property fails. A declared description must DIFFER
 * from the pin, because a declaration says "this one was rewritten": an
 * identical one is a stale entry, not a satisfied one. `title` has no declared
 * exceptions at all.
 *
 * Justify each entry in docs/COMPATIBILITY.md under "Intentional differences".
 */
export const DECLARED_ADDITIONS_PATH = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "declared-additions.json",
);

/** Schema keys an allowance may name. `annotations` is never allowed to grow. */
const ALLOWANCE_KEYS = ["inputSchema", "outputSchema"];
/** Metadata keys an allowance may name. `title` is pinned with no exceptions. */
export const DECLARABLE_METADATA_KEYS = ["description"];
const METADATA_KEY = "metadata";
const TOP_LEVEL_KEYS = ["$comment", "version", METADATA_KEY, ...ALLOWANCE_KEYS];
const PATH_SEGMENT = /^[A-Za-z0-9_$-]+$/;
const TOOL_NAME = /^[A-Za-z0-9_$-]+$/;

/**
 * Read and validate the allowance file. A malformed file throws at import time
 * rather than silently comparing with no allowance: a broken gate that looks
 * green is worse than a gate that refuses to run.
 */
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
    // An absent key declares nothing; a present one must name at least one tool,
    // so a half-written section cannot read as "declared nothing".
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

/**
 * Declared additions for one tool and schema key. Only the `hand` profile may
 * grow: the `legacy` profile is the re-capture check, where the pinned binary
 * has to emit exactly what is committed, byte for byte.
 */
export function declaredAdditionsFor(name, key, profile = "hand") {
  if (profile !== "hand" || !ALLOWANCE_KEYS.includes(key)) return [];
  return DECLARED_ADDITIONS[key]?.[name] ?? [];
}

/**
 * True when this tool's metadata was declared as deliberately rewritten. Only
 * the `hand` profile reads the file, and only `description` is declarable: a
 * declaration is a rewrite, not a licence to drift, so the caller has to treat
 * an identical value as a stale declaration.
 */
export function declaredMetadataFor(name, key, profile = "hand") {
  if (profile !== "hand" || !DECLARABLE_METADATA_KEYS.includes(key)) return false;
  return DECLARED_ADDITIONS[METADATA_KEY][key].includes(name);
}

/** True when `path` resolves through `schema` to an own key that exists. */
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

/**
 * Remove exactly the declared paths from a clone of the candidate schema so the
 * remainder must equal the pin. A path that does not resolve is left alone: the
 * presence check above is what fails such a candidate, and stripping nothing
 * keeps that failure visible instead of hiding it.
 */
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
    // A declared property can also be listed in `required`; that entry has to go
    // with it, or the strip leaves a name the pin never carried.
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
 * `profile: "hand"` requires the 32 prefixed names, and treats name,
 * inputSchema, outputSchema, annotations, title and description as contract. A
 * `description` difference fails unless the tool is listed under
 * `metadata.description` in `baseline/declared-additions.json`, and a listed
 * tool whose description still equals the pin fails too, because the
 * declaration says it was rewritten. `title` has no declared exceptions.
 *
 * In the `hand` profile only, a body may grow exactly where the same file says
 * it may: each declared path must exist in the candidate, is removed, and
 * everything that remains has to match the pin. An unlisted addition, a listed
 * addition that vanished, and any change to a pinned property all fail.
 */
export function compareCatalogs({ baseline, candidate, profile = "hand" }) {
  const base = indexTools(baseline.tools);
  const cand = indexTools(candidate.tools);
  const report = {
    profile,
    // Metadata has no relaxed mode any more: `title` and every undeclared
    // `description` are contract in both profiles. The field keeps its name
    // because `compare-catalog.mjs` prints its report line from it.
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
      // Presence first. Stripping a path that is not there would hide a renamed
      // or deleted addition behind a passing comparison, because the pin never
      // carried it either.
      const absent = declared.filter((path) => !hasDeclaredPath(candTool[key], path));
      if (absent.length) report.declaredMissing.push({ tool: name, kind: key, paths: absent });
      const stripped = withoutDeclaredPaths(candTool[key], declared);
      if (!deepEqual(baseTool[key], stripped)) {
        report.schemaMismatches.push({ tool: name, kind: key, diffs: diffOf(baseTool[key], stripped) });
      } else if (absent.length === 0) {
        report.declaredAdditions.push({ tool: name, kind: key, paths: declared });
      }
    }
    // `title` and `description` are pinned exactly like a schema. `title` has no
    // declared exceptions at all; a description may be declared as rewritten,
    // and that declaration is a claim that the string DID change.
    if (!deepEqual(baseTool.title, candTool.title)) {
      report.metadataDrift.push({ tool: name, kind: "title", baseline: brief(baseTool.title), candidate: brief(candTool.title) });
    }
    if (declaredMetadataFor(name, "description", profile)) {
      if (deepEqual(baseTool.description, candTool.description)) {
        // Stale declaration: the file says this one was rewritten, and it was not.
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
