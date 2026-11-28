//! Assertion contract for row B2.
//!
//! Private home for the `browser_assert` tool: DOM, AX, text, URL and network
//! assertions with bounded polling, returning expected, observed, verdict,
//! evidence and elapsed time. The AX kind reads the same
//! `Accessibility.getFullAXTree` tree `browser_snapshot` renders; the network
//! kind attaches to the live CDP ring the `network` tool drains, enabling that
//! capture lazily if it is not already on (observability only, no page
//! mutation). An empty ring is an honest false verdict with a note, never a
//! minted true.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browser::{Browser, NET_CAP};
use crate::tools::{
    check_unknown, ok_text, optional_string_arg, typed_marker, ToolCode, ToolErr, ToolReason,
};

/// Row id this module owns. The stub test reads it; lib code does not.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B2";

/// Frozen input/output contract for row B2 (`browser_assert`).
pub fn schema() -> Value {
    json!({
        "name": "assert",
        "title": "Browser assert",
        "description": "Assert against the live page with bounded polling. kind dom checks a CSS selector or @eN ref for existence, text and attributes; kind ax matches a role and accessible name on the same Accessibility tree browser_snapshot reads (target forms: role=<role>, role=<role>[name=\\\"<name>\\\"], name=\\\"<name>\\\"; expected {role, name}); kind text checks visible text with contains/equals; kind url checks the current location with equals/contains or a small bounded wildcard pattern (^, $, .*). kind network checks the live CDP network ring the network tool drains with expected {url, method, status, failed, minCount} (url is an exact string or {equals, contains, matches} using the same bounded matcher; minCount must be >= 1 and defaults to 1). It attaches to the existing capture ring and enables it lazily if it is not already on (observability only, no page mutation); an empty ring returns a false verdict with a note, never a true one. Every reply carries expected, observed, verdict, evidence and elapsedMs. dom/text/url/network return a boolean verdict; ax returns the string pass or fail. A check still false at the poll budget returns that verdict with the last observed value, not a timeout. context=<name> asserts against a page of that NAMED isolated browser context instead of the legacy page (without page=, a context holding more than one page is a typed invalid_args/ambiguous_target listing every targetId; an unknown context or page is a typed invalid_target/not_found and never falls back to the legacy page). A @eN ref is legacy-page bound, so target= with context= must be a CSS or AX selector.",
        "inputSchema": {"type": "object", "properties": {
            "kind": {"type": "string", "enum": ["dom", "ax", "text", "url", "network"]},
            "target": {},
            "expected": {},
            "timeoutMs": {"type": "integer", "minimum": 0},
            "pollMs": {"type": "integer", "minimum": 0},
            "context": {"type": "string", "description": "assert against a page of this named isolated browser context instead of the legacy page"},
            "page": {"type": "string", "description": "targetId of the context page to assert against; required when the context holds more than one page"}
        }, "required": ["kind", "expected"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "expected": {},
            "observed": {},
            "verdict": {"type": ["boolean", "string"]},
            "evidence": {"type": "object"},
            "elapsedMs": {"type": "integer", "minimum": 0}
        }, "required": ["expected", "observed", "verdict", "evidence", "elapsedMs"],
        "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}
    })
}

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_assert", "arg": arg}),
    )
}

/// Validate `network` expectation fields at the call site: a bad or missing
/// field is a typed `invalid_args` that NAMES the field, before any browser
/// read happens.
fn validate_network_expected(expected: &Value) -> Result<(), ToolErr> {
    let obj = expected.as_object().ok_or_else(|| {
        invalid(
            "browser_assert: network 'expected' must be an object with {url, method, status, failed, minCount}".to_string(),
            ToolReason::BadValue,
            "expected",
        )
    })?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "url" | "method" | "status" | "failed" | "minCount"
        ) {
            return Err(invalid(
                format!("browser_assert: network 'expected.{key}' is not a supported field"),
                ToolReason::BadValue,
                &format!("expected.{key}"),
            ));
        }
    }
    if let Some(u) = obj.get("url") {
        match u {
            Value::String(s) if s.is_empty() => {
                return Err(invalid(
                    "browser_assert: network 'expected.url' must not be empty".to_string(),
                    ToolReason::BadValue,
                    "expected.url",
                ));
            }
            Value::String(_) => {}
            Value::Object(_) => {
                for key in u.as_object().expect("url is an object").keys() {
                    if !matches!(key.as_str(), "equals" | "contains" | "matches") {
                        return Err(invalid(
                            format!("browser_assert: network 'expected.url.{key}' is not a supported matcher"),
                            ToolReason::BadValue,
                            &format!("expected.url.{key}"),
                        ));
                    }
                }
                let mut any = false;
                for key in ["equals", "contains", "matches"] {
                    if let Some(v) = u.get(key) {
                        any = true;
                        if v.as_str().is_none() {
                            return Err(invalid(
                                format!(
                                    "browser_assert: network 'expected.url.{key}' must be a string"
                                ),
                                ToolReason::BadValue,
                                &format!("expected.url.{key}"),
                            ));
                        }
                    }
                }
                if !any {
                    return Err(invalid(
                        "browser_assert: network 'expected.url' object needs one of equals, contains, matches".to_string(),
                        ToolReason::BadValue,
                        "expected.url",
                    ));
                }
            }
            _ => {
                return Err(invalid(
                    "browser_assert: network 'expected.url' must be a string or an object {equals, contains, matches}".to_string(),
                    ToolReason::BadValue,
                    "expected.url",
                ));
            }
        }
    }
    if let Some(m) = obj.get("method") {
        if m.as_str().is_none() || m.as_str() == Some("") {
            return Err(invalid(
                "browser_assert: network 'expected.method' must be a non-empty string".to_string(),
                ToolReason::BadValue,
                "expected.method",
            ));
        }
    }
    if let Some(s) = obj.get("status") {
        if s.as_i64().is_none() {
            return Err(invalid(
                "browser_assert: network 'expected.status' must be an integer".to_string(),
                ToolReason::BadValue,
                "expected.status",
            ));
        }
    }
    if let Some(f) = obj.get("failed") {
        if f.as_bool().is_none() {
            return Err(invalid(
                "browser_assert: network 'expected.failed' must be a boolean".to_string(),
                ToolReason::BadValue,
                "expected.failed",
            ));
        }
    }
    if let Some(c) = obj.get("minCount") {
        match c.as_u64() {
            Some(n) if n >= 1 => {}
            _ => {
                return Err(invalid(
                    "browser_assert: network 'expected.minCount' must be an integer >= 1"
                        .to_string(),
                    ToolReason::BadValue,
                    "expected.minCount",
                ));
            }
        }
    }
    Ok(())
}

