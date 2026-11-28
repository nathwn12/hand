# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-11-28

### Added

- `browser_metrics`: an eleventh hand browser tool, reading measured page timings (TTFB, DOMContentLoaded, load) and Web Vitals (FCP, LCP, CLS, INP proxy) through read-only page evaluation. Every number is measured; an entry with no data is `null` with a `reason` and a per-field `<field>Reason`, never a fabricated zero. `context:` reads a page of a named isolated context. The catalog is now 44 tools: 36 browser, 8 desktop; the pinned floor stays 32.
- `browser_trace` video: opt-in MJPEG AVI written by hand from a CDP screencast, with no external encoder. Every screencast frame is acknowledged and the AVI is constant-rate: a late or tail slot repeats the previous frame so the file spans the requested duration. The manifest reports `frames` (all slots), `capturedFrames` (fresh screencast frames) and `paddedFrames` (repeats) beside `durationMs`, `fps`, `bytes` and `container: "mjpeg-avi"`. `videoDurationMs` is 250–15000 (an out-of-range value is clamped and reported), and cancellation removes only the files the call wrote. `video` defaults off, so the existing HAR plus step-screenshot bundle is unchanged for callers that do not ask for it.
- `desktop_do` clipboard opt-in: `allow_clipboard:true` permits one real `CF_UNICODETEXT` write; without it `clipboard_set` is refused `no_clipboard_policy` before the clipboard is opened. `restore_clipboard:true` snapshots the previous text before the first write and restores it after the last action, including a partial or failed burst; empty or non-text content is cleared on restore. Both flags default to false: restore is opt-in, not automatic, and `clipboard_get` reads never need either flag.
- `desktop_do` pointer and focus opt-in: `allow_cursor:true` unlocks a real pointer move for `move` and coordinate pointer verbs, only with a locked target; a point not on the target or its descendants is refused `point_not_target`, and without a target the call refuses `target_required`. `cursor_restore:true` (the default) restores the entry pointer after the last action. `allow_foreground:true` unlocks `focus_window`; `focus_guard:true` restores the entry foreground when the call changed it.
- `browser_clipboard`: read or write the PAGE clipboard through `navigator.clipboard` (`Runtime.evaluate` with `awaitPromise:true` and `userGesture:true`; `Browser.grantPermissions` for `clipboardReadWrite` and `clipboardSanitizedWrite` scoped to the page's real origin). In Chromium `navigator.clipboard` IS the Windows clipboard: a write REPLACES the owner's clipboard text (the previous contents are neither snapshotted nor restored) and requires `allow_clipboard:true`; a read needs no flag. hand calls no Windows clipboard API itself and synthesises no desktop input. `action=write` requires non-empty `text` capped at 1 MiB, with an oversized write typed `too_large` before the page is touched; `action=read` returns the page clipboard text capped at 1 MiB with `truncated` when it was cut, and a rejected clipboard call is typed `clipboard_unavailable` with the page's own error message. `context:` reads or writes a page of a named isolated browser context (`page:` selects a specific page); `about:blank` and `data:` pages have no real origin and are refused.
- `desktop_record`: record ONE targeted window to a playable MJPEG AVI file without ever activating the window, moving the pointer or touching the clipboard. `target` is required (an object or selector string), resolved to one top-level window and revalidated before every frame with no fallback to the foreground window; `out` is required, an absolute `.avi` path whose parent exists and whose file does not, with a pre-existing, relative or wrong-extension path refused as typed `invalid_args` before any capture. `duration_ms` 250..15000 (default 3000), `fps` 1..15 (default 8), `max_px` 160..1280 (default 960) and `quality` 1..100 (default 75) are clamped and named in `warnings[]`; overrun slots are dropped (and, once a frame exists, the previous JPEG is written again to keep the AVI constant-rate) and counted in `droppedFrames`; `capturedFrames` counts only fresh captures and `frames` counts every AVI chunk. Foreground, pointer and clipboard invariants are measured before and after like screenshot. If the target vanishes mid-recording the typed result names the frames already written and the partial AVI is kept; if no frame was captured the result is `capture_unavailable` and no file is written.

### Changed

- The default path is unchanged: no pointer movement, no focus theft and no clipboard write. Every new capability above is behind an explicit opt-in, so an existing caller keeps the no-steal contract byte for byte.
- Capture and desktop replies now name the three pointer/focus writers the binary links for the opt-in paths in `noSteal.optInImports`; `bannedImports` lists every other writer and must stay empty.

### Fixed

- `browser_assert` refuses unknown or empty expectation keys instead of silently ignoring them: an ignored key became an empty `contains` needle that passed.
- `browser_run` counts an assertion whose verdict is false (or the string `"fail"` for `ax`) as an `assert_failed` step, so the batch's `ok` is false, and reports `assertions {passed, failed}` over steps that carried a verdict.
- `browser_audit`'s top-level verdict now means every requested check measured and passed, `measured` carries coverage alone, and the page's focus is restored with a failed restore named rather than reported as success.
- `desktop_window`'s `allow_foreground` is a permission for focus/maximize only; the other verbs always proceed no-steal.
- `desktop_do {actions:[]}` is now a typed `invalid_args`/`bad_value` before any action runs; a dry run validates but reports `delivered:"none"` on every result.
- The clipboard open path retries transient contention (30 attempts, 2 ms backoff, 60 ms cap), naming the stage and Win32 error in `clipboard_unavailable`.
- `desktop_do` `clipboard_get` reports a distinct `no_text_format` result when the clipboard is empty or holds no `CF_UNICODETEXT`; the check runs before `OpenClipboard`, so it never reports a retry that did not happen.

## [0.3.4] - 2026-11-26

### Changed

- `npm/README.md` follows the short install-first page: What it does, Configure it and Known limits fold into Tools and Requirements; the retired signature-glyph footer is gone; Status precedes uniform Links with the MIT licence last.
- `npm/package.json` carries a benefit-led description and aligned discovery keywords; the licence entry points at `npm/LICENSE`.

## [0.3.3] - 2026-11-25

### Fixed

- H12: `browser_snapshot` and `browser_assert` reach a named isolated browser context page through an explicit `context:` argument (plus `page:` when the context holds several pages). The reply names the context and page target read. An unknown context or page is a typed `invalid_target`/`not_found` that never falls back to the legacy page; several pages without `page:` is a typed `invalid_args`/`ambiguous_target` listing them. A call without `context:` reads the legacy page unchanged, and `targets select` still refuses a context page.

## [0.3.2] - 2026-11-24

### Fixed

- Modern MCP results now carry `ttlMs` and `cacheScope` beside `resultType` on `tools/list` and `server/discover`, so OpenCode 2.4.1 accepts a 2026-07-28 peer. Legacy results are unchanged.

## [0.3.1] - 2026-11-23

### Changed

- Release tags bind to the published source commit. An existing remote tag is left untouched instead of being retagged; the full gate runs locally and CI only publishes the gated artifact.
- Weekly Dependabot version updates added. README lists all 34 browser tools and `ai/llms.txt` carries a machine-readable project summary.

## [0.3.0] - 2026-11-22

### Added

- `browser_run`: ordered browser batches of 1..64 steps. Each step reports `{index, tool, status, ms, result?|error?}` with its own structured content as evidence, plus an `ok`/`failed`/`ms` aggregate. `continueOnError` (default false) stops at the first failing step; `timeoutMs` defaults to 30000. `run` and `close` are excluded from the tool enum. The catalog is now 41 tools: 34 browser, 7 desktop.
- `selector` is a published alias for `element` on click, type, select, hover, upload and screenshot; passing both is a typed `invalid_args`/`ambiguous_target` refusal. Argument errors now carry the sorted `received` keys.
- `browser_evaluate` returns `value:null` with `void:true` for a valueless result; an explicit `null` stays unflagged. Internal eval helpers keep refusing undefined.
- `initialize.instructions` carries a generated parameter line per tool: required keys first, then optional keys, built from the same prefixed catalog the server publishes (2772 bytes at 41 tools).
- Owned Chromium launches pin 1440x900 (window flag plus device-metrics override); an attached engine keeps its owner's viewport. Navigate, snapshot and screenshot echo the live `{width,height}`.
- Screenshot `freeze` pauses running animations and videos, injects a pause style and restores them; `motion` names residual sources, `paused` names what was acted on. `waitForStable` polls checksums and refuses an unsettled page unless `force` accepts the last frame with an `unfrozen` verdict.
- Screenshot `scale` (0.1..1) scales the CDP clip; `size` is read from png IHDR and jpeg SOF; webp RIFF parsing is implemented but not live-measured. Measured on the 320x200 noise fixture: jpeg q80 40392 bytes, png 89983 bytes; scale 0.5 reported 160x100.

### Changed

- The compatibility floor gained declared input/output properties for the fields above; the floor itself and the 32 pinned bodies are unchanged. The generated instruction index replaced the hand-maintained tool list.

### Fixed

- Discovery, not capture: `format`, `quality` and `fullPage` already existed in 0.2.0. Clients pruned undeclared keys before hand saw them, so adding `selector` to the published schema and generating the parameter index closed the real gap.

## [0.2.0] - 2026-11-21

### Added

- `browser_audit`: responsive audit at six viewports (360, 390, 768, 1024, 1440, 2560) measuring overflow, WCAG 4.5:1 contrast with the relative-luminance formula shipped in every measurement, focus rings and target sizes; per-viewport and overall verdicts; viewports restored on success and failure.
- `browser_trace`: opt-in artifact bundle in a caller-supplied absolute directory - one HAR 1.2 wrapping the live CDP network log plus one deterministic screenshot per recorded step, literal-only counted redaction, and a cancel that removes only the files it wrote.
- Actionability on `browser_click`, `browser_type` and `browser_select`: a bounded wait for visible, enabled, geometrically stable targets, reported as a verdict+delta; a target that never becomes actionable answers a typed timeout. A selector that does not parse refuses typed on the first poll.
- Capture stability on `browser_screenshot`: a double-capture comparison; frames that keep differing are reported typed `unfrozen` with checksums, never pretended final.
- Failure evidence on `browser_form` steps: a failing step carries a screenshot, the console log and the network log beside its status.
- Network interception on `browser_network`: `route`, `mock`, `abort`, `offline` and `restore` plus `blockedUrls`, state keyed per context and removed at every verified teardown point.
- `ax` assertions in `browser_assert`, reading the same accessibility tree `browser_snapshot` prints.
- Desktop: a 14-case capture matrix with 1-px anchor proof, hosted-window locking with appId provenance, typed capture codes for minimized windows, privacy masks with no cross leak, and restore-on-exit semantics. Six cases stay Unverified on hardware (monitor scales 125/150/200, a second monitor, an elevated process, the secure desktop) and are never passed by omission.
- Browser-focused tests grown from 52 to 104; the 30-task browser suite passes three consecutive clean runs.

### Changed

- The server's `instructions` index all 41 tools, including `browser_audit` and `browser_trace`.
- The compatibility floor records the additive `browser_network` action extension through its generator (`applyHand020Extensions`), documented in `docs/COMPATIBILITY.md`.

### Fixed

- The server instructions omitted `audit` and `trace`.
- The `ERR_ABORTED` startup race is retried in every test binary's first navigation, not only the lane-owned ones.
- Two dead counter increments in the desktop burst path and a deprecated atomic rename are cleared; the release build compiles warning-free.

## [0.1.1] - 2026-11-20

### Added

- Initial public release: 38 tools over the pinned, credited Gladius browser engine and Puppet desktop engine. Quiet by default; typed errors; a compact published catalog (instructions 1,301 B to 725 B, `tools/list` 74,054 B to 61,560 B).
