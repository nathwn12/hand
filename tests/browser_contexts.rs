//! Live browser-context tests: named isolated contexts with separate cookies
//! and storage, explicit creation/selection/teardown, over the REAL owned
//! pinned `chrome-headless-shell`.
//!
//! What this file establishes, and what it deliberately does not:
//! - proven on a live engine: two contexts on the same origin do not see each
//!   other's cookies, `localStorage` or `sessionStorage`; a context-scoped
//!   cookie set is verified by identity (name + domain + path) and the
//!   requested attributes, not by name+value; teardown disposes only the named
//!   context and is proven gone; the legacy tab/selection space never adopts a
//!   context page, and `tabs select` plus realm-scoped `evaluate target=` both
//!   refuse one by name;
//! - measured teardown mechanics: `destroy` closes the context's pages with
//!   `pagehide` (Chrome 158 fires it and CDP cannot suppress it) but never
//!   `beforeunload`/`unload`, while `close` (closeTarget) runs both - the second
//!   is the control that proves the fixture's sink works;
//! - safety under failure, each with its own test: a cancelled `create` is
//!   still committed by its own task (no orphan context), a failed `open`
//!   closes the page it made, a failed disposal keeps the context tracked and
//!   its page excluded from legacy tabs, and a dead engine is never relaunched
//!   and never reported as "disposed by hand";
//! - the attached-engine case (opt-in, run alone) shows that with two
//!   independent CDP connections to one external browser, disposing or
//!   detaching one client's contexts leaves the other client's context, cookie
//!   and page untouched, and that a clean detach (`Browser::close` sends a
//!   WebSocket close frame) disposes hand's own contexts. A HARD KILL is not
//!   tested and not claimed: measured, an abrupt socket drop closed the
//!   context's pages but could leave the context object answering
//!   `Storage.getCookies` (see the residual in `contexts.rs`);
//! - NOT established here: IndexedDB/HTTP-cache isolation, process isolation
//!   (contexts share one browser process), network interception, per-context
//!   permissions, or dialog auto-handling inside a context. Those remain
//!   unimplemented or unverified and are not claimed.
//!
//! The tests drive `browser_engine::contexts::dispatch`, the exact hook the
//! server integrates, so the schema, envelopes and typed errors are the ones a
//! client will see.
//!
//! Environment hooks are process-wide, so every test here takes `serial()`
//! before touching one, and any hook is removed by `EnvHooks` on drop.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use browser_engine::contexts::{self, ContextRegistry};
use browser_engine::Browser;
use serde_json::{json, Value};

const TOOL: &str = "context";

const INDEX: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Context Fixture</title></head>
<body><h1>Context fixture</h1><p id="p">ready</p></body></html>"#;

/// Two routes with distinct document titles, so "which page answered?" can
/// never be answered by a cached target-info string that happens to match.
const PAGE_A: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Titled A</title></head>
<body><h1>Titled A</h1><p id="p">a</p></body></html>"#;

const PAGE_B: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Titled B</title></head>
<body><h1>Titled B</h1><p id="p">b</p></body></html>"#;

/// Served after a deliberate delay, so the target spends a measurable window as
/// the initial `about:blank` document (complete, URL `about:blank`) before the
/// requested page commits. That is the window a created context must not report
/// as "settled".
const PAGE_SLOW: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Slow Fixture</title></head>
<body><h1>Slow fixture</h1><p id="p">slow</p></body></html>"#;

/// A page that navigates ITSELF to `/b` after a delay: a real document change
/// that hand does not drive, so a cached title must be replaced.
const PAGE_AUTO: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Auto A</title></head>
<body><h1>Auto A</h1>
<script>setTimeout(function () { location.href = '/b'; }, 1200);</script></body></html>"#;

/// A page that reports its own teardown handlers through the fixture's sink, so
/// "did the engine run them?" is observed, not assumed.
const TEARDOWN: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Teardown Fixture</title></head>
<body><h1>Teardown fixture</h1>
<script>
  function report(name) { navigator.sendBeacon('/seen/' + name, '1'); }
  addEventListener('beforeunload', function () { report('beforeunload'); });
  addEventListener('pagehide', function () { report('pagehide'); });
  addEventListener('unload', function () { report('unload'); });
</script></body></html>"#;

