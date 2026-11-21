//! Regressions for the stdio server's own layer: budget expiry, cancellation,
//! capacity refusal, request-id reuse and EOF shutdown.
//!
//! Each test drives the shipped executable over one stdio connection, the same
//! route a client uses. Where a defect depends on engine state (a capture's
//! restore), the test observes the page itself and proves the restore, not just
//! that a frame was written.

mod common;

use std::time::{Duration, Instant};

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};

/// Mirrors the documented shutdown bounds in `src/server.rs` and `src/main.rs`:
/// owned EOF drain 5 s, one shared abort settle 1 s, browser close bound 30 s,
/// and runtime teardown 2 s. A test about the drain must not be failed by a slow
/// close, so it measures the drain from the log timestamp instead; a test about
/// the whole shutdown asserts against this documented sum.
const DOCUMENTED_SHUTDOWN_BOUND: Duration = Duration::from_secs(38);
const OWNED_DRAIN: Duration = Duration::from_secs(5);
const DRAIN_SLACK: Duration = Duration::from_millis(750);
#[cfg(debug_assertions)]
const RUNTIME_EXIT_BOUND: Duration = Duration::from_secs(2);

/// These tests launch real engines and assert timing-sensitive behavior, so
/// they run one at a time even when the harness defaults to parallel threads.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Evaluate `expression` until it equals `want`, bounded. Replaces fixed sleeps
/// with a condition and a realistic deadline.
///
/// Each probe carries its own request id and is read with [`recv_for`], never
/// with `Session::call`'s "next frame": an early failure frame for a pending
/// capture must not be mistaken for page data.
fn poll_page(s: &mut Session, expression: &str, want: &Value, timeout: Duration) -> Value {
    use std::sync::atomic::{AtomicI64, Ordering};
    static NEXT_ID: AtomicI64 = AtomicI64::new(900_000);

    let deadline = Instant::now() + timeout;
    loop {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "browser_evaluate",
                       "arguments": {"expression": expression}}}));
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "page never reached {want} (expression: {expression})"
        );
        let got = recv_for(s, id, left);
        let value = structured(&got)["value"].clone();
        if &value == want {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("page never reached {want}: last value {value} (expression: {expression})");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Read frames until `id` arrives. Any other frame is a defect in the test's
/// own bookkeeping, not something to skip silently.
fn recv_for(s: &mut Session, id: i64, timeout: Duration) -> Value {
    let frame = s
        .recv_timeout(timeout)
        .unwrap_or_else(|| panic!("no response for id {id} within {timeout:?}"));
    if frame.get("id") == Some(&json!(id)) {
        return frame;
    }
    panic!("unexpected frame while waiting for id {id}: {frame}");
}

/// A TCP listener that accepts connections and never answers. A page image
/// pointing at it stays incomplete, which is what holds a deterministic
/// capture in its image-settle race.
fn hanging_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind hanging server");
    let addr = listener.local_addr().expect("hanging server address");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => break,
            }
        }
    });
    format!("http://{addr}/never.png")
}

/// The shared harness pipes stderr to null; the shutdown-log assertions need
/// it, so this minimal session keeps both pipes. Stderr lines carry the instant
/// they were read, so a test can measure the drain separately from the browser
/// close and the runtime tail.
struct RawSession {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: std::sync::mpsc::Receiver<String>,
    stderr: std::sync::mpsc::Receiver<(Instant, String)>,
}

impl RawSession {
    fn spawn() -> Self {
        Self::spawn_with_env(&[])
    }

