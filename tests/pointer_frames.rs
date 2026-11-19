//! The interaction slice: pointer completeness (right/middle/double click,
//! wheel, drag), element/clip screenshot with format passthrough, and
//! out-of-process frame scoping for snapshot and click.
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
const PARENT: &str = include_str!("../fixtures/interaction/oopif-parent.html");
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
        "/oopif-parent.html" => PARENT.replace("__CROSS_ORIGIN__", other_origin),
        "/oopif-child.html" => CHILD.to_string(),
        _ => {
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

fn oopif_frame(s: &mut Session) -> String {
    let targets = s.call("browser_targets", json!({"action": "list"}));
    assert!(!is_error(&targets), "{}", text_of(&targets));
    structured(&targets)["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|f| f.get("oopif").and_then(|v| v.as_bool()) == Some(true))
        .and_then(|f| f.get("frameId").and_then(|v| v.as_str()))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no OOPIF frame in the fixture page: {}", text_of(&targets)))
}

/// Right click reaches the page as a trusted contextmenu, and the menu the page
/// opens is what `snapshot` then reports as `menu`/`menuitem` nodes.
#[test]
fn right_click_opens_the_pages_menu_and_snapshot_reports_it() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/pointer.html", fx.a)}));

    let hidden = evaluate(&mut s, "document.getElementById('menu').hidden", None);
    assert_eq!(hidden, json!(true), "the menu starts hidden");

    let clicked = s.call("browser_click", json!({"element": "#rc", "button": "right"}));
    assert!(!is_error(&clicked), "{}", text_of(&clicked));

    assert_eq!(
        evaluate(&mut s, "document.getElementById('menu').hidden", None),
        json!(false),
        "a right click must raise the page's contextmenu handler"
    );
    let snap = s.call("browser_snapshot", json!({}));
    let snap_text = text_of(&snap);
    assert!(
        snap_text.contains("role=\"menu\"") || snap_text.contains("- menu "),
        "snapshot must report the opened menu node: {snap_text}"
    );
    assert!(
        snap_text.contains("menuitem"),
        "snapshot must report the menu items: {snap_text}"
    );
}

/// A left click with clickCount 2 is a real double click: the engine's own
/// `dblclick` fires, which no single click can produce.
#[test]
fn click_count_two_raises_dblclick() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/pointer.html", fx.a)}));

    let single = s.call("browser_click", json!({"element": "#dbl"}));
    assert!(!is_error(&single), "{}", text_of(&single));
    assert_eq!(
        evaluate(&mut s, "document.getElementById('dbl-out').textContent", None),
        json!("dbl: 0"),
        "one click must not be a double click"
    );

    let double = s.call("browser_click", json!({"element": "#dbl", "clickCount": 2}));
    assert!(!is_error(&double), "{}", text_of(&double));
    assert_eq!(
        evaluate(&mut s, "document.getElementById('dbl-out').textContent", None),
        json!("dbl: 1"),
        "clickCount 2 must raise exactly one dblclick"
    );
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

/// An element capture is the element's own box (its IHDR width equals the box
/// `browser_layout` reports), two deterministic captures are byte-identical,
/// and a ref that was never minted is a typed refusal.
#[test]
fn element_screenshot_is_the_box_and_is_deterministic() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/pointer.html", fx.a)}));

    let layout = s.call("browser_layout", json!({"selector": "#shot"}));
    assert!(!is_error(&layout), "{}", text_of(&layout));
    let box_w = structured(&layout)["elements"][0]["documentBox"]["w"]
        .as_f64()
        .expect("layout box width");

    let first = s.call("browser_screenshot", json!({"element": "#shot", "deterministic": true}));
    assert!(!is_error(&first), "{}", text_of(&first));
    let one = structured(&first).clone();
    let second = s.call("browser_screenshot", json!({"element": "#shot", "deterministic": true}));
    let two = structured(&second).clone();

    assert_eq!(one["element"], json!("#shot"));
    let width = one["size"]["width"].as_f64().expect("IHDR width");
    assert!(
        (width - box_w).abs() <= 1.0,
        "PNG IHDR width {width} must equal the layout box width {box_w}"
    );
    assert_eq!(one["checksum"], two["checksum"], "deterministic bytes must match");
    assert_eq!(one["bytes"], two["bytes"]);

    let jpeg = s.call("browser_screenshot", json!({"format": "jpeg", "quality": 70}));
    assert!(!is_error(&jpeg), "{}", text_of(&jpeg));
    assert_eq!(structured(&jpeg)["mimeType"], json!("image/jpeg"));

    let png_quality = s.call("browser_screenshot", json!({"format": "png", "quality": 70}));
    assert!(is_error(&png_quality), "png + quality must be refused");

    let wrong = s.call("browser_screenshot", json!({"element": "@e9999", "deterministic": true}));
    assert!(is_error(&wrong), "an unknown ref must fail");
    assert_eq!(structured(&wrong)["code"], json!("stale_ref"));
}

/// A cross-origin frame with its own renderer is addressable: `snapshot` with
/// its frame id returns ITS nodes, a click lands inside it, and a frame id no
/// session owns stays a typed error instead of a main-frame action.
#[test]
fn oopif_frame_snapshot_and_click_with_typed_unmapped_frame() {
    let fx = Fixture::start();
    let mut s = Session::spawn();
    s.call("browser_navigate", json!({"url": format!("{}/oopif-parent.html", fx.a)}));
    let fid = oopif_frame(&mut s);

    let main = text_of(&s.call("browser_snapshot", json!({})));
    assert!(
        !main.contains("Child action"),
        "iframe content is not part of the page's own AX tree: {main}"
    );

    let inside = s.call("browser_snapshot", json!({"frame": fid}));
    assert!(!is_error(&inside), "{}", text_of(&inside));
    let node = structured(&inside)["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .find(|n| n["role"] == json!("button"))
        .cloned()
        .expect("the child frame's button");
    assert_eq!(node["name"], json!("Child action"));
    assert_eq!(structured(&inside)["url"], json!(format!("{}/oopif-child.html", fx.b)));

    let clicked = s.call(
        "browser_click",
        json!({"element": "#child-btn", "frame": fid}),
    );
    assert!(!is_error(&clicked), "{}", text_of(&clicked));
    assert_eq!(
        evaluate(&mut s, "document.getElementById('child-out').textContent", Some(&fid)),
        json!("clicked"),
        "the click must land inside the out-of-process frame"
    );

    let child_ref = node["ref"].as_str().expect("ref").to_string();
    let by_ref = s.call("browser_click", json!({"element": format!("@{child_ref}"), "frame": fid}));
    assert!(!is_error(&by_ref), "{}", text_of(&by_ref));
    let cross = s.call("browser_click", json!({"element": "@e1", "frame": fid}));
    assert!(is_error(&cross), "a main-frame ref must not be clicked in a frame");

    for (tool, args) in [
        ("browser_snapshot", json!({"frame": "NOSUCHFRAMEID"})),
        ("browser_click", json!({"element": "#child-btn", "frame": "NOSUCHFRAMEID"})),
        ("browser_evaluate", json!({"expression": "1", "frame": "NOSUCHFRAMEID"})),
    ] {
        let resp = s.call(tool, args);
        assert!(is_error(&resp), "{tool} with an unknown frame must fail");
        let code = structured(&resp)["code"].as_str().unwrap_or("").to_string();
        assert!(
            code == "invalid_target",
            "{tool} must answer a typed invalid_target, got {code}: {}",
            text_of(&resp)
        );
    }
}
