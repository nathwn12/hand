<p align="center">
  <img src="assets/icon.svg" width="120" height="120" alt="hand" />
</p>

<h1 align="center">hand</h1>

<p align="center"><strong>The MCP server that can see and touch both the browser and Windows.</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/hand-mcp"><img src="https://img.shields.io/npm/v/hand-mcp" alt="npm version"></a>
  <img src="https://img.shields.io/badge/platform-windows%20x64-blue" alt="platform">
  <img src="https://img.shields.io/badge/tools-41-brightgreen" alt="41 tools">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT">
</p>

One stdio connection. One Rust binary. Two domains.

- **Browser**: headless Chrome over CDP. Navigate, snapshot, click, type, screenshot, inspect, test.
- **Desktop**: Windows apps through UI Automation and Win32. Name a window, click its buttons, type into it, screenshot it.

An agent can test your web app in the morning and drive its native installer in the afternoon. Same server, same session, no second toolchain.

## Why hand

Most agent tooling makes you choose: a browser MCP *or* a desktop MCP. hand is both, and every action is evidence-based.

- **No pointer steal.** A desktop action posts to the chosen window. No global mouse, no physical pointer movement, no window activation, no clipboard write. The machine stays usable while the agent works.
- **Evidence, not vibes.** Every capture reports the backend, the window it matched, whether the frame was blank or stale, and whether the foreground window and cursor stayed put.
- **Refusals that say why.** Typed error codes beside the prose. Cancellation and budget answer `outcome: unknown, retry: unsafe` instead of implying the work stopped.
- **Code-mode ready.** Every tool carries an output schema, so a code-mode host builds a typed API. A healthy run writes nothing to the terminal; diagnostics open only with `HAND_DEBUG=1`.
- **One binary.** No browser download step at runtime, no Python sidecar. `npx hand-mcp` and go.

## Install

```powershell
npx -y hand-mcp --version
```

```jsonc
{
  "mcp": {
    "servers": {
      "hand": {
        "type": "local",
        "command": ["cmd", "/c", "npx", "-y", "hand-mcp"]
      }
    }
  }
}
```

The client spawns npx, which runs the bundled binary and speaks newline-delimited JSON-RPC 2.0 on stdio. stdout carries protocol frames only; diagnostics go to stderr. A global install is not supported on purpose; the server belongs in the client's process tree.

From source: `cargo build --release`, then `hand doctor` proves the build, the pinned engines and the browser path.

## Browser, 34 tools

`browser_snapshot` returns stable `@eN` references. Every action returns `resolved`, so a stale ref fails typed instead of clicking the wrong thing.

- Navigate and inspect: `browser_navigate`, `browser_snapshot`, `browser_targets`, `browser_tabs`, `browser_viewport`, `browser_dialog`, `browser_history`
- Act: `browser_click` (left, right, middle, double), `browser_type`, `browser_hover`, `browser_select`, `browser_key`, `browser_upload`, `browser_wheel`, `browser_drag`, `browser_form` (ordered form batches)
- Prove: `browser_assert_layout`, `browser_layout`, `browser_visual`, `browser_assert` (dom, text, url, ax), `browser_audit` (six viewports, WCAG contrast evidence), `browser_screenshot` (whole page, one element, or a clip; png, jpeg, webp)
- Read: `browser_evaluate`, `browser_console`, `browser_network`, `browser_fetch_text`, `browser_storage` (cookies and web storage)
- Download: `browser_download` (caller directory, SHA-256, MIME, cancel cleanup)
- Record: `browser_trace` (opt-in HAR over the live network log plus step screenshots, literal counted redaction, into a caller-supplied absolute directory)
- Batch: `browser_run` (ordered tool batch)
- Isolate: `browser_context` (named, separate cookie and storage jars)
- Reach the protocol: `browser_cdp`
- Wait and close: `browser_wait_for`, `browser_close`

Headless extension testing works out of the box: load an unpacked extension into the owned Chromium, drive its popup, read the console.

## Desktop, 7 tools

- `desktop_list_windows`: every top-level window with hwnd, pid, rect, dpi, foreground and elevated flags
- `desktop_window`: move, resize, minimize, raise, close; never steals focus
- `desktop_elements`: UI Automation tree with stable fingerprints
- `desktop_do`: invoke, set value, click, type on an element
- `desktop_screenshot`, `desktop_diff`: capture and compare, with blank and stale detection
- `desktop_cursor`: where the pointer actually is

## One session, both domains

