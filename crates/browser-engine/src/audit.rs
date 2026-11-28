//! Audit contract for row B7.
//!
//! Private home for the `browser_audit` tool: a viewport matrix that measures
//! overflow, contrast, focus-ring visibility and target size on the LIVE page.
//! Every check carries its measurement, threshold and pass flag; there is no
//! bare boolean and no verdict invented away from the page.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::{check_unknown, ok_text, ToolCode, ToolErr, ToolReason};

/// Row id this module owns. The stub test reads it; lib code does not.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROW: &str = "B7";

/// The checks this row knows how to measure. An unknown name is rejected at
/// the boundary instead of silently producing a record with no evidence.
const CHECKS: [&str; 4] = ["overflow", "contrast", "focus", "targetSize"];

/// The formula is returned with every contrast measurement so a reader can
/// check the number without reading this source. WCAG 2.x relative luminance,
/// sRGB channels linearised, then the lighter-over-darker ratio.
const CONTRAST_FORMULA: &str = "WCAG 2.x: c = c8/255 per sRGB channel; \
    c_lin = c <= 0.03928 ? c/12.92 : ((c+0.055)/1.055)^2.4; \
    L = 0.2126*R_lin + 0.7152*G_lin + 0.0722*B_lin; \
    ratio = (max(L1,L2)+0.05)/(min(L1,L2)+0.05); normal text passes at >= 4.5:1";

/// One read of the page returns every measurement. The page's own computed
/// styles and boxes are the source; the engine never supplies a substitute
/// colour or a guessed box. Text-only children are used for contrast so an
/// ancestor's innerText does not attribute a child's colour to the parent.
const MEASURE_JS: &str = r#"(() => {
  const d = document, de = d.documentElement, body = d.body;
  const clientWidth = de ? de.clientWidth : (body ? body.clientWidth : 0);
  const scrollWidth = Math.max(de ? de.scrollWidth : 0, body ? body.scrollWidth : 0);
  const parseColor = (c) => {
    const s = String(c || '').trim();
    let m = s.match(/^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s,/]+([\d.]+))?/i);
    if (m) return [Number(m[1]), Number(m[2]), Number(m[3]), m[4] === undefined ? 1 : Number(m[4])];
    m = s.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
    if (m) {
      const h = m[1];
      const hex = h.length === 3 ? h.split('').map((x) => x + x).join('') : h;
      return [parseInt(hex.slice(0,2),16), parseInt(hex.slice(2,4),16), parseInt(hex.slice(4,6),16), 1];
    }
    return null;
  };
  const chan = (v) => { const c = v / 255; return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4); };
  const lum = (rgb) => 0.2126*chan(rgb[0]) + 0.7152*chan(rgb[1]) + 0.0722*chan(rgb[2]);
  const ratio = (fg, bg) => { const a = lum(fg), b = lum(bg), hi = Math.max(a,b), lo = Math.min(a,b); return (hi + 0.05) / (lo + 0.05); };
  const bgOf = (el) => {
    let cur = el;
    while (cur) {
      const c = parseColor(getComputedStyle(cur).backgroundColor);
      if (c && c[3] > 0) return c;
      cur = cur.parentElement;
    }
    return [255,255,255,1];
  };
  const describe = (el) => {
    if (el.id) return '#' + el.id;
    const cls = (typeof el.className === 'string') ? el.className.trim().split(/\s+/)[0] : '';
    return el.tagName.toLowerCase() + (cls ? '.' + cls : '');
  };
  const round = (n) => Math.round(n * 1000) / 1000;
  let worst = null;
  for (const el of d.querySelectorAll('body *')) {
    const own = Array.prototype.filter.call(el.childNodes, (n) => n.nodeType === 3)
      .map((n) => n.nodeValue || '').join('').trim();
    if (!own) continue;
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden' || Number(cs.opacity) === 0) continue;
    const fg = parseColor(cs.color);
    if (!fg) continue;
    const bg = bgOf(el);
    const r = ratio(fg, bg);
    if (!worst || r < worst.ratio) worst = {
      selector: describe(el), ratio: round(r), foreground: cs.color,
      background: 'rgb(' + bg.slice(0,3).map(Math.round).join(',') + ')', sample: own.slice(0, 60)
    };
  }
  const focusables = d.querySelectorAll('input,button,a[href],select,textarea,[tabindex]');
  // The audit must leave the page as found: remember the focused control and
  // put focus back before returning, without scrolling to any of them.
  const describeActive = (el) => {
    if (!el || !el.tagName) return null;
    const tag = el.tagName.toLowerCase();
    return tag + (el.id ? '#' + el.id : '');
  };
  const savedActive = d.activeElement;
  const activeElementBefore = describeActive(savedActive);
  const focusStates = [];
  for (const el of focusables) {
    if (getComputedStyle(el).display === 'none') continue;
    el.focus({preventScroll: true});
    if (d.activeElement !== el) continue;
    const cs = getComputedStyle(el);
    focusStates.push({
      selector: describe(el), focused: true,
      outlineStyle: cs.outlineStyle, outlineWidth: cs.outlineWidth, outlineColor: cs.outlineColor,
      ringVisible: cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0 && cs.outlineColor !== 'transparent'
    });
    el.blur();
  }
  // A focus handler may have detached the saved control. Restore only when it
  // is still connected and the focus actually sticks; otherwise blur whatever
  // the loop left focused and report the failure instead of claiming success.
  let focusRestored = false;
  if (savedActive && savedActive.isConnected && typeof savedActive.focus === 'function') {
    try { savedActive.focus({preventScroll: true}); } catch (err) { /* detached by a handler */ }
    focusRestored = d.activeElement === savedActive;
  }
  if (!focusRestored) {
    const cur = d.activeElement;
    if (cur && typeof cur.blur === 'function') {
      try { cur.blur(); } catch (err) { /* already gone */ }
    }
  }
  const activeElementAfter = describeActive(d.activeElement);
  const noRing = focusStates.filter((f) => !f.ringVisible).map((f) => f.selector);
  const first = focusStates[0];
  const focus = first ? {
    selector: first.selector, focused: true,
    outlineStyle: first.outlineStyle, outlineWidth: first.outlineWidth, outlineColor: first.outlineColor,
    ringVisible: noRing.length === 0, controls: focusStates, noRing: noRing
  } : {selector: null, focused: false, ringVisible: false, controls: [], noRing: [],
       note: 'no focusable control on the page'};
  const small = [];
  for (const el of d.querySelectorAll('button,a[href],input,select,textarea,[role="button"],[tabindex]')) {
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    if (r.width < 24 || r.height < 24) small.push({
      selector: describe(el), width: round(r.width), height: round(r.height)
    });
  }
  return JSON.stringify({clientWidth: clientWidth, scrollWidth: scrollWidth,
    contrast: worst, focus: focus, smallTargets: small, focusableCount: focusables.length,
    focusRestored: focusRestored, activeElementBefore: activeElementBefore,
    activeElementAfter: activeElementAfter});
})()"#;

