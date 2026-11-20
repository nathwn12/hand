//! The four browser P0 rows from the 0.2.0 addendum, one live browser at a
//! time. Every assertion reads the page's own observation (a real redirect,
//! a frame's own DOM, a trusted drag/drop handler, a real form submission),
//! never the tool's claim. Fixtures are inline; no files under fixtures/.

mod common;

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// One browser at a time in this binary: concurrent launches flake on this
/// machine. The guard is held for each whole test body.
static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner())
}

const NAV: &str = r#"<!doctype html><html><head><title>Nav Page</title></head>
<body><h1>nav</h1></body></html>"#;
const NEXT: &str = r#"<!doctype html><html><head><title>Next Page</title></head>
<body><h1>next</h1></body></html>"#;

struct Fixture {
    base: String,
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        socket.set_nonblocking(true).expect("nonblocking");
        let port = socket.local_addr().expect("addr").port();
        let base = format!("http://127.0.0.1:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handles = vec![std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match socket.accept() {
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
        })];
        Fixture { base, stop, handles }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let (status, body): (&str, String) = match path.as_str() {
        "/" => ("200 OK", NAV.to_string()),
        "/next" => ("200 OK", NEXT.to_string()),
        "/redirect" => {
            let head = "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\
                        Cache-Control: no-store\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            return Ok(());
        }
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

#[test]
fn b12_navigate_reports_observed_status_redirects_timings() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let r = s.call("browser_navigate", json!({"url": format!("{}/redirect", fx.base)}));
    assert!(!is_error(&r), "redirect navigation must succeed: {}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["url"].as_str().unwrap_or(""), format!("{}/next", fx.base));
    assert_eq!(sc["title"].as_str().unwrap_or(""), "Next Page");

    let obs = &sc["observed"];
    assert_eq!(obs["status"].as_i64(), Some(200), "real navigation status");
    let chain = obs["redirects"].as_array().expect("redirects is an array");
    assert!(!chain.is_empty(), "a 302 hop must appear in the redirect chain");
    assert!(
        chain.iter().any(|h| h["url"].as_str().unwrap_or("").contains("/redirect")),
        "chain must name the redirect hop: {chain:?}"
    );
    let tm = &obs["timings"];
    for k in ["request", "response", "finish"] {
        assert!(tm[k].as_f64().unwrap_or(-1.0) >= 0.0, "timing {k} missing: {tm:?}");
    }

    // A same-document (fragment) navigation never hits the network: there is
    // no HTTP status to report, so observed.status is null with a note.
    let r2 = s.call("browser_navigate", json!({"url": format!("{}/next#frag", fx.base)}));
    assert!(!is_error(&r2), "fragment navigation must succeed: {}", text_of(&r2));
    let o2 = &structured(&r2)["observed"];
    assert!(o2["status"].is_null(), "client-side navigation status must be null: {o2:?}");
    assert!(
        o2["note"].as_str().unwrap_or("").to_ascii_lowercase().contains("null"),
        "null status must say so: {o2:?}"
    );
}
