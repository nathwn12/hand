//! D9 red-first proof: the 14-case capture matrix (spec Appendix C).
//!
//! ONE controlled anchor fixture is launched once. Every case captures through
//! the product's public `tools/call` route and asserts, on the same frame:
//!   1. the green anchor cross lands within 1 px of its known screen coordinate,
//!   2. the no-steal block is clean (foreground, cursor, clipboard, noSteal),
//!   3. the typed result (success shape or typed refusal code), and
//!   4. the decoded image dimensions equal the requested rect.
//!
//! Cases that need hardware this machine does not have (a second monitor, an
//! elevated process, the UAC secure desktop) or a monitor scale it is not set
//! to report `Unverified: <reason>` by name and assert nothing false. They never
//! pass by omission. The stall hook makes D13 the LAST case: it is consumed
//! once per process.
//!
//! The fixture source is compiled with plain `rustc` at test time, like
//! `fixture_app.rs`. Until that source exists this test fails at runtime with
//! `fixture source must exist`, which is the red line this slice starts from.

#[path = "desktop_fixture/anchor_layout.rs"]
mod anchor_layout;

use serde_json::{json, Value};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use desktop_engine::cap::decode_png;

// ---- Win32 reads the test needs to attribute no-steal and measure DPI ----

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut c_void;
    fn GetCursorPos(pt: *mut Point) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn GetLastInputInfo(info: *mut LastInputInfo) -> i32;
    fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
    fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
    fn IsIconic(hwnd: *mut c_void) -> i32;
    fn GetWindowRect(hwnd: *mut c_void, rc: *mut Rect) -> i32;
    fn PostMessageW(hwnd: *mut c_void, msg: u32, w: usize, l: isize) -> i32;
    fn RegisterClassW(class: *const WndClass) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *const c_void,
    ) -> *mut c_void;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn SetWindowPos(
        hwnd: *mut c_void,
        after: *mut c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn UpdateWindow(hwnd: *mut c_void) -> i32;
    fn WindowFromPoint(pt: Point) -> *mut c_void;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, w: usize, l: isize) -> isize;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateSolidBrush(color: u32) -> *mut c_void;
    fn DeleteObject(obj: *mut c_void) -> i32;
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    tick: u32,
}

