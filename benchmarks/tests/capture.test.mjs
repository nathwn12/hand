// Capture-evidence validation tests. These are pure checks of the rule that a
// 1280x720 image is not enough: the response must prove which window it is,
// that the frame came from PrintWindow, that it was not blank, moved or stale,
// and that neither the capture's process nor its target touched the pointer or
// the foreground. A raw pointer delta is accepted only when external input
// explains it and the binary's import table has no writer.

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
    noSteal: {
      audited: true,
      bannedImports: [],
      pointer: {
        readable: true,
        before: [640, 360],
        after: [640, 360],
        inputTickBefore: 1000,
        inputTickAfter: 1000,
        inputDuringCall: false,
        moved: false,
        externalInput: false,
      },
      foreground: {
        before: 111,
        after: 111,
        afterPid: 999,
        afterExe: "owner.exe",
        afterOwnedByThisProcess: false,
        target: 4242,
        targetWasForeground: false,
        targetIsForeground: false,
        targetActivated: false,
        moved: false,
        externalChange: false,
        stolen: false,
      },
      stolen: false,
    },
  };
}

function movePointer(payload, { input }) {
  payload.cursorUnchanged = false;
  payload.noSteal.pointer.after = [700, 400];
  payload.noSteal.pointer.moved = true;
  payload.noSteal.pointer.inputDuringCall = input;
  payload.noSteal.pointer.externalInput = input;
}

function moveForeground(payload, { external }) {
  payload.foregroundUnchanged = false;
  payload.noSteal.foreground.after = 222;
  payload.noSteal.foreground.afterPid = 1000;
  payload.noSteal.foreground.afterExe = "third-party.exe";
  payload.noSteal.foreground.moved = true;
  payload.noSteal.foreground.externalChange = external;
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

test("a pointer delta explained by external input is attributed, and the raw sample stays visible", () => {
  const payload = validPayload();
  movePointer(payload, { input: true });
  payload.noSteal.pointer.inputTickAfter = 1007;
  const result = validateCapture(payload, context());
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.detail.cursorUnchanged, false);
  assert.equal(result.detail.cursorExternal, true);
});

test("a foreground change to a third-party window is attributed as external", () => {
  const payload = validPayload();
  moveForeground(payload, { external: true });
  const result = validateCapture(payload, context());
  assert.equal(result.ok, true, result.reason);
  assert.equal(result.detail.foregroundUnchanged, false);
  assert.equal(result.detail.foregroundExternal, true);
});

const unsafeCases = [
  ["blank frame", (payload) => (payload.capture.blank_check = { verdict: "full_blank", max_channel: 0, threshold: 8 })],
  ["low max channel", (payload) => (payload.capture.blank_check.max_channel = 3)],
  ["moved during capture", (payload) => (payload.capture.moved_during_capture = true)],
  ["crop fallback", (payload) => (payload.capture.backend = "desktop-crop:gdi")],
  ["capture note", (payload) => (payload.capture.note = "PrintWindow refused; crop used")],
  ["missing clean note", (payload) => delete payload.capture.note],
  [
    "a foreground window owned by the capture's process",
    (payload) => {
      payload.foregroundUnchanged = false;
      payload.noSteal.foreground.after = 333;
      payload.noSteal.foreground.afterPid = 4242;
      payload.noSteal.foreground.afterExe = "hand.exe";
      payload.noSteal.foreground.afterOwnedByThisProcess = true;
      payload.noSteal.foreground.moved = true;
      payload.noSteal.foreground.stolen = true;
      payload.noSteal.stolen = true;
    },
  ],
  ["cursor moved with no input", (payload) => movePointer(payload, { input: false })],
  [
    "the target took the foreground",
    (payload) => {
      payload.foregroundUnchanged = false;
      payload.noSteal.foreground.after = 4242;
      payload.noSteal.foreground.afterPid = 4242;
      payload.noSteal.foreground.afterExe = "fake-fixture.exe";
      payload.noSteal.foreground.targetIsForeground = true;
      payload.noSteal.foreground.targetActivated = true;
      payload.noSteal.foreground.moved = true;
      payload.noSteal.foreground.stolen = true;
      payload.noSteal.stolen = true;
    },
  ],
  [
    "a pointer writer in the import table",
    (payload) => (payload.noSteal.bannedImports = ["SetCursorPos"]),
  ],
  [
    "an unaudited import table",
    (payload) => {
      payload.noSteal.audited = false;
      payload.noSteal.auditError = "cannot read the executable";
    },
  ],
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
  ["no attribution at all", (payload) => delete payload.noSteal],
  ["unreadable pointer samples", (payload) => (payload.noSteal.pointer.readable = false)],
  [
    "a raw flag that contradicts the samples",
    (payload) => {
      payload.cursorUnchanged = false;
    },
  ],
  [
    "a foreground change that claims neither a steal nor an external window",
    (payload) => moveForeground(payload, { external: false }),
  ],
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
