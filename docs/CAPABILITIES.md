# Capabilities: verified and missing

Scope: the initial integration slice. A checked item was exercised on this
machine in this working tree; an unchecked item is missing or unverified and is
not to be claimed.

## Environment used

| | |
|---|---|
| Host | `NECESSE`, Windows 11 Pro, x64, Ryzen 7 5800X, 31.9 GB |
| Build | `cargo` 1.100.0, dev and release profiles, `hand 0.3.0` |
| Browser | full `chrome` 158.0.8127.36 (`Google Chrome for Testing`) |
| Browser path | `%LOCALAPPDATA%\gladius\chromium\158.0.8127.36-6a8ad290\chrome-win64\chrome.exe` |
| Browser SHA-256 | `f304cbc534586babe4e1e49817ab6d562f3ac9bf7d0d49f8ebb15295997f8fd6` (matches `crates/browser-engine/scripts/chromium-manifest.json`) |

The smaller `chrome-headless-shell` is an opt-in fallback selected with `GLADIUS_CHROMIUM`; full Chrome is the pinned default.

## Verified

| Capability | Evidence |
|---|---|
| One executable, one stdio MCP connection | `tests/discovery.rs` (7 tests): initialize, ping, tools/list, tools/call, server/discover, version gate |
| Both catalogs plus hand's own tools, legacy bodies byte-identical except the declared additions in `baseline/declared-additions.json` | `catalog_is_the_floor_plus_hand_additions`, `tool_bodies_are_engine_bodies_under_the_prefix` (compares each published body with `baseline/hand-catalog.json` after the declared paths are checked present and stripped; `baseline/test/declared-additions.test.mjs` proves it red on an undeclared addition and on a deleted one); independent `baseline/scripts/compare-catalog.mjs` PASS: 32/32 legacy tools compared, additional hand tools reported as extras, 10 declared addition groups, 7 description differences. The live catalog is **41** tools: the 32-name compatibility floor plus nine hand additions (`browser_context`, `browser_wheel`, `browser_drag`, `browser_form`, `browser_download`, `browser_assert`, `browser_audit`, `browser_trace`, `browser_run`). Additions are reported as extras by the independent comparison and are never entered in `baseline/declared-additions.json` |
| Isolated, named browser contexts over the real MCP route | `tests/context_wire.rs`: published schema; two contexts with separate cookie jars; legacy page and legacy tab space untouched; `targets select` refuses a context page; typed errors; H12: `browser_snapshot` and `browser_assert` read a named context page only when the call says `context:` (with `page:` when the context holds several pages), name the context and page target in the reply, and refuse an unknown context/page as a typed `invalid_target`/`not_found` instead of falling back to the legacy page, while a call without `context:` still reads the legacy page; shutdown disposes only hand's contexts (two clients attached to one external engine, the other's context, page and cookie survive) |
| Browser backend identity and availability | `initialize_identifies_version_platform_and_both_engines`: `gladius` pin, `puppet` pin, platform, counts, instructions |
| Live headless browser inspect → act → verify | `tests/browser_live.rs`: loopback fixture; navigate, AX snapshot, click by `@eN`, DOM effect proven by evaluate, type into a labelled field and read back |
| Deterministic browser screenshot | same test: `deterministic:true`, PNG > 1000 bytes, checksum present |
| 0.3.0 field bar on the live MCP route | `tests/browser_live.rs`: `selector` alias click and typed `ambiguous_target` on both keys; `void 0` returns `value:null` with `void:true` while `null` has no flag; owned launch reports 1440x900 in evaluate/navigate/snapshot/screenshot; CSS-animation page names `spin` in `motion`, `freeze` empties residual motion and names it in `paused`, `waitForStable` refuses an unsettled page and `force` accepts the last frame; jpeg and png carry `size` (webp parsing implemented, not live-measured), scale 0.5 halves it, jpeg q80 40392 B vs png 89983 B on the noise fixture; `browser_run` returns three step verdicts and honours continueOnError |
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
| No foreground takeover or pointer movement | same test + `HAND_LIVE_DESKTOP=1 cargo test --test no_steal live_read_only_target_capture_keeps_foreground` (captured a real GitHub Desktop window, foreground stayed on the terminal); `desktop_live` and `desktop_matrix` run their live proofs under the same `HAND_LIVE_DESKTOP=1` gate and name their skip on a bare session - a hosted runner has no foreground holder, measured |
| Desktop no-steal static, wire and gate checks | `tests/no_steal.rs`, 12 tests: banned APIs absent from all three source trees; target on every acting tool; zero `sendinput_calls`; focus refused with and without the flag; dead target typed; clipboard write refused while the burst continues |
| Browser cancellation | `tests/cancel.rs`: typed `cancelled` immediately, no late second response, engine usable afterwards |
| Desktop cancellation admission | `tests/cancel.rs`: token published before the worker, cancelled job stays silent (direct through the production wrapper) |
| Vendored engine unit tests | `cargo test -p browser-engine`: 91/91 (includes the B6 literal-redaction and HAR-date checks, Chromium pin-drift and full-Chrome identity); `cargo test -p desktop-engine -- --skip live_`: 215/215 |
| Clean EOF shutdown with a launched engine | `tests/lifecycle.rs`: exit 0, bounded; a manual probe observed 0.06 s, zero owned Chrome processes before and after, and the next start swept the previous run's stale owned profile |
| Pinned browser artifact | File SHA-256 recomputed and equal to the manifest; `--version` reports 158.0.8127.36 |
| Selector engine: `role=`, `role=…[name="…"]`, `name=`, `text=`, `label=` | `tests/selectors.rs` (17 tests), resolved on `Accessibility.getFullAXTree` and acted on by backend id. Uniqueness is enforced before acting: zero → `invalid_target`/`not_found`, many → `invalid_target`/`ambiguous` listing every candidate. Names match case-insensitively after whitespace normalisation, and the resolver accepts both the raw AX name and the name `browser_snapshot` prints. `text=` reads visible text with fragments concatenated, innermost owner first; `label=` accepts only `labelfor`/`labelwrapped` provenance. CSS keeps first-match, `@eN` stays snapshot-bound with typed stale refs. Measured on 158.0.8127.36: a frame's tree excludes iframe content (the iframe is one opaque node), so scope is one frame tree and the refusals say so. |
| Installed server proven, not assumed | `scripts/verify-install.mjs`: the installed binary's stamp must equal HEAD and not be `-dirty`; `--loaded` additionally requires a running process on that exact file started after it was written. The binary name is read from `Cargo.toml`, so a rename cannot leave a stale path. |
| Extended navigation result | red-line-then-green test: `browser_navigate` returns an `observed` object with `status`, `redirects` and `timings` |
| Frame-scoped layout, layout assertion and visual | red-line-then-green test: `browser_layout`, `browser_assert_layout` and `browser_visual` accept `frame=` and act inside that frame's DOM |
| Trusted drag with a data transfer | red-line-then-green test: a trusted drag carries a `dataTransfer` into the page's `dragover` and `drop` handlers. CDP has no `dragstart` dispatch, so the page's native `dragstart` sees an empty `DataTransfer`; the tool description states this and the test locks it |
| Ordered form batches | red-line-then-green test: `browser_form` runs an ordered batch and reports a per-step status and verdict |
| Browser download | red-line-then-green test: `browser_download` writes to a caller-supplied directory and reports SHA-256 and MIME; a cancelled download cleans up its partial file |
| Browser assertions | red-line-then-green test: `browser_assert` covers `dom`, `text`, `url` and `ax` with bounded polling (`ax` reads the same accessibility tree `browser_snapshot` prints); `network` answers a typed `not_implemented` |
| Browser trace | red-line-then-green test: `browser_trace` writes a HAR 1.2 wrapping the existing CDP Network ring plus one deterministic step PNG, counts literal HAR redactions (no regex, no false detection), refuses a relative dir and a `video` request as typed errors, and cancel removes only owned artifacts |
| Per-monitor v2 DPI awareness | the process sets per-monitor v2 DPI awareness and reads it back at startup; `doctor` reports it |
| Typed capture codes for frame and no-steal faults | `blank_frame`, `stale_frame`, `too_large` and `no_steal_violation` are typed codes; `no_steal_violation` fails closed and returns no pixels |
| Repeatable browser task suite | `node fixtures/run-tasks.mjs`: pass=30 pending=0 fail=0, exit 0, including t30 trace/HAR/cancel; t30 was red (`PEND waits for browser_trace`) before row B6 landed; B13: `--repeat 3` → three consecutive clean runs, pass=90 pending=0 fail=0, exit 0 |
| Network interception | red-line-then-green tests: `browser_network` `route`/`mock`/`abort`/`offline`/`restore` plus `blockedUrls`, state keyed per context and removed at every verified teardown point; a context's route dies with its context; t17/t18/t19 PASS; `net_rows` ten-run 10/0/0 |
| Viewport audit | red-line-then-green test: `browser_audit` measures six viewports (360/390/768/1024/1440/2560) for overflow, WCAG 4.5:1 contrast with the relative-luminance formula shipped in each measurement, focus rings and target sizes; per-viewport and overall verdicts; viewports restored on success and failure; t29 PASS. Four measured checks - not a screen-reader verdict or a full WCAG audit |
| Browser-focused test count | 104 by the spec's rule (`#[test]` in the browser-domain files): browser_contexts 25, browser_rows 16, reductions 14, selectors 17, determinism 10, net_rows 10, context_wire 4, pointer_frames 6, browser_live 1, navigation_blank 1 |

