//! Element targeting over the real MCP stdio route: the additive AX selector
//! forms (`role=`, `role=[name=]`, `name=`, `text=`, `label=`) resolve on the
//! accessibility tree the snapshot renders, are uniqueness-mandatory, honour
//! `frame=`, and report what they resolved; the legacy forms (CSS first match,
//! exact `@eN`) keep behaving exactly as before.
//!
//! Live by design, against a loopback fixture and the owned pinned
//! `chrome-headless-shell`: a missing browser fails with the engine's own
//! "provision it" message instead of skipping silently.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};

const LONG: &str = "This accessible name is deliberately longer than eighty characters so the snapshot has to truncate it somewhere near the cap";

const INDEX: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Selector fixture</title>
<style>body{font:16px system-ui;margin:24px}button,input{font:inherit}</style></head>
<body>
  <h1>Selector fixture</h1>
  <button id="save" type="button">Save draft</button>
  <button id="go" type="button">Go</button>
  <button class="danger" type="button">Delete</button>
  <button class="danger" type="button">Delete</button>
  <a id="learn" href="#learn">Learn more</a>
  <div id="wrap"><p id="para">Paragraph body</p></div>
  <p id="split">Hel<span>lo</span></p>
  <p id="hiddenp" style="display:none">Hidden words</p>
  <p id="ariahidden"><span aria-hidden="true">Aria hidden words</span></p>
  <label for="email">Email address</label>
  <input id="email" type="text">
  <input id="search" type="text" aria-label="Search box">
  <input id="unlabelled">
  <input id="terms" type="checkbox" aria-label="Accept terms">
  <label for="postal">Postal <b>code</b></label>
  <input id="postal" type="text">
  <label for="nbsp">Name&nbsp;&nbsp;spaced</label>
  <input id="nbsp" type="text">
  <button id="longname" type="button" aria-label="__LONG__">Long name</button>
  <button id="quote" type="button">Say "hi"</button>
  <div id="out">idle</div>
  <iframe id="fr" src="/frame" width="420" height="180"></iframe>
  <script>
    const out = document.querySelector('#out');
    const mark = (v) => { out.textContent = v; };
    for (const [id, v] of [['save','main-hit'],['go','go-hit']]) {
      document.querySelector('#' + id).addEventListener('click', function () { mark(v); });
    }
    for (const b of document.querySelectorAll('.danger')) {
      b.addEventListener('click', function () { mark('delete-hit'); });
    }
  </script>
</body></html>"##;

const FRAME: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Selector frame</title></head>
<body>
  <button id="frame-save" type="button">Save draft</button>
  <label for="fname">Frame name</label>
  <input id="fname" type="text">
  <div id="fout">idle</div>
  <script>
    document.querySelector('#frame-save').addEventListener('click', function () {
      document.querySelector('#fout').textContent = 'frame-hit';
    });
  </script>
</body></html>"##;

