// stage.mjs — put the release binary into the package before `npm pack`.
// The tarball ships the executable, so it must be the built one, never a stale
// copy and never a debug build.
import { copyFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, "..", "target", "release", "hand.exe");
const dest = join(here, "hand.exe");

if (!existsSync(source)) {
  console.error(`stage: no release binary at ${source} — run cargo build --release first`);
  process.exit(1);
}
copyFileSync(source, dest);
console.log(`stage: ${dest} <- ${source}`);
