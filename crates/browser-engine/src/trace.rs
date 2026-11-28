//! Trace contract for row B6.
//!
//! Private home for `browser_trace`: an opt-in artifact bundle in a caller's
//! absolute directory. The HAR wraps the CDP `Network` observations already
//! captured by `Browser::network_get` (cap 500) - there is no second capture
//! path here. Screenshots ride the same deterministic capture the `screenshot`
//! tool uses, one per recorded step (a start records one step: the page as it
//! stands when the call lands). Redaction is literal substring replacement in
//! the HAR text only, counted per literal: no regex, no guessing, no claim of
//! universal secret detection. Cancel removes only the files this tool wrote.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use mjpeg_avi::avi_mjpeg;
use serde_json::{json, Value};

use crate::browser::{Browser, NET_CAP};
use crate::tools::{check_unknown, ok_text, ToolCode, ToolErr, ToolReason};

/// Row id this module owns. The stub test reads it; lib code does not.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B6";

const TOOL: &str = "browser_trace";
const MARKER: &str = "[REDACTED]";
const HAR_NAME: &str = "trace.har";
const STEP_NAME: &str = "step-0001.png";
const VIDEO_NAME: &str = "trace.avi";
const MANIFEST_NAME: &str = "manifest.json";
const ALLOWED: &[&str] = &[
    "dir",
    "screenshots",
    "har",
    "video",
    "videoDurationMs",
    "redact",
    "action",
];
/// Bounds the redaction loop; a caller asking for more is refused by name.
const MAX_REDACTIONS: usize = 64;
/// Bounded screencast window: default 3 s, clamped to 250..15000 ms.
const DEFAULT_VIDEO_MS: u64 = 3000;
const MIN_VIDEO_MS: u64 = 250;
const MAX_VIDEO_MS: u64 = 15_000;

