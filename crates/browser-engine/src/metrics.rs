//! Read-only page performance metrics via Runtime.evaluate.
//!
//! Every number here is read from the page's own Performance APIs: navigation
//! timing for TTFB/DCL/load, and buffered PerformanceObserver entries for
//! paint, largest-contentful-paint, layout-shift and event. A metric with no
//! backing entry is reported as null with a reason, never a fabricated 0.
//! The tool dispatches through the same call table as every other browser
//! tool and targets the active legacy page or one page of a named isolated
//! context.

use serde_json::{json, Value};

use crate::browser::Browser;
use crate::tools::{check_unknown, ok_text, optional_string_arg, ToolCode, ToolErr, ToolReason};

/// Frozen tool contract for browser_metrics.
pub fn schema() -> Value {
    json!({
        "name": "metrics",
        "title": "Metrics",
        "description": "Read measured page performance from the page's own Performance APIs (Runtime.evaluate, read-only). Reports ttfbMs, domContentLoadedMs and loadMs from navigation timing; fcpMs and lcpMs from paint and largest-contentful-paint entries; cls from layout-shift entries; and inpMs from event entries (an interaction-latency proxy). Every number is measured: a metric whose entry type has no data is null with a reason string, never a fabricated 0. context=<name> reads a page of that NAMED isolated browser context (page= selects a specific page); without context= the active legacy page is read. Evaluates no mutations and changes no browser state.",
        "inputSchema": {"type": "object", "properties": {
            "context": {"type": "string", "description": "read a page of this named isolated browser context instead of the legacy page"},
            "page": {"type": "string", "description": "targetId of the context page to read; required when the context holds more than one page"}
        }, "additionalProperties": false},
        "outputSchema": {"type": "object", "properties": {
            "url": {"type": "string"},
            "ttfbMs": {"type": ["number", "null"]},
            "domContentLoadedMs": {"type": ["number", "null"]},
            "loadMs": {"type": ["number", "null"]},
            "fcpMs": {"type": ["number", "null"]},
            "lcpMs": {"type": ["number", "null"]},
            "cls": {"type": ["number", "null"]},
            "inpMs": {"type": ["number", "null"]},
            "observedAt": {"type": "string"},
            "source": {"type": "string"},
            "reason": {"type": "string"},
            "reasons": {"type": "object"},
            "ttfbMsReason": {"type": "string"},
            "domContentLoadedMsReason": {"type": "string"},
            "loadMsReason": {"type": "string"},
            "fcpMsReason": {"type": "string"},
            "lcpMsReason": {"type": "string"},
            "clsReason": {"type": "string"},
            "inpMsReason": {"type": "string"}
        }, "required": ["url", "ttfbMs", "domContentLoadedMs", "loadMs", "fcpMs", "lcpMs", "cls", "inpMs", "observedAt", "source"],
          "additionalProperties": false},
        "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
    })
}

