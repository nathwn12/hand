
/// Geometry and journal vocabulary shared by the anchor fixture app and the
/// 14-case capture matrix. One owner for every number both sides assert.
///
/// The fixture is a WS_POPUP window (no system non-client border), so its
/// CLIENT origin equals its WINDOW origin. A 1-px magenta rectangle is PAINTED
/// on the outermost client pixels, and a green cross is painted at the exact
/// client coordinate `CROSS`. The capture maps an image pixel to a screen pixel
/// with the product's `source` rect; the matrix finds the green cross centroid
/// in the decoded PNG and asserts it lands within 1 px of the client origin
/// (journaled by the fixture) plus `CROSS`.

pub const WINDOW_CLASS: &str = "HandAnchorFixture";
pub const WINDOW_TITLE_PREFIX: &str = "hand-anchor-fixture";

/// Requested outer window size (and, for WS_POPUP, client size), in physical
/// pixels. Per-monitor v2 awareness is set in both processes, so these are
/// physical on every monitor.
pub const WIDTH: i32 = 1280;
pub const HEIGHT: i32 = 720;
pub const ORIGIN: (i32, i32) = (160, 120);

/// Cross centre in CLIENT pixels, and half-arm length of the plus sign.
pub const CROSS: (i32, i32) = (WIDTH / 2, HEIGHT / 2);
pub const CROSS_ARM: i32 = 14;

/// Native RGB colours. Magenta is the 1-px border; green is the anchor cross;
/// the near-white background keeps both far from the blank-frame threshold.
pub const BORDER: (u8, u8, u8) = (255, 0, 255);
pub const CROSS_COLOR: (u8, u8, u8) = (0, 255, 0);
pub const BACKGROUND: (u8, u8, u8) = (250, 250, 250);

/// Child controls as (x, y, w, h) in CLIENT pixels, well away from the cross.
pub const EDIT: (i32, i32, i32, i32) = (24, 24, 340, 26);
pub const CHECK: (i32, i32, i32, i32) = (24, 68, 240, 24);
pub const COMBO: (i32, i32, i32, i32) = (24, 104, 240, 28);

/// The fixture's own control ids (used only to tell its notifications apart).
pub const EDIT_ID: i32 = 100;
pub const CHECK_ID: i32 = 101;
pub const COMBO_ID: i32 = 102;

/// App-defined message: snapshot the OS globals independently of the engine.
/// Posting it moves nothing (no foreground, pointer or clipboard write).
pub const WM_SNAP: u32 = 0x8000 + 1;

/// Journal line prefixes. One line per event, appended in observation order.
pub const EV_READY: &str = "ready";
pub const EV_SNAP: &str = "snap";
pub const EV_EDIT: &str = "edit";
pub const EV_TOGGLE: &str = "toggle";
pub const EV_SELECT: &str = "select";
pub const EV_CLOSE: &str = "close";

/// The text the matrix types into the EDIT to prove a live typed result.
pub const TYPED_TEXT: &str = "anchor-ok";

/// Centre of a control rect, in CLIENT pixels.
pub const fn centre(r: (i32, i32, i32, i32)) -> (i32, i32) {
    (r.0 + r.2 / 2, r.1 + r.3 / 2)
}
