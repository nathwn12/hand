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
PASS hand baseline: 32 baseline tools vs 44 candidate tools
  compared   32 tools
  extra      12: browser_drag, browser_wheel, browser_download, browser_assert, browser_audit, browser_trace, browser_metrics, browser_clipboard, browser_run, browser_form, browser_context, desktop_record
  metadata   0 title/description difference(s) (strict: on)
  rewrites   1 property description rewrite(s) applied
             desktop_do inputSchema.properties.allow_clipboard.description
  declared   42 addition group(s) allowed by baseline/declared-additions.json:
             browser_navigate outputSchema += properties.observed, properties.viewport
             browser_snapshot inputSchema += properties.context, properties.page
             browser_snapshot outputSchema += properties.viewport, properties.context, properties.target
             browser_snapshot metadata.description += description
             browser_screenshot inputSchema += properties.clip, properties.element, properties.format, properties.quality, properties.selector, properties.freeze, properties.waitForStable, properties.force, properties.scale
             browser_screenshot outputSchema += properties.clip, properties.element, properties.format, properties.stability, properties.motion, properties.paused
             browser_screenshot metadata.description += description
             browser_click inputSchema += properties.button, properties.clickCount, properties.selector, anyOf
             browser_click outputSchema += properties.actionability, properties.resolved
             browser_click metadata.description += description
             browser_type inputSchema += properties.selector, anyOf
             browser_type outputSchema += properties.actionability, properties.resolved
             browser_type metadata.description += description
             browser_evaluate outputSchema += properties.void
             browser_upload inputSchema += properties.selector, anyOf
             browser_upload outputSchema += properties.resolved
             browser_upload metadata.description += description
             browser_hover inputSchema += properties.selector, anyOf
             browser_hover outputSchema += properties.resolved
             browser_hover metadata.description += description
             browser_select inputSchema += properties.selector, anyOf
             browser_select outputSchema += properties.actionability, properties.resolved
             browser_select metadata.description += description
             browser_assert_layout inputSchema += properties.frame
             browser_assert_layout metadata.description += description
             browser_layout inputSchema += properties.frame
             browser_layout metadata.description += description
             browser_visual inputSchema += properties.frame
             browser_visual outputSchema += properties.frame, properties.clip
             browser_visual metadata.description += description
             desktop_screenshot inputSchema += properties.privacyMasks
             desktop_screenshot outputSchema += properties.noSteal, properties.clipboardUnchanged, properties.masks
             desktop_screenshot metadata.description += description
             desktop_diff metadata.description += description
             desktop_window inputSchema += properties.allow_foreground.description
             desktop_window outputSchema += properties.foregroundAfter, properties.foregroundBefore, properties.foregroundChanged
             desktop_window metadata.description += description
             desktop_do inputSchema += properties.allow_cursor.description, properties.allow_foreground.description, properties.cursor_restore, properties.restore_clipboard
             desktop_do outputSchema += properties.clipboard, properties.cursorMoved, properties.cursorRestored, properties.foregroundRestored
             desktop_do metadata.description += description
             desktop_elements outputSchema += properties.noSteal
             desktop_elements metadata.description += description
