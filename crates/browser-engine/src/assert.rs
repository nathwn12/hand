//! Assertion contract for row B2.
//!
//! Private home for the `browser_assert` tool: DOM, AX, text and URL assertions
//! with bounded polling, returning expected, observed, verdict, evidence and
//! elapsed time. The AX kind reads the same `Accessibility.getFullAXTree`
//! tree `browser_snapshot` renders; only `network` stays typed
//! `not_implemented`, so no verdict is faked for a check this row cannot
//! actually perform.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browser::Browser;
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
        "description": "Assert against the live page with bounded polling. kind dom checks a CSS selector or @eN ref for existence, text and attributes; kind ax matches a role and accessible name on the same Accessibility tree browser_snapshot reads (target forms: role=<role>, role=<role>[name=\\\"<name>\\\"], name=\\\"<name>\\\"; expected {role, name}); kind text checks visible text with contains/equals; kind url checks the current location with equals/contains or a small bounded wildcard pattern (^, $, .*). Every reply carries expected, observed, verdict, evidence and elapsedMs. dom/text/url return a boolean verdict; ax returns the string pass or fail. A check still false at the poll budget returns that verdict with the last observed value, not a timeout. network is typed not_implemented. context=<name> asserts against a page of that NAMED isolated browser context instead of the legacy page (without page=, a context holding more than one page is a typed invalid_args/ambiguous_target listing every targetId; an unknown context or page is a typed invalid_target/not_found and never falls back to the legacy page). A @eN ref is legacy-page bound, so target= with context= must be a CSS or AX selector.",
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
    let mut role = expected.get("role").and_then(|v| v.as_str()).map(str::to_string);
    let mut name = expected.get("name").and_then(|v| v.as_str()).map(str::to_string);
    if let Some(raw) = target {
        let t = raw.trim();
        if let Some(rest) = t.strip_prefix("role=") {
            let (r, suffix) = match rest.find('[') {
                Some(i) => (&rest[..i], Some(&rest[i..])),
                None => (rest, None),
            };
            role = Some(r.trim().to_lowercase());
            if let Some(inner) = suffix.and_then(|s| s.strip_prefix("[name=")).and_then(|s| s.strip_suffix(']')) {
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
    let nodes = tree.get("nodes").and_then(|v| v.as_array()).unwrap_or(&empty);
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
        &["kind", "target", "expected", "timeoutMs", "pollMs", "context", "page"],
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
    if kind == "network" {
        return Err(not_implemented_kind(kind));
    }
    if kind == "dom" && !expected.is_object() {
        return Err(invalid("browser_assert: dom 'expected' must be an object".to_string(), ToolReason::BadValue, "expected"));
    }
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
        return Err(invalid("browser_assert: dom needs 'target' (a selector or @eN ref)".to_string(), ToolReason::MissingArg, "target"));
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
            "dom" => observe_dom(browser, target, session).await,
            "ax" => observe_ax(browser, target, &expected, session).await,
            "text" => observe_text(browser, target, session).await,
            _ => observe_url(browser, session).await,
        }
        .map_err(ToolErr::from)?;
        let this_checks = match kind {
            "dom" => compare_dom(&expected, &current),
            "ax" => compare_ax(&expected, &current),
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

    // The ax kind answers with the verdict string its frozen fixture expects;
    // dom/text/url keep the boolean verdict their schemas and fixtures expect.
    // Either way the per-check booleans in evidence carry the same decision.
    let verdict_value = if kind == "ax" {
        json!(if verdict { "pass" } else { "fail" })
    } else {
        json!(verdict)
    };
    let mut evidence = json!({"kind": kind, "checks": checks, "polls": polls, "budgetMs": timeout_ms});
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
    use super::{ax_target, compare_ax, not_implemented_kind, wildcard_match, ROW};
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
        let env = not_implemented_kind("network").envelope();
        assert_eq!(env["structuredContent"]["code"], "not_implemented");
        assert_eq!(env["structuredContent"]["kind"], "network");
        let text = env["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(ROW), "message must name {ROW}: {text}");
        assert!(text.contains("network"), "message must name the kind: {text}");
    }

    #[test]
    fn ax_target_reads_role_and_name_from_the_selector_and_expected() {
        let expected = json!({});
        assert_eq!(
            ax_target(Some("role=button[name=\"Act\"]"), &expected),
            (Some("button".to_string()), Some("Act".to_string()))
        );
        let expected = json!({"role": "button", "name": "Act"});
        assert_eq!(ax_target(None, &expected), (Some("button".to_string()), Some("Act".to_string())));
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
        assert!(checks.iter().all(|c| c["pass"] == true), "checks: {checks:?}");
        let empty = compare_ax(&expected, &json!({"candidates": []}));
        assert_eq!(empty[0]["observed"], json!(0));
        assert_eq!(empty[0]["pass"], false);
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
