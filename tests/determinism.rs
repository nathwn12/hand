//! Context-scoped determinism over the real MCP endpoint. Every claim is a
//! page read-back, never the tool repeating its request: the page's own
//! `Date.now()`, `Intl.DateTimeFormat().resolvedOptions()`,
//! `navigator.permissions.query()` and emptied storage are the evidence.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use common::{is_error, navigate_with_retry, structured, text_of, Session};
use serde_json::{json, Value};

const PAGE: &str = r#"<!doctype html><html><head><title>Determinism</title></head>
<body><p id="clock">pending</p>
<script>document.getElementById('clock').textContent = 'clock: ' + Date.now();</script>
</body></html>"#;

/// Concurrent browser launches flake on this machine (a navigate can answer
/// net::ERR_ABORTED), so this binary runs one browser at a time.
static ONE_BROWSER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner())
}

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
    let _ = n;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        PAGE.len(),
        PAGE
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn value(resp: &Value, what: &str) -> Value {
    assert!(!is_error(resp), "{} failed: {}", what, text_of(resp));
    structured(resp)["value"].clone()
}

/// Evaluate in a named context and return the page's own value.
fn ctx_eval(s: &mut Session, name: &str, expression: &str) -> Value {
    let r = s.call(
        "browser_context",
        json!({"action": "eval", "name": name, "expression": expression}),
    );
    value(&r, "context eval")
}

fn ctx_create(s: &mut Session, name: &str, url: &str) {
    let r = s.call(
        "browser_context",
        json!({"action": "create", "name": name, "url": url}),
    );
    assert!(!is_error(&r), "create {name}: {}", text_of(&r));
}

#[test]
fn clock_is_read_back_from_the_page_after_override() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/", fixture.base));
    assert!(!is_error(&nav), "navigate failed: {}", text_of(&nav));

    let frozen = s.call(
        "browser_context",
        json!({"action": "clock", "iso": "2026-01-02T03:04:05.000Z"}),
    );
    assert!(!is_error(&frozen), "clock failed: {}", text_of(&frozen));
    assert_eq!(
        structured(&frozen)["observed"],
        json!(1767323045000i64),
        "the tool's own receipt is checked, then the page is the proof"
    );
    let read = s.call(
        "browser_evaluate",
        json!({"expression": "document.getElementById('clock').textContent"}),
    );
    assert_eq!(
        value(&read, "legacy evaluate"),
        json!("clock: 1767323045000"),
        "the page must have computed the frozen instant itself"
    );
}

#[test]
fn locale_and_timezone_are_read_back_from_the_page() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let nav = navigate_with_retry(&mut s, &format!("{}/", fixture.base));
    assert!(!is_error(&nav), "navigate failed: {}", text_of(&nav));

    let set = s.call(
        "browser_context",
        json!({"action": "locale", "locale": "fr-FR", "timezone": "Europe/Paris"}),
    );
    assert!(!is_error(&set), "locale failed: {}", text_of(&set));
    assert_eq!(structured(&set)["observed"], json!("fr-FR|Europe/Paris"));
    let read = s.call(
        "browser_evaluate",
        json!({"expression": "Intl.DateTimeFormat().resolvedOptions().locale + '|' + Intl.DateTimeFormat().resolvedOptions().timeZone"}),
    );
    assert_eq!(
        value(&read, "legacy evaluate"),
        json!("fr-FR|Europe/Paris"),
        "the page must read back the override, not the tool's claim"
    );
}

#[test]
fn permission_grant_and_reset_are_read_back_from_the_context_page() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let created = s.call(
        "browser_context",
        json!({"action": "create", "name": "perm-a", "url": format!("{}/", fixture.base)}),
    );
    assert!(!is_error(&created), "create failed: {}", text_of(&created));

    let granted = s.call(
        "browser_context",
        json!({"action": "permissions", "name": "perm-a", "grant": ["geolocation"]}),
    );
    assert!(!is_error(&granted), "permissions failed: {}", text_of(&granted));
    let read = s.call(
        "browser_context",
        json!({"action": "eval", "name": "perm-a",
               "expression": "navigator.permissions.query({name:'geolocation'}).then(p => p.state)"}),
    );
    assert_eq!(value(&read, "permission read-back"), json!("granted"));

    let seeded = s.call(
        "browser_context",
        json!({"action": "eval", "name": "perm-a",
               "expression": "localStorage.setItem('k','v'); sessionStorage.setItem('s','v'); 'seeded'"}),
    );
    assert_eq!(value(&seeded, "seed storage"), json!("seeded"));
    let reset = s.call("browser_context", json!({"action": "reset", "name": "perm-a"}));
    assert!(!is_error(&reset), "reset failed: {}", text_of(&reset));
    let after = s.call(
        "browser_context",
        json!({"action": "eval", "name": "perm-a",
               "expression": "localStorage.length + '|' + sessionStorage.length + '|' + document.cookie"}),
    );
    assert_eq!(
        value(&after, "storage read-back"),
        json!("0|0|"),
        "the page must show empty storage after reset"
    );

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "destroy_all failed: {}", text_of(&destroyed));
}

