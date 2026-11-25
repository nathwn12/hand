# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
