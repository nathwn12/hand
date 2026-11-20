# Capabilities: verified and missing

Scope: the initial integration slice. A checked item was exercised on this
machine in this working tree; an unchecked item is missing or unverified and is
not to be claimed.

## Environment used

| | |
|---|---|
| Host | `NECESSE`, Windows 11 Pro, x64, Ryzen 7 5800X, 31.9 GB |
| Build | `cargo` 1.100.0, dev and release profiles, `hand 0.2.0` |
| Browser | `chrome-headless-shell` 155.0.8059.84 (`Google Chrome for Testing`) |
| Browser path | `%LOCALAPPDATA%\gladius\chromium\155.0.8059.84-5ca590a4\chrome-headless-shell-win64\chrome-headless-shell.exe` |
| Browser SHA-256 | `64db9686dab042ba3282f7ab6fef98de266b015fd4d056c30760244425091d39` (matches `crates/browser-engine/scripts/chromium-manifest.json`) |

## Verified

| Capability | Evidence |
|---|---|
| One executable, one stdio MCP connection | `tests/discovery.rs` (7 tests): initialize, ping, tools/list, tools/call, server/discover, version gate |
| Both catalogs plus hand's own tools, legacy bodies byte-identical except the declared additions in `baseline/declared-additions.json` | `catalog_is_the_floor_plus_hand_additions`, `tool_bodies_are_engine_bodies_under_the_prefix` (compares each published body with `baseline/hand-catalog.json` after the declared paths are checked present and stripped; `baseline/test/declared-additions.test.mjs` proves it red on an undeclared addition and on a deleted one); independent `baseline/scripts/compare-catalog.mjs` PASS: 32/32 legacy tools compared, 3 extras, 10 declared addition groups, 7 description differences. The live catalog is **35** tools: the 32-name compatibility floor plus `browser_context`, `browser_wheel` and `browser_drag`, which hand adds on top and which `baseline/hand-catalog.json` records under `live` |
| Isolated, named browser contexts over the real MCP route | `tests/context_wire.rs`: published schema; two contexts with separate cookie jars; legacy page and legacy tab space untouched; `targets select` refuses a context page; typed errors; shutdown disposes only hand's contexts (two clients attached to one external engine, the other's context, page and cookie survive) |
| Browser backend identity and availability | `initialize_identifies_version_platform_and_both_engines`: `gladius` pin, `puppet` pin, platform, counts, instructions |
| Live headless browser inspect → act → verify | `tests/browser_live.rs`: loopback fixture; navigate, AX snapshot, click by `@eN`, DOM effect proven by evaluate, type into a labelled field and read back |
| Deterministic browser screenshot | same test: `deterministic:true`, PNG > 1000 bytes, checksum present |
| Navigation invalidates refs; stale ref refused | same test: old ref after navigation returns typed `stale_ref` (prose names staleness), engine stays healthy |
| Pointer completeness: right/middle click, double click, wheel with read-back, drag | `tests/pointer_frames.rs`: right click raises the page's own `contextmenu` and the menu appears as `menu`/`menuitem` nodes in the next snapshot; `clickCount:2` raises exactly one `dblclick`; a 400 px wheel scrolls (`scrollY` 400) and the reply reports the measured offset, `moved:false` over a page that cannot scroll; a drag reorders the DOM with `isTrusted:true` pointer events (no synthetic DragEvent) and dispatches one positioning move, press, the moves and the release |
| Element and clip screenshot, format passthrough | same test: `element` captures the element's border box (PNG IHDR width equals `browser_layout`'s box width), two `deterministic:true` captures share a checksum, `jpeg`/`webp` with `quality` answer their own `mimeType`, `png` + `quality` is refused by name, an unknown ref is typed `stale_ref` |
| Out-of-process frame scoping | same test: a cross-site iframe (`127.0.0.1` vs `localhost`, site isolation on) is addressable by its frame id - `browser_snapshot frame=` returns that frame's nodes and `browser_click frame=` lands inside it (proven by the child's own DOM); a frame id no session owns is typed `invalid_target` from snapshot, click and evaluate, never a main-frame action |
| Desktop window enumeration | `tests/desktop_live.rs`: real `EnumWindows` rows with hwnd, pid, exe, rects, dpi, flags |
| Desktop semantic inspection of a background window | same test: `desktop_elements` on a real non-foreground window, backend named, `rect_space: "screen"`, explicit empty/UIA state |
| Desktop typed missing-target refusal | same test: unknown `titleContains` → tool error containing `target_gone`/`bad_target` |
| Private-desktop act and effect | `tests/desktop_fixture.rs`: controlled app receives posted input and independently records the resulting state; the tool's unverified delivery verdict is not promoted to proof |
| Hidden launch cleanup | `tests/hidden_cleanup.rs`: hard server kill reaps the controlled target and child; a failed launch reports `ok:false` and `isError:true`; the test cleanup terminates only identity-verified processes (image plus creation time) |
| Browser and desktop shutdown bounds | `tests/server_regressions.rs` (11 tests): a timed-out capture still restores; a timed-out or cancelled call can still land its action and says so; a full pool answers typed `busy`; EOF drain is globally bounded and names the abandoned work; a blocking desktop job cannot stop the process indefinitely |
| No-steal and hidden safety after shutdown changes | `tests/no_steal.rs` (12 tests) and `tests/hidden_cleanup.rs` (7 tests) still pass on this tree |
| No foreground takeover or pointer movement | same test + `HAND_LIVE_DESKTOP=1 cargo test --test no_steal live_read_only_target_capture_keeps_foreground` (captured a real GitHub Desktop window, foreground stayed on the terminal) |
| Desktop no-steal static, wire and gate checks | `tests/no_steal.rs`, 12 tests: banned APIs absent from all three source trees; target on every acting tool; zero `sendinput_calls`; focus refused with and without the flag; dead target typed; clipboard write refused while the burst continues |
| Browser cancellation | `tests/cancel.rs`: typed `cancelled` immediately, no late second response, engine usable afterwards |
| Desktop cancellation admission | `tests/cancel.rs`: token published before the worker, cancelled job stays silent (direct through the production wrapper) |
| Vendored engine unit tests | `cargo test -p browser-engine`: 69/69 (includes Chromium pin-drift check); `cargo test -p desktop-engine -- --skip live_`: 203/203 |
| Clean EOF shutdown with a launched engine | `tests/lifecycle.rs`: exit 0, bounded; a manual probe observed 0.06 s, zero `chrome-headless-shell` processes before and after, and the next start swept the previous run's stale owned profile |
| Pinned browser artifact | File SHA-256 recomputed and equal to the manifest; `--version` reports 155.0.8059.84 |
| Selector engine: `role=`, `role=…[name="…"]`, `name=`, `text=`, `label=` | `tests/selectors.rs` (17 tests), resolved on `Accessibility.getFullAXTree` and acted on by backend id. Uniqueness is enforced before acting: zero → `invalid_target`/`not_found`, many → `invalid_target`/`ambiguous` listing every candidate. Names match case-insensitively after whitespace normalisation, and the resolver accepts both the raw AX name and the name `browser_snapshot` prints. `text=` reads visible text with fragments concatenated, innermost owner first; `label=` accepts only `labelfor`/`labelwrapped` provenance. CSS keeps first-match, `@eN` stays snapshot-bound with typed stale refs. Measured on 155.0.8059.84: a frame's tree excludes iframe content (the iframe is one opaque node), so scope is one frame tree and the refusals say so. |
| Installed server proven, not assumed | `scripts/verify-install.mjs`: the installed binary's stamp must equal HEAD and not be `-dirty`; `--loaded` additionally requires a running process on that exact file started after it was written. The binary name is read from `Cargo.toml`, so a rename cannot leave a stale path. |

