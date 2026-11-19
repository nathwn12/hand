
import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import { join, resolve } from "node:path";

import { loadFixture, startFixtureServer, FIXTURE_FILES } from "../scripts/lib/fixture.mjs";

const REPO_ROOT = resolve(import.meta.dirname, "..", "..");
const FIXTURES = join(REPO_ROOT, "fixtures");

function sha(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

async function withTemp(fn) {
  const dir = await mkdtemp(join(os.tmpdir(), "hand-bench-fixture-"));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

async function copyFixtures(dir) {
  const target = join(dir, "fixtures");
  await cp(FIXTURES, target, { recursive: true });
  return target;
}

async function replacePin(dir, name, body) {
  const path = join(dir, "SHA256SUMS");
  const text = await readFile(path, "utf8");
  const lines = text.split(/\r?\n/).map((line) => {
    const match = /^([0-9a-f]{64})\s+\*?(.+?)\s*$/.exec(line.trim());
    return match && match[2] === name ? `${sha(body)}  ${name}` : line;
  });
  await writeFile(path, `${lines.join("\n")}`, "utf8");
}

test("the repository fixture loads with its SHA256SUMS pin and exactly 100 labels", async () => {
  const fixture = await loadFixture(FIXTURES);
  assert.equal(fixture.ok, true, fixture.reasons.join("; "));
  assert.equal(fixture.files.length, FIXTURE_FILES.length);
  assert.equal(fixture.controlLabels.length, 100);
  assert.equal(new Set(fixture.controlLabels).size, 100);
  assert.equal(fixture.controlLabels[0], "Control 001");
  assert.equal(fixture.controlLabels.at(-1), "Control 100");
  for (const file of fixture.files) {
    assert.match(file.sha256, /^sha256:[0-9a-f]{64}$/);
    assert.equal(file.sha256, fixture.pin.entries[file.name]);
  }
  assert.equal(fixture.manifest.benchmark.count, 100);
});

test("the loopback server returns exactly the bytes the receipt hashed", async () => {
  const fixture = await loadFixture(FIXTURES);
  assert.equal(fixture.ok, true, fixture.reasons.join("; "));
  const server = await startFixtureServer(fixture);
  try {
    for (const [route, name] of [
      ["/index.html", "index.html"],
      ["/actions.html", "actions.html"],
      ["/manifest.json", "manifest.json"],
    ]) {
      const response = await fetch(`${server.origin}${route}`);
      assert.equal(response.status, 200);
      const body = Buffer.from(await response.arrayBuffer());
      const pinned = fixture.files.find((file) => file.name === name);
      assert.equal(`sha256:${sha(body)}`, pinned.sha256);
      assert.equal(body.length, pinned.bytes);
      assert.equal(response.headers.get("cache-control"), "no-store");
    }
    const missing = await fetch(`${server.origin}/serve.mjs`);
    assert.equal(missing.status, 404);
    const post = await fetch(`${server.origin}/index.html`, { method: "POST" });
    assert.equal(post.status, 405);
  } finally {
    await new Promise((resolveClose) => server.server.close(() => resolveClose()));
  }
});

test("a changed fixture byte fails its SHA256SUMS pin", async () => {
  await withTemp(async (dir) => {
    const fixtures = await copyFixtures(dir);
    const path = join(fixtures, "index.html");
    const original = await readFile(path);
    const changed = Buffer.from(
      original.toString("utf8").replace('data-label="Control 100"', 'data-label="Control 001"'),
      "utf8",
    );
    await writeFile(path, changed);
    const fixture = await loadFixture(fixtures);
    assert.equal(fixture.ok, false);
    assert.ok(fixture.reasons.some((reason) => reason.includes("fixture pin mismatch for index.html")));
  });
});

test("exact labels are enforced even when SHA256SUMS matches the changed bytes", async () => {
  await withTemp(async (dir) => {
    const fixtures = await copyFixtures(dir);
    const path = join(fixtures, "index.html");
    const original = await readFile(path);
    const changed = Buffer.from(
      original.toString("utf8").replace('data-label="Control 100"', 'data-label="Control 001"'),
      "utf8",
    );
    await writeFile(path, changed);
    await replacePin(fixtures, "index.html", changed);
    const fixture = await loadFixture(fixtures);
    assert.equal(fixture.ok, false);
    assert.ok(fixture.reasons.some((reason) => reason.includes("control labels are not exactly")));
    assert.equal(
      fixture.reasons.some((reason) => reason.includes("fixture pin mismatch")),
      false,
      fixture.reasons.join("; "),
    );
  });
});

test("a missing pin entry for a served file blocks the fixture", async () => {
  await withTemp(async (dir) => {
    const fixtures = await copyFixtures(dir);
    const path = join(fixtures, "SHA256SUMS");
    const text = await readFile(path, "utf8");
    await writeFile(
      path,
      text
        .split(/\r?\n/)
        .filter((line) => !line.trim().endsWith("  manifest.json"))
        .join("\n"),
      "utf8",
    );
    const fixture = await loadFixture(fixtures);
    assert.equal(fixture.ok, false);
    assert.ok(fixture.reasons.some((reason) => reason.includes("no entry for required fixture file manifest.json")));
  });
});

test("an unverified fixture cannot be served", async () => {
  assert.throws(() => startFixtureServer({ ok: false, bodies: new Map(), files: [] }), /unverified fixture/);
});
