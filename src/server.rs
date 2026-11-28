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
//! Cancellation and a budget expiry settle the client's ANSWER, never the
//! engine's work. The typed `cancelled`/`timeout` result is an acknowledgement:
//! an action the call had already sent may still be delivered, its late result
//! is discarded, and any cleanup it runs (a deterministic capture restoring
//! its viewport/freeze/media emulation) may still run. The outcome is therefore
//! reported as UNKNOWN and a retry as UNSAFE - a retry can duplicate an action
//! that is still in flight. The engine call keeps its concurrency slot until it
//! actually ends, and the answer never claims the work stopped when it did not.
//! Dropping the engine future (which `tokio::time::timeout` would do) is worse:
//! it cuts a deterministic capture off before its restore, leaving the page
//! frozen and its viewport/media emulation pinned.
//!
//! Desktop cancellation keeps Puppet's contract: the token is flipped, the
//! result is dropped, and nothing is written for that id, whatever the job's
//! outcome (including a panic).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::catalog::{self, Domain};

include!(concat!(env!("OUT_DIR"), "/skills.rs"));

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

/// Writer flush budget after EOF.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long a detached browser call may keep its slot after the client was
/// already answered, so the engine can finish a cleanup it had started. This
/// is an ATTEMPT, not a guarantee: the engine's own commands are individually
/// bounded, and a call that outlives this is aborted and reported as unverified.
const ENGINE_CLEANUP_GRACE: Duration = Duration::from_secs(30);

/// EOF drain. An owned engine is killed in the next step, so waiting is only
/// about an orderly exit; an attached engine is NOT ours and keeps running, so
/// its in-flight calls get a longer window. Neither value is a promise that a
/// restore completes: reaching the bound aborts the work and says so.
const EOF_DRAIN_OWNED: Duration = Duration::from_secs(5);
const EOF_DRAIN_ATTACHED: Duration = Duration::from_secs(30);

/// One shared window to let aborted workers drop their locals after the drain
/// deadline. It is a single bound for the whole shutdown, never per worker, so
/// the drain costs at most `grace + ABORT_SETTLE` however many calls were in
/// flight.
const ABORT_SETTLE: Duration = Duration::from_secs(1);

/// Browser close budget. `Browser::close` is internally bounded at 2 s (child
/// exit) + 5 s (tree-kill fallback) + 10 s (profile sweep); 5 s truncated the
/// sweep and the process exited over an incomplete cleanup.
const BROWSER_CLOSE_GRACE: Duration = Duration::from_secs(30);

/// Context disposal budget. `ContextRegistry::shutdown` disposes hand's own
/// contexts over CDP before the browser closes; it must not hold shutdown open.
const CONTEXT_CLOSE_GRACE: Duration = Duration::from_secs(10);

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
            "desktop": desktop_engine::provenance(),
        },
        "count": catalog::BROWSER_TOOLS.len() + catalog::DESKTOP_TOOLS.len(),
        "isolatedContexts": browser_engine::contexts::capability_info()["isolatedContexts"].clone(),
    })
}

/// One compact parameter line per published tool, built from the same prefixed
/// catalog `tools/list` serves. A new tool or a changed key appears here without
/// a second hand-maintained list going stale.
fn instructions() -> String {
    let mut text = String::from(
        "hand is a standalone MCP server. Browser: start with browser_snapshot for @eN refs; refs die on navigation and stale refs fail typed. Desktop: select one named window; never the foreground window, activation, pointer or clipboard. Errors are typed tool results, not protocol errors. Cancel or budget expiry: outcome:unknown, retry:unsafe. Verification is separate from delivery. No telemetry or upload.\nParameters (required | [optional]):\n",
    );
    let catalog = catalog::combined_list();
    let Some(tools) = catalog["tools"].as_array() else {
        return text;
    };
    for tool in tools {
        let Some(name) = tool["name"].as_str() else {
            continue;
        };
        let schema = &tool["inputSchema"];
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|keys| keys.iter().filter_map(|k| k.as_str()).collect())
            .unwrap_or_default();
        let optional: Vec<&str> = schema["properties"]
            .as_object()
            .map(|props| {
                props
                    .keys()
                    .map(String::as_str)
                    .filter(|key| !required.contains(key))
                    .collect()
            })
            .unwrap_or_default();
        // `selector` is an alias for `element` on six tools. Printing both as
        // separate keys hides the one-of relationship, so collapse them into
        // one token wherever they occur (comma replaced by pipe: no growth).
        let aliased = schema["properties"]["selector"].is_object()
            && schema["properties"]["element"].is_object();
        let render = |keys: &[&str]| -> Vec<String> {
            let mut out = Vec::new();
            let mut alias_emitted = false;
            for key in keys {
                if aliased && (*key == "element" || *key == "selector") {
                    if !alias_emitted {
                        out.push("element|selector".to_string());
                        alias_emitted = true;
                    }
                } else {
                    out.push((*key).to_string());
                }
            }
            out
        };
        let req = render(&required);
        let opt = render(&optional);
        text.push_str(name);
        text.push(':');
        text.push_str(&req.join(","));
        if !opt.is_empty() {
            text.push('[');
            text.push_str(&opt.join(","));
            text.push(']');
        }
        text.push('\n');
    }
    text
}

/// One `initialize`/`server/discover` result.
fn handshake_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(|v| v.as_str());
    let echo = requested
        .filter(|v| SUPPORTED.contains(v))
        .unwrap_or(LATEST_LEGACY);
    json!({
        "protocolVersion": echo,
        "capabilities": {"tools": {"listChanged": false}, "resources": {}, "extensions": {"io.modelcontextprotocol/skills": {}}},
        "serverInfo": server_info(),
        "instructions": instructions(),
    })
}

