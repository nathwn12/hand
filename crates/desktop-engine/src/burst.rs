//! `do` burst tool: queued input actions in one round-trip.
//!
//! Wire- and behavior-parity with `puppet.py:do` (2614-3418). Real-input
//! The default path is posted/UIA to a locked target only. Two top-level
//! flags buy real input, never implicitly: `allow_cursor:true` unlocks the
//! guarded pointer path (SetCursorPos/SendInput, point_not_target when the
//! point is not on the locked window or its descendants, cursor restore
//! after the burst) and `allow_foreground:true` unlocks a single explicit
//! `focus_window` SetForegroundWindow. The live tests need an interactive
//! desktop and are `#[ignore]`gated; the headless harness proves the policy.

use crate::cap::{bitblt_grab, resolve_selector};
use crate::shot::element_model;
use crate::{
    enum_child_windows, py_repr_str, py_repr_value, py_round_n, virtual_rect, wrapped_text,
};
use base64::Engine as _;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, CountClipboardFormats, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const M_WHEEL: u32 = 0x0800;
const M_HWHEEL: u32 = 0x1000;

const BG_NOTE: &str = "background PostMessage delivery is best-effort: it is NOT honoured by every app and is never equivalent to foreground input.";
const BG_SUFFIX: &str = ""; // Retained for message shape; there is no foreground delivery to omit to.
const MAX_CLICK_COUNT: i64 = 100;
const MAX_STABLE_EDGE: i64 = 4096;
/// Hard ceiling on one burst. Every action runs under the global input lock,
/// so a huge batch can hold the desktop for hours; 256 is far past any real
/// UI flow and small enough that a stuck burst is a bounded stall.
pub const BURST_MAX_ACTIONS: usize = 256;
/// Total wall-clock budget for one burst call. Checked before every action,
/// inside every pacing loop (chunked `type` pacing, `key` hold/gap repeats)
/// and against the trailing settle, so no sleep chain can hold the lock past
/// it.
pub const BURST_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);
/// Test-only budget override, same idiom as `PUPPET_TEST_STALL_*_MS`: the
/// suite proves the pacing loops are bounded without a five-minute run. It
/// can only shorten the budget (clamped up to 500 ms, never past
/// `BURST_BUDGET`), so a test can never make a real burst run longer.
const BUDGET_TEST_VAR: &str = "PUPPET_TEST_BURST_BUDGET_MS";
const BUDGET_TEST_MIN: std::time::Duration = std::time::Duration::from_millis(500);

/// The burst budget in force for this call.
fn burst_budget() -> std::time::Duration {
    match std::env::var(BUDGET_TEST_VAR)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(ms) => std::time::Duration::from_millis(ms).clamp(BUDGET_TEST_MIN, BURST_BUDGET),
        None => BURST_BUDGET,
    }
}

/// What is left of the burst budget `t_all` elapsed ago (never negative).
fn budget_left_ms(t_all: &Instant, budget: std::time::Duration) -> i64 {
    budget.saturating_sub(t_all.elapsed()).as_millis() as i64
}

/// One pacing pause (`chunk_ms`, `hold_ms`, `gap_ms`, `settle_ms`) clamped to
/// what is left of the burst budget. `None` means the budget is spent: the
/// caller must stop with a typed `burst_budget_exceeded` result instead of
/// sleeping, so a 1 MiB text at `chunk_chars:1, chunk_ms:60000` cannot hold
/// the global input lock for a million minutes.
fn bounded_pause(t_all: &Instant, budget: std::time::Duration, want_ms: u64) -> Option<u64> {
    if want_ms == 0 {
        return Some(0);
    }
    let left = budget_left_ms(t_all, budget);
    if left <= 0 {
        return None;
    }
    Some(want_ms.min(left as u64))
}

/// The typed per-action failure a pacing loop returns when the budget stops
/// it: same `burst_budget_exceeded` code every other budget refusal uses.
fn budget_stop(
    i: usize,
    typ_v: &Value,
    t0: &Instant,
    budget: std::time::Duration,
    what: &str,
) -> Map<String, Value> {
    let mut d = base_entry(i, typ_v, false, elapsed_ms(t0));
    d.insert("code".to_string(), Value::from("burst_budget_exceeded"));
    d.insert(
        "error".to_string(),
        Value::from(format!(
            "burst_budget_exceeded: the {}-second burst budget expired during {what}; the rest of the action was not sent",
            budget.as_secs_f64()
        )),
    );
    d
}

/// Whole-call typed refusal for a target that could not be locked (nothing
/// ran): rendered as an MCP tool error carrying `{ok:false, code, error,
/// candidates?}` so the caller can act on the code, not parse prose.
/// Private-desktop permission is EARNED, never requested: it holds only when
/// THIS call created the desktop (so the server owns it) AND the action target
/// actually lives on that desktop. An explicit `target` wins over hidden
/// desktop creation, so a caller could otherwise pass
/// `target:{hwnd:<ordinary window>}, desktop:"hidden"` and inherit
/// private-desktop rights (posted keyboard) over the owner's session.
fn private_desktop_earned(
    desktop_created: bool,
    created_name: &str,
    target_desktop: Option<&str>,
) -> bool {
    desktop_created && !created_name.is_empty() && target_desktop == Some(created_name)
}

fn target_refusal_blocks(e: &crate::target::TargetError) -> String {
    let text = crate::py_dumps(&Value::Object(e.to_map()));
    crate::tool_err_blocks(&[crate::text_block(&text)])
}

/// Default stable-watch region: the foreground window's monitor, not the
/// whole virtual desktop ÔÇö a full-span grab every poll is the memory win
/// this avoids. Falls back to the virtual desktop with no foreground
/// window. The 4096 cap still applies downstream.
fn default_stable_region() -> Vec<i64> {
    let mons = crate::display_monitors();
    let fg = fg_now();
    if fg != 0 {
        unsafe {
            let mut rc = RECT::default();
            if GetWindowRect(fg as HWND, &mut rc) != 0 {
                let cx = (rc.left as i64 + rc.right as i64) / 2;
                let cy = (rc.top as i64 + rc.bottom as i64) / 2;
                if let Some(m) = monitor_for_point(&mons, cx, cy) {
                    return vec![m[0] as i64, m[1] as i64, m[2] as i64, m[3] as i64];
                }
            }
        }
    }
    let (vx, vy, vr, vb) = virtual_rect();
    vec![vx as i64, vy as i64, vr as i64, vb as i64]
}
fn bg_non_postable(typ: &str) -> Option<&'static str> {
    match typ {
        "move" => Some(
            "background_unavailable: move has no window-message equivalent (the pointer is not \
             part of a window's message stream; use click x/y for a posted click)",
        ),
        _ => None,
    }
}

fn py_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "int"
            } else {
                "float"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Python str(x): None->None, True/False, numbers, strings raw, containers repr.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".to_string(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => py_repr_value(v),
    }
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else {
                n.as_f64().map(|f| f != 0.0).unwrap_or(false)
            }
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// CPython int(x) with CPython-shaped error texts (puppet.py int(...) sites).
/// Overflow policy (F4): REJECT via the shared `crate::is_int_overflow`
/// gate with the shared `crate::int_overflow_err` message ÔÇö never clamp.
fn py_int(v: &Value) -> Result<i64, String> {
    if crate::is_int_overflow(v) {
        return Err(crate::int_overflow_err(v));
    }
    match v {
        Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(u) = n.as_u64() {
                Ok(u as i64)
            } else if let Some(f) = n.as_f64() {
                if f.is_nan() {
                    Err("cannot convert float NaN to integer".to_string())
                } else if f.is_infinite() {
                    Err("cannot convert float infinity to integer".to_string())
                } else {
                    Ok(f.trunc() as i64)
                }
            } else {
                Err("int() argument must be a real number".to_string())
            }
        }
        Value::String(s) => {
            let t = s.trim();
            let (neg, body) = match t.strip_prefix(['+', '-']) {
                Some(b) => (t.starts_with('-'), b),
                None => (false, t),
            };
            let digits = body.replace('_', "");
            let underscores_ok = !body.is_empty()
                && !body.starts_with('_')
                && !body.ends_with('_')
                && !body.contains("__");
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && underscores_ok {
                let mut val: i128 = 0;
                for b in digits.bytes() {
                    val = val * 10 + (b - b'0') as i128;
                }
                if neg {
                    val = -val;
                }
                Ok(i64::try_from(val).map_err(|_| crate::int_overflow_err(v))?)
            } else {
                Err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_repr_str(s)
                ))
            }
        }
        Value::Null => Err(
            "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'"
                .to_string(),
        ),
        Value::Array(_) => Err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_type_name(v)
        )),
        Value::Object(_) => Err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py_type_name(v)
        )),
    }
}

fn opt_py_int(a: &Map<String, Value>, key: &str, dflt: i64) -> Result<i64, String> {
    match a.get(key) {
        None => Ok(dflt),
        Some(v) => py_int(v),
    }
}

fn elapsed_ms(t: &Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Cancellable sleep in <=50ms slices. Returns true when the current
/// `tools/call` was cancelled mid-sleep so the caller can return promptly
/// (the worker drops the response; no late reply is sent).
fn sleep_cancellable(ms: u64) -> bool {
    let mut left = ms;
    while left > 0 {
        if crate::cancel::cancelled() {
            return true;
        }
        let s = left.min(50);
        std::thread::sleep(Duration::from_millis(s));
        left = left.saturating_sub(s);
    }
    crate::cancel::cancelled()
}

fn scroll_wheel(axis: &str, amt: i64) -> Result<(u32, u32), String> {
    let flag = match axis {
        "v" => M_WHEEL,
        "h" => M_HWHEEL,
        _ => return Err(format!("unknown scroll axis \"{axis}\" (want v|h)")),
    };
    if !(-1000..=1000).contains(&amt) {
        return Err(format!(
            "scroll amount out of range -1000..1000 (got {amt})"
        ));
    }
    Ok((flag, (amt as i32).wrapping_mul(120) as u32))
}

fn utf16_units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Resolved key: a virtual-key code (plus whether Shift must be held for the
/// layout's high byte) or a Unicode fallback when VkKeyScanW has no
/// translation (`sc == -1`) or the char needs Ctrl/Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyCode {
    Vk(u16, bool),
    Uni(char),
}

fn vk_of(name: &str) -> Result<KeyCode, String> {
    let key = name.to_lowercase();
    let named: Option<u16> = match key.as_str() {
        "enter" | "return" => Some(0x0D),
        "tab" => Some(0x09),
        "esc" | "escape" => Some(0x1B),
        "space" => Some(0x20),
        "backspace" | "back" => Some(0x08),
        "delete" | "del" => Some(0x2E),
        "insert" | "ins" => Some(0x2D),
        "home" => Some(0x24),
        "end" => Some(0x23),
        "pageup" | "pgup" => Some(0x21),
        "pagedown" | "pgdn" => Some(0x22),
        "up" => Some(0x26),
        "down" => Some(0x28),
        "left" => Some(0x25),
        "right" => Some(0x27),
        "printscreen" | "prtsc" => Some(0x2C),
        "pause" => Some(0x13),
        "break" => Some(0x03),
        "capslock" | "caps" => Some(0x14),
        "numlock" => Some(0x90),
        "scrolllock" => Some(0x91),
        "shift" => Some(0x10),
        "ctrl" | "control" => Some(0x11),
        "alt" | "menu" => Some(0x12),
        "lwin" => Some(0x5B),
        "rwin" => Some(0x5C),
        "win" => Some(0x5B),
        "apps" => Some(0x5D),
        _ => None,
    };
    if let Some(v) = named {
        return Ok(KeyCode::Vk(v, false));
    }
    if let Some(vk) = key
        .strip_prefix('f')
        .and_then(|r| r.parse::<u32>().ok())
        .filter(|i| (1..=24).contains(i))
    {
        return Ok(KeyCode::Vk((0x6F + vk) as u16, false));
    }
    if name.chars().count() == 1 {
        let ch = name.chars().next().unwrap();
        unsafe {
            let sc = VkKeyScanW(ch as u16);
            if sc == -1 {
                return Ok(KeyCode::Uni(ch));
            }
            let vk = (sc as u16) & 0xFF;
            let mods = ((sc as u16) >> 8) as u8;
            if mods & 0x06 != 0 {
                return Ok(KeyCode::Uni(ch));
            }
            return Ok(KeyCode::Vk(vk, mods & 0x01 != 0));
        }
    }
    Err(format!("unknown key name \"{name}\""))
}

/// Monitor containing a virtual-desktop point, for defaulting a watch
/// region to the foreground window's monitor instead of the whole
/// virtual desktop. Pure and unit-tested; `None` when no monitor
/// contains the point (caller falls back to the virtual desktop).
fn monitor_for_point(mons: &[[i32; 4]], x: i64, y: i64) -> Option<[i32; 4]> {
    mons.iter()
        .find(|m| x >= m[0] as i64 && x < m[2] as i64 && y >= m[1] as i64 && y < m[3] as i64)
        .copied()
}

/// Clipboard text budget: 1 MiB of characters. Reads truncate, writes
/// fail typed ÔÇö never silently, never unbounded.
pub(crate) const MAX_CLIP_CHARS: usize = 1_048_576;

/// Typed bound check shared by clipboard_set (before OpenClipboard, so a
/// rejection never touches the clipboard).
fn check_clip_len(n_chars: usize) -> Result<(), String> {
    if n_chars > MAX_CLIP_CHARS {
        return Err(format!(
            "clipboard text too large ({n_chars} chars, max {MAX_CLIP_CHARS})"
        ));
    }
    Ok(())
}

/// A typed clipboard failure: stage, Win32 last-error code and the number of
/// `OpenClipboard` attempts made. Callers emit one `clipboard_unavailable`
/// wire error from this, never a bare string and never a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ClipboardError {
    stage: &'static str,
    code: u32,
    attempts: u32,
}

impl ClipboardError {
    fn new(stage: &'static str, code: u32, attempts: u32) -> Self {
        ClipboardError {
            stage,
            code,
            attempts,
        }
    }

    /// Canonical wire text: the Win32 code and attempt count are always named.
    fn text(&self) -> String {
        format!(
            "clipboard_unavailable: stage={} win32_error={} attempts={}",
            self.stage, self.code, self.attempts
        )
    }
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

/// A clipboard_get failure has two kinds. NoTextFormat is a normal
/// clipboard state (empty, or holding a non-text format), not a failed
/// read: it never reports an attempt count because no OpenClipboard was
/// needed. Unavailable is a failed OpenClipboard/GlobalLock/SetClipboardData
/// after the bounded retry and is the only kind emitted as
/// clipboard_unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClipboardGetError {
    NoTextFormat,
    Unavailable(ClipboardError),
}

impl From<ClipboardError> for ClipboardGetError {
    fn from(e: ClipboardError) -> Self {
        ClipboardGetError::Unavailable(e)
    }
}

impl ClipboardGetError {
    /// (wire code, wire error prose) for the action result.
    fn wire(&self) -> (&'static str, String) {
        match self {
            ClipboardGetError::NoTextFormat => (
                "no_text_format",
                "no_text_format: the clipboard holds no CF_UNICODETEXT (it may be empty or hold another format)".to_string(),
            ),
            ClipboardGetError::Unavailable(e) => ("clipboard_unavailable", e.text()),
        }
    }
}

/// Read GetLastError immediately after the Win32 call that failed, before
/// any other Win32 call (including CloseClipboard) can overwrite it.
unsafe fn clip_last_error(stage: &'static str, attempts: u32) -> ClipboardError {
    ClipboardError::new(stage, GetLastError(), attempts)
}

/// `OpenClipboard` takes an exclusive global lock. A few milliseconds of
/// contention are normal, so retry with a 2 ms backoff; 30 attempts and a
/// 60 ms total cap keep the worst case far below the 300 ms burst budget.
const CLIP_OPEN_MAX_ATTEMPTS: u32 = 30;
const CLIP_OPEN_BACKOFF: Duration = Duration::from_millis(2);
const CLIP_OPEN_WAIT_CAP: Duration = Duration::from_millis(60);

/// Open the clipboard, returning the number of attempts used on success.
fn clipboard_open() -> Result<u32, ClipboardError> {
    let start = Instant::now();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
            return Ok(attempts);
        }
        let code = unsafe { GetLastError() };
        let elapsed = start.elapsed();
        if attempts >= CLIP_OPEN_MAX_ATTEMPTS || elapsed >= CLIP_OPEN_WAIT_CAP {
            return Err(ClipboardError::new("OpenClipboard", code, attempts));
        }
        std::thread::sleep(CLIP_OPEN_BACKOFF.min(CLIP_OPEN_WAIT_CAP - elapsed));
    }
}

/// Bounded NUL-terminated UTF-16 read: scans at most `max_units` for the
/// terminator (the GlobalSize bound), then decodes lossily. A missing NUL
/// is an error, never an over-read.
unsafe fn clip_read_u16(
    ptr: *const u16,
    max_units: usize,
    attempts: u32,
) -> Result<String, ClipboardError> {
    let mut len = 0usize;
    while len < max_units {
        if *ptr.add(len) == 0 {
            break;
        }
        len += 1;
    }
    if len == max_units {
        return Err(ClipboardError::new("clip_read_u16", 0, attempts));
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    Ok(String::from_utf16_lossy(slice))
}

/// Read at most MAX_CLIP_CHARS, and report the ORIGINAL length plus whether
/// truncation happened. The 1 MiB cap stays; the claim in claims.rs is now
/// observable on the action result instead of being a promise without a field.
fn clipboard_get() -> Result<(String, usize, bool, u32), ClipboardGetError> {
    // IsClipboardFormatAvailable needs no open handle. An empty clipboard,
    // or one holding only non-text formats, is a normal state: return the
    // distinct no_text_format BEFORE any OpenClipboard attempt, so the wire
    // result never reports attempts as if a retry happened.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return Err(ClipboardGetError::NoTextFormat);
    }
    unsafe {
        let attempts = clipboard_open()?;
        let r = (|| -> Result<(String, usize, bool), ClipboardGetError> {
            // Recheck under the handle: the format can change between the
            // pre-check above and OpenClipboard. Still no_text_format: the
            // clipboard is readable, it simply holds no text right now.
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return Err(ClipboardGetError::NoTextFormat);
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                return Err(clip_last_error("GetClipboardData", attempts).into());
            }
            let bytes = GlobalSize(h) as usize;
            if bytes < 2 {
                return Err(ClipboardError::new("GlobalSize", 0, attempts).into());
            }
            let max_units = bytes / 2;
            let p = GlobalLock(h);
            if p.is_null() {
                return Err(clip_last_error("GlobalLock", attempts).into());
            }
            let s = clip_read_u16(p as *const u16, max_units, attempts)?;
            GlobalUnlock(h);
            let chars = s.chars().count();
            if chars > MAX_CLIP_CHARS {
                Ok((s.chars().take(MAX_CLIP_CHARS).collect(), chars, true))
            } else {
                Ok((s, chars, false))
            }
        })();
        let close_ok = CloseClipboard() != 0;
        let close_code = if close_ok { 0 } else { GetLastError() };
        let (text, chars, truncated) = r?;
        if !close_ok {
            return Err(ClipboardError::new("CloseClipboard", close_code, attempts).into());
        }
        Ok((text, chars, truncated, attempts))
    }
}

/// CF_UNICODETEXT, the one text format the reader and writer use.
const CF_UNICODETEXT: u32 = 13;

/// The owner's clipboard before the first write of a burst. `non_text`
/// means another format was present: those bytes cannot be copied back, so
/// a restore clears the clipboard instead of inventing content.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ClipboardPrevious {
    Empty,
    Text(String),
    NonText,
}

impl ClipboardPrevious {
    fn label(&self) -> &'static str {
        match self {
            ClipboardPrevious::Empty => "empty",
            ClipboardPrevious::Text(_) => "text",
            ClipboardPrevious::NonText => "non_text",
        }
    }
}

/// Read the current clipboard state while the handle is already open.
/// Shared by the snapshot and the reader; every failure is typed.
unsafe fn clipboard_snapshot_open(attempts: u32) -> Result<ClipboardPrevious, ClipboardError> {
    if IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 {
        let h = GetClipboardData(CF_UNICODETEXT);
        if h.is_null() {
            return Err(clip_last_error("GetClipboardData", attempts));
        }
        let bytes = GlobalSize(h) as usize;
        if bytes < 2 {
            return Ok(ClipboardPrevious::Empty);
        }
        let p = GlobalLock(h);
        if p.is_null() {
            return Err(clip_last_error("GlobalLock", attempts));
        }
        let s = clip_read_u16(p as *const u16, bytes / 2, attempts);
        GlobalUnlock(h);
        let s = s?;
        let s = if s.chars().count() > MAX_CLIP_CHARS {
            s.chars().take(MAX_CLIP_CHARS).collect()
        } else {
            s
        };
        return Ok(ClipboardPrevious::Text(s));
    }
    if CountClipboardFormats() == 0 {
        Ok(ClipboardPrevious::Empty)
    } else {
        Ok(ClipboardPrevious::NonText)
    }
}

/// Snapshot the clipboard before the first write. A failed snapshot refuses
/// the write: never destroy contents that could not be read first.
fn clipboard_snapshot() -> Result<ClipboardPrevious, ClipboardError> {
    unsafe {
        let attempts = clipboard_open()?;
        let r = clipboard_snapshot_open(attempts);
        let close_ok = CloseClipboard() != 0;
        let close_code = if close_ok { 0 } else { GetLastError() };
        let previous = r?;
        if !close_ok {
            return Err(ClipboardError::new("CloseClipboard", close_code, attempts));
        }
        Ok(previous)
    }
}

/// Write UTF-16 text as CF_UNICODETEXT through the Win32 clipboard chain:
/// GlobalAlloc(GMEM_MOVEABLE) -> GlobalLock -> copy -> GlobalUnlock ->
/// OpenClipboard -> EmptyClipboard -> SetClipboardData -> CloseClipboard.
/// The 1 MiB cap is checked before the clipboard is opened, so a rejected
/// write touches nothing. Every failure is a typed error, never a panic;
/// the allocation is freed unless SetClipboardData took ownership of it.
fn clipboard_set_text(text: &str) -> Result<(usize, u32), ClipboardError> {
    // Cap stays before any Win32 interaction; the action layer already
    // returns the distinct too_large code before reaching this defence.
    check_clip_len(text.chars().count()).map_err(|_| ClipboardError::new("clip_len", 0, 0))?;
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * std::mem::size_of::<u16>();
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h.is_null() {
            return Err(clip_last_error("GlobalAlloc", 0));
        }
        let p = GlobalLock(h);
        if p.is_null() {
            let e = clip_last_error("GlobalLock", 0);
            GlobalFree(h);
            return Err(e);
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), p as *mut u16, units.len());
        GlobalUnlock(h);
        let attempts = match clipboard_open() {
            Ok(n) => n,
            Err(e) => {
                GlobalFree(h);
                return Err(e);
            }
        };
        let mut handed_off = false;
        let r = (|| -> Result<(), ClipboardError> {
            if EmptyClipboard() == 0 {
                return Err(clip_last_error("EmptyClipboard", attempts));
            }
            if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                return Err(clip_last_error("SetClipboardData", attempts));
            }
            handed_off = true; // the system owns h from here on
            Ok(())
        })();
        let close_ok = CloseClipboard() != 0;
        let close_code = if close_ok { 0 } else { GetLastError() };
        let outcome = match r {
            Err(e) => Err(e),
            Ok(()) if !close_ok => Err(ClipboardError::new("CloseClipboard", close_code, attempts)),
            Ok(()) => Ok(()),
        };
        if !handed_off {
            GlobalFree(h);
        }
        outcome.map(|_| (bytes, attempts))
    }
}

/// Clear the clipboard when the previous state was `empty` or `non_text`.
fn clipboard_clear() -> Result<u32, ClipboardError> {
    unsafe {
        let attempts = clipboard_open()?;
        let empty_ok = EmptyClipboard() != 0;
        let empty_code = if empty_ok { 0 } else { GetLastError() };
        let close_ok = CloseClipboard() != 0;
        let close_code = if close_ok { 0 } else { GetLastError() };
        if !empty_ok {
            Err(ClipboardError::new("EmptyClipboard", empty_code, attempts))
        } else if !close_ok {
            Err(ClipboardError::new("CloseClipboard", close_code, attempts))
        } else {
            Ok(attempts)
        }
    }
}

/// Per-burst clipboard ledger: one snapshot before the first write, one
/// restore after the last action, with typed failures reported and never
/// panicking. No clipboard handle is held between actions, so the owner is
/// never blocked from using their own clipboard for the length of a burst.
struct ClipboardLedger {
    restore: bool,
    previous: Option<ClipboardPrevious>,
    writes: i64,
    restores: i64,
    bytes: i64,
    error: Option<ClipboardError>,
}

impl ClipboardLedger {
    fn new(restore: bool) -> Self {
        ClipboardLedger {
            restore,
            previous: None,
            writes: 0,
            restores: 0,
            bytes: 0,
            error: None,
        }
    }

    /// Snapshot before a write. A failed snapshot refuses the write rather
    /// than overwriting what we could not read.
    fn before_write(&mut self) -> Result<(), ClipboardError> {
        if self.previous.is_none() {
            self.previous = Some(clipboard_snapshot()?);
        }
        Ok(())
    }

    fn wrote(&mut self, bytes: usize) {
        self.writes += 1;
        self.bytes += bytes as i64;
    }

    /// Run after the last action, including a partial or failed burst. The
    /// snapshot stays in place so the report keeps saying what was found.
    fn finish(&mut self) {
        if !self.restore || self.writes == 0 {
            return;
        }
        let Some(previous) = self.previous.clone() else {
            return;
        };
        let r = match previous {
            ClipboardPrevious::Text(ref s) => clipboard_set_text(s).map(|_| ()),
            ClipboardPrevious::Empty | ClipboardPrevious::NonText => clipboard_clear().map(|_| ()),
        };
        match r {
            Ok(()) => self.restores += 1,
            Err(e) => self.error = Some(e),
        }
    }

    fn to_map(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("writes".to_string(), Value::from(self.writes));
        m.insert("restores".to_string(), Value::from(self.restores));
        m.insert(
            "previous".to_string(),
            Value::from(self.previous.as_ref().map(|p| p.label()).unwrap_or("empty")),
        );
        m.insert("bytes".to_string(), Value::from(self.bytes));
        if let Some(e) = &self.error {
            m.insert("error".to_string(), Value::from(e.text()));
        }
        m
    }
}

/// Signed 16-bit range a window message can carry in one lParam half.
const MSG_COORD_MIN: i64 = -32768;
const MSG_COORD_MAX: i64 = 32767;

fn bg_lparam(x: i64, y: i64) -> LPARAM {
    (((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as i32 as isize
}

/// Refuse coordinates a message lParam cannot carry. `bg_lparam` packs each
/// axis into 16 bits, so 40000 would silently become -25536 and the posting
/// would report success at the wrong point. Typed, before any message.
fn msg_coords(x: i64, y: i64) -> Result<(i64, i64), String> {
    if !(MSG_COORD_MIN..=MSG_COORD_MAX).contains(&x)
        || !(MSG_COORD_MIN..=MSG_COORD_MAX).contains(&y)
    {
        return Err(format!(
            "bad_coordinates: message coordinates are signed 16-bit (got x={x}, y={y}; allowed \
             {}..={}); nothing was posted",
            MSG_COORD_MIN, MSG_COORD_MAX
        ));
    }
    Ok((x, y))
}

fn bg_client(hwnd: i64, x: &Value, y: &Value) -> Result<(i64, i64), String> {
    let xi = py_int(x).map_err(|e| format!("ScreenToClient failed: {e}"))?;
    let yi = py_int(y).map_err(|e| format!("ScreenToClient failed: {e}"))?;
    for (v, name) in [(xi, "x"), (yi, "y")] {
        if v < i32::MIN as i64 || v > i32::MAX as i64 {
            return Err(format!(
                "bad_coordinates: screen {name} out of i32 range (got {v}; allowed {}..={}); a \
                 cast would wrap and post at the wrong point, so nothing was posted",
                i32::MIN,
                i32::MAX
            ));
        }
    }
    unsafe {
        let mut pt = POINT {
            x: xi as i32,
            y: yi as i32,
        };
        if ScreenToClient(hwnd as HWND, &mut pt) != 0 {
            Ok((pt.x as i64, pt.y as i64))
        } else {
            Err("ScreenToClient failed: ScreenToClient returned FALSE".to_string())
        }
    }
}

fn bg_key_lparam(vk: u16, keyup: bool) -> LPARAM {
    unsafe {
        let sc = MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) & 0xFF;
        let mut lp: u32 = 1 | (sc << 16);
        if keyup {
            lp |= (1 << 30) | (1 << 31);
        }
        lp as i32 as isize
    }
}

fn bg_post(hwnd: i64, msg: u32, w: u64, l: LPARAM) -> bool {
    unsafe { PostMessageW(hwnd as HWND, msg, w as usize, l) != 0 }
}

/// Virtual keys whose key messages carry the extended bit (bit 24) in
/// lParam: navigation/edit block, Win keys, Apps, Divide, NumLock and the
/// right-hand Ctrl/Menu.
fn is_extended_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28 | 0x2D | 0x2E | 0x5B..=0x5D | 0x6F | 0x90 | 0xA3 | 0xA5
    )
}

/// lParam for one posted key message: repeat 1, scan code from
/// `MapVirtualKeyW`, extended bit for the extended set, context code when
/// Alt is held, previous-state + transition bits on key-up. Proven against
/// a live PotPlayer: Space down is `1 | (0x39 << 16)` = `0x00390001`,
/// Space up ORs bits 30/31 = `0xC0390001`.
fn posted_key_lparam(vk: u16, keyup: bool, alt: bool) -> LPARAM {
    let sc = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } & 0xFF;
    let mut lp: u32 = 1 | (sc << 16);
    if is_extended_vk(vk) {
        lp |= 1 << 24;
    }
    if alt {
        lp |= 1 << 29;
    }
    if keyup {
        lp |= (1 << 30) | (1 << 31);
    }
    lp as i32 as isize
}

/// Key-down half of one posted key: (msg, wparam, lparam) triples. Alt
/// travels as WM_SYSKEYDOWN/UP; a layout Shift wraps the key.
fn posted_key_downs(code: &KeyCode) -> Vec<(u32, u64, LPARAM)> {
    match code {
        KeyCode::Vk(vk, shift) => {
            let alt = *vk == 0x12;
            let dm = if alt { 0x0104 } else { 0x0100 };
            let mut v = Vec::new();
            if *shift {
                v.push((dm, VK_SHIFT as u64, posted_key_lparam(VK_SHIFT, false, alt)));
            }
            v.push((dm, *vk as u64, posted_key_lparam(*vk, false, alt)));
            v
        }
        KeyCode::Uni(ch) => utf16_units(&ch.to_string())
            .into_iter()
            .map(|u| (0x0102, u as u64, 1))
            .collect(),
    }
}

/// Key-up half of one posted key, in release order.
fn posted_key_ups(code: &KeyCode) -> Vec<(u32, u64, LPARAM)> {
    match code {
        KeyCode::Vk(vk, shift) => {
            let alt = *vk == 0x12;
            let um = if alt { 0x0105 } else { 0x0101 };
            let mut v = Vec::new();
            v.push((um, *vk as u64, posted_key_lparam(*vk, true, alt)));
            if *shift {
                v.push((um, VK_SHIFT as u64, posted_key_lparam(VK_SHIFT, true, alt)));
            }
            v
        }
        KeyCode::Uni(ch) => utf16_units(&ch.to_string())
            .into_iter()
            .map(|u| (0x0102, u as u64, 1))
            .collect(),
    }
}

/// Capture the locked target at native pixels for posted-key verify:
/// the same window lock, lossless PNG, no grid, no elements. A capture
/// failure is an error, never a guessed effect. One bounded retry rides
/// out a transient render race (a window that has not finished painting
/// reads back blank once); a persistent failure stays an error.
fn verify_capture(hwnd: i64) -> Result<crate::cap::Img, String> {
    match verify_capture_once(hwnd) {
        Ok(im) => Ok(im),
        Err(first) => {
            std::thread::sleep(std::time::Duration::from_millis(300));
            verify_capture_once(hwnd).map_err(|second| format!("{second} (first attempt: {first})"))
        }
    }
}

/// One verify capture attempt.
fn verify_capture_once(hwnd: i64) -> Result<crate::cap::Img, String> {
    let args = crate::screen::ShotArgs {
        display: -1,
        region: None,
        window: None,
        max_px: None,
        fmt: "png".to_string(),
        quality: None,
        grid: false,
        allow_crop_fallback: false,
        elements: false,
        out: None,
        target: Some(serde_json::json!({ "hwnd": hwnd })),
        privacy_masks: vec![],
    };
    let reply: Value = serde_json::from_str(&crate::screen::dispatch_screenshot(&args))
        .map_err(|e| format!("verify capture unparseable: {e}"))?;
    if reply.get("isError") == Some(&Value::from(true)) {
        let detail = reply
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_else(|| "capture failed".to_string());
        return Err(detail);
    }
    let b64 = reply
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|blocks| {
            blocks
                .iter()
                .find_map(|b| b.get("data").and_then(|d| d.as_str()))
        })
        .ok_or_else(|| "verify capture returned no image".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("verify capture is not base64: {e}"))?;
    crate::cap::decode_png(&bytes).map_err(|e| format!("verify capture: {e}"))
}

/// Class probe for text-input controls (classic EDIT plus RichEdit
/// flavors such as RichEditD2DPT backing the Win11 Notepad document).
fn bg_is_text_control(h: HWND) -> bool {
    unsafe {
        let mut buf = vec![0u16; 256];
        let n = GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return false;
        }
        let cls = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
        cls.contains("edit") || cls.contains("rich")
    }
}

/// Writable-text probe: a text-input control that is not read-only.
/// Typing into a read-only control is a silent no-op, so the descendant
/// walk prefers a control that can actually receive the text.
fn bg_is_writable_text_control(h: HWND) -> bool {
    if !bg_is_text_control(h) {
        return false;
    }
    unsafe {
        const GWL_STYLE: i32 = -16;
        const ES_READONLY: u32 = 0x0800;
        let style =
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(h, GWL_STYLE) as u32;
        style & ES_READONLY == 0
    }
}

fn bg_descend(h: HWND, out: &mut Vec<HWND>) {
    if let Ok(kids) = crate::enum_child_windows(h) {
        for k in kids {
            out.push(k);
            bg_descend(k, out);
        }
    }
}

/// Text-input target under background delivery (plain `background` and
/// `desktop:"hidden"` share `bg_execute`): keyboard messages posted to a
/// top-level frame are dispatched to the frame's own window proc, which for
/// editors (Notepad) never reaches the child EDIT control ÔÇö so the hidden
/// readback comes back empty. Resolve the control that would actually
/// receive typed text: the thread's focused control when it is itself
/// writable text input (GetGUIThreadInfo, the caret owner), else the first
/// visible + enabled + writable text control in the descendant tree (the
/// document), else the window itself. A bare focus window (tab strip,
/// toolbar) would swallow WM_CHAR, so focus only counts when it is text
/// input; a read-only control would swallow it silently, so writable wins.
/// The descendant walk is recursive because modern editors nest the document
/// deeper than one level. Never activates or focuses; PostMessage-only, so
/// the owner's desktop is untouched.
fn bg_text_target(hwnd: i64) -> i64 {
    unsafe {
        let target = hwnd as HWND;
        if IsWindow(target) == 0 {
            return hwnd;
        }
        let thread = GetWindowThreadProcessId(target, std::ptr::null_mut());
        if thread != 0 {
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(thread, &mut info) != 0 {
                let f = info.hwndFocus;
                if !f.is_null()
                    && f as i64 != hwnd
                    && IsChild(target, f) != 0
                    && bg_is_writable_text_control(f)
                {
                    return f as i64;
                }
            }
        }
        let mut descendants: Vec<HWND> = Vec::new();
        bg_descend(target, &mut descendants);
        for k in descendants {
            if IsWindowVisible(k) == 0 || IsWindowEnabled(k) == 0 {
                continue;
            }
            if bg_is_writable_text_control(k) {
                return k as i64;
            }
        }
        hwnd
    }
}

/// Posted input the target refuses: a typed `background_unavailable`, never
/// an escalation to foreground input.
const BG_POST_FAILED: &str = "background_unavailable: PostMessage failed (the target may have \
                              closed, or its message queue refuses messages); nothing escalated \
                              to foreground input";

