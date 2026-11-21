//! Acceptance scenario: the first desktop ACT/VERIFY loop (spec § Acceptance
//! scenario 3, positive half, and scenario 11's private-desktop half).
//!
//! One controlled fixture program runs on a PRIVATE Windows desktop that the
//! desktop engine created for this call. The engine posts a keyboard act and a
//! pointer act into it; the fixture's own window procedure records what
//! arrived in its own journal; the engine reads its controls back. The owner's
//! foreground window and physical pointer are watched throughout.
//!
//! The two claims this test separates, because a delivered message is not an
//! effect:
//!
//! * **delivered** — the tool posted the messages (`posted`, `delivered`,
//!   `effect:"unverified"` for the raw click);
//! * **the application acted** — the fixture app's journal shows its EDIT
//!   holding the typed text, its own hit test finding the posted point inside
//!   its button rect (so the screen/client mapping is exact and the intended
//!   control was aimed at), and the BUTTON control's own `BN_CLICKED`
//!   notification flipping the fixture's state; the engine's readback of the
//!   same controls reports the same post-state.
//!
//! Not claimed here: that every application accepts posted input (it does not;
//! the fixture is a cooperating app), that UIA patterns work on a private
//! desktop, or anything about a joined visible-browser/desktop scenario. An
//! ambiguous element selector is refused by the engine's own element resolver
//! and is not re-proven here; the stale-target half is.
//!
//! Prerequisites: an interactive Windows session (the engine refuses to launch
//! where a private desktop cannot be established) and a `rustc` to compile the
//! fixture. Both are asserted, never silently skipped.

mod common;

#[path = "desktop_fixture/layout.rs"]
#[allow(dead_code)]
mod layout;

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};


#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut c_void;
    fn GetCursorPos(pt: *mut CursorPoint) -> i32;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
    fn EnumWindows(
        cb: Option<unsafe extern "system" fn(*mut c_void, isize) -> i32>,
        l: isize,
    ) -> i32;
    fn GetClassNameW(hwnd: *mut c_void, buf: *mut u16, max: i32) -> i32;
}

#[repr(C)]
struct CursorPoint {
    x: i32,
    y: i32,
}

fn foreground_now() -> i64 {
    unsafe { GetForegroundWindow() as i64 }
}

fn cursor_now() -> (i32, i32) {
    let mut p = CursorPoint { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut p) } != 0 {
        (p.x, p.y)
    } else {
        (-1, -1)
    }
}

fn pid_of(hwnd: i64) -> Option<u32> {
    if hwnd == 0 {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd as *mut c_void, &mut pid) };
    (pid != 0).then_some(pid)
}

struct ClassScan {
    class: String,
    hits: Vec<i64>,
}

unsafe extern "system" fn class_scan_cb(hwnd: *mut c_void, l: isize) -> i32 {
    let scan = &mut *(l as *mut ClassScan);
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == scan.class {
        scan.hits.push(hwnd as i64);
    }
    1
}

/// Every window on THIS thread's desktop (the owner's) carrying `class` — the
/// fixture's class is unique to the fixture, so a non-empty answer means a
/// private-desktop window leaked onto the user's desktop.
fn owner_desktop_windows_of_class(class: &str) -> Vec<i64> {
    let mut scan = ClassScan {
        class: class.to_string(),
        hits: Vec::new(),
    };
    unsafe {
        EnumWindows(Some(class_scan_cb), &mut scan as *mut ClassScan as isize);
    }
    scan.hits
}


/// Samples the OWNER's desktop while the act runs, from this process, so
/// "nothing of ours ever appeared on the user's desktop" is measured
/// independently of what the server reports about itself.
struct OwnerDeskWatch {
    stop: Arc<AtomicBool>,
    samples: Arc<AtomicUsize>,
    desk_hits: Arc<AtomicUsize>,
    fg_hits: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
}