/// Frozen input/output contract for row B6 (`browser_trace`).
pub fn schema() -> Value {
    json!({
        "name": "trace",
        "title": "Browser trace",
        "description": "Write an opt-in trace bundle into a caller-supplied ABSOLUTE directory. har writes one HAR 1.2 file that wraps the existing CDP Network observation (the same ring browser_network reads, cap 500); the ring holds url, method, status, mime and a timestamp, so headers, bodies and HAR timings are reported as unknown (-1/0) and the entry names its source. screenshots writes one deterministic PNG per recorded step (one step per start, the page as it stands at the call). redact[] replaces every literal substring in the HAR text with [REDACTED] and counts each literal; it is literal-only, not regex or automatic secret detection, and does not touch screenshot pixels. video writes one playable MJPEG AVI (`trace.avi`) from CDP Page.startScreencast (jpeg quality 80, everyNthFrame 1); every Page.screencastFrame is acknowledged and recorded for videoDurationMs (default 3000, clamped to 250..15000) before Page.stopScreencast. No ffmpeg or external binary is used: a small RIFF AVI writer is in-repo. Missing slots (late frames and the tail) are filled by repeating the previous frame so the constant-rate AVI spans videoDurationMs; manifest.json lists every artifact and the video facts (path, frames, capturedFrames, paddedFrames, durationMs, fps, bytes, container, firstFrameTsMs, lastFrameTsMs), where frames includes repeats, capturedFrames counts fresh screencast frames and paddedFrames counts repeats. action cancel removes only the files this tool created; start refuses when any target name already exists in dir, so a pre-existing file is never overwritten or deleted. No upload and no default recording.",
        "inputSchema": {"type": "object", "properties": {
            "dir": {"type": "string", "description": "Absolute directory for the trace bundle. Required for action:\"start\"; a relative path is refused. action:\"cancel\" needs none when there is no active trace (it only removes files this tool created)."},
            "screenshots": {"type": "boolean"},
            "har": {"type": "boolean"},
            "video": {"type": "boolean"},
            "videoDurationMs": {"type": "integer", "minimum": 250, "maximum": 15000, "default": 3000},
            "redact": {"type": "array", "items": {"type": "string", "minLength": 1}},
            "action": {"type": "string", "enum": ["start", "cancel"]}
        }, "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "dir": {"type": "string"},
            "files": {"type": "array", "items": {"type": "object"}},
            "bytes": {"type": "integer", "minimum": 0},
            "redactions": {"type": "array", "items": {"type": "object"}},
            "note": {"type": "string"}
        }, "required": ["ok"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
    })
}

fn invalid(reason: ToolReason, arg: &str, prose: impl Into<String>) -> ToolErr {
    ToolErr::typed(
        prose.into(),
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": TOOL, "arg": arg}),
    )
}

fn ok(v: Value) -> Result<Value, ToolErr> {
    Ok(ok_text(v.to_string()))
}

/// One active bundle per engine, keyed by stable `Browser::id()` (never a
/// pointer). Only file names this tool wrote are stored, so cancel can never
/// delete a caller's unrelated artifact.
struct TraceState {
    dir: PathBuf,
    files: Vec<String>,
}

static TRACES: OnceLock<Mutex<HashMap<u64, TraceState>>> = OnceLock::new();

fn traces() -> &'static Mutex<HashMap<u64, TraceState>> {
    TRACES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cleanup(dir: &Path, files: &[String]) {
    for name in files {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Epoch milliseconds to UTC ISO 8601. Days-from-civil arithmetic, no date
/// dependency: the HAR date field must be a real date, not a placeholder.
fn iso_utc(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let rem = ms % 86_400_000;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600_000,
        (rem % 3_600_000) / 60_000,
        (rem % 60_000) / 1_000,
        rem % 1_000
    )
}

/// Build HAR 1.2 from the existing network ring. Fields the ring does not
/// carry are kept as HAR's "unknown" (-1) or zero and named in `_capture`;
/// nothing is invented from the URL or the MIME type.
async fn har_text(browser: &Browser) -> String {
    let observed = browser.network_get(None, NET_CAP).await;
    let entries: Vec<Value> = observed["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let ts = e["ts"].as_u64().unwrap_or(0);
                    let status = e["status"].as_i64();
                    let mime = e["mime"].as_str().unwrap_or("").to_string();
                    let method = e["method"].as_str().unwrap_or("").to_string();
                    let url = e["url"].as_str().unwrap_or("").to_string();
                    json!({
                        "startedDateTime": iso_utc(ts),
                        "time": 0,
                        "request": {
                            "method": method, "url": url,
                            "httpVersion": "unknown",
                            "cookies": [], "headers": [], "queryString": [],
                            "headersSize": -1, "bodySize": -1
                        },
                        "response": {
                            "status": status.unwrap_or(0),
                            "statusText": "",
                            "httpVersion": "unknown",
                            "cookies": [], "headers": [],
                            "content": {"size": -1, "mimeType": mime},
                            "redirectURL": "",
                            "headersSize": -1, "bodySize": -1,
                            "_statusUnknown": status.is_none()
                        },
                        "cache": {},
                        "timings": {"send": 0, "wait": 0, "receive": 0},
                        "_capture": {
                            "source": "browser.network_get",
                            "domain": "Network",
                            "requestId": e["requestId"].clone(),
                            "type": e["type"].clone(),
                            "finished": e["finished"].clone(),
                            "failed": e["failed"].clone(),
                            "errorText": e["errorText"].clone(),
                            "tsMs": ts,
                            "note": "Wraps the existing CDP Network ring. Headers, bodies, timings and statusText are not retained there; -1/0 mean unknown, not measured."
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let started = iso_utc(now_ms());
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "hand browser_trace", "version": "0.3.0"},
            "browser": {"name": crate::browser::backend_name()},
            "pages": [{"startedDateTime": started, "id": "page_1",
                       "title": "", "pageTimings": {"onContentLoad": -1, "onLoad": -1}}],
            "entries": entries,
            "comment": "HAR 1.2 wrapper over browser.network_get (CDP Network domain ring, cap 500); no separate capture, no header or body reconstruction."
        }
    })
    .to_string()
}

/// Literal-only, counted redaction. Occurrences are counted BEFORE the
/// replacement, each literal is unique, and the marker is fixed.
fn apply_redactions(text: &mut String, literals: &[String]) -> Vec<Value> {
    literals
        .iter()
        .map(|literal| {
            let count = text.matches(literal.as_str()).count();
            if count > 0 {
                *text = text.replace(literal.as_str(), MARKER);
            }
            json!({"literal": literal, "marker": MARKER, "count": count})
        })
        .collect()
}

fn bool_arg(args: &Value, key: &str) -> Result<bool, ToolErr> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(other) => Err(invalid(
            ToolReason::BadValue,
            key,
            format!("{TOOL}: '{key}' must be a boolean, got {other}"),
        )),
    }
}

