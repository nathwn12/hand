//! Representative browser inspect -> act -> verify over the real
//! MCP endpoint, against a local deterministic fixture and the owned pinned
//! `chrome-headless-shell`.
//!
//! This is a live test by design. It proves the hand process launches the
//! engine it reports, not a mock. A missing browser fails the test with the
//! engine's own "provision it" message rather than skipping silently.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use common::{is_error, navigate_with_retry, structured, text_of, tool, Session};
use serde_json::json;

const INDEX: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Hand Fixture</title>
<style>body{font:16px system-ui;margin:24px}button,input,div{font:inherit}</style></head>
<body>
  <h1>Hand fixture</h1>
  <button id="go" type="button">Act</button>
  <input id="name" aria-label="Name" type="text">
  <div id="out">idle</div>
  <a id="next" href="/next">next page</a>
  <script>
    document.querySelector('#go').addEventListener('click', function () {
      document.querySelector('#out').textContent = 'clicked';
    });
  </script>
</body></html>"#;

const NEXT: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Hand Next</title></head>
<body><h1>Second page</h1><button id="other">Other</button></body></html>"#;

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
        "/" => ("200 OK", INDEX, "text/html; charset=utf-8"),
        "/next" => ("200 OK", NEXT, "text/html; charset=utf-8"),
        "/missing" => (
            "404 Not Found",
            "<h1>missing</h1>",
            "text/html; charset=utf-8",
        ),
        "/favicon.ico" => ("204 No Content", "", "text/plain"),
        _ => (
            "404 Not Found",
            "<h1>not found</h1>",
            "text/html; charset=utf-8",
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[test]
fn browser_inspect_act_verify_and_stale_ref_refusal() {
    let chromium = browser_engine::chromium_path()
        .expect("the pinned chrome-headless-shell must be provisioned for this live test");
    assert!(
        chromium.is_file(),
        "chromium path is not a file: {chromium:?}"
    );
    eprintln!("HAND_BROWSER_BIN={}", chromium.display());

    let fixture = Fixture::start();
    let mut s = Session::spawn();

    let nav = navigate_with_retry(&mut s, &format!("{}/", fixture.base));
    assert!(!is_error(&nav), "navigate failed: {}", text_of(&nav));
    let nav_sc = structured(&nav);
    assert_eq!(
        nav_sc["url"].as_str(),
        Some(format!("{}/", fixture.base).as_str())
    );
    assert_eq!(nav_sc["title"].as_str(), Some("Hand Fixture"));

    let snap = s.call("browser_snapshot", json!({}));
    assert!(!is_error(&snap), "snapshot failed: {}", text_of(&snap));
    let snap_sc = structured(&snap);
    assert_eq!(snap_sc["title"].as_str(), Some("Hand Fixture"));
    let nodes = snap_sc["nodes"].as_array().expect("snapshot nodes");
    let button = nodes
        .iter()
        .find(|n| n["role"].as_str() == Some("button") && n["name"].as_str() == Some("Act"))
        .unwrap_or_else(|| panic!("the Act button must be in the AX snapshot: {snap_sc}"));
    let button_ref = button["ref"].as_str().expect("button ref").to_string();
    assert!(
        button_ref.starts_with('e') && button_ref[1..].chars().all(|c| c.is_ascii_digit()),
        "snapshot ref ids are eN (the action tools take @eN): {button_ref}"
    );
    let button_at = format!("@{button_ref}");

    let click = s.call("browser_click", json!({"element": button_at}));
    assert!(!is_error(&click), "click failed: {}", text_of(&click));
    assert_eq!(structured(&click)["ok"].as_bool(), Some(true));

    let out = s.call(
        "browser_evaluate",
        json!({"expression": "document.querySelector('#out').textContent"}),
    );
    assert!(!is_error(&out), "evaluate failed: {}", text_of(&out));
    assert_eq!(
        structured(&out)["value"].as_str(),
        Some("clicked"),
        "the delivered click must have produced the DOM effect"
    );

    // `selector` aliases `element`; both together are refused, never guessed.
    let by_selector = s.call("browser_click", json!({"selector": "#go"}));
    assert!(
        !is_error(&by_selector),
        "click by selector failed: {}",
        text_of(&by_selector)
    );
    assert_eq!(structured(&by_selector)["ok"].as_bool(), Some(true));
    let both = s.call("browser_click", json!({"selector": "#go", "element": "#go"}));
    assert!(is_error(&both), "both target keys must be refused: {both}");
    let both_sc = structured(&both);
    assert_eq!(both_sc["code"].as_str(), Some("invalid_args"), "{both_sc}");
    assert_eq!(both_sc["reason"].as_str(), Some("ambiguous_target"), "{both_sc}");

    let typed = s.call(
        "browser_type",
        json!({"element": "#name", "text": "hello hand"}),
    );
    assert!(!is_error(&typed), "type failed: {}", text_of(&typed));
    let value = s.call(
        "browser_evaluate",
        json!({"expression": "document.querySelector('#name').value"}),
    );
    assert_eq!(structured(&value)["value"].as_str(), Some("hello hand"));

    let shot = s.call("browser_screenshot", json!({"deterministic": true}));
    assert!(!is_error(&shot), "screenshot failed: {}", text_of(&shot));
    let shot_sc = structured(&shot);
    assert_eq!(shot_sc["mimeType"].as_str(), Some("image/png"));
    assert!(
        shot_sc["bytes"].as_u64().unwrap_or(0) > 1000,
        "a PNG of the fixture is not empty: {shot_sc}"
    );
    assert!(
        shot_sc["checksum"].as_str().unwrap_or("").len() >= 8,
        "deterministic capture reports a checksum: {shot_sc}"
    );

    let next = navigate_with_retry(&mut s, &format!("{}/next", fixture.base));
    assert!(
        !is_error(&next),
        "second navigate failed: {}",
        text_of(&next)
    );
    assert_eq!(structured(&next)["title"].as_str(), Some("Hand Next"));

    let stale = s.call("browser_click", json!({"element": button_at}));
    assert!(
        is_error(&stale),
        "a ref from before navigation must not act on the replacement page: {stale}"
    );
    let stale_sc = structured(&stale);
    let code = stale_sc["code"].as_str().unwrap_or("");
    assert!(
        matches!(code, "stale_ref" | "invalid_target" | "target_changed"),
        "stale ref must be a documented typed failure, got code {code:?}: {stale}"
    );
    assert!(
        text_of(&stale).contains("stale"),
        "prose names staleness: {}",
        text_of(&stale)
    );

    let alive = s.call("browser_evaluate", json!({"expression": "document.title"}));
    assert_eq!(structured(&alive)["value"].as_str(), Some("Hand Next"));

    // An undefined result is a void success; an explicit null is not void.
    let void = s.call("browser_evaluate", json!({"expression": "void 0"}));
    assert!(!is_error(&void), "void 0 must be ok: {}", text_of(&void));
    let void_sc = structured(&void);
    assert_eq!(void_sc["value"], json!(null), "{void_sc}");
    assert_eq!(void_sc["void"], json!(true), "{void_sc}");

    let nul = s.call("browser_evaluate", json!({"expression": "null"}));
    assert!(!is_error(&nul), "null must be ok: {}", text_of(&nul));
    let nul_sc = structured(&nul);
    assert_eq!(nul_sc["value"], json!(null), "{nul_sc}");
    assert!(
        nul_sc.get("void").is_none(),
        "explicit null must not set void: {nul_sc}"
    );
}

/// `selector` is a published alias: a client that prunes undeclared keys must
/// see it in the schema. `element` stays canonical and both are named in anyOf.
#[test]
fn element_tools_publish_selector_alias_in_their_schema() {
    let mut s = Session::spawn();
    let tools = s.list_tools();
    for name in [
        "browser_click",
        "browser_type",
        "browser_select",
        "browser_hover",
        "browser_upload",
        "browser_screenshot",
    ] {
        let schema = &tool(&tools, name)["inputSchema"];
        assert!(
            schema["properties"]["selector"].is_object(),
            "{name} must publish selector: {schema}"
        );
        assert!(
            schema["properties"]["element"].is_object(),
            "{name} must keep element canonical: {schema}"
        );
        let any = schema["anyOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} must require one target via anyOf: {schema}"));
        let rendered = serde_json::to_string(any).expect("anyOf serializes");
        assert!(
            rendered.contains("element") && rendered.contains("selector"),
            "{name} anyOf must name element and selector: {rendered}"
        );
    }
}

#[test]
fn unknown_arg_reports_the_bogus_key_and_sorted_received_keys() {
    let mut s = Session::spawn();
    let resp = s.call("browser_click", json!({"element": "#a", "bogus": 1, "aaa": 2}));
    assert_eq!(resp["result"]["isError"], json!(true));
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["code"].as_str(), Some("invalid_args"));
    assert_eq!(sc["reason"].as_str(), Some("unknown_arg"));
    let received: Vec<&str> = sc["received"]
        .as_array()
        .expect("received keys on invalid_args")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(received, vec!["aaa", "bogus", "element"], "sorted keys");
    assert!(text_of(&resp).contains("bogus"), "prose names the bogus key");
}

#[test]
fn missing_arg_reports_sorted_received_keys() {
    let mut s = Session::spawn();
    let resp = s.call("browser_click", json!({"frame": "x"}));
    assert_eq!(resp["result"]["isError"], json!(true));
    let sc = &resp["result"]["structuredContent"];
    assert_eq!(sc["reason"].as_str(), Some("missing_arg"));
    let received: Vec<&str> = sc["received"]
        .as_array()
        .expect("received keys on missing_arg")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(received, vec!["frame"]);
}