/// The 2026-07-28 cache hint. OpenCode 2.4.1 validates modern `tools/list` and
/// `server/discover` results with `ttlMs` (a non-negative integer) and
/// `cacheScope` (`public` or `private`). Zero TTL and private scope are the
/// conservative values: the body is never served from cache (it is still
/// stored for the tool index) and is never shared across authorization contexts.
fn add_cache_hints(result: &mut Value) {
    if let Some(obj) = result.as_object_mut() {
        obj.insert("ttlMs".to_string(), json!(0));
        obj.insert("cacheScope".to_string(), json!("private"));
    }
}

fn discover_result(modern: bool) -> Value {
    let mut result = json!({
        "supportedVersions": SUPPORTED,
        "capabilities": {"tools": {"listChanged": false}, "resources": {}, "extensions": {"io.modelcontextprotocol/skills": {}}},
        "serverInfo": server_info(),
        "instructions": instructions(),
        "resultType": "complete",
    });
    if modern {
        add_cache_hints(&mut result);
    }
    result
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
            "{tool}: call exceeded its total budget of {} ms and is NOT stopped; no result is \
             returned. An action it had already sent may still be delivered, its late result is \
             discarded, and cleanup it runs may still run. The outcome is UNKNOWN: do not retry \
             unless you can verify the state or know the call is read-only, because a retry can \
             duplicate an action that is still in flight. It keeps one of the {MAX_BROWSER_CALLS} \
             browser slots until it actually ends, so a new call can be refused as busy until \
             then. Raise `_budgetMs` for a legitimately long call",
            budget.as_millis()
        ),
        json!({"code": "timeout", "reason": "budget_exceeded", "tool": tool,
               "budgetMs": budget.as_millis() as u64,
               "stopped": false, "outcome": "unknown", "retry": "unsafe",
               "sideEffects": "may_still_land",
               "slots": {"limit": MAX_BROWSER_CALLS, "heldByThisCall": true}}),
    )
}

fn cancelled_envelope(tool: &str) -> Value {
    browser_engine::typed_error(
        &format!(
            "{tool}: cancellation acknowledged (notifications/cancelled), but the call is NOT \
             stopped; its late result is discarded. An action it had already sent may still be \
             delivered, and cleanup it runs may still run. The outcome is UNKNOWN: do not retry \
             unless you can verify the state or know the call is read-only, because a retry can \
             duplicate an action that is still in flight. It keeps one of the {MAX_BROWSER_CALLS} \
             browser slots until the call actually ends (that is when the slot frees), so a new \
             call can be refused as busy until then"
        ),
        json!({"code": "cancelled", "reason": "client_cancelled", "tool": tool,
               "stopped": false, "outcome": "unknown", "retry": "unsafe",
               "sideEffects": "may_still_land",
               "slots": {"limit": MAX_BROWSER_CALLS, "heldByThisCall": true}}),
    )
}

/// A capacity refusal is a tool failure, never a protocol error: it is a
/// valid call the server could not admit, and the client needs to know it may
/// retry. `limit` is hand's own per-domain cap; `available` is the observed
/// free-slot count at the refusal, so the reply never invents "0 in flight".
fn busy_envelope(domain: &str, limit: usize, available: usize) -> Value {
    let in_flight = limit.saturating_sub(available);
    browser_engine::typed_error(
        &format!(
            "hand: {in_flight} of {limit} {domain} slots are in use and this call could not be \
             admitted, so it was not started and retrying it cannot duplicate anything. A \
             timed-out or cancelled call keeps its slot until its engine work actually ends \
             (which may still deliver an action); wait for a slot to free, then retry"
        ),
        json!({"code": "busy", "reason": "concurrency_limit", "domain": domain,
               "limit": limit, "inFlight": in_flight, "available": available,
               "started": false, "outcome": "not_started", "retry": "safe"}),
    )
}

/// The engine's task ended without a value: it panicked (a bug) or was
/// aborted. The client is answered (silence would leave its id pending for
/// ever), and the outcome is stated as unknown rather than guessed. The panic
/// payload is deliberately not logged or echoed: it can contain user content.
fn engine_failure_envelope(tool: &str, panicked: bool) -> Value {
    let what = if panicked { "panicked" } else { "was aborted" };
    browser_engine::typed_error(
        &format!(
            "{tool}: the browser engine task {what}; the call's outcome is UNKNOWN and nothing \
             is retried. An action it had already sent may have landed: do not retry unless you \
             can verify the state"
        ),
        json!({"code": "error",
               "reason": if panicked { "engine_panic" } else { "engine_aborted" },
               "tool": tool, "outcome": "unknown", "retry": "unsafe",
               "sideEffects": "may_have_landed"}),
    )
}

/// A desktop job that emitted nothing although hand never cancelled its id.
/// Puppet's silence contract covers cancellation, not an unexplained drop, so
/// the id is answered instead of left pending.
fn desktop_no_response_envelope(tool: &str) -> Value {
    browser_engine::typed_error(
        &format!(
            "desktop_{tool}: the desktop worker produced no response and hand did not cancel \
             this id; the outcome is UNKNOWN, never success (nothing is retried). Do not retry \
             unless you can verify the state: an action it had already delivered may have landed"
        ),
        json!({"code": "error", "reason": "no_response", "tool": tool,
               "outcome": "unknown", "retry": "unsafe",
               "sideEffects": "may_have_landed"}),
    )
}

