//! Shared MCP session helpers for hand's integration tests.
//!
//! The real executable is spawned over pipes, so these tests exercise the
//! public end-user route: one process, one stdio MCP connection, the published
//! catalog.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

pub const TIMEOUT: Duration = Duration::from_secs(60);

pub struct Session {
    pub child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    next_id: i64,
}

impl Session {
    pub fn spawn() -> Self {
        Self::spawn_with_env(&[])
    }

    /// Spawn the server with a hermetic inherited environment.
    ///
    /// A developer's shell must not decide what a suite exercises: an inherited
    /// `GLADIUS_CDP_URL` would attach every "owned engine" assertion to their
    /// own browser, a budget env would change timing, and a capability env would
    /// remove the rendering path. The test hooks are stripped too, so only a
    /// test that opts in through `env` can enable them.
    ///
    /// Explicit `env` entries are applied AFTER the removals, because a test
    /// that deliberately starts an external fixture (`context_wire`) must still
    /// be able to pass its own URL. Inherited values never win.
    pub fn spawn_with_env(env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hand"));
        // Pin the knobs a suite depends on. Values are hand's documented
        // defaults (90 s budget, render caps, hooks off); an explicit `env`
        // entry below can still opt into a deliberate variant.
        cmd.env("HAND_TOOL_BUDGET_MS", "90000");
        cmd.env("GLADIUS_TOOL_BUDGET_MS", "90000");
        cmd.env("GLADIUS_BACKEND_CAPS", "render");
        cmd.env("HAND_TEST_HOOKS", "0");
        cmd.env_remove("GLADIUS_CDP_URL");
        cmd.env_remove("HAND_TEST_STUCK_DESKTOP_MS");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn hand");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let t = line.trim().to_string();
                        if !t.is_empty() && tx.send(t).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut session = Session {
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
        };
        session.initialize();
        session
    }

    pub fn send(&mut self, frame: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        let line = serde_json::to_string(frame).expect("frame serializes");
        stdin.write_all(line.as_bytes()).expect("write frame");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush frame");
    }

    /// Close the write half: the server must treat EOF as shutdown.
    pub fn close_stdin(&mut self) {
        self.stdin.take();
    }

    /// Wait for a bounded exit; panics (after killing) if the server hangs.
    pub fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("hand did not exit within {timeout:?} after EOF");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn recv(&mut self) -> Value {
        let line = self
            .rx
            .recv_timeout(TIMEOUT)
            .expect("a response within the timeout");
        serde_json::from_str(&line).expect("response is JSON")
    }

    /// `Some(frame)` within `timeout`, `None` when nothing arrived.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<Value> {
        self.rx
            .recv_timeout(timeout)
            .ok()
            .map(|line| serde_json::from_str(&line).expect("response is JSON"))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.recv()
    }

    pub fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
    }

    pub fn call_id(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}}));
        self.recv()
    }

    pub fn list_tools(&mut self) -> Vec<Value> {
        let resp = self.request("tools/list", json!({}));
        resp.get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_else(|| panic!("tools/list must carry tools, got: {resp}"))
    }

    fn initialize(&mut self) {
        let resp = self.request(
            "initialize",
            json!({"protocolVersion": "2025-11-25",
                   "capabilities": {},
                   "clientInfo": {"name": "hand-tests", "version": "0"}}),
        );
        assert!(
            resp.get("result").is_some(),
            "initialize must succeed, got: {resp}"
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn tool<'a>(tools: &'a [Value], name: &str) -> &'a serde_json::Map<String, Value> {
    tools
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
        .and_then(|t| t.as_object())
        .unwrap_or_else(|| panic!("tools/list must contain {name}"))
}

pub fn structured(resp: &Value) -> &Value {
    resp.get("result")
        .and_then(|r| r.get("structuredContent"))
        .unwrap_or_else(|| panic!("expected structuredContent, got: {resp}"))
}

pub fn is_error(resp: &Value) -> bool {
    resp.get("result")
        .and_then(|r| r.get("isError"))
        .and_then(|v| v.as_bool())
        == Some(true)
}

pub fn text_of(resp: &Value) -> String {
    resp.get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
