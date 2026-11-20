//! One MCP server, both engine catalogs, honest availability.
//!
//! These tests use the real executable over stdio and compare every published
//! floor tool body against the committed baseline (`baseline/hand-catalog.json`).
//! The only allowed differences are the domain prefix, the properties listed
//! under `inputSchema`/`outputSchema` in `baseline/declared-additions.json`, and
//! the descriptions listed under `metadata.description` there. A changed schema,
//! annotation, title or undeclared description fails here.

mod common;

use common::{text_of, tool, Session};
use serde_json::{json, Value};

const BROWSER_TOOLS: [&str; 29] = [
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
    "form",
];

const DESKTOP_TOOLS: [&str; 7] = [
    "screenshot",
    "diff",
    "list_windows",
    "window",
    "do",
    "cursor",
    "elements",
];

#[test]
fn initialize_identifies_version_platform_and_both_engines() {
    let mut s = Session::spawn();
    let resp = s.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {},
               "clientInfo": {"name": "probe", "version": "0"}}),
    );
    let result = resp.get("result").expect("initialize result");
    assert_eq!(
        result.get("protocolVersion").and_then(|v| v.as_str()),
        Some("2025-11-25")
    );
    let info = &result["serverInfo"];
    assert_eq!(info["name"].as_str(), Some("hand"));
    assert!(!info["version"].as_str().unwrap_or("").is_empty());
    assert_eq!(info["platform"].as_str(), Some("windows-x64"));
    assert_eq!(
        info["count"].as_u64(),
        Some((BROWSER_TOOLS.len() + DESKTOP_TOOLS.len()) as u64)
    );

    let browser = &info["engines"]["browser"];
    assert_eq!(browser["engine"].as_str(), Some("gladius"));
    assert_eq!(browser["sourcePin"].as_str(), Some("59c6d77"));
    assert_eq!(browser["version"].as_str(), Some("0.8.0"));
    assert_eq!(
        browser["toolCount"].as_u64(),
        Some((BROWSER_TOOLS.len() - 1) as u64),
        "the browser engine counts its own list, one below the published browser_* bodies"
    );
    assert!(
        browser["backend"].is_string(),
        "browser backend must be named: {browser}"
    );

    let desktop = &info["engines"]["desktop"];
    assert_eq!(desktop["engine"].as_str(), Some("puppet"));
    assert_eq!(desktop["sourcePin"].as_str(), Some("0f790be"));
    assert_eq!(desktop["version"].as_str(), Some("8.0.0"));
    assert_eq!(desktop["available"].as_bool(), Some(true));
    assert_eq!(desktop["toolCount"].as_u64(), Some(7));

    let instructions = result["instructions"].as_str().expect("instructions");
    for needle in [
        "browser_snapshot",
        "desktop_",
        "never the foreground window",
        "outcome:unknown, retry:unsafe",
        "typed tool results",
    ] {
        assert!(
            instructions.contains(needle),
            "instructions must name {needle:?}: {instructions}"
        );
    }
}

#[test]
fn catalog_is_the_floor_plus_hand_additions() {
    let mut s = Session::spawn();
    let tools = s.list_tools();
    let expected = BROWSER_TOOLS.len() + DESKTOP_TOOLS.len();
    assert_eq!(
        tools.len(),
        expected,
        "catalog must publish all {expected} tools"
    );
    let mut names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), expected, "tool names must be unique");

    for legacy in BROWSER_TOOLS {
        let name = format!("browser_{legacy}");
        assert!(names.contains(&name), "missing {name}");
        let t = tool(&tools, &name);
        assert_eq!(t["name"].as_str(), Some(name.as_str()));
        assert!(t["description"].as_str().unwrap_or("").len() > 10);
        assert!(t["inputSchema"].is_object(), "{name} inputSchema");
    }
    for legacy in DESKTOP_TOOLS {
        let name = format!("desktop_{legacy}");
        assert!(names.contains(&name), "missing {name}");
        let t = tool(&tools, &name);
        assert!(t["inputSchema"].is_object(), "{name} inputSchema");
    }
    assert!(
        !names
            .iter()
            .any(|n| !n.starts_with("browser_") && !n.starts_with("desktop_")),
        "every name must be namespaced: {names:?}"
    );
}

