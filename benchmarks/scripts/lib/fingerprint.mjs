// Fingerprints for the benchmark receipt: machine, executable, headless browser
// and controlled desktop target. Fixture bytes and their pin live in fixture.mjs.
//
// Nothing here launches a browser, opens a window or takes a screenshot. It
// reads files, asks Windows for the primary display geometry, and matches
// window records returned by the product under test.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { access, readdir, stat } from "node:fs/promises";
import os from "node:os";
import { basename, join } from "node:path";

export function sha256Text(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

export function stableStringify(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableStringify).join(",")}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`).join(",")}}`;
}

export function canonicalHash(value) {
  return `sha256:${sha256Text(stableStringify(value))}`;
}

export async function hashFile(path) {
  const info = await stat(path);
  const hash = createHash("sha256");
  await new Promise((resolve, reject) => {
    const stream = createReadStream(path);
    stream.on("error", reject);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", resolve);
  });
  return { path, bytes: info.size, sha256: `sha256:${hash.digest("hex")}` };
}

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}


let displayProbePromise;

export async function probeDisplay({ enabled = true } = {}) {
  if (!enabled) return { ok: false, skipped: true, reason: "display probe disabled" };
  if (displayProbePromise) return displayProbePromise;
  displayProbePromise = (async () => {
    if (process.platform !== "win32") {
      return { ok: false, reason: "display probe is Windows-only" };
    }
    const script = [
      "$ErrorActionPreference='Stop'",
      "Add-Type -AssemblyName System.Windows.Forms",
      "Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class HBDisplay{[DllImport(\"user32.dll\")]public static extern uint GetDpiForSystem();}'",
      "$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds",
      "$dpi=0",
      "try{$dpi=[int][HBDisplay]::GetDpiForSystem()}catch{}",
      "[Console]::Out.Write(('{{\"width\":{0},\"height\":{1},\"dpi\":{2}}}' -f $b.Width,$b.Height,$dpi))",
    ].join("\n");
    const started = Date.now();
    const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", script], {
      encoding: "utf8",
      timeout: 15_000,
      windowsHide: true,
    });
    if (result.error) return { ok: false, reason: String(result.error), elapsedMs: Date.now() - started };
    if (result.status !== 0) {
      return {
        ok: false,
        reason: `powershell exited ${result.status}: ${(result.stderr || "").trim().slice(0, 400)}`,
        elapsedMs: Date.now() - started,
      };
    }
    try {
      const parsed = JSON.parse(result.stdout);
      return {
        ok: true,
        width: parsed.width,
        height: parsed.height,
        dpi: parsed.dpi || null,
        scaleFactor: parsed.dpi ? Math.round((parsed.dpi / 96) * 100) / 100 : null,
        elapsedMs: Date.now() - started,
      };
    } catch {
      return { ok: false, reason: `unparseable display probe output: ${result.stdout.slice(0, 200)}` };
    }
  })();
  return displayProbePromise;
}

export function sampleCpu() {
  const cpus = os.cpus();
  return {
    at: new Date().toISOString(),
    idle: cpus.reduce((sum, cpu) => sum + cpu.times.idle, 0),
    total: cpus.reduce(
      (sum, cpu) => sum + cpu.times.user + cpu.times.nice + cpu.times.sys + cpu.times.idle + cpu.times.irq,
      0,
    ),
    logicalCores: cpus.length,
  };
}

export function cpuBusyPercent(before, after) {
  if (!before || !after || before.logicalCores !== after.logicalCores) return null;
  const idle = after.idle - before.idle;
  const total = after.total - before.total;
  if (total <= 0) return null;
  return Math.round((1 - idle / total) * 10_000) / 100;
}

