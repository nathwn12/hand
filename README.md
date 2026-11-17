# hand

One local MCP server that gives an agent hands and eyes on this machine: a
browser it can drive, and a Windows desktop it can see and touch without taking
the machine over.

It is one product rather than two glued together — one executable, one MCP
connection, one prefix-scoped catalog, one budget and cancellation contract,
one shutdown. The two domains share a process and a lifetime, and each works
alone as well as together.

What that buys an agent:

- **Control with evidence.** Browser actions run over CDP and can be verified
  against the DOM or accessibility tree. Desktop actions are posted to a chosen
  window — no global mouse, no physical pointer movement, no window activation,
  no clipboard write.
- **Sight that is honest about itself.** A capture states the backend it used,
  the window identity it matched, whether the frame was blank or stale, and
  whether the foreground window and cursor stayed put. An observation that
  cannot be trusted fails instead of being reported as a picture.
- **Isolation where it matters.** Named browser contexts keep cookies and
  storage apart; hidden desktop launches run in a job object that dies with the
  server.

- Product: **hand**
- Executable: **hand**. Future npm package name: **hand-mcp**
- OpenCode connection key: **oc-hand**
- Distribution target: Windows x64
- Repository: private, local, no remote

## Relationship to Gladius and Puppet

**hand is the successor to Gladius and Puppet, not a wrapper around them.** Both
engines are vendored at pinned commits and their 32 tools are republished
unchanged; Gladius and Puppet are disconnected from this machine and remain on
disk only as references. Everything they did, hand does on one connection — and
it adds what neither had:

- one process and one lifetime for both domains, so a browser call and a desktop
  observation share budgets, cancellation and a single shutdown;
- cancellation and budget expiry that report the truth (`outcome: unknown`,
  `retry: unsafe`) instead of implying the work stopped;
- shutdown that is bounded even when a blocking desktop call cannot be aborted;
- hidden desktop launches covered by a kill-on-close job object from the moment
  the process is created;
- isolated, named browser contexts with verified teardown;
- a captured frame that has to prove its own identity, freshness and
  non-blankness before it is returned.

Measured on this machine, release builds, 10 warmups and 100 attempts (see
`benchmarks/`):

| Scenario | hand p95 | Gladius p95 | Puppet p95 |
|---|---:|---:|---:|
| warm ping | 0.11 ms | 0.12 ms | 0.09 ms |
| browser snapshot | 21.7 ms | 29.1 ms | — |
| window enumeration | 2.36 ms | — | 1.99 ms |
| window capture | unmeasured | — | unmeasured |

So: strictly more capable, and faster on the browser path. On the two warm
round trips Puppet is still marginally quicker (0.09 ms vs 0.11 ms ping,
1.99 ms vs 2.36 ms enumeration), and the capture row has no fair measurement
yet. That is the whole of the claim — no other number is being beaten.

Status: the 32-tool compatibility floor is served unchanged and hand adds
`browser_context` on top of it. Targeted end-to-end checks pass, including
cancellation, shutdown, hidden-desktop cleanup and isolated browser contexts.
The specification's remaining added capabilities and one performance row are not
complete. See `docs/CAPABILITIES.md` for exactly what is verified and what is
missing.

## Shape

```
hand.exe            one process, one stdio MCP endpoint
├─ browser_*            25 Gladius tools, headless Chromium over CDP
│  └─ browser_context   hand's isolated contexts (create/select/eval/cookies/…)
├─ desktop_*            7 Puppet tools, Win32 + UI Automation
├─ crates/browser-engine   Gladius 0.8.0 source (pin 59c6d77) as a library
└─ crates/desktop-engine   Puppet 8.0.0 source (pin 0f790be) as a library
```

No subprocess facade: both engines are compiled into the executable. Each keeps
its own module tree, locks, deadlines, typed errors and result bodies.

## Run

```powershell
cargo build --release
.\target\release\hand.exe doctor     # no MCP, no browser launch, no state change
.\target\release\hand.exe            # MCP over stdio (default: mcp)
```

A client spawns `hand` and speaks newline-delimited JSON-RPC 2.0 on stdio.
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
the legacy catalog: 32/32 legacy tools match with zero metadata drift, and
`browser_context` is reported as hand's single addition.

## Tests

```powershell
cargo test                                  # hand end-to-end suite
cargo test -p browser-engine                # vendored Gladius unit tests (69)
cargo test -p desktop-engine -- --skip live_ # vendored Puppet unit tests (228)
$env:HAND_LIVE_DESKTOP='1'; cargo test --test no_steal live_read_only_target_capture_keeps_foreground
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand.exe --baseline hand
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
