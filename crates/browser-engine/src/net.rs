//! Network interception for row B3: `route`, `mock`, `abort`, `offline`,
//! `restore`, scoped to one browser context.
//!
//! The CDP parameter names below were read from the PINNED binary's own domain
//! definitions (`http://127.0.0.1:<port>/json/protocol` on Chrome for Testing
//! 158.0.8127.36), not guessed:
//! - `Fetch.enable({patterns:[{urlPattern,resourceType,requestStage}]})`
//! - `Fetch.fulfillRequest({requestId,responseCode,responseHeaders,body})` where
//!   `body` is a BASE64 binary and headers are `{name,value}` entries;
//! - `Fetch.failRequest({requestId,errorReason})`, `Network.ErrorReason`
//!   includes `Failed`;
//! - `Fetch.continueRequest({requestId})`;
//! - `Network.emulateNetworkConditions({offline,latency,downloadThroughput,
//!   uploadThroughput})` (throughput `-1` means "no throttle" on restore);
//! - `Network.setBlockedURLs({urlPatterns})` (`urls` is deprecated in 158).
//!
//! `Fetch.requestPaused` is captured by the reader in `crate::browser` with its
//! whole envelope, so each reply goes back on the event's own `sessionId`. A
//! paused request left unanswered freezes the page's `fetch()` for ever.
//!
//! State is keyed by `Browser::id()`, never by a pointer address: a fresh
//! `Browser` reusing a dropped one's address must not inherit its routes.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use crate::browser::{Browser, CdpSender};
use crate::tools::{check_unknown, ok_text, ToolCode, ToolErr, ToolReason};

/// Row id this module owns. Kept for the scaffold's contract test.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B3";

const TOOL: &str = "network";

/// All arguments the interception contract accepts. The five actions route
/// here before the legacy network ring's narrower arg check in `tools.rs`.
const ALLOWED: &[&str] = &[
    "action",
    "pattern",
    "url",
    "body",
    "status",
    "headers",
    "context",
    "offline",
    "blockedUrls",
];

/// Frozen input/output contract for row B3. `blockedUrls` is the one optional
/// parameter added by the reconciliation: a context-scoped
/// `Network.setBlockedURLs` so fixture loads can skip image/font classes.
#[cfg_attr(not(test), allow(dead_code))]
pub fn schema() -> Value {
    json!({
        "name": "network",
        "title": "Browser network interception",
        "description": "Route, mock, abort, offline and restore, scoped to one browser context. \
            A route lives on its context and is removed when that context is torn down.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["route", "mock", "abort", "offline", "restore"]},
            "pattern": {"type": "string"},
            "url": {"type": "string"},
            "body": {},
            "status": {"type": "integer", "minimum": 100},
            "headers": {"type": "object", "additionalProperties": {"type": "string"}},
            "context": {"type": "string"},
            "offline": {"type": "boolean"},
            "blockedUrls": {"type": "array", "items": {"type": "string"}}
        }, "required": ["action"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "action": {"type": "string"},
            "routeId": {"type": "string"},
            "context": {"type": "string"},
            "removed": {"type": "integer", "minimum": 0},
            "offline": {"type": "boolean"}
        }, "required": ["ok"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
    })
}

#[derive(Clone)]
enum RuleKind {
    /// A pass-through route: leave it paused only long enough to be counted.
    Continue,
    Mock {
        body: String,
        status: i64,
        headers: Vec<(String, String)>,
    },
    Abort,
}

#[allow(dead_code)] // `id` is the stable route receipt; `session` is the CDP target.
#[derive(Clone)]
struct Rule {
    id: String,
    context: String,
    session: String,
    pattern: String,
    kind: RuleKind,
}

