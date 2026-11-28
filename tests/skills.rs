mod common;

use common::Session;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

#[test]
fn skills_extension_advertised() {
    let mut s = Session::spawn();
    // initialize response already received in Session::spawn; re-issue to inspect caps
    let resp = s.request(
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    let caps = resp["result"]["capabilities"].clone();
    assert!(
        caps.get("resources").is_some(),
        "resources capability missing: {resp}"
    );
    assert!(
        caps.get("extensions")
            .and_then(|e| e.get("io.modelcontextprotocol/skills"))
            .is_some(),
        "skills extension missing: {resp}"
    );
}

#[test]
fn skills_list_and_read() {
    let mut s = Session::spawn();
    let resp = s.request("skills/list", json!({}));
    let result = resp.get("result").expect("result");
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["ttlMs"], 0);
    assert_eq!(result["cacheScope"], "private");
    let skills = result["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1);
    let skill = &skills[0];
    assert_eq!(skill["uri"], "skill://routing/SKILL.md");
    assert_eq!(skill["frontmatter"]["name"], "routing");
    let desc = skill["frontmatter"]["description"].as_str().unwrap();
    assert!(!desc.is_empty() && desc.len() <= 1024);

    // digest/size check against on-disk bytes
    let manifest = env!("CARGO_MANIFEST_DIR");
    let bytes = fs::read(format!("{}/skills/routing/SKILL.md", manifest)).expect("read skill");
    let mut h = Sha256::new();
    h.update(&bytes);
    let hex = format!("{:x}", h.finalize());
    let digest = format!("sha256:{}", hex);

    let resources = skill["resources"].as_array().expect("resources");
    // sorted by uri
    let uris: Vec<&str> = resources.iter().filter_map(|r| r["uri"].as_str()).collect();
    let mut sorted = uris.clone();
    sorted.sort();
    assert_eq!(uris, sorted, "resources not sorted");
    // includes SKILL.md
    let skill_md_res = resources
        .iter()
        .find(|r| r["uri"] == "skill://routing/SKILL.md")
        .expect("SKILL.md resource");
    assert_eq!(skill_md_res["digest"], digest);
    assert_eq!(skill_md_res["size"], bytes.len() as u64);

    // resources/read returns same bytes
    let read_resp = s.request("resources/read", json!({"uri": "skill://routing/SKILL.md"}));
    assert_eq!(read_resp["result"]["resultType"], "complete");
    assert_eq!(read_resp["result"]["ttlMs"], 0);
    assert_eq!(read_resp["result"]["cacheScope"], "private");
    let contents = read_resp["result"]["contents"]
        .as_array()
        .expect("contents");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "skill://routing/SKILL.md");
    assert_eq!(contents[0]["mimeType"], "text/markdown");
    assert_eq!(
        contents[0]["text"].as_str().unwrap().as_bytes(),
        bytes.as_slice()
    );

    // skills/get
    let get_resp = s.request("skills/get", json!({"uri": "skill://routing/SKILL.md"}));
    assert_eq!(
        get_resp["result"]["skill"]["uri"],
        "skill://routing/SKILL.md"
    );
    assert_eq!(get_resp["result"]["resultType"], "complete");
    assert_eq!(get_resp["result"]["ttlMs"], 0);
    assert_eq!(get_resp["result"]["cacheScope"], "private");

    // unknown skill uri -> -32602
    let bad = s.request("skills/get", json!({"uri": "skill://nope/SKILL.md"}));
    assert_eq!(bad["error"]["code"], -32602);
}

#[test]
fn skills_get_unknown_returns_error() {
    let mut s = Session::spawn();
    let bad = s.request(
        "skills/get",
        json!({"uri": "skill://does-not-exist/SKILL.md"}),
    );
    assert_eq!(bad["error"]["code"], -32602);
}

