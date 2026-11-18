//! hand: one Rust MCP server for browser testing and Windows desktop
//! computer use, integrating the Gladius and Puppet engines in one process.
//!
//! `hand` with no arguments serves MCP over stdio. Diagnostics go to
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
            println!("{}", version_line());
            0
        }
        Some("doctor") => doctor(),
        Some(other) => {
            eprintln!("usage: hand [mcp|doctor|--version]; unknown subcommand: {other}");
            2
        }
    };
    std::process::exit(code);
}

/// The build identity, compiled in by `build.rs`: package version, commit and
/// cargo profile. Nothing here reads the executable's path or mtime, so a copy
/// at an arbitrary location (`%TEMP%\hand-copy.exe`) reports the same build the
/// release directory does.
fn version_line() -> String {
    let dirty = env!("HAND_GIT_DIRTY") == "1";
    format!(
        "hand {} ({}{}; {})",
        env!("CARGO_PKG_VERSION"),
        env!("HAND_GIT_SHA"),
        if dirty { "-dirty" } else { "" },
        env!("HAND_BUILD_PROFILE")
    )
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
        .name("hand".to_string())
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

/// Build the local diagnostics report. Split from `doctor` so the published
/// counts can be checked against the catalog the server actually serves.
fn doctor_report() -> serde_json::Value {
    let chromium = match browser_engine::chromium_path() {
        Ok(path) => serde_json::json!({"path": path.display().to_string(), "provisioned": true}),
        Err(e) => serde_json::json!({"provisioned": false, "error": e}),
    };
    // The served catalog is the single source of truth: the engine lists alone
    // miss hand's own `browser_context`, which is how doctor used to report 32
    // while the server published 33. Both counts and the split come from the
    // same names `tools/list` emits.
    let names = catalog::catalog_names();
    let browser_tools = names
        .iter()
        .filter(|name| name.starts_with(catalog::BROWSER_PREFIX))
        .count();
    let desktop_tools = names
        .iter()
        .filter(|name| name.starts_with(catalog::DESKTOP_PREFIX))
        .count();
    let report = serde_json::json!({
        "name": "hand",
        "executable": "hand",
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
            Ok(()) => serde_json::json!({"valid": true, "count": names.len()}),
            Err(e) => serde_json::json!({"valid": false, "error": e}),
        },
    });
    report
}

/// Local diagnostics: no MCP connection, no state change, no browser launch.
fn doctor() -> i32 {
    println!(
        "{}",
        serde_json::to_string_pretty(&doctor_report()).unwrap_or_default()
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published identity must be the compiled-in build, not a guess from
    /// where the file happens to live. `unknown` is only acceptable for a
    /// source tarball with no `.git`, which the tracked build never is here.
    #[test]
    fn version_line_carries_the_embedded_build_identity() {
        let line = version_line();
        assert!(line.starts_with(&format!("hand {}", env!("CARGO_PKG_VERSION"))), "{line}");
        assert!(line.contains(env!("HAND_GIT_SHA")), "{line}");
        assert!(line.contains(env!("HAND_BUILD_PROFILE")), "{line}");
        if env!("HAND_GIT_SHA") != "unknown" {
            assert!(!line.to_lowercase().contains("unknown"), "{line}");
            assert!(
                env!("HAND_GIT_SHA").len() == 40,
                "a git build must stamp the full commit: {line}"
            );
        }
        assert!(
            matches!(env!("HAND_BUILD_PROFILE"), "release" | "debug"),
            "cargo sets PROFILE for a build script: {line}"
        );
    }

    /// Doctor must report what the server publishes, not the engine's raw list:
    /// `browser_context` is hand's own tool and belongs in the browser count.
    #[test]
    fn doctor_reports_the_served_catalog() {
        let report = doctor_report();
        let names = catalog::catalog_names();
        let browser = names
            .iter()
            .filter(|name| name.starts_with(catalog::BROWSER_PREFIX))
            .count();
        let desktop = names
            .iter()
            .filter(|name| name.starts_with(catalog::DESKTOP_PREFIX))
            .count();
        assert_eq!(report["catalog"]["count"], serde_json::json!(names.len()));
        assert_eq!(report["browser"]["toolCount"], serde_json::json!(browser));
        assert_eq!(report["desktop"]["toolCount"], serde_json::json!(desktop));
        assert_eq!(browser + desktop, names.len());
    }
}
