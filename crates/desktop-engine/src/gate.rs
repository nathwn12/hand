//! The input gate: background-only by default, explicit opt-in for real input.
//!
//! Default contract: the server never moves the owner's pointer, injects keys
//! into their session, steals focus or overwrites their clipboard. The owner
//! may be playing a fullscreen game; puppet works purely in the background or
//! says it cannot. That path is the old one: posted messages or UIA on a
//! locked target, typed refusals otherwise.
//!
//! Three top-level flags buy real effects, and only on a LOCKED target:
//!
//! | flag | what it buys | what it cannot do |
//! |------|--------------|-------------------|
//! | `allow_cursor:true` | real pointer move (`SetCursorPos`/`SendInput`) for `move` and coordinate pointer verbs | never calls `SetForegroundWindow`; a point not on the target or its descendants is refused `point_not_target`; without a locked target it refuses `target_required` |
//! | `allow_foreground:true` | `focus_window` calls `SetForegroundWindow` on the locked target | no other verb takes focus; `focus_guard:true` restores the entry foreground when it can |
//! | `allow_clipboard:true` | one real CF_UNICODETEXT write | never changes focus or pointer; `restore_clipboard:true` restores the prior text |
//!
//! `cursor_restore:true` (the default) restores the entry pointer after the
//! last action of an `allow_cursor` burst, including a partial or failed one.
//! `delivery:"sendinput"` stays refused: opt in at the top level, never
//! per action. Every refusal is typed and happens BEFORE any input is sent.

use serde_json::{Map, Value};

/// `delivery:"sendinput"` or `allow_foreground:true` was asked for: the
/// background-only policy forbids every foreground act (SendInput,
/// SetForegroundWindow, focus, maximize) ÔÇö the owner may be playing a
/// fullscreen game, so puppet works purely in the background or says it
/// cannot. Nothing was sent.
pub const NO_FOREGROUND_POLICY: &str = "no_foreground_policy";
/// A pointer act was asked for (`move`, or a click that needs a real
/// cursor): the background-only policy never moves the owner's pointer.
/// Posted mouse messages to a locked target stay available. Nothing was sent.
pub const NO_CURSOR_POLICY: &str = "no_cursor_policy";
/// `clipboard_set` was asked for without `allow_clipboard:true`: writing
/// would overwrite the owner's shared global clipboard (a side effect
/// outside the target window), so the background-only policy refuses it
/// until the caller opts in. Reads (`clipboard_get`) never need the flag.
/// Nothing was written.
pub const NO_CLIPBOARD_POLICY: &str = "no_clipboard_policy";
/// A real pointer action was aimed at a screen point whose root owner is not
/// the locked target (or one of its descendants). Names the window actually
/// found; nothing was sent.
pub const POINT_NOT_TARGET: &str = "point_not_target";
pub const BACKGROUND_UNAVAILABLE: &str = "background_unavailable";
/// An element matched, but it has no on-screen rectangle, so there is no
/// pixel to aim at. Typed so a caller can tell it apart from "no such
/// control" and fix it by scrolling the control into view or using invoke.
pub const OFFSCREEN_ELEMENT: &str = "offscreen_element";
pub const TARGET_REQUIRED: &str = "target_required";

/// Which delivery path a verb takes. There is no foreground/input
/// variant: every acting verb is background-only or refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// UIA pattern on a locked target (Invoke/Toggle/Expand/SelectionItem/
    /// Scroll) ÔÇö no pointer, no foreground, no messages.
    Uia,
    /// `PostMessage` to the locked hwnd (exact client coords for pointer
    /// verbs, key messages for posted keyboard).
    Posted,
    /// Real pointer delivery at a guarded screen point (SetCursorPos /
    /// SendInput). Requires explicit `allow_cursor:true` AND a locked target.
    Cursor,
    /// Explicit focus delivery (SetForegroundWindow on the locked target).
    /// Requires `allow_foreground:true` AND a locked target.
    Foreground,
    /// Nothing to deliver (wait, clipboard_get, wait_for).
    None,
}

/// Typed gate refusal, rendered as `"<code>: <message>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateError {
    pub code: &'static str,
    pub message: String,
}

impl GateError {
    pub fn text(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }

    pub fn to_map(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("ok".to_string(), Value::from(false));
        m.insert("code".to_string(), Value::from(self.code));
        m.insert("error".to_string(), Value::from(self.text()));
        m
    }
}