```

The 32-name floor is served **with exactly the allowances declared in
`baseline/declared-additions.json` and no other difference**: thirteen
`inputSchema` groups and fourteen `outputSchema` groups carry the properties
listed there, fifteen tool descriptions are rewritten there and one existing
property description is rewritten in place; every other property, `required`
list, annotation, `title`, description and name is the pin's. The twelve extras
above are hand's own tools, not legacy bodies. The live catalog is 44 tools
(36 `browser_*`, 8 `desktop_*`) against the same 32-name floor: 25 Gladius
browser bodies plus 11 hand browser tools, and 7 Puppet desktop bodies plus
`desktop_record`. The file is not a relaxed mode: an
undeclared description or any title difference fails, and so does a declared
description that still matches the pin, because the declaration says that one
was rewritten.

## Name mapping

The only catalog change is the prefix. A client migrates one-to-one:

- Gladius `navigate` → `browser_navigate` … `fetch_text` → `browser_fetch_text`
- Puppet `screenshot` → `desktop_screenshot` … `elements` → `desktop_elements`
- hand adds eleven browser tools and one desktop tool with no legacy counterpart:
  `browser_context` (isolated named contexts), `browser_wheel` and `browser_drag`
  (trusted pointer primitives), `browser_form`, `browser_download`,
  `browser_assert`, `browser_audit`, `browser_trace`, `browser_metrics`
  (measured page timings and Web Vitals), `browser_run`, `browser_clipboard`
  (page clipboard read/write through the page's `navigator.clipboard`, which in
  Chromium is the Windows system clipboard; a write replaces the owner's
  clipboard text and requires `allow_clipboard:true`; hand calls no Windows
  clipboard API itself) and `desktop_record`
  (targeted-window MJPEG AVI recording). Their bodies are new; a migration
  stays one-to-one for every legacy name.

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
  "version": "0.4.0",
  "platform": "windows-x64",
  "engines": {
    "browser": {"engine": "gladius", "sourcePin": "59c6d77", "version": "0.8.0",
                 "backend": "chromium", "caps": "render", "cdpUrl": null, "toolCount": 35},
    "desktop": {"engine": "puppet", "sourcePin": "0f790be", "version": "8.0.0",
                 "available": true, "backend": "uia+win32", "toolCount": 8}
  },
  "count": 44,
  "isolatedContexts": {"tool": "browser_context", "available": true, "…": "…"}
}
```

`browser.backend`/`caps`/`cdpUrl` are the browser engine's own `backend_info()`,
so an attached or capability-restricted engine is described as it really is.
`engines.browser.toolCount` is the engine's own `list()` (25 pinned plus
`wheel`, `drag`, `form`, `download`, `assert`, `audit`, `trace`, `metrics`,
`run` and `clipboard`). `browser_context` is published from the engine's
context module beside that list, so the wire carries 36 `browser_*` bodies and
44 tools in total.

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
- Engine unit tests remain compiled and were re-run on this tree: 136 browser
  tests pass, 255 desktop tests pass with `--skip live_` (29 live tests
  filtered out, 1 ignored).

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
   pinned ones, except the schema additions, the fifteen description rewrites
   and the one property-description rewrite declared in
   `baseline/declared-additions.json`.

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
   `properties.<name>`; a `metadata.description` entry is a bare tool name and
   a `metadata.rewrites` entry is a `{tool, schema, path}` descriptor (item 18).
   The declared set is, by group: 13 `inputSchema` groups, 14 `outputSchema`
   groups, 15 `metadata.description` names and 1 `metadata.rewrites` descriptor.
    - `inputSchema`: `browser_assert_layout` `properties.frame`; `browser_click`
      `properties.button`, `properties.clickCount`, `properties.selector`,
      `anyOf`; `browser_hover` `properties.selector`, `anyOf`; `browser_layout`
      `properties.frame`; `browser_screenshot` `properties.clip`,
      `properties.element`, `properties.format`, `properties.quality`,
      `properties.selector`, `properties.freeze`, `properties.waitForStable`,
      `properties.force`, `properties.scale`; `browser_select`
      `properties.selector`, `anyOf`; `browser_snapshot` `properties.context`,
      `properties.page`; `browser_type` `properties.selector`, `anyOf`;
      `browser_upload` `properties.selector`, `anyOf`; `browser_visual`
      `properties.frame`; `desktop_screenshot` `properties.privacyMasks`;
      `desktop_do` `properties.allow_cursor.description`,
      `properties.allow_foreground.description`, `properties.cursor_restore`,
      `properties.restore_clipboard`; `desktop_window`
      `properties.allow_foreground.description`.
    - `outputSchema`: `browser_evaluate` `properties.void`; `browser_click`
      `properties.actionability`, `properties.resolved`; `browser_hover`
      `properties.resolved`; `browser_navigate` `properties.observed`,
      `properties.viewport`; `browser_snapshot` `properties.viewport`,
      `properties.context`, `properties.target`; `browser_screenshot`
      `properties.clip`, `properties.element`, `properties.format`,
      `properties.stability`, `properties.motion`, `properties.paused`;
      `browser_select` `properties.actionability`, `properties.resolved`;
      `browser_type` `properties.actionability`, `properties.resolved`;
      `browser_upload` `properties.resolved`; `browser_visual`
      `properties.frame`, `properties.clip`; `desktop_elements`
      `properties.noSteal`; `desktop_screenshot` `properties.noSteal`,
      `properties.clipboardUnchanged`, `properties.masks`; `desktop_do`
      `properties.clipboard`, `properties.cursorMoved`,
      `properties.cursorRestored`, `properties.foregroundRestored`;
      `desktop_window` `properties.foregroundAfter`,
      `properties.foregroundBefore`, `properties.foregroundChanged`.
    - `metadata.description`: `browser_assert_layout`, `browser_click`,
      `browser_hover`, `browser_layout`, `browser_screenshot`, `browser_select`,
      `browser_snapshot`, `browser_type`, `browser_upload`, `browser_visual`,
      `desktop_diff`, `desktop_do`, `desktop_elements`, `desktop_screenshot`,
      `desktop_window` — fifteen bare tool names, each of which MUST DIFFER
      from the pin (the six presentation trims in item 10 are among them). A
      description is a failing comparison by default, exactly like a schema: the
      file is the only place one may differ, and a listed tool whose description
      still equals the pin fails as a stale declaration. `title` has no declared
      exceptions at all, and no section may name a tool outside the floor.
    - `metadata.rewrites`: one descriptor, `{tool:"desktop_do",
      schema:"inputSchema", path:"properties.allow_clipboard.description"}`
      (item 18). The meaning of the schema groups is recorded in items 10–18:
      presentation trims (10), the network action extension in the extended
      catalog rather than in the floor (11), actionability (12), capture
      stability (13), form failure evidence (14), metrics (15), trace video (16),
      desktop opt-ins (17) and the in-place `allow_clipboard` rewrite (18).

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