/// Client point for a posted message. `client_coords` is the target-lock
/// convention (x/y are client-area physical pixels, used as given); false is
/// the legacy `background` convention (x/y are virtual-desktop screen pixels,
/// converted with ScreenToClient). Both end in the same exact integer pair.
fn bg_point(hwnd: i64, a: &Map<String, Value>, client_coords: bool) -> Result<(i64, i64), String> {
    let xv = a.get("x").ok_or_else(|| "needs x and y".to_string())?;
    let yv = a.get("y").ok_or_else(|| "needs x and y".to_string())?;
    let xi = py_int(xv).map_err(|e| format!("bad x: {e}"))?;
    let yi = py_int(yv).map_err(|e| format!("bad y: {e}"))?;
    let (cx, cy) = if client_coords {
        (xi, yi)
    } else {
        bg_client(hwnd, xv, yv)?
    };
    msg_coords(cx, cy)
}

fn bg_buttons(btn: &str) -> Option<(u32, u32, u64)> {
    match btn {
        "left" => Some((0x0201, 0x0202, 0x0001)),
        "right" => Some((0x0204, 0x0205, 0x0002)),
        "middle" => Some((0x0207, 0x0208, 0x0010)),
        _ => None,
    }
}

fn bg_btn_name(a: &Map<String, Value>) -> String {
    match a.get("button") {
        None => "left".to_string(),
        Some(v) => py_str(v).to_lowercase(),
    }
}

fn bg_btn_raw(a: &Map<String, Value>) -> String {
    match a.get("button") {
        None => "None".to_string(),
        Some(v) => py_str(v),
    }
}

/// Returns (ok, error, posted_count). Never activates/focuses/raises.
fn bg_execute(
    hwnd: i64,
    a: &Map<String, Value>,
    client_coords: bool,
    allow_keyboard: bool,
) -> (bool, Option<String>, i64) {
    let typ = a.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let hover = matches!(a.get("hover"), Some(v) if py_truthy(v));
    if typ == "click" {
        let btn = bg_btn_name(a);
        let Some((dn, up, held)) = bg_buttons(&btn) else {
            return (
                false,
                Some(format!("unknown button \"{}\"", bg_btn_raw(a))),
                0,
            );
        };
        if !a.contains_key("x") || !a.contains_key("y") {
            return (false, Some("click needs x and y".to_string()), 0);
        }
        let count = match opt_py_int(a, "count", 1) {
            Ok(c) => c.max(1),
            Err(_) => return (false, Some("click count must be an int".to_string()), 0),
        };
        if count > MAX_CLICK_COUNT {
            return (
                false,
                Some(format!("click count absurd (>{MAX_CLICK_COUNT}): {count}")),
                0,
            );
        }
        let (cx, cy) = match bg_point(hwnd, a, client_coords) {
            Ok(p) => p,
            Err(e) => return (false, Some(e), 0),
        };
        let lp = bg_lparam(cx, cy);
        let mut n = 0i64;
        if hover {
            if !bg_post(hwnd, 0x0200, 0, lp) {
                return (false, Some(BG_POST_FAILED.to_string()), n);
            }
            n += 1;
        }
        for _ in 0..count {
            if !bg_post(hwnd, dn, held, lp) {
                return (false, Some(BG_POST_FAILED.to_string()), n);
            }
            n += 1;
            if !bg_post(hwnd, up, 0, lp) {
                return (false, Some(BG_POST_FAILED.to_string()), n);
            }
            n += 1;
        }
        return (true, None, n);
    }
    if typ == "down" || typ == "up" {
        let btn = bg_btn_name(a);
        let Some((dn, up, held)) = bg_buttons(&btn) else {
            return (
                false,
                Some(format!("unknown button \"{}\"", bg_btn_raw(a))),
                0,
            );
        };
        let lp = match bg_point(hwnd, a, client_coords) {
            Ok((cx, cy)) => bg_lparam(cx, cy),
            Err(e) => return (false, Some(e), 0),
        };
        let msg = if typ == "down" { dn } else { up };
        let w = if typ == "down" { held } else { 0 };
        if !bg_post(hwnd, msg, w, lp) {
            return (false, Some(BG_POST_FAILED.to_string()), 0);
        }
        return (true, None, 1);
    }
    if typ == "drag" {
        let btn = bg_btn_name(a);
        let Some((dn, up, held)) = bg_buttons(&btn) else {
            return (
                false,
                Some(format!("unknown button \"{}\"", bg_btn_raw(a))),
                0,
            );
        };
        let pt = |key: &str| -> Result<(i64, i64), String> {
            let v = a.get(key).ok_or_else(|| format!("drag needs {key}"))?;
            match v {
                Value::Array(arr) if arr.len() == 2 => {
                    let mut m = Map::new();
                    m.insert("x".to_string(), arr[0].clone());
                    m.insert("y".to_string(), arr[1].clone());
                    bg_point(hwnd, &m, client_coords)
                }
                _ => Err(format!("drag {key} must be [x,y]")),
            }
        };
        let (fx, fy) = match pt("from") {
            Ok(p) => p,
            Err(e) => return (false, Some(e), 0),
        };
        let (tx, ty) = match pt("to") {
            Ok(p) => p,
            Err(e) => return (false, Some(e), 0),
        };
        let from = bg_lparam(fx, fy);
        let to = bg_lparam(tx, ty);
        let mut n = 0i64;
        for (msg, w, lp) in [(dn, held, from), (0x0200u32, 0u64, to), (up, 0u64, to)] {
            if !bg_post(hwnd, msg, w, lp) {
                return (false, Some(BG_POST_FAILED.to_string()), n);
            }
            n += 1;
        }
        return (true, None, n);
    }
    if typ == "scroll" {
        let amount = match opt_py_int(a, "amount", 1) {
            Ok(v) => v,
            Err(e) => return (false, Some(e), 0),
        };
        let axis = match a.get("axis") {
            None | Some(Value::Null) => "v".to_string(),
            Some(v) => py_str(v).to_lowercase(),
        };
        let (msg, delta) = match scroll_wheel(&axis, amount) {
            Ok((flag, data)) => (
                if flag == M_WHEEL {
                    0x020Au32
                } else {
                    0x020Eu32
                },
                data as u16 as u64,
            ),
            Err(e) => return (false, Some(e), 0),
        };
        let (cx, cy) = match bg_point(hwnd, a, client_coords) {
            Ok(p) => p,
            Err(e) => return (false, Some(e), 0),
        };
        let (sx, sy) = if client_coords {
            match crate::target::client_point_to_screen(hwnd, cx, cy) {
                Some(p) => p,
                None => return (false, Some("ClientToScreen failed".to_string()), 0),
            }
        } else {
            (cx, cy)
        };
        let (sx, sy) = match msg_coords(sx, sy) {
            Ok(p) => p,
            Err(e) => return (false, Some(e), 0),
        };
        let lp = bg_lparam(sx, sy);
        if !bg_post(hwnd, msg, delta << 16, lp) {
            return (false, Some(BG_POST_FAILED.to_string()), 0);
        }
        return (true, None, 1);
    }
    if matches!(typ, "type" | "key" | "hotkey") && !allow_keyboard {
        return (
            false,
            Some(
                "no_foreground_policy: keyboard input is never posted to the owner's desktop \
                 (WM_CHAR/WM_KEYDOWN bypass the input queue and are ignored by many controls), \
                 and no flag buys a foreground act: this cannot be done under the background-only \
                 policy. type with an element works only on the server's own hidden desktop. \
                 Nothing was posted."
                    .to_string(),
            ),
            0,
        );
    }
    if typ == "type" {
        let text = match a.get("text") {
            None => String::new(),
            Some(v) => py_str(v),
        };
        let tgt = bg_text_target(hwnd);
        let mut n = 0i64;
        for unit in utf16_units(&text) {
            if !bg_post(tgt, 0x0102, unit as u64, 1) {
                return (false, Some(BG_POST_FAILED.to_string()), n);
            }
            n += 1;
        }
        return (true, None, n);
    }
    if typ == "key" {
        let name = match a.get("name") {
            None => String::new(),
            Some(v) => py_str(v),
        };
        let code = match vk_of(&name) {
            Ok(v) => v,
            Err(e) => return (false, Some(e), 0),
        };
        let tgt = bg_text_target(hwnd);
        match code {
            KeyCode::Vk(vk, shift) => {
                let (dm, um) = if vk == 0x12 {
                    (0x0104, 0x0105)
                } else {
                    (0x0100, 0x0101)
                };
                let mut n = 0i64;
                if shift && !bg_post(tgt, dm, VK_SHIFT as u64, bg_key_lparam(VK_SHIFT, false)) {
                    return (false, Some(BG_POST_FAILED.to_string()), n);
                }
                if shift {
                    n += 1;
                }
                if !bg_post(tgt, dm, vk as u64, bg_key_lparam(vk, false)) {
                    return (false, Some(BG_POST_FAILED.to_string()), n);
                }
                n += 1;
                if !bg_post(tgt, um, vk as u64, bg_key_lparam(vk, true)) {
                    return (false, Some(BG_POST_FAILED.to_string()), n);
                }
                n += 1;
                if shift && !bg_post(tgt, um, VK_SHIFT as u64, bg_key_lparam(VK_SHIFT, true)) {
                    return (false, Some(BG_POST_FAILED.to_string()), n);
                }
                if shift {
                    n += 1;
                }
                return (true, None, n);
            }
            KeyCode::Uni(ch) => {
                let mut n = 0i64;
                for unit in utf16_units(&ch.to_string()) {
                    if !bg_post(tgt, 0x0102, unit as u64, 1) {
                        return (false, Some(BG_POST_FAILED.to_string()), n);
                    }
                    n += 1;
                }
                return (true, None, n);
            }
        }
    }
    if typ == "hotkey" {
        let keys: Result<Vec<String>, String> = match a.get("keys") {
            None => Ok(vec![]),
            Some(Value::Array(arr)) => Ok(arr.iter().map(py_str).collect()),
            Some(Value::String(s)) => Ok(s.chars().map(|c| c.to_string()).collect()),
            Some(v) => Err(format!("'{}' object is not iterable", py_type_name(v))),
        };
        let keys = match keys {
            Ok(k) => k,
            Err(e) => return (false, Some(e), 0),
        };
        if keys.is_empty() {
            return (false, Some("hotkey needs non-empty keys".to_string()), 0);
        }
        let mut codes = Vec::with_capacity(keys.len());
        for k in &keys {
            match vk_of(k) {
                Ok(v) => codes.push(v),
                Err(e) => return (false, Some(e), 0),
            }
        }
        let has_alt = codes
            .iter()
            .any(|c| matches!(c, KeyCode::Vk(vk, _) if *vk == 0x12));
        let (dm, um) = if has_alt {
            (0x0104, 0x0105)
        } else {
            (0x0100, 0x0101)
        };
        let mut n = 0i64;
        let tgt = bg_text_target(hwnd);
        for c in &codes {
            match c {
                KeyCode::Vk(vk, shift) => {
                    if *shift {
                        if !bg_post(tgt, dm, VK_SHIFT as u64, bg_key_lparam(VK_SHIFT, false)) {
                            return (false, Some(BG_POST_FAILED.to_string()), n);
                        }
                        n += 1;
                    }
                    if !bg_post(tgt, dm, *vk as u64, bg_key_lparam(*vk, false)) {
                        return (false, Some(BG_POST_FAILED.to_string()), n);
                    }
                    n += 1;
                }
                KeyCode::Uni(ch) => {
                    for unit in utf16_units(&ch.to_string()) {
                        if !bg_post(tgt, 0x0102, unit as u64, 1) {
                            return (false, Some(BG_POST_FAILED.to_string()), n);
                        }
                        n += 1;
                    }
                }
            }
        }
        for c in codes.iter().rev() {
            match c {
                KeyCode::Vk(vk, shift) => {
                    if !bg_post(tgt, um, *vk as u64, bg_key_lparam(*vk, true)) {
                        return (false, Some(BG_POST_FAILED.to_string()), n);
                    }
                    n += 1;
                    if *shift {
                        if !bg_post(tgt, um, VK_SHIFT as u64, bg_key_lparam(VK_SHIFT, true)) {
                            return (false, Some(BG_POST_FAILED.to_string()), n);
                        }
                        n += 1;
                    }
                }
                KeyCode::Uni(ch) => {
                    for unit in utf16_units(&ch.to_string()) {
                        if !bg_post(tgt, 0x0102, unit as u64, 1) {
                            return (false, Some(BG_POST_FAILED.to_string()), n);
                        }
                        n += 1;
                    }
                }
            }
        }
        return (true, None, n);
    }
    (
        false,
        Some(format!(
            "unknown action type \"{typ}\" for background delivery"
        )),
        0,
    )
}

/// Pure element-wait decision: given whether a matching control was found
/// and (when found) whether it is enabled, decide per `state`. Unit-tested;
/// the polling loop only feeds it live observations.
fn element_wait_satisfied(state: &str, found: bool, enabled: Option<bool>) -> bool {
    match state {
        "disappear" => !found,
        "appear" => found,
        "enabled" => found && enabled == Some(true),
        "disabled" => found && enabled == Some(false),
        _ => false,
    }
}

/// Enabled state for a matched entry: UIA CurrentIsEnabled by path on the
/// uia backend, Win32 IsWindowEnabled on the entry hwnd otherwise. None on
/// any failure (unreadable, unhosted, stale) ÔÇö the waiter treats that as
/// unsatisfied and keeps polling rather than failing the burst.
fn wf_element_enabled(tgt: i64, backend: &str, entry: &Map<String, Value>) -> Option<bool> {
    if backend == "uia" {
        let path = entry_str(entry, "path");
        crate::uia::uia_is_enabled(tgt as HWND, &path).ok()
    } else {
        let h = entry.get("hwnd").and_then(|v| v.as_i64()).unwrap_or(0);
        if h == 0 {
            return None;
        }
        unsafe { Some(IsWindowEnabled(h as HWND) != 0) }
    }
}

/// W2.3: is the control about to be reported still the one the selector
/// matches, with the same identity? The enabled read and the reported entry
/// are two separate UIA reads, so without this a control replaced between them
/// is reported as the winner while its successor satisfied the condition. One
/// fresh paged scan makes the report name a control that existed when it was
/// named; the scan pages exactly like the polling loop, so a control past the
/// first page is confirmed where it was found.
fn wf_winner_current(hwnd: HWND, elem: &Value, winner: &Map<String, Value>) -> bool {
    let mut offset = 0usize;
    loop {
        let model = element_model(hwnd, crate::UIA_DEPTH_LIMIT, offset);
        if let Some(e) = match_element(&model.entries, Some(elem), None).0 {
            return entry_str(&e, "fp") == entry_str(winner, "fp");
        }
        match model.next_offset {
            Some(n) => offset = n,
            None => return false,
        }
    }
}

fn wf_done(t0: &Instant, ok: bool, sat: bool, err: Option<String>) -> Map<String, Value> {
    let mut d = Map::new();
    d.insert("ok".to_string(), Value::from(ok));
    d.insert("satisfied".to_string(), Value::from(sat));
    d.insert(
        "waited_ms".to_string(),
        Value::from(py_round_n(elapsed_ms(t0), 3)),
    );
    if let Some(e) = err {
        d.insert("error".to_string(), Value::from(e));
    }
    d
}

/// `wf_done` for an element wait, which is the only kind that reports which
/// control satisfied it (W2.3).
///
/// `satisfied_by` is the winner ÔÇö `{selector, fp, path}` for a satisfied wait
/// that found a control, `null` when nothing won (a timeout, or a `disappear`
/// satisfied by the control being gone). A caller that passed `expect_fp` also
/// gets `fp_matched`, and a mismatch is a report, never an error: a wait has to
/// survive a churning UI, so it says what it found instead of refusing.
fn wf_done_element(
    t0: &Instant,
    ok: bool,
    sat: bool,
    by: Option<Map<String, Value>>,
    err: Option<String>,
) -> Map<String, Value> {
    let mut d = wf_done(t0, ok, sat, err);
    d.insert(
        "satisfied_by".to_string(),
        by.map(Value::Object).unwrap_or(Value::Null),
    );
    d
}

/// The control that satisfied an element wait, as `satisfied_by` reports it.
fn wf_satisfied_by(selector: &str, entry: &Map<String, Value>) -> Map<String, Value> {
    let mut by = Map::new();
    by.insert("selector".to_string(), Value::from(selector));
    by.insert("fp".to_string(), Value::from(entry_str(entry, "fp")));
    by.insert("path".to_string(), Value::from(entry_str(entry, "path")));
    by
}

fn wf_nap(t0: &Instant, timeout_s: f64, poll_s: f64) -> bool {
    let remain = timeout_s - t0.elapsed().as_secs_f64();
    let nap = if remain > 0.001 {
        poll_s.min(remain)
    } else {
        0.001
    };
    let mut left = nap.max(0.0);
    while left > 0.0 {
        if crate::cancel::cancelled() {
            return true;
        }
        let s = left.min(0.05);
        std::thread::sleep(Duration::from_secs_f64(s));
        left -= s;
    }
    crate::cancel::cancelled()
}

fn wait_for(a: &Map<String, Value>, t0: &Instant) -> Map<String, Value> {
    let kind = match a.get("kind") {
        None => String::new(),
        Some(v) => py_str(v),
    };
    let (timeout_ms, poll_ms) = match (
        opt_py_int(a, "timeout_ms", 5000),
        opt_py_int(a, "poll_ms", 50),
    ) {
        (Ok(t), Ok(p)) => (t, p),
        _ => {
            return wf_done(
                t0,
                false,
                false,
                Some("timeout_ms/poll_ms must be ints".to_string()),
            );
        }
    };
    if timeout_ms < 0 || poll_ms <= 0 {
        return wf_done(
            t0,
            false,
            false,
            Some("timeout_ms must be >= 0 and poll_ms > 0".to_string()),
        );
    }
    if timeout_ms > 300000 {
        return wf_done(
            t0,
            false,
            false,
            Some(format!("timeout_ms absurd (>300000): {timeout_ms}")),
        );
    }
    let timeout_s = timeout_ms as f64 / 1000.0;
    let poll_s = poll_ms as f64 / 1000.0;
    if kind == "window_appear" || kind == "window_disappear" {
        let raw_sel: Option<&Value> = match a.get("selector") {
            Some(v) => Some(v),
            None => a.get("title"),
        };
        let mut sel: Option<String> = None;
        if let Some(v) = raw_sel {
            if py_truthy(v) {
                sel = Some(py_str(v));
            }
        }
        if sel.is_none() {
            if let Some(h) = a.get("hwnd") {
                sel = Some(format!("hwnd:{}", py_str(h)));
            }
        }
        let sel = match sel {
            Some(s) if !s.is_empty() => s,
            _ => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some(format!("wait_for kind={kind} needs \"selector\"")),
                );
            }
        };
        let want_present = kind == "window_appear";
        loop {
            if crate::cancel::cancelled() {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
            let (h, _, _) = resolve_selector(&sel);
            if h.is_some() == want_present {
                return wf_done(t0, true, true, None);
            }
            if t0.elapsed().as_secs_f64() >= timeout_s {
                return wf_done(t0, true, false, None);
            }
            if wf_nap(t0, timeout_s, poll_s) {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
        }
    }
    if kind == "pixel" {
        let parsed: Result<(i64, i64, [i64; 3], i64), ()> = (|| {
            let x = py_int(a.get("x").unwrap_or(&Value::Null)).map_err(|_| ())?;
            let y = py_int(a.get("y").unwrap_or(&Value::Null)).map_err(|_| ())?;
            let color = match a.get("color") {
                Some(Value::Array(c)) if c.len() == 3 => {
                    let mut rgb = [0i64; 3];
                    for (k, v) in c.iter().enumerate() {
                        rgb[k] = py_int(v).map_err(|_| ())?;
                    }
                    rgb
                }
                _ => return Err(()),
            };
            let tol = match a.get("tolerance") {
                None => 12,
                Some(v) => py_int(v).map_err(|_| ())?,
            };
            Ok((x, y, color, tol))
        })();
        let (x, y, color, tol) = match parsed {
            Ok(p) => p,
            Err(_) => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some(
                        "wait_for kind=pixel needs x, y, color:[r,g,b] (ints) and optional tolerance"
                            .to_string(),
                    ),
                );
            }
        };
        if color.iter().any(|v| *v < 0 || *v > 255) || tol < 0 {
            return wf_done(
                t0,
                false,
                false,
                Some("color values must be 0-255 and tolerance >= 0".to_string()),
            );
        }
        loop {
            if crate::cancel::cancelled() {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
            if let Ok(img) = bitblt_grab(x as i32, y as i32, 1, 1) {
                if img.px.len() >= 3 && (0..3).all(|k| (img.px[k] as i64 - color[k]).abs() <= tol) {
                    return wf_done(t0, true, true, None);
                }
            }
            if t0.elapsed().as_secs_f64() >= timeout_s {
                return wf_done(t0, true, false, None);
            }
            if wf_nap(t0, timeout_s, poll_s) {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
        }
    }
    if kind == "stable" {
        let region = match a.get("region") {
            None => default_stable_region(),
            Some(Value::Array(r)) if r.len() == 4 => {
                let mut out = [0i64; 4];
                let mut ok = true;
                for (k, v) in r.iter().enumerate() {
                    match py_int(v) {
                        Ok(n) => out[k] = n,
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    return wf_done(
                        t0,
                        false,
                        false,
                        Some("region must be [left,top,right,bottom] ints".to_string()),
                    );
                }
                out.to_vec()
            }
            _ => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some("region must be [left,top,right,bottom] ints".to_string()),
                );
            }
        };
        let (rl, rt, rr, rb) = (region[0], region[1], region[2], region[3]);
        let (rw, rh) = match crate::validate_region(rl, rt, rr, rb, MAX_STABLE_EDGE) {
            Ok(wh) => wh,
            Err(crate::RegionReject::Order) => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some("region must have right>left and bottom>top".to_string()),
                );
            }
            Err(crate::RegionReject::Extreme) => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some(
                        "region coordinates outside i32 range (would truncate on cast)".to_string(),
                    ),
                );
            }
            Err(crate::RegionReject::OverCap(w, h)) => {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some(format!(
                        "region must be at most {MAX_STABLE_EDGE}x{MAX_STABLE_EDGE} (got {w}x{h})"
                    )),
                );
            }
        };
        let mut prev: Option<Vec<u8>> = None;
        loop {
            if crate::cancel::cancelled() {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
            match bitblt_grab(rl as i32, rt as i32, rw as i32, rh as i32) {
                Ok(img) => {
                    if prev.as_ref() == Some(&img.px) {
                        return wf_done(t0, true, true, None);
                    }
                    prev = Some(img.px);
                }
                Err(_) => {
                    prev = None;
                }
            }
            if t0.elapsed().as_secs_f64() >= timeout_s {
                return wf_done(t0, true, false, None);
            }
            if wf_nap(t0, timeout_s, poll_s) {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
        }
    }
    if kind == "element" {
        let win_sel: Option<String> = match a.get("window") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = py_str(v);
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        };
        let elem_v = a.get("element").cloned().unwrap_or(Value::Null);
        let want_fp: Option<String> = match a.get("expect_fp") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = py_str(v);
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        };
        let done = |sat: bool, by: Option<Map<String, Value>>| {
            let mut d = wf_done_element(t0, true, sat, by.clone(), None);
            if let Some(want) = want_fp.as_deref() {
                let matched = by
                    .as_ref()
                    .and_then(|b| b.get("fp"))
                    .and_then(|v| v.as_str())
                    == Some(want);
                d.insert("fp_matched".to_string(), Value::from(matched));
            }
            d
        };
        if !py_truthy(&elem_v) || py_str(&elem_v).trim().is_empty() {
            return wf_done(
                t0,
                false,
                false,
                Some(
                    "wait_for kind=element needs \"element\" (e.g. \"name:OK\", \"path:0\")"
                        .to_string(),
                ),
            );
        }
        let elem_s = py_str(&elem_v);
        let (ekind, _) = match elem_s.find(':') {
            Some(p) => (
                elem_s[..p].trim().to_lowercase(),
                elem_s[p + 1..].trim().to_string(),
            ),
            None => ("name".to_string(), elem_s.trim().to_string()),
        };
        if !["name", "auto_id", "autoid", "automationid", "id", "path"].contains(&ekind.as_str()) {
            return wf_done(
                t0,
                false,
                false,
                Some(format!(
                    "unknown element selector kind \"{ekind}\" (want name:|auto_id:|path:)"
                )),
            );
        }
        let state = match a.get("state") {
            None | Some(Value::Null) => "appear".to_string(),
            Some(v) => py_str(v).to_lowercase(),
        };
        if !["appear", "disappear", "enabled", "disabled"].contains(&state.as_str()) {
            return wf_done(
                t0,
                false,
                false,
                Some(
                    "wait_for kind=element state must be appear|disappear|enabled|disabled"
                        .to_string(),
                ),
            );
        }
        if let Some(sel) = win_sel.as_deref() {
            let (kind, val) = crate::parse_selector(sel);
            let bad = match kind.as_str() {
                "hwnd" | "pid" => crate::parse_int_loose(&val).is_none(),
                "exe" | "title" | "class" => val.is_empty(),
                _ => false,
            };
            if bad {
                return wf_done(
                    t0,
                    false,
                    false,
                    Some(format!(
                        "wait_for kind=element: bad window selector {sel:?}"
                    )),
                );
            }
        }
        loop {
            if crate::cancel::cancelled() {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
            let tgt: Option<HWND> = match win_sel.as_deref() {
                Some(sel) => resolve_selector(sel).0,
                None => {
                    let f = fg_now();
                    if f == 0 {
                        return wf_done(
                            t0,
                            true,
                            false,
                            Some("wait_for kind=element: no foreground window".to_string()),
                        );
                    }
                    Some(f as HWND)
                }
            };
            let alive = tgt.is_some_and(|h| unsafe { IsWindow(h) != 0 });
            if !alive {
                if state == "disappear" {
                    return done(true, None);
                }
            } else {
                let h = tgt.unwrap() as i64;
                let mut hits: Vec<(Map<String, Value>, String)> = Vec::new();
                let mut refused: Option<String> = None;
                let mut offset = 0usize;
                loop {
                    if crate::cancel::cancelled() {
                        return wf_done(t0, false, false, Some("cancelled".to_string()));
                    }
                    let model = element_model(h as HWND, crate::UIA_DEPTH_LIMIT, offset);
                    let (entry, merr, mcands) = match_element(&model.entries, Some(&elem_v), None);
                    if let Some(me) = merr {
                        if mcands.is_some() || me.contains("unknown element selector kind") {
                            refused = Some(me);
                            break;
                        }
                    } else if let Some(e) = entry {
                        if hits.is_empty() {
                            hits.push((e, model.backend));
                        } else {
                            refused = Some(format!(
                                "ambiguous element: multiple elements match \"{elem_s}\" across pages"
                            ));
                            break;
                        }
                    }
                    match model.next_offset {
                        Some(n) => offset = n,
                        None => break,
                    }
                }
                if let Some(me) = refused {
                    return wf_done(t0, false, false, Some(me));
                }
                match hits.pop() {
                    None => {
                        if state == "disappear" {
                            return done(true, None);
                        }
                    }
                    Some((e, backend)) => {
                        let enabled = if state == "enabled" || state == "disabled" {
                            wf_element_enabled(h, &backend, &e)
                        } else {
                            None
                        };
                        if element_wait_satisfied(&state, true, enabled) {
                            let current =
                                enabled.is_none() || wf_winner_current(h as HWND, &elem_v, &e);
                            if current {
                                return done(true, Some(wf_satisfied_by(&elem_s, &e)));
                            }
                        }
                    }
                }
            }
            if t0.elapsed().as_secs_f64() >= timeout_s {
                return done(false, None);
            }
            if wf_nap(t0, timeout_s, poll_s) {
                return wf_done(t0, false, false, Some("cancelled".to_string()));
            }
        }
    }
    wf_done(
        t0,
        false,
        false,
        Some(format!(
            "unknown wait_for kind \"{kind}\" (want window_appear|window_disappear|pixel|stable|element)"
        )),
    )
}

fn cursor_now() -> (i64, i64) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut pt) != 0 {
            (pt.x as i64, pt.y as i64)
        } else {
            (-1, -1)
        }
    }
}

fn fg_now() -> i64 {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            0
        } else {
            h as i64
        }
    }
}

/// Element-match outcome: the entry, a typed error, or disambiguation
/// candidates when the selector matches several controls.
type MatchOut = (
    Option<Map<String, Value>>,
    Option<String>,
    Option<Vec<Map<String, Value>>>,
);

/// `focus_window`'s legacy `title`/`hwnd` arguments resolved to the SAME
/// `TargetLock` every other verb uses. A bare hwnd is a recyclable handle:
/// `IsWindow` on it can be true for a *different* window than the caller
/// meant, so focus goes through the lock (pid + process creation time +
/// image + class) and is revalidated immediately before the focus call ÔÇö
/// the same seam `screenshot`'s legacy `window:` selector uses.
fn focus_lock(
    title: Option<&Value>,
    hwnd: Option<&Value>,
) -> Result<crate::target::TargetLock, crate::target::TargetError> {
    let h = match hwnd {
        None | Some(Value::Null) => None,
        Some(v) => Some(py_int(v).map_err(|e| crate::target::TargetError {
            code: crate::target::BAD,
            message: format!("bad_window: bad hwnd {}: {e}", py_str(v)),
            candidates: None,
        })?),
    };
    let t = title.filter(|v| py_truthy(v)).map(py_str);
    crate::cap::lock_legacy(h, t.as_deref())
}

/// A `focus_window` refusal in the per-action shape: the target error's own
/// fields (`code`, `error`, `candidates`) inside the action envelope.
fn target_failure(
    i: usize,
    typ_v: &Value,
    t0: &Instant,
    e: &crate::target::TargetError,
) -> Map<String, Value> {
    let mut d = base_entry(i, typ_v, false, elapsed_ms(t0));
    for (k, v) in e.to_map() {
        if k != "ok" {
            d.insert(k, v);
        }
    }
    d.insert("via".to_string(), Value::from("none"));
    d
}

fn entry_str(e: &Map<String, Value>, key: &str) -> String {
    match e.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => py_str(v),
        None => String::new(),
    }
}

fn element_public(e: &Map<String, Value>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("name".to_string(), Value::from(entry_str(e, "name")));
    m.insert("role".to_string(), Value::from(entry_str(e, "role")));
    m.insert("auto_id".to_string(), Value::from(entry_str(e, "auto_id")));
    m.insert("ref".to_string(), Value::from(entry_str(e, "ref")));
    m.insert("fp".to_string(), Value::from(entry_str(e, "fp")));
    m.insert(
        "rect".to_string(),
        e.get("rect")
            .cloned()
            .unwrap_or(Value::from(vec![0, 0, 0, 0])),
    );
    m
}

fn match_element(
    entries: &[Map<String, Value>],
    spec: Option<&Value>,
    index: Option<&Value>,
) -> MatchOut {
    let s = match spec {
        None => String::new(),
        Some(v) => {
            if py_truthy(v) {
                py_str(v)
            } else {
                String::new()
            }
        }
    };
    if s.trim().is_empty() {
        return (
            None,
            Some("invoke needs \"element\" (e.g. \"name:OK\")".to_string()),
            None,
        );
    }
    let (kind, val) = match s.find(':') {
        Some(p) => (s[..p].trim().to_lowercase(), s[p + 1..].trim().to_string()),
        None => ("name".to_string(), s.trim().to_string()),
    };
    let hits: Vec<Map<String, Value>> = if kind == "path" {
        entries
            .iter()
            .filter(|e| entry_str(e, "path") == val)
            .cloned()
            .collect()
    } else if ["auto_id", "autoid", "automationid", "id"].contains(&kind.as_str()) {
        entries
            .iter()
            .filter(|e| entry_str(e, "auto_id") == val)
            .cloned()
            .collect()
    } else if kind == "name" {
        let exact: Vec<Map<String, Value>> = entries
            .iter()
            .filter(|e| entry_str(e, "name") == val)
            .cloned()
            .collect();
        if exact.is_empty() {
            let low = val.to_lowercase();
            entries
                .iter()
                .filter(|e| entry_str(e, "name").to_lowercase().contains(&low))
                .cloned()
                .collect()
        } else {
            exact
        }
    } else {
        return (
            None,
            Some(format!(
                "unknown element selector kind \"{kind}\" (want name:|auto_id:|path:)"
            )),
            None,
        );
    };
    if hits.is_empty() {
        return (
            None,
            Some(format!(
                "no element matching \"{s}\" ({} elements listed)",
                entries.len()
            )),
            None,
        );
    }
    if index.is_none() {
        if hits.len() > 1 {
            let cands: Vec<Map<String, Value>> = hits.iter().map(element_public).collect();
            return (
                None,
                Some(format!(
                    "ambiguous element: {} elements match \"{s}\" (pass \"index\" 0..{} to choose)",
                    hits.len(),
                    hits.len() - 1
                )),
                Some(cands),
            );
        }
        return (Some(hits[0].clone()), None, None);
    }
    let n = match py_int(index.unwrap()) {
        Ok(n) => n,
        Err(_) => return (None, Some("element index must be an int".to_string()), None),
    };
    if n < 0 || n >= hits.len() as i64 {
        return (
            None,
            Some(format!(
                "element index {n} out of range: {} element(s) match \"{s}\"",
                hits.len()
            )),
            None,
        );
    }
    (Some(hits[n as usize].clone()), None, None)
}

fn element_pub_full(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut m = Map::new();
    for k in [
        "win",
        "role",
        "name",
        "auto_id",
        "rect",
        "offscreen",
        "invokable",
        "path",
        "fp",
    ] {
        if let Some(v) = entry.get(k) {
            m.insert(k.to_string(), v.clone());
        }
    }
    m
}

/// Why an element rect cannot become an injection point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RectRefusal {
    /// No area: the control is scrolled away, virtualized or collapsed, so
    /// its "centre" would be the target's client origin.
    Offscreen,
    /// Real geometry, but `ScreenToClient` failed (window moved or closed).
    Unconvertible,
}

/// The rect as `(left, top, width, height)`; a missing or short field reads 0.
fn element_rect(entry: &Map<String, Value>) -> (i64, i64, i64, i64) {
    let rect: Vec<i64> = match entry.get("rect") {
        Some(Value::Array(r)) => r.iter().map(|v| v.as_i64().unwrap_or(0)).collect(),
        _ => Vec::new(),
    };
    (
        rect.first().copied().unwrap_or(0),
        rect.get(1).copied().unwrap_or(0),
        rect.get(2).copied().unwrap_or(0),
        rect.get(3).copied().unwrap_or(0),
    )
}

/// The ONE gate between an element's rect and a pixel the server may inject
/// into, in SCREEN pixels. UIA reports `[0,0,0,0]` for a control with no
/// on-screen rectangle; `left + 0/2, top + 0/2` is then a real screen point ÔÇö
/// the target's client origin ÔÇö so a click posted at that "centre" lands on
/// the window frame instead of the control. Every verb that turns an element
/// rect into an injection point converts through here, so the guard cannot be
/// forgotten by a verb added later.
pub(crate) fn element_rect_centre(entry: &Map<String, Value>) -> Result<(i64, i64), RectRefusal> {
    let (rl, rt, rw, rh) = element_rect(entry);
    if rw <= 0 || rh <= 0 {
        return Err(RectRefusal::Offscreen);
    }
    Ok((rl + rw / 2, rt + rh / 2))
}

/// `element_rect_centre` plus the documented target convention: the rect is
/// SCREEN pixels and the click point is CLIENT pixels (physical).
pub(crate) fn element_centre_client(
    target_hwnd: i64,
    entry: &Map<String, Value>,
) -> Result<(i64, i64), RectRefusal> {
    let (sx, sy) = element_rect_centre(entry)?;
    crate::target::screen_point_to_client(target_hwnd, sx, sy).ok_or(RectRefusal::Unconvertible)
}

/// The typed refusal text for a rect that cannot be aimed at, flag and
/// recovery named (never a silent skip, never a guess).
pub(crate) fn rect_refusal_message(entry: &Map<String, Value>, r: RectRefusal) -> String {
    match r {
        RectRefusal::Offscreen => {
            let (rl, rt, rw, rh) = element_rect(entry);
            format!(
                "offscreen_element: the matched control has no on-screen rectangle \
                 (rect [{rl},{rt},{rw},{rh}]), so it has no pixel to click; scroll it into \
                 view, or use invoke, which acts through the UIA pattern and needs no \
                 geometry. Nothing was sent."
            )
        }
        RectRefusal::Unconvertible => "element rect is not convertible to client pixels \
                                        (window moved or closed mid-call)"
            .to_string(),
    }
}

/// W2.1: an element mutation with no fingerprint. Typed, and carrying the
/// fresh value so the retry costs one call.
const EXPECT_FP_REQUIRED: &str = "expect_fp_required";

