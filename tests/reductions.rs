//! Flake reductions for 0.2.0: actionability verdict+delta, capture
//! stability and failure evidence. One headless browser at a time; every
//! assertion reads a real page observation and the tool's typed result.

mod common;

use common::{is_error, navigate_with_retry, structured, text_of, Session};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner())
}

/// A target that is hidden and disabled at load, becomes visible and enabled
/// 200 ms later, then moves for 500 ms before it settles. The old action
/// fires (or refuses) at call time; the fix waits for a stable, enabled box.
const LATE: &str = r#"<!doctype html><html><head><title>Late</title></head><body>
<button id="late" disabled style="display:none;position:absolute;top:10px;left:0"
  onclick="window.clicked=(window.clicked||0)+1">Late</button>
<button id="never" disabled style="display:none">Never</button>
<script>
setTimeout(function () {
  var b = document.getElementById('late');
  b.style.display = 'block';
  b.disabled = false;
  var start = Date.now();
  function step() {
    var t = Date.now() - start;
    if (t < 500) { b.style.left = (t % 2 < 1 ? 10 : 20) + 'px'; requestAnimationFrame(step); }
    else { b.style.left = '30px'; window.settled = true; }
  }
  requestAnimationFrame(step);
}, 200);
</script></body></html>"#;

/// An animated canvas: every frame changes both position and colour, so two
/// captures taken a few milliseconds apart are genuinely different pixels.
const CANVAS: &str = r#"<!doctype html><html><head><title>Canvas</title></head><body style="margin:0">
<canvas id="c" width="200" height="100"></canvas><script>
var ctx = document.getElementById('c').getContext('2d');
var n = 0;
function draw() {
  n++;
  ctx.fillStyle = '#000'; ctx.fillRect(0, 0, 200, 100);
  ctx.fillStyle = 'rgb(' + (n % 256) + ',80,160)';
  ctx.fillRect((n * 7) % 150, 10, 40, 40);
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);
</script></body></html>"#;

const STILL: &str = r#"<!doctype html><html><head><title>Still</title></head><body><h1>still</h1></body></html>"#;

/// A form whose controls are hidden and disabled at load and become visible
/// and enabled 150 ms later. One control stays hidden the whole time, so a
/// typed timeout can be proved against the same page as a successful wait.
const LATE_INPUT: &str = r#"<!doctype html><html><head><title>LateInput</title></head><body>
<input id="field" disabled style="display:none">
<select id="choice" disabled style="display:none"><option value="a">A</option><option value="b">B</option></select>
<input id="never-field" disabled style="display:none">
<script>
setTimeout(function () {
  var f = document.getElementById('field'); f.style.display = 'block'; f.disabled = false;
  var c = document.getElementById('choice'); c.style.display = 'block'; c.disabled = false;
}, 150);
</script></body></html>"#;

const FORM: &str = r#"<!doctype html><html><head><title>Form</title></head><body>
<form id="f"><input id="name" name="name"><button id="go" type="button">Go</button></form>
<script>console.log('form fixture ready');</script></body></html>"#;

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
    let (status, body): (&str, &str) = match path.as_str() {
        "/late" => ("200 OK", LATE),
        "/late-input" => ("200 OK", LATE_INPUT),
        "/canvas" => ("200 OK", CANVAS),
        "/still" => ("200 OK", STILL),
        "/form" => ("200 OK", FORM),
        "/favicon.ico" => ("204 No Content", ""),
        _ => ("404 Not Found", "missing"),
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

#[test]
fn r3_failing_form_step_carries_evidence_bundle() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/form", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call(
        "browser_form",
        json!({"fields": [
            {"field": "#name", "action": "type", "value": "Ada"},
            {"field": "#bad", "action": "bogus", "value": "x"}
        ]}),
    );
    assert!(!is_error(&resp), "form: {}", text_of(&resp));
    let sc = structured(&resp);
    let steps = sc["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "steps: {sc}");
    assert_eq!(steps[0]["status"], "ok", "steps: {sc}");
    assert_eq!(steps[1]["status"], "error", "steps: {sc}");

    let ev = &steps[1]["evidence"];
    assert!(ev.is_object(), "failing step must carry an evidence bundle: {steps:?}");
    for kind in ["screenshot", "console", "network"] {
        let part = &ev[kind];
        let path = part["path"].as_str().unwrap_or_else(|| panic!("{kind} path: {part}"));
        let bytes = part["bytes"]
            .as_u64()
            .unwrap_or_else(|| panic!("{kind} bytes: {part}"));
        assert!(bytes > 0, "{kind} must carry real bytes: {part}");
        let on_disk = std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("{kind} evidence file {path}: {e}"))
            .len();
        assert_eq!(on_disk, bytes, "{kind} path and bytes must agree: {part}");
    }
}