const NEXT: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Selector next</title></head>
<body><h1>Next page</h1><button id="other" type="button">Other</button></body></html>"##;

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
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let index = INDEX.replace("__LONG__", LONG);
    let (status, body) = match path {
        "/" => ("200 OK", index.clone()),
        "/frame" => ("200 OK", FRAME.to_string()),
        "/next" => ("200 OK", NEXT.to_string()),
        "/favicon.ico" => ("204 No Content", String::new()),
        _ => ("404 Not Found", "<h1>missing</h1>".to_string()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// One browser at a time inside this binary. Every test launches a real
/// `chrome-headless-shell`; a dozen launching at once on this machine can fail
/// a navigation with ERR_SOCKET_NOT_CONNECTED before any selector runs (measured:
/// 9 concurrent launches flaky, 4 never). Serialising the browsers keeps the
/// assertions about selectors, not about the machine.
static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The pinned engine must be provisioned: fail with its own message, never
/// skip.
fn require_engine() {
    let chromium = browser_engine::chromium_path()
        .expect("the pinned chrome-headless-shell must be provisioned for this live test");
    assert!(
        chromium.is_file(),
        "chromium path is not a file: {chromium:?}"
    );
}

/// A navigated session on the fixture index page.
fn opened(fixture: &Fixture) -> Session {
    let mut s = Session::spawn();
    let nav = s.call(
        "browser_navigate",
        json!({"url": format!("{}/", fixture.base)}),
    );
    assert!(!is_error(&nav), "navigate failed: {}", text_of(&nav));
    s
}

fn ok(s: &mut Session, tool: &str, args: Value) -> Value {
    let resp = s.call(tool, args);
    assert!(
        !is_error(&resp),
        "{tool} must succeed, got: {}",
        text_of(&resp)
    );
    structured(&resp).clone()
}

fn eval(s: &mut Session, expression: &str) -> Value {
    ok(s, "browser_evaluate", json!({ "expression": expression }))["value"].clone()
}

fn eval_in_frame(s: &mut Session, frame: &str, expression: &str) -> Value {
    ok(
        s,
        "browser_evaluate",
        json!({ "expression": expression, "frame": frame }),
    )["value"]
        .clone()
}

/// The snapshot node a ref names, so a reported @eN can be checked against the
/// render the caller actually sees.
fn snapshot_node(s: &mut Session, filter: Value) -> Value {
    let snap = ok(s, "browser_snapshot", json!({}));
    let nodes = snap["nodes"].as_array().cloned().unwrap_or_default();
    let want = |n: &Value, k: &str| n[k].as_str() == filter[k].as_str();
    nodes
        .iter()
        .find(|n| want(n, "role") && want(n, "name"))
        .unwrap_or_else(|| panic!("no snapshot node with {filter}: {snap}"))
        .clone()
}

fn frame_id(s: &mut Session, url_suffix: &str) -> String {
    let targets = ok(s, "browser_targets", json!({"action": "list"}));
    targets["frames"]
        .as_array()
        .and_then(|f| {
            f.iter()
                .find(|f| f["url"].as_str().is_some_and(|u| u.ends_with(url_suffix)))
        })
        .and_then(|f| f["frameId"].as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| panic!("no frame for {url_suffix}: {targets}"))
}

#[test]
fn role_selector_resolves_the_unique_role_and_reports_the_snapshot_ref() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);
    let heading = snapshot_node(
        &mut s,
        json!({"role": "heading", "name": "Selector fixture"}),
    );

    let reply = ok(&mut s, "browser_click", json!({"element": "role=heading"}));
    let resolved = &reply["resolved"];
    assert_eq!(resolved["role"].as_str(), Some("heading"), "{reply}");
    assert_eq!(
        resolved["name"].as_str(),
        Some("Selector fixture"),
        "{reply}"
    );
    assert_eq!(
        resolved["selector"].as_str(),
        Some("role=heading"),
        "the reply names the selector it resolved: {reply}"
    );
    assert_eq!(
        resolved["ref"].as_str(),
        Some(format!("@{}", heading["ref"].as_str().unwrap()).as_str()),
        "the resolved element is the @eN the snapshot already lists: {reply}"
    );
    assert_eq!(reply["ok"].as_bool(), Some(true), "{reply}");
}

#[test]
fn role_with_name_acts_on_exactly_that_element() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let reply = ok(
        &mut s,
        "browser_click",
        json!({"element": "role=button[name=\"Go\"]"}),
    );
    assert_eq!(reply["resolved"]["role"].as_str(), Some("button"));
    assert_eq!(reply["resolved"]["name"].as_str(), Some("Go"));
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("go-hit"),
        "the click must land on the Go button, not the first button"
    );
}

#[test]
fn name_selector_acts_by_accessible_name() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let reply = ok(
        &mut s,
        "browser_click",
        json!({"element": "name=\"Save draft\""}),
    );
    assert_eq!(reply["resolved"]["role"].as_str(), Some("button"));
    assert_eq!(reply["resolved"]["name"].as_str(), Some("Save draft"));
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("main-hit"),
        "name= must act on the element with that accessible name"
    );
}

#[test]
fn text_selector_matches_visible_text_and_is_case_insensitive() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let para = ok(
        &mut s,
        "browser_click",
        json!({"element": "text=\"Paragraph body\""}),
    );
    assert_eq!(
        para["resolved"]["role"].as_str(),
        Some("paragraph"),
        "{para}"
    );

    let link = ok(
        &mut s,
        "browser_click",
        json!({"element": "text=\"Learn more\""}),
    );
    assert_eq!(link["resolved"]["role"].as_str(), Some("link"), "{link}");

    let dup = s.call("browser_click", json!({"element": "text=\"delete\""}));
    assert!(
        is_error(&dup),
        "two Delete buttons must not be guessed: {dup}"
    );
    assert_eq!(
        structured(&dup)["reason"].as_str(),
        Some("ambiguous"),
        "{dup}"
    );
    assert_eq!(
        structured(&dup)["candidates"].as_array().map(|c| c.len()),
        Some(2),
        "{dup}"
    );
}

