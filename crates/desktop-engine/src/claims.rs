//! The claims source.
//!
//! Every load-bearing claim an agent can act on is written here exactly once:
//! the tool lines, the coordinate rule, the refusal table, the authoritative
//! code list, the invariants and the named limits. `instructions()` is
//! assembled from these consts, and `tests/contract_truth.rs` pins them to
//! `tools/list` and to the source that emits them.
//!
//! Nothing in this module talks to Windows. A claim is a string; the code
//! that honours it lives in `gate`, `burst`, `uia`, `shot`, `cap`, `winop`.

/// The seven tools, one line each, in `tools_list_result` order.
pub const TOOL_LINES: [(&str, &str); 8] = [
    (
        "screenshot",
        "capture the whole virtual desktop (display=-1), one monitor, a window (window=<selector> or target={...}) or a region, without activating the window; returns the image, source{x,y,w,h}, scale, the monitor list and the image-to-screen formula",
    ),
    (
        "diff",
        "compare two captures (base/next: window selector, file path or the previous capture) pixel by pixel; returns changed, total, ratio, bbox, identical and the optional diff image",
    ),
    (
        "list_windows",
        "read-only visible top-level windows (hwnd/pid/exe/title/rects/elevated); never changes focus",
    ),
    (
        "window",
        "move|resize|minimize|restore|close|topmost by selector, hwnd or target, all no-steal (SWP_NOACTIVATE / SW_SHOWMINNOACTIVE) and ignore allow_foreground; focus and maximize need allow_foreground:true (focus calls SetForegroundWindow, maximize uses ShowWindow(SW_MAXIMIZE)) and are refused no_foreground_policy without it",
    ),
    (
        "do",
        "BURST tool: one call runs an ordered actions[] batch against one target through UIA patterns first, then PostMessage; real pointer input and explicit focus need the top-level allow_cursor:true / allow_foreground:true opt-ins and a locked target; per-action ok, never aborts the rest",
    ),
    ("cursor", "read-only cursor position plus monitor rects"),
    (
        "elements",
        "read-only structured UI elements for a window (target/title/hwnd; zero pixels, no focus change); each entry carries ref, fp, rect, offscreen and invokable",
    ),
    (
        "record",
        "record ONE targeted window to a playable MJPEG AVI without stealing focus, pointer or clipboard; duration/fps/max_px/quality are bounded, overruns become droppedFrames, and a partial recording is reported with the frames written",
    ),
];

/// Which space a coordinate is in. The single rule every tool obeys.
pub const COORD_RULE: &str = "`do` x/y under a `target` are CLIENT-area physical pixels of that window, used as given; without a target they are virtual-desktop SCREEN pixels, converted with ScreenToClient; rects from `elements` and `list_windows` are always SCREEN pixels; a screenshot image pixel maps to screen with screen_x = source.x + round((img_x + 0.5) * source.w / image.w) and the same for y";

/// Every code the server can emit, one entry each. The authoritative list:
/// a code that is not here does not exist, and a code that is here is
/// emitted somewhere outside this file.
pub const ERROR_CODES: &[&str] = &[
    "ambiguous_target",
    "ambiguous_window",
    "background_unavailable",
    "bad_coordinates",
    "bad_display",
    "bad_out",
    "bad_region",
    "bad_target",
    "bad_window",
    "blank_frame",
    "burst_budget_exceeded",
    "capture_failed",
    "capture_timeout",
    "capture_unavailable",
    "clipboard_unavailable",
    "empty_crop",
    "expect_fp_required",
    "invalid_args",
    "no_clipboard_policy",
    "no_cursor_policy",
    "no_foreground_policy",
    "no_steal_violation",
    "no_text_format",
    "point_not_target",
    "occluded_unavailable",
    "offscreen_element",
    "restore_failed",
    "secure_desktop",
    "size_mismatch",
    "stale_frame",
    "stale_ref",
    "target_gone",
    "target_required",
    "too_large",
    "uia_timeout",
    "uipi_blocked",
    "window_minimized",
];

