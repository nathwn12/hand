#![windows_subsystem = "windows"]
//! Controlled Windows fixture for hand's desktop act/verify acceptance test.
//!
//! The test compiles this file with `rustc` (no crates, no cargo) and hands
//! the executable to `desktop_do {desktop:"hidden", background:<exe>}`. The
//! engine then creates a private desktop, launches THIS program on it with
//! `background_args = [<journal file>]`, and drives it with posted messages.
//!
//! The point of a separate program is evidence: this app records what actually
//! arrived at its controls in its own journal, written by its own window
//! procedure. That is an oracle independent of anything the server reports
//! about the same call, so "delivered" (the tool posted messages) and "the
//! application acted" (the app's own record) can be told apart.
//!
//! One window, `WS_POPUP` (no border, so client origin == window origin), with
//! an EDIT (id 100) that accepts text, a read-only status EDIT (id 102), and a
//! BUTTON (id 101). A posted click lands on the top-level window's client
//! pixels, so the window hit-tests the point against `BUTTON` and, when it is
//! inside, invokes the BUTTON control itself (`BM_CLICK`): the native control
//! sends its `BN_CLICKED` notification, and that is what flips the fixture's
//! state. Nothing here is a reimplementation of a button's semantics.
//!
//! Safety: it never activates itself (`SW_SHOWNOACTIVATE`), never calls the
//! pointer or foreground APIs, never touches the clipboard, never reads the
//! owner's desktop. It lives on the private desktop the engine created and
//! dies on `WM_CLOSE` from that engine.

include!("layout.rs");

use std::ffi::c_void;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

type Handle = *mut c_void;

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
    private: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: *const c_void,
    ) -> Handle;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    fn SendMessageW(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn DefWindowProcW(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize;
    fn DestroyWindow(hwnd: Handle) -> i32;
    fn PostQuitMessage(code: i32);
    fn SetWindowTextW(hwnd: Handle, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_READONLY: u32 = 0x0800;
const SW_SHOWNOACTIVATE: i32 = 4;
const IDC_ARROW: usize = 32512;
const COLOR_WINDOW: usize = 5;

const WM_COMMAND: u32 = 0x0111;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const EN_CHANGE: u32 = 0x0300;
const BN_CLICKED: u32 = 0;
const BM_CLICK: u32 = 0x00F5;

/// Where the journal goes; set once from argv before any window exists.
static JOURNAL: OnceLock<String> = OnceLock::new();
/// The status control, so the click handler can update it without the HWND
/// travelling through the message parameters.
static STATUS_HWND: AtomicUsize = AtomicUsize::new(0);
/// The button control the posted click is routed to.
static BUTTON_HWND: AtomicUsize = AtomicUsize::new(0);
/// True while a button-down inside the button is outstanding.
static ARMED: AtomicBool = AtomicBool::new(false);
static TOGGLED: AtomicBool = AtomicBool::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn journal(line: &str) {
    let Some(path) = JOURNAL.get() else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

fn window_text(hwnd: Handle) -> String {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn set_text(hwnd: Handle, text: &str) {
    let w = wide(text);
    unsafe {
        SetWindowTextW(hwnd, w.as_ptr());
    }
}

/// Client point packed into a mouse message's lParam, signed.
fn lparam_point(l: isize) -> (i32, i32) {
    ((l as u16 as i16) as i32, ((l >> 16) as u16 as i16) as i32)
}

unsafe extern "system" fn wndproc(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize {
    match msg {
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            let code = ((w >> 16) & 0xffff) as u32;
            if id == EDIT_ID && code == EN_CHANGE {
                let text = window_text(l as Handle);
                journal(&format!("{EV_EDIT}={text}"));
            }
            if id == BUTTON_ID && code == BN_CLICKED {
                let on = !TOGGLED.swap(true, Ordering::SeqCst);
                let status = STATUS_HWND.load(Ordering::SeqCst) as Handle;
                if !status.is_null() {
                    set_text(status, if on { STATUS_ON } else { STATUS_OFF });
                }
                journal(&format!(
                    "{EV_TOGGLE}={}",
                    if on { TOGGLE_ON } else { "off" }
                ));
            }
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_point(l);
            let inside = contains(BUTTON, x, y);
            ARMED.store(inside, Ordering::SeqCst);
            journal(&format!(
                "{EV_CLICK} x={x} y={y} inside={}",
                i32::from(inside)
            ));
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_point(l);
            if ARMED.swap(false, Ordering::SeqCst) && contains(BUTTON, x, y) {
                let button = BUTTON_HWND.load(Ordering::SeqCst) as Handle;
                if !button.is_null() {
                    SendMessageW(button, BM_CLICK, 0, 0);
                }
            }
            0
        }
        WM_CLOSE => {
            journal(EV_CLOSE);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

fn child(
    parent: Handle,
    class: &str,
    text: Option<&str>,
    style: u32,
    id: i32,
    r: (i32, i32, i32, i32),
) -> Handle {
    let class_w = wide(class);
    let text_w = text.map(wide);
    unsafe {
        CreateWindowExW(
            0,
            class_w.as_ptr(),
            text_w.as_ref().map_or(std::ptr::null(), |t| t.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            r.0,
            r.1,
            r.2,
            r.3,
            parent,
            id as isize as Handle,
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    }
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        std::process::exit(2);
    };
    let _ = JOURNAL.set(path);
    let class = wide(WINDOW_CLASS);
    let title = wide(&format!("{WINDOW_TITLE_PREFIX}-{}", std::process::id()));
    unsafe {
        let mut wc: WndClassW = std::mem::zeroed();
        wc.lpfn_wnd_proc = Some(wndproc);
        wc.cursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16);
        wc.background = COLOR_WINDOW as Handle;
        wc.class_name = class.as_ptr();
        if RegisterClassW(&wc) == 0 {
            journal("error=register_class");
            std::process::exit(3);
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            ORIGIN.0,
            ORIGIN.1,
            WIDTH,
            HEIGHT,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        if hwnd.is_null() {
            journal("error=create_window");
            std::process::exit(4);
        }
        let edit = child(
            hwnd,
            "EDIT",
            None,
            WS_BORDER | ES_AUTOHSCROLL,
            EDIT_ID,
            EDIT,
        );
        let status = child(
            hwnd,
            "EDIT",
            Some(STATUS_IDLE),
            WS_BORDER | ES_READONLY,
            STATUS_ID,
            STATUS,
        );
        let button = child(hwnd, "BUTTON", Some("Toggle"), 0, BUTTON_ID, BUTTON);
        if edit.is_null() || status.is_null() || button.is_null() {
            journal("error=create_children");
            std::process::exit(5);
        }
        STATUS_HWND.store(status as usize, Ordering::SeqCst);
        BUTTON_HWND.store(button as usize, Ordering::SeqCst);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        journal(&format!(
            "{EV_READY} pid={} hwnd={}",
            std::process::id(),
            hwnd as isize
        ));
        let mut msg: Msg = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r == 0 || r == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
