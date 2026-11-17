//! The single MCP endpoint. Newline-delimited JSON-RPC 2.0 over stdio.
//!
//! One connection owns:
//! - a lazy browser engine (Gladius's `Browser`: it launches the owned
//!   `chrome-headless-shell` on first command, or attaches to the explicit
//!   `GLADIUS_CDP_URL` engine);
//! - the Windows desktop engine (Puppet's UIA/background path), which reads
//!   or acts only on an explicitly selected window and never touches the
//!   foreground window, the physical pointer or the clipboard.
//!
//! Both domains keep their own scheduling: 4 concurrent browser calls and 8
//! desktop jobs (Puppet's own cap). A domain failure never retargets the other
//! domain: routing is decided by the tool prefix alone.
//!
//! Cancellation is domain-specific, because the two engines promise different
//! things:
//! - browser: the client gets a typed `cancelled` result immediately and the
//!   in-flight call's late result is discarded (its restore keeps running, so
//!   a temporary viewport/freeze is restored rather than cut off);
//! - desktop: Puppet's contract is silence. The token is flipped, the result
//!   is dropped, and nothing is written for that id.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::catalog::{self, Domain};

/// G1 version matrix, identical to both pinned engines.
const LEGACY_1: &str = "2025-06-18";
const LEGACY_2: &str = "2025-11-25";
const LATEST_LEGACY: &str = "2025-11-25";
const MODERN: &str = "2026-07-28";
const SUPPORTED: [&str; 3] = [LEGACY_1, LEGACY_2, MODERN];

/// Per-domain concurrency caps. Browser matches Gladius (4); desktop matches
/// Puppet's `MAX_IN_FLIGHT` (8).
const MAX_BROWSER_CALLS: usize = 4;
const MAX_DESKTOP_CALLS: usize = 8;

/// Browser per-call budget (Gladius's S2.3): default 90 s, overridable per call
/// with `_budgetMs`, and bounded.
const DEFAULT_TOOL_BUDGET_MS: u64 = 90_000;
const MIN_TOOL_BUDGET_MS: u64 = 2_000;
const MAX_TOOL_BUDGET_MS: u64 = 600_000;

const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Canonical request-id key, identical to both engines: the JSON serialization
/// of the raw id, so `1` and `"1"` never collide.
fn id_key(id: &Value) -> String {
    serde_json::to_string(id).unwrap_or_default()
}

fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "windows-x64"
    } else {
        "windows"
    }
}

fn server_info() -> Value {
    json!({
        "name": "hand",
        "title": "hand",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": platform_name(),
        "engines": {
            "browser": browser_engine::engine_info(),
            "desktop": {
                "engine": "puppet",
                "sourcePin": "0f790be",
                "version": "8.0.0",
                "available": true,
                "backend": "uia+win32",
                "toolCount": catalog::DESKTOP_TOOLS.len(),
            }
        },
        "count": catalog::BROWSER_TOOLS.len() + catalog::DESKTOP_TOOLS.len(),
    })
}

fn instructions() -> &'static str {
    "hand exposes two prefixed catalogs on one connection: browser_* is the Gladius headless browser (owned chrome-headless-shell over CDP; GLADIUS_CDP_URL attaches an explicit external engine instead), desktop_* is the Puppet Windows desktop engine (UIA inspection, background-safe window messages, capture). Browser: start with browser_snapshot for stable @eN refs, then act; snapshot refs are invalidated by navigation and stale refs fail typed. Desktop: list or identify exactly one window first; hand never falls back to the foreground window, never moves the physical pointer, never activates a window and never writes the clipboard. Both domains report typed errors as tool results (structuredContent code/reason), not protocol errors. Verification is separate from delivery: an accepted click is not proof the application reacted. Cancellation: browser calls answer a typed cancelled result and discard their late result; desktop calls stay silent and drop the result, per Puppet's contract. No telemetry and no upload by default."
}

/// One `initialize`/`server/discover` result.
fn handshake_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(|v| v.as_str());
    let echo = requested
        .filter(|v| SUPPORTED.contains(v))
        .unwrap_or(LATEST_LEGACY);
    json!({
        "protocolVersion": echo,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": server_info(),
        "instructions": instructions(),
    })
}

fn discover_result() -> Value {
    json!({
        "supportedVersions": SUPPORTED,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": server_info(),
        "instructions": instructions(),
        "resultType": "complete",
    })
}

fn result_frame(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_frame(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn error_frame_data(id: &Value, code: i64, message: &str, data: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id,
        "error": {"code": code, "message": message, "data": data}})
}

/// A successful `tools/call` result, with the incidental modern marker added
/// for a 2026-07-28 peer. The domain body is never rewritten.
fn tool_result_frame(id: &Value, mut body: Value, modern: bool) -> Value {
    if modern {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("resultType".to_string(), json!("complete"));
        }
    }
    result_frame(id, body)
}

