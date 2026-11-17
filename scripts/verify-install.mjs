#!/usr/bin/env node
/**
 * Does the installed server match this checkout?
 *
 * `oc-hand` runs `C:\Users\nathan\.local\bin\hand-mcp.exe`, which is whatever was
 * last copied there. That copy silently drifts from the branch, and a live probe
 * then answers from old code while the suite is green on new code — observed once
 * during selector work. This compares the stamp the binary prints with HEAD.
 *
 * Run: node scripts/verify-install.mjs [--installed <path>]
 * Exits 1 when the installed binary is missing, unstamped, dirty, or built from a
 * different commit than HEAD.
 */
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const arg = process.argv.indexOf("--installed");
const installed =
  arg !== -1 ? process.argv[arg + 1] : join(homedir(), ".local", "bin", "hand-mcp.exe");

const git = (args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

if (!existsSync(installed)) {
  console.log(`FAIL install: no binary at ${installed}`);
  process.exit(1);
}
const version = execFileSync(installed, ["--version"], { encoding: "utf8" }).trim();
const stamp = version.match(/\(([0-9a-f]{7,40})(-dirty)?\)/);
if (!stamp) {
  console.log(`FAIL install: ${installed} prints no commit stamp: "${version}"`);
  process.exit(1);
}
const [, sha, dirty] = stamp;
const head = git(["rev-parse", "HEAD"]);
const match = head.startsWith(sha) || sha.startsWith(head.slice(0, sha.length));
if (!match) {
  console.log(`FAIL install: installed ${sha}, HEAD ${head.slice(0, 12)} — reinstall before trusting a live probe`);
  process.exit(1);
}
if (dirty) {
  console.log(`FAIL install: installed build came from a dirty tree (${sha}-dirty)`);
  process.exit(1);
}
const status = git(["status", "--porcelain"]);
console.log(`PASS install: ${installed} is ${sha.slice(0, 12)}, same commit as HEAD`);
if (status) console.log("note: the working tree has uncommitted changes; the binary is not built from them");
