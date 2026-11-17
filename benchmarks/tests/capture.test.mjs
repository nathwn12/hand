// Capture-evidence validation tests. These are pure checks of the rule that a
// 1280x720 image is not enough: the response must prove which window it is,
// that the frame came from PrintWindow, that it was not blank, moved or stale,
// and that foreground and cursor were unchanged.

import test from "node:test";
import assert from "node:assert/strict";

import { validateCapture, MODE_B_NOTE } from "../scripts/lib/scenarios.mjs";

function context() {
  return {
    target: { titleContains: "hand-bench-fixture" },
    hwnd: 4242,
    matchedWindow: {
      hwnd: 4242,
      pid: 4242,
      title: "hand-bench-fixture",
      exe: "fake-fixture.exe",
      class: "FakeFixtureWindow",
    },
  };
}

function validPayload() {
  return {
    image: { w: 1280, h: 720, format: "png" },
    source: { x: 100, y: 100, w: 1280, h: 720, kind: "window" },
    scale: 1,
    window: {
      hwnd: 4242,
      pid: 4242,
      exe: "fake-fixture.exe",
      title: "hand-bench-fixture",
      minimized: false,
      cloaked: 0,
      dwm_rect: [100, 100, 1380, 820],
    },
    capture: {
      backend: "printwindow",
      moved_during_capture: false,
      stale: false,
      blank_check: { verdict: "content", max_channel: 255, threshold: 8 },
      note: null,
    },
    foregroundUnchanged: true,
    cursorUnchanged: true,
  };
}

test("a complete, stationary, non-blank PrintWindow capture proves its target", () => {
  const result = validateCapture(validPayload(), context());
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.detail.hwnd, 4242);
  assert.equal(result.detail.backend, "printwindow");
  assert.equal(result.detail.blank.verdict, "content");
  assert.equal(result.detail.note, null);
});

test("the documented benign Mode B note is accepted on a PrintWindow capture", () => {
  const payload = validPayload();
  payload.capture.note = MODE_B_NOTE;
  const result = validateCapture(payload, context());
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.detail.note, MODE_B_NOTE);
});

test("the unchanged-frame stale flag is reported, not treated as an unsafe origin", () => {
  const payload = validPayload();
  payload.capture.stale = true;
  const result = validateCapture(payload, context());
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.detail.stale, true);
});

const unsafeCases = [
  ["blank frame", (payload) => (payload.capture.blank_check = { verdict: "full_blank", max_channel: 0, threshold: 8 })],
  ["low max channel", (payload) => (payload.capture.blank_check.max_channel = 3)],
  ["moved during capture", (payload) => (payload.capture.moved_during_capture = true)],
  ["crop fallback", (payload) => (payload.capture.backend = "desktop-crop:gdi")],
  ["capture note", (payload) => (payload.capture.note = "PrintWindow refused; crop used")],
  ["missing clean note", (payload) => delete payload.capture.note],
  ["foreground changed", (payload) => (payload.foregroundUnchanged = false)],
  ["cursor moved", (payload) => (payload.cursorUnchanged = false)],
  ["minimized target", (payload) => (payload.window.minimized = true)],
  ["warnings", (payload) => (payload.warnings = [{ code: "quality_ignored" }])],
  ["missing blank check", (payload) => delete payload.capture.blank_check],
];

for (const [name, mutate] of unsafeCases) {
  test(`rejects unsafe capture evidence: ${name}`, () => {
    const payload = validPayload();
    mutate(payload);
    const result = validateCapture(payload, context());
    assert.equal(result.ok, false);
    assert.equal(result.unsafe, true, result.reason);
  });
}

const incompleteCases = [
  ["no window identity", (payload) => delete payload.window],
  ["wrong hwnd", (payload) => (payload.window.hwnd = 9999)],
  ["wrong pid", (payload) => (payload.window.pid = 9999)],
  ["wrong title", (payload) => (payload.window.title = "some other window")],
  ["wrong image size", (payload) => (payload.image.w = 1024)],
  ["wrong source size", (payload) => (payload.source.h = 600)],
  ["scaled image", (payload) => (payload.scale = 1.5)],
];

for (const [name, mutate] of incompleteCases) {
  test(`rejects incomplete capture evidence: ${name}`, () => {
    const payload = validPayload();
    mutate(payload);
    const result = validateCapture(payload, context());
    assert.equal(result.ok, false);
    assert.notEqual(result.unsafe, true, result.reason);
  });
}
