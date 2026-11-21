//! D11 red-first proof: `privacyMasks` is an accepted screenshot/diff input,
//! drawn before encode, named in the result, and a mask that matches nothing is
//! a typed error rather than a silent skip.
//!
//! These tests drive the public `tools/call` route, so they fail at runtime on
//! the pre-fix binary (the argument is rejected as an unknown extra field)
//! instead of failing to compile.

use serde_json::{json, Value};

fn call(name: &str, args: Value) -> Value {
    desktop_engine::call_tool(json!(1), "1", json!({"name": name, "arguments": args}))
        .expect("desktop tool answered")
}

fn frame_text(frame: &Value) -> String {
    frame.to_string()
}

#[test]
fn privacy_masks_argument_is_accepted() {
    let out = call(
        "screenshot",
        json!({
            "display": -1,
            "region": [0, 0, 64, 64],
            "max_px": 64,
            "privacyMasks": [{"rect": [0, 0, 8, 8]}]
        }),
    );
    let text = frame_text(&out);
    assert!(
        !text.contains("unexpected_keyword_argument"),
        "privacyMasks must be an accepted input, got: {text}"
    );
}

#[test]
fn privacy_mask_rect_matching_nothing_is_typed() {
    let out = call(
        "screenshot",
        json!({
            "display": -1,
            "region": [0, 0, 64, 64],
            "max_px": 64,
            "privacyMasks": [{"rect": [50000, 50000, 50010, 50010]}]
        }),
    );
    let text = frame_text(&out);
    assert!(
        text.contains("mask_no_match"),
        "a mask that matched nothing must be a typed mask_no_match, got: {text}"
    );
}

#[test]
fn privacy_mask_bad_shape_is_typed() {
    let out = call(
        "screenshot",
        json!({
            "display": -1,
            "region": [0, 0, 64, 64],
            "max_px": 64,
            "privacyMasks": [{"rect": [0, 0, 8]}]
        }),
    );
    let text = frame_text(&out);
    assert!(
        text.contains("bad_mask"),
        "a malformed mask must be a typed bad_mask, got: {text}"
    );
}

#[test]
fn diff_privacy_mask_no_match_is_typed() {
    let operand = |masks: Value| {
        json!({
            "display": -1,
            "region": [0, 0, 64, 64],
            "privacyMasks": masks
        })
    };
    let out = call(
        "diff",
        json!({
            "base": operand(json!([{"rect": [50000, 50000, 50010, 50010]}])),
            "next": operand(json!([{"rect": [0, 0, 8, 8]}])),
            "max_px": 64
        }),
    );
    let text = frame_text(&out);
    assert!(
        text.contains("mask_no_match"),
        "a diff operand's mask that matched nothing must be typed, got: {text}"
    );
}
