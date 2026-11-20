# plan.md — hand 0.2.0 to pristine

Mutable by design. Revised mid-flight by the primary or the owner; every revision is a commit so the history of the plan is auditable. The spec of record: `handoff-spec.md` (Desktop), `handoff-spec-0.2.0-addendum.md` (Desktop), this file.

## 0. The goal, sharpened

A pristine 0.2.0: every P0 row is landed with a red-line-then-green test and a live receipt, or it is a typed hidden stub, or it is named Unverified with the exact reason. No claim without a receipt. A stranger verifies the product from its own gates without reading the source.

Stop condition: every row below reaches `done`, `stub-honest`, or `Unverified-named`, AND the seal (§6) is green. Release, push and install stay owner-gated and are NOT part of this plan.

Two counters, always reported: **verified** and **scaffolded**. Only the first is a claim.

## 1. Scope corrections this plan makes

1. **B6 `browser_trace` returns to 0.2.0.** Task 30 of the suite is "trace, HAR and cancel cleanup"; without B6 the suite can never reach 30. The addendum's deferral is superseded.
2. **B10 stays partial, permanently.** CDP has no `dragstart` dispatch; the payload reaches drop handlers but not the page's native dragstart. Measured, described, test-locked. No amount of work changes a platform limit.
3. **Hardware-gated cases stay Unverified, never passed by omission:** mixed-DPI (needs a second monitor), elevated/UIPI, secure-desktop. The spec's own rule.

## 2. Where we start from (measured, 2026-11-21)

- HEAD `66e4dbb`, branch `feat/0.2.0`, tree clean, 37 commits unpushed. Gate: `cargo test` green ×4 runs.
- **Verified rows: 8 of 24** (B12, B9, B11, B5, B2, D10, D14, D13). B10 partial. D9 half (DPI done, matrix absent).
- Suite: `pass=21 pending=9 fail=0`. Browser-focused tests: ~58 of the 104 target.
- Remaining: 15 rows + 5 reductions.

## 3. Remaining work, by lane

One lane per engine at a time (`tools.rs`/`browser.rs` and the shared `target/` make parallel browser lanes a conflict, not a speedup). Fixture and JS work parallels freely.

### Lane B — browser engine (ordered)

| # | Row | Notes |
|---|---|---|
| B1 | Real Chrome for Testing 158 pin, shell 155 → documented fallback | first: it changes the binary every later proof runs on; diff the suite on shell vs Chrome |
| B4 | Deterministic clock, locale, timezone, permissions grant, test-data reset | context-scoped; no prompt UI, programmatic grants only |
| B8 | Storage state export/import, auth reuse | jars today; export/import per context |
| B3 | Network route/mock/abort/offline/restore | from its private `net.rs`; restore proven on cancel, timeout, EOF |
| B6 | `browser_trace`: HAR, screenshots, redaction, cleanup | restored to scope; wraps B3's capture |
| B7 | `browser_audit` fill: 6 viewports, contrast, typography, focus ring, target size, overflow | from its private `audit.rs` |
| — | Reduction: bounded actionability wait for click/type/select | visible, stable, enabled, receiving events; then act |
| — | Reduction: canvas/WebGL stability — double-capture retry plus explicit `unfrozen` report | stops silent visual flapping |
| — | Reduction: failure evidence bundle | screenshot + console + network at the failing step |
| B13 | Suite completion: 30 of 30 pass, 3 consecutive clean runs | last; consumes every row above |

### Lane D — desktop engine (ordered)

| # | Row | Notes |
|---|---|---|
| D3 | `effect: verified\|pattern-returned\|unchanged` on every UIA act | readback, never assumed |
| D4 | `set_value` and type-with-element scoped to the hidden desktop only | closes the policy hole |
| D11 | Privacy masks for regions and windows, named in the result | drawn before encoding |
| D12 | Restore on timeout and cancel for every scope, mask and route | extends `timeout-drops-restore` to all new paths |
| D5 | Live-desktop occluded `PrintWindow` capture, `occluded_unavailable` detection | gives the constant a real call site |
| D8 | UWP/WinUI target lock, `appId` provenance | |
| — | Reduction: scroll-until-found for virtualized lists | beyond the 200/depth-32 cap |
| — | Reduction: `WM_SETTEXT` for edit controls | `ValuePattern` stays refused |
| D9 | The 14-case capture matrix (Appendix C), 1-px anchor fixture | biggest; DPI already enabled; hardware cases Unverified |

### Lane F — fixtures and the count (parallel, JS + test data)

- Grow browser-focused tests to **104** by giving every tool a live positive test and a typed-refusal test where the refusal is real behaviour. No padding: a test asserts an observable.
- Keep `run-tasks.mjs` the scoreboard: as each row lands, its pending task flips to pass, verified by a run.

## 4. Dependency graph

- B6 depends on B3. B13 depends on every browser row. D9 depends on the anchor fixture and D12. D5 depends on nothing.
- B1 first inside Lane B; everything after it is proven on Chrome 158.
- The 30-run repetition bar runs once, at the seal — never in the edit loop.

## 5. Acceptance (the pristine bar)

1. `cargo test` green, three consecutive runs; `-p browser-engine`, `-p desktop-engine -- --skip live_` green.
2. **22+ verified rows**, each with red-line, green test, live receipt. (24 is achievable; 22 is the floor.)
3. **104 browser-focused tests**, green.
4. `run-tasks.mjs`: **30 pass, 0 pending, 0 fail, 3 consecutive runs.**
5. Appendix C: every case that can run on this machine passes with a 1-px anchor; the hardware-gated three are `Unverified-named`.
6. Catalog floor intact (32 names + declared additions); `compare-catalog.mjs` green.
7. Release identity: `record-build.mjs` at final HEAD; benchmark 0 of 100 with an attested receipt.
8. Docs match the binary exactly: every tool description a test can back.
9. No push, no publish, no install. Owner-gated, unchanged.

## 6. Seal (once, at the end)

`cargo test` ×3 → ten-run repetition on the two changed targets → `record-build.mjs` → benchmark from the release binary → `compare.mjs` → `run-tasks.mjs` ×3 → docs to final numbers → stop receipt with both counters and the Unverified list.

## 7. Status board (mutable — updated as lanes land)

| Row | State | Receipt |
|---|---|---|
| B12, B9, B11, B5, B2, D10, D14, D13 | **done** | in history |
| B10 | **partial — platform limit** | `34c0a93` |
| B1, B3, B4, B6, B7, B8, B13 | open | — |
| D1(hidden-desktop breadth), D3, D4, D5, D8, D9, D11, D12 | open | — |
| reductions ×5 | open | — |

D1 note: hidden-desktop is proven on Win32; WPF/WinUI3 breadth is folded into D8's work, marked Unverified if the apps cannot be produced here.

## 8. Risks

- B1 changes the browser: every suite number moves once. Prove on the new pin before filling other rows.
- D9's hardware cases cannot be faked; they are named, not passed.
- B6's HAR surface is the one L row in scope; it is scheduled after B3 so the capture already exists.
- The 104-test target is a count, not padding; a test that asserts nothing is a defect.

## 9. Out of scope, by prior agreement

Multi-window scope, WGC capture, OCR, app-class matrix — each a subsystem, each named with its reason in the addendum. Release and install: owner-gated, not part of this plan.
