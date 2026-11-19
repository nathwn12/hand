//! Attribution for the two resources a read-only desktop call must not touch:
//! the pointer and the foreground window.
//!
//! Both are process-global. `GetCursorPos` and `GetForegroundWindow` report
//! whoever last changed them, so a raw before/after equality check cannot say
//! *this process* did it. The first 100-attempt `desktop-capture` receipt
//! rejected 37 attempts for a pointer delta that no pointer-writing call in
//! this binary can produce: the owner's mouse moved during the measured window.
//! `tests/no_steal.rs` already treats the pointer as telemetry for that reason;
//! the benchmark gate demanded positional equality anyway.
//!
//! This module keeps the raw booleans (`cursorUnchanged`,
//! `foregroundUnchanged`) and adds the evidence needed to attribute a change:
//!
//! * `bannedImports` is the executable's own import table, read at runtime. An
//!   empty list is the mechanism proof that this process cannot call
//!   `SetCursorPos`, `SendInput`, `mouse_event`, `SetForegroundWindow` or the
//!   other pointer/focus writers, whatever any other document says.
//! * `inputTickBefore/After` come from `GetLastInputInfo`: a pointer delta with
//!   real input in the window is the owner's input. A delta with no input is
//!   not attributable to the owner, and the gate refuses it.
//! * `stolen` is true only when the new foreground belongs to this process or
//!   the locked target became foreground. A third party's window is an external
//!   change, the same classification `tests/no_steal.rs` applies.
//!
//! Nothing here suppresses a measurement: the raw samples stay in the payload.

use serde_json::{Map, Value};
use std::sync::OnceLock;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// APIs that can move the pointer or take the foreground. The capture and
/// element paths must link none of them; `banned_imports()` reads the running
/// executable's import table and reports any that appear.
const BANNED: &[&str] = &[
    "SendInput",
    "SetCursorPos",
    "SetPhysicalCursorPos",
    "mouse_event",
    "keybd_event",
    "SetForegroundWindow",
    "BringWindowToTop",
    "SetActiveWindow",
    "SwitchToThisWindow",
    "AttachThreadInput",
    "SetFocus",
];

#[derive(Clone, Debug)]
pub struct Audit {
    /// True when the import table was read; a parse failure is reported, never
    /// treated as a clean audit.
    pub ok: bool,
    pub banned: Vec<String>,
    pub error: Option<String>,
}

static AUDIT: OnceLock<Audit> = OnceLock::new();

/// The import-table audit for the running executable, computed once.
pub fn banned_imports() -> &'static Audit {
    AUDIT.get_or_init(|| match std::env::current_exe() {
        Ok(path) => match std::fs::read(&path) {
            Ok(bytes) => match pe_import_names(&bytes) {
                Ok(names) => {
                    let mut banned: Vec<String> = names
                        .into_iter()
                        .filter(|name| BANNED.iter().any(|b| b.eq_ignore_ascii_case(name)))
                        .collect();
                    banned.sort();
                    banned.dedup();
                    Audit {
                        ok: true,
                        banned,
                        error: None,
                    }
                }
                Err(e) => Audit {
                    ok: false,
                    banned: Vec::new(),
                    error: Some(format!("cannot parse the executable's import table: {e}")),
                },
            },
            Err(e) => Audit {
                ok: false,
                banned: Vec::new(),
                error: Some(format!("cannot read the executable for the import audit: {e}")),
            },
        },
        Err(e) => Audit {
            ok: false,
            banned: Vec::new(),
            error: Some(format!("cannot locate the executable for the import audit: {e}")),
        },
    })
}

fn cursor() -> Option<(i64, i64)> {
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe { (GetCursorPos(&mut pt) != 0).then_some((pt.x as i64, pt.y as i64)) }
}

fn input_tick() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe { (GetLastInputInfo(&mut info) != 0).then_some(info.dwTime) }
}

/// The state sampled before a read-only desktop call, finished after it.
pub struct Evidence {
    foreground_before: i64,
    cursor_before: Option<(i64, i64)>,
    input_before: Option<u32>,
}

impl Evidence {
    /// `foreground_before` and `cursor_before` are supplied by the caller so
    /// each call site keeps its own test seam; the input tick has no seam.
    pub fn begin(foreground_before: i64, cursor_before: Option<(i64, i64)>) -> Self {
        Self {
            foreground_before,
            cursor_before,
            input_before: input_tick(),
        }
    }

