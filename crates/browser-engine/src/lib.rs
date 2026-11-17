//! hand's browser engine: the Gladius 0.8.0 engine (pinned source `59c6d77`)
//! as a library. The modules below are the pinned engine sources, unmodified
//! except for the public surface declared here and the one `typed_envelope`
//! shim at the end of `tools.rs`. hand's own MCP front end owns transport,
//! catalogs and domain routing; this crate owns the CDP browser:
//! launch/attach, snapshots, refs, actions, capture, layout/visual evidence
//! and the legacy typed tool envelopes.
//!
//! Globals shared by the engine and hand:
//! - `GLADIUS_CDP_URL` attaches to an external engine instead of launching.
//! - `GLADIUS_CHROMIUM` / `GLADIUS_CHROMIUM_CACHE` select the owned
//!   `chrome-headless-shell` build.
//! These environment names are the pinned engine's contract; hand does not
//! rename them, so a compatibility baseline measured against Gladius holds.

// The tool table is one large `json!` literal; see Gladius `src/main.rs`.
#![recursion_limit = "256"]

mod browser;
mod capture;
mod fetch;
mod layout;
mod tools;
mod visual;

pub use browser::{backend_info, Browser};
pub use tools::{call, list};

/// Where the owned `chrome-headless-shell` resolves to, or the engine's clear
/// "provision it" error. Read-only: resolving never launches anything.
pub fn chromium_path() -> Result<std::path::PathBuf, String> {
    browser::find_chromium()
}

/// Mint the engine's own typed tool-error envelope for a framework-level
/// failure (budget, cancellation, routing). hand uses this so a protocol-layer
/// failure carries exactly the `{code, reason}` shape the tools produce.
pub fn typed_error(prose: &str, structured: serde_json::Value) -> serde_json::Value {
    tools::typed_envelope(prose, structured)
}

/// One-line engine provenance for the initialization response.
pub fn engine_info() -> serde_json::Value {
    let info = backend_info();
    serde_json::json!({
        "engine": "gladius",
        "sourcePin": "59c6d77",
        "version": "0.8.0",
        "backend": info.get("backend").cloned().unwrap_or(serde_json::Value::Null),
        "caps": info.get("caps").cloned().unwrap_or(serde_json::Value::Null),
        "cdpUrl": info.get("cdpUrl").cloned().unwrap_or(serde_json::Value::Null),
        "toolCount": list().get("tools").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0),
    })
}