/// Frozen input/output contract for row B7 (`browser_audit`).
pub fn schema() -> Value {
    json!({
        "name": "audit",
        "title": "Browser audit",
        "description": "Audit the live page at each requested viewport width. Measurements come from the page's own computed styles and boxes: scrollWidth vs clientWidth for overflow, WCAG relative-luminance contrast ratio with the formula returned beside it, a computed focus ring read by temporarily focusing each focusable control, and every clickable box under 24px. The audit then restores the previous activeElement with preventScroll and reports that as focusRestored with activeElementBefore and activeElementAfter descriptors, so the page's focus is left as found. One evidence record per viewport carries width, scrollWidth, overflow, contrast, focus, targetSize and a per-viewport verdict. Both the top-level verdict and each per-viewport verdict are pass only when every requested check was measured AND passed at that scope, and fail otherwise: a page that demonstrably fails a check is never reported as pass. The separate top-level measured field is coverage only - pass means every requested check produced a measurement at every viewport, fail means one could not be measured; the check is named in evidence.unmeasuredChecks and in its own note.",
        "inputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "integer", "minimum": 1}},
            "checks": {"type": "array", "items": {"type": "string", "enum": CHECKS}}
        }, "required": ["viewports", "checks"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "object", "properties": {
                "width": {"type": "integer", "minimum": 0},
                "scrollWidth": {"type": "integer", "minimum": 0},
                "overflow": {}, "contrast": {}, "focus": {}, "targetSize": {},
                "verdict": {"type": "string", "enum": ["pass", "fail"]}
            }, "required": ["width", "scrollWidth", "overflow", "contrast", "focus", "targetSize", "verdict"]}},
            "checks": {"type": "array", "items": {"type": "string"}},
            "verdict": {"type": "string", "enum": ["pass", "fail"]},
            "measured": {"type": "string", "enum": ["pass", "fail"]},
            "evidence": {"type": "object"}
        }, "required": ["viewports", "checks", "verdict", "measured", "evidence"], "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true}
    })
}

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_audit", "arg": arg}),
    )
}

