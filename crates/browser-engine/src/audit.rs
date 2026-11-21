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
  const focusStates = [];
  for (const el of focusables) {
    if (getComputedStyle(el).display === 'none') continue;
    el.focus();
    if (d.activeElement !== el) continue;
    const cs = getComputedStyle(el);
    focusStates.push({
      selector: describe(el), focused: true,
      outlineStyle: cs.outlineStyle, outlineWidth: cs.outlineWidth, outlineColor: cs.outlineColor,
      ringVisible: cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) > 0 && cs.outlineColor !== 'transparent'
    });
    el.blur();
  }
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
    contrast: worst, focus: focus, smallTargets: small, focusableCount: focusables.length});
})()"#;

/// Frozen input/output contract for row B7 (`browser_audit`).
pub fn schema() -> Value {
    json!({
        "name": "audit",
        "title": "Browser audit",
        "description": "Audit the live page at each requested viewport width. Measurements come from the page's own computed styles and boxes: scrollWidth vs clientWidth for overflow, WCAG relative-luminance contrast ratio with the formula returned beside it, a focused control's computed focus ring, and every clickable box under 24px. One evidence record per viewport carries width, scrollWidth, overflow, contrast, focus, targetSize and a per-viewport verdict; the top-level verdict is pass when every requested check produced a measurement at every viewport, or fail otherwise.",
        "inputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "integer", "minimum": 1}},
            "checks": {"type": "array", "items": {"type": "string", "enum": CHECKS}}
        }, "required": ["viewports", "checks"], "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "viewports": {"type": "array", "items": {"type": "object", "properties": {
                "width": {"type": "integer", "minimum": 0},
                "scrollWidth": {"type": "integer", "minimum": 0},
                "overflow": {}, "contrast": {}, "focus": {}, "targetSize": {},
                "verdict": {"type": "boolean"}
            }, "required": ["width", "scrollWidth", "overflow", "contrast", "focus", "targetSize", "verdict"]}},
            "checks": {"type": "array", "items": {"type": "string"}},
            "verdict": {"type": "string", "enum": ["pass", "fail"]},
            "evidence": {"type": "object"}
        }, "required": ["viewports", "checks", "verdict", "evidence"], "additionalProperties": false},
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
    let arr = args.get("viewports").and_then(|v| v.as_array()).ok_or_else(|| {
        invalid("browser_audit: 'viewports' must be an array of positive integers".to_string(), ToolReason::BadValue, "viewports")
    })?;
    if arr.is_empty() || arr.len() > 12 {
        return Err(invalid(
            format!("browser_audit: 'viewports' needs 1..12 widths, got {}", arr.len()),
            ToolReason::BadValue, "viewports",
        ));
    }
    arr.iter().map(|v| {
        let n = v.as_i64().filter(|n| *n > 0 && *n <= 4096).ok_or_else(|| {
            invalid(format!("browser_audit: viewport width must be 1..4096, got {v}"), ToolReason::BadValue, "viewports")
        })?;
        Ok(n)
    }).collect()
}

fn validate_checks(args: &Value) -> Result<Vec<String>, ToolErr> {
    let arr = args.get("checks").and_then(|v| v.as_array()).ok_or_else(|| {
        invalid("browser_audit: 'checks' must be an array of check names".to_string(), ToolReason::BadValue, "checks")
    })?;
    if arr.is_empty() {
        return Err(invalid("browser_audit: 'checks' must name at least one check".to_string(), ToolReason::MissingArg, "checks"));
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

/// Apply the device-metrics override, measure the page, and build one record.
async fn measure_viewport(browser: &Browser, width: i64, checks: &[String]) -> Result<Value, ToolErr> {
    browser.viewport_set(json!({
        "width": width, "height": 720, "deviceScaleFactor": 1, "mobile": false
    })).await.map_err(ToolErr::fallback)?;
    let (read_width, read_height, dpr) = browser.viewport_readback().await.map_err(ToolErr::fallback)?;
    let raw = browser.eval(MEASURE_JS).await.map_err(ToolErr::fallback)?;
    let text = raw.as_str().ok_or_else(|| {
        ToolErr::fallback("browser_audit: measurement did not return a JSON string".to_string())
    })?;
    let m: Value = serde_json::from_str(text).map_err(|e| {
        ToolErr::fallback(format!("browser_audit: bad measurement: {e}"))
    })?;
    let sw = m.get("scrollWidth").and_then(|v| v.as_i64()).unwrap_or(0);
    let cw = m.get("clientWidth").and_then(|v| v.as_i64()).unwrap_or(0);

    let overflow_pass = sw <= cw;
    let overflow = check_record("overflow", checks.iter().any(|c| c == "overflow"), json!({
        "scrollWidth": sw, "clientWidth": cw, "excess": sw - cw
    }), overflow_pass, if overflow_pass { "no horizontal page overflow" } else { "the page scrolls horizontally" });

    let contrast_obs = m.get("contrast").cloned().unwrap_or(Value::Null);
    let contrast_pass = contrast_obs.get("ratio").and_then(|v| v.as_f64()).is_some_and(|r| r >= 4.5);
    let mut contrast = check_record("contrast", checks.iter().any(|c| c == "contrast"),
        contrast_obs.clone(), contrast_pass,
        if contrast_pass { "worst text meets 4.5:1" } else { "worst text is below 4.5:1" });
    contrast["formula"] = json!(CONTRAST_FORMULA);

    let focus_obs = m.get("focus").cloned().unwrap_or(Value::Null);
    let focus_pass = focus_obs.get("ringVisible").and_then(|v| v.as_bool()).unwrap_or(false);
    let focus = check_record("focus", checks.iter().any(|c| c == "focus"), focus_obs, focus_pass,
        if focus_pass { "the focused control draws a computed outline" } else { "the focused control has no visible outline" });

    let small = m.get("smallTargets").cloned().unwrap_or(json!([]));
    let small_count = small.as_array().map(Vec::len).unwrap_or(0);
    let target = check_record("targetSize", checks.iter().any(|c| c == "targetSize"), json!({
        "minimumPx": 24, "smallCount": small_count, "small": small
    }), small_count == 0, if small_count == 0 { "no clickable box is under 24px" } else { "named clickable boxes are under 24px" });

    let requested = |name: &str| checks.iter().any(|c| c == name);
    let mut verdict = true;
    if requested("overflow") { verdict &= overflow_pass; }
    if requested("contrast") { verdict &= contrast_pass; }
    if requested("focus") { verdict &= focus_pass; }
    if requested("targetSize") { verdict &= small_count == 0; }

    Ok(json!({
        "width": read_width,
        "requestedWidth": width,
        "height": read_height,
        "deviceScaleFactor": dpr,
        "scrollWidth": sw,
        "overflow": overflow, "contrast": contrast, "focus": focus, "targetSize": target,
        "checks": checks,
        "verdict": verdict
    }))
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

    let measured = records.len() == viewports.len();
    let body = json!({
        "viewports": records,
        "checks": checks,
        "verdict": if measured { "pass" } else { "fail" },
        "evidence": {
            "viewportCount": viewports.len(),
            "measuredCount": records.len(),
            "checkCount": checks.len(),
            "contrastFormula": CONTRAST_FORMULA,
            "note": "verdict pass means every requested check produced a measurement at every viewport; each per-viewport verdict is whether the PAGE passed those checks"
        }
    });
    Ok(ok_text(body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{validate_checks, validate_viewports, CONTRAST_FORMULA, ROW};
    use serde_json::json;

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
}