/// The verb matrix, derived from `Gate::decide` (`src/gate.rs`): what each
/// action type can be delivered as, and the code that refuses it when its
/// precondition is missing.
///
/// `class` is one of `posted` (a window message to the locked target),
/// `pattern` (a UIA pattern on the locked target), `read` (nothing is
/// delivered), `gated` (a clipboard write with `allow_clipboard:true`),
/// `cursor` (real pointer input with `allow_cursor:true` and a locked
/// target, after the point guard), `foreground` (SetForegroundWindow on the
/// locked target with `allow_foreground:true`) or `refused` (no flag
/// unlocks it). A verb with two rows takes both paths; the `refused` row of
/// such a verb is its element form.
pub const REFUSALS: &[(&str, &str, &str)] = &[
    ("move", "refused", "no_cursor_policy"),
    ("move", "cursor", "target_required"),
    ("click", "posted", "no_cursor_policy"),
    ("click", "cursor", "point_not_target"),
    ("click", "pattern", "target_required"),
    ("down", "posted", "no_cursor_policy"),
    ("up", "posted", "no_cursor_policy"),
    ("drag", "posted", "no_cursor_policy"),
    ("drag", "cursor", "point_not_target"),
    ("scroll", "posted", "no_cursor_policy"),
    ("scroll", "cursor", "point_not_target"),
    ("scroll", "pattern", "target_required"),
    ("type", "posted", "target_required"),
    ("type", "refused", "background_unavailable"),
    ("key", "posted", "target_required"),
    ("hotkey", "posted", "target_required"),
    ("wait", "read", ""),
    ("wait_for", "read", ""),
    ("focus_window", "refused", "no_foreground_policy"),
    ("focus_window", "foreground", "target_required"),
    ("invoke", "pattern", "target_required"),
    ("set_value", "refused", "background_unavailable"),
    ("clipboard_set", "gated", "no_clipboard_policy"),
    ("clipboard_get", "read", ""),
];

/// The invariants (I1ÔÇôI6), as an agent reads them.
pub const INVARIANTS: &[(&str, &str)] = &[
    (
        "I1",
        "SendInput, SetCursorPos and SetForegroundWindow have a call site only behind the explicit top-level allow_cursor/allow_foreground flags and the point guard; BringWindowToTop, mouse_event and every other writer have no call site anywhere",
    ),
    (
        "I2",
        "every refusal happens before anything is sent, under dry_run too",
    ),
    (
        "I3",
        "the target lock is revalidated before every action, so a vanished or replaced window refuses instead of being driven",
    ),
    (
        "I4",
        "nothing falls back to the foreground window: without a target an acting verb refuses; real pointer and focus delivery need a locked target, and a point outside it is refused point_not_target",
    ),
    (
        "I5",
        "the server never elevates itself and never escalates a permission",
    ),
    (
        "I6",
        "a property that never appears means the control has no such pattern, never that it is false",
    ),
];

/// The named limits: what this server cannot do, and the readings an agent
/// gets wrong. A limit an agent cannot read is not a limit.
pub const LIMITS: &[(&str, &str)] = &[
    (
        "bursts",
        "a burst is not atomic across a service restart: actions already sent are not undone, so keep bursts short and idempotent",
    ),
    (
        "ok_vs_effect",
        "`ok` means the action was delivered, never that the target acted: read `effect` for what actually happened",
    ),
    (
        "fp_scope",
        "`fp` is identity, never state: rect, text and control state (checked, selected, expanded, range) do not participate, so an fp match means the same control, not the same control in the same condition; a state-sensitive action needs a readback (effect, satisfied_by), never a fingerprint",
    ),
    (
        "wait_for",
        "`satisfied` is the condition's verdict and `ok` is the action's delivery: a timed-out wait_for reports satisfied:false with ok:true",
    ),
    (
        "clipboard",
        "clipboard_get truncates text past 1048576 chars and reports truncated:true; clipboard_set needs allow_clipboard:true, writes CF_UNICODETEXT under the same cap, and restore_clipboard:true puts the previous text back after the burst (empty or non-text content is cleared on restore)",
    ),
    (
        "pointer",
        "the pointer is never in the pixels - neither capture backend composites it; `cursor{x,y}` is a position report. With allow_cursor:true it is really moved (SetCursorPos/SendInput) only onto the locked window; cursor_restore:true puts it back, and cursorMoved/cursorRestored/cursorUnchanged report what happened",
    ),
    (
        "attribution",
        "`foregroundUnchanged`/`cursorUnchanged` are raw before/after samples, not a verdict: the same payload's `noSteal` block carries the executable's import table (`bannedImports` must be empty; `optInImports` names the three writers linked for the opt-in paths), the `GetLastInputInfo` ticks around the call, and whether a new foreground belongs to this process or the locked target. A pointer or foreground delta with real input behind it is the owner's; a delta with no input is unattributable and must be read as a defect",
    ),
    (
        "ime",
        "IME, dead-key and AltGr composition are not supported: there is no composition path on posted WM_CHAR",
    ),
    (
        "touch",
        "touch, pen and stylus input are not supported: there is no schema surface and no way to synthesize it in the background",
    ),
    (
        "drag_and_drop",
        "cross-window drag-and-drop is not supported: even with allow_cursor:true, drag is down/move/up at two guarded points on the same locked window",
    ),
    (
        "clamping",
        "a point outside the target client rect is posted into client message space and lands nowhere",
    ),
    (
        "hdr",
        "captures are SDR-composited, untagged bytes: there is no HDR or wide-gamut conversion",
    ),
    (
        "recording",
        "desktop_record writes a bounded MJPEG AVI of one locked window: a vanished target keeps the frames already captured and reports them as a typed partial result; zero frames writes no file",
    ),
    (
        "raw_input",
        "an app reading raw input (DirectInput) ignores posted messages and cannot be driven",
    ),
    (
        "elevation",
        "an elevated target cannot be driven: the refusal is typed and the server never elevates itself",
    ),
];

