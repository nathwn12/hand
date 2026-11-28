//! Page-clipboard read and write through the page's own navigator.clipboard.
//!
//! This is a hand addition to the pinned Gladius engine (like contexts,
//! trace, metrics): the tool table and the CDP call sequence are ours.
//! Read and write go through the page's navigator.clipboard via
//! Runtime.evaluate. In Chromium navigator.clipboard IS the Windows system
//! clipboard, so a write REPLACES the owner's current clipboard text: the old
//! text is not snapshotted and is not restored. No Windows clipboard API is
//! called by hand and no desktop input is synthesised. Because a write is
//! destructive it is opt-in with allow_clipboard:true; a read needs no flag.
//! Permission is granted with Browser.grantPermissions for clipboardReadWrite
//! and clipboardSanitizedWrite, scoped to the page's real origin, once per
//! origin, and only after the write policy gate has passed.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::browser::Browser;
use crate::tools::{check_unknown, ok_text, optional_string_arg, ToolCode, ToolErr, ToolReason};

/// One mebibyte, in UTF-8 bytes. Applies to a write text and to the text
/// returned by a read (which is cut at a character boundary, never mid-char).
pub(crate) const CLIPBOARD_CAP_BYTES: usize = 1024 * 1024;

/// Origins already granted. The grant is browser-level and per-origin, and a
/// repeated call must not re-grant noisily, so the memory is process-wide.
static GRANTED_ORIGINS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// The wire code the desktop engine uses for its ungated clipboard_set
/// (desktop-engine/src/gate.rs NO_CLIPBOARD_POLICY). A page-side
/// navigator.clipboard.write is the same destructive act, so it refuses with
/// the same code.
const NO_CLIPBOARD_POLICY: &str = "no_clipboard_policy";

/// Frozen tool contract for browser_clipboard.
pub fn schema() -> Value {
    json!({
        "name": "clipboard",
        "title": "Clipboard",
        "description": "Read or write the system clipboard through the page's own navigator.clipboard (Runtime.evaluate with awaitPromise:true and userGesture:true; CDP Browser.grantPermissions for clipboardReadWrite and clipboardSanitizedWrite scoped to the page's real origin). In Chromium navigator.clipboard IS the Windows clipboard: action=read returns the system clipboard text through the page API capped at 1 MiB with truncated when it was cut, while action=write REPLACES the owner's current system clipboard text - the previous contents are not snapshotted and are not restored. No Windows clipboard API is called by hand and no desktop input is synthesised. Because a write is destructive it is opt-in: pass allow_clipboard:true (default false); without it action=write is refused with typed no_clipboard_policy before any permission grant or page evaluation, and nothing is written. A write requires non-empty text capped at 1 MiB (a larger value is typed too_large before the page is touched); a rejected clipboard call is typed clipboard_unavailable with the page's own error message. context=<name> reads or writes a page of that NAMED isolated browser context (page= selects a specific page); without context= the active legacy page is used. about:blank and data: pages have no real origin and are refused.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["read", "write"]},
            "text": {"type": "string", "description": "required and non-empty for write; must be absent for read"},
            "context": {"type": "string", "description": "target a page of this named isolated browser context instead of the legacy page"},
            "page": {"type": "string", "description": "targetId of the context page; required when the context holds more than one page"},
            "allow_clipboard": {"type": "boolean", "default": false,
                "description": "opt in to action=write replacing the owner's Windows system clipboard text; without it the write is refused no_clipboard_policy and nothing is touched; action=read never needs it"}
        }, "required": ["action"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "action": {"type": "string", "enum": ["read", "write"]},
            "text": {"type": "string"},
            "chars": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "url": {"type": "string"},
            "origin": {"type": "string"},
            "systemClipboard": {"type": "boolean",
                "description": "true on every reply: the read/write went through the page's navigator.clipboard, which in Chromium is the Windows system clipboard"}
        }, "required": ["ok", "action", "chars", "url", "origin", "systemClipboard"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
    })
}

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_clipboard", "arg": arg}),
    )
}

/// The CDP permission is scoped to an origin, not a full URL. Only real
/// http(s) origins qualify: about:blank and data: (and any other opaque
/// scheme) have no origin to grant and are refused at the call site.
fn origin_of(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let (scheme, rest) = trimmed.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit('@').next().unwrap_or(authority);
    if host.is_empty() {
        return None;
    }
    Some(format!("{}://{}", scheme.to_ascii_lowercase(), host))
}

