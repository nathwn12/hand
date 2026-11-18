// Release-build evidence tests: a real cargo --release build in a temporary
// git repository, then tampering and missing-evidence cases. The tiny crate is
// only a recorder test; it is not a product benchmark.

import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import { join, resolve } from "node:path";

import { hashFile } from "../scripts/lib/fingerprint.mjs";
import { canonicalHash } from "../scripts/lib/fingerprint.mjs";
import { buildComparison } from "../scripts/lib/compare-core.mjs";
import {
  loadBuildReceipt,
  loadBuildReceipts,
  verifyBaselinePin,
  verifyReleaseEvidence,
} from "../scripts/lib/release-evidence.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const RECORDER = join(REPO_ROOT, "benchmarks", "scripts", "record-build.mjs");

async function withTemp(fn) {
  const dir = await mkdtemp(join(os.tmpdir(), "hand-bench-build-"));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function run(program, args, cwd, timeout = 180_000) {
  return spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    timeout,
    windowsHide: true,
    maxBuffer: 64 * 1024 * 1024,
  });
}

function git(crate, args) {
  const result = run("git", args, crate);
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return result.stdout;
}

async function makeCrate(dir) {
  const crate = join(dir, "crate");
  await mkdir(join(crate, "src"), { recursive: true });
  await writeFile(
    join(crate, "Cargo.toml"),
    '[package]\nname = "hand-bench-sample"\nversion = "0.1.0"\nedition = "2021"\n\n[[bin]]\nname = "hand-bench-sample"\npath = "src/main.rs"\n',
    "utf8",
  );
  // The product contract under test: `--version` prints the commit and cargo
  // profile compiled in, so the measured bytes carry their own identity.
  await writeFile(
    join(crate, "build.rs"),
    [
      "fn main() {",
      "    let out = std::process::Command::new(\"git\").args([\"rev-parse\", \"HEAD\"]).output().expect(\"git\");",
      "    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();",
      "    println!(\"cargo:rustc-env=SAMPLE_SHA={sha}\");",
      "    println!(\"cargo:rustc-env=SAMPLE_PROFILE={}\", std::env::var(\"PROFILE\").unwrap_or_default());",
      "}",
      "",
    ].join("\n"),
    "utf8",
  );
  await writeFile(
    join(crate, "src", "main.rs"),
    'fn main() { println!("sample 0.1.0 ({}; {})", env!("SAMPLE_SHA"), env!("SAMPLE_PROFILE")); }\n',
    "utf8",
  );
  await writeFile(join(crate, ".gitignore"), "target/\n", "utf8");
  git(crate, ["init", "-q"]);
  const lock = run("cargo", ["generate-lockfile", "--manifest-path", join(crate, "Cargo.toml")], crate);
  assert.equal(lock.status, 0, lock.stderr || lock.stdout);
  git(crate, ["add", "."]);
  git(crate, [
    "-c",
    "user.name=Bench Test",
    "-c",
    "user.email=bench@example.invalid",
    "commit",
    "-q",
    "-m",
    "initial",
  ]);
  return crate;
}

test("the recorder runs a real cargo --release build and binds its output", async () => {
  await withTemp(async (dir) => {
    const crate = await makeCrate(dir);
    const out = join(dir, "builds");
    const result = run(
      process.execPath,
      [
        RECORDER,
        "--product",
        "hand",
        "--source-root",
        crate,
        "--manifest-path",
        "Cargo.toml",
        "--bin",
        "hand-bench-sample",
        "--target-dir",
        "target",
        "--out",
        out,
      ],
      REPO_ROOT,
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);

    const entries = await loadBuildReceipts(out);
    assert.equal(entries.length, 1);
    const entry = entries[0];
    assert.equal(entry.ok, true, entry.errors?.join("; "));
    assert.equal(entry.build.ok, true);
    assert.equal(entry.build.profile, "release");
    assert.ok(entry.build.command.args.includes("--release"));
    assert.match(entry.build.source.gitHead, /^[0-9a-f]{40}$/);
    assert.equal(entry.build.source.dirty, false);
    assert.ok(entry.build.toolchain.target);
    assert.match(entry.build.output.sha256, /^sha256:[0-9a-f]{64}$/);

    const binary = entry.build.output.path;
    assert.equal((await hashFile(binary)).sha256, entry.build.output.sha256);
    const receipt = {
      product: {
        id: "hand",
        executable: { path: binary, sha256: entry.build.output.sha256 },
        version: { stdout: entry.build.version.stdout },
        releaseIdentity: { verified: false, basis: "binary-path-heuristic" },
      },
    };
    const evidence = await verifyReleaseEvidence({
      receipt,
      currentBinarySha: entry.build.output.sha256,
      pins: null,
      builds: entries,
    });
    assert.equal(evidence.verified, true, evidence.reason);
    assert.equal(evidence.kind, "local-release-build");
  });
});

