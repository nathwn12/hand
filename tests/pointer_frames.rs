//! The interaction slice: pointer completeness (wheel) first, then the rest of
//! the pointer and frame work in later commits.
//!
//! Every test drives the real executable over stdio against a two-origin
//! loopback fixture (`fixtures/interaction/*.html`, served from 127.0.0.1 and
//! `localhost`), so a page is never a data: URL and an iframe is genuinely
//! cross-site. Each behaviour is asserted from the PAGE's own observation of
//! the event (`isTrusted`, `scrollY`, DOM order), never from the tool's claim.

mod common;

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const POINTER: &str = include_str!("../fixtures/interaction/pointer.html");
const CHILD: &str = include_str!("../fixtures/interaction/oopif-child.html");

/// Two loopback listeners on different hosts: `127.0.0.1` and `localhost` are
/// different sites, so the iframe below is a real cross-site frame and the
/// engine gives it its own renderer.
struct Fixture {
    a: String,
    b: String,
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Self {
        let socket_a = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        let socket_b = TcpListener::bind("localhost:0").expect("bind localhost");
        socket_a.set_nonblocking(true).expect("nonblocking a");
        socket_b.set_nonblocking(true).expect("nonblocking b");
        let port_a = socket_a.local_addr().expect("addr a").port();
        let port_b = socket_b.local_addr().expect("addr b").port();
        let a = format!("http://127.0.0.1:{port_a}");
        let b = format!("http://localhost:{port_b}");
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for (listener, other) in [(socket_a, b.clone()), (socket_b, a.clone())] {
            let flag = stop.clone();
            handles.push(std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            // One thread per connection: the browser opens
                            // speculative sockets that send nothing.
                            let other = other.clone();
                            std::thread::spawn(move || {
                                let _ = serve(stream, &other);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            }));
        }
        Fixture {
            a,
            b,
            stop,
            handles,
        }
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

fn serve(mut stream: TcpStream, other_origin: &str) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let body = match path {
        "/pointer.html" => POINTER.to_string(),
        "/oopif-child.html" => CHILD.to_string(),
        _ => {
            let _ = other_origin;
            let head = "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 14\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes())?;
            stream.write_all(b"404 not found\n")?;
            return Ok(());
        }
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

fn evaluate(s: &mut Session, expression: &str, frame: Option<&str>) -> Value {
    let mut args = json!({ "expression": expression });
    if let Some(f) = frame {
        args["frame"] = json!(f);
    }
    let resp = s.call("browser_evaluate", args);
    assert!(!is_error(&resp), "evaluate failed: {}", text_of(&resp));
    structured(&resp)["value"].clone()
}

/// The wheel scrolls the document through the engine's own input pipeline, and
/// the reply's read-back matches the page's own `scrollY`.
#[test]
fn wheel_scrolls_the_document_and_reports_it() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/pointer.html", fx.a)}));

    let down = s.call("browser_wheel", json!({"deltaY": 400}));
    assert!(!is_error(&down), "{}", text_of(&down));
    let reported = structured(&down)["scroll"]["y"].as_f64().unwrap_or(-1.0);
    let live = evaluate(&mut s, "window.scrollY", None).as_f64().unwrap_or(-1.0);
    assert!(
        live >= 300.0,
        "a 400 px wheel must scroll the page (scrollY={live})"
    );
    assert_eq!(reported, live, "the reply must report the measured offset");
    assert_eq!(structured(&down)["moved"], json!(true));

    // A wheel over a page that cannot scroll reports no movement instead of a
    // false success.
    s.call("browser_navigate", json!({"url": format!("{}/oopif-child.html", fx.a)}));
    let still = s.call("browser_wheel", json!({"deltaY": 400}));
    assert!(!is_error(&still), "{}", text_of(&still));
    assert_eq!(structured(&still)["moved"], json!(false));
}

/// The drag is a trusted pointer sequence (the page's own listener records
/// `isTrusted: true`), and it reorders the DOM by real hit-testing, not by a
/// synthetic DragEvent or a `.value`/array write.
#[test]
fn drag_reorders_with_trusted_pointer_events() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/pointer.html", fx.a)}));

    let before = evaluate(
        &mut s,
        "[...document.querySelectorAll('#list > li')].map(l => l.id).join(',')",
        None,
    );
    assert_eq!(before, json!("item1,item2,item3"));

    let drag = s.call("browser_drag", json!({"from": "#item1", "to": "#item3", "steps": 8}));
    assert!(!is_error(&drag), "{}", text_of(&drag));
    let structured_drag = structured(&drag);
    assert!(
        structured_drag["events"].as_u64().unwrap_or(0) >= 3,
        "a drag must dispatch press, moves and release: {structured_drag}"
    );

    let after = evaluate(
        &mut s,
        "[...document.querySelectorAll('#list > li')].map(l => l.id).join(',')",
        None,
    );
    assert_ne!(after, before, "the drag must reorder the list");
    assert_eq!(after, json!("item2,item3,item1"));

    let ledger = evaluate(&mut s, "window.__ev", None);
    assert_eq!(ledger["trustedDown"], json!(1), "the press must be trusted: {ledger}");
    assert_eq!(ledger["trustedUp"], json!(1), "the release must be trusted: {ledger}");
    assert_eq!(ledger["up"], json!(1));
    assert!(
        ledger["move"].as_u64().unwrap_or(0) >= 2,
        "the path needs move events: {ledger}"
    );
    assert_eq!(
        evaluate(&mut s, "document.getElementById('drag-out').textContent", None),
        json!("Item 2,Item 3,Item 1"),
        "the page's own drop handler must have run"
    );
}
