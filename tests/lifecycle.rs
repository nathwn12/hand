//! Lifecycle: EOF is shutdown. A launched owned browser must not outlive the
//! server, and the process must exit cleanly and promptly.

mod common;

use std::time::{Duration, Instant};

use common::{is_error, structured, Session};
use serde_json::json;

#[test]
fn eof_shuts_down_cleanly_after_the_browser_launched() {
    let mut s = Session::spawn();
    // Launch the owned engine and prove it serves.
    let ua = s.call(
        "browser_evaluate",
        json!({"expression": "navigator.userAgent"}),
    );
    assert!(!is_error(&ua), "evaluate failed");
    assert!(
        structured(&ua)["value"]
            .as_str()
            .unwrap_or("")
            .starts_with("Mozilla/5.0"),
        "the owned engine answered: {ua}"
    );

    s.close_stdin();
    let started = Instant::now();
    let status = s.wait_exit(Duration::from_secs(20));
    let elapsed = started.elapsed();
    assert!(status.success(), "EOF shutdown must exit 0, got: {status}");
    assert!(
        elapsed < Duration::from_secs(10),
        "shutdown must be bounded, took {elapsed:?}"
    );
}
