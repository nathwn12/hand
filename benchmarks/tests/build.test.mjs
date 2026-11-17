// Build identity is a path claim, deliberately not called proof.

import test from "node:test";
import assert from "node:assert/strict";

import { buildConfiguration, releaseIdentityFor } from "../scripts/lib/build.mjs";

test("build configuration is a path claim only", () => {
  assert.equal(buildConfiguration("Q:\\p\\target\\debug\\hand-mcp.exe"), "debug");
  assert.equal(buildConfiguration("Q:\\p\\target\\release\\hand-mcp.exe"), "release");
  assert.equal(buildConfiguration("Q:\\p\\hand-mcp.exe"), "unknown");
  assert.equal(buildConfiguration(null), null);
});

test("release identity is unverified for every path claim", () => {
  for (const path of ["Q:\\p\\target\\debug\\a.exe", "Q:\\p\\target\\release\\a.exe", "Q:\\p\\a.exe"]) {
    const identity = releaseIdentityFor(path);
    assert.equal(identity.verified, false);
    assert.equal(identity.basis, "binary-path-heuristic");
    assert.equal(identity.claimed, buildConfiguration(path));
    assert.match(identity.reason, /not a compiler-profile or signed-build attestation/);
  }
});
