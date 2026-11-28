//! `desktop_record`: bounded, background-safe MJPEG AVI recording of ONE
//! targeted window.
//!
//! Every frame rides the screenshot window path (`capture_window` +
//! `finalize` with `fmt:"jpeg"`): no activation, no pointer write, no
//! clipboard access. The container is the shared `mjpeg_avi` writer used by
//! `browser_trace`, so both tools emit the same RIFF/`hdrl`/`movi`/`idx1`
//! bytes.
//!
//! Slots are paced from a monotonic clock. A capture that runs past its slot
//! makes later slots late; each late slot is dropped and, once a frame
//! exists, the previous JPEG is written again so the constant-rate AVI keeps
//! the requested duration. `droppedFrames` counts dropped slots,
//! `capturedFrames` counts fresh captures and `frames` counts every chunk
//! written, repeats included. If the very first slot overruns there is no
//! frame to repeat: it is counted without padding. A partial recording is
//! never silent: the AVI holds the frames that landed and the typed result
//! names that count.

use crate::shot::capture_window;
use mjpeg_avi::jpeg_dimensions;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

const TOOL: &str = "record";
const DEFAULT_DURATION_MS: i64 = 3000;
const MIN_DURATION_MS: i64 = 250;
const MAX_DURATION_MS: i64 = 15_000;
const DEFAULT_FPS: i64 = 8;
const MIN_FPS: i64 = 1;
const MAX_FPS: i64 = 15;
const DEFAULT_MAX_PX: i64 = 960;
const MIN_MAX_PX: i64 = 160;
const MAX_MAX_PX: i64 = 1280;
const DEFAULT_QUALITY: i64 = 75;
/// A timer wake a few ms late still belongs to the slot it was scheduled for;
/// later arrivals are genuinely missed slots.
const LATE_TOLERANCE: Duration = Duration::from_millis(2);

/// Parsed and bounded arguments. Out-of-range numerics are clamped and
/// reported in `warnings`; `out` failures are typed `invalid_args` before any
/// window is touched.
#[derive(Debug)]
pub(crate) struct Args {
    pub target: Value,
    pub duration_ms: u64,
    pub fps: u32,
    pub max_px: i64,
    pub quality: i64,
    pub out: PathBuf,
    pub warnings: Vec<Value>,
}

fn invalid(field: &str, reason: &str, message: impl Into<String>) -> Value {
    json!({
        "ok": false,
        "code": "invalid_args",
        "reason": reason,
        "field": field,
        "tool": TOOL,
        "error": message.into(),
    })
}

fn clamp_arg(
    args: &Map<String, Value>,
    name: &str,
    default: i64,
    min: i64,
    max: i64,
    warnings: &mut Vec<Value>,
) -> Result<i64, Value> {
    let raw = match args.get(name) {
        None | Some(Value::Null) => default,
        Some(v) => crate::coerce_int(v).map_err(|(msg, typ)| {
            invalid(
                name,
                "bad_type",
                format!("{TOOL}: '{name}' must be an integer ({msg}, {typ})"),
            )
        })?,
    };
    let applied = raw.clamp(min, max);
    if applied != raw {
        warnings.push(json!({
            "code": format!("{name}_clamped"),
            "field": name,
            "requested": raw,
            "applied": applied,
            "reason": if applied < raw { "floor" } else { "cap" },
        }));
    }
    Ok(applied)
}