    fn spawn_with_env(env: &[(&str, &str)]) -> Self {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hand"));
        cmd.env("HAND_DEBUG", "1");
        cmd.env("HAND_TOOL_BUDGET_MS", "90000");
        cmd.env("GLADIUS_TOOL_BUDGET_MS", "90000");
        cmd.env("GLADIUS_BACKEND_CAPS", "render");
        cmd.env("HAND_TEST_HOOKS", "0");
        cmd.env_remove("GLADIUS_CDP_URL");
        cmd.env_remove("HAND_TEST_STUCK_DESKTOP_MS");
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hand");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx_out, rx_out) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let text = line.trim_end().to_string();
                line.clear();
                if !text.is_empty() && tx_out.send(text).is_err() {
                    break;
                }
            }
        });
        let (tx_err, rx_err) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let text = line.trim_end().to_string();
                line.clear();
                let _ = tx_err.send((Instant::now(), text));
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            stdout: rx_out,
            stderr: rx_err,
        }
    }

    fn send(&mut self, frame: &Value) {
        use std::io::Write;
        let stdin = self.stdin.as_mut().expect("stdin is open");
        let line = serde_json::to_string(frame).expect("frame serializes");
        stdin.write_all(line.as_bytes()).expect("write frame");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush frame");
    }

    fn recv_for(&mut self, id: i64, timeout: Duration) -> Value {
        let line = self
            .stdout
            .recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("no frame for id {id} within {timeout:?}"));
        let frame: Value = serde_json::from_str(&line).expect("frame is JSON");
        if frame.get("id") == Some(&json!(id)) {
            return frame;
        }
        panic!("unexpected frame while waiting for id {id}: {frame}");
    }

    fn initialize(&mut self) {
        self.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "hand-regressions", "version": "0"}}}));
        let init = self.recv_for(1, Duration::from_secs(10));
        assert!(init.get("result").is_some(), "initialize: {init}");
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("hand did not exit within {timeout:?} after EOF");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr_until_closed(&self, timeout: Duration) -> Vec<String> {
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.stderr.recv_timeout(left) {
                Ok((_, line)) => lines.push(line),
                Err(_) => break,
            }
        }
        lines
    }

    /// Read stderr until `needle` appears, returning the instant it arrived and
    /// everything read so far (the caller appends the post-exit remainder).
    fn wait_stderr_timed(&self, needle: &str, timeout: Duration) -> (Instant, Vec<String>) {
        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "stderr never contained {needle:?}; got:\n{}",
                lines.join("\n")
            );
            match self.stderr.recv_timeout(left) {
                Ok((at, line)) => {
                    let hit = line.contains(needle);
                    lines.push(line);
                    if hit {
                        return (at, lines);
                    }
                }
                Err(_) => panic!(
                    "stderr never contained {needle:?}; got:\n{}",
                    lines.join("\n")
                ),
            }
        }
    }

    /// Read stderr until `needle` appears; return everything read so far so the
    /// caller can append the post-exit remainder to the same log.
    #[cfg(debug_assertions)]
    fn wait_stderr_contains(&self, needle: &str, timeout: Duration) -> Vec<String> {
        self.wait_stderr_timed(needle, timeout).1
    }
}

impl Drop for RawSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A budget expiry settles the ANSWER, never the engine future. A dropped
/// deterministic capture never runs its restore, leaving the page frozen with
/// its viewport/media emulation pinned - the reproduced defect.
#[test]
fn budget_timeout_does_not_drop_a_capture_restore() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2), "warm call: {warm}");

    const PROBE: &str = "[!!document.getElementById('__gladius_deterministic'), \
                         window.innerWidth, window.innerHeight, \
                         matchMedia('(prefers-reduced-motion: reduce)').matches]";
    let before = s.call("browser_evaluate", json!({"expression": PROBE}));
    let before = structured(&before)["value"].clone();
    assert_eq!(before[0], json!(false), "no freeze before the capture");
    assert_eq!(
        before.as_array().map(Vec::len),
        Some(4),
        "probe shape: {before}"
    );

    let image = hanging_server();
    let injected = s.call(
        "browser_evaluate",
        json!({"expression": format!(
            "(document.body.appendChild(Object.assign(document.createElement('img'), \
             {{src: '{image}'}})), true)"
        )}),
    );
    assert_eq!(structured(&injected)["value"], json!(true));

    s.send(
        &json!({"jsonrpc": "2.0", "id": 1001, "method": "tools/call",
        "params": {"name": "browser_screenshot",
                   "arguments": {"deterministic": true, "_budgetMs": 2000}}}),
    );

    let frozen = poll_page(
        &mut s,
        "!!document.getElementById('__gladius_deterministic')",
        &json!(true),
        Duration::from_millis(1200),
    );
    assert_eq!(frozen, json!(true));

    let timed = recv_for(&mut s, 1001, Duration::from_secs(10));
    assert!(
        is_error(&timed),
        "a budget expiry is a typed error: {timed}"
    );
    let sc = structured(&timed);
    assert_eq!(sc["code"], json!("timeout"));
    assert_eq!(sc["reason"], json!("budget_exceeded"));
    assert_eq!(sc["stopped"], json!(false));
    assert_eq!(sc["outcome"], json!("unknown"));
    assert_eq!(sc["retry"], json!("unsafe"));
    assert_eq!(sc["sideEffects"], json!("may_still_land"));
    assert_eq!(
        sc["slots"]["heldByThisCall"],
        json!(true),
        "the reply must say this call still holds a slot: {sc}"
    );
    let prose = text_of(&timed);
    assert!(
        prose.contains("NOT stopped"),
        "the reply must say the call was not stopped: {prose}"
    );
    assert!(
        prose.to_lowercase().contains("do not retry"),
        "the reply must warn against a blind retry: {prose}"
    );
    assert!(
        prose.contains("duplicate"),
        "the reply must name why a retry is unsafe: {prose}"
    );
    assert!(
        prose.contains("browser slots"),
        "the reply must name the slot it still holds: {prose}"
    );

    let after = poll_page(&mut s, PROBE, &before, Duration::from_secs(8));
    assert_eq!(
        after, before,
        "the timed-out capture must restore the page it froze: before={before} after={after}"
    );

    let healthy = s.call("browser_evaluate", json!({"expression": "2 + 3"}));
    assert_eq!(structured(&healthy)["value"], json!(5));
}

