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
use std::time::{SystemTime, UNIX_EPOCH};

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
const ALLOWED: &[&str] = &["dir", "screenshots", "har", "video", "redact", "action"];
/// Bounds the redaction loop; a caller asking for more is refused by name.
const MAX_REDACTIONS: usize = 64;

/// Frozen input/output contract for row B6 (`browser_trace`).
pub fn schema() -> Value {
    json!({
        "name": "trace",
        "title": "Browser trace",
        "description": "Write an opt-in trace bundle into a caller-supplied ABSOLUTE directory. har writes one HAR 1.2 file that wraps the existing CDP Network observation (the same ring browser_network reads, cap 500); the ring holds url, method, status, mime and a timestamp, so headers, bodies and HAR timings are reported as unknown (-1/0) and the entry names its source. screenshots writes one deterministic PNG per recorded step (one step per start, the page as it stands at the call). redact[] replaces every literal substring in the HAR text with [REDACTED] and counts each literal; it is literal-only, not regex or automatic secret detection, and does not touch screenshot pixels. video is typed not_implemented: a CDP screencast wrapper is not shipped. action cancel removes only the files this tool created. No upload and no default recording.",
        "inputSchema": {"type": "object", "properties": {
            "dir": {"type": "string"},
            "screenshots": {"type": "boolean"},
            "har": {"type": "boolean"},
            "video": {"type": "boolean"},
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

/// A named, typed absence. The field is always named so a runner can report
/// the limit instead of treating it as a broken capability.
fn not_implemented(field: &str) -> ToolErr {
    ToolErr::typed(
        format!("{TOOL}: '{field}' is not implemented; a CDP screencast wrapper is not shipped, so no video artifact is written"),
        json!({"code": "not_implemented", "tool": TOOL, "field": field,
               "reason": "cdp_screencast_not_wrapped"}),
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
            "creator": {"name": "hand browser_trace", "version": "0.2.0"},
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

fn redact_arg(args: &Value) -> Result<Vec<String>, ToolErr> {
    let Some(arr) = args.get("redact").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    if arr.len() > MAX_REDACTIONS {
        return Err(invalid(
            ToolReason::BadValue,
            "redact",
            format!("{TOOL}: 'redact' accepts at most {MAX_REDACTIONS} literals, got {}", arr.len()),
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
                format!("{TOOL}: 'redact' entries must be non-empty literals (no empty-string match)"),
            ));
        }
        if !out.iter().any(|s| s == literal) {
            out.push(literal.to_string());
        }
    }
    Ok(out)
}

fn absolute_dir(args: &Value) -> Result<PathBuf, ToolErr> {
    let dir = args
        .get("dir")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
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
    created: &mut Vec<String>,
) -> Result<(Vec<Value>, usize, Vec<Value>), ToolErr> {
    let mut files = Vec::new();
    let mut bytes = 0usize;
    let mut redactions = Vec::new();
    if har {
        let mut text = har_text(browser).await;
        redactions = apply_redactions(&mut text, literals);
        let n = text.len();
        std::fs::write(dir.join(HAR_NAME), text.as_bytes()).map_err(|e| {
            ToolErr::fallback(format!("{TOOL}: cannot write {HAR_NAME}: {e}"))
        })?;
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
    let literals = redact_arg(args)?;
    // Reject the unimplemented field before touching the caller's directory:
    // a refusal must not leave a partial bundle behind.
    if video {
        return Err(not_implemented("video"));
    }
    let dir = absolute_dir(args)?;
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
    let outcome = produce(browser, &dir, har, screenshots, &literals, &mut created).await;
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
    ok(json!({
        "ok": true, "dir": dir.to_string_lossy(), "files": files,
        "bytes": bytes, "redactions": redactions,
        "note": "redaction is literal-only over the HAR text; no automatic secret detection"
    }))
}

#[cfg(test)]
mod tests {
    use super::{apply_redactions, iso_utc, MARKER, ROW};

    #[test]
    fn row_is_b6() {
        assert_eq!(ROW, "B6");
    }

    #[test]
    fn redaction_counts_literals_without_regex_semantics() {
        let mut text = "a.b Authorization a.b Authorization".to_string();
        let report = apply_redactions(&mut text, &[".".to_string(), "Authorization".to_string(), "none".to_string()]);
        assert_eq!(text, format!("a{MARKER}b {MARKER} a{MARKER}b {MARKER}"));
        assert_eq!(report[0]["count"], 2, "a literal dot matches dots, not every character");
        assert_eq!(report[1]["count"], 2);
        assert_eq!(report[2]["count"], 0, "a missing literal is still named and counted");
        assert!(report.iter().all(|r| r["marker"] == MARKER));
    }

    #[test]
    fn har_dates_are_real_utc_instants() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc(86_400_000), "1970-01-02T00:00:00.000Z");
        assert_eq!(iso_utc(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
    }
}
