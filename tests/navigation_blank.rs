//! A requested page that never loads must not be reported as a success.
//! An agent that trusts `ok:true` then asserts against an unloaded document;
//! the tool must answer with the typed `blank_load` failure instead.

mod common;

use common::{is_error, structured, text_of, Session};
use serde_json::json;
use std::net::TcpListener;

#[test]
fn navigate_to_a_dead_port_is_not_a_blank_success() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").expect("free port");
        l.local_addr().expect("addr").port()
    }; // dropped: nothing is listening on this loopback port
    let url = format!("http://127.0.0.1:{port}/never-loads");
    let mut s = Session::spawn();

    let nav = s.call("browser_navigate", json!({"url": url}));

    assert!(
        is_error(&nav),
        "navigation to a dead port must not be ok:true: {}",
        text_of(&nav)
    );
    let code = structured(&nav)["code"].as_str().unwrap_or("").to_string();
    assert_eq!(
        code, "blank_load",
        "a requested page that never loaded must carry the typed code: {}",
        text_of(&nav)
    );
}