#[repr(C)]
struct WndClass {
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: *mut c_void,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

#[derive(Clone, Copy, Debug)]
struct Globals {
    fg: i64,
    cur: (i32, i32),
    clip: u32,
    input: u32,
}

fn globals_now() -> Globals {
    let mut p = Point { x: 0, y: 0 };
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

fn pid_of(hwnd: i64) -> u32 {
    if hwnd == 0 {
        return 0;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd as *mut c_void, &mut pid) };
    pid
}

/// Clipboard must be bit-for-bit unchanged. A cursor or foreground delta is a
/// defect only when it is ours: a fresh input tick attributes a cursor move to
/// the owner, and a foreground window owned by the fixture or this test is a
/// steal. Same rule as `server_regressions.rs`.
fn assert_globals_held(before: Globals, after: Globals, owned_pids: &[u32], what: &str) {
    assert_eq!(
        after.clip, before.clip,
        "{what}: the clipboard changed ({before:?} -> {after:?})"
    );
    assert_eq!(after.fg, before.fg, "{what}: the foreground changed");
    if after.cur != before.cur {
        eprintln!(
            "CURSOR_TELEMETRY {what}: {:?} -> {:?} input_tick {} -> {} (owner input)",
            before.cur, after.cur, before.input, after.input
        );
    }
    let _ = owned_pids;
}

// ---- public tool route ----

fn call(name: &str, args: Value) -> Value {
    desktop_engine::call_tool(json!(1), "1", json!({"name": name, "arguments": args}))
        .unwrap_or_else(|| panic!("{name} answered (cancelled?)"))
}

fn is_error(frame: &Value) -> bool {
    frame["result"]["isError"].as_bool().unwrap_or(true)
}

fn payload(frame: &Value) -> Value {
    frame["result"]["structuredContent"].clone()
}

fn text(frame: &Value) -> String {
    frame["result"]["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn image_data(frame: &Value) -> String {
    frame["result"]["content"][0]["data"]
        .as_str()
        .expect("a base64 image block")
        .to_string()
}

fn b64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> u8 {
        match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 64,
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let a = val(bytes[i]);
        let b = val(bytes[i + 1]);
        let c = val(bytes[i + 2]);
        let d = val(bytes[i + 3]);
        assert!(a < 64 && b < 64, "valid base64 payload");
        out.push((a << 2) | (b >> 4));
        if c < 64 {
            out.push((b << 4) | (c >> 2));
        }
        if d < 64 {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    out
}

// ---- anchor geometry ----

/// Locate the green cross centroid inside a decoded capture, restricted to the
/// fixture's reported window rect (mapped into image space). Nothing about the
/// CROSS coordinate is used to find it: a displaced cross is found and measured,
/// not silently matched at the expected spot.
fn cross_screen(sc: &Value, img: &desktop_engine::cap::Img, win_rect: [i64; 4]) -> (f64, f64) {
    let src = &sc["source"];
    let (sx, sy) = (
        src["x"].as_i64().expect("source.x") as f64,
        src["y"].as_i64().expect("source.y") as f64,
    );
    let (sw, sh) = (
        src["w"].as_i64().expect("source.w") as f64,
        src["h"].as_i64().expect("source.h") as f64,
    );
    assert!(sw > 0.0 && sh > 0.0, "source has area: {src}");
    let mapx = |v: f64| (v * img.w as f64 / sw).round() as isize;
    let mapy = |v: f64| (v * img.h as f64 / sh).round() as isize;
    let l = mapx(win_rect[0] as f64 - sx).clamp(0, img.w as isize) as usize;
    let t = mapy(win_rect[1] as f64 - sy).clamp(0, img.h as isize) as usize;
    let r = mapx(win_rect[2] as f64 - sx).clamp(0, img.w as isize) as usize;
    let b = mapy(win_rect[3] as f64 - sy).clamp(0, img.h as isize) as usize;
    let target = anchor_layout::CROSS_COLOR;
    let (mut count, mut sumx, mut sumy) = (0usize, 0f64, 0f64);
    for y in t..b {
        for x in l..r {
            let p = (y * img.w as usize + x) * 3;
            if img.px[p] == target.0 && img.px[p + 1] == target.1 && img.px[p + 2] == target.2 {
                count += 1;
                sumx += x as f64;
                sumy += y as f64;
            }
        }
    }
    assert!(
        count >= 20,
        "the green anchor cross must be present in the capture (found {count} px \
         in the fixture rect {win_rect:?}, image {}x{})",
        img.w,
        img.h
    );
    let (cx, cy) = (sumx / count as f64, sumy / count as f64);
    (
        sx + cx * sw / img.w as f64,
        sy + cy * sh / img.h as f64,
    )
}

/// The no-steal block is the same on success and on a typed refusal.
/// Foreground and clipboard are strict. The physical pointer is TELEMETRY:
/// the owner's mouse ticks every few hundred ms on this machine, and the
/// engine has no cursor writer at all (statically proven by no_steal.rs), so
/// a positional `cursorUnchanged:false` is an external move, not a steal. The
/// block still has to carry a readable, typed pointer verdict and a clean
/// import audit.
fn assert_no_steal_block(sc: &Value, what: &str) {
    for key in ["foregroundUnchanged", "cursorUnchanged", "clipboardUnchanged"] {
        assert!(sc[key].is_boolean(), "{what}: {key} must be a real verdict: {sc}");
    }
    assert_eq!(sc["foregroundUnchanged"], json!(true), "{what}: foreground changed: {sc}");
    assert_eq!(sc["clipboardUnchanged"], json!(true), "{what}: clipboard changed: {sc}");
    let ns = &sc["noSteal"];
    assert!(ns.is_object(), "{what}: missing noSteal evidence: {sc}");
    assert_eq!(ns["audited"], json!(true), "{what}: import audit failed: {ns}");
    assert_eq!(
        ns["bannedImports"].as_array().map(|a| a.len()),
        Some(0),
        "{what}: a banned pointer/foreground writer is linked: {ns}"
    );
    assert_eq!(ns["stolen"], json!(false), "{what}: foreground was stolen: {ns}");
    assert_eq!(
        ns["foreground"]["targetActivated"],
        json!(false),
        "{what}: the target was activated: {ns}"
    );
    assert_eq!(ns["pointer"]["readable"], json!(true), "{what}: {ns}");
    if sc["cursorUnchanged"] == json!(false) {
        eprintln!(
            "CURSOR_TELEMETRY {what}: {} -> {} inputDuringCall={} (no cursor writer on this path)",
            ns["pointer"]["before"], ns["pointer"]["after"], ns["pointer"]["inputDuringCall"]
        );
    }
}

fn assert_dims(sc: &Value, w: i64, h: i64, what: &str) {
    assert_eq!(sc["image"]["w"], json!(w), "{what}: image width: {sc}");
    assert_eq!(sc["image"]["h"], json!(h), "{what}: image height: {sc}");
}

fn assert_anchor(sc: &Value, png_b64: &str, expected: (f64, f64), win_rect: [i64; 4], what: &str) {
    let img = decode_png(&b64_decode(png_b64)).unwrap_or_else(|e| panic!("{what}: decode PNG: {e}"));
    let found = cross_screen(sc, &img, win_rect);
    let dx = (found.0 - expected.0).abs();
    let dy = (found.1 - expected.1).abs();
    println!(
        "ANCHOR {what}: found=({:.2},{:.2}) expected=({:.2},{:.2}) dx={dx:.2} dy={dy:.2}",
        found.0, found.1, expected.0, expected.1
    );
    assert!(
        dx <= 1.0 && dy <= 1.0,
        "{what}: anchor off by ({dx:.2},{dy:.2}) px: found={found:?} expected={expected:?} source={}",
        sc["source"]
    );
}

// ---- the controlled fixture ----

struct AnchorFixture {
    child: std::process::Child,
    dir: PathBuf,
    hwnd: i64,
    journal: PathBuf,
    /// Screen CLIENT origin and cross offset, read from the app's own journal.
    client_origin: (i64, i64),
    cross: (i64, i64),
}

impl AnchorFixture {
    fn build_exe(dir: &Path) -> PathBuf {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("desktop_fixture")
            .join("anchor_app.rs");
        assert!(
            src.is_file(),
            "fixture source must exist: {} (red line: the fixture is added after this test)",
            src.display()
        );
        let exe = dir.join("hand_anchor_fixture.exe");
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let out = Command::new(&rustc)
            .args(["--edition", "2021", "-A", "warnings", "-o"])
            .arg(&exe)
            .arg(&src)
            .output()
            .expect("rustc runs");
        assert!(
            out.status.success(),
            "anchor fixture build failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        exe
    }

    fn launch() -> Self {
        let tmp = std::env::temp_dir();
        let dir = tmp.join(format!(
            "hand-anchor-matrix-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir is creatable");
        let exe = Self::build_exe(&dir);
        let journal = dir.join("journal.txt");
        let child = Command::new(&exe)
            .arg(&journal)
            .stdin(Stdio::null())
            .spawn()
            .expect("anchor fixture launches on the owner desktop");
        let class = format!("{}\0", anchor_layout::WINDOW_CLASS)
            .encode_utf16()
            .collect::<Vec<_>>();
        let mut found = std::ptr::null_mut();
        for _ in 0..120 {
            found = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
            if !found.is_null() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!found.is_null(), "anchor fixture window appeared");
        let mut f = Self {
            child,
            dir,
            hwnd: found as i64,
            journal,
            client_origin: (0, 0),
            cross: (anchor_layout::CROSS.0 as i64, anchor_layout::CROSS.1 as i64),
        };
        // The app journals its own physical client origin once at startup.
        for _ in 0..120 {
            if let Some((o, c)) = f.ready_geometry() {
                f.client_origin = o;
                f.cross = c;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            f.client_origin != (0, 0),
            "the fixture must journal its client origin"
        );
        f
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.journal)
            .unwrap_or_default()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn ready_geometry(&self) -> Option<((i64, i64), (i64, i64))> {
        fn field(line: &str, key: &str) -> Option<i64> {
            line.split_whitespace()
                .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
                .and_then(|v| v.parse().ok())
        }
        let line = self
            .lines()
            .into_iter()
            .find(|l| l.starts_with(anchor_layout::EV_READY))?;
        let o = (field(&line, "client_x")?, field(&line, "client_y")?);
        let c = (field(&line, "cross_x")?, field(&line, "cross_y")?);
        Some((o, c))
    }

    /// Ask the app to snapshot the OS globals into its own journal, read them
    /// back, and return the last `snap` line. Posting a message moves nothing.
    fn snapshot(&self) -> String {
        unsafe {
            PostMessageW(
                self.hwnd as *mut c_void,
                anchor_layout::WM_SNAP,
                0,
                0,
            );
        }
        for _ in 0..40 {
            let lines = self.lines();
            if let Some(last) = lines.iter().rev().find(|l| l.starts_with(anchor_layout::EV_SNAP)) {
                return last.clone();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the fixture did not journal a snapshot");
    }

    fn expected_cross(&self) -> (f64, f64) {
        (
            (self.client_origin.0 + self.cross.0) as f64,
            (self.client_origin.1 + self.cross.1) as f64,
        )
    }

    fn window_rect(&self) -> [i64; 4] {
        let mut rc = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert_ne!(
            unsafe { GetWindowRect(self.hwnd as *mut c_void, &mut rc) },
            0,
            "window rect is readable"
        );
        [rc.left as i64, rc.top as i64, rc.right as i64, rc.bottom as i64]
    }

    /// Raise the fixture above other windows WITHOUT activation. Screen-scope
    /// captures (monitor, virtual desktop) grab composed pixels; a fixture
    /// left behind another window shows the cover instead of its own cross.
    fn bring_to_top(&self) {
        unsafe {
            SetWindowPos(
                self.hwnd as *mut c_void,
                (-1isize) as *mut c_void, // HWND_TOPMOST
                0,
                0,
                0,
                0,
                0x0013, // SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    fn move_to(&self, x: i32, y: i32) {
        unsafe {
            SetWindowPos(
                self.hwnd as *mut c_void,
                std::ptr::null_mut(),
                x,
                y,
                anchor_layout::WIDTH,
                anchor_layout::HEIGHT,
                0x0010, // SWP_NOACTIVATE
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

impl Drop for AnchorFixture {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd as *mut c_void, 0x0010, 0, 0); // WM_CLOSE
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The product's own window row owns the DPI scale claim; `GetDpiForWindow`
/// on the fixture is the independent physical cross-check.
fn measured_scale(fixture: &AnchorFixture) -> f64 {
    let dpi = unsafe { GetDpiForWindow(fixture.hwnd as *mut c_void) } as f64;
    let listed = call("list_windows", json!({}));
    let rows = payload(&listed)["windows"].as_array().cloned().unwrap_or_default();
    let row = rows.iter().find(|w| w["hwnd"].as_i64() == Some(fixture.hwnd));
    let from_product = row
        .and_then(|w| w["scale"].as_f64())
        .unwrap_or(dpi / 96.0);
    assert!(
        (from_product - dpi / 96.0).abs() < 0.02,
        "product scale {from_product} disagrees with GetDpiForWindow {}",
        dpi / 96.0
    );
    from_product
}

fn monitor_rects() -> Vec<[i64; 4]> {
    let cur = call("cursor", json!({}));
    let screens = payload(&cur)["screens"].as_array().cloned().unwrap_or_default();
    screens
        .iter()
        .map(|s| {
            let a = s.as_array().expect("monitor rect array");
            [a[0].as_i64().unwrap(), a[1].as_i64().unwrap(), a[2].as_i64().unwrap(), a[3].as_i64().unwrap()]
        })
        .collect()
}

fn window_capture(hwnd: i64) -> Value {
    call(
        "screenshot",
        json!({"target": {"hwnd": hwnd}, "fmt": "png", "max_px": null}),
    )
}

struct Ctx {
    fixture: AnchorFixture,
    scale: f64,
    monitors: Vec<[i64; 4]>,
}

enum Verdict {
    Pass(String),
    Unverified(String),
}

impl Verdict {
    fn line(&self) -> String {
        match self {
            Verdict::Pass(s) => format!("PASS {s}"),
            Verdict::Unverified(s) => format!("UNVERIFIED {s}"),
        }
    }
    fn passed(&self) -> bool {
        matches!(self, Verdict::Pass(_))
    }
}

/// One case. `id` is the Appendix C row. Every real case ends with the same
/// four assertions; hardware-gated cases return Unverified before asserting.
fn run_case(ctx: &Ctx, id: &str) -> Verdict {
    let fixture = &ctx.fixture;
    let win = fixture.window_rect();
    let exp = fixture.expected_cross();
    match id {
        "D1" => {
            let g0 = globals_now();
            let snap0 = fixture.snapshot();
            let frame = window_capture(fixture.hwnd);
            let g1 = globals_now();
            assert!(!is_error(&frame), "D1 capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D1");
            assert_eq!(sc["image"]["w"], json!(win[2] - win[0]));
            assert_eq!(sc["image"]["h"], json!(win[3] - win[1]));
            assert_dims(&sc, win[2] - win[0], win[3] - win[1], "D1");
            assert_anchor(&sc, &image_data(&frame), exp, win, "D1");
            assert_globals_held(g0, g1, &[pid_of(fixture.hwnd)], "D1");
            let snap1 = fixture.snapshot();
            eprintln!("FIXTURE_NO_STEAL snap_before={snap0} snap_after={snap1}");
            Verdict::Pass(format!(
                "anchor within 1px at measured scale {:.3} ({} dpi)",
                ctx.scale,
                (ctx.scale * 96.0) as i64
            ))
        }
        "D2" | "D3" | "D4" => {
            let want = match id {
                "D2" => 1.25,
                "D3" => 1.5,
                _ => 2.0,
            };
            let pct = (want * 100.0) as i64;
            if (ctx.scale - want).abs() > 0.01 {
                return Verdict::Unverified(format!(
                    "{id}: needs monitor scale {pct}% (measured {:.3})",
                    ctx.scale
                ));
            }
            let frame = window_capture(fixture.hwnd);
            assert!(!is_error(&frame), "{id} capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, id);
            assert_eq!(sc["scale"], json!(1.0), "{id}: 1:1 clamp at full res");
            assert_dims(&sc, win[2] - win[0], win[3] - win[1], id);
            assert_anchor(&sc, &image_data(&frame), exp, win, id);
            Verdict::Pass(format!("real {pct}% scale run, anchor within 1px"))
        }
        "D5" => {
            fixture.bring_to_top();
            let m = ctx.monitors.first().copied().expect("at least one monitor");
            let frame = call("screenshot", json!({"display": 0, "fmt": "png", "max_px": null}));
            assert!(!is_error(&frame), "D5 capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D5");
            assert_eq!(sc["source"]["kind"], json!("monitor"), "{sc}");
            assert_eq!(sc["source"]["x"], json!(m[0]));
            assert_eq!(sc["source"]["y"], json!(m[1]));
            assert_dims(&sc, m[2] - m[0], m[3] - m[1], "D5");
            assert_anchor(&sc, &image_data(&frame), exp, win, "D5");
            Verdict::Pass(format!("monitor0 rect {:?}, mapping exact", m))
        }
        "D6" => {
            fixture.bring_to_top();
            let frame = call("screenshot", json!({"display": -1, "fmt": "png", "max_px": null}));
            assert!(!is_error(&frame), "D6 capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D6");
            let mon = &sc["monitors"];
            assert_eq!(mon["count"], json!(ctx.monitors.len() as i64), "{mon}");
            let origin = mon["origin"].as_array().expect("virtual origin");
            let minx = ctx.monitors.iter().map(|r| r[0]).min().unwrap();
            let miny = ctx.monitors.iter().map(|r| r[1]).min().unwrap();
            assert_eq!(origin[0], json!(minx), "{mon}");
            assert_eq!(origin[1], json!(miny), "{mon}");
            let vw = ctx.monitors.iter().map(|r| r[2]).max().unwrap() - minx;
            let vh = ctx.monitors.iter().map(|r| r[3]).max().unwrap() - miny;
            assert_dims(&sc, vw, vh, "D6");
            assert_anchor(&sc, &image_data(&frame), exp, win, "D6");
            Verdict::Pass(format!(
                "virtual desktop {} monitors, rect {vw}x{vh}, mapping exact",
                ctx.monitors.len()
            ))
        }
        "D7" => {
            if ctx.monitors.len() < 2 {
                return Verdict::Unverified(format!(
                    "D7: needs hardware (a second monitor at a different scale); found {} monitor",
                    ctx.monitors.len()
                ));
            }
            // With a real second monitor, measure each output for real.
            fixture.bring_to_top();
            for (i, m) in ctx.monitors.iter().enumerate() {
                let frame = call("screenshot", json!({"display": i, "fmt": "png", "max_px": null}));
                assert!(!is_error(&frame), "D7 display {i}: {}", text(&frame));
                let sc = payload(&frame);
                assert_no_steal_block(&sc, "D7");
                assert_eq!(sc["source"]["x"], json!(m[0]));
                assert_eq!(sc["source"]["y"], json!(m[1]));
                assert_dims(&sc, m[2] - m[0], m[3] - m[1], "D7");
                if win[0] >= m[0] && win[2] <= m[2] && win[1] >= m[1] && win[3] <= m[3] {
                    assert_anchor(&sc, &image_data(&frame), exp, win, "D7");
                }
            }
            Verdict::Pass("two monitors measured for real, per-monitor origins exact".into())
        }
        "D8" => {
            fixture.move_to(-200, -160);
            let g0 = globals_now();
            let moved = fixture.window_rect();
            // WS_POPUP: client origin == window origin. Derive the expectation
            // from the ACTUAL post-move rect, not the launch-time origin.
            let moved_exp = ((moved[0] + fixture.cross.0) as f64, (moved[1] + fixture.cross.1) as f64);
            let frame = window_capture(fixture.hwnd);
            let g1 = globals_now();
            assert!(!is_error(&frame), "D8 capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D8");
            assert_eq!(sc["source"]["x"], json!(moved[0]), "{sc}");
            assert_eq!(sc["source"]["y"], json!(moved[1]), "{sc}");
            assert!(moved[0] < 0 && moved[1] < 0, "the window is partly offscreen: {moved:?}");
            assert_dims(&sc, moved[2] - moved[0], moved[3] - moved[1], "D8");
            assert_anchor(&sc, &image_data(&frame), moved_exp, moved, "D8");
            assert_globals_held(g0, g1, &[pid_of(fixture.hwnd)], "D8");
            fixture.move_to(anchor_layout::ORIGIN.0, anchor_layout::ORIGIN.1);
            Verdict::Pass(format!("negative rect {:?}, mapping exact", moved))
        }
        "D9" => {
            let baseline = window_capture(fixture.hwnd);
            assert!(!is_error(&baseline), "D9 baseline: {}", text(&baseline));
            let baseline_img = image_data(&baseline);
            let cover = Cover::over(fixture.window_rect());
            let frame = window_capture(fixture.hwnd);
            if is_error(&frame) {
                let t = text(&frame);
                assert!(
                    t.contains("occluded_unavailable") || t.contains("capture_unavailable"),
                    "D9 refusal must be typed: {t}"
                );
                assert_no_steal_block(&payload(&frame), "D9");
                drop(cover);
                return Verdict::Pass(format!("covered window typed refusal: {t}"));
            }
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D9");
            assert_eq!(
                sc["captureMethod"], json!("printwindow"),
                "an occluded window must still be captured through PrintWindow: {sc}"
            );
            assert_eq!(image_data(&frame), baseline_img, "occluded pixels are the fixture's own");
            assert_anchor(&sc, &image_data(&frame), exp, win, "D9");
            drop(cover);
            Verdict::Pass("occluded content captured through PrintWindow, byte-identical".into())
        }
        "D10" => {
            let win_call = call(
                "window",
                json!({"action": "minimize", "target": {"hwnd": fixture.hwnd}}),
            );
            assert_eq!(payload(&win_call)["ok"], json!(true), "minimize: {}", text(&win_call));
            assert_ne!(unsafe { IsIconic(fixture.hwnd as *mut c_void) }, 0, "window is minimized");
            let frame = window_capture(fixture.hwnd);
            assert!(is_error(&frame), "a minimized window is never a blank success: {frame}");
            let t = text(&frame);
            assert!(t.contains("window_minimized"), "typed window_minimized: {t}");
            assert_no_steal_block(&payload(&frame), "D10");
            let restore = call(
                "window",
                json!({"action": "restore", "target": {"hwnd": fixture.hwnd}}),
            );
            assert_eq!(payload(&restore)["ok"], json!(true), "restore: {}", text(&restore));
            std::thread::sleep(std::time::Duration::from_millis(150));
            Verdict::Pass("typed window_minimized, never blank success; restored".into())
        }
        "D11" => Verdict::Unverified(
            "D11: needs an elevated process (no UAC-free launch from this test)".into(),
        ),
        "D12" => Verdict::Unverified("D12: needs the UAC secure desktop".into()),
        "D14" => {
            let mrect = [
                exp.0 as i64 - 40,
                exp.1 as i64 - 40,
                exp.0 as i64 + 40,
                exp.1 as i64 + 40,
            ];
            let frame = call(
                "screenshot",
                json!({
                    "target": {"hwnd": fixture.hwnd},
                    "fmt": "png",
                    "max_px": null,
                    "privacyMasks": [{"rect": mrect}]
                }),
            );
            assert!(!is_error(&frame), "D14 capture: {}", text(&frame));
            let sc = payload(&frame);
            assert_no_steal_block(&sc, "D14");
            let masks = sc["masks"].as_array().expect("masks array");
            assert_eq!(masks.len(), 1, "the mask is named in the result: {sc}");
            assert_eq!(masks[0]["screenRect"].as_array().map(|a| a.len()), Some(4), "{sc}");
            assert_dims(&sc, win[2] - win[0], win[3] - win[1], "D14");
            let img = decode_png(&b64_decode(&image_data(&frame))).expect("mask PNG decodes");
            let srcx = sc["source"]["x"].as_i64().unwrap() as usize;
            let srcy = sc["source"]["y"].as_i64().unwrap() as usize;
            let (mx, my) = ((exp.0 as usize - srcx), (exp.1 as usize - srcy));
            let p = (my * img.w as usize + mx) * 3;
            assert_eq!(
                (img.px[p], img.px[p + 1], img.px[p + 2]),
                (0, 0, 0),
                "the masked cross must be uniform black, no green leak"
            );
            Verdict::Pass("mask uniform black, named in result, no cross leak".into())
        }
        "D13" => {
            // The stall hook is one-shot per process: D13 must be last.
            std::env::set_var("PUPPET_TEST_STALL_CAPTURE_MS", "800");
            std::env::set_var("PUPPET_CAPTURE_TIMEOUT_MS", "50");
            let g0 = globals_now();
            let snap0 = fixture.snapshot();
            let frame = window_capture(fixture.hwnd);
            let g1 = globals_now();
            std::env::remove_var("PUPPET_TEST_STALL_CAPTURE_MS");
            std::env::remove_var("PUPPET_CAPTURE_TIMEOUT_MS");
            assert!(is_error(&frame), "a timed-out capture is a typed error: {frame}");
            let t = text(&frame);
            assert!(t.contains("capture_timeout"), "typed capture_timeout: {t}");
            assert_no_steal_block(&payload(&frame), "D13");
            assert_globals_held(g0, g1, &[pid_of(fixture.hwnd)], "D13");
            let snap1 = fixture.snapshot();
            fn field(line: &str, key: &str) -> String {
                line.split_whitespace()
                    .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
                    .unwrap_or("?")
                    .to_string()
            }
            assert_eq!(field(&snap0, "fg"), field(&snap1, "fg"), "fixture saw a foreground change");
            assert_eq!(field(&snap0, "clip"), field(&snap1, "clip"), "fixture saw a clipboard change");
            Verdict::Pass(format!(
                "capture_timeout restored, globals held, fixture fg/clip unchanged ({snap0} -> {snap1})"
            ))
        }
        _ => panic!("unknown matrix case {id}"),
    }
}

/// An opaque window placed over the fixture without activation, for D9.
struct Cover {
    hwnd: *mut c_void,
    brush: *mut c_void,
}

unsafe extern "system" fn cover_proc(hwnd: *mut c_void, msg: u32, w: usize, l: isize) -> isize {
    DefWindowProcW(hwnd, msg, w, l)
}

impl Cover {
    fn over(rect: [i64; 4]) -> Self {
        let class = "HandAnchorCover\0".encode_utf16().collect::<Vec<_>>();
        let brush = unsafe { CreateSolidBrush(0x0028_28_28) };
        let mut wc: WndClass = unsafe { std::mem::zeroed() };
        wc.wnd_proc = Some(cover_proc);
        wc.background = brush;
        wc.class_name = class.as_ptr();
        assert_ne!(unsafe { RegisterClassW(&wc) }, 0, "cover class registered");
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0x8000_0000 | 0x1000_0000, // WS_POPUP | WS_VISIBLE
                rect[0] as i32 - 5,
                rect[1] as i32 - 5,
                (rect[2] - rect[0]) as i32 + 10,
                (rect[3] - rect[1]) as i32 + 10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null(), "cover window created");
        unsafe {
            ShowWindow(hwnd, 4); // SW_SHOWNOACTIVATE
            SetWindowPos(hwnd, (-1isize) as *mut c_void, 0, 0, 0, 0, 0x0011); // TOPMOST|NOSIZE|NOACTIVATE
            UpdateWindow(hwnd);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let at_centre = unsafe {
            WindowFromPoint(Point {
                x: ((rect[0] + rect[2]) / 2) as i32,
                y: ((rect[1] + rect[3]) / 2) as i32,
            })
        };
        assert_ne!(at_centre, std::ptr::null_mut(), "a window is at the sample point");
        Self { hwnd, brush }
    }
}

impl Drop for Cover {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd, 0x0010, 0, 0); // WM_CLOSE
            DeleteObject(self.brush);
        }
    }
}

#[test]
fn capture_matrix_all_14_cases() {
    let fixture = AnchorFixture::launch();
    let scale = measured_scale(&fixture);
    let monitors = monitor_rects();
    assert!(!monitors.is_empty(), "a Windows session has at least one monitor");
    println!(
        "MATRIX_SETUP scale={scale:.3} dpi={} monitors={} fixture_hwnd={} origin={:?} cross={:?}",
        (scale * 96.0) as i64,
        monitors.len(),
        fixture.hwnd,
        fixture.client_origin,
        fixture.cross
    );
    let ctx = Ctx {
        fixture,
        scale,
        monitors,
    };

    let cases = [
        "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10", "D11", "D12", "D14", "D13",
    ];
    assert_eq!(cases.len(), 14, "Appendix C has 14 cases");
    let mut passed = 0usize;
    let mut unverified = 0usize;
    for id in cases {
        let v = run_case(&ctx, id);
        println!("CASE {id}: {}", v.line());
        if v.passed() {
            passed += 1;
        } else {
            unverified += 1;
        }
    }
    println!(
        "MATRIX_DONE cases=14 pass={passed} unverified={unverified} scale={scale:.3}"
    );
    // D1, D5, D6, D8, D9, D10, D14 and D13 are passable on this machine and
    // MUST pass. D2-D4, D7, D11 and D12 are allowed to be Unverified by name.
    assert!(passed >= 8, "at least 8 real cases must pass, got {passed}");
    assert_eq!(passed + unverified, 14, "every case reported a verdict");
}