impl OwnerDeskWatch {
    /// `attributable` is the set of pids this scenario answers for (this test,
    /// the server under test, the launched fixture). A foreground window owned
    /// by one of those is a steal; anybody else is the owner using the machine.
    fn start(attributable: Vec<u32>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(AtomicUsize::new(0));
        let desk_hits = Arc::new(AtomicUsize::new(0));
        let fg_hits = Arc::new(AtomicUsize::new(0));
        let handle = {
            let (stop, samples) = (stop.clone(), samples.clone());
            let (desk_hits, fg_hits) = (desk_hits.clone(), fg_hits.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    samples.fetch_add(1, Ordering::SeqCst);
                    if !owner_desktop_windows_of_class(layout::WINDOW_CLASS).is_empty() {
                        desk_hits.fetch_add(1, Ordering::SeqCst);
                    }
                    let fg = foreground_now();
                    if fg != 0 && pid_of(fg).is_some_and(|p| attributable.contains(&p)) {
                        fg_hits.fetch_add(1, Ordering::SeqCst);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
        };
        OwnerDeskWatch {
            stop,
            samples,
            desk_hits,
            fg_hits,
            handle: Some(handle),
        }
    }

    /// `(samples, owner-desktop hits, attributable foreground samples)`.
    fn finish(mut self) -> (usize, usize, usize) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        (
            self.samples.load(Ordering::SeqCst),
            self.desk_hits.load(Ordering::SeqCst),
            self.fg_hits.load(Ordering::SeqCst),
        )
    }
}


/// The controlled fixture program, compiled from
/// `tests/desktop_fixture/fixture_app.rs` with `rustc` (no cargo, no crates)
/// into a disposable directory, plus the journal file it writes.
struct FixtureApp {
    dir: PathBuf,
    exe: PathBuf,
    journal: PathBuf,
}

impl FixtureApp {
    fn build() -> Self {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("desktop_fixture")
            .join("fixture_app.rs");
        assert!(
            src.is_file(),
            "fixture source must exist: {}",
            src.display()
        );
        let tmp = std::env::temp_dir();
        let base = if tmp.join("opencode").is_dir() {
            tmp.join("opencode")
        } else {
            tmp
        };
        let dir = base.join(format!(
            "hand-desktop-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("fixture directory is creatable");
        let exe = dir.join("hand_fixture.exe");
        let rustc = rustc_path();
        let out = Command::new(&rustc)
            .args(["--edition", "2021", "-A", "warnings", "-o"])
            .arg(&exe)
            .arg(&src)
            .output()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", rustc.display()));
        assert!(
            out.status.success(),
            "rustc must build the fixture with {}:\n{}\n{}",
            rustc.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            exe.is_file(),
            "fixture executable must exist: {}",
            exe.display()
        );
        assert_eq!(
            pe_subsystem(&exe),
            Some(2),
            "the fixture must link as a Windows GUI binary (subsystem 2): a console window on \
             the private desktop would compete with the fixture window for discovery"
        );
        FixtureApp {
            journal: dir.join("journal.txt"),
            dir,
            exe,
        }
    }

    fn exe_arg(&self) -> String {
        self.exe.to_string_lossy().into_owned()
    }

    fn journal_arg(&self) -> String {
        self.journal.to_string_lossy().into_owned()
    }

    /// The journal the app itself wrote, one event per line, in order.
    fn journal_lines(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.journal).unwrap_or_default();
        text.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }
}

impl Drop for FixtureApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// PE optional-header `Subsystem`: 2 = Windows GUI, 3 = console. Read from the
/// built fixture so the test states the property it depends on instead of
/// hoping window discovery agrees.
fn pe_subsystem(exe: &Path) -> Option<u16> {
    let bytes = std::fs::read(exe).ok()?;
    let lfanew = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    let subsystem = lfanew.checked_add(4 + 20 + 68)?;
    Some(u16::from_le_bytes(
        bytes.get(subsystem..subsystem + 2)?.try_into().ok()?,
    ))
}

/// `rustc` from the environment, the PATH, or the rustup home. The fixture is
/// a build product of this test, so a missing compiler is a failed
/// prerequisite, not a skipped proof.
fn rustc_path() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(r) = std::env::var("RUSTC") {
        if !r.trim().is_empty() {
            candidates.push(PathBuf::from(r));
        }
    }
    candidates.push(PathBuf::from("rustc"));
    if let Ok(home) = std::env::var("CARGO_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join("rustc.exe"));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(profile)
                .join(".cargo")
                .join("bin")
                .join("rustc.exe"),
        );
    }
    for c in &candidates {
        let ok = Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return c.clone();
        }
    }
    panic!(
        "no working rustc found (tried {candidates:?}); the desktop fixture is compiled from \
         tests/desktop_fixture/fixture_app.rs at test time and cannot exist without one"
    );
}