fn unknown_tool_envelope(name: &str) -> Value {
    browser_engine::typed_error(
        &format!(
            "hand: unknown tool '{name}'; tools are namespaced browser_<name> or \
             desktop_<name>"
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

/// Add the mandatory 2026-07-28 completion marker to a successful result.
/// Earlier revisions keep their original body: the absent-means-complete
/// bridge belongs to the client, not to us, and must not be preempted.
fn add_result_type(mut result: Value, modern: bool) -> Value {
    if modern {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("resultType".to_string(), json!("complete"));
        }
    }
    result
}

/// Skills and resources are embedded by `build.rs`: build-time content cannot
/// change within a build. Zero TTL and private scope are the conservative hints;
/// `resultType` is required by the skills extension on these results whatever
/// revision the peer negotiated.
fn static_content_meta(result: &mut Value) {
    if let Some(obj) = result.as_object_mut() {
        obj.insert("resultType".to_string(), json!("complete"));
        obj.insert("ttlMs".to_string(), json!(0));
        obj.insert("cacheScope".to_string(), json!("private"));
    }
}

/// A request is modern when its `_meta` says so OR when `initialize` on this
/// connection negotiated 2026-07-28. The second case is the one OpenCode uses.
async fn modern_peer(state: &Arc<State>, requested: &Option<Value>) -> bool {
    requested.as_ref().and_then(|v| v.as_str()) == Some(MODERN)
        || state.negotiated_version.lock().await.as_deref() == Some(MODERN)
}

struct BrowserCall {
    tool: String,
    /// The request's protocol version, so a cancellation answered later still
    /// carries `resultType` for a 2026-07-28 peer.
    modern: bool,
    state: Mutex<CallState>,
}

#[derive(Default)]
struct CallState {
    cancelled: bool,
    emitted: bool,
}

/// One in-flight desktop job. The entry stays in the map until the worker
/// ends, so an id reused while its cancelled worker is still running is
/// refused instead of silently sharing that worker's (already flipped) token.
struct DesktopCall {
    cancelled: AtomicBool,
}

/// A spawned engine call whose lifetime is tied to the worker that owns it.
/// Aborting the outer worker must abort this task too: dropping a
/// `JoinHandle` leaves its task running, which would let an untracked engine
/// call touch the browser while shutdown closes it.
struct EngineTask(JoinHandle<Value>);

impl EngineTask {
    fn spawn<F>(future: F) -> Self
    where
        F: std::future::Future<Output = Value> + Send + 'static,
    {
        Self(tokio::spawn(future))
    }

    fn abort(&self) {
        self.0.abort();
    }
}

impl Drop for EngineTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Which domain a tracked worker belongs to. The two have different shutdown
/// realities: a browser call's engine task is async and can be aborted (at the
/// cost of an unverified restore), while a desktop job runs in a Tokio blocking
/// thread that cannot be aborted once started (Tokio 1.54: "this will not have
/// any effect").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerKind {
    Browser,
    Desktop,
}

struct WorkerTask {
    kind: WorkerKind,
    handle: JoinHandle<()>,
}

/// Who owns the browser behind this connection, so shutdown logs never blur
/// them: an owned browser dies with us, an attached one does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineKind {
    Owned,
    Attached,
}

impl EngineKind {
    fn of(info: &Value) -> Self {
        if info.get("cdpUrl").is_some() {
            Self::Attached
        } else {
            Self::Owned
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::Attached => "attached",
        }
    }

    /// What abandoning in-flight work at the EOF deadline means, by ownership
    /// and by worker kind. Never promises a restore or a bounded cleanup: the
    /// deadline was reached. Desktop is explicit that aborting its wrapper does
    /// not stop the blocking job.
    fn abandonment(self, grace: Duration, browser: usize, desktop: usize) -> String {
        let mut who: Vec<String> = Vec::new();
        if browser > 0 {
            who.push(format!("{browser} browser call(s)"));
        }
        if desktop > 0 {
            who.push(format!("{desktop} desktop job(s)"));
        }
        let mut line = format!(
            "[hand] shutdown ({}): at the {grace:?} drain deadline {} were still running",
            self.label(),
            who.join(" and ")
        );
        if browser > 0 {
            line.push_str(
                "; their async engine tasks were aborted, so any restore or cleanup they had \
                 started is UNVERIFIED",
            );
        }
        if desktop > 0 {
            line.push_str(
                "; a started blocking desktop job CANNOT be aborted (its wrapper is dropped, the \
                 job keeps running), so the in-flight action's outcome is UNKNOWN and the process \
                 exit ends the worker",
            );
        }
        if browser > 0 {
            match self {
                Self::Owned => line.push_str(
                    "; the owned browser (if one was launched) is closed next, so its page state \
                     is discarded",
                ),
                Self::Attached => line.push_str(
                    "; the attached engine keeps running, so page state its calls changed is \
                     UNVERIFIED",
                ),
            }
        }
        line
    }
}

/// Started blocking desktop jobs, for main's runtime-bound log. A started
/// `spawn_blocking` task cannot be aborted; this counter lets the exit line say
/// only what it knows instead of inferring "desktop" from elapsed time.
pub(crate) static ACTIVE_DESKTOP_JOBS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Counts one blocking desktop job for its whole (possibly unstoppable) life.
struct ActiveDesktopJob;

