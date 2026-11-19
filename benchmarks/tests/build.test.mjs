
import test from "node:test";
import assert from "node:assert/strict";

import { buildConfiguration, parseBuildIdentity, releaseIdentityFor } from "../scripts/lib/build.mjs";

test("the cargo profile comes from the embedded identity, not the path", () => {
  assert.equal(buildConfiguration("hand 0.1.0 (bc4cb0145733765930ef44ed744dfc5533ea1bda; release)"), "release");
  assert.equal(buildConfiguration("hand 0.1.0 (bc4cb0145733765930ef44ed744dfc5533ea1bda; debug)"), "debug");
  assert.equal(buildConfiguration("hand 0.1.0 (bc4cb0145733765930ef44ed744dfc5533ea1bda)"), null);
  assert.equal(buildConfiguration("v26.13.1"), null);
  assert.equal(buildConfiguration(null), null);
});

test("a full commit and the dirty marker both parse", () => {
  assert.deepEqual(parseBuildIdentity("hand 0.1.0 (bc4cb0145733765930ef44ed744dfc5533ea1bda-dirty; release)"), {
    sha: "bc4cb0145733765930ef44ed744dfc5533ea1bda",
    dirty: true,
    profile: "release",
  });
});

test("release identity is an embedded claim, unverified on its own", () => {
  const identity = releaseIdentityFor("hand 0.1.0 (bc4cb0145733765930ef44ed744dfc5533ea1bda; release)");
  assert.equal(identity.verified, false);
  assert.equal(identity.basis, "embedded-build-metadata");
  assert.equal(identity.claimed, "release");
  assert.deepEqual(identity.embedded, {
    sha: "bc4cb0145733765930ef44ed744dfc5533ea1bda",
    dirty: false,
    profile: "release",
  });
  assert.match(identity.reason, /self-report is not an attestation/);
});

test("a binary with no embedded identity claims nothing", () => {
  for (const version of ["v26.13.1", "puppet 8.0.0", null]) {
    const identity = releaseIdentityFor(version);
    assert.equal(identity.verified, false);
    assert.equal(identity.claimed, null);
    assert.equal(identity.embedded, null);
    assert.match(identity.reason, /did not carry an embedded commit/);
  }
});
