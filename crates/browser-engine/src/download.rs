//! Download contract for row B5.
//!
//! Private home for the `browser_download` tool: caller directory, accept
//! list, timeout, size cap and filename, returning path, bytes, SHA-256, MIME,
//! headers and duration, with partial-file cleanup on cancel. The engine
//! arms `Browser.setDownloadBehavior` on the browser-level CDP connection,
//! clicks the page's download link, and waits on the
//! `Browser.downloadWillBegin` / `Browser.downloadProgress` events captured
//! by the reader.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::{check_unknown, ok_text, ToolCode, ToolErr, ToolReason};

/// Row id this module owns. The stub test reads it; lib code does not.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B5";

/// Frozen input/output contract for row B5 (`browser_download`).
pub fn schema() -> Value {
    json!({
        "name": "download",
        "title": "Browser download",
        "description": "Download a file into a caller-supplied absolute directory. Arms browser-level download behavior, follows the page's download link, and reports the delivered path, byte count, SHA-256, MIME type, response headers and duration. timeoutMs bounds the wait; maxBytes aborts an oversized download; a cancel, timeout or cap breach leaves no partial file in the directory.",
        "inputSchema": {"type": "object", "properties": {
            "dir": {"type": "string"},
            "accept": {"type": "array", "items": {"type": "string"}},
            "timeoutMs": {"type": "integer", "minimum": 0},
            "maxBytes": {"type": "integer", "minimum": 0},
            "filename": {"type": "string"}
        }, "required": ["dir"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "path": {"type": "string"},
            "filename": {"type": "string"},
            "bytes": {"type": "integer", "minimum": 0},
            "sha256": {"type": "string"},
            "mimeType": {"type": "string"},
            "headers": {"type": "object"},
            "durationMs": {"type": "integer", "minimum": 0}
        }, "required": ["path", "bytes", "sha256", "mimeType", "headers", "durationMs"],
        "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
    })
}

/// SHA-256 round constants.
const SHA_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 over the delivered bytes, lowercase hex. Self-contained so this
/// row adds no dependency and no install step.
fn sha256(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// MIME from the response when the CDP event carries one, else the file
/// extension. Parameters such as `charset=utf-8` are stripped.
fn bare_mime(mime: &str) -> &str {
    mime.split(';').next().unwrap_or("").trim()
}

fn mime_for_name(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "text" | "log" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "xml" => "application/xml",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

fn accepted(mime: &str, accept: &[String]) -> bool {
    if accept.is_empty() {
        return true;
    }
    let mime = bare_mime(mime);
    accept.iter().any(|a| {
        let a = bare_mime(a);
        a == mime || a == "*/*" || a.eq_ignore_ascii_case(mime)
    })
}

/// Remove Chrome's `.crdownload` partials in the caller directory. This is
/// the only cleanup allowed on a SUCCESSFUL download: the delivered file is
/// kept, and unrelated files in the directory are never touched.
fn cleanup_crdownloads(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.file_name().to_string_lossy().ends_with(".crdownload") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Cancel/timeout/cap cleanup: remove the target names this call may have
/// created plus any `.crdownload` partials. Unrelated files are never touched.
fn cleanup_partials(dir: &Path, names: &[String]) {
    for name in names.iter().filter(|n| !n.is_empty()) {
        let _ = std::fs::remove_file(dir.join(name));
    }
    cleanup_crdownloads(dir);
}

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_download", "arg": arg}),
    )
}

fn cancelled(prose: String, code: ToolCode, reason: ToolReason, record: &Value) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": code.as_str(), "reason": reason.as_str(), "tool": "browser_download",
               "state": record.get("state").cloned().unwrap_or(Value::Null),
               "filename": record.get("filename").cloned().unwrap_or(Value::Null),
               "filePath": record.get("filePath").cloned().unwrap_or(Value::Null),
               "receivedBytes": record.get("receivedBytes").cloned().unwrap_or(Value::Null)}),
    )
}

