# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

- The server's `instructions` index all 40 tools, including `browser_audit` and `browser_trace`.
- The compatibility floor records the additive `browser_network` action extension through its generator (`applyHand020Extensions`), documented in `docs/COMPATIBILITY.md`.

### Fixed

- The server instructions omitted `audit` and `trace`.
- The `ERR_ABORTED` startup race is retried in every test binary's first navigation, not only the lane-owned ones.
- Two dead counter increments in the desktop burst path and a deprecated atomic rename are cleared; the release build compiles warning-free.