/// Validate at the boundary: a viewport is a positive, bounded CSS width and
/// a check is one this row can actually measure. The ceiling bounds the loop;
/// it is recorded here where the code can show it.
fn validate_viewports(args: &Value) -> Result<Vec<i64>, ToolErr> {
    let arr = args
        .get("viewports")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            invalid(
                "browser_audit: 'viewports' must be an array of positive integers".to_string(),
                ToolReason::BadValue,
                "viewports",
            )
        })?;
    if arr.is_empty() || arr.len() > 12 {
        return Err(invalid(
            format!(
                "browser_audit: 'viewports' needs 1..12 widths, got {}",
                arr.len()
            ),
            ToolReason::BadValue,
            "viewports",
        ));
    }
    arr.iter()
        .map(|v| {
            let n = v.as_i64().filter(|n| *n > 0 && *n <= 4096).ok_or_else(|| {
                invalid(
                    format!("browser_audit: viewport width must be 1..4096, got {v}"),
                    ToolReason::BadValue,
                    "viewports",
                )
            })?;
            Ok(n)
        })
        .collect()
}

fn validate_checks(args: &Value) -> Result<Vec<String>, ToolErr> {
    let arr = args
        .get("checks")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            invalid(
                "browser_audit: 'checks' must be an array of check names".to_string(),
                ToolReason::BadValue,
                "checks",
            )
        })?;
    if arr.is_empty() {
        return Err(invalid(
            "browser_audit: 'checks' must name at least one check".to_string(),
            ToolReason::MissingArg,
            "checks",
        ));
    }
    arr.iter().map(|v| {
        let name = v.as_str().ok_or_else(|| {
            invalid(format!("browser_audit: check name must be a string, got {v}"), ToolReason::BadValue, "checks")
        })?;
        if !CHECKS.contains(&name) {
            return Err(invalid(
                format!("browser_audit: unknown check '{name}' (overflow, contrast, focus, targetSize)"),
                ToolReason::BadValue, "checks",
            ));
        }
        Ok(name.to_string())
    }).collect()
}

/// One check record. `requested:false` still carries the measurement: every
/// frozen key is present, and only requested checks decide the verdict.
fn check_record(name: &str, requested: bool, observed: Value, pass: bool, note: &str) -> Value {
    json!({
        "name": name,
        "requested": requested,
        "expected": match name {
            "overflow" => json!("scrollWidth <= clientWidth"),
            "contrast" => json!("contrast ratio >= 4.5:1"),
            "focus" => json!("a visible focus ring on a focused control"),
            _ => json!("every clickable box >= 24px"),
        },
        "observed": observed,
        "pass": if requested { Value::Bool(pass) } else { Value::Null },
        "note": note
    })
}

/// A requested check counts as measured only when its observation carries a
/// real value. A null observation (no text to measure, no focus data) is not
/// a pass: it is reported by name and fails both verdicts.
fn observation_measured(name: &str, observed: &Value) -> bool {
    match name {
        "overflow" => {
            observed.get("scrollWidth").is_some() && observed.get("clientWidth").is_some()
        }
        "contrast" => observed.get("ratio").and_then(Value::as_f64).is_some(),
        "focus" => observed
            .get("ringVisible")
            .and_then(Value::as_bool)
            .is_some(),
        "targetSize" => observed.get("smallCount").and_then(Value::as_u64).is_some(),
        _ => false,
    }
}

/// One viewport's two signals, kept separate: verdict is "measured and
/// passed", measured is coverage alone.
struct ViewportOutcome {
    verdict: &'static str,
    measured: bool,
    unmeasured: Vec<String>,
    failed: Vec<String>,
}

