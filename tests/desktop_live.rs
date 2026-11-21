//! Read-only half: discover and inspect a real Windows window
//! through the desktop engine, without taking the foreground or moving the
//! physical pointer.
//!
//! The acting half (background UIA pattern / posted-message actuation with a
//! proven state change) is not part of this slice; it is reported as missing
//! rather than claimed.

mod common;

use common::{is_error, structured, text_of, Session};
use serde_json::json;

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut core::ffi::c_void;
    fn GetCursorPos(pt: *mut CursorPoint) -> i32;
    fn GetWindowThreadProcessId(hwnd: *mut core::ffi::c_void, pid: *mut u32) -> u32;
    fn IsChild(parent: *mut core::ffi::c_void, child: *mut core::ffi::c_void) -> i32;
    fn EnumWindows(
        cb: Option<unsafe extern "system" fn(*mut core::ffi::c_void, isize) -> i32>,
        l: isize,
    ) -> i32;
    fn GetClassNameW(hwnd: *mut core::ffi::c_void, buf: *mut u16, max: i32) -> i32;
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
    unsafe { GetWindowThreadProcessId(hwnd as _, &mut pid) };
    (pid != 0).then_some(pid)
}

fn is_child(parent: i64, child: i64) -> bool {
    parent != 0 && child != 0 && unsafe { IsChild(parent as _, child as _) } != 0
}

struct ClassPidScan {
    class: String,
    pid: u32,
    hits: Vec<i64>,
}

unsafe extern "system" fn class_pid_cb(hwnd: *mut core::ffi::c_void, l: isize) -> i32 {
    let scan = &mut *(l as *mut ClassPidScan);
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == scan.class {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == scan.pid {
            scan.hits.push(hwnd as i64);
        }
    }
    1
}

/// Resolve the fixture by IDENTITY: its window class plus the pid we launched.
/// `FindWindowW` answered with the FIRST class match in Z-order, which under
/// parallel runs is another test's fixture. That handle dies when the other
/// test tears down, and the cached hwnd then answers `target_gone`. Identity
/// is the same answer the product's target lock uses; this is not a retry.
fn hwnd_of_class_and_pid(class: &str, pid: u32) -> Option<i64> {
    let mut scan = ClassPidScan {
        class: class.to_string(),
        pid,
        hits: Vec::new(),
    };
    unsafe {
        EnumWindows(
            Some(class_pid_cb),
            &mut scan as *mut ClassPidScan as isize,
        );
    }
    scan.hits.first().copied()
}

/// These proofs are about an interactive user's real desktop: a bare session
/// (a hosted CI runner) has no foreground holder, so the test's own window
/// spawns legitimately move the foreground and the no-steal assertions stop
/// meaning what they mean on a working desktop (measured: the occlusion
/// proof failed on a runner because spawning moved the foreground). Run them
/// only when HAND_LIVE_DESKTOP=1 names a real interactive session; a skip
/// always prints its reason.
fn live_desktop() -> bool {
    let on = std::env::var("HAND_LIVE_DESKTOP").as_deref() == Ok("1");
    if !on {
        println!("SKIP live desktop proof: set HAND_LIVE_DESKTOP=1 (needs an interactive Windows session)");
    }
    on
}