#[test]
fn state_exports_and_imports_across_two_contexts() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let url = format!("{}/", fixture.base);

    let source = s.call(
        "browser_context",
        json!({"action": "create", "name": "src", "url": url}),
    );
    assert!(!is_error(&source), "create source failed: {}", text_of(&source));
    let cookie = s.call(
        "browser_context",
        json!({"action": "cookies", "name": "src",
               "cookies": [{"name": "sid", "value": "abc123", "url": url}]}),
    );
    assert!(!is_error(&cookie), "set cookie failed: {}", text_of(&cookie));
    let seeded = s.call(
        "browser_context",
        json!({"action": "eval", "name": "src",
               "expression": "localStorage.setItem('theme','dark'); localStorage.setItem('n','7'); 'ok'"}),
    );
    assert_eq!(value(&seeded, "seed localStorage"), json!("ok"));

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let file = std::env::temp_dir().join(format!(
        "hand-state-{}-{nanos}.json",
        std::process::id()
    ));
    let path = file.to_string_lossy().to_string();

    let exported = s.call(
        "browser_context",
        json!({"action": "export_state", "name": "src", "path": path}),
    );
    assert!(!is_error(&exported), "export failed: {}", text_of(&exported));
    assert_eq!(structured(&exported)["cookies"], json!(1));
    assert_eq!(structured(&exported)["localStorage"], json!(2));
    assert!(file.is_file(), "the caller-supplied artifact must exist");

    let fresh = s.call(
        "browser_context",
        json!({"action": "create", "name": "dst", "url": url}),
    );
    assert!(!is_error(&fresh), "create target failed: {}", text_of(&fresh));
    let imported = s.call(
        "browser_context",
        json!({"action": "import_state", "name": "dst", "path": path}),
    );
    assert!(!is_error(&imported), "import failed: {}", text_of(&imported));
    assert_eq!(structured(&imported)["verifiedCookies"], json!(1));
    assert_eq!(structured(&imported)["verifiedLocalStorage"], json!(2));

    let cookie_read = s.call(
        "browser_context",
        json!({"action": "eval", "name": "dst", "expression": "document.cookie"}),
    );
    assert!(
        value(&cookie_read, "cookie read-back")
            .as_str()
            .unwrap_or("")
            .contains("sid=abc123"),
        "the session cookie must survive the export/import round trip"
    );
    let storage_read = s.call(
        "browser_context",
        json!({"action": "eval", "name": "dst",
               "expression": "JSON.stringify(Object.entries(localStorage))"}),
    );
    let text = value(&storage_read, "storage read-back").as_str().unwrap_or("").to_string();
    assert!(text.contains("theme") && text.contains("dark") && text.contains("n"));
    assert!(text.contains("'7'") || text.contains("\"7\""), "the second item must round-trip: {text}");

    let other = s.call(
        "browser_context",
        json!({"action": "create", "name": "other", "url": url}),
    );
    assert!(!is_error(&other), "create third context failed: {}", text_of(&other));
    let untouched = s.call(
        "browser_context",
        json!({"action": "eval", "name": "other",
               "expression": "document.cookie + '|' + localStorage.length"}),
    );
    assert_eq!(
        value(&untouched, "isolation read-back"),
        json!("|0"),
        "a context that did not import must stay empty"
    );

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "destroy_all failed: {}", text_of(&destroyed));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn clock_is_scoped_to_the_named_context() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let url = format!("{}/", fixture.base);
    ctx_create(&mut s, "clka", &url);
    ctx_create(&mut s, "clkb", &url);

    let frozen = 1767323045000i64;
    let set = s.call(
        "browser_context",
        json!({"action": "clock", "name": "clka", "iso": "2026-01-02T03:04:05.000Z"}),
    );
    assert!(!is_error(&set), "clock failed: {}", text_of(&set));
    assert_eq!(structured(&set)["observed"], json!(frozen), "receipt: {set}");

    assert_eq!(ctx_eval(&mut s, "clka", "Date.now()"), json!(frozen), "named context must read the frozen instant");
    let real = ctx_eval(&mut s, "clkb", "Date.now()")
        .as_i64()
        .expect("real clock value");
    assert_ne!(real, frozen, "the other context must keep its own clock");
    assert!(real > frozen, "the untouched context reads wall time: {real}");

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "{}", text_of(&destroyed));
}