// D12: the three process-global resources a read-only capture must not touch.
// Read directly from user32 so a timeout or cancel is checked against the OS,
// not only against the engine's own claim.
#[repr(C)]
struct CursorPt {
    x: i32,
    y: i32,
}

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    tick: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
    fn GetForegroundWindow() -> *mut std::ffi::c_void;
    fn GetCursorPos(pt: *mut CursorPt) -> i32;
    fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
    fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, pid: *mut u32) -> u32;
}

#[derive(Clone, Copy)]
struct Globals {
    fg: i64,
    cur: (i32, i32),
    clip: u32,
    input: u32,
}

fn globals_now() -> Globals {
    let mut p = CursorPt { x: 0, y: 0 };
    let cur = if unsafe { GetCursorPos(&mut p) } != 0 {
        (p.x, p.y)
    } else {
        (i32::MIN, i32::MIN)
    };
    let mut info = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        tick: 0,
    };
    let input = if unsafe { GetLastInputInfo(&mut info) } != 0 {
        info.tick
    } else {
        0
    };
    Globals {
        fg: unsafe { GetForegroundWindow() } as i64,
        cur,
        clip: unsafe { GetClipboardSequenceNumber() },
        input,
    }
}

/// The clipboard must be bit-for-bit unchanged. A pointer or foreground delta
/// is only a defect when it is hand's: a moving input tick attributes a cursor
/// move to the owner, and a foreground owned by another process is external.
fn assert_no_steal(before: Globals, after: Globals, server_pid: u32) {
    assert_eq!(after.clip, before.clip, "the clipboard changed");
    if after.fg != before.fg {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(after.fg as *mut std::ffi::c_void, &mut pid) };
        assert_ne!(pid, server_pid, "the server took the foreground");
    }
    if after.cur != before.cur {
        assert_ne!(
            after.input, before.input,
            "the cursor moved with no owner input: hand moved it"
        );
    }
}

fn assert_restore_proof(sc: &Value, scope: &str) {
    for key in ["foregroundUnchanged", "cursorUnchanged", "clipboardUnchanged"] {
        assert_eq!(
            sc[key],
            json!(true),
            "{scope} dropped the {key} restore proof: {sc}"
        );
    }
    assert!(
        sc["noSteal"].is_object(),
        "{scope} must carry the noSteal evidence: {sc}"
    );
}

/// A timed-out desktop capture must settle with the restore proof attached,
/// not drop it with the answer. The stall is the engine's own one-shot hook;
/// the deadline is clamped to its 50 ms floor so the test stays short.
#[test]
fn desktop_capture_timeout_keeps_globals_and_reports_restore() {
    let _serial = serial();
    let mut s = Session::spawn_with_env(&[
        ("PUPPET_TEST_STALL_CAPTURE_MS", "800"),
        ("PUPPET_CAPTURE_TIMEOUT_MS", "50"),
    ]);
    let wins = s.call("desktop_list_windows", json!({}));
    let hwnd = structured(&wins)["windows"][0]["hwnd"]
        .as_i64()
        .expect("a live window handle");
    let before = globals_now();
    let out = s.call(
        "desktop_screenshot",
        json!({"window": format!("hwnd:{hwnd}"), "max_px": 64}),
    );
    assert!(is_error(&out), "a timed-out capture is a typed error: {out}");
    let text = text_of(&out);
    assert!(
        text.contains("capture_timeout"),
        "the error must name the capture timeout: {text}"
    );
    assert_restore_proof(structured(&out), "window/timeout");
    assert_no_steal(before, globals_now(), s.child.id());
}