/// Dispatcher entry point for the B5 fill lane.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let started = Instant::now();
    if let Err(e) = check_unknown(
        "browser_download",
        args,
        &["dir", "accept", "timeoutMs", "maxBytes", "filename"],
    ) {
        return Err(e);
    }

    let dir_str = args.get("dir").and_then(|v| v.as_str()).ok_or_else(|| {
        invalid(
            "browser_download: missing required arg 'dir'".to_string(),
            ToolReason::MissingArg,
            "dir",
        )
    })?;
    let dir = PathBuf::from(dir_str);
    if !dir.is_absolute() {
        return Err(invalid(
            format!("browser_download: 'dir' must be an absolute path, got '{dir_str}'"),
            ToolReason::BadValue,
            "dir",
        ));
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(ToolErr::typed(
            format!("browser_download: cannot create '{dir_str}': {e}"),
            json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::Unreadable.as_str(),
                   "tool": "browser_download", "arg": "dir"}),
        ));
    }

    let accept: Vec<String> = args
        .get("accept")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let timeout_ms = match args.get("timeoutMs") {
        None => 15_000u64,
        Some(v) => v.as_u64().ok_or_else(|| {
            invalid(
                "browser_download: 'timeoutMs' must be an integer >= 0".to_string(),
                ToolReason::BadValue,
                "timeoutMs",
            )
        })?,
    };
    let max_bytes = match args.get("maxBytes") {
        None => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| {
            invalid(
                "browser_download: 'maxBytes' must be an integer >= 0".to_string(),
                ToolReason::BadValue,
                "maxBytes",
            )
        })?),
    };
    let filename = match args.get("filename") {
        None => None,
        Some(v) => {
            let name = v.as_str().ok_or_else(|| {
                invalid(
                    "browser_download: 'filename' must be a string".to_string(),
                    ToolReason::BadValue,
                    "filename",
                )
            })?;
            if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
                return Err(invalid(
                    format!("browser_download: 'filename' must be a plain file name, got '{name}'"),
                    ToolReason::BadValue,
                    "filename",
                ));
            }
            Some(name.to_string())
        }
    };

    browser.download_reset().await;
    // Windows paths from callers often mix separators (`C:\temp/download`).
    // CDP hands downloadPath to Chrome's native path layer, which rejects a
    // mixed path as a cancelled download, so normalise before sending it.
    let cdp_download_path = if cfg!(windows) {
        dir_str.replace('/', "\\")
    } else {
        dir_str.to_string()
    };
    let params = json!({
        "behavior": "allow",
        "downloadPath": cdp_download_path,
        "eventsEnabled": true
    });
    browser
        .cdp_raw("Browser.setDownloadBehavior", params, None)
        .await
        .map_err(ToolErr::fallback)?;

    // The page owns the URL: click its download link. The optional filename
    // selects among several links; otherwise the first one is used.
    let needle = filename.clone().unwrap_or_default();
    let expr = format!(
        "(() => {{ var links = Array.prototype.slice.call(document.querySelectorAll('a[download]')); \
         if (!links.length) return false; var a = links[0]; \
         for (var i = 0; i < links.length; i++) {{ \
           if (links[i].getAttribute('download') === {needle:?}) {{ a = links[i]; break; }} \
         }} a.click(); return true; }})()"
    );
    let clicked = browser.eval(&expr).await.map_err(ToolErr::fallback)?;
    if clicked.as_bool() != Some(true) {
        return Err(invalid(
            "browser_download: the active page has no a[download] link to follow".to_string(),
            ToolReason::NotFound,
            "dir",
        ));
    }

    let record = browser
        .download_wait(Duration::from_millis(timeout_ms), max_bytes)
        .await
        .map_err(ToolErr::fallback)?;
    // Restore the engine default; the delivered file stays where it landed.
    let _ = browser
        .cdp_raw(
            "Browser.setDownloadBehavior",
            json!({"behavior": "default"}),
            None,
        )
        .await;

    let suggested = record
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let file_path = record
        .get("filePath")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut names = vec![
        suggested.clone(),
        file_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_string(),
    ];
    if let Some(name) = &filename {
        names.push(name.clone());
    }
    let state = record.get("state").and_then(|v| v.as_str()).unwrap_or("");
    if state != "completed" {
        cleanup_partials(&dir, &names);
        let prose = match state {
            "aborted" => format!("browser_download: download exceeded maxBytes and was cancelled"),
            "canceled" | "interrupted" => {
                format!("browser_download: download was cancelled ({state})")
            }
            _ => format!("browser_download: no download completed within {timeout_ms}ms"),
        };
        let (code, reason) = match state {
            "aborted" => (ToolCode::Cancelled, ToolReason::BudgetExceeded),
            "canceled" | "interrupted" => (ToolCode::Cancelled, ToolReason::ClientCancelled),
            _ => (ToolCode::Timeout, ToolReason::BudgetExceeded),
        };
        return Err(cancelled(prose, code, reason, &record));
    }

    let delivered = if !file_path.is_empty() {
        let p = PathBuf::from(&file_path);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    } else {
        None
    }
    .or_else(|| {
        let candidate = dir.join(filename.as_deref().unwrap_or(&suggested));
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
    .or_else(|| {
        let candidate = dir.join(&suggested);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
    .ok_or_else(|| {
        ToolErr::typed(
            format!("browser_download: CDP reported completion but no file landed in '{dir_str}'"),
            json!({"code": ToolCode::FileNotFound.as_str(), "reason": ToolReason::Missing.as_str(),
               "tool": "browser_download"}),
        )
    })?;

    let bytes = std::fs::read(&delivered).map_err(|e| ToolErr::typed(
        format!("browser_download: cannot read delivered file: {e}"),
        json!({"code": ToolCode::FileNotFound.as_str(), "reason": ToolReason::Unreadable.as_str(),
               "tool": "browser_download"}),
    ))?;
    if let Some(max) = max_bytes {
        if bytes.len() as u64 > max {
            cleanup_partials(&dir, &names);
            return Err(cancelled(
                format!(
                    "browser_download: delivered file is {} bytes, over maxBytes {max}",
                    bytes.len()
                ),
                ToolCode::Cancelled,
                ToolReason::BudgetExceeded,
                &record,
            ));
        }
    }

    let response_mime = record
        .get("mimeType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let out_name = filename.clone().unwrap_or_else(|| {
        delivered
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&suggested)
            .to_string()
    });
    let mime = if bare_mime(response_mime).is_empty() {
        mime_for_name(&out_name).to_string()
    } else {
        bare_mime(response_mime).to_string()
    };
    if !accepted(&mime, &accept) {
        cleanup_partials(&dir, &names);
        return Err(invalid(
            format!("browser_download: response MIME '{mime}' is not in accept {accept:?}"),
            ToolReason::Rejected,
            "accept",
        ));
    }

    // Honour the caller's filename when it differs from the suggested name.
    let final_path = if delivered.file_name().and_then(|n| n.to_str()) == Some(out_name.as_str()) {
        delivered.clone()
    } else {
        let target = dir.join(&out_name);
        if let Err(e) = std::fs::rename(&delivered, &target) {
            cleanup_partials(&dir, &names);
            return Err(ToolErr::typed(
                format!("browser_download: cannot name delivered file '{out_name}': {e}"),
                json!({"code": ToolCode::InvalidArgs.as_str(), "reason": ToolReason::SetFailed.as_str(),
                       "tool": "browser_download", "arg": "filename"}),
            ));
        }
        target
    };
    cleanup_crdownloads(&dir);

    let mut headers = json!({
        "content-type": mime.clone(),
        "content-length": bytes.len()
    });
    if let Some(disposition) = record.get("contentDisposition").and_then(|v| v.as_str()) {
        if !disposition.is_empty() {
            headers["content-disposition"] = json!(disposition);
        }
    }
    let body = json!({
        "path": final_path.to_string_lossy(),
        "filename": out_name,
        "bytes": bytes.len(),
        "sha256": sha256(&bytes),
        "mimeType": mime,
        "headers": headers,
        "durationMs": started.elapsed().as_millis() as u64
    });
    Ok(ok_text(body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::sha256;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(b"hand row B5 download payload\n"),
            "20e1664389c59576710eca014871d3f63479d4afe7087fdd182a736e5515800d"
        );
    }
}
