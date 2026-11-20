//! Audit contract for row B7.
//!
//! Private home for the `browser_audit` tool: a viewport matrix with overflow,
//! contrast, focus, target size and other checks, returning one evidence
//! record per viewport. This is a scaffold. [`handle`] returns a typed
//! `not_implemented` error until the B7 fill lane adds the checks.

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::ToolErr;

/// Row id this module owns.
pub const ROW: &str = "B7";

/// Frozen input/output contract for row B7 (`browser_audit`).
/// Implementation is not present in this scaffold.
pub fn schema() -> Value {
    json!({
        "name": "audit",
        "title": "Browser audit",
        "description": "Frozen input/output contract for row B7: an audit over the requested \
            viewports and checks, returning one record per viewport with width, scrollWidth, \
            overflow, contrast, focus, targetSize and verdict. Implementation is not present in \
            this scaffold.",
        "inputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "integer", "minimum": 0}},
            "checks": {"type": "array", "items": {"type": "string"}}
        }, "required": ["viewports", "checks"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "object", "properties": {
                "width": {"type": "integer", "minimum": 0},
                "scrollWidth": {"type": "integer", "minimum": 0},
                "overflow": {},
                "contrast": {},
                "focus": {},
                "targetSize": {},
                "verdict": {"type": "boolean"}
            }, "required": ["width", "scrollWidth", "overflow", "contrast", "focus", "targetSize", "verdict"]}}
        }, "required": ["viewports"], "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}
    })
}

/// The single typed error every call returns while this row is a stub.
fn not_implemented() -> ToolErr {
    ToolErr::typed(
        "row B7: audit is not implemented".to_string(),
        json!({"code": "not_implemented", "tool": "browser_audit"}),
    )
}

/// Dispatcher entry point for the B7 fill lane. Intended one-line arm:
/// `crate::audit::handle(browser, args).await`.
pub async fn handle(_browser: &Browser, _args: &Value) -> Result<Value, ToolErr> {
    Err(not_implemented())
}

#[cfg(test)]
mod tests {
    use super::{not_implemented, ROW};

    #[test]
    fn b7_stub_returns_typed_not_implemented_naming_row() {
        let env = not_implemented().envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
    }
}
