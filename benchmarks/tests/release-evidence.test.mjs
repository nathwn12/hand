
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
import * as release from "../../npm/release.mjs";

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

// Offline release-identity fixtures. No npm publish, no git tag, no push, no
// remote mutation: the helpers take the command result as an injected function.
const shaOf = (char) => char.repeat(40);
const SOURCE = shaOf("a");
const OTHER = shaOf("b");

function viewPresent(gitHead) {
  return () => JSON.stringify([{ version: "9.9.9", gitHead }]);
}
function viewAbsent() {
  return () => {
    const error = new Error("No matching version found for hand-mcp@9.9.9");
    error.stderr = "npm ERR! code E404";
    throw error;
  };
}
function viewUnavailable() {
  return () => {
    const error = new Error("getaddrinfo ENOTFOUND registry.npmjs.org");
    error.stderr = "npm ERR! code ENOTFOUND";
    throw error;
  };
}
function tagPresent(commit) {
  return () => `${shaOf("c")}\trefs/tags/v9.9.9\n${commit}\trefs/tags/v9.9.9^{}\n`;
}
const tagAbsent = () => "";

test("registry lookup separates npm unavailable, no version and a published source", () => {
  assert.equal(release.registryVersion(viewAbsent(), "hand-mcp", "9.9.9").status, "absent");
  const down = release.registryVersion(viewUnavailable(), "hand-mcp", "9.9.9");
  assert.equal(down.status, "unavailable");
  const present = release.registryVersion(viewPresent(SOURCE.toUpperCase()), "hand-mcp", "9.9.9");
  assert.equal(present.status, "present");
  assert.equal(present.gitHead, SOURCE);
});

test("an annotated tag resolves to its dereferenced commit", () => {
  let asked = null;
  const found = release.remoteTagCommit((args) => {
    asked = args;
    return tagPresent(OTHER)();
  }, "v9.9.9");
  assert.deepEqual(asked, ["ls-remote", "--tags", "origin", "refs/tags/v9.9.9*"]);
  assert.equal(found.status, "present");
  assert.equal(found.commit, OTHER, "the tag object id is not the source commit");
  assert.equal(release.remoteTagCommit(tagAbsent, "v9.9.9").status, "absent");
});

test("identity decision uses registry source and refuses ordinary mismatches", () => {
  const base = { version: "9.9.9", head: SOURCE };
  const publish = release.releaseIdentityDecision({
    ...base,
    registry: { status: "absent" },
    tag: { status: "absent" },
  });
  assert.equal(publish.action, "publish");
  assert.equal(publish.sourceCommit, SOURCE);
  assert.equal(publish.tagMode, "tag");

  const skip = release.releaseIdentityDecision({
    ...base,
    registry: { status: "present", gitHead: SOURCE },
    tag: { status: "absent" },
  });
  assert.equal(skip.action, "skip");
  assert.equal(skip.sourceCommit, SOURCE);

  const laterHead = release.releaseIdentityDecision({
    ...base,
    head: OTHER,
    registry: { status: "present", gitHead: SOURCE },
    tag: { status: "absent" },
  });
  assert.equal(laterHead.action, "skip");
  assert.equal(laterHead.sourceCommit, SOURCE, "a later push HEAD is never tagged");
  assert.equal(laterHead.tagCommit, SOURCE);

  const tagVsRegistry = release.releaseIdentityDecision({
    ...base,
    registry: { status: "present", gitHead: SOURCE },
    tag: { status: "present", commit: OTHER },
  });
  assert.equal(tagVsRegistry.action, "fail");
  assert.match(tagVsRegistry.reason, /immutable tag v9\.9\.9/);

  const tagConflict = release.releaseIdentityDecision({
    ...base,
    registry: { status: "absent" },
    tag: { status: "present", commit: OTHER },
  });
  assert.equal(tagConflict.action, "fail");
  assert.match(tagConflict.reason, /immutable tag v9\.9\.9/);

  const noRegistry = release.releaseIdentityDecision({
    ...base,
    registry: { status: "unavailable", reason: "ENOTFOUND" },
    tag: { status: "absent" },
  });
  assert.equal(noRegistry.action, "fail");
  assert.match(noRegistry.reason, /registry lookup failed/);

  const noTag = release.releaseIdentityDecision({
    ...base,
    registry: { status: "absent" },
    tag: { status: "unavailable", reason: "no origin" },
  });
  assert.equal(noTag.action, "fail");
  assert.match(noTag.reason, /tag lookup failed/);
});