/// One expression, evaluated through Runtime.evaluate with
/// awaitPromise true (the same CDP command browser_evaluate uses). Buffered
/// observers let entries recorded during navigation be read after load. No
/// write, no synthetic entry, no clock we mint ourselves.
const READ_METRICS_JS: &str = r#"(() => new Promise(resolve => {
  var nav = (performance.getEntriesByType('navigation') || [])[0] || {};
  var start = (typeof nav.startTime === 'number') ? nav.startTime : 0;
  var rel = function(t) {
    return (typeof t === 'number' && isFinite(t)) ? Math.max(0, t - start) : null;
  };
  var types = ['paint', 'largest-contentful-paint', 'layout-shift', 'event'];
  var buckets = {};
  types.forEach(function(t) { buckets[t] = []; });
  var observers = [];
  types.forEach(function(t) {
    try {
      var po = new PerformanceObserver(function(list) {
        list.getEntries().forEach(function(e) {
          buckets[t].push({
            name: e.name || '',
            startTime: (typeof e.startTime === 'number') ? e.startTime : null,
            value: (typeof e.value === 'number' && isFinite(e.value)) ? e.value : null,
            duration: (typeof e.duration === 'number' && isFinite(e.duration)) ? e.duration : null,
            hadRecentInput: (typeof e.hadRecentInput === 'boolean') ? e.hadRecentInput : null,
            size: (typeof e.size === 'number') ? e.size : null
          });
        });
      });
      po.observe({type: t, buffered: true});
      observers.push(po);
    } catch (e) { /* entry type unsupported here: left unmeasured, with a reason */ }
  });
  setTimeout(function() {
    observers.forEach(function(po) { try { po.disconnect(); } catch (e) {} });
    var fcpEntry = null;
    for (var i = 0; i < buckets['paint'].length; i++) {
      if (buckets['paint'][i].name === 'first-contentful-paint') { fcpEntry = buckets['paint'][i]; break; }
    }
    var fcp = (fcpEntry && typeof fcpEntry.startTime === 'number') ? Math.max(0, fcpEntry.startTime) : null;
    var lcpEntries = buckets['largest-contentful-paint'];
    var lcp = null;
    if (lcpEntries.length) {
      var lastLcp = lcpEntries[lcpEntries.length - 1];
      if (typeof lastLcp.startTime === 'number' && isFinite(lastLcp.startTime)) {
        lcp = Math.max(0, lastLcp.startTime);
      }
    }
    var shifts = buckets['layout-shift'].filter(function(e) {
      return e.hadRecentInput !== true && typeof e.value === 'number';
    });
    var cls = shifts.length ? shifts.reduce(function(a, e) { return a + e.value; }, 0) : null;
    var evts = buckets['event'].filter(function(e) { return typeof e.duration === 'number'; });
    var inp = evts.length ? Math.max(0, Math.max.apply(null, evts.map(function(e) { return e.duration; }))) : null;
    var ttfb = rel(nav.responseStart);
    var dcl = rel(nav.domContentLoadedEventEnd);
    var load = rel(nav.loadEventEnd);
    var reasons = {};
    var why = [];
    function unmeasured(field, text) {
      reasons[field] = text;
      why.push(field);
    }
    if (ttfb === null) unmeasured('ttfbMs', 'navigation timing has no responseStart (no network navigation)');
    if (dcl === null) unmeasured('domContentLoadedMs', 'navigation timing has no domContentLoadedEventEnd');
    if (load === null) unmeasured('loadMs', 'navigation timing has no loadEventEnd');
    if (fcp === null) unmeasured('fcpMs', 'no first-contentful-paint paint entry (unmeasured, not zero)');
    if (lcp === null) unmeasured('lcpMs', 'no largest-contentful-paint entry (unmeasured, not zero)');
    if (cls === null) unmeasured('cls', 'no layout-shift entry with a value (unmeasured, not zero)');
    if (inp === null) unmeasured('inpMs', 'no event entry: no measured interaction (unmeasured, not zero)');
    var out = {
      url: location.href,
      ttfbMs: ttfb,
      domContentLoadedMs: dcl,
      loadMs: load,
      fcpMs: fcp,
      lcpMs: lcp,
      cls: cls,
      inpMs: inp,
      observedAt: new Date().toISOString(),
      source: "Runtime.evaluate: performance.getEntriesByType('navigation') + buffered PerformanceObserver entries (paint, largest-contentful-paint, layout-shift, event)",
      reasons: reasons
    };
    Object.keys(reasons).forEach(function(field) { out[field + 'Reason'] = reasons[field]; });
    if (why.length) out.reason = 'unmeasured (null, not zero): ' + why.join(', ');
    resolve(JSON.stringify(out));
  }, 0);
}))()"#;

fn invalid(prose: String, reason: ToolReason, arg: &str) -> ToolErr {
    ToolErr::typed(
        prose,
        json!({"code": ToolCode::InvalidArgs.as_str(), "reason": reason.as_str(),
               "tool": "browser_metrics", "arg": arg}),
    )
}

/// Dispatcher entry point, called from tools::call.
pub async fn handle(browser: &Browser, args: &Value) -> Result<Value, ToolErr> {
    check_unknown("metrics", args, &["context", "page"])?;
    let context = optional_string_arg("browser_metrics", args, "context")?;
    let page = optional_string_arg("browser_metrics", args, "page")?;
    if page.is_some() && context.is_none() {
        return Err(invalid(
            "browser_metrics: 'page' names a page inside a named context; pass 'context' with it"
                .to_string(),
            ToolReason::MissingArg,
            "context",
        ));
    }
    let page_ctx = match context.as_deref() {
        Some(name) => Some(
            browser
                .context_page(name, page.as_deref())
                .await
                .map_err(ToolErr::from)?,
        ),
        None => None,
    };
    let raw = match page_ctx.as_ref() {
        Some(cp) => browser.eval_in_session(&cp.session, READ_METRICS_JS).await,
        None => browser.eval(READ_METRICS_JS).await,
    }
    .map_err(ToolErr::from)?;
    let text = raw.as_str().ok_or_else(|| {
        ToolErr::fallback("browser_metrics: Runtime.evaluate returned no string".to_string())
    })?;
    let mut body: Value = serde_json::from_str(text)
        .map_err(|e| ToolErr::fallback(format!("browser_metrics: bad metrics value: {e}")))?;
    if let (Some(cp), Some(obj)) = (&page_ctx, body.as_object_mut()) {
        obj.insert("context".to_string(), json!(cp.context));
        obj.insert("target".to_string(), json!(cp.target));
    }
    Ok(ok_text(body.to_string()))
}

