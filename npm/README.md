<h1 align="center">hand-mcp</h1>
<p align="center"><strong>hand is one MCP server for headless browser testing and Windows desktop computer use.</strong></p>

## Install

Run it without installing:

```powershell
npx hand-mcp
```

Or install it into a project:

```powershell
npm i hand-mcp
```

The binary ships inside the package: about 5 MB packed, 14 MB unpacked. No install script downloads anything and no compiler is needed. Pin an exact version with `npx hand-mcp@0.1.0`.

A global install is not supported: the server belongs in the client's own process tree, not on `PATH`. The package's `preinstall` guard exits 1 with `hand-mcp: global install refused — use npx hand-mcp@0.1.0`. npm 12 blocks install scripts by default, so that guard only runs where scripts are allowed; `npx hand-mcp` needs no global install in any case.

### Or let your AI install it

```text
Install nathwn12/hand (hand-mcp) with `npx hand-mcp`, then prove it works.
1. Run: `npx hand-mcp`
2. First run: starts an MCP server on stdio and waits for a client on stdin; nothing else must exist.
3. Done when: `npx hand-mcp --version` exits 0 and prints `hand 0.1.0 (<commit>; release)`.
If it fails, stop and paste the full output; do not improvise a fix.
```

### Wire it yourself

Point your MCP client at the launcher. The client spawns it and speaks newline-delimited JSON-RPC 2.0 on stdio:

```jsonc
{
  "mcp": {
    "servers": {
      "hand-mcp": {
        "type": "local",
        "command": ["npx", "hand-mcp"]
      }
    }
  }
}
```

## Use it

`hand-mcp` with no argument starts the MCP session. The launcher forwards argv, stdio and the exit code to the bundled `hand.exe`, so `hand-mcp --version` prints the version, the commit and the build profile (`hand 0.1.0 (<commit>; release)`) and exits without starting a session.

The binary's own subcommands are `mcp` (the default), `doctor` and `--version`.

One connection carries 33 tools:

- Browser, 25: `browser_navigate`, `browser_snapshot`, then a tool that acts on an `@eN` ref, plus evaluate, console, network, fetch text, layout assertions, screenshots, storage and raw CDP (`browser_cdp`).
- Desktop, 7: `desktop_screenshot` through `desktop_elements`, over Win32 and UI Automation, reading or acting only on an explicitly selected window.
- `browser_context`: named contexts, so cookies and storage stay apart.

The 32 tools the two engines already served keep their `inputSchema`, `outputSchema`, `description` and `annotations`; only the name gained a prefix.

Two properties hold across both catalogs. A desktop action is posted to the chosen window: no global mouse, no physical pointer movement, no window activation, no clipboard write. A refusal names its reason, so a cancelled or expired call answers `outcome: unknown` with `retry: unsafe` instead of implying the work stopped.

## Configure it

No flag serves MCP: `hand-mcp` with no argument is the session. One environment variable is the only switch.

| Variable | Default | Effect |
|---|---|---|
| `GLADIUS_CDP_URL` | unset | Attach to a browser you already run instead of launching one. hand does not launch, kill or sweep that engine, and closes only pages it created. |

With no override, the first browser call provisions the pinned `chrome-headless-shell` 155.0.8059.84 into a per-user cache and reuses it after that.

## Requirements

- Windows 10 or 11, x64. The manifest declares `os: win32` and `cpu: x64`, and the desktop domain is Win32 and UI Automation; there is no other platform.
- Node 22.14.0 or newer, to run the launcher. Nothing else is needed at install time.
- An MCP client that speaks stdio.

## Status

**Experimental** — pre-release; interfaces may change.

## Links

- [Issues](https://github.com/nathwn12/hand/issues)
- [Releases](https://github.com/nathwn12/hand/releases)
- [MIT licence](https://opensource.org/license/mit)
