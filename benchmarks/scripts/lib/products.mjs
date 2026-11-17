// Product resolution. A benchmark never guesses: a missing executable is
// "unsupported" with the paths that were tried, and the driver does not build
// anything on the owner's behalf.

import { access, stat } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

import { PRODUCT_DEFINITIONS } from "./scenarios.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..", "..");

export const PRODUCT_CANDIDATES = {
  hand: {
    env: "HAND_MCP_BIN",
    paths: [
      join(REPO_ROOT, "target", "release", "hand.exe"),
      join(REPO_ROOT, "target", "debug", "hand.exe"),
    ],
  },
  gladius: {
    env: "GLADIUS_BIN",
    paths: [
      "Q:\\PROJECTS\\PERSONAL\\gladius\\target\\release\\gladius.exe",
      join(REPO_ROOT, "baseline", "_work", "gladius-59c6d77", "target", "release", "gladius.exe"),
    ],
  },
  puppet: {
    env: "PUPPET_BIN",
    paths: [
      "Q:\\PROJECTS\\PERSONAL\\puppet\\target\\release\\puppet.exe",
      join(REPO_ROOT, "baseline", "_work", "puppet-0f790be", "target", "release", "puppet.exe"),
    ],
  },
};

async function exists(path) {
  try {
    await access(path);
    const info = await stat(path);
    return info.isFile();
  } catch {
    return false;
  }
}

async function resolveExecutable(name) {
  if (/[\\/]/.test(name)) return resolve(name);
  const extensions =
    process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";") : [""];
  for (const directory of (process.env.PATH ?? "").split(process.platform === "win32" ? ";" : ":")) {
    if (!directory) continue;
    for (const extension of extensions) {
      const candidate = resolve(directory, `${name}${extension}`);
      if (await exists(candidate)) return candidate;
    }
  }
  return resolve(name);
}

export async function resolveProduct({ productId, command, args = [], cwd, testFixture = false }) {
  const definition = PRODUCT_DEFINITIONS[productId];
  if (!definition) {
    return { ok: false, usage: true, reason: `unknown product: ${productId}` };
  }

  if (command) {
    const resolved = await resolveExecutable(command);
    if (!(await exists(resolved))) {
      return {
        ok: false,
        unsupported: true,
        reason: `requested command not found: ${resolved}`,
        tried: [resolved],
      };
    }
    if (testFixture) {
      const entrypoint = args[0] ? resolve(args[0]) : null;
      if (!entrypoint || !(await exists(entrypoint))) {
        return {
          ok: false,
          usage: true,
          reason: "--test-fixture requires --arg <path to the fake MCP server script>",
        };
      }
      if (!basename(entrypoint).includes("fake-mcp-server")) {
        return {
          ok: false,
          usage: true,
          reason: `--test-fixture must point at the deterministic fake server (got ${basename(entrypoint)})`,
        };
      }
    } else if (args.some((arg) => basename(String(arg)).includes("fake-mcp-server"))) {
      return {
        ok: false,
        usage: true,
        reason: "the deterministic fake MCP fixture requires --test-fixture; a test run is never live evidence",
      };
    }
    return {
      ok: true,
      product: {
        id: productId,
        label: definition.label,
        command: resolved,
        args,
        cwd: cwd ?? REPO_ROOT,
        source: testFixture ? "driver test fixture" : "explicit --command",
        testFixture: Boolean(testFixture),
      },
      tried: [resolved],
    };
  }

  const candidates = [];
  const envName = PRODUCT_CANDIDATES[productId]?.env;
  if (envName && process.env[envName]) candidates.push(resolve(process.env[envName]));
  for (const path of PRODUCT_CANDIDATES[productId]?.paths ?? []) candidates.push(resolve(path));

  for (const candidate of candidates) {
    if (await exists(candidate)) {
      return {
        ok: true,
        product: {
          id: productId,
          label: definition.label,
          command: candidate,
          args: [],
          cwd: REPO_ROOT,
          source: candidate === resolve(process.env[envName] ?? "") ? `$${envName}` : "known build path",
          testFixture: false,
        },
        tried: candidates,
      };
    }
  }

  return {
    ok: false,
    unsupported: true,
    reason: `no ${definition.label} binary found; build it or pass --command. Tried: ${candidates.join(", ") || "(no candidates)"}`,
    tried: candidates,
  };
}