/// Foreground escalation was asked for (`delivery:"sendinput"` or
/// `allow_foreground:true`): refused under the background-only policy ÔÇö
/// the owner may be playing a fullscreen game, so puppet works purely in
/// the background or says it cannot.
pub(crate) fn no_foreground_policy(verb: &str) -> GateError {
    GateError {
        code: NO_FOREGROUND_POLICY,
        message: format!(
            "{verb} would take the foreground (SendInput/SetForegroundWindow/focus): refused \
             under the background-only policy ÔÇö the owner may be playing a fullscreen game, \
             so puppet works purely in the background or says it cannot. Pass a locked target \
             for posted delivery where the verb supports it. Nothing was sent."
        ),
    }
}

pub(crate) fn no_cursor_policy(verb: &str) -> GateError {
    GateError {
        code: NO_CURSOR_POLICY,
        message: format!(
            "{verb} would move the system pointer: refused under the background-only policy \
             ÔÇö the owner may be playing a fullscreen game, so puppet works purely in the \
             background or says it cannot. Pass a locked target for an exact posted click \
             with no pointer. Nothing was sent."
        ),
    }
}

/// Overwriting the owner's global clipboard is a side effect OUTSIDE the
/// target window (every application shares that clipboard): an unflagged
/// write is refused under the background-only policy. The top-level
/// `allow_clipboard:true` flag opts in to a real CF_UNICODETEXT write;
/// reads stay free either way.
/// The point guard's typed refusal: real pointer input would land on a
/// different window than the one the caller locked. The actual window is
/// named so the caller can aim again instead of guessing why nothing happened.
pub(crate) fn point_not_target(verb: &str, x: i64, y: i64, actual: &str) -> GateError {
    GateError {
        code: POINT_NOT_TARGET,
        message: format!(
            "{verb} at screen point ({x},{y}) would input into \"{actual}\", not the locked \
             target or one of its descendants: refused by the point guard. Nothing was sent."
        ),
    }
}

pub(crate) fn no_clipboard_policy(verb: &str) -> GateError {
    GateError {
        code: NO_CLIPBOARD_POLICY,
        message: format!(
            "{verb} would overwrite the owner's shared global clipboard: refused under the \
             background-only policy ÔÇö the owner may be playing a fullscreen game, so puppet \
             works purely in the background or says it cannot. Nothing was written."
        ),
    }
}

/// UIA text entry (`ValuePattern.SetValue`) can make the provider focus the
/// control: refused under the background-only policy ÔÇö no flag can buy a
/// focus change. The control selector is named so the caller knows what was
/// left untouched.
pub(crate) fn uia_text_unavailable(element: &str) -> GateError {
    GateError {
        code: BACKGROUND_UNAVAILABLE,
        message: format!(
            "set_value/type on element \"{element}\" may take the \
             foreground (the UIA provider can focus its control): refused under the \
             background-only policy ÔÇö the owner may be playing a fullscreen game, so puppet \
             works purely in the background or says it cannot. Nothing was written."
        ),
    }
}

/// One policy, two scopes: ValuePattern.SetValue may focus its control, so UIA
/// text entry is refused on the owner's desktop and no flag buys it out. A
/// private hidden desktop has no owner to steal focus from, so the same policy
/// permits it there and only there. Because that desktop is created inside the
/// call, no `elements` snapshot can be taken in advance, so `expect_fp` is not
/// required there; the owner's desktop keeps its fingerprint discipline.
pub(crate) fn uia_text_permitted(private_desktop: bool) -> bool {
    private_desktop
}

fn target_refusal(verb: &str) -> GateError {
    GateError {
        code: TARGET_REQUIRED,
        message: format!(
            "{verb} needs an explicit target window ({{\"hwnd\":..}} or another target field); \
             this server never falls back to the foreground window. Nothing was sent."
        ),
    }
}

fn background_refusal(verb: &str, why: &str) -> GateError {
    GateError {
        code: BACKGROUND_UNAVAILABLE,
        message: format!(
            "{verb} has no background delivery to a locked target: {why}. Nothing was sent."
        ),
    }
}

