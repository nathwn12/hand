#![windows_subsystem = "windows"]
//! Anchor fixture for the 14-case capture matrix. The test compiles this file
//! with plain `rustc` (no crates, no cargo) and launches it on the owner
//! desktop. It is the independent oracle for geometry and for no-steal:
//!
//! * a WS_POPUP 1280x720 window, so client origin == window origin;
//! * a 1-px MAGENTA rectangle painted on the outermost client pixels;
//! * a GREEN cross painted at the exact client coordinate `CROSS`;
//! * an EDIT, a toggle CHECKBOX and a SELECT/EXPAND COMBOBOX, whose own
//!   notifications are journaled, so effect states get live proof;
//! * a `WM_SNAP` request that journals the OS foreground, cursor and clipboard
//!   sequence itself, independent of the engine's own no-steal block.
//!
//! The app journal is written by its own window procedure. Posting WM_SNAP
//! moves nothing: no foreground, pointer or clipboard API is ever called.

include!("anchor_layout.rs");

use std::ffi::c_void;
use std::io::Write;
use std::sync::OnceLock;

type Handle = *mut c_void;

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(Handle, u32, usize, isize) -> isize>,
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
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
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
    fn SetProcessDpiAwarenessContext(context: Handle) -> i32;
    fn RegisterClassW(class: *const WndClassW) -> u16;
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
        param: *const c_void,
    ) -> Handle;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn DefWindowProcW(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize;
    fn DestroyWindow(hwnd: Handle) -> i32;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(hwnd: Handle, paint: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Handle, paint: *const PaintStruct) -> i32;
    fn GetClientRect(hwnd: Handle, rc: *mut Rect) -> i32;
    fn ClientToScreen(hwnd: Handle, pt: *mut Point) -> i32;
    fn SetWindowTextW(hwnd: Handle, text: *const u16) -> i32;
    fn GetWindowTextW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn SendMessageW(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize;
    fn GetForegroundWindow() -> Handle;
    fn GetCursorPos(pt: *mut Point) -> i32;
    fn GetClipboardSequenceNumber() -> u32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateSolidBrush(color: u32) -> Handle;
    fn CreatePen(style: i32, width: i32, color: u32) -> Handle;
    fn SelectObject(hdc: Handle, obj: Handle) -> Handle;
    fn DeleteObject(obj: Handle) -> i32;
    fn FillRect(hdc: Handle, rc: *const Rect, brush: Handle) -> i32;
    fn MoveToEx(hdc: Handle, x: i32, y: i32, prev: *mut Point) -> i32;
    fn LineTo(hdc: Handle, x: i32, y: i32) -> i32;
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    rc_paint: Rect,
    f_restore: i32,
    f_inc_update: i32,
    reserved: [u8; 8],
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_BORDER: u32 = 0x0080_0000;
const ES_AUTOHSCROLL: u32 = 0x0080;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const CBS_DROPDOWNLIST: u32 = 0x0002;
const CBS_HASSTRINGS: u32 = 0x0200;
const SW_SHOWNOACTIVATE: i32 = 4;
const IDC_ARROW: usize = 32512;
const COLOR_WINDOW: usize = 5;

const WM_COMMAND: u32 = 0x0111;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000F;
const WM_PRINT: u32 = 0x0317;
const EN_CHANGE: u32 = 0x0300;
const BN_CLICKED: u32 = 0;
const CBN_SELCHANGE: u32 = 1;
const BM_GETCHECK: u32 = 0x00F0;
const CB_ADDSTRING: u32 = 0x0183;
const CB_SETCURSEL: u32 = 0x014E;

static JOURNAL: OnceLock<String> = OnceLock::new();

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

fn colour(c: (u8, u8, u8)) -> u32 {
    c.0 as u32 | ((c.1 as u32) << 8) | ((c.2 as u32) << 16)
}

/// Paint the anchor into ANY hdc (screen WM_PAINT or PrintWindow's WM_PRINT).
/// Border is the outermost client pixel row/column; the cross is a plus at the
/// exact client coordinate CROSS. Children paint over it in their own procs.
unsafe fn paint(hdc: Handle, w: i32, h: i32) {
    let brush = CreateSolidBrush(colour(BACKGROUND));
    let rc = Rect {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    FillRect(hdc, &rc, brush);
    DeleteObject(brush);

    let border = CreatePen(0, 1, colour(BORDER));
    let old = SelectObject(hdc, border);
    MoveToEx(hdc, 0, 0, std::ptr::null_mut());
    LineTo(hdc, w - 1, 0);
    LineTo(hdc, w - 1, h - 1);
    LineTo(hdc, 0, h - 1);
    LineTo(hdc, 0, 0);

    let cross = CreatePen(0, 3, colour(CROSS_COLOR));
    SelectObject(hdc, cross);
    let (cx, cy) = (CROSS.0, CROSS.1);
    MoveToEx(hdc, cx - CROSS_ARM, cy, std::ptr::null_mut());
    LineTo(hdc, cx + CROSS_ARM, cy);
    MoveToEx(hdc, cx, cy - CROSS_ARM, std::ptr::null_mut());
    LineTo(hdc, cx, cy + CROSS_ARM);

    SelectObject(hdc, old);
    DeleteObject(border);
    DeleteObject(cross);
}

unsafe fn window_text(hwnd: Handle) -> String {
    let mut buf = vec![0u16; 256];
    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    if n <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

unsafe extern "system" fn wndproc(hwnd: Handle, msg: u32, w: usize, l: isize) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps: PaintStruct = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            GetClientRect(hwnd, &mut rc);
            paint(hdc, rc.right, rc.bottom);
            EndPaint(hwnd, &ps);
            0
        }
        WM_PRINT => {
            paint(w as Handle, WIDTH, HEIGHT);
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as i32;
            let code = ((w >> 16) & 0xffff) as u32;
            if id == EDIT_ID && code == EN_CHANGE {
                journal(&format!("{EV_EDIT}={}", window_text(l as Handle)));
            }
            if id == CHECK_ID && code == BN_CLICKED {
                let checked = SendMessageW(l as Handle, BM_GETCHECK, 0, 0);
                journal(&format!("{EV_TOGGLE}={}", if checked != 0 { 1 } else { 0 }));
            }
            if id == COMBO_ID && code == CBN_SELCHANGE {
                let sel = SendMessageW(l as Handle, 0x0147, 0, 0); // CB_GETCURSEL
                journal(&format!("{EV_SELECT}={sel}"));
            }
            0
        }
        WM_SNAP => {
            let mut pt = Point { x: 0, y: 0 };
            let cursor = if GetCursorPos(&mut pt) != 0 {
                format!("{},{}", pt.x, pt.y)
            } else {
                "?".to_string()
            };
            journal(&format!(
                "{EV_SNAP} fg={} cursor={cursor} clip={}",
                GetForegroundWindow() as isize,
                GetClipboardSequenceNumber()
            ));
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

fn child(parent: Handle, class: &str, text: Option<&str>, style: u32, id: i32, r: (i32, i32, i32, i32)) -> Handle {
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
    // Physical client pixels on every monitor: the anchor math trusts this.
    unsafe {
        SetProcessDpiAwarenessContext((-4isize) as Handle);
    }
    let Some(path) = std::env::args().nth(1) else {
        std::process::exit(2);
    };
    let _ = JOURNAL.set(path);

    let class = wide(WINDOW_CLASS);
    let title = wide(&format!("{WINDOW_TITLE_PREFIX}-{}", std::process::id()));
    unsafe {
        let mut wc: WndClassW = std::mem::zeroed();
        wc.wnd_proc = Some(wndproc);
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
            WS_POPUP | WS_VISIBLE,
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
        let edit = child(hwnd, "EDIT", None, WS_BORDER | ES_AUTOHSCROLL, EDIT_ID, EDIT);
        let check = child(hwnd, "Button", Some("Toggle"), BS_AUTOCHECKBOX, CHECK_ID, CHECK);
        let combo = child(
            hwnd,
            "ComboBox",
            None,
            CBS_DROPDOWNLIST | CBS_HASSTRINGS | WS_BORDER,
            COMBO_ID,
            COMBO,
        );
        if edit.is_null() || check.is_null() || combo.is_null() {
            journal("error=create_children");
            std::process::exit(5);
        }
        for item in ["Alpha", "Beta", "Gamma"] {
            let s = wide(item);
            SendMessageW(combo, CB_ADDSTRING, 0, s.as_ptr() as isize);
        }
        SendMessageW(combo, CB_SETCURSEL, 0, 0);

        let mut client = Point { x: 0, y: 0 };
        ClientToScreen(hwnd, &mut client);
        journal(&format!(
            "{EV_READY} pid={} hwnd={} client_x={} client_y={} cross_x={} cross_y={}",
            std::process::id(),
            hwnd as isize,
            client.x,
            client.y,
            CROSS.0,
            CROSS.1
        ));
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);

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