/// Validate everything before a capture starts. An `out` that exists, is
/// relative, has the wrong extension or has no parent is refused here.
pub(crate) fn parse(args: &Map<String, Value>) -> Result<Args, Value> {
    let mut extra = Vec::new();
    crate::check_no_extra(
        args,
        &mut extra,
        &["target", "duration_ms", "fps", "max_px", "quality", "out"],
    );
    if !extra.is_empty() {
        let names: Vec<String> = extra.iter().map(|e| e.loc.clone()).collect();
        return Err(invalid(
            "args",
            "unknown",
            format!("{TOOL}: unknown arguments: {}", names.join(", ")),
        ));
    }
    let target = match args.get("target") {
        None | Some(Value::Null) => {
            return Err(invalid(
                "target",
                "missing",
                format!("{TOOL}: 'target' is required"),
            ))
        }
        Some(v @ (Value::Object(_) | Value::String(_))) => v.clone(),
        Some(other) => {
            return Err(invalid(
                "target",
                "bad_type",
                format!("{TOOL}: 'target' must be an object or selector string, got {other}"),
            ))
        }
    };
    let raw_out = match args.get("out") {
        Some(v) => crate::coerce_str(v).map_err(|(msg, typ)| {
            invalid(
                "out",
                "bad_type",
                format!("{TOOL}: 'out' must be a string ({msg}, {typ})"),
            )
        })?,
        None => {
            return Err(invalid(
                "out",
                "missing",
                format!("{TOOL}: 'out' is required"),
            ))
        }
    };
    if raw_out.is_empty() {
        return Err(invalid(
            "out",
            "empty",
            format!("{TOOL}: 'out' must not be empty"),
        ));
    }
    let out = PathBuf::from(&raw_out);
    if !out.is_absolute() {
        return Err(invalid(
            "out",
            "not_absolute",
            format!("{TOOL}: 'out' must be an absolute path, got '{raw_out}'"),
        ));
    }
    if !out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("avi"))
    {
        return Err(invalid(
            "out",
            "extension",
            format!("{TOOL}: 'out' must end in .avi, got '{raw_out}'"),
        ));
    }
    let parent = out.parent().unwrap_or_else(|| Path::new("/"));
    if !parent.is_dir() {
        return Err(invalid(
            "out",
            "parent_missing",
            format!("{TOOL}: parent of 'out' must exist: {}", parent.display()),
        ));
    }
    if out.exists() {
        return Err(invalid(
            "out",
            "exists",
            format!(
                "{TOOL}: 'out' already exists; refusing to overwrite {}",
                out.display()
            ),
        ));
    }
    let mut warnings = Vec::new();
    let duration_ms = clamp_arg(
        args,
        "duration_ms",
        DEFAULT_DURATION_MS,
        MIN_DURATION_MS,
        MAX_DURATION_MS,
        &mut warnings,
    )? as u64;
    let fps = clamp_arg(args, "fps", DEFAULT_FPS, MIN_FPS, MAX_FPS, &mut warnings)? as u32;
    let max_px = clamp_arg(
        args,
        "max_px",
        DEFAULT_MAX_PX,
        MIN_MAX_PX,
        MAX_MAX_PX,
        &mut warnings,
    )?;
    let quality = clamp_arg(args, "quality", DEFAULT_QUALITY, 1, 100, &mut warnings)?;
    Ok(Args {
        target,
        duration_ms,
        fps,
        max_px,
        quality,
        out,
        warnings,
    })
}

/// The same three process-global invariants the screenshot tool measures:
/// foreground, pointer position and clipboard sequence number, sampled on
/// entry and again on the way out (success or typed failure).
struct Witness {
    fg: i64,
    cursor: Option<(i64, i64)>,
    clipboard: u32,
}

impl Witness {
    fn begin() -> Self {
        let mut pt = POINT { x: 0, y: 0 };
        let cursor = unsafe { (GetCursorPos(&mut pt) != 0).then_some((pt.x as i64, pt.y as i64)) };
        Self {
            fg: unsafe { GetForegroundWindow() as i64 },
            cursor,
            clipboard: unsafe { GetClipboardSequenceNumber() },
        }
    }

    fn finish(self, target: i64, payload: &mut Map<String, Value>) {
        let mut evidence = Map::new();
        crate::no_steal::Evidence::begin(self.fg, self.cursor).finish(
            unsafe { GetForegroundWindow() as i64 },
            crate::no_steal::Evidence::cursor_now(),
            target,
            &mut evidence,
        );
        let fg_unchanged = evidence
            .remove("foregroundUnchanged")
            .unwrap_or(Value::Bool(true));
        let cursor_unchanged = evidence
            .remove("cursorUnchanged")
            .unwrap_or(Value::Bool(true));
        let after = unsafe { GetClipboardSequenceNumber() };
        let clipboard_unchanged = self.clipboard == after;
        let mut ns = evidence
            .remove("noSteal")
            .unwrap_or_else(|| Value::Object(Map::new()));
        if let Some(ns) = ns.as_object_mut() {
            let mut clip = Map::new();
            clip.insert("before".to_string(), Value::from(self.clipboard as i64));
            clip.insert("after".to_string(), Value::from(after as i64));
            clip.insert("unchanged".to_string(), Value::from(clipboard_unchanged));
            ns.insert("clipboard".to_string(), Value::Object(clip));
        }
        payload.insert("foregroundUnchanged".to_string(), fg_unchanged);
        payload.insert("cursorUnchanged".to_string(), cursor_unchanged);
        payload.insert(
            "clipboardUnchanged".to_string(),
            Value::from(clipboard_unchanged),
        );
        payload.insert("noSteal".to_string(), ns);
    }
}

fn envelope(payload: &Value) -> String {
    crate::wrapped_text(&crate::py_dumps(payload))
}

fn error_payload(mut payload: Map<String, Value>, code: &str, error: impl Into<String>) -> Value {
    payload.insert("ok".to_string(), Value::Bool(false));
    payload.insert("code".to_string(), Value::from(code));
    payload.insert("error".to_string(), Value::from(error.into()));
    Value::Object(payload)
}

fn code_of(text: &str) -> &str {
    let t = text.strip_prefix("error: ").unwrap_or(text);
    let code = t.split(':').next().unwrap_or("").trim();
    if code.is_empty() {
        "capture_unavailable"
    } else {
        code
    }
}

