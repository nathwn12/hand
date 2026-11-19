
import { basename } from "node:path";

import { canonicalHash, desktopTargetFingerprint } from "./fingerprint.mjs";

export const MODE_B_NOTE =
  "Mode B (Windows.Graphics.Capture) deferred: needs a WinRT + D3D11 frame pool in this Rust stack and is untestable on a single-monitor box here; Mode A covers the verifiable cases.";

export const SCENARIOS = {
  "warm-ping": {
    id: "warm-ping",
    title: "warm no-op MCP round trip",
    domain: "protocol",
    specTargetP95Ms: 50,
  },
  "browser-snapshot": {
    id: "browser-snapshot",
    title: "warm browser snapshot of the controlled 1280x720 fixture",
    domain: "browser",
    specTargetP95Ms: 250,
  },
  "desktop-enumerate": {
    id: "desktop-enumerate",
    title: "desktop window enumeration including the controlled fixture",
    domain: "desktop",
    specTargetP95Ms: 250,
  },
  "desktop-capture": {
    id: "desktop-capture",
    title: "controlled 1280x720 window capture at native resolution",
    domain: "desktop",
    specTargetP95Ms: 500,
  },
};

export const PRODUCT_DEFINITIONS = {
  hand: {
    id: "hand",
    label: "hand",
    domains: ["browser", "desktop"],
    tools: {
      browserNavigate: "browser_navigate",
      browserViewport: "browser_viewport",
      browserSnapshot: "browser_snapshot",
      desktopListWindows: "desktop_list_windows",
      desktopCapture: "desktop_screenshot",
    },
  },
  gladius: {
    id: "gladius",
    label: "Gladius baseline",
    domains: ["browser"],
    tools: {
      browserNavigate: "navigate",
      browserViewport: "viewport",
      browserSnapshot: "snapshot",
      desktopListWindows: null,
      desktopCapture: null,
    },
  },
  puppet: {
    id: "puppet",
    label: "Puppet baseline",
    domains: ["desktop"],
    tools: {
      browserNavigate: null,
      browserViewport: null,
      browserSnapshot: null,
      desktopListWindows: "list_windows",
      desktopCapture: "screenshot",
    },
  },
};

