//! Assertion contract for row B2.
//!
//! Private home for the `browser_assert` tool: DOM, text and URL assertions
//! with bounded polling, returning expected, observed, verdict, evidence and
//! elapsed time. `ax` and `network` kinds stay typed `not_implemented`: no
//! verdict is faked for a check this row cannot actually perform.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::{check_unknown, ok_text, ToolCode, ToolErr, ToolReason};

/// Row id this module owns. The stub test reads it; lib code does not.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B2";

/// Frozen input/output contract for row B2 (`browser_assert`).
pub fn schema() -> Value {
    json!({
        "name": "assert",
        "title": "Browser assert",
        "description": "Assert against the live page with bounded polling. kind dom checks a CSS selector or @eN ref for existence, text and attributes; kind text checks visible text with contains/equals; kind url checks the current location with equals/contains or a small bounded wildcard pattern (^, $, .*). Every reply carries expected, observed, verdict, evidence and elapsedMs. A check still false at the poll budget returns verdict false with the last observed value, not a timeout. ax and network kinds are typed not_implemented.",
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

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_assert", "arg": arg}),
    )
}

/// The two kinds this row does not claim to implement. A typed refusal is the
/// honest answer: no `verdict: true` is minted from an unperformed check.
fn not_implemented_kind(kind: &str) -> ToolErr {
    ToolErr::typed(
        format!("row B2: assertion kind '{kind}' is not implemented"),
        json!({"code": "not_implemented", "tool": "browser_assert", "kind": kind}),
    )
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
    let core = pattern
        .trim_start_matches('^')
        .trim_end_matches('$');
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
        let last = parts.iter().rev().find(|p| !p.is_empty()).copied().unwrap_or("");
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
        let got = observed.get("exists").and_then(|v| v.as_bool()).unwrap_or(false);
        checks.push(check("exists", json!(want), json!(got), want == got));
    }
    if let Some(want) = expected.get("text").and_then(|v| v.as_str()) {
        checks.push(check("text", json!(want), json!(got_text), want == got_text));
    }
    if let Some(want) = expected.get("containsText").and_then(|v| v.as_str()) {
        checks.push(check("containsText", json!(want), json!(got_text), got_text.contains(want)));
    }
    if let Some(attr) = expected.get("attribute") {
        let name = attr.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let got = observed.get("attributes").and_then(|a| a.get(name)).and_then(|v| v.as_str());
        let pass = match attr.get("value").and_then(|v| v.as_str()) {
            Some(want) => got == Some(want),
            None => got.is_some(),
        };
        checks.push(check("attribute", attr.clone(), json!(got), pass));
    }
    if checks.is_empty() {
        let got = observed.get("exists").and_then(|v| v.as_bool()).unwrap_or(false);
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
                let s = expected.get("contains").and_then(|v| v.as_str()).unwrap_or("");
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
                let s = expected.get("matches").and_then(|v| v.as_str()).unwrap_or("");
                ("matches", s.to_string(), wildcard_match(s, got))
            }
        }
        _ => ("equals", String::new(), false),
    };
    vec![check(name, json!(want), json!(got), pass)]
}

