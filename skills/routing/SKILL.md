---
name: routing
description: Route work between hand's browser and desktop domains and avoid damaging the host: sessions, scoping, and cleanup rules.
---

# routing

hand exposes two domains over one stdio MCP connection: browser_* (headless Chrome over CDP) and desktop_* (Windows apps through UI Automation). Pick by where the target lives, not by convenience.

## Choose the domain

- Web page, DOM, network, console, storage, screenshot of a page, extension popup, or anything reachable by URL: use browser_*.
- A native window, installer, dialog, tray app, or anything outside the browser: use desktop_*.
- Never drive a native app through the browser, or a web page through UI Automation. If a task spans both, run them as separate steps and keep their state separate.

## Sessions and scoping

- One MCP connection owns one browser engine and one desktop session. Named browser contexts (browser_context) give separate cookie/storage jars; a context does not share tabs with the legacy page.
- browser_snapshot, browser_assert, and browser_evaluate read the legacy page unless context: (and page: when the context holds several) is passed. An unknown context or page is refused typed; there is no fallback to the legacy page.
- Desktop actions always name a target window (hwnd or titleContains); a dead target returns a typed refusal. A window handle is never assumed to survive a close or relaunch.
- Cancellation or budget expiry answers outcome: unknown, retry: unsafe. Do not retry unless you can verify state or the call is read-only.

## The rule that prevents damage

- Never touch the foreground window, the physical pointer, or the clipboard unless the call opts in explicitly.
- Default desktop actions post to the chosen window without focus, pointer movement, or clipboard writes.
- desktop_do with allow_cursor:true moves the real pointer (point guard, then restored); allow_clipboard:true writes the real clipboard (optional restore); allow_foreground:true focuses a window.
- desktop_window needs allow_foreground:true for focus or maximize.
- browser_clipboard writes replace the owner's clipboard text and need allow_clipboard:true.
- Do not pass these flags "just in case". Pass them only when the task requires it and the consequence is understood.

## Cleanup

- Close what you open: tabs, contexts, traces, downloads. A launched browser profile is owned and reaped on EOF; an attached engine (GLADIUS_CDP_URL) is not.
- Verify before reporting: a tool result is delivery, not proof. Re-read state or capture evidence.
- On failure, prefer typed errors and refusals over retries; a refused call leaves state intact.

## Evidence and verification

- A snapshot, screenshot, or assertion is evidence; a tool's success flag is delivery. Re-check after acting.
- Typed errors name the cause (stale ref, dead target, missing policy flag). Read them before retrying.
- For desktop, prefer posted input on a named window; reserve cursor, clipboard, and focus opt-ins for cases that truly need them.
