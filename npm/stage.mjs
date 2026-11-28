import { copyFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const binarySource = join(here, "..", "target", "release", "hand.exe");
const binaryDest = join(here, "hand.exe");
const manifestSource = join(here, "..", "crates", "browser-engine", "scripts", "chromium-manifest.json");
const manifestDest = join(here, "chromium-manifest.json");

for (const [source, label] of [
  [binarySource, "release binary"],
  [manifestSource, "chromium manifest"],
]) {
  if (!existsSync(source)) {
    console.error(`stage: no ${label} at ${source} — run cargo build --release first and keep crates/browser-engine/scripts/chromium-manifest.json`);
    process.exit(1);
  }
}

copyFileSync(binarySource, binaryDest);
copyFileSync(manifestSource, manifestDest);
console.log(`stage: ${binaryDest} <- ${binarySource}`);
console.log(`stage: ${manifestDest} <- ${manifestSource}`);