    /// Sample the same way `begin` did, for callers that need a fresh reading
    /// (the desktop capture names the cursor at return).
    pub fn cursor_now() -> Option<(i64, i64)> {
        cursor()
    }

    pub fn finish(
        self,
        foreground_after: i64,
        cursor_after: Option<(i64, i64)>,
        target: i64,
        payload: &mut Map<String, Value>,
    ) {
        let input_after = input_tick();
        let input_during = matches!((self.input_before, input_after), (Some(a), Some(b)) if a != b);

        let readable = self.cursor_before.is_some() && cursor_after.is_some();
        let moved = readable && self.cursor_before != cursor_after;

        let fg_moved = foreground_after != self.foreground_before;
        let after_pid = if foreground_after != 0 {
            crate::pid_of(foreground_after as windows_sys::Win32::Foundation::HWND)
        } else {
            0
        };
        let after_owned = after_pid == std::process::id();
        let target_was_foreground = target != 0 && self.foreground_before == target;
        let target_is_foreground = target != 0 && foreground_after == target;
        let target_activated = target_is_foreground && !target_was_foreground;
        let stolen = fg_moved && (after_owned || target_activated);
        let fg_external = fg_moved && !stolen;

        payload.insert("cursorUnchanged".to_string(), Value::from(!moved));
        payload.insert("foregroundUnchanged".to_string(), Value::from(!fg_moved));

        let audit = banned_imports();
        let mut ns = Map::new();
        ns.insert("audited".to_string(), Value::from(audit.ok));
        if let Some(e) = &audit.error {
            ns.insert("auditError".to_string(), Value::from(e.clone()));
        }
        ns.insert(
            "bannedImports".to_string(),
            Value::from(audit.banned.clone()),
        );

        let mut pointer = Map::new();
        pointer.insert("readable".to_string(), Value::from(readable));
        pointer.insert("before".to_string(), pair(self.cursor_before));
        pointer.insert("after".to_string(), pair(cursor_after));
        pointer.insert("inputTickBefore".to_string(), opt_u32(self.input_before));
        pointer.insert("inputTickAfter".to_string(), opt_u32(input_after));
        pointer.insert("inputDuringCall".to_string(), Value::from(input_during));
        pointer.insert("moved".to_string(), Value::from(moved));
        pointer.insert(
            "externalInput".to_string(),
            Value::from(moved && input_during),
        );
        ns.insert("pointer".to_string(), Value::Object(pointer));

        let mut foreground = Map::new();
        foreground.insert("before".to_string(), Value::from(self.foreground_before));
        foreground.insert("after".to_string(), Value::from(foreground_after));
        foreground.insert("afterPid".to_string(), Value::from(after_pid as i64));
        foreground.insert(
            "afterExe".to_string(),
            if after_pid == 0 {
                Value::Null
            } else {
                Value::from(crate::exe_of_pid(after_pid))
            },
        );
        foreground.insert("afterOwnedByThisProcess".to_string(), Value::from(after_owned));
        foreground.insert("target".to_string(), Value::from(target));
        foreground.insert(
            "targetWasForeground".to_string(),
            Value::from(target_was_foreground),
        );
        foreground.insert(
            "targetIsForeground".to_string(),
            Value::from(target_is_foreground),
        );
        foreground.insert("targetActivated".to_string(), Value::from(target_activated));
        foreground.insert("moved".to_string(), Value::from(fg_moved));
        foreground.insert("externalChange".to_string(), Value::from(fg_external));
        foreground.insert("stolen".to_string(), Value::from(stolen));
        ns.insert("foreground".to_string(), Value::Object(foreground));

        ns.insert("stolen".to_string(), Value::from(stolen));
        payload.insert("noSteal".to_string(), Value::Object(ns));
    }
}

fn pair(value: Option<(i64, i64)>) -> Value {
    match value {
        Some((x, y)) => Value::from(vec![Value::from(x), Value::from(y)]),
        None => Value::Null,
    }
}

fn opt_u32(value: Option<u32>) -> Value {
    match value {
        Some(v) => Value::from(v),
        None => Value::Null,
    }
}


fn u16le(bytes: &[u8], at: usize) -> Result<u16, String> {
    let chunk = bytes
        .get(at..at + 2)
        .ok_or_else(|| format!("u16 read past the end at {at:#x}"))?;
    Ok(u16::from_le_bytes([chunk[0], chunk[1]]))
}

