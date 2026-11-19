//! Slice 3 (read-only half): discover and inspect a real Windows window
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

#[test]
fn desktop_enumerates_and_inspects_a_background_window_without_stealing() {
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

    // never an empty success and never a foreground fallback.
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