/// Optional screencast window. Returns the requested value and the
/// effective (clamped) one, so a clamp is reported instead of silent.
fn video_duration_arg(args: &Value) -> Result<(u64, u64), ToolErr> {
    let requested = match args.get("videoDurationMs") {
        None | Some(Value::Null) => DEFAULT_VIDEO_MS,
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| {
            invalid(
                ToolReason::BadValue,
                "videoDurationMs",
                format!("{TOOL}: 'videoDurationMs' must be an integer >= 0, got {n}"),
            )
        })?,
        Some(other) => {
            return Err(invalid(
                ToolReason::BadValue,
                "videoDurationMs",
                format!("{TOOL}: 'videoDurationMs' must be an integer, got {other}"),
            ));
        }
    };
    Ok((requested, requested.clamp(MIN_VIDEO_MS, MAX_VIDEO_MS)))
}

/// Name the bound that was actually hit. The old wording said "clamped to
/// 15000 ms cap" for every clamp, including a 10 ms request clamped UP to
/// the 250 ms floor - a report that named the wrong bound.
fn clamp_note(requested_ms: u64, effective_ms: u64) -> String {
    if effective_ms < requested_ms {
        format!(", clamped down to {effective_ms} ms cap")
    } else if effective_ms > requested_ms {
        format!(", clamped up to {effective_ms} ms floor")
    } else {
        String::new()
    }
}

/// One `1/fps` tick of the AVI's constant-rate timeline. Late captures leave
/// empty slots; fill each with the previous frame, tail included, so
/// `frames.len()/fps` covers the whole recording window. Slots before the
/// first captured frame are dropped: there is no honest byte to repeat.
fn pad_slots(slots: &[Option<Vec<u8>>]) -> (Vec<Vec<u8>>, u64) {
    let Some(first) = slots.iter().position(Option::is_some) else {
        return (Vec::new(), 0);
    };
    let mut out = Vec::with_capacity(slots.len() - first);
    let mut previous = slots[first].clone().expect("first slot holds a frame");
    let mut padded = 0u64;
    for slot in &slots[first..] {
        match slot {
            Some(jpeg) => previous = jpeg.clone(),
            None => padded += 1,
        }
        out.push(previous.clone());
    }
    (out, padded)
}

/// Place each captured frame on that timeline. CDP timestamps are absolute
/// seconds: use their difference from the first frame so a clock offset
/// cannot shift the slots. A collision moves a frame to the next free slot
/// instead of overwriting a fresh capture; a frame arriving past the window
/// is appended rather than dropped.
fn video_slots(
    frames: &[Vec<u8>],
    timestamps: Option<&[f64]>,
    fps: u32,
    target_slots: usize,
) -> Vec<Option<Vec<u8>>> {
    let mut slots = vec![None; target_slots];
    let first_ts = timestamps.and_then(|ts| ts.first()).copied().unwrap_or(0.0);
    let scale = f64::from(fps);
    for (i, jpeg) in frames.iter().enumerate() {
        let wanted = match timestamps {
            Some(ts) => ((ts[i] - first_ts) * scale).max(0.0) as usize,
            None => i,
        };
        let mut at = wanted;
        while at < slots.len() && slots[at].is_some() {
            at += 1;
        }
        if at == slots.len() {
            slots.push(Some(jpeg.clone()));
        } else {
            slots[at] = Some(jpeg.clone());
        }
    }
    slots
}

