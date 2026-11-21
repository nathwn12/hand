# Compatibility contract

This file records what the integration preserves, what it deliberately changes,
and how each claim is checked. It is scoped to the initial slice; it is not a
claim that every specification capability exists.

## Baseline

| Engine | Product version | Pinned source | License |
|---|---|---|---|
| Browser | Gladius 0.8.0 | `59c6d77` | MIT |
| Desktop | Puppet 8.0.0 | `0f790be` | MIT |

Both sources were copied into this repository at those pins. Engine behavior is
the source, not a summary: the same modules, locks, deadlines, schemas, typed
codes and result bodies are compiled in.

Executable check:

```powershell
node baseline/scripts/compare-catalog.mjs --serve <path-to-hand.exe> --baseline hand
```

Observed on a release build of this tree, quoted verbatim:

```text
PASS hand baseline: 32 baseline tools vs 35 candidate tools
  compared   32 tools
  extra      3: browser_drag, browser_wheel, browser_context
  metadata   0 title/description difference(s) (strict: on)
  declared   17 addition group(s) allowed by baseline/declared-additions.json:
             browser_snapshot metadata.description += description
             browser_screenshot inputSchema += properties.clip, properties.element, properties.format, properties.quality
             browser_screenshot outputSchema += properties.clip, properties.element, properties.format
             browser_screenshot metadata.description += description
             browser_click inputSchema += properties.button, properties.clickCount
             browser_click outputSchema += properties.resolved
             browser_click metadata.description += description
             browser_type outputSchema += properties.resolved
             browser_type metadata.description += description
             browser_upload outputSchema += properties.resolved
             browser_upload metadata.description += description
             browser_hover outputSchema += properties.resolved
             browser_hover metadata.description += description
             browser_select outputSchema += properties.resolved
             browser_select metadata.description += description
             desktop_screenshot outputSchema += properties.noSteal
             desktop_screenshot outputSchema += properties.clipboardUnchanged, properties.masks
             desktop_screenshot inputSchema += properties.privacyMasks
             desktop_elements outputSchema += properties.noSteal
```

The 32-name floor is served **with exactly the allowances declared in
`baseline/declared-additions.json` and no other difference**: two input schemas
and nine output-schema groups carry the properties listed there, thirteen tool
descriptions are rewritten there, and every other property, `required` list,
annotation, `title`, description and name is the pin's. The three extras above
are hand's own tools, not legacy bodies. The file is not a relaxed mode: an
undeclared description or any title difference fails, and so does a declared
description that still matches the pin, because the declaration says that one
was rewritten.

## Name mapping

The only catalog change is the prefix. A client migrates one-to-one:

- Gladius `navigate` → `browser_navigate` … `fetch_text` → `browser_fetch_text`
- Puppet `screenshot` → `desktop_screenshot` … `elements` → `desktop_elements`
- hand adds three browser tools with no legacy counterpart: `browser_context`
  (isolated named contexts), `browser_wheel` and `browser_drag` (trusted pointer
  primitives). Their bodies are new; a migration stays one-to-one for every
  legacy name.

`tests/discovery.rs::tool_bodies_are_engine_bodies_under_the_prefix` parses the
committed floor (`baseline/hand-catalog.json`) and `baseline/declared-additions.json`,
asserts every declared addition is present, removes exactly those paths and
requires structural equality with the floor for everything else — name,
`annotations`, `title`, every pinned property, and every description that is not
declared as rewritten. A declared description must **differ** from the pin: the
declaration says that one was rewritten, so an identical one is stale. The
reference is the floor on disk, not `browser_engine::list()`: the engine is the
same source that publishes hand's bodies, so it cannot go red on a schema change,
and a deleted allowance entry must.
`tests/discovery.rs::catalog_is_the_floor_plus_hand_additions` asserts count,
uniqueness and namespacing.

Prefixes are a breaking client change by design. The old products are not
modified or retired; no drop-in compatibility is claimed.

## Protocol surface

- Transport: newline-delimited JSON-RPC 2.0 over stdio, the same framing both
  engines use. LSP-style `Content-Length` framing is not implemented (neither
  engine implements it); a client that sends it is not supported by this slice.
- Versions: `2025-06-18`, `2025-11-25`, `2026-07-28`. `initialize` echoes a
  supported requested version, else `2025-11-25`. An explicit per-request
  version outside the set is refused `-32022` with the supported list.
- Methods: `initialize`, `notifications/initialized`, `notifications/cancelled`,
  `ping`, `tools/list`, `tools/call`, `server/discover`.