impl ActiveDesktopJob {
    fn enter() -> Self {
        ACTIVE_DESKTOP_JOBS.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ActiveDesktopJob {
    fn drop(&mut self) {
        ACTIVE_DESKTOP_JOBS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Test-only hook (`HAND_TEST_HOOKS=1` AND `HAND_TEST_STUCK_DESKTOP_MS` in a
/// debug build): run a controlled blocking job instead of a desktop tool. It
/// returns before the engine call, so it touches no window. The double key and
/// the debug gate exist so a stray environment variable cannot silently divert
/// every desktop tool in a real binary; the clamp bounds a stray duration.
#[cfg(debug_assertions)]
fn test_stuck_desktop_ms() -> Option<u64> {
    if std::env::var("HAND_TEST_HOOKS").ok().as_deref() != Some("1") {
        return None;
    }
    let ms = std::env::var("HAND_TEST_STUCK_DESKTOP_MS")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(ms.clamp(1, 600_000))
}

#[cfg(not(debug_assertions))]
fn test_stuck_desktop_ms() -> Option<u64> {
    None
}

struct State {
    browser: Arc<browser_engine::Browser>,
    /// hand's isolated browser contexts, which own no engine of their own: they
    /// act through `browser` and must be shut down before it closes.
    contexts: Arc<browser_engine::contexts::ContextRegistry>,
    browser_sem: Arc<Semaphore>,
    desktop_sem: Arc<Semaphore>,
    browser_calls: Arc<Mutex<HashMap<String, Arc<BrowserCall>>>>,
    desktop_calls: Arc<Mutex<HashMap<String, Arc<DesktopCall>>>>,
    tasks: Mutex<Vec<WorkerTask>>,
    /// Version `initialize` actually echoed on this connection. A modern peer
    /// does not repeat its version in `_meta` on every later request, so the
    /// negotiated value must decide which results carry modern fields.
    negotiated_version: Arc<Mutex<Option<String>>>,
}

impl State {
    fn new() -> Self {
        let browser = Arc::new(browser_engine::Browser::new());
        let contexts = Arc::new(browser_engine::contexts::ContextRegistry::new(
            browser.clone(),
        ));
        Self {
            browser,
            contexts,
            browser_sem: Arc::new(Semaphore::new(MAX_BROWSER_CALLS)),
            desktop_sem: Arc::new(Semaphore::new(MAX_DESKTOP_CALLS)),
            browser_calls: Arc::new(Mutex::new(HashMap::new())),
            desktop_calls: Arc::new(Mutex::new(HashMap::new())),
            tasks: Mutex::new(Vec::new()),
            negotiated_version: Arc::new(Mutex::new(None)),
        }
    }

    /// Record a worker so shutdown can wait for it. Finished handles are
    /// dropped here: without pruning, one `JoinHandle` per call accumulated
    /// for the life of the process.
    async fn track(&self, kind: WorkerKind, task: JoinHandle<()>) {
        let mut tasks = self.tasks.lock().await;
        tasks.retain(|t| !t.handle.is_finished());
        tasks.push(WorkerTask { kind, handle: task });
    }

    /// Wait for the tracked workers, globally bounded by `grace` however many
    /// are in flight. Anything still running at the deadline is abandoned; the
    /// wrapper tasks are aborted and the abort settle is ONE shared
    /// `ABORT_SETTLE` window, not one per worker, so the drain costs at most
    /// `grace + ABORT_SETTLE`. That bound covers the async side only: a started
    /// `spawn_blocking` desktop job keeps running regardless (Tokio's abort has
    /// no effect on it), which is reported as unknown, not as cleanup.
    async fn drain_tasks(&self, grace: Duration, kind: EngineKind) {
        let workers: Vec<WorkerTask> = {
            let mut tasks = self.tasks.lock().await;
            std::mem::take(&mut *tasks)
        };
        let deadline = Instant::now() + grace;
        let mut abandoned: Vec<WorkerTask> = Vec::new();
        for mut worker in workers {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if !worker.handle.is_finished() {
                    abandoned.push(worker);
                }
                continue;
            }
            if tokio::time::timeout(left, &mut worker.handle)
                .await
                .is_err()
                && !worker.handle.is_finished()
            {
                abandoned.push(worker);
            }
        }
        if abandoned.is_empty() {
            return;
        }
        let browser = abandoned
            .iter()
            .filter(|w| w.kind == WorkerKind::Browser)
            .count();
        let desktop = abandoned.len() - browser;
        err_eprintln!("EABANDON", "{}", kind.abandonment(grace, browser, desktop));
        for worker in abandoned.iter_mut() {
            worker.handle.abort();
        }
        let settle_deadline = Instant::now() + ABORT_SETTLE;
        let mut unsettled = 0usize;
        for worker in abandoned.iter_mut() {
            let left = settle_deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                unsettled += 1;
                continue;
            }
            if tokio::time::timeout(left, &mut worker.handle)
                .await
                .is_err()
            {
                unsettled += 1;
            }
        }
        if unsettled > 0 {
            err_eprintln!(
                "EABORT",
                "[hand] shutdown ({}): {unsettled} aborted wrapper(s) did not settle within \
                 {ABORT_SETTLE:?}; closing anyway (their cleanup is UNVERIFIED)",
                kind.label()
            );
        }
    }
}

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
                err_eprintln!("EWRITE", "[hand] response serialization failed: {e}");
                continue;
            }
        };
        if out.write_all(line.as_bytes()).await.is_err() || out.flush().await.is_err() {
            break;
        }
    }
}

/// Panic payloads can echo user content (a tool argument, page text quoted by
/// an `unwrap`). The default hook prints the payload verbatim, so serving
/// installs a fixed line instead: the location survives, the payload does not.
fn install_payload_free_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());
        err_eprintln!(
            "EPANIC",
            "[hand] panic at {at} (payload suppressed: it can contain user content)"
        );
    }));
}