/// The provider rejects a function whose *root* parameters schema is not a
/// plain object. Enumerate the live catalog rather than a hard-coded list so a
/// new tool cannot reintroduce the defect. Root combinators were lifted into a
/// reserved property; their content must survive for the tools that had them.
#[test]
fn every_advertised_tool_schema_has_a_valid_root() {
    let mut s = Session::spawn();
    let tools = s.list_tools();
    assert!(
        tools.len() >= 44,
        "expected the published catalog, got {} tools",
        tools.len()
    );
    let forbidden = ["oneOf", "anyOf", "allOf", "enum", "const", "not"];
    for tool in &tools {
        let name = tool["name"].as_str().unwrap_or("<unnamed>");
        let schema = &tool["inputSchema"];
        assert_eq!(
            schema["type"].as_str(),
            Some("object"),
            "{name}: the root inputSchema must be an object"
        );
        for key in forbidden {
            assert!(
                schema.get(key).is_none(),
                "{name}: top-level {key} invalidates every tool schema: {schema}"
            );
        }
    }
    let find = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("tools/list omitted {name}"))
    };
    assert!(
        find("browser_assert_layout")["inputSchema"]["properties"]["__argumentConstraints"]
            ["anyOf"]
            .is_array(),
        "assert_layout's ref/selector rule must survive the relocation"
    );
    assert!(
        find("desktop_window")["inputSchema"]["properties"]["__argumentConstraints"]["allOf"]
            .is_array(),
        "window's move/resize rule must survive the relocation"
    );
}

#[test]
fn resources_list_answers_with_embedded_skill_resources() {
    let mut s = Session::spawn();
    let resp = s.request("resources/list", json!({}));
    assert!(
        resp.get("error").is_none(),
        "resources/list is advertised and must be implemented: {resp}"
    );
    let result = &resp["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["ttlMs"], 0);
    assert_eq!(result["cacheScope"], "private");
    let resources = result["resources"].as_array().expect("resources array");
    assert!(!resources.is_empty(), "embedded skills must be listed");
    for resource in resources {
        for key in ["uri", "name", "mimeType"] {
            assert!(
                resource[key].as_str().is_some_and(|v| !v.is_empty()),
                "{key} is required on every listed resource: {resource}"
            );
        }
    }
    let skill_md = resources
        .iter()
        .find(|r| r["uri"] == "skill://routing/SKILL.md")
        .expect("SKILL.md resource");
    assert_eq!(skill_md["name"], "SKILL.md");
    assert_eq!(skill_md["mimeType"], "text/markdown");

    let unknown = s.request(
        "resources/list",
        json!({"uri": "skill://does-not-exist/SKILL.md"}),
    );
    assert!(
        unknown["result"]["resources"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "an unknown filter returns an empty array, never a protocol error: {unknown}"
    );
}

/// `initialize` negotiates the peer's supported version. A 2026-07-28 peer
/// stays modern for the rest of the connection even without a per-request
/// `_meta` marker; a legacy peer keeps the older, marker-free result shape.
#[test]
fn negotiated_modern_revision_marks_every_result() {
    let mut s = Session::spawn();
    assert_eq!(
        s.request("ping", json!({}))["result"],
        json!({}),
        "a legacy peer keeps the empty ping result"
    );
    assert!(
        s.request("tools/list", json!({}))["result"]
            .get("resultType")
            .is_none(),
        "a legacy peer must not gain the modern marker"
    );

    let init = s.request(
        "initialize",
        json!({"protocolVersion": "2026-07-28", "capabilities": {},
               "clientInfo": {"name": "t", "version": "0"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(init["result"]["resultType"], "complete");

    for method in [
        "ping",
        "tools/list",
        "server/discover",
        "skills/list",
        "resources/list",
    ] {
        let resp = s.request(method, json!({}));
        assert!(
            resp.get("error").is_none(),
            "{method} failed for a modern peer: {resp}"
        );
        assert_eq!(
            resp["result"]["resultType"], "complete",
            "{method} must carry resultType for a 2026-07-28 peer: {resp}"
        );
    }
    let read = s.request("resources/read", json!({"uri": "skill://routing/SKILL.md"}));
    assert_eq!(read["result"]["resultType"], "complete");
}
