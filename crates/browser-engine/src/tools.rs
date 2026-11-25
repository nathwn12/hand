use serde_json::{json, Value};

use crate::browser::Browser;
use crate::fetch::fetch_text;

/// Browser tools a `run` batch may dispatch. `run` cannot spawn another batch
/// and `close` cannot tear down the engine the batch is using.
const RUN_TOOLS: &[&str] = &[
    "navigate", "snapshot", "screenshot", "click", "type", "evaluate", "wait_for", "targets",
    "dialog", "history", "network", "console", "storage", "tabs", "viewport", "select", "hover",
    "key", "upload", "layout", "assert_layout", "visual", "cdp", "fetch_text", "context", "wheel",
    "drag", "form", "download", "assert", "audit", "trace",
];

pub fn list() -> Value {
    let mut listed = json!({"tools": [
        {"name": "navigate", "title": "Navigate",
         "description": "Navigate the active page to a URL",
         "inputSchema": {"type": "object", "properties": {"url": {"type": "string", "format": "uri"}}, "required": ["url"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"url": {"type": "string"}, "title": {"type": "string"}, "warning": {"type": "string"},
               "viewport": {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}, "required": ["width", "height"]},
               "observed": {"type": "object", "properties": {
                   "status": {"type": ["integer", "null"]},
                   "redirects": {"type": "array", "items": {"type": "object"}},
                   "timings": {"type": "object", "properties": {
                       "request": {"type": "number"}, "response": {"type": ["number", "null"]},
                       "finish": {"type": ["number", "null"]}}},
                   "note": {"type": "string"}},
                   "required": ["status", "redirects", "timings"], "additionalProperties": false}},
            "required": ["url", "title"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}},
        {"name": "snapshot", "title": "Snapshot",
         "description": "Snapshot: one line per node with a stable ref (@eN, unique per snapshot), role/name/value, plus page text. Default is the real Accessibility tree of the page itself: open shadow roots are included, iframe content is NOT part of this tree (snapshot the frame with frame= to see inside it), and hidden/ignored subtrees are excluded. mode:\"legacy\" keeps the old DOM heuristic (the only path that stamps an attribute into the page); mode:\"text\" is text-only. frame=<CDP frameId> from targets scopes the snapshot to that iframe: a same-process frame is read on the page session, a mapped out-of-process frame (OOPIF) is read on its own session, and an OOPIF with no mapped session is a typed error rather than the page's tree. AX refs resolve by backend node id, so a shadow ref is actionable exactly like a light-DOM ref. Paging: pass the snapshotId from the offset:0 render with offset>0 (required; paging slices that render and never re-renders). context=<name> reads a page of that NAMED isolated browser context instead of the legacy page (the name from browser_context create/list): the reply names the context and the page targetId it read, and the legacy page, its selection and its refs are untouched. Without page=, a context holding exactly one page is read; a context holding several is a typed invalid_args/ambiguous_target listing every targetId, and an unknown context or page is a typed invalid_target/not_found that never falls back to the legacy page. Refs minted from a context page are read-only labels for that render: the legacy action tools still address the legacy page. mode:\"legacy\" and frame= are refused with context= (frame-scoped refs do not exist for context pages).",
         "inputSchema": {"type": "object", "properties": {"mode": {"type": "string", "enum": ["ax", "legacy", "text"]}, "frame": {"type": "string"}, "offset": {"type": "integer", "minimum": 0, "default": 0}, "snapshotId": {"type": "string", "description": "required when offset > 0: the id returned by the offset:0 render"}, "context": {"type": "string", "description": "read a page of this named isolated browser context instead of the legacy page"}, "page": {"type": "string", "description": "targetId of the context page to read; required when the context holds more than one page"}}, "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {
                "url": {"type": "string"}, "title": {"type": "string"},
                "truncated": {"type": "boolean"},
                "nodes": {"type": "array", "items": {"type": "object",
                    "properties": {"ref": {"type": "string"}, "role": {"type": "string"},
                        "name": {"type": "string"}, "tag": {"type": "string"},
                        "disabled": {"type": "boolean"}, "value": {"type": "string"}},
                    "required": ["ref", "role", "name", "tag", "disabled", "value"],
                    "additionalProperties": false}},
                "snapshotId": {"type": "string"},
                "offset": {"type": "integer", "minimum": 0},
                "totalLines": {"type": "integer", "minimum": 0},
                "nextOffset": {"type": ["integer", "null"]},
                "context": {"type": "string"}, "target": {"type": "string"},
                "mode": {"type": "string"}, "text": {"type": "string"},
                "viewport": {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}, "required": ["width", "height"]}},
            "required": [], "additionalProperties": false,
            "anyOf": [{"required": ["url", "title", "truncated", "nodes", "snapshotId", "totalLines", "nextOffset"]}, {"required": ["mode", "text"]}]},
         "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}},
        {"name": "screenshot", "title": "Screenshot",
         "description": "PNG screenshot of the active page (unavailable with GLADIUS_BACKEND_CAPS=dom-only: no rendering pipeline). Optional path: an ABSOLUTE filesystem path the decoded PNG is also written to (missing parent directories are created; a failed write is a typed error, never a silent success). There is no allowlisted screenshot root: only absolute paths are accepted, and a relative path is rejected. When path is absent the reply shape is unchanged. deterministic:true pins the capture: device metrics fixed (current viewport, or width/height; deviceScaleFactor 1, mobile false) and restored afterwards, prefers-reduced-motion:reduce while the capture runs, cleared back to the engine/host default afterwards (a host that asks for reduced motion reports reduce again, so the guarantee is unchanged-from-baseline; reducedMotionEmulated reports whether the emulation was applied at all, and the freeze stylesheet is what stops CSS animation), a temporary style element freezing animation/transition/caret-color/scrollbars, applied by a walk that reaches the main document, every same-process descendant frame and every OPEN shadow root, and that pauses WAAPI animations, SVG SMIL timelines and media playback (each pinned to its start, because a pause at a wall-clock position is not reproducible); a realm the walk cannot reach is named in freeze.skipped, then `document.fonts.status === 'loaded'` plus every `FontFace` reaching `loaded` or `error` (a face still loading at the budget is counted in `fontsPending` and named in `settleTimeouts`), every img complete and two animation-frame ticks before Page.captureScreenshot({format:png, fromSurface:true, captureBeyondViewport for full page}). The reply then carries viewport, deviceScaleFactor, url, byte length, a cheap content checksum, reducedMotionEmulated, settled, settleTimeouts, fontsPending, freezeMode and freeze. deterministic pins the capture so two captures of the same page in the same engine produce the same PNG bytes; the scope of the freeze is the main document, same-process frames, open shadow roots, CSS animation, WAAPI, SMIL and media (see freeze). Canvas, WebGL, video frames and cross-process iframes are sampled, not frozen. Everything rides CDP: no OS cursor, no OS keyboard, no window, no foreground tab. Scope: element= (a CSS selector or an @eN ref from browser_snapshot) captures exactly that element's border box, and clip={x,y,width,height} captures a page-coordinate region (the same space browser_layout's box reports); both ride Page.captureScreenshot's clip, so the pixel size is the box size at deviceScaleFactor 1, and an element with no layout box (display:none, detached) is a typed not_actionable/hidden refusal rather than an empty image. An unknown ref is a typed stale_ref. fullPage and clip together are refused. format=png (default)|jpeg|webp sets the encoding and the reply's mimeType; quality=0-100 applies to jpeg/webp only and is refused with png instead of being silently ignored. The default capture (no element, no clip, no format) is byte-for-byte the historic PNG.",
         "inputSchema": {"type": "object", "properties": {
             "fullPage": {"type": "boolean"}, "path": {"type": "string"},
             "deterministic": {"type": "boolean"},
             "width": {"type": "integer"}, "height": {"type": "integer"},
             "element": {"type": "string", "description": "CSS selector or @eN ref: capture that element's box"},
             "clip": {"type": "object", "description": "page-coordinate region to capture",
                 "properties": {"x": {"type": "number"}, "y": {"type": "number"},
                     "width": {"type": "number", "exclusiveMinimum": 0},
                     "height": {"type": "number", "exclusiveMinimum": 0}},
                 "required": ["x", "y", "width", "height"], "additionalProperties": false},
             "format": {"type": "string", "enum": ["png", "jpeg", "webp"], "default": "png"},
             "quality": {"type": "integer", "minimum": 0, "maximum": 100},
              "freeze": {"type": "boolean"},
              "waitForStable": {"type": "integer", "minimum": 0},
              "force": {"type": "boolean"},
              "scale": {"type": "number", "minimum": 0.1, "maximum": 1}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"mimeType": {"type": "string"}, "format": {"type": "string"}, "bytes": {"type": "integer", "minimum": 0},
                "path": {"type": "string"}, "deterministic": {"type": "boolean"},
                "element": {"type": "string"}, "clip": {"type": "object"},
                "viewport": {"type": "object"}, "deviceScaleFactor": {"type": "number"},
                "size": {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}, "required": ["width", "height"]},
                "url": {"type": "string"}, "checksum": {"type": "string"}, "stability": {"type": "object"},
                "reducedMotionEmulated": {"type": "boolean"},
                "settled": {"type": "boolean"}, "settleTimeouts": {"type": "array"},
                "freeze": {"type": "object"}, "freezeMode": {"type": "string"},
                "fontsPending": {"type": "integer", "minimum": 0},
                 "motion": {"type": "array", "items": {"type": "string"}},
                 "paused": {"type": "array", "items": {"type": "string"}}},
            "required": ["mimeType", "bytes"], "additionalProperties": false},
         "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "click", "title": "Click",
         "description": "Click an element by CSS selector (first match) or AX ref (@eN from snapshot; exact). AX selector forms are also accepted and are uniqueness-mandatory: role=<role>, role=<role>[name=\"<accessible name>\"], name=\"<accessible name>\", text=\"<text>\" (exact visible text inside one element; when nested elements contain exactly that text the innermost one is the match) and label=\"<label text>\" (form control by its associated <label>). They are resolved fresh at action time on the accessibility tree browser_snapshot renders for the page itself (like a CSS selector, so a re-rendered element keeps answering while a snapshot-bound @eN would go stale; iframe content is not part of that tree - pass frame= to search inside a frame); names and text are whitespace-normalised and compared case-insensitively, and text= reads visible text while name=/label= read the accessible name. Zero matches is invalid_target/not_found naming the selector; more than one is invalid_target/ambiguous listing every candidate as @eN plus role and name - the first match is never taken. Every reply adds resolved {selector, ref, role, name}, where ref is that element's @eN in the current snapshot (null when the snapshot holds none). resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control), so a name copied out of a snapshot resolves. resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control). role= takes an ARIA role string such as button, generic, StaticText or Iframe (capital I), never a tag name; quote values with double quotes and escape an inner double quote as \". The click is exactly two CDP events at the element's exact float box centre: mousePressed then mouseReleased - no mouseMoved, no hover, no movement loop, no interpolation (with frame= pointing at a cross-process OOPIF, ONE positioning mouseMoved precedes the press, because the browser routes pointer input to a child renderer only after a move at that position; that is routing, not hover). hoverFirst:true sends ONE mouseMoved at that same point first, for pages whose handlers read hover state; the default is false. frame=<CDP frameId> from targets scopes to that iframe (same-process CSS selectors; a frame-scoped AX selector is resolved inside that frame's own accessibility tree and never against the page; a ref is actionable only in the frame whose own snapshot minted it (main-frame refs stay main-frame); an out-of-process frame with no mapped session is a typed error, never a main-frame click. button=left (default)|right|middle chooses the pointer button; a right click raises the engine's own contextmenu, and the page's menu (a DOM node it shows) is what the next snapshot reports. clickCount (default 1) is passed to the engine: 2 is a DOUBLE click, so the engine raises dblclick on the second press - no synthetic DOM event, no second-guessing. Existing calls are unchanged: no button and no clickCount is exactly the historic press+release pair with left/1)",
         "inputSchema": {"type": "object", "properties": {"element": {"type": "string"}, "frame": {"type": "string"}, "hoverFirst": {"type": "boolean"}, "button": {"type": "string", "enum": ["left", "right", "middle"], "default": "left"}, "clickCount": {"type": "integer", "minimum": 1, "maximum": 3, "default": 1}}, "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "dialog": {"type": "object"}, "note": {"type": "string"}, "actionability": {"type": "object"},
                "resolved": {"type": "object",
                    "properties": {"selector": {"type": "string"}, "ref": {"type": ["string", "null"]},
                        "role": {"type": "string"}, "name": {"type": "string"}},
                    "required": ["selector", "ref", "role", "name"], "additionalProperties": false}},
            "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "type", "title": "Type",
         "description": "Type text into an element by CSS selector (first match) or AX ref (@eN from snapshot; exact). AX selector forms are also accepted and are uniqueness-mandatory: role=<role>, role=<role>[name=\"<accessible name>\"], name=\"<accessible name>\", text=\"<text>\" (exact visible text inside one element; when nested elements contain exactly that text the innermost one is the match) and label=\"<label text>\" (form control by its associated <label>). They are resolved fresh at action time on the accessibility tree browser_snapshot renders for the page itself (like a CSS selector, so a re-rendered element keeps answering while a snapshot-bound @eN would go stale; iframe content is not part of that tree - pass frame= to search inside a frame); names and text are whitespace-normalised and compared case-insensitively, and text= reads visible text while name=/label= read the accessible name. Zero matches is invalid_target/not_found naming the selector; more than one is invalid_target/ambiguous listing every candidate as @eN plus role and name - the first match is never taken. Every reply adds resolved {selector, ref, role, name}, where ref is that element's @eN in the current snapshot (null when the snapshot holds none). resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control), so a name copied out of a snapshot resolves. resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control). role= takes an ARIA role string such as button, generic, StaticText or Iframe (capital I), never a tag name; quote values with double quotes and escape an inner double quote as \". frame=<CDP frameId> from targets scopes to that iframe (same-process CSS selectors; a frame-scoped AX selector is resolved inside that frame's own accessibility tree and never against the page; refs are main-frame only - with frame= use a CSS or AX selector; OOPIF without a mapped target is a typed error, never a main-frame type)",
         "inputSchema": {"type": "object", "properties": {
             "element": {"type": "string"}, "text": {"type": "string"}, "submit": {"type": "boolean"}, "frame": {"type": "string"}},
             "required": ["text"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "valueLength": {"type": "integer", "minimum": 0}, "note": {"type": "string"}, "actionability": {"type": "object"},
                "resolved": {"type": "object",
                    "properties": {"selector": {"type": "string"}, "ref": {"type": ["string", "null"]},
                        "role": {"type": "string"}, "name": {"type": "string"}},
                    "required": ["selector", "ref", "role", "name"], "additionalProperties": false}},
            "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "evaluate", "title": "Evaluate",
         "description": "Evaluate a JS expression in the page, await promises. Optional realm routing: target=<CDP targetId> evaluates in that target's realm (page, dedicated/shared worker, service worker); frame=<CDP frameId> evaluates in that iframe's realm (isolated world, works cross-origin). Without either, the active page's main frame. target and frame are mutually exclusive.",
         "inputSchema": {"type": "object", "properties": {"expression": {"type": "string"}, "target": {"type": "string"}, "frame": {"type": "string"}}, "required": ["expression"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"value": {}, "void": {"type": "boolean"}, "dialog": {"type": "object"}, "note": {"type": "string"}},
            "required": ["value"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "wait_for", "title": "Wait For",
         "description": "Wait for a selector/text, a JS expression condition, a delay, or network idle (max 15s). networkIdle:true waits for a quiet-network window of idleMs (default 500); a page that never goes idle returns the typed timeout. expression: polls the active page until the JS evaluates truthy (backoff cadence inside ms); expiry is the typed timeout.",
         "inputSchema": {"type": "object", "properties": {
             "selector": {"type": "string"}, "text": {"type": "string"}, "ms": {"type": "integer"},
             "networkIdle": {"type": "boolean"}, "idleMs": {"type": "integer"}, "expression": {"type": "string"}},
             "minProperties": 1, "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "targets", "title": "Targets",
         "description": "List every CDP target with its stable id (pages, workers of all kinds) plus the active page's frame tree (main frame + nested iframes, each with its frameId), or make a page target current. action list (default)|select; select needs target=<targetId> (page targets only; worker realms are reached via evaluate target=).",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["list", "select"]}, "target": {"type": "string"}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"targets": {"type": "array"}, "frames": {"type": "array"},
                "tabs": {"type": "array"}, "ok": {"type": "boolean"}},
            "required": [], "additionalProperties": false,
            "anyOf": [{"required": ["targets"]}, {"required": ["tabs"]}]},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "dialog", "title": "Dialog",
         "description": "JS-dialog policy and record. The reader auto-handles every alert/confirm/prompt/beforeunload with the policy (default accept) so a session never deadlocks; the dialog's own text is recorded. action get (default: last dialog + current policy)|policy (set default: needs accept bool, optional promptText for prompt dialogs).",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["get", "policy"]}, "accept": {"type": "boolean"}, "promptText": {"type": "string"}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"dialog": {}, "policy": {"type": "object",
                "properties": {"accept": {"type": "boolean"}, "promptText": {"type": "string"}},
                "required": ["accept"], "additionalProperties": false}},
            "required": ["policy"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "history", "title": "History",
         "description": "Navigation history on the active page: back|forward step one entry, reload replays the current one. Returns the live url+title after the load settle; a step past the boundary is a typed error, never a silent no-op.",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["back", "forward", "reload"]}}, "required": ["action"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "url": {"type": "string"}, "title": {"type": "string"}},
            "required": ["ok", "url", "title"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}},
        {"name": "network", "title": "Network",
         "description": "Observable network: captured requests/responses (method, url, status, resource type, timing) since the Network domain was first enabled, cap 500. action get (default)|drain|clear; filter is a plain substring matched against the url; limit caps returned (default 100).",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["get", "drain", "clear", "route", "mock", "abort", "offline", "restore"]}, "filter": {"type": "string"}, "limit": {"type": "integer"},
             "blockedUrls": {"type": "array", "items": {"type": "string"}}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"entries": {"type": "array"}, "count": {"type": "integer"},
                "total": {"type": "integer"}, "cap": {"type": "integer"}, "cleared": {"type": "integer"}},
            "required": ["cap"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}},
        {"name": "cdp", "title": "CDP",
         "description": "Raw CDP escape hatch: send any protocol method with params on the browser-level connection or on one named session. Returns the raw CDP result; a CDP error reply is a tool error (isError), never a protocol error.",
         "inputSchema": {"type": "object", "properties": {
             "method": {"type": "string"}, "params": {"type": "object"}, "session": {"type": "string"}},
             "required": ["method"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"result": {}}, "required": ["result"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "console", "title": "Console",
         "description": "Console capture: buffered console messages + uncaught exceptions, each {type,text,url,line,ts}. Ring buffer, cap 200. action get (default)|drain|clear; limit caps returned (default 100).",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["get", "drain", "clear"]}, "limit": {"type": "integer"}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"messages": {"type": "array"}, "count": {"type": "integer"},
                "total": {"type": "integer"}, "cap": {"type": "integer"}, "cleared": {"type": "integer"}},
            "required": ["cap"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}},
        {"name": "upload", "title": "Upload",
         "description": "Set files on an <input type=file> by CSS selector (first match) or AX ref (@eN from snapshot; exact). AX selector forms are also accepted and are uniqueness-mandatory: role=<role>, role=<role>[name=\"<accessible name>\"], name=\"<accessible name>\", text=\"<text>\" (exact visible text; the innermost element wins when nested ones contain it) and label=\"<label text>\" (form control by its associated <label>), resolved fresh at action time on the accessibility tree of the page itself (iframe content is not in that tree - pass frame= to search inside a frame) with whitespace-normalised, case-insensitive comparison; zero matches is invalid_target/not_found, more than one is invalid_target/ambiguous with every candidate as @eN plus role and name, and the reply adds resolved {selector, ref, role, name}. resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control). role= takes an ARIA role string such as button, generic, StaticText or Iframe (capital I), never a tag name; quote values with double quotes and escape an inner double quote as \". paths[] must exist locally. frame=<CDP frameId> from targets scopes to that iframe (same-process CSS selectors; a frame-scoped AX selector is resolved inside that frame's own accessibility tree and never against the page; refs are main-frame only - with frame= use a CSS or AX selector; OOPIF without a mapped target is a typed error, never a main-frame upload)",
         "inputSchema": {"type": "object", "properties": {
             "element": {"type": "string"}, "paths": {"type": "array", "items": {"type": "string"}, "minItems": 1}, "frame": {"type": "string"}},
             "required": ["paths"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "files": {"type": "integer"},
                "resolved": {"type": "object",
                    "properties": {"selector": {"type": "string"}, "ref": {"type": ["string", "null"]},
                        "role": {"type": "string"}, "name": {"type": "string"}},
                    "required": ["selector", "ref", "role", "name"], "additionalProperties": false}},
            "required": ["ok", "files"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}},
        {"name": "fetch_text", "title": "Fetch Text",
         "description": "Fast plain-HTTP GET with no browser; HTML stripped to text",
         "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"text": {"type": "string"}, "url": {"type": "string"}, "chars": {"type": "integer"}},
            "required": ["text", "url", "chars"], "additionalProperties": false},
         "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}},
        {"name": "hover", "title": "Hover",
         "description": "Hover over an element by CSS selector (first match) or AX ref (@eN from snapshot; exact). AX selector forms are also accepted and are uniqueness-mandatory: role=<role>, role=<role>[name=\"<accessible name>\"], name=\"<accessible name>\", text=\"<text>\" (exact visible text inside one element; when nested elements contain exactly that text the innermost one is the match) and label=\"<label text>\" (form control by its associated <label>), resolved fresh at action time on the accessibility tree of the page itself (iframe content is not in that tree - pass frame= to search inside a frame) with whitespace-normalised, case-insensitive comparison; zero matches is invalid_target/not_found, more than one is invalid_target/ambiguous with every candidate as @eN plus role and name, and the reply adds resolved {selector, ref, role, name}. resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control). role= takes an ARIA role string such as button, generic, StaticText or Iframe (capital I), never a tag name; quote values with double quotes and escape an inner double quote as \". frame=<CDP frameId> from targets scopes to that iframe (same-process CSS selectors; a frame-scoped AX selector is resolved inside that frame's own accessibility tree and never against the page; refs are main-frame only - with frame= use a CSS or AX selector; OOPIF without a mapped target is a typed error, never a main-frame hover)",
         "inputSchema": {"type": "object", "properties": {"element": {"type": "string"}, "frame": {"type": "string"}}, "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "resolved": {"type": "object",
                "properties": {"selector": {"type": "string"}, "ref": {"type": ["string", "null"]},
                    "role": {"type": "string"}, "name": {"type": "string"}},
                "required": ["selector", "ref", "role", "name"], "additionalProperties": false}},
            "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}},
        {"name": "select", "title": "Select",
         "description": "Choose a <select> option by value (preferred) or visible text; element is a CSS selector or AX ref (@eN). AX selector forms are also accepted and are uniqueness-mandatory: role=<role>, role=<role>[name=\"<accessible name>\"], name=\"<accessible name>\", text=\"<text>\" (exact visible text; the innermost element wins when nested ones contain it) and label=\"<label text>\", resolved fresh at action time on the accessibility tree of the page itself (iframe content is not in that tree - pass frame= to search inside a frame) with whitespace-normalised, case-insensitive comparison; zero matches is invalid_target/not_found, more than one is invalid_target/ambiguous with every candidate as @eN plus role and name, and the reply adds resolved {selector, ref, role, name}. resolved.name is the name browser_snapshot prints for it (capped at 80 characters, or the fallback name of an unlabeled control). role= takes an ARIA role string such as button, generic, StaticText or Iframe (capital I), never a tag name; quote values with double quotes and escape an inner double quote as \". frame=<CDP frameId> from targets scopes to that iframe (same-process CSS selectors; a frame-scoped AX selector is resolved inside that frame's own accessibility tree and never against the page; refs are main-frame only - with frame= use a CSS or AX selector; OOPIF without a mapped target is a typed error, never a main-frame select)",
         "inputSchema": {"type": "object", "properties": {
             "element": {"type": "string"}, "value": {"type": "string"}, "text": {"type": "string"}, "frame": {"type": "string"}},
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}, "actionability": {"type": "object"}, "resolved": {"type": "object",
                "properties": {"selector": {"type": "string"}, "ref": {"type": ["string", "null"]},
                    "role": {"type": "string"}, "name": {"type": "string"}},
                "required": ["selector", "ref", "role", "name"], "additionalProperties": false}},
            "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "key", "title": "Key",
         "description": "Press a named key (Enter, Tab, Escape, Backspace, ArrowDown, ...) on the focused element. Combos with '+' (Ctrl+A, Shift+Tab, Alt+F4, Meta+C; case-insensitive modifiers, last token is the base key) hold modifiers (Alt=1/Ctrl=2/Meta=4/Shift=8); with Ctrl/Meta/Alt held the base dispatches as rawKeyDown with no text (Ctrl+A never inserts text)",
         "inputSchema": {"type": "object", "properties": {"key": {"type": "string", "minLength": 1}}, "required": ["key"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "viewport", "title": "Viewport",
         "description": "Set the page viewport in CSS pixels (Emulation.setDeviceMetricsOverride) and return the applied metrics plus a live read-back of window.innerWidth/innerHeight/devicePixelRatio, so an override the engine ignored cannot read as success. reset:true clears the override and restores the engine default (width/height are then ignored). The override is replayed on every new tab.",
         "inputSchema": {"type": "object", "properties": {
             "width": {"type": "integer"}, "height": {"type": "integer"},
             "deviceScaleFactor": {"type": "number"}, "mobile": {"type": "boolean"},
             "reset": {"type": "boolean"}},
             "oneOf": [
                 {"properties": {"reset": {"const": true}}, "required": ["reset"]},
                 {"properties": {"reset": {"const": false}}, "required": ["reset", "width", "height"]},
                 {"required": ["width", "height"], "not": {"required": ["reset"]}}],
             "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"applied": {}, "readback": {"type": "object"},
                "matched": {"type": "boolean"}, "reset": {"type": "boolean"}, "note": {"type": "string"}},
            "required": ["readback"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}},
        {"name": "tabs", "title": "Tabs",
         "description": "List, open, close, or select browser tabs (pages)",
         "inputSchema": {"type": "object", "properties": {
             "action": {"type": "string", "enum": ["list", "new", "select", "close"]}, "url": {"type": "string"}, "index": {"type": "integer"}},
             "required": ["action"], "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"tabs": {"type": "array"}, "ok": {"type": "boolean"}},
            "required": ["tabs", "ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}},
        {"name": "close", "title": "Close",
         "description": "Shut down the browser (owned process tree) or detach from an attached external engine (GLADIUS_CDP_URL; engine left running)",
         "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
         "outputSchema": {"type": "object",
            "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": false},
         "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}}
    ]});
    {
        let tools = listed["tools"].as_array_mut().expect("tools array");
        let pos = tools
            .iter()
            .position(|t| t.get("name").and_then(|v| v.as_str()) == Some("tabs"))
            .expect("tabs tool")
            + 1;
        tools.insert(pos, json!({"name": "storage", "title": "Storage",
          "description": "Cookies + Web Storage in one call. action get|set|clear|save|load (default get); scope cookies|localStorage|sessionStorage|all (default all). save/load persist a versioned JSON file (replies carry counts/bytes/origin only, never values); load enforces an origin match unless force:true. allSites:true widens cookie get to every site and cookie clear to the whole profile (clear with allSites on an attached engine needs confirm:true). redact:true masks get values. sessionStorage is tab-scoped (clears with the tab) while localStorage persists.",
          "inputSchema": {"type": "object", "properties": {
              "action": {"type": "string"}, "scope": {"type": "string"},
              "cookies": {"type": "array", "items": {"type": "object"}}, "items": {"type": "object"},
              "path": {"type": "string"}, "allSites": {"type": "boolean"},
              "redact": {"type": "boolean"}, "overwrite": {"type": "boolean"},
              "force": {"type": "boolean"}, "confirm": {"type": "boolean"}},
              "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"ok": {"type": "boolean"}, "action": {"type": "string"},
                 "scope": {"type": "string"}, "url": {"type": "string"}, "origin": {"type": "string"},
                 "cookies": {"type": "array"}, "localStorage": {"type": "object"}, "sessionStorage": {"type": "object"},
                 "counts": {"type": "object"}, "set": {"type": "object"}, "cleared": {"type": "object"},
                 "failed": {"type": "array"}, "applied": {"type": "object"},
                 "path": {"type": "string"}, "bytes": {"type": "integer"},
                 "fileOrigin": {"type": "string"}, "redacted": {"type": "boolean"}, "note": {"type": "string"}},
             "required": ["ok", "action"], "additionalProperties": false},
          "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false}}));
        let at = tools
            .iter()
            .position(|t| t.get("name").and_then(|v| v.as_str()) == Some("close"))
            .unwrap_or(tools.len());
        tools.insert(at, json!({"name": "assert_layout", "title": "Assert Layout",
          "description": "Prove geometry without pixels: one call, one or many targets (ref @eN and/or CSS selector, string or array), several checks. Checks: exists (default true; false asserts absence), visible (a real box, not display:none/visibility:hidden/collapse, opacity>0), stableFrames (N animation frames, identical bbox), box {x,y,w,h,tolerance} in VIEWPORT CSS px from DOM.getBoxModel, hitTarget (elementFromPoint at the exact centre is the target or a descendant - the same guard click uses), computed (subset of getComputedStyle, string-exact), noOverflow (scrollWidth/Height <= clientWidth/Height + tolerance), inViewport (whole box inside the visual viewport). Returns per-check {target, check, pass, expected, actual, delta} plus an overall pass. deterministic:true measures under the same freeze a deterministic screenshot uses (animation:none, WAAPI/SMIL/media held, frames and open shadow roots walked) and echoes freezeApplied:true - the ONLY way to compare geometry with a deterministic capture, because that capture measures the animation-off state. screenshotOnFail:true adds one deterministic capture's path/bytes/checksum. Values are engine measurements (DOM.getBoxModel + Runtime.evaluate), never guesses. frame=<CDP frameId> from targets scopes every measurement to that iframe's realm (CSS selectors; refs are main-frame only); the main frame is measured when frame is absent. Headless + CDP only: no OS cursor, no OS keyboard, no window, no foreground tab.",
          "inputSchema": {"type": "object", "properties": {
              "ref": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
              "selector": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
              "checks": {"type": "object", "properties": {
                  "exists": {"type": "boolean"}, "visible": {"type": "boolean"},
                  "stableFrames": {"type": "integer", "minimum": 1},
                  "box": {"type": "object", "properties": {
                      "x": {"type": "number"}, "y": {"type": "number"},
                      "w": {"type": "number"}, "h": {"type": "number"},
                      "tolerance": {"type": "number"}}, "additionalProperties": false},
                  "hitTarget": {"type": "boolean"},
                  "computed": {"type": "object"},
                  "noOverflow": {"oneOf": [{"type": "boolean"}, {"type": "number"},
                      {"type": "object", "properties": {"tolerance": {"type": "number"}}, "additionalProperties": false}]},
                  "inViewport": {"type": "boolean"}},
                  "additionalProperties": false},
              "screenshotOnFail": {"type": "boolean"},
              "screenshotPath": {"type": "string"},
              "deterministic": {"type": "boolean"}, "frame": {"type": "string"}},
              "anyOf": [{"required": ["ref"]}, {"required": ["selector"]}],
              "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"pass": {"type": "boolean"}, "checks": {"type": "array"},
                 "targets": {"type": "array"}, "screenshot": {"type": "object"},
                 "freezeApplied": {"type": "boolean"}},
             "required": ["pass", "checks", "targets"], "additionalProperties": false},
          "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}}));
        tools.insert(at + 1, json!({"name": "layout", "title": "Layout",
          "description": "The where-is-it-actually primitive: every match of a selector or ref list with its viewport rect, page-coordinate box (DOM.getBoxModel), content quad count, scroll size (scrollWidth/Height vs clientWidth/Height), computed display/visibility/opacity, a z-order hint (paint order, z-index, position, stacking-context flag) and every detected overlapping pair with its intersection rect and area (includeOverlaps default true). frame=<CDP frameId> from targets scopes every measurement to that iframe's realm (CSS selectors; refs are main-frame only); the main frame is measured when frame is absent. Read-only; engine measurements only. Headless + CDP only: no OS input, no window.",
          "inputSchema": {"type": "object", "properties": {
              "ref": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
              "selector": {"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]},
              "includeOverlaps": {"type": "boolean"}, "frame": {"type": "string"}},
              "anyOf": [{"required": ["ref"]}, {"required": ["selector"]}],
              "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"elements": {"type": "array"}, "overlaps": {"type": "array"},
                 "viewport": {"type": "object"}, "count": {"type": "integer"},
                 "requested": {"type": "integer"}},
             "required": ["elements", "overlaps", "viewport"], "additionalProperties": false},
          "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}}));
        tools.insert(at + 2, json!({"name": "visual", "title": "Visual Baselines",
          "description": "Deterministic visual-regression baselines under GLADIUS_VISUAL_DIR (default %LOCALAPPDATA%/gladius/visual on Windows, never the caller's cwd), names sanitised to [A-Za-z0-9._-]. The store is capped at 500 baselines and 500 .diff.png by-products (GLADIUS_VISUAL_CAP overrides both): a save at the cap is a typed refusal naming `visual delete` and GLADIUS_VISUAL_CAP, never a silent eviction, while overwriting a stored baseline is always allowed; .diff.png files are the only by-products aged out automatically (oldest first) and `list` reports dir, count, cap and diffCount. action save captures a deterministic PNG named <name> plus <name>.meta.json (browser, viewport, deviceScaleFactor, url, checksum, createdAt). frame=<CDP frameId> from targets scopes a save/compare capture to that iframe's box and reports the clip used; refs are main-frame only and the main frame is captured when frame is absent. compare captures the same deterministic shot and diffs it against the baseline in a scratch headless target created with background:true (the engine decodes both PNGs onto a canvas - no new runtime dependency), returning two separate verdicts plus the numbers: pixelsIdentical (checksum == baselineChecksum, a fact about the bytes, never a judgement) and passWithinTolerance (the whole predicate: no size mismatch, diffPixels/maxDiffPixels, ratio/maxDiffRatio, ignoredPixels when allowIgnoredAA is off, and environment.match when requireEnvironmentMatch is on). pass is an ALIAS of passWithinTolerance, always equal to it, kept for 0.7.x clients; it means 'the diff is within the requested tolerance', never 'the images are the same'. Also returned: diffPixels, totalPixels, ratio, bbox, maxChannelDelta, baselinePath, diffPath and environment {match, known, unknownAxes, baseline, current}; the environment compares browser major AND full version, viewport, deviceScaleFactor, platform, user agent, colour scheme, forced colours, contrast and reduced-motion preferences, the installed-font fingerprint and the scrollbar width - all read live - and any mismatch adds a baseline_environment_mismatch warning (not a failure unless requireEnvironmentMatch:true), while an axis a 0.7.x sidecar does not record is named in unknownAxes and reported as baseline_environment_unknown_axes, never as a silent match. Anti-aliasing is subtracted by a deterministic rule - a candidate whose 3x3 luminance neighbourhood has >=32 levels of contrast in BOTH images is suppressed when its 4-connected component is no larger than minDiffCluster (default 2) - and that subtraction does NOT pass by default: allowIgnoredAA:true is the explicit opt-in that re-admits it, and the warning always names the count and the flag. The threshold is never raised for tolerance. antialias (default true) and includeAA (default false) are inverses. threshold is a per-channel fraction of the 0-255 range (default 0.1), maxDiffPixels/maxDiffRatio set the pass bar (default 0/0), maskRects skip regions and are metered (maskRectsApplied echoes the clipped rects, maskedPixels counts their union, maskCoverageRatio is that over totalPixels, a rect that clips away is named mask_rect_dropped and coverage over 10% warns mask_coverage); update:true rewrites the baseline and its sidecar; list and delete manage them. compare opens and closes one scratch target on the connected engine, always created with background:true so it never activates (on an external attach that is a real extra tab for a moment); an engine that refuses a background target is refused with a typed scratch_background_unsupported error, never retried in the foreground. Headless + CDP only: no OS input, no window.",
          "inputSchema": {"type": "object", "properties": {
              "action": {"type": "string", "enum": ["save", "compare", "list", "delete"]},
              "name": {"type": "string"}, "fullPage": {"type": "boolean"},
               "frame": {"type": "string"},
              "threshold": {"type": "number"}, "maxDiffPixels": {"type": "integer"},
              "maxDiffRatio": {"type": "number"},
              "antialias": {"type": "boolean"},
              "includeAA": {"type": "boolean"},
              "allowIgnoredAA": {"type": "boolean"},
              "minDiffCluster": {"type": "integer", "minimum": 0},
              "requireEnvironmentMatch": {"type": "boolean"},
              "maskRects": {"type": "array", "items": {"type": "object", "properties": {
                  "x": {"type": "number"}, "y": {"type": "number"},
                  "w": {"type": "number"}, "h": {"type": "number"}},
                  "required": ["x", "y", "w", "h"], "additionalProperties": false}},
              "update": {"type": "boolean"}},
              "required": ["action"], "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"ok": {"type": "boolean"}, "action": {"type": "string"},
                 "name": {"type": "string"},
                 "pixelsIdentical": {"type": "boolean"},
                 "passWithinTolerance": {"type": "boolean"},
                 "pass": {"type": "boolean"}, "updated": {"type": "boolean"},
                 "diffPixels": {"type": "integer"}, "totalPixels": {"type": "integer"},
                 "ratio": {"type": "number"},
                 "bbox": {"type": ["object", "null"]},
                 "maxChannelDelta": {"type": "integer"}, "baselinePath": {"type": "string"},
                 "diffPath": {"type": ["string", "null"]}, "bytes": {"type": "integer"},
                 "path": {"type": "string"}, "metaPath": {"type": "string"},
                 "antialias": {"type": "boolean"}, "minDiffCluster": {"type": "integer"},
                 "ignoredPixels": {"type": "integer"}, "ignoredClusters": {"type": "integer"},
                 "maskedPixels": {"type": "integer"}, "maskCoverageRatio": {"type": "number"},
                 "maskRectsApplied": {"type": "array"},
                 "scratchBackground": {"type": "boolean"},
                 "settled": {"type": "boolean"}, "settleTimeouts": {"type": "array"},
                 "environment": {"type": "object"},
                 "warning": {"type": ["string", "null"]},
                 "checksum": {"type": "string"}, "baselineChecksum": {"type": "string"},
                 "viewport": {"type": "object"}, "url": {"type": "string"},
                 "baselines": {"type": "array"}, "dir": {"type": "string"},
                 "count": {"type": "integer"}, "cap": {"type": "integer"},
                 "diffCount": {"type": "integer"}, "deleted": {"type": "array"},
                 "sizeMismatch": {"type": "boolean"}, "size": {"type": "object"},
                 "threshold": {"type": "number"}, "maxDiffPixels": {"type": "integer"},
                 "maxDiffRatio": {"type": "number"}, "fullPage": {"type": "boolean"},
                  "frame": {"type": "string"}, "clip": {"type": "object"}},
             "required": ["ok", "action"], "additionalProperties": false},
          "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": false}}));
        tools.insert(at + 3, json!({"name": "drag", "title": "Drag",
          "description": "Drag from one element (or point) to another with real trusted input: one positioning Input.dispatchMouseEvent mouseMoved at the source, one mousePressed there, `steps` interpolated mouseMoved events with buttons:1, then mouseReleased at the target. The opening move is what routes pointer input to the renderer that owns the source (a cross-process frame receives nothing without it - measured), and the move at the source is also what starts a drag. from/to are a CSS selector (first match), an @eN ref from snapshot, or an AX selector (role=/name=/text=/label=); fromPoint/toPoint {x,y} in viewport CSS px stand in for an element on either side, so a drag between bare coordinates needs no DOM at all. Both endpoints run the same visibility/occlusion preflight click uses: a hidden or occluded endpoint is a typed not_actionable refusal and NOTHING is dispatched. Every event is trusted, so the engine's own HTML5 drag-and-drop machinery and the page's pointer handlers both fire - no synthetic DragEvent, no DataTransfer construction, no .value writes. steps (default 8, 2-64) splits the path into that many move events. Reply: the two points, the events dispatched and the step count. frame=<CDP frameId> resolves from/to inside that iframe (CSS or AX selectors; refs are main-frame only) and the dispatch point is mapped to main-viewport coordinates. Optional dataTransfer is an object of mime type to string data: with it, the drag runs through CDP drag interception (Input.setInterceptDrags / Input.dispatchDragEvent, always restored on success, error and cancel). The page's own dragstart still fires trusted from the real press/move path, but its DataTransfer is empty - CDP has no dragstart dispatch and the supplied items are not attached to a native dragstart. The items reach the page only in the CDP-dispatched dragEnter, dragOver and drop handlers, where both the mime types and the string values are readable. Reply adds dragEvents and echoes dataTransfer.",
          "inputSchema": {"type": "object", "properties": {
              "from": {"type": "string"}, "to": {"type": "string"},
              "fromPoint": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}, "required": ["x", "y"], "additionalProperties": false},
              "toPoint": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}, "required": ["x", "y"], "additionalProperties": false},
              "steps": {"type": "integer", "minimum": 2, "maximum": 64, "default": 8},
               "dataTransfer": {"type": "object", "description": "mime type -> string data"},
              "frame": {"type": "string"}},
              "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"ok": {"type": "boolean"},
                 "from": {"type": "object"}, "to": {"type": "object"},
                 "events": {"type": "integer", "minimum": 0},
                 "steps": {"type": "integer", "minimum": 0},
                  "dragEvents": {"type": "integer", "minimum": 0},
                  "dataTransfer": {"type": "object"}},
             "required": ["ok", "from", "to", "events", "steps"], "additionalProperties": false},
          "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}}));
        tools.insert(at + 4, json!({"name": "wheel", "title": "Wheel",
          "description": "Scroll with real trusted input: ONE Input.dispatchMouseEvent of type mouseWheel at (x,y) - viewport CSS px, default the viewport centre - carrying deltaX/deltaY in pixels. The page's own scroll machinery runs (smooth scrolling, scroll listeners, nested scroll containers, whatever the page uses), so this is not window.scrollBy and not a synthetic WheelEvent. The reply reads scrollX/scrollY back after a bounded settle and reports them with `moved`, so 'it scrolled' is measured rather than assumed: a delta aimed at an element that cannot scroll reports moved:false and the same offsets, never a false success. Coordinates are main-viewport CSS px; pass them from browser_layout when a specific scroller must receive the wheel.",
          "inputSchema": {"type": "object", "properties": {
              "deltaY": {"type": "number"}, "deltaX": {"type": "number"},
              "x": {"type": "number"}, "y": {"type": "number"}},
              "required": ["deltaY"], "additionalProperties": false},
          "outputSchema": {"type": "object",
             "properties": {"ok": {"type": "boolean"}, "moved": {"type": "boolean"},
                 "scroll": {"type": "object"}, "at": {"type": "object"},
                 "deltaX": {"type": "number"}, "deltaY": {"type": "number"}},
             "required": ["ok", "moved", "scroll", "at"], "additionalProperties": false},
          "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}}));
    }
    listed["tools"].as_array_mut().expect("tools").push(crate::download::schema());
    listed["tools"].as_array_mut().expect("tools").push(crate::assert::schema());
    listed["tools"].as_array_mut().expect("tools").push(crate::audit::schema());
    listed["tools"].as_array_mut().expect("tools").push(crate::trace::schema());
    listed["tools"].as_array_mut().expect("tools").push(json!({
        "name": "run", "title": "Run",
        "description": "Ordered browser batch. steps[] runs in order, 1..64 steps of {tool, args?}; tool is one of the browser tools except run and close. continueOnError (default false) stops at the first failing step; true keeps going. timeoutMs (default 30000) bounds each step. A screenshot step with freeze:true is refused with typed invalid_args (a timeout would drop the future and leave the page frozen); call browser_screenshot directly to freeze. Every step reports {index, tool, status, ms, result?|error?} with the step's own structuredContent as evidence; aggregate ok, steps, failed and ms. No desktop tool is accepted.",
        "inputSchema": {"type": "object", "properties": {
            "steps": {"type": "array", "minItems": 1, "maxItems": 64, "items": {"type": "object",
                "properties": {"tool": {"type": "string", "enum": json!(RUN_TOOLS)},
                               "args": {"type": "object"}},
                "required": ["tool"], "additionalProperties": false}},
            "continueOnError": {"type": "boolean"},
            "timeoutMs": {"type": "integer", "minimum": 1}},
            "required": ["steps"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "steps": {"type": "array", "items": {"type": "object", "properties": {
                "index": {"type": "integer"}, "tool": {"type": "string"},
                "status": {"type": "string"}, "ms": {"type": "integer", "minimum": 0},
                "result": {"type": "object"}, "error": {}},
                "required": ["index", "tool", "status", "ms"], "additionalProperties": false}},
            "failed": {"type": "integer", "minimum": 0},
            "ms": {"type": "integer", "minimum": 0}},
            "required": ["ok", "steps", "failed", "ms"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
    }));
    for tool in listed["tools"].as_array_mut().expect("tools") {
        let Some(name) = tool.get("name").and_then(|n| n.as_str()).map(str::to_string) else {
            continue;
        };
        if !matches!(name.as_str(), "click" | "type" | "select" | "hover" | "upload" | "screenshot") {
            continue;
        }
        let Some(schema) = tool.get_mut("inputSchema").and_then(|s| s.as_object_mut()) else {
            continue;
        };
        if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.insert("selector".to_string(), json!({"type": "string"}));
        }
        // Screenshot's element target is optional (no element/clip captures the
        // whole page), so it must stay free of both required and anyOf. The
        // five action tools require one of the two target forms.
        if name != "screenshot" {
            schema.insert(
                "anyOf".to_string(),
                json!([{"required": ["element"]}, {"required": ["selector"]}]),
            );
        }
    }
    listed["tools"].as_array_mut().expect("tools").push(json!({
        "name": "form", "title": "Form",
        "description": "Ordered form batches. fields[] runs in order, each {field, action, value?}: field is a CSS selector, action is type (text in value), select (option value in value), check (click a checkbox) or click. Every step records {field, status: ok|error, error?}; a failing step does NOT stop the batch, so partial failure is reported rather than hidden. Optional submit is a CSS selector clicked after the fields and submitted reports whether that click succeeded. timeoutMs bounds each step. verdict is pass only when every step is ok and a requested submit succeeded, else fail. Every step rides the same trusted CDP path the type/select/click tools use.",
        "inputSchema": {"type": "object", "properties": {
            "fields": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
                "field": {"type": "string"},
                "action": {"type": "string", "enum": ["type", "select", "check", "click"]},
                "value": {"type": "string"}},
                "required": ["field", "action"], "additionalProperties": false}},
            "submit": {"type": "string"}, "timeoutMs": {"type": "integer", "minimum": 1}},
            "required": ["fields"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "steps": {"type": "array", "items": {"type": "object", "properties": {
                "field": {"type": "string"}, "status": {"type": "string"},
                "error": {"type": "string"}, "evidence": {"type": "object"}},
                "required": ["field", "status"], "additionalProperties": false}},
            "submitted": {"type": "boolean"}, "verdict": {"type": "string"}},
            "required": ["steps", "submitted", "verdict"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
    }));
    listed
}

/// Success envelope (G2b): the text block stays byte-identical JSON while
/// `structuredContent` carries the parsed Value for machine readers. Shared
/// with `crate::contexts` so a new tool cannot mint a second envelope shape.
pub(crate) fn ok_text(s: String) -> Value {
    let v: Value = serde_json::from_str(&s).unwrap_or(Value::Null);
    json!({"content": [{"type": "text", "text": s}], "structuredContent": v})
}

/// Typed tool error: the unchanged prose string plus the machine-readable
/// code/reason/context minted at the error SITE (where the failure is
/// known), never derived by re-reading the prose. The prose is partly
/// caller-controlled (selectors, paths, option names), so classifying from
/// it is unsound; each site passes its own code/reason and the real
/// argument value through instead.
///
/// Envelope contract (G1): the prose in `content[0].text` is byte-for-byte
/// what it always was - clients that pattern-match English keep working -
/// and `isError` is unchanged. `structuredContent` rides alongside; a
/// client that ignores the new field sees the old envelope exactly.
#[derive(Debug)]
pub(crate) struct ToolErr {
    prose: String,
    sc: Value,
}

impl ToolErr {
    /// A failure the site types explicitly: stable code/reason plus the
    /// real offending value (`ref` for an AX ref, `element` for a selector,
    /// `path` for a local file, `want` for a wanted option, `name` for an
    /// unknown tool).
    pub(crate) fn typed(prose: String, sc: Value) -> Self {
        Self { prose, sc }
    }

    /// Anything that genuinely knows nothing (CDP/js-engine failures and
    /// other untyped strings): code only, never an invented reason.
    pub(crate) fn fallback(prose: String) -> Self {
        Self {
            prose,
            sc: json!({"code": ToolCode::Error.as_str()}),
        }
    }

    /// Re-mint a failure that WRAPS another one (a rollback reporting its
    /// cause): the cause's `{code, reason}` survive when it carries a marker,
    /// and its `typed:code:reason:` scaffolding never reaches the prose.
    pub(crate) fn wrapping(cause: &str, prose: impl Into<String>) -> Self {
        let mut e = Self::from(cause.to_string());
        e.prose = prose.into();
        e
    }

    pub(crate) fn envelope(self) -> Value {
        json!({"content": [{"type": "text", "text": self.prose}], "isError": true, "structuredContent": self.sc})
    }

    /// The human-readable half (used by tests and by multi-step receipts that
    /// must name a failed step in prose).
    #[cfg(test)]
    pub(crate) fn prose(&self) -> &str {
        &self.prose
    }
}

/// Marker for a typed error minted deep inside the browser/visual layers,
/// which return `Result<_, String>`: `typed:<code>:<reason>: <prose>`.
/// `ToolErr`'s `From<String>` reads it and mints the same
/// `{code, reason}` a tool-site `ToolErr::typed` would, so the type is
/// decided where the failure is known and never re-derived from prose.
pub(crate) const TYPED_MARKER: &str = "typed:";

/// Mint a marker string for [`TYPED_MARKER`]. The code and reason are enum
/// variants, so the marker a deep site mints is always one this build knows.
pub(crate) fn typed_marker(code: ToolCode, reason: ToolReason, prose: &str) -> String {
    format!(
        "{TYPED_MARKER}{}:{}: {prose}",
        code.as_str(),
        reason.as_str()
    )
}

/// `?` on engine calls (`browser.command` / `browser.eval` / serde): those
/// failures carry no site-known type, so they keep the generic fallback ÔÇö
/// except two shapes that DO carry one: the `stale session: ` prefix minted
/// by `send_session` (the typed invalid-target the caller retries
/// explicitly, because the session was dropped without proof of
/// non-execution, so no silent resend happened), and the `typed:` marker
/// minted at a deep site (`engine_dead`, `js_error`, the baseline cap). A
/// marker outside the closed set falls back instead of smuggling a string out.
impl From<String> for ToolErr {
    fn from(prose: String) -> Self {
        if prose.starts_with("stale session: ") {
            return Self::typed(
                prose,
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::StaleSession.as_str()}),
            );
        }
        if let Some(rest) = prose.strip_prefix(TYPED_MARKER) {
            if let Some((code, rest)) = rest.split_once(':') {
                if let Some((reason, msg)) = rest.split_once(':') {
                    if let (Some(code), Some(reason)) =
                        (ToolCode::from_wire(code), ToolReason::from_wire(reason))
                    {
                        return Self::typed(
                            msg.trim_start().to_string(),
                            json!({"code": code.as_str(), "reason": reason.as_str()}),
                        );
                    }
                }
            }
        }
        Self::fallback(prose)
    }
}

/// Closed machine-readable code set for tool errors (SPEC.md ┬º4.1), minted at
/// the error SITE where the failure is known.
///
/// This is a type, not a convention: a code that is not a variant cannot be
/// written, so a typo is a compile error rather than an untyped envelope that
/// only a per-case assertion would catch (L5 ┬º3). `Error` is the residue and
/// is minted by [`ToolErr::fallback`] alone.
///
/// The wire string of each variant is the `code` a client branches on; the
/// reason set is [`ToolReason`], and the pairing each code accepts is in
/// `tests/envelope.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolCode {
    /// An AX ref (`@eN`) that no longer resolves.
    StaleRef,
    /// The element exists but cannot be acted on.
    NotActionable,
    /// The target (selector, ref, tab, frame, file) does not resolve.
    InvalidTarget,
    /// A `select` option that does not match.
    OptionNotFound,
    /// The engine cannot do this under the active backend caps.
    CapabilityGated,
    /// The page moved between resolution and the action.
    TargetChanged,
    /// The owned engine died and the single automatic relaunch did not recover.
    EngineDead,
    /// The target lives in a CLOSED shadow root.
    ShadowClosed,
    /// The call was cancelled or ran past its total budget.
    Cancelled,
    /// The call's own wait expired inside its budget.
    Timeout,
    /// The caller's arguments cannot be honoured.
    InvalidArgs,
    /// The page's JavaScript threw, or returned no value where one was needed.
    JsError,
    /// A local file named by the call is missing.
    FileNotFound,
    /// The engine refused a cookie the caller asked to set.
    CookieRejected,
    /// A saved storage file belongs to another origin.
    OriginMismatch,
    /// The engine cannot host the diff tab in the background.
    ScratchBackgroundUnsupported,
    /// A baseline sidecar carries no usable environment record.
    BaselineEnvironmentUnknown,
    /// A baseline was captured before it settled.
    BaselineEnvironmentUnsettled,
    /// A requested http(s) page never loaded: the document stayed blank.
    BlankLoad,
    /// The residue: a failure that genuinely knows nothing.
    Error,
}

impl ToolCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::StaleRef => "stale_ref",
            Self::NotActionable => "not_actionable",
            Self::InvalidTarget => "invalid_target",
            Self::OptionNotFound => "option_not_found",
            Self::CapabilityGated => "capability_gated",
            Self::TargetChanged => "target_changed",
            Self::EngineDead => "engine_dead",
            Self::ShadowClosed => "shadow_closed",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::InvalidArgs => "invalid_args",
            Self::JsError => "js_error",
            Self::FileNotFound => "file_not_found",
            Self::CookieRejected => "cookie_rejected",
            Self::OriginMismatch => "origin_mismatch",
            Self::ScratchBackgroundUnsupported => "scratch_background_unsupported",
            Self::BaselineEnvironmentUnknown => "baseline_environment_unknown",
            Self::BaselineEnvironmentUnsettled => "baseline_environment_unsettled",
            Self::BlankLoad => "blank_load",
            Self::Error => "error",
        }
    }

    /// The `typed:` marker prefix this code mints, for the
    /// `Result<_, String>` boundary.
    pub(crate) fn marker(self) -> String {
        format!("{TYPED_MARKER}{}:", self.as_str())
    }

    /// Every variant, so a round-trip test can prove none is unreachable.
    pub(crate) const ALL: &'static [ToolCode] = &[
        Self::StaleRef,
        Self::NotActionable,
        Self::InvalidTarget,
        Self::OptionNotFound,
        Self::CapabilityGated,
        Self::TargetChanged,
        Self::EngineDead,
        Self::ShadowClosed,
        Self::Cancelled,
        Self::Timeout,
        Self::InvalidArgs,
        Self::JsError,
        Self::FileNotFound,
        Self::CookieRejected,
        Self::OriginMismatch,
        Self::ScratchBackgroundUnsupported,
        Self::BaselineEnvironmentUnknown,
        Self::BaselineEnvironmentUnsettled,
        Self::BlankLoad,
        Self::Error,
    ];

    /// The variant whose wire string is `s`, or `None` when the marker names
    /// something this build does not know.
    pub(crate) fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_str() == s)
    }
}

/// Closed machine-readable reason set, paired with a [`ToolCode`] at the
/// minting site. One enum rather than one per code: the wire strings are
/// unique across the set, and a shared type is what lets
/// [`ToolCode::marker`]'s inverse stay total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolReason {
    Unknown,
    Navigated,
    NodeGone,
    Hidden,
    Disabled,
    Readonly,
    Occluded,
    NotEditable,
    NotSelect,
    NotFound,
    NotFile,
    NoMatch,
    DomOnly,
    Raced,
    RespawnFailed,
    Transport,
    Unreachable,
    ClientCancelled,
    BudgetExceeded,
    MissingArg,
    /// `element` and its `selector` alias were both passed for one action.
    AmbiguousTarget,
    BadValue,
    BadIndex,
    UnknownAction,
    UnknownTool,
    NeedOption,
    UnknownArg,
    BudgetMismatch,
    Thrown,
    UndefinedResult,
    StaleSession,
    MissingPath,
    BackgroundParamRejected,
    BadEntry,
    Mismatch,
    NotStored,
    OpaqueOrigin,
    Rejected,
    SetFailed,
    SnapshotExpired,
    Unsettled,
    Recorded,
    Missing,
    Malformed,
    Unreadable,
    /// An AX selector form matched more than one element: uniqueness is
    /// mandatory for `role=`/`name=`/`text=`/`label=`, so the call lists the
    /// candidates instead of acting on the first one.
    Ambiguous,
}

impl ToolReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Navigated => "navigated",
            Self::NodeGone => "node_gone",
            Self::Hidden => "hidden",
            Self::Disabled => "disabled",
            Self::Readonly => "readonly",
            Self::Occluded => "occluded",
            Self::NotEditable => "not_editable",
            Self::NotSelect => "not_select",
            Self::NotFound => "not_found",
            Self::NotFile => "not_file",
            Self::NoMatch => "no_match",
            Self::DomOnly => "dom_only",
            Self::Raced => "raced",
            Self::RespawnFailed => "respawn_failed",
            Self::Transport => "transport",
            Self::Unreachable => "unreachable",
            Self::ClientCancelled => "client_cancelled",
            Self::BudgetExceeded => "budget_exceeded",
            Self::MissingArg => "missing_arg",
            Self::AmbiguousTarget => "ambiguous_target",
            Self::BadValue => "bad_value",
            Self::BadIndex => "bad_index",
            Self::UnknownAction => "unknown_action",
            Self::UnknownTool => "unknown_tool",
            Self::NeedOption => "need_option",
            Self::UnknownArg => "unknown_arg",
            Self::BudgetMismatch => "budget_mismatch",
            Self::Thrown => "thrown",
            Self::UndefinedResult => "undefined_result",
            Self::StaleSession => "stale_session",
            Self::MissingPath => "missing_path",
            Self::BackgroundParamRejected => "background_param_rejected",
            Self::BadEntry => "bad_entry",
            Self::Mismatch => "mismatch",
            Self::NotStored => "not_stored",
            Self::OpaqueOrigin => "opaque_origin",
            Self::Rejected => "rejected",
            Self::SetFailed => "set_failed",
            Self::SnapshotExpired => "snapshot_expired",
            Self::Unsettled => "unsettled",
            Self::Recorded => "recorded",
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::Unreadable => "unreadable",
            Self::Ambiguous => "ambiguous",
        }
    }

    pub(crate) const ALL: &'static [ToolReason] = &[
        Self::Unknown,
        Self::Navigated,
        Self::NodeGone,
        Self::Hidden,
        Self::Disabled,
        Self::Readonly,
        Self::Occluded,
        Self::NotEditable,
        Self::NotSelect,
        Self::NotFound,
        Self::NotFile,
        Self::NoMatch,
        Self::DomOnly,
        Self::Raced,
        Self::RespawnFailed,
        Self::Transport,
        Self::Unreachable,
        Self::ClientCancelled,
        Self::BudgetExceeded,
        Self::MissingArg,
        Self::AmbiguousTarget,
        Self::BadValue,
        Self::BadIndex,
        Self::UnknownAction,
        Self::UnknownTool,
        Self::NeedOption,
        Self::UnknownArg,
        Self::BudgetMismatch,
        Self::Thrown,
        Self::UndefinedResult,
        Self::StaleSession,
        Self::MissingPath,
        Self::BackgroundParamRejected,
        Self::BadEntry,
        Self::Mismatch,
        Self::NotStored,
        Self::OpaqueOrigin,
        Self::Rejected,
        Self::SetFailed,
        Self::SnapshotExpired,
        Self::Unsettled,
        Self::Recorded,
        Self::Missing,
        Self::Malformed,
        Self::Unreadable,
        Self::Ambiguous,
    ];

    pub(crate) fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|r| r.as_str() == s)
    }
}

/// Fail loud: every tool rejects arguments outside its own schema with
/// `invalid_args`/`unknown_arg` (never silent acceptance) via
/// `check_unknown` at the top of each dispatch arm.
///
/// `_budgetMs` is the one argument every tool accepts without declaring it:
/// the dispatcher strips and validates it before any tool's `check_unknown`
/// runs, so it is a framework argument rather than a schema key.
///
/// Dispatch a tool call. Always returns the MCP result object
/// (`{"content": [...]}` with optional `isError`); tool failures are NOT
/// protocol errors.
/// Fail-loud argument gate: any key outside the tool's own schema is a
/// typed `invalid_args`/`unknown_arg` error, never a silent ignore. (A
/// silently ignored `target`/`frame`/`contextId` once routed evaluations to
/// the wrong realm with no error; that shape must never recur.) Shared with
/// `crate::contexts` so the new tool rejects unknown keys identically.
pub(crate) fn check_unknown(tool: &str, args: &Value, allowed: &[&str]) -> Result<(), ToolErr> {
    if let Some(obj) = args.as_object() {
        for k in obj.keys() {
            if !allowed.contains(&k.as_str()) {
                return Err(ToolErr::typed(
                    format!("{tool}: unknown argument '{k}' (not in this tool's schema; rejected, not ignored)"),
                    with_received(
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownArg.as_str(), "tool": tool, "arg": k}),
                        args,
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Attach the sorted keys the caller actually sent to an argument-error
/// envelope. A client that prunes unknown keys needs to see what survived.
fn with_received(mut sc: Value, args: &Value) -> Value {
    let mut received: Vec<String> = args
        .as_object()
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();
    received.sort();
    if let Some(obj) = sc.as_object_mut() {
        obj.insert("received".to_string(), json!(received));
    }
    sc
}

/// `selector` is a published alias for `element`. Both is a typed refusal,
/// never a silent first match; neither is left to the caller's own error prose.
fn alias_target(tool: &str, args: &Value) -> Result<Option<String>, ToolErr> {
    let element = args.get("element").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    if element.is_some() && selector.is_some() {
        return Err(ToolErr::typed(
            format!("{tool}: pass element or selector, not both"),
            with_received(
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::AmbiguousTarget.as_str(), "tool": tool}),
                args,
            ),
        ));
    }
    Ok(element.or(selector).map(str::to_string))
}

fn missing_target(tool: &str, args: &Value, prose: &str) -> ToolErr {
    ToolErr::typed(
        prose.to_string(),
        with_received(
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": tool}),
            args,
        ),
    )
}

/// A PRESENT optional string argument. Absent stays `None`; present as any
/// non-string (`5`, `null`, `false`, an object) is a typed refusal, never a
/// silent absence: a caller that names a context or a page must have it read
/// or refused, not quietly answered from another page. An empty string is
/// refused too, because it names nothing.
pub(crate) fn optional_string_arg(
    tool: &str,
    args: &Value,
    key: &str,
) -> Result<Option<String>, ToolErr> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Err(ToolErr::typed(
            format!("{tool}: '{key}' must be a non-empty string when given"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": key}),
        )),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(ToolErr::typed(
            format!("{tool}: '{key}' must be a string when given, got {other}"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": key}),
        )),
    }
}

pub async fn call(name: &str, args: &Value, browser: &Browser) -> Value {
    let r: Result<Value, ToolErr> = match name {
        "navigate" => {
            if let Err(e) = check_unknown("navigate", args, &["url"]) {
                return e.envelope();
            }
            let url = match args.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => return ToolErr::typed(
                    "navigate: missing required arg 'url'".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "navigate"}),
                )
                .envelope(),
            };
            tool_navigate(browser, &url).await
        }
        "snapshot" => {
            if let Err(e) = check_unknown(
                "snapshot",
                args,
                &["mode", "frame", "offset", "snapshotId", "context", "page"],
            ) {
                return e.envelope();
            }
            let offset = match args.get("offset") {
                None => 0usize,
                Some(v) => match v.as_u64() {
                    Some(n) => n as usize,
                    None => {
                        return ToolErr::typed(
                            "snapshot: 'offset' must be an integer >= 0".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "offset"}),
                        )
                        .envelope()
                    }
                },
            };
            // A present-but-wrong-typed target is refused BEFORE any browser
            // read: `and_then(as_str)` would silently turn it into "absent"
            // and answer from another page.
            let context = match optional_string_arg("snapshot", args, "context") {
                Ok(v) => v,
                Err(e) => return e.envelope(),
            };
            let page = match optional_string_arg("snapshot", args, "page") {
                Ok(v) => v,
                Err(e) => return e.envelope(),
            };
            tool_snapshot(
                browser,
                args.get("mode").and_then(|v| v.as_str()),
                args.get("frame").and_then(|v| v.as_str()),
                offset,
                args.get("snapshotId").and_then(|v| v.as_str()),
                context.as_deref(),
                page.as_deref(),
            )
            .await
        }
        "screenshot" => {
            if let Err(e) = check_unknown(
                "screenshot",
                args,
                &[
                    "fullPage",
                    "path",
                    "deterministic",
                    "width",
                    "height",
                    "element",
                    "selector",
                    "clip",
                    "format",
                    "quality",
                    "freeze",
                    "waitForStable",
                    "force",
                    "scale",
                ],
            ) {
                return e.envelope();
            }
            let freeze = args.get("freeze").and_then(|v| v.as_bool()).unwrap_or(false);
            let wait_for_stable = args.get("waitForStable").and_then(|v| v.as_u64());
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let scale = match args.get("scale") {
                None => 1.0,
                Some(v) => match v.as_f64() {
                    Some(s) if (0.1..=1.0).contains(&s) => s,
                    _ => {
                        return ToolErr::typed(
                            "screenshot: 'scale' must be a number 0.1-1".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "scale"}),
                        )
                        .envelope()
                    }
                },
            };
            let full = args
                .get("fullPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let path = args.get("path").and_then(|v| v.as_str());
            let deterministic = args
                .get("deterministic")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let width = args.get("width").and_then(|v| v.as_i64());
            let height = args.get("height").and_then(|v| v.as_i64());
            let element = match alias_target("screenshot", args) {
                Ok(target) => target,
                Err(e) => return e.envelope(),
            };
            let format = match args.get("format").and_then(|v| v.as_str()) {
                None | Some("png") => crate::capture::Format::Png,
                Some("jpeg") => crate::capture::Format::Jpeg,
                Some("webp") => crate::capture::Format::WebP,
                Some(other) => {
                    return ToolErr::typed(
                        format!("screenshot: unknown 'format' '{other}' (png, jpeg, webp)"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "format"}),
                    )
                    .envelope()
                }
            };
            let quality = match args.get("quality") {
                None => None,
                Some(v) => match v.as_u64() {
                    Some(q) if q <= 100 => Some(q as u8),
                    _ => {
                        return ToolErr::typed(
                            "screenshot: 'quality' must be an integer 0-100".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "quality"}),
                        )
                        .envelope()
                    }
                },
            };
            if quality.is_some() && !format.takes_quality() {
                return ToolErr::typed(
                    "screenshot: 'quality' applies to format jpeg/webp only (png is lossless)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "quality"}),
                )
                .envelope();
            }
            if !deterministic && (width.is_some() || height.is_some()) {
                return ToolErr::typed(
                        "screenshot: 'width'/'height' require deterministic:true (a plain capture uses the live viewport)".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "width"}),
                    )
                    .envelope();
            }
            if full && (element.is_some() || args.get("clip").is_some()) {
                return ToolErr::typed(
                    "screenshot: 'fullPage' cannot be combined with 'element'/'clip' (capture one or the other)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "fullPage"}),
                )
                .envelope();
            }
            if element.is_some() && args.get("clip").is_some() {
                return ToolErr::typed(
                    "screenshot: 'element' and 'clip' are two framings of one capture; pass one".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "clip"}),
                )
                .envelope();
            }
            let clip = match args.get("clip") {
                None => None,
                Some(v) => {
                    let pick = |k: &str| v.get(k).and_then(|x| x.as_f64());
                    match (pick("x"), pick("y"), pick("width"), pick("height")) {
                        (Some(x), Some(y), Some(w), Some(h)) if w > 0.0 && h > 0.0 => {
                            Some(crate::capture::Clip {
                                x,
                                y,
                                width: w,
                                height: h,
                            })
                        }
                        _ => {
                            return ToolErr::typed(
                                "screenshot: 'clip' needs finite x, y and positive width/height".to_string(),
                                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "clip"}),
                            )
                            .envelope()
                        }
                    }
                }
            };
            tool_screenshot(
                browser,
                full,
                path,
                deterministic,
                width,
                height,
                element.as_deref(),
                clip,
                format,
                quality,
                freeze,
                wait_for_stable,
                force,
                scale,
            )
            .await
        }
        "assert_layout" => {
            Box::pin(tool_assert_layout(browser, args)).await
        }
        "layout" => Box::pin(tool_layout(browser, args)).await,
        "visual" => Box::pin(tool_visual(browser, args)).await,
        "click" => {
            if let Err(e) = check_unknown(
                "click",
                args,
                &["element", "selector", "frame", "hoverFirst", "button", "clickCount"],
            ) {
                return e.envelope();
            }
            let el = match alias_target("click", args) {
                Ok(Some(target)) => target,
                Ok(None) => {
                    return missing_target("click", args, "click: missing required arg 'element'")
                        .envelope()
                }
                Err(e) => return e.envelope(),
            };
            let frame = args
                .get("frame")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let hover_first = args
                .get("hoverFirst")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let button = match args.get("button").and_then(|v| v.as_str()) {
                None | Some("left") => "left",
                Some("right") => "right",
                Some("middle") => "middle",
                Some(other) => {
                    return ToolErr::typed(
                        format!("click: unknown 'button' '{other}' (left, right, middle)"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "click", "arg": "button"}),
                    )
                    .envelope()
                }
            };
            let click_count = match args.get("clickCount") {
                None => 1i64,
                Some(v) => match v.as_i64() {
                    Some(n) if (1..=3).contains(&n) => n,
                    _ => {
                        return ToolErr::typed(
                            "click: 'clickCount' must be 1, 2 (double click) or 3".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "click", "arg": "clickCount"}),
                        )
                        .envelope()
                    }
                },
            };
            let act = Box::pin(actionability_wait(browser, &el, frame.as_deref())).await;
            if act["verdict"] == "timeout" {
                return ToolErr::typed(
                    format!("click: target not actionable within {ACTIONABILITY_BOUND_MS}ms"),
                    json!({"code": ToolCode::Timeout.as_str(), "reason": ToolReason::BudgetExceeded.as_str(),
                           "tool": "click", "actionability": act}),
                )
                .envelope();
            }
            tool_click(
                browser,
                &el,
                frame.as_deref(),
                hover_first,
                button,
                click_count,
            )
            .await
            .map(|v| with_actionability(v, &act))
        }
        "wheel" => {
            if let Err(e) = check_unknown("wheel", args, &["deltaY", "deltaX", "x", "y"]) {
                return e.envelope();
            }
            let delta_y = match args.get("deltaY").and_then(|v| v.as_f64()) {
                Some(d) => d,
                None => {
                    return ToolErr::typed(
                        "wheel: missing required numeric arg 'deltaY'".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "wheel"}),
                    )
                    .envelope()
                }
            };
            let delta_x = args
                .get("deltaX")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if !delta_x.is_finite() || !delta_y.is_finite() {
                return ToolErr::typed(
                    "wheel: 'deltaX'/'deltaY' must be finite numbers".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "wheel", "arg": "deltaY"}),
                )
                .envelope();
            }
            let point = match (args.get("x"), args.get("y")) {
                (None, None) => None,
                (Some(x), Some(y)) => match (x.as_f64(), y.as_f64()) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => {
                        return ToolErr::typed(
                            "wheel: 'x'/'y' must be numbers when given".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "wheel", "arg": "x"}),
                        )
                        .envelope()
                    }
                },
                _ => {
                    return ToolErr::typed(
                        "wheel: pass both 'x' and 'y', or neither (the viewport centre)"
                            .to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "wheel", "arg": "y"}),
                    )
                    .envelope()
                }
            };
            tool_wheel(browser, delta_x, delta_y, point).await
        }
        "drag" => {
            if let Err(e) = check_unknown(
                "drag",
                args,
                &["from", "to", "fromPoint", "toPoint", "steps", "frame", "dataTransfer"],
            ) {
                return e.envelope();
            }
            let steps = match args.get("steps") {
                None => 8usize,
                Some(v) => match v.as_u64() {
                    Some(n) if (2..=64).contains(&n) => n as usize,
                    _ => {
                        return ToolErr::typed(
                            "drag: 'steps' must be an integer 2-64".to_string(),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": "steps"}),
                        )
                        .envelope()
                    }
                },
            };
            let point = |key: &str| -> Result<Option<(f64, f64)>, ToolErr> {
                match args.get(key) {
                    None => Ok(None),
                    Some(v) => match (
                        v.get("x").and_then(|x| x.as_f64()),
                        v.get("y").and_then(|y| y.as_f64()),
                    ) {
                        (Some(x), Some(y)) => Ok(Some((x, y))),
                        _ => Err(ToolErr::typed(
                            format!("drag: '{key}' needs numeric x and y"),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": key}),
                        )),
                    },
                }
            };
            let from_point = match point("fromPoint") {
                Ok(p) => p,
                Err(e) => return e.envelope(),
            };
            let to_point = match point("toPoint") {
                Ok(p) => p,
                Err(e) => return e.envelope(),
            };
            let from = args.get("from").and_then(|v| v.as_str());
            let to = args.get("to").and_then(|v| v.as_str());
            let frame = args.get("frame").and_then(|v| v.as_str());
            let data_transfer = match args.get("dataTransfer") {
                None => None,
                Some(v @ Value::Object(_)) => Some(v.clone()),
                Some(_) => {
                    return ToolErr::typed(
                        "drag: 'dataTransfer' must be an object of mime type -> string".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": "dataTransfer"}),
                    )
                    .envelope()
                }
            };
            let endpoint = |name: &str, el: Option<&str>, pt: Option<(f64, f64)>| {
                match (el, pt) {
                    (Some(_), Some(_)) => Err(ToolErr::typed(
                        format!("drag: '{name}' and '{name}Point' are two addresses for one endpoint; pass one"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": name}),
                    )),
                    (None, None) => Err(ToolErr::typed(
                        format!("drag: missing '{name}' (an element) or '{name}Point' (x/y)"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "drag", "arg": name}),
                    )),
                    _ => Ok(()),
                }
            };
            if let Err(e) = endpoint("from", from, from_point) {
                return e.envelope();
            }
            if let Err(e) = endpoint("to", to, to_point) {
                return e.envelope();
            }
            tool_drag(
                browser,
                from,
                to,
                from_point,
                to_point,
                steps,
                frame,
                data_transfer.as_ref(),
            )
            .await
        }
        "form" => Box::pin(tool_form(browser, args)).await,
        "run" => Box::pin(tool_run(browser, args)).await,
        "type" => {
            if let Err(e) = check_unknown("type", args, &["element", "selector", "text", "submit", "frame"]) {
                return e.envelope();
            }
            const TYPE_MISSING: &str = "type: missing required args 'element'/'text'";
            let el = match alias_target("type", args) {
                Ok(Some(target)) => target,
                Ok(None) => return missing_target("type", args, TYPE_MISSING).envelope(),
                Err(e) => return e.envelope(),
            };
            let text = match args.get("text").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return missing_target("type", args, TYPE_MISSING).envelope(),
            };
            let submit = args
                .get("submit")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let frame = args
                .get("frame")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let act = Box::pin(actionability_wait(browser, &el, frame.as_deref())).await;
            if act["verdict"] == "timeout" {
                return ToolErr::typed(
                    format!("type: target not actionable within {ACTIONABILITY_BOUND_MS}ms"),
                    json!({"code": ToolCode::Timeout.as_str(), "reason": ToolReason::BudgetExceeded.as_str(),
                           "tool": "type", "actionability": act}),
                )
                .envelope();
            }
            tool_type(browser, &el, &text, submit, frame.as_deref())
                .await
                .map(|v| with_actionability(v, &act))
        }
        "evaluate" => {
            if let Err(e) = check_unknown("evaluate", args, &["expression", "target", "frame"]) {
                return e.envelope();
            }
            let expr = match args.get("expression").and_then(|v| v.as_str()) {
                Some(e) => e.to_string(),
                None => return ToolErr::typed(
                    "evaluate: missing required arg 'expression'".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "evaluate"}),
                )
                .envelope(),
            };
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let frame = args
                .get("frame")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            tool_evaluate(browser, &expr, target.as_deref(), frame.as_deref()).await
        }
        "wait_for" => {
            if let Err(e) = check_unknown(
                "wait_for",
                args,
                &[
                    "selector",
                    "text",
                    "ms",
                    "networkIdle",
                    "idleMs",
                    "expression",
                ],
            ) {
                return e.envelope();
            }
            let sel = args
                .get("selector")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let ms = args.get("ms").and_then(|v| v.as_i64());
            let net_idle = args
                .get("networkIdle")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let idle_ms = args.get("idleMs").and_then(|v| v.as_i64());
            let expr = args
                .get("expression")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            tool_wait_for(
                browser,
                sel.as_deref(),
                text.as_deref(),
                ms,
                net_idle,
                idle_ms,
                expr.as_deref(),
            )
            .await
        }
        "console" => {
            if let Err(e) = check_unknown("console", args, &["action", "limit"]) {
                return e.envelope();
            }
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("get")
                .to_string();
            let limit = args.get("limit").and_then(|v| v.as_i64());
            tool_console(browser, &action, limit).await
        }
        "upload" => {
            if let Err(e) = check_unknown("upload", args, &["element", "selector", "paths", "frame"]) {
                return e.envelope();
            }
            let el = match alias_target("upload", args) {
                Ok(Some(target)) => target,
                Ok(None) => {
                    return missing_target("upload", args, "upload: missing required arg 'element'")
                        .envelope()
                }
                Err(e) => return e.envelope(),
            };
            let paths: Option<Vec<String>> =
                args.get("paths").and_then(|v| v.as_array()).and_then(|a| {
                    let mut out = Vec::with_capacity(a.len());
                    for x in a {
                        out.push(x.as_str()?.to_string());
                    }
                    Some(out)
                });
            let paths =
                match paths {
                    Some(p) => p,
                    None => return ToolErr::typed(
                        "upload: missing required arg 'paths' (array of strings)".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "upload"}),
                    )
                    .envelope(),
                };
            tool_upload(
                browser,
                &el,
                &paths,
                args.get("frame").and_then(|v| v.as_str()),
            )
            .await
        }
        "fetch_text" => {
            if let Err(e) = check_unknown("fetch_text", args, &["url"]) {
                return e.envelope();
            }
            let url = match args.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => return ToolErr::typed(
                    "fetch_text: missing required arg 'url'".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "fetch_text"}),
                )
                .envelope(),
            };
            match fetch_text(&url).await {
                Ok(t) => {
                    let chars = t.chars().count() as i64;
                    Ok(json!({"content": [{"type": "text", "text": t.clone()}],
                              "structuredContent": {"text": t, "url": url, "chars": chars}}))
                }
                Err(e) => Err(ToolErr::fallback(e)),
            }
        }
        "hover" => {
            if let Err(e) = check_unknown("hover", args, &["element", "selector", "frame"]) {
                return e.envelope();
            }
            let el = match alias_target("hover", args) {
                Ok(Some(target)) => target,
                Ok(None) => {
                    return missing_target("hover", args, "hover: missing required arg 'element'")
                        .envelope()
                }
                Err(e) => return e.envelope(),
            };
            tool_hover(browser, &el, args.get("frame").and_then(|v| v.as_str())).await
        }
        "select" => {
            if let Err(e) = check_unknown("select", args, &["element", "selector", "value", "text", "frame"]) {
                return e.envelope();
            }
            let el = match alias_target("select", args) {
                Ok(Some(target)) => target,
                Ok(None) => {
                    return missing_target("select", args, "select: missing required arg 'element'")
                        .envelope()
                }
                Err(e) => return e.envelope(),
            };
            let value = args
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let text = args
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let frame = args.get("frame").and_then(|v| v.as_str());
            let act = Box::pin(actionability_wait(browser, &el, frame)).await;
            if act["verdict"] == "timeout" {
                return ToolErr::typed(
                    format!("select: target not actionable within {ACTIONABILITY_BOUND_MS}ms"),
                    json!({"code": ToolCode::Timeout.as_str(), "reason": ToolReason::BudgetExceeded.as_str(),
                           "tool": "select", "actionability": act}),
                )
                .envelope();
            }
            tool_select(browser, &el, value.as_deref(), text.as_deref(), frame)
                .await
                .map(|v| with_actionability(v, &act))
        }
        "key" => {
            if let Err(e) = check_unknown("key", args, &["key"]) {
                return e.envelope();
            }
            let key = match args.get("key").and_then(|v| v.as_str()) {
                Some(k) => k.to_string(),
                None => {
                    return ToolErr::typed(
                        "key: missing required arg 'key'".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "key"}),
                    )
                    .envelope()
                }
            };
            tool_key(browser, &key).await
        }
        "viewport" => tool_viewport(browser, args).await,
        "tabs" => {
            if let Err(e) = check_unknown("tabs", args, &["action", "url", "index"]) {
                return e.envelope();
            }
            let action =
                match args.get("action").and_then(|v| v.as_str()) {
                    Some(a) => a.to_string(),
                    None => return ToolErr::typed(
                        "tabs: missing required arg 'action'".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "tabs"}),
                    )
                    .envelope(),
                };
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let index = args.get("index").and_then(|v| v.as_i64());
            tool_tabs(browser, &action, url.as_deref(), index).await
        }
        "storage" => {
            if let Err(e) = check_unknown(
                "storage",
                args,
                &[
                    "action",
                    "scope",
                    "cookies",
                    "items",
                    "path",
                    "allSites",
                    "redact",
                    "overwrite",
                    "force",
                    "confirm",
                ],
            ) {
                return e.envelope();
            }
            tool_storage(browser, args).await
        }
        "close" => {
            if let Err(e) = check_unknown("close", args, &[]) {
                return e.envelope();
            }
            match browser.close().await {
                Ok(()) => Ok(ok_text(json!({"ok": true}).to_string())),
                Err(e) => Err(ToolErr::fallback(e)),
            }
        }
        "targets" => {
            if let Err(e) = check_unknown("targets", args, &["action", "target"]) {
                return e.envelope();
            }
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("list")
                .to_string();
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            tool_targets(browser, &action, target.as_deref()).await
        }
        "dialog" => {
            if let Err(e) = check_unknown("dialog", args, &["action", "accept", "promptText"]) {
                return e.envelope();
            }
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("get")
                .to_string();
            tool_dialog(
                browser,
                &action,
                args.get("accept").and_then(|v| v.as_bool()),
                args.get("promptText").and_then(|v| v.as_str()),
            )
            .await
        }
        "history" => {
            if let Err(e) = check_unknown("history", args, &["action"]) {
                return e.envelope();
            }
            let action =
                match args.get("action").and_then(|v| v.as_str()) {
                    Some(a) => a.to_string(),
                    None => return ToolErr::typed(
                        "history: missing required arg 'action'".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "history"}),
                    )
                    .envelope(),
                };
            tool_history(browser, &action).await
        }
        "network" => {
            let action = args
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("get");
            // B3 interception actions belong to the private net module. Route
            // them before the ring's arg check so their own args pass through.
            if matches!(action, "route" | "mock" | "abort" | "offline" | "restore") {
                crate::net::handle(browser, args).await
            } else {
                if let Err(e) = check_unknown("network", args, &["action", "filter", "limit"]) {
                    return e.envelope();
                }
                let filter = args
                    .get("filter")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let limit = args.get("limit").and_then(|v| v.as_i64());
                tool_network(browser, action, filter.as_deref(), limit).await
            }
        }
        "cdp" => {
            if let Err(e) = check_unknown("cdp", args, &["method", "params", "session"]) {
                return e.envelope();
            }
            let method = match args.get("method").and_then(|v| v.as_str()) {
                Some(m) => m.to_string(),
                None => {
                    return ToolErr::typed(
                        "cdp: missing required arg 'method'".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "cdp"}),
                    )
                    .envelope()
                }
            };
            let params = args.get("params").cloned().unwrap_or(json!({}));
            if !params.is_object() {
                return ToolErr::typed(
                    "cdp: 'params' must be an object".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "cdp"}),
                )
                .envelope();
            }
            let session = args
                .get("session")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            tool_cdp(browser, &method, params, session).await
        }
        "assert" => crate::assert::handle(browser, args).await,
        "audit" => crate::audit::handle(browser, args).await,
        "trace" => crate::trace::handle(browser, args).await,
        "download" => crate::download::handle(browser, args).await,
        other => {
            return ToolErr::typed(
                format!("unknown tool: {other}"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownTool.as_str(), "name": other}),
            )
            .envelope()
        }
    };
    match r {
        Ok(v) => v,
        Err(e) => e.envelope(),
    }
}

/// A requested http(s) page that never loaded is a typed failure, never an
/// `ok:true` with a warning an agent can ignore. Explicit `about:blank`,
/// `data:` and `file:` targets are not requested pages and keep working.
fn blank_load_err(url: &str, detail: &str) -> ToolErr {
    ToolErr::typed(
        format!("navigate: {detail}"),
        json!({"code": ToolCode::BlankLoad.as_str(),
               "reason": ToolReason::Navigated.as_str(), "url": url}),
    )
}

async fn tool_navigate(browser: &Browser, url: &str) -> Result<Value, ToolErr> {
    let lower = url.trim().to_ascii_lowercase();
    let requested_page = lower.starts_with("http://") || lower.starts_with("https://");
    browser.command("Page.enable", json!({}), true).await?;
    let _ = browser.ensure_network().await;
    browser.begin_nav_observe().await;
    let nav = browser
        .command("Page.navigate", json!({"url": url}), true)
        .await?;
    if let Some(err) = nav.get("errorText").and_then(|v| v.as_str()) {
        if !err.is_empty() {
            if requested_page {
                return Err(blank_load_err(url, &format!("net error loading {url}: {err}")));
            }
            return Err(ToolErr::fallback(format!("navigate: {err}")));
        }
    }
    browser
        .wait_for_load(std::time::Duration::from_secs(10))
        .await;
    let observed = browser.nav_observed().await;
    let title = browser
        .eval("document.title")
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    let cur = browser
        .eval("window.location.href")
        .await?
        .as_str()
        .unwrap_or(url)
        .to_string();
    if requested_page && cur == "about:blank" {
        return Err(blank_load_err(
            url,
            &format!("requested {url} but the document stayed on about:blank"),
        ));
    }
    let titled = !title.is_empty();
    let on_blank = cur.is_empty() || cur == "about:blank";
    let mut reply = json!({"url": cur, "title": title, "observed": observed});
    if let Ok((w, h, _)) = browser.viewport_readback().await {
        reply["viewport"] = json!({"width": w, "height": h});
    }
    if titled || on_blank {
        return Ok(ok_text(reply.to_string()));
    }
    let body_len = browser
        .eval("(((document.body && document.body.innerText) || '') + '').length")
        .await
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(-1.0);
    if body_len == 0.0 {
        if requested_page {
            return Err(blank_load_err(
                url,
                &format!("requested {url} but the document is empty"),
            ));
        }
        reply["warning"] = json!("possible blank-load race: committed URL with empty document (empty title and empty body text); re-navigate to confirm");
    }
    Ok(ok_text(reply.to_string()))
}

/// Parse an AX ref arg (`@e3`) to its table id (`e3`). Selectors pass through
/// as None so existing selector/text targeting keeps working.
fn parse_ref(s: &str) -> Option<String> {
    let t = s.trim();
    let rest = t.strip_prefix('@')?;
    let id = rest.trim();
    let num = id.strip_prefix('e')?;
    if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
        Some(id.to_string())
    } else {
        None
    }
}

/// The four additive selector forms, plus `role` with an exact name. Every
/// one of them is uniqueness-mandatory: the resolver never falls back to the
/// first match. Parsed from the same `element` string as CSS and `@eN`, so no
/// tool gains a new argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AxKind {
    /// `role=<role>`
    Role,
    /// `role=<role>[name="<accessible name>"]`
    RoleName,
    /// `name="<accessible name>"`
    Name,
    /// `text="<text>"` - exact visible text within one element.
    Text,
    /// `label="<label text>"` - form control by its associated label.
    Label,
}

struct AxSel {
    kind: AxKind,
    role: String,
    want: String,
}

/// Every accepted spelling, for the error a malformed selector earns.
const AX_FORMS: &str = "role=<role>, role=<role>[name=\"<accessible name>\"], \
                        name=\"<accessible name>\", text=\"<text>\" or label=\"<label text>\"; \
                        values are double-quoted and an inner double quote is escaped as \\\"";

/// Collapse whitespace runs to one space and trim: the normalisation both the
/// snapshot's names and these selectors use, so a name that renders on one
/// line matches whether the DOM broke it across ten.
fn ax_norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Name/text comparison: whitespace-normalised, case-insensitive. Uniqueness
/// is enforced before anything acts, so a looser comparison can only turn a
/// silent wrong pick into a loud `ambiguous` candidate list - never the
/// other way round.
fn ax_eq(a: &str, b: &str) -> bool {
    ax_norm(a).to_lowercase() == ax_norm(b).to_lowercase()
}

/// The snapshot's 80-character cap, applied to a normalized name.
fn ax_cap(s: &str) -> String {
    ax_norm(s).chars().take(80).collect()
}

/// The name a snapshot PRINTS for one node, shared by the renderer and the
/// resolver so a name an agent copies out of a snapshot is a name the resolver
/// accepts.
///
/// That name is the accessible name, whitespace-collapsed and capped at 80
/// characters; when that is empty on a form control it is the fallback chain
/// the legacy heuristic has always used - selected option text, placeholder,
/// full value unless sensitive, id, name attribute, type. `info` is the live
/// enrichment (`enrich_objects`) for the node: without it only the accessible
/// half is knowable, which is exactly the difference between the printed name
/// and the raw AX name.
fn snapshot_name(ax_name: &str, tag: &str, sens: bool, info: Option<&Value>) -> String {
    let printed = ax_cap(ax_name);
    let Some(info) = info else {
        return printed;
    };
    if !printed.is_empty() || !matches!(tag, "input" | "textarea" | "select") {
        return printed;
    }
    let s = |k: &str| {
        info.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let (full_val, ph, eid, enm, sel_text, ty) = (
        s("fullVal"),
        s("ph"),
        s("eid"),
        s("enm"),
        s("selText"),
        s("ty"),
    );
    let mut fb = String::new();
    if tag == "select" {
        if !sel_text.is_empty() {
            fb = sel_text;
        } else if !eid.is_empty() {
            fb = eid;
        } else if !enm.is_empty() {
            fb = enm;
        }
    } else if !ph.is_empty() {
        fb = ph;
    } else if !sens && !full_val.is_empty() {
        fb = full_val;
    } else if !eid.is_empty() {
        fb = eid;
    } else if !enm.is_empty() {
        fb = enm;
    } else if tag == "input" && !ty.is_empty() {
        fb = ty;
    }
    ax_cap(&fb)
}

/// A double-quoted selector value, with `\"` and `\\` escapes. Anything else
/// is None, so `name=Save` (unquoted) can never quietly become a CSS parse.
fn ax_unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return None,
            '\\' => match it.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// Parse the additive AX selector forms. `Ok(None)` means a legacy string
/// (CSS selector or `@eN`) and is returned for every string that does not
/// start with one of the form keywords, so legacy behaviour is untouched: a
/// `<label>` element selector or `[name="x"]` never reaches this branch, and
/// every keyword form (`role=...`) is already invalid CSS, so classifying it
/// here cannot change what used to work.
fn ax_sel_parse(tool: &str, raw: &str) -> Result<Option<AxSel>, ToolErr> {
    let t = raw.trim();
    let bad = |why: &str| -> ToolErr {
        ToolErr::typed(
            format!("{tool}: malformed selector {raw:?} ({why}); want {AX_FORMS}"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(),
                   "tool": tool, "arg": "element", "element": raw, "forms": AX_FORMS}),
        )
    };
    let mut found: Option<(&str, AxKind)> = None;
    for (kw, kind) in [
        ("role=", AxKind::Role),
        ("name=", AxKind::Name),
        ("text=", AxKind::Text),
        ("label=", AxKind::Label),
    ] {
        if t.get(..kw.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(kw))
        {
            found = Some((&t[kw.len()..], kind));
            break;
        }
    }
    let Some((rest, kind)) = found else {
        let near = ["role", "name", "text", "label"].iter().any(|kw| {
            t.get(..kw.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(kw))
                && t[kw.len()..].trim_start().starts_with('=')
        });
        if near {
            return Err(bad(
                "no space is allowed before '=' and the keyword is written exactly as shown",
            ));
        }
        return Ok(None);
    };
    let (role, want, kind) = match kind {
        AxKind::Role | AxKind::RoleName => {
            let (role_txt, suffix) = match rest.find('[') {
                Some(i) => (&rest[..i], Some(&rest[i..])),
                None => (rest, None),
            };
            let role_txt = role_txt.trim().to_lowercase();
            if role_txt.is_empty()
                || role_txt
                    .chars()
                    .any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '"'))
            {
                return Err(bad("the role must be a bare ARIA role token"));
            }
            match suffix {
                None => (role_txt, String::new(), AxKind::Role),
                Some(s) => {
                    let inner = s
                        .strip_prefix('[')
                        .and_then(|x| x.strip_suffix(']'))
                        .ok_or_else(|| bad("the name filter must close with ]"))?;
                    let (key, val) = inner
                        .split_once('=')
                        .ok_or_else(|| bad("the name filter must be [name=\"...\"]"))?;
                    if !key.trim().eq_ignore_ascii_case("name") {
                        return Err(bad("the only filter is [name=\"...\"]"));
                    }
                    let val = ax_unquote(val.trim())
                        .ok_or_else(|| bad("the filtered name must be double-quoted"))?;
                    if ax_norm(&val).is_empty() {
                        return Err(bad("the filtered name is empty"));
                    }
                    (role_txt, ax_norm(&val), AxKind::RoleName)
                }
            }
        }
        AxKind::Name | AxKind::Text | AxKind::Label => {
            let val = ax_unquote(rest.trim()).ok_or_else(|| {
                bad("the value must be a double-quoted string: single quotes and bare values are not accepted, and an inner double quote is written \\\" (escaped with a backslash)")
            })?;
            if ax_norm(&val).is_empty() {
                return Err(bad("the value is empty"));
            }
            (String::new(), ax_norm(&val), kind)
        }
    };
    Ok(Some(AxSel { kind, role, want }))
}

/// One AX node, reduced to what the selector forms read.
struct AxNode {
    parent: String,
    role: String,
    name: String,
    backend: Option<i64>,
    ignored: bool,
    /// The accessible name came from a `<label>` association (`labelfor` /
    /// `labelwrapped`), not from aria-label, contents or placeholder.
    from_label: bool,
}

/// One resolved element: the exact AX node, its role/name, and the `@eN` the
/// current snapshot minted for it when there is one.
struct AxHit {
    backend: i64,
    role: String,
    name: String,
    at: Option<String>,
    /// The child session that owns `backend` when the frame is a mapped OOPIF;
    /// every object read and call for this hit must run there.
    session: Option<String>,
}

impl AxHit {
    /// Error context for a resolved (or resolved-then-failed) selector.
    fn ctx(&self, tool: &str, element: &str) -> Value {
        json!({"tool": tool, "element": element, "ref": self.at, "role": self.role, "name": self.name})
    }

    /// The `resolved` field every AX-selector reply carries, so the caller
    /// can see exactly which element was chosen.
    fn resolved(&self, element: &str) -> Value {
        json!({"selector": element, "ref": self.at, "role": self.role, "name": self.name})
    }
}

/// Did the accessible name come from a `<label>` association? Provenance is a
/// non-superseded name source of `labelfor` (a `for=` label) or `labelwrapped`
/// (a wrapping label). The source's own value is NOT compared to the name:
/// measured, that value is the label's text, which differs from the computed
/// name whenever the label carries inline markup or hidden fragments - and a
/// source the engine lists as superseded (aria-label winning, say) never
/// supplied the name at all.
fn ax_name_from_label(node: &Value) -> bool {
    let Some(sources) = node.pointer("/name/sources").and_then(|s| s.as_array()) else {
        return false;
    };
    sources.iter().any(|s| {
        s.get("superseded").and_then(|v| v.as_bool()) != Some(true)
            && matches!(
                s.get("nativeSource").and_then(|v| v.as_str()),
                Some("labelfor") | Some("labelwrapped")
            )
    })
}

/// The accessibility tree for `frame`, and the child session that owns it when
/// the frame is a mapped OOPIF.
///
/// Policy, in order:
/// 1. The page session answers (`ax_tree`): a same-process frame or the page
///    itself. No session.
/// 2. The page session refuses and the frame has a mapped OOPIF session: the
///    tree is read from THAT session, and every backend id in it is resolved
///    there too (a backend id belongs to the renderer that issued it).
/// 3. The page session refuses and no session owns the frame: typed
///    `invalid_target`/`not_found` for an unknown frame, typed
///    `invalid_target`/`unreachable` for a detached or unmapped out-of-process
///    frame. The page's own tree is NEVER substituted for the frame's.
async fn frame_ax_tree(
    browser: &Browser,
    tool: &str,
    fid: Option<&str>,
) -> Result<(Value, Option<String>), ToolErr> {
    match browser.ax_tree(fid).await {
        Ok(t) => Ok((t, None)),
        Err(e) => {
            let l = e.to_lowercase();
            if l.contains("no frame with id")
                || l.contains("no such frame")
                || l.contains("cannot find")
            {
                return Err(frame_lookup_err(tool, fid.unwrap_or(""), e));
            }
            let Some(f) = fid else {
                return Err(ToolErr::from(e));
            };
            if let Some((target, sid)) = browser.oop_session_for_frame(f).await {
                return match browser.ax_tree_in_session(&sid, None).await {
                    Ok(t) => Ok((t, Some(sid))),
                    Err(e2) => Err(ToolErr::typed(
                        format!("{tool}: frame '{f}' is out-of-process (target {target}) and has no AX tree readable from this session ({e2}); nothing was resolved and the page was not searched instead"),
                        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::Unreachable.as_str(), "tool": tool, "frame": f, "target": target}),
                    )),
                };
            }
            Err(ToolErr::typed(
                format!("{tool}: frame '{f}' has no AX tree readable from this session ({e}); nothing was resolved and the page was not searched instead - the frame is detached or out of process with no mapped session, and retrying this call cannot help; re-read targets for a live frameId, or address an element on the page"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::Unreachable.as_str(), "tool": tool, "frame": f}),
            ))
        }
    }
}

/// Resolve one AX selector form against the accessibility tree the snapshot
/// renders (`Accessibility.getFullAXTree`), scoped to `frame` when the caller
/// named one. Zero matches and more than one match are both typed refusals:
/// the caller gets the selector back, and for ambiguity every candidate as
/// `@eN` + role + name, so it can pick one deliberately or narrow the form.
///
/// The resolved element is acted on by its backend node id, the same identity
/// a snapshot ref records, so a successful selector is the element the
/// accessibility tree reports - never a DOM guess.
async fn ax_resolve(
    browser: &Browser,
    tool: &str,
    sel: &AxSel,
    raw: &str,
    fid: Option<&str>,
) -> Result<AxHit, ToolErr> {
    let (tree, session) = frame_ax_tree(browser, tool, fid).await?;
    let empty = Vec::new();
    let raw_nodes = tree
        .get("nodes")
        .and_then(|n| n.as_array())
        .unwrap_or(&empty);
    let mut nodes: Vec<AxNode> = Vec::with_capacity(raw_nodes.len());
    let mut index: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(raw_nodes.len());
    for n in raw_nodes.iter() {
        let role = n
            .pointer("/role/value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let name = ax_norm(
            n.pointer("/name/value")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        if let Some(id) = n.get("nodeId").and_then(|v| v.as_str()) {
            index.insert(id.to_string(), nodes.len());
        }
        nodes.push(AxNode {
            parent: n
                .get("parentId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ignored: n.get("ignored").and_then(|v| v.as_bool()) == Some(true),
            from_label: ax_name_from_label(n),
            backend: n.get("backendDOMNodeId").and_then(|v| v.as_i64()),
            role,
            name,
        });
    }
    let mut text_of: Vec<String> = vec![String::new(); nodes.len()];
    for n in nodes.iter() {
        if n.role != "statictext" || n.name.is_empty() || n.ignored {
            continue;
        }
        let mut cur = n.parent.as_str();
        for _ in 0..64 {
            let Some(&p) = index.get(cur) else { break };
            text_of[p].push_str(&n.name);
            cur = nodes[p].parent.as_str();
        }
    }
    let mut hits: Vec<usize> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.ignored || n.backend.is_none() || n.role == "statictext" || n.role == "inlinetextbox" {
            continue;
        }
        let matched = match sel.kind {
            AxKind::Role => n.role == sel.role,
            AxKind::RoleName => {
                n.role == sel.role
                    && (ax_eq(&n.name, &sel.want) || ax_eq(&ax_cap(&n.name), &sel.want))
            }
            AxKind::Name => ax_eq(&n.name, &sel.want) || ax_eq(&ax_cap(&n.name), &sel.want),
            AxKind::Label => {
                n.from_label && (ax_eq(&n.name, &sel.want) || ax_eq(&ax_cap(&n.name), &sel.want))
            }
            AxKind::Text => ax_eq(&text_of[i], &sel.want),
        };
        if matched {
            hits.push(i);
        }
    }
    if sel.kind == AxKind::Text && hits.len() > 1 {
        let set: std::collections::HashSet<usize> = hits.iter().copied().collect();
        let mut outer: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &h in hits.iter() {
            let mut cur = nodes[h].parent.as_str();
            for _ in 0..64 {
                let Some(&p) = index.get(cur) else { break };
                if set.contains(&p) {
                    outer.insert(p);
                }
                cur = nodes[p].parent.as_str();
            }
        }
        hits.retain(|h| !outer.contains(h));
    }
    let mut synthesized: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    if hits.is_empty() && matches!(sel.kind, AxKind::Name | AxKind::RoleName) {
        for (i, printed) in ax_synthesized_name_hits(browser, &nodes, sel).await {
            hits.push(i);
            synthesized.insert(i, printed);
        }
    }
    let scope = match fid {
        Some(f) => format!(
            "the AX tree of frame '{f}' (that frame only; the page and its other frames are not searched)"
        ),
        None => "the page's own AX tree (content of iframes is not part of it - pass frame= to search inside a frame)".to_string(),
    };
    if hits.is_empty() {
        let about_role = match sel.kind {
            AxKind::Role | AxKind::RoleName => {
                "; the value is an ARIA role string (e.g. button, generic, StaticText, Iframe with a capital I), not a tag name or an element selector".to_string()
            }
            _ => String::new(),
        };
        return Err(ToolErr::typed(
            format!("{tool}: no element matches selector {raw} (searched {scope}){about_role}"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                   "tool": tool, "element": raw, "frame": fid}),
        ));
    }
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut cands: Vec<AxHit> = Vec::with_capacity(hits.len());
    for i in hits.iter().copied() {
        let backend = nodes[i].backend.unwrap_or_default();
        if !seen.insert(backend) {
            continue;
        }
        let at = browser
            .ref_for_backend(backend)
            .await
            .map(|id| format!("@{id}"));
        cands.push(AxHit {
            backend,
            role: nodes[i].role.clone(),
            name: synthesized
                .get(&i)
                .cloned()
                .unwrap_or_else(|| ax_cap(&nodes[i].name)),
            at,
            session: session.clone(),
        });
    }
    if cands.len() > 1 {
        let listed: Vec<String> = cands
            .iter()
            .map(|c| match &c.at {
                Some(at) => format!("{at} {} \"{}\"", c.role, c.name),
                None => format!("(no ref) {} \"{}\"", c.role, c.name),
            })
            .collect();
        let missing = cands.iter().any(|c| c.at.is_none());
        let hint = if missing {
            match fid {
                Some(f) => format!("; snapshot with frame={f} to mint @eN for these candidates"),
                None => "; snapshot to mint @eN for these candidates".to_string(),
            }
        } else {
            String::new()
        };
        return Err(ToolErr::typed(
            format!(
                "{tool}: selector {raw} is ambiguous ({} elements match; act on one @eN instead): {}{hint}",
                cands.len(),
                listed.join(", ")
            ),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::Ambiguous.as_str(),
                   "tool": tool, "element": raw, "frame": fid,
                   "candidates": cands.iter().map(|c| json!({"ref": c.at, "role": c.role, "name": c.name})).collect::<Vec<_>>()}),
        ));
    }
    Ok(cands.remove(0))
}

/// The names a snapshot PRINTS that the AX tree does not carry: an unlabeled
/// form control falls back to its placeholder, value, id, name attribute or
/// type, and an agent copying that name must be able to use it. Runs only
/// after the accessible-name pass missed, resolves only the controls with an
/// empty AX name, and judges them with the same [`snapshot_name`] the renderer
/// uses, so the two cannot drift.
async fn ax_synthesized_name_hits(
    browser: &Browser,
    nodes: &[AxNode],
    sel: &AxSel,
) -> Vec<(usize, String)> {
    const CONTROL_ROLES: &[&str] = &[
        "textbox",
        "searchbox",
        "combobox",
        "listbox",
        "checkbox",
        "radio",
        "spinbutton",
        "slider",
        "button",
    ];
    let mut cands: Vec<usize> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if n.ignored
            || n.backend.is_none()
            || !n.name.is_empty()
            || !CONTROL_ROLES.contains(&n.role.as_str())
            || (sel.kind == AxKind::RoleName && n.role != sel.role)
        {
            continue;
        }
        cands.push(i);
        if cands.len() >= 200 {
            break;
        }
    }
    if cands.is_empty() {
        return Vec::new();
    }
    let mut oids: Vec<String> = Vec::with_capacity(cands.len());
    let mut keep: Vec<usize> = Vec::with_capacity(cands.len());
    for i in cands {
        let backend = nodes[i].backend.unwrap_or_default();
        match browser
            .command("DOM.resolveNode", json!({"backendNodeId": backend}), true)
            .await
        {
            Ok(v) => {
                if let Some(oid) = v
                    .get("object")
                    .and_then(|o| o.get("objectId"))
                    .and_then(|x| x.as_str())
                {
                    oids.push(oid.to_string());
                    keep.push(i);
                }
            }
            Err(_) => continue,
        }
    }
    let infos = enrich_objects(browser, &oids).await;
    keep.into_iter()
        .zip(infos)
        .filter(|(_, info)| info.is_object())
        .filter_map(|(i, info)| {
            let tag = info.get("tag").and_then(|v| v.as_str()).unwrap_or("");
            let sens = info.get("sens").and_then(|v| v.as_bool()).unwrap_or(false);
            let printed = snapshot_name(&nodes[i].name, tag, sens, Some(&info));
            (!printed.is_empty() && ax_eq(&printed, &sel.want)).then_some((i, printed))
        })
        .collect()
}

/// Resolve an AX node's backend id to a live JS object. The same identity a
/// snapshot ref records travels here, so a node the DOM no longer holds is a
/// typed miss, never a wrong-element action.
async fn ax_object(
    browser: &Browser,
    tool: &str,
    element: &str,
    hit: &AxHit,
) -> Result<String, ToolErr> {
    let gone = || {
        ToolErr::typed(
            format!("{tool}: element for selector {element} is gone (its AX node no longer resolves; re-snapshot and pick an @eN, or retry the selector)"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                   "tool": tool, "element": element, "role": hit.role, "name": hit.name}),
        )
    };
    match match hit.session.as_deref() {
        Some(sid) => browser.resolve_node_in_session(sid, hit.backend).await,
        None => {
            browser
                .command(
                    "DOM.resolveNode",
                    json!({"backendNodeId": hit.backend}),
                    true,
                )
                .await
        }
    } {
        Ok(v) => v
            .get("object")
            .and_then(|o| o.get("objectId"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(gone),
        Err(e) => {
            let l = e.to_lowercase();
            if l.contains("could not find node")
                || l.contains("no node with given id")
                || l.contains("does not belong to the document")
            {
                Err(gone())
            } else {
                Err(ToolErr::fallback(e))
            }
        }
    }
}

/// Map a preflight marker from an AX-resolved element to the same typed
/// failure the CSS and ref paths mint, so one condition reads the same
/// whichever way the element was named. `None` means "not a status this
/// helper knows": the caller keeps its own default arm.
fn ax_status_err(tool: &str, element: &str, hit: &AxHit, status: Option<&str>) -> Option<ToolErr> {
    let ctx = hit.ctx(tool, element);
    let typed = |code: ToolCode, reason: ToolReason, what: &str| {
        let mut sc = ctx.clone();
        if let Some(o) = sc.as_object_mut() {
            o.insert("code".to_string(), json!(code.as_str()));
            o.insert("reason".to_string(), json!(reason.as_str()));
        }
        Some(ToolErr::typed(
            format!("{tool}: element is {what}: {element}"),
            sc,
        ))
    };
    match status? {
        "GLADIUS_NOTFOUND" => Some(ToolErr::typed(
            format!(
                "{tool}: element for selector {element} is gone (the AX node no longer resolves)"
            ),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                   "tool": tool, "element": element, "role": hit.role, "name": hit.name}),
        )),
        "GLADIUS_DISABLED" => typed(ToolCode::NotActionable, ToolReason::Disabled, "disabled"),
        "GLADIUS_READONLY" => typed(ToolCode::NotActionable, ToolReason::Readonly, "read-only"),
        "GLADIUS_HIDDEN" => typed(ToolCode::NotActionable, ToolReason::Hidden, "hidden"),
        "GLADIUS_OCCLUDED" => typed(ToolCode::NotActionable, ToolReason::Occluded, "occluded"),
        "GLADIUS_NOTSELECT" => typed(
            ToolCode::InvalidTarget,
            ToolReason::NotSelect,
            "not a <select>",
        ),
        "GLADIUS_NOTFILE" => typed(
            ToolCode::InvalidTarget,
            ToolReason::NotFile,
            "not a file input",
        ),
        s if s.starts_with("GLADIUS_NOTEDIT:") => Some(type_not_editable(
            "type",
            &s["GLADIUS_NOTEDIT:".len()..],
            &ctx,
        )),
        _ => None,
    }
}

/// Add the `resolved` identity to an existing reply without losing or
/// reordering any field: the text block is re-rendered from the same object,
/// so prose and structuredContent still agree.
fn with_resolved(reply: Value, element: &str, hit: &AxHit) -> Value {
    let mut sc = reply
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(o) = sc.as_object_mut() {
        o.insert("resolved".to_string(), hit.resolved(element));
    }
    let text = sc.to_string();
    json!({"content": [{"type": "text", "text": text}], "structuredContent": sc})
}

/// Guard a ref-targeted action: unknown refs and post-navigation refs fail
/// with a TYPED stale-ref error naming the reason - never a silent
/// wrong-element click. Returns the ref id (`eN`) when it may proceed.
/// Shared by the action tools and the geometry tools so a ref means the same
/// thing everywhere.
pub(crate) async fn guard_ref(
    browser: &Browser,
    tool: &str,
    _element: &str,
    id: &str,
) -> Result<String, ToolErr> {
    if browser.ref_label(id).await.is_none() {
        return Err(ToolErr::typed(
            format!("stale ref: {tool}: unknown ref @{id} (no such ref; take a fresh snapshot)"),
            json!({"code": ToolCode::StaleRef.as_str(), "reason": ToolReason::Unknown.as_str(), "tool": tool, "ref": format!("@{id}")}),
        ));
    }
    let snap = browser.snap_url().await;
    if !snap.is_empty() {
        let cur = browser
            .eval("window.location.href")
            .await
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        if !cur.is_empty() && cur != snap {
            return Err(ToolErr::typed(
                format!("stale ref: {tool}: ref @{id} is stale (page navigated from {snap} to {cur}; take a fresh snapshot)"),
                json!({"code": ToolCode::StaleRef.as_str(), "reason": ToolReason::Navigated.as_str(), "tool": tool, "ref": format!("@{id}")}),
            ));
        }
    }
    Ok(format!("@{id}"))
}

pub(crate) fn stale_gone(tool: &str, element: &str, id: &str) -> ToolErr {
    let _ = element;
    ToolErr::typed(
        format!("stale ref: {tool}: ref @{id} is stale (node gone; take a fresh snapshot)"),
        json!({"code": ToolCode::StaleRef.as_str(), "reason": ToolReason::NodeGone.as_str(), "tool": tool, "ref": format!("@{id}")}),
    )
}

/// Resolve a ref's recorded backend node id to a live JS object.
/// `Ok(None)` means the ref came from the explicit legacy path (no backend
/// id recorded), so the caller keeps its selector resolver. A backend id the
/// DOM no longer knows is a typed node-gone, never a wrong-element action.
/// Shared by every ref consumer - the action tools and the geometry tools -
/// so one resolver decides what a ref means.
pub(crate) async fn resolve_ref_object(
    browser: &Browser,
    tool: &str,
    id: &str,
) -> Result<Option<String>, ToolErr> {
    let Some(backend) = browser.ref_backend(id).await else {
        return Ok(None);
    };
    let session = browser.ref_session(id).await;
    let resolved = match session.as_deref() {
        Some(sid) => browser.resolve_node_in_session(sid, backend).await,
        None => {
            browser
                .command("DOM.resolveNode", json!({"backendNodeId": backend}), true)
                .await
        }
    };
    match resolved {
        Ok(v) => match v
            .get("object")
            .and_then(|o| o.get("objectId"))
            .and_then(|x| x.as_str())
        {
            Some(oid) => Ok(Some(oid.to_string())),
            None => Err(stale_gone(tool, "", id)),
        },
        Err(e) => {
            let l = e.to_lowercase();
            if l.contains("could not find node")
                || l.contains("no node with given id")
                || l.contains("does not belong to the document")
            {
                Err(stale_gone(tool, "", id))
            } else {
                Err(ToolErr::fallback(e))
            }
        }
    }
}

/// Call `func` ON the resolved element (`this` is the element) and return the
/// by-value result. The same function body runs for light DOM and shadow DOM.
pub(crate) async fn call_on_object(
    browser: &Browser,
    object_id: &str,
    func: &str,
    args: Vec<Value>,
) -> Result<Value, ToolErr> {
    call_on_object_in(browser, None, object_id, func, args).await
}

/// `call_on_object` with an explicit child session: object ids are
/// session-scoped in CDP, so a node resolved out of an OOPIF must be called on
/// that session and never on the page one.
pub(crate) async fn call_on_object_in(
    browser: &Browser,
    session: Option<&str>,
    object_id: &str,
    func: &str,
    args: Vec<Value>,
) -> Result<Value, ToolErr> {
    let r = browser
        .call_function_on_in_session(
            session,
            json!({"objectId": object_id, "functionDeclaration": func,
                   "returnByValue": true, "awaitPromise": true, "arguments": args}),
        )
        .await
        .map_err(ToolErr::fallback)?;
    if let Some(ex) = r.get("exceptionDetails") {
        return Err(ToolErr::typed(
            format!("js exception: {ex}"),
            json!({"code": ToolCode::JsError.as_str(), "reason": ToolReason::Thrown.as_str()}),
        ));
    }
    match r.get("result").and_then(|x| x.get("value")) {
        Some(v) => Ok(v.clone()),
        None => Err(ToolErr::typed(
            "callFunctionOn: expression returned no value".to_string(),
            json!({"code": ToolCode::JsError.as_str(), "reason": ToolReason::UndefinedResult.as_str()}),
        )),
    }
}

/// Action-time read of an editable field's length ONLY (never its value):
/// the truthful `type` reply needs to know whether the field kept all of it.
const VALUE_LEN_FN: &str = r#"function() {
  const el = this;
  let n = null;
  try { if (typeof el.value === "string") n = el.value.length; else if (el.isContentEditable) n = (el.textContent || "").length; } catch (e) {}
  return n;
}"#;

/// Build the `type` reply. `valueLength` is reported only when the field was
/// readable; a differing length adds a note (never a failure - some fields
/// legitimately transform their input).
fn type_reply(typed: &str, value_length: Option<u64>) -> Value {
    let typed_len = typed.encode_utf16().count() as u64;
    let mut out = json!({"ok": true});
    if let Some(n) = value_length {
        out["valueLength"] = json!(n);
        if n != typed_len {
            out["note"] = json!(format!(
                "typed {typed_len} UTF-16 characters but the field now holds {n} (maxlength, pattern, or an input handler transformed it)"
            ));
        }
    }
    ok_text(out.to_string())
}

/// G4b frame policy: AX refs (`@eN`) are stamped against the MAIN document
/// resolver, so a ref scoped to a frame realm cannot resolve safely. Reject it
/// with a typed `invalid_args` (never a silent main-frame click); frame
/// actions take CSS selectors. Snapshot refs stay main-frame: a frame-scoped
/// snapshot stamps its own table, but acting through the main-document
/// resolver is unsupported, so the typed rejection stands either way.
fn ref_in_frame(tool: &str, element: &str, frame: &str) -> ToolErr {
    ToolErr::typed(
        format!("{tool}: ref '{element}' was minted by a snapshot of a different frame than '{frame}'; a ref is actionable only in the frame whose own snapshot minted it - take a snapshot with frame='{frame}' for its refs, or use a CSS or AX selector"),
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": "frame", "frame": frame, "element": element}),
    )
}

/// G4b: normalize the optional `frame` arg. Empty/blank is a typed
/// `invalid_args` (never the main frame by accident).
fn frame_id_ok(tool: &str, frame: Option<&str>) -> Result<Option<String>, ToolErr> {
    match frame {
        None => Ok(None),
        Some(f) if f.trim().is_empty() => Err(ToolErr::typed(
            format!("{tool}: 'frame' must be a non-empty CDP frameId (from targets)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": "frame"}),
        )),
        Some(f) => Ok(Some(f.to_string())),
    }
}

/// G4b: map a frame-realm engine failure to the site-known type. Unknown
/// frame ids (isolated-world creation refuses them) are typed
/// `invalid_target`/`not_found` with the frame; OOPIF/detached shapes keep
/// their own typed prose from the site; everything else is the generic
/// fallback.
fn frame_lookup_err(tool: &str, fid: &str, e: String) -> ToolErr {
    let l = e.to_lowercase();
    if l.contains("no frame with id") || l.contains("no such frame") || l.contains("cannot find") {
        ToolErr::typed(
            format!("{tool}: no frame with id '{fid}'"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": tool, "frame": fid}),
        )
    } else {
        ToolErr::fallback(e)
    }
}

/// G4b verifier: leaf-realm evaluation with OOPIF routing. When the frame
/// has a mapped OOPIF target session, the leaf JS (preflight, focus,
/// selection, rect reads) runs THERE (`eval_in_session`); otherwise it runs
/// in the frame's isolated world on the page session (`eval_in_frame`,
/// same-process). The owner chain, parent-side hit-test and dispatch always
/// stay on the page session, whose visual surface owns the coordinates.
/// Unknown frames are typed `invalid_target`/`not_found` - never a
/// main-frame evaluation.
pub(crate) async fn frame_realm_eval(
    browser: &Browser,
    tool: &str,
    fid: &str,
    expr: &str,
) -> Result<Value, ToolErr> {
    if let Some((_, sid)) = browser.oop_session_for_frame(fid).await {
        return browser
            .eval_in_session(&sid, expr)
            .await
            .map_err(ToolErr::from);
    }
    browser
        .eval_in_frame(fid, expr)
        .await
        .map_err(|e| frame_lookup_err(tool, fid, e))
}

/// G4b verifier: parent-side hit-test against the TRUE frame owners (not
/// just "any iframe"). Stamps every owner node in the chain with a temporary
/// attribute, then checks `document.elementFromPoint(x, y)` in the MAIN
/// realm: the hit itself or one of its ancestors must carry the stamp. A
/// covering parent overlay or a DIFFERENT iframe has no stamp and is typed
/// `not_actionable`/`occluded` - no dispatch happens. The stamp is removed
/// before returning on every path (snapshot refs already stamp nodes the
/// same way; a `data-*` attribute is page-inert).
/// G4b verifier: per-level parent-side hit-test against the TRUE owners.
/// A main-document-only check passes a stamped outer iframe even when an
/// overlay INSIDE it covers the requested inner iframe, so every ancestor
/// level is checked in its own realm: the point is mapped into that
/// parent's viewport (`frame_chain_points`), `elementFromPoint` runs there,
/// and the hit (or one of its ancestors) must carry the owner stamp for
/// that level. Each parent document holds exactly one stamped node (the
/// on-path child), so a covering overlay or a DIFFERENT iframe at ANY level
/// is typed `not_actionable`/`occluded` and nothing dispatches. Returns the
/// main-viewport point for dispatch once every level passes.
async fn frame_owner_hit_ok(
    browser: &Browser,
    tool: &str,
    element: &str,
    fid: &str,
    lx: f64,
    ly: f64,
) -> Result<(f64, f64), ToolErr> {
    fn occluded(tool: &str, element: &str, fid: &str) -> ToolErr {
        ToolErr::typed(
            format!(
                "{tool}: element is occluded by another element (no {tool} dispatched): {element}"
            ),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": tool, "element": element, "frame": fid}),
        )
    }
    const ATTR: &str = "data-gladius-frame-owner";
    let attr_lit =
        serde_json::to_string(ATTR).unwrap_or_else(|_| "\"data-gladius-frame-owner\"".to_string());
    let levels = browser
        .frame_chain_points(fid, lx, ly)
        .await
        .map_err(|e| frame_lookup_err(tool, fid, e))?;
    if levels.is_empty() {
        return Ok((lx, ly));
    }
    let nodes = browser
        .frame_owner_nodes(fid)
        .await
        .map_err(|e| frame_lookup_err(tool, fid, e))?;
    if nodes.len() != levels.len() {
        return Err(ToolErr::fallback(format!(
            "{tool}: frame owner chain mismatch for frame '{fid}'"
        )));
    }
    let mut verdict: Result<(), ToolErr> = Ok(());
    for nid in &nodes {
        if let Err(e) = browser
            .command(
                "DOM.setAttributeValue",
                json!({"nodeId": nid, "name": ATTR, "value": "1"}),
                true,
            )
            .await
        {
            verdict = Err(ToolErr::from(e));
            break;
        }
    }
    if verdict.is_ok() {
        for (realm, x, y) in &levels {
            let expr = format!("((x, y, attr) => {{ const h = document.elementFromPoint(x, y); if (!h) return 'NONE'; let n = h; while (n) {{ try {{ if (n.hasAttribute && n.hasAttribute(attr)) return 'OWNER'; }} catch (e) {{}} n = n.parentElement; }} return (h.tagName || 'NONE'); }})({x}, {y}, {attr_lit})");
            let v: Result<Value, ToolErr> = match realm {
                None => browser.eval(&expr).await.map_err(ToolErr::from),
                Some(p) => frame_realm_eval(browser, tool, p, &expr).await,
            };
            match v {
                Ok(v) if v.as_str() == Some("OWNER") => {}
                Ok(_) => {
                    verdict = Err(occluded(tool, element, fid));
                    break;
                }
                Err(e) => {
                    verdict = Err(e);
                    break;
                }
            }
        }
    }
    for realm in levels.iter().map(|(r, _, _)| r) {
        let cleanup = format!("((attr) => {{ try {{ document.querySelectorAll('[' + attr + ']').forEach((n) => n.removeAttribute(attr)); }} catch (e) {{}} return 'ok'; }})({attr_lit})");
        match realm {
            None => {
                let _ = browser.eval(&cleanup).await;
            }
            Some(p) => {
                let _ = frame_realm_eval(browser, tool, p, &cleanup).await;
            }
        }
    }
    let (mx, my) = levels.last().map(|(_, x, y)| (*x, *y)).unwrap_or((lx, ly));
    verdict.map(|()| (mx, my))
}

/// Structured `snapshot` payload (G2): the node table the text block was
/// built from (`items`), trimmed to exactly the nodes whose lines survived
/// the 8000-char cap, so `nodes` matches the text block's refs one-for-one
/// (including on a truncated page). Zero new page evaluation - `items` is
/// what the snapshot already computed.
/// One structured snapshot node from a rendered item.
fn node_json(it: &Value) -> Value {
    json!({
        "ref": it.get("ref").and_then(|x| x.as_str()).unwrap_or(""),
        "role": it.get("role").and_then(|x| x.as_str()).unwrap_or(""),
        "name": it.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        "tag": it.get("tag").and_then(|x| x.as_str()).unwrap_or(""),
        "disabled": it.get("disabled").and_then(|x| x.as_bool()).unwrap_or(false),
        "value": it.get("value").and_then(|x| x.as_str()).unwrap_or(""),
    })
}

/// Render one page of a snapshot at the unchanged 8000-char cap.
///
/// `offset` is a line index into the cached render (`0` is the header line);
/// `nextOffset` is the next page's first line, `null` on the last page.
/// Paging NEVER re-renders: it slices the cache stored by the `offset:0`
/// call, so every page keeps the original render's ref namespace.
#[allow(clippy::too_many_arguments)] // one render = one call; a param struct would add no check
async fn emit_paged_snapshot(
    browser: &Browser,
    mode: &str,
    frame: Option<&str>,
    offset: usize,
    snapshot_id: Option<&str>,
    start: usize,
    url: String,
    title: String,
    items: Vec<Value>,
    text: String,
    scope: &SnapScope,
) -> Result<Value, ToolErr> {
    if offset > 0 && snapshot_id.is_none() {
        return Err(ToolErr::typed(
            format!(
                "snapshot: offset {offset} requires snapshotId (the id the offset:0 render returned; paging slices that render and never re-renders)"
            ),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "snapshot",
                   "arg": "snapshotId", "offset": offset}),
        ));
    }
    let cache = if offset == 0 {
        let mut lines: Vec<String> = Vec::with_capacity(items.len() + 3);
        lines.push(format!("AX snapshot: {title} <{url}>"));
        for it in items.iter() {
            let r = it.get("ref").and_then(|x| x.as_str()).unwrap_or("?");
            let role = it.get("role").and_then(|x| x.as_str()).unwrap_or("");
            let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let tag = it.get("tag").and_then(|x| x.as_str()).unwrap_or("");
            let val = it.get("value").and_then(|x| x.as_str()).unwrap_or("");
            let dis = it
                .get("disabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let mut ln = format!("- {role} \"{name}\" [@{r}] ({tag})");
            if !val.is_empty() {
                ln.push_str(&format!(" value=\"{val}\""));
            }
            if dis {
                ln.push_str(" [disabled]");
            }
            lines.push(ln);
        }
        let c = crate::browser::SnapshotCache {
            id: browser.next_snapshot_id(),
            mode: mode.to_string(),
            frame: frame.unwrap_or("").to_string(),
            scope: scope.label.clone(),
            lines,
            text,
            url,
            title,
            nodes: items.iter().map(node_json).collect(),
            start,
        };
        browser.store_snapshot(c.clone()).await;
        c
    } else {
        let _paging_guard = browser.lock_snapshot().await;
        let c = browser.cached_snapshot().await.ok_or_else(|| {
            ToolErr::typed(
                "snapshot: no cached render to page; call snapshot without offset first"
                    .to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::SnapshotExpired.as_str(), "tool": "snapshot", "offset": offset}),
            )
        })?;
        if let Some(want) = snapshot_id {
            if want != c.id {
                return Err(ToolErr::typed(
                    format!(
                        "snapshot: snapshotId '{want}' does not match the cached render '{}'; re-snapshot from offset 0",
                        c.id
                    ),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::SnapshotExpired.as_str(), "tool": "snapshot", "snapshotId": want}),
                ));
            }
        }
        if c.mode != mode || c.frame != frame.unwrap_or("") {
            return Err(ToolErr::typed(
                "snapshot: offset paging must use the same mode/frame as the cached render; re-snapshot from offset 0".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::SnapshotExpired.as_str(), "tool": "snapshot", "offset": offset}),
            ));
        }
        if c.scope != scope.label {
            return Err(ToolErr::typed(
                format!(
                    "snapshot: the cached render belongs to {} and this call reads {}; re-snapshot from offset 0",
                    if c.scope.is_empty() {
                        "the legacy page".to_string()
                    } else {
                        c.scope.clone()
                    },
                    if scope.label.is_empty() {
                        "the legacy page".to_string()
                    } else {
                        scope.label.clone()
                    }
                ),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::SnapshotExpired.as_str(), "tool": "snapshot", "offset": offset}),
            ));
        }
        c
    };
    let cap = 8000usize;
    let total_lines = cache.lines.len();
    let total_nodes = cache.nodes.len();
    if offset >= total_lines {
        return Err(ToolErr::typed(
            format!("snapshot: offset {offset} is past the end (totalLines {total_lines})"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "offset", "offset": offset, "totalLines": total_lines}),
        ));
    }
    let mut end = offset;
    let mut last_fit: Option<(String, usize, bool)> = None;
    while end < total_lines {
        end += 1;
        let last = end >= total_lines;
        let mut footer = if total_nodes == 0 {
            format!("\n[snapshot: {total_nodes} nodes, refs none]")
        } else {
            format!(
                "\n[snapshot: {total_nodes} nodes, refs @e{}..@e{}]",
                cache.start + 1,
                cache.start + total_nodes
            )
        };
        if last {
            footer.push_str(&format!(
                "\n[page: lines {offset}..{end} of {total_lines}; complete]"
            ));
        } else {
            footer.push_str(&format!(
                "\n[page: lines {offset}..{end} of {total_lines}; next offset {end}]"
            ));
            let rest_chars: usize = cache
                .lines
                .iter()
                .skip(end)
                .map(|l| l.chars().count() + 1)
                .sum::<usize>()
                + cache.text.chars().count();
            footer.push_str(&format!(
                "\n[truncated: showing {end} of {total_lines} lines, {rest_chars} chars omitted]"
            ));
        }
        let body = cache.lines[offset..end].join("\n");
        let mut tail = if last && !cache.text.is_empty() {
            format!("\n{}", cache.text)
        } else {
            String::new()
        };
        let mut tail_cut = false;
        let mut full = format!("{body}{tail}{footer}");
        if full.chars().count() > cap && !tail.is_empty() {
            tail_cut = true;
            let room = cap
                .saturating_sub(body.chars().count())
                .saturating_sub(footer.chars().count());
            tail = tail.chars().take(room).collect();
            full = format!("{body}{tail}{footer}");
        }
        if full.chars().count() <= cap {
            let done = last;
            last_fit = Some((full, end, tail_cut));
            if done {
                break;
            }
        } else {
            break;
        }
    }
    let Some((full, end, tail_cut)) = last_fit else {
        let line_chars = cache.lines[offset].chars().count();
        return Err(ToolErr::typed(
            format!(
                "snapshot: line {offset} is {line_chars} chars and alone exceeds the {cap}-char page cap; cannot page this render (re-snapshot with mode:\"text\" for the page text)"
            ),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot",
                   "arg": "offset", "offset": offset, "lineChars": line_chars,
                   "totalLines": total_lines, "cap": cap}),
        ));
    };
    let next = if end < total_lines { Some(end) } else { None };
    let truncated = next.is_some() || tail_cut;
    let node_lo = offset.saturating_sub(1).min(total_nodes);
    let node_hi = end.saturating_sub(1).min(total_nodes);
    let nodes: Vec<Value> = if node_hi > node_lo {
        cache.nodes[node_lo..node_hi].to_vec()
    } else {
        Vec::new()
    };
    let mut structured = json!({
        "url": cache.url, "title": cache.title, "truncated": truncated,
        "nodes": nodes, "snapshotId": cache.id,
        "offset": offset, "totalLines": total_lines,
        "nextOffset": match next { Some(n) => json!(n), None => Value::Null },
    });
    if let Some(obj) = structured.as_object_mut() {
        if let (Some(ctx), Some(tgt)) = (&scope.context, &scope.target) {
            obj.insert("context".to_string(), json!(ctx));
            obj.insert("target".to_string(), json!(tgt));
        }
    }
    Ok(ok_snapshot(
        full,
        structured,
    ))
}

/// AX snapshot result: the human text block stays FIRST and byte-identical
/// (cap, footer, and truncation behavior unchanged); the machine node table
/// rides as the second block plus `structuredContent`. `snapshot
/// {"mode":"text"}` never gains one.
fn ok_snapshot(text: String, structured: Value) -> Value {
    json!({"content": [{"type": "text", "text": text},
                       {"type": "text", "text": structured.to_string()}],
           "structuredContent": structured})
}

/// Add the live viewport to a snapshot's machine channel. The text block is
/// the AX one-line tree, not JSON, so it is left byte-for-byte as the snapshot
/// renderer wrote it.
fn echo_viewport(mut envelope: Value, viewport: &Value) -> Value {
    if let Some(sc) = envelope
        .get_mut("structuredContent")
        .and_then(|v| v.as_object_mut())
    {
        if !sc.contains_key("viewport") {
            sc.insert("viewport".to_string(), viewport.clone());
        }
    }
    envelope
}

/// Which realm a snapshot read. The empty scope is the legacy page; a context
/// read is labelled `context:<name>#<targetId>` so paging can never slice
/// another realm's render.
#[derive(Clone, Default)]
struct SnapScope {
    label: String,
    context: Option<String>,
    target: Option<String>,
}

async fn tool_snapshot(
    browser: &Browser,
    mode: Option<&str>,
    frame: Option<&str>,
    offset: usize,
    snapshot_id: Option<&str>,
    context: Option<&str>,
    page: Option<&str>,
) -> Result<Value, ToolErr> {
    if page.is_some() && context.is_none() {
        return Err(ToolErr::typed(
            "snapshot: 'page' names a page inside a named context; pass 'context' with it"
                .to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "snapshot", "arg": "context"}),
        ));
    }
    if context.is_some() && frame.is_some() {
        return Err(ToolErr::typed(
            "snapshot: 'frame' scopes the legacy page's tree and 'context' reads a context page; \
             pass one (frame-scoped refs do not exist for context pages)"
                .to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "frame"}),
        ));
    }
    let page_ctx = match context {
        Some(name) => Some(
            browser
                .context_page(name, page)
                .await
                .map_err(ToolErr::from)?,
        ),
        None => None,
    };
    let scope = match &page_ctx {
        Some(cp) => SnapScope {
            label: format!("context:{}#{}", cp.context, cp.target),
            context: Some(cp.context.clone()),
            target: Some(cp.target.clone()),
        },
        None => SnapScope::default(),
    };
    let result = match mode {
        Some("text") => {
            if offset > 0 {
                return Err(ToolErr::typed(
                    "snapshot: mode 'text' is not line-paged (use the default AX mode for refs and offset paging)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "offset"}),
                ));
            }
            tool_snapshot_text_frame(browser, frame, page_ctx.as_ref()).await
        }
        Some("legacy") if page_ctx.is_some() => Err(ToolErr::typed(
            "snapshot: mode 'legacy' reads the legacy DOM and stamps refs into it, so it does not \
             read a context page; use the default AX mode (or mode \"text\") with context="
                .to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "mode"}),
        )),
        Some("legacy") if offset > 0 => {
            emit_paged_snapshot(
                browser,
                "legacy",
                frame,
                offset,
                snapshot_id,
                0,
                String::new(),
                String::new(),
                Vec::new(),
                String::new(),
                &scope,
            )
            .await
        }
        Some("ax") | None if offset > 0 => {
            emit_paged_snapshot(
                browser,
                "ax",
                frame,
                offset,
                snapshot_id,
                0,
                String::new(),
                String::new(),
                Vec::new(),
                String::new(),
                &scope,
            )
            .await
        }
        Some("legacy") => tool_snapshot_legacy(browser, frame, offset, snapshot_id).await,
        Some("ax") | None => {
            tool_snapshot_ax(browser, frame, offset, snapshot_id, page_ctx.as_ref()).await
        }
        Some(other) => Err(ToolErr::typed(
            format!("snapshot: unknown mode '{other}' (want 'ax'|'legacy'|'text')"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "snapshot", "arg": "mode", "want": other}),
        )),
    };
    let result = result?;
    if page_ctx.is_some() {
        // The context page's viewport is not the legacy one, so the legacy
        // read-back would be a claim about the wrong page. Omit it.
        return Ok(result);
    }
    if let Ok((w, h, _)) = browser.viewport_readback().await {
        Ok(echo_viewport(result, &json!({"width": w, "height": h})))
    } else {
        Ok(result)
    }
}

async fn tool_snapshot_legacy(
    browser: &Browser,
    frame: Option<&str>,
    offset: usize,
    snapshot_id: Option<&str>,
) -> Result<Value, ToolErr> {
    let _snap_guard = browser.lock_snapshot().await;
    let start = browser.ref_start();
    let raw_js = r#"(() => {
  const base = __REF_BASE__;
  const SENS_AUTOCOMPLETE = new Set(["current-password", "new-password", "cc-number", "cc-exp", "cc-csc", "one-time-code"]);
  const isSensitive = (el) => {
    if ((el.tagName || "").toUpperCase() !== "INPUT") return false;
    const ty = String((el.getAttribute && el.getAttribute("type")) || el.type || "text").toLowerCase();
    if (ty === "password") return true;
    const ac = String((el.getAttribute && el.getAttribute("autocomplete")) || el.autocomplete || "");
    return ac.toLowerCase().split(/\s+/).filter(Boolean).some((tok) => SENS_AUTOCOMPLETE.has(tok));
  };
  const cands = Array.from(document.querySelectorAll("a,button,input,select,textarea,[role],[tabindex],[onclick],[onmouseover]"));
  const seen = new Set(); const items = []; let n = 0;
  for (const el of cands) {
    if (!el || seen.has(el)) continue; seen.add(el);
    if (el.tagName === "OPTION") continue;
    let role = (el.getAttribute && el.getAttribute("role")) || "";
    if (!role) {
      const t = (el.tagName || "").toUpperCase();
      if (t === "BUTTON") role = "button";
      else if (t === "A") role = "link";
      else if (t === "SELECT") role = "combobox";
      else if (t === "TEXTAREA") role = "textbox";
      else if (t === "INPUT") {
        const ty = (el.type || "text").toLowerCase();
        if (ty === "checkbox" || ty === "radio") role = "checkbox";
        else if (ty === "submit" || ty === "button" || ty === "image") role = "button";
        else role = "textbox";
      } else role = t.toLowerCase() || "generic";
    }
    let name = (el.getAttribute && (el.getAttribute("aria-label") || "")) || "";
    if (!name) {
      const t = (el.tagName || "").toUpperCase();
      if (t === "INPUT" || t === "TEXTAREA") { const rawForName = (t === "INPUT" && isSensitive(el)) ? "" : (el.value || ""); name = el.placeholder || rawForName || el.id || el.name || el.type || ""; }
      else if (t === "SELECT") { try { const o = el.options[el.selectedIndex]; name = (o && o.text) || el.id || el.name || ""; } catch (e) { name = el.id || ""; } }
      else name = el.innerText || el.textContent || el.id || "";
    }
    name = (name + "").replace(/\s+/g, " ").trim().slice(0, 80);
    let value = "";
    try {
      const t = (el.tagName || "").toUpperCase();
      if (t === "INPUT" || t === "TEXTAREA" || t === "SELECT") { if (t === "INPUT" && isSensitive(el)) value = "ÔÇóÔÇóÔÇóÔÇó"; else value = String(el.value).slice(0, 60); }
    } catch (e) {}
    n++;
    const ref = "e" + (base + n);
    try { el.dataset.gladiusRef = ref; } catch (e) {}
    items.push({ref: ref, role: role, name: name, value: value,
                tag: (el.tagName || "").toLowerCase(), disabled: !!el.disabled});
  }
  return {url: location.href, title: document.title, items: items};
})()"#;
    let expr = raw_js.replace("__REF_BASE__", &start.to_string());
    let v = match frame {
        Some(fid) => browser.eval_in_frame(fid, &expr).await?,
        None => browser.eval(&expr).await?,
    };
    let empty = Vec::new();
    let items = v.get("items").and_then(|i| i.as_array()).unwrap_or(&empty);
    let url = v
        .get("url")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let title = v
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let mut table = std::collections::HashMap::new();
    for it in items.iter() {
        if let (Some(r), role, name) = (
            it.get("ref").and_then(|x| x.as_str()),
            it.get("role").and_then(|x| x.as_str()).unwrap_or(""),
            it.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        ) {
            table.insert(
                r.to_string(),
                crate::browser::RefEntry {
                    label: format!("{role} {name}"),
                    backend: None,
                    frame: None,
                    session: None,
                },
            );
        }
    }
    browser.store_refs(url.clone(), table).await;
    let total = items.len();
    browser.ref_advance(total);
    let tv = match frame {
        Some(fid) => {
            browser
                .eval_in_frame(
                    fid,
                    "((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)",
                )
                .await
        }
        None => {
            browser
                .eval("((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)")
                .await
        }
    };
    let raw = tv
        .ok()
        .and_then(|x| x.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let mut text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        text = format!("text: {text}");
    }
    let items_owned: Vec<Value> = items.to_vec();
    emit_paged_snapshot(
        browser,
        "legacy",
        frame,
        offset,
        snapshot_id,
        start,
        url,
        title,
        items_owned,
        text,
        &SnapScope::default(),
    )
    .await
}

/// One `Runtime.callFunctionOn` body enriching up to 100 resolved objects in
/// the receiver's world. Kept at module scope: `enrich_objects` splits a
/// rejected chunk and re-runs this exact function on each sub-chunk.
const ENRICH_FN: &str = r#"function(...rest) {
  const SENS = new Set(["current-password", "new-password", "cc-number", "cc-exp", "cc-csc", "one-time-code"]);
  const els = [this, ...rest];
  return els.map((el) => {
    if (!el || !el.tagName) return null;
    const tag = ((el.tagName || "").toLowerCase());
    let rawVal = "";
    try { if ("value" in el) rawVal = String(el.value); } catch (e) {}
    const ty = String((el.getAttribute && el.getAttribute("type")) || el.type || "").toLowerCase();
    const ac = String((el.getAttribute && el.getAttribute("autocomplete")) || el.autocomplete || "");
    const sens = (tag === "input") && (ty === "password" || ac.toLowerCase().split(/\s+/).filter(Boolean).some((t) => SENS.has(t)));
    let ph = ""; try { ph = el.placeholder || ""; } catch (e) {}
    let eid = ""; try { eid = el.id || ""; } catch (e) {}
    let enm = ""; try { enm = el.name || ""; } catch (e) {}
    let selText = "";
    try { if (tag === "select" && el.options && el.selectedIndex >= 0) { const o = el.options[el.selectedIndex]; selText = (o && (o.text || o.textContent)) || ""; } } catch (e) {}
    return {tag: tag, disabled: !!el.disabled, value: rawVal.slice(0, 60), fullVal: rawVal, sens: sens, ph: String(ph || ""), eid: String(eid || ""), enm: String(enm || ""), selText: String(selText || ""), ty: String(ty || "").slice(0, 20)};
  });
}"#;

/// Enrich resolved objects in chunks of 100, splitting a chunk the engine
/// rejects. `Runtime.callFunctionOn` requires every argument to share the
/// receiver's JavaScript world, so a chunk that mixes worlds (nodes of two
/// frames/realms in one tree) fails as a whole; the split keeps that failure
/// to the object that genuinely cannot be read instead of nulling its
/// neighbours. Values keep the input order; a singleton that still fails is
/// `Null` and takes the masked fallback.
async fn enrich_objects(browser: &Browser, ids: &[String]) -> Vec<Value> {
    enrich_objects_in(browser, None, ids).await
}

/// `enrich_objects` on an explicit child session, for object ids that session
/// issued (an OOPIF's own nodes). Object ids do not cross sessions.
async fn enrich_objects_in(browser: &Browser, session: Option<&str>, ids: &[String]) -> Vec<Value> {
    let mut vals: Vec<Value> = vec![Value::Null; ids.len()];
    let mut pending: Vec<(usize, usize)> = (0..ids.len())
        .step_by(100)
        .map(|s| (s, (ids.len() - s).min(100)))
        .collect();
    pending.reverse(); // pop() runs the chunks left to right
    while let Some((start, len)) = pending.pop() {
        let chunk = &ids[start..start + len];
        let Some((receiver, rest)) = chunk.split_first() else {
            continue;
        };
        let args: Vec<Value> = rest.iter().map(|id| json!({"objectId": id})).collect();
        let got = browser
            .call_function_on_in_session(
                session,
                json!({"objectId": receiver, "functionDeclaration": ENRICH_FN,
                       "returnByValue": true, "arguments": args}),
            )
            .await
            .ok()
            .and_then(|v| v.get("result").and_then(|x| x.get("value")).cloned());
        match got {
            Some(Value::Array(a)) if a.len() == len => {
                for (i, v) in a.into_iter().enumerate() {
                    vals[start + i] = v;
                }
            }
            _ if len == 1 => {}
            _ => {
                let mid = len / 2;
                pending.push((start + mid, len - mid));
                pending.push((start, mid));
            }
        }
    }
    vals
}

/// Value reported when a node's live element could not be read at all. The
/// AX tree may already carry a value, but a text-entry control never falls
/// back to it raw: the element is gone, sensitivity cannot be re-checked
/// (`sens` lives on the element), and the credential mask wins over
/// legibility on this rare path.
fn unreadable_value(role: &str, ax_value: &str) -> String {
    if ax_value.is_empty() {
        return String::new();
    }
    match role {
        "textbox" | "searchbox" => "ÔÇóÔÇóÔÇóÔÇó".to_string(),
        _ => ax_value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{list, unreadable_value};

    /// The `selector` alias makes element one of two alternatives. JSON Schema
    /// ANDs `required` with `anyOf`, so leaving `element` in `required` makes a
    /// selector-only call invalid for a validating client even though the
    /// server itself never validates inputSchema. Screenshot's element target
    /// was always optional, so it must stay free of both `required` and `anyOf`.
    #[test]
    fn selector_alias_tools_do_not_require_element() {
        let tools = list();
        let by_name = |name: &str| {
            tools["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .find(|t| t["name"] == name)
                .unwrap_or_else(|| panic!("missing tool {name}"))["inputSchema"]
                .clone()
        };
        for (name, other_required) in [
            ("click", vec![]),
            ("type", vec!["text"]),
            ("select", vec![]),
            ("hover", vec![]),
            ("upload", vec!["paths"]),
        ] {
            let schema = by_name(name);
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            assert!(
                !required.contains(&"element"),
                "{name} must not require element: {schema}"
            );
            for key in other_required {
                assert!(required.contains(&key), "{name} must still require {key}: {schema}");
            }
            assert_eq!(
                schema["anyOf"],
                json!([{"required": ["element"]}, {"required": ["selector"]}]),
                "{name} anyOf must name both target forms: {schema}"
            );
        }
        let shot = by_name("screenshot");
        assert!(shot.get("anyOf").is_none(), "screenshot must have no anyOf: {shot}");
        assert!(
            shot.get("required").is_none(),
            "screenshot must have no required target: {shot}"
        );
        assert!(shot["properties"]["selector"].is_object(), "screenshot keeps selector");
    }

    /// The unreadable fallback is the ONE branch where the value comes from
    /// the AX tree instead of the live element, so it cannot consult the
    /// element's `sens` flag; a text-entry node must mask, everything else
    /// keeps the AX value it already had.
    #[test]
    fn unreadable_fallback_never_emits_a_credential() {
        assert_eq!(unreadable_value("textbox", "PW_SECRET_123"), "ÔÇóÔÇóÔÇóÔÇó");
        assert_eq!(
            unreadable_value("searchbox", "PW_SECRET_123"),
            "ÔÇóÔÇóÔÇóÔÇó"
        );
        assert_eq!(unreadable_value("textbox", ""), "", "empty stays empty");
        assert_eq!(unreadable_value("button", "OK"), "OK");
        assert_eq!(
            unreadable_value("combobox", "chosen option"),
            "chosen option"
        );
    }

    /// The five interception actions must reach the implemented net module,
    /// not fall through to `network: unknown action`. With no live engine the
    /// four arming actions refuse at the engine boundary; `restore` is a typed
    /// success because nothing was armed. The retired B3 stub must be gone.
    #[tokio::test]
    async fn network_interception_actions_reach_the_net_module() {
        use serde_json::json;

        use crate::browser::Browser;

        let browser = Browser::new();
        let restored = super::call("network", &json!({"action": "restore"}), &browser).await;
        assert_eq!(restored["structuredContent"]["ok"].as_bool(), Some(true), "{restored}");
        assert_eq!(restored["structuredContent"]["action"], "restore", "{restored}");
        assert_eq!(restored["structuredContent"]["removed"], 0, "{restored}");
        assert!(restored.get("isError").is_none(), "{restored}");

        for (action, args) in [
            ("route", json!({"action": "route", "pattern": "*/x.json"})),
            ("mock", json!({"action": "mock", "url": "*/x.json", "body": {"value": "mocked"}})),
            ("abort", json!({"action": "abort", "url": "*/x.json"})),
            ("offline", json!({"action": "offline"})),
        ] {
            let env = super::call("network", &args, &browser).await;
            assert_eq!(env["structuredContent"]["tool"], "network", "{action}: {env}");
            assert_eq!(env["isError"], true, "{action}: {env}");
            let text = env["content"][0]["text"].as_str().unwrap_or("");
            assert!(text.contains("no live engine"), "{action}: {text}");
            assert!(!text.contains("not implemented"), "{action}: {text}");
        }

        let unknown = super::call("network", &json!({"action": "bogus"}), &browser).await;
        assert_eq!(unknown["structuredContent"]["reason"], "unknown_action");
    }
}

/// Real Accessibility-tree snapshot (the DEFAULT when `mode` is absent or
/// null; explicit `mode:"ax"` is identical; optional `frame=`).
/// Nodes come from `Accessibility.getFullAXTree`: non-ignored AX nodes with
/// a `backendDOMNodeId`. Nothing is stamped into the page: the backend id IS
/// the ref's identity (`DOM.resolveNode` -> `Runtime.callFunctionOn`), the
/// same resolver `click`/`type`/`hover`/`select`/`upload` and the geometry
/// tools use, and `DOM.resolveNode` pierces open shadow roots - so a shadow
/// ref is actionable exactly like a light-DOM ref. Role/name come from the
/// AX node; tag, disabled and the live value refine from the resolved element
/// (a node whose element cannot be read keeps the AX field and the mask).
/// Unlabeled form controls (empty AX name on input/textarea/select) fall back
/// the same way the legacy heuristic names them (placeholder, live value
/// unless sensitive, selected option, id, name) so an unlabeled control keeps
/// a stable human name on both paths. Sensitive inputs keep the G0 mask, on
/// the enriched path and on the unreadable fallback. AX-ignored subtrees
/// (display:none, aria-hidden, and other ignored nodes) never appear in the
/// lines: an old ref to such a node resolves through the guard to the typed
/// `stale_ref` (unknown/node-gone), never a silent main-frame click.
/// `mode:"legacy"` is the exception: it is the DOM-heuristic path, which
/// still stamps `data-gladius-ref` in one page eval and resolves refs by that
/// attribute (no backend ids, no ML shadow support).
/// Text format, block2 shape, cap, and footer match the legacy path.
/// Same-process iframes are covered when their frame is read directly
/// (snapshot frame=, or an OOPIF session); an engine that refuses a frame
/// surfaces the error instead of faked content.
async fn tool_snapshot_ax(
    browser: &Browser,
    frame: Option<&str>,
    offset: usize,
    snapshot_id: Option<&str>,
    page: Option<&crate::browser::ContextPage>,
) -> Result<Value, ToolErr> {
    let _snap_guard = browser.lock_snapshot().await;
    let start = browser.ref_start();
    let (tree, frame_session) = match page {
        // A context page owns its own session: its AX tree and every backend
        // id in it are read there, never on the legacy page session.
        Some(cp) => (
            browser
                .ax_tree_in_session(&cp.session, None)
                .await
                .map_err(ToolErr::from)?,
            Some(cp.session.clone()),
        ),
        None => frame_ax_tree(browser, "snapshot", frame).await?,
    };
    let empty_nodes = Vec::new();
    let ax_nodes = tree
        .get("nodes")
        .and_then(|n| n.as_array())
        .unwrap_or(&empty_nodes);
    struct Cand {
        backend: i64,
        role: String,
        name: String,
        ax_value: String,
        ax_disabled: bool,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for n in ax_nodes.iter() {
        if n.get("ignored").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let backend = match n.get("backendDOMNodeId").and_then(|v| v.as_i64()) {
            Some(b) => b,
            None => continue,
        };
        let role_raw = n
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if role_raw.is_empty() {
            continue;
        }
        let role = role_raw.to_lowercase();
        if role == "statictext" || role == "inlinetextbox" {
            continue;
        }
        let raw_name = n
            .get("name")
            .and_then(|x| x.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name: String = raw_name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(80)
            .collect();
        let ax_value_raw = match n.get("value").and_then(|x| x.get("value")) {
            Some(Value::String(s)) => s.clone(),
            Some(other) if !other.is_null() => {
                let s = other.to_string();
                s.trim_matches('"').to_string()
            }
            _ => String::new(),
        };
        let ax_value: String = ax_value_raw.chars().take(60).collect();
        let mut disabled = n.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(props) = n.get("properties").and_then(|v| v.as_array()) {
            for p in props {
                if p.get("name").and_then(|v| v.as_str()) != Some("disabled") {
                    continue;
                }
                if let Some(v) = p.get("value").and_then(|v| v.get("value")) {
                    if v.as_bool() == Some(true) || v.as_i64().is_some_and(|i| i != 0) {
                        disabled = true;
                    }
                }
            }
        }
        cands.push(Cand {
            backend,
            role,
            name,
            ax_value,
            ax_disabled: disabled,
        });
    }
    if page.is_none() {
        browser.command("DOM.enable", json!({}), true).await?;
    }
    let mut resolves = Vec::with_capacity(cands.len());
    for c in cands.iter() {
        let backend = c.backend;
        let sid = frame_session.clone();
        resolves.push(async move {
            match sid.as_deref() {
                Some(s) => browser.resolve_node_in_session(s, backend).await,
                None => {
                    browser
                        .command("DOM.resolveNode", json!({"backendNodeId": backend}), true)
                        .await
                }
            }
        });
    }
    let resolved = futures_util::future::join_all(resolves).await;
    let mut items: Vec<Value> = Vec::new();
    let mut object_ids: Vec<String> = Vec::new();
    let mut n_ok: usize = 0;
    for (cand, got) in cands.iter().zip(resolved.into_iter()) {
        let oid = got.ok().and_then(|v| {
            v.get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        });
        let Some(oid) = oid else { continue };
        n_ok += 1;
        items.push(json!({
            "ref": format!("e{}", start + n_ok), "role": cand.role,
            "name": cand.name, "value": cand.ax_value, "tag": "",
            "disabled": cand.ax_disabled, "backend": cand.backend,
        }));
        object_ids.push(oid);
    }
    let enrich_vals: Vec<Value> = enrich_objects_in(browser, frame_session.as_deref(), &object_ids).await;
    let enrich_map: serde_json::Map<String, Value> = items
        .iter()
        .zip(enrich_vals.into_iter())
        .filter_map(|(it, v)| {
            let r = it.get("ref").and_then(|x| x.as_str())?.to_string();
            Some((r, v))
        })
        .collect();
    for it in items.iter_mut() {
        let r = it
            .get("ref")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let ax_value = it
            .get("value")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let ax_disabled = it
            .get("disabled")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        match enrich_map.get(&r) {
            Some(Value::Null) | None => {
                it["tag"] = json!("");
                let role = it.get("role").and_then(|x| x.as_str()).unwrap_or("");
                it["value"] = json!(unreadable_value(role, &ax_value));
                it["disabled"] = json!(ax_disabled);
            }
            Some(info) => {
                let tag = info
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let live_dis = info
                    .get("disabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let live_val = info
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let sens = info.get("sens").and_then(|v| v.as_bool()).unwrap_or(false);
                let value = if sens {
                    "ÔÇóÔÇóÔÇóÔÇó".to_string()
                } else if tag == "input" || tag == "textarea" || tag == "select" {
                    live_val.clone()
                } else {
                    ax_value
                };
                let ax_name = it
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let printed = snapshot_name(&ax_name, &tag, sens, Some(info));
                if printed != ax_name {
                    it["name"] = json!(printed);
                }
                it["tag"] = json!(tag);
                it["value"] = json!(value);
                it["disabled"] = json!(ax_disabled || live_dis);
            }
        }
    }
    let (url, title, text) = match (page, frame) {
        (Some(cp), _) => {
            let pair = browser
                .eval_in_session(&cp.session, "[window.location.href, document.title]")
                .await
                .ok();
            // A live read is the evidence; the target record is the stated
            // fallback (and the reply never invents an empty title).
            let u = pair
                .as_ref()
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| cp.url.clone());
            let t = pair
                .as_ref()
                .and_then(|v| v.get(1))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| cp.title.clone());
            let tx = browser
                .eval_in_session(
                    &cp.session,
                    "((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)",
                )
                .await
                .ok()
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (u, t, tx)
        }
        (None, Some(fid)) => {
            let pair = frame_realm_eval(
                browser,
                "snapshot",
                fid,
                "[window.location.href, document.title]",
            )
            .await
            .ok();
            let u = pair
                .as_ref()
                .and_then(|v| v.get(0))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let t = pair
                .as_ref()
                .and_then(|v| v.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tv = frame_realm_eval(
                browser,
                "snapshot",
                fid,
                "((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)",
            )
            .await
            .ok()
            .and_then(|x| x.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
            let tx = tv.split_whitespace().collect::<Vec<_>>().join(" ");
            (u, t, tx)
        }
        (None, None) => {
            let uv = browser.eval("window.location.href").await.ok();
            let u = uv
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let tv2 = browser.eval("document.title").await.ok();
            let t = tv2
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let tv = browser
                .eval("((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)")
                .await;
            let raw = tv
                .ok()
                .and_then(|x| x.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let tx = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            (u, t, tx)
        }
    };
    let mut table = std::collections::HashMap::new();
    for it in items.iter() {
        if let (Some(r), role, name) = (
            it.get("ref").and_then(|x| x.as_str()),
            it.get("role").and_then(|x| x.as_str()).unwrap_or(""),
            it.get("name").and_then(|x| x.as_str()).unwrap_or(""),
        ) {
            table.insert(
                r.to_string(),
                crate::browser::RefEntry {
                    label: format!("{role} {name}"),
                    backend: it.get("backend").and_then(|b| b.as_i64()),
                    frame: frame.map(str::to_string),
                    session: frame_session.clone(),
                },
            );
        }
    }
    let scope = match page {
        Some(cp) => SnapScope {
            label: format!("context:{}#{}", cp.context, cp.target),
            context: Some(cp.context.clone()),
            target: Some(cp.target.clone()),
        },
        None => SnapScope::default(),
    };
    if page.is_none() {
        let snap_url = match frame {
            Some(_) => browser
                .eval("window.location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            None => url.clone(),
        };
        browser.store_refs(snap_url, table).await;
    }
    let total = items.len();
    browser.ref_advance(total);
    let mut text = text;
    if !text.is_empty() {
        text = format!("text: {text}");
    }
    emit_paged_snapshot(
        browser,
        "ax",
        frame,
        offset,
        snapshot_id,
        start,
        url,
        title,
        items,
        text,
        &scope,
    )
    .await
}

async fn tool_snapshot_text_frame(
    browser: &Browser,
    frame: Option<&str>,
    page: Option<&crate::browser::ContextPage>,
) -> Result<Value, ToolErr> {
    const TEXT_JS: &str = "((((document.body && document.body.innerText) || (document.documentElement && document.documentElement.innerText) || '')) + '').slice(0, 12000)";
    let v = match (page, frame) {
        (Some(cp), _) => browser.eval_in_session(&cp.session, TEXT_JS).await?,
        (None, Some(fid)) => browser.eval_in_frame(fid, TEXT_JS).await?,
        (None, None) => browser.eval(TEXT_JS).await?,
    };
    let raw = v.as_str().unwrap_or("").to_string();
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lines = raw.lines().count().max(1);
    let suffix = format!("\n[text-only snapshot: {lines} lines, no element refs]");
    let budget = 8000usize.saturating_sub(suffix.len());
    let capped: String = if collapsed.chars().count() > budget {
        collapsed.chars().take(budget).collect()
    } else {
        collapsed
    };
    let prose = format!("{capped}{suffix}");
    let mut structured = json!({"mode": "text", "text": prose});
    if let (Some(cp), Some(obj)) = (page, structured.as_object_mut()) {
        obj.insert("context".to_string(), json!(cp.context));
        obj.insert("target".to_string(), json!(cp.target));
    }
    Ok(json!({"content": [{"type": "text", "text": prose.clone()}],
              "structuredContent": structured}))
}

/// Write an already-decoded screenshot PNG to `path`. There is no
/// allowlisted screenshot root in this crate, so only an ABSOLUTE path is
/// accepted (a relative path is a typed `invalid_args`/`bad_value` refusal,
/// never resolved against a working directory). Missing parent directories
/// are created; any I/O failure is a typed error, never a silent success.
/// Split out from `tool_screenshot` so the file path is unit-testable
/// without a browser.
fn write_screenshot_file(path: &str, png: &[u8]) -> Result<(), ToolErr> {
    let dest = std::path::Path::new(path);
    if !dest.is_absolute() {
        return Err(ToolErr::typed(
            format!("screenshot: refusing relative path (pass an absolute path): {path}"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "screenshot", "arg": "path"}),
        ));
    }
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolErr::fallback(format!("screenshot: cannot create parent directory: {e}"))
            })?;
        }
    }
    std::fs::write(dest, png)
        .map_err(|e| ToolErr::fallback(format!("screenshot: save failed: {e}")))?;
    Ok(())
}

/// Resolve an element target (`@eN` ref, AX selector form, or CSS selector) to
/// its border box in PAGE coordinates, for a clipped capture. The box comes
/// from `DOM.getBoxModel` - the same engine measurement `browser_layout`
/// reports - so a capture of `element` is exactly the box layout names, and a
/// node with no layout box is a typed refusal instead of an empty image.
async fn screenshot_element_box(
    browser: &Browser,
    element: &str,
) -> Result<crate::capture::Clip, ToolErr> {
    let node_id: i64 = if let Some(id) = parse_ref(element) {
        let _at = guard_ref(browser, "screenshot", element, &id).await?;
        let backend = browser.ref_backend(&id).await.ok_or_else(|| {
            ToolErr::typed(
                format!("screenshot: ref @{id} has no backend node id (take a fresh snapshot)"),
                json!({"code": ToolCode::StaleRef.as_str(), "reason": ToolReason::Unknown.as_str(), "tool": "screenshot", "ref": format!("@{id}")}),
            )
        })?;
        push_backend_node(browser, "screenshot", element, backend).await?
    } else if let Some(sel) = ax_sel_parse("screenshot", element)? {
        let hit = ax_resolve(browser, "screenshot", &sel, element, None).await?;
        push_backend_node(browser, "screenshot", element, hit.backend).await?
    } else {
        browser.command("DOM.enable", json!({}), true).await?;
        let doc = browser
            .command("DOM.getDocument", json!({"depth": 0, "pierce": true}), true)
            .await?;
        let root = doc
            .get("root")
            .and_then(|r| r.get("nodeId"))
            .and_then(|n| n.as_i64())
            .unwrap_or(0);
        let found = browser
            .command(
                "DOM.querySelector",
                json!({"nodeId": root, "selector": element}),
                true,
            )
            .await?;
        let nid = found.get("nodeId").and_then(|n| n.as_i64()).unwrap_or(0);
        if nid == 0 {
            return Err(ToolErr::typed(
                format!("screenshot: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "screenshot", "element": element}),
            ));
        }
        nid
    };
    let bm = match browser
        .command("DOM.getBoxModel", json!({"nodeId": node_id}), true)
        .await
    {
        Ok(v) => v,
        Err(e)
            if e.contains("Could not compute box model")
                || e.contains("no layout object")
                || e.contains("No node") =>
        {
            return Err(ToolErr::typed(
                format!("screenshot: element has no layout box (not rendered, so nothing to capture): {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "screenshot", "element": element}),
            ))
        }
        Err(e) => return Err(ToolErr::fallback(e)),
    };
    let quad = bm
        .get("model")
        .and_then(|m| m.get("border"))
        .and_then(|q| q.as_array())
        .cloned()
        .unwrap_or_default();
    let nums: Vec<f64> = quad.iter().filter_map(|v| v.as_f64()).collect();
    if nums.len() < 8 {
        return Err(ToolErr::typed(
            format!("screenshot: element has no measurable border box: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "screenshot", "element": element}),
        ));
    }
    let xs: Vec<f64> = nums.iter().step_by(2).copied().collect();
    let ys: Vec<f64> = nums.iter().skip(1).step_by(2).copied().collect();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (x, y) = (min(&xs), min(&ys));
    let (w, h) = (max(&xs) - x, max(&ys) - y);
    if w <= 0.0 || h <= 0.0 {
        return Err(ToolErr::typed(
            format!("screenshot: element box is empty ({w}x{h} CSS px): {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "screenshot", "element": element}),
        ));
    }
    Ok(crate::capture::Clip {
        x,
        y,
        width: w,
        height: h,
    })
}

/// Frontend node id for a backend node id, or a typed stale-node refusal. The
/// bridge the snapshot path uses (`DOM.pushNodesByBackendIdsToFrontend`).
async fn push_backend_node(
    browser: &Browser,
    tool: &str,
    element: &str,
    backend: i64,
) -> Result<i64, ToolErr> {
    browser.command("DOM.enable", json!({}), true).await?;
    let pushed = browser
        .command(
            "DOM.pushNodesByBackendIdsToFrontend",
            json!({"backendNodeIds": [backend]}),
            true,
        )
        .await
        .map_err(ToolErr::fallback)?;
    let nid = pushed
        .get("nodeIds")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if nid == 0 {
        return Err(ToolErr::typed(
            format!("{tool}: node no longer resolves (stale element; take a fresh snapshot): {element}"),
            json!({"code": ToolCode::StaleRef.as_str(), "reason": ToolReason::NodeGone.as_str(), "tool": tool, "element": element}),
        ));
    }
    Ok(nid)
}

/// How many frame pairs a live capture may compare before it stops claiming
/// a frozen image. Bounded: a permanently animating page is reported
/// `unfrozen`, never retried forever.
const CAPTURE_STABILITY_ATTEMPTS: u64 = 3;

/// Poll checksums until two consecutive frames agree or `ms` elapses. A page
/// that never settles is a typed failure unless `force` asks for the last frame
/// with an `unfrozen` verdict.
async fn capture_until_stable(
    browser: &Browser,
    opts: &crate::capture::Options,
    ms: u64,
    force: bool,
) -> Result<(crate::capture::Capture, Value), ToolErr> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut last: Option<crate::capture::Capture> = None;
    loop {
        let frame = crate::capture::capture(browser, opts)
            .await
            .map_err(ToolErr::from)?;
        if last
            .as_ref()
            .is_some_and(|prev| prev.checksum == frame.checksum)
        {
            let verdict = json!({"verdict":"frozen","code":"frozen","mode":"stable-poll",
                                 "checksum":frame.checksum.clone()});
            return Ok((frame, verdict));
        }
        last = Some(frame);
        if std::time::Instant::now() >= deadline {
            let verdict = json!({"verdict":"unfrozen","code":"unfrozen","mode":"stable-poll"});
            if !force {
                return Err(ToolErr::typed(
                    format!("screenshot: page did not settle within {ms}ms"),
                    json!({"code": ToolCode::Timeout.as_str(), "reason": ToolReason::Unsettled.as_str(),
                           "tool": "screenshot", "stability": verdict}),
                ));
            }
            return Ok((last.expect("a frame was captured"), verdict));
        }
    }
}

/// Double-capture a live page and compare the decoded bytes. An equal pair is
/// `frozen`; a page that keeps changing is `unfrozen` with the differing
/// checksums named. Deterministic captures are frozen by construction and pay
/// no extra capture. Both outcomes return real pixels - only the claim changes.
async fn capture_stable(
    browser: &Browser,
    opts: &crate::capture::Options,
) -> Result<(crate::capture::Capture, Value), ToolErr> {
    let first = crate::capture::capture(browser, opts)
        .await
        .map_err(ToolErr::from)?;
    if opts.deterministic {
        return Ok((
            first,
            json!({"verdict":"frozen","code":"frozen","frames":1,"attempts":0,
                   "mode":"deterministic"}),
        ));
    }
    let mut held = first;
    let mut attempts = 0u64;
    loop {
        attempts += 1;
        let next = crate::capture::capture(browser, opts)
            .await
            .map_err(ToolErr::from)?;
        if next.checksum == held.checksum {
            let verdict = json!({"verdict":"frozen","code":"frozen","frames":2,"attempts":attempts,
                                 "checksum":held.checksum.clone()});
            return Ok((held, verdict));
        }
        if attempts >= CAPTURE_STABILITY_ATTEMPTS {
            let verdict = json!({"verdict":"unfrozen","code":"unfrozen","frames":2,"attempts":attempts,
                                 "checksums":[held.checksum.clone(), next.checksum.clone()]});
            return Ok((next, verdict));
        }
        held = next;
    }
}

pub(crate) async fn tool_screenshot(
    browser: &Browser,
    full_page: bool,
    path: Option<&str>,
    deterministic: bool,
    width: Option<i64>,
    height: Option<i64>,
    element: Option<&str>,
    clip: Option<crate::capture::Clip>,
    format: crate::capture::Format,
    quality: Option<u8>,
    freeze: bool,
    wait_for_stable: Option<u64>,
    force: bool,
    scale: f64,
) -> Result<Value, ToolErr> {
    match crate::browser::backend_caps() {
        Ok(crate::browser::BackendCaps::DomOnly) => {
            return Err(ToolErr::typed(
                "screenshot: unavailable with GLADIUS_BACKEND_CAPS=dom-only backend (no rendering pipeline; refusing instead of returning a fake image)".to_string(),
                json!({"code": ToolCode::CapabilityGated.as_str(), "reason": ToolReason::DomOnly.as_str(), "tool": "screenshot"}),
            ));
        }
        Ok(crate::browser::BackendCaps::Render) => {}
        Err(e) => return Err(ToolErr::fallback(format!("screenshot: {e}"))),
    }
    // Resolve the target BEFORE freezing. Element resolution returns typed
    // errors (unknown ref, not found, hidden); applying freeze first left the
    // injected style and paused media behind on every one of those exits.
    let clip = match element {
        Some(el) => Some(screenshot_element_box(browser, el).await?),
        None => clip,
    };
    // `freeze` pauses the page's own motion sources before any framing work;
    // `motion` is the residual after freezing ([] when the pause took effect).
    // A deterministic capture already pins its own freeze and reports no new
    // sources here.
    let freeze_active = freeze && !deterministic;
    let (motion, paused) = if freeze_active {
        let acted_on = crate::capture::apply_shot_freeze(browser).await;
        (crate::capture::shot_motion(browser).await, acted_on)
    } else if deterministic {
        (Vec::new(), Vec::new())
    } else {
        (crate::capture::shot_motion(browser).await, Vec::new())
    };
    let target_before = if deterministic {
        None
    } else {
        Some(browser.active_target_id().await)
    };
    let opts = crate::capture::Options {
        full_page,
        deterministic,
        width,
        height,
        clip,
        format,
        quality,
        scale,
    };
    let captured = match wait_for_stable {
        None => Box::pin(capture_stable(browser, &opts)).await,
        Some(ms) => Box::pin(capture_until_stable(browser, &opts, ms, force)).await,
    };
    if freeze_active {
        crate::capture::restore_shot_freeze(browser).await;
    }
    let (cap, stability) = captured?;
    if let Some(before) = target_before {
        let after = browser.active_target_id().await;
        if after != before {
            return Err(ToolErr::typed(
                format!(
                    "screenshot: the active target changed during the capture ({} -> {}); retry against one page",
                    before.as_deref().unwrap_or("none"),
                    after.as_deref().unwrap_or("none")
                ),
                json!({"code": ToolCode::TargetChanged.as_str(), "reason": ToolReason::Raced.as_str(), "tool": "screenshot",
                       "before": before, "after": after}),
            ));
        }
    }
    if let Some(p) = path {
        write_screenshot_file(p, &cap.bytes)?;
    }
    let mut meta = match cap.meta() {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    meta.insert("stability".to_string(), stability);
    meta.insert("motion".to_string(), json!(motion));
    meta.insert("paused".to_string(), json!(paused));
    // A deterministic capture already carries the pinned viewport; a plain
    // capture must echo the live one so both report the same fact.
    if !meta.contains_key("viewport") {
        if let Ok((w, h, _)) = browser.viewport_readback().await {
            meta.insert("viewport".to_string(), json!({"width": w, "height": h}));
        }
    }
    if let Some(p) = path {
        meta.insert("path".to_string(), json!(p));
    }
    if let Some(el) = element {
        meta.insert("element".to_string(), json!(el));
    }
    if let Some(c) = clip {
        meta.insert(
            "clip".to_string(),
            json!({"x": c.x, "y": c.y, "width": c.width, "height": c.height}),
        );
    }
    let meta = Value::Object(meta);
    Ok(
        json!({"content": [{"type": "image", "data": cap.base64, "mimeType": cap.format.mime()},
                          {"type": "text", "text": meta.to_string()}],
              "structuredContent": meta}),
    )
}

/// Parse the `ref` / `selector` targeting of the geometry tools (string or
/// array of strings; refs must be `@eN` and known to the last snapshot; an
/// empty target set is refused rather than silently measuring nothing).
async fn layout_specs(
    browser: &Browser,
    tool: &str,
    args: &Value,
) -> Result<Vec<crate::layout::Spec>, ToolErr> {
    let mut out: Vec<crate::layout::Spec> = Vec::new();
    for key in ["ref", "selector"] {
        let Some(v) = args.get(key) else { continue };
        if v.is_null() {
            continue;
        }
        let items: Vec<String> = match v {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => {
                let mut acc = Vec::with_capacity(a.len());
                for x in a {
                    match x.as_str() {
                        Some(s) => acc.push(s.to_string()),
                        None => {
                            return Err(ToolErr::typed(
                                format!("{tool}: '{key}' array must contain strings"),
                                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": key}),
                            ))
                        }
                    }
                }
                acc
            }
            _ => {
                return Err(ToolErr::typed(
                    format!("{tool}: '{key}' must be a string or an array of strings"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": key}),
                ))
            }
        };
        for s in items {
            if key == "ref" {
                let Some(id) = parse_ref(&s) else {
                    return Err(ToolErr::typed(
                        format!("{tool}: ref '{s}' must look like @eN (from snapshot)"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": tool, "arg": "ref"}),
                    ));
                };
                guard_ref(browser, tool, &s, &id).await?;
                out.push(crate::layout::Spec::Ref(id));
            } else {
                out.push(crate::layout::Spec::Selector(s));
            }
        }
    }
    if out.is_empty() {
        return Err(ToolErr::typed(
            format!("{tool}: need 'ref' (@eN) or 'selector'"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": tool}),
        ));
    }
    Ok(out)
}

fn not_found_baseline(name: &str) -> ToolErr {
    ToolErr::typed(
        format!(
            "visual: no baseline named '{name}' in {} (save it first)",
            crate::visual::visual_dir().display()
        ),
        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "visual", "name": name}),
    )
}

/// `assert_layout` body (kept out of the dispatch match: see the boxed arms).
async fn tool_assert_layout(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(
        "assert_layout",
        args,
        &[
            "ref",
            "selector",
            "checks",
            "screenshotOnFail",
            "screenshotPath",
            "deterministic",
            "frame",
        ],
    )?;
    let fid = frame_id_ok("assert_layout", args.get("frame").and_then(|v| v.as_str()))?;
    let specs = layout_specs(browser, "assert_layout", args).await?;
    let on_fail = args
        .get("screenshotOnFail")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let deterministic = args
        .get("deterministic")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let shot_path = args.get("screenshotPath").and_then(|v| v.as_str());
    if shot_path.is_some() && !on_fail {
        return Err(ToolErr::typed(
            "assert_layout: 'screenshotPath' requires screenshotOnFail:true".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::NeedOption.as_str(), "tool": "assert_layout", "arg": "screenshotPath"}),
        ));
    }
    crate::layout::assert_layout(
        browser,
        &specs,
        args.get("checks"),
        on_fail,
        shot_path,
        deterministic,
        fid.as_deref(),
    )
    .await
    .map(|v| ok_text(v.to_string()))
}

/// `layout` body.
async fn tool_layout(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown("layout", args, &["ref", "selector", "includeOverlaps", "frame"])?;
    let fid = frame_id_ok("layout", args.get("frame").and_then(|v| v.as_str()))?;
    let specs = layout_specs(browser, "layout", args).await?;
    let overlaps = args
        .get("includeOverlaps")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    crate::layout::layout(browser, &specs, overlaps, fid.as_deref())
        .await
        .map(|v| ok_text(v.to_string()))
}

/// Page-space clip of a frame's viewport, for frame-scoped visual captures.
/// The leaf box is read in the frame realm, mapped to main-viewport CSS px
/// through the owner chain, then offset by the live page scroll.
async fn visual_frame_clip(
    browser: &Browser,
    fid: &str,
) -> Result<(crate::capture::Clip, Value), ToolErr> {
    let b = frame_realm_eval(
        browser,
        "visual",
        fid,
        "(() => { const r = document.documentElement.getBoundingClientRect(); \
         return {x:r.left,y:r.top,w:r.width,h:r.height}; })()",
    )
    .await?;
    let g = |k: &str| b.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (x, y, w, h) = (g("x"), g("y"), g("w"), g("h"));
    let levels = browser
        .frame_chain_points(fid, x, y)
        .await
        .map_err(ToolErr::from)?;
    let (mx, my) = levels.last().map(|(_, px, py)| (*px, *py)).unwrap_or((x, y));
    let scroll = browser
        .eval("[window.scrollX, window.scrollY]")
        .await
        .unwrap_or_else(|_| json!([0, 0]));
    let sx = scroll.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let sy = scroll.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let clip = crate::capture::Clip {
        x: mx + sx,
        y: my + sy,
        width: w,
        height: h,
    };
    let j = json!({"x": clip.x, "y": clip.y, "w": clip.width, "h": clip.height});
    Ok((clip, j))
}

fn stamp_frame(v: &mut Value, fid: Option<&str>, clip: Value) {
    if let Some(obj) = v.as_object_mut() {
        if let Some(f) = fid {
            obj.insert("frame".to_string(), json!(f));
        }
        if !clip.is_null() {
            obj.insert("clip".to_string(), clip);
        }
    }
}

/// `visual` body: save / compare / list / delete.
async fn tool_visual(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(
        "visual",
        args,
        &[
            "action",
            "name",
            "fullPage",
            "threshold",
            "maxDiffPixels",
            "maxDiffRatio",
            "maskRects",
            "update",
            "antialias",
            "includeAA",
            "allowIgnoredAA",
            "minDiffCluster",
            "requireEnvironmentMatch",
            "frame",
        ],
    )?;
    let fid = frame_id_ok("visual", args.get("frame").and_then(|v| v.as_str()))?;
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(action, "save" | "compare") {
        match crate::browser::backend_caps() {
            Ok(crate::browser::BackendCaps::DomOnly) => {
                return Err(ToolErr::typed(
                    format!("visual: action '{action}' is unavailable with GLADIUS_BACKEND_CAPS=dom-only (no rendering pipeline; refusing instead of comparing nothing)"),
                    json!({"code": ToolCode::CapabilityGated.as_str(), "reason": ToolReason::DomOnly.as_str(), "tool": "visual"}),
                ));
            }
            Ok(crate::browser::BackendCaps::Render) => {}
            Err(e) => return Err(ToolErr::fallback(format!("visual: {e}"))),
        }
    }
    let name = match action {
        "list" => None,
        _ => match args.get("name").and_then(|v| v.as_str()) {
            Some(n) => Some(n.to_string()),
            None => {
                return Err(ToolErr::typed(
                    format!("visual: action '{action}' needs 'name'"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "visual", "arg": "name"}),
                ))
            }
        },
    };
    let safe = match &name {
        Some(n) => Some(crate::visual::sanitise_name(n).map_err(|e| {
            ToolErr::typed(
                e,
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "visual", "arg": "name"}),
            )
        })?),
        None => None,
    };
    let safe = safe.as_deref().unwrap_or_default();
    match action {
        "save" => {
            let full = args
                .get("fullPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (clip, clip_json) = match &fid {
                Some(f) => {
                    let (c, j) = visual_frame_clip(browser, f).await?;
                    (Some(c), j)
                }
                None => (None, Value::Null),
            };
            if full && clip.is_some() {
                return Err(ToolErr::typed(
                    "visual: 'fullPage' and 'frame' are mutually exclusive".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::NeedOption.as_str(), "tool": "visual", "arg": "frame"}),
                ));
            }
            let mut v = crate::visual::save(browser, safe, full, clip).await?;
            stamp_frame(&mut v, fid.as_deref(), clip_json);
            Ok(ok_text(v.to_string()))
        }
        "list" => crate::visual::list()
            .map(|v| ok_text(v.to_string()))
            .map_err(ToolErr::from),
        "delete" => {
            if !crate::visual::exists(safe) {
                return Err(not_found_baseline(safe));
            }
            crate::visual::delete(safe)
                .map(|v| ok_text(v.to_string()))
                .map_err(ToolErr::from)
        }
        "compare" => {
            if !crate::visual::exists(safe) {
                return Err(not_found_baseline(safe));
            }
            let (clip, clip_json) = match &fid {
                Some(f) => {
                    let (c, j) = visual_frame_clip(browser, f).await?;
                    (Some(c), j)
                }
                None => (None, Value::Null),
            };
            let antialias_arg = args.get("antialias").and_then(|v| v.as_bool());
            let include_aa = args.get("includeAA").and_then(|v| v.as_bool());
            if let (Some(a), Some(i)) = (antialias_arg, include_aa) {
                if a == i {
                    return Err(ToolErr::typed(
                        "visual: 'antialias' and 'includeAA' are inverses; send one, or send both with opposite values (antialias = !includeAA)".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "visual", "arg": "antialias"}),
                    ));
                }
            }
            let antialias = match (antialias_arg, include_aa) {
                (Some(a), _) => a,
                (None, Some(i)) => !i,
                (None, None) => true,
            };
            let min_cluster = args
                .get("minDiffCluster")
                .and_then(|v| v.as_i64())
                .unwrap_or(2);
            if !(0..=1000).contains(&min_cluster) {
                return Err(ToolErr::typed(
                    "visual: 'minDiffCluster' must be an integer in 0..=1000 (0 counts every pixel)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "visual", "arg": "minDiffCluster"}),
                ));
            }
            let opt = crate::visual::DiffOpts {
                threshold: args
                    .get("threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.1),
                max_diff_pixels: args
                    .get("maxDiffPixels")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
                max_diff_ratio: args
                    .get("maxDiffRatio")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                mask_rects: args
                    .get("maskRects")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default(),
                full_page: args
                    .get("fullPage")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                clip,
                update: args
                    .get("update")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                antialias,
                min_diff_cluster: min_cluster,
                require_environment_match: args
                    .get("requireEnvironmentMatch")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                allow_ignored_aa: args
                    .get("allowIgnoredAA")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            };
            if !(0.0..=1.0).contains(&opt.threshold) {
                return Err(ToolErr::typed(
                    "visual: 'threshold' must be a fraction in 0..=1 (per-channel share of the 0-255 range)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "visual", "arg": "threshold"}),
                ));
            }
            let mut v = crate::visual::compare(browser, safe, &opt).await?;
            stamp_frame(&mut v, fid.as_deref(), clip_json);
            Ok(ok_text(v.to_string()))
        }
        other => Err(ToolErr::typed(
            format!("visual: unknown action '{other}' (want save|compare|list|delete)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "visual", "arg": "action"}),
        )),
    }
}

/// Does `selector` resolve inside a CLOSED shadow root?
///
/// Ground truth from the DOM domain, never a guess: `DOM.getDocument
/// {pierce:true}` exposes closed shadow roots (the page realm cannot see
/// them, so `document.querySelector` can never match), and `DOM.querySelector`
/// accepts a shadow-root nodeId. Read-only, and only run on a MISS, so a real
/// page pays nothing.
///
/// `true` means the element EXISTS and the page realm cannot reach it - a
/// different answer from "not found", which is the lie S2.5 removes.
pub(crate) async fn closed_shadow_hit(browser: &Browser, selector: &str) -> bool {
    let doc = match browser
        .command(
            "DOM.getDocument",
            json!({"depth": -1, "pierce": true}),
            true,
        )
        .await
    {
        Ok(d) => d,
        Err(_) => return false, // any doubt keeps the old answer
    };
    let mut roots: Vec<i64> = Vec::new();
    collect_closed_roots(doc.get("root"), &mut roots);
    for node_id in roots {
        let r = match browser
            .command(
                "DOM.querySelector",
                json!({"nodeId": node_id, "selector": selector}),
                true,
            )
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if r.get("nodeId").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
            return true;
        }
    }
    false
}

/// Collect the nodeId of every `shadowRootType:"closed"` shadow root in a
/// `DOM.getDocument {pierce:true}` tree (nested closed roots included).
fn collect_closed_roots(node: Option<&Value>, out: &mut Vec<i64>) {
    let Some(n) = node else {
        return;
    };
    if let Some(roots) = n.get("shadowRoots").and_then(|v| v.as_array()) {
        for r in roots {
            if r.get("shadowRootType").and_then(|t| t.as_str()) == Some("closed") {
                if let Some(id) = r.get("nodeId").and_then(|v| v.as_i64()) {
                    out.push(id);
                }
            }
            collect_closed_roots(Some(r), out);
        }
    }
    if let Some(kids) = n.get("children").and_then(|v| v.as_array()) {
        for k in kids {
            collect_closed_roots(Some(k), out);
        }
    }
}

/// `shadow_closed`/`unreachable` (S2.5): the element exists inside a closed
/// shadow root, which no selector can pierce. The prose names the one lever
/// that works - the AX `@eN` ref from `snapshot` - instead of pretending the
/// element is absent.
pub(crate) fn shadow_closed(tool: &str, selector: &str) -> ToolErr {
    ToolErr::typed(
        format!(
            "{tool}: '{selector}' is inside a closed shadow root; the AX @eN ref from \
             `snapshot` is the only path that can reach it (no selector syntax can pierce \
             a closed root)"
        ),
        json!({"code": ToolCode::ShadowClosed.as_str(), "reason": ToolReason::Unreachable.as_str(), "tool": tool, "element": selector}),
    )
}

/// Main-frame click preflight (shared by the ref and selector paths): resolve,
/// visibility/disabled checks (as before), `scrollIntoView`, rect center, then
/// a `document.elementFromPoint(cx, cy)` hit-test. The hit element must be the
/// target itself or a descendant of it; anything else (an opaque overlay, a
/// covering sibling) is a typed `not_actionable`/`occluded` and no click is
/// dispatched.
///
/// `<option>` fallback (documented, the ONLY synthetic-click path): an
/// `<option>` has no layout box, so a pointer sequence cannot hit-test or land
/// on it. That single case keeps the old in-page `el.click()`; every rendered
/// element goes through the trusted CDP pointer sequence in `click_dispatch`.
/// Fixed pre-action bound and cadence. No inputSchema change: every CSS/ref
/// action pays this same bounded wait and reports the state it actually saw.
const ACTIONABILITY_BOUND_MS: u64 = 1500;
const ACTIONABILITY_POLL_MS: u64 = 50;

/// Probe one CSS target in the page. The returned object is raw observable
/// state; the Rust loop decides when it is settled. A selector that does not
/// even parse (`querySelector` throws) reports `syntax:true` so the loop can
/// refuse it on the first poll instead of burning the caller's bound.
fn actionability_probe_js(sel: &str) -> String {
    format!(
        "(() => {{ let el; try {{ el = document.querySelector({sel}); }} catch (e) {{ return JSON.stringify({{found:false, syntax:true}}); }} if (!el) return JSON.stringify({{found:false}}); const cs = getComputedStyle(el); const r = el.getBoundingClientRect(); return JSON.stringify({{found:true, visible: cs.display !== 'none' && cs.visibility !== 'hidden' && cs.visibility !== 'collapse' && Number(cs.opacity) > 0 && r.width > 0 && r.height > 0, enabled: !el.disabled && el.getAttribute('aria-disabled') !== 'true', rect: {{x: r.left, y: r.top, w: r.width, h: r.height}}}}); }})()"
    )
}

/// Verdict+delta for an action target: poll until it is visible, enabled and
/// its box is unchanged across two consecutive samples, under a hard bound.
/// AX selectors and frame-scoped targets resolve in their own realm at action
/// time, so they are reported `skipped` rather than probed against the wrong
/// document - an honest delta, never a false "settled". A selector that does
/// not even parse is `refused` on the first poll, so the action's resolver
/// answers with the typed syntax error instead of a timeout the caller never
/// deserved.
async fn actionability_wait(browser: &Browser, element: &str, frame: Option<&str>) -> Value {
    let ax = ["role=", "name=", "text=", "label="]
        .iter()
        .any(|p| element.starts_with(p));
    // Refs resolve through the snapshot's recorded object (including shadow
    // roots and stale-ref detection); probing them by attribute would find a
    // different element or hide the typed stale-ref. AX selectors and frames
    // resolve in their own realm too. Report `skipped`, never a false wait.
    if frame.is_some() || ax || parse_ref(element).is_some() {
        return json!({"verdict":"settled","mode":"skipped","waitedMs":0,"polls":0,
                      "boundMs":ACTIONABILITY_BOUND_MS,"visible":true,"enabled":true,
                      "stable":true,"note":"resolved by its own path at action time"});
    }
    let sel = serde_json::to_string(element).unwrap_or_else(|_| "null".to_string());
    if sel == "null" {
        return json!({"verdict":"settled","mode":"skipped","waitedMs":0,"polls":0,
                      "boundMs":ACTIONABILITY_BOUND_MS,"visible":true,"enabled":true,
                      "stable":true,"note":"element not probeable"});
    }
    let expr = actionability_probe_js(&sel);
    let started = std::time::Instant::now();
    let mut last: Option<Value> = None;
    let mut polls = 0u64;
    loop {
        polls += 1;
        let state = browser
            .eval(&expr)
            .await
            .ok()
            .and_then(|v| v.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()))
            .unwrap_or_else(|| json!({"found":false}));
        let found = state["found"].as_bool().unwrap_or(false);
        let visible = state["visible"].as_bool().unwrap_or(false);
        let enabled = state["enabled"].as_bool().unwrap_or(false);
        let rect = state.get("rect").cloned().unwrap_or(Value::Null);
        let ready = found && visible && enabled && rect.is_object();
        let stable = ready && last.as_ref() == Some(&rect);
        let waited = started.elapsed().as_millis() as u64;
        if state["syntax"].as_bool().unwrap_or(false) {
            return json!({"verdict":"refused","mode":"syntax","waitedMs":waited,"polls":polls,
                          "boundMs":ACTIONABILITY_BOUND_MS,"note":"the selector does not parse; the action's own resolver refuses it typed"});
        }
        if stable {
            return json!({"verdict":"settled","mode":"probed","waitedMs":waited,"polls":polls,
                          "boundMs":ACTIONABILITY_BOUND_MS,"visible":visible,"enabled":enabled,
                          "stable":true,"rect":rect});
        }
        last = if ready { Some(rect.clone()) } else { None };
        if waited >= ACTIONABILITY_BOUND_MS {
            return json!({"verdict":"timeout","mode":"probed","waitedMs":waited,"polls":polls,
                          "boundMs":ACTIONABILITY_BOUND_MS,"visible":visible,"enabled":enabled,
                          "stable":false,"lastRect":rect});
        }
        tokio::time::sleep(std::time::Duration::from_millis(ACTIONABILITY_POLL_MS)).await;
    }
}

/// Attach the actionability delta to a success reply's structuredContent.
/// The text block is left byte-identical (envelope contract G1/G2b).
fn with_actionability(mut reply: Value, a: &Value) -> Value {
    if let Some(obj) = reply
        .get_mut("structuredContent")
        .and_then(|v| v.as_object_mut())
    {
        obj.insert("actionability".to_string(), a.clone());
    }
    reply
}

fn click_preflight_ref(id: &str) -> (String, String) {
    let rid = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    let expr = format!(
        "((ref) => {{ const el = document.querySelector('[data-gladius-ref=\"' + ref + '\"]'); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; if (el.disabled) return 'GLADIUS_DISABLED'; const tag = (el.tagName || '').toUpperCase(); if (tag === 'OPTION') {{ const h0 = location.href; try {{ el.scrollIntoView({{block: 'center'}}); }} catch (e) {{}} try {{ el.click(); }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} try {{ return 'GLADIUS_FALLBACK:' + JSON.stringify({{h0: h0, h1: location.href, rs: document.readyState}}); }} catch (e) {{ return 'GLADIUS_FALLBACK'; }} }} try {{ el.scrollIntoView({{block: 'center'}}); }} catch (e) {{}} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return 'GLADIUS_POINT:' + JSON.stringify({{x: cx, y: cy, h0: location.href}}); }})({rid})"
    );
    (expr, rid)
}

fn click_preflight_sel(sel: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; if (el.disabled) return 'GLADIUS_DISABLED'; const tag = (el.tagName || '').toUpperCase(); if (tag === 'OPTION') {{ const h0 = location.href; try {{ el.scrollIntoView({{block: 'center'}}); }} catch (e) {{}} try {{ el.click(); }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} try {{ return 'GLADIUS_FALLBACK:' + JSON.stringify({{h0: h0, h1: location.href, rs: document.readyState}}); }} catch (e) {{ return 'GLADIUS_FALLBACK'; }} }} try {{ el.scrollIntoView({{block: 'center'}}); }} catch (e) {{}} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return 'GLADIUS_POINT:' + JSON.stringify({{x: cx, y: cy, h0: location.href}}); }})()"
    )
}

/// Ref-path preflights run as `Runtime.callFunctionOn` bodies with the
/// resolved element as `this`, so shadow-DOM refs act exactly like light-DOM
/// refs (no `document.querySelector` involved). The hit-test uses the
/// element's OWN root: `document.elementFromPoint` returns the shadow HOST
/// for a shadow child, which would read as occluded.
fn click_preflight_fn() -> &'static str {
    r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; if (el.disabled) return 'GLADIUS_DISABLED'; const tag = (el.tagName || '').toUpperCase(); if (tag === 'OPTION') { const h0 = location.href; try { el.scrollIntoView({block: 'center'}); } catch (e) {} try { el.click(); } catch (e) { return 'GLADIUS_HIDDEN'; } try { return 'GLADIUS_FALLBACK:' + JSON.stringify({h0: h0, h1: location.href, rs: document.readyState}); } catch (e) { return 'GLADIUS_FALLBACK'; } } try { el.scrollIntoView({block: 'center'}); } catch (e) {} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try { const root = el.getRootNode ? el.getRootNode() : document; hit = (root.elementFromPoint || document.elementFromPoint).call(root, cx, cy); } catch (e) { return 'GLADIUS_OCCLUDED'; } if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return 'GLADIUS_POINT:' + JSON.stringify({x: cx, y: cy, h0: location.href}); }"#
}

fn type_preflight_fn() -> &'static str {
    r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.readOnly) return 'GLADIUS_READONLY'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; const r = el.getBoundingClientRect(); if (!r || (r.width === 0 && r.height === 0)) return 'GLADIUS_HIDDEN'; const tag = (el.tagName || '').toUpperCase(); let kind = ''; if (tag === 'INPUT') { const ty = String(el.type || 'text').toLowerCase(); if (['text','search','url','tel','password','email','number'].includes(ty)) kind = 'TEXT'; else kind = 'NOTEDIT:' + ty; } else if (tag === 'TEXTAREA') kind = 'TEXT'; else if (el.isContentEditable) kind = 'CE'; else kind = 'NOTEDIT:tag:' + tag; if (kind.startsWith('NOTEDIT:')) return 'GLADIUS_NOTEDIT:' + kind.slice(8); try { el.focus(); } catch (e) { return 'GLADIUS_HIDDEN'; } try { if (kind === 'TEXT') { if (typeof el.select === 'function') el.select(); else if (typeof el.setSelectionRange === 'function') el.setSelectionRange(0, el.value.length); } else { const range = document.createRange(); range.selectNodeContents(el); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range); } } catch (e) { return 'GLADIUS_HIDDEN'; } return 'GLADIUS_READY'; }"#
}

fn hover_preflight_fn() -> &'static str {
    r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; try { el.scrollIntoView({block: 'center', inline: 'center'}); } catch (e) {} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try { const root = el.getRootNode ? el.getRootNode() : document; hit = (root.elementFromPoint || document.elementFromPoint).call(root, cx, cy); } catch (e) { return 'GLADIUS_OCCLUDED'; } if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return JSON.stringify({x: cx, y: cy}); }"#
}

fn select_preflight_fn(val_lit: &str, txt_lit: &str) -> String {
    let js = r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; if (!el || el.tagName !== 'SELECT') return 'GLADIUS_NOTSELECT'; const wantValue = __VAL__; const wantText = __TXT__; const opts = Array.from(el.options); let opt = null; if (wantValue !== null) { opt = opts.find(o => o.value === wantValue); } else { opt = opts.find(o => o.text === wantText || o.textContent === wantText); } if (!opt) return 'GLADIUS_NOOPT'; el.value = opt.value; el.selectedIndex = opts.indexOf(opt); el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true})); return 'GLADIUS_OK'; }"#;
    js.replace("__VAL__", val_lit).replace("__TXT__", txt_lit)
}

/// Dispatch a trusted main-frame pointer click at `(x, y)` (viewport CSS px)
/// via `Browser::mouse_click`, then build the `{h0, h1, rs}` settle state the
/// same shape the old in-page click reported (h0 from preflight, h1/rs read
/// live after dispatch). A CDP dispatch failure is a typed error at the call
/// site (never a silent no-op); the settle/dialog/`click_ok` path is unchanged.
async fn click_dispatch(
    browser: &Browser,
    x: f64,
    y: f64,
    h0: &str,
    hover_first: bool,
    button: &str,
    click_count: i64,
    cross_process_frame: bool,
) -> Result<String, ToolErr> {
    let sent = if cross_process_frame {
        browser
            .mouse_click_cross_process(x, y, button, click_count)
            .await
    } else {
        browser
            .mouse_click_button(x, y, button, click_count, hover_first)
            .await
    };
    sent.map_err(ToolErr::from)?;
    let live = browser.eval("[location.href, document.readyState]").await?;
    let h1 = live.get(0).and_then(|v| v.as_str()).unwrap_or(h0);
    let rs = live.get(1).and_then(|v| v.as_str()).unwrap_or("complete");
    let st = serde_json::json!({"h0": h0, "h1": h1, "rs": rs}).to_string();
    Ok(st)
}

async fn tool_click(
    browser: &Browser,
    element: &str,
    frame: Option<&str>,
    hover_first: bool,
    button: &str,
    click_count: i64,
) -> Result<Value, ToolErr> {
    browser.take_dialog().await;
    let fid = frame_id_ok("click", frame)?;

    if let Some(sel) = ax_sel_parse("click", element)? {
        let hit = ax_resolve(browser, "click", &sel, element, fid.as_deref()).await?;
        return ax_click(browser, element, &hit, fid.as_deref(), hover_first, button, click_count).await;
    }
    if let Some(f) = fid.as_deref() {
        if let Some(id) = parse_ref(element) {
            if browser.ref_frame(&id).await.as_deref() == Some(f) {
                return ref_click_frame(
                    browser,
                    element,
                    &id,
                    f,
                    hover_first,
                    button,
                    click_count,
                )
                .await;
            }
            return Err(ref_in_frame("click", element, f));
        }
        return tool_click_frame(browser, element, f, hover_first, button, click_count).await;
    }
    if let Some(id) = parse_ref(element) {
        let at_id = guard_ref(browser, "click", element, &id).await?;
        let v = match resolve_ref_object(browser, "click", &id).await? {
            Some(oid) => call_on_object(browser, &oid, click_preflight_fn(), vec![]).await?,
            None => {
                let (expr, _) = click_preflight_ref(&id);
                browser.eval(&expr).await?
            }
        };
        return match v.as_str() {
            Some(s) if s.starts_with("GLADIUS_FALLBACK:") => {
                click_settle(browser, &s["GLADIUS_FALLBACK:".len()..]).await;
                Ok(click_ok(browser).await)
            }
            Some("GLADIUS_FALLBACK") => {
                click_settle(browser, "").await;
                Ok(click_ok(browser).await)
            }
            Some(s) if s.starts_with("GLADIUS_POINT:") => {
                let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                    .map_err(|_| "click: no layout point".to_string())?;
                let x = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
                let y = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
                let h0 = pt
                    .get("h0")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let st = click_dispatch(browser, x, y, &h0, hover_first, button, click_count, false).await?;
                click_settle(browser, &st).await;
                Ok(click_ok(browser).await)
            }
            Some("GLADIUS_NOTFOUND") => Err(stale_gone("click", element, &id)),
            Some("GLADIUS_DISABLED") => Err(ToolErr::typed(
                format!("click: {at_id} is disabled"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "click", "ref": at_id}),
            )),
            Some("GLADIUS_OCCLUDED") => Err(ToolErr::typed(
                format!("click: {at_id} is occluded by another element (no click dispatched)"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "click", "ref": at_id}),
            )),
            _ => Err(ToolErr::typed(
                format!("click: {at_id} not visible or not clickable"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "click", "ref": at_id}),
            )),
        };
    }
    let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let probe = format!("(!!document.querySelector({sel}))");
            if let Ok(Ok(Value::Bool(true))) =
                tokio::time::timeout(remaining, browser.eval(&probe)).await
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    let expr = click_preflight_sel(&sel);
    let v = browser.eval(&expr).await?;
    match v.as_str() {
        Some(s) if s.starts_with("GLADIUS_FALLBACK:") => {
            click_settle(browser, &s["GLADIUS_FALLBACK:".len()..]).await;
            Ok(click_ok(browser).await)
        }
        Some("GLADIUS_FALLBACK") => {
            click_settle(browser, "").await;
            Ok(click_ok(browser).await)
        }
        Some(s) if s.starts_with("GLADIUS_POINT:") => {
            let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                .map_err(|_| "click: no layout point".to_string())?;
            let x = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let y = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let h0 = pt
                .get("h0")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let st = click_dispatch(browser, x, y, &h0, hover_first, button, click_count, false).await?;
            click_settle(browser, &st).await;
            Ok(click_ok(browser).await)
        }
        Some("GLADIUS_NOTFOUND") => {
            if closed_shadow_hit(browser, element).await {
                return Err(shadow_closed("click", element));
            }
            Err(ToolErr::typed(
                format!("click: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "click", "element": element}),
            ))
        }
        Some("GLADIUS_DISABLED") => Err(ToolErr::typed(
            format!("click: element is disabled: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "click", "element": element}),
        )),
        Some("GLADIUS_OCCLUDED") => Err(ToolErr::typed(
            format!(
                "click: element is occluded by another element (no click dispatched): {element}"
            ),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "click", "element": element}),
        )),
        _ => Err(ToolErr::typed(
            format!("click: element not visible or not clickable: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "click", "element": element}),
        )),
    }
}

/// Click an AX-selector target: the same visibility/occlusion preflight the
/// ref path runs, on the node the selector resolved (not a DOM lookup). The
/// leaf point is the element's own viewport centre; with a frame named it
/// maps through the owner rect chain exactly like the CSS frame path, and the
/// pointer always dispatches at main-viewport coordinates.
async fn ax_click(
    browser: &Browser,
    element: &str,
    hit: &AxHit,
    fid: Option<&str>,
    hover_first: bool,
    button: &str,
    click_count: i64,
) -> Result<Value, ToolErr> {
    let oid = ax_object(browser, "click", element, hit).await?;
    let v = call_on_object_in(browser, hit.session.as_deref(), &oid, click_preflight_fn(), vec![]).await?;
    let (x, y, h0) = match v.as_str() {
        Some(s) if s.starts_with("GLADIUS_FALLBACK:") => {
            click_settle(browser, &s["GLADIUS_FALLBACK:".len()..]).await;
            return Ok(with_resolved(click_ok(browser).await, element, hit));
        }
        Some("GLADIUS_FALLBACK") => {
            click_settle(browser, "").await;
            return Ok(with_resolved(click_ok(browser).await, element, hit));
        }
        Some(s) if s.starts_with("GLADIUS_POINT:") => {
            let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                .map_err(|_| "click: no layout point".to_string())?;
            let x = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let y = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let h0 = pt
                .get("h0")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            (x, y, h0)
        }
        other => {
            return Err(ax_status_err("click", element, hit, other).unwrap_or_else(|| {
                ToolErr::typed(
                    format!("click: element not visible or not clickable: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(),
                           "tool": "click", "element": element, "ref": hit.at, "role": hit.role, "name": hit.name}),
                )
            }))
        }
    };
    let (x, y, h0) = match fid {
        Some(f) => {
            let (mx, my) = frame_owner_hit_ok(browser, "click", element, f, x, y).await?;
            let h0 = browser
                .eval("window.location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            (mx, my, h0)
        }
        None => (x, y, h0),
    };
    let st = click_dispatch(
        browser,
        x,
        y,
        &h0,
        hover_first,
        button,
        click_count,
        hit.session.is_some(),
    )
    .await?;
    click_settle(browser, &st).await;
    Ok(with_resolved(click_ok(browser).await, element, hit))
}

/// Hover an AX-selector target: preflight on the resolved node, then ONE
/// trusted mouseMoved at the (frame-mapped) centre.
async fn ax_hover(
    browser: &Browser,
    element: &str,
    hit: &AxHit,
    fid: Option<&str>,
) -> Result<Value, ToolErr> {
    let oid = ax_object(browser, "hover", element, hit).await?;
    let v = call_on_object_in(browser, hit.session.as_deref(), &oid, hover_preflight_fn(), vec![]).await?;
    let point = v
        .as_str()
        .filter(|s| s.starts_with('{'))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|p| p.get("x").is_some() && p.get("y").is_some());
    let Some(point) = point else {
        return Err(ax_status_err("hover", element, hit, v.as_str()).unwrap_or_else(|| {
            ToolErr::typed(
                format!("hover: element not visible or not hoverable: {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(),
                       "tool": "hover", "element": element, "ref": hit.at, "role": hit.role, "name": hit.name}),
            )
        }));
    };
    let lx = point.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let ly = point.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let (mx, my) = match fid {
        Some(f) => frame_owner_hit_ok(browser, "hover", element, f, lx, ly).await?,
        None => (lx, ly),
    };
    browser.mouse_move(mx, my).await?;
    Ok(with_resolved(
        ok_text(json!({"ok": true}).to_string()),
        element,
        hit,
    ))
}

/// Type into an AX-selector target: focus/select-all runs on the resolved
/// node (so a control inside a frame focuses in its own realm) and the text
/// goes through the same `Input.insertText` as every other path.
async fn ax_type(
    browser: &Browser,
    element: &str,
    hit: &AxHit,
    text: &str,
    submit: bool,
) -> Result<Value, ToolErr> {
    let oid = ax_object(browser, "type", element, hit).await?;
    let v = call_on_object_in(browser, hit.session.as_deref(), &oid, type_preflight_fn(), vec![]).await?;
    if v.as_str() != Some("GLADIUS_READY") {
        return Err(ax_status_err("type", element, hit, v.as_str()).unwrap_or_else(|| {
            ToolErr::typed(
                format!("type: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                       "tool": "type", "element": element, "ref": hit.at, "role": hit.role, "name": hit.name}),
            )
        }));
    }
    browser.insert_text(text).await.map_err(|e| {
        type_not_editable(
            "type",
            &format!("insertText failed: {e}"),
            &hit.ctx("type", element),
        )
    })?;
    let value_length = call_on_object(browser, &oid, VALUE_LEN_FN, vec![])
        .await
        .ok()
        .and_then(|v| v.as_u64());
    if submit {
        submit_enter(browser).await?;
    }
    Ok(with_resolved(type_reply(text, value_length), element, hit))
}

/// Upload to an AX-selector target: the same file-input probe, then
/// `DOM.setFileInputFiles` by backend node id (the identity a ref records),
/// so a file input inside a shadow root or a frame resolves the same way.
async fn ax_upload(
    browser: &Browser,
    element: &str,
    hit: &AxHit,
    paths: &[String],
) -> Result<Value, ToolErr> {
    let oid = ax_object(browser, "upload", element, hit).await?;
    let probe = r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.tagName !== 'INPUT' || (el.type || '').toLowerCase() !== 'file') return 'GLADIUS_NOTFILE'; return 'GLADIUS_OK'; }"#;
    let v = call_on_object_in(browser, hit.session.as_deref(), &oid, probe, vec![]).await?;
    if v.as_str() != Some("GLADIUS_OK") {
        return Err(ax_status_err("upload", element, hit, v.as_str()).unwrap_or_else(|| {
            ToolErr::typed(
                format!("upload: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                       "tool": "upload", "element": element, "ref": hit.at, "role": hit.role, "name": hit.name}),
            )
        }));
    }
    browser
        .command(
            "DOM.setFileInputFiles",
            json!({"backendNodeId": hit.backend, "files": paths}),
            true,
        )
        .await
        .map_err(|e| format!("upload: {e}"))?;
    Ok(with_resolved(
        ok_text(json!({"ok": true, "files": paths.len()}).to_string()),
        element,
        hit,
    ))
}

/// Select an option on an AX-selector target: the same option matcher the
/// ref path runs, on the resolved node.
async fn ax_select(
    browser: &Browser,
    element: &str,
    hit: &AxHit,
    value: Option<&str>,
    text: Option<&str>,
) -> Result<Value, ToolErr> {
    let val_lit = match value {
        Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
        None => "null".to_string(),
    };
    let txt_lit = match text {
        Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
        None => "null".to_string(),
    };
    let oid = ax_object(browser, "select", element, hit).await?;
    let v = call_on_object_in(
        browser,
        hit.session.as_deref(),
        &oid,
        &select_preflight_fn(&val_lit, &txt_lit),
        vec![],
    )
    .await?;
    match v.as_str() {
        Some("GLADIUS_OK") => Ok(with_resolved(
            ok_text(json!({"ok": true}).to_string()),
            element,
            hit,
        )),
        Some("GLADIUS_NOOPT") => {
            let want = value.or(text).unwrap_or("");
            Err(ToolErr::typed(
                format!("select: option not found: {want}"),
                json!({"code": ToolCode::OptionNotFound.as_str(), "reason": ToolReason::NoMatch.as_str(),
                       "tool": "select", "want": want, "element": element}),
            ))
        }
        other => Err(ax_status_err("select", element, hit, other).unwrap_or_else(|| {
            ToolErr::typed(
                format!("select: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(),
                       "tool": "select", "element": element, "ref": hit.at, "role": hit.role, "name": hit.name}),
            )
        })),
    }
}

/// G4b frame-scoped click: the selector preflight (resolve, visibility,
/// disabled, scrollIntoView, `<option>` fallback, elementFromPoint hit-test)
/// runs in the FRAME realm via `eval_in_frame` (same trusted logic as the
/// main frame, but inside the iframe). The leaf center (frame-viewport CSS px)
/// is mapped to main-frame viewport CSS px through the owner rect chain
/// (`frame_chain_points`: owner left/top + padding + border per ancestor, JS rect
/// chain, never getContentQuads), a parent-side overlay guard hit-tests the
/// final point, and the trusted CDP pointer sequence dispatches on the page
/// session at main-frame viewport px. OOPIF/unmapped frames are typed errors.
async fn tool_click_frame(
    browser: &Browser,
    element: &str,
    fid: &str,
    hover_first: bool,
    button: &str,
    click_count: i64,
) -> Result<Value, ToolErr> {
    let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let probe = format!("(!!document.querySelector({sel}))");
            if let Ok(Ok(Value::Bool(true))) =
                tokio::time::timeout(remaining, frame_realm_eval(browser, "click", fid, &probe))
                    .await
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    let expr = click_preflight_sel(&sel);
    let v = frame_realm_eval(browser, "click", fid, &expr).await?;
    match v.as_str() {
        Some(s) if s.starts_with("GLADIUS_FALLBACK:") => {
            click_settle(browser, "").await;
            Ok(click_ok(browser).await)
        }
        Some("GLADIUS_FALLBACK") => {
            click_settle(browser, "").await;
            Ok(click_ok(browser).await)
        }
        Some(s) if s.starts_with("GLADIUS_POINT:") => {
            let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                .map_err(|_| "click: no layout point".to_string())?;
            let lx = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let ly = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
            let h0 = browser
                .eval("window.location.href")
                .await
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let (mx, my) = frame_owner_hit_ok(browser, "click", element, fid, lx, ly).await?;
            let cross = browser.oop_session_for_frame(fid).await.is_some();
            let st = click_dispatch(browser, mx, my, &h0, hover_first, button, click_count, cross)
                .await?;
            click_settle(browser, &st).await;
            Ok(click_ok(browser).await)
        }
        Some("GLADIUS_NOTFOUND") => Err(ToolErr::typed(
            format!("click: element not found: {element}"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "click", "element": element, "frame": fid}),
        )),
        Some("GLADIUS_DISABLED") => Err(ToolErr::typed(
            format!("click: element is disabled: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "click", "element": element, "frame": fid}),
        )),
        Some("GLADIUS_OCCLUDED") => Err(ToolErr::typed(
            format!(
                "click: element is occluded by another element (no click dispatched): {element}"
            ),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "click", "element": element, "frame": fid}),
        )),
        _ => Err(ToolErr::typed(
            format!("click: element not visible or not clickable: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "click", "element": element, "frame": fid}),
        )),
    }
}

/// Preflight for a DRAG endpoint: the same liveness, visibility, disabled,
/// box and occlusion checks a click runs, but it NEVER scrolls. A drag has two
/// endpoints in one viewport, and scrolling one into view moves the other, so
/// an endpoint that is off screen is a typed refusal that says to bring it on
/// screen first - never a silent scroll that makes the pair disagree.
fn drag_preflight_fn() -> &'static str {
    r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; if (el.disabled) return 'GLADIUS_DISABLED'; const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; if (cx < 0 || cy < 0 || cx > window.innerWidth || cy > window.innerHeight) return 'GLADIUS_OFFSCREEN'; let hit = null; try { const root = el.getRootNode ? el.getRootNode() : document; hit = (root.elementFromPoint || document.elementFromPoint).call(root, cx, cy); } catch (e) { return 'GLADIUS_OCCLUDED'; } if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return 'GLADIUS_POINT:' + JSON.stringify({x: cx, y: cy}); }"#
}

/// The same drag preflight, by CSS selector, for the main realm and a frame
/// realm (the element is looked up in the realm the expression runs in).
fn drag_preflight_sel(sel: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; if (el.disabled) return 'GLADIUS_DISABLED'; const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; if (cx < 0 || cy < 0 || cx > window.innerWidth || cy > window.innerHeight) return 'GLADIUS_OFFSCREEN'; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return 'GLADIUS_POINT:' + JSON.stringify({{x: cx, y: cy}}); }})()"
    )
}

/// Turn a click-preflight result into a viewport point, or the same typed
/// refusal `click` would give. `<option>` has no pointer box, so it cannot be
/// an endpoint of a drag: that is a typed refusal, never a guess.
fn preflight_point(v: &Value, tool: &str, element: &str) -> Result<(f64, f64), ToolErr> {
    match v.as_str() {
        Some(s) if s.starts_with("GLADIUS_POINT:") => {
            let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                .map_err(|_| ToolErr::fallback(format!("{tool}: no layout point")))?;
            Ok((
                pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0),
                pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0),
            ))
        }
        Some(s) if s.starts_with("GLADIUS_FALLBACK") => Err(ToolErr::typed(
            format!("{tool}: element has no pointer box (a native <option> cannot be dragged): {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": tool, "element": element}),
        )),
        Some("GLADIUS_NOTFOUND") => Err(ToolErr::typed(
            format!("{tool}: element not found: {element}"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": tool, "element": element}),
        )),
        Some("GLADIUS_DISABLED") => Err(ToolErr::typed(
            format!("{tool}: element is disabled: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": tool, "element": element}),
        )),
        Some("GLADIUS_OCCLUDED") => Err(ToolErr::typed(
            format!("{tool}: element is occluded by another element (no {tool} dispatched): {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": tool, "element": element}),
        )),
        Some("GLADIUS_OFFSCREEN") => Err(ToolErr::typed(
            format!("{tool}: element is not inside the viewport; scroll it into view first (browser_wheel, or evaluate scrollIntoView) - a drag never scrolls, because scrolling one endpoint moves the other: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": tool, "element": element, "offscreen": true}),
        )),
        _ => Err(ToolErr::typed(
            format!("{tool}: element not visible or not actionable: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": tool, "element": element}),
        )),
    }
}

/// Resolve one drag endpoint to a MAIN-VIEWPORT point, running the same
/// visibility/occlusion preflight `click` runs: an endpoint a click could not
/// land on is a typed refusal here too, and nothing is dispatched. Ref, AX
/// selector and CSS selector are all accepted; with `fid` the endpoint is
/// resolved inside that frame's realm and its point mapped through the owner
/// chain exactly like a frame click.
async fn drag_point(
    browser: &Browser,
    tool: &str,
    element: &str,
    fid: Option<&str>,
) -> Result<(f64, f64), ToolErr> {
    if let Some(sel) = ax_sel_parse(tool, element)? {
        let hit = ax_resolve(browser, tool, &sel, element, fid).await?;
        let oid = ax_object(browser, tool, element, &hit).await?;
        let v = call_on_object_in(browser, hit.session.as_deref(), &oid, drag_preflight_fn(), vec![]).await?;
        let (lx, ly) = preflight_point(&v, tool, element)?;
        return match fid {
            Some(f) => frame_owner_hit_ok(browser, tool, element, f, lx, ly).await,
            None => Ok((lx, ly)),
        };
    }
    if let Some(f) = fid {
        if parse_ref(element).is_some() {
            return Err(ref_in_frame(tool, element, f));
        }
        let sel = serde_json::to_string(element).map_err(|e| ToolErr::fallback(e.to_string()))?;
        let v = frame_realm_eval(browser, tool, f, &drag_preflight_sel(&sel)).await?;
        let (lx, ly) = preflight_point(&v, tool, element)?;
        return frame_owner_hit_ok(browser, tool, element, f, lx, ly).await;
    }
    if let Some(id) = parse_ref(element) {
        let _at = guard_ref(browser, tool, element, &id).await?;
        let session = browser.ref_session(&id).await;
        let v = match resolve_ref_object(browser, tool, &id).await? {
            Some(oid) => call_on_object_in(browser, session.as_deref(), &oid, drag_preflight_fn(), vec![]).await?,
            None => {
                let (expr, _) = click_preflight_ref(&id);
                browser.eval(&expr).await?
            }
        };
        return preflight_point(&v, tool, element);
    }
    let sel = serde_json::to_string(element).map_err(|e| ToolErr::fallback(e.to_string()))?;
    let v = browser.eval(&drag_preflight_sel(&sel)).await?;
    preflight_point(&v, tool, element)
}

/// Refuse an explicit pointer point outside the live viewport: an event there
/// lands on nothing, so accepting it would report a success that did nothing.
async fn point_in_viewport(
    browser: &Browser,
    tool: &str,
    arg: &str,
    x: f64,
    y: f64,
) -> Result<(), ToolErr> {
    let (w, h, _) = browser.viewport_readback().await.map_err(ToolErr::from)?;
    if x >= 0.0 && y >= 0.0 && x <= w as f64 && y <= h as f64 {
        return Ok(());
    }
    Err(ToolErr::typed(
        format!("{tool}: '{arg}' ({x}, {y}) is outside the {w}x{h} viewport; a pointer event there lands on nothing"),
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(),
               "tool": tool, "arg": arg, "viewport": {"width": w, "height": h}}),
    ))
}

/// Read the document scroll offsets (the effect a wheel is supposed to have).
async fn scroll_offsets(browser: &Browser) -> Option<(f64, f64)> {
    let v = browser.eval("[window.scrollX, window.scrollY]").await.ok()?;
    Some((
        v.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0),
        v.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0),
    ))
}

/// Wheel: one trusted `mouseWheel` at the point (viewport centre by default),
/// then read the offsets back until they settle. `moved` is measured, not
/// claimed: a wheel over a non-scrolling surface reports the same offsets.
async fn tool_wheel(
    browser: &Browser,
    dx: f64,
    dy: f64,
    point: Option<(f64, f64)>,
) -> Result<Value, ToolErr> {
    let (x, y) = match point {
        Some(p) => p,
        None => {
            let (w, h, _) = browser.viewport_readback().await.map_err(ToolErr::from)?;
            (w as f64 / 2.0, h as f64 / 2.0)
        }
    };
    if point.is_some() {
        point_in_viewport(browser, "wheel", "x/y", x, y).await?;
    }
    let before = scroll_offsets(browser).await.unwrap_or((0.0, 0.0));
    browser
        .mouse_wheel(x, y, dx, dy)
        .await
        .map_err(ToolErr::from)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
    let mut last = before;
    let mut stable = 0u32;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cur = scroll_offsets(browser).await.unwrap_or(last);
        if cur == last {
            stable += 1;
            if stable >= 2 {
                break;
            }
        } else {
            stable = 0;
        }
        last = cur;
        if std::time::Instant::now() >= deadline {
            break;
        }
    }
    let after = last;
    Ok(ok_text(
        json!({
            "ok": true,
            "moved": after != before,
            "scroll": {"x": after.0, "y": after.1},
            "at": {"x": x, "y": y},
            "deltaX": dx,
            "deltaY": dy,
        })
        .to_string(),
    ))
}

/// Ordered form batch. Each field becomes one call to the same trusted
/// type/select/click tool a human agent would use; a failed or timed-out step
/// is recorded with an error and the batch continues. The top-level verdict is
/// pass only when every step is ok and a requested submit also succeeded.
static EVIDENCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write one evidence file and report its real path and byte count. A write
/// failure is typed in the entry rather than turning a failing step into a
/// failing batch.
fn write_evidence(dir: &std::path::Path, name: &str, bytes: &[u8]) -> Value {
    let path = dir.join(name);
    match std::fs::write(&path, bytes) {
        Ok(()) => json!({"path": path.to_string_lossy(), "bytes": bytes.len()}),
        Err(e) => json!({"path": Value::Null, "bytes": 0, "error": e.to_string()}),
    }
}

/// The three pieces of evidence at a failing step: a screenshot of the page
/// as it stood, the console log and the network log. Each is a real file with
/// its path and byte count, never a promise to capture one later.
async fn step_evidence(browser: &Browser, field: &str) -> Value {
    let dir = std::env::temp_dir().join("hand-evidence");
    let _ = std::fs::create_dir_all(&dir);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = EVIDENCE_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let safe: String = field
        .chars()
        .take(24)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let stamp = format!("{nanos}-{seq}-{safe}");
    let shot = crate::capture::capture(browser, &crate::capture::Options::default())
        .await
        .map(|c| c.bytes)
        .unwrap_or_default();
    let console = browser.console_get(100).await.to_string().into_bytes();
    let network = browser.network_get(None, 100).await.to_string().into_bytes();
    json!({
        "screenshot": write_evidence(&dir, &format!("{stamp}-shot.png"), &shot),
        "console": write_evidence(&dir, &format!("{stamp}-console.log"), &console),
        "network": write_evidence(&dir, &format!("{stamp}-network.log"), &network),
    })
}

/// Ordered browser batch with per-step evidence. Each step dispatches through
/// the same `call` a standalone tool uses, so gating, envelopes and typed
/// errors are shared rather than reimplemented here. The boxed future breaks
/// the `call -> tool_run -> call` async recursion.
async fn tool_run(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown("run", args, &["steps", "continueOnError", "timeoutMs"])?;
    let steps = match args.get("steps").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() && a.len() <= 64 => a,
        _ => {
            return Err(ToolErr::typed(
                "run: 'steps' must hold 1 to 64 steps".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "run", "arg": "steps"}),
            ))
        }
    };
    let continue_on_error = args
        .get("continueOnError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let timeout_ms = match args.get("timeoutMs") {
        None => 30_000u64,
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => n,
            _ => {
                return Err(ToolErr::typed(
                    "run: 'timeoutMs' must be an integer >= 1".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "run", "arg": "timeoutMs"}),
                ))
            }
        },
    };
    let started = std::time::Instant::now();
    let mut results = Vec::with_capacity(steps.len());
    let mut failed = 0usize;
    for (i, step) in steps.iter().enumerate() {
        let tool = step.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        if !RUN_TOOLS.contains(&tool) {
            return Err(ToolErr::typed(
                format!("run: step {} names a tool this batch cannot dispatch: '{tool}'", i + 1),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownTool.as_str(), "tool": "run", "step": i + 1, "arg": "tool"}),
            ));
        }
        let step_args = match step.get("args") {
            None => json!({}),
            Some(v) if v.is_object() => v.clone(),
            Some(_) => {
                return Err(ToolErr::typed(
                    format!("run: step {} 'args' must be an object", i + 1),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "run", "step": i + 1, "arg": "args"}),
                ))
            }
        };
        // A per-step timeout drops the future, so a freeze applied inside a
        // batch could never be restored; refuse it before the step starts.
        if tool == "screenshot"
            && step_args.get("freeze").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            return Err(ToolErr::typed(
                "run: 'freeze' is refused in a screenshot step (a timeout would leave the page frozen); call browser_screenshot directly"
                    .to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "run", "step": i + 1, "arg": "freeze"}),
            ));
        }
        let begun = std::time::Instant::now();
        let reply = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            Box::pin(call(tool, &step_args, browser)),
        )
        .await;
        let ms = begun.elapsed().as_millis() as i64;
        let entry = match reply {
            Ok(env) => {
                let is_error = env.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                let evidence = env.get("structuredContent").cloned().unwrap_or(Value::Null);
                if is_error {
                    failed += 1;
                    json!({"index": i + 1, "tool": tool, "status": "error", "ms": ms, "error": evidence})
                } else {
                    json!({"index": i + 1, "tool": tool, "status": "ok", "ms": ms, "result": evidence})
                }
            }
            Err(_) => {
                failed += 1;
                json!({"index": i + 1, "tool": tool, "status": "error", "ms": ms,
                       "error": {"code": ToolCode::Timeout.as_str(), "reason": ToolReason::BudgetExceeded.as_str(), "tool": tool}})
            }
        };
        let stopped = entry["status"] == "error" && !continue_on_error;
        results.push(entry);
        if stopped {
            break;
        }
    }
    let ok = failed == 0 && results.len() == steps.len();
    Ok(ok_text(
        json!({"ok": ok, "steps": results, "failed": failed,
               "ms": started.elapsed().as_millis() as i64})
            .to_string(),
    ))
}

async fn tool_form(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown("form", args, &["fields", "submit", "timeoutMs"])?;
    let fields = match args.get("fields").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            return Err(ToolErr::typed(
                "form: 'fields' must be a non-empty array".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "form", "arg": "fields"}),
            ))
        }
    };
    let timeout = args
        .get("timeoutMs")
        .and_then(|v| v.as_u64())
        .map(std::time::Duration::from_millis);

    let mut steps: Vec<Value> = Vec::new();
    let mut all_ok = true;
    for f in fields {
        let label = f.get("field").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let action = f.get("action").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let value = f.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let (tool_name, call_args) = match action.as_str() {
            "type" => ("type", json!({"element": label, "text": value})),
            "select" => ("select", json!({"element": label, "value": value})),
            "check" | "click" => ("click", json!({"element": label})),
            other => {
                let evidence = Box::pin(step_evidence(browser, &label)).await;
                steps.push(json!({"field": label, "status": "error",
                                  "error": format!("unknown action '{other}'"),
                                  "evidence": evidence}));
                all_ok = false;
                continue;
            }
        };
        let fut = Box::pin(call(tool_name, &call_args, browser));
        let resp = match timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(r) => r,
                Err(_) => {
                    let evidence = Box::pin(step_evidence(browser, &label)).await;
                    steps.push(json!({"field": label, "status": "error",
                                      "error": format!("timeout after {}ms", d.as_millis()),
                                      "evidence": evidence}));
                    all_ok = false;
                    continue;
                }
            },
            None => fut.await,
        };
        if resp.get("isError").and_then(|v| v.as_bool()) == Some(true) {
            let err = resp
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|b| b.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("action failed")
                .to_string();
            let evidence = Box::pin(step_evidence(browser, &label)).await;
            steps.push(json!({"field": label, "status": "error", "error": err,
                              "evidence": evidence}));
            all_ok = false;
        } else {
            steps.push(json!({"field": label, "status": "ok"}));
        }
    }

    let mut submitted = false;
    if let Some(sel) = args.get("submit").and_then(|v| v.as_str()) {
        let resp = Box::pin(call("click", &json!({"element": sel}), browser)).await;
        submitted = resp.get("isError").and_then(|v| v.as_bool()) != Some(true);
    }
    let verdict = if all_ok && (args.get("submit").is_none() || submitted) {
        "pass"
    } else {
        "fail"
    };
    Ok(ok_text(
        json!({"steps": steps, "submitted": submitted, "verdict": verdict}).to_string(),
    ))
}

/// Drag: resolve both endpoints (with the click preflight's occlusion guard),
/// then dispatch the trusted press/move/release sequence. Without a payload
/// every event is `Input.dispatchMouseEvent` and no `DataTransfer` is touched.
/// With one, CDP drag interception carries the mime items into a trusted
/// `drop`; interception is restored on success, error and cancellation.
async fn tool_drag(
    browser: &Browser,
    from: Option<&str>,
    to: Option<&str>,
    from_point: Option<(f64, f64)>,
    to_point: Option<(f64, f64)>,
    steps: usize,
    frame: Option<&str>,
    data_transfer: Option<&Value>,
) -> Result<Value, ToolErr> {
    let fid = frame_id_ok("drag", frame)?;
    let from_pt = match from_point {
        Some(p) => {
            point_in_viewport(browser, "drag", "fromPoint", p.0, p.1).await?;
            p
        }
        None => drag_point(browser, "drag", from.unwrap_or(""), fid.as_deref()).await?,
    };
    let to_pt = match to_point {
        Some(p) => {
            point_in_viewport(browser, "drag", "toPoint", p.0, p.1).await?;
            p
        }
        None => drag_point(browser, "drag", to.unwrap_or(""), fid.as_deref()).await?,
    };
    let (events, drag_events, echo) = match data_transfer {
        Some(dt) => {
            let obj = dt.as_object().ok_or_else(|| {
                ToolErr::typed(
                    "drag: 'dataTransfer' must be an object of mime type -> string".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": "dataTransfer"}),
                )
            })?;
            let mut items = Vec::new();
            for (k, v) in obj {
                let s = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(ToolErr::typed(
                            format!("drag: dataTransfer['{k}'] must be a string"),
                            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "drag", "arg": "dataTransfer"}),
                        ))
                    }
                };
                items.push(json!({"mimeType": k, "data": s, "title": "", "baseURL": ""}));
            }
            let drag_data = json!({"items": items, "files": [], "dragOperationsMask": 1});
            let guard = browser
                .set_drag_intercept(true)
                .await
                .map_err(ToolErr::from)?;
            let dispatched = browser
                .mouse_drag_with_data(from_pt, to_pt, steps, drag_data)
                .await;
            let restore = guard.disable().await;
            let (e, d) = dispatched.map_err(ToolErr::from)?;
            restore.map_err(ToolErr::from)?;
            (e, d, dt.clone())
        }
        None => (
            browser
                .mouse_drag(from_pt, to_pt, steps)
                .await
                .map_err(ToolErr::from)?,
            0,
            Value::Null,
        ),
    };
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let mut out = json!({
        "ok": true,
        "from": {"x": from_pt.0, "y": from_pt.1},
        "to": {"x": to_pt.0, "y": to_pt.1},
        "events": events,
        "steps": steps,
        "dragEvents": drag_events,
    });
    if !echo.is_null() {
        out["dataTransfer"] = echo;
    }
    Ok(ok_text(out.to_string()))
}

/// Click a ref that a frame-scoped snapshot minted, inside that frame. The
/// node is resolved through its recorded session (an OOPIF's own session, or
/// the page session for a same-process frame), the preflight runs on that
/// node, and the point is mapped through the owner chain to main-viewport
/// coordinates before dispatch - exactly the frame CSS path, on a node
/// identity instead of a selector.
async fn ref_click_frame(
    browser: &Browser,
    element: &str,
    id: &str,
    fid: &str,
    hover_first: bool,
    button: &str,
    click_count: i64,
) -> Result<Value, ToolErr> {
    let at_id = guard_ref(browser, "click", element, id).await?;
    let session = browser.ref_session(id).await;
    let v = match resolve_ref_object(browser, "click", id).await? {
        Some(oid) => {
            call_on_object_in(browser, session.as_deref(), &oid, click_preflight_fn(), vec![]).await?
        }
        None => {
            let (expr, _) = click_preflight_ref(id);
            frame_realm_eval(browser, "click", fid, &expr).await?
        }
    };
    let (lx, ly) = match v.as_str() {
        Some("GLADIUS_FALLBACK") | Some("GLADIUS_FALLBACK:") => {
            click_settle(browser, "").await;
            return Ok(click_ok(browser).await);
        }
        Some(s) if s.starts_with("GLADIUS_FALLBACK:") => {
            click_settle(browser, "").await;
            return Ok(click_ok(browser).await);
        }
        Some(s) if s.starts_with("GLADIUS_POINT:") => {
            let pt: Value = serde_json::from_str(&s["GLADIUS_POINT:".len()..])
                .map_err(|_| ToolErr::fallback("click: no layout point".to_string()))?;
            (
                pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0),
                pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0),
            )
        }
        Some("GLADIUS_NOTFOUND") => return Err(stale_gone("click", element, id)),
        Some("GLADIUS_DISABLED") => {
            return Err(ToolErr::typed(
                format!("click: {at_id} is disabled"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "click", "ref": at_id, "frame": fid}),
            ))
        }
        Some("GLADIUS_OCCLUDED") => {
            return Err(ToolErr::typed(
                format!("click: {at_id} is occluded by another element (no click dispatched)"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "click", "ref": at_id, "frame": fid}),
            ))
        }
        _ => {
            return Err(ToolErr::typed(
                format!("click: {at_id} not visible or not clickable"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "click", "ref": at_id, "frame": fid}),
            ))
        }
    };
    let (mx, my) = frame_owner_hit_ok(browser, "click", element, fid, lx, ly).await?;
    let h0 = browser
        .eval("window.location.href")
        .await
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    let st = click_dispatch(
        browser,
        mx,
        my,
        &h0,
        hover_first,
        button,
        click_count,
        session.is_some(),
    )
    .await?;
    click_settle(browser, &st).await;
    Ok(click_ok(browser).await)
}

/// Settle a click that navigates (measured H2H form-02: submit click
/// returned before the GET committed, evaluate read the OLD page while both
/// refs settled on the navigation). `state` is the JSON tail of the click
/// eval (`{h0, h1, rs}` observed synchronously in-page right after dispatch;
/// "" when the eval predates this reporting).
///
/// Cost: zero extra round-trips when the click already shows flight
/// (h1 != h0 or not complete -> straight to the event-driven load wait);
/// otherwise a SHORT bounded quiet watch (120 ms at 10 ms cadence) for a
/// late commit (measured: the selenium GET commits ~100 ms after dispatch).
/// Clicks that never navigate (tab-01's new-tab, dlg-01's alert, plain
/// buttons) pay only the quiet window instead of the old 1 s watch, and
/// return on the old page, unchanged.
async fn click_settle(browser: &Browser, state: &str) {
    let st: Value = serde_json::from_str(state).unwrap_or(Value::Null);
    let h0 = st.get("h0").and_then(|v| v.as_str()).unwrap_or("");
    let h1 = st.get("h1").and_then(|v| v.as_str()).unwrap_or(h0);
    let rs = st.get("rs").and_then(|v| v.as_str()).unwrap_or("complete");
    if h1 != h0 || rs != "complete" {
        browser
            .wait_for_load(std::time::Duration::from_secs(10))
            .await;
        return;
    }
    if h0.is_empty() {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(120);
    loop {
        if browser.last_dialog().await.is_some() {
            browser
                .wait_for_load(std::time::Duration::from_secs(10))
                .await;
            return;
        }
        if let Ok(v) = browser.eval("[location.href, document.readyState]").await {
            let href = v.get(0).and_then(|x| x.as_str()).unwrap_or(h0);
            let cur = v.get(1).and_then(|x| x.as_str()).unwrap_or("complete");
            if href != h0 || cur != "complete" {
                browser
                    .wait_for_load(std::time::Duration::from_secs(10))
                    .await;
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Success envelope for a click. When the click opened a JS dialog, the
/// reader auto-accepted it (the page is unblocked) and the outcome carries
/// a typed dialog report with the dialog's own message text - never a
/// silent ok, never a hang.
async fn click_ok(browser: &Browser) -> Value {
    match browser.last_dialog().await {
        Some(d) => {
            let verb = if d.accepted { "accepted" } else { "dismissed" };
            ok_text(
            json!({"ok": true, "dialog": {"type": d.dtype, "message": d.message, "url": d.url},
                   "note": format!("dialog auto-handled: {} {:?} ({verb}, page unblocked)", d.dtype, d.message)})
                .to_string(),
            )
        }
        None => ok_text(json!({"ok": true}).to_string()),
    }
}

/// Main-frame `type` preflight: resolve, visibility/disabled/readOnly checks,
/// editable-kind gate, then focus plus select-all so `Input.insertText`
/// REPLACES. Text inputs use `el.focus()` plus `el.select()`; contenteditable
/// uses `el.focus()` plus select-all of the element's OWN contents
/// (`Range.selectNodeContents`), so insertText replaces text WITHIN the
/// editable element while siblings outside it stay untouched.
///
/// Non-editable targets (file inputs, date/time-like and other non-text input
/// types, non-editable elements) return `GLADIUS_NOTEDIT:<kind>` and the tool
/// reports typed `not_actionable`/`not_editable` (never a silent no-op).
/// There is NO synthetic value-setter path: if `Input.insertText` fails, the
/// tool returns a typed error and does not fall back.
fn type_preflight_ref(id: &str) -> String {
    let rid = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "((ref) => {{ const el = document.querySelector('[data-gladius-ref=\"' + ref + '\"]'); if (!el) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.readOnly) return 'GLADIUS_READONLY'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; const r = el.getBoundingClientRect(); if (!r || (r.width === 0 && r.height === 0)) return 'GLADIUS_HIDDEN'; const tag = (el.tagName || '').toUpperCase(); let kind = ''; if (tag === 'INPUT') {{ const ty = String(el.type || 'text').toLowerCase(); if (['text','search','url','tel','password','email','number'].includes(ty)) kind = 'TEXT'; else kind = 'NOTEDIT:' + ty; }} else if (tag === 'TEXTAREA') kind = 'TEXT'; else if (el.isContentEditable) kind = 'CE'; else kind = 'NOTEDIT:tag:' + tag; if (kind.startsWith('NOTEDIT:')) return 'GLADIUS_NOTEDIT:' + kind.slice(8); try {{ el.focus(); }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} try {{ if (kind === 'TEXT') {{ if (typeof el.select === 'function') el.select(); else if (typeof el.setSelectionRange === 'function') el.setSelectionRange(0, el.value.length); }} else {{ const range = document.createRange(); range.selectNodeContents(el); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range); }} }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} return 'GLADIUS_READY'; }})({rid})"
    )
}

fn type_preflight_sel(sel: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.readOnly) return 'GLADIUS_READONLY'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; const r = el.getBoundingClientRect(); if (!r || (r.width === 0 && r.height === 0)) return 'GLADIUS_HIDDEN'; const tag = (el.tagName || '').toUpperCase(); let kind = ''; if (tag === 'INPUT') {{ const ty = String(el.type || 'text').toLowerCase(); if (['text','search','url','tel','password','email','number'].includes(ty)) kind = 'TEXT'; else kind = 'NOTEDIT:' + ty; }} else if (tag === 'TEXTAREA') kind = 'TEXT'; else if (el.isContentEditable) kind = 'CE'; else kind = 'NOTEDIT:tag:' + tag; if (kind.startsWith('NOTEDIT:')) return 'GLADIUS_NOTEDIT:' + kind.slice(8); try {{ el.focus(); }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} try {{ if (kind === 'TEXT') {{ if (typeof el.select === 'function') el.select(); else if (typeof el.setSelectionRange === 'function') el.setSelectionRange(0, el.value.length); }} else {{ const range = document.createRange(); range.selectNodeContents(el); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range); }} }} catch (e) {{ return 'GLADIUS_HIDDEN'; }} return 'GLADIUS_READY'; }})()"
    )
}

fn type_not_editable(tool: &str, detail: &str, ctx: &Value) -> ToolErr {
    let mut sc = serde_json::Map::new();
    sc.insert("code".to_string(), json!("not_actionable"));
    sc.insert("reason".to_string(), json!("not_editable"));
    sc.insert("tool".to_string(), json!(tool));
    if let Some(r) = ctx.get("ref").and_then(|v| v.as_str()) {
        sc.insert("ref".to_string(), json!(r));
        ToolErr::typed(
            format!("type: {r} is not editable for typing (input type '{detail}'; use the matching tool)"),
            Value::Object(sc),
        )
    } else if let Some(e) = ctx.get("element").and_then(|v| v.as_str()) {
        sc.insert("element".to_string(), json!(e));
        ToolErr::typed(
            format!("type: element is not editable for typing (input type '{detail}'): {e}"),
            Value::Object(sc),
        )
    } else {
        ToolErr::typed(
            format!("type: target is not editable for typing (input type '{detail}')"),
            Value::Object(sc),
        )
    }
}

async fn tool_type(
    browser: &Browser,
    element: &str,
    text: &str,
    submit: bool,
    frame: Option<&str>,
) -> Result<Value, ToolErr> {
    let fid = frame_id_ok("type", frame)?;
    if let Some(sel) = ax_sel_parse("type", element)? {
        let hit = ax_resolve(browser, "type", &sel, element, fid.as_deref()).await?;
        return ax_type(browser, element, &hit, text, submit).await;
    }
    if let Some(ref f) = fid {
        if parse_ref(element).is_some() {
            return Err(ref_in_frame("type", element, f));
        }
        let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
        let expr = type_preflight_sel(&sel);
        let v = frame_realm_eval(browser, "type", f, &expr).await?;
        match v.as_str() {
            Some("GLADIUS_READY") => {}
            Some("GLADIUS_DISABLED") => {
                return Err(ToolErr::typed(
                    format!("type: element is disabled: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "type", "element": element, "frame": f}),
                ));
            }
            Some("GLADIUS_READONLY") => {
                return Err(ToolErr::typed(
                    format!("type: element is read-only: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Readonly.as_str(), "tool": "type", "element": element, "frame": f}),
                ));
            }
            Some("GLADIUS_HIDDEN") => {
                return Err(ToolErr::typed(
                    format!("type: element not visible or not editable: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "type", "element": element, "frame": f}),
                ));
            }
            Some(s) if s.starts_with("GLADIUS_NOTEDIT:") => {
                let kind = &s["GLADIUS_NOTEDIT:".len()..];
                return Err(type_not_editable(
                    "type",
                    kind,
                    &json!({"element": element, "frame": f}),
                ));
            }
            _ => {
                return Err(ToolErr::typed(
                    format!("type: element not found: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "type", "element": element, "frame": f}),
                ));
            }
        }
        browser.insert_text(text).await.map_err(|e| {
            type_not_editable(
                "type",
                &format!("insertText failed: {e}"),
                &json!({"element": element, "frame": f}),
            )
        })?;
        let len_expr = format!(
            "(() => {{ const el = document.querySelector({sel}); return el && typeof el.value === 'string' ? el.value.length : null; }})()"
        );
        let value_length = browser
            .eval_in_frame(f, &len_expr)
            .await
            .ok()
            .and_then(|v| v.as_u64());
        if submit {
            submit_enter(browser).await?;
        }
        return Ok(type_reply(text, value_length));
    }
    if let Some(id) = parse_ref(element) {
        let at_id = guard_ref(browser, "type", element, &id).await?;
        let oid = resolve_ref_object(browser, "type", &id).await?;
        let v = match &oid {
            Some(o) => call_on_object(browser, o, type_preflight_fn(), vec![]).await?,
            None => {
                let expr = type_preflight_ref(&id);
                browser.eval(&expr).await?
            }
        };
        match v.as_str() {
            Some("GLADIUS_READY") => {}
            Some("GLADIUS_NOTFOUND") => return Err(stale_gone("type", element, &id)),
            Some("GLADIUS_DISABLED") => {
                return Err(ToolErr::typed(
                    format!("type: {at_id} is disabled"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "type", "ref": at_id}),
                ))
            }
            Some("GLADIUS_READONLY") => {
                return Err(ToolErr::typed(
                    format!("type: {at_id} is read-only"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Readonly.as_str(), "tool": "type", "ref": at_id}),
                ))
            }
            Some("GLADIUS_HIDDEN") => {
                return Err(ToolErr::typed(
                    format!("type: {at_id} not visible or not editable"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "type", "ref": at_id}),
                ))
            }
            Some(s) if s.starts_with("GLADIUS_NOTEDIT:") => {
                let kind = &s["GLADIUS_NOTEDIT:".len()..];
                return Err(type_not_editable("type", kind, &json!({"ref": at_id})));
            }
            _ => return Err(stale_gone("type", element, &id)),
        }
        browser.insert_text(text).await.map_err(|e| {
            type_not_editable(
                "type",
                &format!("insertText failed: {e}"),
                &json!({"ref": at_id}),
            )
        })?;
        let value_length = match &oid {
            Some(o) => call_on_object(browser, o, VALUE_LEN_FN, vec![])
                .await
                .ok()
                .and_then(|v| v.as_u64()),
            None => {
                let lit = serde_json::to_string(&format!("[data-gladius-ref=\"{id}\"]"))
                    .map_err(|e| e.to_string())?;
                let expr = format!(
                    "(() => {{ const el = document.querySelector({lit}); return el && typeof el.value === 'string' ? el.value.length : null; }})()"
                );
                browser.eval(&expr).await.ok().and_then(|v| v.as_u64())
            }
        };
        if submit {
            submit_enter(browser).await?;
        }
        return Ok(type_reply(text, value_length));
    }
    let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
    let expr = type_preflight_sel(&sel);
    let v = browser.eval(&expr).await?;
    match v.as_str() {
        Some("GLADIUS_READY") => {}
        Some("GLADIUS_DISABLED") => {
            return Err(ToolErr::typed(
                format!("type: element is disabled: {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "type", "element": element}),
            ));
        }
        Some("GLADIUS_READONLY") => {
            return Err(ToolErr::typed(
                format!("type: element is read-only: {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Readonly.as_str(), "tool": "type", "element": element}),
            ));
        }
        Some("GLADIUS_HIDDEN") => {
            return Err(ToolErr::typed(
                format!("type: element not visible or not editable: {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "type", "element": element}),
            ));
        }
        Some(s) if s.starts_with("GLADIUS_NOTEDIT:") => {
            let kind = &s["GLADIUS_NOTEDIT:".len()..];
            return Err(type_not_editable(
                "type",
                kind,
                &json!({"element": element}),
            ));
        }
        Some("GLADIUS_NOTFOUND") => {
            if closed_shadow_hit(browser, element).await {
                return Err(shadow_closed("type", element));
            }
            return Err(ToolErr::typed(
                format!("type: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "type", "element": element}),
            ));
        }
        _ => {
            return Err(ToolErr::typed(
                format!("type: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "type", "element": element}),
            ))
        }
    }
    browser.insert_text(text).await.map_err(|e| {
        type_not_editable(
            "type",
            &format!("insertText failed: {e}"),
            &json!({"element": element}),
        )
    })?;
    let len_expr = format!(
        "(() => {{ const el = document.querySelector({sel}); return el && typeof el.value === 'string' ? el.value.length : null; }})()"
    );
    let value_length = browser.eval(&len_expr).await.ok().and_then(|v| v.as_u64());
    if submit {
        submit_enter(browser).await?;
    }
    Ok(type_reply(text, value_length))
}

async fn submit_enter(browser: &Browser) -> Result<(), String> {
    browser
        .command(
            "Input.dispatchKeyEvent",
            json!({"type": "keyDown", "key": "Enter", "code": "Enter", "text": "\r",
                   "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13}),
            true,
        )
        .await?;
    browser
        .command(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": "Enter", "code": "Enter",
                   "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13}),
            true,
        )
        .await?;
    Ok(())
}

/// The public evaluate boundary: a valueless CDP result is a void success,
/// not an error. Internal callers keep the strict eval helpers; only this
/// boundary reads the `undefined_result` marker and mints the `void` flag.
async fn eval_public(
    browser: &Browser,
    target: Option<&str>,
    frame: Option<&str>,
    expression: &str,
) -> Result<(Value, bool), ToolErr> {
    // Only a same-process frame eval is a frame lookup. An OOPIF session runs
    // JS in that frame's own realm and its error already carries a site-known
    // type (e.g. typed:js_error); route it through ToolErr::from unchanged,
    // exactly as frame_realm_eval does. Wrapping it in frame_lookup_err loses
    // that type and leaks the typed marker into prose.
    let (raw, same_process_frame): (Result<Value, String>, bool) = match (target, frame) {
        (Some(tid), None) => (browser.eval_in_target(tid, expression).await, false),
        (None, Some(fid)) => match browser.oop_session_for_frame(fid).await {
            Some((_, sid)) => (browser.eval_in_session(&sid, expression).await, false),
            None => (browser.eval_in_frame(fid, expression).await, true),
        },
        (None, None) => (browser.eval(expression).await, false),
        _ => unreachable!("target and frame are mutually exclusive at the dispatch boundary"),
    };
    match raw {
        Ok(v) => Ok((v, false)),
        Err(e) if crate::browser::is_undefined_result(&e) => Ok((Value::Null, true)),
        Err(e) if same_process_frame => Err(frame_lookup_err("evaluate", frame.expect("frame"), e)),
        Err(e) => Err(ToolErr::from(e)),
    }
}

async fn tool_evaluate(
    browser: &Browser,
    expression: &str,
    target: Option<&str>,
    frame: Option<&str>,
) -> Result<Value, ToolErr> {
    if target.is_some() && frame.is_some() {
        return Err(ToolErr::typed(
            "evaluate: 'target' and 'frame' are mutually exclusive (one realm per call)"
                .to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "evaluate"}),
        ));
    }
    browser.take_dialog().await;
    let (v, void) = eval_public(browser, target, frame, expression).await?;
    let mut obj = serde_json::Map::new();
    obj.insert("value".to_string(), v);
    if void {
        obj.insert("void".to_string(), json!(true));
    }
    if let Some(d) = browser.last_dialog().await {
        obj.insert(
            "dialog".to_string(),
            json!({"type": d.dtype, "message": d.message, "url": d.url}),
        );
        let verb = if d.accepted { "accepted" } else { "dismissed" };
        obj.insert(
            "note".to_string(),
            json!(format!(
                "dialog auto-handled: {} {:?} ({verb}, page unblocked)",
                d.dtype, d.message
            )),
        );
    }
    Ok(ok_text(Value::Object(obj).to_string()))
}

async fn tool_console(
    browser: &Browser,
    action: &str,
    limit: Option<i64>,
) -> Result<Value, ToolErr> {
    let lim = limit.unwrap_or(100).clamp(1, 200) as usize;
    match action {
        "get" => Ok(ok_text(browser.console_get(lim).await.to_string())),
        "drain" => {
            Ok(ok_text(browser.console_drain(lim).await.to_string()))
        }
        "clear" => {
            let n = browser.console_clear().await;
            Ok(ok_text(
                json!({"cleared": n, "cap": crate::browser::CONSOLE_CAP}).to_string(),
            ))
        }
        other => Err(ToolErr::typed(
            format!("console: unknown action '{other}' (get|drain|clear)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "console"}),
        )),
    }
}

async fn tool_upload(
    browser: &Browser,
    element: &str,
    paths: &[String],
    frame: Option<&str>,
) -> Result<Value, ToolErr> {
    if paths.is_empty() {
        return Err(ToolErr::typed(
            "upload: 'paths' is empty".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "upload"}),
        ));
    }
    for p in paths {
        if !std::path::Path::new(p).is_file() {
            return Err(ToolErr::typed(
                format!("upload: file not found: {p}"),
                json!({"code": ToolCode::FileNotFound.as_str(), "reason": ToolReason::MissingPath.as_str(), "tool": "upload", "path": p}),
            ));
        }
    }
    let fid = frame_id_ok("upload", frame)?;
    if let Some(sel) = ax_sel_parse("upload", element)? {
        let hit = ax_resolve(browser, "upload", &sel, element, fid.as_deref()).await?;
        return ax_upload(browser, element, &hit, paths).await;
    }
    if let Some(ref f) = fid {
        if parse_ref(element).is_some() {
            return Err(ref_in_frame("upload", element, f));
        }
        let lit = serde_json::to_string(element).map_err(|e| e.to_string())?;
        let probe = format!(
            "(() => {{ const el = document.querySelector({lit}); if (!el) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.tagName !== 'INPUT' || (el.type || '').toLowerCase() !== 'file') return 'GLADIUS_NOTFILE'; return 'GLADIUS_OK'; }})()"
        );
        match frame_realm_eval(browser, "upload", f, &probe)
            .await?
            .as_str()
        {
            Some("GLADIUS_OK") => {}
            Some("GLADIUS_NOTFOUND") => {
                return Err(ToolErr::typed(
                    format!("upload: element not found: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element, "frame": f}),
                ));
            }
            Some("GLADIUS_DISABLED") => {
                return Err(ToolErr::typed(
                    format!("upload: element is disabled: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "upload", "element": element, "frame": f}),
                ));
            }
            _ => {
                return Err(ToolErr::typed(
                    format!("upload: element is not a file input: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFile.as_str(), "tool": "upload", "element": element, "frame": f}),
                ));
            }
        }
        if let Some((_, sid)) = browser.oop_session_for_frame(f).await {
            return match browser.upload_in_session(&sid, element, paths).await {
                Ok(n) => Ok(ok_text(json!({"ok": true, "files": n as i64}).to_string())),
                Err(e) if e.contains("element not found") => Err(ToolErr::typed(
                    format!("upload: element not found: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element, "frame": f}),
                )),
                Err(e) => Err(ToolErr::fallback(e)),
            };
        }
        let node = match browser.frame_query_node(f, element).await {
            Ok(n) => n,
            Err(e) => {
                let l = e.to_lowercase();
                if l.contains("no frame with id") {
                    return Err(frame_lookup_err("upload", f, e));
                }
                if l.contains("not reachable") {
                    let mapped = browser.oop_target_for_frame(f).await;
                    let prose = match mapped {
                        Some(tid) => format!("upload: frame '{f}' is out-of-process (target {tid}); frame upload needs a mapped same-process document (no upload dispatched)"),
                        None => format!("upload: frame '{f}' has no accessible document (out-of-process-unmapped or detached; no upload dispatched)"),
                    };
                    return Err(ToolErr::typed(
                        prose,
                        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element, "frame": f}),
                    ));
                }
                if l.contains("not found in frame") {
                    return Err(ToolErr::typed(
                        format!("upload: element not found: {element}"),
                        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element, "frame": f}),
                    ));
                }
                return Err(frame_lookup_err("upload", f, e));
            }
        };
        if node == 0 {
            return Err(ToolErr::typed(
                format!("upload: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element, "frame": f}),
            ));
        }
        browser
            .command(
                "DOM.setFileInputFiles",
                json!({"nodeId": node, "files": paths}),
                true,
            )
            .await
            .map_err(|e| format!("upload: {e}"))?;
        return Ok(ok_text(
            json!({"ok": true, "files": paths.len()}).to_string(),
        ));
    }
    if let Some(id) = parse_ref(element) {
        guard_ref(browser, "upload", element, &id).await?;
        if let Some(backend) = browser.ref_backend(&id).await {
            let oid = resolve_ref_object(browser, "upload", &id)
                .await?
                .unwrap_or_default();
            let probe_fn = r#"function() { const el = this; if (!el || !el.isConnected) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.tagName !== 'INPUT' || (el.type || '').toLowerCase() !== 'file') return 'GLADIUS_NOTFILE'; return 'GLADIUS_OK'; }"#;
            match call_on_object(browser, &oid, probe_fn, vec![])
                .await?
                .as_str()
            {
                Some("GLADIUS_OK") => {}
                Some("GLADIUS_NOTFOUND") => {
                    return Err(stale_gone("upload", element, &id));
                }
                Some("GLADIUS_DISABLED") => {
                    return Err(ToolErr::typed(
                        format!("upload: element is disabled: {element}"),
                        json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "upload", "ref": format!("@{id}")}),
                    ));
                }
                _ => {
                    return Err(ToolErr::typed(
                        format!("upload: element is not a file input: {element}"),
                        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFile.as_str(), "tool": "upload", "ref": format!("@{id}")}),
                    ));
                }
            }
            browser.command("DOM.enable", json!({}), true).await?;
            browser
                .command(
                    "DOM.setFileInputFiles",
                    json!({"backendNodeId": backend, "files": paths}),
                    true,
                )
                .await
                .map_err(|e| format!("upload: {e}"))?;
            return Ok(ok_text(
                json!({"ok": true, "files": paths.len()}).to_string(),
            ));
        }
    }
    let sel_owned;
    let (sel, rid): (&str, Option<String>) = match parse_ref(element) {
        Some(id) => {
            guard_ref(browser, "upload", element, &id).await?;
            sel_owned = format!("[data-gladius-ref=\"{id}\"]");
            (&sel_owned, Some(id))
        }
        None => (element, None),
    };
    let lit = serde_json::to_string(sel).map_err(|e| e.to_string())?;
    let probe = format!(
        "(() => {{ const el = document.querySelector({lit}); if (!el) return 'GLADIUS_NOTFOUND'; if (el.disabled) return 'GLADIUS_DISABLED'; if (el.tagName !== 'INPUT' || (el.type || '').toLowerCase() !== 'file') return 'GLADIUS_NOTFILE'; return 'GLADIUS_OK'; }})()"
    );
    match browser.eval(&probe).await?.as_str() {
        Some("GLADIUS_OK") => {}
        Some("GLADIUS_NOTFOUND") => match rid {
            Some(id) => return Err(stale_gone("upload", element, &id)),
            None => {
                return Err(ToolErr::typed(
                    format!("upload: element not found: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element}),
                ))
            }
        },
        Some("GLADIUS_DISABLED") => {
            return Err(match &rid {
                Some(id) => ToolErr::typed(
                    format!("upload: element is disabled: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "upload", "ref": format!("@{id}")}),
                ),
                None => ToolErr::typed(
                    format!("upload: element is disabled: {element}"),
                    json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Disabled.as_str(), "tool": "upload", "element": element}),
                ),
            });
        }
        _ => {
            return Err(match &rid {
                Some(id) => ToolErr::typed(
                    format!("upload: element is not a file input: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFile.as_str(), "tool": "upload", "ref": format!("@{id}")}),
                ),
                None => ToolErr::typed(
                    format!("upload: element is not a file input: {element}"),
                    json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFile.as_str(), "tool": "upload", "element": element}),
                ),
            });
        }
    }
    browser.command("DOM.enable", json!({}), true).await?;
    let doc = browser
        .command("DOM.getDocument", json!({"depth": 0}), true)
        .await?;
    let root = doc
        .get("root")
        .and_then(|r| r.get("nodeId"))
        .and_then(|n| n.as_i64())
        .ok_or("upload: no document node".to_string())?;
    let found = browser
        .command(
            "DOM.querySelector",
            json!({"nodeId": root, "selector": sel}),
            true,
        )
        .await?;
    let node = found.get("nodeId").and_then(|n| n.as_i64()).unwrap_or(0);
    if node == 0 {
        return match rid {
            Some(id) => Err(stale_gone("upload", element, &id)),
            None => Err(ToolErr::typed(
                format!("upload: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "upload", "element": element}),
            )),
        };
    }
    browser
        .command(
            "DOM.setFileInputFiles",
            json!({"nodeId": node, "files": paths}),
            true,
        )
        .await
        .map_err(|e| format!("upload: {e}"))?;
    Ok(ok_text(
        json!({"ok": true, "files": paths.len()}).to_string(),
    ))
}

async fn tool_wait_for(
    browser: &Browser,
    selector: Option<&str>,
    text: Option<&str>,
    ms: Option<i64>,
    network_idle: bool,
    idle_ms: Option<i64>,
    expression: Option<&str>,
) -> Result<Value, ToolErr> {
    let budget_ms: i64 = ms.unwrap_or(5000).clamp(0, 15000);
    let budget = std::time::Duration::from_millis(budget_ms.max(0) as u64);
    if expression.is_some() && (selector.is_some() || text.is_some()) {
        return Err(ToolErr::typed(
            "wait_for: 'expression' is mutually exclusive with 'selector'/'text' (one condition per call)".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "wait_for"}),
        ));
    }
    if budget_ms == 0 && !network_idle {
        return Ok(ok_text(json!({"ok": true}).to_string()));
    }
    let deadline = std::time::Instant::now() + budget;
    let mut pinned: Option<String> = None;
    if selector.is_some() || text.is_some() {
        let sel_expr = selector.map(|s| {
            let lit = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
            format!("(() => {{ const el = document.querySelector({lit}); if (!el) return false; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return false; const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }})()")
        });
        let text_lit =
            text.map(|t| serde_json::to_string(t).unwrap_or_else(|_| "\"\"".to_string()));
        loop {
            let mut done = true;
            if let Some(ref e) = sel_expr {
                if std::time::Instant::now() >= deadline {
                    return Err(wait_timeout());
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match tokio::time::timeout(remaining, browser.eval(e)).await {
                    Ok(Ok(v)) => {
                        if let Some(msg) = browser.check_session_pin(&mut pinned).await {
                            return Err(ToolErr::from(msg));
                        }
                        if v != Value::Bool(true) {
                            done = false;
                        }
                    }
                    Ok(Err(e)) => {
                        if let Some(se) = stale_session_err(&e) {
                            return Err(se);
                        }
                        done = false;
                    }
                    _ => done = false,
                }
                if std::time::Instant::now() >= deadline {
                    return Err(wait_timeout());
                }
            }
            if done {
                if let Some(ref lit) = text_lit {
                    if std::time::Instant::now() >= deadline {
                        return Err(wait_timeout());
                    }
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    let e = format!(
                        "((document.body && document.body.innerText) || '').includes({lit})"
                    );
                    match tokio::time::timeout(remaining, browser.eval(&e)).await {
                        Ok(Ok(v)) => {
                            if let Some(msg) = browser.check_session_pin(&mut pinned).await {
                                return Err(ToolErr::from(msg));
                            }
                            if v != Value::Bool(true) {
                                done = false;
                            }
                        }
                        Ok(Err(e)) => {
                            if let Some(se) = stale_session_err(&e) {
                                return Err(se);
                            }
                            done = false;
                        }
                        _ => done = false,
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(wait_timeout());
                    }
                }
            }
            if done {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(wait_timeout());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            tokio::time::sleep(remaining.min(std::time::Duration::from_millis(100))).await;
        }
    } else if !network_idle && expression.is_none() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        tokio::time::sleep(remaining).await;
        return Ok(ok_text(json!({"ok": true}).to_string()));
    }
    if let Some(expr) = expression {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match browser.wait_for_expression(expr, remaining).await {
            Ok(true) => {}
            Ok(false) => return Err(wait_timeout()),
            Err(e) => return Err(stale_session_err(&e).unwrap_or_else(|| ToolErr::fallback(e))),
        }
    }
    if network_idle {
        let idle_win =
            std::time::Duration::from_millis(idle_ms.unwrap_or(500).clamp(0, 5000).max(0) as u64);
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, browser.ensure_network()).await {
            Ok(Ok(())) => {
                if let Some(msg) = browser.check_session_pin(&mut pinned).await {
                    return Err(ToolErr::from(msg));
                }
            }
            Err(_) => return Err(wait_timeout()),
            Ok(Err(e)) => return Err(e.into()),
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match browser
            .wait_network_idle(idle_win, remaining, &mut pinned)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(wait_timeout()),
            Err(e) => return Err(stale_session_err(&e).unwrap_or_else(|| ToolErr::fallback(e))),
        }
        if let Some(msg) = browser.check_session_pin(&mut pinned).await {
            return Err(ToolErr::from(msg));
        }
    }
    Ok(ok_text(json!({"ok": true}).to_string()))
}

/// Budget-exhaustion failure for `wait_for`: the site knows the budget ran
/// out, so it mints the typed timeout (never a generic error).
fn wait_timeout() -> ToolErr {
    ToolErr::typed(
        "wait_for: timeout".to_string(),
        json!({"code": ToolCode::Timeout.as_str(), "reason": ToolReason::BudgetExceeded.as_str(), "tool": "wait_for"}),
    )
}

/// Session-death fast path for polling waits: an engine error minted by
/// `send_session` (`stale session: ...` ÔÇö the session-death class, surfaced
/// either from a failed send or from a failed re-attach inside it) means
/// the page is gone. Return the typed `invalid_target`/`stale_session` at
/// once instead of polling the corpse until the budget burns. Anything
/// else keeps the existing handling (a false condition keeps polling; a
/// throwing expression is its own real error).
fn stale_session_err(e: &str) -> Option<ToolErr> {
    e.strip_prefix("stale session: ")
        .map(|_| ToolErr::from(e.to_string()))
}

async fn tool_hover(
    browser: &Browser,
    element: &str,
    frame: Option<&str>,
) -> Result<Value, ToolErr> {
    let fid = frame_id_ok("hover", frame)?;
    if let Some(sel) = ax_sel_parse("hover", element)? {
        let hit = ax_resolve(browser, "hover", &sel, element, fid.as_deref()).await?;
        return ax_hover(browser, element, &hit, fid.as_deref()).await;
    }
    if let Some(ref f) = fid {
        if parse_ref(element).is_some() {
            return Err(ref_in_frame("hover", element, f));
        }
        let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; try {{ el.scrollIntoView({{block: 'center', inline: 'center'}}); }} catch (e) {{}} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return JSON.stringify({{x: cx, y: cy}}); }})()"
        );
        let v = frame_realm_eval(browser, "hover", f, &expr).await?;
        if v.as_str() == Some("GLADIUS_NOTFOUND") {
            return Err(ToolErr::typed(
                format!("hover: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "hover", "element": element, "frame": f}),
            ));
        }
        if v.as_str() == Some("GLADIUS_HIDDEN") {
            return Err(ToolErr::typed(
                format!("hover: element not visible or not hoverable: {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "hover", "element": element, "frame": f}),
            ));
        }
        if v.as_str() == Some("GLADIUS_OCCLUDED") {
            return Err(ToolErr::typed(
                format!("hover: element is occluded by another element (no hover dispatched): {element}"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "hover", "element": element, "frame": f}),
            ));
        }
        let pt: Value = serde_json::from_str(v.as_str().unwrap_or(""))
            .map_err(|_| "hover: no layout box".to_string())?;
        let lx = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let ly = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let (mx, my) = frame_owner_hit_ok(browser, "hover", element, f, lx, ly).await?;
        browser.mouse_move(mx, my).await?;
        return Ok(ok_text(json!({"ok": true}).to_string()));
    }
    if let Some(id) = parse_ref(element) {
        let at_id = guard_ref(browser, "hover", element, &id).await?;
        let v = match resolve_ref_object(browser, "hover", &id).await? {
            Some(oid) => call_on_object(browser, &oid, hover_preflight_fn(), vec![]).await?,
            None => {
                let rid = serde_json::to_string(&id).map_err(|e| e.to_string())?;
                let expr = format!(
                    "((ref) => {{ const el = document.querySelector('[data-gladius-ref=\"' + ref + '\"]'); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; try {{ el.scrollIntoView({{block: 'center', inline: 'center'}}); }} catch (e) {{}} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return JSON.stringify({{x: cx, y: cy}}); }})({rid})"
                );
                browser.eval(&expr).await?
            }
        };
        if v.as_str() == Some("GLADIUS_NOTFOUND") {
            return Err(stale_gone("hover", element, &id));
        }
        if v.as_str() == Some("GLADIUS_HIDDEN") {
            return Err(ToolErr::typed(
                format!("hover: {at_id} not visible or not hoverable"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "hover", "ref": at_id}),
            ));
        }
        if v.as_str() == Some("GLADIUS_OCCLUDED") {
            return Err(ToolErr::typed(
                format!("hover: {at_id} is occluded by another element (no hover dispatched)"),
                json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "hover", "ref": at_id}),
            ));
        }
        return hover_move(browser, &v).await;
    }
    let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
    let expr = format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; const cs = getComputedStyle(el); if (cs.display === 'none' || cs.visibility === 'hidden' || cs.visibility === 'collapse') return 'GLADIUS_HIDDEN'; try {{ el.scrollIntoView({{block: 'center', inline: 'center'}}); }} catch (e) {{}} const r = el.getBoundingClientRect(); if (!r || r.width === 0 || r.height === 0) return 'GLADIUS_HIDDEN'; const cx = r.left + r.width / 2, cy = r.top + r.height / 2; let hit = null; try {{ hit = document.elementFromPoint(cx, cy); }} catch (e) {{ return 'GLADIUS_OCCLUDED'; }} if (!hit || (hit !== el && !el.contains(hit))) return 'GLADIUS_OCCLUDED'; return JSON.stringify({{x: cx, y: cy}}); }})()"
    );
    let v = browser.eval(&expr).await?;
    if v.as_str() == Some("GLADIUS_NOTFOUND") {
        if closed_shadow_hit(browser, element).await {
            return Err(shadow_closed("hover", element));
        }
        return Err(ToolErr::typed(
            format!("hover: element not found: {element}"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "hover", "element": element}),
        ));
    }
    if v.as_str() == Some("GLADIUS_HIDDEN") {
        return Err(ToolErr::typed(
            format!("hover: element not visible or not hoverable: {element}"),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Hidden.as_str(), "tool": "hover", "element": element}),
        ));
    }
    if v.as_str() == Some("GLADIUS_OCCLUDED") {
        return Err(ToolErr::typed(
            format!(
                "hover: element is occluded by another element (no hover dispatched): {element}"
            ),
            json!({"code": ToolCode::NotActionable.as_str(), "reason": ToolReason::Occluded.as_str(), "tool": "hover", "element": element}),
        ));
    }
    hover_move(browser, &v).await
}

async fn hover_move(browser: &Browser, v: &Value) -> Result<Value, ToolErr> {
    let s = v.as_str().unwrap_or("").to_string();
    let pt: Value = serde_json::from_str(&s).map_err(|_| "hover: no layout box".to_string())?;
    let x = pt.get("x").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let y = pt.get("y").and_then(|n| n.as_f64()).unwrap_or(0.0);
    browser.mouse_move(x, y).await?;
    Ok(ok_text(json!({"ok": true}).to_string()))
}

async fn tool_select(
    browser: &Browser,
    element: &str,
    value: Option<&str>,
    text: Option<&str>,
    frame: Option<&str>,
) -> Result<Value, ToolErr> {
    if value.is_none() && text.is_none() {
        return Err(ToolErr::typed(
            "select: need 'value' or 'text'".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::NeedOption.as_str(), "tool": "select"}),
        ));
    }
    let fid = frame_id_ok("select", frame)?;
    if let Some(sel) = ax_sel_parse("select", element)? {
        let hit = ax_resolve(browser, "select", &sel, element, fid.as_deref()).await?;
        return ax_select(browser, element, &hit, value, text).await;
    }
    if let Some(ref f) = fid {
        if parse_ref(element).is_some() {
            return Err(ref_in_frame("select", element, f));
        }
        let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
        let val_lit = match value {
            Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
            None => "null".to_string(),
        };
        let txt_lit = match text {
            Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
            None => "null".to_string(),
        };
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; if (!el || el.tagName !== 'SELECT') return 'GLADIUS_NOTSELECT'; const wantValue = {val_lit}; const wantText = {txt_lit}; const opts = Array.from(el.options); let opt = null; if (wantValue !== null) {{ opt = opts.find(o => o.value === wantValue); }} else {{ opt = opts.find(o => o.text === wantText || o.textContent === wantText); }} if (!opt) return 'GLADIUS_NOOPT'; el.value = opt.value; el.selectedIndex = opts.indexOf(opt); el.dispatchEvent(new Event('input', {{bubbles: true}})); el.dispatchEvent(new Event('change', {{bubbles: true}})); return 'GLADIUS_OK'; }})()"
        );
        let v = frame_realm_eval(browser, "select", f, &expr).await?;
        return match v.as_str() {
            Some("GLADIUS_OK") => Ok(ok_text(json!({"ok": true}).to_string())),
            Some("GLADIUS_NOTFOUND") => Err(ToolErr::typed(
                format!("select: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "select", "element": element, "frame": f}),
            )),
            Some("GLADIUS_NOTSELECT") => Err(ToolErr::typed(
                format!("select: element is not a <select>: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotSelect.as_str(), "tool": "select", "element": element, "frame": f}),
            )),
            _ => {
                let want = value.or(text).unwrap_or("");
                Err(ToolErr::typed(
                    format!("select: option not found: {want}"),
                    json!({"code": ToolCode::OptionNotFound.as_str(), "reason": ToolReason::NoMatch.as_str(), "tool": "select", "want": want, "frame": f}),
                ))
            }
        };
    }
    if let Some(id) = parse_ref(element) {
        guard_ref(browser, "select", element, &id).await?;
        let val_lit = match value {
            Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
            None => "null".to_string(),
        };
        let txt_lit = match text {
            Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
            None => "null".to_string(),
        };
        let v = match resolve_ref_object(browser, "select", &id).await? {
            Some(oid) => {
                call_on_object(
                    browser,
                    &oid,
                    &select_preflight_fn(&val_lit, &txt_lit),
                    vec![],
                )
                .await?
            }
            None => {
                let rid = serde_json::to_string(&id).map_err(|e| e.to_string())?;
                let expr = format!(
                    "((ref) => {{ const el = document.querySelector('[data-gladius-ref=\"' + ref + '\"]'); if (!el) return 'GLADIUS_NOTFOUND'; if (!el || el.tagName !== 'SELECT') return 'GLADIUS_NOTSELECT'; const wantValue = {val_lit}; const wantText = {txt_lit}; const opts = Array.from(el.options); let opt = null; if (wantValue !== null) {{ opt = opts.find(o => o.value === wantValue); }} else {{ opt = opts.find(o => o.text === wantText || o.textContent === wantText); }} if (!opt) return 'GLADIUS_NOOPT'; el.value = opt.value; el.selectedIndex = opts.indexOf(opt); el.dispatchEvent(new Event('input', {{bubbles: true}})); el.dispatchEvent(new Event('change', {{bubbles: true}})); return 'GLADIUS_OK'; }})({rid})"
                );
                browser.eval(&expr).await?
            }
        };
        return match v.as_str() {
            Some("GLADIUS_OK") => Ok(ok_text(json!({"ok": true}).to_string())),
            Some("GLADIUS_NOTFOUND") => Err(stale_gone("select", element, &id)),
            Some("GLADIUS_NOTSELECT") => Err(ToolErr::typed(
                format!("select: @{id} is not a <select>"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotSelect.as_str(), "tool": "select", "ref": format!("@{id}")}),
            )),
            _ => {
                let want = value.or(text).unwrap_or("");
                Err(ToolErr::typed(
                    format!("select: option not found: {want}"),
                    json!({"code": ToolCode::OptionNotFound.as_str(), "reason": ToolReason::NoMatch.as_str(), "tool": "select", "want": want}),
                ))
            }
        };
    }
    let sel = serde_json::to_string(element).map_err(|e| e.to_string())?;
    let val_lit = match value {
        Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
        None => "null".to_string(),
    };
    let txt_lit = match text {
        Some(s) => serde_json::to_string(s).map_err(|e| e.to_string())?,
        None => "null".to_string(),
    };
    let expr = format!(
        "(() => {{ const el = document.querySelector({sel}); if (!el) return 'GLADIUS_NOTFOUND'; if (!el || el.tagName !== 'SELECT') return 'GLADIUS_NOTSELECT'; const wantValue = {val_lit}; const wantText = {txt_lit}; const opts = Array.from(el.options); let opt = null; if (wantValue !== null) {{ opt = opts.find(o => o.value === wantValue); }} else {{ opt = opts.find(o => o.text === wantText || o.textContent === wantText); }} if (!opt) return 'GLADIUS_NOOPT'; el.value = opt.value; el.selectedIndex = opts.indexOf(opt); el.dispatchEvent(new Event('input', {{bubbles: true}})); el.dispatchEvent(new Event('change', {{bubbles: true}})); return 'GLADIUS_OK'; }})()"
    );
    let v = browser.eval(&expr).await?;
    match v.as_str() {
        Some("GLADIUS_OK") => Ok(ok_text(json!({"ok": true}).to_string())),
        Some("GLADIUS_NOTFOUND") => {
            if closed_shadow_hit(browser, element).await {
                return Err(shadow_closed("select", element));
            }
            Err(ToolErr::typed(
                format!("select: element not found: {element}"),
                json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "select", "element": element}),
            ))
        }
        Some("GLADIUS_NOTSELECT") => Err(ToolErr::typed(
            format!("select: element is not a <select>: {element}"),
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotSelect.as_str(), "tool": "select", "element": element}),
        )),
        _ => {
            let want = value.or(text).unwrap_or("");
            Err(ToolErr::typed(
                format!("select: option not found: {want}"),
                json!({"code": ToolCode::OptionNotFound.as_str(), "reason": ToolReason::NoMatch.as_str(), "tool": "select", "want": want}),
            ))
        }
    }
}

/// Map a human key name to (key, code, vk, text-for-keyDown). Unknown
/// multi-char names are None: the caller fails loud instead of dispatching
/// a silent no-op.
fn key_map(key: &str) -> Option<(String, String, u32, Option<String>)> {
    let lower = key.to_lowercase();
    Some(match lower.as_str() {
        "enter" => ("Enter".into(), "Enter".into(), 13, Some("\r".into())),
        "tab" => ("Tab".into(), "Tab".into(), 9, None),
        "escape" | "esc" => ("Escape".into(), "Escape".into(), 27, None),
        "backspace" => ("Backspace".into(), "Backspace".into(), 8, None),
        "delete" | "del" => ("Delete".into(), "Delete".into(), 46, None),
        "arrowup" | "up" => ("ArrowUp".into(), "ArrowUp".into(), 38, None),
        "arrowdown" | "down" => ("ArrowDown".into(), "ArrowDown".into(), 40, None),
        "arrowleft" | "left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37, None),
        "arrowright" | "right" => ("ArrowRight".into(), "ArrowRight".into(), 39, None),
        "home" => ("Home".into(), "Home".into(), 36, None),
        "end" => ("End".into(), "End".into(), 35, None),
        "pageup" => ("PageUp".into(), "PageUp".into(), 33, None),
        "pagedown" => ("PageDown".into(), "PageDown".into(), 34, None),
        "space" | "spacebar" => (" ".into(), "Space".into(), 32, Some(" ".into())),
        "shift" => ("Shift".into(), "ShiftLeft".into(), 16, None),
        "control" | "ctrl" => ("Control".into(), "ControlLeft".into(), 17, None),
        "alt" => ("Alt".into(), "AltLeft".into(), 18, None),
        "meta" | "cmd" | "command" => ("Meta".into(), "MetaLeft".into(), 91, None),
        _ => {
            if lower.len() > 1
                && lower.len() <= 3
                && lower.as_bytes()[0] == b'f'
                && lower[1..].chars().all(|c| c.is_ascii_digit())
            {
                if let Ok(n) = lower[1..].parse::<u32>() {
                    if (1..=12).contains(&n) {
                        let name = format!("F{n}");
                        return Some((name.clone(), name, 111 + n, None));
                    }
                }
            }
            if key.chars().count() == 1 {
                let c = key.chars().next().unwrap();
                let up = c.to_ascii_uppercase();
                let code = if c.is_ascii_alphabetic() {
                    format!("Key{up}")
                } else if c.is_ascii_digit() {
                    format!("Digit{c}")
                } else if c == ' ' {
                    "Space".to_string()
                } else {
                    String::new()
                };
                return Some((c.to_string(), code, up as u32, Some(c.to_string())));
            }
            return None;
        }
    })
}

async fn tool_key(browser: &Browser, key: &str) -> Result<Value, ToolErr> {
    if key.is_empty() {
        return Err(ToolErr::typed(
            "key: missing required arg 'key'".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "key"}),
        ));
    }
    if key.contains('+') {
        return tool_key_combo(browser, key).await;
    }
    let Some((k, code, vk, text)) = key_map(key) else {
        return Err(ToolErr::typed(
            format!("key: unknown key '{key}' (not a recognized key name or single character)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "key", "arg": "key", "want": key}),
        ));
    };
    let mut down = serde_json::Map::new();
    down.insert("type".to_string(), json!("keyDown"));
    down.insert("key".to_string(), json!(k));
    if !code.is_empty() {
        down.insert("code".to_string(), json!(code));
    }
    if vk != 0 {
        down.insert("windowsVirtualKeyCode".to_string(), json!(vk));
        down.insert("nativeVirtualKeyCode".to_string(), json!(vk));
    }
    if let Some(t) = text {
        down.insert("text".to_string(), json!(t));
    }
    browser
        .command("Input.dispatchKeyEvent", Value::Object(down), true)
        .await?;
    let mut up = serde_json::Map::new();
    up.insert("type".to_string(), json!("keyUp"));
    up.insert("key".to_string(), json!(k));
    if !code.is_empty() {
        up.insert("code".to_string(), json!(code));
    }
    if vk != 0 {
        up.insert("windowsVirtualKeyCode".to_string(), json!(vk));
        up.insert("nativeVirtualKeyCode".to_string(), json!(vk));
    }
    browser
        .command("Input.dispatchKeyEvent", Value::Object(up), true)
        .await?;
    Ok(ok_text(json!({"ok": true}).to_string()))
}

/// G4b key-combo dispatch: `Ctrl+A`, `Shift+Tab`, `Alt+...`, `Meta+...`.
///
/// Parse: split on '+', trim; every token but the last must be a modifier
/// (case-insensitive: ctrl/control, shift, alt, meta/cmd/command); the last
/// token is the base key through the existing `key_map` (so `F5`, `Delete`,
/// single chars all keep working as combo bases). Empty tokens (a bare `+`
/// base excepted: a trailing '+' means the base key IS '+') and unknown
/// names are typed `invalid_args`/`bad_value`, never a silent no-op.
///
/// Modifier mask: Alt=1, Ctrl=2, Meta=4, Shift=8. Every event carries the full
/// mask. Order: modifier keyDowns in written order, base keyDown + keyUp,
/// modifier keyUps in reverse. When Ctrl/Meta/Alt is held the base dispatches
/// as `type:"rawKeyDown"` with NO `text` (Ctrl+A must select-all, never insert
/// "a"); Shift-only combos keep the normal `keyDown` with text. Shift+Tab is
/// `{key:"Tab",code:"Tab",vk:9,modifiers:8}` with no text per the same rule
/// (Tab has no text in the map).
async fn tool_key_combo(browser: &Browser, key: &str) -> Result<Value, ToolErr> {
    fn unknown(k: &str) -> ToolErr {
        ToolErr::typed(
            format!(
                "key: unknown key '{k}' (not a recognized key name, modifier, or single character)"
            ),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "key", "arg": "key", "want": k}),
        )
    }
    let raw_parts: Vec<String> = key.split('+').map(|s| s.trim().to_string()).collect();
    let (mod_parts, base_str): (Vec<String>, String) =
        if raw_parts.last().map(|s| s.is_empty()).unwrap_or(false) {
            let mods: Vec<String> = raw_parts[..raw_parts.len() - 1]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect();
            if mods.is_empty() {
                let Some(mapped) = key_map("+") else {
                    return Err(unknown(key));
                };
                return tool_key_single(browser, mapped).await;
            }
            (mods, "+".to_string())
        } else {
            let b = raw_parts.last().cloned().unwrap_or_default();
            let m = raw_parts[..raw_parts.len().saturating_sub(1)].to_vec();
            (m, b)
        };
    if mod_parts.iter().any(|s| s.is_empty()) || base_str.is_empty() {
        return Err(unknown(key));
    }
    if mod_parts.is_empty() {
        return Err(unknown(key));
    }
    let mut mods: Vec<(String, String, u32)> = Vec::new();
    let mut mask: u32 = 0;
    for m in &mod_parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => {
                mask |= 2;
                mods.push(("Control".to_string(), "ControlLeft".to_string(), 17));
            }
            "shift" => {
                mask |= 8;
                mods.push(("Shift".to_string(), "ShiftLeft".to_string(), 16));
            }
            "alt" => {
                mask |= 1;
                mods.push(("Alt".to_string(), "AltLeft".to_string(), 18));
            }
            "meta" | "cmd" | "command" => {
                mask |= 4;
                mods.push(("Meta".to_string(), "MetaLeft".to_string(), 91));
            }
            _ => return Err(unknown(key)),
        }
    }
    let Some((bk, bcode, bvk, btext)) = key_map(&base_str) else {
        return Err(unknown(key));
    };
    let raw_base = mask & 0x7 != 0; // Ctrl/Meta/Alt held: rawKeyDown, no text.
    let mut seq: Vec<Value> = Vec::new();
    for (k, code, vk) in &mods {
        let mut p = serde_json::Map::new();
        p.insert("type".to_string(), json!("keyDown"));
        p.insert("key".to_string(), json!(k));
        if !code.is_empty() {
            p.insert("code".to_string(), json!(code));
        }
        if *vk != 0 {
            p.insert("windowsVirtualKeyCode".to_string(), json!(vk));
            p.insert("nativeVirtualKeyCode".to_string(), json!(vk));
        }
        p.insert("modifiers".to_string(), json!(mask));
        seq.push(Value::Object(p));
    }
    {
        let mut p = serde_json::Map::new();
        p.insert(
            "type".to_string(),
            json!(if raw_base { "rawKeyDown" } else { "keyDown" }),
        );
        p.insert("key".to_string(), json!(bk));
        if !bcode.is_empty() {
            p.insert("code".to_string(), json!(bcode));
        }
        if bvk != 0 {
            p.insert("windowsVirtualKeyCode".to_string(), json!(bvk));
            p.insert("nativeVirtualKeyCode".to_string(), json!(bvk));
        }
        if !raw_base {
            if let Some(t) = btext {
                p.insert("text".to_string(), json!(t));
            }
        }
        p.insert("modifiers".to_string(), json!(mask));
        seq.push(Value::Object(p));
    }
    {
        let mut p = serde_json::Map::new();
        p.insert("type".to_string(), json!("keyUp"));
        p.insert("key".to_string(), json!(bk));
        if !bcode.is_empty() {
            p.insert("code".to_string(), json!(bcode));
        }
        if bvk != 0 {
            p.insert("windowsVirtualKeyCode".to_string(), json!(bvk));
            p.insert("nativeVirtualKeyCode".to_string(), json!(bvk));
        }
        p.insert("modifiers".to_string(), json!(mask));
        seq.push(Value::Object(p));
    }
    for (k, code, vk) in mods.iter().rev() {
        let mut p = serde_json::Map::new();
        p.insert("type".to_string(), json!("keyUp"));
        p.insert("key".to_string(), json!(k));
        if !code.is_empty() {
            p.insert("code".to_string(), json!(code));
        }
        if *vk != 0 {
            p.insert("windowsVirtualKeyCode".to_string(), json!(vk));
            p.insert("nativeVirtualKeyCode".to_string(), json!(vk));
        }
        p.insert("modifiers".to_string(), json!(mask));
        seq.push(Value::Object(p));
    }
    for params in seq {
        browser
            .command("Input.dispatchKeyEvent", params, true)
            .await
            .map_err(ToolErr::from)?;
    }
    Ok(ok_text(json!({"ok": true}).to_string()))
}

/// Single-key dispatch helper shared by the bare-`+` combo normalization.
/// Params are byte-identical to the historical single-key path (no
/// `modifiers` field).
async fn tool_key_single(
    browser: &Browser,
    mapped: (String, String, u32, Option<String>),
) -> Result<Value, ToolErr> {
    let (k, code, vk, text) = mapped;
    let mut down = serde_json::Map::new();
    down.insert("type".to_string(), json!("keyDown"));
    down.insert("key".to_string(), json!(k));
    if !code.is_empty() {
        down.insert("code".to_string(), json!(code));
    }
    if vk != 0 {
        down.insert("windowsVirtualKeyCode".to_string(), json!(vk));
        down.insert("nativeVirtualKeyCode".to_string(), json!(vk));
    }
    if let Some(t) = text {
        down.insert("text".to_string(), json!(t));
    }
    browser
        .command("Input.dispatchKeyEvent", Value::Object(down), true)
        .await?;
    let mut up = serde_json::Map::new();
    up.insert("type".to_string(), json!("keyUp"));
    up.insert("key".to_string(), json!(k));
    if !code.is_empty() {
        up.insert("code".to_string(), json!(code));
    }
    if vk != 0 {
        up.insert("windowsVirtualKeyCode".to_string(), json!(vk));
        up.insert("nativeVirtualKeyCode".to_string(), json!(vk));
    }
    browser
        .command("Input.dispatchKeyEvent", Value::Object(up), true)
        .await?;
    Ok(ok_text(json!({"ok": true}).to_string()))
}

/// `storage`: cookies + Web Storage (localStorage/sessionStorage) in one
/// call, plus a versioned file save/load.
///
/// Cookie reads are page-scoped (`Network.getCookies` with the active url);
/// `allSites:true` widens the read to every site (`Storage.getCookies`,
/// browser level). Cookie writes go per-cookie through `Network.setCookie`
/// and are verified by a read-back diff against the page jar: anything the
/// engine drops (`success:false` or a read-back miss) is a typed
/// `cookie_rejected` naming the cookie and domain - never a silent partial
/// write. Cookie clear is page-scoped (`Network.deleteCookies`); `allSites`
/// wipes the whole profile (`Storage.clearCookies`, owned engine only - on
/// an attached engine that is a browser-wide wipe of somebody else's
/// profile, so it needs explicit `confirm:true`).
///
/// Web Storage scopes run in the page realm as JSON round-trips. Opaque
/// origins (no origin to scope storage to) throw SecurityError in-page, so
/// every storage leg probes first and fails typed (`opaque_origin`) instead
/// of surfacing a thrown exception.
///
/// `save` writes `{gladiusStorage:1,savedAt,url,origin,cookies,
/// localStorage,sessionStorage}`: the parent directory must exist, an
/// existing file is never clobbered without `overwrite:true`, and the reply
/// carries counts/bytes/origin only - never values. `load` validates the
/// file (JSON shape + version) and enforces an origin match unless
/// `force:true`, then reports per-item applied/failed. `redact:true` masks
/// get values with the G0 mask and sets `redacted:true`. Values are never
/// logged (no stderr carries caller data anywhere on this path).
async fn tool_storage(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("get");
    if !["get", "set", "clear", "save", "load"].contains(&action) {
        return Err(ToolErr::typed(
            format!("storage: unknown action '{action}' (get|set|clear|save|load)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "storage"}),
        ));
    }
    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");
    let want_cookies = matches!(scope, "cookies" | "all");
    let want_local = matches!(scope, "localStorage" | "all");
    let want_session = matches!(scope, "sessionStorage" | "all");
    if !(want_cookies || want_local || want_session) {
        return Err(ToolErr::typed(
            format!("storage: unknown scope '{scope}' (cookies|localStorage|sessionStorage|all)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "scope", "want": scope}),
        ));
    }
    let pair = browser
        .eval("JSON.stringify([window.location.href, window.location.origin])")
        .await
        .map_err(ToolErr::from)?;
    let (url, origin) = match pair
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        Some(v) => (
            v.get(0).and_then(|x| x.as_str()).unwrap_or("").to_string(),
            v.get(1).and_then(|x| x.as_str()).unwrap_or("").to_string(),
        ),
        None => (String::new(), String::new()),
    };
    let req = StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url: &url,
        origin: &origin,
    };
    match action {
        "get" => storage_get(&req, flag(args, "allSites"), flag(args, "redact")).await,
        "set" => storage_set(&req, args).await,
        "clear" => storage_clear(&req, args, flag(args, "allSites")).await,
        "save" => storage_save(&req, args, flag(args, "allSites")).await,
        _ => storage_load(&req, args).await,
    }
}

/// One page-scoped storage request: the shared legs (realm, scope, page
/// identity) bundled so the action fns stay under the argument limit.
struct StorageReq<'a> {
    browser: &'a Browser,
    scope: &'a str,
    want_cookies: bool,
    want_local: bool,
    want_session: bool,
    url: &'a str,
    origin: &'a str,
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Typed opaque-origin refusal: Web Storage has no origin to scope to here,
/// so every storage read/write is refused instead of surfacing a thrown
/// SecurityError (or, worse, an empty success).
fn opaque_err() -> ToolErr {
    ToolErr::typed(
        "storage: web storage is blocked on this page (opaque origin: no origin to scope it to)"
            .to_string(),
        json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::OpaqueOrigin.as_str(), "tool": "storage"}),
    )
}

/// Probe that the requested Web Storage scopes are usable in the page realm
/// (a thrown SecurityError means an opaque origin).
async fn storage_realm_ok(
    browser: &Browser,
    want_local: bool,
    want_session: bool,
) -> Result<(), ToolErr> {
    let expr = match (want_local, want_session) {
        (true, true) => "(() => { try { void localStorage.length; void sessionStorage.length; return 'ok'; } catch (e) { return 'GLADIUS_OPAQUE'; } })()",
        (true, false) => "(() => { try { void localStorage.length; return 'ok'; } catch (e) { return 'GLADIUS_OPAQUE'; } })()",
        _ => "(() => { try { void sessionStorage.length; return 'ok'; } catch (e) { return 'GLADIUS_OPAQUE'; } })()",
    };
    match browser.eval(expr).await.map_err(ToolErr::from)?.as_str() {
        Some("GLADIUS_OPAQUE") => Err(opaque_err()),
        _ => Ok(()),
    }
}

/// Read both Web Storage maps in one round-trip (opaque origins fail typed
/// via the `GLADIUS_OPAQUE` marker, never as a thrown exception).
async fn read_storage_maps(
    browser: &Browser,
) -> Result<
    (
        std::collections::BTreeMap<String, String>,
        std::collections::BTreeMap<String, String>,
    ),
    ToolErr,
> {
    let v = browser.eval("(() => { try { const dump = (s) => { const o = Object.create(null); for (let i = 0; i < s.length; i++) { const k = s.key(i); o[k] = s.getItem(k); } return o; }; return JSON.stringify({local: dump(localStorage), session: dump(sessionStorage)}); } catch (e) { return 'GLADIUS_OPAQUE'; } })()").await.map_err(ToolErr::from)?;
    if v.as_str() == Some("GLADIUS_OPAQUE") {
        return Err(opaque_err());
    }
    let parsed: Value = serde_json::from_str(v.as_str().unwrap_or("{}"))
        .map_err(|_| "storage: unreadable web storage".to_string())?;
    let conv = |key: &str| {
        parsed
            .get(key)
            .and_then(|x| x.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok((conv("local"), conv("session")))
}

/// Shape one CDP cookie for the reply (Õø║Õ«ÜþÜä key set; `redact` masks).
fn shape_cookie(c: &Value, redact: bool) -> Value {
    json!({
        "name": c.get("name").cloned().unwrap_or(Value::Null),
        "value": if redact { json!("ÔÇóÔÇóÔÇóÔÇó") } else { c.get("value").cloned().unwrap_or(Value::Null) },
        "domain": c.get("domain").cloned().unwrap_or(Value::Null),
        "path": c.get("path").cloned().unwrap_or(Value::Null),
        "secure": c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false),
        "httpOnly": c.get("httpOnly").and_then(|v| v.as_bool()).unwrap_or(false),
        "sameSite": c.get("sameSite").cloned().unwrap_or(Value::Null),
        "expires": c.get("expires").cloned().unwrap_or(Value::Null),
    })
}

/// Lifetime note attached whenever sessionStorage is in scope: tab-scoped
/// storage clears with the tab while localStorage persists across reloads.
fn lifetime_note(want_session: bool) -> Value {
    if want_session {
        json!("sessionStorage is tab-scoped and clears with the tab; localStorage persists across reloads")
    } else {
        Value::Null
    }
}

async fn storage_get(r: &StorageReq<'_>, all_sites: bool, redact: bool) -> Result<Value, ToolErr> {
    let StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url,
        origin,
    } = *r;
    if want_local || want_session {
        storage_realm_ok(browser, want_local, want_session).await?;
    }
    let raw: Vec<Value> = if want_cookies {
        let r = if all_sites {
            browser
                .cdp_raw("Storage.getCookies", json!({}), None)
                .await
                .map_err(ToolErr::from)?
        } else {
            browser
                .command("Network.getCookies", json!({"urls": [url]}), true)
                .await
                .map_err(ToolErr::from)?
        };
        r.get("cookies")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let (local, session) = if want_local || want_session {
        read_storage_maps(browser).await?
    } else {
        (Default::default(), Default::default())
    };
    let mask = |m: &std::collections::BTreeMap<String, String>| -> Value {
        if redact {
            json!(m
                .keys()
                .map(|k| (k.clone(), json!("ÔÇóÔÇóÔÇóÔÇó")))
                .collect::<serde_json::Map<String, Value>>())
        } else {
            json!(m)
        }
    };
    let mut out = serde_json::Map::new();
    out.insert("ok".to_string(), json!(true));
    out.insert("action".to_string(), json!("get"));
    out.insert("scope".to_string(), json!(scope));
    out.insert("url".to_string(), json!(url));
    out.insert("origin".to_string(), json!(origin));
    if want_cookies {
        out.insert(
            "cookies".to_string(),
            json!(raw
                .iter()
                .map(|c| shape_cookie(c, redact))
                .collect::<Vec<_>>()),
        );
    }
    if want_local {
        out.insert("localStorage".to_string(), mask(&local));
    }
    if want_session {
        out.insert("sessionStorage".to_string(), mask(&session));
    }
    out.insert(
        "counts".to_string(),
        json!({
            "cookies": raw.len(),
            "localStorage": local.len(),
            "sessionStorage": session.len(),
        }),
    );
    out.insert("redacted".to_string(), json!(redact));
    let note = lifetime_note(want_session);
    if !note.is_null() {
        out.insert("note".to_string(), note);
    }
    Ok(ok_text(Value::Object(out).to_string()))
}

/// A validated `Network.setCookie` row. Validation runs fully before
/// anything applies, so a bad entry changes nothing.
struct SetCookie {
    name: String,
    value: String,
    url: Option<String>,
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    same_site: Option<String>,
    expires: Option<f64>,
}

fn bad_cookie(what: &str) -> ToolErr {
    ToolErr::typed(
        format!("storage: bad cookie entry ({what}; need name+value strings)"),
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "cookies"}),
    )
}

fn parse_set_cookie(c: &Value) -> Result<SetCookie, ToolErr> {
    let obj = c.as_object().ok_or_else(|| bad_cookie("not an object"))?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad_cookie("missing name"))?
        .to_string();
    let value = obj
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad_cookie("missing value"))?
        .to_string();
    let opt_str = |key: &str| -> Result<Option<String>, ToolErr> {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            _ => Err(bad_cookie(&format!("bad {key}"))),
        }
    };
    let opt_bool = |key: &str| -> Result<bool, ToolErr> {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            _ => Err(bad_cookie(&format!("bad {key}"))),
        }
    };
    let same_site = match opt_str("sameSite")? {
        None => None,
        Some(s) => Some(match s.to_ascii_lowercase().as_str() {
            "strict" => "Strict".to_string(),
            "lax" => "Lax".to_string(),
            "none" => "None".to_string(),
            _ => {
                return Err(ToolErr::typed(
                    format!("storage: bad cookie sameSite '{s}' (want Strict|Lax|None)"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "cookies"}),
                ));
            }
        }),
    };
    let secure = opt_bool("secure")?;
    if same_site.as_deref() == Some("None") && !secure {
        return Err(ToolErr::typed(
            "storage: Secure is required with SameSite=None".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "cookies"}),
        ));
    }
    let expires = match obj.get("expires") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_f64() {
            Some(f) if f.is_finite() => Some(f),
            _ => return Err(bad_cookie("bad expires")),
        },
    };
    Ok(SetCookie {
        name,
        value,
        url: opt_str("url")?,
        domain: opt_str("domain")?,
        path: opt_str("path")?,
        secure,
        http_only: opt_bool("httpOnly")?,
        same_site,
        expires,
    })
}

fn cookie_matches(stored: &Value, name: &str, value: &str, domain: &str, path: &str) -> bool {
    if stored.get("name").and_then(|v| v.as_str()) != Some(name) {
        return false;
    }
    if stored.get("value").and_then(|v| v.as_str()) != Some(value) {
        return false;
    }
    if !domain.is_empty() {
        let sd = stored
            .get("domain")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim_start_matches('.');
        if sd != domain.trim_start_matches('.') {
            return false;
        }
    }
    if !path.is_empty() {
        let sp = stored.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if sp != path {
            return false;
        }
    }
    true
}

/// Apply validated cookies: per-cookie `Network.setCookie`, then a
/// read-back diff against the page jar. Returns (applied, failed); a
/// `success:false` reply and a read-back miss both land in `failed` - the
/// caller decides between the typed `cookie_rejected` (set) and the
/// applied/failed report (load).
async fn apply_cookies(
    browser: &Browser,
    default_url: &str,
    cookies: &[SetCookie],
) -> Result<(usize, Vec<Value>), ToolErr> {
    let mut failed = Vec::new();
    for c in cookies {
        let mut params = serde_json::Map::new();
        params.insert("name".to_string(), json!(c.name));
        params.insert("value".to_string(), json!(c.value));
        params.insert(
            "url".to_string(),
            json!(c.url.clone().unwrap_or_else(|| default_url.to_string())),
        );
        if let Some(d) = &c.domain {
            params.insert("domain".to_string(), json!(d));
        }
        if let Some(p) = &c.path {
            params.insert("path".to_string(), json!(p));
        }
        if c.secure {
            params.insert("secure".to_string(), json!(true));
        }
        if c.http_only {
            params.insert("httpOnly".to_string(), json!(true));
        }
        if let Some(s) = &c.same_site {
            params.insert("sameSite".to_string(), json!(s));
        }
        if let Some(e) = c.expires {
            params.insert("expires".to_string(), json!(e));
        }
        match browser
            .command("Network.setCookie", Value::Object(params), true)
            .await
        {
            Ok(r) if r.get("success").and_then(|v| v.as_bool()) == Some(true) => {}
            _ => failed.push(json!({
                "cookie": c.name,
                "domain": c.domain.clone().unwrap_or_default(),
                "reason": ToolReason::SetFailed.as_str(),
            })),
        }
    }
    if !cookies.is_empty() {
        let live = browser
            .cdp_raw("Storage.getCookies", json!({}), None)
            .await
            .map_err(ToolErr::from)?;
        let jar = live
            .get("cookies")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for c in cookies {
            if failed
                .iter()
                .any(|f| f.get("cookie").and_then(|v| v.as_str()) == Some(c.name.as_str()))
            {
                continue;
            }
            let dom = c.domain.clone().unwrap_or_default();
            let pth = c.path.clone().unwrap_or_default();
            if !jar
                .iter()
                .any(|s| cookie_matches(s, &c.name, &c.value, &dom, &pth))
            {
                failed.push(json!({
                    "cookie": c.name,
                    "domain": dom,
                    "reason": ToolReason::NotStored.as_str(),
                }));
            }
        }
    }
    let applied = cookies.len().saturating_sub(failed.len());
    Ok((applied, failed))
}

fn cookie_rejected(first: &Value, failed: &[Value]) -> ToolErr {
    ToolErr::typed(
        format!(
            "storage: cookie rejected (not stored): {}",
            first.get("cookie").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        json!({
            "code": ToolCode::CookieRejected.as_str(),
            "reason": ToolReason::Rejected.as_str(),
            "tool": "storage",
            "cookie": first.get("cookie").cloned().unwrap_or(Value::Null),
            "domain": first.get("domain").cloned().unwrap_or(Value::Null),
            "failed": failed,
        }),
    )
}

/// Parse `items` (key->string map). `None` means absent (empty map); a
/// non-object or non-string value is a typed `bad_value` before anything
/// applies.
fn parse_items(v: Option<&Value>) -> Result<std::collections::BTreeMap<String, String>, ToolErr> {
    let Some(obj) = v else {
        return Ok(Default::default());
    };
    let map = obj.as_object().ok_or_else(|| {
        ToolErr::typed(
            "storage: 'items' must be an object (key->value strings)".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "items"}),
        )
    })?;
    let mut out = std::collections::BTreeMap::new();
    for (k, val) in map {
        match val.as_str() {
            Some(s) => {
                out.insert(k.clone(), s.to_string());
            }
            None => {
                return Err(ToolErr::typed(
                    format!("storage: bad item value for key '{k}' (want a string)"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "items"}),
                ));
            }
        }
    }
    Ok(out)
}

/// Write validated items into one Web Storage scope. Returns
/// (applied, failed-keys); quota/opaque write failures land in `failed`.
async fn apply_items(
    browser: &Browser,
    local: bool,
    items: &std::collections::BTreeMap<String, String>,
) -> Result<(usize, Vec<Value>), ToolErr> {
    if items.is_empty() {
        return Ok((0, Vec::new()));
    }
    let scope = if local {
        "localStorage"
    } else {
        "sessionStorage"
    };
    let store = if local {
        "localStorage"
    } else {
        "sessionStorage"
    };
    let pairs: Vec<(&String, &String)> = items.iter().collect();
    let lit = serde_json::to_string(&pairs).map_err(|e| e.to_string())?;
    let expr = format!("((pairs) => {{ const s = {store}; const failed = []; for (const [k, v] of pairs) {{ try {{ s.setItem(k, v); }} catch (e) {{ failed.push(k); }} }} return JSON.stringify({{failed}}); }})({lit})");
    let v = browser.eval(&expr).await.map_err(ToolErr::from)?;
    let keys: Vec<String> = v
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|p| {
            p.get("failed").and_then(|f| f.as_array()).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
        })
        .unwrap_or_default();
    let failed: Vec<Value> = keys
        .into_iter()
        .map(|k| json!({"scope": scope, "key": k, "reason": ToolReason::NotStored.as_str()}))
        .collect();
    Ok((items.len().saturating_sub(failed.len()), failed))
}

async fn storage_set(r: &StorageReq<'_>, args: &Value) -> Result<Value, ToolErr> {
    let StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url,
        origin,
    } = *r;
    let mut cookies = Vec::new();
    if want_cookies {
        match args.get("cookies") {
            None => {}
            Some(Value::Array(a)) => {
                for c in a {
                    cookies.push(parse_set_cookie(c)?);
                }
            }
            _ => {
                return Err(ToolErr::typed(
                    "storage: 'cookies' must be an array of {name, value, ...}".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "cookies"}),
                ));
            }
        }
    }
    let items = parse_items(args.get("items"))?;
    let has_cookies = want_cookies && !cookies.is_empty();
    let has_items = (want_local || want_session) && !items.is_empty();
    if !has_cookies && !has_items {
        return Err(ToolErr::typed(
            "storage: set needs 'cookies' and/or 'items'".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::NeedOption.as_str(), "tool": "storage"}),
        ));
    }
    if want_local || want_session {
        storage_realm_ok(browser, want_local, want_session).await?;
    }
    let (applied_cookies, mut failed) = if has_cookies {
        apply_cookies(browser, url, &cookies).await?
    } else {
        (0, Vec::new())
    };
    let (applied_local, failed_local) = if want_local {
        apply_items(browser, true, &items).await?
    } else {
        (0, Vec::new())
    };
    let (applied_session, failed_session) = if want_session {
        apply_items(browser, false, &items).await?
    } else {
        (0, Vec::new())
    };
    failed.extend(failed_local);
    failed.extend(failed_session);
    if !failed.is_empty() {
        if let Some(first) = failed.iter().find(|f| f.get("cookie").is_some()) {
            return Err(cookie_rejected(first, &failed));
        }
        return Err(ToolErr::fallback(format!(
            "storage: {} web-storage item(s) not stored",
            failed.len()
        )));
    }
    Ok(ok_text(
        json!({
            "ok": true, "action": "set", "scope": scope, "url": url, "origin": origin,
            "set": {"cookies": applied_cookies, "localStorage": applied_local, "sessionStorage": applied_session},
            "failed": [],
        })
        .to_string(),
    ))
}

async fn storage_clear(
    r: &StorageReq<'_>,
    args: &Value,
    all_sites: bool,
) -> Result<Value, ToolErr> {
    let StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url,
        origin,
    } = *r;
    let mut cleared = (0usize, 0usize, 0usize);
    if want_cookies {
        if all_sites {
            if crate::browser::cdp_url_override().is_some() && !flag(args, "confirm") {
                return Err(ToolErr::typed(
                    "storage: clear with allSites:true on an attached engine needs confirm:true (browser-wide cookie wipe)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "storage", "arg": "confirm"}),
                ));
            }
            let before = browser
                .cdp_raw("Storage.getCookies", json!({}), None)
                .await
                .map_err(ToolErr::from)?;
            cleared.0 = before
                .get("cookies")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            browser
                .cdp_raw("Storage.clearCookies", json!({}), None)
                .await
                .map_err(ToolErr::from)?;
        } else {
            let live = browser
                .command("Network.getCookies", json!({"urls": [url]}), true)
                .await
                .map_err(ToolErr::from)?;
            let jar = live
                .get("cookies")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for c in &jar {
                if let Some(name) = c.get("name").and_then(|v| v.as_str()) {
                    browser
                        .command(
                            "Network.deleteCookies",
                            json!({"name": name, "url": url}),
                            true,
                        )
                        .await
                        .map_err(ToolErr::from)?;
                    cleared.0 += 1;
                }
            }
        }
    }
    if want_local || want_session {
        storage_realm_ok(browser, want_local, want_session).await?;
        if want_local {
            let v = browser.eval("(() => { try { const n = localStorage.length; localStorage.clear(); return n; } catch (e) { return 'GLADIUS_OPAQUE'; } })()").await.map_err(ToolErr::from)?;
            if v.as_str() == Some("GLADIUS_OPAQUE") {
                return Err(opaque_err());
            }
            cleared.1 = v.as_i64().unwrap_or(0).max(0) as usize;
        }
        if want_session {
            let v = browser.eval("(() => { try { const n = sessionStorage.length; sessionStorage.clear(); return n; } catch (e) { return 'GLADIUS_OPAQUE'; } })()").await.map_err(ToolErr::from)?;
            if v.as_str() == Some("GLADIUS_OPAQUE") {
                return Err(opaque_err());
            }
            cleared.2 = v.as_i64().unwrap_or(0).max(0) as usize;
        }
    }
    let mut out = serde_json::Map::new();
    out.insert("ok".to_string(), json!(true));
    out.insert("action".to_string(), json!("clear"));
    out.insert("scope".to_string(), json!(scope));
    out.insert("url".to_string(), json!(url));
    out.insert("origin".to_string(), json!(origin));
    out.insert(
        "cleared".to_string(),
        json!({"cookies": cleared.0, "localStorage": cleared.1, "sessionStorage": cleared.2}),
    );
    let note = lifetime_note(want_session);
    if !note.is_null() {
        out.insert("note".to_string(), note);
    }
    Ok(ok_text(Value::Object(out).to_string()))
}

/// Current time as unix millis for `savedAt` (no new dependency).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn storage_save(r: &StorageReq<'_>, args: &Value, all_sites: bool) -> Result<Value, ToolErr> {
    let StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url,
        origin,
    } = *r;
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            return Err(ToolErr::typed(
                "storage: save needs required arg 'path'".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "storage"}),
            ));
        }
    };
    let dest = std::path::Path::new(&path);
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(ToolErr::typed(
                format!(
                    "storage: save parent directory does not exist: {}",
                    parent.display()
                ),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
            ));
        }
    }
    if dest.exists() && !flag(args, "overwrite") {
        return Err(ToolErr::typed(
            format!("storage: refusing to overwrite existing file (pass overwrite:true): {path}"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
        ));
    }
    let cookies: Vec<Value> = if want_cookies {
        let r = if all_sites {
            browser
                .cdp_raw("Storage.getCookies", json!({}), None)
                .await
                .map_err(ToolErr::from)?
        } else {
            browser
                .command("Network.getCookies", json!({"urls": [url]}), true)
                .await
                .map_err(ToolErr::from)?
        };
        r.get("cookies")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let (local, session) = if want_local || want_session {
        read_storage_maps(browser).await?
    } else {
        (Default::default(), Default::default())
    };
    let doc = json!({
        "gladiusStorage": 1,
        "savedAt": now_ms(),
        "url": url,
        "origin": origin,
        "cookies": if want_cookies { cookies.clone() } else { Vec::new() },
        "localStorage": if want_local { json!(local) } else { json!({}) },
        "sessionStorage": if want_session { json!(session) } else { json!({}) },
    });
    let bytes = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
    std::fs::write(dest, &bytes).map_err(|e| format!("storage: save failed: {e}"))?;
    let counts = json!({
        "cookies": cookies.len(),
        "localStorage": local.len(),
        "sessionStorage": session.len(),
    });
    Ok(ok_text(
        json!({
            "ok": true, "action": "save", "scope": scope,
            "url": url, "origin": origin,
            "counts": counts, "path": path, "bytes": bytes.len() as i64,
        })
        .to_string(),
    ))
}

async fn storage_load(r: &StorageReq<'_>, args: &Value) -> Result<Value, ToolErr> {
    let StorageReq {
        browser,
        scope,
        want_cookies,
        want_local,
        want_session,
        url,
        origin,
    } = *r;
    let path = match args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            return Err(ToolErr::typed(
                "storage: load needs required arg 'path'".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "storage"}),
            ));
        }
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolErr::typed(
                format!("storage: file not found: {path}"),
                json!({"code": ToolCode::FileNotFound.as_str(), "reason": ToolReason::MissingPath.as_str(), "tool": "storage", "path": path}),
            ));
        }
        Err(e) => return Err(ToolErr::fallback(format!("storage: load failed: {e}"))),
    };
    let doc: Value = serde_json::from_str(&text).map_err(|_| {
        ToolErr::typed(
            "storage: not valid gladius storage JSON (want {gladiusStorage:1,...})".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
        )
    })?;
    if doc.get("gladiusStorage").and_then(|v| v.as_i64()) != Some(1) {
        return Err(ToolErr::typed(
            "storage: unsupported storage version (want gladiusStorage:1)".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
        ));
    }
    let file_origin = doc
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if file_origin != origin && !flag(args, "force") {
        return Err(ToolErr::typed(
            format!("storage: origin mismatch (file {file_origin} vs current {origin}; pass force:true to apply anyway)"),
            json!({"code": ToolCode::OriginMismatch.as_str(), "reason": ToolReason::Mismatch.as_str(), "tool": "storage", "fileOrigin": file_origin, "origin": origin, "path": path}),
        ));
    }
    let mut cookies = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    if want_cookies {
        match doc.get("cookies") {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) => {
                for c in a {
                    match parse_set_cookie(c) {
                        Ok(sc) => cookies.push(sc),
                        Err(_) => failed.push(json!({
                            "cookie": c.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                            "domain": c.get("domain").and_then(|v| v.as_str()).unwrap_or(""),
                            "reason": ToolReason::BadEntry.as_str(),
                        })),
                    }
                }
            }
            _ => {
                return Err(ToolErr::typed(
                    "storage: bad file cookies (want an array)".to_string(),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
                ));
            }
        }
    }
    let file_items = |key: &str| -> Result<std::collections::BTreeMap<String, String>, ToolErr> {
        match doc.get(key) {
            None | Some(Value::Null) => Ok(Default::default()),
            Some(v) => parse_items(Some(v)).map_err(|_| {
                ToolErr::typed(
                    format!("storage: bad file {key} (want an object)"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "storage", "arg": "path"}),
                )
            }),
        }
    };
    let local_items = if want_local {
        file_items("localStorage")?
    } else {
        Default::default()
    };
    let session_items = if want_session {
        file_items("sessionStorage")?
    } else {
        Default::default()
    };
    if (want_local && !local_items.is_empty()) || (want_session && !session_items.is_empty()) {
        storage_realm_ok(browser, want_local, want_session).await?;
    }
    let (applied_cookies, failed_cookies) = if want_cookies && !cookies.is_empty() {
        apply_cookies(browser, url, &cookies).await?
    } else {
        (0, Vec::new())
    };
    failed.extend(failed_cookies);
    let (applied_local, failed_local) = if want_local {
        apply_items(browser, true, &local_items).await?
    } else {
        (0, Vec::new())
    };
    let (applied_session, failed_session) = if want_session {
        apply_items(browser, false, &session_items).await?
    } else {
        (0, Vec::new())
    };
    failed.extend(failed_local);
    failed.extend(failed_session);
    Ok(ok_text(
        json!({
            "ok": true, "action": "load", "scope": scope,
            "url": url, "origin": origin,
            "path": path, "fileOrigin": file_origin,
            "applied": {"cookies": applied_cookies, "localStorage": applied_local, "sessionStorage": applied_session},
            "failed": failed,
        })
        .to_string(),
    ))
}

/// `viewport`: set (or clear) the CDP device-metrics override on the active
/// `viewport`: set (or clear) the CDP device-metrics override on the active
/// page. The reply always carries the LIVE read-back of
/// `window.innerWidth` / `window.innerHeight` / `window.devicePixelRatio`
/// next to what was requested, so an override the engine accepted but
/// ignored shows up as `"matched": false` instead of a false success.
async fn tool_viewport(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(
        "viewport",
        args,
        &["width", "height", "deviceScaleFactor", "mobile", "reset"],
    )?;
    if args.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) {
        browser.viewport_reset().await?;
        let (w, h, dpr) = browser.viewport_readback().await?;
        return Ok(ok_text(
            json!({
                "reset": true,
                "applied": Value::Null,
                "readback": {"innerWidth": w, "innerHeight": h, "devicePixelRatio": dpr},
                "note": "override cleared; the read-back above is the engine default for this page",
            })
            .to_string(),
        ));
    }
    let dim = |name: &str| -> Result<i64, ToolErr> {
        let v = args.get(name).ok_or_else(|| {
            ToolErr::typed(
                format!("viewport: missing required arg '{name}'"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "viewport"}),
            )
        })?;
        let n = match v.as_i64() {
            Some(n) => n,
            None => match v.as_f64() {
                Some(f) if f.fract() == 0.0 => f as i64,
                _ => {
                    return Err(ToolErr::typed(
                        format!("viewport: '{name}' must be an integer, got {v}"),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "viewport"}),
                    ));
                }
            },
        };
        if n < 1 {
            return Err(ToolErr::typed(
                format!("viewport: '{name}' must be >= 1, got {n}"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "viewport"}),
            ));
        }
        Ok(n)
    };
    let width = dim("width")?;
    let height = dim("height")?;
    let dsf = match args.get("deviceScaleFactor") {
        None | Some(Value::Null) => 1.0,
        Some(v) => match v.as_f64() {
            Some(f) if f > 0.0 => f,
            _ => {
                return Err(ToolErr::typed(
                    format!("viewport: 'deviceScaleFactor' must be a number > 0, got {v}"),
                    json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadValue.as_str(), "tool": "viewport"}),
                ));
            }
        },
    };
    let mobile = args
        .get("mobile")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let params = json!({
        "width": width,
        "height": height,
        "deviceScaleFactor": dsf,
        "mobile": mobile,
    });
    browser.viewport_set(params).await?;
    let (w, h, dpr) = browser.viewport_readback().await?;
    Ok(ok_text(
        json!({
            "applied": {
                "width": width,
                "height": height,
                "deviceScaleFactor": dsf,
                "mobile": mobile,
            },
            "readback": {"innerWidth": w, "innerHeight": h, "devicePixelRatio": dpr},
            "matched": w == width && h == height,
        })
        .to_string(),
    ))
}

async fn tool_tabs(
    browser: &Browser,
    action: &str,
    url: Option<&str>,
    index: Option<i64>,
) -> Result<Value, ToolErr> {
    let to_usize = |i: Option<i64>| -> Result<usize, ToolErr> {
        match i {
            Some(n) if n >= 0 => Ok(n as usize),
            Some(n) => Err(ToolErr::typed(
                format!("tabs: bad index {n}"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::BadIndex.as_str(), "tool": "tabs"}),
            )),
            None => Err(ToolErr::typed(
                "tabs: missing required arg 'index'".to_string(),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "tabs"}),
            )),
        }
    };
    let inner = match action {
        "list" => browser.tabs_list().await.map_err(tabs_err)?,
        "new" => browser.tabs_new(url).await.map_err(tabs_err)?,
        "select" => browser
            .tabs_select(to_usize(index)?)
            .await
            .map_err(tabs_err)?,
        "close" => browser
            .tabs_close(to_usize(index)?)
            .await
            .map_err(tabs_err)?,
        other => {
            return Err(ToolErr::typed(
                format!("tabs: unknown action '{other}'"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "tabs"}),
            ))
        }
    };
    Ok(ok_text(inner.to_string()))
}

/// Engine-originated `tabs: no tab at index N` (raised in browser.rs, where
/// the tab index space lives and this file must not reach): the suffix is a
/// plain integer, never caller prose, so re-attaching the site-known type
/// here is exact. Anything else keeps the generic fallback.
fn tabs_err(e: String) -> ToolErr {
    if e.starts_with("tabs: no tab at index ") {
        ToolErr::typed(
            e,
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "tabs"}),
        )
    } else {
        ToolErr::fallback(e)
    }
}

/// `targets`: list every CDP target with its stable id plus the active
/// page's frame tree, or make a page target current by id.
async fn tool_targets(
    browser: &Browser,
    action: &str,
    target: Option<&str>,
) -> Result<Value, ToolErr> {
    match action {
        "list" => {
            let frames = browser
                .frame_tree()
                .await
                .map_err(targets_err)?
                .get("frames")
                .cloned()
                .unwrap_or(json!([]));
            let mut merged = browser.targets_all().await.map_err(targets_err)?;
            if let Some(obj) = merged.as_object_mut() {
                obj.insert("frames".to_string(), frames);
            }
            Ok(ok_text(merged.to_string()))
        }
        "select" => {
            let tid = match target {
                Some(t) => t,
                None => {
                    return Err(ToolErr::typed(
                        "targets: missing required arg 'target' (CDP targetId)".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "targets"}),
                    ));
                }
            };
            let inner = browser.select_target(tid).await.map_err(targets_err)?;
            Ok(ok_text(inner.to_string()))
        }
        other => Err(ToolErr::typed(
            format!("targets: unknown action '{other}' (list|select)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "targets"}),
        )),
    }
}

/// Engine-originated `targets:` failures (raised in browser.rs, where the
/// target/session space lives): the prefix before the first detail is
/// caller-free prose, so re-type exact known shapes here. Select of a
/// non-page target and unknown ids carry `invalid_target`; attach/frame
/// failures keep the generic fallback (CDP/js-engine territory).
fn targets_err(e: String) -> ToolErr {
    if e.starts_with("targets: no target with id ") {
        ToolErr::typed(
            e,
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "targets"}),
        )
    } else if e.contains("not a page (select takes page targets") {
        ToolErr::typed(
            e,
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotSelect.as_str(), "tool": "targets"}),
        )
    } else {
        ToolErr::fallback(e)
    }
}

/// `dialog`: report the last auto-handled dialog plus the live policy
/// (`get`), or set the default policy for the next dialog (`policy`).
async fn tool_dialog(
    browser: &Browser,
    action: &str,
    accept: Option<bool>,
    prompt_text: Option<&str>,
) -> Result<Value, ToolErr> {
    match action {
        "get" => {
            let last = match browser.last_dialog().await {
                Some(d) => json!({"type": d.dtype, "message": d.message, "url": d.url}),
                None => Value::Null,
            };
            let p = browser.dialog_policy_get().await;
            Ok(ok_text(
                json!({"dialog": last,
                       "policy": {"accept": p.accept, "promptText": p.prompt_text}})
                .to_string(),
            ))
        }
        "policy" => {
            let a = match accept {
                Some(v) => v,
                None => {
                    return Err(ToolErr::typed(
                        "dialog: 'policy' needs required arg 'accept' (bool)".to_string(),
                        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "dialog"}),
                    ));
                }
            };
            let p = browser
                .dialog_policy_set(a, prompt_text.map(|s| s.to_string()))
                .await
                .map_err(ToolErr::fallback)?;
            Ok(ok_text(
                json!({"policy": {"accept": p.accept, "promptText": p.prompt_text}}).to_string(),
            ))
        }
        other => Err(ToolErr::typed(
            format!("dialog: unknown action '{other}' (get|policy)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "dialog"}),
        )),
    }
}

/// `history`: back / forward / reload on the active page.
async fn tool_history(browser: &Browser, action: &str) -> Result<Value, ToolErr> {
    match browser.history(action).await {
        Ok(v) => Ok(ok_text(v.to_string())),
        Err(e) => Err(history_err(e)),
    }
}

/// Engine-originated `history:` failures: unknown actions and boundary
/// steps are site-known types; CDP failures keep the fallback.
fn history_err(e: String) -> ToolErr {
    if e.starts_with("history: unknown action ") {
        ToolErr::typed(
            e,
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "history"}),
        )
    } else if e.contains("(at boundary)") {
        ToolErr::typed(
            e,
            json!({"code": ToolCode::InvalidTarget.as_str(), "reason": ToolReason::NotFound.as_str(), "tool": "history"}),
        )
    } else {
        ToolErr::fallback(e)
    }
}

/// `network`: get / drain / clear the captured request/response ring, with
/// an optional plain-substring url filter and a return cap.
async fn tool_network(
    browser: &Browser,
    action: &str,
    filter: Option<&str>,
    limit: Option<i64>,
) -> Result<Value, ToolErr> {
    let lim = limit.unwrap_or(100).clamp(1, 500) as usize;
    match action {
        "get" => Ok(ok_text(browser.network_get(filter, lim).await.to_string())),
        "drain" => Ok(ok_text(
            browser.network_drain(filter, lim).await.to_string(),
        )),
        "clear" => {
            let n = browser.network_clear().await;
            Ok(ok_text(
                json!({"cleared": n, "cap": crate::browser::NET_CAP}).to_string(),
            ))
        }
        other => Err(ToolErr::typed(
            format!("network: unknown action '{other}' (get|drain|clear)"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::UnknownAction.as_str(), "tool": "network"}),
        )),
    }
}

/// `cdp`: raw protocol escape hatch. `params` defaults to `{}`; `session`
/// names one attached session (browser-level when absent). The raw CDP
/// `result` rides back verbatim; a CDP error reply is the tool's `isError`,
/// never a protocol error.
async fn tool_cdp(
    browser: &Browser,
    method: &str,
    params: Value,
    session: Option<String>,
) -> Result<Value, ToolErr> {
    if method.trim().is_empty() {
        return Err(ToolErr::typed(
            "cdp: missing required arg 'method'".to_string(),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::MissingArg.as_str(), "tool": "cdp"}),
        ));
    }
    match browser.cdp_raw(method, params, session).await {
        Ok(v) => Ok(ok_text(json!({"result": v}).to_string())),
        Err(e) => Err(ToolErr::fallback(e)),
    }
}

#[cfg(test)]
mod error_taxonomy_tests {
    use super::{ToolCode, ToolReason};

    /// The closed sets are total: every variant survives a wire round trip, so
    /// a `typed:` marker minted here is always readable back by
    /// `ToolErr::from(String)`.
    #[test]
    fn every_variant_round_trips_through_its_wire_string() {
        for code in ToolCode::ALL {
            assert_eq!(
                ToolCode::from_wire(code.as_str()),
                Some(*code),
                "{} does not round trip",
                code.as_str()
            );
        }
        for reason in ToolReason::ALL {
            assert_eq!(
                ToolReason::from_wire(reason.as_str()),
                Some(*reason),
                "{} does not round trip",
                reason.as_str()
            );
        }
    }

    /// Two variants sharing a wire string would make `from_wire` ambiguous and
    /// the marker lossy.
    #[test]
    fn wire_strings_are_unique() {
        let mut seen: Vec<&str> = ToolCode::ALL.iter().map(|c| c.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate code wire string");

        let mut seen: Vec<&str> = ToolReason::ALL.iter().map(|r| r.as_str()).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "duplicate reason wire string");
    }

    /// The marker is the inverse of the parse: what a deep site mints is what
    /// `From<String>` reads.
    #[test]
    fn marker_prefix_matches_the_minted_marker() {
        let minted = super::typed_marker(ToolCode::JsError, ToolReason::Thrown, "boom");
        assert!(minted.starts_with(&ToolCode::JsError.marker()), "{minted}");
        let err: super::ToolErr = minted.into();
        let sc = err.sc.clone();
        assert_eq!(sc.get("code").and_then(|v| v.as_str()), Some("js_error"));
        assert_eq!(sc.get("reason").and_then(|v| v.as_str()), Some("thrown"));
    }

    /// A marker naming a code this build does not know is not smuggled out:
    /// it falls back to the residue.
    #[test]
    fn an_unknown_marker_falls_back() {
        let err: super::ToolErr = super::typed_marker(ToolCode::JsError, ToolReason::Thrown, "x")
            .replace("js_error", "not_a_code")
            .into();
        assert_eq!(
            err.sc.get("code").and_then(|v| v.as_str()),
            Some("error"),
            "an unknown code is not trusted"
        );
    }
}

#[cfg(test)]
mod screenshot_file_tests {
    use super::write_screenshot_file;

    const TINY_PNG_B64: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

    fn decode_fixture() -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(TINY_PNG_B64)
            .expect("fixture must decode")
    }

    #[test]
    fn path_writes_valid_png_and_creates_missing_parents() {
        let dir =
            std::env::temp_dir().join(format!("gladius-screenshot-test-{}", std::process::id()));
        let target = dir.join("nested").join("shot.png");
        let _ = std::fs::remove_dir_all(&dir);
        if let Err(e) = write_screenshot_file(target.to_str().unwrap(), &decode_fixture()) {
            panic!("write must succeed: {}", e.prose);
        }
        let back = std::fs::read(&target).expect("file must exist");
        assert!(
            back.len() >= 8 && back[..8] == PNG_MAGIC,
            "file must start with the PNG magic bytes"
        );
        assert_eq!(back, decode_fixture(), "round trip must be exact");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_path_is_rejected() {
        let r = write_screenshot_file("relative/shot.png", &decode_fixture());
        assert!(r.is_err(), "relative path must fail closed");
    }
}


/// hand integration shim: mint the engine's own typed error envelope for a
/// failure decided outside the tool table (per-call budget, client
/// cancellation, unknown routed tool). The code/reason strings come from the
/// same closed enums the tools use, so the envelope is byte-compatible with a
/// tool-site failure.
pub fn typed_envelope(prose: &str, structured: Value) -> Value {
    ToolErr::typed(prose.to_string(), structured).envelope()
}