#[test]
fn desktop_enumerates_and_inspects_a_background_window_without_stealing() {
    if !live_desktop() {
        return;
    }
    let mut s = Session::spawn();
    let own_pid = std::process::id();

    let fg_before = foreground_now();
    let cursor_before = cursor_now();

    let listed = s.call("desktop_list_windows", json!({}));
    assert!(
        !is_error(&listed),
        "list_windows failed: {}",
        text_of(&listed)
    );
    let sc = structured(&listed);
    let windows = sc["windows"].as_array().expect("windows array");
    assert!(
        !windows.is_empty(),
        "a Windows session has at least one window: {sc}"
    );
    for w in windows {
        assert!(w["hwnd"].as_i64().is_some(), "every row has hwnd: {w}");
        assert!(w["rect"].is_array(), "every row has a screen rect: {w}");
        assert!(
            w["client_rect"].is_array(),
            "every row has a client rect: {w}"
        );
        assert!(w["dpi"].as_i64().is_some(), "every row has dpi: {w}");
    }

    let candidate = windows
        .iter()
        .filter(|w| {
            w["pid"].as_u64().map(|p| p as u32) != Some(own_pid)
                && w["title"].as_str().is_some_and(|t| !t.trim().is_empty())
                && w["minimized"].as_bool() != Some(true)
                && w["cloaked"].as_i64() == Some(0)
        })
        .filter(|w| w["hwnd"].as_i64().is_some())
        .min_by_key(|w| w["foreground"].as_bool() == Some(true))
        .unwrap_or_else(|| {
            panic!("no visible titled background window to inspect; windows: {windows:?}")
        });
    let hwnd = candidate["hwnd"].as_i64().expect("hwnd");
    let target_pid = candidate["pid"].as_u64().map(|p| p as u32);

    let elements = s.call("desktop_elements", json!({"target": {"hwnd": hwnd}}));
    assert!(
        !is_error(&elements),
        "elements on a live window failed: {}",
        text_of(&elements)
    );
    let el = structured(&elements);
    assert!(
        el["backend"].is_string(),
        "elements names its backend: {el}"
    );
    assert_eq!(
        el["rect_space"].as_str(),
        Some("screen"),
        "element and window rectangles are screen pixels: {el}"
    );
    let els = el["elements"].as_array().expect("elements array");
    assert!(
        el["uia_available"].is_boolean(),
        "UIA availability is stated, never inferred: {el}"
    );
    if el["uia_available"].as_bool() == Some(true) {
        assert!(
            !els.is_empty() || el["empty"].as_bool() == Some(true),
            "semantic absence must be explicit: {el}"
        );
        if let Some(first) = els.first() {
            assert!(
                first["role"].is_string() || first["control_type"].is_string(),
                "{first}"
            );
            assert!(
                first["rect"].is_array(),
                "element geometry is present: {first}"
            );
        }
    } else {
        assert!(
            el["uia_error"].is_string() || el["uia_error"].is_object(),
            "an unavailable UIA tree names its error: {el}"
        );
    }
    eprintln!(
        "HAND_DESKTOP_INSPECT_OK hwnd={hwnd} title={:?} backend={} uia={} elements={}",
        candidate["title"].as_str().unwrap_or(""),
        el["backend"].as_str().unwrap_or("?"),
        el["uia_available"].as_bool().unwrap_or(false),
        els.len()
    );

    let missing = s.call(
        "desktop_elements",
        json!({"target": {"titleContains": "__hand_no_such_window_7f3a__"}}),
    );
    assert!(
        is_error(&missing),
        "a missing target must fail, not return an empty tree: {missing}"
    );
    let missing_text = text_of(&missing);
    assert!(
        missing_text.contains("target_gone") || missing_text.contains("bad_target"),
        "the refusal is machine-readable: {missing_text}"
    );

    let fg_after = foreground_now();
    if fg_after != fg_before {
        let attributed = Some(fg_after) == Some(hwnd)
            || pid_of(fg_after) == Some(own_pid)
            || target_pid.is_some() && pid_of(fg_after) == target_pid
            || is_child(hwnd, fg_after);
        assert!(
            !attributed,
            "hand must not activate its target or itself: before={fg_before} after={fg_after}"
        );
        eprintln!(
            "HAND_FG_THIRD_PARTY before={fg_before} after={fg_after} (not attributed to hand)"
        );
    }
    let cursor_after = cursor_now();
    if cursor_after != cursor_before {
        eprintln!(
            "HAND_CURSOR_TELEMETRY before={cursor_before:?} after={cursor_after:?} \
             (read-only calls; no cursor API on this path)"
        );
    }

    let cursor = s.call("desktop_cursor", json!({}));
    assert!(!is_error(&cursor), "cursor failed: {}", text_of(&cursor));
    let c = structured(&cursor);
    assert!(
        c["x"].as_i64().is_some() && c["y"].as_i64().is_some(),
        "{c}"
    );
    assert!(c["screens"].is_array(), "monitor rects are reported: {c}");
}

// ---- D5: live occluded capture ----

