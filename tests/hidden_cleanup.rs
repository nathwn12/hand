//! Hidden-desktop launch lifecycle: the owned target must never outlive the
//! server that launched it, not even through a hard kill that skips every Rust
//! destructor.
//!
//! Evidence route: a controlled fixture (compiled from source with `rustc`, no
//! crates) is launched on the private desktop and journals its own pid - plus,
//! in the hard-kill arm, the pid of a child it creates - then sits in a message
//! loop while the `desktop_do` call is still in flight. The oracle is therefore
//! the fixture's own record, not anything the server says about the same call.
//!
//! The first test kills the server with `TerminateProcess` mid-call and waits
//! on SYNCHRONIZE handles for both pids: with a kill-on-close cover the OS
//! reaps the whole owned tree; without one both survive as orphans on a
//! desktop nobody can see.
//!
//! The second test is the same launch allowed to finish: it proves the cover is
//! armed and reported, and that teardown leaves nothing running. The third
//! pins a refused launch: setup prose (no invented code, no fabricated capture
//! timeout) and `isError:true` even when every action was target-free. The
//! fourth is a source-order pin for the handle lifetime the hard kill cannot
//! observe: the owner transfers the job handle last and never reuses it. The
//! fifth proves the cleanup guard adopts a process only by verified identity
//! (image + creation time) and terminates through the handle it holds, so a
//! recycled pid in an old journal can never be killed.

mod common;

use common::{is_error, structured, text_of, Session};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The fixture, kept as source so the test owns both sides of its contract.
/// It never takes the foreground, never moves the pointer and never touches
/// the clipboard: `WS_POPUP` shown with `SW_SHOWNOACTIVATE`, and it dies on the
/// engine's `WM_CLOSE`.
const FIXTURE_SRC: &str = r##"
#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::io::Write;