- `resultType: "complete"` is added to `tools/list`, `tools/call` and
  `server/discover` results for a 2026-07-28 peer, matching both engines.
- Tool failures are results (`isError: true` + `structuredContent`) except where
  the request itself is invalid: missing name is `-32602`, unknown method is
  `-32601`, malformed frames are `-32700`/`-32600`, a reused in-flight desktop
  request id is `-32600` with `data.reason: "duplicate_in_flight"`, and a
  desktop worker fault outside the engine's own containment is `-32603`.
- Concurrency: 4 browser calls, 8 desktop jobs. An excess call is not started
  and returns a typed `busy`/`concurrency_limit` tool result carrying
  `inFlight`, `available`, `started: false` and `retry: "safe"`.
- A timed-out or cancelled call is never described as stopped. Its answer
  carries `stopped: false`, `outcome: "unknown"`, `retry: "unsafe"`,
  `sideEffects: "may_still_land"` and the browser slot it still holds
  (`slots.limit`, `slots.heldByThisCall`).

### Identity extension

`serverInfo` keeps `name`, `version` and the engines' own fields, and adds:

```json
{
  "name": "hand",
  "title": "hand",
  "version": "0.3.0",
  "platform": "windows-x64",
  "engines": {
    "browser": {"engine": "gladius", "sourcePin": "59c6d77", "version": "0.8.0",
                 "backend": "chromium", "caps": "render", "cdpUrl": null, "toolCount": 27},
    "desktop": {"engine": "puppet", "sourcePin": "0f790be", "version": "8.0.0",
                 "available": true, "backend": "uia+win32", "toolCount": 7}
  },
  "count": 38,
  "isolatedContexts": {"tool": "browser_context", "available": true, "…": "…"}
}
```

`browser.backend`/`caps`/`cdpUrl` are the browser engine's own `backend_info()`,
so an attached or capability-restricted engine is described as it really is.
`engines.browser.toolCount` is the engine's own `list()` (25 pinned plus
`wheel` and `drag`). `browser_context` is published from the engine's context
module beside that list. With `browser_form`, `browser_download` and
`browser_assert` added, the wire carries 31 `browser_*` bodies and 38 tools in
total.

## Preserved behavior

- Browser refs are the engine's own (`eN` in the snapshot body, `@eN` in action
  arguments; both engines' existing contract). Navigation invalidates refs and
  a stale ref is a typed failure.
- Browser typed codes and reasons, envelopes, budgets (`_budgetMs` 2 s–600 s,
  90 s default), lazy launch, explicit attach, private profile, kill-on-close
  job, deterministic capture and its freeze/skip reporting.
- Desktop target schema, target lock, pre-delivery refusal, independent
  `effect`/`satisfied` verdicts, fingerprint-as-identity, bounded bursts,
  capture mapping and the no-steal invariants.
- Desktop production admission: calls run through Puppet's `run_tool_job` +
  `production_execute` wrapper (admission guard, mutating-input lock,
  cancellation token, panic containment), not a direct call to `tools_call`.
- Engine unit tests remain compiled: 69 browser tests, 228 desktop tests (25
  live-desktop tests are not run by default).

## Intentional differences

1. **Cancellation is domain-specific.**
   - Browser: the client receives the typed `cancelled`/`client_cancelled`
     result immediately; the in-flight engine call is allowed to finish so a
     restoration can continue, and its late result is discarded. A dispatched
     action can still happen: `stopped:false`, `outcome:"unknown"` and
     `retry:"unsafe"` distinguish that from a confirmed cancellation. Cleanup
     is bounded, so restoration is unverified if that bound is reached.
   - Desktop: Puppet's silence contract is preserved. The token is flipped, the
     job's result is dropped, and no response is written for that id; the
     outcome of an already-dispatched action is unknown.
   Tested by `tests/cancel.rs`.

2. **One cancellation token is published before desktop execution.** The
   reader calls `cancel::ensure` before the job reaches a worker, so a cancel
   arriving between dispatch and execution is never lost to a re-registration.

3. **One initialize.** The old servers each described themselves; hand answers
   once with both engines under `engines`. Tool bodies and descriptions are the
   pinned ones, except the schema additions and the thirteen description rewrites
   declared in `baseline/declared-additions.json`.

4. **Budget environment.** `_budgetMs` is the browser contract. The default also
   reads `HAND_TOOL_BUDGET_MS` first, then `GLADIUS_TOOL_BUDGET_MS` for
   compatibility.

