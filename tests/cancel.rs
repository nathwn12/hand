//! Cancellation semantics, per domain.
//!
//! Browser: a cancelled call answers immediately with the typed `cancelled`
//! result; its late result is discarded, and the session stays healthy.
//! Desktop: Puppet's contract is silence, proven through the production job
//! wrapper with the token published before the worker starts.

mod common;

use std::time::Duration;

use common::{is_error, structured, text_of, Session};
use serde_json::json;

#[test]
fn browser_cancel_answers_typed_then_discards_the_late_result() {
    let mut s = Session::spawn();
    // Warm the engine first: the cancellation is about a running call, not
    // about launch timing.
    let warm = s.call("browser_evaluate", json!({"expression": "1 + 1"}));
    assert_eq!(structured(&warm)["value"], json!(2));

    s.send(&json!({"jsonrpc": "2.0", "id": 900, "method": "tools/call",
        "params": {"name": "browser_wait_for", "arguments": {"ms": 3000}}}));
    std::thread::sleep(Duration::from_millis(200));
    s.send(
        &json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 900}}),
    );

    let cancelled = s
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel answer arrives promptly");
    assert_eq!(cancelled["id"], json!(900));
    assert!(
        is_error(&cancelled),
        "cancellation is a typed tool error: {cancelled}"
    );
    assert_eq!(
        structured(&cancelled)["code"].as_str(),
        Some("cancelled"),
        "typed code: {cancelled}"
    );
    assert_eq!(
        structured(&cancelled)["reason"].as_str(),
        Some("client_cancelled")
    );
    assert!(
        text_of(&cancelled).contains("cancelled"),
        "prose names the cause: {}",
        text_of(&cancelled)
    );

    // The engine call finishes later; its result must never be written.
    let late = s.recv_timeout(Duration::from_millis(3500));
    assert!(
        late.is_none(),
        "a cancelled browser call must not emit a second response: {late:?}"
    );

    // The session and the engine are still usable.
    let pong = s.request("ping", json!({}));
    assert_eq!(pong["result"], json!({}));
    let after = s.call("browser_evaluate", json!({"expression": "2 + 3"}));
    assert_eq!(structured(&after)["value"], json!(5));
}

#[test]
fn desktop_cancel_token_published_before_the_worker_suppresses_the_result() {
    // The reader publishes this key before the job is handed to a thread.
    desktop_engine::register_call("hand-cancel-1");
    desktop_engine::cancel_call("hand-cancel-1");
    let out = desktop_engine::call_tool(
        json!("hand-cancel-1"),
        "hand-cancel-1",
        json!({"name": "cursor", "arguments": {}}),
    );
    assert!(
        out.is_none(),
        "a cancelled desktop job stays silent, got: {out:?}"
    );

    // An uncancelled call through the same wrapper still answers.
    let ok = desktop_engine::call_tool(
        json!("hand-cancel-2"),
        "hand-cancel-2",
        json!({"name": "cursor", "arguments": {}}),
    )
    .expect("an uncancelled job answers");
    assert!(ok.get("result").is_some(), "result frame: {ok}");
    // Cancelling a completed id is a no-op and cannot rewrite its answer.
    desktop_engine::cancel_call("hand-cancel-2");
    let again = desktop_engine::call_tool(
        json!("hand-cancel-3"),
        "hand-cancel-3",
        json!({"name": "cursor", "arguments": {}}),
    );
    assert!(again.is_some(), "a new id is unaffected by the old cancel");
}