pub async fn serve() -> Result<(), String> {
    install_payload_free_panic_hook();
    catalog::validate()?;
    debug_eprintln!(
        "[hand] engine hand {} (gladius {}, puppet {}) platform {}",
        env!("CARGO_PKG_VERSION"),
        browser_engine::UPSTREAM_PIN,
        desktop_engine::UPSTREAM_PIN,
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
    let browser_calls: Vec<Arc<BrowserCall>> = {
        let mut calls = state.browser_calls.lock().await;
        calls.drain().map(|(_, call)| call).collect()
    };
    for call in browser_calls {
        let mut s = call.state.lock().await;
        s.cancelled = true;
        s.emitted = true;
    }
    let desktop_calls: Vec<(String, Arc<DesktopCall>)> = {
        let mut calls = state.desktop_calls.lock().await;
        calls.drain().collect()
    };
    for (key, call) in desktop_calls {
        call.cancelled.store(true, Ordering::SeqCst);
        desktop_engine::cancel_call(&key);
    }
    let kind = EngineKind::of(&browser_engine::backend_info());
    let grace = match kind {
        EngineKind::Owned => EOF_DRAIN_OWNED,
        EngineKind::Attached => EOF_DRAIN_ATTACHED,
    };
    state.drain_tasks(grace, kind).await;
    if tokio::time::timeout(CONTEXT_CLOSE_GRACE, state.contexts.shutdown())
        .await
        .is_err()
    {
        err_eprintln!("ECONTEXT", 
            "[hand] shutdown ({}): context disposal did not finish within {CONTEXT_CLOSE_GRACE:?}; \
             what the registry still tracks is reported by a later `browser_context list`, and any \
             context it had already removed from tracking is not claimed as disposed",
            kind.label()
        );
    }
    if tokio::time::timeout(BROWSER_CLOSE_GRACE, state.browser.close())
        .await
        .is_err()
    {
        err_eprintln!(
            "ECLOSE",
            "[hand] shutdown ({}): {} did not finish within {BROWSER_CLOSE_GRACE:?}; continuing",
            kind.label(),
            if kind == EngineKind::Attached {
                "detaching from the attached engine"
            } else {
                "closing the owned browser"
            }
        );
    }
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
    let mut modern = modern_peer(state, &requested).await;

    match method.as_str() {
        "initialize" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let result = handshake_result(&params);
            let echoed = result["protocolVersion"]
                .as_str()
                .unwrap_or(LATEST_LEGACY)
                .to_string();
            modern = echoed == MODERN;
            *state.negotiated_version.lock().await = Some(echoed);
            let _ = tx.send(result_frame(&id, add_result_type(result, modern)));
        }
        "server/discover" => {
            let _ = tx.send(result_frame(&id, discover_result(modern)));
        }
        "ping" => {
            let _ = tx.send(result_frame(&id, add_result_type(json!({}), modern)));
        }
        "tools/list" => {
            let mut result = catalog::combined_list();
            if modern {
                add_cache_hints(&mut result);
            }
            let _ = tx.send(result_frame(&id, add_result_type(result, modern)));
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
        "skills/list" => {
            let result = skills_list_result();
            let _ = tx.send(result_frame(&id, result));
        }
        "skills/get" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            match skills_get_result(uri) {
                Ok(result) => {
                    let _ = tx.send(result_frame(&id, result));
                }
                Err(_) => {
                    let _ = tx.send(error_frame(&id, -32602, "unknown skill uri"));
                }
            }
        }
        "resources/list" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let _ = tx.send(result_frame(&id, resources_list_result(&params)));
        }
        "resources/read" => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
            match resources_read_result(uri) {
                Ok(result) => {
                    let _ = tx.send(result_frame(&id, result));
                }
                Err(_) => {
                    let _ = tx.send(error_frame(&id, -32602, "unknown resource uri"));
                }
            }
        }
        _ => {
            let _ = tx.send(error_frame(&id, -32601, "Method not found"));
        }
    }
}

