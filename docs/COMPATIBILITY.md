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
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand-mcp.exe --baseline hand
```

Observed: `PASS hand baseline: 32 baseline tools vs 32 candidate tools`,
`0 title/description difference(s)`.

## Name mapping

The only catalog change is the prefix. A client migrates one-to-one:

- Gladius `navigate` → `browser_navigate` … `fetch_text` → `browser_fetch_text`
- Puppet `screenshot` → `desktop_screenshot` … `elements` → `desktop_elements`

`tests/discovery.rs::tool_bodies_are_engine_bodies_under_the_prefix` compares
every published tool object with the engine's own catalog, asserting equality of
every field except `name`. `tests/discovery.rs::catalog_is_exactly_the_32_prefixed_tools`
asserts count, uniqueness and namespacing.

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
  "version": "0.1.0",
  "platform": "windows-x64",
  "engines": {
    "browser": {"engine": "gladius", "sourcePin": "59c6d77", "version": "0.8.0",
                 "backend": "chromium", "caps": "render", "cdpUrl": null, "toolCount": 25},
    "desktop": {"engine": "puppet", "sourcePin": "0f790be", "version": "8.0.0",
                 "available": true, "backend": "uia+win32", "toolCount": 7}
  },
  "count": 32
}
```

`browser.backend`/`caps`/`cdpUrl` are the browser engine's own `backend_info()`,
so an attached or capability-restricted engine is described as it really is.

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
   once with both engines under `engines`. Tool bodies are unchanged.

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

## Not preserved / not claimed

- A shared ref grammar or merged tools: none. Browser and desktop references
  remain separate.
- Drop-in compatibility with existing Gladius or Puppet clients without the
  prefix change.
- Any added capability from the specification's extension list (isolated
  contexts, interception, clock control, traces, responsive matrices, desktop
  private-desktop launch and act/verify, etc.). Missing work is listed in
  `docs/CAPABILITIES.md`.
- Crash isolation between the domains: both engines share one process. Failure
  routing is by prefix; process failure is shared.
