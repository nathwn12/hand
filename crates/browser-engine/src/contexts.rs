//! Named isolated browser contexts: explicit create / select / teardown over
//! the one CDP connection, with separate cookies and storage.
//!
//! One context is one CDP **browser context** (`Target.createBrowserContext`):
//! Chromium's own storage partition. Cookies, `localStorage`, `sessionStorage`,
//! IndexedDB and the HTTP cache of one context are not visible to another
//! (verified live for cookies, `localStorage` and `sessionStorage`; see
//! `tests/browser_contexts.rs`). This is partitioning, not process isolation:
//! every context lives in the same browser process, so it is not a crash
//! boundary.
//!
//! Deliberate separation from the legacy surface:
//! - the legacy tab/page space is the engine's DEFAULT context only. Targets
//!   in a registered context are filtered out of the legacy `tabs` /
//!   `targets select` / `evaluate target=` path
//!   (`browser::page_targets`, `browser::select_target`,
//!   `browser::eval_in_target`), so a legacy call can never attach to - or run
//!   in - a context's page and two selection models can never share one active
//!   session. The raw `cdp` tool stays the advanced escape hatch: it can name
//!   any session the caller supplies, and that is documented, not hidden;
//! - `select` sets the default for THIS tool's actions only. Legacy
//!   `browser_*` tools keep acting on the legacy default page, and every
//!   result that could be misread says so (`selectedAffects`).
//!
//! Resource ownership (the part that has to survive cancellation):
//! - a context is registered as `phase: creating` BEFORE the first CDP call,
//!   with its own acquisition TOKEN, and its ids are recorded as they arrive.
//!   Every later mutation matches that token, never the name, so destroying and
//!   re-creating a name can never let a stale acquisition write into - or roll
//!   back - its successor;
//! - the acquisition sequence runs in its own task, so dropping the CALLER's
//!   future (per-call budget, `notifications/cancelled`, a dropped await) does
//!   not abandon a half-created context: the sequence completes and commits,
//!   and the client's unanswered call is the spec's "outcome unknown", not a
//!   silent leak. `open` commits the same way;
//! - a page created by `open` is tracked the instant `Target.createTarget`
//!   returns, so a cancellation leaves it visible (and `destroy` disposes it),
//!   never invisible;
//! - a failed create/open rolls back what it made (close the page, dispose the
//!   context) and says in the error what rollback achieved; when rollback
//!   itself cannot be proven, the resource stays tracked as `phase: creating`
//!   and is reported, never dropped. An id-less creation can always be
//!   destroyed (nothing can be named), and the stale task disposes whatever it
//!   later creates;
//! - resurrection is never a side effect: every operation except `create`
//!   sends CDP only over a connection that is already live
//!   (`Browser::cdp_if_live`), and a failed operation never reconnects.
//!
//! Losing the connection means different things, and the registry says which
//! (`Browser::ownership()`):
//! - OWNED: hand launched the process, so a browser context - engine-scoped by
//!   definition - is no longer reachable once that connection ends, and hand
//!   reports it as gone with the engine. That is a statement about reachability
//!   and about what was proved, never the claim that the process exited (the
//!   socket can drop while it runs) and never "disposed by hand". The receipt
//!   key stays `goneWithEngine`.
//! - ATTACHED (`GLADIUS_CDP_URL`): hand only lost its link. The external browser
//!   may still hold the context, so entries stay tracked as UNVERIFIED, their
//!   legacy exclusion is retained, no disposal is claimed, and `list`/`show`
//!   re-establish the explicitly configured connection and probe each id to
//!   reconcile. Mutating actions do not reconnect as a side effect of failing.
//!
//! Teardown is explicit and verified, never assumed: `destroy` disposes the
//! browser context and then proves it is gone (no page target carries its
//! `browserContextId` and `Storage.getCookies` refuses the id), reporting
//! `disposedBy: "hand"` or `"already-gone"`. Measured on pinned Chrome 158:
//! disposing a context fires `pagehide` on its pages but never `beforeunload`
//! or `unload`; `pagehide` cannot be suppressed from CDP, so it is browser
//! behaviour, not a hand action (a plain `closeTarget` does run `pagehide` +
//! `unload`), and `disposeOnDetach: true` disposes a hand-created context when
//! the CDP connection closes cleanly. With two independent CDP connections to
//! one external engine, disposing or detaching one client's contexts left the
//! other client's context, cookie and page untouched (`tests/browser_contexts.rs`,
//! attached case).
//!
//! RESIDUAL - a hard kill is not a cleanup path. `TerminateProcess`, a crash or
//! a lost machine never runs `browser::teardown`, so no WebSocket close frame is
//! sent. Measured on the pinned build, an abrupt socket drop closed that
//! connection's pages but left the browser-context object answering
//! `Storage.getCookies`; in an attached engine such a context can therefore
//! outlive hand until that engine exits. Only the explicit
//! `destroy`/`destroy_all`/`shutdown` path and `Browser::close()`'s clean close
//! are cleanup hand can prove, and no result here claims otherwise. Hand never
//! reports a hard kill as a disposal.
//!
//! Cookie verification is identity-aware: a set is reported verified only when
//! the jar holds a cookie with the requested NAME, DOMAIN and PATH and the
//! requested value and attributes (`secure`, `httpOnly`, `sameSite`); a
//! same-name cookie at another identity is named in the refusal, and an
//! identical cookie that was already there is reported `preexisting: true`
//! rather than as work hand did. Expiry is echoed but not asserted.
//!
//! Known limits (also carried in `capability_info().limits`):
//! - a JS dialog opened by a context page is NOT auto-handled: `Page` events
//!   are not enabled on context sessions, so the page's JS thread can block
//!   until the context is disposed. Legacy dialog policy is untouched;
//! - page `url` and target membership come from CDP target info. The title of
//!   a created or opened page is a live `document.title` read when one answered
//!   (`source: "targetInfo+liveRead"`); otherwise it is the engine's cached
//!   value (`source: "targetInfo"`, with `titleNote` naming a failed read).
//!   `eval` stays the route for any other live DOM evidence;
//! - pages are discovered from `Target.getTargets`, so a page opened by the
//!   site appears after the next `list`/`show`; frame-scoped refs are a
//!   legacy-surface concept and do not exist for context pages.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::browser::{is_session_death, js_no_value, js_thrown, Browser, EngineOwnership};
use crate::tools::{check_unknown, ok_text, typed_marker, ToolCode, ToolErr, ToolReason};

/// Legacy tool name. hand's catalog prefixes it, so a client sees
/// `browser_context`.
pub const TOOL: &str = "context";

/// Longest accepted context name.
pub const MAX_NAME_LEN: usize = 64;

/// Bounded settle for a page created with an explicit URL. Never an unbounded
/// wait: a slow page reports `settled: false` with its last observed
/// `readyState`.
const SETTLE_CAP: Duration = Duration::from_secs(10);

/// Bounded wait for a closed target to disappear from the engine's list.
const CLOSE_CAP: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Typed tool error minted at this module's sites (same envelope shape the
/// legacy tools use).
fn err(code: ToolCode, reason: ToolReason, prose: impl Into<String>) -> ToolErr {
    ToolErr::typed(
        prose.into(),
        json!({"code": code.as_str(), "reason": reason.as_str(), "tool": TOOL}),
    )
}

fn invalid(prose: impl Into<String>) -> ToolErr {
    err(ToolCode::InvalidArgs, ToolReason::BadValue, prose)
}

/// The registry has begun tearing down: an acquisition that has not opened a
/// connection refuses rather than launching or re-attaching. Typed as transport
/// death because no usable engine will be produced by this call.
fn shutting_down(what: &str) -> ToolErr {
    err(
        ToolCode::EngineDead,
        ToolReason::Transport,
        format!(
            "context: the context registry is shutting down; {what} refused before opening any \
             connection (nothing was launched, nothing was re-attached, and nothing is tracked)"
        ),
    )
}

/// Transport death: context state is engine-scoped, so everything is gone.
fn engine_dead(prose: impl Into<String>) -> String {
    typed_marker(ToolCode::EngineDead, ToolReason::Transport, &prose.into())
}

fn is_engine_dead(e: &str) -> bool {
    e.contains("typed:engine_dead:")
}

/// Chromium says "Failed to find context with id ..." for a missing browser
/// context on `Target.disposeBrowserContext` and "Failed to find browser
/// context for id ..." on the storage commands. Both mean the same thing.
fn is_unknown_context(e: &str) -> bool {
    e.contains("Failed to find context") || e.contains("Failed to find browser context")
}

/// The engine's own error text without the `typed:code:reason:` scaffolding.
fn plain(e: &str) -> String {
    if let Some(rest) = e.strip_prefix("typed:") {
        if let Some((_code, rest)) = rest.split_once(':') {
            if let Some((_reason, msg)) = rest.split_once(':') {
                return msg.trim_start().to_string();
            }
        }
    }
    e.to_string()
}


fn hook_flag(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

fn hook_ms(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
}

/// Name-scoped disposal failure for `destroy_all` tests (empty = none).
fn hook_dispose_failure_for(name: &str) -> bool {
    std::env::var("HAND_CONTEXT_TEST_FAIL_DISPOSE_NAME")
        .map(|v| v == name)
        .unwrap_or(false)
}

async fn hook_slow(phase: &str) {
    let Ok(want) = std::env::var("HAND_CONTEXT_TEST_SLOW_AT") else {
        return;
    };
    if want != phase {
        return;
    }
    let ms = std::env::var("HAND_CONTEXT_TEST_SLOW_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if ms > 0 {
        debug_eprintln!("[hand] test hook: slowing create/open at {phase} for {ms} ms");
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}


/// Lifecycle of a tracked context. `Creating` is visible on purpose: a caller
/// must be able to see work in flight (and a shutdown must be able to report
/// it) instead of tracking it nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Creating,
    Ready,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Creating => "creating",
            Phase::Ready => "ready",
        }
    }
}

/// How a page's `title` was obtained. A live `document.title` read and the
/// engine's cached target info are different evidence, and the tool must not
/// present one as the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TitleOrigin {
    /// Read from the document itself over an attached session.
    LiveRead,
    /// The engine's cached `Target.getTargets` title, which can lag the load
    /// (it is empty until the document has a title to report).
    TargetInfo,
}

impl TitleOrigin {
    fn as_str(self) -> &'static str {
        match self {
            TitleOrigin::LiveRead => "targetInfo+liveRead",
            TitleOrigin::TargetInfo => "targetInfo",
        }
    }
}

/// One page (CDP target) inside a context. `session` is attached lazily and
/// dropped on session death, never reused after a failed send.
#[derive(Clone, Debug)]
struct Page {
    target_id: String,
    session: Option<String>,
    url: String,
    title: String,
    /// Where `title` came from, reported as the page record's `source`.
    title_origin: TitleOrigin,
    /// Why a live title read did not supply the value (only when it failed).
    title_note: Option<String>,
    /// `performance.timeOrigin` of the document the live title came from, when
    /// one was read. A document keeps its timeOrigin across fragment and
    /// `history.pushState` navigation, so this separates "same document, new
    /// URL" from "new document" - which a URL string cannot.
    document_id: Option<String>,
}

#[derive(Clone, Debug)]
struct Entry {
    /// Identity of the acquisition that created this entry. An entry is matched
    /// by token, never by name: a destroyed-and-recreated name must never let a
    /// stale acquisition write into - or roll back - its successor.
    token: u64,
    name: String,
    phase: Phase,
    /// CDP `browserContextId`, once the engine has answered with one.
    context_id: Option<String>,
    created_ms: u64,
    /// `Browser::generation()` when the id was recorded. A browser context does
    /// not survive its engine, so a mismatch means this entry is gone.
    generation: u64,
    pages: Vec<Page>,
    /// True while the acquisition task that owns this entry is still running.
    /// A tracked-but-unstarted launch is owned by that task, so a concurrent
    /// `list` must not mistake "engine not live yet" for "context is dead".
    acquiring: bool,
    /// Why a `creating` entry has not committed (rollback failure, aborted
    /// task). `None` for a healthy entry.
    last_error: Option<String>,
    /// Why this entry's fate is UNKNOWN: set when hand's connection to an
    /// ATTACHED engine is gone (the external browser may still hold the
    /// context). An unverified entry is kept tracked, its legacy exclusion is
    /// kept, and no operation claims it is gone or disposed.
    unverified: Option<String>,
}

impl Entry {
    fn id(&self) -> Option<&str> {
        self.context_id.as_deref()
    }
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    /// Registry-scoped default (`select`). Affects this tool's actions only.
    selected: Option<String>,
    /// Monotonic acquisition tokens (`0` is never used).
    next_token: u64,
}

impl State {
    fn mint_token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    fn entry_by_token(&self, token: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.token == token)
    }

    fn entry_by_token_mut(&mut self, token: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.token == token)
    }
}

/// The named-context registry. It owns the `Browser` handle so the acquisition
/// sequence can run in a task that outlives the caller's future (cancellation
/// safety) and so teardown paths can prove whether a live connection exists
/// before sending anything.
pub struct ContextRegistry {
    browser: Arc<Browser>,
    state: Mutex<State>,
    /// Set once teardown has begun. An acquisition that has not yet opened a
    /// connection refuses instead of launching or re-attaching; one already
    /// holding a connection finishes or rolls back (its own token still
    /// resolves). Never cleared: a registry is torn down once.
    closed: AtomicBool,
}

impl ContextRegistry {
    pub fn new(browser: Arc<Browser>) -> Self {
        ContextRegistry {
            browser,
            state: Mutex::new(State::default()),
            closed: AtomicBool::new(false),
        }
    }

    /// True once `shutdown` has begun (see `closed`).
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Tracked context names in creation order (including one still
    /// `creating`; `list` reports its phase).
    pub async fn names(&self) -> Vec<String> {
        self.state
            .lock()
            .await
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    /// The registry default set by `select`.
    pub async fn selected(&self) -> Option<String> {
        self.state.lock().await.selected.clone()
    }

    /// True when nothing is tracked. No engine contact.
    pub async fn is_empty(&self) -> bool {
        self.state.lock().await.entries.is_empty()
    }

    /// Tool dispatch: the full `{content, structuredContent[, isError]}`
    /// envelope for the `context` tool.
    pub async fn call(self: &Arc<Self>, args: &Value) -> Value {
        match self.dispatch_args(args).await {
            Ok(v) => ok_text(v.to_string()),
            Err(e) => e.envelope(),
        }
    }

    /// Explicit teardown of every tracked context, best effort, never launching
    /// an engine. Called by the server at shutdown (before `Browser::close`).
    ///
    /// The receipt separates what hand PROVED from what it could not:
    /// `disposed` (hand performed and verified), `goneWithEngine` (the OWNED
    /// engine's contexts are no longer reachable: engine-scoped, and hand's
    /// connection to the process it launched is not open), `unverified` (an
    /// ATTACHED connection is gone; the external browser may still hold the
    /// context - outcome unknown, kept tracked) and `failed` (neither could be
    /// shown).
    ///
    /// Drop safety: no entry leaves the registry before its disposal is proven,
    /// so a caller whose future is dropped mid-loop (a server grace timeout, an
    /// aborted worker) leaves every unprocessed context tracked and excluded.
    pub async fn shutdown(&self) -> Value {
        self.closed.store(true, Ordering::SeqCst);
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let tokens: Vec<u64> = st.entries.iter().map(|e| e.token).collect();
        drop(st);
        let mut disposed = Vec::new();
        let mut gone_with_engine = rec.dropped;
        let mut unverified = rec.unverified;
        let mut failed = Vec::new();
        for token in tokens {
            let current = {
                let st = self.state.lock().await;
                st.entries.iter().find(|e| e.token == token).map(|e| {
                    (
                        e.name.clone(),
                        e.id().map(str::to_string),
                        e.unverified.clone(),
                    )
                })
            };
            let Some((name, id, already_unverified)) = current else {
                continue;
            };
            if already_unverified.is_some() {
                if !unverified.contains(&name) {
                    unverified.push(name);
                }
                continue;
            }
            let Some(id) = id else {
                failed.push(json!({"name": name,
                    "error": "no browserContextId was ever observed for this creating context; it stays tracked"}));
                continue;
            };
            match dispose_verified(&self.browser, &id).await {
                Ok((by, _)) => {
                    self.remove_entry_by_token(token).await;
                    disposed.push(json!({"name": name, "disposedBy": by.as_str()}));
                }
                Err(e) if is_engine_dead(&e) => match self.browser.ownership() {
                    EngineOwnership::Owned => {
                        self.remove_entry_by_token(token).await;
                        gone_with_engine.push(name);
                    }
                    EngineOwnership::Attached => {
                        self.mark_unverified(token, &attached_lost_note()).await;
                        unverified.push(name);
                    }
                },
                Err(e) => {
                    self.record_error(token, &plain(&e)).await;
                    failed.push(json!({"name": name, "error": plain(&e)}));
                }
            }
        }
        {
            let mut st = self.state.lock().await;
            if let Some(sel) = st.selected.clone() {
                if !st.entries.iter().any(|e| e.name == sel) {
                    st.selected = None;
                }
            }
        }
        json!({"contexts": disposed.len() + gone_with_engine.len() + unverified.len() + failed.len(),
               "disposed": disposed, "goneWithEngine": gone_with_engine,
               "unverified": unverified, "failed": failed})
    }


    async fn dispatch_args(self: &Arc<Self>, args: &Value) -> Result<Value, ToolErr> {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) if !a.is_empty() => a.to_string(),
            Some(_) => {
                return Err(err(
                    ToolCode::InvalidArgs,
                    ToolReason::BadValue,
                    "context: 'action' must be a non-empty string",
                ))
            }
            None => {
                return Err(err(
                    ToolCode::InvalidArgs,
                    ToolReason::MissingArg,
                    "context: missing required arg 'action' (create|list|show|select|open|close|destroy|destroy_all|eval|cookies|clock|locale|permissions|reset|export_state|import_state)",
                ))
            }
        };
        let allowed: &[&str] = match action.as_str() {
            "create" => &["action", "name", "url"],
            "list" => &["action"],
            "show" => &["action", "name"],
            "select" => &["action", "name"],
            "open" => &["action", "name", "url"],
            "close" => &["action", "name", "page"],
            "destroy" => &["action", "name"],
            "destroy_all" => &["action"],
            "eval" => &["action", "name", "page", "expression"],
            "cookies" => &["action", "name", "cookies"],
            "clock" => &["action", "name", "clock", "iso"],
            "locale" => &["action", "name", "locale", "timezone"],
            "permissions" => &["action", "name", "permissions", "grant", "mode", "prompt"],
            "reset" => &["action", "name"],
            "export_state" => &["action", "name", "path"],
            "import_state" => &["action", "name", "path"],
            other => {
                return Err(err(
                    ToolCode::InvalidArgs,
                    ToolReason::UnknownAction,
                    format!(
                        "context: unknown action '{other}' (create|list|show|select|open|close|destroy|destroy_all|eval|cookies|clock|locale|permissions|reset|export_state|import_state)"
                    ),
                ))
            }
        };
        check_unknown(TOOL, args, allowed)?;
        let want_name = args.get("name").and_then(|v| v.as_str());
        let url = args.get("url").and_then(|v| v.as_str());
        let page = args.get("page").and_then(|v| v.as_str());
        let expression = args.get("expression").and_then(|v| v.as_str());
        let iso = args.get("iso").and_then(|v| v.as_str());
        let clock = args.get("clock").and_then(|v| v.as_str());
        let locale = args.get("locale").and_then(|v| v.as_str());
        let timezone = args.get("timezone").and_then(|v| v.as_str());
        let mode = args.get("mode").and_then(|v| v.as_str());
        let prompt = args.get("prompt").and_then(|v| v.as_bool()).unwrap_or(false);
        let state_path = args.get("path").and_then(|v| v.as_str());

        match action.as_str() {
            "create" => {
                let raw = want_name.ok_or_else(|| {
                    err(
                        ToolCode::InvalidArgs,
                        ToolReason::MissingArg,
                        "context: create needs 'name' (a stable caller-chosen label)",
                    )
                })?;
                let name = normalise_name(raw)?;
                self.create(&name, url).await
            }
            "list" => self.list().await,
            "show" => self.show(want_name).await,
            "select" => {
                let raw = want_name.ok_or_else(|| {
                    err(
                        ToolCode::InvalidArgs,
                        ToolReason::MissingArg,
                        "context: select needs 'name'",
                    )
                })?;
                self.select(raw).await
            }
            "open" => self.open(want_name, url).await,
            "close" => {
                let page = page.ok_or_else(|| {
                    err(
                        ToolCode::InvalidArgs,
                        ToolReason::MissingArg,
                        "context: close needs 'page' (a targetId from show/list)",
                    )
                })?;
                self.close_page(want_name, page).await
            }
            "destroy" => {
                let raw = want_name.ok_or_else(|| {
                    err(
                        ToolCode::InvalidArgs,
                        ToolReason::MissingArg,
                        "context: destroy needs 'name'",
                    )
                })?;
                self.destroy(raw).await
            }
            "destroy_all" => self.destroy_all().await,
            "eval" => {
                let expression = expression.ok_or_else(|| {
                    err(
                        ToolCode::InvalidArgs,
                        ToolReason::MissingArg,
                        "context: eval needs 'expression'",
                    )
                })?;
                self.eval(want_name, page, expression).await
            }
            "cookies" => {
                let set = match args.get("cookies") {
                    None => None,
                    Some(Value::Array(items)) => Some(items.as_slice()),
                    Some(other) => {
                        return Err(err(
                            ToolCode::InvalidArgs,
                            ToolReason::BadValue,
                            format!("context: 'cookies' must be an array, got {other}"),
                        ))
                    }
                };
                self.cookies(want_name, set).await
            }
            "clock" => self.clock(want_name, iso.or(clock)).await,
            "locale" => self.locale(want_name, locale, timezone).await,
            "permissions" => {
                let raw = args.get("grant").or_else(|| args.get("permissions"));
                let grants = string_array(raw)?;
                self.permissions(want_name, &grants, mode, prompt).await
            }
            "reset" => self.reset(want_name).await,
            "export_state" => {
                let path = state_path.ok_or_else(|| {
                    invalid("context: export_state needs 'path' (an absolute JSON file path)")
                })?;
                self.export_state(want_name, path).await
            }
            "import_state" => {
                let path = state_path.ok_or_else(|| {
                    invalid("context: import_state needs 'path' (an absolute JSON file path)")
                })?;
                self.import_state(want_name, path).await
            }
            _ => unreachable!("action already validated against the closed set"),
        }
    }