async fn cancel_call(id: &Value, state: &Arc<State>, tx: &mpsc::UnboundedSender<Value>) {
    let key = id_key(id);

    {
        let entry = state.desktop_calls.lock().await.get(&key).cloned();
        if let Some(entry) = entry {
            entry.cancelled.store(true, Ordering::SeqCst);
            desktop_engine::cancel_call(&key);
            return;
        }
    }

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
            entry.modern,
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
                    let available = state.browser_sem.available_permits();
                    let _ = tx.send(tool_result_frame(
                        &id,
                        busy_envelope("browser", MAX_BROWSER_CALLS, available),
                        modern,
                    ));
                    return;
                }
            };
            let key = id_key(&id);
            let entry = Arc::new(BrowserCall {
                tool: legacy.clone(),
                modern,
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
            let contexts = state.contexts.clone();
            let calls = state.browser_calls.clone();
            let tx = tx.clone();
            let task = tokio::spawn(async move {
                let _permit = permit;
                let engine_tool = legacy.clone();
                let engine_args = args.clone();
                let engine_browser = browser.clone();
                let engine_contexts = contexts.clone();
                let mut worker = EngineTask::spawn(async move {
                    if let Some(body) = browser_engine::contexts::dispatch(
                        &engine_tool,
                        &engine_args,
                        &engine_contexts,
                    )
                    .await
                    {
                        return body;
                    }
                    browser_engine::call(&engine_tool, &engine_args, &engine_browser).await
                });

                let outcome = tokio::time::timeout(budget, &mut worker.0).await;
                let timed_out = outcome.is_err();
                let body = match outcome {
                    Ok(Ok(value)) => Some(value),
                    Ok(Err(join)) => {
                        err_eprintln!(
                            "EENGINE",
                            "[hand] browser call '{legacy}': engine task failed ({})",
                            if join.is_panic() { "panic" } else { "aborted" }
                        );
                        Some(engine_failure_envelope(&legacy, join.is_panic()))
                    }
                    Err(_) => Some(budget_timeout_envelope(&legacy, budget)),
                };

                let claimed = {
                    let mut s = entry.state.lock().await;
                    if s.cancelled || s.emitted {
                        false
                    } else {
                        s.emitted = true;
                        true
                    }
                };
                {
                    let mut map = calls.lock().await;
                    if map
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                    {
                        map.remove(&key);
                    }
                }
                if claimed {
                    if let Some(body) = body {
                        let _ = tx.send(tool_result_frame(&id, body, modern));
                    }
                }

                if timed_out
                    && tokio::time::timeout(ENGINE_CLEANUP_GRACE, &mut worker.0)
                        .await
                        .is_err()
                {
                    worker.abort();
                    let _ = (&mut worker.0).await;
                    err_eprintln!(
                        "ECLEANUP",
                        "[hand] browser call '{legacy}': engine cleanup did not finish \
                         within {ENGINE_CLEANUP_GRACE:?} after the budget answer; aborted \
                         (its started restore is UNVERIFIED)"
                    );
                }
            });
            state.track(WorkerKind::Browser, task).await;
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
            let key = id_key(&id);
            let entry = Arc::new(DesktopCall {
                cancelled: AtomicBool::new(false),
            });
            {
                let mut map = state.desktop_calls.lock().await;
                if map.contains_key(&key) {
                    drop(map);
                    let _ = tx.send(error_frame_data(
                        &id,
                        -32600,
                        "Invalid Request: request id is already in flight",
                        json!({"id": id, "reason": "duplicate_in_flight",
                               "hint": "the earlier call keeps its own answer; use a new id"}),
                    ));
                    return;
                }
                map.insert(key.clone(), entry.clone());
            }
            let permit = match state.desktop_sem.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    let available = state.desktop_sem.available_permits();
                    state.desktop_calls.lock().await.remove(&key);
                    let _ = tx.send(tool_result_frame(
                        &id,
                        busy_envelope("desktop", MAX_DESKTOP_CALLS, available),
                        modern,
                    ));
                    return;
                }
            };
            desktop_engine::register_call(&key);
            let tool = legacy.clone();
            let params = json!({"name": legacy, "arguments": args});
            let desktop_calls = state.desktop_calls.clone();
            let tx = tx.clone();
            let stuck_ms = test_stuck_desktop_ms();
            let task = tokio::spawn(async move {
                let _permit = permit;
                let call_key = key.clone();
                let worker_id = id.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let _active = ActiveDesktopJob::enter();
                    if let Some(ms) = stuck_ms {
                        debug_eprintln!(
                            "[hand] TEST HOOK (debug build, HAND_TEST_HOOKS=1): blocking desktop \
                             job started for {ms} ms; no window is touched; TEST ONLY"
                        );
                        std::thread::sleep(Duration::from_millis(ms));
                        return None;
                    }
                    desktop_engine::call_tool(worker_id, &call_key, params)
                })
                .await;
                {
                    let mut map = desktop_calls.lock().await;
                    if map
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                    {
                        map.remove(&key);
                    }
                }
                if entry.cancelled.load(Ordering::SeqCst) {
                    return;
                }
                match result {
                    Ok(Some(frame)) => {
                        let _ = tx.send(adapt_desktop_frame(frame, modern));
                    }
                    Ok(None) => {
                        let _ = tx.send(tool_result_frame(
                            &id,
                            desktop_no_response_envelope(&tool),
                            modern,
                        ));
                    }
                    Err(_) => {
                        let _ = tx.send(error_frame_data(
                            &id,
                            -32603,
                            "desktop worker failed; outcome unknown",
                            json!({"outcome": "unknown", "retry": "unsafe"}),
                        ));
                    }
                }
            });
            state.track(WorkerKind::Desktop, task).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_index_every_tool_and_keep_the_rules() {
        let text = instructions();
        assert!(
            text.starts_with("hand is a standalone MCP server"),
            "instructions must open with hand's identity: {text}"
        );
        // The index is now generated: one line per published tool with its
        // required and optional keys. 2772 bytes at 41 tools; 4096 leaves room
        // for the next tools without letting the index become an essay.
        assert!(
            text.contains("browser_click:[element|selector,"),
            "the aliased target must render as one token: {text}"
        );
        assert!(
            text.len() <= 4096,
            "instructions grew to {} bytes",
            text.len()
        );
        for name in catalog::BROWSER_TOOLS
            .iter()
            .chain(catalog::DESKTOP_TOOLS.iter())
        {
            assert!(text.contains(*name), "instructions omit {name:?}: {text}");
        }
        for needle in [
            "browser_snapshot for @eN refs",
            "refs die on navigation and stale refs fail typed",
            "select one named window",
            "never the foreground window, activation, pointer or clipboard",
            "typed tool results, not protocol errors",
            "outcome:unknown, retry:unsafe",
            "Verification is separate from delivery",
            "No telemetry or upload",
        ] {
            assert!(
                text.contains(needle),
                "instructions lost {needle:?}: {text}"
            );
        }
    }

    /// The handshake publishes the crates' provenance.
    #[test]
    fn server_info_engines_match_crate_provenance() {
        let info = server_info();
        let browser = browser_engine::provenance();
        let desktop = desktop_engine::provenance();
        for (key, value) in browser
            .as_object()
            .expect("browser provenance is an object")
        {
            assert_eq!(
                &info["engines"]["browser"][key.as_str()],
                value,
                "browser provenance key {key} drifted"
            );
        }
        assert_eq!(info["engines"]["desktop"], desktop);
    }

    /// One finished `JoinHandle` per call accumulates
    /// be kept for the life of the process. Tracking must drop finished
    /// handles and keep running ones.
    #[tokio::test]
    async fn finished_workers_are_pruned_from_the_task_list() {
        let state = std::sync::Arc::new(State::new());

        let mut finished = tokio::spawn(async {});
        assert!((&mut finished).await.is_ok(), "the empty worker completes");
        state.track(WorkerKind::Browser, finished).await;
        assert_eq!(
            state.tasks.lock().await.len(),
            1,
            "this cycle stores the handle; pruning happens on the next track"
        );

        let running = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        state.track(WorkerKind::Desktop, running).await;
        let tasks = state.tasks.lock().await;
        assert_eq!(tasks.len(), 1, "the finished handle was pruned");
        assert!(!tasks[0].handle.is_finished());
        assert_eq!(tasks[0].kind, WorkerKind::Desktop);
        for task in tasks.iter() {
            task.handle.abort();
        }
    }

    /// A panicking engine task is observed and mapped to a typed tool
    /// error, so the request id is answered instead of left pending.
    #[tokio::test]
    async fn a_panicking_engine_task_maps_to_a_typed_answer() {
        let mut worker = tokio::spawn(async {
            panic!("simulated engine panic");
        });
        let joined = (&mut worker).await;
        let failure = joined.expect_err("a panicking task must not yield a value");
        assert!(failure.is_panic(), "the join error names a panic");
        let envelope = engine_failure_envelope("snapshot", failure.is_panic());
        assert_eq!(envelope["isError"], json!(true));
        assert_eq!(envelope["structuredContent"]["code"], json!("error"));
        assert_eq!(
            envelope["structuredContent"]["reason"],
            json!("engine_panic")
        );
        assert_eq!(
            envelope["structuredContent"]["outcome"],
            json!("unknown"),
            "a panic is never reported as success"
        );
        assert_eq!(
            envelope["structuredContent"]["retry"],
            json!("unsafe"),
            "a panic can follow a delivered action"
        );
    }

    /// The shutdown log must never blur an engine we kill with one we leave
    /// running, and must never attribute a desktop job's fate to browser
    /// cleanup. It must also never promise a restore at a bound it reached.
    #[test]
    fn shutdown_abandonment_log_distinguishes_owned_attached_and_worker_kind() {
        assert_eq!(
            EngineKind::of(&json!({"cdpUrl": "http://127.0.0.1:9222"})),
            EngineKind::Attached
        );
        assert_eq!(EngineKind::of(&json!({})), EngineKind::Owned);

        let owned_browser = EngineKind::Owned.abandonment(Duration::from_secs(5), 2, 0);
        assert!(
            owned_browser.contains("shutdown (owned)"),
            "{owned_browser}"
        );
        assert!(
            owned_browser.contains("2 browser call(s)"),
            "{owned_browser}"
        );
        assert!(owned_browser.contains("restore"), "{owned_browser}");
        assert!(owned_browser.contains("UNVERIFIED"), "{owned_browser}");
        assert!(owned_browser.contains("closed next"), "{owned_browser}");
        assert!(!owned_browser.contains("desktop"), "{owned_browser}");

        let attached_desktop = EngineKind::Attached.abandonment(Duration::from_secs(30), 0, 3);
        assert!(
            attached_desktop.contains("shutdown (attached)"),
            "{attached_desktop}"
        );
        assert!(
            attached_desktop.contains("3 desktop job(s)"),
            "{attached_desktop}"
        );
        assert!(
            attached_desktop.contains("CANNOT be aborted"),
            "a desktop job is not browser cleanup: {attached_desktop}"
        );
        assert!(
            attached_desktop.contains("outcome is UNKNOWN"),
            "{attached_desktop}"
        );
        assert!(
            !attached_desktop.contains("page state")
                && !attached_desktop.contains("the attached engine keeps running"),
            "a desktop-only shutdown has no browser page clause: {attached_desktop}"
        );
        assert!(
            !attached_desktop.contains("browser call"),
            "{attached_desktop}"
        );

        let attached_browser = EngineKind::Attached.abandonment(Duration::from_secs(30), 2, 0);
        assert!(
            attached_browser.contains("keeps running") && attached_browser.contains("UNVERIFIED"),
            "an attached browser restore is never promised: {attached_browser}"
        );

        let mixed = EngineKind::Owned.abandonment(Duration::from_secs(5), 1, 1);
        assert!(mixed.contains("1 browser call(s)"), "{mixed}");
        assert!(mixed.contains("1 desktop job(s)"), "{mixed}");
        assert!(mixed.contains("closed next"), "{mixed}");
    }

    /// The entry of a cancelled desktop call stays published until its
    /// worker ends, so a reused id is refused instead of silently riding the
    /// already-flipped token (which would be answered with nothing).
    #[tokio::test]
    async fn a_cancelled_desktop_entry_is_kept_and_refuses_id_reuse() {
        let state = std::sync::Arc::new(State::new());
        let id = json!(41);
        let key = id_key(&id);
        let entry = Arc::new(DesktopCall {
            cancelled: AtomicBool::new(false),
        });
        state
            .desktop_calls
            .lock()
            .await
            .insert(key.clone(), entry.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        cancel_call(&id, &state, &tx).await;
        assert!(
            entry.cancelled.load(Ordering::SeqCst),
            "the cancel flips this call's token"
        );
        assert!(
            state.desktop_calls.lock().await.contains_key(&key),
            "the entry stays until its worker ends"
        );
        assert!(
            rx.try_recv().is_err(),
            "desktop cancellation stays silent, per Puppet's contract"
        );

        dispatch_call(
            json!(41),
            "desktop_cursor".to_string(),
            json!({}),
            false,
            &state,
            &tx,
        )
        .await;
        let frame = rx
            .try_recv()
            .expect("a reused in-flight id is answered, never silently dropped");
        assert_eq!(frame["error"]["code"], json!(-32600), "frame: {frame}");
        assert_eq!(
            frame["error"]["data"]["reason"],
            json!("duplicate_in_flight"),
            "frame: {frame}"
        );
        assert!(
            state.desktop_calls.lock().await.contains_key(&key),
            "the earlier call keeps its own answer"
        );
    }
}