Commands run (all exit 0):

```powershell
cargo test
cargo test -p browser-engine
cargo test -p desktop-engine -- --skip live_
$env:HAND_LIVE_DESKTOP='1'; cargo test --test no_steal live_read_only_target_capture_keeps_foreground
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand.exe --baseline hand
node fixtures/run-tasks.mjs
```

## Missing or unverified

These are not implemented, not exercised, or not safe to claim from this slice.

### Browser extensions from the specification

- Isolated browser contexts: built and routed (`browser_context`, see above).
  Remaining limits, all stated in the tool's own `limits` and in
  `capability_info()`: storage partitioning inside one browser process, not a
  crash boundary; IndexedDB and HTTP-cache separation expected but unverified;
  context-scoped permissions are receipted by t23; no dialog auto-handling in
  a context page; a hard kill of hand is not a cleanup path for an attached
  engine.
- Frame-scoped acting: `snapshot`, `click`, `evaluate`, `type`, `hover` and
  `select` cover same-process frames and mapped OOPIFs; `layout`,
  `assert_layout` and `visual` are frame-scoped as well (verified above). A
  frame-scoped snapshot's refs are actionable only with the same `frame=`.
- Native HTML5 `dragstart`: CDP has no `dragstart` dispatch, so the page's
  native `dragstart` sees an empty `DataTransfer`. Trusted `dragover`/`drop`
  with a `dataTransfer` is verified above; that empty `dragstart` is the stated
  limit, not a drop driven end to end from `dragstart`.