/// Clock seam for the pacing logic. The production clock sleeps for real;
/// the test clock advances virtual time, so an overrunning capture can be
/// replayed exactly without waiting on wall time.
trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, d: Duration);
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// What one scheduled slot did. A captured slot carries its JPEG so the
/// pacing loop owns both fresh frames and repeats in write order.
enum Capture {
    Captured(Vec<u8>),
    Gone(String),
    Unavailable(String),
}

/// How the bounded loop stopped.
#[derive(Debug, PartialEq, Eq)]
enum End {
    Duration,
    Gone(String),
    Unavailable(String),
}

struct Pacing {
    dropped: u64,
    captured: u64,
    first_ts_ms: u64,
    last_ts_ms: u64,
    end: End,
}

/// Pace one slot per `interval` until `duration` elapses. A slot already in
/// the past when it comes up is dropped (that is the overrun case); a slot
/// only a couple of ms late is still captured. Once a frame exists a dropped
/// slot writes the previous JPEG again, so the constant-rate file keeps its
/// real-time length; before the first frame there is nothing honest to
/// repeat, so the slot is only counted. `frames` receives fresh captures and
/// repeats in write order; `capture` gets the frame's monotonic timestamp in
/// ms relative to the recording start.
fn run_pacing<C: Clock, F>(
    clock: &C,
    duration: Duration,
    interval: Duration,
    frames: &mut Vec<Vec<u8>>,
    mut capture: F,
) -> Pacing
where
    F: FnMut(u64) -> Capture,
{
    let start = clock.now();
    let deadline = start + duration;
    let mut slot = 0u32;
    let mut dropped = 0u64;
    let mut captured = 0u64;
    let mut first_ts_ms = 0u64;
    let mut last_ts_ms = 0u64;
    loop {
        // Find the next due slot. Every slot whose target time has passed by
        // more than the timer tolerance is a missed slot, not a late capture.
        loop {
            let now = clock.now();
            if now >= deadline {
                return Pacing {
                    dropped,
                    captured,
                    first_ts_ms,
                    last_ts_ms,
                    end: End::Duration,
                };
            }
            let target = start + interval * slot;
            if target >= deadline {
                return Pacing {
                    dropped,
                    captured,
                    first_ts_ms,
                    last_ts_ms,
                    end: End::Duration,
                };
            }
            let late = now.saturating_duration_since(target);
            if late < LATE_TOLERANCE {
                if now < target {
                    clock.sleep(target - now);
                    if clock.now() >= deadline {
                        return Pacing {
                            dropped,
                            captured,
                            first_ts_ms,
                            last_ts_ms,
                            end: End::Duration,
                        };
                    }
                }
                break;
            }
            dropped += 1;
            if let Some(previous) = frames.last().cloned() {
                frames.push(previous);
            }
            slot += 1;
        }
        let now = clock.now();
        let ts = now.saturating_duration_since(start).as_millis() as u64;
        match capture(ts) {
            Capture::Captured(jpeg) => {
                if captured == 0 {
                    first_ts_ms = ts;
                }
                last_ts_ms = ts;
                captured += 1;
                frames.push(jpeg);
                slot += 1;
            }
            Capture::Gone(e) => {
                return Pacing {
                    dropped,
                    captured,
                    first_ts_ms,
                    last_ts_ms,
                    end: End::Gone(e),
                };
            }
            Capture::Unavailable(e) => {
                return Pacing {
                    dropped,
                    captured,
                    first_ts_ms,
                    last_ts_ms,
                    end: End::Unavailable(e),
                };
            }
        }
    }
}

/// Result fields shared by a complete recording and a partial one. A partial
/// file is reported with the same facts plus `ok:false`, `code` and `error`.
#[allow(clippy::too_many_arguments)]
fn record_fields(
    path: &Path,
    bytes: usize,
    frames: usize,
    cfg: &Args,
    dims: (u32, u32),
    pacing: &Pacing,
    warnings: &[Value],
) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ok".to_string(), Value::Bool(true));
    m.insert(
        "path".to_string(),
        Value::from(path.to_string_lossy().to_string()),
    );
    m.insert("bytes".to_string(), Value::from(bytes as i64));
    m.insert("frames".to_string(), Value::from(frames as i64));
    m.insert(
        "capturedFrames".to_string(),
        Value::from(pacing.captured as i64),
    );
    m.insert(
        "durationMs".to_string(),
        Value::from(cfg.duration_ms as i64),
    );
    m.insert("fps".to_string(), Value::from(cfg.fps as i64));
    m.insert("width".to_string(), Value::from(dims.0 as i64));
    m.insert("height".to_string(), Value::from(dims.1 as i64));
    m.insert("container".to_string(), Value::from("mjpeg-avi"));
    m.insert(
        "firstFrameTsMs".to_string(),
        Value::from(pacing.first_ts_ms as i64),
    );
    m.insert(
        "lastFrameTsMs".to_string(),
        Value::from(pacing.last_ts_ms as i64),
    );
    m.insert(
        "droppedFrames".to_string(),
        Value::from(pacing.dropped as i64),
    );
    if !warnings.is_empty() {
        m.insert("warnings".to_string(), Value::from(warnings.to_vec()));
    }
    m
}

