//! Row B3 over the real MCP stdio route: `browser_network` mock, offline,
//! failure injection and restore, proved against a loopback fixture and the
//! owned pinned Chrome 158. One browser at a time, as in `tests/selectors.rs`:
//! concurrent launches make a live navigation flake before any assertion runs.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use common::{is_error, navigate_with_retry, structured, text_of, Session};
use serde_json::{json, Value};

const INDEX: &str = r##"<!doctype html><html><head><meta charset="utf-8"><title>Net fixture</title></head><body>
<button id="net-fetch" type="button">Fetch</button><p id="net-out">network idle</p>
<script>
document.getElementById('net-fetch').addEventListener('click', async () => {
  try {
    const r = await fetch('/network.json', {cache:'no-store'});
    const b = await r.json();
    document.getElementById('net-out').textContent = 'value:' + b.value;
  } catch (e) {
    document.getElementById('net-out').textContent = 'error:' + e.name;
  }
});
</script></body></html>"##;

const JSON: &str = r#"{"ok":true,"value":"real"}"#;

static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner())
}

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        listener.set_nonblocking(true).expect("nonblocking listener");
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
    // Windows accepted sockets inherit nonblocking mode; reset it or a read
    // before the request lands drops the stream as ERR_CONNECTION_ABORTED.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, ctype, body) = match path {
        "/" => ("200 OK", "text/html; charset=utf-8", INDEX.to_string()),
        "/network.json" => ("200 OK", "application/json; charset=utf-8", JSON.to_string()),
        "/favicon.ico" => ("204 No Content", "text/plain", String::new()),
        _ => ("404 Not Found", "text/plain", "<h1>missing</h1>".to_string()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn opened(fixture: &Fixture) -> Session {
    let mut s = Session::spawn();
    let resp = navigate_with_retry(&mut s, &format!("{}/", fixture.base));
    assert!(!is_error(&resp), "navigate failed: {}", text_of(&resp));
    s
}

fn read_out(s: &mut Session) -> String {
    let resp = s.call(
        "browser_evaluate",
        json!({"expression": "document.getElementById('net-out').textContent"}),
    );
    assert!(!is_error(&resp), "evaluate failed: {}", text_of(&resp));
    structured(&resp)["value"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn wait_out(s: &mut Session, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last = String::new();
    while Instant::now() < deadline {
        last = read_out(s);
        if last.contains(want) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(75));
    }
    panic!("net-out never contained {want:?}: {last:?}");
}

fn click_fetch(s: &mut Session) {
    let resp = s.call("browser_click", json!({"element": "#net-fetch"}));
    assert!(!is_error(&resp), "click failed: {}", text_of(&resp));
}

fn net(s: &mut Session, args: Value) -> Value {
    s.call("browser_network", args)
}

#[test]
fn catalog_exposes_the_five_interception_actions_and_block_lists() {
    let _serial = serial();
    let mut s = Session::spawn();
    let tools = s.list_tools();
    let tool = common::tool(&tools, "browser_network");
    let actions = tool["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum");
    let have = actions
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    for action in ["route", "mock", "abort", "offline", "restore"] {
        assert!(have.contains(&action), "missing action {action}: {tool:?}");
    }
    assert!(
        tool["inputSchema"]["properties"]
            .get("blockedUrls")
            .is_some(),
        "the block-lists parameter must be published: {tool:?}"
    );
}

#[test]
fn mock_fulfils_the_paused_request_and_restore_reverts_it() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let armed = net(
        &mut s,
        json!({"action": "mock", "url": "*/network.json", "body": {"ok": true, "value": "mocked"}}),
    );
    assert!(!is_error(&armed), "mock failed: {}", text_of(&armed));
    assert_eq!(structured(&armed)["action"], "mock", "{armed}");

    click_fetch(&mut s);
    assert_eq!(
        wait_out(&mut s, "value:"),
        "value:mocked",
        "the paused fetch must be fulfilled from the mock body"
    );

    let restored = net(&mut s, json!({"action": "restore"}));
    assert!(!is_error(&restored), "restore failed: {}", text_of(&restored));
    assert_eq!(structured(&restored)["offline"], false, "{restored}");

    click_fetch(&mut s);
    assert_eq!(
        wait_out(&mut s, "value:"),
        "value:real",
        "restore must lift the interception, not just forget it"
    );
}

#[test]
fn offline_fails_the_fetch_and_restore_lifts_it() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let armed = net(&mut s, json!({"action": "offline"}));
    assert!(!is_error(&armed), "offline failed: {}", text_of(&armed));
    assert_eq!(structured(&armed)["offline"], true, "{armed}");

    click_fetch(&mut s);
    assert!(
        wait_out(&mut s, "error:").starts_with("error:"),
        "offline must reject the fetch with a page-visible error"
    );

    let restored = net(&mut s, json!({"action": "restore"}));
    assert!(!is_error(&restored), "restore failed: {}", text_of(&restored));

    click_fetch(&mut s);
    assert_eq!(
        wait_out(&mut s, "value:"),
        "value:real",
        "restoring the network must let the same page fetch again"
    );
}

#[test]
fn abort_fails_the_fetch_and_restore_lifts_it() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = opened(&fixture);

    let armed = net(&mut s, json!({"action": "abort", "url": "*/network.json"}));
    assert!(!is_error(&armed), "abort failed: {}", text_of(&armed));

    click_fetch(&mut s);
    assert!(
        wait_out(&mut s, "error:").starts_with("error:"),
        "a failed request must produce a page-visible error"
    );

    let restored = net(&mut s, json!({"action": "restore"}));
    assert!(!is_error(&restored), "restore failed: {}", text_of(&restored));

    click_fetch(&mut s);
    assert_eq!(
        wait_out(&mut s, "value:"),
        "value:real",
        "restore must lift the injected failure"
    );
}

/// The teardown-drop-loss guard shape applied to interception: a route armed
/// against a named context is removed by that context's VERIFIED teardown,
/// not by a later restore. Restoring by the context id after destroy must find
/// nothing left; that is the receipt that the route died with its context.
#[test]
fn a_context_route_dies_with_its_context() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();

    let created = s.call(
        "browser_context",
        json!({"action": "create", "name": "netjar", "url": format!("{}/", fixture.base)}),
    );
    assert!(!is_error(&created), "create failed: {}", text_of(&created));
    let id = structured(&created)["browserContextId"]
        .as_str()
        .expect("browserContextId")
        .to_string();

    let armed = net(
        &mut s,
        json!({"action": "mock", "context": "netjar", "url": "*/network.json",
               "body": {"value": "mocked"}}),
    );
    assert!(!is_error(&armed), "scoped mock failed: {}", text_of(&armed));

    let destroyed = s.call("browser_context", json!({"action": "destroy", "name": "netjar"}));
    assert!(!is_error(&destroyed), "destroy failed: {}", text_of(&destroyed));

    let after = net(&mut s, json!({"action": "restore", "context": id}));
    assert!(!is_error(&after), "restore after teardown failed: {}", text_of(&after));
    assert_eq!(
        structured(&after)["removed"],
        0,
        "teardown must have removed the context's route already: {after}"
    );
}