- Assertions: `browser_assert` covers `dom`, `text`, `url` and `ax` with
  bounded polling (verified above). Its `network` kind answers a typed
  `not_implemented` and is not claimed.
- Responsive viewport audit (row B7) is landed and receipted above; its
  remaining limit is named there: four measured checks, not a screen-reader
  verdict or a full WCAG audit.
- Opt-in traces/artifacts (row B6): `browser_trace` writes a HAR 1.2 over
  `browser.network_get`'s CDP ring and deterministic step captures into a
  caller's absolute directory; `redact[]` is literal and counted, never regex
  or automatic secret detection; cancel deletes only the names it wrote. The
  `video` field is a named typed `not_implemented` - no CDP screencast wrapper
  is shipped. Retention expiry is not implemented: retention is the caller's
  directory and retention cleanup is cancel's owned-file removal.

### Desktop additions from the specification

- UIA pattern actuation with a proven state change on a private desktop.
- Verifying `effect` separately from `satisfied` over hand's endpoint.
- Semantic-tree paging past one page, menus/dialogs/lists navigation evidence.
- Visual-change waits, window-lifecycle waits, cancellation of a long desktop
  flow.
- Screenshot privacy masks are receipted (uniform black, named, no cross
  leak); the 14-case capture matrix landed with 8 passes at 0 px anchor error
  and 6 Unverified-named on hardware (scales 125/150/200, a second monitor,
  an elevated process, the UAC secure desktop).