/// The pinned compatibility floor as committed by the baseline generator, and
/// the one machine-readable list of properties `hand` adds on purpose. The
/// executable gate reads the same two files as `baseline/scripts/compare-catalog.mjs`,
/// so the script and the test cannot disagree about what is allowed.
const FLOOR_CATALOG: &str = include_str!("../baseline/hand-catalog.json");
const DECLARED_ADDITIONS: &str = include_str!("../baseline/declared-additions.json");

fn declared_paths(declared: &Value, schema: &str, tool: &str) -> Vec<String> {
    declared[schema][tool]
        .as_array()
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Tool names whose `description` this tree deliberately rewrote, as declared
/// under `metadata.description`. A declared name must DIFFER from the pin: the
/// declaration says the rewrite happened, so an unchanged string is stale.
fn declared_descriptions(declared: &Value) -> Vec<String> {
    declared["metadata"]["description"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// True when the dotted path resolves to a key that exists in `schema`.
fn has_path(schema: &Value, path: &str) -> bool {
    let mut node = schema;
    for segment in path.split('.') {
        match node.get(segment) {
            Some(next) => node = next,
            None => return false,
        }
    }
    true
}

fn chain_mut<'a>(mut node: &'a mut Value, segments: &[&str]) -> Option<&'a mut Value> {
    for segment in segments {
        node = node.as_object_mut()?.get_mut(*segment)?;
    }
    Some(node)
}

/// Remove one declared addition from a copy of the published schema.
fn strip_path(schema: &mut Value, path: &str) {
    let segments: Vec<&str> = path.split('.').collect();
    let Some((key, parents)) = segments.split_last() else {
        return;
    };
    if let Some(parent) = chain_mut(schema, parents) {
        if let Some(object) = parent.as_object_mut() {
            object.remove(*key);
        }
    }
    if parents.last() == Some(&"properties") {
        if let Some(holder) = chain_mut(schema, &parents[..parents.len() - 1]) {
            if let Some(required) = holder.get_mut("required").and_then(|value| value.as_array_mut()) {
                required.retain(|entry| entry.as_str() != Some(*key));
            }
        }
    }
}

/// Every field of every floor tool body is the pinned one, except `name` (which
/// gained the prefix), the properties listed under `inputSchema`/`outputSchema`
/// in `baseline/declared-additions.json`, and the descriptions listed under
/// `metadata.description` there. This is the executable compatibility inventory
/// for the catalog.
///
/// The reference is the committed floor, not `browser_engine::list()`: the
/// engine is the same source that publishes hand's bodies, so it cannot go red
/// on a schema change. The floor can, and a deleted allowance must.
#[test]
fn tool_bodies_are_engine_bodies_under_the_prefix() {
    let mut s = Session::spawn();
    let published = s.list_tools();

    let floor: Value = serde_json::from_str(FLOOR_CATALOG).expect("baseline/hand-catalog.json parses");
    let declared: Value =
        serde_json::from_str(DECLARED_ADDITIONS).expect("baseline/declared-additions.json parses");
    assert_eq!(declared["version"], json!(1), "the allowance file is versioned");

    if let Some(metadata) = declared["metadata"].as_object() {
        for key in metadata.keys() {
            assert_eq!(
                key, "description",
                "baseline/declared-additions.json: metadata may only declare description"
            );
        }
    }
    let declared_descriptions = declared_descriptions(&declared);

    let floor_tools = floor["tools"].as_array().expect("floor tools");
    assert_eq!(floor_tools.len(), 32, "the committed floor is 32 tools");

    for want in floor_tools {
        let name = want["name"].as_str().expect("floor tool name");
        let got = tool(&published, name);
        assert_eq!(got["name"].as_str(), Some(name), "{name} body drifted: name");

        for key in ["inputSchema", "outputSchema"] {
            let paths = declared_paths(&declared, key, name);
            for path in &paths {
                assert!(
                    has_path(&got[key], path),
                    "{name} {key}: {path} is declared in baseline/declared-additions.json \
                     but absent from the published body"
                );
            }
            let mut stripped = got[key].clone();
            for path in &paths {
                strip_path(&mut stripped, path);
            }
            assert_eq!(
                stripped, want[key],
                "{name} {key} drifted from baseline/hand-catalog.json beyond the declared additions"
            );
        }

        assert_eq!(
            got["annotations"], want["annotations"],
            "{name} annotations drifted"
        );
        assert_eq!(
            got["title"], want["title"],
            "{name} title drifted from baseline/hand-catalog.json: title has no declared exceptions"
        );

        if declared_descriptions.iter().any(|declared| declared == name) {
            assert_ne!(
                got["description"], want["description"],
                "{name} description is declared as rewritten in \
                 baseline/declared-additions.json but is identical to the pin: a stale declaration"
            );
        } else {
            assert_eq!(
                got["description"], want["description"],
                "{name} description drifted from baseline/hand-catalog.json and is not \
                 declared in baseline/declared-additions.json"
            );
        }
    }

    let floor_names: Vec<&str> = floor_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in &declared_descriptions {
        assert!(
            floor_names.contains(&name.as_str()),
            "{name} is declared as a rewritten description but is not a floor tool"
        );
    }
}

#[test]
fn ping_is_an_empty_result_and_unknown_methods_are_protocol_errors() {
    let mut s = Session::spawn();
    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}), "ping result must be {{}}");

    let missing = s.request("no/such/method", json!({}));
    assert_eq!(missing["error"]["code"], json!(-32601));
    assert!(missing.get("result").is_none());
}

