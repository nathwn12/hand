# Benchmark driver

Opt-in MCP stdio measurement for `docs/SPEC.md` §Performance (lines 309–336). It
drives a product's own stdio server, sends one request at a time, and writes a
receipt that keeps every attempt, including failures. Nothing here runs on
install, save or a hook; a run starts only when you invoke `bench.mjs`.

The spec's p95 numbers are thresholds, not results. This driver does not change
them and has no flag to override them.

## Run

```powershell
node benchmarks/scripts/bench.mjs --list

# hand (release is preferred; a debug build is resolved if no release build exists)
node benchmarks/scripts/bench.mjs --product hand --scenario warm-ping
node benchmarks/scripts/bench.mjs --product hand --scenario browser-snapshot
node benchmarks/scripts/bench.mjs --product hand --scenario desktop-enumerate `
  --desktop-title-contains "hand-bench-fixture"
node benchmarks/scripts/bench.mjs --product hand --scenario desktop-capture `
  --desktop-title-contains "hand-bench-fixture"

# pinned baselines use unprefixed tool names
node benchmarks/scripts/bench.mjs --product gladius --scenario browser-snapshot
node benchmarks/scripts/bench.mjs --product puppet --scenario desktop-enumerate `
  --desktop-title-contains "hand-bench-fixture"