fn skills_list_result() -> Value {
    let mut result = json!({ "skills": Vec::<Value>::new() });
    static_content_meta(&mut result);
    let Some(skills) = result.get_mut("skills").and_then(|s| s.as_array_mut()) else {
        return result;
    };
    for name in skill_names() {
        if let Some(skill) = build_skill(&name) {
            skills.push(skill);
        }
    }
    skills.sort_by(|a, b| {
        let ua = a.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let ub = b.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        ua.cmp(ub)
    });
    result
}

fn skills_get_result(uri: &str) -> Result<Value, ()> {
    if uri.contains('?') || uri.contains('#') {
        return Err(());
    }
    let (skill, rel) = parse_skill_uri(uri).ok_or(())?;
    if rel != "SKILL.md" {
        return Err(());
    }
    let built = build_skill(&skill).ok_or(())?;
    let built_uri = built.get("uri").and_then(|v| v.as_str()).unwrap_or("");
    if built_uri != uri {
        return Err(());
    }
    let mut result = json!({ "skill": built });
    static_content_meta(&mut result);
    Ok(result)
}

fn resources_list_result(params: &Value) -> Value {
    let filter = params.get("uri").and_then(|u| u.as_str());
    let mut resources: Vec<Value> = Vec::new();
    for name in skill_names() {
        if let Some(skill) = build_skill(&name) {
            for entry in skill["resources"].as_array().into_iter().flatten() {
                let Some(uri) = entry["uri"].as_str() else {
                    continue;
                };
                if filter.is_some_and(|want| !uri.starts_with(want)) {
                    continue;
                }
                let name = uri.rsplit('/').next().unwrap_or(uri).to_string();
                let mime = if uri.ends_with(".md") {
                    "text/markdown"
                } else {
                    "application/octet-stream"
                };
                resources.push(json!({ "uri": uri, "name": name, "mimeType": mime }));
            }
        }
    }
    resources.sort_by(|a, b| {
        let ua = a["uri"].as_str().unwrap_or("");
        let ub = b["uri"].as_str().unwrap_or("");
        ua.cmp(ub)
    });
    let mut result = json!({ "resources": resources });
    static_content_meta(&mut result);
    result
}