#[test]
fn label_selector_targets_the_associated_control_not_an_aria_label() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let reply = ok(
        &mut s,
        "browser_type",
        json!({"element": "label=\"Email address\"", "text": "hello@example.test"}),
    );
    assert_eq!(
        reply["resolved"]["role"].as_str(),
        Some("textbox"),
        "{reply}"
    );
    assert_eq!(
        reply["resolved"]["name"].as_str(),
        Some("Email address"),
        "{reply}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#email').value").as_str(),
        Some("hello@example.test"),
        "label= must type into the labelled control"
    );

    let miss = s.call("browser_click", json!({"element": "label=\"Search box\""}));
    assert!(
        is_error(&miss),
        "an aria-label is not a <label> association: {miss}"
    );
    assert_eq!(structured(&miss)["reason"].as_str(), Some("not_found"));
    assert!(
        text_of(&miss).contains("label=\"Search box\""),
        "the refusal names the selector: {}",
        text_of(&miss)
    );
}

#[test]
fn ambiguous_selector_refuses_and_lists_every_candidate() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);
    let snap = ok(&mut s, "browser_snapshot", json!({}));
    let buttons: Vec<Value> = snap["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n["role"].as_str() == Some("button"))
        .collect();
    assert!(buttons.len() > 1, "fixture has several buttons: {snap}");

    let resp = s.call("browser_click", json!({"element": "role=button"}));
    assert!(
        is_error(&resp),
        "several buttons must not be guessed: {resp}"
    );
    let sc = structured(&resp);
    assert_eq!(sc["code"].as_str(), Some("invalid_target"), "{resp}");
    assert_eq!(sc["reason"].as_str(), Some("ambiguous"), "{resp}");
    assert_eq!(
        sc["element"].as_str(),
        Some("role=button"),
        "the refusal names the selector: {resp}"
    );
    let cands = sc["candidates"].as_array().expect("candidates");
    assert_eq!(cands.len(), buttons.len(), "{resp}");
    for cand in cands {
        assert_eq!(cand["role"].as_str(), Some("button"), "{cand}");
        let at = cand["ref"]
            .as_str()
            .unwrap_or_else(|| panic!("every candidate carries an @eN: {cand}"));
        assert!(
            at.starts_with('@') && at[1..].starts_with('e'),
            "candidate refs are @eN: {cand}"
        );
        let listed = buttons.iter().any(|b| b["ref"].as_str() == Some(&at[1..]));
        assert!(listed, "candidate {at} must be in the snapshot: {snap}");
        assert!(
            text_of(&resp).contains(at),
            "the prose lists {at}: {}",
            text_of(&resp)
        );
    }
    let mut refs: Vec<&str> = cands.iter().filter_map(|c| c["ref"].as_str()).collect();
    refs.sort_unstable();
    let unique = refs.len();
    refs.dedup();
    assert_eq!(unique, refs.len(), "candidates are unique elements: {resp}");
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("idle"),
        "an ambiguous selector must not click anything"
    );
}

#[test]
fn zero_matches_is_a_typed_not_found_naming_the_selector() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let resp = s.call(
        "browser_click",
        json!({"element": "role=button[name=\"No such button\"]"}),
    );
    assert!(is_error(&resp), "a missing element is a failure: {resp}");
    let sc = structured(&resp);
    assert_eq!(sc["code"].as_str(), Some("invalid_target"), "{resp}");
    assert_eq!(sc["reason"].as_str(), Some("not_found"), "{resp}");
    assert_eq!(
        sc["element"].as_str(),
        Some("role=button[name=\"No such button\"]"),
        "{resp}"
    );
    assert!(
        text_of(&resp).contains("role=button[name=\"No such button\"]"),
        "the refusal names the selector: {}",
        text_of(&resp)
    );
    assert!(
        text_of(&resp).contains("ARIA role string") && text_of(&resp).contains("not a tag name"),
        "a role miss says the value is an ARIA role, not a tag: {}",
        text_of(&resp)
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("idle")
    );
}