/// Grant clipboard permission once per origin. The mutex is held across the
/// CDP send so two concurrent calls on one origin cannot both grant; a second
/// call returns without sending another command.
async fn ensure_permission(browser: &Browser, origin: &str) -> Result<(), ToolErr> {
    let mut granted = GRANTED_ORIGINS.lock().await;
    if granted.contains(origin) {
        return Ok(());
    }
    let params = json!({
        "origin": origin,
        "permissions": ["clipboardReadWrite", "clipboardSanitizedWrite"]
    });
    browser
        .command("Browser.grantPermissions", params, false)
        .await
        .map_err(|e| {
            ToolErr::fallback(format!(
                "browser_clipboard: Browser.grantPermissions for {origin} failed: {e}"
            ))
        })?;
    granted.insert(origin.to_string());
    Ok(())
}

/// One Runtime.evaluate on the selected realm: the active page session for
/// the legacy page, the context page's own session when context= was used
/// (the same session eval_in_session reads). userGesture is added here
/// because the shared eval helpers do not pass it and writeText needs it.
/// exceptionDetails is always a typed clipboard_unavailable naming the
/// page's own message, never a silent empty string.
async fn evaluate_clipboard(
    browser: &Browser,
    session: Option<&str>,
    expression: &str,
) -> Result<Value, ToolErr> {
    let params = json!({
        "expression": expression,
        "awaitPromise": true,
        "userGesture": true,
        "returnByValue": true
    });
    let reply = match session {
        Some(sid) => {
            browser
                .cdp_raw("Runtime.evaluate", params, Some(sid.to_string()))
                .await
        }
        None => browser.command("Runtime.evaluate", params, true).await,
    }
    .map_err(|e| ToolErr::fallback(format!("browser_clipboard: Runtime.evaluate failed: {e}")))?;
    if let Some(ex) = reply.get("exceptionDetails") {
        let message = ex
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("unknown clipboard error")
            .to_string();
        return Err(ToolErr::typed(
            format!("browser_clipboard: navigator.clipboard rejected: {message}"),
            json!({"code": ToolCode::ClipboardUnavailable.as_str(),
                   "reason": ToolReason::Rejected.as_str(),
                   "tool": "browser_clipboard", "message": message}),
        ));
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// Cap returned read text at 1 MiB without cutting a UTF-8 character.
fn cap_text(text: &str) -> (String, bool) {
    if text.len() <= CLIPBOARD_CAP_BYTES {
        return (text.to_string(), false);
    }
    let mut end = CLIPBOARD_CAP_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Dispatcher entry point, called from tools::call.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(
        "clipboard",
        args,
        &["action", "text", "context", "page", "allow_clipboard"],
    )?;
    let action = args.get("action").and_then(Value::as_str).ok_or_else(|| {
        invalid(
            "browser_clipboard: missing required arg 'action'".to_string(),
            ToolReason::MissingArg,
            "action",
        )
    })?;
    if !matches!(action, "read" | "write") {
        return Err(invalid(
            format!("browser_clipboard: unknown action '{action}' (want 'read' or 'write')"),
            ToolReason::UnknownAction,
            "action",
        ));
    }
    let context = optional_string_arg("browser_clipboard", args, "context")?;
    let page = optional_string_arg("browser_clipboard", args, "page")?;
    if page.is_some() && context.is_none() {
        return Err(invalid(
            "browser_clipboard: 'page' names a page inside a named context; pass 'context' with it"
                .to_string(),
            ToolReason::MissingArg,
            "context",
        ));
    }

    let text_arg = args.get("text");
    let text: Option<String> = match text_arg {
        None => None,
        Some(Value::String(s)) if s.is_empty() => {
            return Err(invalid(
                "browser_clipboard: write requires non-empty 'text'".to_string(),
                ToolReason::BadValue,
                "text",
            ));
        }
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return Err(invalid(
                format!("browser_clipboard: 'text' must be a string, got {other}"),
                ToolReason::BadValue,
                "text",
            ));
        }
    };
    if action == "read" && text_arg.is_some() {
        return Err(invalid(
            "browser_clipboard: 'text' is not accepted by read".to_string(),
            ToolReason::BadValue,
            "text",
        ));
    }
    if action == "write" && text.is_none() {
        return Err(invalid(
            "browser_clipboard: write requires non-empty 'text'".to_string(),
            ToolReason::MissingArg,
            "text",
        ));
    }
    // Size gate before any page or permission call: an oversized write is
    // refused even on an unlaunched engine.
    if let Some(t) = &text {
        if t.len() > CLIPBOARD_CAP_BYTES {
            return Err(ToolErr::typed(
                format!(
                    "browser_clipboard: write text is {} bytes, over the {} byte (1 MiB) cap",
                    t.len(),
                    CLIPBOARD_CAP_BYTES
                ),
                json!({"code": ToolCode::TooLarge.as_str(), "reason": ToolReason::BadValue.as_str(),
                       "tool": "browser_clipboard", "arg": "text",
                       "bytes": t.len(), "cap": CLIPBOARD_CAP_BYTES}),
            ));
        }
    }

    // Clipboard policy: a navigator.clipboard.write is a Windows system
    // clipboard write, so it is opt-in. Refuse before any context lookup,
    // URL evaluation, permission grant or clipboard evaluation. The size gate
    // above still wins so an oversized unflagged write stays typed too_large.
    if action == "write"
        && !args
            .get("allow_clipboard")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Err(ToolErr::typed(
            "browser_clipboard: action=write requires allow_clipboard:true; without it nothing is written. A write through the page's navigator.clipboard writes the Windows system clipboard and would replace the owner's current system clipboard text (it is not snapshotted and not restored)."
                .to_string(),
            json!({"code": NO_CLIPBOARD_POLICY, "reason": ToolReason::Rejected.as_str(),
                   "tool": "browser_clipboard", "arg": "allow_clipboard"}),
        ));
    }

    let page_ctx = match context.as_deref() {
        Some(name) => Some(
            browser
                .context_page(name, page.as_deref())
                .await
                .map_err(ToolErr::from)?,
        ),
        None => None,
    };
    let url = match &page_ctx {
        Some(cp) => cp.url.clone(),
        None => browser
            .eval("location.href")
            .await
            .map_err(ToolErr::from)?
            .as_str()
            .unwrap_or("")
            .to_string(),
    };
    let origin = origin_of(&url).ok_or_else(|| {
        ToolErr::typed(
            format!(
                "browser_clipboard: page '{url}' has no real origin (about:blank and data: URLs are opaque); navigate to an http(s) page with a real origin first"
            ),
            json!({"code": ToolCode::InvalidTarget.as_str(),
                   "reason": ToolReason::OpaqueOrigin.as_str(),
                   "tool": "browser_clipboard", "url": url}),
        )
    })?;
    ensure_permission(browser, &origin).await?;

    let expression = match action {
        "write" => format!(
            "navigator.clipboard.writeText({})",
            json!(text.as_deref().unwrap_or(""))
        ),
        _ => "navigator.clipboard.readText()".to_string(),
    };
    let session = page_ctx.as_ref().map(|cp| cp.session.as_str());
    let result = evaluate_clipboard(browser, session, &expression).await?;

    let mut body = json!({
        "ok": true, "action": action, "chars": 0, "truncated": false,
        "url": url, "origin": origin, "systemClipboard": true
    });
    if action == "read" {
        let value = result.get("value").and_then(Value::as_str).ok_or_else(|| {
            ToolErr::typed(
                "browser_clipboard: navigator.clipboard.readText() returned no string".to_string(),
                json!({"code": ToolCode::ClipboardUnavailable.as_str(),
                       "reason": ToolReason::UndefinedResult.as_str(),
                       "tool": "browser_clipboard"}),
            )
        })?;
        let (shown, truncated) = cap_text(value);
        let chars = shown.chars().count();
        let obj = body.as_object_mut().expect("reply object");
        obj.insert("text".to_string(), json!(shown));
        obj.insert("chars".to_string(), json!(chars));
        obj.insert("truncated".to_string(), json!(truncated));
    } else {
        let chars = text.as_deref().unwrap_or("").chars().count();
        body["chars"] = json!(chars);
    }
    Ok(ok_text(body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::call;
    use std::collections::HashSet as TestSet;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The crate's loopback fixture pattern: a bound local listener serving a
    /// real page so navigator.clipboard runs in a secure (trustworthy) origin.
    async fn serve_fixture() -> (String, tokio::task::JoinHandle<()>) {
        let ln = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind");
        let port = ln.local_addr().expect("loopback addr").port();
        let srv = tokio::spawn(async move {
            loop {
                let (mut s, _) = match ln.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = vec![0u8; 4096];
                let mut req = Vec::new();
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            req.extend_from_slice(&buf[..n]);
                            if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 8192 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let body = "<!doctype html><html><body><p>clipboard fixture</p></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n{}",
                    body.len(), body
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://127.0.0.1:{port}/fixture"), srv)
    }

    #[test]
    fn clipboard_schema_is_closed_and_opt_in() {
        let s = schema();
        assert_eq!(s["name"], "clipboard");
        assert_eq!(s["inputSchema"]["additionalProperties"], false);
        assert_eq!(s["inputSchema"]["required"], json!(["action"]));
        assert_eq!(
            s["inputSchema"]["properties"]["action"]["enum"],
            json!(["read", "write"])
        );
        let keys: TestSet<&String> = s["inputSchema"]["properties"]
            .as_object()
            .expect("properties object")
            .keys()
            .collect();
        let want: TestSet<&str> = ["action", "text", "context", "page", "allow_clipboard"]
            .into_iter()
            .collect();
        assert_eq!(
            keys.iter().map(|k| k.as_str()).collect::<TestSet<&str>>(),
            want
        );
        assert!(
            crate::tools::RUN_TOOLS.contains(&"clipboard"),
            "browser_run must batch clipboard"
        );
    }

    #[tokio::test]
    async fn clipboard_write_without_text_is_invalid_args() {
        let browser = Browser::new();
        let missing = call("clipboard", &json!({"action": "write"}), &browser).await;
        assert_eq!(
            missing["structuredContent"]["code"], "invalid_args",
            "{missing}"
        );
        assert_eq!(
            missing["structuredContent"]["reason"], "missing_arg",
            "{missing}"
        );
        let with_text = call(
            "clipboard",
            &json!({"action": "read", "text": "must not be passed"}),
            &browser,
        )
        .await;
        assert_eq!(
            with_text["structuredContent"]["code"], "invalid_args",
            "{with_text}"
        );
        let unknown = call(
            "clipboard",
            &json!({"action": "write", "text": "x", "bogus": 1}),
            &browser,
        )
        .await;
        assert_eq!(
            unknown["structuredContent"]["code"], "invalid_args",
            "{unknown}"
        );
    }

    #[tokio::test]
    async fn clipboard_write_over_cap_is_too_large_before_touching_page() {
        // An unlaunched engine: any page or permission call would fail here,
        // so a typed too_large proves the cap gate runs first.
        let browser = Browser::new();
        let big = "x".repeat(CLIPBOARD_CAP_BYTES + 1);
        let reply = call(
            "clipboard",
            &json!({"action": "write", "text": big}),
            &browser,
        )
        .await;
        assert_eq!(reply["structuredContent"]["code"], "too_large", "{reply}");
        assert_eq!(
            reply["structuredContent"]["cap"], CLIPBOARD_CAP_BYTES,
            "{reply}"
        );
    }

    #[tokio::test]
    async fn clipboard_about_blank_is_invalid_target() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let browser = Browser::new();
        let reply = call("clipboard", &json!({"action": "read"}), &browser).await;
        let _ = browser.close().await;
        assert_eq!(
            reply["structuredContent"]["code"], "invalid_target",
            "{reply}"
        );
        assert_eq!(
            reply["structuredContent"]["reason"], "opaque_origin",
            "{reply}"
        );
        let prose = reply["content"][0]["text"].as_str().expect("prose");
        assert!(prose.contains("real origin"), "{prose}");
    }

    #[tokio::test]
    async fn clipboard_rejected_promise_is_typed_clipboard_unavailable() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let stub = browser
            .eval(
                "Object.defineProperty(navigator.clipboard, 'readText', {configurable: true, \
                 value: function(){ return Promise.reject(new Error('denied by the page')); }}); true",
            )
            .await;
        assert!(stub.is_ok(), "could not stub the page clipboard: {stub:?}");
        let reply = call("clipboard", &json!({"action": "read"}), &browser).await;
        srv.abort();
        let _ = browser.close().await;
        assert_eq!(
            reply["structuredContent"]["code"], "clipboard_unavailable",
            "{reply}"
        );
        let prose = reply["content"][0]["text"].as_str().expect("prose");
        assert!(prose.contains("denied by the page"), "{prose}");
    }

    #[tokio::test]
    async fn write_without_allow_clipboard_is_refused_before_the_page() {
        // Cold engine: any page, origin or permission call would fail here, so
        // a typed policy refusal proves the gate runs before the page.
        let cold = Browser::new();
        let refused = call(
            "clipboard",
            &json!({"action": "write", "text": "SHOULD-NOT-LAND"}),
            &cold,
        )
        .await;
        assert_eq!(
            refused["structuredContent"]["code"], "no_clipboard_policy",
            "{refused}"
        );
        let prose = refused["content"][0]["text"].as_str().expect("prose");
        assert!(prose.contains("allow_clipboard"), "{prose}");
        assert!(prose.contains("replace the owner"), "{prose}");
        assert!(prose.contains("system clipboard"), "{prose}");

        // With a real page, seed the clipboard, then prove an unflagged write
        // leaves the page's (Windows) clipboard text untouched.
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let seeded = call(
            "clipboard",
            &json!({"action": "write", "text": "SEED", "allow_clipboard": true}),
            &browser,
        )
        .await;
        assert!(seeded.get("isError").is_none(), "seed errored: {seeded}");
        let refused = call(
            "clipboard",
            &json!({"action": "write", "text": "SHOULD-NOT-LAND"}),
            &browser,
        )
        .await;
        assert_eq!(
            refused["structuredContent"]["code"], "no_clipboard_policy",
            "{refused}"
        );
        let after = call("clipboard", &json!({"action": "read"}), &browser).await;
        srv.abort();
        let _ = browser.close().await;
        assert_eq!(
            after["structuredContent"]["text"], "SEED",
            "unflagged write must not touch the clipboard: {after}"
        );
    }

    #[tokio::test]
    async fn write_with_allow_clipboard_round_trips() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let text = "héllo, page clipboard ✓ 123";
        let written = call(
            "clipboard",
            &json!({"action": "write", "text": text, "allow_clipboard": true}),
            &browser,
        )
        .await;
        assert!(written.get("isError").is_none(), "write errored: {written}");
        assert_eq!(written["structuredContent"]["action"], "write", "{written}");
        assert_eq!(
            written["structuredContent"]["origin"],
            origin_of(&url).unwrap(),
            "{written}"
        );
        assert_eq!(
            written["structuredContent"]["systemClipboard"], true,
            "{written}"
        );
        let read = call("clipboard", &json!({"action": "read"}), &browser).await;
        srv.abort();
        let _ = browser.close().await;
        assert!(read.get("isError").is_none(), "read errored: {read}");
        let sc = &read["structuredContent"];
        assert_eq!(sc["text"], text, "round trip must be exact: {read}");
        assert_eq!(sc["chars"], text.chars().count(), "{read}");
        assert_eq!(sc["truncated"], false, "{read}");
        assert_eq!(sc["url"], url, "{read}");
        assert_eq!(sc["systemClipboard"], true, "{read}");
    }

    #[tokio::test]
    async fn read_needs_no_flag_and_reports_the_system_clipboard() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let seeded = call(
            "clipboard",
            &json!({"action": "write", "text": "READ-ME", "allow_clipboard": true}),
            &browser,
        )
        .await;
        assert!(seeded.get("isError").is_none(), "seed errored: {seeded}");
        // No allow_clipboard on the read: a read is non-destructive.
        let read = call("clipboard", &json!({"action": "read"}), &browser).await;
        srv.abort();
        let _ = browser.close().await;
        assert!(read.get("isError").is_none(), "read errored: {read}");
        let sc = &read["structuredContent"];
        assert_eq!(sc["text"], "READ-ME", "read must work with no flag: {read}");
        assert_eq!(
            sc["systemClipboard"], true,
            "reply must name the system clipboard: {read}"
        );
    }

    #[test]
    fn clipboard_description_claims_the_system_clipboard_and_the_flag() {
        let s = schema();
        let desc = s["description"]
            .as_str()
            .expect("description")
            .to_ascii_lowercase();
        assert!(desc.contains("navigator.clipboard"), "{desc}");
        assert!(desc.contains("windows clipboard"), "{desc}");
        assert!(desc.contains("system clipboard"), "{desc}");
        assert!(desc.contains("replaces the owner"), "{desc}");
        assert!(desc.contains("not snapshotted"), "{desc}");
        assert!(desc.contains("not restored"), "{desc}");
        assert!(desc.contains("no windows clipboard api"), "{desc}");
        assert!(desc.contains("allow_clipboard:true"), "{desc}");
        assert!(desc.contains("no_clipboard_policy"), "{desc}");
        assert!(!desc.contains("never touches"), "{desc}");
        assert!(
            !desc.contains("never calls a system clipboard api"),
            "{desc}"
        );

        let props = &s["inputSchema"]["properties"];
        assert_eq!(props["allow_clipboard"]["type"], "boolean");
        assert_eq!(props["allow_clipboard"]["default"], false);
        let out = &s["outputSchema"]["properties"];
        assert_eq!(out["systemClipboard"]["type"], "boolean");
    }
}