/// Locate a line by prefix (`key=...` / `key ...`) and return the index.
fn find_line(lines: &[String], prefix: &str) -> Option<usize> {
    lines.iter().position(|l| {
        l.starts_with(prefix) && !l[prefix.len()..].starts_with(|c: char| c.is_ascii_alphanumeric())
    })
}

/// `key=value` from a journal line (`ready pid=1 hwnd=2`).
fn field_i64(line: &str, key: &str) -> Option<i64> {
    line.split_whitespace()
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse::<i64>().ok())
}

/// The fixture is launched BY the product under test, so it must not contain
/// the APIs the product's no-steal invariant bans: a test helper that steals
/// would make the scenario's own evidence worthless. Source-level, like
/// `tests/no_steal.rs`, and deliberately strict about activation: the only
/// show call may be the non-activating one.
#[test]
fn the_fixture_app_holds_no_pointer_foreground_or_activation_api() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("desktop_fixture")
            .join("fixture_app.rs"),
    )
    .expect("fixture source must be readable");
    for banned in [
        "SendInput(",
        "SetCursorPos(",
        "SetForegroundWindow(",
        "SetActiveWindow(",
        "BringWindowToTop(",
        "mouse_event(",
        "keybd_event(",
    ] {
        assert!(
            !src.contains(banned),
            "the fixture app must hold no call to {banned}: it is launched by hand and its \
             journal is this scenario's effect oracle"
        );
    }
    let show_calls: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| l.contains("ShowWindow(") && !l.starts_with("fn "))
        .collect();
    assert_eq!(
        show_calls,
        vec!["ShowWindow(hwnd, SW_SHOWNOACTIVATE);"],
        "the fixture's only show call must be the non-activating one"
    );
    println!("HAND_FIXTURE_STATIC_OK banned=7 show=1(SW_SHOWNOACTIVATE)");
}