#[test]
fn frame_scoped_selector_never_escapes_the_named_frame() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);
    let fid = frame_id(&mut s, "/frame");

    let unrefd = ok(
        &mut s,
        "browser_type",
        json!({"element": "label=\"Frame name\"", "text": "inside-frame", "frame": fid}),
    );
    assert_eq!(
        unrefd["resolved"]["role"].as_str(),
        Some("textbox"),
        "{unrefd}"
    );
    assert!(
        unrefd["resolved"]["ref"].is_null(),
        "no frame render yet means no @eN to report: {unrefd}"
    );
    assert_eq!(
        eval_in_frame(&mut s, &fid, "document.querySelector('#fname').value").as_str(),
        Some("inside-frame"),
        "the frame-scoped type must land inside the frame"
    );

    let frame_snap = ok(&mut s, "browser_snapshot", json!({"frame": fid}));
    let frame_button = frame_snap["nodes"]
        .as_array()
        .and_then(|n| {
            n.iter().find(|n| {
                n["role"].as_str() == Some("button") && n["name"].as_str() == Some("Save draft")
            })
        })
        .cloned()
        .unwrap_or_else(|| panic!("the frame's button must be in its snapshot: {frame_snap}"));

    let reply = ok(
        &mut s,
        "browser_click",
        json!({"element": "name=\"Save draft\"", "frame": fid}),
    );
    assert_eq!(
        reply["resolved"]["ref"].as_str(),
        Some(format!("@{}", frame_button["ref"].as_str().unwrap()).as_str()),
        "the frame-scoped resolution reports the frame render's ref: {reply}"
    );
    assert_eq!(
        eval_in_frame(&mut s, &fid, "document.querySelector('#fout').textContent").as_str(),
        Some("frame-hit"),
        "the click must land inside the frame"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("idle"),
        "the main page's same-named button must not be clicked"
    );

    let escaped = s.call(
        "browser_click",
        json!({"element": "name=\"Search box\"", "frame": fid}),
    );
    assert!(
        is_error(&escaped),
        "a frame-scoped selector must not reach the page: {escaped}"
    );
    assert_eq!(
        structured(&escaped)["reason"].as_str(),
        Some("not_found"),
        "{escaped}"
    );
    assert!(
        text_of(&escaped).contains(&format!("frame '{fid}'")),
        "the refusal names the frame it searched: {}",
        text_of(&escaped)
    );
}

#[test]
fn legacy_css_and_ref_targeting_are_unchanged() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let css = ok(&mut s, "browser_click", json!({"element": ".danger"}));
    assert!(
        css.get("resolved").is_none(),
        "a CSS click gains no new field: {css}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("delete-hit")
    );

    let go = snapshot_node(&mut s, json!({"role": "button", "name": "Go"}));
    let at = format!("@{}", go["ref"].as_str().unwrap());
    let ref_click = ok(&mut s, "browser_click", json!({"element": at}));
    assert!(
        ref_click.get("resolved").is_none(),
        "an @eN click gains no new field: {ref_click}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("go-hit")
    );

    ok(
        &mut s,
        "browser_navigate",
        json!({"url": format!("{}/next", fixture.base)}),
    );
    let stale = s.call("browser_click", json!({"element": at}));
    assert!(is_error(&stale), "a pre-navigation ref must fail: {stale}");
    assert_eq!(
        structured(&stale)["code"].as_str(),
        Some("stale_ref"),
        "{stale}"
    );
}

#[test]
fn hover_resolves_ax_forms_and_keeps_css_behaviour() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let by_role = ok(&mut s, "browser_hover", json!({"element": "role=heading"}));
    assert_eq!(
        by_role["resolved"]["role"].as_str(),
        Some("heading"),
        "hover by role must resolve and dispatch: {by_role}"
    );
    assert_eq!(
        by_role["resolved"]["name"].as_str(),
        Some("Selector fixture")
    );

    let by_name = ok(
        &mut s,
        "browser_hover",
        json!({"element": "name=\"Save draft\""}),
    );
    assert_eq!(
        by_name["resolved"]["role"].as_str(),
        Some("button"),
        "{by_name}"
    );
    assert_eq!(by_name["resolved"]["name"].as_str(), Some("Save draft"));

    let css = ok(&mut s, "browser_hover", json!({"element": "#go"}));
    assert!(
        css.get("resolved").is_none(),
        "a CSS hover gains no new field: {css}"
    );
    assert_eq!(css["ok"].as_bool(), Some(true), "{css}");

    let miss = s.call("browser_hover", json!({"element": "#no-such-element"}));
    assert!(is_error(&miss), "a missing hover target must fail: {miss}");
    assert_eq!(
        structured(&miss)["reason"].as_str(),
        Some("not_found"),
        "{miss}"
    );
}

