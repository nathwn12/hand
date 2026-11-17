// Build identity is a claim, not proof. The binary path tells us whether a
// target looks like a debug or release build; it is not a compiler-profile or
// signed-manifest attestation. Receipts must say so, and the release gate must
// refuse an unverified identity rather than treating a path as cryptographic
// proof.

export function buildConfiguration(path) {
  if (!path) return null;
  if (/[\\/]debug[\\/]/i.test(path)) return "debug";
  if (/[\\/]release[\\/]/i.test(path)) return "release";
  return "unknown";
}

export function releaseIdentityFor(path) {
  const claimed = buildConfiguration(path);
  return {
    claimed,
    verified: false,
    basis: "binary-path-heuristic",
    reason:
      "the executable path is not a compiler-profile or signed-build attestation; release identity is unverified",
  };
}