/// One context's intercept state. `armed`, `offline` and `blocked` are keyed by
/// context key ("" = legacy default page): they name the CDP session each
/// setting was applied to, so teardown can undo exactly that session's state.
#[derive(Default)]
struct NetState {
    rules: Vec<Rule>,
    armed: HashMap<String, String>,
    offline: HashMap<String, String>,
    blocked: HashMap<String, String>,
    /// Context name -> browserContextId, published by `crate::contexts` when
    /// the context is created. Lets a caller name a context and still have its
    /// route removed when that context dies.
    names: HashMap<String, String>,
    seq: u64,
}

static NET_STATES: OnceLock<Mutex<HashMap<u64, std::sync::Arc<AsyncMutex<NetState>>>>> = OnceLock::new();

fn net_state(browser: &Browser) -> std::sync::Arc<AsyncMutex<NetState>> {
    let all = NET_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut all = all.lock().unwrap_or_else(|e| e.into_inner());
    all.entry(browser.id())
        .or_insert_with(|| std::sync::Arc::new(AsyncMutex::new(NetState::default())))
        .clone()
}

fn err(code: ToolCode, reason: ToolReason, prose: impl Into<String>) -> ToolErr {
    ToolErr::typed(
        prose.into(),
        json!({"code": code.as_str(), "reason": reason.as_str(), "tool": TOOL}),
    )
}

fn invalid(prose: impl Into<String>) -> ToolErr {
    err(ToolCode::InvalidArgs, ToolReason::BadValue, prose)
}

fn missing(arg: &str, action: &str) -> ToolErr {
    err(
        ToolCode::InvalidArgs,
        ToolReason::MissingArg,
        format!("network: {action} needs '{arg}'"),
    )
}

fn no_engine(action: &str) -> ToolErr {
    err(
        ToolCode::EngineDead,
        ToolReason::Transport,
        format!("network: no live engine for {action}; nothing was intercepted"),
    )
}

fn engine_err(e: String) -> ToolErr {
    err(ToolCode::EngineDead, ToolReason::Transport, e)
}

/// Success must use the shared `{content, structuredContent}` envelope that
/// every other tool returns; a raw `Value` would leave clients without the
/// parsed body (and is what the first focused gate caught).
fn ok(v: Value) -> Result<Value, ToolErr> {
    Ok(ok_text(v.to_string()))
}

/// Send a session-scoped command to the target and return the session used.
/// The default page goes through the engine's active-session path; a named
/// context needs its own attached session (the legacy path never selects it).
async fn target_session(
    browser: &Browser,
    key: &str,
    method: &str,
    params: Value,
) -> Result<String, ToolErr> {
    if key.is_empty() {
        let (session, _) = browser.cdp_session(method, params).await.map_err(engine_err)?;
        return Ok(session);
    }
    let session = browser.session_for_context(key).await.map_err(engine_err)?;
    let sender = browser
        .cdp_sender()
        .await
        .ok_or_else(|| no_engine(method))?;
    sender
        .send(method, params, Some(session.clone()))
        .await
        .map_err(engine_err)?;
    Ok(session)
}

/// Resolve the caller's optional `context` to the key the state is stored
/// under. A bound context name maps to its browserContextId, so teardown by id
/// can find the route. An unbound name is kept verbatim (still scoped to that
/// caller's own key, never mixed with the default).
fn context_key(state: &NetState, context: &str) -> String {
    if context.is_empty() {
        String::new()
    } else {
        state
            .names
            .get(context)
            .cloned()
            .unwrap_or_else(|| context.to_string())
    }
}

/// `*` matches zero or more characters, `?` exactly one. CDP's own URL
/// patterns use the same wildcards; this matcher decides which rule answers a
/// paused request, so `Fetch.continueRequest` is the fallback, never a guess.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut prev = vec![false; t.len() + 1];
    prev[0] = true;
    for pc in p {
        let mut cur = vec![false; t.len() + 1];
        match pc {
            '*' => {
                cur[0] = prev[0];
                for i in 1..=t.len() {
                    cur[i] = prev[i] || cur[i - 1];
                }
            }
            '?' => {
                for i in 1..=t.len() {
                    cur[i] = prev[i - 1];
                }
            }
            c => {
                for i in 1..=t.len() {
                    cur[i] = prev[i - 1] && t[i - 1] == c;
                }
            }
        }
        prev = cur;
    }
    prev[t.len()]
}