- Clipboard reads (unchanged engine surface; not exercised here) — clipboard
  writes remain prohibited.
- Occluded capture is receipted (PrintWindow, byte-identical to the
  unoccluded frame); a minimized window returns the typed
  `window_minimized` and restores. The secure-desktop and elevated cases
  stay Unverified on hardware and are never passed by omission.

Hidden launches use a kill-on-close job assigned at process creation. The
creation-time job attribute requires Windows 10 version 1607 or later and the
launch fails closed when cover cannot be established. Breakaway is not enabled;
an independently running external process is not owned or covered. An explicit
existing target takes precedence over the hidden-launch request.

### Lifecycle and distribution

- `oc-hand` OpenCode wiring: an installed copy serves the local
  OpenCode config; its version follows the release. Public remote
  `github.com/nathwn12/hand`; `hand-mcp` is published on npm through
  `0.1.1` and `0.2.0`, with `0.3.0` shipping as this release.
- Product and executable are `hand`; the repo, crate and binary were renamed
  from `hand-mcp`. Fixture ids, baseline captures and benchmark receipt schemas
  keep the `hand-mcp` names on purpose: they record what was measured, and
  rewriting a fingerprint would invalidate archived receipts.
- The live capture in `baseline/hand-catalog.json` is a **baseline debug
  capture**, not release evidence. Its receipt names the source SHA of the debug
  binary that produced it (the honest convention), and it is rebuilt from the
  final tree whenever the published catalog changes. No release claim rides on
  it; the separate benchmark build receipts are the release identity evidence.
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

  `desktop-capture` is **measured and passing**. Two consecutive release runs on
  an active desktop each produced 100 attempts against the 500 ms target: 100
  passed, 0 failed, p95 245.802 ms and 257.813 ms. The raw `cursorUnchanged` and
  `foregroundUnchanged` flags can read false while a person uses the machine;
  the attribution engine separates owner input from tool action, and the binary
  links no pointer or foreground writer, so the harness verdict - not a raw
  flag - is the no-steal gate. The release identity is attested by a recorded
  build receipt under `benchmarks/receipts/builds`, not a self-report:
  `compare.mjs` re-checks the embedded commit, profile and artifact hash
  against it before a receipt is claimable.
- Windows x64 only; no macOS/Linux desktop support is implied.

### Scope notes

- `browser_*` result bodies and `desktop_*` result bodies are the engines';
  hand adds only the prefix and the modern `resultType` marker.
- The engine's `visual` tool keeps its own strict/tolerance behavior; hand did
  not add or change it and this slice did not exercise it.

### Known limits

- WebP screenshot `size` parsing is implemented but not live-measured; JPEG and
  PNG are.
- The 500 KB target for a 2560-wide JPEG q80 shot is page-dependent. On the
  seeded-noise fixture, JPEG q80 is 40,392 B against PNG 89,983 B.
- The desktop test suite serializes its live tests inside one binary. An
  external pointer or foreground actor can still flake a no-steal assertion;
  the attribution engine, not a raw flag, remains the gate.
- The browser-only visual-QA lane that drove this release did not exercise the
  desktop tools. They remain Unverified by that lane, not faulty.
