# hand

One local Rust MCP server for agents to test browser interfaces and inspect and
operate Windows applications. `hand` combines the Gladius browser engine and
the Puppet desktop engine in one executable, one MCP connection, one version.

- Product: **hand**
- Executable and future npm package: **hand-mcp**
- OpenCode connection key: **oc-hand** (not wired yet)
- Distribution target: Windows x64
- Repository: private, local, no remote

Status: initial integration slice. The 32-tool compatibility catalog is served
and targeted end-to-end checks pass; the specification's added capabilities and
performance gate are not implemented. See `docs/CAPABILITIES.md` for exactly
what is verified and what is missing.

## Shape

```
hand-mcp.exe            one process, one stdio MCP endpoint
├─ browser_*            25 Gladius tools, headless Chromium over CDP
├─ desktop_*            7 Puppet tools, Win32 + UI Automation
├─ crates/browser-engine   Gladius 0.8.0 source (pin 59c6d77) as a library
└─ crates/desktop-engine   Puppet 8.0.0 source (pin 0f790be) as a library
```

No subprocess facade: both engines are compiled into the executable. Each keeps
its own module tree, locks, deadlines, typed errors and result bodies.

## Run

```powershell
cargo build --release
.\target\release\hand-mcp.exe doctor     # no MCP, no browser launch, no state change
.\target\release\hand-mcp.exe            # MCP over stdio (default: mcp)
```

A client spawns `hand-mcp` and speaks newline-delimited JSON-RPC 2.0 on stdio.
stdout carries protocol frames only; diagnostics go to stderr.

Browser ownership:

- With no environment override, `browser_*` launches the pinned
  `chrome-headless-shell` from `%LOCALAPPDATA%\gladius\chromium\<pin>` on first
  use, in a private temporary profile, attached to a kill-on-close job.
- `GLADIUS_CDP_URL` attaches to an explicitly selected external engine instead.
  hand does not launch, kill or sweep that engine, and closes only pages it
  created.

Desktop safety:

- Reads or acts only on an explicitly selected window.
- Never falls back to the foreground window; never moves the physical pointer,
  activates a window, or writes the clipboard.
- Unsupported or stale targets return typed failures, never a silent simulation.

## Catalog and prefixes

Every legacy tool is published unchanged under a domain prefix:

| Legacy | hand |
|---|---|
| Gladius `navigate` … `fetch_text` (25) | `browser_navigate` … `browser_fetch_text` |
| Puppet `screenshot` … `elements` (7) | `desktop_screenshot` … `desktop_elements` |

Only the `name` changes. `inputSchema`, `outputSchema`, `description` and
`annotations` are the pinned engine's. `tests/discovery.rs` asserts that field
for field, and `baseline/scripts/compare-catalog.mjs` independently recaptures
the legacy catalog and compares it with 32/32 tools.

## Tests

```powershell
cargo test                                  # hand end-to-end suite (24 tests)
cargo test -p browser-engine                # vendored Gladius unit tests (56)
cargo test -p desktop-engine -- --skip live_ # vendored Puppet unit tests (203)
$env:HAND_LIVE_DESKTOP='1'; cargo test --test no_steal live_read_only_target_capture_keeps_foreground
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand-mcp.exe --baseline hand
```

The live browser test starts a loopback fixture, drives inspect → act → verify,
captures a deterministic screenshot, and proves a pre-navigation ref is refused.
The desktop test enumerates real windows and inspects one read-only while the
foreground window and pointer stay put. The adapted no-steal suite keeps Puppet's
source, wire and live foreground checks and extends the static scan to the
browser engine tree.

## Source pins and licensing

Vendored engine sources are MIT, from the owner's repositories:

- Gladius `59c6d77` (0.8.0), `crates/browser-engine/src`
- Puppet `0f790be` (8.0.0), `crates/desktop-engine/src`

See `THIRD-PARTY-NOTICES.md`. The only edits to vendored code are visibility and
two integration seams, each marked `hand` in place: the public entry points in
`desktop-engine/src/serve.rs`/`lib.rs`, `cancel::ensure`, the browser engine's
`typed_envelope`/`chromium_path`, and the tree-shape floor in Puppet's fixture
self-test (its integration tests live in this package instead).