async fn observe_dom(browser: &Browser, target: Option<&str>) -> Result<Value, String> {
    let node_fn = "function(el){var a={};var isEl=!!(el&&el.nodeType===1);if(isEl){for(var i=0;i<el.attributes.length;i++)a[el.attributes[i].name]=el.attributes[i].value;}var t=(el&&(el.innerText||el.textContent))||'';return JSON.stringify({exists:true,tag:isEl?el.tagName:(el?el.nodeName:''),text:String(t).trim(),attributes:a,count:1});}";
    if let Some(raw) = target {
        // Snapshot nodes carry `ref: "eN"` while the prose form is `@eN`.
        // Accept both so a ref copied from either source resolves.
        let as_ref = raw.strip_prefix('@').or_else(|| {
            let digits = raw.strip_prefix('e')?;
            (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(raw)
        });
        if let Some(id) = as_ref {
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
                .ok_or_else(|| format!("browser_assert: resolveNode returned no value: {replied:?}"))?;
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
    let value = browser.eval(&expr).await?;
    let text = value
        .as_str()
        .ok_or_else(|| "browser_assert: DOM evaluation returned no string".to_string())?;
    serde_json::from_str(text).map_err(|e| format!("browser_assert: bad DOM observation: {e}"))
}

async fn observe_text(browser: &Browser, target: Option<&str>) -> Result<Value, String> {
    let expr = match target {
        Some(sel) => format!(
            "(() => {{ var el = document.querySelector({}); return ((el && (el.innerText||el.textContent)) || '') + ''; }})()",
            js_literal(sel)
        ),
        None => "(() => ((document.body||document.documentElement).innerText||'') + '')()".to_string(),
    };
    let value = browser.eval(&expr).await?;
    Ok(json!({"text": value.as_str().unwrap_or("").trim()}))
}

async fn observe_url(browser: &Browser) -> Result<Value, String> {
    let value = browser.eval("location.href").await?;
    Ok(json!({"url": value.as_str().unwrap_or("")}))
}

/// Dispatcher entry point for the B2 fill lane.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    let started = Instant::now();
    if let Err(e) = check_unknown(
        "browser_assert",
        args,
        &["kind", "target", "expected", "timeoutMs", "pollMs"],
    ) {
        return Err(e);
    }
    let kind = args
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid("browser_assert: missing required arg 'kind'".to_string(), ToolReason::MissingArg, "kind"))?;
    if !matches!(kind, "dom" | "ax" | "text" | "url" | "network") {
        return Err(invalid(
            format!("browser_assert: unknown kind '{kind}' (dom, text, url are implemented)"),
            ToolReason::UnknownAction,
            "kind",
        ));
    }
    let expected = args
        .get("expected")
        .cloned()
        .ok_or_else(|| invalid("browser_assert: missing required arg 'expected'".to_string(), ToolReason::MissingArg, "expected"))?;
    if kind == "ax" || kind == "network" {
        return Err(not_implemented_kind(kind));
    }
    if kind == "dom" && !expected.is_object() {
        return Err(invalid("browser_assert: dom 'expected' must be an object".to_string(), ToolReason::BadValue, "expected"));
    }
    let target = args.get("target").and_then(|v| v.as_str());
    if kind == "dom" && target.is_none() {
        return Err(invalid("browser_assert: dom needs 'target' (a selector or @eN ref)".to_string(), ToolReason::MissingArg, "target"));
    }
    let timeout_ms = match args.get("timeoutMs") {
        None => 5_000u64,
        Some(v) => v.as_u64().ok_or_else(|| {
            invalid("browser_assert: 'timeoutMs' must be an integer >= 0".to_string(), ToolReason::BadValue, "timeoutMs")
        })?,
    }
    .min(60_000);
    let poll_ms = match args.get("pollMs") {
        None => 100u64,
        Some(v) => v.as_u64().ok_or_else(|| {
            invalid("browser_assert: 'pollMs' must be an integer >= 0".to_string(), ToolReason::BadValue, "pollMs")
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
            "dom" => observe_dom(browser, target).await,
            "text" => observe_text(browser, target).await,
            _ => observe_url(browser).await,
        }
        .map_err(ToolErr::fallback)?;
        let this_checks = match kind {
            "dom" => compare_dom(&expected, &current),
            "text" => compare_text(&expected, &current),
            _ => compare_url(&expected, &current),
        };
        let pass = this_checks.iter().all(|c| c.get("pass").and_then(|v| v.as_bool()) == Some(true));
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

    let body = json!({
        "expected": expected,
        "observed": observed,
        "verdict": verdict,
        "evidence": {"kind": kind, "checks": checks, "polls": polls, "budgetMs": timeout_ms},
        "elapsedMs": started.elapsed().as_millis() as u64
    });
    Ok(ok_text(body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{not_implemented_kind, wildcard_match, ROW};
    use serde_json::json;

    #[test]
    fn wildcard_match_is_bounded_and_literal_outside_wildcards() {
        assert!(wildcard_match("^http://.*/next$", "http://example.test/a/next"));
        assert!(!wildcard_match("^http://.*/next$", "https://example.test/next"));
        assert!(wildcard_match("abc", "xxabcxx"));
        assert!(!wildcard_match("a.c", "abc"));
        assert!(!wildcard_match(&"x".repeat(300), "x"));
    }

    #[test]
    fn unimplemented_kinds_are_typed_and_name_the_row_and_kind() {
        let env = not_implemented_kind("ax").envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        assert_eq!(env["structuredContent"]["kind"], "ax");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
        assert!(text.contains("ax"), "message must name the kind: {text}");
    }

    #[test]
    fn dom_default_check_is_existence() {
        let observed = json!({"exists": true, "text": "hi", "attributes": {}});
        let checks = super::compare_dom(&json!({}), &observed);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0]["name"], "exists");
        assert_eq!(checks[0]["pass"], true);
    }
}