/// Validate `expected` for every non-network kind before any browser read.
/// An unknown key is refused rather than silently ignored: a typo such as
/// `{"text": "zzz"}` must never degrade into an empty needle that matches
/// everything. Network keeps its own validator at the call site.
fn validate_expected(kind: &str, expected: &Value) -> Result<(), ToolErr> {
    match kind {
        "text" => match expected {
            Value::String(s) if s.is_empty() => Err(invalid(
                "browser_assert: text 'expected' string must not be empty (an empty needle matches everything)".to_string(),
                ToolReason::BadValue,
                "expected",
            )),
            Value::String(_) => Ok(()),
            Value::Object(obj) => {
                for key in obj.keys() {
                    if !matches!(key.as_str(), "contains" | "equals") {
                        return Err(invalid(
                            format!("browser_assert: text 'expected.{key}' is not a supported field"),
                            ToolReason::BadValue,
                            &format!("expected.{key}"),
                        ));
                    }
                }
                if obj.is_empty() {
                    return Err(invalid(
                        "browser_assert: text 'expected' needs one of contains or equals".to_string(),
                        ToolReason::MissingArg,
                        "expected",
                    ));
                }
                for key in ["contains", "equals"] {
                    if let Some(v) = obj.get(key) {
                        match v.as_str() {
                            Some(s) if key == "contains" && s.is_empty() => {
                                return Err(invalid(
                                    "browser_assert: text 'expected.contains' must not be empty (an empty needle matches everything)".to_string(),
                                    ToolReason::BadValue,
                                    "expected.contains",
                                ));
                            }
                            Some(_) => {}
                            None => {
                                return Err(invalid(
                                    format!("browser_assert: text 'expected.{key}' must be a string"),
                                    ToolReason::BadValue,
                                    &format!("expected.{key}"),
                                ));
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => Err(invalid(
                "browser_assert: text 'expected' must be a string or an object {contains, equals}".to_string(),
                ToolReason::BadValue,
                "expected",
            )),
        },
        "url" => match expected {
            Value::String(_) => Ok(()),
            Value::Object(obj) => {
                for key in obj.keys() {
                    if !matches!(key.as_str(), "equals" | "contains" | "matches") {
                        return Err(invalid(
                            format!("browser_assert: url 'expected.{key}' is not a supported field"),
                            ToolReason::BadValue,
                            &format!("expected.{key}"),
                        ));
                    }
                }
                if obj.is_empty() {
                    return Err(invalid(
                        "browser_assert: url 'expected' needs one of equals, contains or matches".to_string(),
                        ToolReason::MissingArg,
                        "expected",
                    ));
                }
                for key in ["equals", "contains", "matches"] {
                    if let Some(v) = obj.get(key) {
                        match v.as_str() {
                            Some(s) if matches!(key, "contains" | "matches") && s.is_empty() => {
                                return Err(invalid(
                                    format!("browser_assert: url 'expected.{key}' must not be empty"),
                                    ToolReason::BadValue,
                                    &format!("expected.{key}"),
                                ));
                            }
                            Some(_) => {}
                            None => {
                                return Err(invalid(
                                    format!("browser_assert: url 'expected.{key}' must be a string"),
                                    ToolReason::BadValue,
                                    &format!("expected.{key}"),
                                ));
                            }
                        }
                    }
                }
                Ok(())
            }
            _ => Err(invalid(
                "browser_assert: url 'expected' must be a string or an object {equals, contains, matches}".to_string(),
                ToolReason::BadValue,
                "expected",
            )),
        },
        "dom" => {
            let obj = expected.as_object().ok_or_else(|| {
                invalid(
                    "browser_assert: dom 'expected' must be an object".to_string(),
                    ToolReason::BadValue,
                    "expected",
                )
            })?;
            for key in obj.keys() {
                if !matches!(key.as_str(), "exists" | "text" | "containsText" | "attribute") {
                    return Err(invalid(
                        format!("browser_assert: dom 'expected.{key}' is not a supported field"),
                        ToolReason::BadValue,
                        &format!("expected.{key}"),
                    ));
                }
            }
            if let Some(v) = obj.get("exists") {
                if v.as_bool().is_none() {
                    return Err(invalid(
                        "browser_assert: dom 'expected.exists' must be a boolean".to_string(),
                        ToolReason::BadValue,
                        "expected.exists",
                    ));
                }
            }
            if let Some(v) = obj.get("text") {
                if v.as_str().is_none() {
                    return Err(invalid(
                        "browser_assert: dom 'expected.text' must be a string".to_string(),
                        ToolReason::BadValue,
                        "expected.text",
                    ));
                }
            }
            if let Some(v) = obj.get("containsText") {
                if !v.as_str().is_some_and(|s| !s.is_empty()) {
                    return Err(invalid(
                        "browser_assert: dom 'expected.containsText' must be a non-empty string".to_string(),
                        ToolReason::BadValue,
                        "expected.containsText",
                    ));
                }
            }
            if let Some(v) = obj.get("attribute") {
                let attr = v.as_object().ok_or_else(|| {
                    invalid(
                        "browser_assert: dom 'expected.attribute' must be an object {name, value?}".to_string(),
                        ToolReason::BadValue,
                        "expected.attribute",
                    )
                })?;
                for key in attr.keys() {
                    if !matches!(key.as_str(), "name" | "value") {
                        return Err(invalid(
                            format!("browser_assert: dom 'expected.attribute.{key}' is not a supported field"),
                            ToolReason::BadValue,
                            &format!("expected.attribute.{key}"),
                        ));
                    }
                }
                if !attr.get("name").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                    return Err(invalid(
                        "browser_assert: dom 'expected.attribute.name' must be a non-empty string".to_string(),
                        ToolReason::BadValue,
                        "expected.attribute.name",
                    ));
                }
                if let Some(v) = attr.get("value") {
                    if v.as_str().is_none() {
                        return Err(invalid(
                            "browser_assert: dom 'expected.attribute.value' must be a string".to_string(),
                            ToolReason::BadValue,
                            "expected.attribute.value",
                        ));
                    }
                }
            }
            Ok(())
        }
        "ax" => match expected {
            Value::String(_) => Ok(()),
            Value::Object(obj) => {
                for key in obj.keys() {
                    if !matches!(key.as_str(), "role" | "name") {
                        return Err(invalid(
                            format!("browser_assert: ax 'expected.{key}' is not a supported field"),
                            ToolReason::BadValue,
                            &format!("expected.{key}"),
                        ));
                    }
                }
                for key in ["role", "name"] {
                    if let Some(v) = obj.get(key) {
                        if !v.as_str().is_some_and(|s| !s.is_empty()) {
                            return Err(invalid(
                                format!("browser_assert: ax 'expected.{key}' must be a non-empty string"),
                                ToolReason::BadValue,
                                &format!("expected.{key}"),
                            ));
                        }
                    }
                }
                Ok(())
            }
            _ => Err(invalid(
                "browser_assert: ax 'expected' must be a string or an object {role, name}".to_string(),
                ToolReason::BadValue,
                "expected",
            )),
        },
        // `network` has its own validator at the call site; do not duplicate it.
        _ => Ok(()),
    }
}

fn js_literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// A deliberately small pattern matcher: `^`/`$` anchors and `.*` wildcards,
/// everything else literal. Bounded input and linear scanning; no regex
/// engine, no backtracking, no unbounded compile.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern.len() > 256 || text.len() > 4096 {
        return false;
    }
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$');
    let core = pattern.trim_start_matches('^').trim_end_matches('$');
    // Anchor-only patterns (`^`, `$`, `^$`) reduce to an empty core. An empty
    // core matches the empty string ONLY, never every URL in the ring.
    if core.is_empty() {
        return text.is_empty();
    }
    let parts: Vec<&str> = core.split(".*").collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 && anchored_start {
            if !text[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if let Some(idx) = text[pos..].find(part) {
            pos += idx + part.len();
        } else {
            return false;
        }
    }
    if anchored_end {
        let last = parts
            .iter()
            .rev()
            .find(|p| !p.is_empty())
            .copied()
            .unwrap_or("");
        return text.ends_with(last);
    }
    true
}

fn check(name: &str, expected: Value, observed: Value, pass: bool) -> Value {
    json!({"name": name, "expected": expected, "observed": observed, "pass": pass})
}

fn compare_dom(expected: &Value, observed: &Value) -> Vec<Value> {
    let mut checks = Vec::new();
    let got_text = observed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(want) = expected.get("exists").and_then(|v| v.as_bool()) {
        let got = observed
            .get("exists")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        checks.push(check("exists", json!(want), json!(got), want == got));
    }
    if let Some(want) = expected.get("text").and_then(|v| v.as_str()) {
        checks.push(check(
            "text",
            json!(want),
            json!(got_text),
            want == got_text,
        ));
    }
    if let Some(want) = expected.get("containsText").and_then(|v| v.as_str()) {
        checks.push(check(
            "containsText",
            json!(want),
            json!(got_text),
            got_text.contains(want),
        ));
    }
    if let Some(attr) = expected.get("attribute") {
        let name = attr.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let got = observed
            .get("attributes")
            .and_then(|a| a.get(name))
            .and_then(|v| v.as_str());
        let pass = match attr.get("value").and_then(|v| v.as_str()) {
            Some(want) => got == Some(want),
            None => got.is_some(),
        };
        checks.push(check("attribute", attr.clone(), json!(got), pass));
    }
    if checks.is_empty() {
        let got = observed
            .get("exists")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        checks.push(check("exists", json!(true), json!(got), got));
    }
    checks
}

fn compare_text(expected: &Value, observed: &Value) -> Vec<Value> {
    let got = observed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let (name, want, pass) = match expected {
        Value::String(s) => ("contains", s.clone(), got.contains(s.as_str())),
        Value::Object(_) => {
            if let Some(s) = expected.get("equals").and_then(|v| v.as_str()) {
                ("equals", s.to_string(), got == s)
            } else {
                let s = expected
                    .get("contains")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                ("contains", s.to_string(), got.contains(s))
            }
        }
        _ => ("equals", String::new(), false),
    };
    vec![check(name, json!(want), json!(got), pass)]
}

fn compare_url(expected: &Value, observed: &Value) -> Vec<Value> {
    let got = observed.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let (name, want, pass) = match expected {
        Value::String(s) => ("equals", s.clone(), got == s),
        Value::Object(_) => {
            if let Some(s) = expected.get("equals").and_then(|v| v.as_str()) {
                ("equals", s.to_string(), got == s)
            } else if let Some(s) = expected.get("contains").and_then(|v| v.as_str()) {
                ("contains", s.to_string(), got.contains(s))
            } else {
                let s = expected
                    .get("matches")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                ("matches", s.to_string(), wildcard_match(s, got))
            }
        }
        _ => ("equals", String::new(), false),
    };
    vec![check(name, json!(want), json!(got), pass)]
}

/// The network kind reuses the url kind's matcher: a plain string is exact,
/// caret/dollar/dot-star inside a string is a bounded wildcard, and an object
/// picks equals / contains / matches explicitly.
fn network_url_match(expect: &Value, actual: &str) -> bool {
    match expect {
        Value::String(s) => {
            if s.contains(".*") || s.starts_with('^') || s.ends_with('$') {
                wildcard_match(s, actual)
            } else {
                s == actual
            }
        }
        Value::Object(_) => {
            if let Some(s) = expect.get("equals").and_then(Value::as_str) {
                actual == s
            } else if let Some(s) = expect.get("contains").and_then(Value::as_str) {
                actual.contains(s)
            } else if let Some(s) = expect.get("matches").and_then(Value::as_str) {
                wildcard_match(s, actual)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// One captured entry against each stated field. Every check carries the real
/// observed value; a missing status is null, never an invented 0.
fn network_entry_matches(expected: &Value, entry: &Value) -> Vec<Value> {
    let mut checks = Vec::new();
    if let Some(u) = expected.get("url") {
        let got = entry.get("url").and_then(Value::as_str).unwrap_or("");
        checks.push(check(
            "url",
            u.clone(),
            json!(got),
            network_url_match(u, got),
        ));
    }
    if let Some(want) = expected.get("method").and_then(Value::as_str) {
        let got = entry.get("method").and_then(Value::as_str).unwrap_or("");
        checks.push(check("method", json!(want), json!(got), want == got));
    }
    if let Some(want) = expected.get("status").and_then(Value::as_i64) {
        let got = entry.get("status").and_then(Value::as_i64);
        checks.push(check("status", json!(want), json!(got), got == Some(want)));
    }
    if let Some(want) = expected.get("failed").and_then(Value::as_bool) {
        let got = entry
            .get("failed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        checks.push(check("failed", json!(want), json!(got), want == got));
    }
    checks
}

/// Attach to the live ring the network tool drains. network_get enables that
/// capture lazily on first use if it is not already on (observability only,
/// no page mutation); an empty ring stays empty and is reported honestly.
async fn observe_network(browser: &Browser, expected: &Value) -> Result<Value, String> {
    let ring = browser.network_get(None, NET_CAP).await;
    let total = ring.get("total").and_then(Value::as_u64).unwrap_or(0);
    let entries = ring
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut matched: Vec<Value> = Vec::new();
    let mut near: Vec<(usize, Value, Vec<Value>)> = Vec::new();
    for e in &entries {
        let checks = network_entry_matches(expected, e);
        let passed = checks.iter().filter(|c| c["pass"] == true).count();
        if passed == checks.len() {
            matched.push(e.clone());
        } else {
            near.push((passed, e.clone(), checks));
        }
    }
    let matched_count = matched.len();
    let matched_sample: Vec<Value> = matched.into_iter().take(5).collect();
    near.sort_by(|a, b| b.0.cmp(&a.0));
    let closest: Vec<Value> = near
        .into_iter()
        .take(5)
        .map(|(score, e, checks)| {
            let missing: Vec<Value> = checks.into_iter().filter(|c| c["pass"] != true).collect();
            json!({"entry": e, "matchedFields": score, "missing": missing})
        })
        .collect();
    let mut observed = json!({
        "total": total,
        "matchedCount": matched_count,
        "matched": matched_sample,
        "closestMisses": closest
    });
    if total == 0 {
        if let Some(obj) = observed.as_object_mut() {
            obj.insert("note".to_string(), json!("network ring was empty (total 0): no Network event has been observed yet. The assertion attaches to the existing capture ring, which network_get enables lazily if it is not already on (observability only, no page mutation); no verdict can be derived from an empty ring"));
        }
    }
    Ok(observed)
}

/// The verdict is an aggregate: the ring must have observed at least one
/// entry AND at least minCount entries must satisfy every stated field. The
/// total>0 guard is what keeps an empty ring from ever passing. One check
/// keeps the shared all(checks.pass) loop correct.
fn compare_network(expected: &Value, observed: &Value) -> Vec<Value> {
    let min = expected
        .get("minCount")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let got = observed
        .get("matchedCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = observed.get("total").and_then(Value::as_u64).unwrap_or(0);
    vec![check(
        "minCount",
        json!(min),
        json!(got),
        total > 0 && got >= min,
    )]
}

/// Evaluate in the assertion's realm: a named context page session when one
/// was resolved, the legacy active page otherwise.
async fn eval_in(
    browser: &Browser,
    session: Option<&str>,
    expression: &str,
) -> Result<Value, String> {
    match session {
        Some(s) => browser.eval_in_session(s, expression).await,
        None => browser.eval(expression).await,
    }
}

async fn observe_dom(
    browser: &Browser,
    target: Option<&str>,
    session: Option<&str>,
) -> Result<Value, String> {
    let node_fn = "function(el){var a={};var isEl=!!(el&&el.nodeType===1);if(isEl){for(var i=0;i<el.attributes.length;i++)a[el.attributes[i].name]=el.attributes[i].value;}var t=(el&&(el.innerText||el.textContent))||'';return JSON.stringify({exists:true,tag:isEl?el.tagName:(el?el.nodeName:''),text:String(t).trim(),attributes:a,count:1});}";
    if let Some(raw) = target {
        // Snapshot nodes carry `ref: "eN"` while the prose form is `@eN`.
        // Accept both so a ref copied from either source resolves.
        let as_ref = raw.strip_prefix('@').or_else(|| {
            let digits = raw.strip_prefix('e')?;
            (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(raw)
        });
        if let Some(id) = as_ref {
            if session.is_some() {
                return Err(typed_marker(
                    ToolCode::InvalidArgs,
                    ToolReason::BadValue,
                    &format!(
                        "browser_assert: ref @{id} is bound to the legacy page's last snapshot, so it \
                         cannot be asserted in a named context page; pass a CSS or AX selector with \
                         context="
                    ),
                ));
            }
            let backend = browser
                .ref_backend(id)
                .await
                .ok_or_else(|| format!("browser_assert: ref @{id} is not in the last snapshot"))?;
            let resolved = browser
                .command("DOM.resolveNode", json!({"backendNodeId": backend}), true)
                .await?;
            let object_id = resolved
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(|o| o.as_str())
                .ok_or_else(|| format!("browser_assert: ref @{id} no longer resolves to a node"))?
                .to_string();
            let replied = browser
                .call_function_on_in_session(
                    None,
                    json!({"objectId": object_id, "functionDeclaration": node_fn, "returnByValue": true}),
                )
                .await?;
            let text = replied
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("browser_assert: resolveNode returned no value: {replied:?}")
                })?;
            return serde_json::from_str(text)
                .map_err(|e| format!("browser_assert: bad DOM observation: {e}"));
        }
    }
    let sel = js_literal(target.unwrap_or("html"));
    let expr = format!(
        "(() => {{ var el = document.querySelector({sel}); \
         if (!el) return JSON.stringify({{exists:false,text:'',attributes:{{}},count:0}}); \
         var a={{}};for(var i=0;i<el.attributes.length;i++)a[el.attributes[i].name]=el.attributes[i].value; \
         return JSON.stringify({{exists:true,text:((el.innerText||el.textContent||'')+'').trim(),attributes:a,count:document.querySelectorAll({sel}).length}}); }})()"
    );
    let value = eval_in(browser, session, &expr).await?;
    let text = value
        .as_str()
        .ok_or_else(|| "browser_assert: DOM evaluation returned no string".to_string())?;
    serde_json::from_str(text).map_err(|e| format!("browser_assert: bad DOM observation: {e}"))
}

async fn observe_text(
    browser: &Browser,
    target: Option<&str>,
    session: Option<&str>,
) -> Result<Value, String> {
    let expr = match target {
        Some(sel) => format!(
            "(() => {{ var el = document.querySelector({}); return ((el && (el.innerText||el.textContent)) || '') + ''; }})()",
            js_literal(sel)
        ),
        None => "(() => ((document.body||document.documentElement).innerText||'') + '')()".to_string(),
    };
    let value = eval_in(browser, session, &expr).await?;
    Ok(json!({"text": value.as_str().unwrap_or("").trim()}))
}

async fn observe_url(browser: &Browser, session: Option<&str>) -> Result<Value, String> {
    let value = eval_in(browser, session, "location.href").await?;
    Ok(json!({"url": value.as_str().unwrap_or("")}))
}

/// Collapse whitespace the way `browser_snapshot` collapses an AX name, so a
/// name copied from a snapshot is the name an assertion compares against.
fn ax_norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A double-quoted selector value with `\"` and `\\` escapes. The same rule
/// the action tools use; an unquoted value is rejected rather than guessed.
fn ax_unquote(s: &str) -> Option<String> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return None,
            '\\' => match it.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// Only the two AX selector forms an assertion needs: `role=<role>` and
/// `role=<role>[name="<name>"]`, plus bare `name="<name>"`. Expected fields
/// widen the filter but never replace it, so the target the caller wrote and
/// the expectation it states must both hold.
fn ax_target(target: Option<&str>, expected: &Value) -> (Option<String>, Option<String>) {
    let mut role = expected
        .get("role")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut name = expected
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if let Some(raw) = target {
        let t = raw.trim();
        if let Some(rest) = t.strip_prefix("role=") {
            let (r, suffix) = match rest.find('[') {
                Some(i) => (&rest[..i], Some(&rest[i..])),
                None => (rest, None),
            };
            role = Some(r.trim().to_lowercase());
            if let Some(inner) = suffix
                .and_then(|s| s.strip_prefix("[name="))
                .and_then(|s| s.strip_suffix(']'))
            {
                name = ax_unquote(inner);
            }
        } else if let Some(rest) = t.strip_prefix("name=") {
            name = ax_unquote(rest);
        }
    }
    (role, name)
}

/// Read the same Accessibility tree `browser_snapshot` renders, and apply the
/// same flattening: ignored nodes and text-only roles are excluded, roles are
/// lowercased, and names are whitespace-collapsed and capped at 80 characters.
/// No DOM write, no synthetic AX node, no verdict minted from a tree this read
/// did not return.
async fn observe_ax(
    browser: &Browser,
    target: Option<&str>,
    expected: &Value,
    session: Option<&str>,
) -> Result<Value, String> {
    let (want_role, want_name) = ax_target(target, expected);
    let tree = match session {
        Some(s) => browser.ax_tree_in_session(s, None).await?,
        None => browser.ax_tree(None).await?,
    };
    let empty = Vec::new();
    let nodes = tree
        .get("nodes")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut candidates: Vec<Value> = Vec::new();
    for n in nodes.iter() {
        if n.get("ignored").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let role_raw = n
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if role_raw.is_empty() {
            continue;
        }
        let role = role_raw.to_lowercase();
        if role == "statictext" || role == "inlinetextbox" {
            continue;
        }
        // A backend DOM id is what the snapshot needs to mint a ref. An
        // assertion mints none, so a node without one is still real evidence.
        let backend = n.get("backendDOMNodeId").and_then(|v| v.as_i64());
        let raw_name = n
            .get("name")
            .and_then(|x| x.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name: String = ax_norm(raw_name).chars().take(80).collect();
        if let Some(w) = &want_role {
            if role != *w {
                continue;
            }
        }
        if let Some(w) = &want_name {
            if ax_norm(&name).to_lowercase() != ax_norm(w).to_lowercase() {
                continue;
            }
        }
        candidates.push(json!({"role": role, "name": name, "backendNodeId": backend}));
    }
    Ok(json!({
        "source": "Accessibility.getFullAXTree via browser.ax_tree, the read browser_snapshot uses",
        "treeNodes": nodes.len(),
        "target": target,
        "wantRole": want_role,
        "wantName": want_name,
        "candidates": candidates
    }))
}

/// One check per stated expectation, each carrying the observed value. A
/// target that matched nothing is an honest `exists:false`, never an error.
fn compare_ax(expected: &Value, observed: &Value) -> Vec<Value> {
    let empty = Vec::new();
    let candidates = observed
        .get("candidates")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut checks = vec![check(
        "exists",
        json!(true),
        json!(candidates.len()),
        !candidates.is_empty(),
    )];
    if let Some(want) = expected.get("role").and_then(|v| v.as_str()) {
        let got = candidates
            .first()
            .and_then(|c| c.get("role").and_then(|v| v.as_str()))
            .unwrap_or("");
        let pass = candidates
            .iter()
            .any(|c| c.get("role").and_then(|v| v.as_str()) == Some(want));
        checks.push(check("role", json!(want), json!(got), pass));
    }
    if let Some(want) = expected.get("name").and_then(|v| v.as_str()) {
        let got = candidates
            .first()
            .and_then(|c| c.get("name").and_then(|v| v.as_str()))
            .unwrap_or("");
        let pass = candidates.iter().any(|c| {
            c.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|n| ax_norm(n).to_lowercase() == ax_norm(want).to_lowercase())
        });
        checks.push(check("name", json!(want), json!(got), pass));
    }
    checks
}

/// Dispatcher entry point for the B2 fill lane.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let started = Instant::now();
    if let Err(e) = check_unknown(
        "browser_assert",
        args,
        &[
            "kind",
            "target",
            "expected",
            "timeoutMs",
            "pollMs",
            "context",
            "page",
        ],
    ) {
        return Err(e);
    }
    let kind = args.get("kind").and_then(|v| v.as_str()).ok_or_else(|| {
        invalid(
            "browser_assert: missing required arg 'kind'".to_string(),
            ToolReason::MissingArg,
            "kind",
        )
    })?;
    if !matches!(kind, "dom" | "ax" | "text" | "url" | "network") {
        return Err(invalid(
            format!("browser_assert: unknown kind '{kind}' (dom, text, url are implemented)"),
            ToolReason::UnknownAction,
            "kind",
        ));
    }
    let expected = args.get("expected").cloned().ok_or_else(|| {
        invalid(
            "browser_assert: missing required arg 'expected'".to_string(),
            ToolReason::MissingArg,
            "expected",
        )
    })?;
    if kind == "network" {
        validate_network_expected(&expected)?;
    }
    validate_expected(kind, &expected)?;
    let target = args.get("target").and_then(|v| v.as_str());
    // Present-but-wrong-typed targets are refused before any browser read
    // (`and_then(as_str)` would turn them into "absent" and assert another
    // page, minting a verdict from the wrong document).
    let want_context = optional_string_arg("browser_assert", args, "context")?;
    let want_page = optional_string_arg("browser_assert", args, "page")?;
    let want_context = want_context.as_deref();
    let want_page = want_page.as_deref();
    if want_page.is_some() && want_context.is_none() {
        return Err(invalid(
            "browser_assert: 'page' names a page inside a named context; pass 'context' with it"
                .to_string(),
            ToolReason::MissingArg,
            "context",
        ));
    }
    if kind == "dom" && target.is_none() {
        return Err(invalid(
            "browser_assert: dom needs 'target' (a selector or @eN ref)".to_string(),
            ToolReason::MissingArg,
            "target",
        ));
    }
    if kind == "ax" && ax_target(target, &expected) == (None, None) {
        return Err(invalid(
            "browser_assert: ax needs 'target' (role=<role>[name=\"...\"]) or expected {role, name}".to_string(),
            ToolReason::MissingArg,
            "target",
        ));
    }
    // Resolved ONCE: every poll observes the same page, and a page that
    // disappears mid-assert surfaces the failed read instead of a retarget.
    let page_ctx = match want_context {
        Some(name) => Some(
            browser
                .context_page(name, want_page)
                .await
                .map_err(ToolErr::from)?,
        ),
        None => None,
    };
    let session = page_ctx.as_ref().map(|p| p.session.as_str());
    let timeout_ms = match args.get("timeoutMs") {
        None => 5_000u64,
        Some(v) => v.as_u64().ok_or_else(|| {
            invalid(
                "browser_assert: 'timeoutMs' must be an integer >= 0".to_string(),
                ToolReason::BadValue,
                "timeoutMs",
            )
        })?,
    }
    .min(60_000);
    let poll_ms = match args.get("pollMs") {
        None => 100u64,
        Some(v) => v.as_u64().ok_or_else(|| {
            invalid(
                "browser_assert: 'pollMs' must be an integer >= 0".to_string(),
                ToolReason::BadValue,
                "pollMs",
            )
        })?,
    }
    .clamp(10, 1_000);
    let deadline = started + Duration::from_millis(timeout_ms);
    let poll = Duration::from_millis(poll_ms);

    let mut polls = 0u64;
    let mut observed;
    let mut checks: Vec<Value>;
    let verdict: bool;
    loop {
        let current = match kind {
            "dom" => observe_dom(browser, target, session).await,
            "ax" => observe_ax(browser, target, &expected, session).await,
            "text" => observe_text(browser, target, session).await,
            "network" => observe_network(browser, &expected).await,
            _ => observe_url(browser, session).await,
        }
        .map_err(ToolErr::from)?;
        let this_checks = match kind {
            "dom" => compare_dom(&expected, &current),
            "ax" => compare_ax(&expected, &current),
            "text" => compare_text(&expected, &current),
            "network" => compare_network(&expected, &current),
            _ => compare_url(&expected, &current),
        };
        let pass = this_checks
            .iter()
            .all(|c| c.get("pass").and_then(|v| v.as_bool()) == Some(true));
        polls += 1;
        observed = current;
        checks = this_checks;
        if pass {
            verdict = true;
            break;
        }
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            verdict = false;
            break;
        }
        tokio::time::sleep(remain.min(poll)).await;
    }

    // The ax kind answers with the verdict string its frozen fixture expects;
    // dom/text/url/network keep the boolean verdict their schemas expect.
    // Either way the per-check booleans in evidence carry the same decision.
    let verdict_value = if kind == "ax" {
        json!(if verdict { "pass" } else { "fail" })
    } else {
        json!(verdict)
    };
    let mut evidence =
        json!({"kind": kind, "checks": checks, "polls": polls, "budgetMs": timeout_ms});
    if kind == "network" {
        if let Some(obj) = evidence.as_object_mut() {
            for key in ["total", "matchedCount", "matched", "closestMisses", "note"] {
                if let Some(v) = observed.get(key) {
                    obj.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    if let (Some(cp), Some(obj)) = (&page_ctx, evidence.as_object_mut()) {
        obj.insert("context".to_string(), json!(cp.context));
        obj.insert("target".to_string(), json!(cp.target));
    }
    let body = json!({
        "expected": expected,
        "observed": observed,
        "verdict": verdict_value,
        "evidence": evidence,
        "elapsedMs": started.elapsed().as_millis() as u64
    });
    Ok(ok_text(body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        ax_target, compare_ax, compare_network, compare_text, network_url_match, validate_expected,
        validate_network_expected, wildcard_match, ROW,
    };
    use crate::browser::Browser;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn wildcard_match_is_bounded_and_literal_outside_wildcards() {
        assert!(wildcard_match(
            "^http://.*/next$",
            "http://example.test/a/next"
        ));
        assert!(!wildcard_match(
            "^http://.*/next$",
            "https://example.test/next"
        ));
        assert!(wildcard_match("abc", "xxabcxx"));
        assert!(!wildcard_match("a.c", "abc"));
        assert!(!wildcard_match(&"x".repeat(300), "x"));
    }

    #[test]
    fn row_is_b2() {
        assert_eq!(ROW, "B2");
    }

    #[test]
    fn network_url_match_uses_the_url_kind_matcher_style() {
        let exact = json!("http://example.test/a");
        assert!(network_url_match(&exact, "http://example.test/a"));
        assert!(!network_url_match(&exact, "http://example.test/b"));
        let wildcard = json!("^http://.*/next$");
        assert!(network_url_match(&wildcard, "http://example.test/a/next"));
        assert!(!network_url_match(&wildcard, "https://example.test/next"));
        let object = json!({"contains": "example.test"});
        assert!(network_url_match(&object, "http://example.test/a"));
    }

    #[test]
    fn network_expected_validation_names_bad_fields() {
        for (bad, field) in [
            (json!(5), "expected"),
            (json!({"url": 5}), "expected.url"),
            (json!({"url": {"matches": 3}}), "expected.url.matches"),
            (json!({"method": 5}), "expected.method"),
            (json!({"status": "200"}), "expected.status"),
            (json!({"failed": "yes"}), "expected.failed"),
            (json!({"minCount": -1}), "expected.minCount"),
            (json!({"minCount": 0}), "expected.minCount"),
        ] {
            let err = validate_network_expected(&bad).expect_err("must refuse bad field");
            let env = err.envelope();
            assert_eq!(
                env["structuredContent"]["code"], "invalid_args",
                "bad={bad}"
            );
            assert_eq!(env["structuredContent"]["arg"], field, "bad={bad}");
        }
        assert!(validate_network_expected(&json!({"url": "http://x", "minCount": 1})).is_ok());
    }

    #[test]
    fn network_compare_is_min_count() {
        let checks = compare_network(
            &json!({"minCount": 2}),
            &json!({"total": 4, "matchedCount": 1}),
        );
        assert_eq!(checks[0]["name"], "minCount");
        assert_eq!(checks[0]["observed"], 1);
        assert_eq!(checks[0]["pass"], false);
        let pass = compare_network(
            &json!({"minCount": 1}),
            &json!({"total": 4, "matchedCount": 2}),
        );
        assert_eq!(pass[0]["pass"], true);
    }

    #[test]
    fn network_compare_requires_nonempty_ring() {
        let empty = compare_network(
            &json!({"minCount": 1}),
            &json!({"total": 0, "matchedCount": 5}),
        );
        assert_eq!(
            empty[0]["pass"], false,
            "an empty ring never passes, even at 5 matches"
        );
        let real = compare_network(
            &json!({"minCount": 1}),
            &json!({"total": 6, "matchedCount": 1}),
        );
        assert_eq!(real[0]["pass"], true);
    }

    #[test]
    fn network_validation_rejects_unknown_keys() {
        for (bad, field) in [
            (json!({"statuz": 200}), "expected.statuz"),
            (
                json!({"url": {"equals": "x", "regex": "y"}}),
                "expected.url.regex",
            ),
        ] {
            let err = validate_network_expected(&bad).expect_err("unknown key must be refused");
            let env = err.envelope();
            assert_eq!(
                env["structuredContent"]["code"], "invalid_args",
                "bad={bad}"
            );
            assert_eq!(env["structuredContent"]["arg"], field, "bad={bad}");
        }
    }

    #[test]
    fn network_anchor_only_pattern_matches_empty_string_only() {
        for pattern in ["^", "$", "^$"] {
            assert!(
                !wildcard_match(pattern, "http://x/y"),
                "pattern {pattern} must not match a URL"
            );
            assert!(
                wildcard_match(pattern, ""),
                "pattern {pattern} must match the empty string"
            );
        }
        assert!(!network_url_match(&json!({"matches": "^$"}), "http://x/y"));
    }

    #[tokio::test]
    async fn assert_network_empty_ring_is_false_with_note_and_zero_min_count_is_refused() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let browser = Browser::new();
        let zero = super::handle(
            &browser,
            &json!({"kind": "network", "expected": {"minCount": 0}, "timeoutMs": 100, "pollMs": 50}),
        )
        .await
        .expect_err("minCount 0 must be refused before any read");
        assert_eq!(
            zero.envelope()["structuredContent"]["arg"],
            "expected.minCount"
        );
        let env = super::handle(
            &browser,
            &json!({"kind": "network", "expected": {"url": "http://x/y"}, "timeoutMs": 100, "pollMs": 50}),
        )
        .await
        .expect("an empty ring still returns a reply");
        let _ = browser.close().await;
        let sc = env.get("structuredContent").expect("structuredContent");
        assert_eq!(sc["verdict"], false, "an empty ring must never pass: {sc}");
        let note = sc["evidence"]["note"].as_str().unwrap_or("");
        assert!(
            note.contains("no Network event has been observed yet"),
            "note: {note}"
        );
        assert!(!note.contains("never enabled"), "stale wording: {note}");
    }

    /// The crate's local-server pattern: a loopback listener plus a spawned
    /// task that answers each request with a real 200 HTML response.
    async fn serve_200_fixture() -> (String, tokio::task::JoinHandle<()>) {
        let ln = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind");
        let port = ln.local_addr().expect("loopback addr").port();
        let srv = tokio::spawn(async move {
            loop {
                let (mut s, _) = match ln.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let mut req = Vec::new();
                    loop {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            s.read(&mut buf),
                        )
                        .await
                        {
                            Ok(Ok(0)) => break,
                            Ok(Ok(n)) => {
                                req.extend_from_slice(&buf[..n]);
                                if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 8192 {
                                    break;
                                }
                            }
                            _ => break,
                        }
                    }
                    let body = "<!doctype html><html><body><h1>network fixture</h1></body></html>";
                    let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n{}",
                    body.len(), body
                );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}/page"), srv)
    }

    #[tokio::test]
    async fn assert_network_real_response_passes_and_wrong_url_fails() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_200_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let ok_env = super::handle(
            &browser,
            &json!({
                "kind": "network",
                "expected": {"url": url, "method": "GET", "status": 200, "failed": false, "minCount": 1},
                "timeoutMs": 2000,
                "pollMs": 50
            }),
        )
        .await
        .expect("matching network assert");
        let wrong_env = super::handle(
            &browser,
            &json!({
                "kind": "network",
                "expected": {"url": format!("{}/wrong", url), "minCount": 1},
                "timeoutMs": 300,
                "pollMs": 50
            }),
        )
        .await
        .expect("wrong-url network assert");
        srv.abort();
        let _ = browser.close().await;
        let ok = ok_env.get("structuredContent").expect("structuredContent");
        assert_eq!(ok["verdict"], true, "a real 200 must pass: {ok}");
        assert!(
            ok["evidence"]["matched"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
            "evidence must name matched entries: {ok}"
        );
        let wrong = wrong_env
            .get("structuredContent")
            .expect("structuredContent");
        assert_eq!(wrong["verdict"], false, "wrong url must be false: {wrong}");
        assert!(
            wrong["evidence"]["closestMisses"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
            "evidence must name closest misses: {wrong}"
        );
        for key in ["expected", "observed", "verdict", "evidence", "elapsedMs"] {
            assert!(
                wrong.get(key).is_some(),
                "reply missing envelope field {key}"
            );
        }
    }

    #[test]
    fn ax_target_reads_role_and_name_from_the_selector_and_expected() {
        let expected = json!({});
        assert_eq!(
            ax_target(Some("role=button[name=\"Act\"]"), &expected),
            (Some("button".to_string()), Some("Act".to_string()))
        );
        let expected = json!({"role": "button", "name": "Act"});
        assert_eq!(
            ax_target(None, &expected),
            (Some("button".to_string()), Some("Act".to_string()))
        );
    }

    #[test]
    fn ax_compare_emits_measured_checks_not_a_bare_boolean() {
        let expected = json!({"role": "button", "name": "Act"});
        let observed = json!({"candidates": [
            {"role": "button", "name": "Act", "backendNodeId": 7}
        ]});
        let checks = compare_ax(&expected, &observed);
        assert_eq!(checks.len(), 3, "exists + role + name: {checks:?}");
        assert_eq!(checks[0]["name"], "exists");
        assert_eq!(checks[0]["observed"], json!(1));
        assert!(
            checks.iter().all(|c| c["pass"] == true),
            "checks: {checks:?}"
        );
        let empty = compare_ax(&expected, &json!({"candidates": []}));
        assert_eq!(empty[0]["observed"], json!(0));
        assert_eq!(empty[0]["pass"], false);
    }

    #[test]
    fn text_expectation_cannot_pass_by_typo() {
        let typo = validate_expected("text", &json!({"text": "zzz"}))
            .expect_err("an unknown key must be refused, not silently ignored");
        let env = typo.envelope();
        assert_eq!(
            env["structuredContent"]["code"], "invalid_args",
            "env={env}"
        );
        assert_eq!(
            env["structuredContent"]["arg"], "expected.text",
            "env={env}"
        );

        assert!(validate_expected("text", &json!({"contains": "zzz"})).is_ok());
        assert!(validate_expected("text", &json!("zzz")).is_ok());
        assert!(validate_expected("text", &json!({"equals": ""})).is_ok());

        let empty_needle = validate_expected("text", &json!({"contains": ""}))
            .expect_err("contains:'' must be refused");
        let env = empty_needle.envelope();
        assert_eq!(
            env["structuredContent"]["arg"], "expected.contains",
            "env={env}"
        );

        let empty =
            validate_expected("text", &json!({})).expect_err("an empty object must be refused");
        let env = empty.envelope();
        assert_eq!(
            env["structuredContent"]["code"], "invalid_args",
            "env={env}"
        );
        assert_eq!(env["structuredContent"]["arg"], "expected", "env={env}");
        assert_eq!(
            env["structuredContent"]["reason"], "missing_arg",
            "env={env}"
        );
        assert!(
            env["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("contains")
                && env["content"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .contains("equals"),
            "empty-object message must name contains/equals: {env}"
        );
    }

    #[test]
    fn dom_url_ax_expectations_refuse_unknown_or_empty_keys() {
        for (kind, good) in [
            (
                "dom",
                json!({"exists": true, "text": "hi", "containsText": "h", "attribute": {"name": "id", "value": "x"}}),
            ),
            ("url", json!({"contains": "/next"})),
            ("ax", json!({"role": "button", "name": "Go"})),
        ] {
            assert!(
                validate_expected(kind, &good).is_ok(),
                "kind={kind} good={good}"
            );
        }
        for (kind, bad, field) in [
            ("dom", json!({"exists": "yes"}), "expected.exists"),
            ("dom", json!({"txt": "x"}), "expected.txt"),
            ("dom", json!({"containsText": ""}), "expected.containsText"),
            (
                "dom",
                json!({"attribute": {"value": "x"}}),
                "expected.attribute.name",
            ),
            ("dom", json!([1]), "expected"),
            ("url", json!({"contains": ""}), "expected.contains"),
            ("url", json!({}), "expected"),
            ("url", json!({"regex": "x"}), "expected.regex"),
            ("ax", json!({"rol": "button"}), "expected.rol"),
            ("ax", json!({"role": ""}), "expected.role"),
        ] {
            let err = validate_expected(kind, &bad).expect_err("must refuse");
            let env = err.envelope();
            assert_eq!(
                env["structuredContent"]["code"], "invalid_args",
                "kind={kind} bad={bad} env={env}"
            );
            assert_eq!(
                env["structuredContent"]["arg"], field,
                "kind={kind} bad={bad} env={env}"
            );
        }
    }

    #[test]
    fn empty_needle_matches_everything_but_the_validator_refuses_it() {
        // compare_text still matches mechanically: an empty needle is contained
        // by every string. The validator, not the comparator, is what refuses it.
        let checks = compare_text(&json!({"contains": ""}), &json!({"text": "anything"}));
        assert_eq!(checks[0]["pass"], true, "an empty needle matches any text");
        assert!(validate_expected("text", &json!({"contains": ""})).is_err());
    }
}
