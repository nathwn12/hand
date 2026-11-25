//! Named isolated browser contexts over the REAL MCP endpoint: what a client
//! discovers, what `tools/call` returns, and what EOF shutdown does.
//!
//! Why the wire and not the engine: `tests/browser_contexts.rs` proves the
//! engine's own `contexts::dispatch`, its typed codes and its CDP mechanics.
//! This file proves the USER route: the spawned `hand` binary, one stdio
//! MCP connection, the catalog a client sees, and the envelopes it receives,
//! against a controlled loopback fixture and the owned pinned
//! `chrome-headless-shell`.
//!
//! What the tests establish:
//! - discovery publishes `browser_context` exactly once, with the closed
//!   action set, `required: ["action"]` and `additionalProperties: false` on
//!   the input (and `ok` required on the output), so the published contract is
//!   the one the engine dispatches;
//! - two named contexts are separate storage partitions: a cookie set in one
//!   is absent from the other's jar and from its `document.cookie`, and each
//!   jar reports only its own cookie;
//! - `select` changes which context an omitted-name action uses and states
//!   that legacy `browser_*` tools are unaffected;
//! - `destroy` proves the context gone, clears a selection of it, and leaves
//!   the other context working;
//! - the legacy page stays separate: `browser_tabs` never lists a context
//!   page, legacy `targets select` refuses one by name, a legacy cookie never
//!   reaches a context (and a context cookie never reaches the legacy page),
//!   and legacy `evaluate` keeps acting on the default page through
//!   create/select/destroy;
//! - H12: a context page is READ by `browser_snapshot` and `browser_assert`
//!   only when the call names it (`context:` plus optional `page:`). The reply
//!   names the context and page target read, an unknown context or page is a
//!   typed `invalid_target`/`not_found` that never falls back to the legacy
//!   page, more than one page without `page:` is a typed
//!   `invalid_args`/`ambiguous_target` listing them, and a call without
//!   `context:` still reads the legacy page;
//! - EOF shutdown with a live isolated context exits 0 in bounded time; in the
//!   attached case it closes only what hand owns: a second hand client's
//!   context, page and cookie survive, and the external engine keeps running;
//! - every refusal is a TOOL result (`isError` plus typed `code`/`reason`),
//!   never a JSON-RPC protocol error.
//!
//! Not established here (and not claimed): IndexedDB/HTTP-cache isolation,
//! process isolation (contexts share one browser process), frame refs,
//! dialog handling inside a context, or network interception.
//!
//! This file is written against the integration contract in
//! `crates/browser-engine/src/contexts.rs` (`tool_schema`, `dispatch`,
//! `shutdown`). It fails until the server publishes and routes
//! `browser_context`; that is what an acceptance test written ahead of the
//! integration is for.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use common::{is_error, navigate_with_retry, structured, text_of, Session};
use serde_json::{json, Value};

const CONTEXT_TOOL: &str = "browser_context";


