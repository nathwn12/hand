
const IDENTITY = /\(([0-9a-f]{7,40})(-dirty)?(?:;[ \t]*(release|debug))?\)/i;

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