/// Reply builders. Each returns the exact structured payload that becomes
/// `structuredContent` - the value a strict client validates against the
/// closed TOOL_RECORD.outputSchema. Keeping assembly here, rather than inline
/// in the dispatch loop, lets the schema-containment test build every real
/// path with the same code production uses.

/// A complete recording: base fields and measured invariants, no error
/// fields at all.
fn success_reply(mut payload: Map<String, Value>, witness: Witness, target: i64) -> Value {
    witness.finish(target, &mut payload);
    Value::Object(payload)
}

/// A typed failure that still carries the base fields and `framesWritten`:
/// a partial recording with frames already on disk, or a failed AVI write.
fn typed_reply(
    mut payload: Map<String, Value>,
    witness: Witness,
    target: i64,
    frames_written: usize,
    code: &str,
    error: impl Into<String>,
) -> Value {
    payload.insert(
        "framesWritten".to_string(),
        Value::from(frames_written as i64),
    );
    witness.finish(target, &mut payload);
    error_payload(payload, code, error)
}

/// No frame was captured, so no AVI exists. Drop `path` instead of naming a
/// file that was never written, and report `framesWritten: 0`.
fn no_frame_reply(
    mut payload: Map<String, Value>,
    witness: Witness,
    target: i64,
    error: impl Into<String>,
) -> Value {
    payload.remove("path");
    payload.insert("framesWritten".to_string(), Value::from(0));
    witness.finish(target, &mut payload);
    error_payload(payload, "capture_unavailable", error)
}

/// A target selector or lock refusal: the typed target payload plus the
/// measured invariants. `candidates` rides along when the target was
/// ambiguous.
fn target_reply(
    mut payload: Map<String, Value>,
    witness: Witness,
    error: &crate::target::TargetError,
) -> Value {
    payload.extend(error.to_map());
    witness.finish(0, &mut payload);
    Value::Object(payload)
}