/// Every scope's error path is a finally-shaped restore: window, monitor,
/// virtual desktop, region and mask. None may answer without the proof.
#[test]
fn desktop_capture_error_carries_restore_proof_for_every_scope() {
    let _serial = serial();
    let mut s = Session::spawn();
    let cases: Vec<(&str, Value)> = vec![
        ("window", json!({"window": "hwnd:99999999", "max_px": 64})),
        ("display=<n>", json!({"display": 999, "max_px": 64})),
        (
            "display=-1",
            json!({"display": -1, "privacyMasks": [{"rect": [50000, 50000, 50010, 50010]}], "max_px": 64}),
        ),
        ("region", json!({"display": -1, "region": [0, 0, 0, 0], "max_px": 64})),
        (
            "mask",
            json!({"display": -1, "privacyMasks": [{"window": "hwnd:99999999"}], "max_px": 64}),
        ),
    ];
    let before = globals_now();
    for (scope, args) in cases {
        let out = s.call("desktop_screenshot", args);
        assert!(is_error(&out), "{scope} must fail: {out}");
        assert_restore_proof(structured(&out), scope);
    }
    assert_no_steal(before, globals_now(), s.child.id());
}

/// A cancelled capture stays silent and leaves all three globals as they were.
#[test]
fn desktop_capture_cancel_leaves_globals_unchanged() {
    let _serial = serial();
    let mut s = RawSession::spawn_with_env(&[
        ("HAND_TEST_HOOKS", "1"),
        ("HAND_TEST_STUCK_DESKTOP_MS", "5000"),
    ]);
    s.initialize();
    let before = globals_now();
    s.send(
        &json!({"jsonrpc": "2.0", "id": 77, "method": "tools/call",
                "params": {"name": "desktop_screenshot",
                           "arguments": {"display": -1, "max_px": 64}}}),
    );
    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": {"requestId": 77}}),
    );
    assert!(
        s.stdout.recv_timeout(Duration::from_secs(1)).is_err(),
        "a cancelled desktop capture must stay silent"
    );
    assert_no_steal(before, globals_now(), s.child.id());
}

/// A cancel answers the request immediately, with the modern marker,
/// and states the truth - the result is discarded while the engine call keeps
/// its slot to finish its own cleanup.
#[test]
fn modern_cancel_is_typed_and_never_claims_the_work_stopped() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    s.send(
        &json!({"jsonrpc": "2.0", "id": 2001, "method": "tools/call",
        "params": {"name": "browser_wait_for", "arguments": {"ms": 4000},
                   "_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}}),
    );
    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 2001}}),
    );

    let cancelled = recv_for(&mut s, 2001, Duration::from_secs(5));
    assert_eq!(
        cancelled["result"]["resultType"],
        json!("complete"),
        "a 2026-07-28 cancel result carries resultType: {cancelled}"
    );
    assert!(
        is_error(&cancelled),
        "cancelled is a typed error: {cancelled}"
    );
    let sc = structured(&cancelled);
    assert_eq!(sc["code"], json!("cancelled"));
    assert_eq!(sc["reason"], json!("client_cancelled"));
    assert_eq!(sc["stopped"], json!(false));
    assert_eq!(sc["outcome"], json!("unknown"));
    assert_eq!(sc["retry"], json!("unsafe"));
    assert_eq!(sc["sideEffects"], json!("may_still_land"));
    assert_eq!(
        sc["slots"]["heldByThisCall"],
        json!(true),
        "a cancelled call still holds its slot: {sc}"
    );
    let prose = text_of(&cancelled);
    assert!(
        prose.contains("NOT stopped"),
        "the acknowledgement must not be mistaken for a stop: {prose}"
    );
    assert!(
        prose.to_lowercase().contains("do not retry"),
        "the reply must warn against a blind retry: {prose}"
    );
    assert!(
        prose.contains("slot"),
        "the reply must say the cancelled call keeps its slot: {prose}"
    );

    let late = s.recv_timeout(Duration::from_millis(4200));
    assert!(
        late.is_none(),
        "a cancelled browser call must not emit a second response: {late:?}"
    );

    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 2001}}),
    );
    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
    let after = s.call("browser_evaluate", json!({"expression": "2 + 3"}));
    assert_eq!(structured(&after)["value"], json!(5));
}