Commands run (all exit 0):

```powershell
cargo test
cargo test -p browser-engine
cargo test -p desktop-engine -- --skip live_
$env:HAND_LIVE_DESKTOP='1'; cargo test --test no_steal live_read_only_target_capture_keeps_foreground
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand.exe --baseline hand
```

## Missing or unverified

These are not implemented, not exercised, or not safe to claim from this slice.

### Browser extensions from the specification

- Isolated browser contexts: built and routed (`browser_context`, see above).
  Remaining limits, all stated in the tool's own `limits` and in
  `capability_info()`: storage partitioning inside one browser process, not a
  crash boundary; IndexedDB and HTTP-cache separation expected but unverified;
  no per-context permissions; no dialog auto-handling in a context page; a hard
  kill of hand is not a cleanup path for an attached engine.
- Frame-scoped acting beyond `snapshot`, `click`, `evaluate`, `type`, `hover`,
  `select` and `upload`: those cover same-process frames and mapped OOPIFs; a
  frame-scoped snapshot's refs are actionable only with the same `frame=`.
  `layout`, `assert_layout` and `visual` remain main-frame only.
- Downloads, and HTML5 `dragstart`/`drop` as a distinct primitive: `browser_drag`
  sends one trusted pointer path, which the engine's own drag machinery sees on
  a page that uses it, but no test pins a native `DataTransfer` drop here.
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