#[test]
fn unknown_tool_is_a_typed_tool_error_and_the_session_survives() {
    let mut s = Session::spawn();
    let resp = s.request(
        "tools/call",
        json!({"name": "navigate", "arguments": {"url": "about:blank"}}),
    );
    assert!(
        resp.get("result").is_some(),
        "an unnamespaced tool must be a tool result, never a dropped request: {resp}"
    );
    assert_eq!(resp["result"]["isError"], json!(true));
    assert_eq!(
        resp["result"]["structuredContent"]["code"].as_str(),
        Some("invalid_args")
    );
    assert_eq!(
        resp["result"]["structuredContent"]["reason"].as_str(),
        Some("unknown_tool")
    );
    let text = text_of(&resp);
    assert!(
        text.contains("browser_") && text.contains("desktop_"),
        "{text}"
    );

    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
}

#[test]
fn modern_peer_gets_result_type_and_a_bad_version_is_refused_before_dispatch() {
    let mut s = Session::spawn();
    let modern = s.request(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}),
    );
    assert_eq!(modern["result"]["resultType"].as_str(), Some("complete"));
    assert_eq!(
        modern["result"]["tools"].as_array().map(Vec::len),
        Some(BROWSER_TOOLS.len() + DESKTOP_TOOLS.len())
    );

    let refused = s.request(
        "tools/list",
        json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "1999-01-01"}}),
    );
    assert_eq!(refused["error"]["code"], json!(-32022));
    let supported = refused["error"]["data"]["supported"]
        .as_array()
        .expect("supported versions");
    assert_eq!(supported.len(), 3);
    assert_eq!(
        refused["error"]["data"]["requested"].as_str(),
        Some("1999-01-01")
    );
}

#[test]
fn discovery_reports_supported_versions_and_the_same_identity() {
    let mut s = Session::spawn();
    let resp = s.request("server/discover", json!({}));
    let result = resp.get("result").expect("discover result");
    assert_eq!(result["resultType"].as_str(), Some("complete"));
    assert_eq!(result["serverInfo"]["name"].as_str(), Some("hand"));
    let versions = result["supportedVersions"]
        .as_array()
        .expect("supported versions");
    assert_eq!(versions.len(), 3);
    assert!(versions.contains(&json!("2026-07-28")));
}