/// The reviewer's residual: the answer must not reduce a timeout to "cleanup
/// continues". An action already sent can still land AFTER the answer, so the
/// outcome is unknown and a retry is unsafe. The page is the proof: the effect
/// is absent when the timeout is written and present afterwards.
#[test]
fn a_timed_out_call_can_still_deliver_its_side_effect() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    s.send(
        &json!({"jsonrpc": "2.0", "id": 1101, "method": "tools/call",
        "params": {"name": "browser_evaluate", "arguments": {
            "_budgetMs": 2000,
            "expression": "new Promise((r) => setTimeout(() => { \
                 window.__late_effect = (window.__late_effect || 0) + 1; r(1); }, 3500))"}}}),
    );
    let timed = recv_for(&mut s, 1101, Duration::from_secs(10));
    assert_eq!(structured(&timed)["outcome"], json!("unknown"));
    assert_eq!(structured(&timed)["retry"], json!("unsafe"));

    let early = s.call(
        "browser_evaluate",
        json!({"expression": "typeof window.__late_effect"}),
    );
    assert_eq!(
        structured(&early)["value"],
        json!("undefined"),
        "the side effect must not have landed before the timeout answer: {early}"
    );
    let _late = poll_page(
        &mut s,
        "window.__late_effect",
        &json!(1),
        Duration::from_secs(6),
    );
}

/// The same residual for cancellation: acknowledging a cancel does not stop an
/// action already in flight, and its effect can land after the acknowledgement.
#[test]
fn a_cancelled_call_can_still_deliver_its_side_effect() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    s.send(&json!({"jsonrpc": "2.0", "id": 1201, "method": "tools/call",
        "params": {"name": "browser_evaluate", "arguments": {
            "expression": "new Promise((r) => setTimeout(() => { \
                 window.__cancelled_effect = (window.__cancelled_effect || 0) + 1; r(1); }, 3000))"}}}));
    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 1201}}),
    );
    let cancelled = recv_for(&mut s, 1201, Duration::from_secs(5));
    assert_eq!(structured(&cancelled)["outcome"], json!("unknown"));
    assert_eq!(structured(&cancelled)["retry"], json!("unsafe"));

    let early = s.call(
        "browser_evaluate",
        json!({"expression": "typeof window.__cancelled_effect"}),
    );
    assert_eq!(structured(&early)["value"], json!("undefined"));
    let _late = poll_page(
        &mut s,
        "window.__cancelled_effect",
        &json!(1),
        Duration::from_secs(6),
    );
}

/// A capacity refusal is a typed tool result, not a protocol error, so a
/// client can tell "retry later" from a broken request.
#[test]
fn a_full_browser_pool_answers_a_typed_busy_tool_result() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    for id in [3001, 3002, 3003, 3004] {
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "browser_wait_for", "arguments": {"ms": 5000}}}));
    }
    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));

    let busy = s.call_id(3005, "browser_evaluate", json!({"expression": "1"}));
    assert!(
        busy.get("error").is_none(),
        "capacity is a tool result, not a protocol error: {busy}"
    );
    assert!(is_error(&busy), "busy is a typed tool error: {busy}");
    let sc = structured(&busy);
    assert_eq!(sc["code"], json!("busy"));
    assert_eq!(sc["reason"], json!("concurrency_limit"));
    assert_eq!(sc["domain"], json!("browser"));
    assert_eq!(sc["limit"], json!(4));
    assert_eq!(sc["inFlight"], json!(4));
    assert_eq!(sc["available"], json!(0));
    assert_eq!(sc["retry"], json!("safe"));
    let prose = text_of(&busy);
    assert!(
        prose.contains("not started"),
        "the reply must say the call did not start: {prose}"
    );
    assert!(
        prose.contains("slots"),
        "the reply must explain the slot accounting: {prose}"
    );

    for id in [3001, 3002, 3003, 3004] {
        s.send(
            &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": id}}),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    while seen.len() < 4 {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = s.recv_timeout(left).expect("four cancel answers");
        seen.push(frame);
    }
    for frame in &seen {
        assert!(is_error(frame), "cancel answers are typed: {frame}");
        assert_eq!(structured(frame)["code"], json!("cancelled"));
    }
}

/// An id already in flight is refused. Without this, the second call
/// silently rides the first call's cancellation token and its answer is
/// swallowed (Puppet's silence contract applies to cancellation only).
#[test]
fn a_desktop_id_in_flight_is_refused_instead_of_riding_a_cancelled_token() {
    let _serial = serial();
    let mut s = Session::spawn();

    s.send(
        &json!({"jsonrpc": "2.0", "id": 4001, "method": "tools/call",
        "params": {"name": "desktop_do",
                   "arguments": {"actions": [{"type": "wait", "ms": 200}]}}}),
    );
    s.send(
        &json!({"jsonrpc": "2.0", "id": 4001, "method": "tools/call",
        "params": {"name": "desktop_cursor", "arguments": {}}}),
    );

    let duplicate = s
        .recv_timeout(Duration::from_secs(2))
        .expect("duplicate refusal");
    assert_eq!(duplicate["id"], json!(4001));
    assert_eq!(duplicate["error"]["code"], json!(-32600), "{duplicate}");
    assert_eq!(
        duplicate["error"]["data"]["reason"],
        json!("duplicate_in_flight"),
        "{duplicate}"
    );
    let first = s
        .recv_timeout(Duration::from_secs(2))
        .expect("the first call's result");
    assert_eq!(first["id"], json!(4001));
    assert!(
        !is_error(&first),
        "the in-flight call keeps its answer: {first}"
    );
    assert_eq!(structured(&first)["ok"], json!(true));

    let fresh = s.call_id(4002, "desktop_cursor", json!({}));
    assert!(fresh.get("result").is_some(), "new id answers: {fresh}");
}

