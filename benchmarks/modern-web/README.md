# Modern-web conformance harness

Turns modern-web-guidance use cases into repeatable browser cases for the
`hand` MCP server. No npm dependencies; Node 22+.

## Run

```powershell
node benchmarks/modern-web/run.mjs --limit 10
node benchmarks/modern-web/run.mjs
node benchmarks/modern-web/run.mjs --negative
node benchmarks/modern-web/run.mjs --filter dialog
```

Flags: `--exe <path>`, `--filter <substr>`, `--limit N`, `--negative`.
The executable defaults to `target/release/hand.exe` with a
`target/debug/hand.exe` fallback. One MCP process serves the whole run; each
case navigates to a fresh fixture URL. Output is one line per case
(`PASS|FAIL|SKIP <id> <detail>`), then `RESULT: n/N`, `SKIPPED`, `DURATION`
and a `report-<utc>.json` with every step and verdict. Exit 0 only when every
non-skipped case passed; in `--negative` mode only when all broken fixtures
failed for their intended reason.

## Layout

- `guides.json` — all 211 guides, classification (testable/context), one-line
  reason, and status (covered/pending). The CLI `list` returns 210; the gated
  `prompt-api` guide ships in the package and is added as context.
- `cases/<category>.json` — cases: `expectSnapshot` (role/name pairs),
  `act` steps and `expect` verifications. `@ref:<accessible name>` resolves
  against the case's own snapshot; no hard-coded `@eN`.
- `fixtures/<id>.html` — small static dependency-free pages.
- `negative/` — three deliberately broken fixtures and their expected failure
  stage/detail.

A case is a simplified stand-in for its guidance: it proves the browser can
open the page, find a control by role/name, act and verify state. It does not
assert the full production-quality implementation of the guidance.