node benchmarks/scripts/compare.mjs --receipts benchmarks/receipts --require-pass
```

Live runs require **at least 10 warmups and 100 measured requests**. Smaller sizes
are accepted only with `--test-fixture`, which marks the receipt `testOnly: true`
and is intended for the driver's own tests. Exit codes: `0` pass, `1` fail/blocked,
`2` unsupported, `3` usage, `4` driver error.

## Scenarios

| Scenario | Spec p95 | hand tool | Gladius | Puppet |
|---|---:|---|---|---|
| `warm-ping` | 50 ms | `ping` method | `ping` method | `ping` method |
| `browser-snapshot` | 250 ms | `browser_snapshot` | `snapshot` | — |
| `desktop-enumerate` | 250 ms | `desktop_list_windows` | — | `list_windows` |
| `desktop-capture` | 500 ms | `desktop_screenshot` | — | `screenshot` |

A missing binary, domain or tool name is reported as **unsupported** with a
reason and no samples. It is never rewritten as a pass, a fallback tool, or an
empty table. A tool that exists but produces incomplete output fails the
attempt; a scenario whose setup fails is **blocked**.

Browser setup navigates to the loopback fixture, sets 1280×720 at DPR 1, then
takes one readiness snapshot. Each measured snapshot must be non-truncated, on
`/index.html`, and contain all 100 labelled controls. Desktop setup resolves the
controlled window by exact target matcher and captures once to prove the image
and screen-space source are both 1280×720 at scale 1. Only then does the measured
set run.

## Safety and fixture rules

- **No personal browser.** The browser fingerprint must be a
  `chrome-headless-shell.exe`; any other executable is refused. Discovery is
  limited to `%LOCALAPPDATA%\gladius\chromium` and `GLADIUS_CHROMIUM_CACHE`, or
  an explicit `--browser-exe`. The driver then binds the child to that exact file
  with `GLADIUS_CHROMIUM`, clears `GLADIUS_CDP_URL`, reads the product's
  `[gladius] backend: chromium <path>` log, re-hashes the logged path, and blocks
  the scenario if the launched browser is missing, different or unverifiable.
- **No visible UI.** The driver never launches, creates, resizes or focuses a
  desktop window. Desktop scenarios attach only to a pre-existing controlled
  window that matches `--desktop-target` / `--desktop-title-contains`; without a
  target they are unsupported before the product is spawned.
- **Controlled browser fixture.** The fixture is always `<repo>/fixtures`; there
  is no directory override. The driver loads `index.html`, `actions.html` and
  `manifest.json` once, verifies every pinned byte against
  `fixtures/SHA256SUMS`, verifies the labels are exactly `Control 001` through
  `Control 100`, then serves those in-memory buffers on `127.0.0.1`. The server
  never re-reads the directory, so the receipt hash and the served bytes cannot
  diverge.
- **Fixture pin failure blocks.** A missing or malformed `SHA256SUMS`, a hash
  mismatch, a missing pin entry, a changed control label or a viewport mismatch
  yields `blocked` before any sample.
- **Capture evidence must prove itself.** A successful desktop capture needs the
  target `hwnd`/`pid`/`title`/`exe` identity matching the setup-resolved target,
  `capture.backend = "printwindow"`, `moved_during_capture = false`, a boolean
  `stale` flag (the unchanged-frame retry flag; static content may legitimately
  report `true`), a `content` blank-check at or above its threshold,
  `foregroundUnchanged = true`, `cursorUnchanged = true`, no warnings, and a
  `capture.note` that is either `null` or exactly the pinned engine's documented
  Mode B deferral note. Crop-fallback notes and any other note are refused.
  Blank, moved, crop-fallback, minimized or unverifiable evidence is an unsafe
  failure, not a sample.
- **Test fixture is not evidence.** `benchmarks/fixtures/fake-mcp-server.mjs`
  is deterministic, touches no browser or desktop, and stamps every receipt
  `testOnly: true` with `evidence: driver-test-only`. `compare.mjs --require-pass`
  refuses test-only receipts.

## What a receipt contains

`benchmarks/receipts/<product>-<scenario>-<UTC>.json` plus a `.sha256` sidecar:

- `product`: id, resolved command/args, executable SHA-256, `--version` output,
  `releaseIdentity` (path claim, `verified: false`, basis and reason), and the
  fake entrypoint hash when test-only.
- `scenario`: tool or method, params, and a stable params hash with the ephemeral
  fixture port normalized.
- `fingerprint.machine`: host, OS release/version, CPU, memory, display
  resolution/DPI, Node version, and a stable hash.
- `fingerprint.fixture`: the `SHA256SUMS` pin path and entries, per-file SHA-256,
  manifest version/viewport/control bounds, exact label count/bounds, and a
  stable hash of the bytes actually served.
- `fingerprint.browser`: headless-shell path, size, SHA-256 and cache version,
  plus `launch`: whether the product's logged chromium path was re-hashed and
  matched the fingerprint.
- `fingerprint.desktopTarget`: matcher, matched window identity, hwnd, and a hash
  of the stable identity (not the runtime hwnd). Capture attempts must repeat and
  match that identity.
- `protocol` and `samples`: warmup and measured attempt lists, each with
  `elapsedMs`, `ok`, `dispatched`, and an error when failed. Every measured
  attempt is present.
- `stats`: attempted/observed/passed/failed counts, nearest-rank p50/p95/max,
  and the percentile base (`observed/attempted`).
- `verdict`: `pass`, `fail`, `blocked` or `unsupported`, the target, failures and
  every reason; `releaseBlocked` is true for anything but a measured pass.
- `phases`: cold server start, initialize round trip, setup steps (the browser
  navigate includes lazy cold browser launch and is not thresholded), warmups,
  and the measured set.
- `load`: machine CPU busy percentage across the run, free memory at start/end,
  max concurrent requests (always 1), and an explicit note that unrelated load
  is not externally audited.
- `setup`, `coldStart`, `catalog`, `process`: precondition evidence, tool
  catalogue hash/count/names, exit state, stderr tail, protocol violations and
  notification count.

Percentiles are **nearest-rank over all measured attempts that produced a
duration**. A failed request that returned a reply stays in the distribution. A
request that could not be dispatched has no duration, remains a failure, and the
receipt states the smaller observed base rather than inventing a fast sample. Any
failed measured attempt or protocol violation blocks release regardless of p95.

Warmup failures are recorded in `samples.warmup`, counted in
`verdict.warmupFailures`, listed in `verdict.warnings`, and printed by the
summary, but they are not part of the 100 measured samples and do not feed p95 or
by themselves fail the gate. A warmup failure never becomes an invisible pass;
the corresponding measured failures still block. This follows the spec's gate,
which names the measured operations.

## Release-build evidence

A release identity is no longer permanently unverified merely because nothing is
signed. Two local routes supply evidence; neither uses a secret:

```powershell
node benchmarks/scripts/record-build.mjs --product hand `
  --source-root . --manifest-path Cargo.toml --bin hand `
  --target-dir C:\path\to\scratch-target