/// Fold one viewport record against the requested checks. Only requested
/// checks decide; an unmeasurable request is both unmeasured and failed.
fn fold_viewport(record: &Value, checks: &[String]) -> ViewportOutcome {
    let mut outcome = ViewportOutcome {
        verdict: "pass",
        measured: true,
        unmeasured: Vec::new(),
        failed: Vec::new(),
    };
    for name in checks {
        let check = record.get(name.as_str());
        let measured = check
            .and_then(|c| c.get("observed"))
            .is_some_and(|o| observation_measured(name, o));
        let passed = check
            .and_then(|c| c.get("pass"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !measured {
            outcome.measured = false;
            outcome.unmeasured.push(name.clone());
        }
        if !measured || !passed {
            outcome.failed.push(name.clone());
        }
    }
    if !outcome.failed.is_empty() {
        outcome.verdict = "fail";
    }
    outcome
}

/// Fold the whole matrix. A missing viewport is missing coverage too, so an
/// incomplete matrix can never report pass at the top level.
fn aggregate(
    viewports: &[Value],
    checks: &[String],
    expected: usize,
) -> (String, String, Vec<String>, Vec<String>) {
    let mut measured = viewports.len() == expected;
    let mut unmeasured = Vec::new();
    let mut failed = Vec::new();
    for record in viewports {
        let outcome = fold_viewport(record, checks);
        measured &= outcome.measured;
        unmeasured.extend(outcome.unmeasured);
        failed.extend(outcome.failed);
    }
    let verdict = if measured && failed.is_empty() {
        "pass"
    } else {
        "fail"
    };
    (
        verdict.to_string(),
        if measured { "pass" } else { "fail" }.to_string(),
        unmeasured,
        failed,
    )
}

/// Apply the device-metrics override, measure the page, and build one record.
async fn measure_viewport(
    browser: &Browser,
    width: i64,
    checks: &[String],
) -> Result<Value, ToolErr> {
    browser
        .viewport_set(json!({
            "width": width, "height": 720, "deviceScaleFactor": 1, "mobile": false
        }))
        .await
        .map_err(ToolErr::fallback)?;
    let (read_width, read_height, dpr) = browser
        .viewport_readback()
        .await
        .map_err(ToolErr::fallback)?;
    let raw = browser.eval(MEASURE_JS).await.map_err(ToolErr::fallback)?;
    let text = raw.as_str().ok_or_else(|| {
        ToolErr::fallback("browser_audit: measurement did not return a JSON string".to_string())
    })?;
    let m: Value = serde_json::from_str(text)
        .map_err(|e| ToolErr::fallback(format!("browser_audit: bad measurement: {e}")))?;
    let sw = m.get("scrollWidth").and_then(|v| v.as_i64()).unwrap_or(0);
    let cw = m.get("clientWidth").and_then(|v| v.as_i64()).unwrap_or(0);

    let overflow_pass = sw <= cw;
    let overflow = check_record(
        "overflow",
        checks.iter().any(|c| c == "overflow"),
        json!({
            "scrollWidth": sw, "clientWidth": cw, "excess": sw - cw
        }),
        overflow_pass,
        if overflow_pass {
            "no horizontal page overflow"
        } else {
            "the page scrolls horizontally"
        },
    );

    let contrast_obs = m.get("contrast").cloned().unwrap_or(Value::Null);
    let contrast_measured = contrast_obs.get("ratio").and_then(Value::as_f64).is_some();
    let contrast_pass = contrast_measured
        && contrast_obs
            .get("ratio")
            .and_then(Value::as_f64)
            .is_some_and(|r| r >= 4.5);
    let contrast_note = if !contrast_measured {
        "contrast could not be measured: no text with a parseable colour was found on the page"
    } else if contrast_pass {
        "worst text meets 4.5:1"
    } else {
        "worst text is below 4.5:1"
    };
    let mut contrast = check_record(
        "contrast",
        checks.iter().any(|c| c == "contrast"),
        contrast_obs.clone(),
        contrast_pass,
        contrast_note,
    );
    contrast["formula"] = json!(CONTRAST_FORMULA);

    let mut focus_obs = m.get("focus").cloned().unwrap_or(Value::Null);
    // The restore result is part of the measurement, not a claim: every
    // viewport names the before/after focus beside the ring it read.
    if let Some(obj) = focus_obs.as_object_mut() {
        for key in ["focusRestored", "activeElementBefore", "activeElementAfter"] {
            if let Some(v) = m.get(key) {
                obj.insert(key.to_string(), v.clone());
            }
        }
    }
    let focus_measured = focus_obs
        .get("ringVisible")
        .and_then(Value::as_bool)
        .is_some();
    let focus_pass = focus_obs
        .get("ringVisible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let focus_note = if !focus_measured {
        "focus could not be measured: the page returned no focus observation"
    } else if focus_pass {
        "the focused control draws a computed outline"
    } else {
        "the focused control has no visible outline"
    };
    let focus = check_record(
        "focus",
        checks.iter().any(|c| c == "focus"),
        focus_obs,
        focus_pass,
        focus_note,
    );

    let small = m.get("smallTargets").cloned().unwrap_or(json!([]));
    let small_count = small.as_array().map(Vec::len).unwrap_or(0);
    let target = check_record(
        "targetSize",
        checks.iter().any(|c| c == "targetSize"),
        json!({
            "minimumPx": 24, "smallCount": small_count, "small": small
        }),
        small_count == 0,
        if small_count == 0 {
            "no clickable box is under 24px"
        } else {
            "named clickable boxes are under 24px"
        },
    );

    let mut record = json!({
        "width": read_width,
        "requestedWidth": width,
        "height": read_height,
        "deviceScaleFactor": dpr,
        "scrollWidth": sw,
        "overflow": overflow, "contrast": contrast, "focus": focus, "targetSize": target,
        "checks": checks
    });
    for key in ["focusRestored", "activeElementBefore", "activeElementAfter"] {
        if let Some(v) = m.get(key) {
            record[key.to_string()] = v.clone();
        }
    }
    Ok(record)
}

/// Fold each viewport record into its typed verdict, then fold the matrix
/// into the top-level verdict and its separate coverage signal. Pure: the
/// tests inject records here without a browser.
fn reply_body(mut viewports: Vec<Value>, checks: &[String], expected: usize) -> Value {
    for record in &mut viewports {
        let outcome = fold_viewport(record, checks);
        record["verdict"] = json!(outcome.verdict);
    }
    let (verdict, measured, unmeasured, failed) = aggregate(&viewports, checks, expected);
    let coverage = if unmeasured.is_empty() {
        "every requested check produced a measurement at every viewport".to_string()
    } else {
        format!("unmeasurable requested check(s): {}", unmeasured.join(", "))
    };
    // Focus is restored only when every viewport restored it: one failure
    // means the page was left focused somewhere the caller did not choose.
    let focus_restored = !viewports.is_empty()
        && viewports
            .iter()
            .all(|v| v.get("focusRestored").and_then(Value::as_bool) == Some(true));
    let active_before = viewports
        .first()
        .and_then(|v| v.get("activeElementBefore"))
        .cloned()
        .unwrap_or(Value::Null);
    let active_after = viewports
        .last()
        .and_then(|v| v.get("activeElementAfter"))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "viewports": viewports,
        "checks": checks,
        "verdict": verdict,
        "measured": measured,
        "evidence": {
            "viewportCount": expected,
            "measuredCount": viewports.len(),
            "checkCount": checks.len(),
            "contrastFormula": CONTRAST_FORMULA,
            "focusRestored": focus_restored,
            "activeElementBefore": active_before,
            "activeElementAfter": active_after,
            "unmeasuredChecks": unmeasured,
            "failedChecks": failed,
            "note": format!("verdict and each per-viewport verdict are pass only when every requested check was measured and passed at that scope, fail otherwise; measured is coverage only ({coverage}); unmeasurable checks are named in unmeasuredChecks and in their own note")
        }
    })
}