#[test]
fn locale_is_scoped_to_the_named_context() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let url = format!("{}/", fixture.base);
    ctx_create(&mut s, "loca", &url);
    ctx_create(&mut s, "locb", &url);

    let set = s.call(
        "browser_context",
        json!({"action": "locale", "name": "loca", "locale": "fr-FR", "timezone": "Europe/Paris"}),
    );
    assert!(!is_error(&set), "locale failed: {}", text_of(&set));
    let expr = "Intl.DateTimeFormat().resolvedOptions().locale + '|' + Intl.DateTimeFormat().resolvedOptions().timeZone";
    let a = ctx_eval(&mut s, "loca", expr);
    let b = ctx_eval(&mut s, "locb", expr);
    assert_eq!(a, json!("fr-FR|Europe/Paris"), "named context locale: {a}");
    assert_ne!(b, a, "the other context must keep its own locale: {b}");

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "{}", text_of(&destroyed));
}

#[test]
fn reset_clears_only_the_named_context() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    let url = format!("{}/", fixture.base);
    ctx_create(&mut s, "rsa", &url);
    ctx_create(&mut s, "rsb", &url);

    for name in ["rsa", "rsb"] {
        assert_eq!(
            ctx_eval(&mut s, name, "localStorage.setItem('seed','1'); localStorage.length"),
            json!(1),
            "seed {name}"
        );
    }
    let reset = s.call("browser_context", json!({"action": "reset", "name": "rsa"}));
    assert!(!is_error(&reset), "reset failed: {}", text_of(&reset));

    assert_eq!(ctx_eval(&mut s, "rsa", "localStorage.length"), json!(0), "reset context is empty");
    assert_eq!(ctx_eval(&mut s, "rsb", "localStorage.length"), json!(1), "other context is untouched");

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "{}", text_of(&destroyed));
}

#[test]
fn clock_override_refuses_a_non_rfc3339_instant() {
    let _serial = serial();
    let mut s = Session::spawn();
    let resp = s.call("browser_context", json!({"action": "clock", "iso": "yesterday"}));
    assert!(is_error(&resp), "a non-instant must refuse typed");
    // `new Date('yesterday').getTime()` is NaN, so the engine refuses the
    // non-value as a typed js_error before it can install a broken clock.
    assert_eq!(structured(&resp)["code"], "js_error", "typed code: {}", text_of(&resp));
}

#[test]
fn export_state_relative_path_is_a_typed_refusal() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    ctx_create(&mut s, "exr", &format!("{}/", fixture.base));

    let resp = s.call(
        "browser_context",
        json!({"action": "export_state", "name": "exr", "path": "relative-state.json"}),
    );
    assert!(is_error(&resp), "a relative path must refuse typed");
    let sc = structured(&resp);
    assert_eq!(sc["code"], "invalid_args", "typed code: {sc}");
    assert!(
        text_of(&resp).to_lowercase().contains("absolute"),
        "refusal must say the path needs to be absolute: {}",
        text_of(&resp)
    );

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "{}", text_of(&destroyed));
}

#[test]
fn import_state_missing_file_is_a_typed_refusal() {
    let _serial = serial();
    let fixture = Fixture::start();
    let mut s = Session::spawn();
    ctx_create(&mut s, "imp", &format!("{}/", fixture.base));

    let missing = std::env::temp_dir().join(format!(
        "hand-missing-state-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
    ));
    let resp = s.call(
        "browser_context",
        json!({"action": "import_state", "name": "imp", "path": missing.to_string_lossy()}),
    );
    assert!(is_error(&resp), "a missing file must refuse typed");
    let sc = structured(&resp);
    assert_eq!(sc["code"], "file_not_found", "typed code: {sc}");
    assert_eq!(sc["reason"], "unreadable", "typed reason: {sc}");

    let destroyed = s.call("browser_context", json!({"action": "destroy_all"}));
    assert!(!is_error(&destroyed), "{}", text_of(&destroyed));
}