/// The usage contract a client that has never seen this server reads first.
/// Assembled from the consts above; no claim is typed twice.
pub fn instructions() -> String {
    let mut s = String::new();
    s.push_str(
        "puppet desktop-control MCP (Windows). Background-safe by default: every acting tool \
         takes a `target` selector {hwnd?,pid?,exe?,appId?,titleContains?,className?} that \
         resolves ONE window for the call and is revalidated before every action. By default \
         no cursor movement, no keyboard injection into the owner's session, no focus stealing \
         and no raised window, whether the target is foreground or alt-tabbed. An ambiguous or \
         vanished target is a typed error (ambiguous_target / target_gone). Real input is an \
         explicit opt-in, never a fallback: allow_cursor:true gives guarded pointer delivery \
         (SetCursorPos/SendInput, point_not_target off the locked window) and defaults to \
         restoring the entry pointer; allow_foreground:true gives ONE SetForegroundWindow on \
         the locked target, with focus_guard:true restoring the entry foreground. Both need a \
         locked target; delivery:\"sendinput\" stays refused. Clipboard writes need \
         allow_clipboard:true and are denied by default, so a refused call touches nothing.\n",
    );

    s.push_str("\nTools (one line each):\n");
    for (name, line) in TOOL_LINES {
        s.push_str(&format!("- {name}: {line}\n"));
    }

    s.push_str(&format!("\nCoordinates: {COORD_RULE}.\n"));

    s.push_str("\nVerbs by delivery class:\n");
    for class in [
        "posted",
        "pattern",
        "read",
        "gated",
        "cursor",
        "foreground",
        "refused",
    ] {
        let verbs: Vec<String> = REFUSALS
            .iter()
            .filter(|(_, c, _)| *c == class)
            .map(|(verb, c, code)| {
                let element_form = *c == "pattern"
                    || (*c == "refused"
                        && REFUSALS
                            .iter()
                            .any(|(v, oc, _)| v == verb && *oc != "refused"));
                let name = if element_form {
                    format!("{verb} with an element")
                } else {
                    verb.to_string()
                };
                if *c == "refused" || *c == "gated" {
                    format!("{name} ({code})")
                } else {
                    name
                }
            })
            .collect();
        let meaning = match class {
            "posted" => "window messages to the locked target",
            "pattern" => "UIA patterns on the locked target",
            "read" => "nothing is delivered",
            "gated" => "allowed only with allow_clipboard:true",
            "cursor" => "real pointer input only with allow_cursor:true and a locked target; point_not_target otherwise",
            "foreground" => "SetForegroundWindow only with allow_foreground:true and a locked target",
            _ => "no flag unlocks them",
        };
        s.push_str(&format!("- {class} ({meaning}): {}\n", verbs.join(", ")));
    }

    s.push_str("\nInvariants:\n");
    for (id, line) in INVARIANTS {
        s.push_str(&format!("- {id}: {line}\n"));
    }

    s.push_str("\nLimits:\n");
    for (name, line) in LIMITS {
        s.push_str(&format!("- {name}: {line}\n"));
    }

    s.push_str(&format!(
        "\nError codes (every code the server can emit): {}.\n",
        ERROR_CODES.join(", ")
    ));
    s.push_str(
        "dry_run=true validates exactly like the live call (every policy refusal included) but \
         posts nothing.",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six codes from the D10 row must be registered, published in the
    /// instructions, and unique. Without them this test is red.
    #[test]
    fn d10_codes_are_registered_and_published() {
        let required = [
            "occluded_unavailable",
            "secure_desktop",
            "blank_frame",
            "stale_frame",
            "too_large",
            "no_steal_violation",
        ];
        let published = instructions();
        for code in required {
            assert!(
                ERROR_CODES.contains(&code),
                "{code} missing from ERROR_CODES"
            );
            assert!(
                published.contains(code),
                "{code} missing from published instructions"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for code in ERROR_CODES {
            assert!(seen.insert(*code), "duplicate error code {code}");
        }
    }

    /// Every error-code literal emitted by either engine must be registered.
    /// The scan is deliberately narrow: it reads only code-bearing slots (a
    /// `code` field, a `gate.rs` code constant, the `win_code_of` mapping or
    /// the clipboard `wire()` tuple), so ordinary snake_case words are not
    /// swept in. The known-set assertion at the end stops a vacuous pass.
    #[test]
    fn emitted_code_literals_are_registered() {
        // Scanned as `code:` values, but neither is an error code.
        let not_codes: &[(&str, &str)] = &[
            ("frozen", "browser visual stable-poll verdict, not an error"),
            (
                "unfrozen",
                "browser visual stable-poll verdict, not an error",
            ),
        ];

        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let mut files = Vec::new();
        for src in ["crates/desktop-engine/src", "crates/browser-engine/src"] {
            collect_rs(&workspace.join(src), &mut files);
        }
        files.sort();

        let mut found = std::collections::BTreeSet::new();
        for path in &files {
            if path.ends_with("claims.rs") {
                continue; // the registry itself, not an emitter
            }
            let raw = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            // Comments emit nothing; blank comment-only lines before scanning.
            let text = raw
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("//") {
                        ""
                    } else {
                        line
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            scan_code_fields(&text, &mut found);
            if path.ends_with("gate.rs") {
                scan_code_consts(&text, &mut found);
            }
            if let Some(body) = fn_body(&text, "fn win_code_of") {
                scan_quoted(&body, &mut found);
            }
            if let Some(body) = fn_body(&text, "fn wire(&self)") {
                scan_quoted(&body, &mut found);
            }
        }

        for code in &found {
            if not_codes.iter().any(|(c, _)| *c == code.as_str()) {
                continue;
            }
            assert!(
                ERROR_CODES.contains(&code.as_str()),
                "emitted code literal {code:?} is not registered in ERROR_CODES"
            );
        }
        for known in [
            "restore_failed",
            "no_text_format",
            "no_clipboard_policy",
            "point_not_target",
        ] {
            assert!(
                found.contains(known),
                "scan did not find known code {known:?}; the scan is vacuous or broken"
            );
        }
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read dir {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("dir entry: {e}"));
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    fn is_code_shaped(s: &str) -> bool {
        !s.is_empty()
            && s.as_bytes()[0].is_ascii_lowercase()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }

    fn scan_code_fields(text: &str, out: &mut std::collections::BTreeSet<String>) {
        for key in ["\"code\"", "code"] {
            let needle = format!("{key}:");
            let bytes = text.as_bytes();
            let mut from = 0;
            while let Some(rel) = text[from..].find(&needle) {
                let start = from + rel;
                let idx = start + needle.len();
                if start == 0
                    || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                {
                    let mut k = idx;
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'"' {
                        if let Some(end) = text[k + 1..].find('"') {
                            let value = &text[k + 1..k + 1 + end];
                            if is_code_shaped(value) {
                                out.insert(value.to_string());
                            }
                        }
                    }
                }
                from = idx;
            }
        }
    }

    fn scan_code_consts(text: &str, out: &mut std::collections::BTreeSet<String>) {
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("pub const ") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once(':') else {
                continue;
            };
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                continue;
            }
            let Some(value) = tail.split('=').nth(1) else {
                continue;
            };
            if let Some((code, _)) = value
                .trim()
                .strip_prefix('"')
                .and_then(|v| v.split_once('"'))
            {
                if is_code_shaped(code) {
                    out.insert(code.to_string());
                }
            }
        }
    }

    fn fn_body<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
        let start = text.find(marker)?;
        let open = text[start..].find('{')? + start;
        let mut depth = 0usize;
        for (i, ch) in text[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&text[open..open + i + 1]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn scan_quoted(slice: &str, out: &mut std::collections::BTreeSet<String>) {
        let bytes = slice.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'"' {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    if bytes[j] == b'\\' {
                        j += 1;
                    }
                    j += 1;
                }
                if j < bytes.len() {
                    let value = &slice[i + 1..j];
                    if is_code_shaped(value) {
                        out.insert(value.to_string());
                    }
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
}