/// A JSON object body is serialised (the manifest passes an object); a string
/// body is used verbatim. Either way CDP gets text, then base64 below.
fn body_text(body: Option<&Value>) -> String {
    match body {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

fn header_pairs(headers: Option<&Value>, json_body: bool) -> Result<Vec<(String, String)>, ToolErr> {
    let mut out = Vec::new();
    if let Some(v) = headers {
        let obj = v.as_object().ok_or_else(|| invalid("network: 'headers' must be an object"))?;
        for (k, val) in obj {
            let s = val
                .as_str()
                .ok_or_else(|| invalid("network: each header value must be a string"))?;
            out.push((k.clone(), s.to_string()));
        }
    }
    if json_body && !out.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
        out.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    Ok(out)
}

/// Dispatcher entry point used by `tools.rs`'s `network` arm.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(TOOL, args, ALLOWED)?;
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| missing("action", "call"))?
        .to_string();
    match action.as_str() {
        "route" => arm(browser, args, &action, RuleKind::Continue, true).await,
        "mock" => {
            let body = body_text(args.get("body"));
            let status = args
                .get("status")
                .and_then(|v| v.as_i64())
                .unwrap_or(200);
            if status < 100 {
                return Err(invalid("network: 'status' must be at least 100"));
            }
            let is_json = args.get("body").is_some_and(|v| v.is_object() || v.is_array());
            let headers = header_pairs(args.get("headers"), is_json)?;
            arm(
                browser,
                args,
                &action,
                RuleKind::Mock {
                    body,
                    status,
                    headers,
                },
                false,
            )
            .await
        }
        "abort" => arm(browser, args, &action, RuleKind::Abort, false).await,
        "offline" => offline(browser, args).await,
        "restore" => restore(browser, args).await,
        other => Err(err(
            ToolCode::InvalidArgs,
            ToolReason::UnknownAction,
            format!("network: unknown action '{other}'"),
        )),
    }
}

/// Register a rule and enable `Fetch` on the target session if this is the
/// first rule for that context. `allow_pattern` lets `route` name a `pattern`;
/// `mock`/`abort` take `url` (falling back to `pattern` for symmetry).
async fn arm(
    browser: &Browser,
    args: &Value,
    action: &str,
    kind: RuleKind,
    allow_pattern: bool,
) -> Result<Value, ToolErr> {
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let raw_pattern = if allow_pattern {
        args.get("pattern")
            .or_else(|| args.get("url"))
            .and_then(|v| v.as_str())
    } else {
        args.get("url")
            .or_else(|| args.get("pattern"))
            .and_then(|v| v.as_str())
    }
    .ok_or_else(|| missing(if allow_pattern { "pattern" } else { "url" }, action))?;
    if raw_pattern.is_empty() {
        return Err(invalid(format!("network: {action} needs a non-empty URL pattern")));
    }
    let pattern = raw_pattern.to_string();

    let state = net_state(browser);
    let (key, session, is_new, id) = {
        let mut st = state.lock().await;
        let key = context_key(&st, context);
        let session = match st.armed.get(&key) {
            Some(s) => s.clone(),
            None => {
                let s = target_session(
                    browser,
                    &key,
                    "Fetch.enable",
                    json!({"patterns": [{"urlPattern": "*"}]}),
                )
                .await?;
                st.armed.insert(key.clone(), s.clone());
                s
            }
        };
        st.seq += 1;
        let id = format!("route-{}", st.seq);
        st.rules.push(Rule {
            id: id.clone(),
            context: key.clone(),
            session: session.clone(),
            pattern: pattern.clone(),
            kind,
        });
        let is_new = st.armed.len() == 1 && st.rules.len() == 1;
        (key, session, is_new, id)
    };

    if is_new {
        let Some(sender) = browser.cdp_sender().await else {
            return Err(no_engine(action));
        };
        tokio::spawn(consume_paused(sender, state.clone()));
    }
    // `session` is bound above so teardown knows where to send Fetch.disable;
    // the rule keeps it too, but keep the local use explicit for the receipt.
    let _ = &session;

    apply_blocked_urls(browser, args, &key).await?;

    ok(json!({
        "ok": true, "action": action, "routeId": id, "context": context,
        "pattern": pattern, "session": session, "scope": if key.is_empty() {
            "legacy default page"
        } else {
            "named browser context"
        },
    }))
}

