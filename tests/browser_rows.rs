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
const FRAMES: &str = r#"<!doctype html><html><head><title>Frames</title></head>
<body><iframe id="f" src="/frame" style="width:300px;height:200px;border:0"></iframe></body></html>"#;
const FRAME: &str = r#"<!doctype html><html><head><title>Frame</title></head>
<body><div id="in-frame" style="width:120px;height:40px">Inside</div></body></html>"#;
const DRAG: &str = r#"<!doctype html><html><head><title>Drag</title></head><body>
<div id="src" draggable="true" style="width:120px;height:40px;background:#ccc">drag me</div>
<div id="dst" style="width:180px;height:90px;margin-top:20px;background:#eee">drop here</div>
<script>
window.dragLog = [];
document.getElementById('src').addEventListener('dragstart', function (e) {
  window.dragLog.push({type:'dragstart', trusted:e.isTrusted});
  e.dataTransfer.setData('text/plain', 'from-page');
});
var dst = document.getElementById('dst');
dst.addEventListener('dragover', function (e) { e.preventDefault(); });
dst.addEventListener('drop', function (e) {
  e.preventDefault();
  window.dragLog.push({type:'drop', trusted:e.isTrusted,
    text:e.dataTransfer.getData('text/plain'),
    extra:e.dataTransfer.getData('application/x-hand')});
});
</script></body></html>"#;
const FORM: &str = r#"<!doctype html><html><head><title>Form</title></head><body>
<form id="f">
<input id="name" name="name">
<select id="color" name="color"><option value="red">Red</option><option value="blue">Blue</option></select>
<input id="ok" type="checkbox">
<button id="go" type="button" onclick="window.submitted = (window.submitted || 0) + 1">Go</button>
</form></body></html>"#;
const DL_PAGE: &str = r#"<!doctype html><html><head><title>Download Page</title></head>
<body><a id="dl" href="/payload.txt" download="hand-task.txt">download fixture</a></body></html>"#;
/// Exact bytes the loopback fixture delivers, and their SHA-256.
const DOWNLOAD_BYTES: &[u8] = b"hand row B5 download payload\n";
const DOWNLOAD_SHA256: &str = "20e1664389c59576710eca014871d3f63479d4afe7087fdd182a736e5515800d";

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
    if path == "/payload.txt" {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                    Content-Disposition: attachment; filename=\"hand-task.txt\"\r\n\
                    Cache-Control: no-store\r\nConnection: close\r\n\r\n";
        stream.write_all(head.as_bytes())?;
        stream.write_all(DOWNLOAD_BYTES)?;
        stream.flush()?;
        return Ok(());
    }
    let (status, body): (&str, String) = match path.as_str() {
        "/dl" => ("200 OK", DL_PAGE.to_string()),
        "/" => ("200 OK", NAV.to_string()),
        "/next" => ("200 OK", NEXT.to_string()),
        "/frames" => ("200 OK", FRAMES.to_string()),
        "/frame" => ("200 OK", FRAME.to_string()),
        "/drag" => ("200 OK", DRAG.to_string()),
        "/form" => ("200 OK", FORM.to_string()),
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

fn evaluate(s: &mut Session, expr: &str) -> Value {
    let r = s.call("browser_evaluate", json!({"expression": expr}));
    assert!(!is_error(&r), "evaluate failed: {}", text_of(&r));
    structured(&r)["value"].clone()
}

fn child_frame(s: &mut Session) -> String {
    let targets = s.call("browser_targets", json!({"action": "list"}));
    assert!(!is_error(&targets), "{}", text_of(&targets));
    structured(&targets)["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|f| f.get("url").and_then(|v| v.as_str()).unwrap_or("").ends_with("/frame"))
        .and_then(|f| f.get("frameId").and_then(|v| v.as_str()))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no child frame in fixture: {}", text_of(&targets)))
}

#[test]
fn b11_browser_form_runs_ordered_steps_with_partial_failure() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let names = s.list_tools();
    assert!(names.iter().any(|t| t["name"] == "browser_form"),
            "catalog must publish browser_form");

    let r = s.call("browser_navigate", json!({"url": format!("{}/form", fx.base)}));
    assert!(!is_error(&r), "{}", text_of(&r));

    // A missing field fails its step but the batch continues and submits.
    let r = s.call("browser_form", json!({
        "fields": [
            {"field": "#name", "action": "type", "value": "Ada"},
            {"field": "#missing", "action": "type", "value": "x"},
            {"field": "#color", "action": "select", "value": "blue"},
            {"field": "#ok", "action": "check"}
        ],
        "submit": "#go"
    }));
    assert!(!is_error(&r), "form batch failed: {}", text_of(&r));
    let sc = structured(&r);
    let steps = sc["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "one step per field: {sc:?}");
    assert_eq!(steps[0]["field"].as_str().unwrap_or(""), "#name");
    assert_eq!(steps[0]["status"].as_str().unwrap_or(""), "ok");
    assert_eq!(steps[1]["status"].as_str().unwrap_or(""), "error", "partial failure: {sc:?}");
    assert!(steps[1]["error"].as_str().unwrap_or("").len() > 0, "error detail: {sc:?}");
    assert_eq!(steps[2]["status"].as_str().unwrap_or(""), "ok");
    assert_eq!(steps[3]["status"].as_str().unwrap_or(""), "ok");
    assert_eq!(sc["submitted"].as_bool(), Some(true), "submitted even after a step error");
    assert_eq!(sc["verdict"].as_str().unwrap_or(""), "fail");

    let state = evaluate(&mut s, "JSON.stringify({n:document.getElementById('name').value, c:document.getElementById('color').value, k:document.getElementById('ok').checked, sub:window.submitted})");
    let state: Value = serde_json::from_str(state.as_str().expect("json string")).expect("parse state");
    assert_eq!(state["n"].as_str().unwrap_or(""), "Ada");
    assert_eq!(state["c"].as_str().unwrap_or(""), "blue");
    assert_eq!(state["k"].as_bool(), Some(true));
    assert_eq!(state["sub"].as_i64(), Some(1), "one submit per batch: {state:?}");

    // An all-good batch yields a pass verdict.
    let r = s.call("browser_form", json!({
        "fields": [{"field": "#name", "action": "type", "value": "Grace"}],
        "submit": "#go"
    }));
    assert!(!is_error(&r), "{}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["verdict"].as_str().unwrap_or(""), "pass");
    assert_eq!(sc["submitted"].as_bool(), Some(true));
}

#[test]
fn b10_native_datatransfer_drag_is_trusted() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let r = s.call("browser_navigate", json!({"url": format!("{}/drag", fx.base)}));
    assert!(!is_error(&r), "{}", text_of(&r));

    let r = s.call("browser_drag", json!({
        "from": "#src", "to": "#dst", "steps": 8,
        "dataTransfer": {"text/plain": "payload-42", "application/x-hand": "row-7"}
    }));
    assert!(!is_error(&r), "drag failed: {}", text_of(&r));
    let sc = structured(&r);
    assert!(sc["dragEvents"].as_i64().unwrap_or(0) >= 3, "drag events: {sc:?}");

    let log = evaluate(&mut s, "window.dragLog");
    let arr = log.as_array().expect("dragLog array");
    let start = arr.iter().find(|e| e["type"] == "dragstart").expect("dragstart");
    assert_eq!(start["trusted"].as_bool(), Some(true), "dragstart trusted: {start:?}");
    let drop = arr.iter().find(|e| e["type"] == "drop").expect("drop");
    assert_eq!(drop["trusted"].as_bool(), Some(true), "drop trusted: {drop:?}");
    assert_eq!(drop["text"].as_str().unwrap_or(""), "payload-42", "drop data: {drop:?}");
    assert_eq!(drop["extra"].as_str().unwrap_or(""), "row-7", "second mime item: {drop:?}");
}

#[test]
fn b9_frame_scoped_layout_assert_layout_visual() {
    let _g = serial();
    let fx = Fixture::start();
    let visual_dir = std::env::temp_dir().join(format!("hand-browser-rows-visual-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&visual_dir);
    let mut s = Session::spawn_with_env(&[("GLADIUS_VISUAL_DIR", visual_dir.to_string_lossy().as_ref())]);
    let r = s.call("browser_navigate", json!({"url": format!("{}/frames", fx.base)}));
    assert!(!is_error(&r), "{}", text_of(&r));
    let fid = child_frame(&mut s);

    // The main frame cannot see an element inside the iframe.
    let main = s.call("browser_layout", json!({"selector": "#in-frame"}));
    assert!(!is_error(&main), "{}", text_of(&main));
    assert_eq!(structured(&main)["count"].as_i64(), Some(0));

    // Frame-scoped layout finds it, with a real measured rect.
    let scoped = s.call("browser_layout", json!({"selector": "#in-frame", "frame": fid}));
    assert!(!is_error(&scoped), "frame layout failed: {}", text_of(&scoped));
    let sc = structured(&scoped);
    assert_eq!(sc["count"].as_i64(), Some(1), "frame layout: {sc:?}");
    let el = &sc["elements"][0];
    assert!(el["rect"]["w"].as_f64().unwrap_or(0.0) > 0.0, "frame rect: {el:?}");
    assert_eq!(el["computed"]["display"].as_str().unwrap_or(""), "block");

    // assert_layout honours the same frame contract.
    let a = s.call("browser_assert_layout", json!({
        "selector": "#in-frame", "frame": fid,
        "checks": {"exists": true, "visible": true,
                   "box": {"w": 120.0, "h": 40.0, "tolerance": 1.0}}
    }));
    assert!(!is_error(&a), "frame assert_layout failed: {}", text_of(&a));
    let asc = structured(&a);
    assert_eq!(asc["pass"].as_bool(), Some(true), "checks: {:?}", asc["checks"]);

    // visual accepts the same frame id and scopes its capture to it.
    let v = s.call("browser_visual", json!({
        "action": "save", "name": "frame-baseline", "frame": fid
    }));
    assert!(!is_error(&v), "frame visual failed: {}", text_of(&v));
    let vsc = structured(&v);
    assert_eq!(vsc["frame"].as_str(), Some(fid.as_str()));
    let clip = &vsc["clip"];
    assert!((clip["w"].as_f64().unwrap_or(0.0) - 300.0).abs() <= 1.0,
            "capture must be clipped to the 300px iframe: {clip:?}");
    drop(s);
    let _ = std::fs::remove_dir_all(&visual_dir);
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

fn download_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("hand-b5-{tag}-{}", std::process::id()))
}

#[test]
fn b5_download_writes_file_with_sha256_mime_and_cleans_up() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let names = s.list_tools();
    assert!(names.iter().any(|t| t["name"] == "browser_download"),
            "catalog must publish browser_download");

    let dir = download_dir("ok");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create caller dir");

    let page = s.call("browser_navigate", json!({"url": format!("{}/dl", fx.base)}));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_download", json!({
        "dir": dir.to_string_lossy(), "accept": ["text/plain"],
        "timeoutMs": 15000, "maxBytes": 1024, "filename": "hand-task.txt"
    }));
    assert!(!is_error(&r), "download failed: {}", text_of(&r));
    let sc = structured(&r).clone();
    assert_eq!(sc["bytes"].as_u64(), Some(DOWNLOAD_BYTES.len() as u64), "delivered bytes: {sc:?}");
    assert_eq!(sc["sha256"].as_str().unwrap_or(""), DOWNLOAD_SHA256, "real SHA-256: {sc:?}");
    assert_eq!(sc["mimeType"].as_str().unwrap_or(""), "text/plain", "mime: {sc:?}");
    assert_eq!(sc["filename"].as_str().unwrap_or(""), "hand-task.txt", "filename: {sc:?}");
    assert!(sc["headers"].is_object(), "headers: {sc:?}");
    assert!(sc["durationMs"].as_u64().unwrap_or(u64::MAX) < 15000, "duration: {sc:?}");
    let path = std::path::PathBuf::from(sc["path"].as_str().expect("path"));
    assert!(path.is_absolute(), "path must be absolute: {path:?}");
    assert_eq!(std::fs::read(&path).expect("delivered file").as_slice(), DOWNLOAD_BYTES);

    // Over-maxBytes is a cancel: it fails typed and leaves no partial file.
    let cancel_dir = download_dir("cancel");
    let _ = std::fs::remove_dir_all(&cancel_dir);
    std::fs::create_dir_all(&cancel_dir).expect("create cancel dir");
    let r2 = s.call("browser_download", json!({
        "dir": cancel_dir.to_string_lossy(), "maxBytes": 0, "timeoutMs": 15000
    }));
    assert!(is_error(&r2), "over-maxBytes must fail typed: {}", text_of(&r2));
    let leftovers: Vec<_> = std::fs::read_dir(&cancel_dir).expect("read cancel dir")
        .filter_map(Result::ok).collect();
    assert!(leftovers.is_empty(), "no partial file may remain: {:?}",
        leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&cancel_dir);
}
