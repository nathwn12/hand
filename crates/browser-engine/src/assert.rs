//! Assertion contract for row B2.
//!
//! Private home for the `browser_assert` tool: DOM, accessibility, text, URL
//! and network assertions with bounded polling, returning expected, observed,
//! verdict, evidence and elapsed time. This is a scaffold. [`handle`] returns
//! a typed `not_implemented` error until the B2 fill lane adds the checks.

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::ToolErr;

/// Row id this module owns.
pub const ROW: &str = "B2";

/// Frozen input/output contract for row B2 (`browser_assert`).
/// Implementation is not present in this scaffold.
pub fn schema() -> Value {
    json!({
        "name": "assert",
        "title": "Browser assert",
        "description": "Frozen input/output contract for row B2: DOM, accessibility, text, URL and \
            network assertions with bounded polling, returning expected, observed, verdict, \
            evidence and elapsedMs. target is required for dom, ax and text, and optional for url \
            and network where the page's own location and last response are the subject. \
            Implementation is not present in this scaffold.",
        "inputSchema": {"type": "object", "properties": {
            "kind": {"type": "string", "enum": ["dom", "ax", "text", "url", "network"]},
            "target": {},
            "expected": {},
            "timeoutMs": {"type": "integer", "minimum": 0},
            "pollMs": {"type": "integer", "minimum": 0}
        }, "required": ["kind", "expected"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "expected": {},
            "observed": {},
            "verdict": {"type": "boolean"},
            "evidence": {"type": "object"},
            "elapsedMs": {"type": "integer", "minimum": 0}
        }, "required": ["expected", "observed", "verdict", "evidence", "elapsedMs"],
        "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}
    })
}

/// The single typed error every call returns while this row is a stub.
fn not_implemented() -> ToolErr {
    ToolErr::typed(
        "row B2: assertion is not implemented".to_string(),
        json!({"code": "not_implemented", "tool": "browser_assert"}),
    )
}

/// Dispatcher entry point for the B2 fill lane. Intended one-line arm:
/// `crate::assert::handle(browser, args).await`.
pub async fn handle(_browser: &Browser, _args: &Value) -> Result<Value, ToolErr> {
    Err(not_implemented())
}

#[cfg(test)]
mod tests {
    use super::{not_implemented, ROW};

    #[test]
    fn b2_stub_returns_typed_not_implemented_naming_row() {
        let env = not_implemented().envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
    }
}
