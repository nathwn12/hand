# Capabilities: verified and missing

Scope: the initial integration slice. A checked item was exercised on this
machine in this working tree; an unchecked item is missing or unverified and is
not to be claimed.

## Environment used

| | |
|---|---|
| Host | `NECESSE`, Windows 11 Pro, x64, Ryzen 7 5800X, 31.9 GB |
| Build | `cargo` 1.100.0, dev profile, `hand-mcp 0.1.0` |
| Browser | `chrome-headless-shell` 155.0.8059.84 (`Google Chrome for Testing`) |
| Browser path | `%LOCALAPPDATA%\gladius\chromium\155.0.8059.84-5ca590a4\chrome-headless-shell-win64\chrome-headless-shell.exe` |
| Browser SHA-256 | `64db9686dab042ba3282f7ab6fef98de266b015fd4d056c30760244425091d39` (matches `crates/browser-engine/scripts/chromium-manifest.json`) |

## Verified

| Capability | Evidence |
|---|---|
| One executable, one stdio MCP connection | `tests/discovery.rs` (7 tests): initialize, ping, tools/list, tools/call, server/discover, version gate |
| Both catalogs, 32 tools, exact legacy bodies | `catalog_is_exactly_the_32_prefixed_tools`, `tool_bodies_are_engine_bodies_under_the_prefix`; independent `baseline/scripts/compare-catalog.mjs` PASS 32/32, 0 metadata differences |
| Browser backend identity and availability | `initialize_identifies_version_platform_and_both_engines`: `gladius` pin, `puppet` pin, platform, counts, instructions |
| Live headless browser inspect → act → verify | `tests/browser_live.rs`: loopback fixture; navigate, AX snapshot, click by `@eN`, DOM effect proven by evaluate, type into a labelled field and read back |
| Deterministic browser screenshot | same test: `deterministic:true`, PNG > 1000 bytes, checksum present |
| Navigation invalidates refs; stale ref refused | same test: old ref after navigation returns typed `stale_ref` (prose names staleness), engine stays healthy |
| Desktop window enumeration | `tests/desktop_live.rs`: real `EnumWindows` rows with hwnd, pid, exe, rects, dpi, flags |
| Desktop semantic inspection of a background window | same test: `desktop_elements` on a real non-foreground window, backend named, `rect_space: "screen"`, explicit empty/UIA state |
| Desktop typed missing-target refusal | same test: unknown `titleContains` → tool error containing `target_gone`/`bad_target` |
| No foreground takeover or pointer movement | same test + `HAND_LIVE_DESKTOP=1 cargo test --test no_steal live_read_only_target_capture_keeps_foreground` (captured a real GitHub Desktop window, foreground stayed on the terminal) |
| Desktop no-steal static, wire and gate checks | `tests/no_steal.rs`, 12 tests: banned APIs absent from all three source trees; target on every acting tool; zero `sendinput_calls`; focus refused with and without the flag; dead target typed; clipboard write refused while the burst continues |
| Browser cancellation | `tests/cancel.rs`: typed `cancelled` immediately, no late second response, engine usable afterwards |
| Desktop cancellation admission | `tests/cancel.rs`: token published before the worker, cancelled job stays silent (direct through the production wrapper) |
| Vendored engine unit tests | `cargo test -p browser-engine`: 56/56 (includes Chromium pin-drift check); `cargo test -p desktop-engine -- --skip live_`: 203/203 |
| Clean EOF shutdown with a launched engine | `tests/lifecycle.rs`: exit 0, bounded; a manual probe observed 0.06 s, zero `chrome-headless-shell` processes before and after, and the next start swept the previous run's stale owned profile |
| Pinned browser artifact | File SHA-256 recomputed and equal to the manifest; `--version` reports 155.0.8059.84 |

Commands run (all exit 0):

```powershell
cargo test
cargo test -p browser-engine
cargo test -p desktop-engine -- --skip live_
$env:HAND_LIVE_DESKTOP='1'; cargo test --test no_steal live_read_only_target_capture_keeps_foreground
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand-mcp.exe --baseline hand
```

## Missing or unverified

These are not implemented, not exercised, or not safe to claim from this slice.

### Browser extensions from the specification

- Isolated, named browser contexts with separate cookie/storage; explicit
  create/select/teardown. (Only the engine's tabs exist, in one context.)
- Same-origin/OOPIF frame-scoped acting beyond the engine's existing `frame`
  parameters: no new frame capability was added or tested here.
- Selector engine by role/accessible name/text with scoping and uniqueness; the
  catalog keeps refs and CSS only.
- Scroll-with-read-back, double/right-click, drag/drop, downloads.
- Ordered form batches with per-step partial failure reporting.
- Extended navigation result with observed HTTP status and a versioned
  interface (the legacy `navigate` result is unchanged).
- Network interception, mocks, failure injection, offline mode.
- Deterministic clock, locale, per-context permissions, test-data reset.
- Assertions on DOM/accessibility/text/URL/network with expected-vs-observed
  verdicts beyond `assert_layout`.
- Responsive viewport matrices, contrast/typography/focus-indicator target-size
  evidence.
- Opt-in traces/artifacts with redaction and retention.
- Local fixtures and a repeatable assertion sequence through MCP.

### Desktop additions from the specification

- Launching a selected application on a private desktop; reporting whether
  containment was verified.
- Background actuation with a proven state change (UIA patterns / posted
  messages) through hand. The engine code exists, but this slice tested only
  read-only desktop behavior.
- Verifying `effect` separately from `satisfied` over hand's endpoint.
- Semantic-tree paging past one page, menus/dialogs/lists navigation evidence.
- Visual-change waits, window-lifecycle waits, cancellation of a long desktop
  flow.
- Screenshot privacy masks and multi-monitor/DPI matrix evidence.
- Clipboard reads (unchanged engine surface; not exercised here) — clipboard
  writes remain prohibited.
- Occlusion/minimized/elevated/secure-desktop reporting beyond what the engine
  already returns (partially visible in `list_windows`, not systematically
  tested).

### Lifecycle and distribution

- `oc-hand` OpenCode wiring: not added. No npm package, no publication, no
  remote.
- Attached external browser (`GLADIUS_CDP_URL`) survival of hand shutdown: not
  tested.
- Hard-kill reap/job-object behavior inside hand: inherited from the engine,
  not re-tested.
- Crash isolation between domains: none (one process by design).
- Performance gate: not measured by this slice. The repository carries a
  `fixtures/` 100-control page and a `benchmarks/` harness from the parallel
  baseline work, but no 100-sample p95 run (ping, snapshot, window enumeration,
  capture), no comparison against installed Gladius/Puppet, and no benchmark
  receipt was produced here. The specification's numeric targets remain
  unverified.
- Windows x64 only; no macOS/Linux desktop support is implied.

### Scope notes

- `browser_*` result bodies and `desktop_*` result bodies are the engines';
  hand adds only the prefix and the modern `resultType` marker.
- The engine's `visual` tool keeps its own strict/tolerance behavior; hand did
  not add or change it and this slice did not exercise it.
