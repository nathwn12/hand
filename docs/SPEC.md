# hand-mcp: product specification

Status: specification, with explicit baseline and release gates. No product
implementation is authorized by this document. This is a proposed contract,
not evidence that the product or its performance exists.

## Purpose

One local Rust product for agents to use and test browser interfaces and inspect
and operate Windows applications. It combines the useful capabilities of
Gladius and Puppet behind one MCP endpoint, then fills the material capability
gaps in both suites without pretending their execution environments or safety
guarantees are identical.

The desired outcome is a trustworthy inspect → act → verify loop: an agent can
identify a surface, perform an allowed action, and distinguish action delivery
from a proven effect. Latency should be low enough not to slow that loop.

## Scope and boundaries

- Browser: headless Chromium over CDP by default, browser inspection, interaction,
  screenshots and test assertions. An attached browser remains an explicitly
  selected external target, not an implicit route into a personal browser.
- Desktop: Windows windows, screenshots, UI Automation, and background-safe
  actions. It must not steal focus, move the physical pointer, inject input into
  the user's foreground session, or silently target another window.
- One product, installation, MCP connection, version and release. Internal
  browser and desktop engines may retain separate modules, state and failure
  isolation where that makes the contract safer.
- No claim of macOS/Linux desktop support, human-like foreground input,
  transparent access to a user's logged-in browser, mobile, touch, cross-window dragging, universal
  application compatibility, or a replacement for a browser's native test
  runner. Feature work outside these boundaries needs its own decision.
- Initial supported distribution target: Windows x64, matching the current
  launchers. Broader platform support is not implied by Rust.

## Product identity and distribution