15. **`browser_metrics` is a hand tool outside the floor.** It reads measured
    page timings (TTFB, DOMContentLoaded, load) and Web Vitals (FCP, LCP, CLS,
    INP proxy) through read-only page evaluation. Every number is measured: an
    entry with no data is `null` with a `reason` and a per-field
    `<field>Reason`, never a fabricated zero. `context:` reads a page of a named
    isolated context. It has no pinned legacy body, so it needs no
    `declared-additions.json` entry; `tests/discovery.rs` covers its name and
    catalog count, `crates/browser-engine/src/metrics.rs` covers the schema and
    the null-with-reason rule (`metrics_schema_is_read_only_and_null_is_documented`,
    `metrics_reports_measured_values_and_reasons_for_nulls`,
    `metrics_null_lcp_start_time_is_null_with_reason`), and the live wire check
    is the `browser_metrics` step in `scripts/acceptance/live.mjs`.

16. **Trace video is MJPEG AVI, written without an external encoder.**
    `browser_trace {video:true}` records a CDP screencast into the caller's
    absolute directory for `videoDurationMs` 250–15000 (an out-of-range value
    is clamped and reported). The manifest names `container: "mjpeg-avi"`;
    cancellation removes only the files the call wrote. `video` defaults off,
    so the HAR plus step-screenshot bundle is byte-for-byte the same for callers
    that do not ask for video. Tested by
    `crates/browser-engine/tests/video_trace.rs` (`video_trace_writes_playable_avi_and_cancel_deletes_it`,
    `video_duration_above_cap_is_clamped_and_reported`) and the `browser_trace`
    step in `scripts/acceptance/live.mjs`.