export async function machineFingerprint({ displayProbeEnabled = true } = {}) {
  const cpus = os.cpus();
  const display = await probeDisplay({ enabled: displayProbeEnabled });
  const machine = {
    hostname: os.hostname(),
    platform: os.platform(),
    release: os.release(),
    version: os.version(),
    arch: os.arch(),
    cpu: {
      model: cpus[0]?.model?.trim() ?? null,
      logicalCores: cpus.length,
      speedMHz: cpus[0]?.speed ?? null,
    },
    memoryTotalBytes: os.totalmem(),
    display: display.ok
      ? { width: display.width, height: display.height, dpi: display.dpi, scaleFactor: display.scaleFactor }
      : null,
    displayProbe: display.ok ? { ok: true } : { ok: false, reason: display.reason ?? "unknown" },
    node: { version: process.version, arch: process.arch, execPath: process.execPath },
  };
  return {
    ...machine,
    hash: canonicalHash({
      hostname: machine.hostname,
      platform: machine.platform,
      release: machine.release,
      arch: machine.arch,
      cpu: machine.cpu,
      memoryTotalBytes: machine.memoryTotalBytes,
      display: machine.display,
    }),
  };
}


export async function fingerprintExecutable({ command, versionArgs = ["--version"], runVersion = true }) {
  if (!(await exists(command))) {
    return { ok: false, reason: `executable not found: ${command}` };
  }
  const file = await hashFile(command);
  let version = null;
  if (runVersion) {
    try {
      const stdout = execFileSync(command, versionArgs, {
        encoding: "utf8",
        timeout: 15_000,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      });
      version = { status: 0, stdout: stdout.trim(), stderr: "" };
    } catch (error) {
      version = {
        status: typeof error.status === "number" ? error.status : null,
        stdout: String(error.stdout ?? "").trim().slice(0, 2000),
        stderr: String(error.stderr ?? error.message ?? "").trim().slice(0, 2000),
      };
    }
  }
  return { ok: true, executable: file, version };
}


function versionKey(name) {
  const match = /(\d+)\.(\d+)\.(\d+)\.(\d+)/.exec(name);
  if (!match) return [0, 0, 0, 0];
  return match.slice(1).map(Number);
}

function compareVersionDirs(a, b) {
  const left = versionKey(b);
  const right = versionKey(a);
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const delta = (left[index] ?? 0) - (right[index] ?? 0);
    if (delta !== 0) return delta;
  }
  return 0;
}

async function findHeadlessShellUnder(root, depth = 0) {
  if (depth > 3) return [];
  let entries;
  try {
    entries = await readdir(root, { withFileTypes: true });
  } catch {
    return [];
  }
  const found = [];
  for (const entry of entries) {
    const path = join(root, entry.name);
    if (entry.isFile() && entry.name.toLowerCase() === "chrome-headless-shell.exe") {
      found.push(path);
      continue;
    }
    if (entry.isDirectory()) {
      if (entry.name === "chrome-headless-shell-win64") {
        const candidate = join(path, "chrome-headless-shell.exe");
        if (await exists(candidate)) found.push(candidate);
        continue;
      }
      found.push(...(await findHeadlessShellUnder(path, depth + 1)));
    }
  }
  return found;
}

export function cachesForBrowser() {
  const localAppData = process.env.LOCALAPPDATA;
  const roots = [];
  if (process.env.GLADIUS_CHROMIUM_CACHE) roots.push(process.env.GLADIUS_CHROMIUM_CACHE);
  if (localAppData) roots.push(join(localAppData, "gladius", "chromium"));
  return roots;
}

export async function browserFingerprint({ explicitPath, cacheRoots = cachesForBrowser() } = {}) {
  if (explicitPath) {
    if (basename(explicitPath).toLowerCase() !== "chrome-headless-shell.exe") {
      return {
        ok: false,
        reason:
          "refusing a non-headless-shell browser: the benchmark runs only a pinned chrome-headless-shell, never a personal browser",
      };
    }
    const file = await hashFile(explicitPath);
    return {
      ok: true,
      browser: {
        ...file,
        role: "chrome-headless-shell",
        source: "explicit --browser-exe",
        version: basename(join(explicitPath, "..", "..")),
      },
    };
  }

  const candidates = [];
  for (const root of cacheRoots) {
    for (const path of await findHeadlessShellUnder(root)) {
      candidates.push({ path, versionDir: basename(join(path, "..", "..")) });
    }
  }
  if (candidates.length === 0) {
    return {
      ok: false,
      reason: `no chrome-headless-shell.exe under ${cacheRoots.join(", ") || "(no cache roots)"}; pass --browser-exe to freeze the digest`,
    };
  }
  candidates.sort((a, b) => compareVersionDirs(a.versionDir, b.versionDir));
  const chosen = candidates[0];
  const file = await hashFile(chosen.path);
  return {
    ok: true,
    browser: {
      ...file,
      role: "chrome-headless-shell",
      source: "cache discovery",
      version: chosen.versionDir,
    },
  };
}


