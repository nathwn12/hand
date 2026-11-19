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

use common::{is_error, structured, text_of, Session};
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
                        let _ = serve(stream);
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
            "close",
            "cookies",
            "create",
            "destroy",
            "destroy_all",
            "eval",
            "list",
            "open",
            "select",
            "show",
        ],
        "the published action set must be the dispatched one"
    );

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

    let nav = s.call("browser_navigate", json!({"url": format!("{base}/legacy")}));
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