/// The flags a call carries, and whether it named a target. The flags
/// survive in the schema so old callers get a typed refusal instead of an
/// "unexpected keyword" error. Each flag buys exactly one real effect, and
/// only with a locked window: `allow_cursor` unlocks pointer delivery,
/// `allow_foreground` unlocks focus, `allow_clipboard` unlocks a write.
/// No flag changes any other verb.
#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub has_target: bool,
    pub allow_foreground: bool,
    pub allow_cursor: bool,
    /// True only when the call holds a concrete window handle. Real pointer
    /// and focus delivery need one for the point/root-owner guard; a bare
    /// background/hidden mode without an hwnd is not enough.
    pub has_locked_hwnd: bool,
    /// Top-level opt-in for a real clipboard write: `clipboard_set` is
    /// refused unless this is true. Never set by default.
    pub allow_clipboard: bool,
    /// True only for `desktop:"hidden"`: a private desktop the server created
    /// for one launched program. Posted keyboard to the hidden desktop stays
    /// available (no owner session to desync there); the owner's desktop is
    /// posted-to only with a locked target.
    pub private_desktop: bool,
}

impl Gate {
    pub fn new(has_target: bool, allow_foreground: bool, allow_cursor: bool) -> Self {
        Gate {
            has_target,
            allow_foreground,
            allow_cursor,
            has_locked_hwnd: has_target,
            allow_clipboard: false,
            private_desktop: false,
        }
    }

    /// Narrow the target test for real input to a concrete locked HWND.
    /// Defaults to `has_target` so ordinary posted/UIA calls are unchanged.
    pub fn locked_hwnd(mut self, has: bool) -> Self {
        self.has_locked_hwnd = has;
        self
    }

    /// Opt in to a real clipboard write for this call. No other verb reads
    /// this flag.
    pub fn allow_clipboard(mut self, allow: bool) -> Self {
        self.allow_clipboard = allow;
        self
    }

    /// Mark this call as driving a PRIVATE desktop (the server's own hidden
    /// desktop), where posted keyboard to the launched program stays
    /// available.
    pub fn private_desktop(mut self, private: bool) -> Self {
        self.private_desktop = private;
        self
    }

