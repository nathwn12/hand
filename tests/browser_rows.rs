//! The four browser P0 rows from the 0.2.0 addendum, one live browser at a
//! time. Every assertion reads the page's own observation (a real redirect,
//! a frame's own DOM, a trusted drag/drop handler, a real form submission),
//! never the tool's claim. Fixtures are inline; no files under fixtures/.

mod common;

use common::{is_error, navigate_with_retry, structured, text_of, Session};
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
/// Visible text arrives after 400 ms, so a bounded assertion must poll rather
/// than read once and give up.
const DELAYED: &str = r#"<!doctype html><html><head><title>Delayed</title></head>
<body><p id="out">waiting</p><script>
setTimeout(function () { document.getElementById('out').textContent = 'ready now'; }, 400);
</script></body></html>"#;
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
  window.dragLog.push({type:'dragstart', trusted:e.isTrusted,
    types:Array.prototype.slice.call(e.dataTransfer.types)});
});
var dst = document.getElementById('dst');
dst.addEventListener('dragover', function (e) { e.preventDefault(); });
dst.addEventListener('drop', function (e) {
  e.preventDefault();
  window.dragLog.push({type:'drop', trusted:e.isTrusted,
    types:Array.prototype.slice.call(e.dataTransfer.types),
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
/// Mirrors fixtures/tasks/viewport.html: deliberate overflow, low contrast,
/// a tiny target and a control whose focus rule suppresses the ring.
const AUDIT: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Hand Tasks Viewport</title><style>
body { font: 16px/1.4 system-ui; margin: 0; padding: 8px; background: #fff; color: #111; }
#wide { width: 1400px; height: 40px; background: #e8e8e8; }
#low-contrast { color: #b8b8b8; background: #fff; }
#tiny { width: 12px; height: 12px; padding: 0; border: 1px solid #999; }
#focusable:focus { outline: none; }
</style></head><body><h1>Hand Tasks Viewport</h1>
<div id="wide">deliberate horizontal overflow</div>
<p id="low-contrast">Low contrast text for the audit check.</p>
<button id="tiny" type="button" aria-label="Tiny target"></button>
<input id="focusable" type="text" aria-label="No focus ring">
</body></html>"#;
/// Exact bytes the loopback fixture delivers, and their SHA-256.
const TRACE_PAGE: &str = r#"<!doctype html><html><head><title>Trace Page</title></head>
<body><h1>trace</h1><p>row B6 fixture</p></body></html>"#;
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
        "/delayed" => ("200 OK", DELAYED.to_string()),
        "/frames" => ("200 OK", FRAMES.to_string()),
        "/frame" => ("200 OK", FRAME.to_string()),
        "/drag" => ("200 OK", DRAG.to_string()),
        "/form" => ("200 OK", FORM.to_string()),
        "/audit" => ("200 OK", AUDIT.to_string()),
        p if p.starts_with("/trace") => ("200 OK", TRACE_PAGE.to_string()),
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

    let r = navigate_with_retry(&mut s, &format!("{}/form", fx.base));
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
    let r = navigate_with_retry(&mut s, &format!("{}/drag", fx.base));
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
    let start_types = start["types"].as_array().expect("dragstart DataTransfer.types array");
    assert!(start_types.is_empty(), "CDP items must not be claimed on native dragstart: {start:?}");
    let drop = arr.iter().find(|e| e["type"] == "drop").expect("drop");
    assert_eq!(drop["trusted"].as_bool(), Some(true), "drop trusted: {drop:?}");
    let types = drop["types"].as_array().expect("drop DataTransfer.types array");
    assert!(types.iter().any(|t| t.as_str() == Some("text/plain")), "text/plain item missing from page DataTransfer: {drop:?}");
    assert!(types.iter().any(|t| t.as_str() == Some("application/x-hand")), "application/x-hand item missing from page DataTransfer: {drop:?}");
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
    let r = navigate_with_retry(&mut s, &format!("{}/frames", fx.base));
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

    let r = navigate_with_retry(&mut s, &format!("{}/redirect", fx.base));
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
    let r2 = navigate_with_retry(&mut s, &format!("{}/next#frag", fx.base));
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

    let page = navigate_with_retry(&mut s, &format!("{}/dl", fx.base));
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

#[test]
fn b2_assert_reports_expected_observed_verdict_for_dom_text_url() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let names = s.list_tools();
    assert!(names.iter().any(|t| t["name"] == "browser_assert"),
            "catalog must publish browser_assert");

    let page = navigate_with_retry(&mut s, &format!("{}/next", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let dom = s.call("browser_assert", json!({
        "kind": "dom", "target": "h1",
        "expected": {"exists": true, "text": "next"}
    }));
    assert!(!is_error(&dom), "dom assert failed: {}", text_of(&dom));
    let d = structured(&dom);
    assert_eq!(d["verdict"].as_bool(), Some(true), "dom verdict: {d:?}");
    assert_eq!(d["expected"], json!({"exists": true, "text": "next"}));
    assert_eq!(d["observed"]["exists"].as_bool(), Some(true));
    assert!(d["evidence"].is_object() && d["evidence"].get("polls").is_some());
    assert!(d["elapsedMs"].as_u64().unwrap_or(u64::MAX) < 5000);

    // An @eN ref from a real snapshot resolves as a dom target too.
    let snap = s.call("browser_snapshot", json!({}));
    assert!(!is_error(&snap), "{}", text_of(&snap));
    let nodes = structured(&snap)["nodes"].as_array().cloned().unwrap_or_default();
    let refs: Vec<String> = nodes.iter()
        .filter_map(|n| n["ref"].as_str().map(str::to_string)).collect();
    assert!(!refs.is_empty(), "snapshot must mint refs");
    let by_ref = s.call("browser_assert", json!({
        "kind": "dom", "target": refs[0], "expected": {"exists": true}
    }));
    assert!(!is_error(&by_ref), "ref assert failed: {}", text_of(&by_ref));
    assert_eq!(structured(&by_ref)["verdict"].as_bool(), Some(true));

    let text = s.call("browser_assert", json!({
        "kind": "text", "expected": {"contains": "next"}
    }));
    assert!(!is_error(&text), "text assert failed: {}", text_of(&text));
    assert_eq!(structured(&text)["verdict"].as_bool(), Some(true));

    let url = s.call("browser_assert", json!({
        "kind": "url", "expected": {"matches": "^http://.*/next$"}
    }));
    assert!(!is_error(&url), "url assert failed: {}", text_of(&url));
    assert_eq!(structured(&url)["verdict"].as_bool(), Some(true));

    // A still-false check is a normal verdict, not a timeout error: it carries
    // the LAST observed value and stops at the poll budget.
    let fail = s.call("browser_assert", json!({
        "kind": "text", "expected": {"contains": "NOT-ON-PAGE"},
        "timeoutMs": 300, "pollMs": 50
    }));
    assert!(!is_error(&fail), "a false verdict must not be an error: {}", text_of(&fail));
    let f = structured(&fail);
    assert_eq!(f["verdict"].as_bool(), Some(false), "false verdict: {f:?}");
    assert!(f["observed"]["text"].as_str().unwrap_or("").contains("next"),
            "last observed text: {f:?}");

    // ax reads the same Accessibility tree the snapshot renders: the h1 on
    // this page is a heading named "next" (the page title is "Next Page").
    // The verdict is the string the frozen ax fixture expects, and every
    // check carries its measurement.
    let ax = s.call("browser_assert", json!({
        "kind": "ax", "target": "role=heading[name=\"next\"]",
        "expected": {"role": "heading", "name": "next"}
    }));
    assert!(!is_error(&ax), "ax assert failed: {}", text_of(&ax));
    let a = structured(&ax);
    assert_eq!(a["verdict"].as_str().unwrap_or(""), "pass", "ax verdict: {a:?}");
    assert!(a["observed"]["source"].as_str().unwrap_or("").contains("Accessibility.getFullAXTree"),
            "ax must read the snapshot's AX tree: {a:?}");
    let ax_checks = a["evidence"]["checks"].as_array().expect("checks");
    assert_eq!(ax_checks.len(), 3, "exists + role + name: {a:?}");
    assert!(ax_checks.iter().all(|c| c["pass"] == true), "checks: {ax_checks:?}");

    // A false ax check is a normal verdict, not a typed error.
    let ax_miss = s.call("browser_assert", json!({
        "kind": "ax", "target": "role=heading[name=\"Not On This Page\"]",
        "expected": {"role": "heading", "name": "Not On This Page"}, "timeoutMs": 200
    }));
    assert!(!is_error(&ax_miss), "a false ax verdict must not be an error: {}", text_of(&ax_miss));
    assert_eq!(structured(&ax_miss)["verdict"].as_str().unwrap_or(""), "fail");

    // network stays honestly unimplemented rather than faked.
    let network = s.call("browser_assert", json!({"kind": "network", "expected": {}}));
    assert!(is_error(&network), "network must be a typed not_implemented");
    assert_eq!(structured(&network)["code"].as_str().unwrap_or(""), "not_implemented");
}

#[test]
fn b7_audit_measures_six_viewports_with_evidence_per_check() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let names = s.list_tools();
    assert!(names.iter().any(|t| t["name"] == "browser_audit"),
            "catalog must publish browser_audit");

    let page = navigate_with_retry(&mut s, &format!("{}/audit", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));
    let default_width = evaluate(&mut s, "window.innerWidth").as_i64().unwrap_or(0);

    let r = s.call("browser_audit", json!({
        "viewports": [360, 390, 768, 1024, 1440, 2560],
        "checks": ["overflow", "contrast", "focus", "targetSize"]
    }));
    assert!(!is_error(&r), "audit failed: {}", text_of(&r));
    let sc = structured(&r).clone();
    assert_eq!(sc["verdict"].as_str().unwrap_or(""), "pass", "audit must complete: {sc:?}");
    let records = sc["viewports"].as_array().expect("viewports");
    assert_eq!(records.len(), 6, "one record per viewport: {sc:?}");

    for (i, rec) in records.iter().enumerate() {
        let requested = [360, 390, 768, 1024, 1440, 2560][i];
        assert_eq!(rec["width"].as_i64(), Some(requested), "live read-back: {rec:?}");
        for key in ["scrollWidth", "overflow", "contrast", "focus", "targetSize", "verdict"] {
            assert!(rec.get(key).is_some(), "missing {key}: {rec:?}");
        }
        let sw = rec["scrollWidth"].as_i64().unwrap_or(0);
        let cw = rec["overflow"]["observed"]["clientWidth"].as_i64().unwrap_or(0);
        assert!(sw > 0 && cw > 0, "measured widths: {rec:?}");
        assert_eq!(rec["overflow"]["observed"]["excess"].as_i64(), Some(sw - cw));
        if requested == 360 {
            assert!(sw > cw, "360px must show the deliberate overflow: {rec:?}");
        }
        for key in ["overflow", "contrast", "focus", "targetSize"] {
            assert!(rec[key].get("observed").is_some(), "{key} needs a measurement: {rec:?}");
            assert!(rec[key].get("expected").is_some(), "{key} needs an expectation: {rec:?}");
            assert!(rec[key].get("pass").is_some(), "{key} needs a pass flag: {rec:?}");
        }
        assert!(rec["contrast"]["formula"].as_str().unwrap_or("").contains("0.2126"),
                "contrast must state its formula: {rec:?}");
        let ratio = rec["contrast"]["observed"]["ratio"].as_f64().unwrap_or(99.0);
        assert!(ratio > 0.0 && ratio < 4.5, "low-contrast block must be named: {rec:?}");
        assert_eq!(rec["contrast"]["observed"]["selector"].as_str().unwrap_or(""), "#low-contrast");
        assert_eq!(rec["focus"]["observed"]["ringVisible"].as_bool(), Some(false),
                   "one of the focused controls has no ring: {rec:?}");
        let no_ring = rec["focus"]["observed"]["noRing"].as_array().expect("noRing");
        assert!(no_ring.iter().any(|n| n.as_str() == Some("#focusable")),
                "the control whose focus rule suppresses the ring must be named: {rec:?}");
        let small = rec["targetSize"]["observed"]["small"].as_array().expect("small targets");
        assert!(small.iter().any(|t| t["selector"] == "#tiny"), "tiny target must be named: {rec:?}");
        assert_eq!(rec["verdict"].as_bool(), Some(false), "the page fails these checks: {rec:?}");
    }

    // The matrix restores the connection-wide viewport override.
    let after = evaluate(&mut s, "window.innerWidth").as_i64().unwrap_or(0);
    assert_eq!(after, default_width, "viewport must be restored after the audit");
}

#[test]
fn b6_trace_writes_har_with_counted_redaction_and_cancel_cleans_up() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let tools = s.list_tools();
    assert!(tools.iter().any(|t| t["name"] == "browser_trace"),
            "catalog must publish browser_trace");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hand-trace-b6-{}-{nanos}", std::process::id()));
    let dir_str = dir.to_string_lossy().to_string();

    // Both refusals are typed and land before any file is created.
    let relative = s.call("browser_trace", json!({"dir": "relative/trace", "har": true}));
    assert!(is_error(&relative), "a relative dir must be refused: {}", text_of(&relative));
    let sc = structured(&relative);
    assert_eq!(sc["code"].as_str().unwrap_or(""), "invalid_args");
    assert_eq!(sc["reason"].as_str().unwrap_or(""), "bad_value");
    assert_eq!(sc["arg"].as_str().unwrap_or(""), "dir");

    let video = s.call("browser_trace", json!({"dir": dir_str, "video": true}));
    assert!(is_error(&video), "video must stay typed not_implemented: {}", text_of(&video));
    let sc = structured(&video);
    assert_eq!(sc["code"].as_str().unwrap_or(""), "not_implemented");
    assert_eq!(sc["field"].as_str().unwrap_or(""), "video");
    assert!(!dir.exists(), "a refusal must not create the caller's directory");

    // Arm the existing CDP Network ring, then navigate to a URL that carries
    // the literal we will redact. The HAR must wrap that real observation.
    let armed = s.call("browser_network", json!({"action": "get"}));
    assert!(!is_error(&armed), "arming network capture failed: {}", text_of(&armed));
    let page = navigate_with_retry(&mut s, &format!("{}/trace?Authorization=secret", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_trace", json!({
        "dir": dir_str,
        "screenshots": true,
        "har": true,
        "redact": ["Authorization", "not-on-this-page"]
    }));
    assert!(!is_error(&r), "trace failed: {}", text_of(&r));
    let sc = structured(&r).clone();
    assert_eq!(sc["ok"].as_bool(), Some(true), "trace payload: {sc:?}");
    assert_eq!(sc["dir"].as_str(), Some(dir_str.as_str()));
    let files = sc["files"].as_array().expect("files");
    assert_eq!(files.len(), 2, "one HAR and one deterministic step: {sc:?}");
    assert!(sc["bytes"].as_u64().unwrap_or(0) > 0, "bytes must be counted: {sc:?}");

    let har_path = dir.join("trace.har");
    let shot_path = dir.join("step-0001.png");
    let har_text = std::fs::read_to_string(&har_path).expect("HAR on disk");
    let har: Value = serde_json::from_str(&har_text).expect("HAR is valid JSON");
    assert_eq!(har["log"]["version"].as_str(), Some("1.2"), "HAR 1.2: {har:?}");
    assert!(har["log"]["comment"].as_str().unwrap_or("").contains("browser.network_get"),
            "HAR must name the capture it wraps");
    let entries = har["log"]["entries"].as_array().expect("entries");
    assert!(!entries.is_empty(), "the armed ring must have real network entries");
    assert!(entries.iter().all(|e| e["_capture"]["source"] == "browser.network_get"),
            "every entry must name its source: {entries:?}");
    assert!(entries.iter().any(|e| e["request"]["url"].as_str().unwrap_or("").contains("/trace")),
            "the traced navigation must appear: {entries:?}");
    assert!(!har_text.contains("Authorization"), "literal must be gone from the HAR text");
    assert!(har_text.contains("[REDACTED]"), "replacement marker must be visible");
    let redactions = sc["redactions"].as_array().expect("redactions");
    let found = |literal: &str| redactions.iter().find(|r| r["literal"] == literal).cloned();
    let hit = found("Authorization").expect("named literal");
    assert_eq!(hit["marker"].as_str(), Some("[REDACTED]"));
    assert!(hit["count"].as_u64().unwrap_or(0) >= 1, "counted occurrence: {hit:?}");
    let miss = found("not-on-this-page").expect("a literal with zero hits is still named");
    assert_eq!(miss["count"].as_u64(), Some(0), "no false detection: {miss:?}");

    let shot = std::fs::read(&shot_path).expect("deterministic step on disk");
    assert!(shot.len() > 1000 && shot.starts_with(b"\x89PNG"), "real PNG: {} bytes", shot.len());

    // Cancel removes only what this tool wrote; a caller's file survives.
    let bystander = dir.join("caller-note.txt");
    std::fs::write(&bystander, b"keep me").expect("bystander");
    let cancel = s.call("browser_trace", json!({"action": "cancel"}));
    assert!(!is_error(&cancel), "cancel failed: {}", text_of(&cancel));
    assert_eq!(structured(&cancel)["ok"].as_bool(), Some(true));
    assert!(!har_path.exists() && !shot_path.exists(), "owned artifacts removed on cancel");
    assert_eq!(std::fs::read_to_string(&bystander).expect("bystander survived"), "keep me");

    let cancel_again = s.call("browser_trace", json!({"action": "cancel"}));
    assert!(!is_error(&cancel_again), "cancel is idempotent: {}", text_of(&cancel_again));
    assert_eq!(structured(&cancel_again)["ok"].as_bool(), Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A per-test absolute trace directory, unique so two tests never share the
/// artifacts they are about to assert on.
fn trace_dir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("hand-trace-{tag}-{}-{nanos}", std::process::id()))
}

#[test]
fn navigate_404_reports_the_real_status() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();

    let r = navigate_with_retry(&mut s, &format!("{}/missing", fx.base));
    assert!(!is_error(&r), "an HTTP 404 is still a navigation: {}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["observed"]["status"].as_i64(), Some(404), "observed status: {sc}");
    assert_eq!(evaluate(&mut s, "document.querySelector('h1').textContent"), json!("missing"));
}

#[test]
fn assert_dom_false_keeps_the_last_observed_value() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let page = navigate_with_retry(&mut s, &format!("{}/next", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_assert", json!({
        "kind": "dom", "target": "h1", "expected": {"exists": false}, "timeoutMs": 300
    }));
    assert!(!is_error(&r), "a false verdict is not an error: {}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["verdict"], false, "the h1 exists: {sc}");
    assert_eq!(sc["expected"], json!({"exists": false}));
    assert_eq!(sc["observed"]["exists"], true, "last observation: {sc}");
}

#[test]
fn assert_text_poll_waits_for_delayed_content() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let page = navigate_with_retry(&mut s, &format!("{}/delayed", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_assert", json!({
        "kind": "text", "expected": {"contains": "ready now"},
        "timeoutMs": 3000, "pollMs": 50
    }));
    assert!(!is_error(&r), "poll must find the late text: {}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["verdict"], true, "verdict: {sc}");
    assert!(
        sc["observed"]["text"].as_str().unwrap_or("").contains("ready now"),
        "observed text: {sc}"
    );
    assert!(
        sc["elapsedMs"].as_u64().unwrap_or(0) >= 100,
        "the assertion must have waited, not read once: {sc}"
    );
}

#[test]
fn assert_unknown_kind_is_a_typed_refusal() {
    let _g = serial();
    let mut s = Session::spawn();
    let r = s.call("browser_assert", json!({"kind": "bogus", "expected": {}}));
    assert!(is_error(&r), "an unknown kind must refuse typed");
    let sc = structured(&r);
    assert_eq!(sc["code"], "invalid_args", "typed code: {sc}");
    assert_eq!(sc["arg"], "kind", "bad argument named: {sc}");
    assert!(text_of(&r).to_lowercase().contains("unknown kind"), "reason: {}", text_of(&r));
}

#[test]
fn download_relative_dir_is_a_typed_refusal() {
    let _g = serial();
    let mut s = Session::spawn();
    let r = s.call("browser_download", json!({"dir": "relative/downloads"}));
    assert!(is_error(&r), "a relative dir must refuse typed");
    let sc = structured(&r);
    assert_eq!(sc["code"], "invalid_args", "typed code: {sc}");
    assert_eq!(sc["reason"], "bad_value", "typed reason: {sc}");
    assert_eq!(sc["arg"], "dir", "bad argument named: {sc}");
}

#[test]
fn download_accept_list_refuses_a_wrong_mime() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let page = navigate_with_retry(&mut s, &format!("{}/dl", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let dir = download_dir("mime");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    let r = s.call("browser_download", json!({
        "dir": dir.to_string_lossy(), "accept": ["image/png"], "timeoutMs": 15000
    }));
    assert!(is_error(&r), "a MIME outside accept must refuse typed");
    let sc = structured(&r);
    assert_eq!(sc["code"], "invalid_args", "typed code: {sc}");
    assert_eq!(sc["arg"], "accept", "bad argument named: {sc}");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).expect("read dir").filter_map(Result::ok).collect();
    assert!(leftovers.is_empty(), "a refused download leaves no file: {:?}", leftovers.iter().map(|e| e.file_name()).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trace_har_only_writes_one_file() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let dir = trace_dir("har-only");
    let dir_str = dir.to_string_lossy().to_string();

    let armed = s.call("browser_network", json!({"action": "get"}));
    assert!(!is_error(&armed), "{}", text_of(&armed));
    let page = navigate_with_retry(&mut s, &format!("{}/trace", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_trace", json!({"dir": dir_str, "har": true, "screenshots": false}));
    assert!(!is_error(&r), "trace: {}", text_of(&r));
    let sc = structured(&r);
    let files = sc["files"].as_array().expect("files");
    assert_eq!(files.len(), 1, "HAR only, no step PNG: {sc}");
    assert_eq!(files[0]["name"], "trace.har", "file name: {sc}");
    assert_eq!(files[0]["kind"], "har", "typed file kind: {sc}");
    assert!(dir.join("trace.har").is_file(), "HAR on disk");
    assert!(!dir.join("step-0001.png").exists(), "no screenshot requested");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trace_empty_redact_list_is_accepted_and_named() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let dir = trace_dir("no-redact");
    let dir_str = dir.to_string_lossy().to_string();

    let armed = s.call("browser_network", json!({"action": "get"}));
    assert!(!is_error(&armed), "{}", text_of(&armed));
    let page = navigate_with_retry(&mut s, &format!("{}/trace", fx.base));
    assert!(!is_error(&page), "{}", text_of(&page));

    let r = s.call("browser_trace", json!({
        "dir": dir_str, "har": true, "screenshots": true, "redact": []
    }));
    assert!(!is_error(&r), "an empty redaction list must be valid: {}", text_of(&r));
    let sc = structured(&r);
    assert_eq!(sc["ok"], true, "trace payload: {sc}");
    let redactions = sc["redactions"].as_array().expect("redactions");
    assert!(redactions.is_empty(), "nothing named when nothing is redacted: {sc}");
    assert_eq!(sc["files"].as_array().expect("files").len(), 2, "both artifacts: {sc}");

    let _ = std::fs::remove_dir_all(&dir);
}