17. **`desktop_do` schema additions are opt-ins, not new defaults.** The
    published `inputSchema` gains `allow_clipboard`, `restore_clipboard`,
    `allow_cursor` and `allow_foreground` beside the existing `focus_guard`; all
    default to false. The `do` handler also accepts `cursor_restore` (true by
    default), which restores the entry pointer after an opt-in burst.
    `clipboard_set` writes real `CF_UNICODETEXT` only with
    `allow_clipboard:true` and is refused `no_clipboard_policy` before
    `OpenClipboard` without it; `restore_clipboard:true` restores the prior
    text after the last action, including a partial or failed burst (empty or
    non-text content is cleared). `clipboard_get` needs no flag and reports a
    distinct `no_text_format` when the clipboard holds no `CF_UNICODETEXT`;
    that check runs before `OpenClipboard`, so it never claims an attempt that
    did not happen. A failed open/lock/set after the bounded retry (30 attempts,
    2 ms backoff, 60 ms cap) is `clipboard_unavailable` and names the stage,
    Win32 code and attempts. `allow_cursor:true` unlocks a real pointer
    move for `move` and coordinate verbs with a locked target; a point not on
    the target or its descendants is refused `point_not_target`.
    `allow_foreground:true` unlocks `focus_window`; `focus_guard:true` restores
    the entry foreground when it can. The default path still never moves the
    pointer, steals focus or writes the clipboard. These are additive properties
    on a floor tool; their allowance belongs in `baseline/declared-additions.json`,
    not in the 32 pinned bodies. Tested by
    `tests/no_steal.rs` (`every_acting_tool_declares_target_and_the_gate_flags`,
    `clipboard_write_is_refused_over_the_wire`,
    `focus_is_refused_with_or_without_the_flag`,
    `gated_burst_refuses_with_typed_codes_and_zero_events`),
    `crates/desktop-engine/src/serve.rs::do_schema_declares_clipboard_opt_in_and_restore`,
    and the `desktop_do_focus_guard` and `desktop_clipboard_roundtrip` steps in
    `scripts/acceptance/live.mjs`.

18. **One `desktop_do` property description is a rewrite; the other new
    descriptions are additions.** The floor pins
    `allow_clipboard.description` to a string saying the flag "buys nothing".
    That claim is false now: with `allow_clipboard:true`, `clipboard_set`
    performs a real `CF_UNICODETEXT` write. The published description was
    therefore rewritten in place. The floor has only `type`/`default` for
    `allow_cursor` and `allow_foreground`, so their new descriptions are
    additions, not rewrites, and are declared as additions. The real
    comparison (`node baseline/scripts/compare-catalog.mjs --serve
    target\release\hand.exe --json`) named every difference; all of them are
    intended and declared:
    - `metadata.rewrites`:
      `{tool:"desktop_do", schema:"inputSchema",
      path:"properties.allow_clipboard.description"}`.
    - `inputSchema.desktop_do` adds `properties.allow_cursor.description`,
      `properties.allow_foreground.description`, `properties.cursor_restore`
      and `properties.restore_clipboard`.
    - `outputSchema.desktop_do` adds `properties.clipboard`,
      `properties.cursorMoved`, `properties.cursorRestored` and
      `properties.foregroundRestored`. `cursorMoved` is in the published diff
      and is declared too: it is the honest read-back of real pointer input.
    For a rewrite the published value at `<schema>.<path>` MUST differ from
    the pin value (an identical value is a stale declaration), and the pin
    value is substituted before the equality check, so no other change can
    hide behind it. A path absent from the floor or from the published body
    fails with a message naming the tool, schema and path. The rule is
    implemented once in
    `baseline/scripts/lib/catalog.mjs::applyDeclaredRewrites`, consumed by
    `baseline/scripts/compare-catalog.mjs`, and mirrored in
    `tests/discovery.rs::tool_bodies_are_engine_bodies_under_the_prefix`.
    `tests/discovery.rs` accepts only `description` and `rewrites` under
    `metadata` and rejects any other key.
    `baseline/test/declared-additions.test.mjs` proves the stale and
    missing-path directions red through the real comparator and CLI, and is
    green on the declared set; `baseline/test/catalog.test.mjs` carries the
    same allowance in its synthetic candidate.

## External CDP engines

`GLADIUS_CDP_URL` attaches the browser domain to any CDP endpoint instead of the
owned Chrome-for-Testing browser; `initialize` then reports `backend: "external-cdp"`
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
**not** measured here and are repeated as claims, not as receipts. The owned full Chrome for Testing `chrome.exe` remains the default backend;
this route is an option, not a replacement. The artifact's signature was not verified.

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