#[test]
fn desktop_act_verify_on_a_private_desktop_leaves_the_owner_alone() {
    let app = FixtureApp::build();
    let mut s = Session::spawn();
    let own_pid = std::process::id();
    let server_pid = s.child.id();
    let fg_before = foreground_now();
    let cursor_before = cursor_now();
    let (click_x, click_y) = layout::button_screen_centre();
    let typed_units = layout::TYPED_TEXT.encode_utf16().count();

    let watch = OwnerDeskWatch::start(vec![own_pid, server_pid]);

    let response = s.call(
        "desktop_do",
        json!({
            "desktop": "hidden",
            "background": app.exe_arg(),
            "background_args": [app.journal_arg()],
            "actions": [
                {"type": "type", "text": layout::TYPED_TEXT},
                {"type": "click", "x": click_x, "y": click_y},
            ],
        }),
    );
    assert!(
        !is_error(&response),
        "the private-desktop act/verify call must not be a tool error: {}",
        text_of(&response)
    );
    let sc = structured(&response);

    assert_eq!(
        sc["ok"],
        Value::from(true),
        "every action must succeed: {sc}"
    );
    assert_eq!(sc["failed"], Value::from(0), "{sc}");
    assert_eq!(
        sc["sendinput_calls"],
        Value::from(0),
        "a private-desktop burst must post messages, never inject: {sc}"
    );
    let results = sc["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "both actions report: {sc}");

    let typed = &results[0];
    assert_eq!(typed["type"].as_str(), Some("type"), "{typed}");
    assert_eq!(typed["ok"].as_bool(), Some(true), "{typed}");
    assert_eq!(typed["mode"].as_str(), Some("background"), "{typed}");
    assert_eq!(
        typed["posted"].as_u64(),
        Some(typed_units as u64),
        "one posted WM_CHAR per UTF-16 unit reached the text control: {typed}"
    );

    let clicked = &results[1];
    assert_eq!(clicked["type"].as_str(), Some("click"), "{clicked}");
    assert_eq!(clicked["ok"].as_bool(), Some(true), "{clicked}");
    assert_eq!(
        clicked["delivered"].as_str(),
        Some("posted"),
        "the click names its delivery: {clicked}"
    );
    assert_eq!(
        clicked["effect"].as_str(),
        Some("unverified"),
        "a posted x/y click proves delivery, not an effect: {clicked}"
    );
    assert_eq!(clicked["posted"].as_u64(), Some(2), "down + up: {clicked}");

    let hd = &sc["hidden_desktop"];
    assert_eq!(hd["desktop_created"].as_bool(), Some(true), "{hd}");
    assert_eq!(hd["set_thread_desktop"].as_bool(), Some(true), "{hd}");
    assert_eq!(
        hd["window_on_hidden_desktop"].as_bool(),
        Some(true),
        "the acted window lives on the created desktop: {hd}"
    );
    assert_eq!(
        hd["absent_from_owner_enum_windows"].as_bool(),
        Some(true),
        "the acted window is never on the owner's desktop: {hd}"
    );
    assert_eq!(
        hd["window_class"].as_str(),
        Some(layout::WINDOW_CLASS),
        "the engine resolved the fixture's window, not another window on that \
         desktop (a console, say): {hd}"
    );
    assert!(
        hd["window_title"]
            .as_str()
            .is_some_and(|t| t.starts_with(layout::WINDOW_TITLE_PREFIX)),
        "{hd}"
    );
    let launched_pid = hd["launched_pid"].as_u64().expect("launched pid") as u32;
    let window_hwnd = hd["window_hwnd"].as_i64().expect("window hwnd");
    assert!(window_hwnd != 0, "{hd}");
    assert_eq!(
        hd["window_pid"].as_u64(),
        Some(launched_pid as u64),
        "the resolved window belongs to the process the engine launched: {hd}"
    );
    assert_eq!(
        hd["window_alive"].as_bool(),
        Some(true),
        "the window was alive when the evidence was read: {hd}"
    );

    let readback = hd["text_readback"].as_str().unwrap_or("");
    let read_lines: Vec<&str> = readback.lines().collect();
    assert!(
        read_lines.contains(&layout::TYPED_TEXT),
        "the EDIT holds exactly the typed text after the burst: {readback:?}"
    );
    assert!(
        read_lines.contains(&layout::STATUS_ON),
        "the fixture's state changed as a result of the click: {readback:?}"
    );
    assert_eq!(
        hd["pre_drive_text"].as_str(),
        Some(layout::STATUS_IDLE),
        "the readback was live before the acts: {hd}"
    );
    assert_eq!(
        hd["capture"]["ok"].as_bool(),
        Some(true),
        "a real frame of the private window was captured: {}",
        hd["capture"]
    );
    assert_eq!(
        hd["capture"]["w"].as_i64(),
        Some(layout::WIDTH as i64),
        "capture is the window's own size: {}",
        hd["capture"]
    );
    assert_eq!(
        hd["capture"]["h"].as_i64(),
        Some(layout::HEIGHT as i64),
        "{}",
        hd["capture"]
    );

    let lines = app.journal_lines();
    assert!(!lines.is_empty(), "the fixture recorded nothing: {lines:?}");
    let ready = find_line(&lines, layout::EV_READY).expect("the app recorded its start");
    assert_eq!(
        field_i64(&lines[ready], "pid").map(|p| p as u32),
        Some(launched_pid),
        "the app's own pid matches the engine's launched pid: {}",
        lines[ready]
    );
    assert_eq!(
        field_i64(&lines[ready], "hwnd"),
        Some(window_hwnd),
        "the app's own window handle matches the engine's resolved window: {}",
        lines[ready]
    );
    let typed_at = lines
        .iter()
        .position(|l| l == &format!("{}={}", layout::EV_EDIT, layout::TYPED_TEXT))
        .unwrap_or_else(|| panic!("the EDIT's change notification names the full text: {lines:?}"));
    let click_at = find_line(&lines, layout::EV_CLICK).expect("the app received the posted click");
    let (cx, cy) = layout::centre(layout::BUTTON);
    assert_eq!(
        lines[click_at],
        format!("{} x={cx} y={cy} inside=1", layout::EV_CLICK),
        "the posted click arrived at the button's own client centre, so the screen/client \
         mapping is exact and the point hit the intended control: {lines:?}"
    );
    assert!(
        layout::contains(layout::BUTTON, cx, cy),
        "the asserted point is inside the fixture's button rect"
    );
    let toggle_at =
        find_line(&lines, layout::EV_TOGGLE).expect("the BUTTON control's BN_CLICKED flipped it");
    assert_eq!(
        lines[toggle_at],
        format!("{}={}", layout::EV_TOGGLE, layout::TOGGLE_ON),
        "{}",
        lines[toggle_at]
    );
    assert!(
        typed_at < click_at && click_at < toggle_at,
        "the app observed the burst in order (type, click, toggle): {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.starts_with(layout::EV_CLICK) && !l.ends_with("inside=1")),
        "no posted click landed outside the intended control: {lines:?}"
    );

    let teardown = &hd["teardown"];
    assert_eq!(teardown["proven"].as_bool(), Some(true), "{teardown}");
    assert_eq!(
        teardown["process_exited"].as_bool(),
        Some(true),
        "{teardown}"
    );
    assert_eq!(
        teardown["terminated"].as_bool(),
        Some(false),
        "no force kill needed: {teardown}"
    );
    assert_eq!(
        teardown["desktop_empty"].as_bool(),
        Some(true),
        "{teardown}"
    );
    assert_eq!(
        teardown["open_after_close_null"].as_bool(),
        Some(true),
        "{teardown}"
    );
    assert_eq!(
        find_line(&lines, layout::EV_CLOSE),
        Some(lines.len() - 1),
        "the app recorded the engine's close as its last event: {lines:?}"
    );

    let stale = s.call(
        "desktop_do",
        json!({"target": {"hwnd": window_hwnd}, "actions": [{"type": "click", "x": 1, "y": 1}]}),
    );
    assert!(
        is_error(&stale),
        "a torn-down target must fail the whole call, not run against something else: {stale}"
    );
    let stale_text = text_of(&stale);
    assert!(
        stale_text.contains("target_gone"),
        "the refusal is machine-readable: {stale_text}"
    );

    let (samples, desk_hits, fg_hits) = watch.finish();
    assert!(
        samples >= 5,
        "the watcher must have sampled the owner's desktop during the call, got {samples}"
    );
    assert_eq!(
        desk_hits, 0,
        "no window of the fixture's class may ever appear on the owner's desktop ({samples} samples)"
    );
    assert_eq!(
        fg_hits, 0,
        "the foreground window never became one this scenario owns ({samples} samples)"
    );
    let fg_after = foreground_now();
    if fg_after != fg_before {
        let ours =
            pid_of(fg_after).is_some_and(|p| p == own_pid || p == server_pid || p == launched_pid);
        assert!(
            !ours,
            "hand must not activate any window it owns or targets: before={fg_before} after={fg_after}"
        );
    }
    let cursor_after = cursor_now();
    eprintln!(
        "HAND_FIXTURE_POINTER before={cursor_before:?} after={cursor_after:?} \
         delta=({}, {}) button_screen_centre=({click_x}, {click_y}) \
         (telemetry: no cursor API exists on this path)",
        cursor_after.0 - cursor_before.0,
        cursor_after.1 - cursor_before.1
    );
    eprintln!(
        "HAND_FIXTURE_OK samples={samples} fg_before={fg_before} fg_after={fg_after} \
         desktop={} window_hwnd={window_hwnd} launched_pid={launched_pid} typed={} readback={readback:?}",
        hd["desktop_name"].as_str().unwrap_or("?"),
        layout::TYPED_TEXT
    );
}

/// D4 + D3 LIVE: UIA text entry is legal on the PRIVATE hidden desktop - there
/// is no owner to steal focus from - and every act reports a readback `effect`.
/// The owner's desktop keeps the exact refusal (burst.rs unit tests). The
/// fixture's own journal is the independent oracle that both writes landed.
#[test]
fn hidden_desktop_uia_text_entry_reports_verified_effect() {
    let app = FixtureApp::build();
    let mut s = Session::spawn();
    let fg_before = foreground_now();
    let cursor_before = cursor_now();
    let journal = app.dir.join("journal-text.txt").to_string_lossy().into_owned();

    let response = s.call(
        "desktop_do",
        json!({
            "desktop": "hidden",
            "background": app.exe_arg(),
            "background_args": [journal],
            "actions": [
                {"type": "set_value", "element": "auto_id:100", "text": "alpha"},
                {"type": "type", "element": "auto_id:100", "text": "beta"},
            ],
        }),
    );
    assert!(
        !is_error(&response),
        "UIA text entry on the hidden desktop must be allowed: {}",
        text_of(&response)
    );
    let sc = structured(&response);
    assert_eq!(
        sc["sendinput_calls"],
        Value::from(0),
        "a hidden-desktop burst never injects: {sc}"
    );
    assert_eq!(
        sc["foregroundUnchanged"],
        Value::from(true),
        "the owner's foreground must not move: {sc}"
    );
    assert_eq!(
        sc["cursorUnchanged"],
        Value::from(true),
        "the owner's pointer must not move: {sc}"
    );
    let results = sc["results"].as_array().expect("results array");
    assert_eq!(results.len(), 2, "both actions report: {sc}");
    for (k, r) in results.iter().enumerate() {
        assert_eq!(r["ok"], Value::from(true), "action {k}: {r}");
        assert_eq!(r["delivered"], Value::from("uia"), "action {k}: {r}");
        assert_eq!(
            r["effect"],
            Value::from("verified"),
            "the readable state must have moved to the requested value: action {k}: {r}"
        );
    }

    let lines: Vec<String> = std::fs::read_to_string(&app.dir.join("journal-text.txt"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    assert!(
        lines.iter().any(|l| l == "edit=alpha"),
        "the app's own journal must record the set_value: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "edit=beta"),
        "the app's own journal must record the type: {lines:?}"
    );

    let fg_after = foreground_now();
    if fg_after != fg_before {
        let ours = pid_of(fg_after).is_some_and(|p| p == std::process::id() || p == s.child.id());
        assert!(!ours, "hand must not activate itself: {fg_before} -> {fg_after}");
    }
    let cursor_after = cursor_now();
    eprintln!(
        "HAND_HIDDEN_TEXT_OK effect=verified write1=alpha write2=beta \
         fg_unchanged={} cursor_unchanged={} sendinput=0 fg={fg_before}->{fg_after} cursor={cursor_before:?}->{cursor_after:?}",
        sc["foregroundUnchanged"], sc["cursorUnchanged"]
    );
}