```js
// code mode: a typed call per schema
await browser_navigate({ url: "http://localhost:3000" });
const snap = await browser_snapshot();
await browser_click({ ref: "@e12", button: "left" });
await browser_screenshot({ format: "png", fullPage: true });

const windows = await desktop_list_windows();
await desktop_do({ hwnd: windows[0].hwnd, action: "invoke", name: "Install" });
```

## Compatibility

hand serves 41 tools: 34 browser tools (25 mapped one-to-one from Gladius plus `browser_context`, `browser_wheel`, `browser_drag`, `browser_form`, `browser_download`, `browser_assert`, `browser_audit`, `browser_trace`, `browser_run`) and Puppet's 7 desktop tools. Names are prefixed, bodies are the pins'. A client migrating from either engine has a one-to-one mapping. Intentional differences, fourteen description rewrites included, are declared in `baseline/declared-additions.json`; `tests/discovery.rs` asserts them against the committed floor and `baseline/scripts/compare-catalog.mjs` recaptures it independently. The 41 is the live hand catalog a user gets; the 32 in `baseline/hand-catalog.json` (`tools: 32`) is the pinned engine floor (25 Gladius browser + 7 Puppet desktop) that the comparison check diffs against.

The current release adds `selector` as an alias for `element`, a `void` flag on `browser_evaluate`, a generated parameter index in `initialize.instructions`, a 1440x900 owned-launch viewport echoed by navigate/snapshot/screenshot, screenshot `freeze`, `waitForStable`, `force` and `scale` with motion evidence and jpeg/webp `size`, and the `browser_run` ordered batch. `format`, `quality` and `fullPage` were already present in the previous release; discovery was the gap. Known limits are named in `docs/CAPABILITIES.md`.

Engine versions and source pins live under `sources` in `baseline/pins.json` and in the table in `docs/COMPATIBILITY.md`; both engines are MIT, credited in `THIRD-PARTY-NOTICES.md`.

## Compared

| | hand | Playwright MCP | Chrome DevTools MCP |
| --- | --- | --- | --- |
| Browser automation | yes | yes | yes |
| Windows desktop control | yes | no | no |
| One connection for both | yes | - | - |
| No pointer steal | yes | - | - |
| Single Rust binary | yes | no | no |

## Configure it

No flag serves MCP: `hand` with no argument is `hand mcp`. The other subcommands are `doctor`, `--version` and `--help`.

| Variable | Default | Effect |
|---|---|---|
| `GLADIUS_CDP_URL` | unset | Attach to an engine you already run instead of launching one. hand does not launch, kill or sweep it, and closes only pages it created. |
| `HAND_DEBUG` | unset | Print engine diagnostics to stderr. Off by default: a healthy run is silent. |

With no override, `browser_*` launches the pinned `chrome-headless-shell` from the per-user cache under `gladius/chromium/<pin>` on first use, in a private temporary profile, attached to a kill-on-close job.

## Requirements

- Windows 10/11 x64. The desktop domain is Win32 and UI Automation; there is no other platform.
- An MCP client that speaks stdio.
- The first browser call provisions the pinned `chrome-headless-shell`; later runs reuse it. Rust is needed to build, not to run.

## Development

```powershell
cargo test                                  # hand end-to-end suite
cargo test -p browser-engine                # vendored Gladius unit tests
cargo test -p desktop-engine -- --skip live_ # vendored Puppet unit tests
node baseline/scripts/compare-catalog.mjs --serve target\debug\hand.exe --baseline hand
node npm/release.mjs --check                # packaged artifact, from its own bytes
```

The live browser test starts a loopback fixture and drives inspect, act and verify; the desktop test enumerates real windows and inspects one read-only while the foreground window and pointer stay put. Vendored engine sources are MIT, pinned and listed in `THIRD-PARTY-NOTICES.md`.

## Status

**Experimental**: pre-release; interfaces may change.

Three commits in this history carried the local harness seed: `a0bdbe5`, `3295432` and `f1013fa`. They remain reachable objects. Leaving them was deliberate: they contain no secret, and a copy of the seed is kept in the harness archive. The seed files stopped being tracked at `f08da64`.

## Links

- [Issues](https://github.com/nathwn12/hand/issues)
- [Releases](https://github.com/nathwn12/hand/releases)
- [Capabilities](docs/CAPABILITIES.md)
- [Compatibility](docs/COMPATIBILITY.md)
- [Third-party notices](THIRD-PARTY-NOTICES.md)
- [MIT](LICENSE)

<p align="center"><sub>Built by nathwn12</sub></p>