test("the historical exception is pinned to one version and commit pair", () => {
  const pinned = release.HISTORICAL_RELEASE_IDENTITY["0.3.0"];
  const historical = release.releaseIdentityDecision({
    version: "0.3.0",
    head: shaOf("d"),
    registry: { status: "present", gitHead: pinned.sourceCommit },
    tag: { status: "present", commit: pinned.tagCommit },
  });
  assert.equal(historical.action, "skip");
  assert.equal(historical.tagMode, "leave");
  assert.equal(historical.tagCommit, pinned.tagCommit);

  const wrongTag = release.releaseIdentityDecision({
    version: "0.3.0",
    head: shaOf("d"),
    registry: { status: "present", gitHead: pinned.sourceCommit },
    tag: { status: "present", commit: OTHER },
  });
  assert.equal(wrongTag.action, "fail");

  const wrongVersion = release.releaseIdentityDecision({
    version: "0.3.1",
    head: shaOf("d"),
    registry: { status: "present", gitHead: pinned.sourceCommit },
    tag: { status: "present", commit: pinned.tagCommit },
  });
  assert.equal(wrongVersion.action, "fail");
});

test("release identity is attested only when embedded commit matches the receipt", async () => {
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
        releaseIdentity: { verified: false, basis: "embedded-build-metadata" },
      },
    };
    const evidence = await verifyReleaseEvidence({
      receipt,
      currentBinarySha: build.output.sha256,
      pins: null,
      builds: entries,
    });
    assert.equal(evidence.verified, true, evidence.reason);
    assert.equal(evidence.releaseIdentity.verified, true, evidence.releaseIdentity.reason);
    assert.equal(evidence.releaseIdentity.basis, "local-release-build-attestation");
    assert.equal(evidence.releaseIdentity.head, build.source.gitHead);

    const mismatch = await verifyReleaseEvidence({
      receipt: {
        product: {
          ...receipt.product,
          version: { stdout: `hand 9.9.9 (${OTHER}; release)` },
        },
      },
      currentBinarySha: build.output.sha256,
      pins: null,
      builds: entries,
    });
    assert.equal(mismatch.verified, false);
    assert.match(mismatch.reason, /the binary embeds commit bbbbbbbb/);
  });
});

// --- Authoritative post-publish read-back (release propagation) -------------
// GitHub 45097306460 attempt1: publish accepted with provenance, but the cached
// `npm view ... version --prefer-online` packument still 404'd for 90s, so the
// step exited 2 and the tag was held until attempt2 tagged the exact gitHead.
// These fixtures inject fetch and sleep: no publish, no tag, no real waiting.
function versionDocResponse(status, doc) {
  return {
    status,
    ok: status >= 200 && status < 300,
    text: async () => (typeof doc === "string" ? doc : JSON.stringify(doc)),
  };
}

test("authoritative read targets the write path and returns a typed source", async () => {
  let call = null;
  const got = await release.authoritativeVersion(
    async (url, opts) => {
      call = { url, opts };
      return versionDocResponse(200, { version: "9.9.9", gitHead: SOURCE.toUpperCase() });
    },
    "hand-mcp",
    "9.9.9",
  );
  assert.equal(call.url, "https://registry.npmjs.org/hand-mcp/9.9.9?write=true");
  assert.equal(call.opts.headers["cache-control"], "no-cache");
  assert.equal(got.status, "present");
  assert.equal(got.gitHead, SOURCE, "gitHead is lower-cased for comparison");
});

test("authoritative read separates absent, server error and network failure", async () => {
  const absent = await release.authoritativeVersion(
    async () => versionDocResponse(404, ""),
    "hand-mcp",
    "9.9.9",
  );
  assert.equal(absent.status, "absent");
  const serverError = await release.authoritativeVersion(
    async () => versionDocResponse(503, "unavailable"),
    "hand-mcp",
    "9.9.9",
  );
  assert.equal(serverError.status, "unavailable");
  const offline = await release.authoritativeVersion(
    async () => { throw new Error("getaddrinfo ENOTFOUND registry.npmjs.org"); },
    "hand-mcp",
    "9.9.9",
  );
  assert.equal(offline.status, "unavailable");
  assert.match(offline.reason, /ENOTFOUND/);
});