/// EOF accepts no new work, lets an in-flight call finish before closing
/// the engine, and still exits cleanly instead of deadlocking on the drain.
#[test]
fn eof_with_a_running_call_exits_cleanly_and_bounded() {
    let _serial = serial();
    let mut s = Session::spawn();
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    s.send(
        &json!({"jsonrpc": "2.0", "id": 5001, "method": "tools/call",
        "params": {"name": "browser_wait_for", "arguments": {"ms": 3000}}}),
    );
    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
    s.close_stdin();

    let started = Instant::now();
    let status = s.wait_exit(DOCUMENTED_SHUTDOWN_BOUND + Duration::from_secs(5));
    let elapsed = started.elapsed();
    assert!(status.success(), "EOF shutdown must exit 0, got: {status}");
    assert!(
        elapsed < DOCUMENTED_SHUTDOWN_BOUND,
        "shutdown exceeded the documented bound of {DOCUMENTED_SHUTDOWN_BOUND:?}: {elapsed:?}"
    );
}

/// The reviewer's residual: the drain must be bounded for the WHOLE shutdown
/// (grace plus one shared abort settle), not per stuck worker, and the log must
/// name the engine's ownership and say that abandoned cleanup is unverified.
#[test]
fn eof_drain_is_globally_bounded_and_reports_abandoned_work() {
    let _serial = serial();
    let mut s = RawSession::spawn();
    s.initialize();
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "browser_evaluate", "arguments": {"expression": "1 + 1"}}}));
    let warm = s.recv_for(2, Duration::from_secs(60));
    assert_eq!(structured(&warm)["value"], json!(2));

    for id in [11, 12, 13, 14] {
        s.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "browser_evaluate",
                       "arguments": {"expression": "new Promise((r) => setTimeout(r, 20000))"}}}));
    }
    s.send(&json!({"jsonrpc": "2.0", "id": 10, "method": "ping", "params": {}}));
    let pong = s.recv_for(10, Duration::from_secs(5));
    assert_eq!(pong["result"], json!({}));
    s.send(&json!({"jsonrpc": "2.0", "id": 15, "method": "tools/call",
        "params": {"name": "browser_evaluate", "arguments": {"expression": "1"}}}));
    let busy = s.recv_for(15, Duration::from_secs(5));
    assert_eq!(structured(&busy)["code"], json!("busy"), "{busy}");

    s.close_stdin();
    let eof_at = Instant::now();
    let (logged_at, mut log_lines) = s.wait_stderr_timed("drain deadline", Duration::from_secs(15));
    let drain_took = logged_at.saturating_duration_since(eof_at);
    assert!(
        drain_took <= OWNED_DRAIN + DRAIN_SLACK,
        "the drain waited past its {OWNED_DRAIN:?} grace: {drain_took:?}"
    );
    let status = s.wait_exit(DOCUMENTED_SHUTDOWN_BOUND + Duration::from_secs(5));
    assert!(status.success(), "EOF shutdown must exit 0, got: {status}");
    log_lines.extend(s.stderr_until_closed(Duration::from_secs(5)));
    let log = log_lines.join("\n");
    assert!(
        log.contains("shutdown (owned)"),
        "the shutdown log must name engine ownership: {log}"
    );
    assert!(
        log.contains("4 browser call(s)") && log.contains("drain deadline"),
        "the log must count the abandoned browser calls: {log}"
    );
    assert!(
        log.contains("restore") && log.contains("UNVERIFIED"),
        "a browser restore cut at the bound is reported unverified: {log}"
    );
    assert!(
        log.contains("closed next"),
        "the log must say what happens to the owned engine: {log}"
    );
    assert!(
        !log.contains("desktop job"),
        "browser cleanup must not be attributed to desktop jobs: {log}"
    );
    assert!(
        !log.contains("shutdown (attached)"),
        "an owned engine must never be labelled attached: {log}"
    );
}