/// Environment hooks are process-wide; serialise every test that can touch
/// them or set `GLADIUS_CDP_URL`.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set environment hooks for one test and remove them on drop (also on panic).
struct EnvHooks(Vec<&'static str>);

impl EnvHooks {
    fn new(vars: &[(&'static str, &str)]) -> Self {
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        EnvHooks(vars.iter().map(|(k, _)| *k).collect())
    }
}

impl Drop for EnvHooks {
    fn drop(&mut self) {
        for k in &self.0 {
            std::env::remove_var(k);
        }
    }
}

struct Fixture {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let flag = stop.clone();
        let sink = seen.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let sink = sink.clone();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &sink);
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
            seen,
            stop,
            handle: Some(handle),
        }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn clear_seen(&self) {
        self.seen.lock().unwrap().clear();
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

fn serve(mut stream: TcpStream, seen: &Arc<Mutex<Vec<String>>>) -> std::io::Result<()> {
    // Accepted sockets inherit the listener's nonblocking mode on Windows.
    // Without this, a read before the request arrives returns WouldBlock and
    // drops the stream, which Chrome sees as ERR_CONNECTION_ABORTED.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    if let Some(name) = path.strip_prefix("/seen/") {
        seen.lock().unwrap().push(name.to_string());
        let body = "ok";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes())?;
        return stream.flush();
    }
    if path.starts_with("/slow") {
        std::thread::sleep(Duration::from_millis(400));
    }
    let body = match path {
        "/teardown" => TEARDOWN,
        p if p.starts_with("/auto") => PAGE_AUTO,
        p if p.starts_with("/slow") => PAGE_SLOW,
        p if p.starts_with("/a") => PAGE_A,
        p if p.starts_with("/b") => PAGE_B,
        _ => INDEX,
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(f)
}

/// The pinned browser must be provisioned; the engine's own message otherwise.
fn require_owned_browser() {
    let chromium = browser_engine::chromium_path()
        .expect("the pinned chrome-headless-shell must be provisioned for this live test");
    assert!(
        chromium.is_file(),
        "chromium path is not a file: {chromium:?}"
    );
    eprintln!("HAND_BROWSER_BIN={}", chromium.display());
}

struct Rig {
    browser: Arc<Browser>,
    reg: Arc<ContextRegistry>,
}

impl Rig {
    fn start() -> Self {
        let browser = Arc::new(Browser::new());
        let reg = Arc::new(ContextRegistry::new(Arc::clone(&browser)));
        Rig { browser, reg }
    }

    async fn call(&self, args: Value) -> Value {
        contexts::dispatch(TOOL, &args, &self.reg)
            .await
            .unwrap_or_else(|| panic!("dispatch must claim the context tool: {args}"))
    }

    /// Pages the engine reports inside one context, straight from CDP (not from
    /// hand's bookkeeping).
    async fn engine_pages(&self, context_id: &str) -> usize {
        let targets = self
            .browser
            .cdp_raw("Target.getTargets", json!({}), None)
            .await
            .expect("getTargets");
        targets
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
            .unwrap_or(0)
    }

    async fn close(self) {
        self.browser.close().await.expect("close engine");
    }
}

fn sc(envelope: &Value) -> &Value {
    envelope
        .get("structuredContent")
        .unwrap_or_else(|| panic!("expected structuredContent, got: {envelope}"))
}

fn is_error(envelope: &Value) -> bool {
    envelope
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn text_of(envelope: &Value) -> String {
    envelope
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// `(code, reason)` of a typed failure.
fn typed_code(envelope: &Value) -> (String, String) {
    let s = sc(envelope);
    (
        s.get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        s.get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    )
}

fn listed<'a>(envelope: &'a Value, name: &str) -> Option<&'a Value> {
    sc(envelope)["contexts"]
        .as_array()?
        .iter()
        .find(|c| c["name"].as_str() == Some(name))
}

async fn wait_for_phase(rig: &Rig, name: &str, want: &str, cap: Duration) -> Option<Value> {
    let deadline = Instant::now() + cap;
    loop {
        let list = rig.call(json!({"action": "list"})).await;
        if let Some(entry) = listed(&list, name) {
            if entry["phase"].as_str() == Some(want) {
                return Some(entry.clone());
            }
        }
        if Instant::now() >= deadline {
            eprintln!("phase {want} never observed for {name}: {list}");
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The live page URL of a context page, read from its own session.
async fn page_url(rig: &Rig, name: &str) -> String {
    let r = rig
        .call(json!({"action": "eval", "name": name, "expression": "location.href"}))
        .await;
    assert!(!is_error(&r), "eval location.href failed: {}", text_of(&r));
    sc(&r)["value"].as_str().unwrap_or("").to_string()
}

/// Wait for the LEGACY active page to be the expected document, returning the
/// last observation so a failure explains itself.
///
/// One postcondition (`document.title`), one deadline. The old version polled
/// the same thing from a 10 s stopwatch; a loaded machine can outlast that while
/// the page is still loading (14 CPU burners stretched the wait to 8.2 s of the
/// 10 s budget, and 22 reproduced the failure), so the budget is a precondition
/// allowance rather than a measurement: the poll returns as soon as the title
/// matches, normally in well under a second.
async fn wait_legacy_page(rig: &Rig, want: &str, cap: Duration) -> Result<(), String> {
    let deadline = Instant::now() + cap;
    let mut last = String::from("no evaluate completed");
    loop {
        let r = browser_engine::call(
            "evaluate",
            &json!({"expression": "document.title"}),
            &rig.browser,
        )
        .await;
        if is_error(&r) {
            last = format!("evaluate failed: {}", text_of(&r));
        } else {
            let title = sc(&r)["value"].as_str().unwrap_or("").to_string();
            if title == want {
                return Ok(());
            }
            last = format!("document.title is {title:?}, want {want:?}");
        }
        if Instant::now() >= deadline {
            return Err(format!("{last} (gave up after {}s)", cap.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A freshly launched engine can still be committing its initial about:blank
/// when the first raw `Page.navigate` lands; the requested page then never
/// commits and the title stays empty (measured on a cold hosted runner: a
/// 30 s title poll gave up twice while 21 sibling tests passed). Navigate,
/// poll the title, re-navigate boundedly: a cold start becomes deterministic
/// and a genuinely broken page still fails with its own last observation.
async fn open_legacy_page(rig: &Rig, url: &str, want: &str) {
    let mut last = String::from("no attempt completed");
    for attempt in 1..=3u8 {
        match rig
            .browser
            .command("Page.navigate", json!({"url": url}), true)
            .await
        {
            Err(e) => {
                last = format!("Page.navigate failed: {e}");
                eprintln!("LEGACY_RETRY attempt={attempt} url={url} {last}");
            }
            Ok(_) => match wait_legacy_page(rig, want, Duration::from_secs(30)).await {
                Ok(()) => return,
                Err(e) => {
                    last = e;
                    eprintln!("LEGACY_RETRY attempt={attempt} url={url} title never committed: {last}");
                }
            },
        }
    }
    panic!("the legacy fixture page never committed across 3 navigations: {last}");
}
/// Bounded poll of a context's refreshed page list for a page whose url
/// contains `needle`.
async fn wait_for_page(rig: &Rig, name: &str, needle: &str, cap: Duration) -> Option<Value> {
    let deadline = Instant::now() + cap;
    loop {
        let shown = rig.call(json!({"action": "show", "name": name})).await;
        if is_error(&shown) {
            return None;
        }
        if let Some(p) = sc(&shown)["pages"].as_array().and_then(|pages| {
            pages
                .iter()
                .find(|p| p["url"].as_str().unwrap_or("").contains(needle))
        }) {
            return Some(p.clone());
        }
        if Instant::now() >= deadline {
            eprintln!("pages never gained a {needle} page: {shown}");
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn assert_owned_engine() {
    if std::env::var("GLADIUS_CDP_URL").is_ok() {
        eprintln!(
            "NOTE: GLADIUS_CDP_URL is set; this run exercises an attached engine, not the owned \
             pinned browser"
        );
        return;
    }
    let info = browser_engine::engine_info();
    assert_eq!(info["backend"].as_str(), Some("chromium"));
    assert_eq!(info["caps"].as_str(), Some("render"));
}

#[test]
fn two_contexts_have_separate_cookies_and_storage() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    block_on(async {
        let rig = Rig::start();
        assert_owned_engine();

        let a = rig
            .call(json!({"action": "create", "name": "alpha", "url": format!("{}/", fixture.base)}))
            .await;
        assert!(!is_error(&a), "create alpha failed: {}", text_of(&a));
        let a_sc = sc(&a);
        assert_eq!(a_sc["ok"].as_bool(), Some(true));
        assert_eq!(a_sc["name"].as_str(), Some("alpha"));
        let alpha_ctx = a_sc["browserContextId"]
            .as_str()
            .expect("a real browserContextId")
            .to_string();
        assert!(!alpha_ctx.is_empty());
        assert_eq!(
            a_sc["settled"].as_bool(),
            Some(true),
            "the fixture page must settle: {a_sc}"
        );
        assert_eq!(a_sc["readyState"].as_str(), Some("complete"));
        let alpha_target = a_sc["page"]["targetId"]
            .as_str()
            .expect("the context's page targetId")
            .to_string();

        let b = rig
            .call(json!({"action": "create", "name": "beta", "url": format!("{}/", fixture.base)}))
            .await;
        assert!(!is_error(&b), "create beta failed: {}", text_of(&b));
        let beta_ctx = sc(&b)["browserContextId"].as_str().unwrap().to_string();
        assert_ne!(
            alpha_ctx, beta_ctx,
            "each context is a distinct CDP context"
        );

        assert_eq!(page_url(&rig, "alpha").await, format!("{}/", fixture.base));
        assert_eq!(page_url(&rig, "beta").await, format!("{}/", fixture.base));

        let seeded = rig
            .call(json!({"action": "eval", "name": "alpha", "expression":
                "document.cookie='who=alpha; path=/'; localStorage.setItem('who','alpha'); \
                 sessionStorage.setItem('who','alpha'); document.cookie"}))
            .await;
        assert!(
            !is_error(&seeded),
            "seed alpha failed: {}",
            text_of(&seeded)
        );
        assert_eq!(sc(&seeded)["value"].as_str(), Some("who=alpha"));

        let beta_view = rig
            .call(json!({"action": "eval", "name": "beta", "expression":
                "JSON.stringify([document.cookie, localStorage.getItem('who'), \
                 sessionStorage.getItem('who')])"}))
            .await;
        assert!(
            !is_error(&beta_view),
            "read beta failed: {}",
            text_of(&beta_view)
        );
        assert_eq!(
            sc(&beta_view)["value"].as_str(),
            Some("[\"\",null,null]"),
            "a second context must not see alpha's cookie or storage: {beta_view}"
        );

        let alpha_view = rig
            .call(json!({"action": "eval", "name": "alpha", "expression":
                "JSON.stringify([document.cookie, localStorage.getItem('who'), \
                 sessionStorage.getItem('who')])"}))
            .await;
        assert_eq!(
            sc(&alpha_view)["value"].as_str(),
            Some("[\"who=alpha\",\"alpha\",\"alpha\"]")
        );

        let jars = rig
            .call(json!({"action": "cookies", "name": "alpha"}))
            .await;
        assert!(!is_error(&jars), "cookies read failed: {}", text_of(&jars));
        let cookies = sc(&jars)["cookies"].as_array().cloned().unwrap_or_default();
        assert!(
            cookies
                .iter()
                .any(|c| c["name"].as_str() == Some("who") && c["value"].as_str() == Some("alpha")),
            "alpha's jar carries its cookie: {jars}"
        );
        let beta_jars = rig.call(json!({"action": "cookies", "name": "beta"})).await;
        assert_eq!(
            sc(&beta_jars)["cookies"].as_array().map(|a| a.len()),
            Some(0),
            "beta's jar is empty: {beta_jars}"
        );

        let set = rig
            .call(json!({"action": "cookies", "name": "beta", "cookies": [
                {"name": "betaonly", "value": "1", "url": format!("{}/", fixture.base)}
            ]}))
            .await;
        assert!(!is_error(&set), "cookie set failed: {}", text_of(&set));
        assert_eq!(
            sc(&set)["set"][0]["verified"].as_bool(),
            Some(true),
            "a set cookie is reported only after the jar shows it: {set}"
        );
        let alpha_after = rig
            .call(json!({"action": "cookies", "name": "alpha"}))
            .await;
        assert!(
            !sc(&alpha_after)["cookies"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|c| c["name"].as_str() == Some("betaonly")),
            "beta's cookie must not leak into alpha: {alpha_after}"
        );

        let opened = rig
            .call(json!({"action": "open", "name": "beta", "url": format!("{}/?second", fixture.base)}))
            .await;
        assert!(
            !is_error(&opened),
            "open second page failed: {}",
            text_of(&opened)
        );
        let second_target = sc(&opened)["page"]["targetId"]
            .as_str()
            .expect("the second page's targetId")
            .to_string();
        assert_eq!(sc(&opened)["pages"].as_array().map(|a| a.len()), Some(2));
        let named = rig
            .call(
                json!({"action": "eval", "name": "beta", "page": second_target,
                   "expression": "location.search"}),
            )
            .await;
        assert!(
            !is_error(&named),
            "page-scoped eval failed: {}",
            text_of(&named)
        );
        assert_eq!(
            sc(&named)["value"].as_str(),
            Some("?second"),
            "eval with page= runs in that page: {named}"
        );

        let popup = rig
            .call(json!({"action": "eval", "name": "beta",
                   "expression": format!(
                       "window.open('{}/?popup', '_blank') === null ? 'blocked' : 'opened'",
                       fixture.base)}))
            .await;
        assert!(!is_error(&popup), "window.open failed: {}", text_of(&popup));
        assert_eq!(
            sc(&popup)["value"].as_str(),
            Some("opened"),
            "the fixture's popup must open inside the context: {popup}"
        );
        let adopted = wait_for_page(&rig, "beta", "?popup", Duration::from_secs(5)).await;
        assert!(
            adopted.is_some(),
            "a site-opened page is adopted by the context's next refresh"
        );

        let tabs = rig.browser.tabs_list().await.expect("tabs list");
        let tabs = tabs
            .get("tabs")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tabs.len(),
            1,
            "legacy tabs are the default context only: {tabs:?}"
        );
        assert!(
            tabs.iter().all(|t| {
                t["url"].as_str() != Some(&fixture.base[..])
                    && !t["url"].as_str().unwrap_or("").starts_with(&fixture.base)
            }),
            "no context page may appear as a legacy tab: {tabs:?}"
        );
        let legacy = rig
            .browser
            .eval("location.href")
            .await
            .expect("legacy eval");
        assert_eq!(
            legacy.as_str(),
            Some("about:blank"),
            "selecting/creating a context must not move the legacy active page"
        );
        let hijack = rig
            .browser
            .select_target(&alpha_target)
            .await
            .expect_err("selecting a context page as the legacy page must fail");
        assert!(
            hijack.contains("isolated browser context 'alpha'"),
            "the refusal names the owning context: {hijack}"
        );

        let closed = rig
            .call(json!({"action": "close", "name": "alpha", "page": alpha_target}))
            .await;
        assert!(
            !is_error(&closed),
            "close page failed: {}",
            text_of(&closed)
        );
        assert_eq!(sc(&closed)["gone"].as_bool(), Some(true));
        assert_eq!(sc(&closed)["pages"].as_array().map(|a| a.len()), Some(0));

        let beta_show = rig.call(json!({"action": "show", "name": "beta"})).await;
        assert!(
            !is_error(&beta_show),
            "show beta failed: {}",
            text_of(&beta_show)
        );
        let beta_pages = sc(&beta_show)["pages"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            beta_pages
                .iter()
                .any(|p| p["targetId"].as_str() == Some(second_target.as_str())),
            "the second page survives alpha's teardown: {beta_show}"
        );

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "alpha"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        let d = sc(&destroyed);
        assert_eq!(d["gone"].as_bool(), Some(true));
        assert_eq!(
            d["disposedBy"].as_str(),
            Some("hand"),
            "hand performed the disposal and says so: {d}"
        );
        assert_eq!(d["evidence"]["pageTargets"].as_u64(), Some(0));
        assert_eq!(
            d["evidence"]["cookieProbe"].as_str(),
            Some("failed to find browser context")
        );
        assert_eq!(rig.reg.names().await, vec!["beta".to_string()]);

        let probe = rig
            .browser
            .cdp_raw(
                "Storage.getCookies",
                json!({"browserContextId": alpha_ctx}),
                None,
            )
            .await
            .expect_err("a disposed context must refuse Storage.getCookies");
        assert!(
            probe.contains("Failed to find browser context"),
            "unexpected probe error: {probe}"
        );
        let beta_still = rig
            .call(json!({"action": "eval", "name": "beta", "expression": "document.cookie"}))
            .await;
        assert!(
            !is_error(&beta_still),
            "beta must still work: {}",
            text_of(&beta_still)
        );
        assert_eq!(
            sc(&beta_still)["value"].as_str(),
            Some("betaonly=1"),
            "tearing down alpha left beta's state intact: {beta_still}"
        );

        let all = rig.call(json!({"action": "destroy_all"})).await;
        assert!(!is_error(&all), "destroy_all failed: {}", text_of(&all));
        assert_eq!(sc(&all)["destroyed"].as_array().map(|a| a.len()), Some(1));
        assert!(rig.reg.is_empty().await);
        let receipt = rig.reg.shutdown().await;
        assert_eq!(
            receipt["contexts"].as_u64(),
            Some(0),
            "nothing left to dispose: {receipt}"
        );

        rig.close().await;
    });
}

#[test]
fn context_arguments_and_unknown_names_are_typed_refusals() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();

        assert!(
            contexts::dispatch("navigate", &json!({"url": "about:blank"}), &rig.reg)
                .await
                .is_none(),
            "dispatch only owns the context tool"
        );

        let missing = rig.call(json!({})).await;
        assert!(is_error(&missing));
        assert_eq!(
            typed_code(&missing),
            ("invalid_args".into(), "missing_arg".into())
        );

        let bad_action = rig.call(json!({"action": "frobnicate"})).await;
        assert!(is_error(&bad_action));
        assert_eq!(
            typed_code(&bad_action),
            ("invalid_args".into(), "unknown_action".into())
        );

        let unknown_arg = rig.call(json!({"action": "list", "colour": "red"})).await;
        assert!(is_error(&unknown_arg));
        assert_eq!(
            typed_code(&unknown_arg),
            ("invalid_args".into(), "unknown_arg".into()),
            "unknown keys are rejected, never ignored: {unknown_arg}"
        );
        let wrong_key = rig
            .call(json!({"action": "show", "name": "x", "expression": "1+1"}))
            .await;
        assert!(is_error(&wrong_key));
        assert_eq!(
            typed_code(&wrong_key),
            ("invalid_args".into(), "unknown_arg".into()),
            "per-action argument gate: {wrong_key}"
        );

        let empty = rig.call(json!({"action": "list"})).await;
        assert!(!is_error(&empty), "list failed: {}", text_of(&empty));
        assert_eq!(sc(&empty)["contexts"].as_array().map(|a| a.len()), Some(0));
        assert!(
            !rig.browser.is_live().await,
            "list must not launch an engine"
        );

        let unknown = rig.call(json!({"action": "show", "name": "nope"})).await;
        assert!(is_error(&unknown));
        assert_eq!(
            typed_code(&unknown),
            ("invalid_target".into(), "not_found".into()),
            "an unknown context is a typed not-found: {unknown}"
        );

        let created = rig.call(json!({"action": "create", "name": "gamma"})).await;
        assert!(
            !is_error(&created),
            "create gamma failed: {}",
            text_of(&created)
        );
        assert_eq!(sc(&created)["readyState"].as_str(), Some("complete"));
        assert_eq!(sc(&created)["settled"].as_bool(), Some(true));

        let thrown = rig
            .call(
                json!({"action": "eval", "name": "gamma", "expression": "throw new Error('boom')"}),
            )
            .await;
        assert!(is_error(&thrown));
        assert_eq!(
            typed_code(&thrown),
            ("js_error".into(), "thrown".into()),
            "a thrown JS error is typed: {thrown}"
        );

        let dup = rig.call(json!({"action": "create", "name": "gamma"})).await;
        assert!(is_error(&dup));
        assert_eq!(
            typed_code(&dup),
            ("invalid_args".into(), "bad_value".into())
        );
        assert!(
            text_of(&dup).contains("already exists"),
            "the refusal explains itself: {}",
            text_of(&dup)
        );

        let control = rig.call(json!({"action": "create", "name": "a\nb"})).await;
        assert!(is_error(&control));
        assert_eq!(
            typed_code(&control),
            ("invalid_args".into(), "bad_value".into())
        );

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "gamma"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy gamma failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// Measured teardown semantics: disposing a context fires `pagehide` on each
/// page but never `beforeunload` or `unload`. Chrome 158 full Chrome fires
/// `pagehide` on `Target.disposeBrowserContext` and CDP cannot suppress it, so
/// this pins the browser's behaviour, not an engine action. `close` (a plain
/// closeTarget) runs `pagehide` + `unload`: the control that proves the
/// fixture's sink observes handler traffic.
#[test]
fn destroy_skips_page_teardown_handlers_while_close_runs_them() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    block_on(async {
        let rig = Rig::start();

        let abrupt = rig
            .call(json!({"action": "create", "name": "abrupt", "url": format!("{}/teardown", fixture.base)}))
            .await;
        assert!(
            !is_error(&abrupt),
            "create abrupt failed: {}",
            text_of(&abrupt)
        );
        if sc(&abrupt)["settled"].as_bool() != Some(true) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let state = rig
                    .call(json!({"action": "eval", "name": "abrupt", "expression": "document.readyState"}))
                    .await;
                if sc(&state)["value"].as_str() == Some("complete") {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "the teardown fixture must load: {}",
                    text_of(&state)
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        fixture.clear_seen();
        let destroyed = rig
            .call(json!({"action": "destroy", "name": "abrupt"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(true));
        tokio::time::sleep(Duration::from_millis(700)).await;
        let seen = fixture.seen();
        // Chrome 158 emits `pagehide` on dispose; it cannot be suppressed from
        // CDP. The guarantee that matters - no handler that can block navigation
        // (`beforeunload`) or persist work (`unload`) - still holds, and the
        // pagehide assertion pins the truth the old tool text got wrong.
        assert!(
            seen.iter().any(|s| s == "pagehide"),
            "disposeBrowserContext must fire pagehide on Chrome 158; observed {seen:?}"
        );
        assert!(
            !seen.iter().any(|s| s == "beforeunload" || s == "unload"),
            "disposeBrowserContext must run no beforeunload/unload; observed {seen:?}"
        );

        let orderly = rig
            .call(json!({"action": "create", "name": "orderly", "url": format!("{}/teardown", fixture.base)}))
            .await;
        assert!(
            !is_error(&orderly),
            "create orderly failed: {}",
            text_of(&orderly)
        );
        let page = sc(&orderly)["page"]["targetId"]
            .as_str()
            .expect("page targetId")
            .to_string();
        tokio::time::sleep(Duration::from_millis(200)).await;
        fixture.clear_seen();
        let closed = rig
            .call(json!({"action": "close", "name": "orderly", "page": page}))
            .await;
        assert!(!is_error(&closed), "close failed: {}", text_of(&closed));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seen = fixture.seen();
        while seen.is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            seen = fixture.seen();
        }
        assert!(
            seen.iter().any(|s| s == "pagehide" || s == "unload"),
            "closeTarget runs the page's teardown handlers (pagehide/unload), so the sink works \
             and the dispose result is meaningful: {seen:?}"
        );

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "orderly"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy orderly failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// Gap: realm-scoped `evaluate target=` used to reach a context page, so the
/// advertised boundary was only true for `tabs select`. The legacy routes must
/// refuse a context page by name; `targets list` must mark it; legacy acting on
/// the default page must keep working.
#[test]
fn legacy_realm_routing_refuses_a_context_page() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    block_on(async {
        let rig = Rig::start();
        open_legacy_page(&rig, &format!("{}/", fixture.base), "Context Fixture").await;

        let created = rig
            .call(json!({"action": "create", "name": "sealed", "url": format!("{}/?sealed", fixture.base)}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let target = sc(&created)["page"]["targetId"]
            .as_str()
            .expect("targetId")
            .to_string();

        let evaluated = browser_engine::call(
            "evaluate",
            &json!({"target": target, "expression": "location.href"}),
            &rig.browser,
        )
        .await;
        assert!(
            is_error(&evaluated),
            "evaluate(target=<context page>) must not reach it: {evaluated}"
        );
        assert_eq!(
            typed_code(&evaluated),
            ("invalid_target".into(), "not_found".into())
        );
        assert!(
            text_of(&evaluated).contains("isolated browser context 'sealed'"),
            "the refusal names the boundary: {}",
            text_of(&evaluated)
        );

        let hijack = rig
            .browser
            .select_target(&target)
            .await
            .expect_err("tabs select on a context page must fail");
        assert!(hijack.contains("isolated browser context 'sealed'"));

        let listed_targets =
            browser_engine::call("targets", &json!({"action": "list"}), &rig.browser).await;
        assert!(!is_error(&listed_targets));
        let rows = sc(&listed_targets)["targets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let marked = rows
            .iter()
            .find(|r| r["targetId"].as_str() == Some(target.as_str()))
            .expect("the context page is still listed as a raw target");
        assert_eq!(
            marked["context"].as_str(),
            Some("sealed"),
            "targets list says which context owns it: {marked}"
        );
        // Chrome 158 full Chrome runs component-extension service workers in
        // every browser context, so they are legitimately marked with the
        // context name too. Only pages decide the default-context legacy shape.
        for row in rows
            .iter()
            .filter(|r| r["targetId"].as_str() != Some(target.as_str()))
            .filter(|r| r["type"].as_str() == Some("page"))
        {
            assert!(
                row.get("context").is_none(),
                "default-context pages keep the legacy shape: {row}"
            );
        }

        let legacy = browser_engine::call(
            "evaluate",
            &json!({"expression": "document.title"}),
            &rig.browser,
        )
        .await;
        assert!(
            !is_error(&legacy),
            "legacy evaluate must keep working: {}",
            text_of(&legacy)
        );
        assert_eq!(sc(&legacy)["value"].as_str(), Some("Context Fixture"));

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "sealed"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// Gap: shutdown/destroy against a dead engine used to RELAUNCH a browser (and
/// could then report a failure for a context that was already gone). Nothing
/// may launch, and nothing may be reported as disposed by hand.
#[test]
fn dead_engine_never_relaunches_and_never_claims_disposal() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig.call(json!({"action": "create", "name": "gone"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let generation = rig.browser.generation();
        assert!(generation >= 1, "an engine was launched");

        rig.browser.close().await.expect("close engine");
        assert!(!rig.browser.is_live().await);

        let destroyed = rig.call(json!({"action": "destroy", "name": "gone"})).await;
        assert!(
            is_error(&destroyed),
            "destroy must not claim success: {destroyed}"
        );
        assert_eq!(
            typed_code(&destroyed),
            ("engine_dead".into(), "transport".into())
        );
        assert!(
            text_of(&destroyed).contains("not reachable")
                && !text_of(&destroyed).contains("disposed by hand"),
            "the receipt refuses to claim hand disposed it and does not claim the process exited: {}",
            text_of(&destroyed)
        );
        assert_eq!(
            rig.browser.generation(),
            generation,
            "a destroy against a dead engine must never launch one"
        );
        assert!(!rig.browser.is_live().await);

        let listed = rig.call(json!({"action": "list"})).await;
        assert!(!is_error(&listed), "list failed: {}", text_of(&listed));
        assert_eq!(sc(&listed)["contexts"].as_array().map(|a| a.len()), Some(0));

        let all = rig.call(json!({"action": "destroy_all"})).await;
        assert!(!is_error(&all), "destroy_all failed: {}", text_of(&all));
        assert_eq!(
            sc(&all)["destroyed"].as_array().map(|a| a.len()),
            Some(0),
            "nothing was disposed by hand: {all}"
        );
        let receipt = rig.reg.shutdown().await;
        assert_eq!(
            receipt["disposed"].as_array().map(|a| a.len()),
            Some(0),
            "shutdown never claims a disposal it did not perform: {receipt}"
        );
        assert_eq!(
            rig.browser.generation(),
            generation,
            "shutdown must never launch an engine"
        );
    });
}

/// Gap: a cancelled `create` (budget/abort) used to leave a context that the
/// engine had created but the registry had never recorded. The acquisition now
/// runs in its own task, so the context commits and stays tracked.
#[test]
fn cancelled_create_is_committed_by_its_own_task() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    let _hooks = EnvHooks::new(&[
        ("HAND_CONTEXT_TEST_SLOW_AT", "after-page"),
        ("HAND_CONTEXT_TEST_SLOW_MS", "600"),
    ]);
    block_on(async {
        let rig = Rig::start();
        let once = rig
            .call(json!({"action": "create", "name": "slow", "url": format!("{}/", fixture.base)}));
        let cancelled = tokio::time::timeout(Duration::from_millis(120), once).await;
        assert!(
            cancelled.is_err(),
            "the caller's future must be the thing that is cancelled"
        );

        let entry = wait_for_phase(&rig, "slow", "ready", Duration::from_secs(10))
            .await
            .expect("the cancelled create must still commit");
        let context_id = entry["browserContextId"]
            .as_str()
            .expect("committed context id")
            .to_string();
        assert_eq!(
            rig.engine_pages(&context_id).await,
            1,
            "the engine has exactly the one page the context committed"
        );
        let listed = rig.call(json!({"action": "list"})).await;
        assert_eq!(
            listed["structuredContent"]["contexts"]
                .as_array()
                .map(|a| a.len()),
            Some(1)
        );

        let destroyed = rig.call(json!({"action": "destroy", "name": "slow"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["disposedBy"].as_str(), Some("hand"));
        rig.close().await;
    });
}

/// Gap: a failed `open` (attach failure) used to leave the freshly created page
/// behind, untracked. It must be closed, and the failure must say so.
#[test]
fn failed_open_closes_the_page_it_made() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig.call(json!({"action": "create", "name": "p"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        let before = rig.engine_pages(&context_id).await;
        assert_eq!(before, 1);

        let opened = {
            let _fail = EnvHooks::new(&[("HAND_CONTEXT_TEST_FAIL_ATTACH", "1")]);
            rig.call(json!({"action": "open", "name": "p", "url": "about:blank"}))
                .await
        };
        assert!(is_error(&opened), "open must report the failure: {opened}");
        assert!(
            text_of(&opened).contains("closed"),
            "the error states the rollback: {}",
            text_of(&opened)
        );
        assert_eq!(
            typed_code(&opened),
            ("target_changed".into(), "raced".into()),
            "the cause's code and reason survive the rollback: {opened}"
        );
        assert!(
            !text_of(&opened).contains("typed:"),
            "no marker in the prose: {}",
            text_of(&opened)
        );
        assert_eq!(
            rig.engine_pages(&context_id).await,
            before,
            "the page the failed open made must be gone from the engine"
        );
        let shown = rig.call(json!({"action": "show", "name": "p"})).await;
        assert_eq!(
            sc(&shown)["pages"].as_array().map(|a| a.len()),
            Some(1),
            "hand tracks only the original page: {shown}"
        );

        let destroyed = rig.call(json!({"action": "destroy", "name": "p"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// Gap: a failed disposal used to unregister the context's exclusion, so its
/// page could be adopted by the legacy tab space while hand still tracked it.
#[test]
fn failed_disposal_keeps_tracking_and_legacy_exclusion() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig
            .call(json!({"action": "create", "name": "sticky"}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();

        let failed = {
            let _hook = EnvHooks::new(&[("HAND_CONTEXT_TEST_FAIL_DISPOSE", "1")]);
            rig.call(json!({"action": "destroy", "name": "sticky"}))
                .await
        };
        assert!(
            is_error(&failed),
            "a failed disposal is a failure: {failed}"
        );
        assert!(
            text_of(&failed).contains("stays tracked"),
            "the error says the context is still tracked: {}",
            text_of(&failed)
        );
        assert_eq!(
            rig.reg.names().await,
            vec!["sticky".to_string()],
            "a failed disposal must keep tracking"
        );
        assert!(
            rig.browser
                .excluded_browser_contexts()
                .contains_key(&context_id),
            "a failed disposal must keep the legacy exclusion"
        );
        let tabs = rig.browser.tabs_list().await.expect("tabs list");
        assert_eq!(
            tabs["tabs"].as_array().map(|a| a.len()),
            Some(1),
            "the context's page stays out of legacy tabs: {tabs}"
        );

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "sticky"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy after the failure failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["disposedBy"].as_str(), Some("hand"));
        assert!(
            !rig.browser
                .excluded_browser_contexts()
                .contains_key(&context_id),
            "a verified disposal releases the exclusion"
        );
        rig.close().await;
    });
}

/// Gap: cookie verification matched name+value only, so a same-name cookie at
/// another path, a missing attribute or a pre-existing identical cookie were
/// all reported as a successful set. Verification must use identity and the
/// requested attributes.
#[test]
fn cookie_identity_and_attributes_are_verified() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    block_on(async {
        let rig = Rig::start();
        let url = format!("{}/", fixture.base);
        let created = rig
            .call(json!({"action": "create", "name": "jar", "url": url}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));

        let a = rig
            .call(json!({"action": "cookies", "name": "jar", "cookies": [
                {"name": "k", "value": "one", "url": url, "path": "/a"}]}))
            .await;
        assert!(!is_error(&a), "set /a failed: {}", text_of(&a));
        let rec = &sc(&a)["set"][0];
        assert_eq!(rec["identity"]["path"].as_str(), Some("/a"));
        assert_eq!(rec["identity"]["domain"].as_str(), Some("127.0.0.1"));
        assert_eq!(rec["preexisting"].as_bool(), Some(false));

        let b = rig
            .call(json!({"action": "cookies", "name": "jar", "cookies": [
                {"name": "k", "value": "two", "url": url, "path": "/b"}]}))
            .await;
        assert!(!is_error(&b), "set /b failed: {}", text_of(&b));
        assert_eq!(sc(&b)["set"][0]["identity"]["path"].as_str(), Some("/b"));
        let jar = sc(&b)["cookies"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            jar.iter()
                .filter(|c| c["name"].as_str() == Some("k"))
                .count(),
            2,
            "the two identities coexist: {jar:?}"
        );

        let c = rig
            .call(json!({"action": "cookies", "name": "jar", "cookies": [
                {"name": "k", "value": "three", "url": url, "path": "/b",
                 "httpOnly": true, "sameSite": "Lax"}]}))
            .await;
        assert!(!is_error(&c), "set httpOnly failed: {}", text_of(&c));
        let rec = &sc(&c)["set"][0];
        assert_eq!(rec["verified"].as_bool(), Some(true));
        assert_eq!(rec["attributes"]["httpOnly"].as_bool(), Some(true));
        assert_eq!(rec["attributes"]["sameSite"].as_str(), Some("Lax"));

        let d = rig
            .call(json!({"action": "cookies", "name": "jar", "cookies": [
                {"name": "k", "value": "three", "url": url, "path": "/b",
                 "httpOnly": true, "sameSite": "Lax"}]}))
            .await;
        assert!(!is_error(&d), "re-set failed: {}", text_of(&d));
        assert_eq!(
            sc(&d)["set"][0]["preexisting"].as_bool(),
            Some(true),
            "an identical pre-existing cookie is not hand's change: {d}"
        );

        let skipped = {
            let _hook = EnvHooks::new(&[("HAND_CONTEXT_TEST_SKIP_COOKIE_SET", "1")]);
            rig.call(json!({"action": "cookies", "name": "jar", "cookies": [
                {"name": "k", "value": "four", "url": url, "path": "/b"}]}))
                .await
        };
        assert!(
            is_error(&skipped),
            "a set the jar cannot show is a failure: {skipped}"
        );
        assert_eq!(
            typed_code(&skipped),
            ("cookie_rejected".into(), "set_failed".into()),
            "the failure is typed as a rejected set: {skipped}"
        );
        assert!(
            text_of(&skipped).contains("could not be verified"),
            "the refusal explains the verification: {}",
            text_of(&skipped)
        );

        let destroyed = rig.call(json!({"action": "destroy", "name": "jar"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// A disposal hand did not perform (the context was already removed, e.g. a
/// retried request after a lost reply) must report `already-gone`, never
/// `hand` - and must still verify the goal state.
#[test]
fn disposal_of_an_already_gone_context_is_reported_as_such() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig
            .call(json!({"action": "create", "name": "vanished"}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();

        rig.browser
            .cdp_raw(
                "Target.disposeBrowserContext",
                json!({"browserContextId": context_id}),
                None,
            )
            .await
            .expect("out-of-band dispose");

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "vanished"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "the goal state holds, so destroy succeeds: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(true));
        assert_eq!(
            sc(&destroyed)["disposedBy"].as_str(),
            Some("already-gone"),
            "hand did not perform this disposal and must not claim it: {destroyed}"
        );
        assert!(rig.reg.is_empty().await);
        rig.close().await;
    });
}

#[test]
fn selection_is_registry_scoped_and_teardown_leaves_the_other_context_alone() {
    let _serial = serial();
    require_owned_browser();
    let fixture = Fixture::start();
    block_on(async {
        let rig = Rig::start();

        open_legacy_page(&rig, &format!("{}/", fixture.base), "Context Fixture").await;

        for name in ["one", "two", "three"] {
            let r = rig
                .call(
                    json!({"action": "create", "name": name, "url": format!("{}/", fixture.base)}),
                )
                .await;
            assert!(!is_error(&r), "create {name} failed: {}", text_of(&r));
            assert_eq!(
                sc(&r)["selected"].as_str(),
                Some(name),
                "create reports the context it made the registry default"
            );
        }

        let defaulted = rig
            .call(json!({"action": "eval", "expression": "1+1"}))
            .await;
        assert!(
            !is_error(&defaulted),
            "defaulted eval failed: {}",
            text_of(&defaulted)
        );
        assert_eq!(sc(&defaulted)["name"].as_str(), Some("three"));

        let selected = rig.call(json!({"action": "select", "name": "two"})).await;
        assert!(
            !is_error(&selected),
            "select failed: {}",
            text_of(&selected)
        );
        let s = sc(&selected);
        assert_eq!(s["selected"].as_str(), Some("two"));
        assert_eq!(s["legacyUnaffected"].as_bool(), Some(true));
        assert!(
            s["selectedAffects"]
                .as_str()
                .unwrap_or("")
                .contains("legacy browser_* tools keep acting on the legacy default page"),
            "selection states its own scope: {s}"
        );

        let hit = rig
            .call(json!({"action": "eval", "expression": "document.title"}))
            .await;
        assert!(!is_error(&hit), "selected eval failed: {}", text_of(&hit));
        assert_eq!(sc(&hit)["name"].as_str(), Some("two"));
        assert_eq!(sc(&hit)["value"].as_str(), Some("Context Fixture"));

        let legacy_title = rig
            .browser
            .eval("document.title")
            .await
            .expect("legacy eval");
        assert_eq!(legacy_title.as_str(), Some("Context Fixture"));
        let legacy_target = rig.browser.active_target_id().await.expect("legacy target");
        let contexts_now = rig.call(json!({"action": "list"})).await;
        let listed = sc(&contexts_now)["contexts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(listed.len(), 3);
        for c in &listed {
            for p in c["pages"].as_array().cloned().unwrap_or_default() {
                assert_ne!(
                    p["targetId"].as_str(),
                    Some(legacy_target.as_str()),
                    "the legacy active target can never be a context page: {contexts_now}"
                );
            }
        }

        let destroyed = rig.call(json!({"action": "destroy", "name": "two"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy two failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(rig.reg.selected().await, None);
        assert_eq!(
            rig.reg.names().await,
            vec!["one".to_string(), "three".to_string()]
        );
        let ambiguous = rig.call(json!({"action": "eval", "expression": "1"})).await;
        assert!(
            is_error(&ambiguous),
            "no selection, two contexts: {ambiguous}"
        );
        assert_eq!(
            typed_code(&ambiguous),
            ("invalid_args".into(), "missing_arg".into())
        );
        assert!(
            text_of(&ambiguous).contains("\"one\"") && text_of(&ambiguous).contains("\"three\""),
            "the refusal names the candidates: {}",
            text_of(&ambiguous)
        );

        for name in ["one", "three"] {
            let survivor = rig
                .call(
                    json!({"action": "eval", "name": name, "expression": "[document.title, 1+1]"}),
                )
                .await;
            assert!(
                !is_error(&survivor),
                "survivor {name} failed: {}",
                text_of(&survivor)
            );
            assert_eq!(
                sc(&survivor)["value"],
                json!(["Context Fixture", 2]),
                "context {name} is untouched: {survivor}"
            );
        }

        let tabs = rig.browser.tabs_list().await.expect("tabs list");
        assert_eq!(
            tabs["tabs"].as_array().map(|a| a.len()),
            Some(1),
            "legacy tabs stay in the default context: {tabs}"
        );

        let shutdown = rig.reg.shutdown().await;
        assert_eq!(shutdown["contexts"].as_u64(), Some(2));
        assert_eq!(
            shutdown["disposed"].as_array().map(|a| a.len()),
            Some(2),
            "live contexts are disposed by hand: {shutdown}"
        );
        assert!(rig.reg.names().await.is_empty());
        rig.close().await;
    });
}


/// A stale acquisition must never write into, or roll back, a later entry that
/// reused the same name. Reproduced with a slowed first acquisition, a destroy
/// during its flight, a re-create, then a forced rollback of the stale one.
#[test]
fn stale_acquisition_cannot_touch_a_recreated_name() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let hooks = EnvHooks::new(&[
            ("HAND_CONTEXT_TEST_SLOW_AT", "after-page"),
            ("HAND_CONTEXT_TEST_SLOW_MS", "700"),
        ]);
        let first = tokio::spawn({
            let reg = Arc::clone(&rig.reg);
            async move {
                contexts::dispatch(TOOL, &json!({"action": "create", "name": "x"}), &reg)
                    .await
                    .expect("dispatch claims the tool")
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        let destroyed = rig.call(json!({"action": "destroy", "name": "x"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy of the in-flight acquisition failed: {}",
            text_of(&destroyed)
        );
        drop(hooks);
        let second = rig.call(json!({"action": "create", "name": "x"})).await;
        assert!(!is_error(&second), "re-create failed: {}", text_of(&second));
        let second_id = sc(&second)["browserContextId"]
            .as_str()
            .expect("recreated id")
            .to_string();

        let fail = EnvHooks::new(&[("HAND_CONTEXT_TEST_FAIL_ATTACH", "1")]);
        let stale_result = first.await.expect("join the stale acquisition");
        assert!(
            is_error(&stale_result),
            "the stale acquisition must report its failure: {stale_result}"
        );
        assert_eq!(
            typed_code(&stale_result),
            ("target_changed".into(), "raced".into()),
            "the rolled-back cause keeps its code and reason: {stale_result}"
        );
        assert!(
            !text_of(&stale_result).contains("typed:"),
            "and its marker never reaches the prose: {}",
            text_of(&stale_result)
        );
        drop(fail);

        let names = rig.reg.names().await;
        assert_eq!(
            names,
            vec!["x".to_string()],
            "the recreated context must survive the stale rollback"
        );
        let listing = rig.call(json!({"action": "list"})).await;
        let entry = listed(&listing, "x").expect("the recreated context is listed");
        assert_eq!(
            entry["browserContextId"].as_str(),
            Some(second_id.as_str()),
            "tracking points at the recreated context: {entry}"
        );
        assert_eq!(entry["phase"].as_str(), Some("ready"));
        let value = rig
            .call(json!({"action": "eval", "name": "x", "expression": "1+1"}))
            .await;
        assert!(
            !is_error(&value),
            "the recreated context still works: {}",
            text_of(&value)
        );
        assert_eq!(sc(&value)["value"], json!(2));
        let destroyed = rig.call(json!({"action": "destroy", "name": "x"})).await;
        assert!(
            !is_error(&destroyed),
            "final destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// `cookies` must reconcile first, keep a live CDP failure's cause (instead of
/// calling it engine death), and drop a context the engine says no longer
/// exists - releasing its legacy exclusion.
#[test]
fn cookies_reconciles_vanished_contexts_and_keeps_cdp_causes() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();

        let created = rig
            .call(json!({"action": "create", "name": "vanished"}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        rig.browser
            .cdp_raw(
                "Target.disposeBrowserContext",
                json!({"browserContextId": context_id}),
                None,
            )
            .await
            .expect("out-of-band dispose");
        let read = rig
            .call(json!({"action": "cookies", "name": "vanished"}))
            .await;
        assert!(
            is_error(&read),
            "a vanished context must not answer cookies: {read}"
        );
        assert_eq!(
            typed_code(&read),
            ("invalid_target".into(), "not_found".into()),
            "the engine's 'no such context' is a typed not-found: {read}"
        );
        assert!(
            text_of(&read).contains("no longer in the engine"),
            "the cause is kept: {}",
            text_of(&read)
        );
        assert!(
            rig.reg.names().await.is_empty(),
            "the vanished context is no longer tracked"
        );
        assert!(
            !rig.browser
                .excluded_browser_contexts()
                .contains_key(&context_id),
            "its legacy exclusion is released"
        );

        let created = rig.call(json!({"action": "create", "name": "jar"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let failing = {
            let _hook = EnvHooks::new(&[("HAND_CONTEXT_TEST_JAR_ERROR", "1")]);
            rig.call(json!({"action": "cookies", "name": "jar"})).await
        };
        assert!(is_error(&failing), "a CDP failure is an error: {failing}");
        assert!(
            text_of(&failing).contains("synthetic jar failure"),
            "the engine's cause survives: {}",
            text_of(&failing)
        );
        assert_ne!(
            typed_code(&failing),
            ("engine_dead".into(), "transport".into()),
            "a live CDP failure is not engine death: {failing}"
        );
        assert_eq!(
            rig.reg.names().await,
            vec!["jar".to_string()],
            "the context stays tracked after a CDP failure"
        );
        rig.close().await;
    });
}

/// A cancelled `open` is committed by its own task: the page exists and stays
/// tracked (never invisible).
#[test]
fn cancelled_open_leaves_the_page_tracked() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig.call(json!({"action": "create", "name": "p"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(rig.engine_pages(&context_id).await, 1);

        let hooks = EnvHooks::new(&[
            ("HAND_CONTEXT_TEST_SLOW_AT", "open-after-attach"),
            ("HAND_CONTEXT_TEST_SLOW_MS", "600"),
        ]);
        let call = rig.call(json!({"action": "open", "name": "p", "url": "about:blank"}));
        let cancelled = tokio::time::timeout(Duration::from_millis(150), call).await;
        assert!(
            cancelled.is_err(),
            "the caller's future must be the thing that is cancelled"
        );
        drop(hooks);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if rig.engine_pages(&context_id).await == 2 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the cancelled open's page must exist in the engine"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let shown = rig.call(json!({"action": "show", "name": "p"})).await;
        assert!(!is_error(&shown), "show failed: {}", text_of(&shown));
        assert_eq!(
            sc(&shown)["pages"].as_array().map(|a| a.len()),
            Some(2),
            "the cancelled open's page is tracked: {shown}"
        );
        let destroyed = rig.call(json!({"action": "destroy", "name": "p"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// `close` must fail - not report success - when its context is destroyed while
/// the page is closing.
#[test]
fn close_reports_a_context_that_vanished_mid_close() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig.call(json!({"action": "create", "name": "c"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let page = sc(&created)["page"]["targetId"]
            .as_str()
            .unwrap()
            .to_string();
        let opened = rig
            .call(json!({"action": "open", "name": "c", "url": "about:blank"}))
            .await;
        assert!(!is_error(&opened), "open failed: {}", text_of(&opened));

        let hooks = EnvHooks::new(&[("HAND_CONTEXT_TEST_SLOW_ON_CLOSE_MS", "400")]);
        let closing = tokio::spawn({
            let reg = Arc::clone(&rig.reg);
            async move {
                contexts::dispatch(
                    TOOL,
                    &json!({"action": "close", "name": "c", "page": page}),
                    &reg,
                )
                .await
                .expect("dispatch")
            }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let destroyed = rig.call(json!({"action": "destroy", "name": "c"})).await;
        assert!(
            !is_error(&destroyed),
            "the concurrent destroy failed: {}",
            text_of(&destroyed)
        );
        drop(hooks);
        let closed = closing.await.expect("join close");
        assert!(
            is_error(&closed),
            "a close whose context vanished must fail, not report success: {closed}"
        );
        assert_eq!(
            typed_code(&closed),
            ("target_changed".into(), "raced".into()),
            "the failure names the race: {closed}"
        );
        assert!(
            text_of(&closed).contains("destroyed while page"),
            "the report says what happened: {}",
            text_of(&closed)
        );
        rig.close().await;
    });
}

/// An acquisition that never observed an id is not stuck forever: destroy
/// removes it (reporting that nothing could be disposed), the name becomes
/// reusable, and the stale task disposes whatever it created.
#[test]
fn an_id_less_acquisition_can_be_destroyed_and_its_name_reused() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let hooks = EnvHooks::new(&[
            ("HAND_CONTEXT_TEST_SLOW_AT", "before-create"),
            ("HAND_CONTEXT_TEST_SLOW_MS", "600"),
        ]);
        let first = tokio::spawn({
            let reg = Arc::clone(&rig.reg);
            async move {
                contexts::dispatch(TOOL, &json!({"action": "create", "name": "z"}), &reg)
                    .await
                    .expect("dispatch")
            }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let listing = rig.call(json!({"action": "list"})).await;
        let entry = listed(&listing, "z").expect("an in-flight acquisition is visible");
        assert_eq!(entry["phase"].as_str(), Some("creating"));

        let destroyed = rig.call(json!({"action": "destroy", "name": "z"})).await;
        assert!(
            !is_error(&destroyed),
            "an id-less acquisition must be destroyable: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(false));
        assert_eq!(sc(&destroyed)["disposedBy"].as_str(), Some("none"));
        assert!(rig.reg.names().await.is_empty());

        drop(hooks);
        let recreated = rig.call(json!({"action": "create", "name": "z"})).await;
        assert!(
            !is_error(&recreated),
            "the name is reusable: {}",
            text_of(&recreated)
        );
        let recreated_id = sc(&recreated)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        let _ = first.await.expect("join stale acquisition");
        assert_eq!(
            rig.engine_pages(&recreated_id).await,
            1,
            "the stale acquisition disposed its own context"
        );
        assert_eq!(rig.reg.names().await, vec!["z".to_string()]);
        let value = rig
            .call(json!({"action": "eval", "name": "z", "expression": "2+2"}))
            .await;
        assert_eq!(sc(&value)["value"], json!(4));
        rig.close().await;
    });
}

/// `destroy_all` must not clear the registry selection while the selected
/// context survives, and its receipt must name each outcome once.
#[test]
fn destroy_all_preserves_the_selection_when_it_survives() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        for name in ["keep", "go"] {
            let r = rig.call(json!({"action": "create", "name": name})).await;
            assert!(!is_error(&r), "create {name} failed: {}", text_of(&r));
        }
        let selected = rig.call(json!({"action": "select", "name": "keep"})).await;
        assert!(
            !is_error(&selected),
            "select failed: {}",
            text_of(&selected)
        );

        let all = {
            let _hook = EnvHooks::new(&[("HAND_CONTEXT_TEST_FAIL_DISPOSE_NAME", "keep")]);
            rig.call(json!({"action": "destroy_all"})).await
        };
        assert!(is_error(&all), "a partial destroy_all is a failure: {all}");
        assert_eq!(
            rig.reg.names().await,
            vec!["keep".to_string()],
            "only the failed context remains tracked"
        );
        assert_eq!(
            rig.reg.selected().await,
            Some("keep".to_string()),
            "the selection survives while its context is tracked"
        );
        let failed = sc(&all)["failed"].as_array().cloned().unwrap_or_default();
        assert_eq!(failed.len(), 1, "the failed context is named once: {all}");
        assert_eq!(failed[0]["name"].as_str(), Some("keep"));
        let destroyed = sc(&all)["destroyed"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            destroyed.len(),
            1,
            "the disposed context is named once: {all}"
        );
        assert_eq!(destroyed[0]["name"].as_str(), Some("go"));

        let destroyed = rig.call(json!({"action": "destroy", "name": "keep"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(rig.reg.selected().await, None);
        rig.close().await;
    });
}

/// A settled created page must report the DOCUMENT's title, not the engine's
/// cached target-info string (which is empty until the load progresses), and the
/// record must say which of the two it is. `show` must not relabel a live-read
/// title as target info while the document is unchanged.
#[test]
fn a_settled_page_reports_the_document_title_and_its_source() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let fixture = Fixture::start();
        let created = rig
            .call(
                json!({"action": "create", "name": "titled", "url": format!("{}/a", fixture.base)}),
            )
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let result = sc(&created);
        assert_eq!(
            result["title"].as_str(),
            Some("Titled A"),
            "create reports the document's own title: {}",
            text_of(&created)
        );
        assert_eq!(
            result["source"].as_str(),
            Some("targetInfo+liveRead"),
            "a live-read title is never labelled targetInfo: {}",
            text_of(&created)
        );
        assert_eq!(result["settled"].as_bool(), Some(true));
        assert_eq!(
            result["page"]["title"].as_str(),
            Some("Titled A"),
            "the nested page record agrees: {}",
            text_of(&created)
        );
        assert_eq!(
            result["page"]["source"].as_str(),
            Some("targetInfo+liveRead")
        );
        assert!(
            result.get("titleNote").is_none(),
            "a successful read has nothing to explain: {}",
            text_of(&created)
        );

        let eval = rig
            .call(json!({"action": "eval", "name": "titled", "expression": "document.title"}))
            .await;
        assert_eq!(sc(&eval)["value"].as_str(), Some("Titled A"));

        let shown = rig.call(json!({"action": "show", "name": "titled"})).await;
        assert!(!is_error(&shown), "show failed: {}", text_of(&shown));
        let page = &sc(&shown)["pages"][0];
        assert_eq!(
            page["title"].as_str(),
            Some("Titled A"),
            "a refresh keeps the live title: {shown}"
        );
        assert_eq!(
            page["source"].as_str(),
            Some("targetInfo+liveRead"),
            "and keeps saying where it came from: {shown}"
        );

        let opened = rig
            .call(json!({"action": "open", "name": "titled", "url": format!("{}/b", fixture.base)}))
            .await;
        assert!(!is_error(&opened), "open failed: {}", text_of(&opened));
        assert_eq!(
            sc(&opened)["page"]["title"].as_str(),
            Some("Titled B"),
            "open reports the new document's title: {}",
            text_of(&opened)
        );
        assert_eq!(
            sc(&opened)["page"]["source"].as_str(),
            Some("targetInfo+liveRead")
        );

        let destroyed = rig
            .call(json!({"action": "destroy", "name": "titled"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// While the response is still in flight the target is the initial `about:blank`
/// document: complete, with no title and no URL of its own. A create that
/// returned then would report a settled page with no title - the exact shape
/// this fix is about - so settle must wait for the requested document.
#[test]
fn a_created_page_never_settles_on_the_initial_about_blank_document() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let fixture = Fixture::start();
        let created = rig
            .call(
                json!({"action": "create", "name": "slow", "url": format!("{}/slow", fixture.base)}),
            )
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let result = sc(&created);
        assert_eq!(
            result["settled"].as_bool(),
            Some(true),
            "settle waits for the requested page, not the initial document: {}",
            text_of(&created)
        );
        assert_eq!(
            result["title"].as_str(),
            Some("Slow Fixture"),
            "the initial document's empty title is never reported: {}",
            text_of(&created)
        );
        assert_eq!(
            result["source"].as_str(),
            Some("targetInfo+liveRead"),
            "the title came from the document: {}",
            text_of(&created)
        );
        let eval = rig
            .call(json!({"action": "eval", "name": "slow", "expression": "[document.title, location.href]"}))
            .await;
        assert_eq!(
            sc(&eval)["value"],
            json!(["Slow Fixture", format!("{}/slow", fixture.base)]),
            "the page really is the fixture route: {}",
            text_of(&eval)
        );
        let destroyed = rig.call(json!({"action": "destroy", "name": "slow"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// A teardown future dropped mid-loop (the server's grace timeout, an aborted
/// worker) must leave every unproven context tracked: the registry is the
/// authority, not a `Vec` taken into the dropped future.
#[test]
fn a_dropped_shutdown_leaves_the_remaining_contexts_tracked() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let mut ids = Vec::new();
        for name in ["alpha", "beta"] {
            let r = rig.call(json!({"action": "create", "name": name})).await;
            assert!(!is_error(&r), "create {name} failed: {}", text_of(&r));
            ids.push(
                sc(&r)["browserContextId"]
                    .as_str()
                    .expect("browserContextId")
                    .to_string(),
            );
        }

        {
            let _hook = EnvHooks::new(&[("HAND_CONTEXT_TEST_SLOW_DISPOSE_MS", "600")]);
            let dropped =
                tokio::time::timeout(Duration::from_millis(150), rig.reg.shutdown()).await;
            assert!(
                dropped.is_err(),
                "the caller's future must be the thing that is dropped mid-teardown"
            );
        }

        let listing = rig.call(json!({"action": "list"})).await;
        assert!(!is_error(&listing), "list failed: {}", text_of(&listing));
        assert_eq!(
            sc(&listing)["contexts"].as_array().map(|a| a.len()),
            Some(2),
            "a dropped teardown must not lose contexts: {listing}"
        );
        for id in &ids {
            assert!(
                rig.browser.excluded_browser_contexts().contains_key(id),
                "the legacy exclusion stays until a disposal is proven"
            );
        }

        for name in ["alpha", "beta"] {
            let destroyed = rig.call(json!({"action": "destroy", "name": name})).await;
            assert!(
                !is_error(&destroyed),
                "destroy {name} failed: {}",
                text_of(&destroyed)
            );
        }
        assert!(rig.reg.names().await.is_empty());
        rig.close().await;
    });
}

/// Teardown begins while an acquisition is asleep before its first CDP call:
/// the acquisition must refuse instead of launching an owned engine or
/// re-attaching an external one.
#[test]
fn an_acquisition_after_teardown_never_launches() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        assert_eq!(rig.browser.generation(), 0, "nothing launched yet");
        let hooks = EnvHooks::new(&[
            ("HAND_CONTEXT_TEST_SLOW_AT", "before-create"),
            ("HAND_CONTEXT_TEST_SLOW_MS", "500"),
        ]);
        let first = tokio::spawn({
            let reg = Arc::clone(&rig.reg);
            async move {
                contexts::dispatch(TOOL, &json!({"action": "create", "name": "late"}), &reg)
                    .await
                    .expect("dispatch claims the tool")
            }
        });
        tokio::time::sleep(Duration::from_millis(120)).await;

        let receipt = rig.reg.shutdown().await;
        assert!(
            receipt["contexts"].as_u64().unwrap_or(0) >= 1,
            "the in-flight acquisition is reported by teardown: {receipt}"
        );
        drop(hooks);

        let refused = first.await.expect("join the acquisition");
        assert!(
            is_error(&refused),
            "an acquisition that starts after teardown must be refused: {refused}"
        );
        assert_eq!(
            typed_code(&refused),
            ("engine_dead".into(), "transport".into()),
            "the refusal is typed: {refused}"
        );
        assert!(
            text_of(&refused).contains("shutting down"),
            "the refusal names the reason: {}",
            text_of(&refused)
        );
        assert_eq!(
            rig.browser.generation(),
            0,
            "a refused acquisition must not launch a browser"
        );
        assert!(!rig.browser.is_live().await);
        assert!(
            rig.reg.names().await.is_empty(),
            "the refused acquisition removes its own entry: {:?}",
            rig.reg.names().await
        );
        rig.close().await;
    });
}

/// A stale "the engine no longer has this context" report retires the
/// acquisition that observed it, never a successor that reused the name.
#[test]
fn a_stale_vanished_report_cannot_retire_a_recreated_name() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let created = rig.call(json!({"action": "create", "name": "v"})).await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let old = sc(&created)["browserContextId"]
            .as_str()
            .expect("browserContextId")
            .to_string();
        rig.browser
            .cdp_raw(
                "Target.disposeBrowserContext",
                json!({"browserContextId": old}),
                None,
            )
            .await
            .expect("out-of-band dispose");

        let hooks = EnvHooks::new(&[("HAND_CONTEXT_TEST_SLOW_BEFORE_VANISH_MS", "600")]);
        let stale = tokio::spawn({
            let reg = Arc::clone(&rig.reg);
            async move {
                contexts::dispatch(TOOL, &json!({"action": "cookies", "name": "v"}), &reg)
                    .await
                    .expect("dispatch claims the tool")
            }
        });
        tokio::time::sleep(Duration::from_millis(150)).await;

        let destroyed = rig.call(json!({"action": "destroy", "name": "v"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy of the vanished context failed: {}",
            text_of(&destroyed)
        );
        let recreated = rig.call(json!({"action": "create", "name": "v"})).await;
        assert!(
            !is_error(&recreated),
            "re-create failed: {}",
            text_of(&recreated)
        );
        let new = sc(&recreated)["browserContextId"]
            .as_str()
            .expect("browserContextId")
            .to_string();
        assert_ne!(old, new);
        drop(hooks);

        let stale_out = stale.await.expect("join the stale report");
        assert!(
            is_error(&stale_out),
            "the stale report is still a failure: {stale_out}"
        );
        assert_eq!(
            typed_code(&stale_out),
            ("invalid_target".into(), "not_found".into())
        );

        let listing = rig.call(json!({"action": "list"})).await;
        let entry = listed(&listing, "v").expect("the re-created context is still tracked");
        assert_eq!(
            entry["browserContextId"].as_str(),
            Some(new.as_str()),
            "tracking still points at the successor: {entry}"
        );
        assert!(
            rig.browser.excluded_browser_contexts().contains_key(&new),
            "the successor's legacy exclusion stays"
        );
        assert!(
            !rig.browser.excluded_browser_contexts().contains_key(&old),
            "the vanished id's exclusion was released"
        );
        let value = rig
            .call(json!({"action": "eval", "name": "v", "expression": "1+1"}))
            .await;
        assert_eq!(
            sc(&value)["value"],
            json!(2),
            "the successor still works: {}",
            text_of(&value)
        );
        rig.close().await;
    });
}

/// Document identity, not a URL string: a fragment/`pushState` navigation moves
/// the URL while the document (and its live-read title) stays; a real
/// navigation is a new document and the cached title is replaced, labelled.
#[test]
fn a_same_document_navigation_keeps_the_title_and_a_real_one_replaces_it() {
    let _serial = serial();
    require_owned_browser();
    block_on(async {
        let rig = Rig::start();
        let fixture = Fixture::start();
        let created = rig
            .call(json!({"action": "create", "name": "nav", "url": format!("{}/a", fixture.base)}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        assert_eq!(sc(&created)["title"].as_str(), Some("Titled A"));

        let pushed = rig
            .call(json!({"action": "eval", "name": "nav",
                   "expression": "history.pushState({}, '', '/a?stage=2#frag'); document.title"}))
            .await;
        assert_eq!(
            sc(&pushed)["value"].as_str(),
            Some("Titled A"),
            "the document is unchanged: {}",
            text_of(&pushed)
        );
        let shown = rig.call(json!({"action": "show", "name": "nav"})).await;
        let page = &sc(&shown)["pages"][0];
        assert_eq!(
            page["url"].as_str(),
            Some(format!("{}/a?stage=2#frag", fixture.base).as_str()),
            "the reported url did move: {shown}"
        );
        assert_eq!(
            page["title"].as_str(),
            Some("Titled A"),
            "a same-document navigation keeps the live title: {shown}"
        );
        assert_eq!(
            page["source"].as_str(),
            Some("targetInfo+liveRead"),
            "and keeps saying where it came from: {shown}"
        );

        let opened = rig
            .call(json!({"action": "open", "name": "nav", "url": format!("{}/auto", fixture.base)}))
            .await;
        assert!(!is_error(&opened), "open failed: {}", text_of(&opened));
        assert_eq!(
            sc(&opened)["page"]["title"].as_str(),
            Some("Auto A"),
            "the page starts as its first document: {}",
            text_of(&opened)
        );
        let auto_target = sc(&opened)["page"]["targetId"]
            .as_str()
            .expect("targetId")
            .to_string();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let shown = rig.call(json!({"action": "show", "name": "nav"})).await;
            assert!(!is_error(&shown), "show failed: {}", text_of(&shown));
            let page = sc(&shown)["pages"]
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .find(|p| p["targetId"].as_str() == Some(&auto_target))
                })
                .cloned()
                .expect("the self-navigating page is tracked");
            if page["url"].as_str().is_some_and(|u| u.ends_with("/b")) {
                assert_eq!(
                    page["source"].as_str(),
                    Some("targetInfo"),
                    "a new document is relabelled from target info: {shown}"
                );
                // Chrome 158 can leave the target-info title as the URL for a
                // new document before the real <title> propagates. Either value
                // proves the stale "Auto A" was replaced; "Auto A" does not.
                let title = page["title"].as_str().unwrap_or("");
                assert!(
                    title == "Titled B" || title.ends_with("/b"),
                    "the stale title must be replaced: {shown}"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the self-navigation must commit and refresh: {shown}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let destroyed = rig.call(json!({"action": "destroy", "name": "nav"})).await;
        assert!(
            !is_error(&destroyed),
            "destroy failed: {}",
            text_of(&destroyed)
        );
        rig.close().await;
    });
}

/// An externally started engine this test owns only because it launched it: it
/// stands in for "someone else's browser" that hand attaches to.
struct ExternalEngine {
    child: std::process::Child,
    profile: std::path::PathBuf,
    base: String,
}

impl ExternalEngine {
    fn start() -> Self {
        let chromium = browser_engine::chromium_path()
            .expect("the pinned chrome-headless-shell must be provisioned");
        let profile =
            std::env::temp_dir().join(format!("hand-attached-contexts-{}", std::process::id()));
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
        while Instant::now() < deadline {
            if http_get(&base, "/json/version").contains("webSocketDebuggerUrl") {
                return ExternalEngine {
                    child,
                    profile,
                    base,
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut me = ExternalEngine {
            child,
            profile,
            base,
        };
        let _ = me.child.kill();
        let _ = me.child.wait();
        panic!("the external engine did not become ready on {}", me.base);
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

/// Attached-engine semantics with two INDEPENDENT CDP connections to one
/// external browser. Each `Browser` has its own registry, which is the shape of
/// "another client's contexts": hand must dispose only contexts it created,
/// and `disposeOnDetach` must remove only hand's own contexts when it leaves.
///
/// This exercises a CLEAN detach (`Browser::close` sends a close frame). It does
/// not test a hard kill: an abrupt socket drop was measured to close the
/// context's pages while possibly leaving the context object behind, and hand
/// makes no cleanup claim for that case (see `contexts.rs`'s residual note).
///
/// `#[ignore]` on purpose: it sets `GLADIUS_CDP_URL` for the whole test
/// process, which would misroute any test running beside it. Run it alone:
///
/// ```text
/// cargo test --test browser_contexts -- --ignored --test-threads=1 attached_engine
/// ```
#[test]
#[ignore = "sets GLADIUS_CDP_URL process-wide; run alone: cargo test --test browser_contexts -- --ignored --test-threads=1 attached_engine"]
fn attached_engine_keeps_other_clients_contexts_alive() {
    let _serial = serial();
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("GLADIUS_CDP_URL");
        }
    }

    require_owned_browser();
    let engine = ExternalEngine::start();
    std::env::set_var("GLADIUS_CDP_URL", &engine.base);
    let _env = EnvGuard;
    let fixture = Fixture::start();

    block_on(async {
        let owner = Rig::start();
        let guest = Rig::start();

        let a = owner
            .call(json!({"action": "create", "name": "owner", "url": format!("{}/", fixture.base)}))
            .await;
        assert!(!is_error(&a), "owner create failed: {}", text_of(&a));
        assert!(
            sc(&a)["engine"]
                .as_str()
                .unwrap_or("")
                .contains("external-cdp"),
            "the create result names the attached engine: {a}"
        );
        let owner_ctx = sc(&a)["browserContextId"].as_str().unwrap().to_string();
        let seed = owner
            .call(json!({"action": "eval", "name": "owner",
                   "expression": "document.cookie='owner=1; path=/'; document.cookie"}))
            .await;
        assert_eq!(sc(&seed)["value"].as_str(), Some("owner=1"));

        let b = guest
            .call(json!({"action": "create", "name": "guest", "url": format!("{}/", fixture.base)}))
            .await;
        assert!(!is_error(&b), "guest create failed: {}", text_of(&b));
        let guest_ctx = sc(&b)["browserContextId"].as_str().unwrap().to_string();
        assert_ne!(owner_ctx, guest_ctx);

        let destroyed = guest
            .call(json!({"action": "destroy", "name": "guest"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "guest destroy failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(true));

        let owner_cookie = owner
            .call(json!({"action": "eval", "name": "owner", "expression": "document.cookie"}))
            .await;
        assert_eq!(
            sc(&owner_cookie)["value"].as_str(),
            Some("owner=1"),
            "the other client's context must survive hand's teardown: {owner_cookie}"
        );
        let cross = owner
            .browser
            .cdp_raw(
                "Storage.getCookies",
                json!({"browserContextId": guest_ctx}),
                None,
            )
            .await
            .expect_err("a disposed context must be gone on the engine");
        assert!(
            cross.contains("Failed to find browser context"),
            "unexpected cross-client probe: {cross}"
        );

        let c = guest
            .call(json!({"action": "create", "name": "guest2"}))
            .await;
        assert!(!is_error(&c), "guest2 create failed: {}", text_of(&c));
        let guest2_ctx = sc(&c)["browserContextId"].as_str().unwrap().to_string();
        guest.browser.close().await.expect("guest detach");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match owner
                .browser
                .cdp_raw(
                    "Storage.getCookies",
                    json!({"browserContextId": guest2_ctx}),
                    None,
                )
                .await
            {
                Err(e) if e.contains("Failed to find browser context") => break,
                _ => {
                    assert!(
                        Instant::now() < deadline,
                        "disposeOnDetach must remove hand's context when its connection closes cleanly (Browser::close), not on a hard kill"
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
        let owner_after = owner
            .call(json!({"action": "eval", "name": "owner", "expression": "document.cookie"}))
            .await;
        assert_eq!(
            sc(&owner_after)["value"].as_str(),
            Some("owner=1"),
            "another client's disconnect must not disturb the other context: {owner_after}"
        );

        let destroyed = owner
            .call(json!({"action": "destroy", "name": "owner"}))
            .await;
        assert!(
            !is_error(&destroyed),
            "owner destroy failed: {}",
            text_of(&destroyed)
        );
        assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(true));
        owner.browser.close().await.expect("owner detach");
    });

    assert!(
        http_get(&engine.base, "/json/version").contains("webSocketDebuggerUrl"),
        "an attached engine must be left running after hand disconnects"
    );
}

/// A TCP proxy in front of the external engine, so a test can cut hand's CDP
/// connection while the browser itself keeps running. That is the shape of a
/// real attached-engine connection loss.
struct Proxy {
    port: u16,
    stop: Arc<AtomicBool>,
    cut: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
}

impl Proxy {
    fn start(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
        listener
            .set_nonblocking(true)
            .expect("nonblocking proxy listener");
        let port = listener.local_addr().expect("proxy addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let cut = Arc::new(AtomicBool::new(false));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::clone(&stop);
        let cut_flag = Arc::clone(&cut);
        let registry = Arc::clone(&conns);
        let upstream = upstream.to_string();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((client, _)) => {
                        let _ = client.set_nonblocking(false);
                        if cut_flag.load(Ordering::SeqCst) {
                            let _ = client.shutdown(std::net::Shutdown::Both);
                            continue;
                        }
                        let Ok(server) = TcpStream::connect(&upstream) else {
                            let _ = client.shutdown(std::net::Shutdown::Both);
                            continue;
                        };
                        let (Ok(c1), Ok(s1), Ok(c2), Ok(s2)) = (
                            client.try_clone(),
                            server.try_clone(),
                            client.try_clone(),
                            server.try_clone(),
                        ) else {
                            continue;
                        };
                        registry.lock().unwrap().extend([c1, s1, c2, s2]);
                        std::thread::spawn(move || pump(client, server));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Proxy {
            port,
            stop,
            cut,
            conns,
            handle: Some(handle),
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Drop every proxied connection abruptly (no WebSocket close frame) and
    /// refuse new ones until `restore`.
    fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
        for s in self.conns.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn restore(&self) {
        self.cut.store(false, Ordering::SeqCst);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.cut();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn pump(mut a: TcpStream, mut b: TcpStream) {
    let (mut ar, mut br) = match (a.try_clone(), b.try_clone()) {
        (Ok(ar), Ok(br)) => (ar, br),
        _ => return,
    };
    let t1 = std::thread::spawn(move || {
        let _ = std::io::copy(&mut ar, &mut b);
        let _ = b.shutdown(std::net::Shutdown::Both);
    });
    let t2 = std::thread::spawn(move || {
        let _ = std::io::copy(&mut br, &mut a);
        let _ = a.shutdown(std::net::Shutdown::Both);
    });
    let _ = t1.join();
    let _ = t2.join();
}

/// Attached connection loss proves nothing about the external browser: hand must
/// keep the context tracked and excluded, report UNKNOWN (not "gone"), never
/// claim a disposal, and reconcile on a later re-attach.
///
/// `#[ignore]` for the same reason as the other attached test: `GLADIUS_CDP_URL`
/// is process-wide. Run alone:
///
/// ```text
/// cargo test --test browser_contexts -- --ignored --test-threads=1 attached_connection_loss
/// ```
#[test]
#[ignore = "sets GLADIUS_CDP_URL process-wide; run alone: cargo test --test browser_contexts -- --ignored --test-threads=1 attached_connection_loss"]
fn attached_connection_loss_is_unverified_not_gone_and_reconciles() {
    let _serial = serial();
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("GLADIUS_CDP_URL");
        }
    }

    require_owned_browser();
    let engine = ExternalEngine::start();
    let upstream = engine.base.trim_start_matches("http://").to_string();
    let proxy = Proxy::start(&upstream);
    std::env::set_var("GLADIUS_CDP_URL", proxy.base());
    let _env = EnvGuard;

    block_on(async {
        let rig = Rig::start();
        let created = rig
            .call(json!({"action": "create", "name": "keep", "url": "about:blank"}))
            .await;
        assert!(!is_error(&created), "create failed: {}", text_of(&created));
        let context_id = sc(&created)["browserContextId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            sc(&created)["engine"].as_str(),
            Some("external-cdp (attached; contexts are hand's, the engine is not)")
        );

        proxy.cut();
        let deadline = Instant::now() + Duration::from_secs(10);
        while rig.browser.is_live().await {
            assert!(
                Instant::now() < deadline,
                "the cut connection must be observed as dead"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            http_get(&engine.base, "/json/version").contains("webSocketDebuggerUrl"),
            "the cut must only affect hand's proxied connection"
        );

        let listing = rig.call(json!({"action": "list"})).await;
        assert!(!is_error(&listing), "list failed: {}", text_of(&listing));
        let entry = listed(&listing, "keep").expect("the context is still tracked");
        assert!(
            entry["unverified"]
                .as_str()
                .unwrap_or("")
                .contains("UNKNOWN"),
            "the entry states its fate is unknown: {entry}"
        );
        assert_eq!(
            sc(&listing)["dropped"].as_array().map(|a| a.len()),
            Some(0),
            "an attached loss drops nothing: {listing}"
        );
        assert!(
            rig.browser
                .excluded_browser_contexts()
                .contains_key(&context_id),
            "the legacy exclusion is retained while the external browser may hold the page"
        );

        let destroyed = rig.call(json!({"action": "destroy", "name": "keep"})).await;
        assert!(
            is_error(&destroyed),
            "destroy must not claim success: {destroyed}"
        );
        assert_eq!(
            typed_code(&destroyed),
            ("engine_dead".into(), "transport".into())
        );
        assert!(
            text_of(&destroyed).contains("UNVERIFIED")
                && text_of(&destroyed).contains("outcome unknown"),
            "the report is UNKNOWN, not 'gone with the engine': {}",
            text_of(&destroyed)
        );
        assert_eq!(rig.reg.names().await, vec!["keep".to_string()]);
        assert!(rig
            .browser
            .excluded_browser_contexts()
            .contains_key(&context_id));

        let all = rig.call(json!({"action": "destroy_all"})).await;
        assert!(
            is_error(&all),
            "destroy_all cannot succeed while unverified: {all}"
        );
        assert_eq!(sc(&all)["destroyed"].as_array().map(|a| a.len()), Some(0));
        assert!(
            sc(&all)["unverified"]
                .as_array()
                .map(|a| a.iter().any(|n| n.as_str() == Some("keep")))
                .unwrap_or(false),
            "the receipt lists it as unverified: {all}"
        );
        let receipt = rig.reg.shutdown().await;
        assert_eq!(
            receipt["disposed"].as_array().map(|a| a.len()),
            Some(0),
            "shutdown never claims a disposal it did not perform: {receipt}"
        );
        assert!(
            receipt["unverified"]
                .as_array()
                .map(|a| a.iter().any(|n| n.as_str() == Some("keep")))
                .unwrap_or(false),
            "shutdown reports it as unverified: {receipt}"
        );
        assert_eq!(rig.reg.names().await, vec!["keep".to_string()]);

        proxy.restore();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut reconciled = None;
        while Instant::now() < deadline {
            let r = rig.call(json!({"action": "list"})).await;
            if !is_error(&r) {
                reconciled = Some(r);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let listing = reconciled.expect("hand must re-attach and list after the link is restored");
        let entry = listed(&listing, "keep");
        match entry {
            Some(entry) => {
                assert!(
                    entry["unverified"].is_null(),
                    "a successfully probed context is verified again: {entry}"
                );
                let destroyed = rig.call(json!({"action": "destroy", "name": "keep"})).await;
                assert!(
                    !is_error(&destroyed),
                    "destroy after reconcile failed: {}",
                    text_of(&destroyed)
                );
                assert_eq!(sc(&destroyed)["gone"].as_bool(), Some(true));
                assert!(
                    matches!(
                        sc(&destroyed)["disposedBy"].as_str(),
                        Some("hand") | Some("already-gone")
                    ),
                    "the disposal names who performed it: {destroyed}"
                );
            }
            None => {
                assert!(
                    sc(&listing)["dropped"]
                        .as_array()
                        .map(|a| a.iter().any(|n| n.as_str() == Some("keep")))
                        .unwrap_or(false),
                    "a vanished context is reported in dropped: {listing}"
                );
                let destroyed = rig.call(json!({"action": "destroy", "name": "keep"})).await;
                assert!(
                    is_error(&destroyed),
                    "nothing is left to destroy: {destroyed}"
                );
            }
        }
        assert!(rig.reg.names().await.is_empty());
        rig.browser.close().await.expect("close hand's connection");
    });

    assert!(
        http_get(&engine.base, "/json/version").contains("webSocketDebuggerUrl"),
        "an attached engine must be left running"
    );
}