export function resolveScenarioPlan({ productId, scenarioId, origin, target }) {
  const product = PRODUCT_DEFINITIONS[productId];
  const scenario = SCENARIOS[scenarioId];
  if (!product) return { ok: false, reason: `unknown product: ${productId}` };
  if (!scenario) return { ok: false, reason: `unknown scenario: ${scenarioId}` };

  if (scenario.domain === "protocol") {
    return {
      ok: true,
      plan: {
        scenario,
        tool: null,
        method: "ping",
        requiredTools: [],
        prepare: [],
        request: () => ({ method: "ping", params: {} }),
        validate: validatePing,
        paramsHash: null,
      },
    };
  }

  if (!product.domains.includes(scenario.domain)) {
    return {
      ok: false,
      unsupported: true,
      reason: `${product.label} exposes no ${scenario.domain} capability in the pinned surface`,
    };
  }

  if (scenario.domain === "browser") {
    const snapshot = product.tools.browserSnapshot;
    const navigate = product.tools.browserNavigate;
    const viewport = product.tools.browserViewport;
    const missing = [
      ["snapshot", snapshot],
      ["navigate", navigate],
      ["viewport", viewport],
    ].filter(([, name]) => !name);
    if (missing.length) {
      return { ok: false, unsupported: true, reason: `${product.label} exposes no browser ${missing.map(([key]) => key).join("/")} tool` };
    }
    if (!origin) return { ok: false, reason: "browser scenario requires the loopback fixture origin" };
    const url = `${origin}/index.html`;
    const params = { url };
    return {
      ok: true,
      plan: {
        scenario,
        tool: snapshot,
        requiredTools: [navigate, viewport, snapshot],
        paramsHash: paramsHash({
          url: "http://127.0.0.1:<fixture-port>/index.html",
          viewport: { width: 1280, height: 720, deviceScaleFactor: 1 },
        }),
        prepare: [
          {
            name: "navigate to controlled fixture",
            tool: navigate,
            params: { url },
            validate: validateNavigate,
          },
          {
            name: "set 1280x720 viewport",
            tool: viewport,
            params: { width: 1280, height: 720, deviceScaleFactor: 1 },
            validate: validateViewport,
          },
          {
            name: "readiness snapshot",
            tool: snapshot,
            params: {},
            validate: validateSnapshot,
          },
        ],
        request: () => ({ tool: snapshot, params: {} }),
        validate: validateSnapshot,
      },
    };
  }

  if (!target) {
    return {
      ok: false,
      unsupported: true,
      reason:
        "no controlled desktop fixture target configured; pass --desktop-target or --desktop-title-contains. The driver never creates, resizes or launches a desktop window",
    };
  }

  if (scenario.domain === "desktop") {
    if (scenario.id === "desktop-enumerate") {
      const listWindows = product.tools.desktopListWindows;
      if (!listWindows) return { ok: false, unsupported: true, reason: `${product.label} exposes no desktop window enumeration tool` };
      return {
        ok: true,
        plan: {
          scenario,
          tool: listWindows,
          requiredTools: [listWindows],
          paramsHash: paramsHash({ target, params: {} }),
          prepare: [
            {
              name: "resolve controlled fixture window",
              tool: listWindows,
              params: {},
              validate: (payload, ctx) => validateListWindows(payload, ctx),
            },
          ],
          request: () => ({ tool: listWindows, params: {} }),
          validate: (payload, ctx) => validateListWindows(payload, ctx),
        },
      };
    }

    const capture = product.tools.desktopCapture;
    if (!capture) return { ok: false, unsupported: true, reason: `${product.label} exposes no desktop capture tool` };
    const listWindows = product.tools.desktopListWindows;
    if (!listWindows) {
      return {
        ok: false,
        unsupported: true,
        reason: `${product.label} exposes no desktop enumeration to resolve the controlled target before capture`,
      };
    }
    const params = { target, fmt: "png", max_px: 8192, allow_crop_fallback: false };
    return {
      ok: true,
      plan: {
        scenario,
        tool: capture,
        requiredTools: [listWindows, capture],
        paramsHash: paramsHash({ target, params }),
        prepare: [
          {
            name: "resolve controlled fixture window",
            tool: listWindows,
            params: {},
            validate: (payload, ctx) => validateListWindows(payload, ctx),
          },
          {
            name: "verify native 1280x720 capture geometry",
            tool: capture,
            params,
            validate: (payload, ctx) => validateCapture(payload, ctx),
          },
        ],
        request: () => ({ tool: capture, params }),
        validate: (payload, ctx) => validateCapture(payload, ctx),
      },
    };
  }

  return { ok: false, reason: `unhandled scenario domain: ${scenario.domain}` };
}

function paramsHash(value) {
  return canonicalHash(value);
}


export function extractPayload(result) {
  if (!result || typeof result !== "object") {
    return { ok: false, reason: "response has no result object" };
  }
  if (result.isError === true) {
    const text = Array.isArray(result.content)
      ? result.content
          .filter((item) => item?.type === "text")
          .map((item) => item.text)
          .join("\n")
      : "";
    return { ok: false, reason: `tool reported isError: ${text.slice(0, 400) || "no message"}` };
  }
  if (result.structuredContent && typeof result.structuredContent === "object") {
    return { ok: true, payload: result.structuredContent, source: "structuredContent" };
  }
  const text = Array.isArray(result.content)
    ? result.content.find((item) => item?.type === "text" && typeof item.text === "string")?.text
    : null;
  if (typeof text !== "string" || text.trim() === "") {
    return { ok: false, reason: "response has neither structuredContent nor a text content block" };
  }
  try {
    return { ok: true, payload: JSON.parse(text), source: "content.text JSON" };
  } catch {
    return { ok: false, reason: "content.text is not JSON" };
  }
}


function validatePing(result) {
  if (!result || typeof result !== "object") {
    return { ok: false, incomplete: true, reason: "ping returned no result object" };
  }
  return { ok: true };
}

function validateNavigate(payload) {
  if (typeof payload?.url !== "string" || !/\/index\.html$/.test(payload.url)) {
    return { ok: false, incomplete: true, reason: `navigate did not reach the fixture: ${JSON.stringify(payload?.url)}` };
  }
  if (typeof payload?.title !== "string" || payload.title.trim() === "") {
    return { ok: false, incomplete: true, reason: "navigate returned no page title" };
  }
  return { ok: true, detail: { url: payload.url, title: payload.title } };
}