type Handle = *mut c_void;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    w: usize,
    l: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct WndClassW {
    style: u32,
    lpfn_wnd_proc: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct StartupInfoW {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: i32,
    y: i32,
    x_size: i32,
    y_size: i32,
    x_count_chars: i32,
    y_count_chars: i32,
    fill_attribute: u32,
    flags: u32,
    show_window: u16,
    reserved2: u16,
    reserved2_ptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[repr(C)]
struct FileTime {
    lo: u32,
    hi: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessTimes(
        h: Handle,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        pa: *const c_void,
        ta: *const c_void,
        inherit: i32,
        flags: u32,
        env: *const c_void,
        cwd: *const u16,
        si: *const StartupInfoW,
        pi: *mut ProcessInformation,
    ) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn RegisterClassW(wc: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Handle;
    fn ShowWindow(h: Handle, cmd: i32) -> i32;
    fn UpdateWindow(h: Handle) -> i32;
    fn GetMessageW(m: *mut Msg, h: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(m: *const Msg) -> i32;
    fn DispatchMessageW(m: *const Msg) -> isize;
    fn DefWindowProcW(h: Handle, m: u32, w: usize, l: isize) -> isize;
    fn PostQuitMessage(code: i32);
    fn DestroyWindow(h: Handle) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn note(path: &str, line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", line);
    }
}

/// Creation time (FILETIME ticks) of a process, 0 when unreadable. The journal
/// records it so the test can prove later that a pid still names THIS process.
fn created_of(h: Handle) -> u64 {
    unsafe {
        let mut c = FileTime { lo: 0, hi: 0 };
        let mut e = FileTime { lo: 0, hi: 0 };
        let mut k = FileTime { lo: 0, hi: 0 };
        let mut u = FileTime { lo: 0, hi: 0 };
        if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
            return 0;
        }
        ((c.hi as u64) << 32) | c.lo as u64
    }
}

unsafe extern "system" fn wnd_proc(h: Handle, m: u32, w: usize, l: isize) -> isize {
    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const ES_MULTILINE: u32 = 0x0004;
    const ES_AUTOVSCROLL: u32 = 0x0040;
    match m {
        WM_CREATE => {
            let cls = wide("EDIT");
            let inst = GetModuleHandleW(std::ptr::null());
            let edit = CreateWindowExW(
                0,
                cls.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | ES_MULTILINE | ES_AUTOVSCROLL,
                12,
                12,
                380,
                240,
                h,
                100isize as Handle,
                inst,
                std::ptr::null_mut(),
            );
            if edit.is_null() {
                return -1;
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(|s| s == "--sleeper").unwrap_or(false) {
        std::thread::sleep(std::time::Duration::from_secs(300));
        return;
    }
    let journal = argv.get(2).cloned().unwrap_or_default();
    let with_child = argv.get(3).map(|s| s == "child").unwrap_or(false);
    let pid = unsafe { GetCurrentProcessId() };
    note(
        &journal,
        &format!("boot {} {}", pid, created_of(unsafe { GetCurrentProcess() })),
    );

    if with_child {
        unsafe {
            let self_exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let mut si: StartupInfoW = std::mem::zeroed();
            si.cb = std::mem::size_of::<StartupInfoW>() as u32;
            let mut pi: ProcessInformation = std::mem::zeroed();
            let mut cmd = wide(&format!("\"{self_exe}\" --sleeper"));
            let ok = CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0x0800_0000, // CREATE_NO_WINDOW
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            );
            if ok != 0 {
                note(
                    &journal,
                    &format!("child {} {}", pi.process_id, created_of(pi.process)),
                );
                CloseHandle(pi.thread);
                CloseHandle(pi.process);
            } else {
                note(&journal, "child failed");
                std::process::exit(3);
            }
        }
    }

    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let cls = wide("HandHiddenCleanupFixture");
        let wc = WndClassW {
            style: 0,
            lpfn_wnd_proc: Some(wnd_proc),
            cls_extra: 0,
            wnd_extra: 0,
            instance: inst,
            icon: std::ptr::null_mut(),
            cursor: std::ptr::null_mut(),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: cls.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide(&format!("hand-hidden-cleanup {}", pid));
        const WS_POPUP: u32 = 0x8000_0000;
        const WS_VISIBLE: u32 = 0x1000_0000;
        let h = CreateWindowExW(
            0,
            cls.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            120,
            120,
            420,
            300,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null_mut(),
        );
        if h.is_null() {
            note(&journal, "window failed");
            std::process::exit(2);
        }
        ShowWindow(h, 4); // SW_SHOWNOACTIVATE
        UpdateWindow(h);
        note(&journal, "ready");
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    note(&journal, "exit");
}
"##;


#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
    fn WaitForSingleObject(h: *mut core::ffi::c_void, ms: u32) -> u32;
    fn TerminateProcess(h: *mut core::ffi::c_void, code: u32) -> i32;
    fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
    fn QueryFullProcessImageNameW(
        h: *mut core::ffi::c_void,
        flags: u32,
        buf: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn GetProcessTimes(
        h: *mut core::ffi::c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
}

#[repr(C)]
struct FileTime {
    lo: u32,
    hi: u32,
}

const PROCESS_TERMINATE: u32 = 0x0001;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

/// One process, held by HANDLE and adopted only after its image path proves it
/// is the fixture this test built.
///
/// A pid is not an identity: Windows recycles pids, so a value read from an old
/// journal can name an unrelated process later. Keeping the handle pins the
/// process object (a pid cannot be reused while a handle is open) and proves
/// WHAT was opened, which is why termination never happens by pid.
struct Proc {
    h: *mut core::ffi::c_void,
    pid: u32,
    created: u64,
    exe: String,
}

impl Proc {
    /// Open `pid` only when it really is `expected_exe`, and — when a creation
    /// time is known — only when it is that exact process instance. Identity is
    /// checked before the handle is kept, so a recycled pid belonging to
    /// anything else is refused rather than adopted (and later terminated).
    ///
    /// `expected_created: None` is reserved for a pid that a live handle owned
    /// by this test already pins (`std::process::Child`), where the pid cannot
    /// be recycled while that handle is open; every journal-derived open must
    /// pass `Some`.
    fn open(pid: u32, expected_exe: &Path, expected_created: Option<u64>) -> Option<Proc> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let h = OpenProcess(
                PROCESS_TERMINATE | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if h.is_null() {
                return None;
            }
            let mut buf = vec![0u16; 2048];
            let mut size = buf.len() as u32;
            if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) == 0 {
                CloseHandle(h);
                return None;
            }
            let exe = String::from_utf16_lossy(&buf[..size as usize]);
            if !same_path(&exe, expected_exe) {
                CloseHandle(h);
                return None;
            }
            let mut creation = FileTime { lo: 0, hi: 0 };
            let mut exit = FileTime { lo: 0, hi: 0 };
            let mut kernel = FileTime { lo: 0, hi: 0 };
            let mut user = FileTime { lo: 0, hi: 0 };
            if GetProcessTimes(h, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
                CloseHandle(h);
                return None;
            }
            let created = ((creation.hi as u64) << 32) | creation.lo as u64;
            if created == 0 || expected_created.is_some_and(|want| want != created) {
                CloseHandle(h);
                return None;
            }
            Some(Proc {
                h,
                pid,
                created,
                exe,
            })
        }
    }

    /// Creation time of a live pid whose image matches `expected_exe`. The
    /// caller must be pinning the pid (a held handle), which is why the open
    /// here may omit the expected time.
    fn creation_of(pid: u32, expected_exe: &Path) -> Option<u64> {
        Proc::open(pid, expected_exe, None).map(|p| p.created)
    }

    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.h, 0) == WAIT_TIMEOUT }
    }

    fn wait_exit(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.h, ms) == WAIT_OBJECT_0 }
    }

    /// Terminate THIS handle's process. The handle is the identity: it stays
    /// valid for the process it was opened on, so no unrelated process can be
    /// hit even if the pid has been recycled meanwhile.
    fn terminate(&self) -> bool {
        if !self.alive() {
            return false;
        }
        eprintln!(
            "cleanup: terminating {} ({}, created {})",
            self.pid, self.exe, self.created
        );
        unsafe { TerminateProcess(self.h, 1) != 0 }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.h);
        }
    }
}