5. **`_budgetMs` is a browser-domain argument.** Passing it to a `desktop_*`
   tool reaches Puppet's argument validator and is refused there; desktop
   budgets stay Puppet's own.

6. **Shutdown and teardown are bounded and reported honestly.** EOF drains
   tracked workers with 5 s (owned engine) or 30 s (attached engine), then one
   shared 1 s abort settle, then the browser close (30 s bound), then a 2 s
   runtime teardown bound in `main`. A browser worker cut at a bound has its
   restoration reported `UNVERIFIED`. A started `spawn_blocking` desktop job
   cannot be aborted: its outcome is `UNKNOWN` and process exit ends the thread.
   A panic in the server loop skips normal shutdown, so an attached browser is
   not detached and its page state is unverified.
7. **A hidden desktop launch that never created its target is an error.**
   Puppet's burst keeps `ok` tied to per-action results, so a refused hidden
   launch with only target-free actions used to answer `ok: true`. hand reports
   `ok: false` and `isError: true` with `hidden_desktop.desktop_created: false`.
   An explicit `target` still takes precedence and keeps its ordinary verdict.
8. **Vendored-tree adaptations** (marked in place):
   - `desktop-engine/src/lib.rs` is Puppet's `main.rs` as a library: no `main`,
     no Windows-subsystem attribute, public module declarations.
   - `desktop-engine/src/serve.rs` gains `hand_tools_list`, `hand_call` and the
     `hand_tools_list` parse; `hand_call` is a thin caller of the production job
     wrapper.
   - `desktop-engine/src/cancel.rs` gains `ensure` (register-if-absent).
   - `browser-engine/src/tools.rs` gains `typed_envelope`; `browser.rs` grants
     `find_chromium` crate visibility; `lib.rs` is new.
   - Puppet's fixture self-test floor is 15 files instead of 20 because the
     library crate carries no integration `tests/` tree; its scans still cover
     every file present.
9. **The declared allowances are one versioned file.** `baseline/declared-additions.json`
   (version 1) is the only place a legacy tool may differ, and
   `baseline/scripts/lib/catalog.mjs` is its only reader. Schema paths are dotted
   and relative to the schema object, so a top-level property is
   `properties.<name>`; a metadata entry is a bare tool name.
    - `desktop_screenshot` adds `properties.clipboardUnchanged`, the third
      global a capture must not touch, and `properties.masks`, the regions
      blanked before encode; both are reported on success and on typed
      failures. Its `inputSchema` adds `properties.privacyMasks`, the mask
      input. Legacy fields stay unchanged.
   - `outputSchema`: `click`, `type`, `hover`, `select` and `upload` add
     `properties.resolved` — `{selector, ref, role, name}` when the element
     argument used one of the accessibility forms, so the caller can see which
     element answered. `browser_screenshot` adds `properties.format`,
     `properties.element` and `properties.clip`, which state how the pixels were
     encoded and what box they cover. `desktop_screenshot` and
     `desktop_elements` add `properties.noSteal`: attribution beside the raw
     before/after booleans that says whether a pointer or foreground change was
     the owner's input or this process's. The legacy fields stay unchanged.
     `browser_evaluate` adds `properties.void`: a valueless JavaScript result
     is a success with `value: null` and `void: true`, while an explicit
     `null` keeps `value: null` with no flag. The engine's internal eval
     helpers stay strict; only the public tool boundary reads the
     `undefined_result` marker. `browser_navigate` and `browser_snapshot` add
     `properties.viewport`, the live `{width, height}` read-back from the active
     page; `browser_screenshot` already published `viewport` in the pin and now
     fills it on a plain capture too. An owned Chromium launch pins the window
     to 1440x900, and the owned session replays a 1440x900 device-metrics
     override so the page viewport matches; an attached engine has no override
     and keeps its owner's viewport. `browser_screenshot` also adds input
     `properties.freeze`, `properties.waitForStable` and `properties.force`,
     plus output `properties.motion` and `properties.paused`. `freeze` pauses
     the page's running animations and videos, injects a pause style, and
     restores them; `motion` names what is still moving after that, `paused`
     names what was acted on. `waitForStable` polls checksums and refuses an
     unsettled page unless `force` accepts the last frame with an `unfrozen`
     stability verdict. `properties.scale` (0.1..1) scales the CDP clip, so a
     half-scale capture reports half-size `size`. `size` is now read from the
     encoded bytes for jpeg (SOF) and webp (RIFF chunks) as well as png (IHDR);
     the field was already in the pinned output schema. No option changes the
     historic default capture.
   - `inputSchema`: `browser_click` adds `properties.button` and
     `properties.clickCount` (right/middle button, double click) and
     `browser_screenshot` adds `properties.clip`, `properties.element`,
     `properties.format` and `properties.quality` (region or element capture,
     jpeg/webp encoding and its quality). A call that passes none of them is the
     historic call. `browser_click`, `browser_type`, `browser_select`,
     `browser_hover`, `browser_upload` and `browser_screenshot` also add
     `properties.selector`: `selector` is an alias for the canonical `element`
     target, so a client that prunes undeclared keys can still name a target.
     `element` stays in each schema. The five action tools add an `anyOf`
     naming `element` and `selector` and drop `element` from `required`, so a
     selector-only call is valid for a validating client while an old `element`
     call stays valid; `browser_type` still requires `text` and `browser_upload`
     still requires `paths`. `browser_screenshot` never required a target and
     has no `anyOf`. The dispatcher refuses a call that passes both keys with a
     typed `invalid_args`/`ambiguous_target` error.
   - `metadata.description`: `browser_click`, `browser_type`, `browser_upload`,
     `browser_hover`, `browser_select`, `browser_snapshot` and
     `browser_screenshot` have a rewritten description, because each one now
     names the parameters or the result field this tree added to it; the six
     presentation trims in item 10 are declared here too. A
     description is a failing comparison by default, exactly like a schema: the
     file is the only place one may differ, and a listed tool whose description
     still equals the pin fails as a stale declaration. `title` has no declared
     exceptions at all, and no section may name a tool outside the floor.

   For every listed path the gate checks presence first — a listed addition that
   is missing or renamed fails — then removes exactly those paths, and requires
   structural equality with the pin for everything else, `required` lists
   included. An unlisted addition fails, any change to a pinned property fails,
   and an undeclared or stale description fails. The `legacy` re-capture profile
   never reads the file: a pinned binary must still emit what is committed, byte
   for byte.

   Proven both ways in `baseline/test/declared-additions.test.mjs`, through the
   real comparator and its exit code: red on an undeclared `properties.bogus`,
   red on a deleted `properties.clickCount`, red on an eighth tool's rewritten
   description, red on a declared description restored to the pin, green on the
   declared set. The two description directions also turn
   `tests/discovery.rs::tool_bodies_are_engine_bodies_under_the_prefix` red: with
   `browser_snapshot` dropped from `metadata.description` it fails with
   "description drifted … and is not declared", and with `browser_navigate` added
   it fails with "identical to the pin: a stale declaration". Both were run, not
   assumed.