fn budget_timeout_envelope(tool: &str, budget: Duration) -> Value {
    browser_engine::typed_error(
        &format!(
            "{tool}: call exceeded its total budget of {} ms and was stopped; nothing it had \
             started is retried (raise `_budgetMs` for a legitimately long call)",
            budget.as_millis()
        ),
        json!({"code": "timeout", "reason": "budget_exceeded", "tool": tool,
               "budgetMs": budget.as_millis() as u64}),
    )
}

fn cancelled_envelope(tool: &str) -> Value {
    browser_engine::typed_error(
        &format!(
            "{tool}: cancelled by the client (notifications/cancelled); the call stopped and \
             nothing is retried"
        ),
        json!({"code": "cancelled", "reason": "client_cancelled", "tool": tool}),
    )
}

fn unknown_tool_envelope(name: &str) -> Value {
    browser_engine::typed_error(
        &format!(
            "hand: unknown tool '{name}'; tools are namespaced browser_<gladius name> or \
             desktop_<puppet name>"
        ),
        json!({"code": "invalid_args", "reason": "unknown_tool", "name": name}),
    )
}

/// Read and strip the per-call browser budget. `_budgetMs` is removed before
/// any tool sees it; a value that cannot be honoured is refused as a tool
/// result, never silently replaced. Environment precedence:
/// `HAND_TOOL_BUDGET_MS`, then `GLADIUS_TOOL_BUDGET_MS` (compatibility).
fn take_budget(args: &mut Value) -> Result<Duration, Value> {
    if let Some(v) = args.as_object_mut().and_then(|o| o.remove("_budgetMs")) {
        return match v
            .as_u64()
            .filter(|ms| (MIN_TOOL_BUDGET_MS..=MAX_TOOL_BUDGET_MS).contains(ms))
        {
            Some(ms) => Ok(Duration::from_millis(ms)),
            None => Err(browser_engine::typed_error(
                &format!(
                    "_budgetMs must be an integer number of milliseconds in \
                     {MIN_TOOL_BUDGET_MS}..={MAX_TOOL_BUDGET_MS}; got {v}"
                ),
                json!({"code": "invalid_args", "reason": "budget_mismatch", "arg": "_budgetMs"}),
            )),
        };
    }
    let env = std::env::var("HAND_TOOL_BUDGET_MS")
        .ok()
        .or_else(|| std::env::var("GLADIUS_TOOL_BUDGET_MS").ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_TOOL_BUDGET_MS);
    Ok(Duration::from_millis(env))
}

/// Per-request modern detection, exactly as both engines read it.
fn meta_version(v: &Value) -> Option<Value> {
    v.get("params")?
        .as_object()?
        .get("_meta")?
        .as_object()?
        .get("io.modelcontextprotocol/protocolVersion")
        .cloned()
}

// ---------------------------------------------------------------- state

struct BrowserCall {
    tool: String,
    state: Mutex<CallState>,
}

#[derive(Default)]
struct CallState {
    cancelled: bool,
    emitted: bool,
}

struct State {
    browser: Arc<browser_engine::Browser>,
    browser_sem: Arc<Semaphore>,
    desktop_sem: Arc<Semaphore>,
    browser_calls: Arc<Mutex<HashMap<String, Arc<BrowserCall>>>>,
    desktop_calls: Arc<Mutex<HashSet<String>>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl State {
    fn new() -> Self {
        Self {
            browser: Arc::new(browser_engine::Browser::new()),
            browser_sem: Arc::new(Semaphore::new(MAX_BROWSER_CALLS)),
            desktop_sem: Arc::new(Semaphore::new(MAX_DESKTOP_CALLS)),
            browser_calls: Arc::new(Mutex::new(HashMap::new())),
            desktop_calls: Arc::new(Mutex::new(HashSet::new())),
            tasks: Mutex::new(Vec::new()),
        }
    }
}

// ---------------------------------------------------------------- framing

enum FrameRead {
    Frame(String),
    NotUtf8,
    Eof,
}

/// One frame: bytes up to and including the next `\n`, terminator stripped the
/// way `BufRead::lines` strips it (one `\n`, then at most one `\r`). Invalid
/// UTF-8 is a frame-level error, not an I/O stop.
async fn read_frame<R>(reader: &mut R) -> std::io::Result<FrameRead>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
    let n = reader.read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(FrameRead::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    match std::str::from_utf8(&buf) {
        Ok(frame) => Ok(FrameRead::Frame(frame.to_string())),
        Err(_) => Ok(FrameRead::NotUtf8),
    }
}

async fn writer_loop(mut rx: mpsc::UnboundedReceiver<Value>) {
    let mut out = tokio::io::stdout();
    while let Some(frame) = rx.recv().await {
        let line = match serde_json::to_string(&frame) {
            Ok(mut s) => {
                s.push('\n');
                s
            }
            Err(e) => {
                eprintln!("[hand] response serialization failed: {e}");
                continue;
            }
        };
        if out.write_all(line.as_bytes()).await.is_err() || out.flush().await.is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------- serve

pub async fn serve() -> Result<(), String> {
    catalog::validate()?;
    eprintln!(
        "[hand] engine hand-mcp {} (gladius 59c6d77, puppet 0f790be) platform {}",
        env!("CARGO_PKG_VERSION"),
        platform_name()
    );
    let state = Arc::new(State::new());
    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(writer_loop(rx));
    let mut reader = BufReader::new(tokio::io::stdin());

    loop {
        match read_frame(&mut reader).await {
            Ok(FrameRead::Eof) | Err(_) => break,
            Ok(FrameRead::NotUtf8) => {
                let _ = tx.send(error_frame(&Value::Null, -32700, "Parse error"));
            }
            Ok(FrameRead::Frame(text)) => {
                let value: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = tx.send(error_frame(&Value::Null, -32700, "Parse error"));
                        continue;
                    }
                };
                handle_frame(value, &state, &tx).await;
            }
        }
    }

    shutdown(&state).await;
    drop(tx);
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, writer).await;
    Ok(())
}

