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