/// Dispatcher entry point for the B7 fill lane.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown("browser_audit", args, &["viewports", "checks"])?;
    let viewports = validate_viewports(args)?;
    let checks = validate_checks(args)?;

    // The override is connection-wide: leave the engine exactly as we found
    // it, including when a measurement in the middle of the matrix fails.
    // Remember the LIVE metrics too: viewport_reset only waits for a change,
    // so a read at an intermediate width can be mistaken for a restore.
    let original = browser.viewport_readback().await.ok();
    let before = browser.viewport_override().await;
    let mut records = Vec::with_capacity(viewports.len());
    let mut failure = None;
    for width in viewports.iter() {
        match measure_viewport(browser, *width, &checks).await {
            Ok(record) => records.push(record),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    let restore = match before {
        Some(params) => browser.viewport_set(params).await,
        None => browser.viewport_reset().await,
    };
    if let Some(e) = failure {
        return Err(e);
    }
    restore.map_err(ToolErr::fallback)?;
    if let Some((width, height, _)) = original {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if browser
                .viewport_readback()
                .await
                .is_ok_and(|(w, h, _)| w == width && h == height)
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(ToolErr::fallback(format!(
                    "browser_audit: viewport did not restore to {width}x{height}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    Ok(ok_text(
        reply_body(records, &checks, viewports.len()).to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        fold_viewport, reply_body, schema, validate_checks, validate_viewports, CONTRAST_FORMULA,
        ROW,
    };
    use serde_json::{json, Value};

    #[test]
    fn viewports_are_bounded_at_the_boundary() {
        let args = json!({"viewports": [360, 390, 768, 1024, 1440, 2560], "checks": ["overflow"]});
        assert_eq!(validate_viewports(&args).unwrap().len(), 6);
        let zero = json!({"viewports": [0]});
        assert!(validate_viewports(&zero).is_err());
        let huge = json!({"viewports": [5000]});
        assert!(validate_viewports(&huge).is_err());
        let many = json!({"viewports": [1,2,3,4,5,6,7,8,9,10,11,12,13]});
        assert!(validate_viewports(&many).is_err());
    }

    #[test]
    fn unknown_checks_are_rejected_not_silently_skipped() {
        let args = json!({"checks": ["overflow", "contrast", "focus", "targetSize"]});
        assert_eq!(validate_checks(&args).unwrap().len(), 4);
        let bad = json!({"checks": ["typography"]});
        assert!(validate_checks(&bad).is_err());
        assert_eq!(ROW, "B7");
    }

    #[test]
    fn contrast_formula_is_returned_not_assumed() {
        assert!(CONTRAST_FORMULA.contains("0.2126") && CONTRAST_FORMULA.contains("4.5"));
    }

    /// Inject one viewport record in the shape the live measurement builds,
    /// so the verdict fold can be tested without a browser.
    fn injected_viewport(observed: &[(&str, Value, bool)]) -> Value {
        let mut record = json!({"width": 800, "scrollWidth": 800});
        for (name, observation, pass) in observed {
            record[*name] = json!({
                "name": *name,
                "requested": true,
                "expected": "injected expectation",
                "observed": observation,
                "pass": pass,
                "note": format!("injected note for {name}")
            });
        }
        record
    }

    fn requested(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn top_level_verdict_fails_when_a_requested_check_fails() {
        let checks = requested(&["contrast"]);
        let views = vec![injected_viewport(&[(
            "contrast",
            json!({"ratio": 1.16, "selector": "p", "sample": "low contrast text here"}),
            false,
        )])];
        let reply = reply_body(views.clone(), &checks, 1);
        assert_eq!(reply["verdict"].as_str(), Some("fail"), "{reply}");
        assert_eq!(reply["measured"].as_str(), Some("pass"), "{reply}");
        assert_eq!(
            reply["viewports"][0]["verdict"].as_str(),
            Some("fail"),
            "{reply}"
        );
        assert!(reply["evidence"]["unmeasuredChecks"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(
            fold_viewport(&views[0], &checks).measured,
            "ratio was measured"
        );
    }

    #[test]
    fn top_level_verdict_passes_only_when_every_check_passes() {
        let checks = requested(&["contrast", "overflow"]);
        let views = vec![injected_viewport(&[
            ("contrast", json!({"ratio": 7.0}), true),
            (
                "overflow",
                json!({"scrollWidth": 800, "clientWidth": 800, "excess": 0}),
                true,
            ),
        ])];
        let reply = reply_body(views, &checks, 1);
        assert_eq!(reply["verdict"].as_str(), Some("pass"), "{reply}");
        assert_eq!(reply["measured"].as_str(), Some("pass"), "{reply}");
        assert_eq!(
            reply["viewports"][0]["verdict"].as_str(),
            Some("pass"),
            "{reply}"
        );
        assert_eq!(
            reply["evidence"]["failedChecks"].as_array().unwrap().len(),
            0,
            "{reply}"
        );
    }

    #[test]
    fn unmeasurable_check_fails_measured_and_names_the_check() {
        let checks = requested(&["contrast"]);
        let views = vec![injected_viewport(&[("contrast", Value::Null, false)])];
        let reply = reply_body(views, &checks, 1);
        assert_eq!(reply["measured"].as_str(), Some("fail"), "{reply}");
        assert_eq!(reply["verdict"].as_str(), Some("fail"), "{reply}");
        let unmeasured = reply["evidence"]["unmeasuredChecks"].as_array().unwrap();
        assert_eq!(unmeasured[0].as_str(), Some("contrast"));
        let evidence = reply["evidence"].to_string();
        assert!(
            evidence.contains("contrast"),
            "unmeasurable check must be named: {evidence}"
        );
    }

    #[test]
    fn per_viewport_verdict_aggregates_all_requested_checks() {
        let checks = requested(&["contrast", "overflow"]);
        let views = vec![injected_viewport(&[
            ("contrast", json!({"ratio": 8.0}), true),
            (
                "overflow",
                json!({"scrollWidth": 900, "clientWidth": 800, "excess": 100}),
                false,
            ),
        ])];
        let outcome = fold_viewport(&views[0], &checks);
        assert_eq!(outcome.verdict, "fail");
        assert!(outcome.measured, "both checks produced measurements");
        let reply = reply_body(views, &checks, 1);
        assert_eq!(
            reply["viewports"][0]["verdict"].as_str(),
            Some("fail"),
            "{reply}"
        );
        assert_eq!(reply["verdict"].as_str(), Some("fail"), "{reply}");
        assert_eq!(reply["measured"].as_str(), Some("pass"), "{reply}");
    }

    #[test]
    fn reply_schema_types_both_verdicts_identically() {
        let out = &schema()["outputSchema"];
        let vp_verdict = &out["properties"]["viewports"]["items"]["properties"]["verdict"];
        let top_verdict = &out["properties"]["verdict"];
        let top_measured = &out["properties"]["measured"];
        assert_eq!(
            vp_verdict, top_verdict,
            "both levels must be typed the same"
        );
        assert_eq!(top_measured, top_verdict, "measured uses the same enum");
        assert_eq!(vp_verdict["type"].as_str(), Some("string"));
        let required = out["required"].as_array().unwrap();
        assert!(
            required.iter().any(|k| k.as_str() == Some("measured")),
            "measured is required"
        );
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A real page over loopback HTTP: the audit must leave its focus as
    /// found. Shares the crate's browser lock with the other live tests.
    async fn serve_focus_fixture(body: &'static str) -> (String, tokio::task::JoinHandle<()>) {
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
                let mut buf = vec![0u8; 4096];
                let mut req = Vec::new();
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            req.extend_from_slice(&buf[..n]);
                            if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() > 8192 {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{}",
                    body.len(), body
                );
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://127.0.0.1:{port}/fixture"), srv)
    }

    /// Run the real tool rather than injecting a record: the restore happens
    /// inside MEASURE_JS and has to be read back from the live page.
    async fn run_audit(browser: &crate::Browser) -> Value {
        let env = crate::tools::call(
            "audit",
            &json!({"viewports": [800], "checks": ["focus"]}),
            browser,
        )
        .await;
        assert!(env.get("isError").is_none(), "audit errored: {env}");
        env.get("structuredContent")
            .expect("structuredContent")
            .clone()
    }

    async fn active_descriptor(browser: &crate::Browser) -> String {
        let raw = browser.eval(r#"var e=document.activeElement; e ? e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') : null"#)
            .await.expect("read activeElement");
        raw.as_str().unwrap_or("null").to_string()
    }

    #[tokio::test]
    async fn audit_restores_the_page_focus() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        const PAGE: &str = r#"<!doctype html><html><head><title>Focus fixture</title><style>input:focus,button:focus{outline:2px solid #333}</style></head><body><input id="name" type="text"><button id="go" type="button">Go</button></body></html>"#;
        let (url, srv) = serve_focus_fixture(PAGE).await;
        let browser = crate::metrics::launch_and_navigate(&url).await;

        browser
            .eval("(document.getElementById('name').focus(), true)")
            .await
            .expect("focus #name");
        let before = active_descriptor(&browser).await;
        assert_eq!(before, "input#name", "fixture must start focused on #name");

        let sc = run_audit(&browser).await;
        let after = active_descriptor(&browser).await;
        let ev = &sc["evidence"];
        assert_eq!(
            after, "input#name",
            "the page must still have #name focused: {after}"
        );
        assert_eq!(ev["focusRestored"].as_bool(), Some(true), "evidence: {ev}");
        assert_eq!(
            ev["activeElementBefore"].as_str(),
            Some("input#name"),
            "evidence: {ev}"
        );
        assert_eq!(
            ev["activeElementAfter"].as_str(),
            Some("input#name"),
            "evidence: {ev}"
        );
        let observed = &sc["viewports"][0]["focus"]["observed"];
        assert_eq!(
            observed["focusRestored"].as_bool(),
            Some(true),
            "viewport evidence: {observed}"
        );

        srv.abort();
        let _ = browser.close().await;
    }

    #[tokio::test]
    async fn audit_with_body_focus_leaves_body_focused() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        const PAGE: &str = r#"<!doctype html><html><head><title>Body focus fixture</title><style>input:focus{outline:2px solid #333}</style></head><body><input id="name" type="text"><button id="go" type="button">Go</button></body></html>"#;
        let (url, srv) = serve_focus_fixture(PAGE).await;
        let browser = crate::metrics::launch_and_navigate(&url).await;

        let before = active_descriptor(&browser).await;
        assert_eq!(before, "body", "nothing else is focused before the audit");

        let sc = run_audit(&browser).await;
        let after = active_descriptor(&browser).await;
        let ev = &sc["evidence"];
        assert_eq!(
            after, "body",
            "the audit must not move focus off body: {after}"
        );
        assert_eq!(ev["focusRestored"].as_bool(), Some(true), "evidence: {ev}");
        assert_eq!(
            ev["activeElementBefore"].as_str(),
            Some("body"),
            "evidence: {ev}"
        );
        assert_eq!(
            ev["activeElementAfter"].as_str(),
            Some("body"),
            "evidence: {ev}"
        );

        srv.abort();
        let _ = browser.close().await;
    }

    #[tokio::test]
    async fn audit_names_a_failed_restore() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        // The second control's focus handler removes the previously focused
        // input, so the saved element is detached when the audit tries to
        // restore it. That failure must be named, never reported as success.
        const PAGE: &str = r#"<!doctype html><html><head><title>Failed restore fixture</title><style>input:focus,button:focus{outline:2px solid #333}</style></head><body><input id="victim" type="text"><button id="remover" type="button" onfocus="var v=document.getElementById('victim'); if(v){v.remove();}">Remove</button></body></html>"#;
        let (url, srv) = serve_focus_fixture(PAGE).await;
        let browser = crate::metrics::launch_and_navigate(&url).await;

        browser
            .eval("(document.getElementById('victim').focus(), true)")
            .await
            .expect("focus #victim");
        let before = active_descriptor(&browser).await;
        assert_eq!(
            before, "input#victim",
            "fixture must start focused on #victim"
        );

        let sc = run_audit(&browser).await;
        let after = active_descriptor(&browser).await;
        let ev = &sc["evidence"];
        assert_eq!(
            after, "body",
            "a detached control cannot be focused: {after}"
        );
        assert_eq!(
            ev["focusRestored"].as_bool(),
            Some(false),
            "a failed restore is not a success: {ev}"
        );
        assert_eq!(
            ev["activeElementBefore"].as_str(),
            Some("input#victim"),
            "evidence: {ev}"
        );
        assert_eq!(
            ev["activeElementAfter"].as_str(),
            Some("body"),
            "evidence: {ev}"
        );
        let observed = &sc["viewports"][0]["focus"]["observed"];
        assert_eq!(
            observed["focusRestored"].as_bool(),
            Some(false),
            "viewport evidence: {observed}"
        );

        srv.abort();
        let _ = browser.close().await;
    }
}