function validateViewport(payload) {
  const readback = payload?.readback;
  if (!readback || typeof readback !== "object") {
    return { ok: false, incomplete: true, reason: "viewport returned no readback" };
  }
  if (readback.innerWidth !== 1280 || readback.innerHeight !== 720) {
    return {
      ok: false,
      incomplete: true,
      reason: `viewport readback is ${readback.innerWidth}x${readback.innerHeight}; required 1280x720`,
    };
  }
  if (typeof readback.devicePixelRatio === "number" && readback.devicePixelRatio !== 1) {
    return { ok: false, incomplete: true, reason: `devicePixelRatio is ${readback.devicePixelRatio}; required 1` };
  }
  return { ok: true, detail: { readback } };
}

export function countFixtureControls(payload) {
  if (!Array.isArray(payload?.nodes)) return 0;
  const names = payload.nodes
    .map((node) => node?.name)
    .filter((name) => typeof name === "string" && /^Control \d{3}$/.test(name));
  return new Set(names).size;
}

function validateSnapshot(payload) {
  if (!payload || typeof payload !== "object") {
    return { ok: false, incomplete: true, reason: "snapshot returned no payload" };
  }
  if (typeof payload.url !== "string" || !/\/index\.html$/.test(payload.url)) {
    return { ok: false, incomplete: true, reason: `snapshot is not on the fixture page: ${JSON.stringify(payload.url)}` };
  }
  if (payload.truncated !== false) {
    return { ok: false, incomplete: true, reason: `snapshot reports truncated=${JSON.stringify(payload.truncated)}` };
  }
  if (!Array.isArray(payload.nodes)) {
    return { ok: false, incomplete: true, reason: "snapshot returned no nodes array" };
  }
  const controls = countFixtureControls(payload);
  if (controls !== 100) {
    return { ok: false, incomplete: true, reason: `snapshot contains ${controls} labelled fixture controls; required 100` };
  }
  if (typeof payload.snapshotId !== "string" || payload.snapshotId === "") {
    return { ok: false, incomplete: true, reason: "snapshot returned no snapshotId" };
  }
  return { ok: true, detail: { controls, totalLines: payload.totalLines ?? null } };
}

function validateListWindows(payload, ctx) {
  if (!Array.isArray(payload?.windows)) {
    return { ok: false, incomplete: true, reason: "list_windows returned no windows array" };
  }
  const fingerprint = desktopTargetFingerprint(payload.windows, ctx.target);
  if (!fingerprint.ok) {
    return { ok: false, incomplete: true, reason: fingerprint.reason, observed: { windows: payload.windows.length } };
  }
  const window = fingerprint.matchedWindow;
  if (!Number.isFinite(window.hwnd) || !Number.isFinite(window.pid)) {
    return { ok: false, incomplete: true, reason: "matched fixture window has no numeric hwnd/pid" };
  }
  if (typeof window.title !== "string") {
    return { ok: false, incomplete: true, reason: "matched fixture window has no title" };
  }
  if (!Array.isArray(window.rect) || window.rect.length !== 4) {
    return { ok: false, incomplete: true, reason: "matched fixture window has no 4-element rect" };
  }
  if (!Array.isArray(window.clientRect) || window.clientRect.length !== 4) {
    return { ok: false, incomplete: true, reason: "matched fixture window has no 4-element client_rect" };
  }
  if (ctx.hwnd !== undefined && window.hwnd !== ctx.hwnd) {
    return { ok: false, incomplete: true, reason: `controlled target changed hwnd from ${ctx.hwnd} to ${window.hwnd}` };
  }
  ctx.hwnd = window.hwnd;
  ctx.matchedWindow = fingerprint.matchedWindow;
  ctx.targetIdentity = fingerprint.identity;
  ctx.targetHash = fingerprint.hash;
  return { ok: true, detail: { hwnd: window.hwnd, title: window.title, windows: payload.windows.length } };
}

