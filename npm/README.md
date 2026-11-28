<h1 align="center">hand-mcp</h1>
<p align="center"><strong>The MCP server that sees and touches both the browser and Windows desktop.</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/hand-mcp"><img src="https://img.shields.io/npm/v/hand-mcp" alt="npm version" /></a>
  <img src="https://img.shields.io/badge/platform-windows%20x64-blue" alt="Windows x64" />
  <img src="https://img.shields.io/badge/tools-44-brightgreen" alt="44 tools" />
  <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT licence" />
</p>

## Install

```powershell
npx hand-mcp
```

The binary ships inside the package: about 6.6 MB packed and 19.9 MB unpacked. No install script downloads anything and no compiler is needed. Pin an exact version (`npx hand-mcp@<version>`) when you need reproducibility.

A global install is not supported on purpose: the server belongs in the client's own process tree, not on `PATH`. The package refuses a global install in `preinstall`; on npm 12 and newer, dependency install scripts are blocked by default, so `npm install -g` can still place the files — use `npx hand-mcp` either way.

### Or let your AI install it

```text
Install nathwn12/hand (hand-mcp) with `npx hand-mcp`, then prove it works.
1. Run: `npx hand-mcp --version`
2. Done when: exit 0 and a line `hand <version> (<commit>; release)`.
3. Wire it: add the server below to the client config and restart.
If it fails, stop and paste the full output; do not improvise a fix.
```

### Wire it yourself

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

The client spawns npx, which runs the bundled binary and speaks newline-delimited JSON-RPC 2.0 on stdio. stdout carries protocol frames only.

## Tools

One connection, two domains, 44 tools:

- **Browser, 36**: `browser_navigate`, `browser_snapshot` (stable `@eN` refs), click with right/middle/double, type, hover, select, key, upload, wheel, drag, ordered form batches, downloads, evaluate, console, network interception (route, mock, abort, offline), fetch text, wait, layout and layout assertions, DOM/text/URL/ax assertions, viewport audits (1..12 caller-supplied widths, WCAG contrast evidence), visual regression, screenshots (whole page, one element, or a clip; png, jpeg, webp), storage, tabs, viewport, dialog, history, targets, named contexts, opt-in traces (HAR, counted redaction, optional MJPEG AVI video from a CDP screencast), `browser_metrics` (measured page timings and Web Vitals; unmeasured entries are null with a reason), raw CDP, `browser_run` (ordered batches with per-step evidence), page clipboard read/write with `browser_clipboard` (through the page's `navigator.clipboard`, which in Chromium is the Windows clipboard; a write replaces the owner's clipboard text and requires `allow_clipboard:true`; hand calls no Windows clipboard API itself), close. Screenshots add `freeze`, `waitForStable`, `force` and `scale`; element tools accept `selector`; evaluate marks undefined results with `void: true`.
- **Desktop, 8**: `desktop_screenshot`, `desktop_diff`, `desktop_list_windows`, `desktop_window`, `desktop_do`, `desktop_cursor`, `desktop_elements`, `desktop_record` (targeted-window MJPEG AVI recording, no activation, pointer movement or clipboard access), over Win32 and UI Automation.

A desktop action posts to the chosen window: no global mouse, no physical pointer movement, no window activation, no clipboard write — unless a call explicitly opts in with `desktop_do` `allow_cursor` (real pointer after a point guard, restored), `allow_clipboard` (a real `CF_UNICODETEXT` write, with optional restore), or `allow_foreground` (real `SetForegroundWindow` for `focus_window`), or `desktop_window` `allow_foreground` for `focus`/`maximize`. A refusal names its reason: a cancelled or expired call answers `outcome: unknown` with `retry: unsafe` instead of implying the work stopped. A healthy run writes nothing to the terminal.

Set `GLADIUS_CDP_URL` to attach to a browser you already run instead of launching one; hand does not launch, kill or sweep it, and closes only pages it created. `HAND_DEBUG` prints engine diagnostics to stderr (off by default). The first browser call provisions the pinned `chrome-headless-shell` into a per-user cache and reuses it after that.

## Requirements

- Windows 10 or 11, x64. The manifest declares `os: win32` and `cpu: x64`.
- Node 22.14.0 or newer, to run the launcher.
- An MCP client that speaks stdio.
- Screenshot `size` is measured from live captures for PNG and JPEG; for WebP it is read from the encoded file and has not been checked against a live capture.
- Screenshot byte size depends on the page, so no fixed size target is promised.
- Desktop actions never move the pointer, steal focus or write the clipboard unless a call opts in; other pointer or foreground activity on the machine can still affect an observation, and hand reports its own actions rather than trusting a raw flag.

## Status

**Experimental** — pre-release; interfaces may change.

## Links

- [Issues](https://github.com/nathwn12/hand/issues)
- [Releases](https://github.com/nathwn12/hand/releases)
- [npm package](https://www.npmjs.com/package/hand-mcp)
- [MIT licence](LICENSE)