/// Case-insensitive path compare that tolerates a `\\?\` prefix, so the image
/// path the kernel reports matches the path this test built.
fn same_path(actual: &str, expected: &Path) -> bool {
    let norm = |s: &str| {
        s.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase()
    };
    norm(actual) == norm(&expected.to_string_lossy())
}

/// Test hygiene, not evidence: whatever the assertions decided, never leave a
/// launched fixture (or anything it spawned) alive on the owner's machine - and
/// never touch a process this run did not start.
///
/// Two safety rules make a red run safe:
/// - a tracked process is held by HANDLE and terminated through it, so a
///   recycled pid cannot redirect the kill;
/// - a pid found only in the journal (a panic before the test parsed it) is
///   adopted only when its image path proves it is this run's fixture, and is
///   left alone otherwise. A bare pid from an old journal is never enough.
struct Owned {
    journal: PathBuf,
    fixture: PathBuf,
    procs: Vec<Proc>,
}

impl Owned {
    fn new(journal: PathBuf, fixture: PathBuf) -> Self {
        Owned {
            journal,
            fixture,
            procs: Vec::new(),
        }
    }

    /// Adopt a pid only when BOTH the image and the journal's exact creation
    /// time prove it is this run's fixture process.
    fn track_pid(&mut self, pid: u32, created: u64) -> bool {
        match Proc::open(pid, &self.fixture, Some(created)) {
            Some(p) => {
                assert!(
                    p.created == created,
                    "a tracked process must match the journal's creation time (pid {pid})"
                );
                self.procs.push(p);
                true
            }
            None => false,
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Ok(text) = std::fs::read_to_string(&self.journal) {
            let j = parse_journal(&text);
            for (pid, created) in [(j.pid, j.pid_created), (j.child, j.child_created)] {
                let (Some(pid), Some(created)) = (pid, created) else {
                    continue;
                };
                if self.procs.iter().any(|p| p.pid == pid) {
                    continue;
                }
                match Proc::open(pid, &self.fixture, Some(created)) {
                    Some(p) => self.procs.push(p),
                    None => {
                        if let Some(image) = live_image(pid) {
                            eprintln!(
                                "cleanup: journal pid {pid} is alive but is not the claimed \
                                 process (image {image}); left untouched"
                            );
                        }
                    }
                }
            }
        }
        for p in &self.procs {
            let _ = p.terminate();
        }
    }
}

/// Image path of a live pid, or `None` when it cannot be opened. Diagnostic
/// only: never used to decide a termination.
fn live_image(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 2048];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..size as usize]))
    }
}


fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("opencode")
        .join(format!("hand-hidden-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn build_fixture(dir: &Path) -> PathBuf {
    let src = dir.join("fixture.rs");
    std::fs::write(&src, FIXTURE_SRC).expect("write fixture source");
    let exe = dir.join("fixture.exe");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let out = std::process::Command::new(rustc)
        .arg("--edition=2021")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("run rustc for the fixture");
    assert!(
        out.status.success(),
        "fixture build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(exe.is_file(), "fixture exe missing at {}", exe.display());
    exe
}

#[derive(Default, Debug)]
struct Journal {
    pid: Option<u32>,
    pid_created: Option<u64>,
    child: Option<u32>,
    child_created: Option<u64>,
    ready: bool,
}

fn parse_journal(text: &str) -> Journal {
    let mut j = Journal::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match (it.next(), it.next(), it.next()) {
            (Some("boot"), Some(pid), Some(created)) => {
                j.pid = pid.parse().ok();
                j.pid_created = created.parse().ok();
            }
            (Some("child"), Some(pid), Some(created)) => {
                j.child = pid.parse().ok();
                j.child_created = created.parse().ok();
            }
            (Some("ready"), _, _) => j.ready = true,
            _ => {}
        }
    }
    j
}

fn wait_ready(path: &Path, ms: u64, need_child: bool) -> Journal {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let j = parse_journal(&text);
            let complete = j.ready
                && j.pid.is_some()
                && j.pid_created.is_some()
                && (!need_child || (j.child.is_some() && j.child_created.is_some()));
            if complete {
                return j;
            }
        }
        assert!(
            Instant::now() < deadline,
            "fixture never reached the expected state within {ms}ms (ready, pid{} with creation \
             time) ; journal: {:?}",
            if need_child { ", child" } else { "" },
            std::fs::read_to_string(path).ok()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A caller-facing message may name only registered codes: any `name:` token
/// must appear in the published catalog (`claims::ERROR_CODES`), so a refusal
/// stays prose instead of inventing a code.
fn assert_prose_only(text: &str) {
    for tok in text.split_whitespace() {
        let Some(code) = tok.strip_suffix(':') else {
            continue;
        };
        if code.is_empty()
            || code.len() < 3
            || !code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            continue;
        }
        assert!(
            desktop_engine::claims::ERROR_CODES.contains(&code),
            "unregistered error code {code:?} in {text:?}"
        );
    }
}

fn hidden_args(exe: &Path, journal: &Path, with_child: bool) -> Value {
    let mut argv = vec![json!("run"), json!(journal.display().to_string())];
    if with_child {
        argv.push(json!("child"));
    }
    json!({
        "desktop": "hidden",
        "background": format!("exe:{}", exe.display()),
        "background_args": argv,
        "settle_ms": 0,
    })
}


/// Regression: `desktop:"hidden"` alongside an explicit `target` is the legacy
/// target-wins path - nothing is launched and the call keeps the ordinary
/// target-locked verdict. A hidden-specific failure rule must not turn a
/// successful action against the locked window into `ok:false`.
#[test]
fn hidden_flag_with_an_explicit_target_keeps_the_target_wins_verdict() {
    let dir = scratch("target-wins");
    let exe = build_fixture(&dir);
    let journal = dir.join("never-launched.txt");

    let mut s = Session::spawn();
    let listed = s.call("desktop_list_windows", json!({}));
    let windows = structured(&listed)["windows"]
        .as_array()
        .expect("windows array")
        .clone();
    let own_pid = std::process::id();
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
        .unwrap_or_else(|| panic!("no visible titled window to lock; windows: {windows:?}"));
    let hwnd = candidate["hwnd"].as_i64().expect("hwnd");

    let mut args = hidden_args(&exe, &journal, false);
    args["target"] = json!({ "hwnd": hwnd });
    args["actions"] = json!([{"type": "wait", "ms": 0}]);
    let resp = s.call("desktop_do", args);
    let sc = structured(&resp);
    assert!(
        !is_error(&resp),
        "a target-locked call must not be a tool error: {}",
        text_of(&resp)
    );
    assert_eq!(
        sc["ok"].as_bool(),
        Some(true),
        "a hidden flag with an explicit target must keep the target-win verdict: {sc}"
    );
    assert!(
        sc.get("hidden_desktop").is_none(),
        "no hidden launch happened, so no hidden_desktop block may be reported: {sc}"
    );
    assert_eq!(
        sc["target_locked"].as_i64(),
        Some(hwnd),
        "the explicit target must be the lock: {sc}"
    );
    assert!(
        !journal.exists(),
        "target-wins must not launch the hidden program"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Regression: `desktop:"hidden"` with no usable `background` fails before any
/// spawn. The verdict must be consistent (`ok:false` and `isError:true`) and the
/// request must still be named, instead of an `ok:false` payload with no
/// `hidden_desktop` marker that the envelope reads as success.
#[test]
fn hidden_without_a_usable_background_reports_a_consistent_error() {
    let mut s = Session::spawn();
    for (case, background, expect) in [
        ("missing", None, "needs background"),
        (
            "window-selector",
            Some("hwnd:1234".to_string()),
            "window selector",
        ),
    ] {
        let mut args = json!({
            "desktop": "hidden",
            "actions": [{"type": "wait", "ms": 0}],
            "settle_ms": 0,
        });
        if let Some(b) = background {
            args["background"] = json!(b);
        }
        let resp = s.call("desktop_do", args);
        assert!(
            is_error(&resp),
            "{case}: a hidden request that never launched must be a tool error: {}",
            text_of(&resp)
        );
        let sc = structured(&resp);
        assert_eq!(sc["ok"].as_bool(), Some(false), "{case}: {sc}");
        assert_eq!(
            sc["failed"].as_u64(),
            Some(0),
            "{case}: failed counts actions, not the launch: {sc}"
        );
        let hd = &sc["hidden_desktop"];
        assert_eq!(
            hd["requested"].as_str(),
            Some("hidden"),
            "{case}: the request is still named: {sc}"
        );
        assert_eq!(hd["desktop_created"].as_bool(), Some(false), "{case}: {sc}");
        assert_eq!(hd["hard_kill_cover"].as_bool(), Some(false), "{case}: {sc}");
        assert_eq!(hd["launched_pid"], Value::Null, "{case}: {sc}");
        assert_eq!(hd["teardown"], Value::Null, "{case}: {sc}");
        let why = sc["background_error"]
            .as_str()
            .unwrap_or_else(|| panic!("{case}: the refusal is reported: {sc}"));
        assert!(
            why.contains("bad_window") && why.contains(expect),
            "{case}: the refusal names the reason: {why}"
        );
        assert_prose_only(why);
    }
}

/// The reviewer's finding, as a test: a hard server kill (TerminateProcess,
/// which runs no destructor and no `desktop_do` teardown) must still reap the
/// process the hidden launch created, and every process it spawned.
#[test]
fn hard_server_kill_reaps_the_whole_hidden_launch_tree() {
    let dir = scratch("kill");
    let exe = build_fixture(&dir);
    let journal = dir.join("journal.txt");
    let mut cleanup = Owned::new(journal.clone(), exe.clone());

    let mut s = Session::spawn();
    let mut args = hidden_args(&exe, &journal, true);
    args["actions"] = json!([{"type": "wait", "ms": 20000}]);
    s.send(&json!({
        "jsonrpc": "2.0",
        "id": 9001,
        "method": "tools/call",
        "params": {"name": "desktop_do", "arguments": args},
    }));

    let j = wait_ready(&journal, 25000, true);
    let parent_pid = j.pid.expect("fixture journal records its pid");
    let parent_created = j
        .pid_created
        .expect("fixture journal records its creation time");
    let child_pid = j
        .child
        .expect("the fixture must have created the child this arm proves");
    let child_created = j
        .child_created
        .expect("fixture journal records the child's creation time");
    assert!(
        cleanup.track_pid(parent_pid, parent_created),
        "the journal pid {parent_pid} must be this run's fixture process"
    );
    assert!(
        cleanup.track_pid(child_pid, child_created),
        "the child pid {child_pid} must be this run's fixture process"
    );
    let parent = Proc::open(parent_pid, &exe, Some(parent_created))
        .expect("fixture process must be openable");
    let child =
        Proc::open(child_pid, &exe, Some(child_created)).expect("fixture child must be openable");
    assert!(
        parent.alive(),
        "fixture pid {parent_pid} must be alive before the kill"
    );
    assert!(
        child.alive(),
        "fixture child pid {child_pid} must be alive before the kill"
    );

    s.child.kill().expect("hard kill the server");
    let _ = s.child.wait();

    assert!(
        parent.wait_exit(15000),
        "a hard server kill orphaned the hidden target (pid {parent_pid} still running)"
    );
    assert!(
        child.wait_exit(15000),
        "a hard server kill orphaned the hidden target's child (pid {child_pid} still running)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same launch, allowed to finish: the cover is reported as armed, the
/// engine resolved THIS fixture, teardown is proven, and nothing survives it.
#[test]
fn hidden_teardown_arms_and_reports_the_kill_on_close_cover() {
    let dir = scratch("teardown");
    let exe = build_fixture(&dir);
    let journal = dir.join("journal.txt");
    let mut cleanup = Owned::new(journal.clone(), exe.clone());

    let mut s = Session::spawn();
    let mut args = hidden_args(&exe, &journal, false);
    args["actions"] = json!([{"type": "wait", "ms": 200}]);
    let resp = s.call("desktop_do", args);
    assert!(!is_error(&resp), "hidden launch failed: {}", text_of(&resp));

    let sc = structured(&resp);
    let hd = &sc["hidden_desktop"];
    assert_eq!(
        hd["window_class"].as_str(),
        Some("HandHiddenCleanupFixture"),
        "the engine must have resolved the controlled fixture window: {hd}"
    );
    assert_eq!(
        hd["hard_kill_cover"].as_bool(),
        Some(true),
        "a hidden launch must report an armed OS kill-on-close cover: {hd}"
    );
    assert_eq!(
        hd["teardown"]["proven"].as_bool(),
        Some(true),
        "teardown must be proven: {hd}"
    );
    assert_eq!(
        hd["absent_from_owner_enum_windows"].as_bool(),
        Some(true),
        "the launched window must never appear on the owner's desktop: {hd}"
    );

    let j = parse_journal(&std::fs::read_to_string(&journal).expect("fixture journal"));
    let parent_pid = j.pid.expect("fixture journal records its pid");
    let parent_created = j
        .pid_created
        .expect("fixture journal records its creation time");
    let _ = cleanup.track_pid(parent_pid, parent_created);
    let launched = hd["launched_pid"].as_u64().expect("launched pid") as u32;
    assert_eq!(
        launched, parent_pid,
        "the reported launched pid must be the fixture"
    );
    if let Some(p) = Proc::open(parent_pid, &exe, Some(parent_created)) {
        assert!(
            p.wait_exit(10000),
            "the hidden target survived its own teardown (pid {parent_pid})"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A launch that cannot start must say so as setup prose: no invented error
/// code, and no fabricated capture diagnosis. Before this was fixed, the
/// owner's setup refusal dropped its channel and `teardown()` reported
/// `capture_timeout` ("the target's UI thread is not responding") for a call
/// that never launched or captured anything.
#[test]
fn refused_hidden_launch_reports_setup_prose_not_a_capture_timeout() {
    let dir = scratch("refused");
    let journal = dir.join("journal.txt");
    let mut s = Session::spawn();
    let missing = dir.join("no-such-program.exe");
    let resp = s.call(
        "desktop_do",
        json!({
            "actions": [{"type": "wait", "ms": 0}],
            "desktop": "hidden",
            "background": format!("exe:{}", missing.display()),
            "background_args": ["run", journal.display().to_string()],
            "settle_ms": 0,
        }),
    );
    assert!(
        is_error(&resp),
        "a hidden launch that never happened must be a tool error: {}",
        text_of(&resp)
    );
    let sc = structured(&resp);
    let hd = &sc["hidden_desktop"];
    assert_eq!(sc["ok"].as_bool(), Some(false), "{sc}");
    assert_eq!(sc["failed"].as_u64(), Some(0), "{sc}");
    let setup = sc["background_error"]
        .as_str()
        .unwrap_or_else(|| panic!("the setup refusal is reported: {sc}"));
    assert!(
        setup.contains("CreateProcessW failed") && setup.contains("no-such-program.exe"),
        "the refusal names what failed and for which program: {setup}"
    );
    assert_eq!(hd["desktop_created"].as_bool(), Some(false), "{hd}");
    assert_eq!(hd["window_hwnd"], Value::Null, "{hd}");
    let teardown_error = hd["teardown"]["error"]
        .as_str()
        .unwrap_or_else(|| panic!("a refused launch still reports a teardown line: {hd}"));
    assert!(
        !teardown_error.contains("capture_timeout"),
        "a refused launch must not be reported as a capture timeout: {teardown_error}"
    );
    assert!(
        teardown_error.contains("ended during setup"),
        "the teardown line states that setup ended: {teardown_error}"
    );
    assert_eq!(hd["launched_pid"], Value::Null, "{hd}");
    assert_eq!(hd["hard_kill_cover"].as_bool(), Some(false), "{hd}");
    assert_eq!(hd["teardown"]["proven"].as_bool(), Some(false), "{hd}");

    assert_prose_only(setup);
    assert_prose_only(teardown_error);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Lexical pin for the lifetime rule the hard-kill test cannot observe: the
/// owner keeps the only job handle until the target exists inside it and
/// membership is verified, then transfers it LAST and never touches the handle
/// again. A raw handle (or a copy such as `hjob`) used after the transfer can be
/// closed and recycled by a caller that stops waiting, which is exactly the
/// defect this pins against.
#[test]
fn owner_hands_the_job_over_last_and_never_reuses_the_handle() {
    let src = include_str!("../crates/desktop-engine/src/hidden.rs");
    let start = src
        .find("fn owner(")
        .expect("hidden.rs defines the owner thread function");
    let body = &src[start..];
    let body = match body.find("\nstruct EnumCtx") {
        Some(end) => &body[..end],
        None => body,
    };
    let send = body
        .find("tx_cover.send(cover)")
        .expect("the owner transfers the cover to the caller");
    for needle in ["cover.raw()", "job::contains"] {
        for (idx, _) in body.match_indices(needle) {
            assert!(
                idx < send,
                "{needle} appears after the cover was handed over; the owner must not use \
                 a handle it no longer owns"
            );
        }
    }
    assert!(
        !body.contains("let hjob"),
        "no raw copy of the job handle may outlive the transfer"
    );
    assert!(
        body[send..].contains("kill_target"),
        "a failed hand-over must terminate the launched process"
    );
    for banned in ["uncovered:", "refused:"] {
        assert!(
            !src.contains(banned),
            "hidden.rs must not emit {banned:?}: it reads like an unregistered error code"
        );
    }
}

/// Cleanup must never trust a pid. A journal left by a dead run can name a pid
/// Windows has since recycled, and terminating it would kill an unrelated
/// process - even one running the same image. So the guard adopts a process only
/// when the journal's exact creation time AND the image path prove it is this
/// run's fixture, and terminates through a held handle (which pins the process
/// object) rather than by pid.
#[test]
fn cleanup_guard_terminates_only_identity_verified_processes() {
    let dir = scratch("guard");
    let exe = build_fixture(&dir);

    let foreign = dir.join("foreign-journal.txt");
    std::fs::write(
        &foreign,
        format!("boot {} {}\nchild none\nready\n", std::process::id(), 1),
    )
    .expect("write foreign journal");
    assert!(
        Proc::open(std::process::id(), &exe, None).is_none(),
        "an unrelated process must fail the fixture-image check"
    );
    drop(Owned::new(foreign, exe.clone()));
    let self_exe = std::env::current_exe().expect("test exe path");
    assert!(
        Proc::open(std::process::id(), &self_exe, None).is_some(),
        "the guard must not have terminated this test process"
    );

    let mut sleeper = std::process::Command::new(&exe)
        .arg("--sleeper")
        .spawn()
        .expect("spawn the fixture sleeper");
    let pid = sleeper.id();
    let created = Proc::creation_of(pid, &exe).expect("sleeper identity is readable");

    assert!(
        Proc::open(pid, &exe, Some(created.wrapping_add(1))).is_none(),
        "a same-image process with a different creation time must be refused"
    );
    let stale = dir.join("stale-journal.txt");
    std::fs::write(
        &stale,
        format!(
            "boot {pid} {}\nchild none\nready\n",
            created.wrapping_add(1)
        ),
    )
    .expect("write stale journal");
    drop(Owned::new(stale, exe.clone()));
    assert!(
        Proc::open(pid, &exe, Some(created))
            .map(|p| p.alive())
            .unwrap_or(false),
        "a journal with the wrong creation time must not terminate the process"
    );

    let mut guard = Owned::new(dir.join("no-such-journal.txt"), exe.clone());
    assert!(
        guard.track_pid(pid, created),
        "the sleeper must be adopted by exact identity (image + creation time)"
    );
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if sleeper.try_wait().expect("try_wait").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the cleanup guard did not terminate the fixture it had identified (pid {pid})"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&dir);
}