10. **Six descriptions are presentation trims.** Each becomes one line stating
    the action and its required argument, to cut per-session catalog cost. No
    schema, name, title or body changes:
    - `browser_visual`: 3,469 chars upstream; one line naming `action`.
    - `browser_assert_layout`: 1,288 chars upstream; one line naming the `ref`/`selector` targets.
    - `desktop_screenshot`: 1,581 chars upstream; one line naming the capture scope.
    - `desktop_elements`: 1,462 chars upstream; one line naming `target`/`hwnd`.
    - `desktop_do`: 4,027 chars upstream; one line naming `actions`.
    - `desktop_diff`: 1,133 chars upstream; one line naming `base` and `next`.

11. **0.2.0 extends `browser_network` additively.** The published `action` enum
    gains `route`, `mock`, `abort`, `offline` and `restore` after the pinned
    `get`, `drain` and `clear`; the optional `blockedUrls` string array applies
    `Network.setBlockedURLs` to one browser context. The old actions keep their
    bodies and behavior, so existing callers are unaffected. The extended body
    is recorded in `baseline/hand-catalog.json`, not in
    `baseline/declared-additions.json`: a declaration covers a new property, not
    an added enum value on an existing property.

12. **Actionability verdict+delta on `browser_click`, `browser_type` and
    `browser_select`.** Before dispatch these tools poll a CSS-selector target
    for up to 1500 ms at a 50 ms cadence. The wait ends when the target is visible, enabled and its
    box is unchanged across two consecutive samples. Every success result
    adds `actionability` — `verdict`, `mode`, `waitedMs`, `polls`, `boundMs`,
    `visible`, `enabled`, `stable`. AX selectors, `@eN` refs and frame-scoped
    targets report `mode:"skipped"` because their own paths resolve them at
    action time. A target still unready at the bound becomes a typed
    `timeout`/`budget_exceeded` result carrying the same delta, never a silent
    miss or a stale-element crash. No input schema changed; the new output
    property is declared under `outputSchema` in
    `baseline/declared-additions.json`.

