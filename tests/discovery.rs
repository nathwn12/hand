//! Slice 1: one MCP server, both engine catalogs, honest availability.
//!
//! These tests use the real executable over stdio and compare every published
//! tool body against the engine's own catalog. The only allowed difference is
//! the domain prefix; a changed schema, description, annotation or output
//! schema fails here.

mod common;

use common::{text_of, tool, Session};
use serde_json::{json, Value};

const BROWSER_TOOLS: [&str; 25] = [
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
    assert_eq!(info["count"].as_u64(), Some(32));

    let browser = &info["engines"]["browser"];
    assert_eq!(browser["engine"].as_str(), Some("gladius"));
    assert_eq!(browser["sourcePin"].as_str(), Some("59c6d77"));
    assert_eq!(browser["version"].as_str(), Some("0.8.0"));
    assert_eq!(browser["toolCount"].as_u64(), Some(25));
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
        "never moves the physical pointer",
        "typed",
    ] {
        assert!(
            instructions.contains(needle),
            "instructions must name {needle:?}: {instructions}"
        );
    }
}

#[test]
fn catalog_is_exactly_the_32_prefixed_tools() {
    let mut s = Session::spawn();
    let tools = s.list_tools();
    assert_eq!(tools.len(), 32, "catalog must publish all 32 tools");
    let mut names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 32, "tool names must be unique");

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

/// Every field of every tool body is the engine's, unchanged, except `name`.
/// This is the executable compatibility inventory for the catalog.
#[test]
fn tool_bodies_are_engine_bodies_under_the_prefix() {
    let mut s = Session::spawn();
    let published = s.list_tools();

    let browser = browser_engine::list();
    for legacy in browser["tools"].as_array().expect("browser tools") {
        let legacy_name = legacy["name"].as_str().expect("browser tool name");
        let name = format!("browser_{legacy_name}");
        let got = Value::Object(tool(&published, &name).clone());
        let mut want = legacy.clone();
        want["name"] = Value::String(name.clone());
        assert_eq!(got, want, "browser_{legacy_name} body drifted");
    }

    let desktop = desktop_engine::tools_list();
    for legacy in desktop["tools"].as_array().expect("desktop tools") {
        let legacy_name = legacy["name"].as_str().expect("desktop tool name");
        let name = format!("desktop_{legacy_name}");
        let got = Value::Object(tool(&published, &name).clone());
        let mut want = legacy.clone();
        want["name"] = Value::String(name.clone());
        assert_eq!(got, want, "desktop_{legacy_name} body drifted");
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

    // Still serving.
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
    assert_eq!(modern["result"]["tools"].as_array().map(Vec::len), Some(32));

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