fn resources_read_result(uri: &str) -> Result<Value, ()> {
    if uri.contains('?') || uri.contains('#') {
        return Err(());
    }
    let (skill, rel) = parse_skill_uri(uri).ok_or(())?;
    let file = SKILL_FILES
        .iter()
        .find(|f| f.skill == skill && f.rel == rel)
        .ok_or(())?;
    let _ = build_skill(&skill).ok_or(())?;
    let contents = if let Ok(text) = std::str::from_utf8(file.bytes) {
        json!([{
            "uri": uri,
            "mimeType": "text/markdown",
            "text": text
        }])
    } else {
        json!([{
            "uri": uri,
            "mimeType": "application/octet-stream",
            "blob": base64_encode(file.bytes)
        }])
    };
    let mut result = json!({ "contents": contents });
    static_content_meta(&mut result);
    Ok(result)
}

fn parse_skill_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("skill://")?;
    let slash = rest.find('/')?;
    let skill = &rest[..slash];
    let rel = &rest[slash + 1..];
    if skill.is_empty() || rel.is_empty() {
        return None;
    }
    if skill.contains("..") || rel.contains("..") {
        return None;
    }
    Some((skill.to_string(), rel.to_string()))
}

fn skill_names() -> Vec<String> {
    let mut names: Vec<String> = SKILL_FILES.iter().map(|f| f.skill.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

fn files_for_skill(skill: &str) -> Vec<&'static SkillFile> {
    SKILL_FILES.iter().filter(|f| f.skill == skill).collect()
}

fn build_skill(skill_name: &str) -> Option<Value> {
    let files = files_for_skill(skill_name);
    if files.is_empty() {
        return None;
    }
    let skill_md = files.iter().find(|f| f.rel == "SKILL.md")?;
    let (name, description) = parse_frontmatter(skill_md.bytes)?;
    if name != skill_name {
        return None;
    }
    if !valid_skill_name(&name) {
        return None;
    }
    if description.is_empty() || description.len() > 1024 {
        return None;
    }
    if files.len() > 512 {
        return None;
    }
    let total: usize = files.iter().map(|f| f.size).sum();
    if total > 16 * 1024 * 1024 {
        return None;
    }
    let mut resources: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "uri": format!("skill://{}/{}", f.skill, f.rel),
                "digest": f.digest,
                "size": f.size
            })
        })
        .collect();
    resources.sort_by(|a, b| {
        let ua = a.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        let ub = b.get("uri").and_then(|v| v.as_str()).unwrap_or("");
        ua.cmp(ub)
    });
    Some(json!({
        "uri": format!("skill://{}/SKILL.md", skill_name),
        "frontmatter": {
            "name": name,
            "description": description
        },
        "resources": resources
    }))
}

fn parse_frontmatter(bytes: &[u8]) -> Option<(String, String)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.strip_prefix("---\n")?;
    let close = text.find("\n---")?;
    let yaml = &text[..close];
    let mut name = None;
    let mut description = None;
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':')?;
        let key = k.trim();
        let val = unquote(v.trim());
        match key {
            "name" => name = Some(val),
            "description" => description = Some(val),
            _ => {}
        }
    }
    Some((name?, description?))
}

fn unquote(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn valid_skill_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes[0] == b'-' || bytes[name.len() - 1] == b'-' {
        return false;
    }
    let mut prev_hyphen = false;
    for &c in bytes {
        if c == b'-' {
            if prev_hyphen {
                return false;
            }
            prev_hyphen = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_hyphen = false;
        } else {
            return false;
        }
    }
    true
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[(n >> 18) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}