fn u32le(bytes: &[u8], at: usize) -> Result<u32, String> {
    let chunk = bytes
        .get(at..at + 4)
        .ok_or_else(|| format!("u32 read past the end at {at:#x}"))?;
    Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn u64le(bytes: &[u8], at: usize) -> Result<u64, String> {
    let chunk = bytes
        .get(at..at + 8)
        .ok_or_else(|| format!("u64 read past the end at {at:#x}"))?;
    Ok(u64::from_le_bytes([
        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
    ]))
}

/// Every function name in a PE file's import table. Names only: the imported
/// module (USER32.dll, …) is not part of the answer.
fn pe_import_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return Err("no MZ header".to_string());
    }
    let pe = u32le(bytes, 0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("no PE signature".to_string());
    }
    let coff = pe + 4;
    let section_count = u16le(bytes, coff + 2)? as usize;
    let optional_size = u16le(bytes, coff + 16)? as usize;
    let optional = coff + 20;
    let magic = u16le(bytes, optional)?;
    let directories = match magic {
        0x10b => optional + 96,
        0x20b => optional + 112,
        other => return Err(format!("unknown optional-header magic {other:#x}")),
    };
    let import_rva = u32le(bytes, directories + 8)? as usize;
    if import_rva == 0 {
        return Ok(Vec::new());
    }

    let section_table = optional + optional_size;
    let mut sections: Vec<(usize, usize, usize)> = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let at = section_table + index * 40;
        let virtual_size = u32le(bytes, at + 8)? as usize;
        let virtual_address = u32le(bytes, at + 12)? as usize;
        let raw_size = u32le(bytes, at + 16)? as usize;
        let raw_pointer = u32le(bytes, at + 20)? as usize;
        sections.push((virtual_address, virtual_size.max(raw_size), raw_pointer));
    }
    let map = |rva: usize| -> Result<usize, String> {
        for (address, size, raw) in &sections {
            if rva >= *address && rva < address + size {
                return Ok(raw + (rva - address));
            }
        }
        if rva < bytes.len() {
            Ok(rva)
        } else {
            Err(format!("rva {rva:#x} is outside the image"))
        }
    };

    let thunk_size = if magic == 0x20b { 8 } else { 4 };
    let mut names = Vec::new();
    let mut descriptor = map(import_rva)?;
    for _ in 0..4096 {
        let original_thunk = u32le(bytes, descriptor)? as usize;
        let name_rva = u32le(bytes, descriptor + 12)? as usize;
        let first_thunk = u32le(bytes, descriptor + 16)? as usize;
        if original_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }
        let thunks = if original_thunk != 0 {
            original_thunk
        } else {
            first_thunk
        };
        let mut thunk = map(thunks)?;
        for _ in 0..65536 {
            let value = if thunk_size == 8 {
                u64le(bytes, thunk)?
            } else {
                u32le(bytes, thunk)? as u64
            };
            if value == 0 {
                break;
            }
            let by_ordinal = value >> (thunk_size * 8 - 1);
            if by_ordinal == 0 {
                let name_at = map(value as usize)?;
                let _hint = u16le(bytes, name_at)?;
                let start = name_at + 2;
                let mut end = start;
                while end < bytes.len() && bytes[end] != 0 {
                    end += 1;
                }
                if end == bytes.len() {
                    return Err("unterminated import name".to_string());
                }
                if let Ok(name) = std::str::from_utf8(&bytes[start..end]) {
                    names.push(name.to_string());
                }
            }
            thunk += thunk_size;
        }
        descriptor += 20;
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The running test binary imports user32 through the same library as the
    /// product. The banned writers must not be in its import table.
    #[test]
    fn the_running_binary_links_no_pointer_or_foreground_writer() {
        let audit = banned_imports();
        assert!(
            audit.ok,
            "the import audit must succeed, not pass silently: {:?}",
            audit.error
        );
        assert!(
            audit.banned.is_empty(),
            "banned imports present: {:?}",
            audit.banned
        );
    }

    #[test]
    fn a_non_pe_file_is_a_typed_error_not_an_empty_audit() {
        let error = pe_import_names(b"not a portable executable").expect_err("must refuse");
        assert!(error.contains("MZ"), "{error}");
    }

    /// `GetLastInputInfo` is the attribution source. If it stops working on a
    /// real session the gate should fail closed, so the sample must be present
    /// here rather than silently null on the owner's desktop.
    #[test]
    fn the_input_tick_is_readable_on_a_live_session() {
        let tick = input_tick();
        assert!(tick.is_some(), "GetLastInputInfo failed");
    }
}
