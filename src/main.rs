//! hand-mcp: one Rust MCP server for browser testing and Windows desktop
//! computer use, integrating the Gladius and Puppet engines in one process.
//!
//! `hand-mcp` with no arguments serves MCP over stdio. Diagnostics go to
//! stderr; stdout carries protocol frames only.

// No console window: an MCP client spawns this process and owns its pipes.
#![windows_subsystem = "windows"]

mod catalog;
mod server;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("mcp") => run_server(),
        Some("--version") | Some("-V") | Some("version") => {
            println!(
                "hand-mcp {} (gladius 59c6d77, puppet 0f790be; {})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH
            );
            0
        }
        Some("doctor") => doctor(),
        Some(other) => {
            eprintln!("usage: hand-mcp [mcp|doctor|--version]; unknown subcommand: {other}");
            2
        }
    };
    std::process::exit(code);
}

/// Runtime teardown bound after `serve()` returns. Tokio's `Runtime::drop`
/// waits forever for a started `spawn_blocking` task (Tokio 1.54:
/// "blocking functions ... keep running until they return" and "Drop waits
/// forever"), so a desktop job stuck in a UIA call would hold the process past
/// every drain bound. `shutdown_timeout` stops waiting at this bound; the
/// process then exits, which ends the blocked thread. It does NOT cancel the
/// job: an action it had already delivered keeps its outcome unknown, and the
/// log says so.
const RUNTIME_EXIT_BOUND: std::time::Duration = std::time::Duration::from_secs(2);

fn run_server() -> i32 {
    // The browser launch + ws connect chain is deep in a debug profile. Give
    // the runtime a known stack instead of trusting the process default.
    let handle = std::thread::Builder::new()
        .name("hand-mcp".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("[hand] failed to start the async runtime: {e}");
                    return 1;
                }
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.block_on(server::serve())
            }));
            let code = match result {
                Ok(Ok(())) => 0,
                Ok(Err(e)) => {
                    eprintln!("[hand] {e}");
                    1
                }
                Err(_) => {
                    // The hook already printed the location, not the payload.
                    // The normal shutdown did NOT run, so say exactly what that
                    // leaves unverified; do not claim a restore or a detach.
                    eprintln!(
                        "[hand] server loop panicked; its normal shutdown did not run, so an \
                         attached browser was NOT detached and any page state it changed is \
                         UNVERIFIED; in-flight desktop actions' outcomes are UNKNOWN"
                    );
                    1
                }
            };
            // `serve()` has closed/detached the browser and flushed the writer.
            // Only blocking threads can remain; bound their teardown so the
            // process cannot hang behind a desktop job that cannot be aborted.
            let started = std::time::Instant::now();
            rt.shutdown_timeout(RUNTIME_EXIT_BOUND);
            if started.elapsed() >= RUNTIME_EXIT_BOUND {
                let outstanding =
                    server::ACTIVE_DESKTOP_JOBS.load(std::sync::atomic::Ordering::SeqCst);
                if outstanding > 0 {
                    eprintln!(
                        "[hand] shutdown: runtime teardown reached its {RUNTIME_EXIT_BOUND:?} \
                         bound; {outstanding} blocking desktop job(s) were still running and were \
                         NOT stopped, so their outcome is UNKNOWN (the process exits now)"
                    );
                } else {
                    eprintln!(
                        "[hand] shutdown: runtime teardown reached its {RUNTIME_EXIT_BOUND:?} \
                         bound before teardown completed; the process exits now"
                    );
                }
            }
            code
        });
    match handle {
        Ok(h) => h.join().unwrap_or(1),
        Err(e) => {
            eprintln!("[hand] failed to start the server thread: {e}");
            1
        }
    }
}

/// Local diagnostics: no MCP connection, no state change, no browser launch.
fn doctor() -> i32 {
    let chromium = match browser_engine::chromium_path() {
        Ok(path) => serde_json::json!({"path": path.display().to_string(), "provisioned": true}),
        Err(e) => serde_json::json!({"provisioned": false, "error": e}),
    };
    let browser_tools = browser_engine::list()
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let desktop_tools = desktop_engine::tools_list()
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let report = serde_json::json!({
        "name": "hand",
        "executable": "hand-mcp",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "browser": {
            "engine": "gladius",
            "sourcePin": "59c6d77",
            "version": "0.8.0",
            "backend": browser_engine::backend_info(),
            "toolCount": browser_tools,
            "chromium": chromium,
        },
        "desktop": {
            "engine": "puppet",
            "sourcePin": "0f790be",
            "version": "8.0.0",
            "toolCount": desktop_tools,
        },
        "catalog": match catalog::validate() {
            Ok(()) => serde_json::json!({"valid": true, "count": browser_tools + desktop_tools}),
            Err(e) => serde_json::json!({"valid": false, "error": e}),
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    0
}