/// The reviewer's shutdown gap: a started `spawn_blocking` desktop job cannot
/// be aborted, and Tokio's `Runtime::drop` waits for it for ever. The test-only
/// hook (debug builds, double-keyed) runs a controlled blocking job that touches
/// no window; the process must still exit within the advertised bound, and the
/// log must say the job was not stopped (outcome unknown), not that its cleanup
/// was bounded.
#[cfg(debug_assertions)]
#[test]
fn eof_is_bounded_when_a_blocking_desktop_job_cannot_stop() {
    let _serial = serial();
    let mut s = RawSession::spawn_with_env(&[
        ("HAND_TEST_HOOKS", "1"),
        ("HAND_TEST_STUCK_DESKTOP_MS", "60000"),
    ]);
    s.initialize();
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "desktop_cursor", "arguments": {}}}));
    let mut log = s
        .wait_stderr_contains("blocking desktop job started", Duration::from_secs(10))
        .join("\n");
    assert!(
        log.contains("no window is touched") && log.contains("TEST ONLY"),
        "the hook must prove it is test-only and avoids unrelated apps: {log}"
    );

    s.close_stdin();
    let eof_at = Instant::now();
    let (logged_at, mut lines) = s.wait_stderr_timed("drain deadline", Duration::from_secs(15));
    let drain_took = logged_at.saturating_duration_since(eof_at);
    assert!(
        drain_took <= OWNED_DRAIN + DRAIN_SLACK,
        "the drain waited past its {OWNED_DRAIN:?} grace: {drain_took:?}"
    );
    lines.extend(s.stderr_until_closed(Duration::from_secs(5)));
    log.push('\n');
    log.push_str(&lines.join("\n"));

    let status = s.wait_exit(Duration::from_secs(20));
    let elapsed = eof_at.elapsed();
    assert!(status.success(), "EOF shutdown must exit 0, got: {status}");
    let expected =
        OWNED_DRAIN + Duration::from_secs(1) + RUNTIME_EXIT_BOUND + Duration::from_secs(2);
    assert!(
        elapsed < expected,
        "the process must not hang on a blocking desktop job, took {elapsed:?} (bound {expected:?})"
    );

    assert!(
        log.contains("1 desktop job(s)") && log.contains("drain deadline"),
        "the log must count the abandoned desktop job: {log}"
    );
    assert!(
        log.contains("CANNOT be aborted"),
        "a desktop job is not browser cleanup: {log}"
    );
    assert!(
        log.contains("were still running and were NOT stopped")
            && log.contains("outcome is UNKNOWN"),
        "the runtime bound must state the job was not stopped: {log}"
    );
    assert!(
        log.contains("runtime teardown reached its"),
        "the runtime bound must be logged: {log}"
    );
    assert!(
        !log.contains("browser call(s)"),
        "no browser call was in flight in this test: {log}"
    );
}

/// Double-key proof: a stray `HAND_TEST_STUCK_DESKTOP_MS` alone must not divert
/// a desktop tool in a debug binary. Only `HAND_TEST_HOOKS=1` arms the hook.
#[cfg(debug_assertions)]
#[test]
fn a_stray_stuck_desktop_ms_without_the_hooks_key_is_ignored() {
    let _serial = serial();
    let mut s = RawSession::spawn_with_env(&[("HAND_TEST_STUCK_DESKTOP_MS", "60000")]);
    s.initialize();
    s.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "desktop_cursor", "arguments": {}}}));
    let answer = s.recv_for(2, Duration::from_secs(5));
    assert!(
        answer.get("result").is_some(),
        "the hook must require HAND_TEST_HOOKS=1, not just the duration: {answer}"
    );
    s.close_stdin();
    let status = s.wait_exit(Duration::from_secs(20));
    assert!(status.success(), "clean exit, got: {status}");
}

/// A cancelled desktop id stays silent over the wire, whatever the job's
/// outcome; the next id is unaffected.
#[test]
fn a_cancelled_desktop_call_stays_silent_over_the_wire() {
    let _serial = serial();
    let mut s = Session::spawn();
    s.send(
        &json!({"jsonrpc": "2.0", "id": 6001, "method": "tools/call",
        "params": {"name": "desktop_do",
                   "arguments": {"actions": [{"type": "wait", "ms": 250}]}}}),
    );
    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 6001}}),
    );
    assert!(
        s.recv_timeout(Duration::from_secs(1)).is_none(),
        "a cancelled desktop id must stay silent"
    );
    let fresh = s.call_id(6002, "desktop_cursor", json!({}));
    assert!(fresh.get("result").is_some(), "new id answers: {fresh}");
}