export function validateCapture(payload, ctx) {
  const image = payload?.image;
  const source = payload?.source;
  if (!image || typeof image !== "object" || !Number.isFinite(image.w) || !Number.isFinite(image.h)) {
    return { ok: false, incomplete: true, reason: "capture returned no image dimensions" };
  }
  if (!source || typeof source !== "object") {
    return { ok: false, incomplete: true, reason: "capture returned no screen-space source rect" };
  }
  if (image.w !== 1280 || image.h !== 720) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture image is ${image.w}x${image.h}; required a native 1280x720 window`,
    };
  }
  if (source.w !== 1280 || source.h !== 720) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture source is ${source.w}x${source.h}; required 1280x720 at native resolution`,
    };
  }
  if (payload.scale !== 1) {
    return { ok: false, incomplete: true, reason: `capture scale is ${JSON.stringify(payload.scale)}; required 1` };
  }

  const win = payload.window;
  if (!win || typeof win !== "object") {
    return { ok: false, incomplete: true, reason: "capture returned no window identity" };
  }
  if (ctx.hwnd === undefined || win.hwnd !== ctx.hwnd) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture window hwnd ${JSON.stringify(win.hwnd)} does not prove the controlled target hwnd ${JSON.stringify(ctx.hwnd)}`,
    };
  }
  if (ctx.matchedWindow?.hwnd !== undefined && win.hwnd !== ctx.matchedWindow.hwnd) {
    return { ok: false, incomplete: true, reason: "capture hwnd changed from the setup-resolved target" };
  }
  if (!Number.isFinite(win.pid)) {
    return { ok: false, incomplete: true, reason: "capture window has no numeric pid" };
  }
  if (ctx.matchedWindow?.pid !== undefined && win.pid !== ctx.matchedWindow.pid) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture window pid ${win.pid} does not match the setup-resolved pid ${ctx.matchedWindow.pid}`,
    };
  }
  if (typeof win.title !== "string" || typeof win.exe !== "string") {
    return { ok: false, incomplete: true, reason: "capture window has no title/exe identity" };
  }
  if (ctx.target?.titleContains && !win.title.toLowerCase().includes(ctx.target.titleContains.toLowerCase())) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture window title ${JSON.stringify(win.title)} does not match ${JSON.stringify(ctx.target.titleContains)}`,
    };
  }
  if (ctx.target?.exe && basename(win.exe).toLowerCase() !== basename(ctx.target.exe).toLowerCase()) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture window exe ${JSON.stringify(win.exe)} does not match ${JSON.stringify(ctx.target.exe)}`,
    };
  }
  if (ctx.target?.pid !== undefined && win.pid !== ctx.target.pid) {
    return { ok: false, incomplete: true, reason: "capture window pid does not match the configured target" };
  }
  if (
    ctx.target?.className &&
    String(ctx.matchedWindow?.class ?? "").toLowerCase() !== ctx.target.className.toLowerCase()
  ) {
    return { ok: false, incomplete: true, reason: "setup did not prove the configured target class" };
  }
  if (win.minimized !== false) {
    return { ok: false, unsafe: true, reason: "capture reports the target as minimized or does not report it" };
  }
  if (win.cloaked !== 0 && win.cloaked !== false) {
    return { ok: false, unsafe: true, reason: `capture reports the target cloaked (${win.cloaked})` };
  }

  const capture = payload.capture;
  if (!capture || typeof capture !== "object") {
    return { ok: false, incomplete: true, reason: "capture returned no capture evidence" };
  }
  if (capture.backend !== "printwindow") {
    return {
      ok: false,
      unsafe: true,
      reason: `capture backend is ${JSON.stringify(capture.backend)}; required PrintWindow, not a crop fallback`,
    };
  }
  if (capture.moved_during_capture !== false) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture moved_during_capture is ${JSON.stringify(capture.moved_during_capture)}; required false`,
    };
  }
  if (typeof capture.stale !== "boolean") {
    return { ok: false, incomplete: true, reason: "capture stale flag is missing or not boolean" };
  }
  if (!Object.prototype.hasOwnProperty.call(capture, "note")) {
    return {
      ok: false,
      unsafe: true,
      reason: "capture omitted the note field; only the documented benign note or null is acceptable",
    };
  }
  if (capture.note !== null && capture.note !== MODE_B_NOTE) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture note is not the documented benign Mode B note: ${String(capture.note).slice(0, 200)}`,
    };
  }
  const blank = capture.blank_check;
  if (!blank || blank.verdict !== "content") {
    return {
      ok: false,
      unsafe: true,
      reason: `capture blank_check verdict is ${JSON.stringify(blank?.verdict)}; required content`,
    };
  }
  const blankThreshold = Number.isFinite(blank.threshold) ? blank.threshold : 8;
  if (!Number.isFinite(blank.max_channel) || blank.max_channel < blankThreshold || blank.max_channel < 8) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture blank_check max_channel is ${JSON.stringify(blank.max_channel)}; required at least ${Math.max(8, blankThreshold)}`,
    };
  }
  const noSteal = payload.noSteal;
  const hasAttribution = Boolean(noSteal && typeof noSteal === "object");
  let cursorExternal = false;
  let foregroundExternal = false;
  let baselineProbe = null;
  if (!hasAttribution) {
    if (ctx.product === "hand") {
      return {
        ok: false,
        incomplete: true,
        reason: "hand capture returned no pointer/foreground attribution (noSteal); the raw flags alone cannot say whose change it was",
      };
    }
    const probe = ctx.probe;
    if (!probe || !probe.before || !probe.after) {
      return {
        ok: false,
        incomplete: true,
        reason: `the product reports no attribution and the driver's input probe is unavailable${ctx.inputProbeError ? `: ${ctx.inputProbeError}` : ""}`,
      };
    }
    const before = probe.before;
    const after = probe.after;
    if (!Array.isArray(before.cursor) || !Array.isArray(after.cursor)) {
      return {
        ok: false,
        incomplete: true,
        reason: "the driver's input probe could not read the pointer around the call",
      };
    }
    const pointerMoved =
      before.cursor[0] !== after.cursor[0] || before.cursor[1] !== after.cursor[1];
    const inputArrived =
      Number.isInteger(before.inputTick) &&
      Number.isInteger(after.inputTick) &&
      before.inputTick !== after.inputTick;
    baselineProbe = { before, after, pointerMoved, inputArrived };
    if (payload.cursorUnchanged !== true && !(pointerMoved && inputArrived)) {
      return {
        ok: false,
        unsafe: true,
        reason: `capture reports the pointer moved (${JSON.stringify(before.cursor)} -> ${JSON.stringify(after.cursor)}) and the driver saw no physical input across the call; the movement is unattributable`,
      };
    }
    if (typeof payload.foregroundUnchanged !== "boolean") {
      return { ok: false, incomplete: true, reason: "capture foregroundUnchanged is missing or not boolean" };
    }
    if (after.foreground === ctx.hwnd && before.foreground !== ctx.hwnd) {
      return { ok: false, unsafe: true, reason: "the controlled target took the foreground during the capture" };
    }
    if (payload.foregroundUnchanged !== true && before.foreground === after.foreground) {
      return {
        ok: false,
        unsafe: true,
        reason: `capture reports the foreground moved but the driver saw ${JSON.stringify(before.foreground)} -> ${JSON.stringify(after.foreground)}; the claim is unattributable`,
      };
    }
  } else {
  if (noSteal.audited !== true) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture could not audit its own import table: ${noSteal.auditError ?? "no reason recorded"}`,
    };
  }
  if (!Array.isArray(noSteal.bannedImports)) {
    return { ok: false, incomplete: true, reason: "capture noSteal.bannedImports is missing or not an array" };
  }
  if (noSteal.bannedImports.length > 0) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture links pointer/foreground-writing APIs: ${noSteal.bannedImports.join(", ")}`,
    };
  }
  const pointer = noSteal.pointer;
  if (!pointer || typeof pointer !== "object") {
    return { ok: false, incomplete: true, reason: "capture noSteal.pointer is missing" };
  }
  if (typeof payload.cursorUnchanged !== "boolean") {
    return { ok: false, incomplete: true, reason: "capture cursorUnchanged is missing or not boolean" };
  }
  if (pointer.readable !== true) {
    return {
      ok: false,
      incomplete: true,
      reason: "capture could not read the pointer before and after the call",
    };
  }
  if (
    !Array.isArray(pointer.before) ||
    pointer.before.length !== 2 ||
    !Array.isArray(pointer.after) ||
    pointer.after.length !== 2
  ) {
    return { ok: false, incomplete: true, reason: "capture pointer before/after samples are malformed" };
  }
  const pointerMoved = pointer.before[0] !== pointer.after[0] || pointer.before[1] !== pointer.after[1];
  if ((pointer.moved === true) !== pointerMoved) {
    return { ok: false, incomplete: true, reason: "capture pointer.moved contradicts the before/after samples" };
  }
  if ((payload.cursorUnchanged === true) === pointerMoved) {
    return { ok: false, incomplete: true, reason: "capture cursorUnchanged contradicts the before/after samples" };
  }
  const tickReadable =
    Number.isInteger(pointer.inputTickBefore) && Number.isInteger(pointer.inputTickAfter);
  const inputAdvanced = tickReadable && pointer.inputTickBefore !== pointer.inputTickAfter;
  if (pointer.inputDuringCall !== inputAdvanced) {
    return { ok: false, incomplete: true, reason: "capture pointer.inputDuringCall contradicts the input ticks" };
  }
  cursorExternal = pointerMoved && inputAdvanced;
  if ((pointer.externalInput === true) !== cursorExternal) {
    return { ok: false, incomplete: true, reason: "capture pointer.externalInput contradicts the samples" };
  }
  if (payload.cursorUnchanged !== true && !cursorExternal) {
    return {
      ok: false,
      unsafe: true,
      reason: `the pointer moved (${JSON.stringify(pointer.before)} -> ${JSON.stringify(pointer.after)}) with no external input in the window; the capture cannot attribute it`,
    };
  }

  const foreground = noSteal.foreground;
  if (!foreground || typeof foreground !== "object") {
    return { ok: false, incomplete: true, reason: "capture noSteal.foreground is missing" };
  }
  if (typeof payload.foregroundUnchanged !== "boolean") {
    return { ok: false, incomplete: true, reason: "capture foregroundUnchanged is missing or not boolean" };
  }
  if (foreground.target !== ctx.hwnd) {
    return {
      ok: false,
      incomplete: true,
      reason: `capture noSteal.foreground.target ${JSON.stringify(foreground.target)} does not name the controlled hwnd ${JSON.stringify(ctx.hwnd)}`,
    };
  }
  const foregroundMoved = foreground.before !== foreground.after;
  if ((foreground.moved === true) !== foregroundMoved) {
    return { ok: false, incomplete: true, reason: "capture foreground.moved contradicts the before/after handles" };
  }
  if ((payload.foregroundUnchanged === true) === foregroundMoved) {
    return { ok: false, incomplete: true, reason: "capture foregroundUnchanged contradicts the before/after handles" };
  }
  const targetActivated =
    foregroundMoved &&
    foreground.targetIsForeground === true &&
    foreground.targetWasForeground === false;
  if ((foreground.targetActivated === true) !== targetActivated) {
    return { ok: false, incomplete: true, reason: "capture foreground.targetActivated contradicts the handles" };
  }
  const stolen =
    foregroundMoved &&
    (foreground.afterOwnedByThisProcess === true || targetActivated);
  if ((foreground.stolen === true) !== stolen || noSteal.stolen !== stolen) {
    return { ok: false, incomplete: true, reason: "capture stolen flag contradicts the foreground observation" };
  }
  foregroundExternal = foregroundMoved && !stolen;
  if ((foreground.externalChange === true) !== foregroundExternal) {
    return { ok: false, incomplete: true, reason: "capture foreground.externalChange contradicts the observation" };
  }
  if (stolen) {
    return {
      ok: false,
      unsafe: true,
      reason: `the capture's process or target took the foreground (after=${JSON.stringify(foreground.after)}, targetActivated=${JSON.stringify(targetActivated)})`,
    };
  }
  if (payload.foregroundUnchanged !== true && !foregroundExternal) {
    return {
      ok: false,
      unsafe: true,
      reason: `the foreground changed from ${JSON.stringify(foreground.before)} to ${JSON.stringify(foreground.after)} and is not an external window`,
    };
  }
  }
  if (Array.isArray(payload.warnings) && payload.warnings.length > 0) {
    return {
      ok: false,
      unsafe: true,
      reason: `capture returned ${payload.warnings.length} warning(s) despite png/no-quality request`,
    };
  }

  return {
    ok: true,
    detail: {
      image: { w: image.w, h: image.h },
      source: { w: source.w, h: source.h },
      scale: payload.scale,
      hwnd: win.hwnd,
      pid: win.pid,
      title: win.title,
      exe: win.exe,
      backend: capture.backend,
      blank: { verdict: blank.verdict, max_channel: blank.max_channel, threshold: blankThreshold },
      note: capture.note,
      stale: capture.stale,
      foregroundUnchanged: payload.foregroundUnchanged,
      cursorUnchanged: payload.cursorUnchanged,
      attribution: hasAttribution ? "noSteal" : "raw",
      foregroundExternal,
      cursorExternal,
      bannedImports: hasAttribution ? noSteal.bannedImports : null,
      driverProbe: baselineProbe,
    },
  };
}
