<h1 align="center">hand-mcp</h1>
<p align="center"><strong>The MCP server that sees and touches both the browser and Windows desktop.</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/hand-mcp"><img src="https://img.shields.io/npm/v/hand-mcp" alt="npm version" /></a>
  <img src="https://img.shields.io/badge/platform-windows%20x64-blue" alt="Windows x64" />
  <img src="https://img.shields.io/badge/tools-41-brightgreen" alt="41 tools" />
  <img src="https://img.shields.io/badge/license-MIT-green" alt="MIT licence" />
</p>

## Install

```powershell
npx hand-mcp
```

The binary ships inside the package: about 6.4 MB packed, 18.9 MB unpacked (18.0 MiB). No install script downloads anything and no compiler is needed. Pin an exact version (`npx hand-mcp@<version>`) when you need reproducibility.

A global install is not supported on purpose: the server belongs in the client's own process tree, not on `PATH`. `npx hand-mcp` needs none.

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

One connection, two domains, 41 tools:

- **Browser, 34**: `browser_navigate`, `browser_snapshot` (stable `@eN` refs), click with right/middle/double, type, hover, select, key, upload, wheel, drag, ordered form batches, downloads, evaluate, console, network interception (route, mock, abort, offline), fetch text, wait, layout and layout assertions, DOM/text/URL/ax assertions, viewport audits (six sizes, WCAG contrast evidence), visual regression, screenshots (whole page, one element, or a clip; png, jpeg, webp), storage, tabs, viewport, dialog, history, targets, named contexts, opt-in traces (HAR, counted redaction), raw CDP, `browser_run` (ordered batches with per-step evidence), close. Screenshots add `freeze`, `waitForStable`, `force` and `scale`; element tools accept `selector`; evaluate marks undefined results with `void: true`.
- **Desktop, 7**: `desktop_screenshot`, `desktop_diff`, `desktop_list_windows`, `desktop_window`, `desktop_do`, `desktop_cursor`, `desktop_elements`, over Win32 and UI Automation.

A desktop action posts to the chosen window: no global mouse, no physical pointer movement, no window activation, no clipboard write. A refusal names its reason: a cancelled or expired call answers `outcome: unknown` with `retry: unsafe` instead of implying the work stopped. A healthy run writes nothing to the terminal.

Set `GLADIUS_CDP_URL` to attach to a browser you already run instead of launching one; hand does not launch, kill or sweep it, and closes only pages it created. `HAND_DEBUG` prints engine diagnostics to stderr (off by default). The first browser call provisions the pinned `chrome-headless-shell` into a per-user cache and reuses it after that.

## Requirements

- Windows 10 or 11, x64. The manifest declares `os: win32` and `cpu: x64`.
- Node 22.14.0 or newer, to run the launcher.
- An MCP client that speaks stdio.
- WebP screenshot `size` parsing ships but is not live-measured; JPEG and PNG are.
- The 500 KB target for a 2560-wide JPEG q80 shot is page-dependent: on the seeded-noise fixture, JPEG q80 is 40,392 B against PNG 89,983 B.
- The desktop test suite serializes its live tests inside one binary; an external pointer or foreground actor can still flake a no-steal assertion.
- The browser-only visual-QA lane that drove this release did not exercise the desktop tools; they remain Unverified by that lane, not faulty.

## Status

**Experimental** — pre-release; interfaces may change.

## Links

- [Issues](https://github.com/nathwn12/hand/issues)
- [Releases](https://github.com/nathwn12/hand/releases)
- [npm package](https://www.npmjs.com/package/hand-mcp)
- [MIT licence](LICENSE)
