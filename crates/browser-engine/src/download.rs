//! Download contract for row B5.
//!
//! Private home for the `browser_download` tool: caller directory, accept
//! list, timeout, size cap and filename, returning path, bytes, SHA-256, MIME,
//! headers and duration, with partial-file cleanup on cancel. This is a
//! scaffold. [`handle`] returns a typed `not_implemented` error until the B5
//! fill lane adds the CDP download logic.

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::ToolErr;

/// Row id this module owns.
pub const ROW: &str = "B5";

/// Frozen input/output contract for row B5 (`browser_download`).
/// Implementation is not present in this scaffold.
pub fn schema() -> Value {
    json!({
        "name": "download",
        "title": "Browser download",
        "description": "Frozen input/output contract for row B5: download into a caller directory \
            with accept, timeout, maxBytes and filename, returning path, bytes, sha256, mimeType, \
            headers and durationMs, with partial-file cleanup on cancel. Implementation is not \
            present in this scaffold.",
        "inputSchema": {"type": "object", "properties": {
            "dir": {"type": "string"},
            "accept": {"type": "array", "items": {"type": "string"}},
            "timeoutMs": {"type": "integer", "minimum": 0},
            "maxBytes": {"type": "integer", "minimum": 0},
            "filename": {"type": "string"}
        }, "required": ["dir"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "path": {"type": "string"},
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

/// The single typed error every call returns while this row is a stub.
fn not_implemented() -> ToolErr {
    ToolErr::typed(
        "row B5: download is not implemented".to_string(),
        json!({"code": "not_implemented", "tool": "browser_download"}),
    )
}

/// Dispatcher entry point for the B5 fill lane. Intended one-line arm:
/// `crate::download::handle(browser, args).await`.
pub async fn handle(_browser: &Browser, _args: &Value) -> Result<Value, ToolErr> {
    Err(not_implemented())
}

#[cfg(test)]
mod tests {
    use super::{not_implemented, ROW};

    #[test]
    fn b5_stub_returns_typed_not_implemented_naming_row() {
        let env = not_implemented().envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
    }
}