/// A CDP reply larger than one WebSocket frame is a big page, not a dead
/// engine.
///
/// `Accessibility.getFullAXTree` answers a large page in ONE frame (measured
/// on chrome-headless-shell 155: 1 683 bytes of AX JSON per list item). The
/// transport's default 16 MiB frame ceiling rejected that frame,
/// tokio-tungstenite fuses the stream on any error, and the reader's exit was
/// read as engine death - so the recovery path reaped a healthy browser and
/// the page was lost. The 5 000-item page always survived because its frame
/// fit and the render was refused by the 8 000-char page cap instead. This
/// pins the larger page to that same shape: a typed refusal, no transport
/// death, and the page the engine already had still answering.
#[test]
fn a_snapshot_reply_larger_than_one_transport_frame_keeps_the_engine() {
    let _serial = serial();
    let mut s = Session::spawn();
    const ITEMS: usize = 12_000;
    let rows: String = (0..ITEMS).map(|i| format!("<li>row {i}</li>")).collect();
    let url = format!("data:text/html,<ul>{rows}</ul>");
    let nav = s.call("browser_navigate", json!({"url": url}));
    assert!(!is_error(&nav), "navigate: {}", text_of(&nav));

    let snap = s.call("browser_snapshot", json!({}));
    let prose = text_of(&snap);
    assert!(
        !prose.contains("response channel closed"),
        "a large but valid reply must not read as transport death: {prose}"
    );
    assert!(
        is_error(&snap),
        "a page one line cannot page is refused, not answered: {prose}"
    );
    let sc = structured(&snap);
    assert_eq!(sc["code"], json!("invalid_args"), "typed refusal: {sc}");
    assert_eq!(sc["reason"], json!("bad_value"), "typed refusal: {sc}");
    assert!(
        prose.contains("page cap"),
        "the refusal is the same capped error the smaller page gets: {prose}"
    );

    let count = s.call(
        "browser_evaluate",
        json!({"expression": "document.querySelectorAll('li').length"}),
    );
    assert_eq!(
        structured(&count)["value"],
        json!(ITEMS),
        "the rendered page must still be the one answering: {}",
        text_of(&count)
    );
    let tabs = s.call("browser_tabs", json!({"action": "list"}));
    assert!(!is_error(&tabs), "tabs after the refusal: {}", text_of(&tabs));
    assert_eq!(
        structured(&tabs)["tabs"].as_array().map(Vec::len),
        Some(1),
        "one tab, the surviving page: {}",
        text_of(&tabs)
    );
}

/// An out-of-range tab index is refused when it is asked, not when a CDP
/// deadline expires.
///
/// `select` waited up to 10 s for the index to appear, which is the right
/// tolerance for a tab a click just opened (measured: listed ~120 ms after
/// `click_settle`) and the wrong answer for an index no tab can occupy - the
/// caller paid the whole deadline for the same typed error. An index at the
/// end of the list keeps the bounded settle; a wider gap is refused on the
/// spot.
#[test]
fn an_out_of_range_tab_select_is_refused_without_waiting_on_the_deadline() {
    let _serial = serial();
    let mut s = Session::spawn();
    let nav = s.call("browser_navigate", json!({"url": "about:blank"}));
    assert!(!is_error(&nav), "navigate: {}", text_of(&nav));

    let started = Instant::now();
    let one_past = s.call("browser_tabs", json!({"action": "select", "index": 1}));
    let elapsed = started.elapsed();
    assert!(is_error(&one_past), "an absent index is refused: {one_past}");
    assert_eq!(structured(&one_past)["code"], json!("invalid_target"));
    assert_eq!(structured(&one_past)["reason"], json!("not_found"));
    assert!(
        text_of(&one_past).contains("tabs: no tab at index 1"),
        "the refusal names the index: {}",
        text_of(&one_past)
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "the refusal must not wait on a 10 s deadline (took {elapsed:?})"
    );

    let started = Instant::now();
    let far = s.call("browser_tabs", json!({"action": "select", "index": 9999}));
    let elapsed = started.elapsed();
    assert!(is_error(&far), "a far index is refused: {far}");
    assert!(
        text_of(&far).contains("tabs: no tab at index 9999"),
        "the refusal names the index: {}",
        text_of(&far)
    );
    assert!(
        elapsed < Duration::from_millis(300),
        "an index the list cannot reach is refused without a settle (took {elapsed:?})"
    );

    let tabs = s.call("browser_tabs", json!({"action": "list"}));
    assert!(!is_error(&tabs), "tabs after the refusals: {}", text_of(&tabs));
}