/// One typed refusal from the fingerprint gate.
struct FpRefusal {
    code: &'static str,
    resolution: &'static str,
    text: String,
}

/// The re-walked entry's fingerprint must equal `expect_fp`. Returns the typed
/// refusal when it does not, None when the gate passes. `required` refuses an
/// absent fingerprint too: a mutation without one may act on a different
/// control than the one the caller saw (W2.1). Applies live and under dry_run
/// alike: a stale ref never acts.
fn check_expect_fp(
    a: &Map<String, Value>,
    entry: &Map<String, Value>,
    sel: &str,
    required: bool,
) -> Option<FpRefusal> {
    let got = entry_str(entry, "fp");
    let want = match a.get("expect_fp") {
        None | Some(Value::Null) => String::new(),
        Some(v) => py_str(v),
    };
    if want.trim().is_empty() {
        if !required {
            return None;
        }
        return Some(FpRefusal {
            code: EXPECT_FP_REQUIRED,
            resolution: "expect_fp_required",
            text: format!(
                "expect_fp_required: element \"{sel}\" was resolved fresh (fp {got}). Pass expect_fp \
                 with the fingerprint from your elements snapshot; a mutation without one may act \
                 on a different control."
            ),
        });
    }
    if got == want {
        return None;
    }
    Some(FpRefusal {
        code: "stale_ref",
        resolution: "stale",
        text: format!(
            "stale_ref: element \"{sel}\" changed since its snapshot (expected fp {want}, now {got}). Take a fresh `elements` snapshot and retry with the new ref/fp."
        ),
    })
}

/// BM_CLICK a Win32 button child. No mouse, no focus change (puppet.py:1442-1462).
fn win32_activate(entry: &Map<String, Value>) -> (bool, String, Option<String>) {
    let inv = entry
        .get("invokable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !inv {
        return (
            false,
            String::new(),
            Some(
                "element is not invokable under the Win32 backend: there is no pixel fallback, \
                 because fallback:\"click\" is refused with no_cursor_policy and the pointer is \
                 never moved"
                    .to_string(),
            ),
        );
    }
    let root = entry.get("win").and_then(|v| v.as_i64()).unwrap_or(0);
    let path = entry_str(entry, "path");
    let seg = path.split('.').next().unwrap_or("0");
    let idx = match py_int(&Value::String(seg.to_string())) {
        Ok(n) => n,
        Err(e) => {
            return (
                false,
                String::new(),
                Some(format!("win32-click failed: {e}")),
            )
        }
    };
    let kids = match enum_child_windows(root as HWND) {
        Ok(k) => k,
        Err(e) => {
            return (
                false,
                String::new(),
                Some(format!("win32-click failed: {e}")),
            )
        }
    };
    if idx < 0 || idx >= kids.len() as i64 {
        return (
            false,
            String::new(),
            Some(format!("Win32 path {} went stale", py_repr_str(&path))),
        );
    }
    unsafe {
        let mut _res: usize = 0;
        let r = SendMessageTimeoutW(
            kids[idx as usize],
            0x00F5,
            0,
            0,
            SMTO_ABORTIFHUNG,
            2000,
            &mut _res,
        );
        if r == 0 {
            return (
                false,
                String::new(),
                Some("win32-click failed: target did not respond within 2s".to_string()),
            );
        }
    }
    (true, "win32-click".to_string(), None)
}

/// Pure refusal text (gap 2): the typed, actionable error naming the
/// elevated target. Separated from the live reader below for testability;
/// essentials are pinned by `uipi_refusal_text_names_target_and_recovery`.
fn format_uipi_refusal(exe: &str, pid: u32, hwnd: i64) -> String {
    format!(
        "uipi_blocked: target \"{exe}\" (pid {pid}, hwnd {hwnd}) runs elevated; input from this unelevated process would be silently dropped by UIPI. Relaunch this server elevated and retry; under dry_run this refusal is reported without injecting anything."
    )
}

/// Typed, actionable UIPI refusal naming the elevated target (gap 2). None
/// when input may proceed: zero hwnd, non-elevated target, elevated self, or
/// unreadable elevation (fail-open). Never injects, never switches modes.
fn uipi_refusal(hwnd: i64) -> Option<String> {
    if hwnd == 0 {
        return None;
    }
    if !crate::uipi_blocked(crate::window_elevated(hwnd as HWND), crate::self_elevated()) {
        return None;
    }
    let pid = crate::pid_of(hwnd as HWND);
    Some(format_uipi_refusal(&crate::exe_of_pid(pid), pid, hwnd))
}

fn window_at(x: i64, y: i64) -> Option<i64> {
    let (x, y) = (
        x.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        y.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
    );
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.is_null() {
            None
        } else {
            Some(h as i64)
        }
    }
}

/// The screen point's real input recipient, with the facts the point guard
/// needs. Kept as data so the guard decision is unit-testable with a stub
/// instead of a desktop: nothing here is trusted from the action's own text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PointOwner {
    /// Window `WindowFromPoint` found at the point.
    hit: i64,
    /// Its root owner (`GA_ROOTOWNER`), which is where a click lands.
    root: i64,
    /// Whether the point is inside the locked target's screen rect.
    inside_target_rect: bool,
    /// Whether `hit` is a descendant of the locked target.
    descendant: bool,
    /// One-line identity of the window actually under the point.
    actual: String,
}

/// Pure point guard. Real pointer input is delivered only when the point is
/// inside the locked target's rect AND the window under it is the target, a
/// descendant, or has the target as its root owner. Every other point names
/// the window that WOULD have received the input and delivers nothing.
fn point_guard(
    target: i64,
    verb: &str,
    x: i64,
    y: i64,
    owner: &PointOwner,
) -> Result<(), crate::gate::GateError> {
    let on_target =
        target != 0 && (owner.hit == target || owner.descendant || owner.root == target);
    if on_target && owner.inside_target_rect {
        Ok(())
    } else {
        Err(crate::gate::point_not_target(verb, x, y, &owner.actual))
    }
}

/// Readable identity for a refusal: the window the caller actually aimed at.
fn describe_window(hwnd: i64) -> String {
    if hwnd == 0 {
        return "hwnd=0 (no window under the point)".to_string();
    }
    unsafe {
        let mut cls = [0u16; 128];
        let nc = GetClassNameW(hwnd as HWND, cls.as_mut_ptr(), cls.len() as i32);
        let mut ttl = [0u16; 128];
        let nt = GetWindowTextW(hwnd as HWND, ttl.as_mut_ptr(), ttl.len() as i32);
        let class = String::from_utf16_lossy(&cls[..nc.max(0) as usize]);
        let title = String::from_utf16_lossy(&ttl[..nt.max(0) as usize]);
        format!(
            "hwnd={hwnd} class={class:?} title={title:?} pid={}",
            crate::pid_of(hwnd as HWND)
        )
    }
}

/// Resolve one screen point against the locked target. This is the ONLY
/// source of the guard's facts: the action cannot name its own recipient.
fn point_owner_at(target: i64, x: i64, y: i64) -> PointOwner {
    let hit = window_at(x, y).unwrap_or(0);
    let root = if hit == 0 {
        0
    } else {
        unsafe { GetAncestor(hit as HWND, GA_ROOTOWNER) as i64 }
    };
    let descendant =
        target != 0 && hit != 0 && unsafe { IsChild(target as HWND, hit as HWND) != 0 };
    let inside_target_rect = if target == 0 {
        false
    } else {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        unsafe {
            GetWindowRect(target as HWND, &mut rc) != 0
                && x >= rc.left as i64
                && x <= rc.right as i64
                && y >= rc.top as i64
                && y <= rc.bottom as i64
        }
    };
    PointOwner {
        hit,
        root,
        inside_target_rect,
        descendant,
        actual: describe_window(hit),
    }
}

/// ABSOLUTE coordinates are relative to the PRIMARY monitor unless
/// VIRTUALDESK is set; mouse_normalized uses the whole virtual screen, so
/// every absolute event must carry both flags or a secondary monitor is
/// driven at the wrong point.
const ABS_MOVE_FLAGS: u32 = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;

/// One real mouse INPUT. Pointer delivery is the sole reason this helper
/// exists; it is reachable only from the opt-in real-pointer branch.
fn mouse_ev(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.r#type = INPUT_MOUSE;
    input.Anonymous = INPUT_0 {
        mi: MOUSEINPUT {
            dx,
            dy,
            mouseData: data,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        },
    };
    input
}

/// `SendInput` absolute mouse coordinates are normalised over the whole
/// virtual desktop, not just the primary monitor.
fn mouse_normalized(x: i64, y: i64) -> (i32, i32) {
    unsafe {
        let sx = GetSystemMetrics(SM_XVIRTUALSCREEN) as i64;
        let sy = GetSystemMetrics(SM_YVIRTUALSCREEN) as i64;
        let sw = (GetSystemMetrics(SM_CXVIRTUALSCREEN) as i64).max(1);
        let sh = (GetSystemMetrics(SM_CYVIRTUALSCREEN) as i64).max(1);
        let nx = (((x - sx) as f64 * 65535.0) / (sw - 1).max(1) as f64)
            .round()
            .clamp(0.0, 65535.0) as i32;
        let ny = (((y - sy) as f64 * 65535.0) / (sh - 1).max(1) as f64)
            .round()
            .clamp(0.0, 65535.0) as i32;
        (nx, ny)
    }
}

/// One absolute mouse event at a screen point, with the virtual-desktop
/// flags on EVERY event (move, button and wheel), not just the move.
fn mouse_at(x: i64, y: i64, data: u32, extra: u32) -> INPUT {
    let (nx, ny) = mouse_normalized(x, y);
    mouse_ev(nx, ny, data, ABS_MOVE_FLAGS | extra)
}

/// The INPUT sequence for a click/down/up at one screen point. Every event
/// carries ABSOLUTE|VIRTUALDESK; only the first is a MOVE.
fn click_inputs(sx: i64, sy: i64, verb: &str, btn: &str, count: i64) -> Result<Vec<INPUT>, String> {
    let (down, up) = mouse_button_flags(btn).ok_or_else(|| format!("unknown button \"{btn}\""))?;
    let mut out = vec![mouse_at(sx, sy, 0, MOUSEEVENTF_MOVE)];
    match verb {
        "down" => out.push(mouse_at(sx, sy, 0, down)),
        "up" => out.push(mouse_at(sx, sy, 0, up)),
        "click" => {
            if count > MAX_CLICK_COUNT {
                return Err(format!("click count absurd (>{MAX_CLICK_COUNT}): {count}"));
            }
            for _ in 0..count.max(1) {
                out.push(mouse_at(sx, sy, 0, down));
                out.push(mouse_at(sx, sy, 0, up));
            }
        }
        _ => return Err(format!("unknown click verb \"{verb}\"")),
    }
    Ok(out)
}

fn mouse_button_flags(btn: &str) -> Option<(u32, u32)> {
    match btn {
        "left" => Some((MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)),
        "right" => Some((MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)),
        "middle" => Some((MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP)),
        _ => None,
    }
}

/// Real-pointer state for one opt-in burst. Absent when the caller did not
/// pass `allow_cursor:true`, so a default burst cannot reach SendInput at all.
struct RealInput {
    dry_run: bool,
    queued: Vec<INPUT>,
    moves: i64,
}

/// Windows that would receive one foreground input action (gap 2): the real
/// recipient, not the cursor heuristic -- WindowFromPoint for coordinate
/// actions, the foreground window for keyboard/scroll-without-coords. Empty
/// when the action injects into no process (move), is unreadable, or is not
/// foreground input at all. Parse failures yield no targets here; the action
/// handler below reports its own argument error.
fn uipi_targets_for_action(typ: &str, a: &Map<String, Value>) -> Vec<i64> {
    let mut out = Vec::new();
    let mut push_opt = |h: Option<i64>| {
        if let Some(v) = h {
            out.push(v);
        }
    };
    match typ {
        "click" | "down" | "up" => match (a.get("x"), a.get("y")) {
            (Some(xv), Some(yv)) => {
                if let (Ok(x), Ok(y)) = (py_int(xv), py_int(yv)) {
                    push_opt(window_at(x, y));
                }
            }
            _ => {
                let (cx, cy) = cursor_now();
                push_opt(window_at(cx, cy));
            }
        },
        "drag" => {
            for key in ["from", "to"] {
                if let Some(v) = a.get(key) {
                    if let Ok((x, y)) = unpack2(v) {
                        push_opt(window_at(x, y));
                    }
                }
            }
        }
        "scroll" => match (a.get("x"), a.get("y")) {
            (Some(xv), Some(yv)) => {
                if let (Ok(x), Ok(y)) = (py_int(xv), py_int(yv)) {
                    push_opt(window_at(x, y));
                }
            }
            _ => {
                let f = fg_now();
                if f != 0 {
                    out.push(f);
                }
            }
        },
        "type" | "key" | "hotkey" => {
            let f = fg_now();
            if f != 0 {
                out.push(f);
            }
        }
        _ => {}
    }
    out
}

struct Burst {
    pending: Vec<INPUT>,
    marked: Vec<usize>,
    results: Vec<Map<String, Value>>,
    calls: i64,
    events: i64,
    sendinput_ms: f64,
    /// Gap 2: set when the last flush short-sent against an elevated
    /// foreground (UIPI-suspected). Read by fail_since_flush at the call site.
    flush_uipi: bool,
    /// Present only for an `allow_cursor:true` burst. When absent, no real
    /// pointer INPUT can be queued or sent on any path.
    real: Option<RealInput>,
}

impl Burst {
    /// Flush the queue. Without an opt-in, or under dry_run, this remains a
    /// no-op: no input is queued and nothing is injected. With the opt-in it
    /// sends the queued pointer INPUTs in one `SendInput` call, counts the
    /// call, and reports a short send as a typed failure rather than claiming
    /// delivery.
    fn flush(&mut self) -> (f64, Option<String>) {
        self.flush_uipi = false;
        self.pending.clear();
        self.marked.clear();
        let Some(real) = self.real.as_mut() else {
            return (0.0, None);
        };
        let inputs = std::mem::take(&mut real.queued);
        if inputs.is_empty() || real.dry_run {
            return (0.0, None);
        }
        let t = Instant::now();
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        } as usize;
        self.calls += 1;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        self.sendinput_ms += ms;
        self.events += sent as i64;
        let err = if sent == inputs.len() {
            None
        } else {
            Some(format!(
                "sendinput_failed: {sent} of {} pointer events were injected; the rest were not sent",
                inputs.len()
            ))
        };
        (ms, err)
    }

    /// Queue one pointer event for the opt-in path. Never called from the
    /// default path: `real` is None there.
    fn queue_pointer(&mut self, input: INPUT, is_move: bool) {
        if let Some(real) = self.real.as_mut() {
            real.queued.push(input);
            if is_move {
                real.moves += 1;
            }
        }
    }

    /// Whether a real pointer move was queued on this call.
    fn pointer_moved(&self) -> bool {
        self.real.as_ref().is_some_and(|r| r.moves > 0)
    }
}

/// Deliver one coordinate pointer verb through the real input queue. Called
/// ONLY after the gate returned `Delivery::Cursor`, which requires both the
/// `allow_cursor:true` opt-in and a locked HWND. Every screen point is
/// guarded against the target BEFORE anything is queued; a refusal names the
/// window actually found. Never calls `SetForegroundWindow`.
fn real_pointer_step(
    i: usize,
    typ_v: &Value,
    ad: &Map<String, Value>,
    t0: &Instant,
    target: i64,
    client_coords: bool,
    st: &mut Burst,
    dry_run: bool,
) -> Result<Map<String, Value>, String> {
    let verb = typ_v.as_str().unwrap_or("");
    let to_screen = |x: i64, y: i64| -> Result<(i64, i64), String> {
        if client_coords {
            crate::target::client_point_to_screen(target, x, y)
                .ok_or_else(|| "ClientToScreen failed for the locked target".to_string())
        } else {
            Ok((x, y))
        }
    };
    let mut points: Vec<(i64, i64)> = Vec::new();
    let mut inputs: Vec<INPUT> = Vec::new();
    let move_count: i64;
    let mut absolute_move = false;
    match verb {
        "move" => {
            let has_abs = ad.contains_key("x") || ad.contains_key("y");
            let has_rel = ad.contains_key("dx")
                || ad.contains_key("dy")
                || ad.get("relative").is_some_and(py_truthy);
            if has_abs && has_rel {
                return Err(
                    "move takes x/y or dx/dy, not both (absolute or relative, never mixed)"
                        .to_string(),
                );
            }
            if has_abs {
                let xv = ad
                    .get("x")
                    .ok_or_else(|| "move needs x and y".to_string())?;
                let yv = ad
                    .get("y")
                    .ok_or_else(|| "move needs x and y".to_string())?;
                let (x, y) = (py_int(xv)?, py_int(yv)?);
                points.push(to_screen(x, y)?);
                absolute_move = true;
            } else {
                let dx = opt_py_int(ad, "dx", 0)?;
                let dy = opt_py_int(ad, "dy", 0)?;
                let (cx, cy) = cursor_now();
                points.push((cx.saturating_add(dx), cy.saturating_add(dy)));
            }
            move_count = 1;
        }
        "click" | "down" | "up" => {
            let xv = ad.get("x").ok_or_else(|| format!("{verb} needs x and y"))?;
            let yv = ad.get("y").ok_or_else(|| format!("{verb} needs x and y"))?;
            let (sx, sy) = to_screen(py_int(xv)?, py_int(yv)?)?;
            points.push((sx, sy));
            let count = opt_py_int(ad, "count", 1)?;
            inputs = click_inputs(sx, sy, verb, &bg_btn_name(ad), count)?;
            move_count = 1;
        }
        "drag" => {
            let from = unpack2(
                ad.get("from")
                    .ok_or_else(|| "drag needs from".to_string())?,
            )?;
            let to = unpack2(ad.get("to").ok_or_else(|| "drag needs to".to_string())?)?;
            let (fx, fy) = to_screen(from.0, from.1)?;
            let (tx, ty) = to_screen(to.0, to.1)?;
            points.push((fx, fy));
            points.push((tx, ty));
            let btn = bg_btn_name(ad);
            let Some((down, up)) = mouse_button_flags(&btn) else {
                return Err(format!("unknown button \"{btn}\""));
            };
            inputs.push(mouse_at(fx, fy, 0, MOUSEEVENTF_MOVE));
            inputs.push(mouse_at(fx, fy, 0, down));
            inputs.push(mouse_at(tx, ty, 0, MOUSEEVENTF_MOVE));
            inputs.push(mouse_at(tx, ty, 0, up));
            move_count = 2;
        }
        "scroll" => {
            let xv = ad
                .get("x")
                .ok_or_else(|| "scroll needs x and y".to_string())?;
            let yv = ad
                .get("y")
                .ok_or_else(|| "scroll needs x and y".to_string())?;
            let (sx, sy) = to_screen(py_int(xv)?, py_int(yv)?)?;
            points.push((sx, sy));
            let amount = opt_py_int(ad, "amount", 1)?;
            let axis = match ad.get("axis") {
                None | Some(Value::Null) => "v".to_string(),
                Some(v) => py_str(v).to_lowercase(),
            };
            let (flag, data) = scroll_wheel(&axis, amount)?;
            inputs.push(mouse_at(sx, sy, 0, MOUSEEVENTF_MOVE));
            inputs.push(mouse_at(sx, sy, data, flag));
            move_count = 1;
        }
        _ => return Err(format!("unknown real-pointer verb \"{verb}\"")),
    }

    // Guard EVERY point before a single event is queued. A refusal names the
    // window that would really have received the input.
    let mut guarded: Vec<(i64, i64)> = Vec::new();
    for &(sx, sy) in &points {
        let owner = point_owner_at(target, sx, sy);
        if let Err(e) = point_guard(target, verb, sx, sy, &owner) {
            let mut d = base_entry(i, typ_v, false, elapsed_ms(t0));
            d.insert("code".to_string(), Value::from(e.code));
            d.insert("error".to_string(), Value::from(e.text()));
            d.insert(
                "point".to_string(),
                Value::from(vec![Value::from(sx), Value::from(sy)]),
            );
            d.insert("actualWindow".to_string(), Value::from(owner.actual));
            d.insert("via".to_string(), Value::from("none"));
            return Ok(d);
        }
        guarded.push((sx, sy));
    }

    let mut d = base_entry(i, typ_v, true, elapsed_ms(t0));
    d.insert("delivered".to_string(), Value::from("cursor"));
    d.insert("via".to_string(), Value::from("sendinput"));
    d.insert("guarded".to_string(), Value::from(true));
    d.insert(
        "point".to_string(),
        Value::from(
            guarded
                .iter()
                .map(|&(x, y)| Value::from(vec![Value::from(x), Value::from(y)]))
                .collect::<Vec<_>>(),
        ),
    );
    if dry_run {
        d.insert("dry_run".to_string(), Value::from(true));
        return Ok(d);
    }

    if verb == "move" && absolute_move {
        let (sx, sy) = guarded[0];
        if !(i32::MIN as i64..=i32::MAX as i64).contains(&sx)
            || !(i32::MIN as i64..=i32::MAX as i64).contains(&sy)
        {
            d.insert("ok".to_string(), Value::from(false));
            d.insert(
                "error".to_string(),
                Value::from(format!(
                    "bad_coordinates: screen point ({sx},{sy}) is outside i32 range; nothing was sent"
                )),
            );
            return Ok(d);
        }
        let ok = unsafe { SetCursorPos(sx as i32, sy as i32) != 0 };
        if !ok {
            d.insert("ok".to_string(), Value::from(false));
            d.insert(
                "error".to_string(),
                Value::from("SetCursorPos failed: the pointer was not moved".to_string()),
            );
            return Ok(d);
        }
        if let Some(real) = st.real.as_mut() {
            real.moves += move_count;
        }
    } else {
        for input in inputs {
            st.queue_pointer(input, false);
        }
        if let Some(real) = st.real.as_mut() {
            real.moves += move_count;
        }
        let (_, ferr) = st.flush();
        if let Some(fe) = ferr {
            d.insert("ok".to_string(), Value::from(false));
            d.insert("error".to_string(), Value::from(fe));
        }
    }
    Ok(d)
}

/// Typed code carried by a refusal, so the retro-marked action exposes
/// the same machine-readable code an in-branch refusal would.
fn gate_code_of(msg: &str) -> Option<&'static str> {
    [
        crate::gate::NO_FOREGROUND_POLICY,
        crate::gate::NO_CURSOR_POLICY,
        crate::gate::NO_CLIPBOARD_POLICY,
        crate::gate::POINT_NOT_TARGET,
        crate::gate::BACKGROUND_UNAVAILABLE,
        crate::gate::TARGET_REQUIRED,
    ]
    .into_iter()
    .find(|code| msg.starts_with(code))
}

/// Readable control state used as posted-click proof: the ExpandCollapse
/// state, the RangeValue, the TextPattern text and the enabled bit. `None`
/// when the control exposes none of them ÔÇö in that case a posted click is
/// delivered but its effect is honestly reported `unverified`.
fn element_state(hwnd: i64, path: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(s) = crate::uia::uia_expand_state(hwnd, path) {
        parts.push(format!("expand={}", crate::uia::expand_state_name(s)));
    }
    if let Ok(v) = crate::uia::uia_range_value(hwnd, path) {
        parts.push(format!("value={v}"));
    }
    if let Ok(t) = crate::uia::uia_element_text(hwnd, path) {
        let (t, _) = crate::shot::cap_text(&t);
        parts.push(format!("text={t}"));
    }
    if let Ok(e) = crate::uia::uia_is_enabled(hwnd as HWND, path) {
        parts.push(format!("enabled={e}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(";"))
    }
}

/// Bounded poll for a state change after a posted click (controls process
/// posted messages on their own loop, so the read is not instant).
fn wait_state_change(hwnd: i64, path: &str, before: &str) -> bool {
    let t0 = Instant::now();
    let timeout = Duration::from_millis(400);
    while t0.elapsed() < timeout {
        match element_state(hwnd, path) {
            Some(now) if now != before => return true,
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    element_state(hwnd, path).is_some_and(|now| now != before)
}

/// D3: the readable state a UIA pattern act can actually change. Text and the
/// enabled bit are deliberately absent: Invoke, Toggle, Select and SetValue do
/// not move them, so counting them would report a false `unchanged`. `None`
/// means the control exposes no such state, which is `pattern-returned`, never
/// a guessed `verified`.
fn action_state(hwnd: i64, path: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(s) = crate::uia::uia_expand_state(hwnd, path) {
        parts.push(format!("expand={}", crate::uia::expand_state_name(s)));
    }
    if let Ok(v) = crate::uia::uia_value(hwnd, path) {
        parts.push(format!("value={v}"));
    }
    if let Ok(v) = crate::uia::uia_range_value(hwnd, path) {
        parts.push(format!("range={v}"));
    }
    if let Ok(s) = crate::uia::uia_toggle_state(hwnd, path) {
        parts.push(format!("toggle={s}"));
    }
    if let Ok(s) = crate::uia::uia_is_selected(hwnd, path) {
        parts.push(format!("selected={s}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(";"))
    }
}

/// Pure D3 classifier, kept separate so the three effects have a unit test.
/// Readable-and-moved is `verified`; unreadable on either side is
/// `pattern-returned`; readable-and-still is `unchanged`.
fn effect_of(before: Option<String>, after: Option<String>) -> &'static str {
    match (before, after) {
        (None, _) | (Some(_), None) => "pattern-returned",
        (Some(b), Some(a)) if b == a => "unchanged",
        (Some(_), Some(_)) => "verified",
    }
}

/// Bounded readback after a UIA pattern act. Providers apply synchronously;
/// the poll covers the ones that post to their own loop. `before` must come
/// from `action_state` BEFORE the pattern fired.
fn effect_after(hwnd: i64, path: &str, before: Option<String>) -> &'static str {
    let Some(b) = before else {
        return "pattern-returned";
    };
    let t0 = Instant::now();
    let mut after = action_state(hwnd, path);
    while after.as_deref() == Some(b.as_str()) && t0.elapsed() < Duration::from_millis(400) {
        std::thread::sleep(Duration::from_millis(20));
        after = action_state(hwnd, path);
    }
    effect_of(Some(b), after)
}

/// `unchanged` is a result, not an unproven success: ok:false with the typed
/// policy code and the before-state named in the error.
fn mark_unchanged(d: &mut Map<String, Value>, before: &str) {
    d.insert("ok".to_string(), Value::from(false));
    d.insert(
        "code".to_string(),
        Value::from(crate::gate::BACKGROUND_UNAVAILABLE),
    );
    d.insert(
        "error".to_string(),
        Value::from(format!(
            "background_unavailable: the UIA pattern was accepted but the control's readable state did not change ({before}); nothing was verified"
        )),
    );
}

fn fail_since_flush(
    results: &mut [Map<String, Value>],
    marked: &mut Vec<usize>,
    msg: &str,
    uipi: bool,
) {
    for &i in marked.iter() {
        if let Some(r) = results.get_mut(i) {
            let is_false = r.get("ok") == Some(&Value::from(false));
            if !is_false {
                r.insert("ok".to_string(), Value::from(false));
                r.insert("error".to_string(), Value::from(msg.to_string()));
                if let Some(code) = gate_code_of(msg) {
                    r.insert("code".to_string(), Value::from(code));
                }
                if uipi {
                    r.insert("blocked_by_uipi".to_string(), Value::from(true));
                }
            }
        }
    }
    marked.clear();
}

fn base_entry(index: usize, typ: &Value, ok: bool, ms: f64) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("index".to_string(), Value::from(index as i64));
    m.insert("type".to_string(), typ.clone());
    m.insert("ok".to_string(), Value::from(ok));
    m.insert("ms".to_string(), Value::from(ms));
    m
}

/// A dry run delivers nothing. Normalize EVERY result at the end of the
/// burst, so no early-return path can leak a delivery claim into the reply:
/// some branches used to report delivered:"posted" with posted:0 and an
/// effect:"unverified" that read as "it may have acted". Only the four
/// delivery fields are forced; ok, mode, bg_hwnd, codes and errors are left
/// exactly as the branch produced them (mode/bg_hwnd describe the resolved
/// target, not what was delivered).
fn normalize_dry_results(results: &mut [Map<String, Value>]) {
    for r in results.iter_mut() {
        r.insert("delivered".to_string(), Value::from("none"));
        r.insert("posted".to_string(), Value::from(0));
        r.insert("effect".to_string(), Value::from("none"));
        r.insert("dry_run".to_string(), Value::from(true));
    }
}

fn unpack2(v: &Value) -> Result<(i64, i64), String> {
    match v {
        Value::Array(a) if a.len() == 2 => Ok((py_int(&a[0])?, py_int(&a[1])?)),
        Value::Array(a) if a.len() < 2 => Err(format!(
            "not enough values to unpack (expected 2, got {})",
            a.len()
        )),
        Value::Array(_) => Err("too many values to unpack (expected 2)".to_string()),
        _ => Err(format!(
            "cannot unpack non-iterable {} object",
            py_type_name(v)
        )),
    }
}

/// Optional ExpandCollapse directive for `invoke`: `state` "expand" /
/// "collapse" (or absent for plain activation). Anything else is a typed
/// argument error. Pure; unit-tested.
fn expand_arg(v: Option<&Value>) -> Result<Option<bool>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(x) => match py_str(x).to_lowercase().as_str() {
            "expand" => Ok(Some(true)),
            "collapse" => Ok(Some(false)),
            s => Err(format!(
                "unknown invoke state \"{s}\" (want expand|collapse)"
            )),
        },
    }
}

/// `do` call arguments. Explicit struct (not a 10-positional-args function)
/// so every call site names what it is asking for: a target lock, and the
/// two no-steal permissions the gate mints permits from.
pub struct DoArgs<'a> {
    pub actions: &'a [Value],
    pub background: Option<String>,
    pub background_args: Option<Vec<String>>,
    pub focus_guard: bool,
    pub settle_ms: i64,
    pub dry_run: bool,
    pub desktop: Option<String>,
    /// Structured target lock: `{hwnd?,pid?,exe?,appId?,titleContains?,
    /// className?}` or a selector string. Resolved once, revalidated before
    /// every action.
    pub target: Option<Value>,
    /// Explicit permission for the SendInput / SetForegroundWindow family.
    pub allow_foreground: bool,
    /// Explicit permission to move the owner's pointer.
    pub allow_cursor: bool,
    /// Explicit permission to overwrite the owner's global clipboard (a side
    /// effect outside the target window).
    pub allow_clipboard: bool,
    /// Snapshot the prior clipboard text before the first write and put it
    /// back after the last action, including a partial or failed burst.
    pub restore_clipboard: bool,
    /// Restore the entry pointer after the last action of an
    /// `allow_cursor:true` burst, including a partial or failed one. Default
    /// true; false leaves the pointer wherever the real input put it.
    pub cursor_restore: bool,
}

impl<'a> DoArgs<'a> {
    /// Background-safe defaults: no target, no permissions.
    pub fn new(actions: &'a [Value]) -> Self {
        DoArgs {
            actions,
            background: None,
            background_args: None,
            focus_guard: false,
            settle_ms: 0,
            dry_run: false,
            desktop: None,
            target: None,
            allow_foreground: false,
            allow_cursor: false,
            allow_clipboard: false,
            restore_clipboard: false,
            cursor_restore: true,
        }
    }

    /// Dry-run (assembles events, never injects).
    pub fn dry(actions: &'a [Value]) -> Self {
        DoArgs {
            dry_run: true,
            ..DoArgs::new(actions)
        }
    }

    /// Test-only opt-in batch (dry_run). Flags buy real input only with a
    /// locked target; without one every acting verb still refuses.
    pub fn foreground(actions: &'a [Value]) -> Self {
        DoArgs {
            dry_run: true,
            allow_foreground: true,
            allow_cursor: true,
            ..DoArgs::new(actions)
        }
    }
}