test("delayed visibility resolves through authoritative reads but strands the cached view", async () => {
  // The old reader: a cached packument that still 404s for the whole window.
  const staleView = () => {
    const error = new Error("No matching version found for hand-mcp@9.9.9");
    error.stderr = "npm ERR! code E404";
    throw error;
  };
  const legacy = await release.waitForPublishedVersion({
    read: () => release.registryVersion(staleView, "hand-mcp", "9.9.9"),
    expected: SOURCE,
    attempts: 18,
    delayMs: 5000,
    sleep: async () => {},
  });
  assert.equal(legacy.kind, "pending", "the cached view strands an accepted publish for 90s");
  assert.equal(legacy.reads, 18);

  let reads = 0;
  const authoritative = await release.waitForPublishedVersion({
    read: async () => {
      reads += 1;
      return reads < 4
        ? { status: "absent", reason: "not replicated yet" }
        : { status: "present", gitHead: SOURCE, version: "9.9.9" };
    },
    expected: SOURCE,
    attempts: 18,
    delayMs: 5000,
    sleep: async () => {},
  });
  assert.equal(authoritative.kind, "verified");
  assert.equal(authoritative.reads, 4, "bounded eventual consistency resolves at the fourth authoritative read");
  assert.equal(authoritative.gitHead, SOURCE);
});

test("a published version from another commit is a conflict, never a tag", async () => {
  const verdict = await release.waitForPublishedVersion({
    read: async () => ({ status: "present", gitHead: OTHER, version: "9.9.9" }),
    expected: SOURCE,
    attempts: 5,
    delayMs: 1,
    sleep: async () => {},
  });
  assert.equal(verdict.kind, "conflict");
  assert.equal(verdict.reads, 1, "a terminal conflict stops the loop before any tag");
  assert.equal(verdict.gitHead, OTHER);
  assert.match(verdict.reason, /refusing to tag/);
  const conflict = release.classifyPublishedVersion({
    observed: { status: "present", gitHead: OTHER, version: "9.9.9" },
    expected: SOURCE,
  });
  assert.equal(conflict.kind, "conflict");
  assert.equal(conflict.gitHead, OTHER);
});

test("unavailable authoritative reads stay pending and never authorise a tag", async () => {
  const verdict = await release.waitForPublishedVersion({
    read: async () => ({ status: "unavailable", reason: "ENOTFOUND" }),
    expected: SOURCE,
    attempts: 3,
    delayMs: 1,
    sleep: async () => {},
  });
  assert.equal(verdict.kind, "pending");
  assert.equal(verdict.status, "unavailable");
  assert.equal(verdict.gitHead, undefined);
  assert.match(verdict.reason, /ENOTFOUND/);
  assert.equal(
    release.classifyPublishedVersion({ observed: { status: "absent", reason: "404" }, expected: SOURCE }).kind,
    "pending",
  );
});

test("a pending publish resumes on retry with a skip at the verified gitHead", async () => {
  const pending = await release.waitForPublishedVersion({
    read: async () => ({ status: "absent", reason: "not replicated yet" }),
    expected: SOURCE,
    attempts: 2,
    delayMs: 1,
    sleep: async () => {},
  });
  assert.equal(pending.kind, "pending");

  // Retry: the authoritative pre-publish read now sees the published source.
  const registry = await release.authoritativeVersion(
    async () => versionDocResponse(200, { version: "9.9.9", gitHead: SOURCE }),
    "hand-mcp",
    "9.9.9",
  );
  assert.equal(registry.status, "present");
  const decision = release.releaseIdentityDecision({
    version: "9.9.9",
    head: OTHER, // a later push HEAD must never be tagged
    registry,
    tag: { status: "absent" },
  });
  assert.equal(decision.action, "skip");
  assert.equal(decision.sourceCommit, SOURCE);
  assert.equal(decision.tagCommit, SOURCE);
});