/// Record one bounded screencast and encode it as an MJPEG AVI. The CDP
/// reader ACKs each frame (`Page.screencastFrameAck`) as it arrives, and
/// `screencast_stop` is attempted before any frame/encode work.
async fn record_video(
    browser: &Browser,
    dir: &Path,
    requested_ms: u64,
    duration_ms: u64,
) -> Result<Value, ToolErr> {
    browser
        .screencast_start()
        .await
        .map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot start Page.screencast: {e}")))?;
    tokio::time::sleep(Duration::from_millis(duration_ms)).await;
    let stopped = browser.screencast_stop().await;
    let raw = browser.screencast_drain().await;
    stopped.map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot stop Page.screencast: {e}")))?;

    // Keep each frame's metadata paired with its decoded bytes so timing
    // can never shift onto a different frame when one frame is dropped.
    let decoded: Vec<(Vec<u8>, Option<f64>)> = raw
        .iter()
        .filter_map(|f| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(f.data.as_bytes())
                .ok()?;
            (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8)
                .then_some((bytes, f.metadata_ts))
        })
        .collect();
    if decoded.is_empty() {
        return Err(ToolErr::fallback(format!(
            "{TOOL}: Page.screencast produced no JPEG frames in {duration_ms} ms"
        )));
    }
    let (frames, stamps): (Vec<Vec<u8>>, Vec<Option<f64>>) = decoded.into_iter().unzip();
    let timestamps: Option<Vec<f64>> = stamps.into_iter().collect();
    let (first_ts_ms, last_ts_ms, fps, timing) = match &timestamps {
        Some(ts) if !ts.is_empty() => {
            let first = ts[0] * 1000.0;
            let last = ts[ts.len() - 1] * 1000.0;
            let span_s = (last - first) / 1000.0;
            let fps = if span_s > 0.0 {
                ((frames.len() as f64 / span_s).round() as u32).clamp(1, 60)
            } else {
                10
            };
            (first, last, fps, "cdp-metadata")
        }
        _ => (
            0.0,
            (frames.len().saturating_sub(1) as f64) * 100.0,
            10,
            "fixed-10fps",
        ),
    };

    let session_id = raw
        .first()
        .map(|f| f.session_id.clone())
        .unwrap_or_default();
    let received_span_ms = raw
        .first()
        .zip(raw.last())
        .map(|(first, last)| last.received_ms.saturating_sub(first.received_ms))
        .unwrap_or(0);
    // The AVI is constant-rate: give it one slot per 1/fps tick across the
    // whole screencast window, then fill missing slots with the previous
    // frame. A short frame list would play faster than the wall-clock window.
    let target_slots = ((duration_ms as f64 * f64::from(fps)) / 1000.0)
        .round()
        .max(frames.len() as f64) as usize;
    let slots = video_slots(&frames, timestamps.as_deref(), fps, target_slots);
    let (written, padded) = pad_slots(&slots);
    let avi = avi_mjpeg(&written, fps);

    std::fs::write(dir.join(VIDEO_NAME), &avi)
        .map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot write {VIDEO_NAME}: {e}")))?;
    Ok(json!({
        "path": dir.join(VIDEO_NAME).to_string_lossy(),
        "frames": written.len(),
        "capturedFrames": frames.len(),
        "paddedFrames": padded,
        "durationMs": duration_ms,
        "requestedDurationMs": requested_ms,
        "clamped": requested_ms != duration_ms,
        "fps": fps,
        "bytes": avi.len(),
        "container": "mjpeg-avi",
        "firstFrameTsMs": first_ts_ms,
        "lastFrameTsMs": last_ts_ms,
        "timing": timing,
        "session": session_id,
        "receivedSpanMs": received_span_ms,

        "note": if timing == "fixed-10fps" {
            "CDP screencast metadata had no timestamp; frames are spaced at a fixed 10 fps"
        } else {
            "frame timing from CDP Page.screencastFrame metadata timestamps"
        },
    }))
}
fn redact_arg(args: &Value) -> Result<Vec<String>, ToolErr> {
    let Some(arr) = args.get("redact").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    if arr.len() > MAX_REDACTIONS {
        return Err(invalid(
            ToolReason::BadValue,
            "redact",
            format!(
                "{TOOL}: 'redact' accepts at most {MAX_REDACTIONS} literals, got {}",
                arr.len()
            ),
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for value in arr {
        let literal = value.as_str().ok_or_else(|| {
            invalid(
                ToolReason::BadValue,
                "redact",
                format!("{TOOL}: every 'redact' entry must be a string, got {value}"),
            )
        })?;
        if literal.is_empty() {
            return Err(invalid(
                ToolReason::BadValue,
                "redact",
                format!(
                    "{TOOL}: 'redact' entries must be non-empty literals (no empty-string match)"
                ),
            ));
        }
        if !out.iter().any(|s| s == literal) {
            out.push(literal.to_string());
        }
    }
    Ok(out)
}

fn absolute_dir(args: &Value) -> Result<PathBuf, ToolErr> {
    let dir = args.get("dir").and_then(|v| v.as_str()).ok_or_else(|| {
        invalid(
            ToolReason::MissingArg,
            "dir",
            format!("{TOOL}: start needs an absolute 'dir'"),
        )
    })?;
    let path = PathBuf::from(dir);
    if !path.is_absolute() {
        return Err(invalid(
            ToolReason::BadValue,
            "dir",
            format!("{TOOL}: 'dir' must be an absolute path, got '{dir}'"),
        ));
    }
    std::fs::create_dir_all(&path).map_err(|e| {
        ToolErr::typed(
            format!("{TOOL}: cannot create '{dir}': {e}"),
            json!({"code": ToolCode::FileNotFound.as_str(), "reason": ToolReason::Unreadable.as_str(),
                   "tool": TOOL, "arg": "dir"}),
        )
    })?;
    Ok(path)
}

/// Refuse to start when any fixed target name already exists in `dir`.
/// Without this gate, `cancel` would delete a pre-existing file of the same
/// name that this tool never created. The typed error names the exact path.
fn refuse_existing_targets(
    dir: &Path,
    har: bool,
    screenshots: bool,
    video: bool,
) -> Result<(), ToolErr> {
    let mut names = vec![MANIFEST_NAME];
    if har {
        names.push(HAR_NAME);
    }
    if screenshots {
        names.push(STEP_NAME);
    }
    if video {
        names.push(VIDEO_NAME);
    }
    for name in names {
        let path = dir.join(name);
        if path.exists() {
            let shown = path.to_string_lossy().to_string();
            return Err(ToolErr::typed(
                format!(
                    "{TOOL}: target '{shown}' already exists; cancel the previous trace or use another 'dir'"
                ),
                json!({"code": ToolCode::InvalidArgs.as_str(),
                       "reason": ToolReason::Recorded.as_str(),
                       "tool": TOOL, "arg": "dir", "path": shown}),
            ));
        }
    }
    Ok(())
}

/// One deterministic capture per recorded step. The screenshot tool's own
/// gate (dom-only refusal, capture lock, restore, absolute path write) is
/// reused whole; no second pixel path lives in this module.
async fn capture_step(browser: &Browser, dir: &Path) -> Result<(String, usize), ToolErr> {
    let path = dir.join(STEP_NAME);
    let path_str = path.to_string_lossy().to_string();
    // Call the screenshot implementation directly, not the dispatcher: the
    // dispatcher arms this tool, and that cycle would be an infinite future.
    // All of screenshot's gates and its deterministic freeze stay in force.
    crate::tools::tool_screenshot(
        browser,
        false,
        Some(&path_str),
        true,
        None,
        None,
        None,
        None,
        crate::capture::Format::Png,
        None,
        false,
        None,
        false,
        1.0,
    )
    .await?;
    let bytes = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot size the captured step: {e}")))?;
    Ok((STEP_NAME.to_string(), bytes))
}

fn file_record(dir: &Path, name: &str, bytes: usize, kind: &str) -> Value {
    json!({"name": name, "path": dir.join(name).to_string_lossy(),
           "bytes": bytes, "kind": kind})
}

/// Write every requested artifact, tracking each owned name so a mid-way
/// failure can remove exactly what this call created.
async fn produce(
    browser: &Browser,
    dir: &Path,
    har: bool,
    screenshots: bool,
    literals: &[String],
    video: Value,
    created: &mut Vec<String>,
) -> Result<(Vec<Value>, usize, Vec<Value>), ToolErr> {
    let mut files = Vec::new();
    let mut bytes = 0usize;
    let mut redactions = Vec::new();
    if har {
        let mut text = har_text(browser).await;
        redactions = apply_redactions(&mut text, literals);
        let n = text.len();
        std::fs::write(dir.join(HAR_NAME), text.as_bytes())
            .map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot write {HAR_NAME}: {e}")))?;
        created.push(HAR_NAME.to_string());
        files.push(file_record(dir, HAR_NAME, n, "har"));
        bytes += n;
    }
    if screenshots {
        // One recorded step per start: the page as it stands when the call
        // lands. The step is captured once, under the deterministic freeze.
        let (name, n) = capture_step(browser, dir).await?;
        created.push(name.clone());
        files.push(file_record(dir, &name, n, "screenshot"));
        bytes += n;
    }
    // The AVI was written by record_video before produce ran. Register it in
    // the SAME file list and byte total as the other artifacts, so both the
    // reply and manifest.json account for every file this call created.
    if !video.is_null() {
        let n = video["bytes"].as_u64().unwrap_or(0) as usize;
        files.push(file_record(dir, VIDEO_NAME, n, "video"));
        bytes += n;
    }
    // The manifest is written LAST so its file list describes every artifact
    // that actually landed, including the AVI when video was requested.
    let manifest = json!({
        "tool": TOOL,
        "dir": dir.to_string_lossy(),
        "files": &files,
        "video": video,
    });
    let text = manifest.to_string();
    std::fs::write(dir.join(MANIFEST_NAME), text.as_bytes())
        .map_err(|e| ToolErr::fallback(format!("{TOOL}: cannot write {MANIFEST_NAME}: {e}")))?;
    created.push(MANIFEST_NAME.to_string());
    files.push(file_record(dir, MANIFEST_NAME, text.len(), "manifest"));
    bytes += text.len();
    Ok((files, bytes, redactions))
}

/// Dispatch from `tools::call`. Cancel is idempotent and never fails for an
/// absent bundle; start is opt-in and validates everything before writing.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown(TOOL, args, ALLOWED)?;
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("start");
    if action == "cancel" {
        let (dir, files) = {
            let mut all = traces().lock().unwrap_or_else(|e| e.into_inner());
            match all.remove(&browser.id()) {
                Some(state) => (state.dir, state.files),
                None => (PathBuf::new(), Vec::new()),
            }
        };
        cleanup(&dir, &files);
        return ok(json!({
            "ok": true, "dir": dir.to_string_lossy(), "files": [],
            "bytes": 0, "redactions": [],
            "note": if files.is_empty() { "no active trace" } else { "trace artifacts removed" }
        }));
    }
    if action != "start" {
        return Err(invalid(
            ToolReason::UnknownAction,
            "action",
            format!("{TOOL}: unknown action '{action}' (start|cancel)"),
        ));
    }

    let har = bool_arg(args, "har")?;
    let screenshots = bool_arg(args, "screenshots")?;
    let video = bool_arg(args, "video")?;
    let (requested_ms, duration_ms) = video_duration_arg(args)?;
    let literals = redact_arg(args)?;
    let dir = absolute_dir(args)?;
    refuse_existing_targets(&dir, har, screenshots, video)?;
    if traces()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&browser.id())
    {
        return Err(invalid(
            ToolReason::Recorded,
            "action",
            format!("{TOOL}: a trace is already active on this engine; cancel it first"),
        ));
    }

    let mut created: Vec<String> = Vec::new();
    let mut video_info = Value::Null;
    if video {
        match record_video(browser, &dir, requested_ms, duration_ms).await {
            Ok(info) => {
                created.push(VIDEO_NAME.to_string());
                video_info = info;
            }
            Err(e) => {
                cleanup(&dir, &created);
                return Err(e);
            }
        }
    }
    let outcome = produce(
        browser,
        &dir,
        har,
        screenshots,
        &literals,
        video_info.clone(),
        &mut created,
    )
    .await;
    let (files, bytes, redactions) = match outcome {
        Ok(v) => v,
        Err(e) => {
            cleanup(&dir, &created);
            return Err(e);
        }
    };
    traces().lock().unwrap_or_else(|e| e.into_inner()).insert(
        browser.id(),
        TraceState {
            dir: dir.clone(),
            files: created,
        },
    );
    let note = if video {
        format!(
            "video: {} frames of MJPEG AVI over {} ms ({} fresh, {} repeated; requested {} ms{}); timing: {}; redaction is literal-only over the HAR text",
            video_info["frames"],
            video_info["durationMs"],
            video_info["capturedFrames"],
            video_info["paddedFrames"],
            video_info["requestedDurationMs"],
            clamp_note(requested_ms, duration_ms),
            video_info["timing"],
        )
    } else {
        "redaction is literal-only over the HAR text; no automatic secret detection".to_string()
    };
    ok(json!({
        "ok": true, "dir": dir.to_string_lossy(), "files": files,
        "bytes": bytes, "redactions": redactions,
        "note": note,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_redactions, avi_mjpeg, clamp_note, iso_utc, pad_slots, video_duration_arg,
        video_slots, DEFAULT_VIDEO_MS, MARKER, MAX_VIDEO_MS, MIN_VIDEO_MS, ROW,
    };

    #[test]
    fn row_is_b6() {
        assert_eq!(ROW, "B6");
    }

    #[test]
    fn redaction_counts_literals_without_regex_semantics() {
        let mut text = "a.b Authorization a.b Authorization".to_string();
        let report = apply_redactions(
            &mut text,
            &[
                ".".to_string(),
                "Authorization".to_string(),
                "none".to_string(),
            ],
        );
        assert_eq!(text, format!("a{MARKER}b {MARKER} a{MARKER}b {MARKER}"));
        assert_eq!(
            report[0]["count"], 2,
            "a literal dot matches dots, not every character"
        );
        assert_eq!(report[1]["count"], 2);
        assert_eq!(
            report[2]["count"], 0,
            "a missing literal is still named and counted"
        );
        assert!(report.iter().all(|r| r["marker"] == MARKER));
    }

    #[test]
    fn har_dates_are_real_utc_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(86_400_000), "1970-01-02T00:00:00.000Z");
        assert_eq!(iso_utc(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
    }

    fn le32_at(buf: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
    }

    fn find(buf: &[u8], needle: &[u8]) -> Option<usize> {
        buf.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn padding_repeats_previous_frame_across_gaps_and_tail() {
        let f = |n: u8| vec![0xFF, 0xD8, n];
        // Three captures at slots 0, 3 and 7 of an 8-slot window: five
        // missing slots, each of which must hold the previous JPEG.
        let slots = vec![
            Some(f(1)),
            None,
            None,
            Some(f(2)),
            None,
            None,
            None,
            Some(f(3)),
        ];
        let (frames, padded) = pad_slots(&slots);
        assert_eq!(frames.len(), 8);
        assert_eq!(padded, 5);
        assert_eq!(frames[0], f(1));
        assert_eq!(frames[1], f(1));
        assert_eq!(frames[2], f(1));
        assert_eq!(frames[3], f(2));
        assert_eq!(frames[4], f(2));
        assert_eq!(frames[5], f(2));
        assert_eq!(frames[6], f(2));
        assert_eq!(frames[7], f(3));
    }

    #[test]
    fn padding_leaves_a_gapless_video_untouched() {
        let slots: Vec<Option<Vec<u8>>> = (1..=3).map(|n| Some(vec![0xFF, 0xD8, n])).collect();
        let (frames, padded) = pad_slots(&slots);
        assert_eq!(padded, 0, "no gaps means no repeats");
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames,
            vec![
                vec![0xFF, 0xD8, 1],
                vec![0xFF, 0xD8, 2],
                vec![0xFF, 0xD8, 3]
            ]
        );
    }

    #[test]
    fn padding_drops_leading_empty_slots_without_inventing_a_frame() {
        let slots = vec![None, Some(vec![1]), None];
        let (frames, padded) = pad_slots(&slots);
        assert_eq!(frames, vec![vec![1], vec![1]]);
        assert_eq!(padded, 1);
    }

    #[test]
    fn video_slots_maps_cdp_timestamps_and_pads_to_the_window() {
        let frames = vec![vec![1], vec![2]];
        let ts = [0.0, 0.5]; // the second frame arrived half a second late
        let slots = video_slots(&frames, Some(&ts), 10, 30);
        assert_eq!(slots.len(), 30, "3 s at 10 fps");
        assert_eq!(slots[0], Some(vec![1]));
        assert_eq!(slots[5], Some(vec![2]));
        assert!(slots[29].is_none(), "the tail is empty until padding");
        let (written, padded) = pad_slots(&slots);
        assert_eq!(written.len(), 30);
        assert_eq!(padded, 28);
        assert_eq!(
            written.len() as f64 / 10.0,
            3.0,
            "AVI duration matches the window"
        );
    }

    #[test]
    fn video_slots_uses_successive_slots_without_metadata() {
        let frames = vec![vec![1], vec![2], vec![3]];
        let slots = video_slots(&frames, None, 10, 30);
        assert_eq!(slots[0], Some(vec![1]));
        assert_eq!(slots[1], Some(vec![2]));
        assert_eq!(slots[2], Some(vec![3]));
        assert!(slots[3].is_none());
    }

    #[test]
    fn avi_writer_emits_riff_movi_chunks_and_index() {
        // Two odd-length frames and one even: every odd chunk must carry a
        // zero pad byte that is NOT counted in its chunk length.
        let frames = vec![
            vec![0xFF, 0xD8, 0x01],
            vec![0xFF, 0xD8, 0x02, 0x03],
            vec![0xFF, 0xD8, 0x04, 0x05, 0xFF],
        ];
        let avi = avi_mjpeg(&frames, 10);
        // Independent spec checks: nothing below reuses the writer's
        // running offset arithmetic.
        assert_eq!(&avi[0..4], b"RIFF", "RIFF magic");
        assert_eq!(&avi[8..12], b"AVI ", "AVI form type");
        assert_eq!(
            le32_at(&avi, 4) as usize,
            avi.len() - 8,
            "RIFF size = file - 8"
        );
        for id in [
            &b"hdrl"[..],
            &b"avih"[..],
            &b"strl"[..],
            &b"strh"[..],
            &b"strf"[..],
        ] {
            assert!(find(&avi, id).is_some(), "missing {id:?}");
        }
        let avih = find(&avi, b"avih").expect("avih chunk");
        assert_eq!(le32_at(&avi, avih + 4), 56, "AVIMAINHEADER is 56 bytes");
        let strl = avih + 8 + 56; // hard spec constant, not writer arithmetic
        assert_eq!(&avi[strl..strl + 4], b"LIST", "strl LIST follows avih");
        assert_eq!(&avi[strl + 8..strl + 12], b"strl", "strl list type");

        let movi = find(&avi, b"movi").expect("movi list");
        let idx = find(&avi, b"idx1").expect("idx1 index");
        assert!(movi < idx, "movi precedes idx1");

        // Every '00dc' FOURCC before idx1 is a movi chunk. idx1 repeats
        // those FOURCCs after itself, so it is excluded from this count.
        let chunks: Vec<usize> = avi[..idx]
            .windows(4)
            .enumerate()
            .filter_map(|(at, w)| (w == b"00dc").then_some(at))
            .collect();
        assert_eq!(chunks.len(), frames.len(), "one 00dc chunk per frame");
        for (at, frame) in chunks.iter().zip(&frames) {
            let len = le32_at(&avi, at + 4) as usize;
            assert_eq!(len, frame.len(), "chunk length excludes pad");
            assert_eq!(&avi[at + 8..at + 8 + len], frame.as_slice());
            if len % 2 == 1 {
                assert_eq!(avi[at + 8 + len], 0, "odd frame is zero padded");
            }
        }

        let idx_entries = le32_at(&avi, idx + 4) as usize / 16;
        assert_eq!(idx_entries, chunks.len(), "idx1 count equals 00dc count");
        let first_offset = le32_at(&avi, idx + 16) as usize;
        assert_eq!(
            first_offset, 4,
            "first idx1 offset is 4 from the movi FOURCC"
        );
        assert_eq!(&avi[movi + first_offset..movi + first_offset + 4], b"00dc");
        for (k, &chunk_at) in chunks.iter().enumerate() {
            let entry = idx + 8 + k * 16;
            assert_eq!(&avi[entry..entry + 4], b"00dc");
            let offset = le32_at(&avi, entry + 8) as usize;
            let len = le32_at(&avi, entry + 12) as usize;
            assert_eq!(movi + offset, chunk_at, "idx1 points at a real movi chunk");
            assert_eq!(len, le32_at(&avi, chunk_at + 4) as usize);
        }
    }

    #[test]
    fn avih_is_56_bytes_and_strl_starts_after_it() {
        let avi = avi_mjpeg(&[vec![0xFF, 0xD8, 0x01]], 10);
        let avih = find(&avi, b"avih").expect("avih chunk");
        assert_eq!(le32_at(&avi, avih + 4), 56, "AVIMAINHEADER size is 56");
        let strl = avih + 8 + 56;
        assert_eq!(&avi[strl..strl + 4], b"LIST");
        assert_eq!(
            &avi[strl + 8..strl + 12],
            b"strl",
            "56-byte avih leaves strl here"
        );
    }

    #[test]
    fn video_duration_arg_clamps_and_reports_both_bounds() {
        let (requested, effective) =
            video_duration_arg(&serde_json::json!({"videoDurationMs": 99_999})).unwrap();
        assert_eq!(requested, 99_999, "request is preserved for reporting");
        assert_eq!(effective, MAX_VIDEO_MS, "above-cap request is clamped");
        let (_, floor) = video_duration_arg(&serde_json::json!({"videoDurationMs": 10})).unwrap();
        assert_eq!(floor, MIN_VIDEO_MS);
        let (requested, effective) = video_duration_arg(&serde_json::json!({})).unwrap();
        assert_eq!(requested, DEFAULT_VIDEO_MS);
        assert_eq!(effective, DEFAULT_VIDEO_MS);
        assert!(video_duration_arg(&serde_json::json!({"videoDurationMs": "3000"})).is_err());
    }

    #[test]
    fn clamp_note_names_the_hit_bound() {
        assert_eq!(clamp_note(10, MIN_VIDEO_MS), ", clamped up to 250 ms floor");
        assert_eq!(
            clamp_note(99_999, MAX_VIDEO_MS),
            ", clamped down to 15000 ms cap"
        );
        assert_eq!(clamp_note(DEFAULT_VIDEO_MS, DEFAULT_VIDEO_MS), "");
        assert!(clamp_note(10, MIN_VIDEO_MS).contains("floor"));
        assert!(!clamp_note(10, MIN_VIDEO_MS).contains("cap"));
        assert!(clamp_note(99_999, MAX_VIDEO_MS).contains("cap"));
    }
}
