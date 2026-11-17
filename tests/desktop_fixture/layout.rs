// The one contract shared by the private-desktop fixture app and the test
// that drives it: window geometry, control rectangles and the journal
// vocabulary. Plain data only, no FFI, no state.
//
// The app (`fixture_app.rs`) is compiled by the test at run time with this file
// `include!`d; the test pulls it in as a module. Both sides therefore read the
// SAME numbers and the SAME event names, so a geometry or keyword change
// cannot land on one side only.
//
// The header is `//` rather than `//!` on purpose: `include!` splices this
// file into a crate root, where an inner doc comment is not allowed.

/// Window class registered by the fixture app. The test asserts the engine
/// resolved THIS window (not another window on the private desktop, such as a
/// console) by class and by the app's own journal.
pub const WINDOW_CLASS: &str = "HandDesktopFixture";

/// Top-level window title prefix; the app appends its pid so two fixtures can
/// never be confused.
pub const WINDOW_TITLE_PREFIX: &str = "hand-desktop-fixture";

/// Top-left corner of the fixture window, in the private desktop's screen
/// pixels. The window is `WS_POPUP` with no border, so its CLIENT origin is
/// exactly this point: client = screen - ORIGIN. The test relies on that to
/// name a client point without ever having seen the window.
pub const ORIGIN: (i32, i32) = (300, 300);
pub const WIDTH: i32 = 420;
pub const HEIGHT: i32 = 320;

/// Child controls as `(x, y, w, h)` in the fixture window's CLIENT pixels.
pub const EDIT: (i32, i32, i32, i32) = (10, 10, 260, 24);
pub const STATUS: (i32, i32, i32, i32) = (10, 44, 260, 24);
pub const BUTTON: (i32, i32, i32, i32) = (10, 80, 96, 28);

/// Control ids (the fixture's own; `do` delivers by coordinates, the ids only
/// let the app tell its own notifications apart).
pub const EDIT_ID: i32 = 100;
pub const STATUS_ID: i32 = 102;
pub const BUTTON_ID: i32 = 101;

/// Initial status text, so the engine's pre-drive readback is non-empty and a
/// test can tell "read something" from "read nothing".
pub const STATUS_IDLE: &str = "status=idle";
/// The status text after the button's own activation.
pub const STATUS_ON: &str = "status=on";
/// The status text when the button is activated again.
pub const STATUS_OFF: &str = "status=off";

/// Journal line prefixes. One line per event, appended in the order the app
/// observed them; every value is `prefix=value`.
pub const EV_READY: &str = "ready";
pub const EV_EDIT: &str = "edit";
pub const EV_CLICK: &str = "click";
pub const EV_TOGGLE: &str = "toggle";
pub const EV_CLOSE: &str = "close";

/// Values on the toggle line.
pub const TOGGLE_ON: &str = "on";

/// The text the act/verify scenario types into the EDIT.
pub const TYPED_TEXT: &str = "hand-ok";

/// Centre of a control rect, in the fixture window's client pixels.
pub const fn centre(r: (i32, i32, i32, i32)) -> (i32, i32) {
    (r.0 + r.2 / 2, r.1 + r.3 / 2)
}

/// The BUTTON's centre in the private desktop's SCREEN pixels: what `do`
/// takes for an x/y click under `desktop:"hidden"` (no target => screen
/// space, converted by the engine with ScreenToClient).
pub const fn button_screen_centre() -> (i32, i32) {
    let (cx, cy) = centre(BUTTON);
    (ORIGIN.0 + cx, ORIGIN.1 + cy)
}

/// True when a client point falls inside a control rect.
pub const fn contains(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}