async fn shutdown(state: &Arc<State>) {
    // Nothing answered after EOF has a reader, so discarded results are silent.
    for (_, call) in state.browser_calls.lock().await.drain() {
        let mut s = call.state.lock().await;
        s.cancelled = true;
        s.emitted = true;
    }
    for key in state.desktop_calls.lock().await.drain() {
        desktop_engine::cancel_call(&key);
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), state.browser.close()).await;
}

async fn handle_frame(value: Value, state: &Arc<State>, tx: &mpsc::UnboundedSender<Value>) {
    let Some(obj) = value.as_object() else {
        let _ = tx.send(error_frame(&Value::Null, -32600, "Invalid Request"));
        return;
    };
    let has_id = obj.contains_key("id");
    let method = match obj.get("method").and_then(|m| m.as_str()) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => {
            if has_id {
                let id = obj.get("id").cloned().unwrap_or(Value::Null);
                let _ = tx.send(error_frame(&id, -32600, "Invalid Request"));
            }
            return;
        }
    };
    let jsonrpc_ok = obj.get("jsonrpc").and_then(|j| j.as_str()) == Some("2.0");
    if !jsonrpc_ok {
        if has_id {
            let id = obj.get("id").cloned().unwrap_or(Value::Null);
            let _ = tx.send(error_frame(&id, -32600, "Invalid Request"));
        }
        return;
    }

    // Notifications: no response, whatever the method. Cancellation is the only
    // one with an effect.
    if !has_id {
        if method == "notifications/cancelled" {
            if let Some(rid) = value.get("params").and_then(|p| p.get("requestId")) {
                cancel_call(rid, state, tx).await;
            }
        }
        return;
    }

    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    if !(id.is_string() || id.is_number() || id.is_null()) {
        let _ = tx.send(error_frame(&Value::Null, -32600, "Invalid Request"));
        return;
    }

    // Per-request version: an explicit version outside the known set is refused
    // before dispatch, with the supported list.
    let requested = meta_version(&value);
    if let Some(ref v) = requested {
        if !v.as_str().is_some_and(|s| SUPPORTED.contains(&s)) {
            let _ = tx.send(error_frame_data(
                &id,
                -32022,
                "Unsupported protocol version",
                json!({"supported": SUPPORTED, "requested": v}),
            ));
            return;
        }
    }
    let modern = requested.as_ref().and_then(|v| v.as_str()) == Some(MODERN);

    match method.as_str() {
        "initialize" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(result_frame(&id, handshake_result(&params)));
        }
        "server/discover" => {
            let _ = tx.send(result_frame(&id, discover_result()));
        }
        "ping" => {
            let _ = tx.send(result_frame(&id, json!({})));
        }
        "tools/list" => {
            let mut result = catalog::combined_list();
            if modern {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("resultType".to_string(), json!("complete"));
                }
            }
            let _ = tx.send(result_frame(&id, result));
        }
        "tools/call" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let name = match params.get("name").and_then(|n| n.as_str()) {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => {
                    let _ = tx.send(error_frame(&id, -32602, "missing tool name"));
                    return;
                }
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            dispatch_call(id, name, args, modern, state, tx).await;
        }
        _ => {
            let _ = tx.send(error_frame(&id, -32601, "Method not found"));
        }
    }
}

