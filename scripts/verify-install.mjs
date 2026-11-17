#!/usr/bin/env node
/**
 * Two proofs, because a fresh install is not a running server.
 *
 * Memory 04.05: installed bytes and the loaded process are separate lifetimes.
 * Memory 04.43: prove an artifact from its content, never from its file name.
 *
 * `oc-hand` runs whatever binary sits at the configured path, and that copy has
 * silently drifted from the branch before: a live probe answered from old code
 * while the suite was green on new code. So this asserts, in order:
 *
 *   1. the installed binary exists and prints a commit stamp (not "no stamp");
 *   2. that stamp is HEAD, undirty;
 *   3. with `--loaded`, a process is actually running *that* file, started at or
 *      after it was last written — bytes on disk are not a loaded image.
 *
 * The binary name is read from Cargo.toml, so renaming the crate cannot leave a
 * stale default path behind here.
 *
 * Run: node scripts/verify-install.mjs [--installed <path>] [--loaded]
 * Exits 1 on either unproven claim.
 */
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const argv = process.argv.slice(2);
const flag = (name) => argv.includes(name);
const value = (name) => {
  const at = argv.indexOf(name);
  return at === -1 ? undefined : argv[at + 1];
};

const fail = (message) => {
  console.log(`FAIL install: ${message}`);
  process.exit(1);
};

const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
const binBlock = manifest.split(/^\[\[bin\]\]$/m)[1] ?? "";
const binName = binBlock.match(/^\s*name\s*=\s*"([^"]+)"/m)?.[1];
if (!binName) fail("Cargo.toml declares no [[bin]] name, so the installed path is unknown");

const installed = resolve(
  value("--installed") ?? join(homedir(), ".local", "bin", `${binName}.exe`),
);
if (!existsSync(installed)) fail(`no binary at ${installed}`);

const git = (args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

/** The commits that can change the binary. A docs-only HEAD must not force a reinstall. */
const BUILD_INPUTS = ["src", "crates", "build.rs", "Cargo.toml", "Cargo.lock"];

// 1+2: the bytes on disk say which commit they are.
const version = execFileSync(installed, ["--version"], { encoding: "utf8" }).trim();
const stamp = version.match(/\(([0-9a-f]{7,40})(-dirty)?[,)]/);
if (!stamp) fail(`${installed} prints no commit stamp: "${version}"`);
const [, sha, dirty] = stamp;
const head = git(["rev-parse", "HEAD"]);
const sameCommit = head.startsWith(sha) || sha.startsWith(head.slice(0, sha.length));
if (!sameCommit) {
  // The stamp records HEAD at build time. That is fine as long as the binary's
  // inputs have not moved since: no build-input file may differ in between.
  let ancestor = true;
  try {
    git(["merge-base", "--is-ancestor", sha, "HEAD"]);
  } catch {
    ancestor = false;
  }
  if (!ancestor) {
    fail(`installed ${sha} is not in this branch's history, HEAD ${head.slice(0, 12)} — rebuild and install`);
  }
  const changed = git(["diff", "--name-only", `${sha}..HEAD`, "--", ...BUILD_INPUTS]);
  if (changed) {
    fail(`installed ${sha} predates build-input changes: ${changed.split("\n").slice(0, 3).join(", ")} — rebuild and install`);
  }
}
if (dirty) fail(`installed build came from a dirty tree (${sha}-dirty)`);
console.log(
  `PASS install bytes: ${installed} is ${sha.slice(0, 12)}${sameCommit ? ", HEAD" : ", with no build input changed since"}`,
);

// 3: the loaded process, proven by image path and start time.
if (flag("--loaded")) {
  const written = statSync(installed).mtimeMs;
  const query = [
    "-NoProfile",
    "-Command",
    `Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -eq '${installed.replace(/'/g, "''")}' } |` +
      " Select-Object ProcessId,ExecutablePath,CreationDate | ConvertTo-Json -Compress",
  ];
  let rows;
  try {
    const out = execFileSync("pwsh", query, { encoding: "utf8" }).trim();
    rows = out ? [JSON.parse(out)].flat() : [];
  } catch (error) {
    fail(`could not enumerate processes: ${String(error.message ?? error).slice(0, 200)}`);
  }
  if (rows.length === 0) {
    fail(`nothing is running ${installed} — installed, not loaded; restart the OpenCode service`);
  }
  const started = (row) => {
    // ConvertTo-Json renders /Date(ms)/ on Windows PowerShell and ISO on pwsh 7.
    const raw = row.CreationDate;
    if (typeof raw === "number") return raw;
    const fromDate = String(raw).match(/\/Date\((\d+)\)\//);
    if (fromDate) return Number(fromDate[1]);
    return Date.parse(String(raw));
  };
  const newest = rows.map(started).sort((a, b) => b - a)[0];
  if (!Number.isFinite(newest)) fail(`cannot read the process start time: ${JSON.stringify(rows[0])}`);
  if (newest < written) {
    fail(`a process is running the old image: started ${new Date(newest).toISOString()}, file written ${new Date(written).toISOString()}`);
  }
  console.log(`PASS loaded process: ${rows.length} process(es) running that file, started after it was written`);
}

if (git(["status", "--porcelain"])) {
  console.log("note: the working tree has uncommitted changes; the binary is not built from them");
}