/// Every fixture page has its own title, so "which page answered?" is never
/// inferred from a shared value.
fn page(title: &str, marker: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title>\n\
         <style>body{{font:16px system-ui;margin:24px}}</style></head>\n\
         <body data-page=\"{marker}\"><h1>{title}</h1>\n\
         <a id=\"next\" href=\"/legacy\">legacy</a></body></html>"
    )
}

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let port = listener.local_addr().expect("addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        std::thread::spawn(move || {
                            let _ = serve(stream);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Fixture {
            base: format!("http://127.0.0.1:{port}"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    // Accepted sockets inherit the listener's nonblocking mode on Windows.
    // Without this, a read before the request arrives returns WouldBlock and
    // drops the stream, which Chrome sees as ERR_CONNECTION_ABORTED.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, body, content_type) = match path {
        "/" => ("200 OK", page("Ctx Fixture", "root"), HTML),
        "/a" => ("200 OK", page("Ctx A", "a"), HTML),
        "/b" => ("200 OK", page("Ctx B", "b"), HTML),
        "/legacy" => ("200 OK", page("Legacy Fixture", "legacy"), HTML),
        "/favicon.ico" => ("204 No Content", String::new(), "text/plain"),
        _ => ("404 Not Found", page("Not Found", "missing"), HTML),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

const HTML: &str = "text/html; charset=utf-8";


/// The pinned browser must be provisioned; the engine's own message otherwise.
fn require_pinned_chromium() -> std::path::PathBuf {
    let chromium = browser_engine::chromium_path()
        .expect("the pinned chrome-headless-shell must be provisioned for this live test");
    assert!(
        chromium.is_file(),
        "chromium path is not a file: {chromium:?}"
    );
    eprintln!("HAND_BROWSER_BIN={}", chromium.display());
    chromium
}

/// One `browser_context` call that must arrive as a TOOL result. A protocol
/// error here would mean the name never reached a dispatcher.
fn call(s: &mut Session, args: Value) -> Value {
    let resp = s.call(CONTEXT_TOOL, args);
    assert!(
        resp.get("error").is_none(),
        "{CONTEXT_TOOL} must answer as a tool result, not a protocol error: {resp}"
    );
    assert!(
        resp.get("result").is_some(),
        "{CONTEXT_TOOL} must answer with a result: {resp}"
    );
    resp
}

/// A call that must succeed; returns its `structuredContent`.
fn ok(s: &mut Session, args: Value) -> Value {
    let resp = call(s, args);
    assert!(
        !is_error(&resp),
        "{CONTEXT_TOOL} failed: {}",
        text_of(&resp)
    );
    structured(&resp).clone()
}

/// A call that must fail as a typed tool error; returns
/// `(code, reason, human text)`.
fn failed(s: &mut Session, args: Value) -> (String, String, String) {
    let resp = call(s, args);
    assert!(
        is_error(&resp),
        "expected a typed tool error, got success: {resp}"
    );
    let sc = structured(&resp);
    assert_ne!(
        sc.get("code").and_then(|v| v.as_str()).unwrap_or(""),
        "",
        "a tool error carries a code: {sc}"
    );
    assert_ne!(
        sc.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
        "",
        "a tool error carries a reason: {sc}"
    );
    (
        sc["code"].as_str().unwrap_or("").to_string(),
        sc["reason"].as_str().unwrap_or("").to_string(),
        text_of(&resp),
    )
}

fn jar_pairs(sc: &Value) -> Vec<(String, String)> {
    sc["cookies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|c| {
                    (
                        c["name"].as_str().unwrap_or("").to_string(),
                        c["value"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn jar_has(sc: &Value, name: &str, value: &str) -> bool {
    jar_pairs(sc).iter().any(|(n, v)| n == name && v == value)
}


#[test]
fn catalog_publishes_browser_context_with_its_closed_action_set() {
    let mut s = Session::spawn();
    let tools = s.list_tools();
    let names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    let entries: Vec<&Value> = tools
        .iter()
        .filter(|t| t["name"].as_str() == Some(CONTEXT_TOOL))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "the catalog must publish exactly one {CONTEXT_TOOL}; it published {} tool(s): {names:?}",
        tools.len()
    );
    let entry = entries[0];

    let description = entry["description"].as_str().unwrap_or("");
    assert!(
        description.contains("isolated browser contexts"),
        "the description must name the capability: {description}"
    );
    assert!(
        description.contains("legacy browser_* tools keep acting on the legacy default page"),
        "the description must scope selection away from legacy tools: {description}"
    );

    let input = &entry["inputSchema"];
    assert_eq!(input["type"].as_str(), Some("object"), "input schema");
    assert_eq!(
        input["required"],
        json!(["action"]),
        "`action` is the only required argument"
    );
    assert_eq!(
        input["additionalProperties"].as_bool(),
        Some(false),
        "unknown arguments are refused, so the schema must say so"
    );
    let mut actions: Vec<String> = input["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    actions.sort();
    assert_eq!(
        actions,
        vec![
            "clock",
            "close",
            "cookies",
            "create",
            "destroy",
            "destroy_all",
            "eval",
            "export_state",
            "import_state",
            "list",
            "locale",
            "open",
            "permissions",
            "reset",
            "select",
            "show",
        ],
        "the published action set must be the dispatched one"
    );
    for field in ["clock", "iso", "locale", "timezone", "permissions", "grant"] {
        assert!(
            input["properties"].get(field).is_some(),
            "the manifest's requires field {field} must be in the published inputSchema"
        );
    }

    let output = &entry["outputSchema"];
    assert!(
        output["required"]
            .as_array()
            .is_some_and(|r| r.iter().any(|v| v.as_str() == Some("ok"))),
        "a successful result always carries `ok`: {output}"
    );
    assert_eq!(
        output["additionalProperties"].as_bool(),
        Some(false),
        "the output schema is closed too"
    );
    assert_eq!(
        entry["annotations"]["destructiveHint"].as_bool(),
        Some(true),
        "create/destroy are destructive; the annotation must not hide it"
    );
}


#[test]
fn two_contexts_isolate_cookies_and_the_legacy_page_stays_separate() {
    require_pinned_chromium();
    let fixture = Fixture::start();
    let base = fixture.base.clone();
    let mut s = Session::spawn();

    let nav = navigate_with_retry(&mut s, &format!("{base}/legacy"));
    assert!(!is_error(&nav), "legacy navigate failed: {}", text_of(&nav));
    assert_eq!(
        structured(&nav)["title"].as_str(),
        Some("Legacy Fixture"),
        "the legacy page must be the fixture's legacy route"
    );

    let alpha = ok(
        &mut s,
        json!({"action": "create", "name": "alpha", "url": format!("{base}/a")}),
    );
    assert_eq!(alpha["name"].as_str(), Some("alpha"));
    assert_eq!(alpha["url"].as_str(), Some(format!("{base}/a").as_str()));
    assert_eq!(alpha["title"].as_str(), Some("Ctx A"));
    assert_eq!(
        alpha["settled"].as_bool(),
        Some(true),
        "a loopback fixture page settles before create returns: {alpha}"
    );
    assert_eq!(
        alpha["selected"].as_str(),
        Some("alpha"),
        "create registers the new context as the registry default"
    );
    assert!(
        alpha["limits"].as_array().is_some_and(|l| !l.is_empty()),
        "the result states its limits instead of implying isolation: {alpha}"
    );
    let alpha_ctx = alpha["browserContextId"]
        .as_str()
        .expect("browserContextId")
        .to_string();
    let alpha_page = alpha["page"]["targetId"]
        .as_str()
        .expect("create reports its page target")
        .to_string();

    let beta = ok(
        &mut s,
        json!({"action": "create", "name": "beta", "url": format!("{base}/b")}),
    );
    assert_eq!(beta["title"].as_str(), Some("Ctx B"));
    assert_eq!(beta["selected"].as_str(), Some("beta"));
    let beta_ctx = beta["browserContextId"]
        .as_str()
        .expect("browserContextId")
        .to_string();
    let beta_page = beta["page"]["targetId"]
        .as_str()
        .expect("create reports its page target")
        .to_string();
    assert_ne!(
        alpha_ctx, beta_ctx,
        "two contexts are two storage partitions"
    );
    assert_ne!(alpha_page, beta_page, "two contexts are two pages");

    let duplicate = failed(&mut s, json!({"action": "create", "name": "alpha"}));
    assert_eq!(
        (duplicate.0.as_str(), duplicate.1.as_str()),
        ("invalid_args", "bad_value")
    );
    assert!(
        duplicate.2.contains("already exists"),
        "the duplicate-name refusal explains itself: {}",
        duplicate.2
    );

    let unknown_action = failed(&mut s, json!({"action": "nuke"}));
    assert_eq!(
        (unknown_action.0.as_str(), unknown_action.1.as_str()),
        ("invalid_args", "unknown_action")
    );

    let foreign_key = failed(
        &mut s,
        json!({"action": "list", "url": format!("{base}/a")}),
    );
    assert_eq!(
        (foreign_key.0.as_str(), foreign_key.1.as_str()),
        ("invalid_args", "unknown_arg"),
        "an argument belonging to another action is refused, never ignored: {}",
        foreign_key.2
    );

    let empty_set = failed(
        &mut s,
        json!({"action": "cookies", "name": "alpha", "cookies": []}),
    );
    assert_eq!(
        (empty_set.0.as_str(), empty_set.1.as_str()),
        ("invalid_args", "bad_value")
    );

    let alpha_empty = ok(&mut s, json!({"action": "cookies", "name": "alpha"}));
    assert!(
        jar_pairs(&alpha_empty).is_empty(),
        "a fresh context partition starts with an empty jar: {alpha_empty}"
    );

    let set_alpha = ok(
        &mut s,
        json!({"action": "cookies", "name": "alpha", "cookies": [
            {"name": "who", "value": "alpha", "url": format!("{base}/")}
        ]}),
    );
    assert!(
        jar_has(&set_alpha, "who", "alpha"),
        "a set is reported only when the jar shows it: {set_alpha}"
    );
    assert_eq!(
        set_alpha["set"][0]["verified"].as_bool(),
        Some(true),
        "the per-cookie receipt is verified, not assumed: {set_alpha}"
    );

    let set_beta = ok(
        &mut s,
        json!({"action": "cookies", "name": "beta", "cookies": [
            {"name": "who", "value": "beta", "url": format!("{base}/")}
        ]}),
    );
    assert!(jar_has(&set_beta, "who", "beta"), "{set_beta}");

    let alpha_jar = ok(&mut s, json!({"action": "cookies", "name": "alpha"}));
    assert_eq!(
        jar_pairs(&alpha_jar),
        vec![("who".to_string(), "alpha".to_string())],
        "alpha's jar must hold only alpha's cookie: {alpha_jar}"
    );
    assert!(
        !jar_has(&alpha_jar, "who", "beta"),
        "beta's cookie must be invisible to alpha: {alpha_jar}"
    );

    let alpha_cookie = ok(
        &mut s,
        json!({"action": "eval", "name": "alpha", "expression": "document.cookie"}),
    );
    assert_eq!(
        alpha_cookie["value"].as_str(),
        Some("who=alpha"),
        "the page in alpha reads alpha's jar: {alpha_cookie}"
    );
    let beta_cookie = ok(
        &mut s,
        json!({"action": "eval", "name": "beta", "expression": "document.cookie"}),
    );
    assert_eq!(
        beta_cookie["value"].as_str(),
        Some("who=beta"),
        "the page in beta reads beta's jar: {beta_cookie}"
    );

    let legacy_cookie = s.call("browser_evaluate", json!({"expression": "document.cookie"}));
    assert!(
        !is_error(&legacy_cookie),
        "legacy evaluate failed: {}",
        text_of(&legacy_cookie)
    );
    assert!(
        !structured(&legacy_cookie)["value"]
            .as_str()
            .unwrap_or("")
            .contains("who="),
        "a context's cookie must not appear in the legacy default context: {legacy_cookie}"
    );
    let set_legacy = s.call(
        "browser_evaluate",
        json!({"expression": "document.cookie='legacy=1; path=/'; document.cookie"}),
    );
    assert!(
        structured(&set_legacy)["value"]
            .as_str()
            .unwrap_or("")
            .contains("legacy=1"),
        "the legacy page's own cookie must be set for the direction check: {set_legacy}"
    );
    let alpha_after_legacy = ok(
        &mut s,
        json!({"action": "eval", "name": "alpha", "expression": "document.cookie"}),
    );
    assert_eq!(
        alpha_after_legacy["value"].as_str(),
        Some("who=alpha"),
        "a legacy cookie must not leak into an isolated context: {alpha_after_legacy}"
    );

    let tabs = s.call("browser_tabs", json!({"action": "list"}));
    assert!(!is_error(&tabs), "tabs failed: {}", text_of(&tabs));
    let tab_list = structured(&tabs)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        tab_list.len(),
        1,
        "the legacy tab space is the default context only: {tabs}"
    );
    assert_eq!(tab_list[0]["title"].as_str(), Some("Legacy Fixture"));
    for t in &tab_list {
        assert_ne!(
            t["title"].as_str(),
            Some("Ctx A"),
            "a context page must never be listed as a legacy tab: {tabs}"
        );
        assert_ne!(
            t["title"].as_str(),
            Some("Ctx B"),
            "a context page must never be listed as a legacy tab: {tabs}"
        );
    }

    let select_context_page = s.call(
        "browser_targets",
        json!({"action": "select", "target": alpha_page}),
    );
    assert!(
        is_error(&select_context_page),
        "legacy targets select must refuse a context page: {select_context_page}"
    );
    let guard = structured(&select_context_page);
    assert!(
        guard["code"].as_str().is_some_and(|c| !c.is_empty()),
        "a tool error carries a code: {guard}"
    );
    assert!(
        text_of(&select_context_page).contains("isolated browser context")
            && text_of(&select_context_page).contains("alpha"),
        "the refusal names the context, not just the target: {}",
        text_of(&select_context_page)
    );
    let legacy_after_refusal = s.call("browser_evaluate", json!({"expression": "document.title"}));
    assert_eq!(
        structured(&legacy_after_refusal)["value"].as_str(),
        Some("Legacy Fixture"),
        "a refused legacy select must not have attached to the context page: {legacy_after_refusal}"
    );

    let selected = ok(&mut s, json!({"action": "select", "name": "alpha"}));
    assert_eq!(selected["selected"].as_str(), Some("alpha"));
    assert_eq!(
        selected["legacyUnaffected"].as_bool(),
        Some(true),
        "selection must not claim to move the legacy tools: {selected}"
    );
    assert!(
        selected["selectedAffects"]
            .as_str()
            .unwrap_or("")
            .contains("legacy browser_* tools keep acting on the legacy default page"),
        "the scope is stated: {selected}"
    );

    let defaulted = ok(
        &mut s,
        json!({"action": "eval", "expression": "document.title"}),
    );
    assert_eq!(
        defaulted["name"].as_str(),
        Some("alpha"),
        "create left beta selected; select must have moved the default: {defaulted}"
    );
    assert_eq!(defaulted["value"].as_str(), Some("Ctx A"));

    let shown = ok(&mut s, json!({"action": "show"}));
    assert_eq!(shown["name"].as_str(), Some("alpha"));
    assert_eq!(shown["selected"].as_bool(), Some(true));
    assert_eq!(
        shown["pages"][0]["targetId"].as_str(),
        Some(alpha_page.as_str())
    );

    let legacy_title = s.call("browser_evaluate", json!({"expression": "document.title"}));
    assert_eq!(
        structured(&legacy_title)["value"].as_str(),
        Some("Legacy Fixture"),
        "a context selection must not move the legacy page"
    );

    let destroyed = ok(&mut s, json!({"action": "destroy", "name": "alpha"}));
    assert_eq!(destroyed["gone"].as_bool(), Some(true));
    assert_eq!(destroyed["name"].as_str(), Some("alpha"));
    assert_eq!(
        destroyed["disposedPages"].as_u64(),
        Some(1),
        "the receipt says how many of its pages went: {destroyed}"
    );
    assert!(
        destroyed["selected"].is_null(),
        "destroying the selected context clears the selection: {destroyed}"
    );
    assert_eq!(
        destroyed["evidence"]["pageTargets"].as_u64(),
        Some(0),
        "gone is proven, not assumed: {destroyed}"
    );
    assert!(
        destroyed["evidence"]["cookieProbe"]
            .as_str()
            .unwrap_or("")
            .contains("failed to find"),
        "the disposed id must be refused by the engine's own cookie probe: {destroyed}"
    );

    let remaining = ok(&mut s, json!({"action": "list"}));
    let remaining_list = remaining["contexts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        remaining_list.len(),
        1,
        "destroy tears down exactly one context: {remaining}"
    );
    assert_eq!(remaining_list[0]["name"].as_str(), Some("beta"));
    assert_eq!(
        remaining_list[0]["browserContextId"].as_str(),
        Some(beta_ctx.as_str())
    );
    assert!(
        remaining["selected"].is_null(),
        "no selection survives the destroyed one: {remaining}"
    );

    let only = ok(&mut s, json!({"action": "show"}));
    assert_eq!(
        only["name"].as_str(),
        Some("beta"),
        "the only remaining context resolves without a name: {only}"
    );
    let beta_still = ok(
        &mut s,
        json!({"action": "eval", "expression": "document.cookie"}),
    );
    assert_eq!(
        beta_still["value"].as_str(),
        Some("who=beta"),
        "the other context's jar and page must be untouched: {beta_still}"
    );

    let gone = failed(&mut s, json!({"action": "destroy", "name": "alpha"}));
    assert_eq!(
        (gone.0.as_str(), gone.1.as_str()),
        ("invalid_target", "not_found"),
        "a destroyed context is a typed not-found, not a silent success: {}",
        gone.2
    );

    let tabs_after = s.call("browser_tabs", json!({"action": "list"}));
    let tab_list_after = structured(&tabs_after)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        tab_list_after.len(),
        1,
        "context teardown moved no tab: {tabs_after}"
    );
    assert_eq!(tab_list_after[0]["title"].as_str(), Some("Legacy Fixture"));
    let legacy_after = s.call("browser_evaluate", json!({"expression": "document.cookie"}));
    assert!(
        structured(&legacy_after)["value"]
            .as_str()
            .unwrap_or("")
            .contains("legacy=1"),
        "the legacy page and its own cookie survive context teardown: {legacy_after}"
    );

    s.close_stdin();
    let status = s.wait_exit(Duration::from_secs(60));
    assert!(
        status.success(),
        "EOF shutdown with a live isolated context must exit 0, got {status:?}"
    );
}


/// H12: a page in a named context is reachable by `browser_snapshot` and
/// `browser_assert` through an EXPLICIT `context` argument, and only that way.
///
/// The contract this locks:
/// - `context: "<name>"` renders that context page's own AX tree / text and
///   runs assertions in its own realm; the reply names the context and the
///   page target it read;
/// - `page: "<targetId>"` picks one page when the context holds several; a
///   context with more than one page and no `page` is a typed
///   `invalid_args`/`ambiguous_target`, never a first-page guess;
/// - an unknown context or page is a typed `invalid_target`/`not_found` that
///   names what was asked for, and NEVER falls back to the legacy page;
/// - a call without `context` still reads the legacy default page, so a
///   context snapshot never moves the legacy selection.
#[test]
fn named_context_page_is_addressable_by_snapshot_and_assert() {
    require_pinned_chromium();
    let fixture = Fixture::start();
    let base = fixture.base.clone();
    let mut s = Session::spawn();

    let nav = navigate_with_retry(&mut s, &format!("{base}/legacy"));
    assert!(!is_error(&nav), "legacy navigate failed: {}", text_of(&nav));
    assert_eq!(
        structured(&nav)["title"].as_str(),
        Some("Legacy Fixture"),
        "the legacy page must be the fixture's legacy route"
    );

    let alpha = ok(
        &mut s,
        json!({"action": "create", "name": "alpha", "url": format!("{base}/a")}),
    );
    assert_eq!(alpha["title"].as_str(), Some("Ctx A"));
    let alpha_page = alpha["page"]["targetId"]
        .as_str()
        .expect("create reports its page target")
        .to_string();

    let snap = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert!(
        !is_error(&snap),
        "browser_snapshot must reach a named context page: {}",
        text_of(&snap)
    );
    let sc = structured(&snap);
    assert_eq!(
        sc["title"].as_str(),
        Some("Ctx A"),
        "the snapshot must be the context page's own document: {sc}"
    );
    assert_eq!(
        sc["context"].as_str(),
        Some("alpha"),
        "the reply names the context it read: {sc}"
    );
    assert_eq!(
        sc["target"].as_str(),
        Some(alpha_page.as_str()),
        "the reply names the page target it read: {sc}"
    );
    let nodes = sc["nodes"].as_array().cloned().unwrap_or_default();
    assert!(
        nodes.iter().any(|n| {
            n["role"].as_str() == Some("heading") && n["name"].as_str() == Some("Ctx A")
        }),
        "the context page's own AX nodes must be rendered: {sc}"
    );
    assert!(
        text_of(&snap).contains("Ctx A"),
        "the rendered text is the context page's: {}",
        text_of(&snap)
    );

    let text = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "mode": "text"}),
    );
    assert!(
        !is_error(&text),
        "text-mode snapshot must reach a named context page: {}",
        text_of(&text)
    );
    let tsc = structured(&text);
    assert_eq!(tsc["context"].as_str(), Some("alpha"), "{tsc}");
    assert!(
        tsc["text"].as_str().unwrap_or("").contains("Ctx A"),
        "the text is the context page's own body text: {tsc}"
    );

    let legacy_after = s.call("browser_snapshot", json!({}));
    assert_eq!(
        structured(&legacy_after)["title"].as_str(),
        Some("Legacy Fixture"),
        "a context snapshot must not move the legacy snapshot: {}",
        text_of(&legacy_after)
    );
    let legacy_title = s.call("browser_evaluate", json!({"expression": "document.title"}));
    assert_eq!(
        structured(&legacy_title)["value"].as_str(),
        Some("Legacy Fixture"),
        "a context snapshot must not move the legacy page: {legacy_title}"
    );

    let unknown = s.call("browser_snapshot", json!({"context": "nope"}));
    assert!(
        is_error(&unknown),
        "an unknown context must be refused, not answered from the legacy page: {}",
        text_of(&unknown)
    );
    assert_eq!(
        (
            structured(&unknown)["code"].as_str(),
            structured(&unknown)["reason"].as_str()
        ),
        (Some("invalid_target"), Some("not_found")),
        "an unknown context is a typed not-found: {}",
        text_of(&unknown)
    );
    assert!(
        text_of(&unknown).contains("nope"),
        "the refusal names the context asked for: {}",
        text_of(&unknown)
    );

    let checks = [
        (
            "url",
            json!({"context": "alpha", "kind": "url", "expected": {"contains": "/a"}}),
            true,
        ),
        (
            "dom",
            json!({"context": "alpha", "kind": "dom", "target": "#next",
                   "expected": {"exists": true, "text": "legacy"}}),
            true,
        ),
        (
            "text",
            json!({"context": "alpha", "kind": "text", "expected": {"contains": "Ctx A"}}),
            true,
        ),
        (
            "ax",
            json!({"context": "alpha", "kind": "ax",
                   "target": "role=heading[name=\"Ctx A\"]",
                   "expected": {"role": "heading", "name": "Ctx A"}}),
            true,
        ),
    ];
    for (kind, args, want) in checks {
        let resp = s.call("browser_assert", args);
        assert!(
            !is_error(&resp),
            "browser_assert kind {kind} must run on the named context page: {}",
            text_of(&resp)
        );
        let asc = structured(&resp);
        assert_eq!(
            asc["evidence"]["kind"].as_str(),
            Some(kind),
            "the evidence names the kind: {asc}"
        );
        let got = match asc["verdict"].as_bool() {
            Some(b) => b,
            None => asc["verdict"].as_str() == Some("pass"),
        };
        assert_eq!(got, want, "assert kind {kind} on the context page: {asc}");
    }

    let bad_assert = s.call(
        "browser_assert",
        json!({"context": "nope", "kind": "url", "expected": {"contains": "/"}}),
    );
    assert!(
        is_error(&bad_assert),
        "an assert on an unknown context must be refused: {}",
        text_of(&bad_assert)
    );
    assert_eq!(
        (
            structured(&bad_assert)["code"].as_str(),
            structured(&bad_assert)["reason"].as_str()
        ),
        (Some("invalid_target"), Some("not_found")),
        "an unknown context is a typed not-found: {}",
        text_of(&bad_assert)
    );

    let opened = ok(
        &mut s,
        json!({"action": "open", "name": "alpha", "url": format!("{base}/b")}),
    );
    let second_page = opened["page"]["targetId"]
        .as_str()
        .expect("open reports its page target")
        .to_string();
    let ambiguous = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert!(
        is_error(&ambiguous),
        "two pages in one context must not be guessed from: {}",
        text_of(&ambiguous)
    );
    assert_eq!(
        (
            structured(&ambiguous)["code"].as_str(),
            structured(&ambiguous)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("ambiguous_target")),
        "the ambiguity is typed: {}",
        text_of(&ambiguous)
    );
    let prose = text_of(&ambiguous);
    assert!(
        prose.contains(&alpha_page) && prose.contains(&second_page),
        "the refusal lists every page it refused to choose between: {prose}"
    );

    let pinned_page = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "page": alpha_page}),
    );
    assert!(
        !is_error(&pinned_page),
        "an explicit page must select that page: {}",
        text_of(&pinned_page)
    );
    assert_eq!(
        structured(&pinned_page)["title"].as_str(),
        Some("Ctx A"),
        "the pinned page is the one read: {}",
        text_of(&pinned_page)
    );

    let wrong_page = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "page": "not-a-target"}),
    );
    assert!(
        is_error(&wrong_page),
        "an unknown page in a known context must be refused: {}",
        text_of(&wrong_page)
    );
    assert_eq!(
        (
            structured(&wrong_page)["code"].as_str(),
            structured(&wrong_page)["reason"].as_str()
        ),
        (Some("invalid_target"), Some("not_found")),
        "an unknown page is a typed not-found: {}",
        text_of(&wrong_page)
    );

    let legacy_final = s.call("browser_evaluate", json!({"expression": "document.title"}));
    assert_eq!(
        structured(&legacy_final)["value"].as_str(),
        Some("Legacy Fixture"),
        "no context call moved the legacy page: {legacy_final}"
    );
}

