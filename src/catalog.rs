//! The one `hand` catalog: the browser and desktop tool sets, namespaced by prefix.
//!
//! The prefix is the only change to a tool entry. Every schema, description,
//! annotation, output schema and result body stays the pinned source's, so a
//! client has a one-to-one mapping: `hand` = `browser_` + Gladius name, or
//! `desktop_` + Puppet name.

use serde_json::{json, Value};

pub const BROWSER_PREFIX: &str = "browser_";
pub const DESKTOP_PREFIX: &str = "desktop_";

/// The 25 Gladius tools, in catalog order, followed by hand's own additions:
/// `context` (isolated contexts) and the two pointer primitives that had no
/// legacy tool (`wheel`, `drag`).
pub const BROWSER_TOOLS: [&str; 28] = [
    "navigate",
    "snapshot",
    "screenshot",
    "click",
    "type",
    "evaluate",
    "wait_for",
    "targets",
    "dialog",
    "history",
    "network",
    "console",
    "storage",
    "tabs",
    "viewport",
    "select",
    "hover",
    "key",
    "upload",
    "layout",
    "assert_layout",
    "visual",
    "cdp",
    "close",
    "fetch_text",
    "context",
    "wheel",
    "drag",
];

/// The 7 Puppet tools, in catalog order.
pub const DESKTOP_TOOLS: [&str; 7] = [
    "screenshot",
    "diff",
    "list_windows",
    "window",
    "do",
    "cursor",
    "elements",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    Browser,
    Desktop,
}

/// Split a prefixed name into its domain and the legacy tool name. `None` when
/// no known prefix matches: hand never guesses a domain.
pub fn split(name: &str) -> Option<(Domain, &str)> {
    if let Some(rest) = name.strip_prefix(BROWSER_PREFIX) {
        if !rest.is_empty() {
            return Some((Domain::Browser, rest));
        }
    }
    if let Some(rest) = name.strip_prefix(DESKTOP_PREFIX) {
        if !rest.is_empty() {
            return Some((Domain::Desktop, rest));
        }
    }
    None
}

/// One-line descriptions for six entries; schemas, titles and bodies stay pinned.
fn present(tool: &mut Value) {
    let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
        return;
    };
    let short = match name {
        "browser_visual" => {
            "Deterministic visual-regression baselines under GLADIUS_VISUAL_DIR; requires action."
        }
        "browser_assert_layout" => {
            "Prove element geometry without pixels using checks on ref and/or selector targets."
        }
        "desktop_screenshot" => {
            "Capture the virtual desktop, one monitor, a window or a region without focusing; scope with window or target."
        }
        "desktop_elements" => {
            "List a window's UI Automation elements as JSON without pixels or input; pass target or hwnd."
        }
        "desktop_do" => {
            "Run an ordered actions[] batch against one locked window; requires actions."
        }
        "desktop_diff" => "Pixel-diff two captures; requires base and next.",
        _ => return,
    };
    if let Some(obj) = tool.as_object_mut() {
        obj.insert("description".to_string(), Value::String(short.to_string()));
    }
}

fn prefixed(prefix: &str, tools: Value) -> Vec<Value> {
    tools
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|list| {
            list.iter()
                .map(|tool| {
                    let mut tool = tool.clone();
                    if let Some(obj) = tool.as_object_mut() {
                        if let Some(legacy) = obj.get("name").and_then(|n| n.as_str()) {
                            obj.insert(
                                "name".to_string(),
                                Value::String(format!("{prefix}{legacy}")),
                            );
                        }
                    }
                    present(&mut tool);
                    tool
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The combined catalog: `browser_*` then `desktop_*`.
pub fn combined_list() -> Value {
    let mut tools = prefixed(BROWSER_PREFIX, browser_engine::list());
    tools.extend(prefixed(
        BROWSER_PREFIX,
        json!({ "tools": [browser_engine::contexts::tool_schema()] }),
    ));
    tools.extend(prefixed(DESKTOP_PREFIX, desktop_engine::tools_list()));
    json!({ "tools": tools })
}

/// The names this build actually publishes, for startup checks and tests.
pub fn catalog_names() -> Vec<String> {
    combined_list()
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert the compatibility floor before serving. A missing or duplicated
/// tool is a build defect, not a runtime condition: hand refuses to publish a
/// catalog it cannot map one-to-one.
pub fn validate() -> Result<(), String> {
    let names = catalog_names();
    let mut expected: Vec<String> = BROWSER_TOOLS
        .iter()
        .map(|n| format!("{BROWSER_PREFIX}{n}"))
        .chain(DESKTOP_TOOLS.iter().map(|n| format!("{DESKTOP_PREFIX}{n}")))
        .collect();
    expected.sort();
    let mut got = names.clone();
    got.sort();
    if got != expected {
        let missing: Vec<&String> = expected.iter().filter(|n| !got.contains(n)).collect();
        let extra: Vec<&String> = got.iter().filter(|n| !expected.contains(n)).collect();
        return Err(format!(
            "catalog does not match the compatibility floor: {} tools, missing {:?}, extra {:?}",
            got.len(),
            missing,
            extra
        ));
    }
    for name in &got {
        let (domain, legacy) = split(name).ok_or_else(|| format!("unprefixed tool {name}"))?;
        let known = match domain {
            Domain::Browser => BROWSER_TOOLS.contains(&legacy),
            Domain::Desktop => DESKTOP_TOOLS.contains(&legacy),
        };
        if !known {
            return Err(format!("unknown tool {name}"));
        }
    }
    Ok(())
}