    /// Create a context and its first page. The entry is tracked before the
    /// first CDP call - with its own acquisition token - and the acquisition
    /// runs in its own task, so a cancelled caller cannot orphan a context and a
    /// stale acquisition can never write into a later entry of the same name.
    async fn create(self: &Arc<Self>, name: &str, url: Option<&str>) -> Result<Value, ToolErr> {
        if self.is_closed() {
            return Err(shutting_down("create"));
        }
        let url = normalise_url(url)?;
        let (token, dropped) = {
            let mut st = self.state.lock().await;
            let rec = reconcile(&self.browser, &mut st, false).await;
            if let Some(existing) = st.entries.iter().find(|e| e.name == name) {
                let phase = existing.phase.as_str();
                let pursuing = if existing.acquiring {
                    " (an acquisition is still running)"
                } else {
                    ""
                };
                return Err(invalid(format!(
                    "context: a context named '{name}' already exists (phase {phase}{pursuing}); names are not replaced - destroy it first or choose another name"
                )));
            }
            let token = st.mint_token();
            st.entries.push(Entry {
                token,
                name: name.to_string(),
                phase: Phase::Creating,
                context_id: None,
                created_ms: now_ms(),
                generation: self.browser.generation(),
                pages: Vec::new(),
                acquiring: true,
                last_error: None,
                unverified: None,
            });
            (token, rec.dropped)
        };
        let me = Arc::clone(self);
        let owned = name.to_string();
        let url_owned = url.clone();
        let handle =
            tokio::spawn(
                async move { me.create_sequence(token, &owned, &url_owned, dropped).await },
            );
        match handle.await {
            Ok(r) => r,
            Err(join) => {
                self.note_failure(
                    token,
                    &format!("create task aborted before it committed: {join}"),
                )
                .await;
                Err(err(
                    ToolCode::Cancelled,
                    ToolReason::ClientCancelled,
                    format!(
                        "context: create '{name}' was aborted before it committed; a partly created context stays tracked as phase 'creating' (acquisition {token}) and destroy can remove it, or dispose it once its id is known - nothing is claimed created"
                    ),
                ))
            }
        }
    }

