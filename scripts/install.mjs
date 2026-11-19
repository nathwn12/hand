#!/usr/bin/env node
import { copyFileSync, existsSync, renameSync, rmSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const argv = process.argv.slice(2);
const value = (flag) => {
  const at = argv.indexOf(flag);
  return at === -1 ? undefined : argv[at + 1];
};

const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
const binName = (manifest.split(/^\[\[bin\]\]$/m)[1] ?? "").match(/^\s*name\s*=\s*"([^"]+)"/m)?.[1];
if (!binName) {
  console.log("FAIL install: Cargo.toml declares no [[bin]] name");
  process.exit(1);
}

const source = resolve(value("--source") ?? join(root, "target", "release", `${binName}.exe`));
const dest = resolve(value("--dest") ?? join(homedir(), ".local", "bin", `${binName}.exe`));
if (!existsSync(source)) {
  console.log(`FAIL install: no built binary at ${source} — run cargo build --release`);
  process.exit(1);
}

const stamp = execFileSync(source, ["--version"], { encoding: "utf8" }).trim();
if (/-dirty\)$/.test(stamp)) {
  console.log(`FAIL install: refusing to install a dirty build: ${stamp}`);
  process.exit(1);
}

if (existsSync(dest)) {
  const previous = `${dest}.prev`;
  try {
    rmSync(previous, { force: true });
  } catch {
  }
  renameSync(dest, previous);
  console.log(`kept the previous binary at ${previous}`);
}

copyFileSync(source, dest);
console.log(`PASS install: ${dest} now holds ${stamp} (${statSync(dest).size} bytes)`);
console.log("note: a running process keeps the previous image until the client reloads; this run killed nothing");