    /// Decide the delivery path for one action, or refuse before acting.
    /// Default: posted/UIA to a locked target, or a typed refusal. Real
    /// pointer/focus delivery is a separate path, unlocked only by the
    /// matching flag together with a concrete locked HWND.
    ///
    /// `has_coords`: x/y supplied. `has_element`: an element selector
    /// supplied.
    pub fn decide(
        &self,
        verb: &str,
        has_coords: bool,
        has_element: bool,
    ) -> Result<Delivery, GateError> {
        match verb {
            "move" => {
                if !self.allow_cursor {
                    Err(no_cursor_policy("move"))
                } else if !self.has_locked_hwnd {
                    Err(target_refusal("move (real pointer)"))
                } else {
                    Ok(Delivery::Cursor)
                }
            }
            "click" => {
                if has_element {
                    if self.has_target {
                        Ok(Delivery::Uia)
                    } else {
                        Err(target_refusal("click on an element"))
                    }
                } else if has_coords {
                    if self.allow_cursor && self.has_locked_hwnd {
                        Ok(Delivery::Cursor)
                    } else if self.has_target {
                        Ok(Delivery::Posted)
                    } else if self.allow_cursor {
                        Err(target_refusal("click x/y (real pointer)"))
                    } else {
                        Err(no_cursor_policy("click x/y"))
                    }
                } else if self.has_target {
                    Err(background_refusal(
                        "click",
                        "a posted click needs exact client x/y (or an element)",
                    ))
                } else {
                    Err(no_cursor_policy("click"))
                }
            }
            "down" | "up" => {
                if self.has_target {
                    if has_coords {
                        if self.allow_cursor && self.has_locked_hwnd {
                            Ok(Delivery::Cursor)
                        } else {
                            Ok(Delivery::Posted)
                        }
                    } else {
                        Err(background_refusal(
                            verb,
                            "a posted button message needs exact client x/y",
                        ))
                    }
                } else {
                    Err(no_cursor_policy(verb))
                }
            }
            "drag" => {
                if self.has_target {
                    if self.allow_cursor && self.has_locked_hwnd {
                        Ok(Delivery::Cursor)
                    } else {
                        Ok(Delivery::Posted)
                    }
                } else if self.allow_cursor {
                    Err(target_refusal("drag (real pointer)"))
                } else {
                    Err(no_cursor_policy("drag"))
                }
            }
            "scroll" => {
                if has_element {
                    if self.has_target {
                        Ok(Delivery::Uia)
                    } else {
                        Err(target_refusal("scroll on an element"))
                    }
                } else if has_coords {
                    if self.allow_cursor && self.has_locked_hwnd {
                        Ok(Delivery::Cursor)
                    } else if self.has_target {
                        Ok(Delivery::Posted)
                    } else if self.allow_cursor {
                        Err(target_refusal("scroll x/y (real pointer)"))
                    } else {
                        Err(no_cursor_policy("scroll x/y"))
                    }
                } else if self.has_target {
                    Err(background_refusal(
                        "scroll",
                        "a posted wheel message needs the exact point (x/y)",
                    ))
                } else {
                    Err(no_cursor_policy("scroll"))
                }
            }
            "type" | "key" | "hotkey" => {
                if verb == "type" && has_element {
                    if uia_text_permitted(self.private_desktop) {
                        return Ok(Delivery::Uia);
                    }
                    return Err(background_refusal(
                        "type on an element",
                        "UIA ValuePattern.SetValue may focus the control; use the per-action \
                         refusal naming the element",
                    ));
                }
                if self.has_target {
                    return Ok(Delivery::Posted);
                }
                Err(target_refusal(verb))
            }
            "invoke" => {
                if !self.has_target {
                    return Err(target_refusal(verb));
                }
                Ok(Delivery::Uia)
            }
            "set_value" => {
                if !self.has_target {
                    return Err(target_refusal(verb));
                }
                if uia_text_permitted(self.private_desktop) {
                    return Ok(Delivery::Uia);
                }
                Err(background_refusal(
                    "set_value",
                    "UIA ValuePattern.SetValue may focus the control",
                ))
            }
            "focus_window" => {
                if !self.allow_foreground {
                    Err(no_foreground_policy("focus_window"))
                } else if !self.has_locked_hwnd {
                    Err(target_refusal("focus_window"))
                } else {
                    Ok(Delivery::Foreground)
                }
            }
            "clipboard_set" if self.allow_clipboard => Ok(Delivery::None),
            "clipboard_set" => Err(no_clipboard_policy("clipboard_set")),
            _ => Ok(Delivery::None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(has_target: bool, fg: bool, cur: bool) -> Gate {
        Gate::new(has_target, fg, cur)
    }

    fn code(r: Result<Delivery, GateError>) -> Option<&'static str> {
        r.err().map(|e| e.code)
    }

    #[test]
    fn default_gate_refuses_every_injecting_verb() {
        let gate = g(false, false, false);
        assert_eq!(
            code(gate.decide("move", true, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("move", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("click", true, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("click", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("down", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("up", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("drag", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("scroll", true, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("scroll", false, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(gate.decide("type", false, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("key", false, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("hotkey", false, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("focus_window", false, false)),
            Some(NO_FOREGROUND_POLICY)
        );
        assert_eq!(
            code(gate.decide("invoke", false, true)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("set_value", false, true)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("click", false, true)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(gate.decide("scroll", false, true)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(gate.decide("wait", false, false), Ok(Delivery::None));
        assert_eq!(gate.decide("wait_for", false, false), Ok(Delivery::None));
        assert_eq!(
            gate.decide("clipboard_get", false, false),
            Ok(Delivery::None)
        );
        assert_eq!(
            code(gate.decide("clipboard_set", false, false)),
            Some(NO_CLIPBOARD_POLICY)
        );
    }

    #[test]
    fn clipboard_write_is_gated_by_allow_clipboard_only() {
        let e = g(false, false, false)
            .decide("clipboard_set", false, false)
            .unwrap_err();
        assert_eq!(e.code, NO_CLIPBOARD_POLICY);
        assert!(
            e.text().contains("background-only policy"),
            "refusal must name the rule: {}",
            e.text()
        );
        assert!(
            e.text().starts_with(NO_CLIPBOARD_POLICY) && e.text().contains("Nothing was written."),
            "refusal must be typed and state nothing changed: {}",
            e.text()
        );
        // Foreground and cursor flags never buy a write; only the clipboard
        // flag does, and it changes no other verb.
        for gate in [g(false, true, true), g(true, true, true)] {
            assert_eq!(
                code(gate.decide("clipboard_set", false, false)),
                Some(NO_CLIPBOARD_POLICY),
                "no flag authorises a clipboard write"
            );
        }
        let allow = g(false, false, false).allow_clipboard(true);
        assert_eq!(
            allow.decide("clipboard_set", false, false),
            Ok(Delivery::None),
            "the flag unlocks the write and nothing else"
        );
        assert_eq!(
            allow.decide("clipboard_get", false, false),
            Ok(Delivery::None)
        );
        assert_eq!(
            code(allow.decide("type", false, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(allow.decide("move", true, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(
            code(allow.decide("focus_window", false, false)),
            Some(NO_FOREGROUND_POLICY)
        );
        let gated_target = g(true, false, false).allow_clipboard(true);
        assert_eq!(
            code(gated_target.decide("set_value", false, true)),
            Some(BACKGROUND_UNAVAILABLE),
            "the flag must not unlock UIA text entry"
        );
        let both = g(true, true, true).allow_clipboard(true);
        for (verb, coords, element, want) in [
            ("type", false, false, Delivery::Posted),
            ("key", false, false, Delivery::Posted),
            ("hotkey", false, false, Delivery::Posted),
            ("click", true, false, Delivery::Cursor),
            ("drag", false, false, Delivery::Cursor),
            ("clipboard_get", false, false, Delivery::None),
            ("wait", false, false, Delivery::None),
        ] {
            assert_eq!(both.decide(verb, coords, element), Ok(want), "{verb}");
        }
        assert_eq!(
            both.decide("clipboard_set", false, false),
            Ok(Delivery::None)
        );
        assert_eq!(
            both.decide("move", true, false),
            Ok(Delivery::Cursor),
            "allow_cursor buys a real pointer move, nothing else"
        );
        assert_eq!(
            both.decide("focus_window", false, false),
            Ok(Delivery::Foreground),
            "allow_foreground buys focus, nothing else"
        );
    }

    #[test]
    fn flags_buy_real_input_only_with_a_locked_target() {
        // A flag without a locked window buys nothing: the point guard needs
        // a concrete window to aim at, and there is no foreground fallback.
        for gate in [
            g(false, false, true),
            g(false, true, false),
            g(false, true, true),
        ] {
            assert_eq!(
                code(gate.decide("move", true, false)),
                if gate.allow_cursor {
                    Some(TARGET_REQUIRED)
                } else {
                    Some(NO_CURSOR_POLICY)
                },
                "cursor without a lock must name the missing target"
            );
            assert_eq!(
                code(gate.decide("click", true, false)),
                if gate.allow_cursor {
                    Some(TARGET_REQUIRED)
                } else {
                    Some(NO_CURSOR_POLICY)
                }
            );
            assert_eq!(
                code(gate.decide("focus_window", false, false)),
                if gate.allow_foreground {
                    Some(TARGET_REQUIRED)
                } else {
                    Some(NO_FOREGROUND_POLICY)
                }
            );
            assert_eq!(
                code(gate.decide("type", false, false)),
                Some(TARGET_REQUIRED)
            );
            assert_eq!(
                code(gate.decide("clipboard_set", false, false)),
                Some(NO_CLIPBOARD_POLICY),
                "cursor/focus flags never buy a clipboard write"
            );
        }
        let gate = g(true, true, true);
        assert_eq!(gate.decide("move", true, false), Ok(Delivery::Cursor));
        assert_eq!(gate.decide("click", true, false), Ok(Delivery::Cursor));
        assert_eq!(gate.decide("down", true, false), Ok(Delivery::Cursor));
        assert_eq!(gate.decide("up", true, false), Ok(Delivery::Cursor));
        assert_eq!(gate.decide("drag", false, false), Ok(Delivery::Cursor));
        assert_eq!(gate.decide("scroll", true, false), Ok(Delivery::Cursor));
        assert_eq!(
            gate.decide("focus_window", false, false),
            Ok(Delivery::Foreground)
        );
        // Everything else is exactly the old default path.
        assert_eq!(gate.decide("type", false, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("key", false, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("hotkey", false, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("click", false, true), Ok(Delivery::Uia));
        assert_eq!(
            code(gate.decide("set_value", false, true)),
            Some(BACKGROUND_UNAVAILABLE)
        );
        assert_eq!(
            code(gate.decide("clipboard_set", false, false)),
            Some(NO_CLIPBOARD_POLICY)
        );
        // A lock that is only a hidden/background mode, not an HWND, cannot
        // take real input: the guard has no window to resolve a point against.
        let no_hwnd = g(true, true, true).locked_hwnd(false);
        assert_eq!(
            code(no_hwnd.decide("move", true, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(
            code(no_hwnd.decide("focus_window", false, false)),
            Some(TARGET_REQUIRED)
        );
        assert_eq!(no_hwnd.decide("click", true, false), Ok(Delivery::Posted));
    }

    #[test]
    fn pointer_verbs_need_allow_cursor_and_a_locked_target() {
        // Without the flag every pointer verb keeps the old refusal/post path.
        let plain = g(true, false, false);
        assert_eq!(
            code(plain.decide("move", true, false)),
            Some(NO_CURSOR_POLICY)
        );
        assert_eq!(plain.decide("click", true, false), Ok(Delivery::Posted));
        assert_eq!(plain.decide("down", true, false), Ok(Delivery::Posted));
        assert_eq!(plain.decide("up", true, false), Ok(Delivery::Posted));
        assert_eq!(plain.decide("drag", false, false), Ok(Delivery::Posted));
        assert_eq!(plain.decide("scroll", true, false), Ok(Delivery::Posted));

        // With the flag and a lock, every coordinate pointer verb is real.
        let opted = g(true, false, true);
        for (verb, coords) in [
            ("move", true),
            ("click", true),
            ("down", true),
            ("up", true),
            ("drag", false),
            ("scroll", true),
        ] {
            assert_eq!(
                opted.decide(verb, coords, false),
                Ok(Delivery::Cursor),
                "{verb} with allow_cursor must be real pointer input"
            );
        }
        // With the flag but no lock, the guard cannot run: typed refusal.
        let no_target = g(false, false, true);
        for (verb, coords) in [
            ("move", true),
            ("click", true),
            ("drag", false),
            ("scroll", true),
        ] {
            assert_eq!(
                code(no_target.decide(verb, coords, false)),
                Some(TARGET_REQUIRED),
                "{verb} needs a locked target for the point guard"
            );
        }
        assert_eq!(
            code(no_target.decide("type", false, false)),
            Some(TARGET_REQUIRED)
        );
    }

    #[test]
    fn move_is_real_only_with_allow_cursor() {
        assert_eq!(
            code(g(true, false, false).decide("move", true, false)),
            Some(NO_CURSOR_POLICY),
            "the default never moves the pointer"
        );
        assert_eq!(
            g(true, false, true).decide("move", true, false),
            Ok(Delivery::Cursor)
        );
        assert_eq!(
            code(g(true, false, true).decide("focus_window", false, false)),
            Some(NO_FOREGROUND_POLICY),
            "the cursor flag does not buy focus"
        );
    }

    #[test]
    fn focus_is_real_only_with_allow_foreground_and_a_lock() {
        assert_eq!(
            code(g(true, false, true).decide("focus_window", false, false)),
            Some(NO_FOREGROUND_POLICY),
            "allow_cursor alone does not buy focus"
        );
        assert_eq!(
            g(true, true, false).decide("focus_window", false, false),
            Ok(Delivery::Foreground)
        );
        assert_eq!(
            code(g(false, true, false).decide("focus_window", false, false)),
            Some(TARGET_REQUIRED),
            "focus still needs a locked target"
        );
    }

    #[test]
    fn point_guard_refusal_is_typed_and_names_the_window() {
        let e = point_not_target("click", 17, 42, "hwnd=9 class=Notepad title=Untitled");
        assert_eq!(e.code, POINT_NOT_TARGET);
        let text = e.text();
        assert!(text.starts_with(POINT_NOT_TARGET), "{text}");
        assert!(
            text.contains("hwnd=9 class=Notepad title=Untitled"),
            "{text}"
        );
        assert!(text.contains("(17,42)"), "{text}");
        assert!(text.contains("Nothing was sent."), "{text}");
    }

    #[test]
    fn a_target_switches_to_uia_and_posted_delivery_without_flags() {
        let gate = g(true, false, false);
        assert_eq!(gate.decide("click", true, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("click", false, true), Ok(Delivery::Uia));
        assert_eq!(gate.decide("down", true, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("up", true, false), Ok(Delivery::Posted));
        let e = gate.decide("down", false, false).unwrap_err();
        assert_eq!(e.code, BACKGROUND_UNAVAILABLE);
        assert_eq!(gate.decide("drag", false, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("scroll", true, false), Ok(Delivery::Posted));
        assert_eq!(gate.decide("scroll", false, true), Ok(Delivery::Uia));
        assert_eq!(
            code(gate.decide("type", false, true)),
            Some(BACKGROUND_UNAVAILABLE)
        );
        for verb in ["type", "key", "hotkey"] {
            assert_eq!(
                gate.decide(verb, false, false),
                Ok(Delivery::Posted),
                "{verb} posts to a locked target"
            );
            let e = g(false, false, false)
                .decide(verb, false, false)
                .unwrap_err();
            assert_eq!(e.code, TARGET_REQUIRED, "{verb}");
        }
        let private = g(true, false, false).private_desktop(true);
        assert_eq!(private.decide("type", false, false), Ok(Delivery::Posted));
        assert_eq!(private.decide("key", false, false), Ok(Delivery::Posted));
        // One policy, two scopes: UIA text entry is UIA on a private desktop,
        // refused on the owner's desktop whatever the flags say.
        assert!(!uia_text_permitted(false));
        assert!(uia_text_permitted(true));
        assert_eq!(private.decide("type", false, true), Ok(Delivery::Uia));
        assert_eq!(private.decide("set_value", false, true), Ok(Delivery::Uia));
        assert_eq!(gate.decide("invoke", false, true), Ok(Delivery::Uia));
        assert_eq!(
            code(gate.decide("set_value", false, true)),
            Some(BACKGROUND_UNAVAILABLE)
        );
        assert_eq!(
            code(
                g(true, true, true)
                    .allow_clipboard(true)
                    .decide("set_value", false, true)
            ),
            Some(BACKGROUND_UNAVAILABLE),
            "no flag buys UIA text entry on the owner's desktop"
        );
        let e = gate.decide("move", true, false).unwrap_err();
        assert_eq!(e.code, NO_CURSOR_POLICY);
        assert!(e.text().contains("background-only policy"), "{}", e.text());
        let e = gate.decide("click", false, false).unwrap_err();
        assert_eq!(e.code, BACKGROUND_UNAVAILABLE);
        assert!(e.text().contains("needs exact client x/y"), "{}", e.text());
        let e = gate.decide("scroll", false, false).unwrap_err();
        assert_eq!(e.code, BACKGROUND_UNAVAILABLE);
    }

    #[test]
    fn no_permits_no_injection_apis() {
        let gate = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/gate.rs"))
            .expect("gate.rs readable");
        let prod = gate.split("#[cfg(test)]").next().unwrap_or("");
        for sym in [
            concat!("Send", "Input("),
            concat!("Set", "CursorPos("),
            concat!("Set", "ForegroundWindow("),
            concat!("Bring", "WindowToTop("),
            concat!("mouse", "_event("),
            "ForegroundPermit",
            "CursorPermit",
            "ClipboardPermit",
        ] {
            assert!(
                !prod.contains(sym),
                "gate.rs must not mention {sym}: the escalation path is deleted"
            );
        }
    }

    #[test]
    fn refusals_name_the_recovery_in_every_code() {
        for (gate, verb) in [
            (g(false, false, false), "type"),
            (g(false, false, false), "move"),
            (g(true, false, false), "move"),
            (g(false, false, false), "invoke"),
        ] {
            let e = gate.decide(verb, true, false).unwrap_err();
            let t = e.text();
            assert!(
                t.starts_with(e.code),
                "refusal text must start with its code: {t}"
            );
            assert!(
                t.contains("Nothing was sent.") || t.contains("Nothing was written."),
                "refusal must state no input: {t}"
            );
        }
        for verb in ["focus_window", "clipboard_set", "move"] {
            let e = g(false, false, false)
                .decide(verb, true, false)
                .unwrap_err();
            assert!(
                e.text().contains("background-only policy"),
                "{verb} must name the rule: {}",
                e.text()
            );
            assert!(
                !e.text().contains("allow_foreground:true")
                    && !e.text().contains("allow_cursor:true")
                    && !e.text().contains("allow_clipboard:true"),
                "{verb} must not beg for a flag: {}",
                e.text()
            );
        }
    }
}