```

`record-build.mjs` runs `cargo build --release --locked` itself and records the
exact command, cargo/rustc/target, source root, HEAD, tracked diff + dirty state,
output path/hash/size and the built binary's `--version`. A caller cannot pass a
binary hash, profile or "verified" flag. The built binary is copied to
`benchmarks/receipts/builds/artifacts/` by default (gitignored, durable) and that
path is what the receipt records; use `--artifact-dir` to change it. Comparison
re-hashes the durable artifact and the measured binary; the recorded source state
is build-time provenance. A failed build, missing artifact, sidecar mismatch or
edited receipt leaves the identity unverified. Source edits after the build do
not change the artifact, but the receipt names the state it was built from.
Failed builds write a `-failed` receipt that never verifies. An existing verified
artifact can be copied to durable storage with
`relocate-build-artifact.mjs`.

Baseline Gladius and Puppet use the existing pins:
`baseline/pins.json` → `capturedExecutableSha256` plus the pinned version. A
measured binary matching both is accepted as `baseline-pin` evidence.

The local build route is evidence, not a signature. It proves that this recorder
ran a release-profile build and recorded a source state; it cannot
prove who ran it or defeat a caller who rewrites both the receipt and the source
tree. The strict gate still checks every hash and rejects debug/unknown paths.

## Comparing products

`compare.mjs` reads receipts (not reruns), groups them by scenario, and checks
the expected product set for that scenario: warm ping needs hand, Gladius and
Puppet; browser snapshot needs hand and Gladius; desktop scenarios need hand and
Puppet. A missing receipt is named, not green. The gate also requires, for every
measured receipt: a re-hashed binary whose SHA-256 still matches, a verified
release identity, `machine`, `fixture`, `browser` and launch proof, or
`desktopTarget` as the scenario requires, 10/100 sequential protocol with
identical protocol and params across receipts, at least 10 recorded warmup
attempts and at least 100 recorded measured attempts, warmup/measured failure
counts consistent with the records, p50/p95/max and observed counts recomputed
from the recorded attempts and matching the stats, fixture pin entries matching
the recorded files and the fixture hash, launch SHA-256 matching the browser
fingerprint, the declared target equal to the spec target, p95 within that
target, and no test-only receipt. Missing or malformed hashes fail rather than
being skipped.

Build type comes from the executable path. That is a claim, not a compiler-profile
or signed-build attestation; the receipt records it and a debug/unknown path sets
`releaseBlocked: true`. The comparison gate does **not** use that path to decide:
it uses the baseline pin or a local build receipt re-checked against the artifact.
The path is informational, so a release binary copied into a non-release directory
can still verify with a matching build receipt. Missing or failed evidence fails
the gate. Unsupported and failed rows remain visible. Comparisons are written under
`benchmarks/receipts/comparisons/` so a later directory scan does not read one as
a receipt.

## Controlled desktop fixture

`benchmarks/scripts/run-desktop-fixture.mjs` compiles and launches a borderless
1280×720 WinForms fixture with fixed content, `WS_EX_NOACTIVATE` and
`ShowWithoutActivation`; it never calls `SetForegroundWindow`,
`AllowSetForegroundWindow` or a modal loop. The launcher samples
`GetForegroundWindow` and `GetCursorPos` itself before and after showing, and the
fixture records its own before/after samples. If either moves, or if the desktop
moves during a one-second pre-show quiet check, the launcher stops without
running a scenario. On exit it terminates the process, confirms the window no
longer exists and samples foreground/cursor again.

```powershell
node benchmarks/scripts/run-desktop-fixture.mjs --startup-only
node benchmarks/scripts/run-desktop-fixture.mjs --product hand --scenario desktop-capture
```

Current measured state:

- Startup safety run: foreground `66414` before/after, cursor `(1246,1411)`
  before/after, fixture terminated, `windowAfterClose: false`.
- `desktop-enumerate`: **measured** against the 1280×720 fixture, 10 warmups and
  100/100 attempts. hand p50 `1.646 ms`, p95 `2.045 ms`, max `2.295 ms`; Puppet
  p50 `1.543 ms`, p95 `1.663 ms`, max `1.936 ms`.
- `desktop-capture`: **measured**, 10 warmups and 100/100 attempts. hand p50
  `215.051 ms`, p95 `228.575 ms`, max `234.03 ms`; Puppet p50 `209.88 ms`, p95
  `215.62 ms`, max `220.19 ms`. Both sit under the 500 ms target and the earlier
  blocker is gone: the driver now accepts a static fixture that legitimately
  reports `stale: true`, and a show with an unmoved foreground and cursor is
  enough for the launcher.
- **The fixture title is part of the comparison.** `compare.mjs` requires one
  `fingerprint.desktopTarget.hash` and one `scenario.paramsHash` across every
  receipt in a desktop scenario, hand and Puppet together, and both fold in the
  window title. The launcher defaults to a per-run title
  `hand-bench-desktop-1280x720-<pid>`, which no pinned baseline shares, so a fresh
  run lands in a new comparison group. Pass `--title hand-bench-desktop-1280x720-26036`
  to match the Puppet baseline receipts. A hand receipt measured against any other
  title still passes on its own numbers but drags its group to `comparable=false`
  and the gate refuses.

## Verification limits

- The benchmark-owned 1280×720 fixture proved the native capture setup live:
  image/source `1280×720`, scale 1, PrintWindow backend, `content` blank check,
  matching window identity, `MODE_B_NOTE`, `foregroundUnchanged: true` and
  `cursorUnchanged: true`, and full 100-attempt `desktop-capture` receipts now
  exist for hand and Puppet. The earlier 420×320 `tests/desktop_fixture` remains
  private-desktop-only and does not exercise `desktop_screenshot`.
- Browser launch verification trusts the product's own backend log plus the
  bound `GLADIUS_CHROMIUM`; the driver does not inspect the OS process tree.
- `fixtures/SHA256SUMS` is a local pin, not a signed external root.
- Source binding covers the tracked HEAD, tracked diff and tracked dirty status.
  Untracked source files are not part of that hash; a build that depends on them
  is only bound if they are added to the tree first.
- Build type is a path claim. The local build route and baseline pins are
  evidence, not signatures. The current receipts were measured against the binary
  built at HEAD `8d1d41c` from a clean tree; that build receipt re-hashes to
  `benchmarks/receipts/builds/artifacts/hand-8d1d41cf-20261118T055014405Z-hand.exe`
  (`sha256:82440a7e…`), which is the `release` binary staged into the npm tarball.
  Earlier attempts stay on the record: one was rejected because tracked source
  changed during the build, one at HEAD `eb75149a` (dirty) succeeded and was
  relocated, and a later recorder rerun failed to compile (`src/server.rs` E0061)
  and correctly wrote a failed receipt.
- Baseline Gladius/Puppet verify from `baseline/pins.json` because their measured
  binary hashes match the captured executable hashes. All four scenarios pass the
  strict gate: hand's receipts verify as `local-release-build` against the recorded
  artifact, and the desktop groups compare against the Puppet baselines.

## Tests

```powershell
node --test benchmarks/tests/
```

`stats.test.mjs` proves the percentile arithmetic and the failure rules.
`build.test.mjs` proves a path claim is never reported as a verified release
identity. `release-evidence.test.mjs` runs a real cargo `--release` build in a
temporary git repository and proves output/source binding plus refusal of a
tampered binary, edited receipt and missing evidence; it also proves the
Gladius/Puppet pin evidence and that a real build receipt plus a baseline pin can
make a browser group pass. `fixture.test.mjs` proves SHA256SUMS
verification, exact labels, and that the server returns the same bytes it hashed.
`capture.test.mjs` proves
blank, moved, crop-fallback, unsafe, wrong-note and target-less capture
evidence is refused, while the documented Mode B note, null and the static-content
stale flag are accepted.
`driver.test.mjs` drives the deterministic fake server to prove prefixed and
unprefixed tool mapping, 100 measured attempts and 10 warmups, browser binding,
fingerprinting, failure/timeout/incomplete/unsafe visibility, unsupported
handling without a spawn, mid-run exit accounting, warmup warnings, sample-size
enforcement and the test-only guard. `compare-core.test.mjs` proves the release
gate refuses debug/unknown builds, missing hashes, changed binaries, mismatched
protocol/params, missing products and test-only receipts. `compare.test.mjs`
keeps failed/unsupported rows and the refusing CLI gate visible.

These tests prove the **driver**, not live browser or desktop behavior, and not
any product's performance. A live run is the only thing that can produce a
measured receipt.
