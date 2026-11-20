//! hand's browser engine: the Gladius 0.8.0 engine (pinned source `59c6d77`)
//! as a library. `browser`, `capture`, `fetch`, `layout` and `visual` are the
//! pinned engine sources; `tools` is the pinned tool table plus the
//! `typed_envelope` shim at its end. hand's own MCP front end owns transport,
//! catalogs and domain routing; this crate owns the CDP browser:
//! launch/attach, snapshots, refs, actions, capture, layout/visual evidence
//! and the legacy typed tool envelopes.
//!
//! Two additions are hand's, not Gladius's, and are listed so the provenance
//! claim stays exact:
//! - `contexts`: named isolated browser contexts (`Target.createBrowserContext`)
//!   with explicit create/select/teardown, cookie/storage isolation and a
//!   public dispatch hook for the `browser_context` tool;
//! - the context-exclusion seam in `browser`: pages of a registered context
//!   are filtered out of the legacy tab/selection space (`page_targets`,
//!   `select_target`), the exclusion map is threaded through every launch and
//!   attach, and `js_thrown` / `js_no_value` / `is_session_death` are
//!   crate-visible so the context module reuses the pinned error shapes
//!   instead of minting copies. Legacy behaviour is unchanged when no context
//!   exists (the exclusion map is empty).
//!
//! Globals shared by the engine and hand:
//! - `GLADIUS_CDP_URL` attaches to an external engine instead of launching.
//! - `GLADIUS_CHROMIUM` / `GLADIUS_CHROMIUM_CACHE` select the owned
//!   `chrome-headless-shell` build.
//!
//! These environment names are the pinned engine's contract; hand does not
//! rename them, so a compatibility baseline measured against Gladius holds.

#![recursion_limit = "256"]

mod browser;
mod capture;
pub mod contexts;
mod fetch;
mod layout;
mod tools;
mod visual;

pub use browser::{backend_info, Browser, EngineOwnership};
pub use contexts::ContextRegistry;
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

/// Upstream identity of the vendored browser engine. Single source for every
/// pin and version hand reports; do not repeat these literals elsewhere.
pub const UPSTREAM_PIN: &str = "59c6d77";
pub const UPSTREAM_VERSION: &str = "0.8.0";

fn provenance_from(info: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "engine": "gladius",
        "sourcePin": UPSTREAM_PIN,
        "version": UPSTREAM_VERSION,
        "available": true,
        "backend": info.get("backend").cloned().unwrap_or(serde_json::Value::Null),
        "toolCount": list().get("tools").and_then(|t| t.as_array()).map(|a| a.len()).unwrap_or(0),
    })
}

/// The vendored engine's provenance, as published by hand's diagnostics and
/// handshake. The engine is credited provenance, not hand's definition.
pub fn provenance() -> serde_json::Value {
    provenance_from(&backend_info())
}

/// One-line engine provenance for the initialization response. Keeps the
/// pinned `caps` and `cdpUrl` fields on top of the shared provenance block.
pub fn engine_info() -> serde_json::Value {
    let backend = backend_info();
    let mut info = provenance_from(&backend);
    if let Some(obj) = info.as_object_mut() {
        obj.insert(
            "caps".to_string(),
            backend.get("caps").cloned().unwrap_or(serde_json::Value::Null),
        );
        obj.insert(
            "cdpUrl".to_string(),
            backend.get("cdpUrl").cloned().unwrap_or(serde_json::Value::Null),
        );
    }
    info
}