/// Serialise the crate's real-browser tests: simultaneous Chromium launches
/// under a loaded test runner can stall a CDP response, which is a harness
/// flake, not an engine defect. Shared by this module and assert's tests.
#[cfg(test)]
pub(crate) static BROWSER_FIXTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Launch an owned browser and navigate through the normal entry point,
/// retrying a transient CDP navigation failure with a fresh engine.
#[cfg(test)]
pub(crate) async fn launch_and_navigate(url: &str) -> Browser {
    let mut last = Value::Null;
    for attempt in 0..4 {
        let browser = Browser::new();
        let resp = crate::tools::call("navigate", &json!({"url": url}), &browser).await;
        if resp.get("isError").is_none() {
            return browser;
        }
        last = resp;
        let _ = browser.close().await;
        let backoff = 250u64 << attempt;
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.subsec_nanos() % 50) as u64)
            .unwrap_or(0);
        tokio::time::sleep(std::time::Duration::from_millis(backoff + jitter)).await;
    }
    let probe = probe_fixture(url).await;
    panic!("navigate failed after retries: {last}; fixture probe: {probe}");
}

/// Direct-from-the-test-process probe of the same fixture URL, used only to
/// explain a navigate failure: does loopback answer at all? Short string,
/// because it lands in a panic message.
#[cfg(test)]
async fn probe_fixture(url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let rest = match url.split_once("://").map(|(_, r)| r) {
        Some(r) => r,
        None => return format!("no answer: bad url {url}"),
    };
    let slash = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..slash];
    let path = if slash < rest.len() { &rest[slash..] } else { "/" };
    let port: u16 = match authority.rsplit(':').next().and_then(|p| p.parse().ok()) {
        Some(p) => p,
        None => return format!("no answer: bad port in {url}"),
    };
    let mut stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(s) => s,
        Err(e) => return format!("no answer: connect: {e}"),
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if let Err(e) = stream.write_all(req.as_bytes()).await {
        return format!("no answer: write: {e}");
    }
    let mut buf = [0u8; 64];
    match tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut buf)).await {
        Ok(Ok(n)) => {
            let head = String::from_utf8_lossy(&buf[..n]);
            let line = head.lines().next().unwrap_or("").trim();
            if line.is_empty() {
                "answered: empty response".to_string()
            } else {
                format!("answered: {line}")
            }
        }
        Ok(Err(e)) => format!("no answer: read: {e}"),
        Err(_) => "no answer: Elapsed".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{schema, READ_METRICS_JS};
    use crate::browser::Browser;
    use crate::tools::call;
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Reuse the crate's local-server pattern (tokio::TcpListener::bind
    /// loopback + tokio::spawn) to serve a real fixture page over HTTP.
    async fn serve_fixture() -> (String, tokio::task::JoinHandle<()>) {
        let ln = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback bind");
        let port = ln.local_addr().expect("loopback addr").port();
        let srv = tokio::spawn(async move {
            loop {
                let (mut s, _) = match ln.accept().await {
                    Ok(v) => v,
                    _ => break,
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
                    let body = "<!doctype html><html><body><div id='anchor'>metrics fixture</div><script>requestAnimationFrame(function(){ setTimeout(function(){ var d = document.createElement('div'); d.style.height = '120px'; d.style.background = '#ccc'; document.body.insertBefore(d, document.getElementById('anchor')); }, 30); });</script></body></html>";
                    let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: text/html\r\n\r\n{}",
                    body.len(), body
                );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}/fixture"), srv)
    }

    /// A speculative/preconnect socket that sends no headers must not stall
    /// the next navigation. Chrome opens such sockets; a serial accept loop
    /// that reads until `\r\n\r\n` would block forever behind one, and the
    /// real request surfaces as net::ERR_ABORTED.
    #[tokio::test]
    async fn fixture_server_survives_a_silent_preconnect() {
        let (url, srv) = serve_fixture().await;
        let port = url
            .split(':')
            .last()
            .and_then(|s| s.split('/').next())
            .and_then(|s| s.parse::<u16>().ok())
            .expect("fixture port");
        let silent = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("silent preconnect");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut real = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("real connection");
        real.write_all(b"GET /fixture HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .expect("request");
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), real.read(&mut buf))
            .await
            .expect("fixture blocked behind a silent preconnect")
            .expect("read");
        assert!(n > 0, "fixture sent an empty response");
        drop(silent);
        srv.abort();
    }

    fn metric_num(v: &Value, key: &str) -> f64 {
        v.get(key)
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("{key} must be a number, got {:?}", v.get(key)))
    }

    #[test]
    fn metrics_schema_is_read_only_and_null_is_documented() {
        let s = schema();
        assert_eq!(s["name"], "metrics");
        assert_eq!(s["annotations"]["readOnlyHint"], true);
        let desc = s["description"].as_str().unwrap_or("");
        assert!(
            desc.contains("null"),
            "description must explain null: {desc}"
        );
        assert!(
            desc.contains("read-only"),
            "description must say read-only: {desc}"
        );
        assert!(READ_METRICS_JS.contains("PerformanceObserver"));
        assert!(READ_METRICS_JS.contains("buffered: true"));
    }

    #[tokio::test]
    async fn metrics_reports_measured_values_and_reasons_for_nulls() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let (url, srv) = serve_fixture().await;
        let browser = crate::metrics::launch_and_navigate(&url).await;
        let bad = call("metrics", &json!({"bogus": 1}), &browser).await;
        assert_eq!(
            bad["structuredContent"]["code"], "invalid_args",
            "unknown arg refused: {bad}"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        let env = call("metrics", &json!({}), &browser).await;
        srv.abort();
        let _ = browser.close().await;
        assert!(env.get("isError").is_none(), "metrics errored: {env}");
        let sc = env.get("structuredContent").expect("structuredContent");
        for key in [
            "url",
            "ttfbMs",
            "domContentLoadedMs",
            "loadMs",
            "fcpMs",
            "lcpMs",
            "cls",
            "inpMs",
            "observedAt",
            "source",
        ] {
            assert!(sc.get(key).is_some(), "missing {key}: {sc}");
        }
        for key in ["ttfbMs", "domContentLoadedMs"] {
            let n = metric_num(sc, key);
            assert!(
                n.is_finite() && n >= 0.0,
                "{key} must be finite >= 0, got {n}"
            );
        }
        assert!(
            sc.get("cls").and_then(Value::as_f64).is_some(),
            "cls must be a number: {sc}"
        );
        let reasons = sc
            .get("reasons")
            .and_then(Value::as_object)
            .expect("reasons object");
        for (field, reason) in reasons {
            assert!(
                sc.get(field).and_then(Value::as_f64).is_none(),
                "{field} must be null when it has a reason"
            );
            assert!(
                reason.as_str().is_some_and(|s| !s.is_empty()),
                "{field} reason must be a string"
            );
        }
    }

    #[tokio::test]
    async fn metrics_null_lcp_start_time_is_null_with_reason() {
        let _guard = crate::metrics::BROWSER_FIXTURE_LOCK.lock().await;
        let browser = Browser::new();
        // Inject an LCP entry with no numeric startTime into the observer the
        // metrics expression reads; Math.max(0, null) used to fabricate 0.
        let stub = "window.PerformanceObserver = function(cb) { \
            this.observe = function(o) { \
              if (o.type === 'largest-contentful-paint') { \
                cb({ getEntries: function() { return [ \
                  { name: 'largest-contentful-paint', startTime: null, value: 1 } \
                ]; } }); \
              } \
            }; \
            this.disconnect = function() {}; \
          };";
        let raw = browser
            .eval(&format!("{stub}\n{READ_METRICS_JS}"))
            .await
            .expect("evaluate injected LCP");
        let _ = browser.close().await;
        let text = raw.as_str().expect("metrics JSON string");
        let v: Value = serde_json::from_str(text).expect("parse metrics");
        assert!(
            v["lcpMs"].is_null(),
            "null startTime must not become 0: {v}"
        );
        assert!(
            v["reasons"]["lcpMs"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "null lcpMs must carry a reason: {v}"
        );
    }
}
