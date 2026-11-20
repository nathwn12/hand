//! Network interception contract for row B3.
//!
//! Private home for the `browser_network` interception surface: route, mock,
//! abort, offline and restore, scoped to a browser context. This is a scaffold.
//! [`handle`] returns a typed `not_implemented` error until the B3 fill lane
//! adds the route registry and the CDP logic.

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::ToolErr;

/// Row id this module owns.
pub const ROW: &str = "B3";

/// Frozen input/output contract for row B3 (`browser_network` interception).
/// Implementation is not present in this scaffold.
pub fn schema() -> Value {
    json!({
        "name": "network",
        "title": "Browser network interception",
        "description": "Frozen input/output contract for row B3: route, mock, abort, offline and \
            restore, scoped to a browser context. Implementation is not present in this scaffold.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["route", "mock", "abort", "offline", "restore"]},
            "pattern": {"type": "string"},
            "url": {"type": "string"},
            "body": {"type": "string"},
            "status": {"type": "integer", "minimum": 100},
            "headers": {"type": "object", "additionalProperties": {"type": "string"}},
            "context": {"type": "string"},
            "offline": {"type": "boolean"}
        }, "required": ["action"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "ok": {"type": "boolean"},
            "action": {"type": "string"},
            "routeId": {"type": "string"},
            "context": {"type": "string"},
            "removed": {"type": "integer", "minimum": 0},
            "offline": {"type": "boolean"}
        }, "required": ["ok"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
    })
}

/// The single typed error every call returns while this row is a stub.
fn not_implemented() -> ToolErr {
    ToolErr::typed(
        "row B3: network interception is not implemented".to_string(),
        json!({"code": "not_implemented", "tool": "browser_network"}),
    )
}

/// Dispatcher entry point for the B3 fill lane. Intended one-line arm:
/// `crate::net::handle(browser, args).await`.
pub async fn handle(_browser: &Browser, _args: &Value) -> Result<Value, ToolErr> {
    Err(not_implemented())
}

#[cfg(test)]
mod tests {
    use super::{not_implemented, ROW};

    #[test]
    fn b3_stub_returns_typed_not_implemented_naming_row() {
        let env = not_implemented().envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
    }
}