/// Compile the controlled fixture program with plain rustc, as the other
/// desktop suites do. GUI subsystem: it shows itself without activation.
fn build_fixture_app() -> std::path::PathBuf {
    use std::process::Command;
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("desktop_fixture")
        .join("fixture_app.rs");
    let tmp = std::env::temp_dir();
    let dir = tmp.join(format!(
        "hand-occluded-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir is creatable");
    let exe = dir.join("hand_fixture.exe");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = Command::new(&rustc)
        .args(["--edition", "2021", "-A", "warnings", "-o"])
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("rustc runs");
    assert!(
        out.status.success(),
        "fixture build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    exe
}

#[repr(C)]
struct CoverClass {
    style: u32,
    wnd_proc: Option<
        unsafe extern "system" fn(*mut core::ffi::c_void, u32, usize, isize) -> isize,
    >,
    cls_extra: i32,
    wnd_extra: i32,
    instance: *mut core::ffi::c_void,
    icon: *mut core::ffi::c_void,
    cursor: *mut core::ffi::c_void,
    background: *mut core::ffi::c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

unsafe extern "system" fn cover_proc(
    hwnd: *mut core::ffi::c_void,
    msg: u32,
    w: usize,
    l: isize,
) -> isize {
    DefWindowProcW(hwnd, msg, w, l)
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const CoverClass) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: *mut core::ffi::c_void,
        menu: *mut core::ffi::c_void,
        instance: *mut core::ffi::c_void,
        param: *const core::ffi::c_void,
    ) -> *mut core::ffi::c_void;
    fn ShowWindow(hwnd: *mut core::ffi::c_void, cmd: i32) -> i32;
    fn SetWindowPos(
        hwnd: *mut core::ffi::c_void,
        after: *mut core::ffi::c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn PostMessageW(hwnd: *mut core::ffi::c_void, msg: u32, w: usize, l: isize) -> i32;
    fn UpdateWindow(hwnd: *mut core::ffi::c_void) -> i32;
    fn WindowFromPoint(pt: CursorPoint) -> *mut core::ffi::c_void;
    fn DefWindowProcW(
        hwnd: *mut core::ffi::c_void,
        msg: u32,
        w: usize,
        l: isize,
    ) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> *mut core::ffi::c_void;
    fn DeleteObject(obj: *mut core::ffi::c_void) -> i32;
}

fn screenshot(hwnd: i64) -> serde_json::Value {
    desktop_engine::call_tool(
        json!(1),
        "1",
        json!({
            "name": "screenshot",
            "arguments": {
                "target": {"hwnd": hwnd},
                "fmt": "png",
                "max_px": null
            }
        }),
    )
    .expect("screenshot answered")
}

fn result(frame: &serde_json::Value) -> &serde_json::Value {
    &frame["result"]
}

fn image_data(frame: &serde_json::Value) -> String {
    frame["result"]["content"][0]["data"]
        .as_str()
        .expect("image data block")
        .to_string()
}

/// The controlled fixture app on the OWNER desktop, with its window handle.
/// Drop closes it; nothing here moves the pointer or the foreground.
struct LiveFixture {
    child: std::process::Child,
    dir: std::path::PathBuf,
    hwnd: i64,
}

impl LiveFixture {
    fn launch() -> Self {
        let exe = build_fixture_app();
        let dir = exe.parent().expect("fixture dir").to_path_buf();
        let journal = dir.join("journal.txt");
        let child = std::process::Command::new(&exe)
            .arg(&journal)
            .spawn()
            .expect("fixture app launches on the owner desktop");
        let mut found = std::ptr::null_mut();
        for _ in 0..100 {
            if let Some(h) = hwnd_of_class_and_pid("HandDesktopFixture", child.id()) {
                found = h as *mut core::ffi::c_void;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!found.is_null(), "fixture app window appeared for pid {}", child.id());
        Self {
            child,
            dir,
            hwnd: found as i64,
        }
    }
}

impl Drop for LiveFixture {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd as *mut core::ffi::c_void, 0x0010, 0, 0); // WM_CLOSE
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runtime red line: the capture result names the method that produced the
/// pixels. On the pre-fix binary `captureMethod` is absent.
#[test]
fn screenshot_reports_capture_method() {
    if !live_desktop() {
        return;
    }
    let f = LiveFixture::launch();
    let frame = screenshot(f.hwnd);
    assert_eq!(
        result(&frame)["isError"], json!(false),
        "fixture capture failed: {}",
        text_of(&frame)
    );
    let sc = &result(&frame)["structuredContent"];
    assert_eq!(
        sc["captureMethod"].as_str(),
        Some("printwindow"),
        "a window capture must name PrintWindow as its method: {sc}"
    );
}

/// D5 live proof. The controlled fixture app is covered by an opaque window
/// of ours; PrintWindow still returns the fixture's OWN pixels (the baseline
/// and occluded images are byte-identical), and the no-steal evidence reports
/// foregroundUnchanged. A BitBlt/DXGI screen crop would return the cover.
#[test]
fn occluded_live_window_captures_with_foreground_unchanged() {
    if !live_desktop() {
        return;
    }
    let f = LiveFixture::launch();
    let fg_before = foreground_now();
    let fixture = f.hwnd as *mut core::ffi::c_void;

    // HWND_TOP + SWP_NOACTIVATE: place the fixture on top without stealing.
    const SWP_NOACTIVATE: u32 = 0x0010;
    unsafe {
        SetWindowPos(fixture, std::ptr::null_mut(), 200, 200, 420, 320, SWP_NOACTIVATE);
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let hwnd = f.hwnd;
    let baseline = screenshot(hwnd);
    assert_eq!(result(&baseline)["isError"], json!(false), "baseline capture: {}", text_of(&baseline));
    let baseline_img = image_data(&baseline);

    // An opaque cover, larger than the fixture, on top with no activation.
    let cover_class = "HandOcclusionCover\0".encode_utf16().collect::<Vec<_>>();
    let brush = unsafe { CreateSolidBrush(0x0028_28_28) };
    let mut wc: CoverClass = unsafe { std::mem::zeroed() };
    wc.wnd_proc = Some(cover_proc);
    wc.background = brush;
    wc.class_name = cover_class.as_ptr();
    assert_ne!(unsafe { RegisterClassW(&wc) }, 0, "cover class registered");
    let cover = unsafe {
        CreateWindowExW(
            0,
            cover_class.as_ptr(),
            std::ptr::null(),
            0x8000_0000 | 0x1000_0000, // WS_POPUP | WS_VISIBLE
            195,
            195,
            440,
            340,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!cover.is_null(), "cover window created");
    unsafe {
        ShowWindow(cover, 4); // SW_SHOWNOACTIVATE
        // HWND_TOPMOST (-1) and a synchronous paint: the test thread has no
        // message pump, so UpdateWindow is what actually puts pixels up.
        SetWindowPos(
            cover,
            (-1isize) as *mut core::ffi::c_void,
            195,
            195,
            440,
            340,
            SWP_NOACTIVATE,
        );
        UpdateWindow(cover);
    }
    std::thread::sleep(std::time::Duration::from_millis(250));
    let at_centre = unsafe {
        WindowFromPoint(CursorPoint {
            x: 400,
            y: 360,
        })
    };
    assert_ne!(
        at_centre, fixture,
        "the cover must really be on top of the fixture at the sample point"
    );

    let occluded = screenshot(hwnd);
    assert_eq!(result(&occluded)["isError"], json!(false), "occluded capture: {}", text_of(&occluded));
    let sc = &result(&occluded)["structuredContent"];
    assert_eq!(
        sc["captureMethod"].as_str(),
        Some("printwindow"),
        "an occluded window must still be captured through PrintWindow: {sc}"
    );
    assert_eq!(
        sc["foregroundUnchanged"].as_bool(),
        Some(true),
        "capture must not change the foreground: {sc}"
    );
    assert_eq!(
        image_data(&occluded),
        baseline_img,
        "the occluded capture must return the fixture's own pixels, byte for byte"
    );
    assert_eq!(
        sc["capture"]["occluded"].as_bool(),
        Some(true),
        "the visibility-region signal must report the fixture as covered: {sc}"
    );

    unsafe {
        DeleteObject(brush);
    }
    assert_eq!(
        foreground_now(),
        fg_before,
        "the test itself must not move the foreground"
    );
}