async fn cancel_call(id: &Value, state: &Arc<State>, tx: &mpsc::UnboundedSender<Value>) {
    let key = id_key(id);

    // Desktop: only flip the engine token. Puppet's contract is silence, so no
    // response is written for this id.
    if state.desktop_calls.lock().await.remove(&key) {
        desktop_engine::cancel_call(&key);
        return;
    }

    // Browser: answer now with the typed cancelled result and discard the late
    // result. The call itself is not aborted here, so a restore that is already
    // running completes instead of being cut off mid-way.
    let entry = state.browser_calls.lock().await.remove(&key);
    if let Some(entry) = entry {
        let mut s = entry.state.lock().await;
        if s.cancelled || s.emitted {
            return;
        }
        s.cancelled = true;
        s.emitted = true;
        drop(s);
        let _ = tx.send(tool_result_frame(
            id,
            cancelled_envelope(&entry.tool),
            false,
        ));
    }
}

async fn dispatch_call(
    id: Value,
    name: String,
    mut args: Value,
    modern: bool,
    state: &Arc<State>,
    tx: &mpsc::UnboundedSender<Value>,
) {
    let Some((domain, legacy)) = catalog::split(&name) else {
        let _ = tx.send(tool_result_frame(&id, unknown_tool_envelope(&name), modern));
        return;
    };
    let legacy = legacy.to_string();

    match domain {
        Domain::Browser => {
            let budget = match take_budget(&mut args) {
                Ok(b) => b,
                Err(envelope) => {
                    let _ = tx.send(tool_result_frame(&id, envelope, modern));
                    return;
                }
            };
            let permit = match state.browser_sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    let _ = tx.send(error_frame(&id, -32603, "Server busy"));
                    return;
                }
            };
            let key = id_key(&id);
            let entry = Arc::new(BrowserCall {
                tool: legacy.clone(),
                state: Mutex::new(CallState::default()),
            });
            {
                let mut map = state.browser_calls.lock().await;
                if let Some(old) = map.remove(&key) {
                    let mut s = old.state.lock().await;
                    s.cancelled = true;
                    s.emitted = true;
                }
                map.insert(key.clone(), entry.clone());
            }
            let browser = state.browser.clone();
            let calls = state.browser_calls.clone();
            let tx = tx.clone();
            let task = tokio::spawn(async move {
                let _permit = permit;
                let fut = browser_engine::call(&legacy, &args, &browser);
                let outcome = tokio::time::timeout(budget, fut).await;

                let mut s = entry.state.lock().await;
                if s.cancelled || s.emitted {
                    return;
                }
                s.emitted = true;
                drop(s);

                let mut map = calls.lock().await;
                if map
                    .get(&key)
                    .is_some_and(|current| Arc::ptr_eq(current, &entry))
                {
                    map.remove(&key);
                }
                drop(map);

                let body = match outcome {
                    Ok(v) => v,
                    Err(_) => budget_timeout_envelope(&legacy, budget),
                };
                let _ = tx.send(tool_result_frame(&id, body, modern));
            });
            state.tasks.lock().await.push(task);
        }
        Domain::Desktop => {
            if !args.is_null() && !args.is_object() {
                let _ = tx.send(error_frame(
                    &id,
                    -32602,
                    "Invalid params: arguments must be an object",
                ));
                return;
            }
            let permit = match state.desktop_sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    let _ = tx.send(error_frame(&id, -32603, "Server busy"));
                    return;
                }
            };
            let key = id_key(&id);
            // Publish the token before the worker starts so a cancel that
            // arrives in between is not lost.
            desktop_engine::register_call(&key);
            state.desktop_calls.lock().await.insert(key.clone());
            let params = json!({"name": legacy, "arguments": args});
            let desktop_calls = state.desktop_calls.clone();
            let tx = tx.clone();
            let task = tokio::spawn(async move {
                let _permit = permit;
                let call_key = key.clone();
                let result = tokio::task::spawn_blocking(move || {
                    desktop_engine::call_tool(id.clone(), &call_key, params)
                })
                .await;
                desktop_calls.lock().await.remove(&key);
                match result {
                    Ok(Some(frame)) => {
                        let frame = adapt_desktop_frame(frame, modern);
                        let _ = tx.send(frame);
                    }
                    // Cancelled: silence is the contract.
                    Ok(None) => {}
                    Err(e) => {
                        let _ = tx.send(error_frame(
                            &Value::Null,
                            -32603,
                            &format!("desktop worker failed: {e}"),
                        ));
                    }
                }
            });
            state.tasks.lock().await.push(task);
        }
    }
}

/// Add the modern marker to a desktop result without touching its body.
fn adapt_desktop_frame(mut frame: Value, modern: bool) -> Value {
    if modern {
        if let Some(result) = frame.get_mut("result").and_then(|r| r.as_object_mut()) {
            result.insert("resultType".to_string(), json!("complete"));
        }
    }
    frame
}