/// Dispatch from `serve::tools_call`. The response is the same MCP envelope
/// the other desktop tools emit; `structuredContent` carries the typed result.
pub(crate) fn dispatch_record(args: &Map<String, Value>) -> String {
    let cfg = match parse(args) {
        Ok(c) => c,
        Err(e) => return envelope(&e),
    };
    let witness = Witness::begin();
    let spec = match crate::target::TargetSpec::parse(&cfg.target) {
        Ok(s) => s,
        Err(e) => return envelope(&target_reply(Map::new(), witness, &e)),
    };
    let lock = match crate::target::lock(&spec) {
        Ok(l) => l,
        Err(e) => return envelope(&target_reply(Map::new(), witness, &e)),
    };
    let target_hwnd = lock.hwnd;
    let interval = Duration::from_nanos(1_000_000_000 / u64::from(cfg.fps));
    let duration = Duration::from_millis(cfg.duration_ms);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut dims = (0u32, 0u32);
    let mut first_frame = true;

    let pacing = run_pacing(&SystemClock, duration, interval, &mut frames, |ts| {
        if let Err(e) = lock.revalidate() {
            return Capture::Gone(e.text());
        }
        let cap = match unsafe { capture_window(lock.hwnd as HWND, false) } {
            Ok(c) => c,
            Err(e) => return Capture::Unavailable(e),
        };
        let (jpeg, iw, ih, _, fmt, _) =
            match crate::cap::finalize(cap.img, Some(cfg.max_px), "jpeg", Some(cfg.quality), false)
            {
                Ok(v) => v,
                Err(e) => return Capture::Unavailable(e),
            };
        if fmt != "jpeg" || jpeg.is_empty() {
            return Capture::Unavailable(format!(
                "error: capture_unavailable: frame {ts} encoded as {fmt} with {} bytes",
                jpeg.len()
            ));
        }
        if first_frame {
            dims = jpeg_dimensions(&jpeg).unwrap_or((iw as u32, ih as u32));
            first_frame = false;
        }
        Capture::Captured(jpeg)
    });

    if frames.is_empty() {
        let why = match &pacing.end {
            End::Duration => format!(
                "{TOOL}: no window frames were captured in {} ms",
                cfg.duration_ms
            ),
            End::Gone(e) => format!("{TOOL}: no frames captured: {e}"),
            End::Unavailable(e) => format!("{TOOL}: no frames captured: {e}"),
        };
        let fields = record_fields(&cfg.out, 0, 0, &cfg, dims, &pacing, &cfg.warnings);
        return envelope(&no_frame_reply(fields, witness, target_hwnd, why));
    }

    let avi = mjpeg_avi::avi_mjpeg(&frames, cfg.fps);
    if let Err(e) = std::fs::write(&cfg.out, &avi) {
        let fields = record_fields(
            &cfg.out,
            0,
            frames.len(),
            &cfg,
            dims,
            &pacing,
            &cfg.warnings,
        );
        return envelope(&typed_reply(
            fields,
            witness,
            target_hwnd,
            frames.len(),
            "invalid_args",
            format!("{TOOL}: cannot write '{}': {e}", cfg.out.display()),
        ));
    }

    let fields = record_fields(
        &cfg.out,
        avi.len(),
        frames.len(),
        &cfg,
        dims,
        &pacing,
        &cfg.warnings,
    );
    let partial = match &pacing.end {
        End::Duration => None,
        End::Gone(e) => Some(("target_gone", e.clone())),
        End::Unavailable(e) => Some((code_of(e), e.clone())),
    };
    if let Some((code, why)) = partial {
        return envelope(&typed_reply(
            fields,
            witness,
            target_hwnd,
            frames.len(),
            code,
            format!(
                "{why}; the AVI keeps the {} frames already written at {}",
                frames.len(),
                cfg.out.display()
            ),
        ));
    }
    envelope(&success_reply(fields, witness, target_hwnd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct TestClock {
        now: RefCell<Instant>,
        slept: RefCell<Vec<Duration>>,
    }

    impl TestClock {
        fn new() -> Self {
            Self {
                now: RefCell::new(Instant::now()),
                slept: RefCell::new(Vec::new()),
            }
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            *self.now.borrow()
        }
        fn sleep(&self, d: Duration) {
            self.slept.borrow_mut().push(d);
            *self.now.borrow_mut() += d;
        }
    }

    /// A clock whose first read is the recording start and whose later reads
    /// are one slot late. It replays an overrun that happens before the first
    /// frame exists, without waiting on wall time.
    struct LateFirstClock {
        now: RefCell<Instant>,
        reads: Cell<u32>,
        drift: Duration,
    }

    impl LateFirstClock {
        fn new(drift: Duration) -> Self {
            Self {
                now: RefCell::new(Instant::now()),
                reads: Cell::new(0),
                drift,
            }
        }
    }

    impl Clock for LateFirstClock {
        fn now(&self) -> Instant {
            let n = self.reads.get();
            self.reads.set(n + 1);
            let base = *self.now.borrow();
            if n == 0 {
                base
            } else {
                base + self.drift
            }
        }
        fn sleep(&self, d: Duration) {
            *self.now.borrow_mut() += d;
        }
    }

    fn args(out: &str) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("target".to_string(), json!({"hwnd": 1}));
        m.insert("out".to_string(), Value::from(out));
        m
    }

    fn scratch(tag: &str) -> PathBuf {
        let unique = format!(
            "hand-record-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn pacing_hits_each_slot_when_captures_are_fast() {
        let clock = TestClock::new();
        let mut stamps = Vec::new();
        let mut frames = Vec::new();
        let p = run_pacing(
            &clock,
            Duration::from_millis(800),
            Duration::from_millis(200),
            &mut frames,
            |ts| {
                stamps.push(ts);
                Capture::Captured(vec![0xFF, 0xD8, stamps.len() as u8])
            },
        );
        assert_eq!(stamps, vec![0, 200, 400, 600], "four slots in 800 ms");
        assert_eq!(frames.len(), 4, "one chunk per slot, no repeats");
        assert_eq!(p.captured, 4);
        assert_eq!(p.dropped, 0);
        assert_eq!(p.first_ts_ms, 0);
        assert_eq!(p.last_ts_ms, 600);
        assert_eq!(p.end, End::Duration);
    }

    #[test]
    fn pacing_skips_overrun_slots_and_counts_dropped_frames() {
        let clock = TestClock::new();
        // Each capture takes 600 ms with a 250 ms interval: after the first
        // frame the 250 and 500 ms slots have already passed. They must be
        // counted as drops, not replaced by a "10 fps" lie.
        let mut stamps = Vec::new();
        let mut frames = Vec::new();
        let p = run_pacing(
            &clock,
            Duration::from_millis(1000),
            Duration::from_millis(250),
            &mut frames,
            |ts| {
                stamps.push(ts);
                clock.sleep(Duration::from_millis(600));
                Capture::Captured(vec![0xFF, 0xD8, stamps.len() as u8])
            },
        );
        assert_eq!(stamps, vec![0, 750], "slots 1 and 2 were overrun");
        assert_eq!(p.captured, 2, "only two fresh captures");
        assert_eq!(frames.len(), 4, "two overrun slots were padded");
        assert_eq!(frames[1], frames[0], "first missed slot repeats frame 1");
        assert_eq!(frames[2], frames[0], "second missed slot repeats frame 1");
        assert_eq!(frames[3][2], 2, "the last slot is a fresh capture");
        assert_eq!(p.dropped, 2, "overrun slots are counted");
        assert_eq!(p.end, End::Duration, "the 1000 ms bound still wins");
        assert!(
            !clock.slept.borrow().is_empty(),
            "the clock seam really scheduled the second slot"
        );
    }

    #[test]
    fn pacing_repeats_the_previous_frame_for_an_overrun_slot() {
        let clock = TestClock::new();
        let mut stamps = Vec::new();
        let mut frames = Vec::new();
        let p = run_pacing(
            &clock,
            Duration::from_millis(450),
            Duration::from_millis(150),
            &mut frames,
            |ts| {
                stamps.push(ts);
                if stamps.len() == 2 {
                    // This capture overruns its 150 ms slot by 10 ms: the next
                    // slot's target (300 ms) is already in the past.
                    clock.sleep(Duration::from_millis(160));
                }
                Capture::Captured(vec![0xFF, 0xD8, stamps.len() as u8])
            },
        );
        assert_eq!(stamps, vec![0, 150], "the third slot was overrun");
        assert_eq!(frames.len(), 3, "two fresh frames plus one repeat");
        assert_eq!(
            frames[2], frames[1],
            "the repeated slot holds the previous JPEG"
        );
        assert_eq!(p.captured, 2);
        assert_eq!(p.dropped, 1);
        assert_eq!(p.end, End::Duration);
    }

    #[test]
    fn pacing_drops_a_late_first_slot_without_padding_any_frame() {
        let clock = LateFirstClock::new(Duration::from_millis(100));
        let mut frames = Vec::new();
        let p = run_pacing(
            &clock,
            Duration::from_millis(300),
            Duration::from_millis(100),
            &mut frames,
            |ts| Capture::Captured(vec![0xFF, 0xD8, ts as u8]),
        );
        assert_eq!(
            frames.len(),
            2,
            "no frame existed for the first dropped slot"
        );
        assert_eq!(
            frames[0][2], 100,
            "the first written frame is a fresh capture"
        );
        assert_eq!(frames[1][2], 200, "no phantom frame was written before it");
        assert_eq!(p.captured, 2);
        assert_eq!(p.dropped, 1);
    }

    #[test]
    fn pacing_reports_a_vanished_target_with_the_frames_already_written() {
        let clock = TestClock::new();
        let mut calls = 0;
        let mut frames = Vec::new();
        let p = run_pacing(
            &clock,
            Duration::from_millis(1000),
            Duration::from_millis(200),
            &mut frames,
            |_| {
                calls += 1;
                if calls == 3 {
                    Capture::Gone("target_gone: window closed".to_string())
                } else {
                    Capture::Captured(vec![0xFF, 0xD8, calls as u8])
                }
            },
        );
        assert_eq!(calls, 3, "two captures plus the vanished-slot attempt");
        assert_eq!(frames.len(), 2, "only the two fresh frames were written");
        assert_eq!(p.captured, 2);
        assert_eq!(p.dropped, 0);
        assert_eq!(p.end, End::Gone("target_gone: window closed".to_string()));
    }

    #[test]
    fn argument_validation_refuses_bad_out_before_capture() {
        let dir = scratch("args");
        let existing = dir.join("exists.avi");
        std::fs::write(&existing, b"caller-owned").expect("seed existing file");
        let bad = [
            (existing.to_string_lossy().to_string(), "exists"),
            ("relative.avi".to_string(), "not_absolute"),
            (
                dir.join("wrong.mp4").to_string_lossy().to_string(),
                "extension",
            ),
            (
                dir.join("missing")
                    .join("x.avi")
                    .to_string_lossy()
                    .to_string(),
                "parent_missing",
            ),
        ];
        for (out, reason) in bad {
            let err = parse(&args(&out)).expect_err("bad out must be refused");
            assert_eq!(err["code"], "invalid_args", "{err}");
            assert_eq!(err["reason"], reason, "{err}");
            assert_eq!(err["field"], "out", "{err}");
        }
        assert_eq!(
            std::fs::read(&existing).expect("seed survives"),
            b"caller-owned"
        );
        assert!(!dir.join("wrong.mp4").exists(), "no file was written");
        std::fs::remove_dir_all(&dir).expect("scratch cleanup");
    }

    #[test]
    fn argument_validation_clamps_duration_fps_max_px_and_quality() {
        let dir = scratch("clamp");
        let out = dir.join("clamp.avi");
        let mut a = args(out.to_string_lossy().as_ref());
        a.insert("duration_ms".to_string(), json!(99_999));
        a.insert("fps".to_string(), json!(99));
        a.insert("max_px".to_string(), json!(1));
        a.insert("quality".to_string(), json!(500));
        let cfg = parse(&a).expect("clamped args are valid");
        assert_eq!(cfg.duration_ms, MAX_DURATION_MS as u64);
        assert_eq!(cfg.fps, MAX_FPS as u32);
        assert_eq!(cfg.max_px, MIN_MAX_PX);
        assert_eq!(cfg.quality, 100);
        assert_eq!(cfg.warnings.len(), 4);
        assert!(cfg.warnings.iter().all(|w| w["field"].is_string()));

        let defaults = parse(&args(out.to_string_lossy().as_ref())).expect("defaults");
        assert_eq!(defaults.duration_ms, DEFAULT_DURATION_MS as u64);
        assert_eq!(defaults.fps, DEFAULT_FPS as u32);
        assert_eq!(defaults.max_px, DEFAULT_MAX_PX);
        assert_eq!(defaults.quality, DEFAULT_QUALITY);
        assert!(defaults.warnings.is_empty());
        std::fs::remove_dir_all(&dir).expect("scratch cleanup");
    }

    #[test]
    fn record_schema_declares_caps_no_steal_and_partial_reporting() {
        let tool: Value =
            serde_json::from_str(crate::schemas::TOOL_RECORD).expect("TOOL_RECORD JSON");
        assert_eq!(tool["name"], "record");
        let input = &tool["inputSchema"];
        assert_eq!(input["additionalProperties"], false);
        let required = input["required"].as_array().expect("required");
        assert!(required.iter().any(|v| v == "target"));
        assert!(required.iter().any(|v| v == "out"));
        let desc = tool["description"].as_str().expect("description");
        // Caps, no-steal invariants and partial-file reporting stay load-bearing.
        for phrase in [
            "window",
            "250",
            "15000",
            "1..15",
            "160",
            "1280",
            "absolute",
            ".avi",
            "activating",
            "pointer",
            "clipboard",
            "quality",
            "droppedFrames",
            "frames already written",
            // Real overrun mechanism, not a hidden-fps claim.
            "previous JPEG is written again",
            "constant-rate",
            "capturedFrames",
            "capture_unavailable",
        ] {
            assert!(desc.contains(phrase), "description must name {phrase}");
        }
        // The retired claim said overruns were "never hidden" by the requested
        // fps; it must not creep back into the published contract.
        assert!(
            !desc.contains("never hidden"),
            "description must not claim overruns are never hidden"
        );
        let output = &tool["outputSchema"];
        assert_eq!(output["additionalProperties"], false);
        for field in [
            "ok",
            "path",
            "bytes",
            "frames",
            "capturedFrames",
            "durationMs",
            "fps",
            "width",
            "height",
            "container",
            "firstFrameTsMs",
            "lastFrameTsMs",
            "droppedFrames",
            "foregroundUnchanged",
            "cursorUnchanged",
            "clipboardUnchanged",
        ] {
            assert!(
                output["properties"].get(field).is_some(),
                "output schema must declare {field}"
            );
        }
        let required_out = output["required"]
            .as_array()
            .expect("outputSchema.required");
        assert_eq!(
            required_out.len(),
            1,
            "only ok is emitted on every reply path"
        );
        assert_eq!(required_out[0], json!("ok"));
        assert!(
            output["properties"].get("capturedFrames").is_some(),
            "capturedFrames is declared but not required: error replies omit it"
        );
    }

    #[test]
    fn reply_counts_written_fresh_and_dropped_frames_separately() {
        let dir = scratch("counts");
        let out = dir.join("counts.avi");
        let cfg = parse(&args(out.to_string_lossy().as_ref())).expect("valid args");
        let pacing = Pacing {
            dropped: 1,
            captured: 2,
            first_ts_ms: 0,
            last_ts_ms: 200,
            end: End::Duration,
        };
        let fields = record_fields(&out, 300, 3, &cfg, (320, 240), &pacing, &[]);
        assert_eq!(fields["frames"], 3, "frames counts every AVI chunk");
        assert_eq!(fields["capturedFrames"], 2, "fresh captures only");
        assert_eq!(fields["droppedFrames"], 1, "dropped slots only");
        std::fs::remove_dir_all(&dir).expect("scratch cleanup");
    }

    /// A strict client validates `structuredContent` against the closed
    /// outputSchema. Build each reply path with the very builders dispatch
    /// uses, then assert set containment both ways: every emitted key is
    /// declared, and every required key is emitted. No hand-maintained key
    /// list to drift when a builder starts emitting something new.
    #[test]
    fn every_reply_path_emits_only_schema_declared_keys() {
        let tool: Value =
            serde_json::from_str(crate::schemas::TOOL_RECORD).expect("TOOL_RECORD JSON");
        let output = &tool["outputSchema"];
        assert_eq!(
            output["additionalProperties"], false,
            "output schema stays closed"
        );
        let declared: std::collections::BTreeSet<String> = output["properties"]
            .as_object()
            .expect("outputSchema.properties")
            .keys()
            .cloned()
            .collect();
        let required: std::collections::BTreeSet<String> = output["required"]
            .as_array()
            .expect("outputSchema.required")
            .iter()
            .map(|v| v.as_str().expect("required names a string").to_string())
            .collect();

        let dir = scratch("schema-keys");
        let out = dir.join("record.avi");
        let cfg = parse(&args(out.to_string_lossy().as_ref())).expect("valid args");
        let pacing = Pacing {
            dropped: 1,
            captured: 2,
            first_ts_ms: 0,
            last_ts_ms: 200,
            end: End::Duration,
        };
        let fields = || record_fields(&out, 300, 3, &cfg, (320, 240), &pacing, &[]);
        let witness = || Witness::begin();
        let bad_target =
            crate::target::TargetSpec::parse(&json!(123)).expect_err("a numeric target is refused");

        let replies: Vec<(&str, Value)> = vec![
            ("success", success_reply(fields(), witness(), 0)),
            (
                "partial/target_gone",
                typed_reply(fields(), witness(), 0, 3, "target_gone", "window closed"),
            ),
            (
                "capture_unavailable/zero frames",
                no_frame_reply(fields(), witness(), 0, "no frames captured"),
            ),
            (
                "invalid_args/pre-capture",
                parse(&args("relative.avi")).expect_err("out must be absolute"),
            ),
            (
                "target refusal",
                target_reply(Map::new(), witness(), &bad_target),
            ),
            (
                "AVI write failure",
                typed_reply(fields(), witness(), 0, 3, "invalid_args", "cannot write"),
            ),
        ];

        let mut common: Option<std::collections::BTreeSet<String>> = None;
        for (name, reply) in &replies {
            let emitted: std::collections::BTreeSet<String> = reply
                .as_object()
                .unwrap_or_else(|| panic!("{name}: reply is not an object"))
                .keys()
                .cloned()
                .collect();
            let undeclared: Vec<&String> = emitted.difference(&declared).collect();
            assert!(
                undeclared.is_empty(),
                "{name} emits undeclared keys {undeclared:?}"
            );
            let missing: Vec<&String> = required.difference(&emitted).collect();
            assert!(missing.is_empty(), "{name} omits required keys {missing:?}");
            common = Some(match common {
                None => emitted,
                Some(acc) => acc.intersection(&emitted).cloned().collect(),
            });
        }
        assert_eq!(
            required,
            common.expect("at least one reply path"),
            "required must be exactly the keys every path emits"
        );
        // The no-frame path must not advertise a file that was never written.
        let no_frame = &replies[2].1;
        assert!(!no_frame.as_object().expect("object").contains_key("path"));
        assert_eq!(no_frame["framesWritten"], 0);
        std::fs::remove_dir_all(&dir).expect("scratch cleanup");
    }

    fn movi_frames(avi: &[u8]) -> usize {
        let pos = avi
            .windows(4)
            .position(|w| w == b"movi")
            .expect("movi list");
        let size = u32::from_le_bytes(avi[pos - 4..pos].try_into().unwrap()) as usize;
        let end = pos + size;
        let mut at = pos + 4;
        let mut frames = 0;
        while at + 8 <= end && &avi[at..at + 4] == b"00dc" {
            let len = u32::from_le_bytes(avi[at + 4..at + 8].try_into().unwrap()) as usize;
            at += 8 + len + len % 2;
            frames += 1;
        }
        frames
    }

    #[ignore = "needs an interactive desktop; run with cargo test -p desktop-engine -- --ignored live_record"]
    #[test]
    fn live_record_fixture_window_writes_playable_avi() {
        let _g = crate::fixture::desktop();
        let f = crate::fixture::Fixture::new();
        let dir = scratch("live");
        let out = dir.join("fixture.avi");
        let mut a = args(out.to_string_lossy().as_ref());
        a.insert("target".to_string(), json!({"hwnd": f.hwnd_i64()}));
        a.insert("duration_ms".to_string(), json!(800));
        a.insert("fps".to_string(), json!(4));
        let frame = envelope_result(&dispatch_record(&a));
        assert_eq!(
            frame["structuredContent"]["ok"], true,
            "recording failed: {frame}"
        );
        let avi = std::fs::read(&out).expect("AVI was written");
        assert_eq!(&avi[0..4], b"RIFF", "AVI magic");
        assert_eq!(&avi[8..12], b"AVI ", "AVI form type");
        assert!(movi_frames(&avi) >= 1, "at least one 00dc frame");

        if std::process::Command::new("ffprobe")
            .arg("-version")
            .output()
            .is_ok()
        {
            let probe = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .arg(&out)
                .output()
                .expect("run ffprobe");
            assert!(
                probe.status.success(),
                "ffprobe failed: {}",
                String::from_utf8_lossy(&probe.stderr)
            );
            let parsed: Value = serde_json::from_slice(&probe.stdout).expect("ffprobe JSON");
            let streams = parsed["streams"].as_array().expect("streams");
            assert_eq!(streams.len(), 1, "one video stream: {parsed}");
            assert_eq!(streams[0]["codec_name"], "mjpeg");
        }
        std::fs::remove_dir_all(&dir).expect("scratch cleanup");
    }

    fn envelope_result(text: &str) -> Value {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("bad envelope: {e}: {text}"))
    }
}
