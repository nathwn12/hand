// Build identity comes from the bytes, never from where the bytes are stored.
//
// A path only says where someone put the file: `C:\Users\<name>\bin\hand.exe`
// claims nothing about a cargo profile. `hand --version` prints the commit and
// the cargo profile compiled in by `build.rs`, so a copy at an arbitrary path
// reports the same identity. The string is still a claim, not an attestation:
// `compare.mjs` proves it against a recorded `cargo build --release` receipt
// and the artifact hash.

const IDENTITY = /\(([0-9a-f]{7,40})(-dirty)?(?:;[ \t]*(release|debug))?\)/i;

/** The embedded build identity in a `--version` line, or null. */
export function parseBuildIdentity(version) {
  const stdout = typeof version === "string" ? version : (version?.stdout ?? "");
  const match = IDENTITY.exec(stdout);
  if (!match) return null;
  return {
    sha: match[1].toLowerCase(),
    dirty: Boolean(match[2]),
    profile: match[3] ? match[3].toLowerCase() : null,
  };
}

/** Build profile claimed by the binary itself; null when it embeds none. */
export function buildConfiguration(version) {
  return parseBuildIdentity(version)?.profile ?? null;
}

export function releaseIdentityFor(version) {
  const stdout = typeof version === "string" ? version : (version?.stdout ?? "");
  const embedded = parseBuildIdentity(stdout);
  return {
    claimed: embedded?.profile ?? null,
    verified: false,
    basis: "embedded-build-metadata",
    embedded,
    reason: embedded
      ? `the binary embeds commit ${embedded.sha}${embedded.dirty ? "-dirty" : ""} and profile ${JSON.stringify(embedded.profile)}; a self-report is not an attestation, so compare.mjs re-checks it against a recorded --release build receipt and artifact hash`
      : `--version did not carry an embedded commit and cargo profile: ${JSON.stringify(stdout.slice(0, 200))}`,
  };
}