/// Cold-review FAIL on 9e6ac5e: a PRESENT but wrong-typed `context`/`page`
/// (a number, null, a boolean) was read with `get().and_then(as_str)`, so it
/// silently became "absent" and the call answered from another page - the
/// legacy page for `context`, or the context's only page for `page`. A value
/// the caller sent must be honoured or refused, never dropped.
///
/// This also locks the two scope guards: an offset:>0 page cannot be sliced
/// from another realm's cached render, and a legacy `@eN` with `context=` is a
/// typed argument error on the wire (not an untyped cd engine failure). An
/// empty string names nothing, so it is refused the same way, and the SAME
/// realm still continues: after a render, `offset` 1 returns that realm's own
/// document with the same `snapshotId`, not merely "no error".
#[test]
fn context_target_refuses_wrong_typed_values_and_cross_scope_paging() {
    require_pinned_chromium();
    let fixture = Fixture::start();
    let base = fixture.base.clone();
    let mut s = Session::spawn();

    let nav = navigate_with_retry(&mut s, &format!("{base}/legacy"));
    assert!(!is_error(&nav), "legacy navigate failed: {}", text_of(&nav));
    let alpha = ok(
        &mut s,
        json!({"action": "create", "name": "alpha", "url": format!("{base}/a")}),
    );
    let alpha_page = alpha["page"]["targetId"]
        .as_str()
        .expect("create reports its page target")
        .to_string();

    // Both realms render first, so a silent fallback later is visible as a
    // success with the wrong document's title.
    let good_ctx = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert!(
        !is_error(&good_ctx),
        "the valid context snapshot must work before its refusals are tested: {}",
        text_of(&good_ctx)
    );
    let ctx_sc = structured(&good_ctx).clone();
    assert_eq!(ctx_sc["title"].as_str(), Some("Ctx A"));
    let good_legacy = s.call("browser_snapshot", json!({}));
    let legacy_sc = structured(&good_legacy).clone();
    assert_eq!(legacy_sc["title"].as_str(), Some("Legacy Fixture"));

    // 1. A number where a context name belongs is not "no context".
    let numeric = s.call("browser_snapshot", json!({"context": 5}));
    assert!(
        is_error(&numeric),
        "a wrong-typed 'context' must be refused, not answered from the legacy page: {}",
        text_of(&numeric)
    );
    assert_eq!(
        (
            structured(&numeric)["code"].as_str(),
            structured(&numeric)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a wrong-typed context is a typed argument error: {}",
        text_of(&numeric)
    );
    assert!(
        text_of(&numeric).contains("context"),
        "the refusal names the argument: {}",
        text_of(&numeric)
    );

    // 2. An explicit null is present, not absent.
    let nulled = s.call("browser_snapshot", json!({"context": null}));
    assert!(
        is_error(&nulled),
        "a null 'context' must be refused, not answered from the legacy page: {}",
        text_of(&nulled)
    );
    assert_eq!(
        (
            structured(&nulled)["code"].as_str(),
            structured(&nulled)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a null context is a typed argument error: {}",
        text_of(&nulled)
    );

    // 3. A wrong-typed page must not fall through to the context's only page.
    let bad_page = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "page": 5}),
    );
    assert!(
        is_error(&bad_page),
        "a wrong-typed 'page' must be refused, not answered from the only page: {}",
        text_of(&bad_page)
    );
    assert_eq!(
        (
            structured(&bad_page)["code"].as_str(),
            structured(&bad_page)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a wrong-typed page is a typed argument error: {}",
        text_of(&bad_page)
    );
    assert!(
        text_of(&bad_page).contains("page"),
        "the refusal names the argument: {}",
        text_of(&bad_page)
    );

    // 3b. An empty string names nothing, so it is a typed refusal too - on
    //     snapshot and on assert, for context and for page.
    let empty_ctx = s.call("browser_snapshot", json!({"context": ""}));
    assert!(
        is_error(&empty_ctx),
        "an empty 'context' must be refused, not answered from the legacy page: {}",
        text_of(&empty_ctx)
    );
    assert_eq!(
        (
            structured(&empty_ctx)["code"].as_str(),
            structured(&empty_ctx)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "an empty context is a typed argument error: {}",
        text_of(&empty_ctx)
    );
    let empty_page = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "page": ""}),
    );
    assert!(
        is_error(&empty_page),
        "an empty 'page' must be refused, not answered from the only page: {}",
        text_of(&empty_page)
    );
    assert_eq!(
        (
            structured(&empty_page)["code"].as_str(),
            structured(&empty_page)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "an empty page is a typed argument error: {}",
        text_of(&empty_page)
    );

    // 4. The same rules on browser_assert, where a wrong page would mint a
    //    false verdict instead of a wrong snapshot.
    let assert_null = s.call(
        "browser_assert",
        json!({"context": null, "kind": "url", "expected": {"contains": "/a"}}),
    );
    assert!(
        is_error(&assert_null),
        "a null 'context' must be refused, not asserted against the legacy page: {}",
        text_of(&assert_null)
    );
    assert_eq!(
        (
            structured(&assert_null)["code"].as_str(),
            structured(&assert_null)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a null context is a typed argument error: {}",
        text_of(&assert_null)
    );
    let assert_page = s.call(
        "browser_assert",
        json!({"context": "alpha", "page": false, "kind": "url", "expected": {"contains": "/a"}}),
    );
    assert!(
        is_error(&assert_page),
        "a wrong-typed 'page' must be refused, not asserted against the only page: {}",
        text_of(&assert_page)
    );
    assert_eq!(
        (
            structured(&assert_page)["code"].as_str(),
            structured(&assert_page)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a wrong-typed page is a typed argument error: {}",
        text_of(&assert_page)
    );
    let assert_empty_ctx = s.call(
        "browser_assert",
        json!({"context": "", "kind": "url", "expected": {"contains": "/"}}),
    );
    assert!(
        is_error(&assert_empty_ctx),
        "an empty 'context' must be refused, not asserted against the legacy page: {}",
        text_of(&assert_empty_ctx)
    );
    assert_eq!(
        (
            structured(&assert_empty_ctx)["code"].as_str(),
            structured(&assert_empty_ctx)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "an empty context is a typed argument error: {}",
        text_of(&assert_empty_ctx)
    );
    let assert_empty_page = s.call(
        "browser_assert",
        json!({"context": "alpha", "page": "", "kind": "url", "expected": {"contains": "/a"}}),
    );
    assert!(
        is_error(&assert_empty_page),
        "an empty 'page' must be refused, not asserted against the only page: {}",
        text_of(&assert_empty_page)
    );
    assert_eq!(
        (
            structured(&assert_empty_page)["code"].as_str(),
            structured(&assert_empty_page)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "an empty page is a typed argument error: {}",
        text_of(&assert_empty_page)
    );

    // 5. A legacy @eN ref cannot be asserted in a context page; that is an
    //    argument error, typed on the wire like every other refusal.
    let ref_ctx = s.call(
        "browser_assert",
        json!({"context": "alpha", "kind": "dom", "target": "@e1",
               "expected": {"exists": true}}),
    );
    assert!(
        is_error(&ref_ctx),
        "a legacy ref must be refused in a context page: {}",
        text_of(&ref_ctx)
    );
    assert_eq!(
        (
            structured(&ref_ctx)["code"].as_str(),
            structured(&ref_ctx)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("bad_value")),
        "a legacy ref with context= is a typed argument error, never an untyped failure: {}",
        text_of(&ref_ctx)
    );

    // 6. Same-realm continuation is a REAL continuation, not just "no error":
    //    after a render, offset 1 returns that realm's own document, keeps the
    //    same snapshotId, and names the same realm it reads.
    let ctx_render = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert!(
        !is_error(&ctx_render),
        "the context realm must re-render before its continuation: {}",
        text_of(&ctx_render)
    );
    let ctx_render_sc = structured(&ctx_render).clone();
    let ctx_id = ctx_render_sc["snapshotId"]
        .as_str()
        .expect("a context render returns a snapshotId")
        .to_string();
    let ctx_total = ctx_render_sc["totalLines"]
        .as_u64()
        .expect("a render reports its totalLines");
    assert!(
        ctx_total > 1,
        "the fixture render needs more than one line for a real window: {ctx_render_sc}"
    );
    let ctx_next = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "offset": 1, "snapshotId": ctx_id}),
    );
    assert!(
        !is_error(&ctx_next),
        "a context render must page in its own realm: {}",
        text_of(&ctx_next)
    );
    let ctx_next_sc = structured(&ctx_next).clone();
    assert_eq!(
        ctx_next_sc["offset"].as_u64(),
        Some(1),
        "the continuation reports the offset it read: {ctx_next_sc}"
    );
    assert_eq!(
        ctx_next_sc["snapshotId"].as_str(),
        Some(ctx_id.as_str()),
        "the continuation slices the render it was given: {ctx_next_sc}"
    );
    assert_eq!(
        ctx_next_sc["totalLines"].as_u64(),
        Some(ctx_total),
        "the continuation keeps the original render's line count: {ctx_next_sc}"
    );
    // The window is real content, not just metadata: offset 1 starts where
    // line 1 of that render started (the header line 0 is gone).
    let ctx_first_line = text_of(&ctx_next)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let ctx_render_line1 = text_of(&ctx_render)
        .lines()
        .nth(1)
        .unwrap_or("")
        .to_string();
    assert!(!ctx_render_line1.is_empty(), "the render has a second line");
    assert_eq!(
        ctx_first_line, ctx_render_line1,
        "offset 1 returns the shifted window, not the whole render again: {}",
        text_of(&ctx_next)
    );
    assert_eq!(
        ctx_next_sc["title"].as_str(),
        Some("Ctx A"),
        "the continuation reads the context page's document: {ctx_next_sc}"
    );
    assert_eq!(
        ctx_next_sc["context"].as_str(),
        Some("alpha"),
        "the continuation names its realm: {ctx_next_sc}"
    );
    assert_eq!(
        ctx_next_sc["target"].as_str(),
        Some(alpha_page.as_str()),
        "the continuation names the page it read: {ctx_next_sc}"
    );

    let legacy_render = s.call("browser_snapshot", json!({}));
    assert!(
        !is_error(&legacy_render),
        "the legacy realm must re-render before its continuation: {}",
        text_of(&legacy_render)
    );
    let legacy_render_sc = structured(&legacy_render).clone();
    let legacy_id = legacy_render_sc["snapshotId"]
        .as_str()
        .expect("a legacy render returns a snapshotId")
        .to_string();
    let legacy_total = legacy_render_sc["totalLines"]
        .as_u64()
        .expect("a render reports its totalLines");
    assert!(
        legacy_total > 1,
        "the fixture render needs more than one line for a real window: {legacy_render_sc}"
    );
    let legacy_next = s.call(
        "browser_snapshot",
        json!({"offset": 1, "snapshotId": legacy_id}),
    );
    assert!(
        !is_error(&legacy_next),
        "a legacy render must page in its own realm: {}",
        text_of(&legacy_next)
    );
    let legacy_next_sc = structured(&legacy_next).clone();
    assert_eq!(
        legacy_next_sc["offset"].as_u64(),
        Some(1),
        "the continuation reports the offset it read: {legacy_next_sc}"
    );
    assert_eq!(
        legacy_next_sc["snapshotId"].as_str(),
        Some(legacy_id.as_str()),
        "the continuation slices the render it was given: {legacy_next_sc}"
    );
    assert_eq!(
        legacy_next_sc["totalLines"].as_u64(),
        Some(legacy_total),
        "the continuation keeps the original render's line count: {legacy_next_sc}"
    );
    let legacy_first_line = text_of(&legacy_next)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    let legacy_render_line1 = text_of(&legacy_render)
        .lines()
        .nth(1)
        .unwrap_or("")
        .to_string();
    assert!(
        !legacy_render_line1.is_empty(),
        "the render has a second line"
    );
    assert_eq!(
        legacy_first_line, legacy_render_line1,
        "offset 1 returns the shifted window, not the whole render again: {}",
        text_of(&legacy_next)
    );
    assert_eq!(
        legacy_next_sc["title"].as_str(),
        Some("Legacy Fixture"),
        "the continuation reads the legacy page's document: {legacy_next_sc}"
    );
    assert!(
        legacy_next_sc.get("context").is_none_or(Value::is_null)
            && legacy_next_sc.get("target").is_none_or(Value::is_null),
        "a legacy continuation names no context realm: {legacy_next_sc}"
    );

    // 7. Paging is per realm: a cached render from one space cannot be sliced
    //    by a call aimed at the other, in either direction. The cache now holds
    //    the legacy render, so the context call is the one that must refuse.
    let ctx_pages_legacy = s.call(
        "browser_snapshot",
        json!({"context": "alpha", "page": alpha_page, "offset": 1,
               "snapshotId": legacy_id}),
    );
    assert!(
        is_error(&ctx_pages_legacy),
        "a context call must not page a legacy render: {}",
        text_of(&ctx_pages_legacy)
    );
    assert_eq!(
        (
            structured(&ctx_pages_legacy)["code"].as_str(),
            structured(&ctx_pages_legacy)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("snapshot_expired")),
        "cross-scope paging is a typed expired render: {}",
        text_of(&ctx_pages_legacy)
    );
    // Now re-render the context realm so its id IS the cached render, and aim
    // the legacy call at it: the refusal must come from the scope, not from a
    // stale id.
    let ctx_again = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert!(!is_error(&ctx_again), "{}", text_of(&ctx_again));
    let ctx_id2 = structured(&ctx_again)["snapshotId"]
        .as_str()
        .expect("a context render returns a snapshotId")
        .to_string();
    let legacy_pages_ctx = s.call(
        "browser_snapshot",
        json!({"offset": 1, "snapshotId": ctx_id2}),
    );
    assert!(
        is_error(&legacy_pages_ctx),
        "a legacy call must not page a context render: {}",
        text_of(&legacy_pages_ctx)
    );
    assert_eq!(
        (
            structured(&legacy_pages_ctx)["code"].as_str(),
            structured(&legacy_pages_ctx)["reason"].as_str()
        ),
        (Some("invalid_args"), Some("snapshot_expired")),
        "cross-scope paging is a typed expired render: {}",
        text_of(&legacy_pages_ctx)
    );

    // 8. Nothing above moved either page.
    let legacy_after = s.call("browser_snapshot", json!({}));
    assert_eq!(
        structured(&legacy_after)["title"].as_str(),
        Some("Legacy Fixture"),
        "the legacy page still answers its own snapshot: {}",
        text_of(&legacy_after)
    );
    let ctx_after = s.call("browser_snapshot", json!({"context": "alpha"}));
    assert_eq!(
        structured(&ctx_after)["title"].as_str(),
        Some("Ctx A"),
        "the context page still answers its own snapshot: {}",
        text_of(&ctx_after)
    );
}

/// An engine this test launched but hand does not own: it stands in for
/// "someone else's browser" that a client attaches to.
struct ExternalEngine {
    child: std::process::Child,
    profile: std::path::PathBuf,
    base: String,
}

impl ExternalEngine {
    fn start() -> Self {
        let chromium = require_pinned_chromium();
        let profile =
            std::env::temp_dir().join(format!("hand-context-wire-attached-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).expect("external profile dir");
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").expect("free port");
            l.local_addr().expect("addr").port()
        };
        let child = std::process::Command::new(&chromium)
            .args([
                "--headless".to_string(),
                format!("--remote-debugging-port={port}"),
                format!("--user-data-dir={}", profile.display()),
                "--no-first-run".to_string(),
                "--no-default-browser-check".to_string(),
                format!("--remote-allow-origins=http://127.0.0.1:{port}"),
                "about:blank".to_string(),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the external engine");
        let base = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if http_get(&base, "/json/version").contains("webSocketDebuggerUrl") {
                return ExternalEngine {
                    child,
                    profile,
                    base,
                };
            }
            if Instant::now() >= deadline {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_dir_all(&profile);
                panic!("the external engine did not become ready on {base}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for ExternalEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

fn http_get(base: &str, path: &str) -> String {
    let addr = base.trim_start_matches("http://").to_string();
    let Ok(mut stream) = TcpStream::connect(&addr) else {
        return String::new();
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return String::new();
    }
    let mut out = String::new();
    let _ = BufReader::new(stream).read_to_string(&mut out);
    out
}

/// Two independent hand clients, one external engine: this is the wire-level
/// shape of "shutdown closes owned only". The guest's EOF shutdown must leave
/// the owner's context, page and cookie working and the engine process running.
#[test]
fn shutdown_closes_only_the_contexts_hand_owns() {
    require_pinned_chromium();
    let mut engine = ExternalEngine::start();
    let fixture = Fixture::start();
    let base = fixture.base.clone();

    let mut owner = Session::spawn_with_env(&[("GLADIUS_CDP_URL", engine.base.as_str())]);
    let owner_ctx = ok(
        &mut owner,
        json!({"action": "create", "name": "owner", "url": format!("{base}/a")}),
    );
    assert!(
        owner_ctx["engine"]
            .as_str()
            .unwrap_or("")
            .contains("external-cdp"),
        "the owner session must be attached to the external engine, not launch its own: {owner_ctx}"
    );
    let owner_cookie = ok(
        &mut owner,
        json!({"action": "cookies", "name": "owner", "cookies": [
            {"name": "who", "value": "owner", "url": format!("{base}/")}
        ]}),
    );
    assert!(jar_has(&owner_cookie, "who", "owner"), "{owner_cookie}");

    let mut guest = Session::spawn_with_env(&[("GLADIUS_CDP_URL", engine.base.as_str())]);
    let guest_ctx = ok(
        &mut guest,
        json!({"action": "create", "name": "guest", "url": format!("{base}/b")}),
    );
    assert!(
        guest_ctx["engine"]
            .as_str()
            .unwrap_or("")
            .contains("external-cdp"),
        "the guest session must be attached too, or this test proves nothing: {guest_ctx}"
    );
    assert_eq!(guest_ctx["title"].as_str(), Some("Ctx B"));
    let set_guest = ok(
        &mut guest,
        json!({"action": "cookies", "name": "guest", "cookies": [
            {"name": "who", "value": "guest", "url": format!("{base}/")}
        ]}),
    );
    assert!(jar_has(&set_guest, "who", "guest"), "{set_guest}");
    let guest_cookie = ok(
        &mut guest,
        json!({"action": "eval", "name": "guest", "expression": "document.cookie"}),
    );
    assert_eq!(
        guest_cookie["value"].as_str(),
        Some("who=guest"),
        "the guest's cookie is in its own partition: {guest_cookie}"
    );

    guest.close_stdin();
    let guest_status = guest.wait_exit(Duration::from_secs(60));
    assert!(
        guest_status.success(),
        "attached EOF shutdown must exit 0, got {guest_status:?}"
    );

    let owner_still = ok(
        &mut owner,
        json!({"action": "eval", "name": "owner", "expression": "[document.title, document.cookie]"}),
    );
    assert_eq!(
        owner_still["value"],
        json!(["Ctx A", "who=owner"]),
        "another client's context, page and cookie must survive hand's shutdown: {owner_still}"
    );

    assert!(
        engine.child.try_wait().expect("try_wait engine").is_none(),
        "shutdown must not close an engine hand does not own"
    );
    assert!(
        http_get(&engine.base, "/json/version").contains("webSocketDebuggerUrl"),
        "the external engine must still answer after hand's shutdown"
    );

    owner.close_stdin();
    let owner_status = owner.wait_exit(Duration::from_secs(60));
    assert!(
        owner_status.success(),
        "the owner's own clean shutdown must exit 0, got {owner_status:?}"
    );
    assert!(
        engine.child.try_wait().expect("try_wait engine").is_none(),
        "the engine must still be running after both clients detached"
    );
}

/// A UI permission prompt is a REJECT-class call: it is refused before any CDP
/// command is sent, with code invalid_args and reason rejected.
#[test]
fn permission_prompt_is_a_rejected_call() {
    let mut s = Session::spawn();
    for args in [
        json!({"action": "permissions", "name": "x", "grant": ["geolocation"], "prompt": true}),
        json!({"action": "permissions", "name": "x", "grant": ["geolocation"], "mode": "prompt"}),
    ] {
        let (code, reason, text) = failed(&mut s, args);
        assert_eq!(
            (code.as_str(), reason.as_str()),
            ("invalid_args", "rejected"),
            "a prompt must be rejected, never opened: {text}"
        );
        assert!(
            text.contains("prompt"),
            "the refusal must name the rejected prompt: {text}"
        );
    }
}