    /// The acquisition sequence. Runs detached from the caller, and matches its
    /// own entry by TOKEN: if the name was destroyed and re-created meanwhile,
    /// this sequence only ever touches the entry it owns.
    async fn create_sequence(
        self: &Arc<Self>,
        token: u64,
        name: &str,
        url: &str,
        dropped: Vec<String>,
    ) -> Result<Value, ToolErr> {
        hook_slow("before-create").await;
        if self.is_closed() {
            self.remove_entry_by_token(token).await;
            return Err(shutting_down("create"));
        }
        let context_id = match create_browser_context(&self.browser).await {
            Ok(id) => id,
            Err(e) => {
                self.remove_entry_by_token(token).await;
                if is_engine_dead(&e) {
                    return Err(ToolErr::from(e));
                }
                return Err(ToolErr::wrapping(
                    &e,
                    format!(
                        "context: {}; no browserContextId was observed, so nothing is tracked as created",
                        plain(&e)
                    ),
                ));
            }
        };
        self.browser.exclude_browser_context(&context_id, name);
        // Row B3: bind the name so a route armed against it is removed when
        // this context is torn down. No engine contact, no claim about state.
        crate::net::bind_context(&self.browser, name, &context_id).await;
        {
            let mut st = self.state.lock().await;
            match st.entry_by_token_mut(token) {
                Some(e) => {
                    e.context_id = Some(context_id.clone());
                    e.generation = self.browser.generation();
                }
                None => {
                    drop(st);
                    let _ = dispose_verified(&self.browser, &context_id).await;
                    self.browser.include_browser_context(&context_id);
                    return Err(err(
                        ToolCode::TargetChanged,
                        ToolReason::Raced,
                        format!(
                            "context: acquisition {token} for '{name}' was removed while it was being created; the new context was disposed and nothing is tracked"
                        ),
                    ));
                }
            }
        }
        let target_id = match create_page(&self.browser, &context_id, url).await {
            Ok(t) => t,
            Err(e) => {
                return Err(self
                    .rollback_create(token, name, &context_id, None, &e)
                    .await)
            }
        };
        {
            let mut st = self.state.lock().await;
            if let Some(e) = st.entry_by_token_mut(token) {
                e.pages.push(Page {
                    target_id: target_id.clone(),
                    session: None,
                    url: String::new(),
                    title: String::new(),
                    title_origin: TitleOrigin::TargetInfo,
                    title_note: None,
                    document_id: None,
                });
            }
        }
        hook_slow("after-page").await;
        let mut session = match attach_page(&self.browser, &target_id).await {
            Ok(s) => s,
            Err(e) => {
                return Err(self
                    .rollback_create(token, name, &context_id, Some(&target_id), &e)
                    .await)
            }
        };
        {
            let mut st = self.state.lock().await;
            if let Some(p) = st
                .entry_by_token_mut(token)
                .and_then(|e| e.pages.iter_mut().find(|p| p.target_id == target_id))
            {
                p.session = Some(session.clone());
            }
        }
        hook_slow("after-attach").await;
        let (settled, ready_state) = match settle_page(
            &self.browser,
            &target_id,
            &mut session,
            SETTLE_CAP,
            Some(url),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(self
                    .rollback_create(token, name, &context_id, Some(&target_id), &e)
                    .await)
            }
        };
        let info = match page_info(&self.browser, &target_id).await {
            Ok(i) => i,
            Err(e) => {
                return Err(self
                    .rollback_create(token, name, &context_id, Some(&target_id), &e)
                    .await)
            }
        };
        let live = match live_title(&self.browser, &target_id, &mut session).await {
            Ok(v) => Ok(v),
            Err(e) if is_engine_dead(&e) => {
                return Err(self
                    .rollback_create(token, name, &context_id, Some(&target_id), &e)
                    .await)
            }
            Err(e) => Err(e),
        };
        let (title, title_origin, title_note) = reported_title(&info.title, live);
        let document_id = document_identity(&self.browser, &session, title_origin).await;
        let mut st = self.state.lock().await;
        let others: Vec<String> = st
            .entries
            .iter()
            .filter(|e| e.token != token)
            .map(|e| e.name.clone())
            .collect();
        if st.entry_by_token(token).is_none() {
            drop(st);
            let _ = close_target_verified(&self.browser, &context_id, &target_id).await;
            let _ = dispose_verified(&self.browser, &context_id).await;
            self.browser.include_browser_context(&context_id);
            return Err(err(
                ToolCode::TargetChanged,
                ToolReason::Raced,
                format!(
                    "context: acquisition {token} for '{name}' was removed while it was being created; the new context was disposed and nothing is tracked"
                ),
            ));
        }
        st.selected = Some(name.to_string());
        let entry = st
            .entry_by_token_mut(token)
            .expect("checked immediately above");
        entry.phase = Phase::Ready;
        entry.acquiring = false;
        entry.last_error = None;
        entry.unverified = None;
        if let Some(p) = entry.pages.iter_mut().find(|p| p.target_id == target_id) {
            p.url = info.url.clone();
            p.title = title.clone();
            p.title_origin = title_origin;
            p.title_note = title_note.clone();
            p.document_id = document_id;
            p.session = Some(session.clone());
        }
        let view = entry_view(entry, Some(name), &others);
        drop(st);
        let mut out = json!({
            "ok": true,
            "name": name,
            "browserContextId": context_id,
            "phase": Phase::Ready.as_str(),
            "context": view,
            "pages": view.get("pages").cloned().unwrap_or(json!([])),
            "page": view.get("pages").and_then(|p| p.get(0)).cloned().unwrap_or(Value::Null),
            "url": info.url,
            "title": title,
            "source": title_origin.as_str(),
            "settled": settled,
            "readyState": ready_state,
            "selected": name,
            "selectedAffects": SELECT_AFFECTS,
            "engine": engine_label(),
            "dropped": dropped,
            "limits": LIMITS,
        });
        if let (Some(note), Some(o)) = (title_note, out.as_object_mut()) {
            o.insert("titleNote".to_string(), json!(note));
        }
        Ok(out)
    }

    /// Roll back a failed acquisition: close the page that was made (when any),
    /// dispose the context, and only then stop tracking THIS acquisition (by
    /// token). Anything that cannot be proven gone stays tracked as `creating`
    /// with the reason. A successor entry of the same name is never touched.
    async fn rollback_create(
        &self,
        token: u64,
        name: &str,
        context_id: &str,
        page: Option<&str>,
        cause: &str,
    ) -> ToolErr {
        let page_result = match page {
            Some(t) => Some(close_target_verified(&self.browser, context_id, t).await),
            None => None,
        };
        let disposed = dispose_verified(&self.browser, context_id).await;
        let engine_gone =
            is_engine_dead(cause) || disposed.as_ref().err().is_some_and(|e| is_engine_dead(e));
        let page_note = match &page_result {
            None => String::new(),
            Some(Ok(())) => "the new page was closed".to_string(),
            Some(Err(e)) => format!("the new page could NOT be closed ({})", plain(e)),
        };
        let ctx_note = match &disposed {
            Ok((by, _)) => format!("the new context was disposed ({})", by.as_str()),
            Err(e) if is_engine_dead(e) => match self.browser.ownership() {
                EngineOwnership::Owned => {
                    "hand's connection to the owned engine it launched ended, so the new context is no \
                     longer reachable (engine-scoped; hand does not claim the process exited)"
                        .to_string()
                }
                EngineOwnership::Attached => {
                    "hand's connection is gone; the new context may still exist (outcome unknown)"
                        .to_string()
                }
            },
            Err(e) => format!(
                "the new context could NOT be disposed ({}) and stays tracked as phase 'creating'",
                plain(e)
            ),
        };
        let mut st = self.state.lock().await;
        let proven_gone = engine_gone && matches!(self.browser.ownership(), EngineOwnership::Owned)
            || disposed.is_ok();
        if proven_gone {
            crate::net::on_context_gone(&self.browser, context_id).await;
            st.entries.retain(|e| e.token != token);
            self.browser.include_browser_context(context_id);
        } else if let Some(e) = st.entry_by_token_mut(token) {
            e.acquiring = false;
            e.last_error = Some(format!("{ctx_note}; {page_note}"));
        }
        drop(st);
        let joined = [page_note, ctx_note]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        ToolErr::wrapping(
            cause,
            format!(
                "context: acquisition {token} for '{name}': {}; {joined}",
                plain(cause)
            ),
        )
    }

    /// Every tracked context, refreshed from `Target.getTargets` so a page
    /// opened by the site or closed elsewhere is reported truthfully. Never
    /// launches an engine. Ownership decides what a lost connection means: an
    /// owned connection that is not open drops its contexts, an attached connection
    /// that is gone keeps them as UNVERIFIED.
    async fn list(&self) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let mut rec = reconcile(&self.browser, &mut st, true).await;
        let live = self.browser.is_live().await;
        let mut out = Vec::new();
        for i in 0..st.entries.len() {
            let refreshable = live
                && st.entries[i].phase == Phase::Ready
                && st.entries[i].id().is_some()
                && st.entries[i].unverified.is_none();
            if refreshable {
                if let Err(e) = refresh_pages(&self.browser, &mut st.entries[i]).await {
                    if is_engine_dead(&e) {
                        let again = reconcile(&self.browser, &mut st, false).await;
                        rec.dropped.extend(again.dropped);
                        rec.unverified.extend(again.unverified);
                        out.clear();
                        for e in &st.entries {
                            out.push(entry_summary(e, st.selected.as_deref()));
                        }
                        break;
                    }
                    return Err(ToolErr::from(e));
                }
            }
            out.push(entry_summary(&st.entries[i], st.selected.as_deref()));
        }
        Ok(json!({
            "ok": true,
            "contexts": out,
            "selected": st.selected,
            "dropped": rec.dropped,
            "unverified": rec.unverified,
            "selectedAffects": SELECT_AFFECTS,
            "engine": engine_label(),
            "ownership": self.browser.ownership().as_str(),
            "limits": LIMITS,
        }))
    }

    /// One context. An UNVERIFIED entry is reported as such (inspection must not
    /// pretend), and a lost OWNED engine drops it (an engine-scoped context is
    /// no longer reachable; the receipt does not claim the process exited).
    async fn show(&self, want_name: Option<&str>) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, true).await;
        let name = match resolve_name(&st, want_name) {
            Ok(n) => n,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        let idx = entry_index(&st, &name)?;
        let live = self.browser.is_live().await;
        if live
            && st.entries[idx].phase == Phase::Ready
            && st.entries[idx].id().is_some()
            && st.entries[idx].unverified.is_none()
        {
            if let Err(e) = refresh_pages(&self.browser, &mut st.entries[idx]).await {
                if is_engine_dead(&e) {
                    let _ = reconcile(&self.browser, &mut st, false).await;
                    return Err(ToolErr::from(e));
                }
                return Err(ToolErr::from(e));
            }
        }
        let others: Vec<String> = st
            .entries
            .iter()
            .filter(|e| e.token != st.entries[idx].token)
            .map(|e| e.name.clone())
            .collect();
        let mut view = entry_view(&st.entries[idx], st.selected.as_deref(), &others);
        if let Value::Object(ref mut o) = view {
            o.insert("ok".to_string(), json!(true));
            o.insert("selectedAffects".to_string(), json!(SELECT_AFFECTS));
            o.insert("engine".to_string(), json!(engine_label()));
            o.insert(
                "ownership".to_string(),
                json!(self.browser.ownership().as_str()),
            );
            o.insert("limits".to_string(), json!(LIMITS));
            o.insert("dropped".to_string(), json!(rec.dropped));
        }
        Ok(view)
    }

    /// Set the registry default. Deliberately does NOT touch legacy selection:
    /// the result says so, because claiming otherwise would be false. An
    /// UNVERIFIED context cannot be selected: that would be a claim about it.
    async fn select(&self, raw_name: &str) -> Result<Value, ToolErr> {
        let name = normalise_name(raw_name)?;
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let idx = match entry_index(&st, &name) {
            Ok(i) => i,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        if st.entries[idx].phase != Phase::Ready {
            return Err(still_creating(&name));
        }
        st.selected = Some(name.clone());
        Ok(json!({
            "ok": true,
            "selected": name,
            "browserContextId": st.entries[idx].context_id,
            "selectedAffects": SELECT_AFFECTS,
            "legacyUnaffected": true,
        }))
    }

    /// Add a page. The acquisition runs in its own task (like `create`), so a
    /// dropped caller can never leave a page the registry never saw: the page is
    /// recorded as soon as `Target.createTarget` returns, and the task commits
    /// or rolls it back.
    async fn open(
        self: &Arc<Self>,
        want_name: Option<&str>,
        url: Option<&str>,
    ) -> Result<Value, ToolErr> {
        if self.is_closed() {
            return Err(shutting_down("open"));
        }
        let url = normalise_url(url)?;
        let (token, name) = {
            let mut st = self.state.lock().await;
            let rec = reconcile(&self.browser, &mut st, false).await;
            let name = match resolve_name(&st, want_name) {
                Ok(n) => n,
                Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
            };
            let idx = entry_index(&st, &name)?;
            if st.entries[idx].unverified.is_some() {
                return Err(attached_unknown(&name));
            }
            if st.entries[idx].phase != Phase::Ready {
                return Err(still_creating(&name));
            }
            if st.entries[idx].id().is_none() {
                return Err(attached_unknown(&name));
            }
            (st.entries[idx].token, name)
        };
        let me = Arc::clone(self);
        let url_owned = url.clone();
        let handle = tokio::spawn(async move { me.open_sequence(token, &url_owned).await });
        match handle.await {
            Ok(r) => r,
            Err(join) => Err(err(
                ToolCode::Cancelled,
                ToolReason::ClientCancelled,
                format!(
                    "context: open in '{name}' was aborted ({join}); a page it had already created stays tracked so destroy/list can see it - nothing else is claimed"
                ),
            )),
        }
    }

    /// The page-acquisition sequence, committed by token.
    async fn open_sequence(self: &Arc<Self>, token: u64, url: &str) -> Result<Value, ToolErr> {
        let (name, context_id) = {
            let st = self.state.lock().await;
            match st.entry_by_token(token) {
                Some(i) => match st.entries[i].id() {
                    Some(id) => (st.entries[i].name.clone(), id.to_string()),
                    None => {
                        return Err(err(
                            ToolCode::EngineDead,
                            ToolReason::Transport,
                            "context: the context has no browserContextId any more".to_string(),
                        ))
                    }
                },
                None => {
                    return Err(err(
                        ToolCode::TargetChanged,
                        ToolReason::Raced,
                        "context: the context was destroyed before the page was opened".to_string(),
                    ))
                }
            }
        };
        let target_id = match create_page(&self.browser, &context_id, url).await {
            Ok(t) => t,
            Err(e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
        };
        {
            let mut st = self.state.lock().await;
            match st.entry_by_token_mut(token) {
                Some(e) => e.pages.push(Page {
                    target_id: target_id.clone(),
                    session: None,
                    url: String::new(),
                    title: String::new(),
                    title_origin: TitleOrigin::TargetInfo,
                    title_note: None,
                    document_id: None,
                }),
                None => {
                    drop(st);
                    let _ = close_target_verified(&self.browser, &context_id, &target_id).await;
                    return Err(err(
                        ToolCode::TargetChanged,
                        ToolReason::Raced,
                        format!(
                            "context: '{name}' was destroyed while a page was being opened in it; the new page was closed"
                        ),
                    ));
                }
            }
        }
        let mut session = match attach_page(&self.browser, &target_id).await {
            Ok(s) => s,
            Err(e) => {
                return Err(self
                    .rollback_page(token, &name, &context_id, &target_id, &e)
                    .await)
            }
        };
        {
            let mut st = self.state.lock().await;
            if let Some(p) = st
                .entry_by_token_mut(token)
                .and_then(|e| e.pages.iter_mut().find(|p| p.target_id == target_id))
            {
                p.session = Some(session.clone());
            }
        }
        hook_slow("open-after-attach").await;
        let (settled, ready_state) = match settle_page(
            &self.browser,
            &target_id,
            &mut session,
            SETTLE_CAP,
            Some(url),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(self
                    .rollback_page(token, &name, &context_id, &target_id, &e)
                    .await)
            }
        };
        let info = match page_info(&self.browser, &target_id).await {
            Ok(i) => i,
            Err(e) => {
                return Err(self
                    .rollback_page(token, &name, &context_id, &target_id, &e)
                    .await)
            }
        };
        let live = match live_title(&self.browser, &target_id, &mut session).await {
            Ok(v) => Ok(v),
            Err(e) if is_engine_dead(&e) => {
                return Err(self
                    .rollback_page(token, &name, &context_id, &target_id, &e)
                    .await)
            }
            Err(e) => Err(e),
        };
        let (title, title_origin, title_note) = reported_title(&info.title, live);
        let document_id = document_identity(&self.browser, &session, title_origin).await;
        let mut st = self.state.lock().await;
        let Some(entry) = st.entry_by_token_mut(token) else {
            drop(st);
            let _ = close_target_verified(&self.browser, &context_id, &target_id).await;
            return Err(err(
                ToolCode::TargetChanged,
                ToolReason::Raced,
                format!(
                    "context: '{name}' was destroyed while its new page was settling; the page was closed and nothing is reported opened"
                ),
            ));
        };
        if let Some(p) = entry.pages.iter_mut().find(|p| p.target_id == target_id) {
            p.url = info.url.clone();
            p.title = title.clone();
            p.title_origin = title_origin;
            p.title_note = title_note.clone();
            p.document_id = document_id;
            p.session = Some(session.clone());
        }
        let pages = entry.pages.iter().map(page_view).collect::<Vec<_>>();
        drop(st);
        let mut page = json!({
            "targetId": target_id,
            "url": info.url,
            "title": title,
            "settled": settled,
            "readyState": ready_state,
            "source": title_origin.as_str(),
            "sessionSource": "hand-tracked (not a live probe)",
        });
        if let (Some(note), Some(o)) = (title_note, page.as_object_mut()) {
            o.insert("titleNote".to_string(), json!(note));
        }
        Ok(json!({
            "ok": true,
            "name": name,
            "browserContextId": context_id,
            "page": page,
            "pages": pages,
            "selectedAffects": SELECT_AFFECTS,
            "limits": LIMITS,
        }))
    }

    /// Close the page an `open` just made when a later step failed. If the
    /// close cannot be proven, the page stays tracked (it was recorded before
    /// the failing step) and the error says so.
    async fn rollback_page(
        &self,
        token: u64,
        name: &str,
        context_id: &str,
        target_id: &str,
        cause: &str,
    ) -> ToolErr {
        let closed = close_target_verified(&self.browser, context_id, target_id).await;
        let note = match &closed {
            Ok(()) => {
                let mut st = self.state.lock().await;
                if let Some(e) = st.entry_by_token_mut(token) {
                    e.pages.retain(|p| p.target_id != target_id);
                }
                "the new page was closed".to_string()
            }
            Err(e) => {
                let why = plain(e);
                let mut st = self.state.lock().await;
                if let Some(entry) = st.entry_by_token_mut(token) {
                    entry.last_error =
                        Some(format!("a page {target_id} could not be closed: {why}"));
                }
                format!(
                    "the new page {target_id} could NOT be closed ({why}) and stays tracked in the context"
                )
            }
        };
        ToolErr::wrapping(cause, format!("context '{name}': {}; {note}", plain(cause)))
    }

    async fn close_page(&self, want_name: Option<&str>, target_id: &str) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let name = match resolve_name(&st, want_name) {
            Ok(n) => n,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        let idx = entry_index(&st, &name)?;
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        if st.entries[idx].phase != Phase::Ready {
            return Err(still_creating(&name));
        }
        let Some(context_id) = st.entries[idx].context_id.clone() else {
            return Err(attached_unknown(&name));
        };
        let token = st.entries[idx].token;
        if !st.entries[idx]
            .pages
            .iter()
            .any(|p| p.target_id == target_id)
        {
            return Err(err(
                ToolCode::InvalidTarget,
                ToolReason::NotFound,
                format!(
                    "context: page '{target_id}' is not in context '{name}' (pages: {:?})",
                    st.entries[idx]
                        .pages
                        .iter()
                        .map(|p| p.target_id.as_str())
                        .collect::<Vec<_>>()
                ),
            ));
        }
        drop(st);
        match close_target_verified(&self.browser, &context_id, target_id).await {
            Ok(()) => {
                if let Some(ms) = hook_ms("HAND_CONTEXT_TEST_SLOW_ON_CLOSE_MS") {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }
            Err(e) if is_engine_dead(&e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
            Err(e) => return Err(ToolErr::from(e)),
        }
        let mut st = self.state.lock().await;
        let Some(entry) = st.entry_by_token_mut(token) else {
            return Err(err(
                ToolCode::TargetChanged,
                ToolReason::Raced,
                format!(
                    "context: '{name}' was destroyed while page {target_id} was closing; the page is gone (verified) and the context no longer exists"
                ),
            ));
        };
        entry.pages.retain(|p| p.target_id != target_id);
        let pages = entry.pages.iter().map(page_view).collect::<Vec<_>>();
        drop(st);
        Ok(json!({
            "ok": true,
            "name": name,
            "page": target_id,
            "gone": true,
            "pages": pages,
            "note": if pages.is_empty() {
                "the context now has no page; `open` adds one, `destroy` tears the context down"
            } else {
                "the context and its other pages are untouched"
            },
            "limits": LIMITS,
        }))
    }

    /// Dispose one context and PROVE it is gone.
    ///
    /// - OWNED engine loss: hand's connection to the process it launched ended, so the
    ///   is gone; it is dropped and reported as such (never "disposed by hand").
    /// - ATTACHED connection loss: outcome UNKNOWN; the entry and its legacy
    ///   exclusion are KEPT, and no disposal is claimed.
    /// - A failed disposal with a live connection keeps entry + exclusion.
    /// - An id-less `creating` entry can always be removed from tracking (its
    ///   acquisition task disposes whatever it created once it learns the id).
    async fn destroy(&self, raw_name: &str) -> Result<Value, ToolErr> {
        let name = normalise_name(raw_name)?;
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let idx = match entry_index(&st, &name) {
            Ok(i) => i,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        let token = st.entries[idx].token;
        let created_ms = st.entries[idx].created_ms;
        let page_count = st.entries[idx].pages.len();
        let Some(context_id) = st.entries[idx].context_id.clone() else {
            st.entries.retain(|e| e.token != token);
            if st.selected.as_deref() == Some(name.as_str()) {
                st.selected = None;
            }
            return Ok(json!({
                "ok": true,
                "name": name,
                "browserContextId": Value::Null,
                "gone": false,
                "disposedBy": "none",
                "disposedPages": 0,
                "createdAt": created_ms,
                "note": "no browserContextId was ever observed for this acquisition, so nothing could be named or disposed; it is no longer tracked, and if the acquisition ever learns its id it disposes that context itself",
                "selected": st.selected,
            }));
        };
        match dispose_verified(&self.browser, &context_id).await {
            Ok((by, evidence)) => {
                crate::net::on_context_gone(&self.browser, &context_id).await;
                st.entries.retain(|e| e.token != token);
                if st.selected.as_deref() == Some(name.as_str()) {
                    st.selected = None;
                }
                self.browser.include_browser_context(&context_id);
                Ok(json!({
                    "ok": true,
                    "name": name,
                    "browserContextId": context_id,
                    "gone": true,
                    "disposedBy": by.as_str(),
                    "disposedPages": page_count,
                    "createdAt": created_ms,
                    "evidence": evidence,
                    "selected": st.selected,
                }))
            }
            Err(e) if is_engine_dead(&e) => match self.browser.ownership() {
                EngineOwnership::Owned => {
                    crate::net::on_context_gone(&self.browser, &context_id).await;
                    st.entries.retain(|e| e.token != token);
                    if st.selected.as_deref() == Some(name.as_str()) {
                        st.selected = None;
                    }
                    self.browser.include_browser_context(&context_id);
                    Err(err(
                        ToolCode::EngineDead,
                        ToolReason::Transport,
                        format!(
                            "context: '{name}' is no longer reachable - hand's connection to the owned engine it launched ended, and a browser context is engine-scoped; hand did not dispose it and does not claim the process exited: {}",
                            plain(&e)
                        ),
                    ))
                }
                EngineOwnership::Attached => {
                    if let Some(entry) = st.entry_by_token_mut(token) {
                        entry.unverified = Some(attached_lost_note());
                    }
                    Err(err(
                        ToolCode::EngineDead,
                        ToolReason::Transport,
                        format!(
                            "context: '{name}' is UNVERIFIED - hand's connection to the attached engine is gone, the external browser may still hold it, and nothing was disposed (outcome unknown): {}",
                            plain(&e)
                        ),
                    ))
                }
            },
            Err(e) => Err(ToolErr::wrapping(
                &e,
                format!(
                    "context: {}; context '{name}' stays tracked and stays excluded from the legacy tab space",
                    plain(&e)
                ),
            )),
        }
    }

    /// Destroy every tracked context. Drop-safe like `shutdown`: no entry
    /// leaves the registry before its disposal is proven, so a dropped caller
    /// leaves the rest tracked (and reported as `remaining`).
    async fn destroy_all(&self) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let tokens: Vec<u64> = st.entries.iter().map(|e| e.token).collect();
        drop(st);
        let mut destroyed = Vec::new();
        let mut gone_with_engine = rec.dropped;
        let mut unverified = rec.unverified;
        let mut pending = Vec::new();
        let mut failed = Vec::new();
        for token in tokens {
            let current = {
                let st = self.state.lock().await;
                st.entries.iter().find(|e| e.token == token).map(|e| {
                    (
                        e.name.clone(),
                        e.id().map(str::to_string),
                        e.unverified.clone(),
                    )
                })
            };
            let Some((name, id, already_unverified)) = current else {
                continue;
            };
            if already_unverified.is_some() {
                if !unverified.contains(&name) {
                    unverified.push(name);
                }
                continue;
            }
            let Some(id) = id else {
                if !pending.contains(&name) {
                    pending.push(name);
                }
                continue;
            };
            let disposal = if hook_dispose_failure_for(&name) {
                Err(typed_marker(
                    ToolCode::TargetChanged,
                    ToolReason::Raced,
                    "context: dispose forced to fail by HAND_CONTEXT_TEST_FAIL_DISPOSE_NAME (test-only)",
                ))
            } else {
                dispose_verified(&self.browser, &id).await
            };
            match disposal {
                Ok((by, _)) => {
                    self.remove_entry_by_token(token).await;
                    destroyed.push(json!({"name": name, "disposedBy": by.as_str()}));
                }
                Err(e) if is_engine_dead(&e) => match self.browser.ownership() {
                    EngineOwnership::Owned => {
                        self.remove_entry_by_token(token).await;
                        gone_with_engine.push(name);
                    }
                    EngineOwnership::Attached => {
                        self.mark_unverified(token, &attached_lost_note()).await;
                        if !unverified.contains(&name) {
                            unverified.push(name);
                        }
                    }
                },
                Err(e) => {
                    self.record_error(token, &plain(&e)).await;
                    failed.push(json!({"name": name, "error": plain(&e)}));
                }
            }
        }
        let mut st = self.state.lock().await;
        if let Some(sel) = st.selected.clone() {
            if !st.entries.iter().any(|e| e.name == sel) {
                st.selected = None;
            }
        }
        let remaining: Vec<String> = st.entries.iter().map(|e| e.name.clone()).collect();
        let all_done = failed.is_empty() && unverified.is_empty() && pending.is_empty();
        if all_done {
            Ok(
                json!({"ok": true, "destroyed": destroyed, "goneWithEngine": gone_with_engine,
                      "unverified": [], "pending": [], "failed": [], "remaining": remaining}),
            )
        } else {
            Err(ToolErr::typed(
                format!(
                    "context: destroy_all disposed {}, {} are no longer reachable with the owned engine, {} are UNVERIFIED (attached connection), {} are still being created, {} could not be disposed; remaining {:?}",
                    destroyed.len(),
                    gone_with_engine.len(),
                    unverified.len(),
                    pending.len(),
                    failed.len(),
                    remaining
                ),
                json!({"code": ToolCode::TargetChanged.as_str(),
                       "reason": ToolReason::Raced.as_str(),
                       "tool": TOOL,
                       "destroyed": destroyed,
                       "goneWithEngine": gone_with_engine,
                       "unverified": unverified,
                       "pending": pending,
                       "failed": failed,
                       "remaining": remaining}),
            ))
        }
    }

    async fn eval(
        &self,
        want_name: Option<&str>,
        page: Option<&str>,
        expression: &str,
    ) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let name = match resolve_name(&st, want_name) {
            Ok(n) => n,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        let idx = entry_index(&st, &name)?;
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        if st.entries[idx].phase != Phase::Ready {
            return Err(still_creating(&name));
        }
        if let Err(e) = refresh_pages(&self.browser, &mut st.entries[idx]).await {
            if is_engine_dead(&e) {
                let _ = reconcile(&self.browser, &mut st, false).await;
                return Err(ToolErr::from(e));
            }
            return Err(ToolErr::from(e));
        }
        let target_id = pick_page(&st.entries[idx], page)?.to_string();
        let session = match ensure_attached(&self.browser, &mut st.entries[idx], &target_id).await {
            Ok(s) => s,
            Err(e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
        };
        let value = match eval_in(&self.browser, &session, expression).await {
            Ok(v) => v,
            Err(e) => {
                if is_session_death(&e) {
                    if let Some(p) = st.entries[idx]
                        .pages
                        .iter_mut()
                        .find(|p| p.target_id == target_id)
                    {
                        p.session = None;
                    }
                    return Err(err(
                        ToolCode::InvalidTarget,
                        ToolReason::StaleSession,
                        format!(
                            "context: the page session for {target_id} died while evaluating; the expression may or may not have run and is not retried"
                        ),
                    ));
                }
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
        };
        Ok(json!({
            "ok": true,
            "name": name,
            "targetId": target_id,
            "value": value,
            "selectedAffects": SELECT_AFFECTS,
        }))
    }

    /// Context-scoped cookies: read the context's own jar, or set cookies in it
    /// and verify each by identity (name + domain + path) and the attributes
    /// the caller asked for.
    async fn cookies(
        &self,
        want_name: Option<&str>,
        set: Option<&[Value]>,
    ) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let rec = reconcile(&self.browser, &mut st, false).await;
        let name = match resolve_name(&st, want_name) {
            Ok(n) => n,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        let idx = entry_index(&st, &name)?;
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        if st.entries[idx].phase != Phase::Ready {
            return Err(still_creating(&name));
        }
        let Some(context_id) = st.entries[idx].context_id.clone() else {
            return Err(attached_unknown(&name));
        };
        let token = st.entries[idx].token;
        drop(st);
        let before = match live_jar(&self.browser, &context_id).await {
            Some(Ok(jar)) => jar,
            None => return Err(no_live_engine(&self.browser, "reading the jar")),
            Some(Err(e)) if is_unknown_context(&e) => {
                return Err(self.context_vanished(token, &name, &e).await);
            }
            Some(Err(e)) if is_engine_dead(&e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
            Some(Err(e)) => return Err(ToolErr::from(e)),
        };
        let Some(items) = set else {
            return Ok(json!({
                "ok": true,
                "name": name,
                "browserContextId": context_id,
                "cookies": before,
                "set": [],
                "scope": "this browser context only (Storage.getCookies with its browserContextId)",
                "limits": LIMITS,
            }));
        };
        if items.is_empty() {
            return Err(invalid(
                "context: 'cookies' is empty; pass at least one {name, value, url} cookie, or omit it to read",
            ));
        }
        let mut wanted = Vec::with_capacity(items.len());
        for item in items {
            wanted.push(parse_cookie(item)?);
        }
        let specs: Vec<Value> = wanted.iter().map(|c| c.spec.clone()).collect();
        let set_result = if test_hook_skip_cookie_set() {
            Some(Ok(Value::Null))
        } else {
            self.browser
                .cdp_if_live(
                    "Storage.setCookies",
                    json!({"cookies": specs, "browserContextId": context_id}),
                    None,
                )
                .await
        };
        match set_result {
            None => return Err(no_live_engine(&self.browser, "setting cookies")),
            Some(Err(e)) if is_unknown_context(&e) => {
                return Err(self.context_vanished(token, &name, &e).await);
            }
            Some(Err(e)) if is_engine_dead(&e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
            Some(Err(e)) => {
                return Err(err(
                    ToolCode::CookieRejected,
                    ToolReason::Rejected,
                    format!("context: the engine refused Storage.setCookies: {e}"),
                ))
            }
            Some(Ok(_)) => {}
        }
        let after = match live_jar(&self.browser, &context_id).await {
            Some(Ok(jar)) => jar,
            None => return Err(no_live_engine(&self.browser, "verifying cookies")),
            Some(Err(e)) if is_unknown_context(&e) => {
                return Err(self.context_vanished(token, &name, &e).await);
            }
            Some(Err(e)) if is_engine_dead(&e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
            Some(Err(e)) => return Err(ToolErr::from(e)),
        };
        let mut applied = Vec::with_capacity(wanted.len());
        let mut rejected = Vec::new();
        for cookie in &wanted {
            match cookie.verify(&before, &after) {
                Ok(v) => applied.push(v),
                Err(r) => rejected.push(r),
            }
        }
        if !rejected.is_empty() {
            return Err(ToolErr::typed(
                format!(
                    "context: {} cookie(s) could not be verified in the context jar ({}); the jar is authoritative, and a set is never reported from the acknowledgement alone",
                    rejected.len(),
                    rejected
                        .iter()
                        .map(|r| r
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                json!({"code": ToolCode::CookieRejected.as_str(),
                       "reason": ToolReason::SetFailed.as_str(),
                       "tool": TOOL, "applied": applied, "rejected": rejected}),
            ));
        }
        Ok(json!({
            "ok": true,
            "name": name,
            "browserContextId": context_id,
            "cookies": after,
            "set": applied,
            "scope": "this browser context only (Storage.setCookies/getCookies with its browserContextId); verified by identity (name+domain+path) and requested attributes, not by name+value alone",
            "limits": LIMITS,
        }))
    }


    /// The page an emulation acts on. No `name` is the LEGACY default page -
    /// the same page `browser_navigate` and `browser_evaluate` use - so the
    /// manifest can set a clock or locale and read it back through the legacy
    /// tools. A name resolves to that context's tracked page.
    async fn emulation_target(
        &self,
        st: &mut State,
        want_name: Option<&str>,
    ) -> Result<(Option<String>, Option<String>, String), ToolErr> {
        let Some(raw) = want_name else {
            return Ok((None, None, "legacy default page".to_string()));
        };
        let name = normalise_name(raw)?;
        let rec = reconcile(&self.browser, st, false).await;
        let idx = match entry_index(st, &name) {
            Ok(i) => i,
            Err(e) => return Err(reconcile_reason(&self.browser, &rec, e)),
        };
        if st.entries[idx].unverified.is_some() {
            return Err(attached_unknown(&name));
        }
        if st.entries[idx].phase != Phase::Ready {
            return Err(still_creating(&name));
        }
        let context_id = st
            .entries[idx]
            .id()
            .map(str::to_string)
            .ok_or_else(|| attached_unknown(&name))?;
        let target_id = pick_page(&st.entries[idx], None)?.to_string();
        let session = match ensure_attached(&self.browser, &mut st.entries[idx], &target_id).await {
            Ok(s) => s,
            Err(e) => {
                self.reconcile_if_dead(&e).await;
                return Err(ToolErr::from(e));
            }
        };
        Ok((Some(session), Some(context_id), name))
    }

    /// Fixed clock. A preload script overrides `Date` BEFORE the page reloads,
    /// so the page's own script computes the frozen instant; the tool then
    /// removes the script so later documents keep the real clock. The value
    /// returned is read back from the page, not echoed from the request.
    async fn clock(&self, want_name: Option<&str>, instant: Option<&str>) -> Result<Value, ToolErr> {
        let Some(instant) = instant else {
            return Err(invalid(
                "context: clock needs 'iso' (an RFC 3339 instant), or 'clock' with the same value",
            ));
        };
        let mut st = self.state.lock().await;
        let (session, _, label) = self.emulation_target(&mut st, want_name).await?;
        drop(st);
        let parsed = emulation_eval(
            &self.browser,
            session.as_deref(),
            &format!("new Date({}).getTime()", json!(instant)),
        )
        .await
        .map_err(ToolErr::from)?;
        let Some(epoch) = parsed.as_i64() else {
            return Err(invalid(format!(
                "context: clock 'iso' is not a parseable instant: {instant}"
            )));
        };
        let source = format!(
            "(() => {{ const fixed = {epoch}; const RealDate = Date; \
             function HandDate(...a) {{ return a.length ? new RealDate(...a) : new RealDate(fixed); }} \
             HandDate.now = () => fixed; HandDate.parse = RealDate.parse; HandDate.UTC = RealDate.UTC; \
             HandDate.prototype = RealDate.prototype; window.Date = HandDate; }})();"
        );
        emulation_send(&self.browser, session.as_deref(), "Page.enable", json!({}))
            .await
            .map_err(ToolErr::from)?;
        let added = emulation_send(
            &self.browser,
            session.as_deref(),
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": source}),
        )
        .await
        .map_err(ToolErr::from)?;
        let identifier = added
            .get("identifier")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if identifier.is_empty() {
            return Err(err(
                ToolCode::Error,
                ToolReason::Unknown,
                "context: clock: CDP did not return a script identifier, so no clock was installed",
            ));
        }
        emulation_send(&self.browser, session.as_deref(), "Page.reload", json!({}))
            .await
            .map_err(ToolErr::from)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match emulation_eval(&self.browser, session.as_deref(), "document.readyState").await {
                Ok(v) if v.as_str() == Some("complete") => break,
                Ok(_) => {}
                Err(e) => {
                    let _ = emulation_send(
                        &self.browser,
                        session.as_deref(),
                        "Page.removeScriptToEvaluateOnNewDocument",
                        json!({"identifier": identifier}),
                    )
                    .await;
                    return Err(ToolErr::from(e));
                }
            }
            if Instant::now() >= deadline {
                let _ = emulation_send(
                    &self.browser,
                    session.as_deref(),
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({"identifier": identifier}),
                )
                .await;
                return Err(err(
                    ToolCode::Timeout,
                    ToolReason::Unsettled,
                    "context: clock: the page did not finish reloading within 10 s",
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let observed = emulation_eval(&self.browser, session.as_deref(), "Date.now()").await;
        let _ = emulation_send(
            &self.browser,
            session.as_deref(),
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({"identifier": identifier}),
        )
        .await;
        let observed = observed.map_err(ToolErr::from)?;
        if observed.as_i64() != Some(epoch) {
            return Err(err(
                ToolCode::Error,
                ToolReason::Mismatch,
                format!(
                    "context: clock: the page read back {observed} but {epoch} was requested; no claim is made"
                ),
            ));
        }
        Ok(json!({
            "ok": true, "action": "clock", "iso": instant, "epoch": epoch,
            "observed": observed, "target": label,
            "scope": if want_name.is_some() { "this context's page" } else { "the legacy default page" },
            "limits": LIMITS,
        }))
    }

    /// Locale and timezone overrides, read back from the page's own
    /// `Intl.DateTimeFormat().resolvedOptions()`. CDP applies them to the page,
    /// so a later `browser_evaluate` sees the same values.
    async fn locale(
        &self,
        want_name: Option<&str>,
        locale: Option<&str>,
        timezone: Option<&str>,
    ) -> Result<Value, ToolErr> {
        if locale.is_none() && timezone.is_none() {
            return Err(invalid("context: locale needs 'locale' and/or 'timezone'"));
        }
        let mut st = self.state.lock().await;
        let (session, _, label) = self.emulation_target(&mut st, want_name).await?;
        drop(st);
        if let Some(locale) = locale {
            emulation_send(
                &self.browser,
                session.as_deref(),
                "Emulation.setLocaleOverride",
                json!({"locale": locale}),
            )
            .await
            .map_err(ToolErr::from)?;
        }
        if let Some(timezone) = timezone {
            emulation_send(
                &self.browser,
                session.as_deref(),
                "Emulation.setTimezoneOverride",
                json!({"timezoneId": timezone}),
            )
            .await
            .map_err(ToolErr::from)?;
        }
        let observed = emulation_eval(
            &self.browser,
            session.as_deref(),
            "Intl.DateTimeFormat().resolvedOptions().locale + '|' + \
             Intl.DateTimeFormat().resolvedOptions().timeZone",
        )
        .await
        .map_err(ToolErr::from)?;
        let observed = observed.as_str().unwrap_or("").to_string();
        for want in [locale, timezone].into_iter().flatten() {
            if !observed.split('|').any(|part| part == want) {
                return Err(err(
                    ToolCode::Error,
                    ToolReason::Mismatch,
                    format!("context: locale: the page read back {observed:?}, which does not contain {want:?}"),
                ));
            }
        }
        Ok(json!({
            "ok": true, "action": "locale", "observed": observed,
            "locale": locale, "timezone": timezone, "target": label,
            "scope": if want_name.is_some() { "this context's page" } else { "the legacy default page" },
            "limits": LIMITS,
        }))
    }

    /// Programmatic permission grants only. A prompt - anything that would ask
    /// a human through a UI - is a REJECT-class call: it is refused before any
    /// CDP command is sent, with code invalid_args / reason rejected.
    async fn permissions(
        &self,
        want_name: Option<&str>,
        grants: &[String],
        mode: Option<&str>,
        prompt: bool,
    ) -> Result<Value, ToolErr> {
        if prompt || matches!(mode, Some("prompt") | Some("ask")) {
            return Err(err(
                ToolCode::InvalidArgs,
                ToolReason::Rejected,
                "context: permissions: prompts are REJECTED (never a UI prompt); grant programmatically with 'grant' instead",
            ));
        }
        if grants.is_empty() {
            return Err(invalid("context: permissions needs a non-empty 'grant' array"));
        }
        let mut st = self.state.lock().await;
        let (session, context_id, label) = self.emulation_target(&mut st, want_name).await?;
        drop(st);
        let mut params = json!({"permissions": grants});
        if let Some(id) = &context_id {
            params.as_object_mut().expect("object").insert("browserContextId".into(), json!(id));
        }
        self.browser
            .cdp_raw("Browser.grantPermissions", params, None)
            .await
            .map_err(ToolErr::from)?;
        let checks = grants
            .iter()
            .map(|g| {
                format!(
                    "navigator.permissions.query({{name: {}}}).then(p => p.state)",
                    json!(g)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let expr = if checks.len() == 1 {
            checks
        } else {
            format!("Promise.all([{checks}]).then(a => a.join('|'))")
        };
        let observed = emulation_eval(&self.browser, session.as_deref(), &expr)
            .await
            .map_err(ToolErr::from)?;
        let observed = observed.as_str().unwrap_or("").to_string();
        if observed.split('|').any(|state| state != "granted") {
            return Err(err(
                ToolCode::Error,
                ToolReason::Mismatch,
                format!("context: permissions: the page read back {observed:?}, not all 'granted'"),
            ));
        }
        Ok(json!({
            "ok": true, "action": "permissions", "granted": grants,
            "observed": observed, "target": label,
            "browserContextId": context_id,
            "scope": "Browser.grantPermissions for this context only; no prompt UI",
            "limits": LIMITS,
        }))
    }

    /// Test-data reset: clear cookies through CDP with the context's
    /// browserContextId, clear local/session storage and IndexedDB on the
    /// context's own page, then read the emptied state back from the document.
    async fn reset(&self, want_name: Option<&str>) -> Result<Value, ToolErr> {
        let mut st = self.state.lock().await;
        let (session, context_id, label) = self.emulation_target(&mut st, want_name).await?;
        drop(st);
        let origin = emulation_eval(&self.browser, session.as_deref(), "location.origin")
            .await
            .map_err(ToolErr::from)?;
        let origin = origin.as_str().unwrap_or("").to_string();
        if origin.is_empty() || origin == "null" {
            return Err(err(
                ToolCode::InvalidTarget,
                ToolReason::OpaqueOrigin,
                "context: reset: the page has no usable origin to clear",
            ));
        }
        let mut params = json!({});
        if let Some(id) = &context_id {
            params.as_object_mut().expect("object").insert("browserContextId".into(), json!(id));
        }
        self.browser
            .cdp_raw("Storage.clearCookies", params, None)
            .await
            .map_err(ToolErr::from)?;
        let clear = "localStorage.clear(); sessionStorage.clear(); \
                     indexedDB.databases \
                       ? indexedDB.databases().then(dbs => Promise.all(dbs.map(d => new Promise(r => { \
                           const q = indexedDB.deleteDatabase(d.name); \
                           q.onsuccess = r; q.onerror = r; q.onblocked = r; \
                         })))).then(() => 1) \
                       : 1";
        emulation_eval(&self.browser, session.as_deref(), clear)
            .await
            .map_err(ToolErr::from)?;
        let observed = emulation_eval(
            &self.browser,
            session.as_deref(),
            "localStorage.length + '|' + sessionStorage.length + '|' + document.cookie",
        )
        .await
        .map_err(ToolErr::from)?;
        let observed = observed.as_str().unwrap_or("").to_string();
        if !observed.starts_with("0|0|") {
            return Err(err(
                ToolCode::Error,
                ToolReason::Mismatch,
                format!("context: reset: storage still present after clearing: {observed:?}"),
            ));
        }
        Ok(json!({
            "ok": true, "action": "reset", "origin": origin,
            "observed": observed, "target": label,
            "browserContextId": context_id,
            "scope": "this browser context only",
            "limits": LIMITS,
        }))
    }

    /// Write one context's cookie jar and localStorage as one JSON artifact at
    /// a caller-supplied ABSOLUTE path. Programmatic CDP and page reads only:
    /// no UI, no clipboard, no implicit destination.
    async fn export_state(&self, want_name: Option<&str>, path: &str) -> Result<Value, ToolErr> {
        let target = PathBuf::from(path);
        if !target.is_absolute() {
            return Err(invalid(format!(
                "context: export_state 'path' must be absolute, got '{path}'"
            )));
        }
        let mut st = self.state.lock().await;
        let (session, context_id, label) = self.emulation_target(&mut st, want_name).await?;
        drop(st);
        let Some(context_id) = context_id else {
            return Err(invalid("context: export_state needs a named context"));
        };
        let jar = match live_jar(&self.browser, &context_id).await {
            Some(Ok(j)) => j,
            None => return Err(no_live_engine(&self.browser, "exporting cookies")),
            Some(Err(e)) => return Err(ToolErr::from(e)),
        };
        let origin = emulation_eval(&self.browser, session.as_deref(), "location.origin")
            .await
            .map_err(ToolErr::from)?;
        let origin = origin.as_str().unwrap_or("").to_string();
        let listed = emulation_eval(
            &self.browser,
            session.as_deref(),
            "JSON.stringify(Object.entries(localStorage))",
        )
        .await
        .map_err(ToolErr::from)?;
        let entries: Vec<Value> = serde_json::from_str(listed.as_str().unwrap_or("[]"))
            .map_err(|e| invalid(format!("context: export_state cannot read localStorage: {e}")))?;
        let artifact = json!({
            "version": 1, "tool": "browser_context", "kind": "state",
            "origin": origin, "cookies": jar, "localStorage": entries,
        });
        let bytes = serde_json::to_string_pretty(&artifact).map_err(|e| {
            err(
                ToolCode::Error,
                ToolReason::Unreadable,
                format!("context: export_state cannot serialize state: {e}"),
            )
        })?;
        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(err(
                    ToolCode::InvalidArgs,
                    ToolReason::Unreadable,
                    format!("context: export_state cannot create '{}': {e}", parent.display()),
                ));
            }
        }
        if let Err(e) = std::fs::write(&target, bytes.as_bytes()) {
            return Err(err(
                ToolCode::InvalidArgs,
                ToolReason::Unreadable,
                format!("context: export_state cannot write '{path}': {e}"),
            ));
        }
        Ok(json!({
            "ok": true, "action": "export_state", "path": path, "name": label,
            "browserContextId": context_id, "origin": origin,
            "cookies": jar.len(), "localStorage": entries.len(),
            "scope": "one JSON file; programmatic CDP and page reads, no UI or clipboard",
            "limits": LIMITS,
        }))
    }

    /// Import a previously exported artifact into a FRESH named context: the
    /// jar must still be empty before import. Cookies are set with CDP under
    /// that context's browserContextId and localStorage on its own page. Every
    /// imported item is then read back from the target before success.
    async fn import_state(&self, want_name: Option<&str>, path: &str) -> Result<Value, ToolErr> {
        let target = PathBuf::from(path);
        if !target.is_absolute() {
            return Err(invalid(format!(
                "context: import_state 'path' must be absolute, got '{path}'"
            )));
        }
        let Some(raw) = want_name else {
            return Err(invalid("context: import_state needs a named fresh context"));
        };
        let text = std::fs::read_to_string(&target).map_err(|e| {
            err(
                ToolCode::FileNotFound,
                ToolReason::Unreadable,
                format!("context: import_state cannot read '{path}': {e}"),
            )
        })?;
        let artifact: Value = serde_json::from_str(&text)
            .map_err(|e| invalid(format!("context: import_state bad JSON: {e}")))?;
        if artifact["version"] != json!(1) || artifact["kind"] != json!("state") {
            return Err(invalid(
                "context: import_state needs an export_state artifact (version 1, kind state)",
            ));
        }
        let cookies = artifact["cookies"]
            .as_array()
            .ok_or_else(|| invalid("context: import_state artifact has no cookies array"))?;
        let entries = artifact["localStorage"]
            .as_array()
            .ok_or_else(|| invalid("context: import_state artifact has no localStorage array"))?;
        let mut st = self.state.lock().await;
        let (session, context_id, label) = self.emulation_target(&mut st, Some(raw)).await?;
        drop(st);
        let Some(context_id) = context_id else {
            return Err(invalid("context: import_state needs a named context"));
        };
        let before = match live_jar(&self.browser, &context_id).await {
            Some(Ok(j)) => j,
            None => return Err(no_live_engine(&self.browser, "checking the target jar")),
            Some(Err(e)) => return Err(ToolErr::from(e)),
        };
        if !before.is_empty() {
            return Err(err(
                ToolCode::InvalidArgs,
                ToolReason::BadValue,
                "context: import_state needs a FRESH context; the target jar is not empty",
            ));
        }
        let occupied = emulation_eval(&self.browser, session.as_deref(), "localStorage.length")
            .await
            .map_err(ToolErr::from)?;
        if occupied.as_u64().unwrap_or(0) != 0 {
            return Err(err(
                ToolCode::InvalidArgs,
                ToolReason::BadValue,
                "context: import_state needs a FRESH context; the target localStorage is not empty",
            ));
        }
        if !cookies.is_empty() {
            let specs: Vec<Value> = cookies
                .iter()
                .map(|c| {
                    let mut spec = json!({
                        "name": c.get("name").cloned().unwrap_or(Value::Null),
                        "value": c.get("value").cloned().unwrap_or(Value::Null),
                        "domain": c.get("domain").cloned().unwrap_or(Value::Null),
                        "path": c.get("path").cloned().unwrap_or(Value::Null),
                    });
                    let obj = spec.as_object_mut().expect("object");
                    for key in ["secure", "httpOnly", "sameSite"] {
                        if let Some(v) = c.get(key) {
                            obj.insert(key.to_string(), v.clone());
                        }
                    }
                    spec
                })
                .collect();
            let result = self
                .browser
                .cdp_if_live(
                    "Storage.setCookies",
                    json!({"cookies": specs, "browserContextId": context_id}),
                    None,
                )
                .await;
            match result {
                None => return Err(no_live_engine(&self.browser, "importing cookies")),
                Some(Err(e)) => return Err(ToolErr::from(e)),
                Some(Ok(_)) => {}
            }
        }
        if !entries.is_empty() {
            let expr = format!(
                "(function(items){{ items.forEach(function(p){{ localStorage.setItem(p[0], p[1]); }}); return items.length; }})({})",
                json!(entries)
            );
            emulation_eval(&self.browser, session.as_deref(), &expr)
                .await
                .map_err(ToolErr::from)?;
        }
        let after = match live_jar(&self.browser, &context_id).await {
            Some(Ok(j)) => j,
            None => return Err(no_live_engine(&self.browser, "verifying imported cookies")),
            Some(Err(e)) => return Err(ToolErr::from(e)),
        };
        let listed = emulation_eval(
            &self.browser,
            session.as_deref(),
            "JSON.stringify(Object.entries(localStorage))",
        )
        .await
        .map_err(ToolErr::from)?;
        let after_entries: Vec<Value> = serde_json::from_str(listed.as_str().unwrap_or("[]"))
            .map_err(|e| invalid(format!("context: import_state cannot read localStorage: {e}")))?;
        for c in cookies {
            let (name, value, domain, path) = (
                c["name"].as_str().unwrap_or(""),
                c["value"].as_str().unwrap_or(""),
                c["domain"].as_str().unwrap_or(""),
                c["path"].as_str().unwrap_or(""),
            );
            if !after.iter().any(|a| {
                a["name"].as_str() == Some(name)
                    && a["value"].as_str() == Some(value)
                    && a["domain"].as_str() == Some(domain)
                    && a["path"].as_str() == Some(path)
            }) {
                return Err(err(
                    ToolCode::Error,
                    ToolReason::Mismatch,
                    format!("context: import_state: cookie {name} did not round-trip"),
                ));
            }
        }
        for pair in entries {
            if !after_entries.iter().any(|a| a == pair) {
                return Err(err(
                    ToolCode::Error,
                    ToolReason::Mismatch,
                    format!("context: import_state: a localStorage item did not round-trip: {pair}"),
                ));
            }
        }
        Ok(json!({
            "ok": true, "action": "import_state", "path": path, "name": label,
            "browserContextId": context_id,
            "cookies": after.len(), "localStorage": after_entries.len(),
            "verifiedCookies": cookies.len(), "verifiedLocalStorage": entries.len(),
            "scope": "target context only; programmatic CDP and page writes, no UI or clipboard",
            "limits": LIMITS,
        }))
    }

    /// The engine itself said this context does not exist. Retire THE acquisition
    /// that saw it (by token, never by name: a destroy + re-create of the same
    /// name must keep its successor and its successor's exclusion), release that
    /// entry's exclusion, and report the cause - never as an engine death.
    async fn context_vanished(&self, token: u64, name: &str, cause: &str) -> ToolErr {
        if let Some(ms) = hook_ms("HAND_CONTEXT_TEST_SLOW_BEFORE_VANISH_MS") {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let mut st = self.state.lock().await;
        let retired = match st.entries.iter().position(|e| e.token == token) {
            Some(idx) => {
                let entry = st.entries.remove(idx);
                if let Some(id) = entry.id() {
                    self.browser.include_browser_context(id);
                }
                if st.selected.as_deref() == Some(entry.name.as_str()) {
                    st.selected = None;
                }
                true
            }
            None => false,
        };
        let consequence = if retired {
            "it was removed outside hand, so it is dropped from tracking and its legacy exclusion is released"
        } else {
            "it was removed outside hand; the entry now tracked under that name is a later acquisition, so this failure retires nothing"
        };
        err(
            ToolCode::InvalidTarget,
            ToolReason::NotFound,
            format!(
                "context: '{name}' is no longer in the engine ({}) - {consequence}",
                plain(cause)
            ),
        )
    }

    async fn remove_entry_by_token(&self, token: u64) {
        let mut st = self.state.lock().await;
        let name = st
            .entries
            .iter()
            .find(|e| e.token == token)
            .map(|e| e.name.clone());
        if let Some(entry) = st.entries.iter().find(|e| e.token == token) {
            if let Some(id) = entry.id() {
                let id = id.to_string();
                // Row B3: a route lives on this context and dies with it. The
                // removal is verified state work, not a claim about the engine.
                crate::net::on_context_gone(&self.browser, &id).await;
                self.browser.include_browser_context(&id);
            }
        }
        st.entries.retain(|e| e.token != token);
        if let (Some(name), Some(sel)) = (name, st.selected.clone()) {
            if sel == name {
                st.selected = None;
            }
        }
    }

    async fn note_failure(&self, token: u64, why: &str) {
        let mut st = self.state.lock().await;
        if let Some(e) = st.entry_by_token_mut(token) {
            e.acquiring = false;
            e.last_error = Some(why.to_string());
        }
    }

    /// Keep the entry, record the cause. Never clears `acquiring` (a task may
    /// still own the entry); used by teardown paths that failed to dispose.
    async fn record_error(&self, token: u64, why: &str) {
        let mut st = self.state.lock().await;
        if let Some(e) = st.entry_by_token_mut(token) {
            e.last_error = Some(why.to_string());
        }
    }

    /// Keep the entry and say its fate is UNKNOWN (attached connection loss).
    async fn mark_unverified(&self, token: u64, note: &str) {
        let mut st = self.state.lock().await;
        if let Some(e) = st.entry_by_token_mut(token) {
            e.unverified = Some(note.to_string());
        }
    }

    /// An engine-death error means the connection is gone: reconcile (ownership
    /// decides whether the contexts are gone or merely unverified).
    async fn reconcile_if_dead(&self, e: &str) {
        if !is_engine_dead(e) {
            return;
        }
        let mut st = self.state.lock().await;
        let _ = reconcile(&self.browser, &mut st, false).await;
    }
}

impl std::fmt::Debug for ContextRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextRegistry").finish_non_exhaustive()
    }
}


const SELECT_AFFECTS: &str =
    "browser_context actions that omit `name` only; legacy browser_* tools keep acting on the \
     legacy default page";

const LIMITS: [&str; 14] = [
    "separate cookies/localStorage/sessionStorage per context in ONE browser process: storage partitioning, not a crash-isolation boundary",
    "legacy browser_* tools (snapshot/click/type/evaluate/storage/tabs/...) act on the legacy default page and never inside a context; realm-scoped evaluate and tabs select refuse a context page by name, while the raw cdp tool remains the advanced escape hatch",
    "a page's url and target membership come from CDP target info; the title of a created or opened page is a live document.title read when one answered (source: targetInfo+liveRead) and otherwise the engine's cached value (source: targetInfo, with titleNote saying why); `eval` is the live-evidence route, and a page's `session` field is hand bookkeeping, not a live probe",
    "a JS dialog opened by a context page is not auto-handled (Page events are not enabled on context sessions); dispose the context to recover",
    "pages are discovered from Target.getTargets, so a page the site opened appears after the next list/show",
    "frame-scoped refs are legacy-surface only and do not exist for context pages",
    "destroy closes the context's pages abruptly: `pagehide` fires but never `beforeunload` or `unload` (measured on Chrome 158; pagehide cannot be suppressed from CDP); use close for an orderly page close",
    "in an attached engine (GLADIUS_CDP_URL) only contexts hand created are touched: a clean detach (Browser::close sends a WebSocket close frame) disposes them, but a hard kill of hand never runs that path - an abrupt socket drop was measured to close the context pages while leaving the context object answering Storage.getCookies, so an external browser may retain it until that engine exits; a lost attached connection is reported UNVERIFIED (kept tracked and excluded, no disposal claimed) and list/show re-establish the configured connection to reconcile; a hard kill is never reported as a disposal",
    "an acquisition is matched by its own token, never by name: destroying and re-creating a name cannot let a stale acquisition write into, or roll back, its successor; an id-less creation can always be destroyed (nothing is named) and its task disposes whatever it later creates",
    "create/open are committed by their own task: a cancelled call leaves tracked state behind (phase 'creating' when it cannot finish) instead of an invisible context, and shutdown reports what it could not dispose",
    "a set cookie is verified by name+domain+path plus the requested secure/httpOnly/sameSite attributes; expiry is echoed but not asserted, and an identical cookie that already existed is reported preexisting",
    "clock/locale apply to one page: a preload script fixes Date before a reload and is removed once read back, while locale/timezone are CDP page overrides only; neither is a device or a real clock, and a different page needs its own call",
    "permissions are programmatic Browser.grantPermissions only; a UI prompt is refused with code invalid_args / reason rejected, and grants are scoped by browserContextId with the page's own navigator.permissions state read back as proof",
    "state export/import are programmatic CDP plus page reads to one caller-supplied absolute JSON file; import requires a fresh context and verifies every cookie and localStorage item by reading it back, with no UI or clipboard",
];

/// The `context` tool entry in the exact shape `tools::list()` uses. hand's
/// catalog prefixes the name, so a client sees `browser_context`.
pub fn tool_schema() -> Value {
    json!({
        "name": TOOL,
        "title": "Browser context",
        "description": "Named isolated browser contexts with separate cookies and storage, explicit \
    create/selection/teardown. One context is one CDP browser context (its own cookie jar, \
    localStorage/sessionStorage/IndexedDB/cache partition) inside hand's one engine. \
    action: create (name, optional url; creates the context and its first page, sets it as the \
    registry default; the acquisition is committed by its own task, so a cancelled call cannot \
    orphan it) | list (refreshes every tracked context from Target.getTargets; contexts lost with \
    a dead engine are named in dropped) | show (one context, name optional: the registry default \
    or the only one) | select (name; sets the default for THIS tool's actions only - legacy \
    browser_* tools keep acting on the legacy default page, and realm-scoped evaluate refuses a \
    context page by name) | open (add a page; optional url; the page is tracked from the moment \
    it exists) | close (page=targetId; the context and its other pages stay) | destroy (name; \
    disposes the context and PROVES it is gone: no page target carries its browserContextId and \
    Storage.getCookies refuses the id; disposedBy says hand|already-gone, a dead engine reports \
    went-with-engine, and a failed disposal keeps the context tracked and excluded from legacy \
    tabs; on destroy the pages it closes fire `pagehide` but never `beforeunload` or `unload` \
    (measured on Chrome 158; pagehide cannot be suppressed from CDP) - so use close when a page \
    must run its own teardown handlers) | destroy_all | eval (expression in the \
    context's page; typed js_error on a throw) | cookies (read this context's jar, or set cookies \
    and verify each by name+domain+path identity plus the requested attributes, with preexisting \
    reported; a cookie needs name+value and either url or domain, or the engine cannot tell which \
    origin it belongs to and the tool refuses it) | clock (iso=RFC 3339 instant; fixes the page's \
    Date before a reload, reads the frozen instant back from the page and removes the preload script; \
    with name it targets that context's page, without it the legacy default page) | locale (locale \
    and/or timezone; CDP page overrides read back from Intl.DateTimeFormat().resolvedOptions()) | \
    permissions (name + grant array; programmatic Browser.grantPermissions scoped to the context, a UI \
    prompt REJECTED as invalid_args/rejected, and the page's own navigator.permissions state as proof) \
    | reset (clears cookies by browserContextId and local/session storage plus IndexedDB on the \
    page, then reads the emptied state back) | export_state (name + absolute path; one JSON file \
    of this context's cookies and localStorage) | import_state (name + path; a FRESH context only, \
    every item read back before success; no UI or clipboard). Limits: one browser process (partitioning, not \
    crash isolation); a dialog in a \
    context page is not auto-handled; a page's url and target membership are target-info values, \
    while a created/opened page's title is a live document.title read labelled source \
    targetInfo+liveRead (targetInfo when the engine's cached value had to stand in, with titleNote \
    saying why); frame refs do not exist for context pages.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["create", "list", "show", "select", "open", "close", "destroy", "destroy_all", "eval", "cookies", "clock", "locale", "permissions", "reset", "export_state", "import_state"]},
            "name": {"type": "string", "maxLength": MAX_NAME_LEN},
            "url": {"type": "string", "format": "uri-reference"},
            "page": {"type": "string"},
            "expression": {"type": "string"},
            "cookies": {"type": "array", "items": {"type": "object",
                "properties": {
                    "name": {"type": "string"}, "value": {"type": "string"},
                    "url": {"type": "string"}, "domain": {"type": "string"},
                    "path": {"type": "string"}, "secure": {"type": "boolean"},
                    "httpOnly": {"type": "boolean"},
                    "sameSite": {"type": "string", "enum": ["Strict", "Lax", "None"]},
                    "expires": {"type": "number"}},
                "required": ["name", "value"],
                "anyOf": [{"required": ["url"]}, {"required": ["domain"]}],
                "additionalProperties": false}},
            "iso": {"type": "string"},
            "clock": {"type": "string"},
            "locale": {"type": "string"},
            "timezone": {"type": "string"},
            "permissions": {"type": "array", "items": {"type": "string"}},
            "grant": {"type": "array", "items": {"type": "string"}},
            "mode": {"type": "string"},
            "prompt": {"type": "boolean"},
            "path": {"type": "string"}
            },
            "required": ["action"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "name": {"type": "string"},
            "browserContextId": {},
            "phase": {"type": "string"},
            "context": {"type": "object"},
            "contexts": {"type": "array"},
            "otherContexts": {"type": "array"},
            "pageCount": {"type": "integer", "minimum": 0},
            "createdAt": {"type": "integer", "minimum": 0},
            "page": {},
            "pages": {"type": "array"},
            "targetId": {"type": "string"},
            "value": {},
            "cookies": {"type": "array"},
            "set": {"type": "array"},
            "selected": {},
            "selectedAffects": {"type": "string"},
            "legacyUnaffected": {"type": "boolean"},
            "engine": {"type": "string"},
            "url": {"type": "string"},
            "title": {"type": "string"},
            "source": {"type": "string"},
            "titleNote": {"type": "string"},
            "settled": {"type": "boolean"},
            "readyState": {"type": "string"},
            "gone": {"type": "boolean"},
            "disposedBy": {"type": "string"},
            "disposedPages": {"type": "integer", "minimum": 0},
            "destroyed": {"type": "array"},
            "goneWithEngine": {"type": "array"},
            "unverified": {"type": "array"},
            "dropped": {"type": "array"},
            "pending": {"type": "array"},
            "failed": {"type": "array"},
            "remaining": {"type": "array"},
            "ownership": {"type": "string"},
            "evidence": {"type": "object"},
            "lastError": {"type": "string"},
            "note": {"type": "string"},
            "scope": {"type": "string"},
            "observed": {},
            "epoch": {"type": "integer"},
            "granted": {"type": "array"},
            "path": {"type": "string"},
            "localStorage": {"type": "array"},
            "verifiedCookies": {"type": "integer", "minimum": 0},
            "verifiedLocalStorage": {"type": "integer", "minimum": 0},
            "limits": {"type": "array"}},
            "required": ["ok"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
    })
}

/// True for the legacy name this module dispatches. hand prefixes it, so the
/// server's browser domain compares the unprefixed name.
pub fn is_context_tool(name: &str) -> bool {
    name == TOOL
}

/// Server integration point: `Some(envelope)` for the context tool, `None` for
/// every other name so the caller falls through to `browser_engine::call`.
pub async fn dispatch(name: &str, args: &Value, registry: &Arc<ContextRegistry>) -> Option<Value> {
    if !is_context_tool(name) {
        return None;
    }
    Some(registry.call(args).await)
}

/// Initialization/discovery block for this capability. Additive: `server_info`
/// can merge it under `engines.browser` without touching any legacy field.
pub fn capability_info() -> Value {
    json!({
        "isolatedContexts": {
            "tool": "browser_context",
            "available": true,
            "isolation": {
                "verified": ["cookies", "localStorage", "sessionStorage"],
                "expected": ["indexedDB", "httpCache"],
                "process": "one browser process (partitioning only; not a crash boundary)"
            },
            "selection": SELECT_AFFECTS,
            "legacyReach": "tabs, tabs select and realm-scoped evaluate refuse a context page by name; the raw cdp tool is the documented escape hatch",
            "teardown": "Target.disposeBrowserContext plus a verified gone-check; on destroy the pages fire `pagehide` but never `beforeunload` or `unload` (measured on Chrome 158; pagehide cannot be suppressed from CDP), and a clean detach (Browser::close sends a WebSocket close frame) disposes hand-created contexts while another client's contexts stay untouched",
            "hardKillResidual": "a hard kill of hand sends no close frame, so disposeOnDetach cannot run: measured on the pinned build, an abrupt socket drop closed the context's pages but left the context object answering Storage.getCookies, so an attached engine may retain it until that engine exits; hand never reports a hard kill as a disposal and a hard kill is not a cleanup path",
            "cancellation": "create/open are committed by their own task; tracked state (phase 'creating') survives a cancelled call, and a lost owned engine is reported as no longer reachable instead of a fabricated disposal",
            "cookieVerification": "name+domain+path identity plus requested secure/httpOnly/sameSite; preexisting reported; expiry not asserted",
            "stateTransfer": {
                "export": "action export_state with name + absolute path: one JSON artifact with the context's cookies (Storage.getCookies by browserContextId) and localStorage (page read), no UI or clipboard",
                "import": "action import_state with a fresh named context + path: Storage.setCookies under that browserContextId and localStorage on its page, each item read back before success"
            },
            "determinism": {
                "clock": "action clock with iso=RFC 3339 instant: a preload script fixes Date before a reload, the page's own Date.now() is read back, and the script is removed; no name targets the legacy default page, a name targets that context's page",
                "locale": "action locale with locale and/or timezone: Emulation.setLocaleOverride / setTimezoneOverride on one page, verified from Intl.DateTimeFormat().resolvedOptions()",
                "permissions": "action permissions with name + grant: programmatic Browser.grantPermissions scoped by browserContextId, verified from the page's navigator.permissions state; a UI prompt is REJECTED (invalid_args/rejected)",
                "reset": "action reset: Storage.clearCookies scoped by browserContextId plus localStorage/sessionStorage.clear() and indexedDB.deleteDatabase on the context's page, verified from the emptied document state"
            },
            "disposeOnDetach": true,
            "limits": LIMITS,
        }
    })
}


#[derive(Clone, Debug)]
struct PageInfo {
    url: String,
    title: String,
}

/// A cookie the caller asked to set, normalised once so identity and attribute
/// verification use exactly what was sent.
#[derive(Clone, Debug)]
struct ParsedCookie {
    spec: Value,
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    url: Option<String>,
    secure: Option<bool>,
    http_only: Option<bool>,
    same_site: Option<String>,
}

impl ParsedCookie {
    fn expected_domain(&self) -> Option<String> {
        match &self.domain {
            Some(d) => Some(d.clone()),
            None => self
                .url
                .as_deref()
                .and_then(url_host)
                .map(|h| h.to_lowercase()),
        }
    }

    fn expected_path(&self) -> String {
        match &self.path {
            Some(p) if !p.is_empty() => p.clone(),
            _ => "/".to_string(),
        }
    }

    /// Verify against the jar. `Ok` is the verified record; `Err` is the
    /// machine-readable refusal naming what the jar actually holds.
    fn verify(&self, before: &[Value], after: &[Value]) -> Result<Value, Value> {
        let domain = self.expected_domain();
        let path = self.expected_path();
        let mut same_name: Vec<Value> = after
            .iter()
            .filter(|c| c["name"].as_str() == Some(self.name.as_str()))
            .map(|c| {
                json!({"domain": c["domain"], "path": c["path"], "value": c["value"],
                       "httpOnly": c["httpOnly"], "secure": c["secure"]})
            })
            .collect();
        same_name.truncate(8);
        let identity = json!({"name": self.name, "domain": domain, "path": path,
                              "derivedPath": self.path.is_none()});
        let matches_identity = |c: &&Value| {
            cookie_identity(c) == (self.name.clone(), domain.clone(), Some(path.clone()))
        };
        let Some(found) = after
            .iter()
            .filter(matches_identity)
            .find(|c| c["value"].as_str() == Some(self.value.as_str()))
        else {
            return Err(json!({
                "name": self.name,
                "verified": false,
                "reason": if same_name.is_empty() { "not-in-jar" } else { "no-cookie-with-this-identity-and-value" },
                "expected": identity,
                "observedSameName": same_name,
            }));
        };
        for (attr, want) in [("secure", self.secure), ("httpOnly", self.http_only)] {
            if let Some(w) = want {
                if found[attr].as_bool() != Some(w) {
                    return Err(json!({
                        "name": self.name,
                        "verified": false,
                        "reason": "attribute-mismatch",
                        "attribute": attr,
                        "expected": w,
                        "observed": found[attr],
                        "identity": identity,
                    }));
                }
            }
        }
        if let Some(want_ss) = &self.same_site {
            let observed = found["sameSite"].as_str().unwrap_or("");
            if !observed.eq_ignore_ascii_case(want_ss) {
                return Err(json!({
                    "name": self.name,
                    "verified": false,
                    "reason": "attribute-mismatch",
                    "attribute": "sameSite",
                    "expected": want_ss,
                    "observed": observed,
                    "identity": identity,
                }));
            }
        }
        let preexisting = before
            .iter()
            .filter(matches_identity)
            .any(|c| c["value"].as_str() == Some(self.value.as_str()));
        Ok(json!({
            "name": self.name,
            "verified": true,
            "preexisting": preexisting,
            "identity": identity,
            "value": found["value"],
            "attributes": {
                "secure": found["secure"],
                "httpOnly": found["httpOnly"],
                "sameSite": found["sameSite"],
                "expires": found["expires"],
            },
        }))
    }
}

fn url_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_string)
}

/// `(name, domain, path)` with the domain normalised (lowercase, no leading
/// dot) exactly as the jar echoes it.
fn cookie_identity(c: &Value) -> (String, Option<String>, Option<String>) {
    (
        c["name"].as_str().unwrap_or("").to_string(),
        c["domain"]
            .as_str()
            .map(|d| d.trim_start_matches('.').to_lowercase()),
        c["path"].as_str().map(str::to_string),
    )
}

fn parse_cookie(item: &Value) -> Result<ParsedCookie, ToolErr> {
    let obj = item.as_object().ok_or_else(|| {
        err(
            ToolCode::InvalidArgs,
            ToolReason::BadValue,
            "context: every entry of 'cookies' must be an object",
        )
    })?;
    const ALLOWED: [&str; 9] = [
        "name", "value", "url", "domain", "path", "secure", "httpOnly", "sameSite", "expires",
    ];
    for k in obj.keys() {
        if !ALLOWED.contains(&k.as_str()) {
            return Err(err(
                ToolCode::InvalidArgs,
                ToolReason::UnknownArg,
                format!("context: unknown cookie field '{k}' (not in this schema; rejected, not ignored)"),
            ));
        }
    }
    let name = obj.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
        err(
            ToolCode::InvalidArgs,
            ToolReason::MissingArg,
            "context: every cookie needs a string 'name'",
        )
    })?;
    if name.trim().is_empty() {
        return Err(invalid("context: a cookie 'name' must not be empty"));
    }
    let value = obj.get("value").and_then(|v| v.as_str()).ok_or_else(|| {
        err(
            ToolCode::InvalidArgs,
            ToolReason::MissingArg,
            "context: every cookie needs a string 'value'",
        )
    })?;
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let domain = obj
        .get("domain")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_start_matches('.').to_lowercase())
        .filter(|s| !s.is_empty());
    if url.is_none() && domain.is_none() {
        return Err(err(
            ToolCode::InvalidArgs,
            ToolReason::MissingArg,
            "context: every cookie needs 'url' (e.g. http://127.0.0.1:8080/) or 'domain' + 'path': without one the engine cannot tell which origin the cookie belongs to",
        ));
    }
    let mut spec = serde_json::Map::new();
    spec.insert("name".into(), json!(name));
    spec.insert("value".into(), json!(value));
    for (k, v) in obj {
        if k == "name" || k == "value" {
            continue;
        }
        spec.insert(k.clone(), v.clone());
    }
    if !spec.contains_key("path") && url.is_none() {
        spec.insert("path".into(), json!("/"));
    }
    Ok(ParsedCookie {
        spec: Value::Object(spec),
        name: name.to_string(),
        value: value.to_string(),
        domain,
        path: obj.get("path").and_then(|v| v.as_str()).map(str::to_string),
        url,
        secure: obj.get("secure").and_then(|v| v.as_bool()),
        http_only: obj.get("httpOnly").and_then(|v| v.as_bool()),
        same_site: obj
            .get("sameSite")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn engine_label() -> &'static str {
    if crate::browser::cdp_url_override().is_some() {
        "external-cdp (attached; contexts are hand's, the engine is not)"
    } else {
        "chromium (owned)"
    }
}

fn normalise_name(raw: &str) -> Result<String, ToolErr> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(err(
            ToolCode::InvalidArgs,
            ToolReason::BadValue,
            "context: 'name' must be a non-empty string",
        ));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(err(
            ToolCode::InvalidArgs,
            ToolReason::BadValue,
            format!("context: 'name' is longer than {MAX_NAME_LEN} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(err(
            ToolCode::InvalidArgs,
            ToolReason::BadValue,
            "context: 'name' must not contain control characters",
        ));
    }
    Ok(name.to_string())
}

fn normalise_url(url: Option<&str>) -> Result<String, ToolErr> {
    match url {
        None => Ok("about:blank".to_string()),
        Some(u) if u.trim().is_empty() => Err(err(
            ToolCode::InvalidArgs,
            ToolReason::BadValue,
            "context: 'url' must be a non-empty string when present",
        )),
        Some(u) => Ok(u.trim().to_string()),
    }
}

/// The registry default: the explicitly selected name, else the only tracked
/// context. Two or more contexts with no selection is genuinely ambiguous, so
/// it is refused with the candidates named rather than guessed.
fn resolve_name(st: &State, want: Option<&str>) -> Result<String, ToolErr> {
    if let Some(raw) = want {
        let name = normalise_name(raw)?;
        entry_index(st, &name)?;
        return Ok(name);
    }
    if let Some(sel) = st.selected.clone() {
        return Ok(sel);
    }
    match st.entries.as_slice() {
        [] => Err(err(
            ToolCode::InvalidTarget,
            ToolReason::NotFound,
            "context: no context exists yet; create one first",
        )),
        [only] => Ok(only.name.clone()),
        many => Err(err(
            ToolCode::InvalidArgs,
            ToolReason::MissingArg,
            format!(
                "context: 'name' is required when several contexts exist and none is selected; tracked: {:?}",
                many.iter().map(|e| e.name.as_str()).collect::<Vec<_>>()
            ),
        )),
    }
}

fn entry_index(st: &State, name: &str) -> Result<usize, ToolErr> {
    st.entries
        .iter()
        .position(|e| e.name == name)
        .ok_or_else(|| {
            err(
                ToolCode::InvalidTarget,
                ToolReason::NotFound,
                format!(
                    "context: no context named '{name}' (tracked: {:?})",
                    st.entries
                        .iter()
                        .map(|e| e.name.as_str())
                        .collect::<Vec<_>>()
                ),
            )
        })
}

/// What one reconciliation pass observed.
#[derive(Default)]
struct Reconciled {
    /// Entries that cannot belong to the current engine and are no longer
    /// tracked: owned-engine death, an owned relaunch, an external disappearance
    /// proven by a probe, or an abandoned id-less acquisition.
    dropped: Vec<String>,
    /// Entries whose fate is UNKNOWN, still tracked: hand's connection to an
    /// ATTACHED engine is gone, or a probe could not answer. Never reported as
    /// gone and never released from the legacy exclusion.
    unverified: Vec<String>,
}

/// How a tracked context id answers on the current connection.
enum Probe {
    Alive,
    Gone,
    Unknown(String),
}

/// Probe one context id through a live connection. `Storage.getCookies` is the
/// cheapest call that distinguishes "the context exists" from "the engine says
/// there is no such context".
async fn probe_context(browser: &Browser, context_id: &str) -> Probe {
    match live_cdp(
        browser,
        "Storage.getCookies",
        json!({"browserContextId": context_id}),
        None,
    )
    .await
    {
        Ok(None) => Probe::Unknown("no live connection".to_string()),
        Ok(Some(_)) => Probe::Alive,
        Err(e) if is_unknown_context(&e) => Probe::Gone,
        Err(e) => Probe::Unknown(plain(&e)),
    }
}

/// Fate of one tracked entry in a reconciliation pass. Pure, so the whole
/// ownership matrix is unit-testable without a live engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryFate {
    /// Belongs to the current connection: keep as is.
    Live,
    /// Attached engine, live again after a re-attach: probe to decide.
    ProbeOnNewConnection,
    /// Cannot belong to the current engine: drop and release the exclusion.
    Gone,
    /// Fate unknown (attached connection lost): keep tracked and excluded.
    Unverified,
}

fn entry_fate(ownership: EngineOwnership, live: bool, same_generation: bool) -> EntryFate {
    match (ownership, live, same_generation) {
        (_, true, true) => EntryFate::Live,
        (EngineOwnership::Owned, true, false) => EntryFate::Gone,
        (EngineOwnership::Attached, true, false) => EntryFate::ProbeOnNewConnection,
        (EngineOwnership::Owned, false, _) => EntryFate::Gone,
        (EngineOwnership::Attached, false, _) => EntryFate::Unverified,
    }
}

/// Whether the engine-generation question applies to an entry at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreFate {
    /// A running acquisition owns its entry: its task may be launching the
    /// engine right now, and dropping it here would hide the resource from the
    /// task mid-flight.
    Acquiring,
    /// An abandoned `creating` entry with no id can never be resolved: nothing
    /// can be named or probed, so it leaves tracking.
    Abandoned,
    /// The ownership/generation policy decides (`entry_fate`).
    ApplyPolicy,
}

fn pre_fate(entry: &Entry) -> PreFate {
    if entry.acquiring {
        PreFate::Acquiring
    } else if entry.phase == Phase::Creating && entry.context_id.is_none() {
        PreFate::Abandoned
    } else {
        PreFate::ApplyPolicy
    }
}

/// Reconcile tracked entries with the engine, ownership-aware. This is the ONLY
/// place contexts leave the registry without an explicit destroy.
///
/// - OWNED engine, not live: hand launched it and closed/killed it, so its
///   contexts are gone. Dropped, exclusions released.
/// - OWNED engine, live but a different generation: the owned browser was
///   relaunched, a new process, so the old contexts are gone. Dropped.
/// - ATTACHED engine, not live: hand's connection ended; the external browser
///   may still be running and may still hold the context. Kept as UNVERIFIED,
///   exclusion retained, no claim made.
/// - ATTACHED engine, live after a re-attach (new generation): probe each id.
///   Alive -> adopt onto the new generation; gone -> dropped (the engine said
///   so); no answer -> unverified.
/// - An acquisition task still running owns its own entry.
async fn reconcile(browser: &Browser, st: &mut State, allow_reconnect: bool) -> Reconciled {
    let ownership = browser.ownership();
    let mut live = browser.is_live().await;
    let mut generation = browser.generation();
    if allow_reconnect
        && !live
        && ownership == EngineOwnership::Attached
        && browser.reconnect_attached().await
    {
        live = browser.is_live().await;
        generation = browser.generation();
    }
    let was_selected = st.selected.clone();
    let mut out = Reconciled::default();
    let tokens: Vec<u64> = st.entries.iter().map(|e| e.token).collect();
    for token in tokens {
        let Some(idx) = st.entries.iter().position(|e| e.token == token) else {
            continue;
        };
        let selected_here = was_selected.as_deref() == Some(st.entries[idx].name.as_str());
        match pre_fate(&st.entries[idx]) {
            PreFate::Acquiring => continue,
            PreFate::Abandoned => {
                let name = st.entries.remove(idx).name;
                if selected_here {
                    st.selected = None;
                }
                out.dropped.push(name);
                continue;
            }
            PreFate::ApplyPolicy => {}
        }
        let same_generation = st.entries[idx].generation == generation;
        match entry_fate(ownership, live, same_generation) {
            EntryFate::Live => {
                st.entries[idx].unverified = None;
            }
            EntryFate::Gone => {
                let entry = st.entries.remove(idx);
                if let Some(id) = entry.id() {
                    crate::net::on_context_gone(browser, id).await;
                    browser.include_browser_context(id);
                }
                if selected_here {
                    st.selected = None;
                }
                out.dropped.push(entry.name);
            }
            EntryFate::ProbeOnNewConnection => {
                let Some(id) = st.entries[idx].id().map(str::to_string) else {
                    continue;
                };
                let name = st.entries[idx].name.clone();
                let probe = probe_context(browser, &id).await;
                let Some(idx) = st.entries.iter().position(|e| e.token == token) else {
                    continue;
                };
                match probe {
                    Probe::Alive => {
                        st.entries[idx].generation = generation;
                        st.entries[idx].unverified = None;
                    }
                    Probe::Gone => {
                        st.entries.remove(idx);
                        crate::net::on_context_gone(browser, &id).await;
                        browser.include_browser_context(&id);
                        if selected_here {
                            st.selected = None;
                        }
                        out.dropped.push(name);
                    }
                    Probe::Unknown(why) => {
                        st.entries[idx].unverified = Some(format!(
                            "hand re-attached but could not verify this context ({why}); its state is UNKNOWN and it stays tracked"
                        ));
                        out.unverified.push(name);
                    }
                }
            }
            EntryFate::Unverified => {
                let name = st.entries[idx].name.clone();
                st.entries[idx].unverified = Some(attached_lost_note());
                out.unverified.push(name);
            }
        }
    }
    out
}

fn attached_lost_note() -> String {
    "hand's CDP connection to the attached external engine is gone; whether that browser and this \
     context survived is UNKNOWN - hand did not dispose it and keeps it tracked for a later \
     re-attach"
        .to_string()
}

/// The typed "still being created" failure.
fn still_creating(name: &str) -> ToolErr {
    err(
        ToolCode::TargetChanged,
        ToolReason::Raced,
        format!("context: '{name}' is still being created"),
    )
}

/// Turn a name-resolution failure into an ownership-aware explanation when the
/// same pass dropped entries (an owned connection ended) instead of pretending
/// the name never existed.
fn reconcile_reason(browser: &Browser, rec: &Reconciled, e: ToolErr) -> ToolErr {
    if rec.dropped.is_empty() {
        return e;
    }
    match browser.ownership() {
        EngineOwnership::Owned => err(
            ToolCode::EngineDead,
            ToolReason::Transport,
            format!(
                "context: hand's connection to the owned engine it launched ended, so [{}] is not \
                 reachable (a browser context is engine-scoped); create contexts again",
                rec.dropped.join(", ")
            ),
        ),
        EngineOwnership::Attached => err(
            ToolCode::InvalidTarget,
            ToolReason::NotFound,
            format!(
                "context: [{}] is no longer in the attached engine (the engine said it does not exist); tracked contexts are reconciled on every call",
                rec.dropped.join(", ")
            ),
        ),
    }
}

/// No live connection for an operation that needs one. Ownership decides the
/// claim: an owned connection that is not open leaves its contexts unreachable; an attached
/// that is not open proves nothing about the external browser.
/// An engine-death error means the connection is gone: reconcile (ownership
/// decides whether the contexts are gone or merely unverified).
fn no_live_engine(browser: &Browser, what: &str) -> ToolErr {
    match browser.ownership() {
        EngineOwnership::Owned => err(
            ToolCode::EngineDead,
            ToolReason::Transport,
            format!(
                "context: no live connection to the engine hand launched for {what}; a browser \
                 context is engine-scoped, so its contexts are not reachable (nothing is relaunched, \
                 and the process exit is not claimed here)"
            ),
        ),
        EngineOwnership::Attached => err(
            ToolCode::EngineDead,
            ToolReason::Transport,
            format!(
                "context: no live connection to the attached engine for {what}; whether that browser and its contexts survived is UNKNOWN (nothing is relaunched and nothing is claimed)"
            ),
        ),
    }
}
/// The typed failure for an action on an entry whose fate is UNKNOWN.
fn attached_unknown(name: &str) -> ToolErr {
    err(
        ToolCode::EngineDead,
        ToolReason::Transport,
        format!(
            "context: '{name}' is UNVERIFIED - hand's connection to the attached engine is gone and \
             the external browser may still hold it (outcome unknown, not disposed by hand); \
             re-attach to reconcile, or destroy once a connection is back"
        ),
    )
}

/// Identity for the entries in `list` (no pages' live state beyond target info
/// already refreshed by the caller).
fn entry_summary(e: &Entry, selected: Option<&str>) -> Value {
    let mut v = json!({
        "name": e.name,
        "browserContextId": e.context_id,
        "phase": e.phase.as_str(),
        "createdAt": e.created_ms,
        "pageCount": e.pages.len(),
        "selected": selected == Some(e.name.as_str()),
        "pages": e.pages.iter().map(page_view).collect::<Vec<_>>(),
    });
    if let Some(err) = &e.last_error {
        if let Some(o) = v.as_object_mut() {
            o.insert("lastError".to_string(), json!(err));
        }
    }
    if let Some(why) = &e.unverified {
        if let Some(o) = v.as_object_mut() {
            o.insert("unverified".to_string(), json!(why));
        }
    }
    v
}

fn entry_view(e: &Entry, selected: Option<&str>, others: &[String]) -> Value {
    let mut v = entry_summary(e, selected);
    if let Some(o) = v.as_object_mut() {
        o.insert("otherContexts".to_string(), json!(others));
    }
    v
}

fn page_view(p: &Page) -> Value {
    let mut v = json!({
        "targetId": p.target_id,
        "url": p.url,
        "title": p.title,
        "session": if p.session.is_some() { "attached" } else { "none" },
        "sessionSource": "hand-tracked (not a live probe)",
        "source": p.title_origin.as_str(),
    });
    if let (Some(note), Some(o)) = (&p.title_note, v.as_object_mut()) {
        o.insert("titleNote".to_string(), json!(note));
    }
    v
}

fn pick_page<'a>(entry: &'a Entry, want: Option<&str>) -> Result<&'a str, ToolErr> {
    match want {
        Some(tid) => entry
            .pages
            .iter()
            .find(|p| p.target_id == tid)
            .map(|p| p.target_id.as_str())
            .ok_or_else(|| {
                err(
                    ToolCode::InvalidTarget,
                    ToolReason::NotFound,
                    format!(
                        "context: page '{tid}' is not in context '{}' (pages: {:?})",
                        entry.name,
                        entry
                            .pages
                            .iter()
                            .map(|p| p.target_id.as_str())
                            .collect::<Vec<_>>()
                    ),
                )
            }),
        None => entry
            .pages
            .first()
            .map(|p| p.target_id.as_str())
            .ok_or_else(|| {
                err(
                    ToolCode::InvalidTarget,
                    ToolReason::NotFound,
                    format!(
                        "context: context '{}' has no page; open one first",
                        entry.name
                    ),
                )
            }),
    }
}


async fn create_browser_context(browser: &Browser) -> Result<String, String> {
    let r = browser
        .cdp_raw(
            "Target.createBrowserContext",
            json!({"disposeOnDetach": true}),
            None,
        )
        .await?;
    r.get("browserContextId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            engine_dead("context: Target.createBrowserContext returned no browserContextId")
        })
}

/// Live-only CDP: `Ok(Some(v))` on a reply, `Ok(None)` when no engine is live
/// (nothing sent), `Err` on the CDP failure.
async fn live_cdp(
    browser: &Browser,
    method: &str,
    params: Value,
    session: Option<String>,
) -> Result<Option<Value>, String> {
    match browser.cdp_if_live(method, params, session).await {
        None => Ok(None),
        Some(Ok(v)) => Ok(Some(v)),
        Some(Err(e)) => Err(e),
    }
}

fn not_live(what: &str) -> String {
    engine_dead(format!(
        "context: no live connection to the engine for {what}; the context is engine-scoped and is not reachable (nothing is relaunched)"
    ))
}

async fn create_page(browser: &Browser, context_id: &str, url: &str) -> Result<String, String> {
    let r = live_cdp(
        browser,
        "Target.createTarget",
        json!({"url": url, "browserContextId": context_id}),
        None,
    )
    .await?
    .ok_or_else(|| not_live("opening a page"))?;
    r.get("targetId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| engine_dead("context: Target.createTarget returned no targetId"))
}

async fn attach_page(browser: &Browser, target_id: &str) -> Result<String, String> {
    if test_hook_attach_failure() {
        return Err(typed_marker(
            ToolCode::TargetChanged,
            ToolReason::Raced,
            "context: attach forced to fail by HAND_CONTEXT_TEST_FAIL_ATTACH (test-only)",
        ));
    }
    let r = live_cdp(
        browser,
        "Target.attachToTarget",
        json!({"targetId": target_id, "flatten": true}),
        None,
    )
    .await?
    .ok_or_else(|| not_live("attaching a page"))?;
    r.get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| engine_dead("context: Target.attachToTarget returned no sessionId"))
}

fn test_hook_attach_failure() -> bool {
    hook_flag("HAND_CONTEXT_TEST_FAIL_ATTACH")
}

fn test_hook_skip_cookie_set() -> bool {
    hook_flag("HAND_CONTEXT_TEST_SKIP_COOKIE_SET")
}

/// Send a CDP command to an emulation target: the engine's tracked active page
/// when no context name was given, or an explicit context page session.
async fn emulation_send(
    browser: &Browser,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    match session {
        Some(s) => browser.cdp_raw(method, params, Some(s.to_string())).await,
        None => browser.command(method, params, true).await,
    }
}

/// Read a value back from the same page the emulation was applied to.
async fn emulation_eval(
    browser: &Browser,
    session: Option<&str>,
    expression: &str,
) -> Result<Value, String> {
    match session {
        Some(s) => eval_in(browser, s, expression).await,
        None => browser.eval(expression).await,
    }
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, ToolErr> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(invalid("context: expected an array of strings"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid(format!("context: expected a string, got {item}")))
        })
        .collect()
}

/// Evaluate in one named page session. `Runtime.evaluate` needs no domain
/// enable, so a context session is created with zero page-side mutations.
async fn eval_in(browser: &Browser, session: &str, expression: &str) -> Result<Value, String> {
    let r = live_cdp(
        browser,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        Some(session.to_string()),
    )
    .await?
    .ok_or_else(|| not_live("evaluating"))?;
    if let Some(ex) = r.get("exceptionDetails") {
        return Err(js_thrown(ex));
    }
    let robj = r
        .get("result")
        .ok_or("context: evaluate returned no result")?;
    match robj.get("value") {
        Some(v) => Ok(v.clone()),
        None => Err(js_no_value(robj)),
    }
}

async fn ensure_attached(
    browser: &Browser,
    entry: &mut Entry,
    target_id: &str,
) -> Result<String, String> {
    if let Some(p) = entry.pages.iter().find(|p| p.target_id == target_id) {
        if let Some(s) = &p.session {
            return Ok(s.clone());
        }
    } else {
        return Err(format!(
            "context: page '{target_id}' is not in context '{}'",
            entry.name
        ));
    }
    let session = attach_page(browser, target_id).await?;
    if let Some(p) = entry.pages.iter_mut().find(|p| p.target_id == target_id) {
        p.session = Some(session.clone());
    }
    Ok(session)
}

async fn targets_in_context(browser: &Browser, context_id: &str) -> Result<Vec<Value>, String> {
    let r = live_cdp(browser, "Target.getTargets", json!({}), None)
        .await?
        .ok_or_else(|| not_live("listing targets"))?;
    Ok(r.get("targetInfos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            t.get("type").and_then(|v| v.as_str()) == Some("page")
                && t.get("browserContextId").and_then(|v| v.as_str()) == Some(context_id)
        })
        .collect())
}

/// Re-read a context's page list from the engine: new pages (site-opened
/// windows) are adopted, vanished pages dropped, cached url/title refreshed.
/// Never attaches: sessions are created on first use.
async fn refresh_pages(browser: &Browser, entry: &mut Entry) -> Result<(), String> {
    if entry.phase != Phase::Ready {
        return Ok(());
    }
    let Some(context_id) = entry.context_id.clone() else {
        return Ok(());
    };
    let infos = targets_in_context(browser, &context_id).await?;
    let mut seen: Vec<String> = Vec::new();
    for t in &infos {
        let Some(tid) = t.get("targetId").and_then(|v| v.as_str()) else {
            continue;
        };
        seen.push(tid.to_string());
        let url = t
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let title = t
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        match entry.pages.iter_mut().find(|p| p.target_id == tid) {
            Some(p) => {
                let new_document = if p.url == url {
                    false
                } else if p.document_id.is_some() {
                    match p.session.as_deref() {
                        Some(session) => match live_document_id(browser, session).await {
                            Ok(now) => p.document_id.as_deref() != Some(now.as_str()),
                            Err(_) => true,
                        },
                        None => false,
                    }
                } else {
                    true
                };
                p.url = url;
                if new_document {
                    p.title = title;
                    p.title_origin = TitleOrigin::TargetInfo;
                    p.title_note = None;
                    p.document_id = None;
                }
            }
            None => entry.pages.push(Page {
                target_id: tid.to_string(),
                session: None,
                url,
                title,
                title_origin: TitleOrigin::TargetInfo,
                title_note: None,
                document_id: None,
            }),
        }
    }
    entry.pages.retain(|p| seen.contains(&p.target_id));
    Ok(())
}

async fn page_info(browser: &Browser, target_id: &str) -> Result<PageInfo, String> {
    let r = live_cdp(browser, "Target.getTargets", json!({}), None)
        .await?
        .ok_or_else(|| not_live("reading target info"))?;
    let hit = r
        .get("targetInfos")
        .and_then(|v| v.as_array())
        .and_then(|infos| {
            infos
                .iter()
                .find(|t| t.get("targetId").and_then(|v| v.as_str()) == Some(target_id))
        });
    Ok(PageInfo {
        url: hit
            .and_then(|t| t.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        title: hit
            .and_then(|t| t.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// What a create/open reports as the page title: the live document title when
/// the read produced one, otherwise the engine's cached target-info value with
/// the reason the live read did not answer. Never invents a title.
fn reported_title(
    cached: &str,
    live: Result<Option<String>, String>,
) -> (String, TitleOrigin, Option<String>) {
    match live {
        Ok(Some(title)) if !title.is_empty() => (title, TitleOrigin::LiveRead, None),
        Ok(_) => (cached.to_string(), TitleOrigin::TargetInfo, None),
        Err(why) => (
            cached.to_string(),
            TitleOrigin::TargetInfo,
            Some(format!(
                "document.title could not be read ({why}); the title is the engine's cached \
                 target-info value and may lag the document"
            )),
        ),
    }
}

/// Read the document's own title over an attached session. One re-attach is
/// attempted when the navigation replaced the execution context, mirroring
/// `settle_page`. A read that still fails is returned, never papered over.
async fn live_title(
    browser: &Browser,
    target_id: &str,
    session: &mut String,
) -> Result<Option<String>, String> {
    match eval_in(browser, session, "document.title").await {
        Ok(v) => Ok(v.as_str().map(str::to_string)),
        Err(e) if is_session_death(&e) => {
            *session = attach_page(browser, target_id).await?;
            match eval_in(browser, session, "document.title").await {
                Ok(v) => Ok(v.as_str().map(str::to_string)),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// A `complete` document is only the caller's page once it is no longer the
/// initial empty document of a freshly created target. Measured on the pinned
/// build: a target created with a real URL answers `document.URL ==
/// "about:blank"` with `readyState == "complete"` for as long as the navigation
/// takes (a 500 ms hang was observed), so treating that as settled would report
/// a page that has not loaded yet - with an empty title and no url of its own.
///
/// `document` is `None` when the live read failed; that is not evidence the
/// requested page committed, so it counts as pending. The scheme comparison is
/// case-insensitive because URL schemes are.
fn commit_pending(requested: Option<&str>, document: Option<&str>) -> bool {
    let Some(want) = requested.map(str::trim).filter(|w| !w.is_empty()) else {
        return false;
    };
    if want.eq_ignore_ascii_case("about:blank") {
        return false;
    }
    match document {
        None => true,
        Some(url) => {
            let url = url.trim();
            url.is_empty() || url.eq_ignore_ascii_case("about:blank")
        }
    }
}

/// The identity to record for a page after a settle: only a live-read title has
/// a document behind it to name. A failed identity read leaves `None`, which
/// `refresh_pages` treats as "no live value to protect" (target info wins).
async fn document_identity(
    browser: &Browser,
    session: &str,
    origin: TitleOrigin,
) -> Option<String> {
    match origin {
        TitleOrigin::LiveRead => live_document_id(browser, session).await.ok(),
        TitleOrigin::TargetInfo => None,
    }
}

/// Identity of the document currently loaded in a page: `performance.timeOrigin`
/// is per-document and stable across fragment/`pushState` navigation. Measured
/// on the pinned build: a cached page session survives a real navigation (no
/// session death), and the value changes with the new document.
async fn live_document_id(browser: &Browser, session: &str) -> Result<String, String> {
    match eval_in(browser, session, "String(performance.timeOrigin)").await {
        Ok(v) => v
            .as_str()
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "performance.timeOrigin returned no string".to_string()),
        Err(e) => Err(e),
    }
}

/// Bounded readiness wait: `(settled, last readyState observed)`. The
/// observation is always made - there is no default "complete". A navigation
/// can replace the execution context under a live session, so a
/// session-death-class error re-attaches to the SAME target and keeps polling;
/// if the target itself is gone the attach error is propagated. An engine death
/// is propagated too. `requested` is the URL the caller asked for: the initial
/// `about:blank` document of this target is complete long before that URL
/// commits, so it never counts as settled (see `commit_pending`).
async fn settle_page(
    browser: &Browser,
    target_id: &str,
    session: &mut String,
    cap: Duration,
    requested: Option<&str>,
) -> Result<(bool, String), String> {
    let deadline = Instant::now() + cap;
    let mut state = String::from("unobserved");
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok((false, state));
        }
        match tokio::time::timeout(remaining, eval_in(browser, session, "document.readyState"))
            .await
        {
            Ok(Ok(v)) => {
                state = v.as_str().unwrap_or("unknown").to_string();
                if state == "complete" {
                    let url = eval_in(browser, session, "document.URL").await;
                    let document = match &url {
                        Ok(v) => Some(v.as_str().unwrap_or("")),
                        Err(e) if is_engine_dead(e) => return Err(e.clone()),
                        Err(_) => None,
                    };
                    if commit_pending(requested, document) {
                        state = "about:blank (the requested page has not committed)".to_string();
                    } else {
                        return Ok((true, state));
                    }
                }
            }
            Ok(Err(e)) if is_session_death(&e) => {
                *session = attach_page(browser, target_id).await?;
            }
            Ok(Err(e)) if is_engine_dead(&e) => return Err(e),
            Ok(Err(_)) => return Ok((false, state)),
            Err(_) => return Ok((false, state)),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok((false, state));
        }
        tokio::time::sleep(left.min(Duration::from_millis(50))).await;
    }
}

async fn live_jar(browser: &Browser, context_id: &str) -> Option<Result<Vec<Value>, String>> {
    if hook_flag("HAND_CONTEXT_TEST_JAR_ERROR") {
        return Some(Err(
            r#"cdp error: {"code":-32000,"message":"synthetic jar failure (test hook)"}"#
                .to_string(),
        ));
    }
    match live_cdp(
        browser,
        "Storage.getCookies",
        json!({"browserContextId": context_id}),
        None,
    )
    .await
    {
        Ok(None) => None,
        Ok(Some(r)) => Some(Ok(r
            .get("cookies")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default())),
        Err(e) => Some(Err(e)),
    }
}

/// Close one target and wait (bounded) until the engine stops listing it.
async fn close_target_verified(
    browser: &Browser,
    context_id: &str,
    target_id: &str,
) -> Result<(), String> {
    if live_cdp(
        browser,
        "Target.closeTarget",
        json!({"targetId": target_id}),
        None,
    )
    .await?
    .is_none()
    {
        return Err(not_live("closing a page"));
    }
    let deadline = Instant::now() + CLOSE_CAP;
    loop {
        let gone = !targets_in_context(browser, context_id)
            .await?
            .iter()
            .any(|t| t.get("targetId").and_then(|v| v.as_str()) == Some(target_id));
        if gone {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(typed_marker(
                ToolCode::TargetChanged,
                ToolReason::Raced,
                &format!(
                    "context: closeTarget was acknowledged but {target_id} is still listed in {context_id}"
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[derive(Clone, Copy, Debug)]
enum DisposedBy {
    Hand,
    AlreadyGone,
}

impl DisposedBy {
    fn as_str(self) -> &'static str {
        match self {
            DisposedBy::Hand => "hand",
            DisposedBy::AlreadyGone => "already-gone",
        }
    }
}

/// Dispose a context and prove it is gone: no page target carries its id AND
/// `Storage.getCookies` refuses the id. Never launches an engine. The test hook
/// `HAND_CONTEXT_TEST_FAIL_DISPOSE=1` simulates a failed disposal by skipping
/// the dispose command (so verification sees the context still present).
async fn dispose_verified(
    browser: &Browser,
    context_id: &str,
) -> Result<(DisposedBy, Value), String> {
    if let Some(ms) = hook_ms("HAND_CONTEXT_TEST_SLOW_DISPOSE_MS") {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if test_hook_dispose_failure() {
        return Err(verified_not_gone(browser, context_id).await);
    }
    let disposed_by = match live_cdp(
        browser,
        "Target.disposeBrowserContext",
        json!({"browserContextId": context_id}),
        None,
    )
    .await
    {
        Ok(None) => return Err(not_live("disposing a context")),
        Ok(Some(_)) => DisposedBy::Hand,
        Err(e) if is_unknown_context(&e) => DisposedBy::AlreadyGone,
        Err(e) => return Err(e),
    };
    let pages_left = match live_cdp(browser, "Target.getTargets", json!({}), None).await? {
        None => return Err(not_live("verifying a disposal")),
        Some(r) => r
            .get("targetInfos")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|t| {
                        t.get("type").and_then(|v| v.as_str()) == Some("page")
                            && t.get("browserContextId").and_then(|v| v.as_str())
                                == Some(context_id)
                    })
                    .count()
            })
            .unwrap_or(0),
    };
    let cookie_probe = match live_cdp(
        browser,
        "Storage.getCookies",
        json!({"browserContextId": context_id}),
        None,
    )
    .await
    {
        Ok(None) => return Err(not_live("verifying a disposal")),
        Ok(Some(_)) => "context still answers Storage.getCookies".to_string(),
        Err(e) if is_unknown_context(&e) => "failed to find browser context".to_string(),
        Err(e) => return Err(e),
    };
    if pages_left == 0 && cookie_probe == "failed to find browser context" {
        Ok((
            disposed_by,
            json!({"pageTargets": 0, "cookieProbe": cookie_probe}),
        ))
    } else {
        Err(typed_marker(
            ToolCode::TargetChanged,
            ToolReason::Raced,
            &format!(
                "context: dispose was acknowledged but the context is not proven gone (page targets left: {pages_left}; cookie probe: {cookie_probe}) - it is still tracked"
            ),
        ))
    }
}

fn test_hook_dispose_failure() -> bool {
    hook_flag("HAND_CONTEXT_TEST_FAIL_DISPOSE")
}

/// Re-derive the "not proven gone" failure for the dispose-failure hook.
async fn verified_not_gone(browser: &Browser, context_id: &str) -> String {
    let pages_left = match live_cdp(browser, "Target.getTargets", json!({}), None).await {
        Ok(Some(r)) => r
            .get("targetInfos")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|t| {
                        t.get("type").and_then(|v| v.as_str()) == Some("page")
                            && t.get("browserContextId").and_then(|v| v.as_str())
                                == Some(context_id)
                    })
                    .count()
            })
            .unwrap_or(0),
        _ => 0,
    };
    typed_marker(
        ToolCode::TargetChanged,
        ToolReason::Raced,
        &format!(
            "context: dispose was not confirmed (test hook HAND_CONTEXT_TEST_FAIL_DISPOSE); page targets left: {pages_left} - it is still tracked"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(names: &[&str]) -> State {
        let mut st = State::default();
        for (i, n) in names.iter().enumerate() {
            st.entries.push(Entry {
                token: (i as u64) + 1,
                name: (*n).to_string(),
                phase: Phase::Ready,
                context_id: Some(format!("CTX{i}")),
                created_ms: 0,
                generation: 1,
                pages: Vec::new(),
                acquiring: false,
                last_error: None,
                unverified: None,
            });
        }
        st
    }

    #[test]
    fn names_are_trimmed_bounded_and_control_free() {
        assert_eq!(normalise_name("  alpha  ").unwrap(), "alpha");
        assert!(normalise_name("").is_err());
        assert!(normalise_name("   ").is_err());
        assert!(normalise_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
        assert!(normalise_name("a\nb").is_err());
    }

    #[test]
    fn urls_default_to_about_blank_and_refuse_empty() {
        assert_eq!(normalise_url(None).unwrap(), "about:blank");
        assert_eq!(
            normalise_url(Some(" http://127.0.0.1:1/ ")).unwrap(),
            "http://127.0.0.1:1/"
        );
        assert!(normalise_url(Some("  ")).is_err());
    }

    /// The published text must not claim that destroy fires no teardown handler.
    /// Chrome 158 fires `pagehide` on `Target.disposeBrowserContext`, and CDP
    /// cannot suppress it; only `beforeunload` and `unload` never run.
    #[test]
    fn published_text_states_the_measured_destroy_teardown() {
        let schema = tool_schema();
        let info = capability_info();
        let description = schema["description"].as_str().expect("tool description");
        let limits = LIMITS.join("\n");
        let teardown = info["isolatedContexts"]["teardown"]
            .as_str()
            .expect("capability teardown");
        for text in [description, limits.as_str(), teardown] {
            assert!(
                text.contains("`pagehide`"),
                "the published text must name pagehide as firing on destroy: {text}"
            );
            assert!(
                text.contains("never `beforeunload` or `unload`"),
                "the published text must state that beforeunload/unload never fire: {text}"
            );
            assert!(
                !text.contains("NO beforeunload/pagehide/unload")
                    && !text.contains("no beforeunload, pagehide or unload")
                    && !text.contains("no beforeunload/pagehide/unload"),
                "the old false claim must be gone: {text}"
            );
        }
    }

    #[test]
    fn resolution_uses_selection_then_the_only_context_then_refuses_ambiguity() {
        let mut st = state_with(&["a", "b"]);
        assert!(
            resolve_name(&st, None).is_err(),
            "two contexts, no selection: ambiguous"
        );
        let ambiguous = resolve_name(&st, None).unwrap_err();
        assert!(
            ambiguous.prose().contains("'name' is required")
                && ambiguous.prose().contains("\"a\"")
                && ambiguous.prose().contains("\"b\""),
            "ambiguity names the candidates: {}",
            ambiguous.prose()
        );
        st.selected = Some("b".into());
        assert_eq!(resolve_name(&st, None).unwrap(), "b");
        assert_eq!(resolve_name(&st, Some("a")).unwrap(), "a");
        assert!(resolve_name(&st, Some("missing")).is_err());
        let single = state_with(&["only"]);
        assert_eq!(resolve_name(&single, None).unwrap(), "only");
        let empty = State::default();
        assert!(resolve_name(&empty, None).is_err());
    }

    #[test]
    fn an_owned_engine_that_is_not_running_drops_its_contexts() {
        let browser = Browser::new();
        let mut st = state_with(&["a", "b"]);
        st.entries[1].generation = 0; // current generation of a never-launched engine
        st.selected = Some("a".into());
        let rec = futures_lite_block(reconcile(&browser, &mut st, false));
        assert_eq!(rec.dropped, vec!["a".to_string(), "b".to_string()]);
        assert!(rec.unverified.is_empty());
        assert!(st.entries.is_empty());
        assert_eq!(st.selected, None);
        assert!(browser.excluded_browser_contexts().is_empty());
    }

    #[test]
    fn the_ownership_matrix_never_reports_attached_loss_as_gone() {
        use EngineOwnership::{Attached, Owned};
        assert_eq!(entry_fate(Owned, true, true), EntryFate::Live);
        assert_eq!(entry_fate(Attached, true, true), EntryFate::Live);
        assert_eq!(entry_fate(Owned, false, true), EntryFate::Gone);
        assert_eq!(entry_fate(Owned, true, false), EntryFate::Gone);
        assert_eq!(
            entry_fate(Attached, true, false),
            EntryFate::ProbeOnNewConnection
        );
        assert_eq!(entry_fate(Attached, false, true), EntryFate::Unverified);
        assert_eq!(entry_fate(Attached, false, false), EntryFate::Unverified);
    }

    fn futures_lite_block<F: std::future::Future<Output = T>, T>(f: F) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    #[test]
    fn the_initial_empty_document_never_counts_as_the_requested_page() {
        assert!(commit_pending(
            Some("http://127.0.0.1/a"),
            Some("about:blank")
        ));
        assert!(commit_pending(Some("http://127.0.0.1/a"), Some("")));
        assert!(commit_pending(
            Some("http://127.0.0.1/a"),
            Some("ABOUT:BLANK")
        ));
        assert!(commit_pending(
            Some("http://127.0.0.1/a"),
            Some("  about:blank  ")
        ));
        assert!(commit_pending(Some("http://127.0.0.1/a"), None));
        assert!(!commit_pending(
            Some("http://127.0.0.1/a"),
            Some("http://127.0.0.1/a")
        ));
        assert!(!commit_pending(
            Some("http://127.0.0.1/a"),
            Some("http://127.0.0.1/elsewhere")
        ));
        assert!(!commit_pending(Some("about:blank"), Some("about:blank")));
        assert!(!commit_pending(Some("ABOUT:BLANK"), Some("about:blank")));
        assert!(!commit_pending(None, Some("about:blank")));
        assert!(!commit_pending(Some("   "), Some("about:blank")));
    }

    #[test]
    fn a_title_is_labelled_by_how_it_was_obtained() {
        let (title, origin, note) = reported_title("cached", Ok(Some("Titled A".into())));
        assert_eq!(title, "Titled A");
        assert_eq!(origin, TitleOrigin::LiveRead);
        assert_eq!(origin.as_str(), "targetInfo+liveRead");
        assert!(note.is_none(), "a successful read has nothing to explain");

        for live in [Ok(None), Ok(Some(String::new()))] {
            let (title, origin, note) = reported_title("cached", live);
            assert_eq!(title, "cached");
            assert_eq!(origin, TitleOrigin::TargetInfo);
            assert_eq!(origin.as_str(), "targetInfo");
            assert!(note.is_none());
        }

        let (title, origin, note) = reported_title("cached", Err("session died".into()));
        assert_eq!(title, "cached");
        assert_eq!(origin, TitleOrigin::TargetInfo);
        let note = note.expect("a failed live read must explain itself");
        assert!(note.contains("session died"), "{note}");
        assert!(note.contains("may lag"), "{note}");
    }

    #[test]
    fn page_view_reports_its_provenance_and_never_hides_a_failed_read() {
        let mut page = Page {
            target_id: "T1".into(),
            session: None,
            url: "http://127.0.0.1/a".into(),
            title: "Titled A".into(),
            title_origin: TitleOrigin::LiveRead,
            title_note: None,
            document_id: None,
        };
        let v = page_view(&page);
        assert_eq!(v["title"].as_str(), Some("Titled A"));
        assert_eq!(v["source"].as_str(), Some("targetInfo+liveRead"));
        assert!(v.get("titleNote").is_none(), "{v}");

        page.title_origin = TitleOrigin::TargetInfo;
        page.title_note = Some("document.title could not be read (session died)".into());
        let v = page_view(&page);
        assert_eq!(v["source"].as_str(), Some("targetInfo"));
        assert!(
            v["titleNote"]
                .as_str()
                .unwrap_or("")
                .contains("could not be read"),
            "the failed read is stated, not hidden: {v}"
        );
    }

    #[test]
    fn aliveness_never_drops_a_live_entry_because_a_sibling_is_stale() {
        let mut ready = state_with(&["live"]).entries.remove(0);
        ready.generation = 3;
        let mut stale = ready.clone();
        stale.name = "stale".into();
        stale.generation = 2;
        let mut creating = ready.clone();
        creating.name = "pending".into();
        creating.phase = Phase::Creating;
        creating.context_id = None;
        creating.generation = 0;
        creating.acquiring = true;
        let mut abandoned = creating.clone();
        abandoned.name = "abandoned".into();
        abandoned.acquiring = false;

        assert_eq!(pre_fate(&creating), PreFate::Acquiring);
        assert_eq!(pre_fate(&abandoned), PreFate::Abandoned);
        assert_eq!(pre_fate(&ready), PreFate::ApplyPolicy);

        assert_eq!(
            entry_fate(EngineOwnership::Owned, true, true),
            EntryFate::Live
        );
        assert_eq!(
            entry_fate(EngineOwnership::Owned, true, false),
            EntryFate::Gone
        );
    }

    #[test]
    fn duplicate_or_unknown_context_names_are_typed_not_found() {
        let st = state_with(&["a"]);
        let e = entry_index(&st, "a").unwrap();
        assert_eq!(e, 0);
        let e = entry_index(&st, "b").unwrap_err();
        assert!(e.prose().contains("no context named 'b'"));
    }

    #[test]
    fn cookies_require_an_origin_and_reject_unknown_fields() {
        assert!(parse_cookie(&json!({"name": "k", "value": "v"})).is_err());
        assert!(parse_cookie(&json!({"name": "k", "value": "v",
            "url": "http://127.0.0.1:1/"}))
        .is_ok());
        let d = parse_cookie(&json!({"name": "k", "value": "v", "domain": "127.0.0.1"})).unwrap();
        assert_eq!(
            d.spec["path"].as_str(),
            Some("/"),
            "domain form gets a default path"
        );
        assert!(parse_cookie(&json!({"name": "k", "value": "v",
            "url": "http://127.0.0.1:1/", "extra": 1}))
        .is_err());
        assert!(parse_cookie(&json!({"name": "", "value": "v",
            "url": "http://127.0.0.1:1/"}))
        .is_err());
    }

    #[test]
    fn cookie_verification_uses_identity_and_attributes() {
        let jar = |entries: &[(&str, &str, &str, bool)]| -> Vec<Value> {
            entries
                .iter()
                .map(|(name, path, value, http_only)| {
                    json!({"name": name, "domain": "127.0.0.1", "path": path,
                           "value": value, "httpOnly": http_only, "secure": false,
                           "sameSite": "Lax", "expires": -1})
                })
                .collect()
        };
        let parse = |v: Value| parse_cookie(&v).unwrap();

        let want_b =
            parse(json!({"name":"k","value":"two","url":"http://127.0.0.1:1/","path":"/b"}));
        let before = jar(&[("k", "/a", "two", false)]);
        let after = jar(&[("k", "/a", "two", false)]);
        let refusal = want_b.verify(&before, &after).unwrap_err();
        assert_eq!(
            refusal["reason"],
            json!("no-cookie-with-this-identity-and-value"),
            "a same-name cookie at another path is not the requested identity"
        );
        assert_eq!(refusal["observedSameName"][0]["path"], json!("/a"));

        let want_ho = parse(json!({"name":"k","value":"two","url":"http://127.0.0.1:1/",
            "path":"/b","httpOnly":true}));
        let after = jar(&[("k", "/b", "two", false)]);
        let refusal = want_ho.verify(&[], &after).unwrap_err();
        assert_eq!(refusal["reason"], json!("attribute-mismatch"));
        assert_eq!(refusal["attribute"], json!("httpOnly"));

        let after = jar(&[("k", "/b", "two", true)]);
        let ok = want_ho.verify(&[], &after).unwrap();
        assert_eq!(ok["verified"], json!(true));
        assert_eq!(ok["preexisting"], json!(false));
        assert_eq!(ok["identity"]["domain"], json!("127.0.0.1"));
        let ok = want_ho.verify(&after, &after).unwrap();
        assert_eq!(
            ok["preexisting"],
            json!(true),
            "an identical pre-existing cookie is not hand's work"
        );
    }

    #[test]
    fn page_selection_defaults_to_the_first_and_names_the_pages_on_miss() {
        let entry = Entry {
            token: 7,
            name: "a".into(),
            phase: Phase::Ready,
            context_id: Some("CTX".into()),
            created_ms: 0,
            generation: 1,
            pages: vec![Page {
                target_id: "T1".into(),
                session: None,
                url: "about:blank".into(),
                title: String::new(),
                title_origin: TitleOrigin::TargetInfo,
                title_note: None,
                document_id: None,
            }],
            acquiring: false,
            last_error: None,
            unverified: None,
        };
        assert_eq!(pick_page(&entry, None).unwrap(), "T1");
        assert_eq!(pick_page(&entry, Some("T1")).unwrap(), "T1");
        assert!(pick_page(&entry, Some("T2"))
            .unwrap_err()
            .prose()
            .contains("T2"));
        let empty = Entry {
            pages: Vec::new(),
            ..entry.clone()
        };
        assert!(pick_page(&empty, None)
            .unwrap_err()
            .prose()
            .contains("no page"));
    }

    #[test]
    fn typed_markers_are_recognised_as_engine_death() {
        assert!(is_engine_dead(&engine_dead("gone")));
        assert!(!is_engine_dead("cdp error: {}"));
        assert_eq!(plain(&engine_dead("gone")), "gone");
        assert_eq!(
            plain("cdp error: {\"code\":-32602}"),
            "cdp error: {\"code\":-32602}"
        );
    }

    #[test]
    fn the_tool_entry_is_one_unprefixed_catalog_entry() {
        let schema = tool_schema();
        assert_eq!(
            schema["name"].as_str(),
            Some(TOOL),
            "hand adds the browser_ prefix"
        );
        assert_eq!(
            schema["inputSchema"]["required"][0].as_str(),
            Some("action")
        );
        assert_eq!(
            schema["inputSchema"]["additionalProperties"].as_bool(),
            Some(false),
            "unknown arguments are refused, so the schema must say so"
        );
        assert_eq!(
            schema["outputSchema"]["additionalProperties"].as_bool(),
            Some(false)
        );
        for key in [
            "ok",
            "name",
            "browserContextId",
            "phase",
            "contexts",
            "pages",
            "unverified",
            "dropped",
            "pending",
            "failed",
            "ownership",
            "disposedBy",
            "goneWithEngine",
            "source",
            "titleNote",
            "limits",
        ] {
            assert!(
                schema["outputSchema"]["properties"][key].is_object(),
                "outputSchema must declare `{key}`"
            );
        }
        let item = &schema["inputSchema"]["properties"]["cookies"]["items"];
        assert_eq!(
            item["required"],
            json!(["name", "value"]),
            "name and value are required"
        );
        let any_of = item["anyOf"]
            .as_array()
            .expect("the url|domain requirement is published");
        assert_eq!(any_of.len(), 2, "exactly url or domain: {any_of:?}");
        assert_eq!(any_of[0]["required"], json!(["url"]));
        assert_eq!(any_of[1]["required"], json!(["domain"]));
        assert!(
            parse_cookie(&json!({"name": "a", "value": "b"})).is_err(),
            "the enforced refusal backs the published schema"
        );
        assert!(
            parse_cookie(&json!({"name": "a", "value": "b", "url": "http://127.0.0.1/"})).is_ok()
        );
        assert!(parse_cookie(&json!({"name": "a", "value": "b", "domain": "127.0.0.1"})).is_ok());
        let actions: Vec<&str> = schema["inputSchema"]["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for a in [
            "create",
            "list",
            "show",
            "select",
            "open",
            "close",
            "destroy",
            "destroy_all",
            "eval",
            "cookies",
        ] {
            assert!(actions.contains(&a), "action {a} is published: {actions:?}");
        }
        assert!(is_context_tool("context"));
        assert!(
            !is_context_tool("browser_context"),
            "the catalog prefixes, not this module"
        );
        assert!(!is_context_tool("navigate"));
    }

    #[test]
    fn capability_info_names_the_scope_and_the_limits() {
        let caps = capability_info();
        let ctx = &caps["isolatedContexts"];
        assert_eq!(ctx["available"].as_bool(), Some(true));
        assert_eq!(ctx["tool"].as_str(), Some("browser_context"));
        for k in ["cookies", "localStorage", "sessionStorage"] {
            assert!(
                ctx["isolation"]["verified"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v.as_str() == Some(k)),
                "{k} is verified isolation"
            );
        }
        assert!(
            ctx["selection"]
                .as_str()
                .unwrap_or("")
                .contains("legacy browser_* tools keep acting on the legacy default page"),
            "selection states that legacy tools are unaffected"
        );
        assert!(
            !ctx["limits"].as_array().unwrap().is_empty(),
            "limits are stated, not implied"
        );
        assert!(
            ctx["isolation"]["process"]
                .as_str()
                .unwrap_or("")
                .contains("not a crash boundary"),
            "process isolation is not claimed"
        );
        assert!(
            ctx["legacyReach"]
                .as_str()
                .unwrap_or("")
                .contains("refuse a context page"),
            "the legacy boundary names the refused routes"
        );
    }
}