/// The one optional parameter from the reconciliation: a list of URL patterns
/// to block at the network layer for this context. Chrome 158's preferred key
/// is `urlPatterns`; the deprecated `urls` key is not used.
async fn apply_blocked_urls(browser: &Browser, args: &Value, key: &str) -> Result<(), ToolErr> {
    let Some(value) = args.get("blockedUrls") else {
        return Ok(());
    };
    let patterns = value
        .as_array()
        .ok_or_else(|| invalid("network: 'blockedUrls' must be an array of strings"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid("network: each blockedUrls entry must be a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let session = target_session(
        browser,
        key,
        "Network.setBlockedURLs",
        json!({"urlPatterns": patterns}),
    )
    .await?;
    net_state(browser)
        .lock()
        .await
        .blocked
        .insert(key.to_string(), session);
    Ok(())
}

/// Offline mode is a CDP emulation on the target page session, not a JS shim.
/// `offline:false` is the same command turned off: no separate verb.
async fn offline(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let offline = args.get("offline").and_then(|v| v.as_bool()).unwrap_or(true);
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let key = context_key(&*net_state(browser).lock().await, context);
    let session = target_session(
        browser,
        &key,
        "Network.emulateNetworkConditions",
        json!({
            "offline": offline,
            "latency": 0,
            "downloadThroughput": if offline { 0 } else { -1 },
            "uploadThroughput": if offline { 0 } else { -1 },
        }),
    )
    .await?;
    let state = net_state(browser);
    let mut st = state.lock().await;
    if offline {
        st.offline.insert(key.clone(), session.clone());
    } else {
        st.offline.remove(&key);
    }
    ok(json!({
        "ok": true, "action": "offline", "offline": offline,
        "context": context, "session": session,
    }))
}

/// Restore disables `Fetch`, lifts offline emulation and clears blocked URLs on
/// exactly the sessions this tool armed. A `context` restricts it to that
/// context; without one it restores everything.
async fn restore(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let context = args.get("context").and_then(|v| v.as_str()).unwrap_or("");
    let state = net_state(browser);
    let (key, fetch_sessions, offline_sessions, blocked_sessions, removed) = {
        let st = state.lock().await;
        let key = context_key(&st, context);
        let want = |k: &String| key.is_empty() || *k == key;
        let fetch_sessions = st
            .armed
            .iter()
            .filter(|(k, _)| want(k))
            .map(|(_, s)| s.clone())
            .collect::<Vec<_>>();
        let offline_sessions = st
            .offline
            .iter()
            .filter(|(k, _)| want(k))
            .map(|(_, s)| s.clone())
            .collect::<Vec<_>>();
        let blocked_sessions = st
            .blocked
            .iter()
            .filter(|(k, _)| want(k))
            .map(|(_, s)| s.clone())
            .collect::<Vec<_>>();
        let removed = st.rules.iter().filter(|r| want(&r.context)).count();
        (key, fetch_sessions, offline_sessions, blocked_sessions, removed)
    };

    for session in fetch_sessions {
        let _ = browser
            .cdp_if_live("Fetch.disable", json!({}), Some(session))
            .await;
    }
    for session in offline_sessions {
        let _ = browser
            .cdp_if_live(
                "Network.emulateNetworkConditions",
                json!({"offline": false, "latency": 0,
                       "downloadThroughput": -1, "uploadThroughput": -1}),
                Some(session),
            )
            .await;
    }
    for session in blocked_sessions {
        let _ = browser
            .cdp_if_live("Network.setBlockedURLs", json!({"urlPatterns": []}), Some(session))
            .await;
    }

    let mut st = state.lock().await;
    st.rules.retain(|r| !(key.is_empty() || r.context == key));
    st.armed.retain(|k, _| !(key.is_empty() || *k == key));
    st.offline.retain(|k, _| !(key.is_empty() || *k == key));
    st.blocked.retain(|k, _| !(key.is_empty() || *k == key));
    drop(st);

    ok(json!({
        "ok": true, "action": "restore", "removed": removed,
        "offline": false, "context": context,
    }))
}

/// Bind a context name to its `browserContextId` so a route armed by name can
/// be removed when the context is torn down. Called by `crate::contexts` at
/// creation time; no engine contact.
pub(crate) async fn bind_context(browser: &Browser, name: &str, context_id: &str) {
    net_state(browser)
        .lock()
        .await
        .names
        .insert(name.to_string(), context_id.to_string());
}

/// Called by every verified context teardown path. Removes that context's
/// rules and undoes its CDP settings on the sessions it used, and leaves every
/// other context's interception untouched. The registry is the authority: a
/// caller whose future is dropped mid-teardown cannot lose a route, because
/// only a verified removal changes the state.
pub(crate) async fn on_context_gone(browser: &Browser, context_id: &str) -> usize {
    let state = net_state(browser);
    let (fetch, offline, blocked, removed) = {
        let mut st = state.lock().await;
        let mut fetch = Vec::new();
        let mut offline = Vec::new();
        let mut blocked = Vec::new();
        st.armed.retain(|k, s| {
            if k == context_id {
                fetch.push(s.clone());
                false
            } else {
                true
            }
        });
        st.offline.retain(|k, s| {
            if k == context_id {
                offline.push(s.clone());
                false
            } else {
                true
            }
        });
        st.blocked.retain(|k, s| {
            if k == context_id {
                blocked.push(s.clone());
                false
            } else {
                true
            }
        });
        let before = st.rules.len();
        st.rules.retain(|r| r.context != context_id);
        st.names.retain(|_, v| v != context_id);
        (fetch, offline, blocked, before - st.rules.len())
    };
    for session in fetch {
        let _ = browser
            .cdp_if_live("Fetch.disable", json!({}), Some(session))
            .await;
    }
    for session in offline {
        let _ = browser
            .cdp_if_live(
                "Network.emulateNetworkConditions",
                json!({"offline": false, "latency": 0,
                       "downloadThroughput": -1, "uploadThroughput": -1}),
                Some(session),
            )
            .await;
    }
    for session in blocked {
        let _ = browser
            .cdp_if_live("Network.setBlockedURLs", json!({"urlPatterns": []}), Some(session))
            .await;
    }
    removed
}

/// Background consumer. One per live interception epoch; it exits when
/// `restore`/teardown empties `armed`, so no task outlives its routes. The
/// 25 ms bounded wait also covers a lost `notify_waiters` when no waiter was
/// registered yet.
async fn consume_paused(sender: CdpSender, state: std::sync::Arc<AsyncMutex<NetState>>) {
    let notify = sender.fetch_notify();
    loop {
        for event in sender.take_fetch_events().await {
            respond_paused(&sender, &state, &event).await;
        }
        if state.lock().await.armed.is_empty() {
            break;
        }
        let _ = tokio::time::timeout(Duration::from_millis(25), notify.notified()).await;
    }
    // Drain any event that landed between the last check and the exit.
    for event in sender.take_fetch_events().await {
        respond_paused(&sender, &state, &event).await;
    }
}

async fn respond_paused(sender: &CdpSender, state: &std::sync::Arc<AsyncMutex<NetState>>, event: &Value) {
    let session = event
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let params = event.get("params").unwrap_or(&Value::Null);
    let Some(request_id) = params.get("requestId").and_then(|v| v.as_str()) else {
        return;
    };
    let url = params
        .get("request")
        .and_then(|r| r.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    // Match the session too: two contexts may arm the same URL pattern, and a
    // pause from one must never be answered by the other context's rule.
    let kind = {
        let st = state.lock().await;
        st.rules
            .iter()
            .find(|r| {
                session.as_deref().is_none_or(|s| r.session == s) && glob_match(&r.pattern, url)
            })
            .map(|r| r.kind.clone())
    };

    let (method, payload) = match kind {
        Some(RuleKind::Mock {
            body,
            status,
            headers,
        }) => {
            let response_headers: Vec<Value> = headers
                .into_iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect();
            let encoded = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
            (
                "Fetch.fulfillRequest",
                json!({"requestId": request_id, "responseCode": status,
                       "responseHeaders": response_headers, "body": encoded}),
            )
        }
        Some(RuleKind::Abort) => (
            "Fetch.failRequest",
            json!({"requestId": request_id, "errorReason": "Failed"}),
        ),
        Some(RuleKind::Continue) | None => {
            ("Fetch.continueRequest", json!({"requestId": request_id}))
        }
    };
    let _ = sender.send(method, payload, session).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Browser;

    #[test]
    fn glob_matches_cdp_style_wildcards() {
        assert!(glob_match("*/tasks/network.json", "http://x/tasks/network.json"));
        assert!(glob_match("*network.json", "network.json"));
        assert!(glob_match("http://x/?", "http://x/1"));
        assert!(!glob_match("http://x/?", "http://x/12"));
        assert!(!glob_match("*/tasks/network.json", "http://x/tasks/other.json"));
    }

    #[test]
    fn body_text_keeps_strings_and_serialises_objects() {
        assert_eq!(body_text(None), "");
        assert_eq!(body_text(Some(&json!("raw"))), "raw");
        assert_eq!(
            body_text(Some(&json!({"ok": true, "value": "mocked"}))),
            r#"{"ok":true,"value":"mocked"}"#
        );
    }

    #[test]
    fn a_dropped_context_removes_only_its_own_interception() {
        let browser = Browser::new();
        let state = net_state(&browser);
        {
            let mut st = state.try_lock().expect("uncontended in test");
            st.names.insert("jar-a".into(), "id-a".into());
            st.armed.insert("id-a".into(), "session-a".into());
            st.armed.insert("id-b".into(), "session-b".into());
            st.offline.insert("id-a".into(), "session-a".into());
            st.rules.push(Rule {
                id: "route-1".into(),
                context: "id-a".into(),
                session: "session-a".into(),
                pattern: "*/a.json".into(),
                kind: RuleKind::Abort,
            });
            st.rules.push(Rule {
                id: "route-2".into(),
                context: "id-b".into(),
                session: "session-b".into(),
                pattern: "*/b.json".into(),
                kind: RuleKind::Continue,
            });
        }
        let removed = futures_lite_block(on_context_gone(&browser, "id-a"));
        assert_eq!(removed, 1, "only jar-a's route is removed");
        let st = state.try_lock().expect("uncontended in test");
        assert!(st.armed.get("id-b").is_some(), "another context stays armed");
        assert!(st.offline.get("id-a").is_none(), "jar-a's offline is lifted");
        assert_eq!(
            st.rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["route-2"],
            "jar-b's route survives jar-a's teardown"
        );
        assert!(!st.names.contains_key("jar-a"), "the name binding is released");
    }

    fn futures_lite_block<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }
}