#[test]
fn text_selector_reads_visible_text_only() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let split = ok(
        &mut s,
        "browser_click",
        json!({"element": "text=\"Hello\""}),
    );
    assert_eq!(
        split["resolved"]["role"].as_str(),
        Some("paragraph"),
        "split text must read as one word: {split}"
    );

    for hidden in ["text=\"Hidden words\"", "text=\"Aria hidden words\""] {
        let miss = s.call("browser_click", json!({"element": hidden}));
        assert!(is_error(&miss), "{hidden} is not visible text: {miss}");
        assert_eq!(
            structured(&miss)["reason"].as_str(),
            Some("not_found"),
            "{miss}"
        );
    }

    let name_not_text = s.call("browser_click", json!({"element": "text=\"Accept terms\""}));
    assert!(
        is_error(&name_not_text),
        "an aria-label is not visible text: {name_not_text}"
    );
    assert_eq!(
        structured(&name_not_text)["reason"].as_str(),
        Some("not_found")
    );
    let by_name = ok(
        &mut s,
        "browser_click",
        json!({"element": "name=\"Accept terms\""}),
    );
    assert_eq!(
        by_name["resolved"]["role"].as_str(),
        Some("checkbox"),
        "{by_name}"
    );
}

#[test]
fn name_accepts_the_names_the_snapshot_prints() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);
    let printed: String = LONG.chars().take(80).collect();

    let snap = ok(&mut s, "browser_snapshot", json!({}));
    let nodes = snap["nodes"].as_array().cloned().unwrap_or_default();
    let long = nodes
        .iter()
        .find(|n| {
            n["role"].as_str() == Some("button") && n["name"].as_str() == Some(printed.as_str())
        })
        .unwrap_or_else(|| panic!("the snapshot prints the capped name {printed:?}: {snap}"));
    assert!(
        long["name"].as_str().unwrap().chars().count() <= 80,
        "the printed name is capped: {long}"
    );

    let by_printed = ok(
        &mut s,
        "browser_click",
        json!({"element": format!("name=\"{printed}\"")}),
    );
    assert_eq!(
        by_printed["resolved"]["role"].as_str(),
        Some("button"),
        "{by_printed}"
    );
    assert_eq!(
        by_printed["resolved"]["name"].as_str(),
        Some(printed.as_str()),
        "{by_printed}"
    );

    let unlabelled = nodes
        .iter()
        .find(|n| n["role"].as_str() == Some("textbox") && n["name"].as_str() == Some("unlabelled"))
        .unwrap_or_else(|| panic!("the snapshot names the unlabeled input from its id: {snap}"));
    assert_eq!(unlabelled["tag"].as_str(), Some("input"), "{unlabelled}");
    let narrowed = ok(
        &mut s,
        "browser_hover",
        json!({"element": "role=textbox[name=\"unlabelled\"]"}),
    );
    assert_eq!(
        narrowed["resolved"]["name"].as_str(),
        Some("unlabelled"),
        "{narrowed}"
    );

    let typed = ok(
        &mut s,
        "browser_type",
        json!({"element": "name=\"unlabelled\"", "text": "synthesized"}),
    );
    assert_eq!(
        typed["resolved"]["name"].as_str(),
        Some("unlabelled"),
        "{typed}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#unlabelled').value").as_str(),
        Some("synthesized"),
        "the synthesized name must type into that control"
    );
}

#[test]
fn label_reads_markup_and_spaced_labels_without_name_guesswork() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let markup = ok(
        &mut s,
        "browser_type",
        json!({"element": "label=\"Postal code\"", "text": "4022"}),
    );
    assert_eq!(
        markup["resolved"]["role"].as_str(),
        Some("textbox"),
        "{markup}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#postal').value").as_str(),
        Some("4022")
    );

    let spaced = ok(
        &mut s,
        "browser_type",
        json!({"element": "label=\"Name spaced\"", "text": "normalised"}),
    );
    assert_eq!(
        spaced["resolved"]["name"].as_str(),
        Some("Name spaced"),
        "{spaced}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#nbsp').value").as_str(),
        Some("normalised")
    );

    let aria = s.call(
        "browser_click",
        json!({"element": "label=\"Accept terms\""}),
    );
    assert!(is_error(&aria), "an aria-label is not a <label>: {aria}");
    assert_eq!(structured(&aria)["reason"].as_str(), Some("not_found"));
}