#[test]
fn r3_action_error_and_step_timeout_carry_evidence() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/form", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let action_error = s.call(
        "browser_form",
        json!({"fields": [{"field": "#missing", "action": "type", "value": "x"}]}),
    );
    let step_timeout = s.call(
        "browser_form",
        json!({"fields": [{"field": "#missing", "action": "type", "value": "x"}],
               "timeoutMs": 200}),
    );
    for resp in [&action_error, &step_timeout] {
        assert!(!is_error(resp), "form: {}", text_of(resp));
        let steps = structured(resp)["steps"].as_array().expect("steps");
        assert_eq!(steps[0]["status"], "error", "steps: {steps:?}");
        let ev = &steps[0]["evidence"];
        assert!(ev.is_object(), "failing step must carry evidence: {steps:?}");
        for kind in ["screenshot", "console", "network"] {
            let path = ev[kind]["path"].as_str().unwrap_or_else(|| panic!("{kind} path: {ev}"));
            let bytes = ev[kind]["bytes"].as_u64().expect("bytes");
            assert!(bytes > 0, "{kind}: {ev}");
            assert_eq!(
                std::fs::metadata(path).expect("evidence file").len(),
                bytes,
                "{kind} path/bytes"
            );
        }
    }
}

#[test]
fn r2_canvas_capture_reports_stability_verdict() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/canvas", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_screenshot", json!({}));
    assert!(!is_error(&resp), "screenshot: {}", text_of(&resp));
    let sc = structured(&resp);
    let st = &sc["stability"];
    assert!(st.is_object(), "screenshot must carry a stability verdict: {sc}");
    let verdict = st["verdict"].as_str().unwrap_or("");
    assert!(
        verdict == "frozen" || verdict == "unfrozen",
        "stability verdict must be typed: {st}"
    );
    assert_eq!(st["code"], verdict, "verdict must be typed: {st}");
    assert!(st["attempts"].as_u64().unwrap_or(0) >= 1, "must compare frames: {st}");
    if verdict == "unfrozen" {
        let sums = st["checksums"].as_array().expect("checksums for an unfrozen capture");
        assert!(sums.len() >= 2, "two differing frames named: {st}");
        assert_ne!(sums[0], sums[1], "unfrozen must be truthful: {st}");
    } else {
        assert_eq!(st["frames"], 2, "frozen needs an equal pair: {st}");
    }
}

#[test]
fn r2_static_capture_is_frozen() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/still", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_screenshot", json!({}));
    assert!(!is_error(&resp), "screenshot: {}", text_of(&resp));
    let st = &structured(&resp)["stability"];
    assert_eq!(st["verdict"], "frozen", "static page must compare equal: {st}");
    assert_eq!(st["frames"], 2, "static page compared two frames: {st}");
}

#[test]
fn r1_click_waits_for_actionable_target_and_reports_delta() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_click", json!({"element": "#late"}));
    assert!(
        !is_error(&resp),
        "late target must land after a bounded wait: {}",
        text_of(&resp)
    );
    let sc = structured(&resp);
    let a = &sc["actionability"];
    assert_eq!(a["verdict"], "settled", "delta: {a}");
    assert_eq!(a["mode"], "probed", "delta: {a}");
    assert_eq!(a["boundMs"], 1500, "delta: {a}");
    assert!(a["waitedMs"].as_u64().unwrap_or(0) >= 50, "delta: {a}");
    assert!(a["polls"].as_u64().unwrap_or(0) >= 2, "delta: {a}");
    assert_eq!(a["visible"], true, "delta: {a}");
    assert_eq!(a["enabled"], true, "delta: {a}");
    assert_eq!(a["stable"], true, "delta: {a}");
    assert_eq!(evaluate(&mut s, "window.clicked || 0"), json!(1));
}

#[test]
fn r1_click_timeout_is_typed_with_delta() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_click", json!({"element": "#never"}));
    assert!(is_error(&resp), "never-actionable target must fail typed");
    let sc = structured(&resp);
    assert_eq!(sc["code"], "timeout", "structured error: {sc}");
    assert_eq!(sc["actionability"]["verdict"], "timeout", "structured error: {sc}");
    assert!(
        sc["actionability"]["waitedMs"].as_u64().unwrap_or(0) >= 1500,
        "waited to the hard bound: {sc}"
    );
}

#[test]
fn r1_type_waits_for_actionable_input_and_readback() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late-input", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_type", json!({"element": "#field", "text": "Ada"}));
    assert!(!is_error(&resp), "late input must accept text after the wait: {}", text_of(&resp));
    let a = &structured(&resp)["actionability"];
    assert_eq!(a["verdict"], "settled", "type delta: {a}");
    assert!(a["waitedMs"].as_u64().unwrap_or(0) >= 50, "type had to wait: {a}");
    assert_eq!(a["visible"], true, "type delta: {a}");
    assert_eq!(a["enabled"], true, "type delta: {a}");
    assert_eq!(evaluate(&mut s, "document.getElementById('field').value"), json!("Ada"));
}