- UIA pattern actuation with a proven state change on a private desktop.
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

Hidden launches use a kill-on-close job assigned at process creation. The
creation-time job attribute requires Windows 10 version 1607 or later and the
launch fails closed when cover cannot be established. Breakaway is not enabled;
an independently running external process is not owned or covered. An explicit
existing target takes precedence over the hidden-launch request.

### Lifecycle and distribution

- `oc-hand` OpenCode wiring: live at `C:\Users\nathan\.local\bin\hand.exe`, 35
  tools. Private remote `github.com/nathwn12/hand`. No npm package and nothing
  published; the npm name `hand-mcp` is reserved by intent only.
- Product and executable are `hand`; the repo, crate and binary were renamed
  from `hand-mcp`. Fixture ids, baseline captures and benchmark receipt schemas
  keep the `hand-mcp` names on purpose: they record what was measured, and
  rewriting a fingerprint would invalidate archived receipts.
- Attached external browser (`GLADIUS_CDP_URL`) survival of hand shutdown: not
  tested end to end. A clean detach preserves an external engine, but a restore
  cut at the EOF drain bound is reported `UNVERIFIED`, and no external CDP
  fixture exercises that path in the suite.
- Context page `source`/`titleNote`: a page's `url` and membership come from CDP
  target info, while its `title` is labelled `targetInfo+liveRead` when the
  document itself supplied it and `targetInfo` when the engine's cached value is
  all that exists; a failed live read carries a `titleNote` instead of pretending
  the cached title is current. `tests/browser_contexts.rs` covers both labels.
- Hard-kill reap/job-object behavior inside hand: inherited from the engine,
  not re-tested.
- Crash isolation between domains: none (one process by design).
- Performance gate: partially measured. `benchmarks/` binds each receipt to a
  fingerprinted fixture, a fingerprinted browser and a reproduced local release
  build, and the comparison gate rejects debug or unverifiable builds.
  Measured on this machine (release binaries, 10 warmups + 100 attempts):

  | Scenario | hand p95 | baseline p95 |
  |---|---:|---:|
  | warm-ping | 0.071 ms | Gladius 0.115 ms, Puppet 0.093 ms |
  | browser-snapshot | 18.6 ms | Gladius 29.1 ms |
  | desktop-enumerate | 1.854 ms | Puppet 1.683 ms |

  Enumerate is one shared measurement: both products ran against the same
  content-addressed fixture (`sha256:d6a8a05b…`) on the same machine hash, 100
  attempts, 0 failures each. hand is **10 % slower** than Puppet here, and that
  is what the receipt says.

  `desktop-capture` is **measured but not claimable**. One run on an active
  desktop produced 100 attempts at p95 247.3 ms against the 500 ms target, but 37
  attempts failed the safety precondition (`cursorUnchanged` false, one
  `foregroundUnchanged` false), so the receipt's verdict is `fail` and the release
  gate stays blocked. Over the 63 attempts that did hold the precondition, p95 was
  224.7 ms with a 232.9 ms maximum — under target, but a subset is not a receipt.
  A clean row needs an idle desktop for the full 100. The run also rejected the
  binary as an unverified release because it was invoked from its install path;
  `target\release\hand.exe` is the path the identity check accepts.
- Windows x64 only; no macOS/Linux desktop support is implied.

### Scope notes

- `browser_*` result bodies and `desktop_*` result bodies are the engines';
  hand adds only the prefix and the modern `resultType` marker.
- The engine's `visual` tool keeps its own strict/tolerance behavior; hand did
  not add or change it and this slice did not exercise it.
