# hand-mcp browser fixtures

Deterministic, loopback-only browser fixtures for the hand-mcp performance and
acceptance work. This directory owns fixture bytes only. It contains no product
source and does not change the baseline.

## Files

| File | Purpose |
| --- | --- |
| `index.html` | Benchmark page: exactly 100 visible labelled controls at 1280x720. |
| `actions.html` | Inspect, act, verify, reset and deliberately false assertions. |
| `manifest.json` | Machine-readable fixture contract and expected states. |
| `serve.mjs` | Node static server bound to `127.0.0.1`; allowlists the payload routes. |
| `validate.mjs` | Structure checks, SHA256 verification and headless browser self-test. |
| `SHA256SUMS` | SHA256 of the served payload files. |
| `.gitattributes` | Disables line-ending conversion so the hashed bytes survive checkout. |
| `tasks/` | Pages for the 30-task browser sequence: main, frame, next, viewport, network, download, upload. |
| `tasks.json` | The 30 tasks as data: ordered calls, expected evidence, cleanup. |
| `run-tasks.mjs` | One-command runner over the copied hand binary and the loopback fixture. |

The server serves `/`, `/index.html`, `/actions.html` and `/manifest.json`.
Every other path returns HTTP 404 with the body `404 not found\n`. This includes
`/missing.html`, which the actions page links to on purpose. Methods other than
GET and HEAD receive HTTP 405.

## Benchmark page

`/index.html` is a fixed 10-by-10 grid of native `<button>` elements:

- 100 controls, ids `control-001` through `control-100`, in DOM order.
- Labels `Control 001` through `Control 100`, each present as visible text and
  as `aria-label` equal to `data-label`.
- Every control is 112 by 48 CSS pixels and lies fully inside the 1280x720
  viewport at DPR 1. The page has no document overflow.
- The only interactive elements are the 100 controls; the page has no anchors,
  images, iframes, external assets or network calls.
- The only `<link>` element is an inline empty favicon (`href="data:,"`), so the
  browser makes no `/favicon.ico` request.
- `#status` starts at `last activated: none (0)`. A real pointer click on any
  control sets it to `last activated: Control NNN (count)`.
- The page has no timers, animation, randomness or clock reads.

Use a 1280x720 viewport at device scale factor 1. Benchmark results and the
pinned browser digest belong in the benchmark receipt, not here.

## Actions page

`/actions.html` starts in the state recorded in `manifest.actions.initial` and
returns to it through the `Reset fixture` control.

| Control | Observable effect |
| --- | --- |
| `Apply change` | `#result` and `body[data-state]` become `applied`. |
| `No-op control` | Only `#noop-count` increments; `#result` stays `idle`. |
| `Increment counter` | `#counter-value` increments by one per click. |
| `Apply name` | `#greeting` becomes `Hello, <name>.` or `Hello, nobody.`. |
| `Mode` select | `#mode-output` follows the selected value. |
| `Enable flag` | `#flag-state` becomes `on` or `off`. |
| `Open dialog` / `Close dialog` | Native dialog opens and closes; `#dialog-state` follows. |
| Nested scroll region | Setting actual scroll offsets updates `#scroll-offset` as `x,y`. |
| Missing page link | Navigates to a real loopback HTTP 404. |

Negative assertion contract: after one `No-op control` click, `#noop-count` is
`1` and `#result` is still `idle`. Asserting `#result == "applied"` at that point
must fail, with `observed: "idle"` recorded as evidence. The validator exercises
this exact case.

Selectors use stable ids, visible labels and `data-testid` attributes. Nothing
is randomized between loads.

## Browser 30 tasks

`node fixtures/run-tasks.mjs` copies the hand binary to a temp directory,
starts the loopback fixture and the existing two-origin interaction fixture,
then runs all 30 cells from `tasks.json` through one MCP session. It prints one
line per task and a `pass/pending/fail` summary. `--only <id>`, `--json`,
`--repeat <n>` and `--bin <path>` are supported. A `pending` cell names the
tool or capability it waits for; it is never reported as a pass. See
`docs/BROWSER-TASKS.md`.

The task pages are served from `/tasks/*`. They are outside the frozen
benchmark payload: `SHA256SUMS` and the 100-control contract in `index.html`
are unchanged.

## Run locally

Serve the fixture:

```powershell
node fixtures/serve.mjs --port 4173
# open http://127.0.0.1:4173/
```

Use `--port 0` for an ephemeral port; the actual origin is printed.

Validate structure, SHA256 and the headless browser self-test:

```powershell
node fixtures/validate.mjs
```

Validation requires Node 22 or newer, for the global `WebSocket` used by the
CDP client. Serving alone needs only Node with top-level `await`.

Static checks without a browser:

```powershell
node fixtures/validate.mjs --static-only
```

After an intentional payload change, rewrite the recorded hashes and then
re-validate:

```powershell
node fixtures/validate.mjs --write-sums
node fixtures/validate.mjs
```

Select a specific Chromium-family executable with `--browser <path>` or the
`HAND_FIXTURE_BROWSER` environment variable. The validator probes Chrome,
Brave and Edge in that order.

The browser checks launch a private `--headless=new` browser with a temporary
profile and `windowsHide`, and close it afterwards. They do not use the owner's
browser, the browse bridge, or any visible window.

## Fixture SHA256

These hashes cover the served payload only. They do not cover `README.md`,
`serve.mjs` or `validate.mjs`.

```text
2cdb034a3dab47018cf7915246fcf388d25d3b117402531c3573171dd3492d14  index.html
a907df9e7d6d77269f3c74c66ade1b02843aaeb3934b406e4696be3faf90808f  actions.html
6c7f9db917043d235a798d9604d5a633db9274d56e1ddfae13dc2b8cb861a189  manifest.json
```

Verify on Windows:

```powershell
Get-FileHash fixtures/index.html, fixtures/actions.html, fixtures/manifest.json -Algorithm SHA256
```

## Determinism and limits

Frozen: HTML, CSS, inline JavaScript, control count, labels, ids, CSS geometry,
initial state, action effects, 404 body, and payload hashes.

Not frozen: font rasterization and native control rendering can differ by
browser version, so screenshot pixels are not hashed. The validator prints the
browser product and revision it used but does not pin a browser digest; the
benchmark receipt must pin that separately.

The accessible-name check reads `aria-label`, associated labels and text from
the DOM. It is not a full WCAG audit or a screen-reader verdict. Geometry checks
show visibility and containment, not visual quality.

The validator records every HTTP request received by the fixture server and
fails on any path outside the allowlist, so an unexpected favicon or asset
request is visible rather than silently ignored.

Not included in this fixture slice: popup, cross-origin or inaccessible frame,
download, upload, console-error and visual-baseline pages. They can be added
later without changing the benchmark page's 100-control contract.

The validator's browser check is a fixture self-test, not a product benchmark.
It does not measure latency, does not compare Gladius, Puppet or hand-mcp, and
does not claim any performance target was met.