#[test]
fn r1_select_waits_for_actionable_control_and_readback() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late-input", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_select", json!({"element": "#choice", "value": "b"}));
    assert!(!is_error(&resp), "late select must settle: {}", text_of(&resp));
    let a = &structured(&resp)["actionability"];
    assert_eq!(a["verdict"], "settled", "select delta: {a}");
    assert_eq!(a["mode"], "probed", "select delta: {a}");
    assert_eq!(evaluate(&mut s, "document.getElementById('choice').value"), json!("b"));
}

#[test]
fn r1_type_on_hidden_input_is_typed_timeout_with_delta() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late-input", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_type", json!({"element": "#never-field", "text": "x"}));
    assert!(is_error(&resp), "permanently hidden input must refuse typed");
    let sc = structured(&resp);
    assert_eq!(sc["code"], "timeout", "typed code: {sc}");
    assert_eq!(sc["actionability"]["verdict"], "timeout", "delta: {sc}");
    assert!(
        sc["actionability"]["waitedMs"].as_u64().unwrap_or(0) >= 1500,
        "bounded wait: {sc}"
    );
}

#[test]
fn r1_unknown_ref_is_typed_stale_ref_and_lands_nothing() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/late", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_click", json!({"element": "@e999999"}));
    assert!(is_error(&resp), "an unminted ref must refuse typed");
    assert_eq!(structured(&resp)["code"], "stale_ref", "typed code: {}", text_of(&resp));
    assert_eq!(evaluate(&mut s, "window.clicked || 0"), json!(0), "nothing may fire");
}

#[test]
fn r2_element_capture_reports_stability_verdict() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/canvas", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_screenshot", json!({"element": "#c"}));
    assert!(!is_error(&resp), "element capture: {}", text_of(&resp));
    let sc = structured(&resp);
    let st = &sc["stability"];
    assert!(st.is_object(), "element capture must carry stability: {sc}");
    let verdict = st["verdict"].as_str().unwrap_or("");
    assert!(verdict == "frozen" || verdict == "unfrozen", "typed verdict: {st}");
    assert_eq!(st["code"], verdict, "verdict code: {st}");
    assert!(st["attempts"].as_u64().unwrap_or(0) >= 1, "compared frames: {st}");
}

#[test]
fn r2_clip_capture_reports_stability_verdict() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/canvas", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_screenshot", json!({
        "clip": {"x": 0, "y": 0, "width": 100, "height": 50}
    }));
    assert!(!is_error(&resp), "clip capture: {}", text_of(&resp));
    let st = &structured(&resp)["stability"];
    assert!(st.is_object(), "clip capture must carry stability: {st}");
    let verdict = st["verdict"].as_str().unwrap_or("");
    assert!(verdict == "frozen" || verdict == "unfrozen", "typed verdict: {st}");
    assert_eq!(st["code"], verdict, "verdict code: {st}");
}

#[test]
fn r2_screenshot_publishes_stability_in_output_schema() {
    let _g = serial();
    let mut s = Session::spawn();
    let tools = s.list_tools();
    let tool = common::tool(&tools, "browser_screenshot");
    let stability = &tool["outputSchema"]["properties"]["stability"];
    assert!(
        stability["type"] == "object",
        "stability must be a published result field: {tool:?}"
    );
    for tool_name in ["browser_click", "browser_type", "browser_select"] {
        let t = common::tool(&tools, tool_name);
        assert_eq!(
            t["outputSchema"]["properties"]["actionability"]["type"],
            "object",
            "{tool_name} must publish actionability: {t:?}"
        );
    }
}

#[test]
fn r3_form_bad_first_step_still_runs_later_steps() {
    let _g = serial();
    let fx = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/form", fx.base));
    assert!(!is_error(&nav), "nav: {}", text_of(&nav));

    let resp = s.call("browser_form", json!({"fields": [
        {"field": "#missing", "action": "bogus", "value": "x"},
        {"field": "#name", "action": "type", "value": "Ada"}
    ]}));
    assert!(!is_error(&resp), "form: {}", text_of(&resp));
    let sc = structured(&resp);
    let steps = sc["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "one step per field: {sc}");
    assert_eq!(steps[0]["status"], "error", "bad action first: {sc}");
    assert_eq!(steps[1]["status"], "ok", "a bad first step must not abort the batch: {sc}");
    assert_eq!(sc["verdict"], "fail", "one error makes the batch fail: {sc}");
    assert_eq!(evaluate(&mut s, "document.getElementById('name').value"), json!("Ada"));
}