pub fn do_burst(args: &DoArgs) -> String {
    let actions: &[Value] = if args.actions.len() > BURST_MAX_ACTIONS {
        &[]
    } else {
        args.actions
    };
    let background = args.background.clone();
    let background_args = args.background_args.clone();
    let focus_guard = args.focus_guard;
    let cursor_restore = args.cursor_restore;
    let settle_ms = args.settle_ms;
    let dry_run = args.dry_run;
    let desktop = args.desktop.clone();
    // Clipboard writes are opt-in and ledgered; the ledger lives for the
    // whole call so finish() runs even when actions fail or the burst stops.
    let mut clip = ClipboardLedger::new(args.restore_clipboard);
    let t_all = Instant::now();
    let budget = burst_budget();
    let focus_before = fg_now();
    let cursor_before = cursor_now();

    let mut boot_errors: Vec<Map<String, Value>> = Vec::new();
    if args.actions.len() > BURST_MAX_ACTIONS {
        let mut m = Map::new();
        m.insert("index".to_string(), Value::from(0));
        m.insert("type".to_string(), Value::from("burst"));
        m.insert("ok".to_string(), Value::from(false));
        m.insert("ms".to_string(), Value::from(0.0));
        m.insert("code".to_string(), Value::from("burst_budget_exceeded"));
        m.insert(
            "error".to_string(),
            Value::from(format!(
                "burst_budget_exceeded: {} actions exceed the {BURST_MAX_ACTIONS}-action burst cap; nothing was run",
                args.actions.len()
            )),
        );
        boot_errors.push(m);
    }
    let mut settle_n = settle_ms;
    if settle_n < 0 {
        settle_n = 0;
    } else if settle_n > 60000 {
        let mut m = Map::new();
        m.insert("index".to_string(), Value::from(0));
        m.insert("type".to_string(), Value::from("settle"));
        m.insert("ok".to_string(), Value::from(false));
        m.insert("ms".to_string(), Value::from(0.0));
        m.insert(
            "error".to_string(),
            Value::from(format!("settle_ms absurd (>60000): {settle_n}")),
        );
        boot_errors.push(m);
        settle_n = 0;
    }
    let hidden_mode = desktop
        .as_deref()
        .map(|d| d.trim().eq_ignore_ascii_case("hidden"))
        .unwrap_or(false);

    let target_lock: Option<crate::target::TargetLock> = match args.target.as_ref() {
        None | Some(Value::Null) => None,
        Some(v) => {
            let spec = match crate::target::TargetSpec::parse(v) {
                Ok(s) => s,
                Err(e) => return target_refusal_blocks(&e),
            };
            match crate::target::lock(&spec) {
                Ok(l) => Some(l),
                Err(e) => return target_refusal_blocks(&e),
            }
        }
    };
    let mut hidden_owner: Option<crate::hidden::Hidden> = None;
    let mut hidden_created = false;
    let hidden_launch_requested = hidden_mode && target_lock.is_none();
    let mut hidden_set_desktop = false;
    let mut hidden_pid: Option<u32> = None;
    let mut hidden_name = String::new();
    let mut hidden_hwnd: Option<i64> = None;
    let mut hidden_class = String::new();
    let mut hidden_title = String::new();
    let mut hidden_win_pid: u32 = 0;

    let bg_mode = background.is_some() || hidden_mode || target_lock.is_some();
    let (bg_hwnd, bg_error): (Option<i64>, Option<String>) = if let Some(l) = &target_lock {
        (Some(l.hwnd), None)
    } else if hidden_mode {
        let prog = match background.as_deref() {
            None => Err(
                "bad_window: desktop=\"hidden\" needs background to name the program to launch (e.g. exe:notepad.exe)"
                    .to_string(),
            ),
            Some(s) => crate::hidden::program_from_background(s),
        };
        match prog {
            Err(e) => (None, Some(e)),
            Ok(p) => {
                let empty: Vec<String> = Vec::new();
                let h = crate::hidden::spawn(&p, background_args.as_ref().unwrap_or(&empty));
                hidden_created = h.ready.created;
                hidden_set_desktop = h.ready.set_desktop;
                hidden_pid = h.ready.pid;
                hidden_hwnd = h.ready.hwnd;
                hidden_class = h.ready.hwnd_class.clone();
                hidden_title = h.ready.hwnd_title.clone();
                hidden_win_pid = h.ready.hwnd_pid;
                hidden_name = h.ready.desktop_name.clone();
                let err = h.ready.error.clone();
                hidden_owner = Some(h);
                match err {
                    Some(e) => (None, Some(e)),
                    None => (hidden_hwnd, None),
                }
            }
        }
    } else if !bg_mode {
        (None, None)
    } else {
        let s = background.unwrap();
        if s.trim().is_empty() {
            (
                None,
                Some(
                    "bad_window: background must be a selector string (hwnd:|pid:|exe:|title:)"
                        .to_string(),
                ),
            )
        } else {
            let (h, err, _) = resolve_selector(&s);
            match h {
                Some(x) => (Some(x as i64), None),
                None => (
                    None,
                    Some(err.unwrap_or("bad_window: cannot resolve background target".to_string())),
                ),
            }
        }
    };
    let private_desktop = if hidden_mode {
        let created = hidden_owner
            .as_ref()
            .map(|h| h.ready.created && !h.ready.desktop_name.is_empty())
            .unwrap_or(false);
        let target_desktop = bg_hwnd.and_then(|h| crate::window_desktop_name(h as HWND));
        let earned =
            private_desktop_earned(created, hidden_name.as_str(), target_desktop.as_deref());
        if hidden_mode && !earned {
            debug_eprintln!(
                "puppet: desktop:'hidden' did not earn private-desktop permission \
                 (created={created}, target_desktop={target_desktop:?}, expected={:?}); \
                 using ordinary target rules",
                hidden_name
            );
        }
        earned
    } else {
        false
    };
    let mut posted_total: i64 = 0;

    let mut st = Burst {
        pending: Vec::new(),
        marked: Vec::new(),
        results: boot_errors,
        calls: 0,
        events: 0,
        sendinput_ms: 0.0,
        flush_uipi: false,
        real: args.allow_cursor.then(|| RealInput {
            dry_run,
            queued: Vec::new(),
            moves: 0,
        }),
    };

    for (i, a) in actions.iter().enumerate() {
        if t_all.elapsed() >= budget {
            let mut m = base_entry(i, &Value::from("burst"), false, 0.0);
            m.insert("code".to_string(), Value::from("burst_budget_exceeded"));
            m.insert(
                "error".to_string(),
                Value::from(format!(
                    "burst_budget_exceeded: the {}-second burst budget expired before action {i}; {} action(s) not run",
                    budget.as_secs_f64(),
                    actions.len() - i
                )),
            );
            st.results.push(m);
            break;
        }
        if crate::cancel::cancelled() {
            let mut m = base_entry(i, &Value::from("cancelled"), false, 0.0);
            m.insert("error".to_string(), Value::from("cancelled"));
            st.results.push(m);
            break;
        }
        let t0 = Instant::now();
        let ad = match a {
            Value::Object(m) if m.contains_key("type") => m,
            _ => {
                let typ = match a {
                    Value::Object(m) => m.get("type").cloned().unwrap_or(Value::Null),
                    _ => Value::Null,
                };
                let mut m = base_entry(i, &typ, false, 0.0);
                m.insert(
                    "error".to_string(),
                    Value::from("action must be a dict with 'type'"),
                );
                st.results.push(m);
                continue;
            }
        };
        let typ_v = ad["type"].clone();
        let typ = typ_v.as_str().unwrap_or("\u{0}");
        let has_element =
            matches!(ad.get("element"), Some(v) if py_truthy(v) && !py_str(v).trim().is_empty());
        if !crate::gate::uia_text_permitted(private_desktop)
            && (typ == "set_value" || (typ == "type" && has_element))
        {
            let sel = ad.get("element").map(py_str).unwrap_or_default();
            let e = crate::gate::uia_text_unavailable(sel.trim());
            let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
            m.insert("code".to_string(), Value::from(e.code));
            m.insert("error".to_string(), Value::from(e.text()));
            if bg_mode {
                m.insert("mode".to_string(), Value::from("background"));
                m.insert(
                    "bg_hwnd".to_string(),
                    bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                );
            }
            if dry_run {
                m.insert("dry_run".to_string(), Value::from(true));
            }
            st.results.push(m);
            continue;
        }
        let delivery = match ad.get("delivery") {
            None | Some(Value::Null) => "auto".to_string(),
            Some(Value::String(s)) => s.to_lowercase(),
            Some(v) => {
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert(
                    "error".to_string(),
                    Value::from(format!(
                        "unknown delivery {} (want auto|posted|sendinput)",
                        crate::py_repr_value(v)
                    )),
                );
                st.results.push(m);
                continue;
            }
        };
        if !["auto", "posted", "sendinput"].contains(&delivery.as_str()) {
            let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
            m.insert(
                "error".to_string(),
                Value::from(format!(
                    "unknown delivery \"{delivery}\" (want auto|posted|sendinput)"
                )),
            );
            st.results.push(m);
            continue;
        }
        // `delivery:"sendinput"` is still refused unless the caller ALSO
        // opted in at the top level with a locked target and a pointer verb.
        // There is no implicit foreground/input path.
        let sendinput_opted = args.allow_cursor
            && bg_hwnd.is_some()
            && matches!(typ, "move" | "click" | "down" | "up" | "drag" | "scroll");
        if delivery == "sendinput" && !sendinput_opted {
            let e = crate::gate::no_foreground_policy(&format!("{typ} delivery:\"sendinput\""));
            let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
            m.insert("code".to_string(), Value::from(e.code));
            m.insert("error".to_string(), Value::from(e.text()));
            if bg_mode {
                m.insert("mode".to_string(), Value::from("background"));
                m.insert(
                    "bg_hwnd".to_string(),
                    bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                );
            }
            if dry_run {
                m.insert("dry_run".to_string(), Value::from(true));
            }
            st.results.push(m);
            continue;
        }
        if let Some(l) = &target_lock {
            if let Err(e) = l.revalidate() {
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                if let Some(c) = e.candidates {
                    m.insert(
                        "candidates".to_string(),
                        Value::Array(c.into_iter().map(Value::Object).collect()),
                    );
                }
                st.results.push(m);
                continue;
            }
        }
        let has_scroll_element = typ == "scroll" && has_element;
        let has_coords = ad.contains_key("x") && ad.contains_key("y");
        let gate = crate::gate::Gate::new(
            bg_hwnd.is_some() || bg_mode,
            args.allow_foreground,
            args.allow_cursor,
        )
        .locked_hwnd(bg_hwnd.is_some() || (typ == "focus_window" && ad.get("hwnd").is_some()))
        .private_desktop(private_desktop)
        .allow_clipboard(args.allow_clipboard);
        let gate_delivery = match gate.decide(typ, has_coords, has_element) {
            Err(e) => {
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                if bg_mode {
                    m.insert("mode".to_string(), Value::from("background"));
                    m.insert(
                        "bg_hwnd".to_string(),
                        bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                    );
                }
                if dry_run {
                    m.insert("dry_run".to_string(), Value::from(true));
                }
                st.results.push(m);
                continue;
            }
            Ok(d) => d,
        };
        let keyboard_postable = private_desktop && matches!(typ, "type" | "key" | "hotkey");
        let is_bg_postable = bg_mode
            && !matches!(
                gate_delivery,
                crate::gate::Delivery::Cursor | crate::gate::Delivery::Foreground
            )
            && ((matches!(typ, "click" | "down" | "up" | "move" | "drag")
                || keyboard_postable
                || (typ == "scroll" && !has_scroll_element))
                && !(has_element && matches!(typ, "type" | "click")));

        let step: Result<Option<Map<String, Value>>, String> = (|| {
            if gate_delivery == crate::gate::Delivery::Cursor {
                return Ok(Some(real_pointer_step(
                    i,
                    &typ_v,
                    ad,
                    &t0,
                    bg_hwnd.unwrap_or(0),
                    target_lock.is_some(),
                    &mut st,
                    dry_run,
                )?));
            }
            if !bg_mode {
                for t in uipi_targets_for_action(typ, ad) {
                    if let Some(msg) = uipi_refusal(t) {
                        let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        m.insert("error".to_string(), Value::from(msg));
                        m.insert("blocked_by_uipi".to_string(), Value::from(true));
                        return Ok(Some(m));
                    }
                }
            }
            if is_bg_postable {
                if let Some(note) = bg_non_postable(typ) {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("mode".to_string(), Value::from("background"));
                    m.insert(
                        "bg_hwnd".to_string(),
                        bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                    );
                    m.insert(
                        "error".to_string(),
                        Value::from(format!("{note}{BG_SUFFIX}")),
                    );
                    m.insert("note".to_string(), Value::from(BG_NOTE));
                    return Ok(Some(m));
                }
                if let Some(be) = bg_error.clone() {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("mode".to_string(), Value::from("background"));
                    m.insert("bg_hwnd".to_string(), Value::Null);
                    m.insert("error".to_string(), Value::from(be));
                    m.insert("note".to_string(), Value::from(BG_NOTE));
                    return Ok(Some(m));
                }
                if let Some(bh) = bg_hwnd {
                    if let Some(msg) = uipi_refusal(bh) {
                        let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        m.insert("mode".to_string(), Value::from("background"));
                        m.insert("bg_hwnd".to_string(), Value::from(bh));
                        m.insert("posted".to_string(), Value::from(0));
                        m.insert("error".to_string(), Value::from(msg));
                        m.insert("blocked_by_uipi".to_string(), Value::from(true));
                        m.insert("note".to_string(), Value::from(BG_NOTE));
                        if dry_run {
                            m.insert("dry_run".to_string(), Value::from(true));
                        }
                        return Ok(Some(m));
                    }
                }
                if dry_run {
                    let mut m = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    m.insert("mode".to_string(), Value::from("background"));
                    m.insert(
                        "bg_hwnd".to_string(),
                        bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                    );
                    m.insert("posted".to_string(), Value::from(0));
                    m.insert("dry_run".to_string(), Value::from(true));
                    m.insert("note".to_string(), Value::from(BG_NOTE));
                    return Ok(Some(m));
                }
                let (ok_b, err_b, n_b) = bg_execute(
                    bg_hwnd.unwrap_or(0),
                    ad,
                    target_lock.is_some(),
                    private_desktop,
                );
                posted_total += n_b;
                let mut m = base_entry(i, &typ_v, ok_b, elapsed_ms(&t0));
                m.insert("mode".to_string(), Value::from("background"));
                m.insert(
                    "bg_hwnd".to_string(),
                    bg_hwnd.map(Value::from).unwrap_or(Value::Null),
                );
                m.insert("posted".to_string(), Value::from(n_b));
                m.insert("note".to_string(), Value::from(BG_NOTE));
                if typ == "click" {
                    m.insert("delivered".to_string(), Value::from("posted"));
                    m.insert("effect".to_string(), Value::from("unverified"));
                }
                if let Some(e) = err_b {
                    m.insert("error".to_string(), Value::from(e));
                }
                return Ok(Some(m));
            }
            if typ == "move" {
                let has_abs = ad.contains_key("x") || ad.contains_key("y");
                let has_rel = ad.contains_key("dx")
                    || ad.contains_key("dy")
                    || ad.get("relative").is_some_and(py_truthy);
                if has_abs && has_rel {
                    return Err(
                        "move takes x/y or dx/dy, not both (absolute or relative, never mixed)"
                            .to_string(),
                    );
                }
                let e = crate::gate::no_cursor_policy("move");
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                return Ok(Some(m));
            }
            if typ == "click" && !has_element {
                let e = crate::gate::no_cursor_policy("click x/y");
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                return Ok(Some(m));
            }
            if typ == "down" || typ == "up" {
                let e = crate::gate::no_cursor_policy(typ);
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                return Ok(Some(m));
            }
            if typ == "drag" {
                let e = crate::gate::no_cursor_policy("drag");
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                return Ok(Some(m));
            }
            if typ == "scroll" {
                let elem_sel: Option<Value> = match ad.get("element") {
                    None | Some(Value::Null) => None,
                    Some(v) if !py_truthy(v) || py_str(v).trim().is_empty() => None,
                    Some(v) => Some(v.clone()),
                };
                let has_xy = ad.contains_key("x") || ad.contains_key("y");
                if elem_sel.is_some() && has_xy {
                    return Err(
                        "scroll takes x/y or element, not both (element scroll never moves the mouse)"
                            .to_string(),
                    );
                }
                if elem_sel.is_none() && ad.contains_key("x") && ad.contains_key("y") {
                    let e = crate::gate::no_cursor_policy("scroll x/y");
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("code".to_string(), Value::from(e.code));
                    m.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(m));
                }
                let amt = opt_py_int(ad, "amount", 1)?;
                let axis = match ad.get("axis") {
                    None | Some(Value::Null) => "v".to_string(),
                    Some(v) => py_str(v).to_lowercase(),
                };
                if let Some(elem_v) = elem_sel {
                    let (_, ferr) = st.flush();
                    if let Some(fe) = ferr {
                        fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                    } else {
                        st.marked.clear();
                    }
                    let mut d = base_entry(i, &typ_v, false, 0.0);
                    if bg_mode && (bg_error.is_some() || bg_hwnd.is_none()) {
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                bg_error
                                    .clone()
                                    .unwrap_or("bad_window: no background target".to_string()),
                            ),
                        );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let tgt = match (bg_mode, bg_hwnd) {
                        (true, Some(h)) => h,
                        _ => fg_now(),
                    };
                    d.insert("target_hwnd".to_string(), Value::from(tgt));
                    if tgt == 0 {
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                "bad_window: scroll element needs a background target or a foreground window",
                            ),
                        );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    if let Some(msg) = uipi_refusal(tgt) {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("error".to_string(), Value::from(msg));
                        d.insert("blocked_by_uipi".to_string(), Value::from(true));
                        d.insert("resolution".to_string(), Value::from("uipi_blocked"));
                        d.insert("via".to_string(), Value::from("none"));
                        if dry_run {
                            d.insert("dry_run".to_string(), Value::from(true));
                        }
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let model = element_model(tgt as HWND, crate::UIA_DEPTH_LIMIT, 0);
                    if dry_run {
                        d.insert("dry_run".to_string(), Value::from(true));
                    }
                    d.insert("backend".to_string(), Value::from(model.backend.clone()));
                    let (entry, merr, mcands) =
                        match_element(&model.entries, Some(&elem_v), ad.get("index"));
                    if let Some(me) = merr {
                        d.insert("error".to_string(), Value::from(me));
                        d.insert(
                            "resolution".to_string(),
                            Value::from(if mcands.is_some() {
                                "ambiguous"
                            } else {
                                "not-found"
                            }),
                        );
                        if let Some(c) = mcands {
                            d.insert(
                                "candidates".to_string(),
                                Value::from(c.into_iter().map(Value::Object).collect::<Vec<_>>()),
                            );
                        }
                        if dry_run {
                            d.insert("via".to_string(), Value::from("none"));
                        }
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let entry = entry.unwrap();
                    if let Some(se) = check_expect_fp(ad, &entry, &py_str(&elem_v), false) {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("code".to_string(), Value::from(se.code));
                        d.insert("error".to_string(), Value::from(se.text));
                        d.insert("resolution".to_string(), Value::from(se.resolution));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    if dry_run {
                        d.insert("ok".to_string(), Value::from(true));
                        d.insert("resolution".to_string(), Value::from("found"));
                        d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                    d.insert("resolution".to_string(), Value::from("found"));
                    if model.backend != "uia" {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                "scroll element needs the UIA backend (Win32 fallback has no ScrollPattern)",
                            ),
                        );
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let (h, v) = match crate::uia::scroll_pattern_args(&axis, amt) {
                        Ok(p) => p,
                        Err(e) => {
                            d.insert("ok".to_string(), Value::from(false));
                            d.insert("error".to_string(), Value::from(e));
                            d.insert("via".to_string(), Value::from("none"));
                            d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                            return Ok(Some(d));
                        }
                    };
                    let (ok_v, via, verr) =
                        crate::uia::uia_scroll(tgt, &entry_str(&entry, "path"), h, v);
                    d.insert(
                        "via".to_string(),
                        Value::from(if via.is_empty() {
                            "none".to_string()
                        } else {
                            via
                        }),
                    );
                    d.insert("ok".to_string(), Value::from(ok_v));
                    if let Some(e) = verr {
                        if !ok_v {
                            d.insert("error".to_string(), Value::from(e));
                        }
                    }
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let e = crate::gate::no_cursor_policy("scroll");
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("code".to_string(), Value::from(e.code));
                m.insert("error".to_string(), Value::from(e.text()));
                return Ok(Some(m));
            }
            if typ == "click" && has_element {
                let (_, ferr) = st.flush();
                if let Some(fe) = ferr {
                    fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                } else {
                    st.marked.clear();
                }
                let mut d = base_entry(i, &typ_v, false, 0.0);
                let Some(tgt) = bg_hwnd else {
                    d.insert(
                        "error".to_string(),
                        Value::from(
                            "target_required: click on an element needs a target window \
                             (target:{\"hwnd\":..}); this server never falls back to the \
                             foreground window",
                        ),
                    );
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                };
                if let Some(msg) = uipi_refusal(tgt) {
                    d.insert("error".to_string(), Value::from(msg));
                    d.insert("blocked_by_uipi".to_string(), Value::from(true));
                    d.insert("resolution".to_string(), Value::from("uipi_blocked"));
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let model = element_model(tgt as HWND, crate::UIA_DEPTH_LIMIT, 0);
                if dry_run {
                    d.insert("dry_run".to_string(), Value::from(true));
                }
                d.insert("target_hwnd".to_string(), Value::from(tgt));
                d.insert("backend".to_string(), Value::from(model.backend.clone()));
                let (entry, merr, mcands) =
                    match_element(&model.entries, ad.get("element"), ad.get("index"));
                if let Some(me) = merr {
                    d.insert("error".to_string(), Value::from(me));
                    d.insert(
                        "resolution".to_string(),
                        Value::from(if mcands.is_some() {
                            "ambiguous"
                        } else {
                            "not-found"
                        }),
                    );
                    if let Some(c) = mcands {
                        d.insert(
                            "candidates".to_string(),
                            Value::from(c.into_iter().map(Value::Object).collect::<Vec<_>>()),
                        );
                    }
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let entry = entry.unwrap();
                if let Some(se) = check_expect_fp(
                    ad,
                    &entry,
                    &py_str(ad.get("element").unwrap_or(&Value::Null)),
                    true,
                ) {
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert("code".to_string(), Value::from(se.code));
                    d.insert("error".to_string(), Value::from(se.text));
                    d.insert("resolution".to_string(), Value::from(se.resolution));
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                d.insert("resolution".to_string(), Value::from("found"));
                let centre = element_centre_client(tgt, &entry);
                if dry_run {
                    if let Err(r) = centre {
                        if model.backend != "uia" {
                            d.insert("ok".to_string(), Value::from(false));
                            d.insert("via".to_string(), Value::from("none"));
                            d.insert(
                                "code".to_string(),
                                Value::from(if r == RectRefusal::Offscreen {
                                    crate::gate::OFFSCREEN_ELEMENT
                                } else {
                                    crate::gate::BACKGROUND_UNAVAILABLE
                                }),
                            );
                            d.insert(
                                "error".to_string(),
                                Value::from(rect_refusal_message(&entry, r)),
                            );
                            d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                            return Ok(Some(d));
                        }
                    }
                    d.insert(
                        "via".to_string(),
                        Value::from(if model.backend == "uia" {
                            "uia-pattern"
                        } else {
                            "posted-click"
                        }),
                    );
                    d.insert("ok".to_string(), Value::from(true));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let mut via = "posted-click".to_string();
                let mut ok_v = false;
                let mut verr: Option<String> = None;
                let mut delivered = "none";
                let mut effect = "unverified";
                let path = entry_str(&entry, "path");
                if model.backend == "uia" {
                    let (ok_a, via_a, err_a) = crate::uia::uia_activate(tgt, &path);
                    if ok_a {
                        ok_v = true;
                        via = via_a;
                        delivered = "uia";
                        let pre = element_state(tgt, &path);
                        effect = match pre {
                            Some(pre) => {
                                if wait_state_change(tgt, &path, &pre) {
                                    "verified"
                                } else {
                                    "pattern-returned"
                                }
                            }
                            None => "pattern-returned",
                        };
                    } else {
                        verr = err_a;
                    }
                }
                if !ok_v {
                    match centre {
                        Ok((cx, cy)) => {
                            let pre = element_state(tgt, &path);
                            let mut m = Map::new();
                            m.insert("type".to_string(), Value::from("click"));
                            m.insert("x".to_string(), Value::from(cx));
                            m.insert("y".to_string(), Value::from(cy));
                            if let Some(b) = ad.get("button") {
                                m.insert("button".to_string(), b.clone());
                            }
                            if let Some(c) = ad.get("count") {
                                m.insert("count".to_string(), c.clone());
                            }
                            let (ok_b, err_b, n_b) = bg_execute(tgt, &m, true, false);
                            posted_total += n_b;
                            ok_v = ok_b;
                            via = "posted-click".to_string();
                            delivered = "posted";
                            if let Some(e) = err_b {
                                verr = Some(e);
                            }
                            if ok_b {
                                effect = match pre {
                                    Some(pre) => {
                                        if wait_state_change(tgt, &path, &pre) {
                                            "verified"
                                        } else {
                                            ok_v = false;
                                            verr = Some(format!(
                                                "background_unavailable: the posted click was \
                                             delivered to the control but its state did not \
                                             change ({pre}); controls that ignore posted button \
                                             messages report exactly this. Use invoke for the \
                                             UIA pattern; when the control has no pattern there \
                                             is no pixel fallback, and allow_foreground:true is \
                                             refused with no_foreground_policy."
                                            ));
                                            "unchanged"
                                        }
                                    }
                                    None => "unverified",
                                };
                            } else {
                                effect = "unverified";
                            }
                        }
                        Err(r) => {
                            d.insert("ok".to_string(), Value::from(false));
                            d.insert("via".to_string(), Value::from("none"));
                            d.insert("delivered".to_string(), Value::from("none"));
                            d.insert("effect".to_string(), Value::from("unverified"));
                            d.insert(
                                "code".to_string(),
                                Value::from(if r == RectRefusal::Offscreen {
                                    crate::gate::OFFSCREEN_ELEMENT
                                } else {
                                    crate::gate::BACKGROUND_UNAVAILABLE
                                }),
                            );
                            d.insert(
                                "error".to_string(),
                                Value::from(rect_refusal_message(&entry, r)),
                            );
                            d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                            return Ok(Some(d));
                        }
                    }
                }
                d.insert("ok".to_string(), Value::from(ok_v));
                d.insert("via".to_string(), Value::from(via));
                d.insert("delivered".to_string(), Value::from(delivered));
                d.insert("effect".to_string(), Value::from(effect));
                if !ok_v {
                    if let Some(e) = verr {
                        d.insert(
                            "code".to_string(),
                            Value::from(crate::gate::BACKGROUND_UNAVAILABLE),
                        );
                        d.insert("error".to_string(), Value::from(e));
                    }
                }
                d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                return Ok(Some(d));
            }
            if typ == "type" && has_element {
                if !crate::gate::uia_text_permitted(private_desktop) {
                    let sel = ad.get("element").map(py_str).unwrap_or_default();
                    let e = crate::gate::uia_text_unavailable(sel.trim());
                    let mut dd = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    dd.insert("code".to_string(), Value::from(e.code));
                    dd.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(dd));
                }
                #[allow(unreachable_code)]
                {
                    let (_, ferr) = st.flush();
                    if let Some(fe) = ferr {
                        fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                    } else {
                        st.marked.clear();
                    }
                    let mut d = base_entry(i, &typ_v, false, 0.0);
                    let Some(tgt) = bg_hwnd else {
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                "target_required: type on an element needs a target window; this \
                             server never falls back to the foreground window",
                            ),
                        );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    };
                    if let Some(msg) = uipi_refusal(tgt) {
                        d.insert("error".to_string(), Value::from(msg));
                        d.insert("blocked_by_uipi".to_string(), Value::from(true));
                        d.insert("resolution".to_string(), Value::from("uipi_blocked"));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let text = ad.get("text").map(py_str).unwrap_or_default();
                    let model = element_model(tgt as HWND, crate::UIA_DEPTH_LIMIT, 0);
                    if dry_run {
                        d.insert("dry_run".to_string(), Value::from(true));
                    }
                    d.insert("target_hwnd".to_string(), Value::from(tgt));
                    d.insert("backend".to_string(), Value::from(model.backend.clone()));
                    let (entry, merr, mcands) =
                        match_element(&model.entries, ad.get("element"), ad.get("index"));
                    if let Some(me) = merr {
                        d.insert("error".to_string(), Value::from(me));
                        d.insert(
                            "resolution".to_string(),
                            Value::from(if mcands.is_some() {
                                "ambiguous"
                            } else {
                                "not-found"
                            }),
                        );
                        if let Some(c) = mcands {
                            d.insert(
                                "candidates".to_string(),
                                Value::from(c.into_iter().map(Value::Object).collect::<Vec<_>>()),
                            );
                        }
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let entry = entry.unwrap();
                    if let Some(se) = check_expect_fp(
                        ad,
                        &entry,
                        &py_str(ad.get("element").unwrap_or(&Value::Null)),
                        !crate::gate::uia_text_permitted(private_desktop),
                    ) {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("code".to_string(), Value::from(se.code));
                        d.insert("error".to_string(), Value::from(se.text));
                        d.insert("resolution".to_string(), Value::from(se.resolution));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                    d.insert("resolution".to_string(), Value::from("found"));
                    d.insert(
                        "value_len".to_string(),
                        Value::from(text.chars().count() as i64),
                    );
                    if dry_run {
                        d.insert("ok".to_string(), Value::from(true));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("delivered".to_string(), Value::from("uia"));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let path = entry_str(&entry, "path");
                    let before = action_state(tgt, &path);
                    let (ok_v, via, verr) = crate::uia::uia_set_value(tgt, &path, &text);
                    d.insert("ok".to_string(), Value::from(ok_v));
                    d.insert(
                        "via".to_string(),
                        Value::from(if via.is_empty() {
                            "uia-set-value".to_string()
                        } else {
                            via
                        }),
                    );
                    d.insert("delivered".to_string(), Value::from("uia"));
                    let effect = if ok_v {
                        effect_after(tgt, &path, before.clone())
                    } else {
                        "unverified"
                    };
                    d.insert("effect".to_string(), Value::from(effect));
                    if effect == "unchanged" {
                        let b = before.unwrap_or_default();
                        mark_unchanged(&mut d, &b);
                    }
                    if let Some(e) = verr {
                        if !ok_v {
                            d.insert("error".to_string(), Value::from(e));
                        }
                    }
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
            }
            if typ == "type" {
                let text = match ad.get("text") {
                    None => String::new(),
                    Some(v) => py_str(v),
                };
                if let Err(e) = check_clip_len(text.chars().count()) {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("error".to_string(), Value::from(e));
                    return Ok(Some(m));
                }
                let Some(lock) = target_lock.as_ref() else {
                    let e = crate::gate::GateError {
                        code: crate::gate::TARGET_REQUIRED,
                        message: "type needs an explicit target window; this server never \
                                  falls back to the foreground window. Nothing was sent."
                            .to_string(),
                    };
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("code".to_string(), Value::from(e.code));
                    d.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(d));
                };
                let tgt = lock.hwnd;
                if let Some(msg) = uipi_refusal(tgt) {
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("error".to_string(), Value::from(msg));
                    d.insert("blocked_by_uipi".to_string(), Value::from(true));
                    return Ok(Some(d));
                }
                if dry_run {
                    let mut d = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    d.insert("dry_run".to_string(), Value::from(true));
                    d.insert("delivered".to_string(), Value::from("posted"));
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert("posted".to_string(), Value::from(0));
                    return Ok(Some(d));
                }
                let dest = bg_text_target(tgt);
                let mut posted = 0i64;
                let mut failed = false;
                for unit in utf16_units(&text) {
                    if crate::cancel::cancelled() {
                        let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        d.insert("delivered".to_string(), Value::from("posted"));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from("cancelled"));
                        d.insert("posted".to_string(), Value::from(posted));
                        return Ok(Some(d));
                    }
                    if !bg_post(dest, 0x0102, unit as u64, 1) {
                        failed = true;
                        break;
                    }
                    posted += 1;
                }
                let mut d = base_entry(i, &typ_v, !failed, elapsed_ms(&t0));
                d.insert("delivered".to_string(), Value::from("posted"));
                d.insert("effect".to_string(), Value::from("unverified"));
                d.insert("posted".to_string(), Value::from(posted));
                if failed {
                    d.insert(
                        "error".to_string(),
                        Value::from("PostMessage failed: the target did not accept the text"),
                    );
                }
                d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                return Ok(Some(d));
            }
            if typ == "key" {
                let name = match ad.get("name") {
                    None => String::new(),
                    Some(v) => py_str(v),
                };
                let code = vk_of(&name)?;
                let hold_ms = opt_py_int(ad, "hold_ms", 80)?;
                let repeat = opt_py_int(ad, "repeat", 1)?;
                let gap_ms = opt_py_int(ad, "gap_ms", 0)?;
                if !(0..=60000).contains(&hold_ms) {
                    return Err(format!("hold_ms out of range 0..60000 (got {hold_ms})"));
                }
                if !(1..=100).contains(&repeat) {
                    return Err(format!("key repeat out of range 1..100 (got {repeat})"));
                }
                if !(0..=60000).contains(&gap_ms) {
                    return Err(format!("gap_ms out of range 0..60000 (got {gap_ms})"));
                }
                let verify = match ad.get("verify") {
                    None | Some(Value::Null) => true,
                    Some(Value::Bool(b)) => *b,
                    Some(v) => {
                        return Err(format!(
                            "verify must be a boolean (got {})",
                            crate::py_repr_value(v)
                        ));
                    }
                };
                let Some(lock) = target_lock.as_ref() else {
                    let e = crate::gate::GateError {
                        code: crate::gate::TARGET_REQUIRED,
                        message: "key needs an explicit target window; this server never \
                                  falls back to the foreground window. Nothing was sent."
                            .to_string(),
                    };
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("code".to_string(), Value::from(e.code));
                    d.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(d));
                };
                let tgt = lock.hwnd;
                if let Some(msg) = uipi_refusal(tgt) {
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("error".to_string(), Value::from(msg));
                    d.insert("blocked_by_uipi".to_string(), Value::from(true));
                    return Ok(Some(d));
                }
                if dry_run {
                    let mut d = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    d.insert("dry_run".to_string(), Value::from(true));
                    d.insert("delivered".to_string(), Value::from("posted"));
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert("posted".to_string(), Value::from(0));
                    return Ok(Some(d));
                }
                let before = match (verify, verify_capture(tgt)) {
                    (true, Ok(im)) => Some(im),
                    (true, Err(e)) => {
                        let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        d.insert("delivered".to_string(), Value::from("none"));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                        return Ok(Some(d));
                    }
                    (false, _) => None,
                };
                let downs = posted_key_downs(&code);
                let ups = posted_key_ups(&code);
                let mut posted = 0i64;
                let mut failed = false;
                for r in 0..repeat {
                    for (msg, w, l) in &downs {
                        if !bg_post(tgt, *msg, *w, *l) {
                            failed = true;
                            break;
                        }
                        posted += 1;
                    }
                    if failed {
                        break;
                    }
                    if hold_ms > 0 {
                        match bounded_pause(&t_all, budget, hold_ms as u64) {
                            Some(h) => {
                                if sleep_cancellable(h) || crate::cancel::cancelled() {
                                    for (msg, w, l) in &ups {
                                        if bg_post(tgt, *msg, *w, *l) {
                                            posted += 1;
                                        }
                                    }
                                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                                    d.insert("delivered".to_string(), Value::from("posted"));
                                    d.insert("effect".to_string(), Value::from("unverified"));
                                    d.insert("error".to_string(), Value::from("cancelled"));
                                    d.insert("posted".to_string(), Value::from(posted));
                                    return Ok(Some(d));
                                }
                            }
                            None => {
                                // The budget-stop entry does not carry `posted`;
                                // the release posts still fire.
                                for (msg, w, l) in &ups {
                                    bg_post(tgt, *msg, *w, *l);
                                }
                                return Ok(Some(budget_stop(i, &typ_v, &t0, budget, "a held key")));
                            }
                        }
                    }
                    for (msg, w, l) in &ups {
                        if !bg_post(tgt, *msg, *w, *l) {
                            failed = true;
                            break;
                        }
                        posted += 1;
                    }
                    if failed {
                        break;
                    }
                    if r + 1 < repeat && gap_ms > 0 && !dry_run {
                        match bounded_pause(&t_all, budget, gap_ms as u64) {
                            Some(g) => {
                                if sleep_cancellable(g) || crate::cancel::cancelled() {
                                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                                    d.insert("delivered".to_string(), Value::from("posted"));
                                    d.insert("effect".to_string(), Value::from("unverified"));
                                    d.insert("error".to_string(), Value::from("cancelled"));
                                    d.insert("posted".to_string(), Value::from(posted));
                                    return Ok(Some(d));
                                }
                            }
                            None => {
                                return Ok(Some(budget_stop(
                                    i,
                                    &typ_v,
                                    &t0,
                                    budget,
                                    "a key repeat gap",
                                )));
                            }
                        }
                    }
                }
                let mut d = base_entry(i, &typ_v, !failed, elapsed_ms(&t0));
                d.insert("delivered".to_string(), Value::from("posted"));
                d.insert("posted".to_string(), Value::from(posted));
                if failed {
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert(
                        "error".to_string(),
                        Value::from("PostMessage failed: the target did not accept the key"),
                    );
                    return Ok(Some(d));
                }
                if !verify {
                    d.insert("effect".to_string(), Value::from("unverified"));
                    return Ok(Some(d));
                }
                let after = match verify_capture(tgt) {
                    Ok(im) => im,
                    Err(e) => {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                        return Ok(Some(d));
                    }
                };
                let before = before.expect("verify:true captured before");
                match crate::cap::pixel_diff(&before, &after, 0, None) {
                    Ok(diff) => {
                        d.insert(
                            "effect".to_string(),
                            Value::from(if diff.identical {
                                "unchanged"
                            } else {
                                "changed"
                            }),
                        );
                        d.insert("changed".to_string(), Value::from(diff.changed));
                        d.insert(
                            "bbox".to_string(),
                            match diff.bbox {
                                Some(b) => Value::from(vec![
                                    Value::from(b[0]),
                                    Value::from(b[1]),
                                    Value::from(b[2]),
                                    Value::from(b[3]),
                                ]),
                                None => Value::Null,
                            },
                        );
                    }
                    Err(e) => {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                    }
                }
                return Ok(Some(d));
            }
            if typ == "hotkey" {
                let keys: Vec<String> = match ad.get("keys") {
                    None => vec![],
                    Some(Value::Array(arr)) => arr.iter().map(py_str).collect(),
                    Some(Value::String(s)) => s.chars().map(|c| c.to_string()).collect(),
                    Some(v) => {
                        return Err(format!("'{}' object is not iterable", py_type_name(v)));
                    }
                };
                if keys.is_empty() {
                    return Err("hotkey needs non-empty keys".to_string());
                }
                let mut codes = Vec::with_capacity(keys.len());
                for k in &keys {
                    codes.push(vk_of(k)?);
                }
                let hold_ms = opt_py_int(ad, "hold_ms", 80)?;
                if !(0..=60000).contains(&hold_ms) {
                    return Err(format!("hold_ms out of range 0..60000 (got {hold_ms})"));
                }
                let verify = match ad.get("verify") {
                    None | Some(Value::Null) => true,
                    Some(Value::Bool(b)) => *b,
                    Some(v) => {
                        return Err(format!(
                            "verify must be a boolean (got {})",
                            crate::py_repr_value(v)
                        ));
                    }
                };
                let Some(lock) = target_lock.as_ref() else {
                    let e = crate::gate::GateError {
                        code: crate::gate::TARGET_REQUIRED,
                        message: "hotkey needs an explicit target window; this server never \
                                  falls back to the foreground window. Nothing was sent."
                            .to_string(),
                    };
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("code".to_string(), Value::from(e.code));
                    d.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(d));
                };
                let tgt = lock.hwnd;
                if let Some(msg) = uipi_refusal(tgt) {
                    let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    d.insert("error".to_string(), Value::from(msg));
                    d.insert("blocked_by_uipi".to_string(), Value::from(true));
                    return Ok(Some(d));
                }
                if dry_run {
                    let mut d = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    d.insert("dry_run".to_string(), Value::from(true));
                    d.insert("delivered".to_string(), Value::from("posted"));
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert("posted".to_string(), Value::from(0));
                    return Ok(Some(d));
                }
                let before = match (verify, verify_capture(tgt)) {
                    (true, Ok(im)) => Some(im),
                    (true, Err(e)) => {
                        let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        d.insert("delivered".to_string(), Value::from("none"));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                        return Ok(Some(d));
                    }
                    (false, _) => None,
                };
                let downs: Vec<(u32, u64, LPARAM)> =
                    codes.iter().flat_map(posted_key_downs).collect();
                let ups: Vec<(u32, u64, LPARAM)> =
                    codes.iter().rev().flat_map(posted_key_ups).collect();
                let mut posted = 0i64;
                let mut failed = false;
                for (msg, w, l) in &downs {
                    if !bg_post(tgt, *msg, *w, *l) {
                        failed = true;
                        break;
                    }
                    posted += 1;
                }
                if !failed && hold_ms > 0 {
                    match bounded_pause(&t_all, budget, hold_ms as u64) {
                        Some(h) => {
                            if sleep_cancellable(h) || crate::cancel::cancelled() {
                                for (msg, w, l) in &ups {
                                    if bg_post(tgt, *msg, *w, *l) {
                                        posted += 1;
                                    }
                                }
                                let mut d = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                                d.insert("delivered".to_string(), Value::from("posted"));
                                d.insert("effect".to_string(), Value::from("unverified"));
                                d.insert("error".to_string(), Value::from("cancelled"));
                                d.insert("posted".to_string(), Value::from(posted));
                                return Ok(Some(d));
                            }
                        }
                        None => {
                            // The budget-stop entry does not carry `posted`;
                            // the release posts still fire.
                            for (msg, w, l) in &ups {
                                bg_post(tgt, *msg, *w, *l);
                            }
                            return Ok(Some(budget_stop(i, &typ_v, &t0, budget, "a held hotkey")));
                        }
                    }
                }
                if !failed {
                    for (msg, w, l) in &ups {
                        if !bg_post(tgt, *msg, *w, *l) {
                            failed = true;
                            break;
                        }
                        posted += 1;
                    }
                }
                let mut d = base_entry(i, &typ_v, !failed, elapsed_ms(&t0));
                d.insert("delivered".to_string(), Value::from("posted"));
                d.insert("posted".to_string(), Value::from(posted));
                if failed {
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert(
                        "error".to_string(),
                        Value::from("PostMessage failed: the target did not accept the hotkey"),
                    );
                    return Ok(Some(d));
                }
                if !verify {
                    d.insert("effect".to_string(), Value::from("unverified"));
                    return Ok(Some(d));
                }
                let after = match verify_capture(tgt) {
                    Ok(im) => im,
                    Err(e) => {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                        return Ok(Some(d));
                    }
                };
                let before = before.expect("verify:true captured before");
                match crate::cap::pixel_diff(&before, &after, 0, None) {
                    Ok(diff) => {
                        d.insert(
                            "effect".to_string(),
                            Value::from(if diff.identical {
                                "unchanged"
                            } else {
                                "changed"
                            }),
                        );
                        d.insert("changed".to_string(), Value::from(diff.changed));
                        d.insert(
                            "bbox".to_string(),
                            match diff.bbox {
                                Some(b) => Value::from(vec![
                                    Value::from(b[0]),
                                    Value::from(b[1]),
                                    Value::from(b[2]),
                                    Value::from(b[3]),
                                ]),
                                None => Value::Null,
                            },
                        );
                    }
                    Err(e) => {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("effect".to_string(), Value::from("unverified"));
                        d.insert("error".to_string(), Value::from(e));
                    }
                }
                return Ok(Some(d));
            }
            if typ == "clipboard_set" {
                // The gate refuses unflagged writes before this branch, so
                // no clipboard API is reached without the opt-in. Keep the
                // check here as defence in depth.
                if !args.allow_clipboard {
                    let e = crate::gate::no_clipboard_policy("clipboard_set");
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("code".to_string(), Value::from(e.code));
                    m.insert("error".to_string(), Value::from(e.text()));
                    if dry_run {
                        m.insert("dry_run".to_string(), Value::from(true));
                    }
                    return Ok(Some(m));
                }
                let text = match ad.get("text").and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        m.insert(
                            "error".to_string(),
                            Value::from("clipboard_set needs a text string"),
                        );
                        return Ok(Some(m));
                    }
                };
                let n_chars = text.chars().count();
                if let Err(e) = check_clip_len(n_chars) {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("code".to_string(), Value::from("too_large"));
                    m.insert("error".to_string(), Value::from(format!("too_large: {e}")));
                    return Ok(Some(m));
                }
                if dry_run {
                    let mut m = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    m.insert("dry_run".to_string(), Value::from(true));
                    m.insert("delivered".to_string(), Value::from("none"));
                    m.insert("effect".to_string(), Value::from("unverified"));
                    m.insert("text_chars".to_string(), Value::from(n_chars as i64));
                    return Ok(Some(m));
                }
                // Snapshot before touching the owner's clipboard; a failed
                // snapshot refuses the write instead of losing content.
                if let Err(e) = clip.before_write() {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("code".to_string(), Value::from("clipboard_unavailable"));
                    m.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(m));
                }
                match clipboard_set_text(text) {
                    Ok((bytes, attempts)) => {
                        clip.wrote(bytes);
                        let mut m = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                        m.insert("delivered".to_string(), Value::from("none"));
                        m.insert("effect".to_string(), Value::from("unverified"));
                        m.insert("bytes".to_string(), Value::from(bytes as i64));
                        m.insert("attempts".to_string(), Value::from(attempts as i64));
                        return Ok(Some(m));
                    }
                    Err(e) => {
                        let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        m.insert("code".to_string(), Value::from("clipboard_unavailable"));
                        m.insert("error".to_string(), Value::from(e.text()));
                        return Ok(Some(m));
                    }
                }
            }
            if typ == "clipboard_get" {
                let (_, ferr) = st.flush();
                if let Some(fe) = ferr {
                    fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                } else {
                    st.marked.clear();
                }
                if dry_run {
                    // No OpenClipboard/GetClipboardData on a dry run: report
                    // the resolved action without reading the owner's data.
                    let mut m = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                    m.insert("dry_run".to_string(), Value::from(true));
                    return Ok(Some(m));
                }
                match clipboard_get() {
                    Ok((txt, chars, truncated, attempts)) => {
                        let mut m = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                        m.insert("text".to_string(), Value::from(txt));
                        m.insert("chars".to_string(), Value::from(chars as i64));
                        m.insert("truncated".to_string(), Value::from(truncated));
                        m.insert("attempts".to_string(), Value::from(attempts as i64));
                        if dry_run {
                            m.insert("dry_run".to_string(), Value::from(true));
                        }
                        return Ok(Some(m));
                    }
                    Err(e) => {
                        let (code, text) = e.wire();
                        let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                        m.insert("code".to_string(), Value::from(code));
                        m.insert("error".to_string(), Value::from(text));
                        return Ok(Some(m));
                    }
                }
            }
            if typ == "invoke" {
                let (_, ferr) = st.flush();
                if let Some(fe) = ferr {
                    fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                } else {
                    st.marked.clear();
                }
                let mut d = base_entry(i, &typ_v, false, 0.0);
                if bg_mode && (bg_error.is_some() || bg_hwnd.is_none()) {
                    d.insert(
                        "error".to_string(),
                        Value::from(
                            bg_error
                                .clone()
                                .unwrap_or("bad_window: no background target".to_string()),
                        ),
                    );
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let tgt = match (bg_mode, bg_hwnd) {
                    (true, Some(h)) => h,
                    _ => {
                        d.insert(
                            "error".to_string(),
                            Value::from(format!(
                                "target_required: {typ} needs an explicit target window \
                                 (target:{{\"hwnd\":..}} or another field); this server never \
                                 falls back to the foreground window"
                            )),
                        );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                };
                d.insert("target_hwnd".to_string(), Value::from(tgt));
                if tgt == 0 {
                    d.insert(
                        "error".to_string(),
                        Value::from(
                            "bad_window: invoke needs a background target or a foreground window",
                        ),
                    );
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                if let Some(msg) = uipi_refusal(tgt) {
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert("error".to_string(), Value::from(msg));
                    d.insert("blocked_by_uipi".to_string(), Value::from(true));
                    d.insert("resolution".to_string(), Value::from("uipi_blocked"));
                    d.insert("via".to_string(), Value::from("none"));
                    if dry_run {
                        d.insert("dry_run".to_string(), Value::from(true));
                    }
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let model = element_model(tgt as HWND, crate::UIA_DEPTH_LIMIT, 0);
                if dry_run {
                    d.insert("dry_run".to_string(), Value::from(true));
                }
                d.insert("backend".to_string(), Value::from(model.backend.clone()));
                let (entry, merr, mcands) =
                    match_element(&model.entries, ad.get("element"), ad.get("index"));
                if let Some(me) = merr {
                    d.insert("error".to_string(), Value::from(me));
                    d.insert(
                        "resolution".to_string(),
                        Value::from(if mcands.is_some() {
                            "ambiguous"
                        } else {
                            "not-found"
                        }),
                    );
                    if let Some(c) = mcands {
                        d.insert(
                            "candidates".to_string(),
                            Value::from(c.into_iter().map(Value::Object).collect::<Vec<_>>()),
                        );
                    }
                    if dry_run {
                        d.insert("via".to_string(), Value::from("none"));
                    }
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let entry = entry.unwrap();
                if let Some(se) = check_expect_fp(
                    ad,
                    &entry,
                    &py_str(ad.get("element").unwrap_or(&Value::Null)),
                    true,
                ) {
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert("code".to_string(), Value::from(se.code));
                    d.insert("error".to_string(), Value::from(se.text));
                    d.insert("resolution".to_string(), Value::from(se.resolution));
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                let expand_req = match expand_arg(ad.get("state")) {
                    Ok(r) => r,
                    Err(e) => {
                        let mut d = base_entry(i, &typ_v, false, 0.0);
                        d.insert("target_hwnd".to_string(), Value::from(tgt));
                        d.insert("backend".to_string(), Value::from(model.backend.clone()));
                        d.insert("error".to_string(), Value::from(e));
                        d.insert("resolution".to_string(), Value::from("not-found"));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                };
                if dry_run {
                    d.insert("ok".to_string(), Value::from(true));
                    d.insert("resolution".to_string(), Value::from("found"));
                    d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
                d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                d.insert("resolution".to_string(), Value::from("found"));
                let path = entry_str(&entry, "path");
                let uia_act = model.backend == "uia";
                let before = if uia_act {
                    action_state(tgt, &path)
                } else {
                    None
                };
                let (ok_v, via, verr) = if let Some(want) = expand_req {
                    if !uia_act {
                        (
                            false,
                            "none".to_string(),
                            Some(
                                "invoke state needs the UIA backend (Win32 fallback has no ExpandCollapse)"
                                    .to_string(),
                            ),
                        )
                    } else {
                        let (ok_e, via_e, err_e) =
                            crate::uia::uia_expand_collapse(tgt, &path, want);
                        (ok_e, via_e, err_e)
                    }
                } else if uia_act {
                    crate::uia::uia_activate(tgt, &path)
                } else {
                    win32_activate(&entry)
                };
                let fallback = match ad.get("fallback") {
                    None => String::new(),
                    Some(v) => py_str(v).to_lowercase(),
                };
                if !ok_v && fallback == "click" && expand_req.is_none() {
                    let refusal = crate::gate::no_cursor_policy("fallback:\"click\"");
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert(
                        "code".to_string(),
                        Value::from(crate::gate::NO_CURSOR_POLICY),
                    );
                    d.insert("error".to_string(), Value::from(refusal.text()));
                    d.insert("via".to_string(), Value::from("none"));
                    d.insert("effect".to_string(), Value::from("unverified"));
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                } else {
                    d.insert("ok".to_string(), Value::from(ok_v));
                }
                d.insert(
                    "via".to_string(),
                    Value::from(if via.is_empty() {
                        "none".to_string()
                    } else {
                        via
                    }),
                );
                let ok_now = d.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(e) = verr {
                    if !ok_now {
                        d.insert("error".to_string(), Value::from(e));
                    }
                }
                if uia_act {
                    let effect = if ok_now {
                        effect_after(tgt, &path, before.clone())
                    } else {
                        "unverified"
                    };
                    d.insert("effect".to_string(), Value::from(effect));
                    if effect == "unchanged" {
                        let b = before.unwrap_or_default();
                        mark_unchanged(&mut d, &b);
                    }
                }
                if expand_req.is_some() && uia_act {
                    if let Ok(st) = crate::uia::uia_expand_state(tgt, &entry_str(&entry, "path")) {
                        d.insert(
                            "expand_state".to_string(),
                            Value::from(crate::uia::expand_state_name(st)),
                        );
                    }
                }
                d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                return Ok(Some(d));
            }
            if typ == "set_value" {
                if !crate::gate::uia_text_permitted(private_desktop) {
                    let sel = ad.get("element").map(py_str).unwrap_or_default();
                    let e = crate::gate::uia_text_unavailable(sel.trim());
                    let mut dd = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    dd.insert("code".to_string(), Value::from(e.code));
                    dd.insert("error".to_string(), Value::from(e.text()));
                    return Ok(Some(dd));
                }
                #[allow(unreachable_code)]
                {
                    let (_, ferr) = st.flush();
                    if let Some(fe) = ferr {
                        fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                    } else {
                        st.marked.clear();
                    }
                    let mut d = base_entry(i, &typ_v, false, 0.0);
                    if bg_mode && (bg_error.is_some() || bg_hwnd.is_none()) {
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                bg_error
                                    .clone()
                                    .unwrap_or("bad_window: no background target".to_string()),
                            ),
                        );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let tgt = match (bg_mode, bg_hwnd) {
                        (true, Some(h)) => h,
                        _ => {
                            d.insert(
                                "error".to_string(),
                                Value::from(format!(
                                    "target_required: {typ} needs an explicit target window \
                                 (target:{{\"hwnd\":..}} or another field); this server never \
                                 falls back to the foreground window"
                                )),
                            );
                            d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                            return Ok(Some(d));
                        }
                    };
                    d.insert("target_hwnd".to_string(), Value::from(tgt));
                    if tgt == 0 {
                        d.insert(
                        "error".to_string(),
                        Value::from(
                            "bad_window: set_value needs a background target or a foreground window",
                        ),
                    );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    if let Some(msg) = uipi_refusal(tgt) {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("error".to_string(), Value::from(msg));
                        d.insert("blocked_by_uipi".to_string(), Value::from(true));
                        d.insert("resolution".to_string(), Value::from("uipi_blocked"));
                        d.insert("via".to_string(), Value::from("none"));
                        if dry_run {
                            d.insert("dry_run".to_string(), Value::from(true));
                        }
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let model = element_model(tgt as HWND, crate::UIA_DEPTH_LIMIT, 0);
                    if dry_run {
                        d.insert("dry_run".to_string(), Value::from(true));
                    }
                    d.insert("backend".to_string(), Value::from(model.backend.clone()));
                    let elem_ok = matches!(ad.get("element"), Some(v) if py_truthy(v) && !py_str(v).trim().is_empty());
                    if !elem_ok {
                        d.insert(
                            "error".to_string(),
                            Value::from("set_value needs \"element\" (e.g. \"name:OK\")"),
                        );
                        d.insert("resolution".to_string(), Value::from("not-found"));
                        if dry_run {
                            d.insert("via".to_string(), Value::from("none"));
                        }
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let (entry, merr, mcands) =
                        match_element(&model.entries, ad.get("element"), ad.get("index"));
                    if let Some(me) = merr {
                        d.insert("error".to_string(), Value::from(me));
                        d.insert(
                            "resolution".to_string(),
                            Value::from(if mcands.is_some() {
                                "ambiguous"
                            } else {
                                "not-found"
                            }),
                        );
                        if let Some(c) = mcands {
                            d.insert(
                                "candidates".to_string(),
                                Value::from(c.into_iter().map(Value::Object).collect::<Vec<_>>()),
                            );
                        }
                        if dry_run {
                            d.insert("via".to_string(), Value::from("none"));
                        }
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let entry = entry.unwrap();
                    if let Some(se) = check_expect_fp(
                        ad,
                        &entry,
                        &py_str(ad.get("element").unwrap_or(&Value::Null)),
                        !crate::gate::uia_text_permitted(private_desktop),
                    ) {
                        d.insert("ok".to_string(), Value::from(false));
                        d.insert("code".to_string(), Value::from(se.code));
                        d.insert("error".to_string(), Value::from(se.text));
                        d.insert("resolution".to_string(), Value::from(se.resolution));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let text: Option<String> = match ad.get("text") {
                        None | Some(Value::Null) => None,
                        Some(v) => Some(py_str(v)),
                    };
                    let text = match text {
                        Some(t) => t,
                        None => {
                            d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                            d.insert("resolution".to_string(), Value::from("found"));
                            d.insert("via".to_string(), Value::from("none"));
                            d.insert(
                            "error".to_string(),
                            Value::from(
                                "set_value needs \"text\" (e.g. {\"type\":\"set_value\",\"element\":\"name:OK\",\"text\":\"hello\"})",
                            ),
                        );
                            d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                            return Ok(Some(d));
                        }
                    };
                    let value_len = text.chars().count() as i64;
                    if dry_run {
                        d.insert("ok".to_string(), Value::from(true));
                        d.insert("resolution".to_string(), Value::from("found"));
                        d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert("value_len".to_string(), Value::from(value_len));
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    d.insert("element".to_string(), Value::from(element_pub_full(&entry)));
                    d.insert("resolution".to_string(), Value::from("found"));
                    if model.backend != "uia" {
                        d.insert("via".to_string(), Value::from("none"));
                        d.insert(
                        "error".to_string(),
                        Value::from(
                            "element has no ValuePattern under the Win32 backend (set_value needs the UIA backend)",
                        ),
                    );
                        d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                        return Ok(Some(d));
                    }
                    let path = entry_str(&entry, "path");
                    let before = action_state(tgt, &path);
                    let (ok_v, via, verr) = crate::uia::uia_set_value(tgt, &path, &text);
                    d.insert("delivered".to_string(), Value::from("uia"));
                    d.insert(
                        "via".to_string(),
                        Value::from(if via.is_empty() {
                            "none".to_string()
                        } else {
                            via
                        }),
                    );
                    d.insert("ok".to_string(), Value::from(ok_v));
                    let effect = if ok_v {
                        effect_after(tgt, &path, before.clone())
                    } else {
                        "unverified"
                    };
                    d.insert("effect".to_string(), Value::from(effect));
                    if effect == "unchanged" {
                        let b = before.unwrap_or_default();
                        mark_unchanged(&mut d, &b);
                    }
                    if ok_v {
                        d.insert("value_len".to_string(), Value::from(value_len));
                        if let Ok(v) = crate::uia::uia_range_value(tgt, &path) {
                            d.insert("value".to_string(), Value::from(v));
                        }
                    }
                    if let Some(e) = verr {
                        if !ok_v {
                            d.insert("error".to_string(), Value::from(e));
                        }
                    }
                    d.insert("ms".to_string(), Value::from(elapsed_ms(&t0)));
                    return Ok(Some(d));
                }
            }
            if typ == "wait" {
                let (_, ferr) = st.flush();
                if let Some(fe) = ferr {
                    fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                } else {
                    st.marked.clear();
                }
                let raw = ad.get("ms").cloned().unwrap_or(Value::from(0));
                let mut wait_n = py_int(&raw)
                    .map_err(|_| format!("wait ms must be an int (got {})", py_repr_value(&raw)))?;
                if wait_n < 0 {
                    wait_n = 0;
                } else if wait_n > 60000 {
                    return Err(format!("wait ms absurd (>60000): {wait_n}"));
                }
                let left_ms = budget_left_ms(&t_all, budget);
                if wait_n > left_ms {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("code".to_string(), Value::from("burst_budget_exceeded"));
                    m.insert(
                        "error".to_string(),
                        Value::from(format!(
                            "burst_budget_exceeded: wait {wait_n}ms exceeds the remaining {left_ms}ms of the {}-second burst budget",
                            budget.as_secs_f64()
                        )),
                    );
                    return Ok(Some(m));
                }
                if sleep_cancellable(wait_n as u64) {
                    let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                    m.insert("error".to_string(), Value::from("cancelled"));
                    return Ok(Some(m));
                }
                return Ok(Some(base_entry(i, &typ_v, true, elapsed_ms(&t0))));
            }
            if typ == "wait_for" {
                let (_, ferr) = st.flush();
                if let Some(fe) = ferr {
                    fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
                } else {
                    st.marked.clear();
                }
                let left_ms = budget_left_ms(&t_all, budget);
                let want_ms = opt_py_int(ad, "timeout_ms", 5000).unwrap_or(5000);
                let wf_action: Map<String, Value> = if want_ms > left_ms {
                    let mut m = ad.clone();
                    m.insert("timeout_ms".to_string(), Value::from(left_ms.max(0)));
                    m
                } else {
                    ad.clone()
                };
                let wr = wait_for(&wf_action, &t0);
                let mut d = Map::new();
                d.insert("index".to_string(), Value::from(i as i64));
                d.insert("type".to_string(), typ_v.clone());
                d.insert("ok".to_string(), wr["ok"].clone());
                d.insert("satisfied".to_string(), wr["satisfied"].clone());
                d.insert("waited_ms".to_string(), wr["waited_ms"].clone());
                d.insert("ms".to_string(), wr["waited_ms"].clone());
                if let Some(e) = wr.get("error") {
                    d.insert("error".to_string(), e.clone());
                }
                if ad.get("kind").and_then(|k| k.as_str()) == Some("element") {
                    d.insert(
                        "satisfied_by".to_string(),
                        wr.get("satisfied_by").cloned().unwrap_or(Value::Null),
                    );
                    if let Some(m) = wr.get("fp_matched") {
                        d.insert("fp_matched".to_string(), m.clone());
                    }
                }
                if want_ms > left_ms && wr.get("satisfied") != Some(&Value::from(true)) {
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert("code".to_string(), Value::from("burst_budget_exceeded"));
                    d.insert(
                        "error".to_string(),
                        Value::from(format!(
                            "burst_budget_exceeded: wait_for timeout {want_ms}ms exceeds the remaining {left_ms}ms of the {}-second burst budget",
                            budget.as_secs_f64()
                        )),
                    );
                }
                return Ok(Some(d));
            }
            if typ == "focus_window" {
                let lock = match focus_lock(ad.get("title"), ad.get("hwnd")) {
                    Ok(l) => l,
                    Err(e) => return Ok(Some(target_failure(i, &typ_v, &t0, &e))),
                };
                if let Err(e) = lock.revalidate() {
                    return Ok(Some(target_failure(i, &typ_v, &t0, &e)));
                }
                let mut d = base_entry(i, &typ_v, true, elapsed_ms(&t0));
                d.insert("hwnd".to_string(), Value::from(lock.hwnd));
                d.insert("delivered".to_string(), Value::from("foreground"));
                d.insert("via".to_string(), Value::from("setforegroundwindow"));
                if gate_delivery == crate::gate::Delivery::Foreground && !dry_run {
                    // Explicit opt-in only. This is the sole call site of
                    // SetForegroundWindow in a burst; no other verb reaches it.
                    let ok = unsafe { SetForegroundWindow(lock.hwnd as HWND) != 0 };
                    d.insert("ok".to_string(), Value::from(ok));
                    if !ok {
                        d.insert(
                            "error".to_string(),
                            Value::from(
                                "SetForegroundWindow was refused by Windows; no other window was changed".to_string(),
                            ),
                        );
                    }
                } else {
                    d.insert("via".to_string(), Value::from("none"));
                    let e = crate::gate::no_foreground_policy("focus_window");
                    d.insert("ok".to_string(), Value::from(false));
                    d.insert("code".to_string(), Value::from(e.code));
                    d.insert("error".to_string(), Value::from(e.text()));
                }
                if dry_run {
                    d.insert("dry_run".to_string(), Value::from(true));
                }
                return Ok(Some(d));
            }
            let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
            m.insert(
                "error".to_string(),
                Value::from(format!("unknown action type \"{}\"", py_str(&typ_v))),
            );
            Ok(Some(m))
        })();

        match step {
            Ok(Some(m)) => st.results.push(m),
            Ok(None) => {}
            Err(e) => {
                let mut m = base_entry(i, &typ_v, false, elapsed_ms(&t0));
                m.insert("error".to_string(), Value::from(e));
                st.results.push(m);
            }
        }
    }

    let (_, ferr) = st.flush();
    if let Some(fe) = ferr {
        fail_since_flush(&mut st.results, &mut st.marked, &fe, st.flush_uipi);
    } else {
        st.marked.clear();
    }
    let mut settle_applied: i64 = 0;
    if settle_n != 0 && !crate::cancel::cancelled() {
        if let Some(p) = bounded_pause(&t_all, budget, settle_n as u64) {
            settle_applied = p as i64;
            sleep_cancellable(p);
        }
    }
    // Restore after the last action, including a failed or partial burst.
    // Nothing holds a clipboard handle between actions, so the owner is
    // never blocked for the length of the burst.
    clip.finish();
    // The entry pointer is put back after the LAST action, including a
    // partial or failed burst. A refused action never queued input, so no
    // restore is attempted and cursorRestored stays null.
    let cursor_mid = cursor_now();
    let mut cursor_restored: Option<bool> = None;
    let cursor_used = args.allow_cursor && !dry_run && st.pointer_moved();
    let entry_readable = (i32::MIN as i64..=i32::MAX as i64).contains(&cursor_before.0)
        && (i32::MIN as i64..=i32::MAX as i64).contains(&cursor_before.1)
        && cursor_before.0 >= 0
        && cursor_before.1 >= 0;
    if cursor_used && cursor_restore && entry_readable {
        let ok = unsafe { SetCursorPos(cursor_before.0 as i32, cursor_before.1 as i32) != 0 };
        cursor_restored = Some(ok);
    }
    let total_ms = t_all.elapsed().as_secs_f64() * 1000.0;
    let focus_after = fg_now();
    for r in st.results.iter_mut() {
        if !r.contains_key("mode") {
            r.insert(
                "mode".to_string(),
                Value::from(if bg_mode { "background" } else { "foreground" }),
            );
        }
        if bg_mode && !r.contains_key("bg_hwnd") {
            r.insert(
                "bg_hwnd".to_string(),
                bg_hwnd.map(Value::from).unwrap_or(Value::Null),
            );
        }
    }
    let mut focus_restored = true;
    let mut foreground_restored: Option<bool> = None;
    let mut focus_guard_refusal: Option<String> = None;
    if focus_guard && focus_after != focus_before && focus_before != 0 {
        if dry_run || !args.allow_foreground {
            focus_restored = false;
            focus_guard_refusal =
                Some(crate::gate::no_foreground_policy("focus_guard restore").text());
        } else {
            let ok = unsafe { SetForegroundWindow(focus_before as HWND) != 0 };
            foreground_restored = Some(ok);
            focus_restored = ok;
            if !ok {
                focus_guard_refusal = Some(
                    "focus_guard: SetForegroundWindow could not restore the entry window"
                        .to_string(),
                );
            }
        }
    }
    let mut hidden_meta: Option<Map<String, Value>> = None;
    let mut hidden_png: Option<Vec<u8>> = None;
    if let Some(owner) = hidden_owner.take() {
        if !dry_run && bg_error.is_none() && hidden_hwnd.is_some() {
            sleep_cancellable(1000);
        }
        let mut hd = Map::new();
        hd.insert("requested".to_string(), Value::from("hidden"));
        hd.insert("desktop_name".to_string(), Value::from(hidden_name.clone()));
        hd.insert("desktop_created".to_string(), Value::from(hidden_created));
        hd.insert(
            "hard_kill_cover".to_string(),
            Value::from(owner.hard_kill_cover),
        );
        hd.insert(
            "set_thread_desktop".to_string(),
            Value::from(hidden_set_desktop),
        );
        hd.insert(
            "launched_pid".to_string(),
            hidden_pid
                .map(|p| Value::from(p as i64))
                .unwrap_or(Value::Null),
        );
        hd.insert(
            "window_hwnd".to_string(),
            hidden_hwnd.map(Value::from).unwrap_or(Value::Null),
        );
        hd.insert(
            "window_class".to_string(),
            Value::from(hidden_class.clone()),
        );
        hd.insert(
            "window_title".to_string(),
            Value::from(hidden_title.clone()),
        );
        hd.insert("window_pid".to_string(), Value::from(hidden_win_pid as i64));
        hd.insert(
            "window_on_hidden_desktop".to_string(),
            Value::from(hidden_hwnd.is_some()),
        );
        let owner_absent = match hidden_hwnd {
            Some(h) => !crate::enum_windows().iter().any(|w| *w as i64 == h),
            None => false,
        };
        hd.insert(
            "absent_from_owner_enum_windows".to_string(),
            Value::from(owner_absent),
        );
        let done = owner.teardown();
        let cursor_after = cursor_now();
        let fg_end = fg_now();
        hd.insert("window_alive".to_string(), Value::from(done.alive));
        hd.insert(
            "text_readback".to_string(),
            done.text.clone().map(Value::from).unwrap_or(Value::Null),
        );
        hd.insert(
            "pre_drive_text".to_string(),
            done.pre_drive_text
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        hd.insert(
            "post_revert_text".to_string(),
            done.post_revert_text
                .clone()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        hd.insert("reverted".to_string(), Value::from(done.reverted));
        match done.capture.as_ref() {
            Some(c) => {
                let mut cap = Map::new();
                cap.insert("ok".to_string(), Value::from(c.ok));
                cap.insert("print_flags".to_string(), Value::from(c.flags as i64));
                cap.insert("w".to_string(), Value::from(c.w as i64));
                cap.insert("h".to_string(), Value::from(c.h as i64));
                cap.insert("min".to_string(), Value::from(c.min as i64));
                cap.insert("max".to_string(), Value::from(c.max as i64));
                cap.insert("uniform".to_string(), Value::from(c.min == c.max));
                if let Some(png) = c.png.as_ref() {
                    cap.insert("bytes".to_string(), Value::from(png.len() as i64));
                    cap.insert(
                        "png_b64".to_string(),
                        Value::from(base64::engine::general_purpose::STANDARD.encode(png)),
                    );
                    if c.ok && !png.is_empty() {
                        hidden_png = Some(png.clone());
                    }
                }
                if let Some(e) = c.error.as_ref() {
                    cap.insert("error".to_string(), Value::from(e.clone()));
                }
                hd.insert("capture".to_string(), Value::Object(cap));
            }
            None => {
                hd.insert("capture".to_string(), Value::Null);
            }
        }
        let mut td = Map::new();
        td.insert("close_posted".to_string(), Value::from(done.close_posted));
        td.insert(
            "process_exited".to_string(),
            Value::from(done.process_exited),
        );
        td.insert("terminated".to_string(), Value::from(done.terminated));
        td.insert("desktop_empty".to_string(), Value::from(done.desktop_empty));
        td.insert(
            "set_thread_desktop_restored".to_string(),
            Value::from(done.restored_desktop),
        );
        td.insert(
            "close_desktop".to_string(),
            Value::from(done.closed_desktop),
        );
        td.insert(
            "open_after_close_null".to_string(),
            Value::from(done.open_after_close_null),
        );
        td.insert("proven".to_string(), Value::from(done.teardown_proven));
        if let Some(e) = done.error {
            td.insert("error".to_string(), Value::from(e));
        }
        hd.insert("teardown".to_string(), Value::Object(td));
        hd.insert(
            "owner_cursor_delta".to_string(),
            Value::from(if cursor_after == cursor_before { 0 } else { 1 }),
        );
        hd.insert(
            "cursor_before".to_string(),
            Value::from(vec![
                Value::from(cursor_before.0),
                Value::from(cursor_before.1),
            ]),
        );
        hd.insert(
            "cursor_after".to_string(),
            Value::from(vec![
                Value::from(cursor_after.0),
                Value::from(cursor_after.1),
            ]),
        );
        hd.insert(
            "owner_foreground_delta".to_string(),
            Value::from(if fg_end == focus_before { 0 } else { 1 }),
        );
        hd.insert("foreground_before".to_string(), Value::from(focus_before));
        hd.insert("foreground_after".to_string(), Value::from(fg_end));
        hidden_meta = Some(hd);
    } else if hidden_launch_requested {
        let mut hd = Map::new();
        hd.insert("requested".to_string(), Value::from("hidden"));
        hd.insert("desktop_name".to_string(), Value::from(""));
        hd.insert("desktop_created".to_string(), Value::from(false));
        hd.insert("hard_kill_cover".to_string(), Value::from(false));
        hd.insert("launched_pid".to_string(), Value::Null);
        hd.insert("window_hwnd".to_string(), Value::Null);
        hd.insert("teardown".to_string(), Value::Null);
        hidden_meta = Some(hd);
    }
    if dry_run {
        normalize_dry_results(&mut st.results);
        // Defence in depth: the delivery gates already keep these at zero;
        // make the whole-call contract explicit for a dry burst.
        posted_total = 0;
        st.calls = 0;
        st.events = 0;
    }
    let mut payload = Map::new();
    let failed = st
        .results
        .iter()
        .filter(|r| r.get("ok") != Some(&Value::from(true)))
        .count();
    let launch_failed = hidden_launch_requested && !hidden_created;
    payload.insert("ok".to_string(), Value::from(failed == 0 && !launch_failed));
    payload.insert("failed".to_string(), Value::from(failed as i64));
    payload.insert(
        "results".to_string(),
        Value::from(
            st.results
                .into_iter()
                .map(Value::Object)
                .collect::<Vec<_>>(),
        ),
    );
    payload.insert("total_ms".to_string(), Value::from(py_round_n(total_ms, 3)));
    if settle_n != 0 {
        payload.insert("settle_requested_ms".to_string(), Value::from(settle_n));
        payload.insert("settle_ms".to_string(), Value::from(settle_applied));
    }
    payload.insert("sendinput_calls".to_string(), Value::from(st.calls));
    payload.insert(
        "sendinput_ms".to_string(),
        Value::from(py_round_n(st.sendinput_ms, 3)),
    );
    payload.insert("events".to_string(), Value::from(st.events));
    payload.insert("clipboard".to_string(), Value::Object(clip.to_map()));
    payload.insert("focus_before".to_string(), Value::from(focus_before));
    payload.insert("focus_after".to_string(), Value::from(focus_after));
    if focus_guard {
        payload.insert("focus_restored".to_string(), Value::from(focus_restored));
        payload.insert(
            "foregroundRestored".to_string(),
            foreground_restored.map(Value::from).unwrap_or(Value::Null),
        );
        if let Some(r) = focus_guard_refusal {
            payload.insert("focus_guard_error".to_string(), Value::from(r));
        }
    }
    let cursor_after = cursor_now();
    payload.insert(
        "cursorUnchanged".to_string(),
        Value::from(cursor_after == cursor_before),
    );
    payload.insert(
        "cursorMoved".to_string(),
        Value::from(cursor_used && cursor_mid != cursor_before),
    );
    payload.insert(
        "cursorRestored".to_string(),
        cursor_restored.map(Value::from).unwrap_or(Value::Null),
    );
    payload.insert(
        "foregroundUnchanged".to_string(),
        Value::from(fg_now() == focus_before),
    );
    if let Some(l) = &target_lock {
        payload.insert("target_locked".to_string(), Value::from(l.hwnd));
        payload.insert("target_pid".to_string(), Value::from(l.pid as i64));
        payload.insert("target_exe".to_string(), Value::from(l.exe.clone()));
        payload.insert("target_spec".to_string(), Value::from(l.spec.describe()));
        if let Some(id) = &l.app_id {
            payload.insert("appId".to_string(), Value::from(id.clone()));
        }
        if let Some(h) = &l.host {
            payload.insert("hostHwnd".to_string(), Value::from(h.hwnd));
        }
    }
    if bg_mode {
        payload.insert(
            "background_note".to_string(),
            Value::from(format!(
                "{BG_NOTE} The target window was never activated, focused, or raised."
            )),
        );
        payload.insert("posted_messages".to_string(), Value::from(posted_total));
        if let Some(be) = bg_error {
            payload.insert("background_error".to_string(), Value::from(be));
        }
    }
    if let Some(hd) = hidden_meta {
        payload.insert("hidden_desktop".to_string(), Value::Object(hd));
    }
    let text = crate::py_dumps(&Value::Object(payload));
    match hidden_png {
        Some(png) if !png.is_empty() => crate::wrapped_text_with_image(
            &text,
            &base64::engine::general_purpose::STANDARD.encode(&png),
            "image/png",
        ),
        _ => wrapped_text(&text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub resolver for the pure point guard: no window is created and no
    /// API is called, so the decision is tested without a desktop.
    fn owner(hit: i64, root: i64, inside: bool, descendant: bool) -> PointOwner {
        PointOwner {
            hit,
            root,
            inside_target_rect: inside,
            descendant,
            actual: format!("hwnd={hit} class=Other title=Other pid=9"),
        }
    }

    #[test]
    fn point_guard_accepts_only_the_locked_window_or_its_descendants() {
        let target = 4242;
        // The target itself, and a child inside it, are both deliverable.
        assert!(
            point_guard(target, "click", 10, 20, &owner(target, target, true, false)).is_ok(),
            "a point on the locked window must be deliverable"
        );
        assert!(
            point_guard(target, "click", 10, 20, &owner(9001, target, true, true)).is_ok(),
            "a descendant inside the target must be deliverable"
        );
        // Outside the target's rect: same typed refusal, even at the same hit.
        let e = point_guard(
            target,
            "click",
            10,
            20,
            &owner(target, target, false, false),
        )
        .expect_err("a point outside the rect must be refused");
        assert_eq!(e.code, crate::gate::POINT_NOT_TARGET);
        assert!(e.text().contains("(10,20)"), "{}", e.text());
        assert!(e.text().contains("hwnd=4242"), "{}", e.text());
        // Someone else's top-level window wins the point: refuse, name it.
        let e = point_guard(target, "click", 30, 40, &owner(7777, 7777, true, false))
            .expect_err("another window under the point must be refused");
        assert_eq!(e.code, crate::gate::POINT_NOT_TARGET);
        assert!(
            e.text().contains("hwnd=7777 class=Other"),
            "the refusal must name the actual window: {}",
            e.text()
        );
    }

    #[test]
    fn abs_pointer_events_carry_virtualdesk_on_every_click_event() {
        assert_ne!(ABS_MOVE_FLAGS & MOUSEEVENTF_ABSOLUTE, 0);
        assert_ne!(ABS_MOVE_FLAGS & MOUSEEVENTF_VIRTUALDESK, 0);
        // A click's move, button-down and button-up all carry the absolute
        // virtual-desktop flags, so a secondary monitor lands correctly.
        let seq = click_inputs(100, 200, "click", "left", 2).expect("click INPUTs");
        assert_eq!(seq.len(), 5, "move + two down/up pairs");
        for (k, input) in seq.iter().enumerate() {
            let flags = unsafe { input.Anonymous.mi }.dwFlags;
            assert_ne!(
                flags & MOUSEEVENTF_ABSOLUTE,
                0,
                "event {k} must be absolute"
            );
            assert_ne!(
                flags & MOUSEEVENTF_VIRTUALDESK,
                0,
                "event {k} must use the virtual desktop, not the primary monitor"
            );
        }
        assert_ne!(
            unsafe { seq[0].Anonymous.mi }.dwFlags & MOUSEEVENTF_MOVE,
            0,
            "the first event positions the pointer"
        );
        assert_ne!(
            unsafe { seq[1].Anonymous.mi }.dwFlags & MOUSEEVENTF_LEFTDOWN,
            0,
            "the second event presses the button"
        );
        assert_ne!(
            unsafe { seq[4].Anonymous.mi }.dwFlags & MOUSEEVENTF_LEFTUP,
            0,
            "the last event releases the button"
        );
    }

    /// Raw Win32 writer for the over-cap case: the production writer
    /// correctly refuses >1 MiB, so the test seeds the clipboard itself to
    /// prove the READER reports truncation honestly.
    fn clip_write_raw_over_cap(text: &str) {
        unsafe {
            let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let h = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
            assert!(!h.is_null(), "GlobalAlloc failed");
            let p = GlobalLock(h) as *mut u16;
            assert!(!p.is_null(), "GlobalLock failed");
            std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
            GlobalUnlock(h);
            assert_ne!(
                OpenClipboard(std::ptr::null_mut()),
                0,
                "OpenClipboard failed"
            );
            assert_ne!(EmptyClipboard(), 0, "EmptyClipboard failed");
            assert!(
                !SetClipboardData(13, h).is_null(),
                "SetClipboardData failed"
            );
            CloseClipboard();
        }
    }

    /// The 1 MiB read cap is honest: the action result now reports the
    /// original character count and whether the returned text was cut.
    #[test]
    fn clipboard_get_over_cap_reports_truncated_and_original_chars() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        let chars = MAX_CLIP_CHARS + 7;
        clip_write_raw_over_cap(&"x".repeat(chars));
        let payload = do_burst_clip_retry(&DoArgs::new(&[serde_json::json!({
            "type": "clipboard_get"
        })]));
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(true), "{first}");
        assert_eq!(first["truncated"], Value::from(true), "{first}");
        assert_eq!(first["chars"], Value::from(chars as i64), "{first}");
        assert_eq!(
            first["text"].as_str().unwrap_or("").chars().count(),
            MAX_CLIP_CHARS,
            "the returned text stays capped at 1 MiB"
        );
    }

    #[test]
    fn cursor_restore_defaults_to_true_and_can_be_opted_out() {
        let args = DoArgs::new(&[]);
        assert!(args.cursor_restore, "restore is opt-out, not opt-in");
        let opted_out = DoArgs {
            cursor_restore: false,
            ..DoArgs::new(&[])
        };
        assert!(!opted_out.cursor_restore);
        // A dry-run burst never injects, so there is nothing to restore.
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_cursor: true,
            ..DoArgs::new(&[serde_json::json!({"type": "move", "x": 1, "y": 1})])
        });
        let payload = sc(&out);
        assert_eq!(payload["cursorRestored"], Value::Null);
        assert_eq!(payload["cursorMoved"], Value::from(false));
        assert_eq!(payload["cursorUnchanged"], Value::from(true));
    }

    #[test]
    fn a_point_outside_the_target_names_the_actual_window_and_injects_nothing() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        // The fixture is created offscreen, so a screen point derived from a
        // client coordinate cannot land on it. dry_run proves the refusal
        // happens before any injection would.
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_cursor: true,
            target: Some(serde_json::json!({ "hwnd": fx.hwnd_i64() })),
            ..DoArgs::new(&[serde_json::json!({"type": "move", "x": 1000, "y": 1000})])
        });
        let payload = sc(&out);
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(false), "{first}");
        assert_eq!(first["code"], Value::from("point_not_target"), "{first}");
        assert!(
            first["error"]
                .as_str()
                .unwrap_or("")
                .contains("point guard"),
            "the refusal must name the guard: {first}"
        );
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(payload["events"], Value::from(0));
    }

    /// LIVE (ignored): create a visible test window, move the real pointer
    /// inside it with the flag, then assert the move happened and the entry
    /// pointer was restored. Run only on an interactive Windows desktop with
    /// `cargo test -p desktop-engine live_allow_cursor_moves_and_restores_pointer -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop and moves the real pointer"]
    fn live_allow_cursor_moves_and_restores_pointer() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        unsafe {
            SetWindowPos(
                hwnd as HWND,
                HWND_TOPMOST,
                200,
                160,
                400,
                300,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        std::thread::sleep(Duration::from_millis(100));
        let (sx, sy) = crate::target::client_point_to_screen(hwnd, 200, 150)
            .expect("ClientToScreen on the visible test window");
        let before = cursor_now();
        let out = do_burst(&DoArgs {
            allow_cursor: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[serde_json::json!({"type": "move", "x": 200, "y": 150})])
        });
        let payload = sc(&out);
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(true), "real move failed: {first}");
        assert_eq!(first["delivered"], Value::from("cursor"), "{first}");
        assert_eq!(payload["cursorMoved"], Value::from(true), "{payload}");
        assert_eq!(payload["cursorRestored"], Value::from(true), "{payload}");
        assert_eq!(
            cursor_now(),
            before,
            "the entry pointer must be restored (the test moved it to {sx},{sy})"
        );
    }

    /// The one geometry gate (pure): a rect with no area is refused, never
    /// turned into a "centre". On the pre-fix tree `[0,0,0,0]` fed straight
    /// into `screen_point_to_client` and produced a real client point.
    #[test]
    fn element_geometry_gate_refuses_empty_rects() {
        let mut e = Map::new();
        e.insert(
            "rect".to_string(),
            Value::from(vec![
                Value::from(0),
                Value::from(0),
                Value::from(0),
                Value::from(0),
            ]),
        );
        assert_eq!(element_rect_centre(&e), Err(RectRefusal::Offscreen));
        e.insert(
            "rect".to_string(),
            Value::from(vec![
                Value::from(10),
                Value::from(10),
                Value::from(-5),
                Value::from(20),
            ]),
        );
        assert_eq!(element_rect_centre(&e), Err(RectRefusal::Offscreen));
        let bare = Map::new();
        assert_eq!(element_rect_centre(&bare), Err(RectRefusal::Offscreen));
        let msg = rect_refusal_message(&bare, RectRefusal::Offscreen);
        assert!(msg.starts_with("offscreen_element: "), "{msg}");
        assert!(msg.contains("scroll it into view"), "{msg}");
        assert!(msg.contains("invoke"), "{msg}");
        e.insert(
            "rect".to_string(),
            Value::from(vec![
                Value::from(100),
                Value::from(200),
                Value::from(40),
                Value::from(20),
            ]),
        );
        assert_eq!(element_rect_centre(&e), Ok((120, 210)));
    }

    /// The same gate through a real burst: an element whose rect has no area
    /// is refused with `offscreen_element` and nothing is delivered. On the
    /// pre-fix tree this call posted a button at the target's client origin
    /// (`msg_counts().0 == 1`), which is the wrong click the guard closes.
    #[test]
    fn click_on_an_element_without_a_rect_is_refused_and_never_posts() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        for child in [fx.edit(), fx.edit_ro(), fx.button()] {
            let moved = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    child,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                )
            };
            assert_ne!(moved, 0, "collapse a fixture child");
        }
        let (path, fp, rect) = {
            let t0 = std::time::Instant::now();
            loop {
                let m = element_model(fx.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
                let hit = m
                    .entries
                    .iter()
                    .find(|e| e.get("offscreen") == Some(&Value::from(true)));
                if let Some(e) = hit {
                    break (
                        entry_str(e, "path"),
                        entry_str(e, "fp"),
                        e.get("rect").cloned().unwrap_or(Value::Null),
                    );
                }
                if t0.elapsed() > std::time::Duration::from_secs(10) {
                    panic!("no zero-area entry appeared: {:?}", m.entries);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        crate::fixture::reset_msg_counts();
        let actions = vec![
            serde_json::json!({"type": "click", "element": format!("path:{path}"), "expect_fp": fp}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let r = payload["results"][0].clone();
        assert_eq!(r["ok"], Value::from(false), "refused, not clicked: {r}");
        assert_eq!(r["code"], Value::from("offscreen_element"), "{r}");
        let err = r["error"].as_str().unwrap_or("");
        assert!(err.contains("scroll it into view"), "names the fix: {err}");
        assert_ne!(
            r["via"],
            Value::from("posted-click"),
            "no click may be posted: {r}"
        );
        let (down, up, ..) = crate::fixture::msg_counts();
        assert_eq!((down, up), (0, 0), "no button message may be delivered");
        assert_eq!(
            payload["sendinput_calls"],
            Value::from(0),
            "no synthetic input may be sent"
        );
        println!("OFFSCREEN_CLICK_REFUSED_OK code=offscreen_element posts=0 rect={rect}");
    }

    /// Dry-run honesty: a dry run must report the refusal the live path
    /// gives. It used to return `ok:true` for an element the live click
    /// refuses with `offscreen_element`, so a caller who dry-ran a burst got a
    /// green light for a click that could never land.
    #[test]
    fn dry_run_click_on_an_element_without_a_rect_is_refused_like_the_live_path() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        for child in [fx.edit(), fx.edit_ro(), fx.button()] {
            let moved = unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    child,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                )
            };
            assert_ne!(moved, 0, "collapse a fixture child");
        }
        let (path, fp) = {
            let t0 = std::time::Instant::now();
            loop {
                let m = element_model(fx.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
                if let Some(e) = m
                    .entries
                    .iter()
                    .find(|e| e.get("offscreen") == Some(&Value::from(true)))
                {
                    break (entry_str(e, "path"), entry_str(e, "fp"));
                }
                if t0.elapsed() > std::time::Duration::from_secs(10) {
                    panic!("no zero-area entry appeared: {:?}", m.entries);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        let actions = vec![
            serde_json::json!({"type": "click", "element": format!("path:{path}"), "expect_fp": fp}),
        ];
        let out = do_burst(&DoArgs {
            dry_run: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let r = payload["results"][0].clone();
        assert_eq!(
            r["ok"],
            Value::from(false),
            "a dry run must not report a click the live path would refuse: {r}"
        );
        assert_eq!(r["code"], Value::from("offscreen_element"), "{r}");
        assert_ne!(
            r["via"],
            Value::from("posted-click"),
            "no click may be claimed: {r}"
        );
        assert_eq!(
            payload["sendinput_calls"],
            Value::from(0),
            "a dry run sends nothing either way"
        );
        println!("OFFSCREEN_CLICK_DRY_RUN_REFUSED_OK code=offscreen_element");
    }

    /// Focus_window lock: a `hwnd` that names a window is refused with
    /// the POLICY code (`no_foreground_policy`) ÔÇö focusing IS taking the
    /// foreground, and the gate fires before anything else. No window is
    /// touched, recycled handle or not.
    #[test]
    fn focus_window_refuses_with_the_policy_code() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let actions = vec![serde_json::json!({"type": "focus_window", "hwnd": hwnd})];
        let out = do_burst(&DoArgs::new(&actions));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let payload = outer["structuredContent"].clone();
        let first = payload["results"][0].clone();
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(false)),
            "nothing may be focused: {first}"
        );
        assert_eq!(
            first.get("code"),
            Some(&Value::from("no_foreground_policy")),
            "the refusal must carry the policy code: {first}"
        );
        assert_eq!(
            payload["focus_before"], payload["focus_after"],
            "a refused focus must not change the foreground window"
        );
    }

    fn burst_first(actions: &[Value]) -> Map<String, Value> {
        let out = do_burst(&DoArgs::dry(actions));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let payload = outer
            .get("structuredContent")
            .cloned()
            .expect("structuredContent is the payload object");
        let results = payload
            .get("results")
            .and_then(|r| r.as_array())
            .expect("results");
        results
            .first()
            .and_then(|r| r.as_object())
            .expect("first result")
            .clone()
    }

    #[test]
    fn click_count_bound_rejected_before_posting() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[serde_json::json!({"type": "click", "count": 101, "x": 5, "y": 5})])
        });
        let payload = sc(&out);
        let first = &payload["results"][0];
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        let err = first.get("error").and_then(|e| e.as_str()).unwrap_or("");
        assert!(
            err.contains("absurd"),
            "count error should name the bound, got: {err}"
        );
        let (down, up, ..) = crate::fixture::msg_counts();
        assert_eq!((down, up), (0, 0), "no button message may be delivered");
    }

    #[test]
    fn click_count_sane_passes_dry_run() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let out = do_burst(&DoArgs {
            dry_run: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[serde_json::json!({"type": "click", "count": 2, "x": 5, "y": 5})])
        });
        let payload = sc(&out);
        assert_eq!(payload["results"][0].get("ok"), Some(&Value::from(true)));
    }

    fn burst_payload(actions: &[Value]) -> Map<String, Value> {
        let out = do_burst(&DoArgs::dry(actions));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        outer
            .get("structuredContent")
            .and_then(|v| v.as_object())
            .expect("structuredContent is the payload object")
            .clone()
    }

    #[test]
    fn vk_of_reports_shift_from_layout_high_byte() {
        assert_eq!(vk_of("A"), Ok(KeyCode::Vk(0x41, true)));
        assert_eq!(vk_of("!"), Ok(KeyCode::Vk(0x31, true)));
        assert_eq!(vk_of("+"), Ok(KeyCode::Vk(0xBB, true)));
        assert_eq!(vk_of("a"), Ok(KeyCode::Vk(0x41, false)));
        assert_eq!(vk_of("1"), Ok(KeyCode::Vk(0x31, false)));
        assert_eq!(vk_of("="), Ok(KeyCode::Vk(0xBB, false)));
        assert_eq!(vk_of("enter"), Ok(KeyCode::Vk(0x0D, false)));
        assert_eq!(vk_of("F5"), Ok(KeyCode::Vk(0x74, false)));
        assert!(vk_of("not_a_key_xyz").is_err());
    }

    /// Posted-key lParam assembly: Space is the proven pair
    /// (down `0x00390001`, up `0xC0390001`); F5 carries scan `0x3F` with no
    /// extended bit; Right-arrow sets the extended bit; Alt selects the
    /// SYSKEY variants.
    #[test]
    fn posted_key_lparam_assembly() {
        assert_eq!(posted_key_lparam(0x20, false, false) as u32, 0x00390001);
        assert_eq!(posted_key_lparam(0x20, true, false) as u32, 0xC0390001);
        assert!(!is_extended_vk(0x74));
        assert_eq!(posted_key_lparam(0x74, false, false) as u32, 0x003F0001);
        assert_eq!(posted_key_lparam(0x74, true, false) as u32, 0xC03F0001);
        assert!(is_extended_vk(0x27));
        let down = posted_key_lparam(0x27, false, false) as u32;
        assert_ne!(down & (1 << 24), 0, "extended bit must be set: {down:#x}");
        let up = posted_key_lparam(0x27, true, false) as u32;
        assert_ne!(up & (1 << 24), 0, "extended bit survives key-up: {up:#x}");
        assert_ne!(up & (1 << 30), 0);
        assert_ne!(up & (1 << 31), 0);
        let down = posted_key_lparam(0x41, false, false) as u32;
        assert_eq!(down & (1 << 29), 0);
        let down = posted_key_lparam(0x41, false, true) as u32;
        assert_ne!(down & (1 << 29), 0, "Alt sets the context code: {down:#x}");
        assert_eq!(posted_key_downs(&KeyCode::Vk(0x12, false))[0].0, 0x0104);
        assert_eq!(posted_key_ups(&KeyCode::Vk(0x12, false))[0].0, 0x0105);
        assert_eq!(posted_key_downs(&KeyCode::Vk(0x41, false))[0].0, 0x0100);
        assert_eq!(posted_key_ups(&KeyCode::Vk(0x41, false))[0].0, 0x0101);
        println!("POSTED_KEY_LPARAM_OK space+f5+extended+alt-syskey");
    }

    #[test]
    fn key_shift_wraps_in_posted_order() {
        let downs = posted_key_downs(&KeyCode::Vk(0x41, true));
        assert_eq!(
            downs.iter().map(|(m, w, _)| (*m, *w)).collect::<Vec<_>>(),
            vec![(0x0100, VK_SHIFT as u64), (0x0100, 0x41)],
            "shift must wrap the key down"
        );
        let ups = posted_key_ups(&KeyCode::Vk(0x41, true));
        assert_eq!(
            ups.iter().map(|(m, w, _)| (*m, *w)).collect::<Vec<_>>(),
            vec![(0x0101, 0x41), (0x0101, VK_SHIFT as u64)],
            "shift must unwrap the key up"
        );
        let codes = [KeyCode::Vk(0x11, false), KeyCode::Vk(0x41, true)];
        let downs: Vec<(u32, u64)> = codes
            .iter()
            .flat_map(posted_key_downs)
            .map(|(m, w, _)| (m, w))
            .collect();
        assert_eq!(
            downs,
            vec![(0x0100, 0x11), (0x0100, VK_SHIFT as u64), (0x0100, 0x41)],
            "hotkey must press in order, got: {downs:?}"
        );
        let ups: Vec<(u32, u64)> = codes
            .iter()
            .rev()
            .flat_map(posted_key_ups)
            .map(|(m, w, _)| (m, w))
            .collect();
        assert_eq!(
            ups,
            vec![(0x0101, 0x41), (0x0101, VK_SHIFT as u64), (0x0101, 0x11)],
            "hotkey must release in reverse, got: {ups:?}"
        );
    }

    #[test]
    fn expand_arg_parses_state_directive() {
        assert_eq!(expand_arg(None), Ok(None));
        assert_eq!(
            expand_arg(Some(&Value::Null)),
            Ok(None),
            "explicit null state means plain activation"
        );
        assert_eq!(expand_arg(Some(&Value::from("expand"))), Ok(Some(true)));
        assert_eq!(
            expand_arg(Some(&Value::from("Collapse"))),
            Ok(Some(false)),
            "state matching is case-insensitive"
        );
        let e = expand_arg(Some(&Value::from("open"))).unwrap_err();
        assert!(
            e.contains("expand|collapse"),
            "bad state must name the accepted values, got: {e}"
        );
    }

    #[test]
    fn invoke_state_validated_before_dry_run_return() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let fp = live_fp(fx.hwnd(), "name", "OK");
        let actions = vec![
            serde_json::json!({"type": "invoke", "element": "name:OK", "state": "sideways", "expect_fp": fp}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(false)),
            "dry-run bogus state must fail, got: {first:?}"
        );
        assert!(
            first
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("expand|collapse"),
            "dry-run bogus state must name the accepted values, got: {first:?}"
        );
    }

    #[test]
    fn invoke_state_expand_resolves_under_dry_run() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let fp = live_fp(fx.hwnd(), "name", "OK");
        let actions = vec![
            serde_json::json!({"type": "invoke", "element": "name:OK", "state": "expand", "expect_fp": fp}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(true)),
            "dry-run valid state must resolve, got: {first:?}"
        );
        assert_eq!(
            first.get("resolution").and_then(|v| v.as_str()),
            Some("found"),
            "dry-run valid state must report found, got: {first:?}"
        );
    }

    #[test]
    fn scroll_wheel_flag_data_and_bounds() {
        assert_eq!(scroll_wheel("v", 1), Ok((M_WHEEL, 120)));
        assert_eq!(scroll_wheel("v", -3), Ok((M_WHEEL, (-360i32) as u32)));
        assert_eq!(scroll_wheel("h", 2), Ok((M_HWHEEL, 240)));
        assert_eq!(scroll_wheel("h", -1), Ok((M_HWHEEL, (-120i32) as u32)));
        assert!(scroll_wheel("x", 1).is_err());
        assert!(scroll_wheel("v", 1001).is_err());
        assert!(scroll_wheel("h", -1001).is_err());
        assert!(scroll_wheel("v", 1000).is_ok());
        assert!(scroll_wheel("h", -1000).is_ok());
        assert!(scroll_wheel("v", i64::MIN).is_err());
        assert!(scroll_wheel("h", i64::MIN).is_err());
        assert!(scroll_wheel("v", i64::MAX).is_err());
    }

    #[test]
    fn scroll_axis_h_selects_horizontal_wheel_with_target() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[
                serde_json::json!({"type": "scroll", "x": 5, "y": 5, "amount": 2, "axis": "h"}),
            ])
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results[0].get("ok"), Some(&Value::from(true)));
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[
                serde_json::json!({"type": "scroll", "x": 5, "y": 5, "amount": 2, "axis": "bogus"}),
            ])
        });
        let payload = sc(&out);
        assert_eq!(payload["results"][0].get("ok"), Some(&Value::from(false)));
    }

    /// Without the opt-in the pointer is never moved: every move refuses
    /// with the policy code, whatever the coordinates, and sends nothing.
    #[test]
    fn move_without_opt_in_always_refuses_with_no_cursor_policy() {
        for action in [
            serde_json::json!({"type": "move", "dx": 30000, "dy": -30000}),
            serde_json::json!({"type": "move", "x": 10, "y": 10}),
            serde_json::json!({"type": "move", "x": 10, "y": 10, "dx": 1, "dy": 1}),
            serde_json::json!({"type": "move", "dx": 5}),
        ] {
            let first = burst_first(&[action]);
            assert_eq!(first.get("ok"), Some(&Value::from(false)));
            let code = first.get("code").and_then(|c| c.as_str());
            assert!(
                code == Some("no_cursor_policy") || first.get("error").is_some(),
                "move must refuse, got: {first:?}"
            );
        }
        let p = burst_payload(&[serde_json::json!({"type": "move", "x": 10, "y": 10})]);
        assert_eq!(
            p["results"][0]["code"],
            Value::from("no_cursor_policy"),
            "move must name the rule"
        );
        assert_eq!(p.get("sendinput_calls"), Some(&Value::from(0)));
        assert_eq!(p.get("events"), Some(&Value::from(0)));
    }

    #[test]
    fn key_repeat_and_hold_post_without_event_counts() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let out = do_burst(&DoArgs {
            dry_run: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[
                serde_json::json!({"type": "key", "name": "a", "repeat": 3, "hold_ms": 10}),
            ])
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results[0].get("ok"), Some(&Value::from(true)));
        assert_eq!(results[0].get("delivered"), Some(&Value::from("none")));
        assert_eq!(payload.get("events"), Some(&Value::from(0)));
        assert_eq!(payload.get("sendinput_calls"), Some(&Value::from(0)));
    }

    #[test]
    fn key_hold_repeat_bounds_rejected() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        for bad in [
            serde_json::json!({"type": "key", "name": "a", "repeat": 0}),
            serde_json::json!({"type": "key", "name": "a", "repeat": 101}),
            serde_json::json!({"type": "key", "name": "a", "hold_ms": 60001}),
            serde_json::json!({"type": "key", "name": "a", "gap_ms": 60001}),
            serde_json::json!({"type": "key", "name": "a", "hold_ms": -1}),
        ] {
            let out = do_burst(&DoArgs {
                dry_run: true,
                target: Some(serde_json::json!({ "hwnd": hwnd })),
                ..DoArgs::new(&[bad])
            });
            let payload = sc(&out);
            let first = &payload["results"][0];
            assert_eq!(
                first.get("ok"),
                Some(&Value::from(false)),
                "out-of-range hold/repeat must fail, got: {first:?}"
            );
        }
    }

    #[test]
    fn posted_key_dry_run_reports_nothing_delivered() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let out = do_burst(&DoArgs {
            dry_run: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[serde_json::json!({"type": "key", "name": "space"})])
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["ok"], Value::from(true), "dry-run posted key: {r}");
        assert_eq!(r["delivered"], Value::from("none"), "{r}");
        assert_eq!(r["effect"], Value::from("none"), "{r}");
        assert_eq!(r["posted"], Value::from(0), "{r}");
        assert_eq!(r["dry_run"], Value::from(true), "{r}");
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(payload["events"], Value::from(0));
        println!("POSTED_KEY_DRY_RUN_OK delivered=none effect=none");
    }

    #[test]
    fn scroll_element_dry_run_assembles_no_wheel() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let out = do_burst(&DoArgs {
            background: Some(bg),
            dry_run: true,
            ..DoArgs::new(&[
                serde_json::json!({"type": "scroll", "element": "name:OK", "axis": "h"}),
            ])
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let payload = outer
            .get("structuredContent")
            .and_then(|v| v.as_object())
            .expect("payload");
        assert_eq!(payload.get("events"), Some(&Value::from(0)));
        let first = payload
            .get("results")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .expect("first result");
        assert_eq!(first.get("ok"), Some(&Value::from(true)));
    }

    #[test]
    fn scroll_element_without_pattern_is_typed() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let _ = fx.hwnd();
        let model = crate::shot::element_model(fx.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
        let (entry, merr, _) = match_element(&model.entries, Some(&Value::from("name:OK")), None);
        assert!(merr.is_none(), "fixture must list OK, got: {merr:?}");
        let entry = entry.expect("OK entry");
        let path = entry_str(&entry, "path");
        let (ok, via, err) = crate::uia::uia_scroll(fx.hwnd_i64(), &path, 4, 2);
        assert!(!ok, "scroll without the pattern must fail");
        let err = err.expect("typed error");
        assert!(
            err.contains("no ScrollPattern"),
            "missing pattern must be named, got: {err}"
        );
        assert_eq!(via, "uia-scroll", "failure stays on the pattern path");
    }

    #[test]
    fn scroll_element_bg_error_never_falls_back_to_foreground() {
        let actions = vec![serde_json::json!({"type": "scroll", "element": "name:OK"})];
        let out = do_burst(&DoArgs {
            background: Some("title:NoSuchWindowXYZ".to_string()),
            dry_run: true,
            ..DoArgs::new(&actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let first = outer
            .get("structuredContent")
            .and_then(|v| v.get("results"))
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .expect("first result");
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        let err = first.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            err.contains("NoSuchWindowXYZ"),
            "must surface the background failure, not a foreground fallback, got: {err}"
        );
    }

    #[test]
    fn scroll_element_rejects_xy_without_mouse() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions =
            vec![serde_json::json!({"type": "scroll", "element": "name:OK", "x": 10, "y": 10})];
        let out = do_burst(&DoArgs {
            background: Some(bg),
            dry_run: true,
            ..DoArgs::new(&actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let payload = outer
            .get("structuredContent")
            .and_then(|v| v.as_object())
            .expect("payload");
        let first = payload
            .get("results")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .expect("first result");
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        let err = first.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            err.contains("never moves the mouse"),
            "x/y with element must be refused as mouse input, got: {err}"
        );
        assert_eq!(
            payload.get("events"),
            Some(&Value::from(0)),
            "refused element scroll must queue nothing, got: {payload:?}"
        );
    }

    #[test]
    fn bg_click_count_bound_rejected_before_post() {
        let mut a = Map::new();
        a.insert("type".to_string(), Value::from("click"));
        a.insert("x".to_string(), Value::from(1));
        a.insert("y".to_string(), Value::from(1));
        a.insert("count".to_string(), Value::from(1_000_000_000_000i64));
        let (ok, err, _) = bg_execute(0, &a, false, true);
        assert!(!ok);
        assert!(
            err.unwrap_or_default().contains("absurd"),
            "bg count error should name the bound"
        );
    }

    #[test]
    fn stable_huge_region_refused_before_grab() {
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("stable"));
        a.insert(
            "region".to_string(),
            Value::from(vec![
                Value::from(0),
                Value::from(0),
                Value::from(100000),
                Value::from(100000),
            ]),
        );
        a.insert("timeout_ms".to_string(), Value::from(1000));
        let t0 = Instant::now();
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
        let err = r.get("error").and_then(|e| e.as_str()).unwrap_or("");
        assert!(
            err.contains("region must"),
            "huge region should hit the region guard, got: {err}"
        );
    }

    #[test]
    fn stable_region_i64_extremes_typed_error() {
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("stable"));
        a.insert(
            "region".to_string(),
            Value::from(vec![
                Value::from(i64::MIN),
                Value::from(0),
                Value::from(5000),
                Value::from(1),
            ]),
        );
        a.insert("timeout_ms".to_string(), Value::from(1000));
        let t0 = Instant::now();
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
        let err = r.get("error").and_then(|e| e.as_str()).unwrap_or("");
        assert!(
            err.contains("region"),
            "MIN-edge stable region must be a typed region error, got: {err}"
        );
    }

    #[test]
    fn stable_region_bound_is_4096() {
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("stable"));
        a.insert(
            "region".to_string(),
            Value::from(vec![
                Value::from(0),
                Value::from(0),
                Value::from(5000),
                Value::from(5000),
            ]),
        );
        a.insert("timeout_ms".to_string(), Value::from(1000));
        let t0 = Instant::now();
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
        let err = r.get("error").and_then(|e| e.as_str()).unwrap_or("");
        assert!(
            err.contains("4096"),
            "5000px region must name the 4096 bound, got: {err}"
        );
    }

    #[test]
    fn monitor_for_point_picks_containing_monitor() {
        let mons = vec![
            [0, 0, 1920, 1080],
            [1920, 0, 3840, 1080],
            [-1920, 0, 0, 1080],
        ];
        assert_eq!(monitor_for_point(&mons, 100, 100), Some([0, 0, 1920, 1080]));
        assert_eq!(
            monitor_for_point(&mons, 2000, 500),
            Some([1920, 0, 3840, 1080])
        );
        assert_eq!(
            monitor_for_point(&mons, -100, 500),
            Some([-1920, 0, 0, 1080]),
            "negative-origin monitors must match"
        );
        assert_eq!(monitor_for_point(&mons, 9000, 9000), None);
        assert_eq!(monitor_for_point(&[], 0, 0), None);
    }

    #[test]
    fn check_clip_len_bounds() {
        assert!(check_clip_len(0).is_ok());
        assert!(check_clip_len(MAX_CLIP_CHARS).is_ok());
        let e = check_clip_len(MAX_CLIP_CHARS + 1).unwrap_err();
        assert!(
            e.contains("1048576"),
            "over-cap text must name the bound, got: {e}"
        );
    }

    #[test]
    fn clip_read_u16_requires_nul_within_bounds() {
        let buf: Vec<u16> = "hi".encode_utf16().chain(std::iter::once(0)).collect();
        let s = unsafe { clip_read_u16(buf.as_ptr(), 10, 0).expect("nul-terminated") };
        assert_eq!(s, "hi");
        let no_nul: Vec<u16> = "hi".encode_utf16().collect();
        assert!(
            unsafe { clip_read_u16(no_nul.as_ptr(), 2, 0) }.is_err(),
            "missing NUL must fail instead of over-reading"
        );
        let mut big = vec!['x' as u16; 100];
        big.push(0);
        assert!(
            unsafe { clip_read_u16(big.as_ptr(), 10, 0) }.is_err(),
            "scan must stop at the GlobalSize bound even with a later NUL"
        );
        let empty: Vec<u16> = vec![0];
        assert_eq!(unsafe { clip_read_u16(empty.as_ptr(), 1, 0).unwrap() }, "");
    }

    #[test]
    fn clipboard_over_cap_set_rejected_before_touch() {
        let big = "x".repeat(1_048_576 + 1);
        let actions = vec![serde_json::json!({"type": "clipboard_set", "text": big})];
        let out = do_burst(&DoArgs {
            allow_clipboard: true,
            ..DoArgs::new(&actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let first = outer
            .get("structuredContent")
            .and_then(|v| v.get("results"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .expect("first result");
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        assert_eq!(
            first.get("code").and_then(|c| c.as_str()),
            Some("too_large"),
            "over-cap set must be typed before OpenClipboard, got: {first:?}"
        );
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        assert_eq!(
            outer["structuredContent"]["clipboard"]["writes"],
            Value::from(0)
        );
    }

    /// A burst over the 256-action cap is refused whole with a typed
    /// per-result failure naming the limit; nothing runs (no wait sleeps).
    #[test]
    fn burst_over_the_action_cap_is_a_typed_refusal() {
        let actions: Vec<Value> = (0..=BURST_MAX_ACTIONS)
            .map(|_| serde_json::json!({"type": "wait", "ms": 0}))
            .collect();
        assert_eq!(actions.len(), BURST_MAX_ACTIONS + 1);
        let out = do_burst(&DoArgs::new(&actions));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let sc = outer.get("structuredContent").expect("payload");
        assert_eq!(sc.get("ok"), Some(&Value::from(false)));
        let results = sc["results"].as_array().expect("results");
        assert_eq!(results.len(), 1, "a refused burst runs nothing");
        assert_eq!(
            results[0].get("code"),
            Some(&Value::from("burst_budget_exceeded")),
            "the cap needs the typed code, got: {:?}",
            results[0]
        );
        assert!(
            results[0]
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("256"),
            "the refusal must name the limit, got: {:?}",
            results[0]
        );
    }

    /// Exactly the cap still runs: the refusal is off-by-one-free.
    #[test]
    fn burst_at_the_action_cap_still_runs() {
        let actions: Vec<Value> = (0..BURST_MAX_ACTIONS)
            .map(|_| serde_json::json!({"type": "wait", "ms": 0}))
            .collect();
        let out = do_burst(&DoArgs {
            settle_ms: 0,
            ..DoArgs::new(&actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let sc = outer.get("structuredContent").expect("payload");
        assert_eq!(
            sc["results"].as_array().map(|a| a.len()),
            Some(BURST_MAX_ACTIONS),
            "a burst at the cap must run every action"
        );
        assert_eq!(sc.get("failed"), Some(&Value::from(0)));
    }

    #[test]
    fn clipboard_set_without_flag_is_a_typed_refusal_and_burst_continues() {
        // The follow-up read needs CF_UNICODETEXT present; seed it and
        // serialise instead of depending on another test's clipboard state.
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("A");
        let actions = vec![
            serde_json::json!({"type": "clipboard_set", "text": "SHOULD-NOT-LAND"}),
            serde_json::json!({"type": "wait", "ms": 1}),
        ];
        let out = do_burst(&DoArgs::new(&actions));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let results = outer
            .get("structuredContent")
            .and_then(|v| v.get("results"))
            .and_then(|v| v.as_array())
            .expect("results");
        let first = results.first().expect("first result");
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        assert_eq!(
            first.get("code").and_then(|c| c.as_str()),
            Some("no_clipboard_policy"),
            "clipboard_set is always refused, got: {first:?}"
        );
        assert!(
            first
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("background-only policy"),
            "refusal must name the rule, got: {first:?}"
        );
        assert_eq!(
            results.get(1).and_then(|r| r.get("ok")),
            Some(&Value::from(true)),
            "a refused clipboard_set must not abort the burst: {results:?}"
        );
        let reads = vec![serde_json::json!({"type": "clipboard_get"})];
        let out = do_burst(&DoArgs::new(&reads));
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let first = outer
            .get("structuredContent")
            .and_then(|v| v.get("results"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .expect("first result");
        assert!(
            first.get("code").is_none(),
            "clipboard_get needs no flag, got: {first:?}"
        );
    }

    /// Clipboard is a single global resource. Serialise the tests that
    /// touch it so one test's write cannot move another's sequence number.
    static CLIPBOARD_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clip_test_lock() -> std::sync::MutexGuard<'static, ()> {
        CLIPBOARD_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Retry the clipboard briefly: another application can hold the handle
    /// for a moment. Tests must not fail on that transient contention.
    fn clip_snapshot_retry() -> Option<ClipboardPrevious> {
        for _ in 0..10 {
            if let Ok(p) = clipboard_snapshot() {
                return Some(p);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn clip_write_retry(s: &str) {
        for _ in 0..10 {
            if clipboard_set_text(s).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("could not open the clipboard for test setup");
    }

    fn clip_get_retry() -> String {
        for _ in 0..10 {
            if let Ok((s, _, _, _)) = clipboard_get() {
                return s;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("clipboard stayed unreadable for the test");
    }

    /// A transient OpenClipboard failure is a typed result, not a test
    /// failure: retry the same burst until the OS lets us through.
    fn do_burst_clip_retry(args: &DoArgs) -> Value {
        for _ in 0..10 {
            let payload = sc(&do_burst(args));
            let unavailable = payload["results"].as_array().is_some_and(|rs| {
                rs.iter()
                    .any(|r| r["code"] == Value::from("clipboard_unavailable"))
            });
            if !unavailable {
                return payload;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("clipboard stayed unavailable for the test burst");
    }

    fn clip_restore_best_effort(previous: &Option<ClipboardPrevious>) {
        let r = match previous.clone() {
            Some(ClipboardPrevious::Text(ref s)) => clipboard_set_text(s).map(|_| ()),
            Some(ClipboardPrevious::Empty) | Some(ClipboardPrevious::NonText) => {
                clipboard_clear().map(|_| ())
            }
            None => Ok(()),
        };
        if let Err(e) = r {
            eprintln!("clipboard test cleanup failed: {e}");
        }
    }

    /// Saves whatever the owner had before the test touches the clipboard
    /// and puts it back in Drop, so even a panic leaves no residue.
    struct ClipboardOwnerGuard(Option<ClipboardPrevious>);

    impl Drop for ClipboardOwnerGuard {
        fn drop(&mut self) {
            clip_restore_best_effort(&self.0);
        }
    }

    #[test]
    fn clipboard_error_names_stage_code_and_attempts() {
        let e = ClipboardError::new("OpenClipboard", 5, 3);
        let text = e.text();
        assert!(text.starts_with("clipboard_unavailable:"), "{text}");
        assert!(text.contains("stage=OpenClipboard"), "{text}");
        assert!(text.contains("win32_error=5"), "{text}");
        assert!(text.contains("attempts=3"), "{text}");
    }

    /// An empty clipboard is a normal state: the read must say that no
    /// text format is present, not that the clipboard could not be opened.
    #[test]
    fn clipboard_get_with_no_text_format_is_typed_and_not_unavailable() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("owner text");
        let mut cleared = false;
        for _ in 0..10 {
            if clipboard_clear().is_ok() {
                cleared = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(cleared, "could not clear the clipboard for test setup");
        assert_eq!(
            unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) },
            0,
            "the clipboard must hold no text format for this test"
        );
        let payload = do_burst_clip_retry(&DoArgs::new(&[serde_json::json!({
            "type": "clipboard_get"
        })]));
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(false), "{first}");
        assert_eq!(first["code"], Value::from("no_text_format"), "{first}");
        let err = first["error"].as_str().unwrap_or("");
        assert_eq!(
            err,
            "no_text_format: the clipboard holds no CF_UNICODETEXT (it may be empty or hold another format)",
            "{err}"
        );
        assert!(!err.contains("attempts="), "no retry was made: {err}");
        assert!(!err.contains("clipboard_unavailable"), "{err}");
    }

    /// Hold the clipboard open from another thread past the bounded retry
    /// cap. Drop releases and joins before the owner guard restores, so a
    /// panic cannot strand the owner's clipboard behind our handle.
    struct ClipboardContentionHold {
        release: Option<std::sync::mpsc::Sender<()>>,
        holder: Option<std::thread::JoinHandle<()>>,
    }

    impl ClipboardContentionHold {
        fn new() -> Self {
            let (opened_tx, opened_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                unsafe {
                    let mut opened = false;
                    for _ in 0..CLIP_OPEN_MAX_ATTEMPTS {
                        if OpenClipboard(GetDesktopWindow()) != 0 {
                            opened = true;
                            break;
                        }
                        std::thread::sleep(CLIP_OPEN_BACKOFF);
                    }
                    if !opened {
                        return; // opened_tx drops: the receiver sees failure
                    }
                    let _ = opened_tx.send(());
                    // Bounded even if the release signal is lost.
                    let _ = release_rx.recv_timeout(Duration::from_secs(2));
                    CloseClipboard();
                }
            });
            opened_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("contention thread never opened the clipboard");
            ClipboardContentionHold {
                release: Some(release_tx),
                holder: Some(holder),
            }
        }
    }

    impl Drop for ClipboardContentionHold {
        fn drop(&mut self) {
            if let Some(tx) = self.release.take() {
                let _ = tx.send(());
            }
            if let Some(h) = self.holder.take() {
                let _ = h.join();
            }
        }
    }

    /// A truly locked clipboard still fails after the bounded retry and
    /// keeps the clipboard_unavailable code, stage and attempt count.
    #[test]
    fn clipboard_get_under_open_contention_stays_clipboard_unavailable() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("R");
        let hold = ClipboardContentionHold::new();
        let payload = sc(&do_burst(&DoArgs::new(&[serde_json::json!({
            "type": "clipboard_get"
        })])));
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(false), "{first}");
        assert_eq!(
            first["code"],
            Value::from("clipboard_unavailable"),
            "{first}"
        );
        let err = first["error"].as_str().unwrap_or("");
        assert!(err.starts_with("clipboard_unavailable: "), "{err}");
        assert!(err.contains("stage=OpenClipboard"), "{err}");
        let attempts = err
            .split("attempts=")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        assert!(attempts > 1, "the bounded retry must be reported: {err}");
        drop(hold);
    }

    /// A second thread holds the global clipboard open for ~10 ms while a
    /// read runs. The bounded retry must ride out that transient contention
    /// and report more than one attempt.
    #[test]
    fn clipboard_get_retries_transient_open_contention() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("R");
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            unsafe {
                // A window-owned open really excludes this process: two
                // OpenClipboard(NULL) calls in one task share ownership.
                // Retry here too: another process can win the lock first.
                let mut opened = false;
                for _ in 0..CLIP_OPEN_MAX_ATTEMPTS {
                    if OpenClipboard(GetDesktopWindow()) != 0 {
                        opened = true;
                        break;
                    }
                    std::thread::sleep(CLIP_OPEN_BACKOFF);
                }
                if !opened {
                    return; // opened_tx drops: the receiver sees a failed holder
                }
                let _ = opened_tx.send(());
                std::thread::sleep(Duration::from_millis(12));
                CloseClipboard();
            }
        });
        opened_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("contention thread never opened the clipboard");
        let (text, _, _, attempts) =
            clipboard_get().expect("read must survive transient OpenClipboard contention");
        holder.join().expect("contention thread panicked");
        assert_eq!(text, "R");
        assert!(attempts > 1, "the read must retry; attempts={attempts}");
    }

    /// LIVE stress: 200 real set/get cycles must all succeed against the
    /// global clipboard. Ignored by default so normal runs stay quick.
    #[test]
    #[ignore = "live clipboard stress: 200 set/get cycles on the global clipboard"]
    fn clipboard_set_get_200_cycles_without_failures() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        for i in 0..200 {
            let text = format!("cycle-{i}");
            let (bytes, attempts) =
                clipboard_set_text(&text).expect("stress set failed on cycle {i}");
            assert_eq!(bytes, (text.len() + 1) * 2, "cycle {i}");
            assert!(attempts >= 1, "cycle {i}");
            let (read, _, truncated, read_attempts) =
                clipboard_get().expect("stress get failed on cycle {i}");
            assert_eq!(read, text, "cycle {i}");
            assert!(!truncated, "cycle {i}");
            assert!(read_attempts >= 1, "cycle {i}");
        }
    }

    /// LIVE clipboard round trip: A is on the clipboard before the burst, B
    /// is written by it, and A is back afterwards. The burst itself sees B
    /// on its own clipboard_get; only the post-burst read sees A again.
    #[test]
    fn restore_clipboard_brings_back_previous_text() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("A");
        let actions = vec![
            serde_json::json!({"type": "clipboard_set", "text": "B"}),
            serde_json::json!({"type": "clipboard_get"}),
        ];
        let payload = do_burst_clip_retry(&DoArgs {
            allow_clipboard: true,
            restore_clipboard: true,
            ..DoArgs::new(&actions)
        });
        assert_eq!(payload["results"][0]["ok"], Value::from(true), "{payload}");
        assert_eq!(
            payload["results"][1]["text"],
            Value::from("B"),
            "the burst must see its own write before the restore: {payload}"
        );
        let clip = &payload["clipboard"];
        assert_eq!(clip["writes"], Value::from(1), "{clip}");
        assert_eq!(clip["restores"], Value::from(1), "{clip}");
        assert_eq!(clip["previous"], Value::from("text"), "{clip}");
        assert!(clip["bytes"].as_i64().unwrap_or(0) > 0, "{clip}");
        assert_eq!(
            clip_get_retry(),
            "A",
            "restore must put the owner's text back"
        );
    }

    /// A partial burst still restores: a refused action after the write
    /// must not strand the owner's clipboard with our text.
    #[test]
    fn restore_runs_after_a_partial_failed_burst() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("A");
        let actions = vec![
            serde_json::json!({"type": "clipboard_set", "text": "B"}),
            serde_json::json!({"type": "move", "dx": 5, "dy": 5}),
        ];
        let payload = do_burst_clip_retry(&DoArgs {
            allow_clipboard: true,
            restore_clipboard: true,
            ..DoArgs::new(&actions)
        });
        assert_eq!(payload["results"][0]["ok"], Value::from(true), "{payload}");
        assert_eq!(
            payload["results"][1]["code"],
            Value::from("no_cursor_policy"),
            "the later action must still be refused: {payload}"
        );
        assert_eq!(payload["clipboard"]["writes"], Value::from(1), "{payload}");
        assert_eq!(
            payload["clipboard"]["restores"],
            Value::from(1),
            "{payload}"
        );
        assert_eq!(clip_get_retry(), "A", "partial burst must still restore");
    }

    /// The flag alone writes text and reports the ledger; no restore is
    /// attempted, so the burst's write stays on the clipboard.
    #[test]
    fn allowed_clipboard_set_writes_and_reports_ledger() {
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("A");
        let actions = vec![serde_json::json!({"type": "clipboard_set", "text": "B"})];
        let payload = do_burst_clip_retry(&DoArgs {
            allow_clipboard: true,
            ..DoArgs::new(&actions)
        });
        assert_eq!(payload["results"][0]["ok"], Value::from(true), "{payload}");
        assert_eq!(
            payload["results"][0]["bytes"].as_i64().unwrap_or(0),
            4,
            "B plus its UTF-16 NUL is four bytes: {payload}"
        );
        assert_eq!(payload["clipboard"]["writes"], Value::from(1), "{payload}");
        assert_eq!(
            payload["clipboard"]["restores"],
            Value::from(0),
            "{payload}"
        );
        assert_eq!(
            payload["clipboard"]["previous"],
            Value::from("text"),
            "{payload}"
        );
        assert_eq!(clip_get_retry(), "B");
    }

    /// Unflagged writes are refused before OpenClipboard: the clipboard
    /// sequence number does not move and the no-steal verdicts stay true.
    #[test]
    fn unflagged_clipboard_set_never_touches_the_clipboard() {
        let _lock = clip_test_lock();
        use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
        let actions = vec![
            serde_json::json!({"type": "clipboard_set", "text": "SHOULD-NOT-LAND"}),
            serde_json::json!({"type": "wait", "ms": 1}),
        ];
        let before = unsafe { GetClipboardSequenceNumber() };
        let out = do_burst(&DoArgs::new(&actions));
        let after = unsafe { GetClipboardSequenceNumber() };
        let payload = sc(&out);
        assert_eq!(
            before, after,
            "unflagged write must not touch the clipboard"
        );
        assert_eq!(
            payload["results"][0]["code"],
            Value::from("no_clipboard_policy"),
            "{payload}"
        );
        assert_eq!(payload["clipboard"]["writes"], Value::from(0), "{payload}");
        assert!(payload["foregroundUnchanged"].is_boolean(), "{payload}");
        assert!(payload["cursorUnchanged"].is_boolean(), "{payload}");
    }

    #[test]
    fn element_wait_predicate_truth_table() {
        assert!(element_wait_satisfied("appear", true, None));
        assert!(!element_wait_satisfied("appear", false, None));
        assert!(element_wait_satisfied("disappear", false, None));
        assert!(!element_wait_satisfied("disappear", true, None));
        assert!(element_wait_satisfied("enabled", true, Some(true)));
        assert!(!element_wait_satisfied("enabled", true, Some(false)));
        assert!(!element_wait_satisfied("enabled", true, None));
        assert!(!element_wait_satisfied("enabled", false, None));
        assert!(element_wait_satisfied("disabled", true, Some(false)));
        assert!(!element_wait_satisfied("disabled", true, Some(true)));
        assert!(!element_wait_satisfied("disabled", true, None));
        assert!(!element_wait_satisfied("disabled", false, Some(false)));
        assert!(!element_wait_satisfied("bogus", true, Some(true)));
        assert!(!element_wait_satisfied("bogus", false, None));
    }

    #[test]
    fn element_wait_rejects_malformed_arguments() {
        let t0 = Instant::now();
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("element"));
        a.insert("timeout_ms".to_string(), Value::from(0));
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
        assert!(
            r.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .contains("needs \"element\""),
            "missing element must be a typed per-action error, got: {r:?}"
        );
        let mut b = Map::new();
        b.insert("kind".to_string(), Value::from("element"));
        b.insert("element".to_string(), Value::from("ref:12.3"));
        b.insert("timeout_ms".to_string(), Value::from(0));
        let r2 = wait_for(&b, &t0);
        assert_eq!(r2.get("ok"), Some(&Value::from(false)));
        assert!(
            r2.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .contains("unknown element selector kind"),
            "ref: must be rejected, got: {r2:?}"
        );
        let mut c = Map::new();
        c.insert("kind".to_string(), Value::from("element"));
        c.insert("element".to_string(), Value::from("name:OK"));
        c.insert("state".to_string(), Value::from("hovered"));
        c.insert("timeout_ms".to_string(), Value::from(0));
        let r3 = wait_for(&c, &t0);
        assert_eq!(r3.get("ok"), Some(&Value::from(false)));
        let mut d = Map::new();
        d.insert("kind".to_string(), Value::from("nope_not_a_kind"));
        let r4 = wait_for(&d, &t0);
        assert_eq!(r4.get("ok"), Some(&Value::from(false)));
        assert!(
            r4.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .contains("stable|element"),
            "unknown kind must name the accepted kinds, got: {r4:?}"
        );
    }

    #[test]
    fn element_wait_rejects_malformed_window_fast() {
        let t0 = Instant::now();
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("element"));
        a.insert("element".to_string(), Value::from("path:0"));
        a.insert("window".to_string(), Value::from("hwnd:nope"));
        a.insert("timeout_ms".to_string(), Value::from(0));
        let t_start = Instant::now();
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
        assert!(
            r.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .contains("bad window selector"),
            "unparseable hwnd must fail fast, got: {r:?}"
        );
        assert!(
            t_start.elapsed() < Duration::from_secs(2),
            "malformed window must not poll, took {:?}",
            t_start.elapsed()
        );
    }

    #[test]
    fn stable_empty_region_still_rejected() {
        let mut a = Map::new();
        a.insert("kind".to_string(), Value::from("stable"));
        a.insert(
            "region".to_string(),
            Value::from(vec![
                Value::from(10),
                Value::from(10),
                Value::from(5),
                Value::from(5),
            ]),
        );
        a.insert("timeout_ms".to_string(), Value::from(0));
        let t0 = Instant::now();
        let r = wait_for(&a, &t0);
        assert_eq!(r.get("ok"), Some(&Value::from(false)));
    }

    #[test]
    fn overflow_string_rejected_with_shared_error() {
        let v = serde_json::json!("9223372036854775808"); // i64::MAX + 1
        assert!(crate::is_int_overflow(&v));
        let err = py_int(&v).expect_err("MAX+1 must be rejected, never clamped");
        assert_eq!(err, crate::int_overflow_err(&v));
    }

    #[test]
    fn overflow_u64_rejected_with_shared_error() {
        let v = Value::from(u64::MAX);
        assert!(crate::is_int_overflow(&v));
        let err = py_int(&v).expect_err("u64::MAX must be rejected, never clamped");
        assert_eq!(err, crate::int_overflow_err(&v));
    }

    #[test]
    fn overflow_float_rejected_with_shared_error() {
        let v = serde_json::json!(1e19);
        assert!(crate::is_int_overflow(&v));
        let err = py_int(&v).expect_err("1e19 must be rejected, never clamped");
        assert_eq!(err, crate::int_overflow_err(&v));
    }

    #[test]
    fn in_range_values_still_accepted() {
        assert_eq!(py_int(&serde_json::json!(42)).unwrap(), 42);
        assert_eq!(
            py_int(&serde_json::json!("-9223372036854775808")).unwrap(),
            i64::MIN
        );
    }

    fn ref_entry(win: i64, path: &str, name: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("win".to_string(), Value::from(win));
        m.insert("role".to_string(), Value::from("Button"));
        m.insert("name".to_string(), Value::from(name));
        m.insert("auto_id".to_string(), Value::from(""));
        m.insert(
            "rect".to_string(),
            Value::from(vec![
                Value::from(0),
                Value::from(0),
                Value::from(10),
                Value::from(10),
            ]),
        );
        m.insert("invokable".to_string(), Value::from(true));
        m.insert("path".to_string(), Value::from(path));
        m.insert("ref".to_string(), Value::from(format!("{win}.{path}")));
        m
    }

    #[test]
    fn ref_selector_rejected_names_accepted_forms() {
        let _g = crate::fixture::desktop();
        let entries = vec![
            ref_entry(199210, "0", "Cancel"),
            ref_entry(199210, "0.3.1", "OK"),
        ];
        for bad in ["ref:199210.0.3.1", "ref:199210.0.9", "ref:abc", "ref:"] {
            let (hit, err, _) = match_element(&entries, Some(&Value::from(bad)), None);
            assert!(hit.is_none(), "{bad} must not resolve");
            let e = err.expect("ref selector must be rejected, never silent");
            assert!(
                e.contains("unknown element selector kind"),
                "ref selector needs the unknown-kind shape, got: {e}"
            );
            assert!(
                e.contains("(want name:|auto_id:|path:)"),
                "rejection must name the accepted forms, got: {e}"
            );
            assert!(
                !e.contains("ref:"),
                "rejection must not advertise a ref: selector, got: {e}"
            );
        }
    }

    fn burst_bg(actions: &[Value], bg: &str, dry_run: bool) -> (String, Map<String, Value>) {
        let out = do_burst(&DoArgs {
            dry_run,
            background: Some(bg.to_string()),
            ..DoArgs::new(actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer frame");
        let payload = outer
            .get("structuredContent")
            .cloned()
            .expect("structuredContent is the payload object");
        let first = payload
            .get("results")
            .and_then(|r| r.as_array())
            .expect("results")[0]
            .as_object()
            .expect("first")
            .clone();
        (out, first)
    }

    fn fixture_bg() -> (crate::fixture::Fixture, String) {
        let f = crate::fixture::Fixture::new();
        let bg = format!("hwnd:{}", f.hwnd_i64());
        (f, bg)
    }

    /// Background-only policy: every set_value/type-on-element is refused
    /// with background_unavailable naming the element, before any provider
    /// call ÔÇö dry_run included. The EDIT must be untouched.
    fn assert_set_value_refused(first: &Map<String, Value>, element: &str) {
        assert_eq!(first.get("ok"), Some(&Value::from(false)));
        assert_eq!(
            first.get("code").and_then(|c| c.as_str()),
            Some("background_unavailable"),
            "set_value must name the policy, got: {first:?}"
        );
        let e = first.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            e.contains(element),
            "refusal must name the element {element:?}, got: {e}"
        );
        assert!(
            e.contains("background-only policy"),
            "refusal must name the rule, got: {e}"
        );
    }

    #[test]
    fn set_value_missing_element_rejected() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions = vec![serde_json::json!({"type": "set_value", "text": "hi"})];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, "");
        assert_eq!(fx.edit_text(), "", "no provider call may have written");
    }

    #[test]
    fn set_value_missing_text_rejected() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions = vec![serde_json::json!({"type": "set_value", "element": "name:OK"})];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, "name:OK");
    }

    #[test]
    fn set_value_ref_rejected_like_invoke() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions =
            vec![serde_json::json!({"type": "set_value", "element": "ref:123.0", "text": "hi"})];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, "ref:123.0");
    }

    #[test]
    fn set_value_not_found_typed() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": "name:NoSuchControlXYZ123", "text": "hi"}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, "name:NoSuchControlXYZ123");
    }

    #[test]
    fn set_value_button_has_no_valuepattern() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions =
            vec![serde_json::json!({"type": "set_value", "element": "name:OK", "text": "hi"})];
        let (_, first) = burst_bg(&actions, &bg, false);
        assert_set_value_refused(&first, "name:OK");
    }

    /// The live fingerprint of one control from a fresh walk. W2.1 makes it
    /// mandatory for every element mutation, so a test that means to prove
    /// something else passes the current value.
    fn live_fp(hwnd: HWND, key: &str, want: &str) -> String {
        let m = element_model(hwnd, crate::UIA_DEPTH_LIMIT, 0);
        m.entries
            .iter()
            .find(|e| entry_str(e, key) == want)
            .map(|e| entry_str(e, "fp"))
            .unwrap_or_else(|| panic!("the walk must list {key}:{want}"))
    }

    fn edit_paths() -> (crate::fixture::Fixture, String, String, String) {
        let f = crate::fixture::Fixture::new();
        let bg = format!("hwnd:{}", f.hwnd_i64());
        let model = element_model(f.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
        let mut edits: Vec<(i64, String)> = Vec::new();
        for e in &model.entries {
            if e.get("role").and_then(|v| v.as_str()).unwrap_or("") == "Edit" {
                let top = e
                    .get("rect")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.get(1))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let path = e
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                edits.push((top, path));
            }
        }
        edits.sort();
        assert!(
            edits.len() >= 2,
            "fixture must expose 2 Edit controls via UIA, got {}",
            edits.len()
        );
        let w = edits[0].1.clone();
        let r = edits[1].1.clone();
        (f, bg, w, r)
    }

    #[test]
    fn set_value_readonly_typed() {
        let _g = crate::fixture::desktop();
        let (fx, bg, _w, ro) = edit_paths();
        let _ = fx.hwnd();
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": format!("path:{ro}"), "text": "hi"}),
        ];
        let (_, first) = burst_bg(&actions, &bg, false);
        assert_set_value_refused(&first, &format!("path:{ro}"));
        assert_eq!(fx.edit_text(), "", "the EDIT must be untouched");
    }

    /// ROOT-CAUSE REGRESSION: the fixture EDIT refuses real typing.
    ///
    /// `ValuePattern.SetValue` is performed by the UIA provider, which focuses
    /// the control first; the off-screen fixture EDIT then holds keyboard
    /// focus and the owner's keystrokes land in the document at the caret
    /// (observed: "www" prepended to the written value, which made
    /// `set_value_success_writes_and_never_echoes` fail on a real machine).
    /// The fixture guard must make that impossible: inject the same messages
    /// real typing sends and assert the document is untouched, while the
    /// provider write still lands its exact value.
    #[test]
    fn fixture_edit_refuses_external_typing() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::Fixture::new();
        crate::fixture::reset_swallowed_keys(f.edit());
        unsafe {
            let t: Vec<u16> = "keep".encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(f.edit(), t.as_ptr());
            for ch in ['w' as u16, 'w' as u16, 'w' as u16] {
                PostMessageW(f.edit(), 0x0102, ch as usize, 1);
            }
            PostMessageW(f.edit(), 0x0302, 0, 0); // WM_PASTE
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        assert_eq!(
            f.edit_text(),
            "keep",
            "external keyboard input must not reach the fixture document"
        );
        assert!(
            crate::fixture::swallowed_key_count(f.edit()) >= 4,
            "the guard must have seen the injected messages, got {}",
            crate::fixture::swallowed_key_count(f.edit())
        );
        println!(
            "FIXTURE_TYPING_GUARD_OK swallowed={} text={:?}",
            crate::fixture::swallowed_key_count(f.edit()),
            f.edit_text()
        );
    }

    /// `ValuePattern.SetValue` REPLACES and uses no keystroke messages, so
    /// the fixture's typing guard cannot change what the product does.
    #[test]
    fn value_pattern_set_value_replaces_without_keystrokes() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::Fixture::new();
        let model = crate::shot::element_model(f.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
        let entry = model
            .entries
            .iter()
            .find(|e| e.get("auto_id").and_then(|v| v.as_str()) == Some("100"))
            .expect("writable EDIT");
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        unsafe {
            let t: Vec<u16> = "abc".encode_utf16().chain(std::iter::once(0)).collect();
            SetWindowTextW(f.edit(), t.as_ptr());
        }
        println!("SETVALUE_PRE text={:?}", f.edit_text());
        let (ok, via, err) = crate::uia::uia_set_value(f.hwnd_i64(), &path, "xyz");
        println!(
            "SETVALUE_POST ok={ok} via={via} err={err:?} text={:?}",
            f.edit_text()
        );
        assert_eq!(
            f.edit_text(),
            "xyz",
            "SetValue must REPLACE the control text, not append"
        );
        println!(
            "SETVALUE_SWALLOWED_KEYS={} (0 = no external typing during the write)",
            crate::fixture::swallowed_key_count(f.edit())
        );
        unsafe {
            let thread = GetWindowThreadProcessId(f.hwnd(), std::ptr::null_mut());
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            if GetGUIThreadInfo(thread, &mut info) != 0 {
                println!(
                    "SETVALUE_FOCUS edit_focused={} (provider focus is why the fixture guard exists)",
                    info.hwndFocus == f.edit()
                );
            }
        }
        println!("SETVALUE_SEMANTICS_OK replace=xyz keystrokes=0");
    }

    /// Gap-1 proof: the value is never written and never echoed. The refusal
    /// names the control, not the secret; the EDIT stays empty.
    #[test]
    fn set_value_success_writes_and_never_echoes() {
        let _g = crate::fixture::desktop();
        let fg_before = crate::fixture::foreground_now();
        let (fx, bg, wpath, _ro) = edit_paths();
        assert_eq!(
            fx.edit_text(),
            "",
            "the fixture EDIT must start empty (external typing is refused)"
        );
        let secret = "fixture-secret-4821";
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": format!("path:{wpath}"), "text": secret}),
        ];
        let (out, first) = burst_bg(&actions, &bg, false);
        assert_set_value_refused(&first, &format!("path:{wpath}"));
        assert!(
            !out.contains(secret),
            "refusal must never echo the text value"
        );
        assert_eq!(fx.edit_text(), "");
        println!("SET_VALUE_READBACK= (refused, untouched)");
        println!(
            "FG_INFO: set_value: before={fg_before} after={} fixture={} (refused)",
            fg_now(),
            fx.hwnd_i64()
        );
    }

    /// Puppet-side "no activation" gate: the policy refusal runs before any
    /// provider call, so the foreground contract IS stable here.
    #[test]
    fn set_value_dry_run_leaves_foreground_clean() {
        let _g = crate::fixture::desktop();
        let fg_before = crate::fixture::foreground_now();
        let (fx, bg, wpath, _ro) = edit_paths();
        let _ = fx.hwnd();
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": format!("path:{wpath}"), "text": "no-focus-probe"}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, &format!("path:{wpath}"));
        crate::fixture::check_foreground_left_clean(fx.hwnd_i64(), fg_before, "set_value:dry_run");
    }

    /// Gap-1 proof (fails before the fix, passes after): a fingerprint from
    /// an older snapshot must not silently address a different control.
    /// Resolve the live OK button, then invoke it with a WRONG expect_fp:
    /// the burst must refuse with a typed stale_ref (plus fresh-snapshot
    /// recovery advice), never act.
    #[test]
    fn invoke_stale_fp_refused_with_typed_error() {
        let _g = crate::fixture::desktop();
        let (fx, bg, _w, _ro) = edit_paths();
        let _ = fx.hwnd();
        let actions = vec![
            serde_json::json!({"type": "invoke", "element": "name:OK", "expect_fp": "definitely-wrong-fingerprint"}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(false)),
            "stale fp must refuse, got: {first:?}"
        );
        let e = first.get("error").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            e.contains("stale_ref"),
            "refusal must be typed stale_ref, got: {e}"
        );
        assert!(
            e.contains("fresh"),
            "refusal must advise a fresh snapshot, got: {e}"
        );
        assert_eq!(first.get("resolution"), Some(&Value::from("stale")));
    }

    /// Background-only policy: the UIA write never happens, so a stale
    /// fingerprint refuses with background_unavailable (not stale_ref) ÔÇö
    /// the policy check runs before any resolution.
    #[test]
    fn set_value_stale_fp_refused_with_typed_error() {
        let _g = crate::fixture::desktop();
        let (fx, bg, wpath, _ro) = edit_paths();
        let _ = fx.hwnd();
        let _ = wpath;
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": "name:OK", "text": "hi", "expect_fp": "definitely-wrong-fingerprint"}),
        ];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_set_value_refused(&first, "name:OK");
    }

    #[test]
    fn invoke_matching_fp_resolves() {
        let _g = crate::fixture::desktop();
        let (fx, bg, _w, _ro) = edit_paths();
        let _ = fx.hwnd();
        let model = crate::shot::element_model(fx.hwnd(), crate::UIA_DEPTH_LIMIT, 0);
        let entry = model
            .entries
            .iter()
            .find(|e| e.get("name").and_then(|v| v.as_str()) == Some("OK"))
            .expect("fixture must list the OK button");
        let fp = entry
            .get("fp")
            .and_then(|v| v.as_str())
            .expect("entry must carry fp");
        let actions =
            vec![serde_json::json!({"type": "invoke", "element": "name:OK", "expect_fp": fp})];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(true)),
            "matching fp must resolve, got: {first:?}"
        );
        assert_eq!(first.get("resolution"), Some(&Value::from("found")));
    }

    /// Gap-2 proof: the detection logic under test. Blocked exactly when the
    /// target is determinably elevated and this process is not -- elevated
    /// self may inject, unknown targets fail open.
    #[test]
    fn uipi_predicate_matrix() {
        assert!(crate::uipi_blocked(Some(true), false));
        assert!(!crate::uipi_blocked(Some(true), true));
        assert!(!crate::uipi_blocked(Some(false), false));
        assert!(!crate::uipi_blocked(Some(false), true));
        assert!(!crate::uipi_blocked(None, false));
        assert!(!crate::uipi_blocked(None, true));
        println!("UIPI_PREDICATE_MATRIX_OK=blockediff-target-elevated-and-self-not");
    }

    /// Gap-2 proof: the refusal text names the elevated target and the
    /// recovery, pinned by essentials (never byte-exact). The live
    /// end-to-end (medium server vs elevated target) is unproducible on a
    /// box with EnableLUA=0, where every process token reads elevated;
    /// reproduce it on a UAC-on box by running the server unelevated and
    /// aiming a dry_run background action at an elevated window.
    #[test]
    fn uipi_refusal_text_names_target_and_recovery() {
        let e = format_uipi_refusal("Taskmgr.exe", 1234, 5678);
        assert!(
            e.starts_with("uipi_blocked:"),
            "refusal must be typed, got: {e}"
        );
        for bit in ["Taskmgr.exe", "1234", "5678", "elevated", "dry_run"] {
            assert!(e.contains(bit), "refusal must name {bit}, got: {e}");
        }
    }

    #[test]
    fn uipi_own_process_readable_and_own_window_unblocked() {
        let _g = crate::fixture::desktop();
        let me_pid = std::process::id();
        assert!(
            crate::process_elevated(me_pid).is_some(),
            "own process token must be readable"
        );
        let f = crate::fixture::Fixture::new();
        assert!(
            uipi_refusal(f.hwnd_i64()).is_none(),
            "same-integrity target must never refuse"
        );
    }

    #[test]
    fn uipi_background_dry_run_reports_normally_when_not_elevated() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let _ = fx.hwnd();
        let actions = vec![serde_json::json!({"type": "click", "x": 1, "y": 1})];
        let (_, first) = burst_bg(&actions, &bg, true);
        assert_eq!(
            first.get("ok"),
            Some(&Value::from(true)),
            "non-elevated dry_run must still report ok, got: {first:?}"
        );
        assert!(
            first.get("blocked_by_uipi").is_none(),
            "non-elevated target must carry no UIPI flag, got: {first:?}"
        );
    }

    #[test]
    fn set_value_reresolve_failure_typed() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::Fixture::new();
        let (ok, via, err) = crate::uia::uia_set_value(f.hwnd_i64(), "9999", "x");
        assert!(!ok);
        let e = err.expect("stale path must be typed, never silent");
        assert!(
            e.contains("went stale") || e.contains("resolve failed"),
            "re-resolution failure must be typed, got: {e}"
        );
        assert!(
            via.is_empty() || via == "none",
            "resolve failure carries empty via, got: {via}"
        );
    }

    #[test]
    fn set_value_burst_continues_after_failure() {
        let _g = crate::fixture::desktop();
        let (fx, bg, wpath, _ro) = edit_paths();
        let _ = fx.hwnd();
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": "name:NoSuchXYZ", "text": "x"}),
            serde_json::json!({"type": "set_value", "element": format!("path:{wpath}"), "text": "after-failure"}),
        ];
        let out = do_burst(&DoArgs {
            background: Some(bg.to_string()),
            ..DoArgs::new(&actions)
        });
        let outer: Value = serde_json::from_str(&out).expect("outer");
        let payload = outer["structuredContent"].clone();
        let res = payload["results"].as_array().expect("results");
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].get("ok"), Some(&Value::from(false)));
        assert_eq!(res[1].get("ok"), Some(&Value::from(false)));
        assert_eq!(
            res[1].get("code").and_then(|c| c.as_str()),
            Some("background_unavailable")
        );
        assert_eq!(fx.edit_text(), "");
    }

    fn sc(out: &str) -> Value {
        let outer: Value = serde_json::from_str(out).expect("outer frame");
        outer
            .get("structuredContent")
            .cloned()
            .expect("structuredContent payload")
    }

    /// The foreground handle is the property the tool CAN violate, so a
    /// harness-owned foreground fails strictly ÔÇö with no exemption, and with
    /// both HWNDs plus the new owner's module in the message. A foreground
    /// change onto a window this process does NOT own is a third party taking
    /// focus (owner activity, shell churn, a parallel test binary): the gate
    /// re-baselines and samples again rather than blaming the tool, because
    /// attributing another process's activation to the harness is a false
    /// alarm, not evidence. See `fixture::assert_foreground_held_owned` for
    /// the decision rule, the sample bound and the log lines.
    fn assert_foreground_held(before: i64, owned: &crate::fixture::OwnedWindows, what: &str) {
        crate::fixture::assert_foreground_held_owned(before, owned, what);
    }

    /// Pointer movement is TELEMETRY, never a failure. The tool cannot move
    /// the pointer on these paths (no `SetCursorPos`/`SendInput` outside
    /// `src/gate.rs`, and no cursor permit is passed), while the owner using
    /// the machine moves it constantly ÔÇö a positional assertion here would be
    /// a flaky gate, not evidence. Report the delta and continue.
    fn report_pointer(before: (i64, i64), what: &str) {
        let after = cursor_now();
        if after == before {
            return;
        }
        println!(
            "CURSOR_EXTERNAL: {what}: pointer moved before={before:?} after={after:?} delta=({}, {}); telemetry only (no cursor API on this path)",
            after.0 - before.0,
            after.1 - before.1
        );
    }

    fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        f()
    }

    /// LIVE no-steal proof: set_value and type-on-element are refused with
    /// background_unavailable naming the control ÔÇö no provider call, no
    /// value written. No flags, no pointer, no focus.
    #[test]
    fn uia_text_entry_is_refused_by_policy() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let actions = vec![
            serde_json::json!({"type": "set_value", "element": "auto_id:100", "text": "secret"}),
            serde_json::json!({"type": "type", "element": "auto_id:100", "text": "secret"}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        for (k, r) in results.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(false), "action {k}: {r}");
            assert_eq!(
                r["code"],
                Value::from("background_unavailable"),
                "action {k} must name the policy: {r}"
            );
            assert!(
                r["error"].as_str().unwrap_or("").contains("auto_id:100"),
                "action {k} must name the control: {r}"
            );
            assert!(r.get("via").is_none(), "no provider path may run: {r}");
        }
        assert_eq!(
            fx.edit_text(),
            "",
            "no provider call may have written the control"
        );
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(payload["events"], Value::from(0));
        println!("UIA_TEXT_POLICY_OK refusals=2 text=\"\"");

        // Scope: `desktop:"hidden"` WITH an explicit ordinary target is
        // target-wins, not a private desktop. Both verbs stay refused exactly
        // as on the owner's desktop; the flag buys nothing there either.
        let scoped = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            desktop: Some("hidden".to_string()),
            ..DoArgs::new(&actions)
        });
        let sp = sc(&scoped);
        let sr = sp["results"].as_array().expect("results");
        for (k, r) in sr.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(false), "scoped action {k}: {r}");
            assert_eq!(
                r["code"],
                Value::from("background_unavailable"),
                "scoped action {k}: {r}"
            );
            assert!(
                r["error"].as_str().unwrap_or("").contains("auto_id:100"),
                "scoped action {k} must name the control: {r}"
            );
            assert!(r.get("via").is_none(), "no provider path may run: {r}");
        }
        assert!(
            sp.get("hidden_desktop").is_none(),
            "target-wins must not launch a hidden program: {sp}"
        );
        assert_eq!(fx.edit_text(), "", "no provider call may have written");
        println!("UIA_TEXT_SCOPE_OK owner refusals=2 hidden-with-target refusals=2");
    }

    /// SAFETY: `desktop:"hidden"` plus an explicit ordinary target is just
    /// an ordinary target: posted keyboard goes to the locked window (never
    /// SendInput, never the foreground), like any target-locked burst. The
    /// private-desktop permission is still EARNED, never granted by the flag.
    #[test]
    fn hidden_flag_with_an_ordinary_target_earns_nothing() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.edit() as i64;
        let actions = vec![
            serde_json::json!({"type": "type", "text": "x"}),
            serde_json::json!({"type": "key", "name": "a"}),
            serde_json::json!({"type": "hotkey", "keys": ["ctrl", "c"]}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            desktop: Some("hidden".to_string()),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        for (k, r) in results.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(true), "action {k}: {r}");
            assert_eq!(
                r["delivered"],
                Value::from("posted"),
                "action {k} must post to the locked target: {r}"
            );
        }
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(
            payload["ok"],
            Value::from(true),
            "a hidden flag with an explicit target must keep the target-win verdict: {payload}"
        );
        assert!(!private_desktop_earned(false, "hidden-x", Some("hidden-x")));
        assert!(!private_desktop_earned(true, "hidden-x", Some("Default")));
        assert!(!private_desktop_earned(true, "", Some("hidden-x")));
        assert!(private_desktop_earned(true, "hidden-x", Some("hidden-x")));
        println!("HIDDEN_BYPASS_CLOSED_OK posted=3 sendinput=0");
    }

    /// The desktop-name read is real: an ordinary fixture lives on the
    /// owner's desktop, never on a made-up private one.
    #[test]
    fn window_desktop_name_reads_a_real_desktop() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let name = crate::window_desktop_name(fx.hwnd()).expect("desktop name readable");
        assert!(!name.is_empty(), "desktop name must not be empty");
        assert_ne!(name, "puppet-hidden-does-not-exist");
        assert_eq!(
            crate::window_desktop_name(std::ptr::null_mut()),
            None,
            "a null window has no desktop"
        );
        println!("WINDOW_DESKTOP_NAME_OK name={name:?}");
    }

    /// LIVE: a target-locked burst delivers a posted click and posted text
    /// to an offscreen fixture window (its own WndProc counts the click, the
    /// EDIT guard counts the swallowed WM_CHAR) while GetForegroundWindow
    /// and GetCursorPos stay exactly where they were. No flags, no pointer,
    /// no focus, no SendInput.
    #[test]
    fn live_target_post_delivers_without_stealing() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        crate::fixture::reset_msg_counts();
        crate::fixture::reset_swallowed_keys(fx.edit());
        crate::fixture::reset_swallowed_keys(fx.edit_ro());
        let fg_before = crate::fixture::foreground_now();
        let owned = crate::fixture::OwnedWindows::new().with_fixture(&fx);
        let cur_before = cursor_now();
        let actions = vec![
            serde_json::json!({"type": "click", "x": 20, "y": 90}),
            serde_json::json!({"type": "type", "text": "hi"}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results[0]["ok"], Value::from(true), "click: {results:?}");
        assert_eq!(
            results[1]["ok"],
            Value::from(true),
            "plain type posts to the locked target: {results:?}"
        );
        assert_eq!(results[1]["delivered"], Value::from("posted"));
        assert_eq!(results[1]["effect"], Value::from("unverified"));
        assert_eq!(results[1]["posted"], Value::from(2), "h+i = 2 WM_CHAR");
        assert!(
            payload["foregroundUnchanged"].is_boolean(),
            "telemetry must be reported: {payload}"
        );
        assert!(
            payload["cursorUnchanged"].is_boolean(),
            "telemetry must be reported: {payload}"
        );
        assert_eq!(payload["target_locked"], Value::from(hwnd));
        assert!(
            payload["mode"].is_null(),
            "per-action mode lives on results"
        );
        assert_eq!(results[0]["mode"], Value::from("background"));

        assert!(
            wait_until(|| {
                let (down, up, ..) = crate::fixture::msg_counts();
                down >= 1 && up >= 1
            }),
            "posted click pair must reach the target, got: {:?}",
            crate::fixture::msg_counts()
        );
        let (down, up, mv, _kd, _ch) = crate::fixture::msg_counts();
        assert!(
            down >= 1 && up >= 1,
            "down/up pair must arrive: {down}/{up}"
        );
        assert_eq!(mv, 0, "a plain click must not post WM_MOUSEMOVE");
        assert!(
            wait_until(|| {
                crate::fixture::swallowed_key_count(fx.edit())
                    + crate::fixture::swallowed_key_count(fx.edit_ro())
                    >= 2
            }),
            "posted WM_CHAR must reach a text control, swallowed(edit)={} swallowed(ro)={}",
            crate::fixture::swallowed_key_count(fx.edit()),
            crate::fixture::swallowed_key_count(fx.edit_ro())
        );
        assert_eq!(
            fx.edit_text(),
            "",
            "the guard swallows instead of inserting"
        );
        assert_foreground_held(fg_before, &owned, "post-delivery burst");
        report_pointer(cur_before, "post-delivery burst");
        assert_eq!(
            payload["sendinput_calls"],
            Value::from(0),
            "a target burst never calls SendInput"
        );
    }

    /// LIVE: plain `type` posts WM_CHAR to the locked target's text
    /// control ÔÇö delivery, not proof of effect. The fixture guard swallows
    /// the characters (counter proof they arrived) instead of inserting
    /// them, so the EDIT stays empty while the post count is exact.
    #[test]
    fn live_type_posts_wm_char_to_the_locked_target() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        crate::fixture::reset_msg_counts();
        crate::fixture::reset_swallowed_keys(fx.edit());
        crate::fixture::reset_swallowed_keys(fx.edit_ro());
        let actions = vec![serde_json::json!({
            "type": "type", "text": "hi"
        })];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["ok"], Value::from(true), "type posts: {r}");
        assert_eq!(r["delivered"], Value::from("posted"), "{r}");
        assert_eq!(r["effect"], Value::from("unverified"), "{r}");
        assert_eq!(r["posted"], Value::from(2), "h+i = 2 WM_CHAR: {r}");
        assert_eq!(
            fx.edit_text(),
            "",
            "the guard swallows instead of inserting"
        );
        assert!(
            wait_until(|| {
                crate::fixture::swallowed_key_count(fx.edit())
                    + crate::fixture::swallowed_key_count(fx.edit_ro())
                    >= 2
            }),
            "posted WM_CHAR must reach a text control, swallowed(edit)={} swallowed(ro)={}",
            crate::fixture::swallowed_key_count(fx.edit()),
            crate::fixture::swallowed_key_count(fx.edit_ro())
        );
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        println!("TYPE_POSTED_OK text=\"\"");
    }

    /// LIVE: a posted element click is delivery, not proof. The fixture EDIT
    /// has no Invoke pattern, so the click is posted at its centre; its
    /// readable state (text/enabled) does not change, and the action must
    /// report `background_unavailable` with `delivered:"posted"` instead of
    /// claiming success.
    #[test]
    fn live_posted_element_click_with_unchanged_state_is_refused() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let fp = live_fp(fx.hwnd(), "auto_id", "100");
        let actions = vec![serde_json::json!({
            "type": "click", "element": "auto_id:100", "expect_fp": fp
        })];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["delivered"], Value::from("posted"), "{r}");
        assert_eq!(r["effect"], Value::from("unchanged"), "{r}");
        assert_eq!(r["ok"], Value::from(false), "{r}");
        assert_eq!(r["code"], Value::from("background_unavailable"), "{r}");
        assert!(
            r["error"]
                .as_str()
                .unwrap_or("")
                .contains("state did not change"),
            "the refusal must name the unverified effect: {r}"
        );
        println!("POSTED_CLICK_HONESTY_OK effect=unchanged");
    }

    /// LIVE: hover is opt-in. The same click with `hover:true` posts exactly
    /// one WM_MOUSEMOVE before the pair; without it, none.
    #[test]
    fn live_hover_is_the_only_mouse_move() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let actions = vec![serde_json::json!({"type": "click", "x": 5, "y": 5, "hover": true})];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        assert_eq!(sc(&out)["results"][0]["ok"], Value::from(true));
        assert!(wait_until(|| crate::fixture::msg_counts().2 >= 1));
        assert_eq!(
            crate::fixture::msg_counts().2,
            1,
            "one hover move, not a journey"
        );
    }

    /// LIVE: drag is down/move/up at exact points; scroll is one wheel
    /// message at the point. Both delivered by message, never by pointer.
    #[test]
    fn live_drag_and_scroll_are_posted_not_pointer() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let cur_before = cursor_now();
        crate::fixture::reset_msg_counts();
        let actions = vec![
            serde_json::json!({"type": "drag", "from": [10, 10], "to": [40, 40]}),
            serde_json::json!({"type": "scroll", "x": 10, "y": 10, "amount": 2}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        assert_eq!(payload["results"][0]["ok"], Value::from(true), "{payload}");
        assert_eq!(payload["results"][1]["ok"], Value::from(true), "{payload}");
        assert!(wait_until(|| {
            let (down, up, mv, _, _) = crate::fixture::msg_counts();
            down >= 1 && up >= 1 && mv >= 1
        }));
        let (down, up, mv, _, _) = crate::fixture::msg_counts();
        assert_eq!((down, up, mv), (1, 1, 1), "down, one move, up");
        assert!(wait_until(|| crate::fixture::wheel_count() >= 1));
        report_pointer(cur_before, "posted drag + wheel");
    }

    /// LIVE gate: no target and no flags refuses every injecting verb BEFORE
    /// acting - typed codes, zero events, desktop untouched.
    #[test]
    fn live_gate_refuses_without_target_or_flags() {
        let _g = crate::fixture::desktop();
        let fg_before = crate::fixture::foreground_now();
        let owned = crate::fixture::OwnedWindows::new();
        let cur_before = cursor_now();
        let actions = vec![
            serde_json::json!({"type": "click", "x": 5, "y": 5}),
            serde_json::json!({"type": "type", "text": "x"}),
            serde_json::json!({"type": "hotkey", "keys": ["ctrl", "c"]}),
            serde_json::json!({"type": "move", "dx": 5, "dy": 5}),
            serde_json::json!({"type": "invoke", "element": "name:OK"}),
        ];
        let out = do_burst(&DoArgs::new(&actions));
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        let codes: Vec<&str> = results
            .iter()
            .map(|r| r["code"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            codes,
            vec![
                "no_cursor_policy",
                "target_required",
                "target_required",
                "no_cursor_policy",
                "target_required",
            ],
            "gate matrix, got: {results:?}"
        );
        for r in results {
            assert_eq!(r["ok"], Value::from(false));
        }
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(payload["events"], Value::from(0));
        assert_foreground_held(fg_before, &owned, "gate refusal");
        report_pointer(cur_before, "gate refusal");
    }

    /// LIVE-safe (dry_run): flags alone buy nothing. Without a locked target
    /// the point guard cannot run and no action is injected; with one the
    /// cursor verb takes the real path (proved by the ignored live test).
    #[test]
    fn live_flags_without_a_target_refuse_dry_run() {
        let actions = vec![
            serde_json::json!({"type": "click", "x": 5, "y": 5}),
            serde_json::json!({"type": "type", "text": "x"}),
        ];
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_foreground: true,
            allow_cursor: true,
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        assert_eq!(
            payload["results"][0]["code"],
            Value::from("target_required"),
            "a real pointer needs a locked target: {payload:?}"
        );
        assert_eq!(
            payload["results"][1]["code"],
            Value::from("target_required"),
            "no fallback to the foreground window: {payload:?}"
        );
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(payload["events"], Value::from(0));
        println!("FLAGS_WITHOUT_TARGET_OK codes=target_required events=0");
    }

    /// LIVE: a target that closed mid-call is `target_gone` on the next
    /// action (revalidation), never a retarget and never a foreground fall.
    #[test]
    fn live_target_gone_mid_burst_is_typed() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        let actions = vec![
            serde_json::json!({"type": "wait", "ms": 1}),
            serde_json::json!({"type": "type", "text": "x"}),
        ];
        let args = DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        };
        let spec = crate::target::TargetSpec::parse(&serde_json::json!({ "hwnd": hwnd })).unwrap();
        let lock = crate::target::lock(&spec).expect("fresh fixture locks");
        drop(fx);
        let e = lock
            .revalidate()
            .expect_err("destroyed target must not validate");
        assert_eq!(e.code, "target_gone", "{}", e.text());
        let outer: Value = serde_json::from_str(&do_burst(&args)).expect("outer frame");
        assert_eq!(outer["isError"], Value::from(true), "{outer}");
        assert!(outer["content"][0]["text"].as_str().is_some());
    }

    /// LIVE, message-only: the harshest delivery case ÔÇö a `HWND_MESSAGE`
    /// window that is never visible and never activatable. A target-locked
    /// posted CLICK arrives in its own WndProc while GetForegroundWindow and
    /// GetCursorPos stay put; posted KEYS arrive too (the route is
    /// posted-only now), proven with verify:false; the default verify:true
    /// honestly reports unverified because a message-only window cannot be
    /// captured for the before/after diff.
    #[test]
    fn live_message_only_click_and_key_post_without_stealing() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let hwnd = f.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let fg_before = crate::fixture::foreground_now();
        let owned = crate::fixture::OwnedWindows::new().with_top(hwnd);
        let cur_before = cursor_now();
        let actions = vec![
            serde_json::json!({"type": "click", "x": 4, "y": 6}),
            serde_json::json!({"type": "key", "name": "a", "verify": false}),
            serde_json::json!({"type": "hotkey", "keys": ["ctrl", "c"], "verify": false}),
            serde_json::json!({"type": "key", "name": "a"}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results[0]["ok"], Value::from(true), "click: {results:?}");
        for k in [1, 2] {
            assert_eq!(
                results[k]["ok"],
                Value::from(true),
                "posted keyboard: {results:?}"
            );
            assert_eq!(results[k]["delivered"], Value::from("posted"));
            assert_eq!(results[k]["effect"], Value::from("unverified"));
        }
        assert_eq!(results[3]["ok"], Value::from(false), "verify: {results:?}");
        assert_eq!(results[3]["delivered"], Value::from("none"));
        assert_eq!(results[3]["effect"], Value::from("unverified"));
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert!(
            wait_until(|| {
                let (down, up, ..) = crate::fixture::msg_counts();
                down >= 1 && up >= 1
            }),
            "posted click pair must be received: {:?}",
            crate::fixture::msg_counts()
        );
        let (down, up, mv, kd, ch) = crate::fixture::msg_counts();
        assert_eq!((down, up, mv), (1, 1, 0), "one down, one up, no move");
        assert!(
            kd >= 3,
            "posted keys must arrive as WM_KEYDOWN: kd={kd} ch={ch}"
        );
        assert_eq!(
            results[0]["delivered"],
            Value::from("posted"),
            "a raw click reports delivery only: {results:?}"
        );
        assert_eq!(results[0]["effect"], Value::from("unverified"));
        assert_foreground_held(fg_before, &owned, "message-only delivery");
        report_pointer(cur_before, "message-only delivery");
    }

    /// BACKGROUND-ONLY POLICY refusal tests: `delivery:"sendinput"` and
    /// any unknown delivery never act ÔÇö typed refusal, zero events, zero
    /// SendInput calls, under dry_run too.
    #[test]
    fn sendinput_delivery_and_unknown_delivery_are_refused() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.hwnd_i64();
        for (action, want) in [
            (
                serde_json::json!({"type": "key", "name": "a", "delivery": "sendinput"}),
                "no_foreground_policy",
            ),
            (
                serde_json::json!({"type": "hotkey", "keys": ["ctrl", "c"], "delivery": "sendinput"}),
                "no_foreground_policy",
            ),
            (
                serde_json::json!({"type": "key", "name": "a", "delivery": "telepathy"}),
                "",
            ),
        ] {
            let out = do_burst(&DoArgs {
                dry_run: true,
                target: Some(serde_json::json!({ "hwnd": hwnd })),
                ..DoArgs::new(&[action])
            });
            let payload = sc(&out);
            let r = &payload["results"][0];
            assert_eq!(r["ok"], Value::from(false), "delivery refusal: {r}");
            if !want.is_empty() {
                assert_eq!(r["code"], Value::from(want), "typed refusal: {r}");
            }
            assert_eq!(payload["events"], Value::from(0));
            assert_eq!(payload["sendinput_calls"], Value::from(0));
        }
        println!("SENDINPUT_DELIVERY_REFUSED_OK code=no_foreground_policy events=0");
    }

    /// D3 classifier: the three effects are exclusive and none assumes a
    /// readback the control never exposed.
    #[test]
    fn uia_effect_classifier_three_states() {
        assert_eq!(
            super::effect_of(Some("value=alpha".into()), Some("value=beta".into())),
            "verified",
            "a readable state that moved is verified"
        );
        assert_eq!(
            super::effect_of(Some("value=alpha".into()), Some("value=alpha".into())),
            "unchanged",
            "a readable state that stayed is unchanged"
        );
        assert_eq!(
            super::effect_of(None, None),
            "pattern-returned",
            "no readable state is the provider accepting it, never verified"
        );
        assert_eq!(
            super::effect_of(None, Some("value=beta".into())),
            "pattern-returned",
            "a state that only appeared after cannot prove the act"
        );
        assert_eq!(
            super::effect_of(Some("value=alpha".into()), None),
            "pattern-returned",
            "a state that vanished after cannot prove the act"
        );
        println!("UIA_EFFECT_CLASSIFIER_OK verified|unchanged|pattern-returned");
    }

    /// LIVE: `verify:true` on a key that changes nothing visible reports
    /// `effect:"unchanged"` ÔÇö a result, not an error ÔÇö with the exact zero
    /// count and a null bbox.
    #[test]
    fn verify_true_on_a_no_op_key_reports_unchanged() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let hwnd = fx.edit() as i64;
        let fg_before = crate::fixture::foreground_now();
        let owned = crate::fixture::OwnedWindows::new().with_fixture(&fx);
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&[serde_json::json!({"type": "key", "name": "F13", "verify": true})])
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["ok"], Value::from(true), "verify on a no-op: {r}");
        assert_eq!(r["delivered"], Value::from("posted"), "{r}");
        assert_eq!(r["effect"], Value::from("unchanged"), "{r}");
        assert_eq!(r["changed"], Value::from(0), "{r}");
        assert!(r["bbox"].is_null(), "no pixels changed, no box: {r}");
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_foreground_held(fg_before, &owned, "verified posted key");
        println!("VERIFY_UNCHANGED_OK effect=unchanged changed=0");
    }

    /// LIVE: `click` on an element is UIA-pattern-first on the locked
    /// target (dry_run resolves and names the path without firing).
    #[test]
    fn live_click_on_element_resolves_to_uia_pattern() {
        let _g = crate::fixture::desktop();
        let (fx, bg) = fixture_bg();
        let fp = live_fp(fx.hwnd(), "name", "OK");
        let actions = vec![serde_json::json!({
            "type": "click", "element": "name:OK", "expect_fp": fp
        })];
        let out = do_burst(&DoArgs {
            background: Some(bg),
            dry_run: true,
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let first = &payload["results"][0];
        assert_eq!(first["ok"], Value::from(true), "{first}");
        assert_eq!(first["resolution"], Value::from("found"), "{first}");
        assert_eq!(first["via"], Value::from("uia-pattern"), "{first}");
        assert_eq!(payload["events"], Value::from(0), "dry run queues nothing");
    }

    /// LIVE-safe: a cursor-only call with NO target cannot run the point
    /// guard, so it refuses with target_required BEFORE anything is posted:
    /// ok:false, typed code, zero events, zero posts, zero injected input.
    #[test]
    fn live_cursor_only_pointer_actions_are_typed_failures() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let _hwnd = f.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let actions = vec![
            serde_json::json!({"type": "click", "x": 5, "y": 5}),
            serde_json::json!({"type": "drag", "from": [1, 1], "to": [2, 2]}),
            serde_json::json!({"type": "move", "dx": 1, "dy": 1}),
        ];
        let out = do_burst(&DoArgs {
            allow_cursor: true,
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        for (k, r) in results.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(false), "action {k} must fail: {r}");
            assert_eq!(
                r["code"],
                Value::from("target_required"),
                "action {k} must name the missing lock: {r}"
            );
        }
        assert_eq!(payload["events"], Value::from(0), "nothing may be queued");
        assert_eq!(payload["sendinput_calls"], Value::from(0));
        assert_eq!(
            crate::fixture::msg_counts(),
            (0, 0, 0, 0, 0),
            "nothing may reach the target"
        );
    }

    /// LIVE-safe: with BOTH flags but no target nothing is injected. The
    /// other half of "refuses or posts, never silent".
    #[test]
    fn live_both_flags_without_a_target_refuse() {
        let actions = vec![serde_json::json!({"type": "click", "x": 5, "y": 5})];
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_foreground: true,
            allow_cursor: true,
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        assert_eq!(payload["results"][0]["ok"], Value::from(false));
        assert_eq!(
            payload["results"][0]["code"],
            Value::from("target_required"),
            "flags without a lock must refuse: {payload:?}"
        );
        assert_eq!(payload["events"], Value::from(0));
        assert_eq!(payload["sendinput_calls"], Value::from(0));
    }

    /// Without the opt-in there is no queue, so a flush never sends, never
    /// fails and never marks anything ÔÇö posted branches sequence through it
    /// harmlessly. The opt-in queue is proved by the ignored live test.
    #[test]
    fn flush_is_a_noop_and_sends_nothing() {
        let mut st = Burst {
            pending: Vec::new(),
            marked: vec![0],
            results: vec![base_entry(0, &Value::from("click"), true, 0.0)],
            calls: 0,
            events: 0,
            sendinput_ms: 0.0,
            flush_uipi: false,
            real: None,
        };
        let (dt, err) = st.flush();
        assert!(err.is_none(), "a flush never fails: {err:?}");
        assert_eq!(dt, 0.0);
        assert_eq!(st.events, 0, "nothing was sent");
        assert_eq!(st.calls, 0, "no SendInput call was made");
        assert_eq!(st.results[0]["ok"], Value::from(true), "nothing is marked");
    }

    /// LIVE, message-only: coordinates a message lParam cannot carry are a
    /// typed `bad_coordinates` refusal BEFORE posting; the 16-bit boundaries
    /// still post.
    #[test]
    fn live_bad_coordinates_refused_before_posting() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let hwnd = f.hwnd_i64();
        let target = Some(serde_json::json!({ "hwnd": hwnd }));
        crate::fixture::reset_msg_counts();
        let bad = vec![
            serde_json::json!({"type": "click", "x": 40000, "y": 0}),
            serde_json::json!({"type": "click", "x": 0, "y": 40000}),
            serde_json::json!({"type": "click", "x": 32768, "y": 0}),
            serde_json::json!({"type": "click", "x": 0, "y": -32769}),
            serde_json::json!({"type": "scroll", "x": 40000, "y": 10}),
        ];
        for action in bad {
            let out = do_burst(&DoArgs {
                target: target.clone(),
                ..DoArgs::new(&[action.clone()])
            });
            let payload = sc(&out);
            let r = &payload["results"][0];
            assert_eq!(r["ok"], Value::from(false), "{action}: {r}");
            assert!(
                r["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("bad_coordinates"),
                "{action} must be typed bad_coordinates, got: {r}"
            );
            assert_eq!(
                crate::fixture::msg_counts(),
                (0, 0, 0, 0, 0),
                "{action} must post nothing"
            );
        }
        let action = serde_json::json!({"type": "click", "x": 32767, "y": -32768});
        let out = do_burst(&DoArgs {
            target: target.clone(),
            ..DoArgs::new(&[action.clone()])
        });
        let payload = sc(&out);
        assert_eq!(
            payload["results"][0]["ok"],
            Value::from(true),
            "boundary {action}: {payload}"
        );
        assert!(
            wait_until(|| crate::fixture::msg_counts().0 >= 1),
            "the boundary click must be posted"
        );
    }

    /// LIVE (message-only): a legacy `background` screen point wider than i32
    /// must be refused BEFORE the `as i32` cast (4294967296 would otherwise
    /// become 0 and pass the 16-bit check), and an i32-representable point
    /// that leaves the 16-bit range after ScreenToClient is still refused.
    #[test]
    fn live_bg_screen_coords_validated_before_and_after_the_cast() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let bg = format!("hwnd:{}", f.hwnd_i64());
        crate::fixture::reset_msg_counts();
        for x in [4294967296i64, i32::MAX as i64 + 1, i32::MIN as i64 - 1] {
            let out = do_burst(&DoArgs {
                background: Some(bg.clone()),
                ..DoArgs::new(&[serde_json::json!({"type": "click", "x": x, "y": 0})])
            });
            let payload = sc(&out);
            let r = &payload["results"][0];
            assert_eq!(r["ok"], Value::from(false), "x={x}: {r}");
            assert!(
                r["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("bad_coordinates"),
                "x={x} must be typed bad_coordinates, got: {r}"
            );
            assert_eq!(
                crate::fixture::msg_counts(),
                (0, 0, 0, 0, 0),
                "x={x} must post nothing"
            );
        }
        let out = do_burst(&DoArgs {
            background: Some(bg.clone()),
            ..DoArgs::new(&[serde_json::json!({"type": "click", "x": i32::MAX as i64, "y": 0})])
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["ok"], Value::from(false), "i32::MAX: {r}");
        assert!(
            r["error"]
                .as_str()
                .unwrap_or("")
                .contains("bad_coordinates"),
            "the post-conversion 16-bit check must still fire, got: {r}"
        );
        let out = do_burst(&DoArgs {
            background: Some(bg),
            ..DoArgs::new(&[serde_json::json!({"type": "click", "x": 5, "y": 5})])
        });
        let payload = sc(&out);
        assert_eq!(payload["results"][0]["ok"], Value::from(true), "{payload}");
        assert!(
            wait_until(|| crate::fixture::msg_counts().0 >= 1),
            "the in-range click must be posted"
        );
    }

    /// LIVE: an ambiguous target is listed, never guessed.
    #[test]
    fn live_ambiguous_target_reports_candidates() {
        let _g = crate::fixture::desktop();
        let a = crate::fixture::Fixture::new();
        let b = crate::fixture::Fixture::new();
        let spec = crate::target::TargetSpec::parse(&serde_json::json!({
            "exe": std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .unwrap_or_else(|| "puppet.exe".to_string()),
            "className": format!("PuppetFixture"),
        }))
        .unwrap();
        let _ = (&a, &b);
        let class_a = unsafe { crate::win_class(a.hwnd()) };
        let spec_a = crate::target::TargetSpec::parse(&serde_json::json!({
            "exe": std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .unwrap_or_else(|| "puppet.exe".to_string()),
            "className": class_a,
        }))
        .unwrap();
        let l = crate::target::lock(&spec_a).expect("pid-free exe+class target resolves");
        assert_eq!(l.hwnd, a.hwnd_i64());
        let e = crate::target::lock(&spec).expect_err("prefix class must not match");
        assert_eq!(e.code, "target_gone", "{}", e.text());
    }

    /// A dry burst reports nothing delivered. This is the exact shape that
    /// used to answer delivered:"posted", effect:"unverified", posted:0.
    #[test]
    fn dry_run_reports_nothing_delivered() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let hwnd = f.hwnd_i64();
        let actions = vec![
            serde_json::json!({"type": "type", "text": "hi"}),
            serde_json::json!({"type": "click", "x": 10, "y": 10}),
        ];
        let out = do_burst(&DoArgs {
            dry_run: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results.len(), 2, "{payload}");
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(true), "result {i}: {r}");
            assert_eq!(r["delivered"], Value::from("none"), "result {i}: {r}");
            assert_eq!(r["posted"], Value::from(0), "result {i}: {r}");
            assert_eq!(r["effect"], Value::from("none"), "result {i}: {r}");
            assert_eq!(r["dry_run"], Value::from(true), "result {i}: {r}");
            assert_eq!(r["mode"], Value::from("background"), "result {i}: {r}");
            assert_eq!(r["bg_hwnd"], Value::from(hwnd), "result {i}: {r}");
        }
    }

    /// A dry run makes no delivery call: no PostMessage, no SendInput, no
    /// SetCursorPos, no SetForegroundWindow. The opt-in flags are set here on
    /// purpose: they must not unlock a delivery under dry_run.
    #[test]
    fn dry_run_posts_no_messages_and_keeps_the_pointer() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let hwnd = f.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let fg_before = fg_now();
        let cur_before = cursor_now();
        let actions = vec![
            serde_json::json!({"type": "click", "x": 10, "y": 10}),
            serde_json::json!({"type": "type", "text": "hi"}),
            serde_json::json!({"type": "key", "name": "a"}),
        ];
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_cursor: true,
            allow_foreground: true,
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        for (i, r) in payload["results"]
            .as_array()
            .expect("results")
            .iter()
            .enumerate()
        {
            assert_eq!(r["delivered"], Value::from("none"), "result {i}: {r}");
            assert_eq!(r["posted"], Value::from(0), "result {i}: {r}");
            assert_eq!(r["effect"], Value::from("none"), "result {i}: {r}");
            assert_eq!(r["dry_run"], Value::from(true), "result {i}: {r}");
        }
        assert_eq!(payload["posted_messages"], Value::from(0), "{payload}");
        assert_eq!(payload["sendinput_calls"], Value::from(0), "{payload}");
        assert_eq!(payload["events"], Value::from(0), "{payload}");
        assert_eq!(payload["clipboard"]["writes"], Value::from(0), "{payload}");
        assert_eq!(
            payload["clipboard"]["restores"],
            Value::from(0),
            "{payload}"
        );
        assert_eq!(payload["clipboard"]["bytes"], Value::from(0), "{payload}");
        assert_eq!(payload["cursorMoved"], Value::from(false), "{payload}");
        assert!(payload["cursorRestored"].is_null(), "{payload}");
        assert_eq!(
            payload["foregroundUnchanged"],
            Value::from(true),
            "{payload}"
        );
        assert_eq!(
            crate::fixture::msg_counts(),
            (0, 0, 0, 0, 0),
            "a dry run must post no window message"
        );
        assert_eq!(
            cursor_now(),
            cur_before,
            "a dry run must not move the pointer"
        );
        assert_eq!(
            fg_now(),
            fg_before,
            "a dry run must not change the foreground"
        );
    }

    /// The control: the same burst without dry_run still reports real
    /// delivery and real posted counts, so the dry-run fix is not a blanket
    /// zeroing of every result.
    #[test]
    fn non_dry_burst_reports_real_delivery() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::MessageWindow::new();
        let hwnd = f.hwnd_i64();
        crate::fixture::reset_msg_counts();
        let actions = vec![
            serde_json::json!({"type": "type", "text": "hi"}),
            serde_json::json!({"type": "click", "x": 10, "y": 10}),
        ];
        let out = do_burst(&DoArgs {
            target: Some(serde_json::json!({ "hwnd": hwnd })),
            ..DoArgs::new(&actions)
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        assert_eq!(results[0]["ok"], Value::from(true), "{results:?}");
        assert_eq!(
            results[0]["delivered"],
            Value::from("posted"),
            "{results:?}"
        );
        assert_eq!(results[0]["posted"], Value::from(2), "{results:?}");
        assert!(results[0].get("dry_run").is_none(), "{results:?}");
        assert_eq!(results[1]["ok"], Value::from(true), "{results:?}");
        assert_eq!(
            results[1]["delivered"],
            Value::from("posted"),
            "{results:?}"
        );
        assert_eq!(results[1]["posted"], Value::from(2), "{results:?}");
        assert!(results[1].get("dry_run").is_none(), "{results:?}");
        // The summary counts posted window messages (the click pair); the
        // posted-key count lives on the type result, not in this counter.
        assert_eq!(payload["posted_messages"], Value::from(2), "{payload}");
        assert!(
            wait_until(|| {
                let (down, up, ..) = crate::fixture::msg_counts();
                down >= 1 && up >= 1
            }),
            "a real burst must still post the click pair: {:?}",
            crate::fixture::msg_counts()
        );
    }

    /// A dry run does not even open the clipboard. The sentinel is seeded
    /// first: the old clipboard_get branch would have read it, and the old
    /// clipboard_set branch would have overwritten it.
    #[test]
    fn dry_run_clipboard_actions_never_open_or_write() {
        let _g = crate::fixture::desktop();
        let _lock = clip_test_lock();
        let _owner = ClipboardOwnerGuard(clip_snapshot_retry());
        clip_write_retry("dry-run-sentinel");
        let out = do_burst(&DoArgs {
            dry_run: true,
            allow_clipboard: true,
            restore_clipboard: true,
            ..DoArgs::new(&[
                serde_json::json!({"type": "clipboard_set", "text": "should-not-land"}),
                serde_json::json!({"type": "clipboard_get"}),
            ])
        });
        let payload = sc(&out);
        let results = payload["results"].as_array().expect("results");
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r["ok"], Value::from(true), "result {i}: {r}");
            assert_eq!(r["delivered"], Value::from("none"), "result {i}: {r}");
            assert_eq!(r["posted"], Value::from(0), "result {i}: {r}");
            assert_eq!(r["effect"], Value::from("none"), "result {i}: {r}");
        }
        assert!(
            results[0].get("bytes").is_none(),
            "no clipboard write: {}",
            results[0]
        );
        assert!(
            results[1].get("text").is_none(),
            "no clipboard read: {}",
            results[1]
        );
        assert_eq!(payload["clipboard"]["writes"], Value::from(0), "{payload}");
        assert_eq!(
            payload["clipboard"]["restores"],
            Value::from(0),
            "{payload}"
        );
        assert_eq!(payload["clipboard"]["bytes"], Value::from(0), "{payload}");
        assert_eq!(
            clip_get_retry(),
            "dry-run-sentinel",
            "the clipboard is untouched"
        );
    }

    /// A dry run never sends BM_CLICK (SendMessageTimeoutW in win32_activate)
    /// and never fires a UIA pattern: invoke resolves the control and stops
    /// at via:"none".
    #[test]
    fn dry_run_invoke_resolves_without_sending_bm_click() {
        let _g = crate::fixture::desktop();
        let fx = crate::fixture::Fixture::new();
        let bg = format!("hwnd:{}", fx.hwnd_i64());
        let fp = live_fp(fx.hwnd(), "name", "OK");
        let out = do_burst(&DoArgs {
            background: Some(bg),
            dry_run: true,
            ..DoArgs::new(&[serde_json::json!({
                "type": "invoke", "element": "name:OK", "expect_fp": fp
            })])
        });
        let payload = sc(&out);
        let r = &payload["results"][0];
        assert_eq!(r["ok"], Value::from(true), "{r}");
        assert_eq!(r["resolution"], Value::from("found"), "{r}");
        assert_eq!(r["via"], Value::from("none"), "{r}");
        assert_eq!(r["delivered"], Value::from("none"), "{r}");
        assert_eq!(r["effect"], Value::from("none"), "{r}");
        assert_eq!(payload["sendinput_calls"], Value::from(0), "{payload}");
        assert_eq!(payload["events"], Value::from(0), "{payload}");
    }
}