test("tampered binary, source, receipt or missing evidence is refused", async () => {
  await withTemp(async (dir) => {
    const crate = await makeCrate(dir);
    const out = join(dir, "builds");
    const result = run(
      process.execPath,
      [
        RECORDER,
        "--product",
        "hand",
        "--source-root",
        crate,
        "--manifest-path",
        "Cargo.toml",
        "--bin",
        "hand-bench-sample",
        "--target-dir",
        "target",
        "--out",
        out,
      ],
      REPO_ROOT,
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const entries = await loadBuildReceipts(out);
    const build = entries[0].build;
    const receipt = {
      product: {
        id: "hand",
        executable: { path: build.output.path, sha256: build.output.sha256 },
        version: { stdout: build.version.stdout },
      },
    };

    const tampered = join(dir, "tampered.exe");
    await writeFile(tampered, Buffer.concat([await readFile(build.output.path), Buffer.from("tamper")]));
    const tamperedHash = (await hashFile(tampered)).sha256;
    const tamperedReceipt = {
      product: { ...receipt.product, executable: { path: tampered, sha256: build.output.sha256 } },
    };
    const badBinary = await verifyReleaseEvidence({
      receipt: tamperedReceipt,
      currentBinarySha: tamperedHash,
      pins: null,
      builds: entries,
    });
    assert.equal(badBinary.verified, false);
    assert.match(badBinary.reason, /binary at the measured path is/);

    const receiptFile = join(out, (await readdir(out)).find((name) => name.endsWith(".json")));
    const receiptJson = JSON.parse(await readFile(receiptFile, "utf8"));
    receiptJson.profile = "debug";
    await writeFile(receiptFile, `${JSON.stringify(receiptJson, null, 2)}\n`, "utf8");
    const sidecarBroken = await loadBuildReceipt(receiptFile);
    assert.equal(sidecarBroken.ok, false);
    assert.ok(sidecarBroken.errors.some((error) => error.includes("sidecar hash mismatch")));

    const missing = await verifyReleaseEvidence({
      receipt,
      currentBinarySha: build.output.sha256,
      pins: null,
      builds: [],
    });
    assert.equal(missing.verified, false);
    assert.match(missing.reason, /no local release-build receipts available/);
  });
});

test("post-build source drift is recorded but does not change the built artifact evidence", async () => {
  await withTemp(async (dir) => {
    const crate = await makeCrate(dir);
    const out = join(dir, "builds");
    const result = run(
      process.execPath,
      [
        RECORDER,
        "--product",
        "hand",
        "--source-root",
        crate,
        "--manifest-path",
        "Cargo.toml",
        "--bin",
        "hand-bench-sample",
        "--target-dir",
        "target",
        "--out",
        out,
      ],
      REPO_ROOT,
    );
    assert.equal(result.status, 0, result.stderr || result.stdout);
    const entries = await loadBuildReceipts(out);
    const build = entries[0].build;
    const receipt = {
      product: {
        id: "hand",
        executable: { path: build.output.path, sha256: build.output.sha256 },
        version: { stdout: build.version.stdout },
      },
    };
    await writeFile(join(crate, "src", "main.rs"), 'fn main() { println!("changed"); }\n', "utf8");
    const evidence = await verifyReleaseEvidence({
      receipt,
      currentBinarySha: build.output.sha256,
      pins: null,
      builds: entries,
    });
    assert.equal(evidence.verified, true, evidence.reason);
    assert.equal(evidence.detail.head, build.source.gitHead);
    assert.equal(evidence.detail.dirty, build.source.dirty);
  });
});

test("pinned Gladius and Puppet binaries are accepted as release evidence", async () => {
  const pins = JSON.parse(await readFile(join(REPO_ROOT, "baseline", "pins.json"), "utf8"));
  for (const id of ["gladius", "puppet"]) {
    const source = pins.sources[id];
    const receipt = {
      product: {
        id,
        executable: { path: `Q:\\pinned\\${id}.exe`, sha256: source.capture.capturedExecutableSha256 },
        version: { stdout: `${id} ${source.version} (pinned)` },
      },
    };
    const ok = verifyBaselinePin({ receipt, pins });
    assert.equal(ok.verified, true, ok.reason);
    assert.equal(ok.kind, "baseline-pin");

    const wrong = verifyBaselinePin({
      receipt: {
        product: { ...receipt.product, executable: { path: receipt.product.executable.path, sha256: `sha256:${"0".repeat(64)}` } },
      },
      pins,
    });
    assert.equal(wrong.verified, false);
    assert.match(wrong.reason, /does not match the pinned/);
  }
});

function hash(char) {
  return `sha256:${char.repeat(64)}`;
}

function measuredReceipt({ id, path, sha256, version, paramsHash }) {
  const attempts = Array.from({ length: 100 }, (_, index) => ({
    index: index + 1,
    ok: true,
    elapsedMs: 1,
    dispatched: true,
  }));
  const warmups = Array.from({ length: 10 }, (_, index) => ({
    index: index + 1,
    ok: true,
    elapsedMs: 1,
    dispatched: true,
  }));
  const fixtureFiles = [
    { name: "index.html", bytes: 16374, sha256: hash("d") },
    { name: "actions.html", bytes: 8494, sha256: hash("d") },
    { name: "manifest.json", bytes: 2746, sha256: hash("d") },
  ];
  const fixtureHash = canonicalHash(
    fixtureFiles.map(({ name, bytes, sha256: digest }) => ({ name, bytes, sha256: digest })),
  );
  return {
    product: { id, executable: { path, sha256 }, version: { stdout: version } },
    scenario: { id: "browser-snapshot", paramsHash },
    protocol: {
      warmup: 10,
      measured: 100,
      sequential: true,
      timeoutMs: 60000,
      framing: "newline-delimited JSON-RPC",
      latencyBasis: "stdin write to complete response frame",
    },
    fingerprint: {
      machine: { hash: hash("c") },
      fixture: {
        hash: fixtureHash,
        files: fixtureFiles,
        pin: { entries: Object.fromEntries(fixtureFiles.map((file) => [file.name, file.sha256])) },
      },
      browser: {
        sha256: hash("e"),
        launch: { verified: true, path: "Q:\\browser\\chrome-headless-shell.exe", sha256: hash("e") },
      },
      desktopTarget: { hash: hash("f") },
    },
    stats: { attempted: 100, observed: 100, failed: 0, p50Ms: 1, p95Ms: 1, maxMs: 1 },
    samples: {
      warmup: { attempts: warmups, failures: 0 },
      measured: { attempts, failures: 0 },
    },
    verdict: { status: "pass", targetP95Ms: 250, reasons: [] },
  };
}

test("the gate passes a browser group backed by a real local release build and a baseline pin", async () => {
  await withTemp(async (dir) => {
    const crate = await makeCrate(dir);
    const out = join(dir, "builds");
    const buildRun = run(
      process.execPath,
      [
        RECORDER,
        "--product",
        "hand",
        "--source-root",
        crate,
        "--manifest-path",
        "Cargo.toml",
        "--bin",
        "hand-bench-sample",
        "--target-dir",
        "target",
        "--out",
        out,
      ],
      REPO_ROOT,
    );
    assert.equal(buildRun.status, 0, buildRun.stderr || buildRun.stdout);
    const builds = await loadBuildReceipts(out);
    const build = builds[0].build;
    const pins = JSON.parse(await readFile(join(REPO_ROOT, "baseline", "pins.json"), "utf8"));
    const gladiusPin = pins.sources.gladius;

    const handReceipt = measuredReceipt({
      id: "hand",
      path: build.output.path,
      sha256: build.output.sha256,
      version: build.version.stdout,
      paramsHash: hash("b"),
    });
    const gladiusReceipt = measuredReceipt({
      id: "gladius",
      path: "Q:\\pinned\\target\\release\\gladius.exe",
      sha256: gladiusPin.capture.capturedExecutableSha256,
      version: `gladius ${gladiusPin.version}`,
      paramsHash: hash("b"),
    });

    const handEvidence = await verifyReleaseEvidence({
      receipt: handReceipt,
      currentBinarySha: build.output.sha256,
      pins,
      builds,
    });
    const gladiusEvidence = verifyBaselinePin({ receipt: gladiusReceipt, pins });
    assert.equal(handEvidence.verified, true, handEvidence.reason);
    assert.equal(gladiusEvidence.verified, true, gladiusEvidence.reason);

    const comparison = buildComparison([
      {
        file: "hand.json",
        sha256: hash("1"),
        receipt: handReceipt,
        currentBinarySha: build.output.sha256,
        releaseEvidence: handEvidence,
      },
      {
        file: "gladius.json",
        sha256: hash("1"),
        receipt: gladiusReceipt,
        currentBinarySha: gladiusPin.capture.capturedExecutableSha256,
        releaseEvidence: gladiusEvidence,
      },
    ]);
    const browser = comparison.groups.find((group) => group.scenario === "browser-snapshot");
    assert.equal(browser.allPass, true, browser.differences.join("; "));
  });
});