Product name: **hand**; intended npm distribution and executable: **hand-mcp**.
The npm registry returned 404 for `hand-mcp` during discovery; this does not
reserve the name or establish trademark availability. Recheck registry, local
executable collisions and rights before release. Previously considered
`oc-pilot` and `oc-pilot-mcp` are executable names used by an unrelated
[npm package](https://registry.npmjs.org/oc-pilot/latest); do not use them.

MCP stdio is the primary agent interface. One installed Rust server owns one
MCP catalog and delegates to explicit browser and desktop domains. A CLI for
installation diagnostics is in scope; a broad standalone test framework,
remote listener, cloud service and GUI are not initial requirements.

## Initial public surface and lifecycle

Expose both existing catalogs, prefixed `browser_` and `desktop_`, retaining
their existing input and output schemas, semantics, typed errors and limits
unless the prefix or server identity necessarily changes them:

- Browser: `navigate`, `snapshot`, `evaluate`, `console`, `network`,
  `fetch_text`, `click`, `type`, `hover`, `select`, `key`, `upload`, `wait_for`,
  `assert_layout`, `layout`, `screenshot`, `visual`, `viewport`, `tabs`,
  `targets`, `close`, `storage`, `dialog`, `history`, `cdp`.
- Desktop: `screenshot`, `diff`, `list_windows`, `window`, `do`, `cursor`,
  `elements`.

All 32 are required for the first release; none is silently deferred. They are
the compatibility floor, not the complete product: the suite requirements below
also need a documented, discoverable interface and passing tests. Existing
tests define detailed legacy behavior; inventory exact schemas, codes and
versions against Gladius 0.8.0 and Puppet 8.0.0 at the captured revisions
`59c6d77` and `0f790be` before implementation. Prefixes are an intentional
breaking client change; document a one-to-one migration mapping and preserve
old products independently. No claim of drop-in compatibility. Do not create a
new common ref grammar or silently merge tools.

One MCP connection owns a lazy browser engine and its explicitly identified
isolated contexts and pages; the legacy default retains Gladius's existing
page/tab model. Desktop calls select targets using Puppet's target schema. Tool
prefix identifies the domain without changing legacy tool result bodies. A catalog/initialization
response identifies version, platform, browser backend and available
capabilities; unavailable engines must return a typed failure, not vanish
silently. Browser and desktop retain independent scheduling and deadlines.
Shutdown terminates only owned processes and temporary resources. For an
external browser, leave its process, pre-existing pages and profile in place;
close only pages hand created, and restore temporary settings where possible.
Caller-requested actions on explicitly selected external pages remain allowed.
On Windows, owned browser processes must be attached to a kill-on-close job so
server death closes them. Temporary profile deletion after a hard crash is
best-effort on the next start and limited to positively identified owned
profiles; a crash may leave files until then. External resources are never
enrolled in that job or swept.

## Core contract

1. **Explicit target.** Every browser operation identifies its browser session
   and page or uses an unambiguous session-scoped default. Every action on an
   existing desktop window identifies exactly one window; a launch instead
   identifies the explicitly selected executable and arguments before any
   window exists. Missing, ambiguous, vanished or replaced window targets are
   typed failures. Never fall back to the foreground window.
2. **Inspect first.** Return stable references, semantic properties, geometry,
   and freshness information when available. References are scoped to their
   owning session and surface; stale, navigated, detached or replaced targets
   fail rather than being guessed.
3. **Act narrowly.** Name the requested operation, target and arguments. Browser
   input uses CDP against the selected browser; the owned browser is headless.
   Desktop operations use
   supported UIA patterns or window messages without foreground takeover.
   Unsupported input is refused, not simulated by an unsafe fallback.
4. **Verify honestly.** Distinguish request accepted, action delivered, and
   observable postcondition satisfied. A delivered click is not proof that the
   application responded. Assertions and waits return a separate verdict;
   timeout, absent evidence, partial observation and transport errors cannot be
   reported as success.
5. **Observable failures.** Structured, documented error codes and human-readable
   reasons identify ambiguity, staleness, unsupported operations, access
   barriers, timeout, target loss and transport failures. Do not silently
   downgrade a safety guarantee or retry side effects.
6. **Ownership and cleanup.** Track which processes, profiles, pages and
   artifacts the product created. Cleanup only those resources; never terminate
   an attached external browser or affect unrelated applications. An attached
   browser may be changed only by the caller's explicitly targeted operations;
   temporary browser settings must be restored when possible.
7. **Accessibility and pixels are different evidence.** A semantic tree, a
   screenshot and a geometry assertion answer different questions. Expose their
   scope and limitations; do not infer accessibility compliance from geometry,
   or pixel correctness from a semantic tree.
8. **Keep both target models.** Browser CDP refs and desktop UIA refs are not
   interchangeable, nor are viewport, screen, and client coordinates. Preserve
   domain-specific schemas; any conversion must name its space and scale.
   Headless Chromium is not a visible desktop window. A joint visible-browser
   scenario requires an explicitly attached compatible browser and separate
   validation; do not assert it works from the headless tests.

Keep Gladius's lazy browser start, explicit attach, per-call budgets,
restoration on cancellation, typed errors, frame boundaries and distinct
pixel-identity versus tolerance verdicts. Keep Puppet's target lock,
pre-delivery refusal even in dry-run, independent `effect` and `satisfied`
verdicts, fingerprint as identity rather than state, bounded burst and
source-level no-steal checks. Capture both existing contracts in executable
compatibility tests before replacing either surface.

## Safety and privacy

- Background safety is a hard invariant, not an option or permission that a
  caller can override. Validate a desktop target before every action, including
  each action within a batch; a batch is ordered but not atomic.
- Elevated windows and inaccessible desktops fail explicitly; never escalate.
  Sensitive or destructive application actions are not inherently safe merely
  because the transport is background-safe. The calling agent remains
  responsible for authorization.
- A browser engine owned by hand uses its own temporary profile. Attaching
  to an external CDP endpoint must be explicit and must disclose that the
  endpoint can expose its browser state. Never import browser credentials.
- A private Windows desktop isolates windows and input, not application
  profiles, saved documents or credentials; it is not a security sandbox.
- Capture only what the caller requests. Screenshots, text, page content, window
  titles and logs can contain private data; no telemetry or external upload by
  default. No secrets or page content in diagnostic logs by default. Avoid
  persistent artifacts unless explicitly requested.
- Named challenges, access controls, permissions and user-confirmation steps
  are not bypassed. Return the blocker; do not claim an action succeeded.
- A client disconnect or crash does not imply a sent action was undone. A
  response lost after dispatch is **outcome unknown**, not safe to replay.
  The client correlates MCP request IDs with its own pending calls; after a
  transport loss it must treat each dispatched call without a response as
  unknown. The server does not promise crash recovery or durable action
  journaling and must not assert an outcome it cannot observe.

## Browser testing

Provide the existing useful inspect, interact and verification capabilities:
navigation and page/session lifecycle; semantic snapshots and element
references; actions and form input; waits for explicit conditions; screenshot;
geometry/layout checks; console/network observation where supported. Keep
browser tests headless and bounded; define navigation and assertion timeouts.
An assertion reports expected condition, observed evidence, target, verdict and
timeout. Deterministic capture must state what is frozen and what is sampled,
including cross-process frames, canvas and media limitations.

Do not impose a single universal selector. Preserve
useful evidence and low-context inspection rather than hiding state behind an
opaque convenience call. Raw CDP is explicitly advanced and does
not broaden the default desktop safety contract.

### Additional browser-use and testing requirements

The 25 Gladius tools are the floor. Add the following as explicit capabilities
with names and schemas decided from the smallest coherent catalog; extending an
existing tool is preferable when its contract remains compatible. Do not
claim they already exist:

- Isolated, named browser contexts with separate cookies and storage; explicit
  creation, selection and teardown. Support multiple pages, popups and frames
  with typed cross-origin and inaccessible-frame limits. No accidental
  cross-context state leakage.
- Reliable selectors and references by role, accessible name, text and CSS
  where supported; explicit scoping, uniqueness, visibility and actionability.
  Cover scroll with read-back, hover, keyboard, selection, double/right click,
  focus/blur and browser-internal drag/drop, file upload, dialogs and downloads.
  Cross-frame references must retain frame identity; unsupported frames fail
  rather than retargeting the main frame. An extended navigation result returns
  observed HTTP status when known and `null` otherwise; never infer success
  from a title. Preserve the legacy result schema; expose the extended result
  through an explicitly versioned interface.
  Ordered form batches report each applied step and partial failure, not atomicity.
  Downloads and uploaded paths need explicit caller destinations or inputs;
  no automatic opening, execution or exfiltration.
- Test controls for network interception and mocks, request/response
  inspection, failure injection and offline mode, scoped to a test context.
  Restoring interception on exit and cancellation is mandatory; never silently
  intercept another context or an attached browser's unrelated traffic.
  Registrations have explicit IDs and context ownership, and changes within a
  context are serialized. Cancellation removes only registrations owned by
  that request; other active registrations remain effective. Conflicting
  rules use documented deterministic priority or fail explicitly.
- Deterministic clock and viewport controls, configurable device scale and
  locale where supported, per-context permissions, and explicit test data
  setup/reset. Emulation must identify its limits and cannot claim to be a real
  mobile device or replace real assistive-technology testing.
- Assertions on DOM, accessibility properties, visibility, text, navigation,
  URLs and network observations; bounded polling and clear expected/actual
  evidence. Capture console, request failures and page errors with limits.
  Support visual baseline comparison with strict identity and tolerance
  separately, together with the rendering/environment fingerprint.
- Include responsive viewport matrices and scoped geometry checks, and expose
  measurable contrast, typography, focus indication and target-size evidence
  with explicit inability to assess complex backgrounds. These checks do not
  amount to a WCAG audit or a screen-reader verdict.
- Bounded, opt-in traces and artifacts for failure diagnosis, including
  screenshots and logs, with explicit destination, redaction and retention.
  User content, credentials and network payloads are sensitive; no default
  recording or upload. Do not promise universal automatic secret redaction.
- Local deterministic fixtures and a repeatable way to run a sequence of
  assertions through MCP; this is not a new general-purpose test language or
  a replacement for existing CI runners. A failed assertion must produce a
  failing verdict suitable for a client to use as an exit status.

### Additional Windows computer-use requirements

The seven Puppet tools are the floor. Provide a discoverable capability matrix
per operation and target, including whether discovery is complete, whether an
action can be attempted without stealing, and how an effect can be verified.
Where the underlying platform cannot safely support an operation, return a
typed unsupported or unverified result instead of pretending to be complete.

- Window and application lifecycle: find and identify windows and processes,
  observe focus and state, launch an explicitly selected application on a
  private desktop where supported, and close only caller-selected resources.
  Launching an arbitrary program is a side effect and needs caller authority.
  A private desktop is preferred for owned test apps, but does not isolate
  application profile or data and is not guaranteed to support every app.
  Launch the selected process on the private desktop, never by switching the
  user's desktop; refuse when that setup cannot be established. Report
  foreground and cursor observations and whether window containment was
  verified. The no-steal invariant covers hand's own operations: arbitrary
  launched applications and their descendants may independently alter the
  interactive desktop or data, which cannot be universally prevented or
  represented as a guarantee. Do not launch an untrusted app as a sandbox.
- Read semantic trees with paging, roles, names, supported patterns and
  coordinate provenance; navigate and inspect controls, lists, menus, dialogs,
  text and values where providers expose them. Missing or unavailable data is
  distinct from an empty or false value.
- Perform supported background UIA pattern actions and target-locked keyboard,
  pointer and scroll messages; expose supported selection, expand/collapse,
  and range operations only when a safe background mechanism is actually
  available. Preserve Puppet's refusal of `set_value` and typing on a UIA
  element; do not silently reinterpret those verbs. Do not substitute physical
  input, foreground activation or clipboard writes when the target has no safe
  pattern.
- Capture a selected window, monitor or region; provide exact source and
  image-to-screen mapping, visual comparison and optional privacy masks.
  Report occlusion, minimized/elevated/secure-desktop restrictions, unavailable
  capture backends and stale target identity rather than returning misleading
  pixels.
- Let clients wait on semantic state, window lifecycle or a bounded visual
  change, and return the observed evidence separately from delivery. Support
  cancellation and bounded work; a target disappearance during a multi-action
  flow must prevent later actions reaching a replacement.
- Clipboard reads, if retained, are explicit, bounded and sensitive; clipboard
  writes remain prohibited. Screenshots and clipboard data must not be logged
  or uploaded by default.

### Coverage and hard limits

“Complete” means the capabilities above, existing compatibility inventory, and
honest reporting of unimplemented or platform-impossible cases—not that every
application is controllable invisibly. Some programs require foreground focus,
read raw input, run at higher integrity, use a secure desktop, or do not expose
usable UIA. For those, hand must refuse or report unverified; it must not
silently fall back to foreground injection. Hidden operation and no-steal
invariants outrank coverage and latency.

## Desktop computer use

Provide window discovery, semantic element inspection, window/region capture,
cursor observation, background-safe window control, and bounded actions.
Coordinate spaces must be explicit: element and window rectangles in screen
pixels; target-bound pointer coordinates in client pixels; screenshots include
the mapping back to screen coordinates. Multiple monitors, DPI and offscreen
elements must not be silently normalized into incorrect coordinates.

Never promise that every Windows application accepts UIA or posted messages.
Report unavailable patterns, blocked integrity levels, minimized windows,
unsupported raw-input applications and desktop/session restrictions. No
clipboard writes, physical cursor movement, foreground activation, IME
composition or cross-window drag in the initial contract.

## Performance: measure “instant”

Performance is an acceptance gate, not a claim based on Rust or an isolated
microbenchmark. Measure on the owner's Windows machine and report CPU, memory,
OS, build, version, browser binary, resolution, sample size and load. Separate
cold server start, cold browser launch, warm browser round trip, desktop window
enumeration, semantic inspection, screenshot capture and end-to-end action
verification. Include client/MCP transport, not just inner function time.

Initial release targets, subject to baseline validation before implementation:
warm no-op MCP round trip p95 ≤ 50 ms; warm browser
snapshot of a specified local fixture p95 ≤ 250 ms; desktop window enumeration
p95 ≤ 250 ms; 1280×720 window capture p95 ≤ 500 ms. Each uses ≥100 measured
requests after ≥10 warmups, reports median/p95/max, failures and concurrent
load. Run one request at a time with no unrelated intentional load. Use a
versioned local fixture containing 100 visible labelled controls, served from
loopback, a 1280×720 viewport and the pinned browser; enumerate the controlled
Windows fixture and capture its 1280×720 window with PrintWindow at native
resolution. Use an MCP `ping` for no-op. These are targets, not observed
results, and do not cover app response, network or cold startup. Freeze the
exact fixture bytes and pinned browser digest in the benchmark receipt before
comparing either product; the fixture does not yet exist. Release is blocked if
any of the 100 measured operations fails or produces incomplete output, or if
the p95 for the full set exceeds its target. Calculate p95 over all 100
attempts; failures are failures, not omitted samples. Baseline results may inform a documented
spec revision, not silently change the threshold.
Compare the same scenario against the current Gladius or Puppet release on the
same machine. A slower or failing scenario is visible; no percentile may hide
errors, retries, or incomplete output. If targets are infeasible, present
measured tradeoffs rather than weaken safety or falsify the benchmark.

## Acceptance scenarios

1. Start one MCP server, discover both browser and Windows desktop
   capabilities, and obtain explicit engine/platform availability.
2. Open a controlled local browser fixture, inspect it, activate a control,
   assert the expected DOM effect, and capture a screenshot. A deliberately
   false assertion must fail with observed evidence.
3. Inspect a controlled background Windows test window, invoke a supported
   control and prove its state changed. The foreground window and physical
   pointer remain unchanged. Repeat with an ambiguous selector and a stale
   window: no unintended target receives an action.
4. A desktop application that ignores posted input yields delivered or
   unverified, not verified success; an elevated target is refused.
5. Browser navigation invalidates relevant refs; stale refs fail clearly.
   Attached external browser processes, pre-existing pages and profile survive
   hand shutdown; only hand-created pages are closed.
6. Browser disconnection does not silently retarget desktop, and a desktop
   failure does not corrupt the browser session. On crash or disconnect the
   client classifies each dispatched request without a response as unknown.
7. Run the measured performance protocol against the same fixtures and machine
   for both existing tools and the combined product. Publish the actual
   distribution and failed operations, not only the best run.
8. Run package installation and MCP discovery from the actual public install
   route before any release claim. Verify no collision with an existing package
   or executable.
9. Create two isolated browser contexts; set a cookie in one and prove it is
   absent in the other. Mock a request in one context, verify the expected
   response, and prove the other context's request is unaffected; cancellation
   must restore interception.
10. In a local fixture, exercise a popup, frame, download, upload, dialog,
    console error and failed assertion. Prove all artifacts stay within the
    requested destination and that an inaccessible frame is reported.
11. Launch a controlled fixture on a private desktop; inspect and actuate a
    supported control while the user's foreground window and pointer remain
    unchanged. Demonstrate a raw-input-only or unsupported control produces an
    explicit failure or unverified result, never physical input fallback.
12. Prove desktop semantic absence is not reported as `false`, a stale window
    cannot receive a later action, and wait timeout is distinguishable from a
    successful postcondition.
13. Install two interception registrations in one context, cancel one and
    prove the other remains active; remove the second and prove normal traffic
    resumes. Verify a conflicting rule gives the documented priority or error.
14. In controlled fixtures, freeze and advance a test clock, simulate offline
    and a failed request, and set a permission in one context; verify effects,
    isolation from a second context, and restoration after teardown.
15. Verify trace capture is opt-in, caller-specified redaction is applied, and
    retention cleanup removes only owned artifacts. Demonstrate unredacted
    secrets are not falsely claimed to be detected. Exercise semantic paging
    past one page, plus a visual-change wait that succeeds and one that times
    out without reporting success.
16. Source-level no-steal checks and controlled live fixtures prove no physical
    pointer movement, foreground activation or global keyboard injection by
    hand. Exercise typed refusal before delivery under both normal and dry-run
    paths. Verify a blocked elevated window, a locked/secure desktop and a
    minimized or uncapturable target do not masquerade as empty success.
17. Scroll a nested browser container and verify its actual offsets; test
    frame-scoped references, partial form failure and a local HTTP 404 with
    observed status. Verify an unsupported frame never acts on the main page.

## Integration and release

First establish compatibility inventories for both existing tool catalogs,
contracts, error codes, supported platforms and behavioral tests. Every listed
tool is required; record any discrepancy as a blocker with a test and migration
note rather than silently dropping a capability. Prefer
cohesive browser and desktop engine boundaries over forced shared abstractions.
Implement the state and lifecycle contract above before exposing the combined
catalog. Use existing licenses and attribution as required.

Implement in vertical slices only after the specification is complete:
(1) shared server and discovery with isolated engines; (2) representative
browser inspect/act/verify; (3) representative desktop inspect/act/verify and
safety proof; (4) full compatibility inventory and additional browser and
desktop capabilities above; (5) packaging, end-to-end verification and
performance gate. Each slice preserves existing work and has
its own targeted checks. Release, publication and irreversible migration need
separate authority.

## Known design risks

- One MCP endpoint is a single process-level failure domain unless deliberately
  isolated; internal modules alone do not prove crash isolation.
- Browser references and UIA references have different lifetimes. An apparent
  common ID must not erase those differences.
- A single tool catalog can consume more agent context and make targeting
  ambiguous. Discovery, namespacing and schemas need measurement in a real
  client.
- Distribution under a scoped npm name does not by itself avoid a globally
  conflicting executable name.
- A browser test suite and Windows computer use have different security,
  platform and timing constraints. Shared UX must not obscure them.
- Migration, current installed versions, and platform parity need inventory
  before any compatibility promise. No source repository has been modified.

## Evidence and decisions

Source review was read-only: Gladius at `59c6d77`, especially
`docs/ARCHITECTURE.md`, `docs/ENGINE.md`, `docs/HEADED-MODE.md`, `src/tools.rs`
and its safety and MCP tests; Puppet at `0f790be`, especially `SPEC.md`,
`FREEZE.md`, `docs/GUIDE.md`, `docs/HEADLESS.md`, `src/schemas.rs`,
`src/target.rs`, `src/gate.rs` and `tests/no_steal.rs`. These establish current
source contracts, not proof that live tests passed in this session.

Decisions: one namespaced catalog rather than an invented common automation
language; Windows x64 first rather than unsupported portability claims; owned
headless browser plus separately targeted background desktop rather than
pretending headless Chromium is a desktop window; no durable action journal,
with unresolved requests classified by the client; no implicit compatibility or
automatic retirement of either existing product. Baseline measurements and
first-release fixture creation are implementation prerequisites, not results.

## Ambition and independent judgment

- Goal beyond the goal: agents can operate and verify browser and desktop
  workflows quickly without disturbing the person at the machine.
- Altitude: (1) unify existing capabilities without regressions; (2) make the
  inspect → act → verify loop truthful and measurable across both domains;
  (3) make that loop reliable enough to use routinely. The third rung depends
  on actual integration and live evidence; it is not proven by this document.
- Depth: start from both engines' current source, schemas, tests and measured
  behavior, not their summaries. Use conformance and live fixture tests,
  no-steal checks, failure injection and end-to-end benchmarks. Target the
  acceptance and latency gates above without sacrificing correctness.
- Range: apply browser automation to DOM and CDP, Windows accessibility to UIA
  and HWND targeting, and performance engineering to timed MCP round trips.
  Each discipline uses its own evidence rather than treating a screenshot,
  semantic tree or timing number as proof of the others.
- Outside judge: the executable acceptance scenarios and independent review.
  Pass only when every required contract check passes, no-steal invariants hold,
  and the benchmark gate passes on the stated fixture. This spec has had
  independent document review; the product tests and benchmarks do not exist yet.

Reusable implementation brief, for when implementation is separately authorized:

> Build hand-mcp from this specification as one Rust MCP product, preserving the
> namespaced Gladius and Puppet contracts and their safety guarantees. Establish
> exact baseline inventories and reproducible fixtures first; implement in the
> specified vertical slices without retiring the existing products. The outside
> judge is the complete acceptance suite, independent no-steal and compatibility
> checks, and the stated end-to-end latency gate: all must pass on the documented
> environment, with failures and unverified behavior reported rather than hidden.