const TARGET_KEYS = new Set(["titleContains", "exe", "className", "hwnd", "pid"]);

export function normalizeDesktopTarget(input) {
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return { ok: false, reason: "desktop target must be a JSON object" };
  }
  const unknown = Object.keys(input).filter((key) => !TARGET_KEYS.has(key));
  if (unknown.length) {
    return { ok: false, reason: `desktop target has unknown keys: ${unknown.join(", ")}` };
  }
  const target = {};
  for (const key of ["titleContains", "exe", "className"]) {
    if (input[key] !== undefined) {
      if (typeof input[key] !== "string" || input[key].trim() === "") {
        return { ok: false, reason: `desktop target ${key} must be a non-empty string` };
      }
      target[key] = input[key].trim();
    }
  }
  for (const key of ["hwnd", "pid"]) {
    if (input[key] !== undefined) {
      if (!Number.isInteger(input[key]) || input[key] <= 0) {
        return { ok: false, reason: `desktop target ${key} must be a positive integer` };
      }
      target[key] = input[key];
    }
  }
  if (Object.keys(target).length === 0) {
    return { ok: false, reason: "desktop target must set at least one matcher" };
  }
  return { ok: true, target };
}

export function matchWindows(windows, target) {
  const list = Array.isArray(windows) ? windows : [];
  return list.filter((window) => {
    if (!window || typeof window !== "object") return false;
    if (target.titleContains) {
      const title = String(window.title ?? "").toLowerCase();
      if (!title.includes(target.titleContains.toLowerCase())) return false;
    }
    if (target.exe) {
      const exe = String(window.exe ?? "").toLowerCase();
      const wanted = target.exe.toLowerCase();
      if (exe !== wanted && basename(exe) !== basename(wanted)) return false;
    }
    if (target.className && String(window.class ?? "").toLowerCase() !== target.className.toLowerCase()) return false;
    if (target.hwnd !== undefined && window.hwnd !== target.hwnd) return false;
    if (target.pid !== undefined && window.pid !== target.pid) return false;
    return true;
  });
}

export function desktopTargetFingerprint(windows, target) {
  const matches = matchWindows(windows, target);
  if (matches.length === 0) {
    return { ok: false, reason: `no visible window matches ${JSON.stringify(target)}` };
  }
  if (matches.length > 1) {
    return {
      ok: false,
      reason: `${matches.length} windows match ${JSON.stringify(target)}; the fixture target must be unambiguous`,
      hwnds: matches.map((window) => window.hwnd),
    };
  }
  const window = matches[0];
  const identity = {
    title: window.title ?? null,
    exe: window.exe ?? null,
    class: window.class ?? null,
    clientRect: window.client_rect ?? null,
    rect: window.rect ?? null,
    dpi: window.dpi ?? null,
    minimized: window.minimized ?? null,
  };
  const stableIdentity = {
    title: identity.title,
    exe: identity.exe,
    class: identity.class,
    dpi: identity.dpi,
    minimized: identity.minimized,
    clientWidth: Array.isArray(identity.clientRect) ? identity.clientRect[2] - identity.clientRect[0] : null,
    clientHeight: Array.isArray(identity.clientRect) ? identity.clientRect[3] - identity.clientRect[1] : null,
  };
  return {
    ok: true,
    matchedWindow: {
      hwnd: window.hwnd ?? null,
      pid: window.pid ?? null,
      ...identity,
    },
    identity,
    hash: canonicalHash(stableIdentity),
  };
}
