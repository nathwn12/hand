//! hand: one Rust MCP server for headless browser testing and Windows desktop
//! computer use.
//!
//! `hand` with no arguments serves MCP over stdio. Diagnostics go to
//! stderr; stdout carries protocol frames only.

#![windows_subsystem = "windows"]

/// Informational logs are opt-in: `HAND_DEBUG=1` or `HAND_VERBOSE=1`.
fn verbose_logs() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        ["HAND_DEBUG", "HAND_VERBOSE"]
            .iter()
            .any(|k| std::env::var(k).is_ok_and(|v| v == "1" || v == "true"))
    });
    *ON
}

macro_rules! debug_eprintln {
    ($($arg:tt)*) => {
        if crate::verbose_logs() { eprintln!($($arg)*); }
    };
}

macro_rules! err_eprintln {
    ($code:literal, $($arg:tt)*) => {
        eprintln!("[hand] {}: {}", $code, format!($($arg)*));
    };
}

mod catalog;
mod server;

fn main() {
    // Set process DPI awareness before any window or monitor query so Win32
    // coordinates are physical pixels, never virtualised by Windows scaling.
    desktop_engine::enable_per_monitor_dpi_v2();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("mcp") => run_server(),
        Some("--version") | Some("-V") | Some("version") => {
            println!("{}", version_line());
            0
        }
        Some("doctor") => doctor(),
        Some(other) => {
            err_eprintln!(
                "EUSAGE",
                "Unknown subcommand '{other}'; use hand with mcp, doctor or --version."
            );
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
    let handle = std::thread::Builder::new()
        .name("hand".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            // Nested tool dispatch (form -> per-step call + evidence capture)
            // needs more than tokio's 2 MB worker default at opt-level=0; debug
            // overflows where release fits. Match the main thread's 8 MB bound.
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .thread_stack_size(8 * 1024 * 1024)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    err_eprintln!("ERUNTIME", "[hand] failed to start the async runtime: {e}");
                    return 1;
                }
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.block_on(server::serve())
            }));
            let code = match result {
                Ok(Ok(())) => 0,
                Ok(Err(e)) => {
                    err_eprintln!("ESERVE", "[hand] {e}");
                    1
                }
                Err(_) => {
                    err_eprintln!(
                        "EPANIC",
                        "[hand] server loop panicked; its normal shutdown did not run, so an \
                         attached browser was NOT detached and any page state it changed is \
                         UNVERIFIED; in-flight desktop actions' outcomes are UNKNOWN"
                    );
                    1
                }
            };
            let started = std::time::Instant::now();
            rt.shutdown_timeout(RUNTIME_EXIT_BOUND);
            if started.elapsed() >= RUNTIME_EXIT_BOUND {
                let outstanding =
                    server::ACTIVE_DESKTOP_JOBS.load(std::sync::atomic::Ordering::SeqCst);
                if outstanding > 0 {
                    err_eprintln!(
                        "ESHUTDOWN",
                        "[hand] shutdown: runtime teardown reached its {RUNTIME_EXIT_BOUND:?} \
                         bound; {outstanding} blocking desktop job(s) were still running and were \
                         NOT stopped, so their outcome is UNKNOWN (the process exits now)"
                    );
                } else {
                    debug_eprintln!(
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
            err_eprintln!("ETHREAD", "[hand] failed to start the server thread: {e}");
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
    let names = catalog::catalog_names();
    let browser_tools = names
        .iter()
        .filter(|name| name.starts_with(catalog::BROWSER_PREFIX))
        .count();
    let desktop_tools = names
        .iter()
        .filter(|name| name.starts_with(catalog::DESKTOP_PREFIX))
        .count();
    let mut browser = browser_engine::provenance();
    if let Some(obj) = browser.as_object_mut() {
        // Doctor reports served tools, including hand's `browser_context`.
        obj.insert("chromium".to_string(), chromium);
        obj.insert("toolCount".to_string(), serde_json::json!(browser_tools));
        obj.insert("backend".to_string(), browser_engine::backend_info());
    }
    let mut desktop = desktop_engine::provenance();
    if let Some(obj) = desktop.as_object_mut() {
        obj.insert("toolCount".to_string(), serde_json::json!(desktop_tools));
        obj.insert(
            "dpiAwareness".to_string(),
            serde_json::json!(desktop_engine::current_dpi_awareness().as_str()),
        );
    }
    let report = serde_json::json!({
        "name": "hand",
        "executable": "hand",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "browser": browser,
        "desktop": desktop,
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

    /// Doctor's pins and versions come from the engine crates.
    #[test]
    fn doctor_engine_pins_match_the_crate_constants() {
        let report = doctor_report();
        assert_eq!(
            report["browser"]["sourcePin"],
            serde_json::json!(browser_engine::UPSTREAM_PIN)
        );
        assert_eq!(
            report["browser"]["version"],
            serde_json::json!(browser_engine::UPSTREAM_VERSION)
        );
        assert_eq!(
            report["desktop"]["sourcePin"],
            serde_json::json!(desktop_engine::UPSTREAM_PIN)
        );
        assert_eq!(
            report["desktop"]["version"],
            serde_json::json!(desktop_engine::UPSTREAM_VERSION)
        );
    }

    /// The published identity must be the compiled-in build, not a guess from
    /// where the file happens to live. `unknown` is only acceptable for a
    /// source tarball with no `.git`, which the tracked build never is here.
    #[test]
    fn version_line_carries_the_embedded_build_identity() {
        let line = version_line();
        assert!(
            line.starts_with(&format!("hand {}", env!("CARGO_PKG_VERSION"))),
            "{line}"
        );
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