13. **Capture stability on `browser_screenshot`.** A live (non-deterministic)
    capture takes two frames and compares their decoded bytes, up to three
    pairs. An equal pair is `stability.verdict:"frozen"`; a page that keeps
    changing is `stability.verdict:"unfrozen"` with `code:"unfrozen"` and the
    two differing checksums named. Both outcomes return real pixels - only
    the claim changes. Deterministic captures are frozen by construction and
    pay no extra capture. The new output property is declared under
    `outputSchema` in `baseline/declared-additions.json`.

14. **Failure evidence on `browser_form` steps.** A step whose action fails,
    times out or uses an unknown action now carries `evidence` beside its
    `error`: a PNG screenshot of the page at that moment, the console log and
    the network log. Each piece is a real file under the temp `hand-evidence`
    directory, reported with its `path` and `bytes`, never a promise to
    capture one later. Successful steps are unchanged, and a failing step no
    longer stops the batch. `browser_form` is a hand addition outside the
    32-tool floor, so this step-level output needs no `declared-additions.json`
    entry; its published `outputSchema` records the optional property.

## External CDP engines

`GLADIUS_CDP_URL` attaches the browser domain to any CDP endpoint instead of the
owned `chrome-headless-shell`; `initialize` then reports `backend: "external-cdp"`
with that URL. The engine is not bundled, not installed and not owned: `hand`
sends commands and does not reap the process.

Compatibility is **per release and by observation**. A vendor's documented
domain list is not evidence — the measured case below had a domain that every
one of 22 sampled vendor documents omitted and the binary answered.

Measured 2026-11-18 against Obscura v0.2.7 (`obscura-x86_64-windows.zip`,
72,427,564 B), attach over loopback, `data:` URL fixture, no credentials:

| Call | Result |
|---|---|
| `browser_navigate` | ok, `{"url":"data:…","title":"probe"}` |
| `browser_cdp Accessibility.getFullAXTree` | ok, 8 nodes; `button "Press"`, `textbox "Field"`, `StaticText "body text"` |
| `browser_snapshot` | 6 nodes `@e1..@e6`, page text `Press body text` |
| `browser_click #b` | `{"ok":true}`; `#b.textContent` `"Press"` → `"1"` |
| `browser_assert_layout #b` | `pass:true`, box `8,8 19×20`, viewport 1280×720 |
| `browser_screenshot` | PNG 6,958 B, 1280×720 |

The accessibility tree — the surface `browser_snapshot`, ref clicks and ref
typing are built on — **works** on that engine even though its documentation
does not list the domain.

Known limits on that engine, measured, not inferred:

- `browser_context {action:"destroy"}` fails `-32601` (`Browser context not
  found`): its `createBrowserContext` mints a context that `Storage.getCookies`
  and `Target.disposeBrowserContext` cannot then resolve. `hand` reports the
  context as still tracked rather than claiming disposal; on this engine an
  isolated context leaks until the engine exits, and `disposeOnDetach` cannot be
  honoured.
- `browser_visual {action:"compare"}` needs the engine started with
  `--allow-file-access`; with it, `pixelsIdentical:true`, `diffPixels:0`. That
  flag also permits `file://` navigation, so `hand` does not request it and the
  user must enable it knowingly.
- `RootWebArea.name` is absent rather than empty; `browser_snapshot` normalises
  both to `""`, so a selector keyed on the document root's name does not resolve.
- Visual baselines are comparable only within one engine: `hand` records backend
  and version, so a cross-engine comparison reports an environment mismatch by
  design.

Vendor performance claims for that engine (about 30 MB resident against 200+ MB,
85 ms page load against about 500 ms, instant against about 2 s startup) were
**not** measured here and are repeated as claims, not as receipts. The owned
`chrome-headless-shell` remains the default backend; this route is an option,
not a replacement. The artifact's signature was not verified.

## Not preserved / not claimed

- A shared ref grammar or merged tools: none. Browser and desktop references
  remain separate.
- Drop-in compatibility with existing Gladius or Puppet clients without the
  prefix change.
- The rest of the specification's extension list (interception, clock control,
  traces, responsive matrices, per-context permissions). Isolated contexts and
  private-desktop launch/act-verify are implemented; remaining gaps are listed in
  `docs/CAPABILITIES.md`.
- Crash isolation between the domains: both engines share one process. Failure
  routing is by prefix; process failure is shared.