#[test]
fn malformed_forms_fail_fast_with_the_intended_syntax() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    for sel in [
        "role = button",
        "text = \"Learn more\"",
        "name = \"Go\"",
        "label = \"Email address\"",
    ] {
        let resp = s.call("browser_click", json!({"element": sel}));
        assert!(is_error(&resp), "{sel} must be refused: {resp}");
        let sc = structured(&resp);
        assert_eq!(sc["code"].as_str(), Some("invalid_args"), "{sel}: {resp}");
        assert_eq!(sc["reason"].as_str(), Some("bad_value"), "{sel}: {resp}");
        assert_eq!(sc["element"].as_str(), Some(sel), "{resp}");
        assert!(
            text_of(&resp).contains("role=<role>"),
            "{sel}: the refusal shows the intended syntax: {}",
            text_of(&resp)
        );
    }

    let capital = ok(&mut s, "browser_click", json!({"element": "Name=\"Go\""}));
    assert_eq!(
        capital["resolved"]["role"].as_str(),
        Some("button"),
        "{capital}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("go-hit")
    );

    let unescaped = s.call("browser_click", json!({"element": "name=\"Say \"hi\"\""}));
    assert!(
        is_error(&unescaped),
        "an unescaped quote must be refused: {unescaped}"
    );
    assert_eq!(
        structured(&unescaped)["code"].as_str(),
        Some("invalid_args")
    );
    assert!(
        text_of(&unescaped).contains("escaped with a backslash"),
        "the refusal names escaping: {}",
        text_of(&unescaped)
    );

    let escaped = ok(
        &mut s,
        "browser_click",
        json!({"element": "name=\"Say \\\"hi\\\"\""}),
    );
    assert_eq!(
        escaped["resolved"]["name"].as_str(),
        Some("Say \"hi\""),
        "{escaped}"
    );
}

#[test]
fn page_scope_excludes_iframe_content_and_says_so() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let miss = s.call("browser_click", json!({"element": "label=\"Frame name\""}));
    assert!(
        is_error(&miss),
        "an iframe's content is not part of the page tree: {miss}"
    );
    assert_eq!(
        structured(&miss)["reason"].as_str(),
        Some("not_found"),
        "{miss}"
    );
    let prose = text_of(&miss);
    assert!(
        prose.contains("iframe") && prose.contains("not part of it"),
        "the refusal says iframe content is not searched: {prose}"
    );

    let fid = frame_id(&mut s, "/frame");
    let inside = ok(
        &mut s,
        "browser_click",
        json!({"element": "name=\"Save draft\"", "frame": fid}),
    );
    assert_eq!(
        inside["resolved"]["name"].as_str(),
        Some("Save draft"),
        "{inside}"
    );
}

#[test]
fn resolved_is_declared_in_every_element_tools_schema() {
    let _serial = serial();
    require_engine();
    let mut s = Session::spawn();
    let tools = s.list_tools();
    for name in [
        "browser_click",
        "browser_type",
        "browser_hover",
        "browser_upload",
        "browser_select",
    ] {
        let entry = common::tool(&tools, name);
        let resolved = &entry["outputSchema"]["properties"]["resolved"];
        let props = resolved["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} must declare resolved's properties: {resolved}"));
        for key in ["selector", "ref", "role", "name"] {
            assert!(
                props.contains_key(key),
                "{name}.resolved must declare {key}: {resolved}"
            );
        }
        let required = resolved["required"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            required.len(),
            4,
            "{name}.resolved must require all four fields: {resolved}"
        );
    }
}

#[test]
fn unreadable_frame_is_typed_unreachable_not_a_selector_miss() {
    let _serial = serial();
    require_engine();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);
    let fid = frame_id(&mut s, "/frame");

    ok(
        &mut s,
        "browser_evaluate",
        json!({"expression": "(document.querySelector('#fr').remove(), 'removed')"}),
    );
    let resp = s.call(
        "browser_click",
        json!({"element": "name=\"Save draft\"", "frame": fid}),
    );
    assert!(is_error(&resp), "a detached frame must refuse: {resp}");
    let sc = structured(&resp);
    assert_eq!(sc["code"].as_str(), Some("invalid_target"), "{resp}");
    assert_eq!(
        sc["reason"].as_str(),
        Some("unreachable"),
        "an unreadable frame is not a missing element: {resp}"
    );
    assert_eq!(sc["frame"].as_str(), Some(fid.as_str()), "{resp}");
    let prose = text_of(&resp);
    assert!(
        prose.contains("no AX tree readable") && prose.contains("retrying this call cannot help"),
        "the refusal says the frame, not the selector, is the problem: {prose}"
    );
    assert_eq!(
        eval(&mut s, "document.querySelector('#out').textContent").as_str(),
        Some("idle"),
        "nothing may be clicked on the page instead"
    );
}
