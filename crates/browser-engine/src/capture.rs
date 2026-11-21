//! Deterministic pixel capture.
//!
//! One screenshot taken under conditions that are fully pinned by the engine
//! instead of by whatever the page happened to be doing:
//!
//! - `Emulation.setDeviceMetricsOverride` fixes the viewport and forces
//!   `deviceScaleFactor: 1` / `mobile: false` (restored afterwards: the prior
//!   override is replayed, or cleared when there was none);
//! - `Emulation.setEmulatedMedia` reports `prefers-reduced-motion: reduce`
//!   while the capture runs and is cleared afterwards. The clear returns the
//!   page to the ENGINE/HOST default, not to `false`: a host that asks for
//!   reduced motion (CI runners disable animations) reports `reduce` again
//!   after the clear. The invariant is therefore "unchanged from the
//!   pre-capture value", the reply reports whether the emulation was applied
//!   at all (`reducedMotionEmulated`), and the freeze stylesheet below is what
//!   stops CSS animation on every engine;
//! - a temporary style element freezes `animation`, `transition`,
//!   `caret-color`, smooth scrolling and scrollbar gutter/width;
//! - the page is settled before the shot: `document.fonts.ready`, every `img`
//!   `complete` (or errored), then two `requestAnimationFrame` ticks, each
//!   wait raced against a timer so a background tab that starves rAF still
//!   settles instead of hanging;
//! - `Page.captureScreenshot` with `format: "png"`, `fromSurface: true` and
//!   `captureBeyondViewport` for full-page shots.
//!
//! Restoration is CONFIRMED, not assumed. The overrides are reverted with
//! awaited CDP commands, but a reply only says the agent accepted them: the
//! page's media-query and viewport state are refreshed on the renderer's own
//! schedule, and a clear can be lost outright. The capture therefore polls the
//! page (bounded, 2 s cap) until it reports the pre-capture viewport/dsf, no
//! freeze stylesheet and no emulated reduced-motion, and re-applies the
//! restore once if it does not - so a caller that reads the page immediately
//! after this call can never observe the emulated state. That gap is what the
//! release gate hit with `matchMedia('(prefers-reduced-motion: reduce)')`.
//!
//! Everything rides CDP: no OS input, no window, no foreground tab.
//! The checksum is FNV-1a 64 over the decoded PNG bytes - cheap, dependency
//! free, and enough to prove two captures are byte-identical.

use base64::Engine as _;
use serde_json::{json, Value};

use crate::browser::Browser;

/// Backstop budget for the settle evaluate. The page races its own waits
/// against timers, so this only fires for a genuinely stuck renderer.
const SETTLE_CAP: std::time::Duration = std::time::Duration::from_secs(5);

/// Temporary freeze style. Injected with a stable id so removal is exact.
const STYLE_ID: &str = "__gladius_deterministic";

/// The freeze walk (S1.5): the stylesheet alone only reaches the main
/// document, so a WAAPI animation, an SVG SMIL timeline, a playing video, an
/// open shadow root or a same-process iframe kept moving. This pass visits
/// every realm it can reach, injects the stylesheet there, pauses WAAPI
/// animations, SMIL timelines and media playback, and records one restore
/// closure per mutation so the undo is exact.
///
/// What it cannot reach is recorded, never silent: a cross-origin frame throws
/// on `contentDocument` and lands in `skipped`. A CLOSED shadow root is not
/// reachable from the page realm at all (`el.shadowRoot` is null for it), so
/// it cannot be enumerated here; naming it is S2.5's job, where the CDP DOM
/// tree is the ground truth.
fn freeze_js() -> String {
    format!(
        r#"(() => {{
  const STYLE_ID = '{STYLE_ID}';
  const CSS = '{CSS}';
  const state = {{restore: [], skipped: [], shadowRoots: 0, frames: 0,
                 waapiPaused: 0, smilPaused: 0, mediaPaused: 0}};
  const seenAnims = new Set();
  const applyStyle = (root, doc) => {{
    try {{
      let el = root.querySelector ? root.querySelector('#' + STYLE_ID) : null;
      if (!el) {{
        el = doc.createElement('style');
        el.id = STYLE_ID;
        const target = root.nodeType === 9 ? (doc.head || doc.documentElement) : root;
        if (!target) return;
        target.appendChild(el);
      }}
      el.textContent = CSS;
      state.restore.push(() => {{ try {{ el.remove(); }} catch (e) {{}} }});
    }} catch (e) {{}}
  }};
  const pauseAnims = (root) => {{
    let anims = [];
    try {{ anims = root.getAnimations ? root.getAnimations() : []; }} catch (e) {{ return; }}
    for (const a of anims) {{
      if (seenAnims.has(a)) continue;
      seenAnims.add(a);
      try {{
        const was = a.playState;
        const at = a.currentTime;
        a.pause();
        try {{ a.currentTime = 0; }} catch (e) {{}}
        state.waapiPaused++;
        state.restore.push(() => {{
          try {{
            if (at !== null && at !== undefined) a.currentTime = at;
            if (was === 'running') a.play();
          }} catch (e) {{}}
        }});
      }} catch (e) {{}}
    }}
  }};
  const visit = (root, doc) => {{
    applyStyle(root, doc);
    pauseAnims(root);
    let svgs = [], media = [], hosts = [], frameEls = [];
    try {{
      svgs = Array.prototype.slice.call(root.querySelectorAll('svg'));
      media = Array.prototype.slice.call(root.querySelectorAll('video,audio'));
      hosts = Array.prototype.slice.call(root.querySelectorAll('*')).filter((el) => el.shadowRoot);
      frameEls = Array.prototype.slice.call(root.querySelectorAll('iframe'));
    }} catch (e) {{}}
    for (const svg of svgs) {{
      try {{
        svg.pauseAnimations();
        try {{ svg.setCurrentTime(0); }} catch (e) {{}}
        state.smilPaused++;
        state.restore.push(() => {{ try {{ svg.unpauseAnimations(); }} catch (e) {{}} }});
      }} catch (e) {{}}
    }}
    for (const m of media) {{
      try {{
        const wasPaused = m.paused;
        const t = m.currentTime;
        m.pause();
        try {{ m.currentTime = 0; }} catch (e) {{}}
        state.mediaPaused++;
        state.restore.push(() => {{
          try {{
            m.currentTime = t;
            if (!wasPaused) {{
              const p = m.play();
              if (p && p.catch) p.catch(() => {{}});
            }}
          }} catch (e) {{}}
        }});
      }} catch (e) {{}}
    }}
    for (const el of hosts) {{
      state.shadowRoots++;
      visit(el.shadowRoot, el.shadowRoot.ownerDocument || doc);
    }}
    for (const f of frameEls) {{
      let d = null;
      try {{ d = f.contentDocument; }} catch (e) {{ d = null; }}
      if (!d) {{
        if (state.skipped.indexOf('cross-origin-iframe') < 0) state.skipped.push('cross-origin-iframe');
        continue;
      }}
      state.frames++;
      visit(d, d);
    }}
  }};
  visit(document, document);
  window.__gladiusFreeze = state;
  return {{waapiPaused: state.waapiPaused, smilPaused: state.smilPaused,
           mediaPaused: state.mediaPaused, shadowRoots: state.shadowRoots,
           frames: state.frames, skipped: state.skipped}};
}})()"#
    )
}

/// Undo the freeze walk in reverse order, from the state the apply pass left on
/// the window. Every closure is guarded: a frame that navigated away must not
/// abort the rest of the restore.
fn freeze_restore_js() -> String {
    r#"(() => {
  const st = window.__gladiusFreeze;
  let restored = 0;
  if (st && Array.isArray(st.restore)) {
    for (let i = st.restore.length - 1; i >= 0; i--) {
      try { st.restore[i](); restored++; } catch (e) {}
    }
  }
  window.__gladiusFreeze = null;
  return restored;
})()"#
        .to_string()
}

fn style_restore_js() -> String {
    format!(
        "(() => {{ const el = document.getElementById('{STYLE_ID}'); if (el) el.remove(); \
         return document.getElementById('{STYLE_ID}') === null; }})()"
    )
}

/// The motion inventory for a plain capture. It names the running animations
/// (CSS by animation name, WAAPI by id or constructor), playing videos and
/// fonts still loading. It is a read: nothing here mutates the page.
const MOTION_JS: &str = r#"(() => {
  const out = [];
  try {
    for (const a of (document.getAnimations ? document.getAnimations() : [])) {
      let state = ''; try { state = a.playState; } catch (e) {}
      if (state !== 'running') continue;
      let name = ''; try { name = a.animationName || ''; } catch (e) {}
      if (!name) { try { name = a.id || ''; } catch (e) {} }
      if (!name) { try { name = a.constructor.name || 'animation'; } catch (e) { name = 'animation'; } }
      out.push('animation:' + name);
    }
  } catch (e) {}
  try {
    for (const v of document.querySelectorAll('video')) {
      if (v.paused || v.ended) continue;
      let id = 'video'; try { id = v.id || v.className || 'video'; } catch (e) {}
      out.push('video:' + id);
    }
  } catch (e) {}
  try { if (document.fonts && document.fonts.status === 'loading') out.push('fonts-loading'); } catch (e) {}
  return out;
})()"#;

/// Pause the live motion sources and inject a style that holds animations and
/// transitions still. Every mutation records its own restore closure; the
/// restore runs in reverse so the page is left as it was found.
const SHOT_FREEZE_JS: &str = r#"(() => {
  const paused = [];
  const restore = [];
  try {
    for (const a of (document.getAnimations ? document.getAnimations() : [])) {
      let state = ''; try { state = a.playState; } catch (e) {}
      if (state !== 'running') continue;
      let name = ''; try { name = a.animationName || ''; } catch (e) {}
      if (!name) { try { name = a.id || ''; } catch (e) {} }
      if (!name) { try { name = a.constructor.name || 'animation'; } catch (e) { name = 'animation'; } }
      let at = null; try { at = a.currentTime; } catch (e) {}
      try { a.pause(); } catch (e) { continue; }
      paused.push('animation:' + name);
      restore.push(() => { try { if (at !== null && at !== undefined) a.currentTime = at; a.play(); } catch (e) {} });
    }
  } catch (e) {}
  try {
    for (const v of document.querySelectorAll('video')) {
      if (v.paused || v.ended) continue;
      let id = 'video'; try { id = v.id || v.className || 'video'; } catch (e) {}
      let at = null; try { at = v.currentTime; } catch (e) {}
      try { v.pause(); } catch (e) { continue; }
      paused.push('video:' + id);
      restore.push(() => { try {
        if (at !== null && at !== undefined) v.currentTime = at;
        const p = v.play(); if (p && p.catch) p.catch(() => {});
      } catch (e) {} });
    }
  } catch (e) {}
  const style = document.createElement('style');
  style.id = '__hand_shot_freeze';
  style.textContent = '*,*::before,*::after{animation-play-state:paused!important;transition:none!important;transition-duration:0s!important;caret-color:transparent!important;scroll-behavior:auto!important;}';
  try { (document.head || document.documentElement).appendChild(style); } catch (e) {}
  restore.push(() => { try { style.remove(); } catch (e) {} });
  // Bounded page-side self-restore. If the Rust future is dropped by a cancel
  // (no normal-path restore can run), the timer restores this freeze anyway.
  // The closure captures `restore` by value so a later freeze overwriting the
  // registry cannot make this timer undo the wrong set of closures.
  const timer = setTimeout(() => {
    for (let i = restore.length - 1; i >= 0; i--) {
      try { restore[i](); } catch (e) {}
    }
    if (window.__handShotFreeze && window.__handShotFreeze.restore === restore) {
      window.__handShotFreeze = null;
    }
  }, 60000);
  window.__handShotFreeze = {restore: restore, timer: timer};
  return paused;
})()"#;

const SHOT_FREEZE_RESTORE_JS: &str = r#"(() => {
  const st = window.__handShotFreeze;
  let restored = 0;
  if (st && st.timer) { try { clearTimeout(st.timer); } catch (e) {} }
  if (st && Array.isArray(st.restore)) {
    for (let i = st.restore.length - 1; i >= 0; i--) {
      try { st.restore[i](); restored++; } catch (e) {}
    }
  }
  window.__handShotFreeze = null;
  return restored;
})()"#;

/// Names of the motion sources still running on the page.
pub(crate) async fn shot_motion(browser: &Browser) -> Vec<String> {
    match browser.eval(MOTION_JS).await {
        Ok(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Pause the page's motion sources; returns the names acted on.
pub(crate) async fn apply_shot_freeze(browser: &Browser) -> Vec<String> {
    match browser.eval(SHOT_FREEZE_JS).await {
        Ok(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) async fn restore_shot_freeze(browser: &Browser) {
    if let Err(e) = browser.eval(SHOT_FREEZE_RESTORE_JS).await {
        warn(&format!("screenshot: could not undo the motion freeze: {e}"));
    }
}

/// The freeze rules: animation/transition stopped (delay and duration
/// pinned to 0 so a running one cannot resume), caret hidden, smooth
/// scrolling and overflow anchoring off, scrollbar gutter forced to auto and
/// the scrollbar itself collapsed to zero so its presence cannot change the
/// content box between runs.
const CSS: &str = "*,*::before,*::after{animation:none!important;animation-delay:0s!important;animation-duration:0s!important;animation-iteration-count:1!important;transition:none!important;transition-delay:0s!important;transition-duration:0s!important;caret-color:transparent!important;scroll-behavior:auto!important;overflow-anchor:none!important;}html,body{scrollbar-gutter:auto!important;}::-webkit-scrollbar{width:0!important;height:0!important;display:none!important;}";

/// Fonts + images + two animation frames, every wait bounded.
///
/// Each race that loses records its own name, so "the capture settled" is a
/// fact the reply can carry instead of a stderr line the caller never sees.
/// The closed set is `fonts`, `images`, `raf` (the outer `SETTLE_CAP` adds
/// `settleCap` from the Rust side).
const SETTLE_JS: &str = r#"(() => {
  const timeouts = [];
  const race = (name, ms, wait) => new Promise((resolve) => {
    let finished = false;
    const end = (late) => {
      if (finished) return;
      finished = true;
      if (late) timeouts.push(name);
      resolve(true);
    };
    const timer = setTimeout(() => end(true), ms);
    try { wait(() => { clearTimeout(timer); end(false); }); } catch (e) { clearTimeout(timer); end(false); }
  });
  return (async () => {
    const facesLoading = () => {
      try {
        return Array.from(document.fonts).filter((f) => f.status === 'loading');
      } catch (e) { return []; }
    };
    await race('fonts', 3000, (ok) => {
      const check = () => {
        let pending = [];
        try { pending = facesLoading(); } catch (e) { ok(); return; }
        if (pending.length === 0) { ok(); return; }
        try {
          Promise.all(pending.map((f) => {
            try { return f.loaded.catch(() => {}); } catch (e) { return null; }
          })).then(check, check);
        } catch (e) { ok(); }
      };
      check();
    });
    const fontsPending = facesLoading().length;
    if (fontsPending > 0 && timeouts.indexOf('fonts') < 0) timeouts.push('fonts');
    const imgs = Array.prototype.slice.call(document.images || []);
    for (const i of imgs) {
      if (i.complete) continue;
      await race('images', 3000, (ok) => {
        i.addEventListener('load', () => ok(), {once: true});
        i.addEventListener('error', () => ok(), {once: true});
      });
    }
    await race('raf', 250, (ok) => requestAnimationFrame(() => ok()));
    await race('raf', 250, (ok) => requestAnimationFrame(() => ok()));
    return {settled: timeouts.length === 0, timeouts: timeouts, fontsPending: fontsPending};
  })();
})()"#;

/// Apply the freeze walk to the page and return the counts it reached.
/// `pub(crate)` because `assert_layout {deterministic:true}` must measure the
/// same state the camera sees - one freeze implementation, not two (S1.6).
pub(crate) async fn apply_freeze(browser: &Browser) -> Value {
    match browser.eval(&freeze_js()).await {
        Ok(v) => {
            if v.is_null() {
                warn("deterministic capture: the freeze walk returned no state");
            }
            v
        }
        Err(e) => {
            warn(&format!("deterministic capture: freeze walk failed: {e}"));
            Value::Null
        }
    }
}

/// Undo the freeze walk (reverse order, every closure guarded).
pub(crate) async fn restore_freeze(browser: &Browser) {
    if let Err(e) = browser.eval(&freeze_restore_js()).await {
        warn(&format!(
            "deterministic capture: could not undo the freeze walk: {e}"
        ));
    }
}

/// Live `(innerWidth, innerHeight, devicePixelRatio, href)` for the capture
/// metadata. Every value is read back from the page, never assumed.
async fn live_metrics(browser: &Browser) -> Value {
    browser
        .eval("[window.innerWidth, window.innerHeight, window.devicePixelRatio, location.href]")
        .await
        .unwrap_or(Value::Null)
}

/// The image encoding a capture asks for. `Png` is the historic default and
/// stays byte-identical: no `quality` is ever sent with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Png,
    Jpeg,
    WebP,
}

impl Format {
    /// The `Page.captureScreenshot` `format` value.
    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::WebP => "webp",
        }
    }

    /// The MIME type the reply advertises for exactly these bytes.
    pub fn mime(self) -> &'static str {
        match self {
            Format::Png => "image/png",
            Format::Jpeg => "image/jpeg",
            Format::WebP => "image/webp",
        }
    }

    /// Does this encoding take a `quality` (0-100)? PNG is lossless and its
    /// `quality` parameter is meaningless, so a caller that sends one is
    /// refused by name instead of having it silently dropped.
    pub fn takes_quality(self) -> bool {
        matches!(self, Format::Jpeg | Format::WebP)
    }
}

/// A capture region in PAGE coordinates (CSS px from the document origin) -
/// the same space `DOM.getBoxModel` reports and `browser_layout` returns, so a
/// box measured there can be captured here without a conversion. `scale: 1`
/// is always sent, so the pixels are CSS px at the page's device scale factor.
#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One capture, either plain (the historic behaviour) or deterministic.
pub struct Options {
    pub full_page: bool,
    pub deterministic: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// Capture only this page-space region. Mutually exclusive with
    /// `full_page`; the tool refuses both together rather than picking one.
    pub clip: Option<Clip>,
    pub format: Format,
    /// 0-100, only for the encodings that take one (`Format::takes_quality`).
    pub quality: Option<u8>,
    /// CDP clip scale, 0.1..=1. 1 keeps the historic pixel dimensions.
    pub scale: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            full_page: false,
            deterministic: false,
            width: None,
            height: None,
            clip: None,
            format: Format::Png,
            quality: None,
            scale: 1.0,
        }
    }
}

/// A decoded capture plus the metadata that makes it auditable.
pub struct Capture {
    pub base64: String,
    pub bytes: Vec<u8>,
    pub checksum: String,
    pub width: i64,
    pub height: i64,
    /// Real PNG pixel dimensions from the IHDR chunk (device pixels); these
    /// differ from `width`/`height` on a full-page capture by construction.
    pub png_width: i64,
    pub png_height: i64,
    pub dsf: f64,
    pub url: String,
    pub deterministic: bool,
    /// The encoding of `bytes` - the reply's `mimeType` and `format` are this,
    /// never assumed PNG.
    pub format: Format,
    /// True when this engine actually emulated `prefers-reduced-motion`
    /// (`Emulation.setEmulatedMedia` implemented). False on the pinned
    /// `chrome-headless-shell`, where the preference stays the host's and only
    /// the freeze stylesheet stops motion.
    pub reduced_motion_emulated: bool,
    /// Did every settle wait finish inside its budget? `settled ==
    /// settle_timeouts.is_empty()` by construction: a lost race still returns
    /// the pixels, only the claim changes.
    pub settled: bool,
    /// The closed set of waits that lost: `fonts`, `images`, `raf`, `settleCap`
    /// (and, after S1.5, `waapi`, `smil`, `media`).
    pub settle_timeouts: Vec<String>,
    /// What the freeze walk actually reached: `{waapiPaused, smilPaused,
    /// mediaPaused, shadowRoots, frames, skipped[]}`. Deterministic only.
    pub freeze: Value,
    /// `FontFace`s still `loading` when the capture was taken (0 = none).
    pub fonts_pending: i64,
}

impl Capture {
    /// Deterministic metadata block (view n/a when `deterministic` is false).
    pub fn meta(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("mimeType".to_string(), json!(self.format.mime()));
        m.insert("format".to_string(), json!(self.format.name()));
        m.insert("bytes".to_string(), json!(self.bytes.len() as i64));
        if self.png_width > 0 {
            m.insert(
                "size".to_string(),
                json!({"width": self.png_width, "height": self.png_height}),
            );
        }
        if self.deterministic {
            m.insert("deterministic".to_string(), json!(true));
            m.insert(
                "viewport".to_string(),
                json!({"width": self.width, "height": self.height}),
            );
            m.insert("deviceScaleFactor".to_string(), json!(self.dsf));
            m.insert("url".to_string(), json!(self.url));
            m.insert("checksum".to_string(), json!(self.checksum));
            m.insert(
                "reducedMotionEmulated".to_string(),
                json!(self.reduced_motion_emulated),
            );
            m.insert("settled".to_string(), json!(self.settled));
            m.insert("settleTimeouts".to_string(), json!(self.settle_timeouts));
            m.insert("freeze".to_string(), self.freeze.clone());
            m.insert("freezeMode".to_string(), json!("animation-none"));
            m.insert("fontsPending".to_string(), json!(self.fonts_pending));
        }
        Value::Object(m)
    }
}

/// One stderr line for a non-fatal capture problem. Restoration problems are
/// never swallowed: the capture still succeeds (the pixels are real), but the
/// operator can see that a page may be left with a stale override.
pub(crate) fn warn(msg: &str) {
    err_eprintln!("EWARN", "[gladius] warn: {msg}");
}

/// Test-only hook: hold the critical section open for this many milliseconds
/// after the overrides are applied (and, in `visual::compare`, after the
/// scratch target exists). It exists so the cancellation tests can cancel
/// deterministically in the middle of the mutation window instead of racing a
/// fast capture. Never set in production; an unparseable value is ignored.
pub(crate) fn test_delay_ms() -> Option<u64> {
    std::env::var("GLADIUS_TEST_CRITICAL_DELAY_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
}

/// Apply the test-only critical-section delay when the hook is set.
pub(crate) async fn maybe_test_delay() {
    if let Some(ms) = test_delay_ms() {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// Does this engine implement `Emulation.setEmulatedMedia`? The pinned
/// `chrome-headless-shell` 155 DOES, on a PAGE session (on the browser-level
/// connection the same method answers `-32601`, which is why a capability
/// probe must go through a session - measured on the pin). This cache only
/// guards an engine that genuinely lacks it: the first definitive answer is
/// cached (0 = unknown, 1 = supported, 2 = unsupported).
static MEDIA_EMU: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn media_emu_known() -> Option<bool> {
    use std::sync::atomic::Ordering;
    match MEDIA_EMU.load(Ordering::SeqCst) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn media_emu_remember(supported: bool) {
    use std::sync::atomic::Ordering;
    MEDIA_EMU.store(if supported { 1 } else { 2 }, Ordering::SeqCst);
}

/// "This engine has no such command" - the CDP error for a method that is not
/// part of the build's protocol surface.
fn method_missing(err: &str) -> bool {
    err.contains("-32601") || err.contains("wasn't found") || err.contains("was not found")
}

/// The explicit reset: empty media type plus an empty feature list. On an
/// engine that implements the command this removes the emulation entirely
/// (back to the engine/host default); it is never a "set no-preference"
/// override, which would *change* a host that prefers reduced motion.
fn clear_media_params() -> Value {
    json!({"media": "", "features": []})
}

/// How long the restore may take to become visible to the PAGE. Bounded: a
/// wedged renderer must not hang a capture (the pixels are already valid, so
/// the confirmation is warned about, never fatal).
const RESTORE_CONFIRM_CAP: std::time::Duration = std::time::Duration::from_secs(2);

/// Cadence of the confirmation probe. Small: normally the very first probe
/// already agrees (the clear is applied by the time the reply lands), so the
/// steady-state cost is one `Runtime.evaluate`.
const RESTORE_CONFIRM_STEP: std::time::Duration = std::time::Duration::from_millis(10);

/// Test-only hook (never set in production): drop the first N media clears,
/// simulating a lost clear (the symptom the release gate hit). The restore
/// confirmation is what repairs it, and the counter lets the retry succeed.
fn test_drop_media_clears() -> u32 {
    std::env::var("GLADIUS_TEST_DROP_MEDIA_CLEAR")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

/// Consume one "drop the clear" credit, if any are left. Re-armed from the
/// environment whenever it runs dry, so a harness may set the hook at any
/// point in the process (the selftest does) and each capture gets its own
/// drop.
fn take_dropped_clear() -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    static REMAINING: AtomicU32 = AtomicU32::new(0);
    if REMAINING.load(Ordering::SeqCst) == 0 {
        let want = test_drop_media_clears();
        if want == 0 {
            return false;
        }
        REMAINING.store(want, Ordering::SeqCst);
    }
    REMAINING
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
}

/// Page-level view of the restored state: metrics, freeze stylesheet gone and
/// the motion preference back to what the page reported BEFORE the capture.
/// That last value is deliberately the baseline, not `false`: the clear
/// returns the page to the engine/host default, and a host that asks for
/// reduced motion (a CI runner with animations disabled reports `reduce`)
/// reports it again afterwards - so `false` would assert something the capture
/// neither set nor owns.
///
/// This is the ONLY proof that restoration is complete - a CDP reply says the
/// agent accepted the command, not that the page has applied it.
fn restore_probe_js() -> String {
    format!(
        "[window.innerWidth, window.innerHeight, window.devicePixelRatio, \
          document.getElementById('{STYLE_ID}') === null, \
          matchMedia('(prefers-reduced-motion: reduce)').matches]"
    )
}

/// Poll the page (bounded) until it reports the pre-capture state: the
/// viewport/dsf we read before overriding, no freeze stylesheet, and no
/// emulated reduced-motion. Returns true when the page agrees.
async fn confirm_restored(browser: &Browser, expect: (i64, i64, f64, bool)) -> bool {
    let (ew, eh, edpr, e_reduce) = expect;
    let expr = restore_probe_js();
    let deadline = std::time::Instant::now() + RESTORE_CONFIRM_CAP;
    loop {
        if let Ok(v) = browser.eval(&expr).await {
            let a = v.as_array().cloned().unwrap_or_default();
            let num = |i: usize| a.get(i).and_then(|x| x.as_f64());
            let style_gone = a.get(3).and_then(|x| x.as_bool()) == Some(true);
            let reduced_motion_restored = a.get(4).and_then(|x| x.as_bool()) == Some(e_reduce);
            let size_ok = num(0) == Some(ew as f64) && num(1) == Some(eh as f64);
            let dpr_ok = num(2).map(|d| (d - edpr).abs() < 0.001).unwrap_or(false);
            if size_ok && dpr_ok && style_gone && reduced_motion_restored {
                return true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(RESTORE_CONFIRM_STEP).await;
    }
}

/// FNV-1a 64 over `bytes`, 16 hex digits. Cheap content fingerprint: two
/// captures of the same pixels produce the same PNG bytes and the same
/// checksum, and any pixel change breaks it.
pub fn checksum(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The `Page.captureScreenshot` params for this capture. PNG without a clip
/// keeps the historic key order and set exactly (`format`, then
/// `captureBeyondViewport` for a full page, then `fromSurface`).
fn shot_params(opts: &Options) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("format".to_string(), json!(opts.format.name()));
    if let Some(c) = opts.clip {
        p.insert(
            "clip".to_string(),
            json!({"x": c.x, "y": c.y, "width": c.width, "height": c.height, "scale": opts.scale}),
        );
        p.insert("captureBeyondViewport".to_string(), json!(true));
    } else if opts.full_page {
        p.insert("captureBeyondViewport".to_string(), json!(true));
    }
    p.insert("fromSurface".to_string(), json!(true));
    if opts.format.takes_quality() {
        if let Some(q) = opts.quality {
            p.insert("quality".to_string(), json!(q));
        }
    }
    Value::Object(p)
}

fn decode(r: &Value, deterministic: bool, format: Format) -> Result<Capture, String> {
    let data = r
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or("screenshot: no data returned".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("screenshot: capture payload is not valid base64: {e}"))?;
    let sum = checksum(&bytes);
    // Real decoded pixel dimensions for every encoding, read from the bytes
    // themselves (a scale override changes these without changing the box).
    let (png_width, png_height) = image_size(&bytes, format).unwrap_or((0, 0));
    Ok(Capture {
        base64: data.to_string(),
        bytes,
        checksum: sum,
        width: 0,
        height: 0,
        png_width,
        png_height,
        dsf: 1.0,
        url: String::new(),
        deterministic,
        format,
        reduced_motion_emulated: false,
        settled: true,
        settle_timeouts: Vec::new(),
        freeze: Value::Null,
        fonts_pending: 0,
    })
}

/// Real PNG pixel size from the IHDR chunk: signature (8) + length (4) +
/// type (4) puts width/height at bytes 16..24, big-endian. No dependency.
fn png_size(bytes: &[u8]) -> Option<(i64, i64)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?) as i64;
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?) as i64;
    Some((w, h))
}

/// JPEG dimensions from the first SOF marker (C0..CF except C4/C8/CC).
fn jpeg_size(bytes: &[u8]) -> Option<(i64, i64)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if len < 2 || i + 2 + len > bytes.len() {
            break;
        }
        let sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof && len >= 7 {
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as i64;
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as i64;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
        i += 2 + len;
    }
    None
}

/// WebP dimensions from the RIFF header's VP8X/VP8 /VP8L chunk.
fn webp_size(bytes: &[u8]) -> Option<(i64, i64)> {
    if bytes.len() < 16 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8X" if bytes.len() >= 30 => {
            let w = 1 + bytes[24] as i64 + (bytes[25] as i64) * 256 + (bytes[26] as i64) * 65536;
            let h = 1 + bytes[27] as i64 + (bytes[28] as i64) * 256 + (bytes[29] as i64) * 65536;
            Some((w, h))
        }
        b"VP8 " if bytes.len() >= 30 => {
            let w = u16::from_le_bytes([bytes[26], bytes[27]]) as i64 & 0x3fff;
            let h = u16::from_le_bytes([bytes[28], bytes[29]]) as i64 & 0x3fff;
            Some((w, h))
        }
        b"VP8L" if bytes.len() >= 25 => {
            let bits = bytes[21] as i64
                | (bytes[22] as i64) * 256
                | (bytes[23] as i64) * 65536
                | (bytes[24] as i64) * 16_777_216;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

fn image_size(bytes: &[u8], format: Format) -> Option<(i64, i64)> {
    match format {
        Format::Png => png_size(bytes),
        Format::Jpeg => jpeg_size(bytes),
        Format::WebP => webp_size(bytes),
    }
}

/// Replay the prior device-metrics override (or clear it when there was
/// none). Used by the restore and by its retry, so both paths are identical.
async fn restore_metrics(browser: &Browser, prior: Option<&Value>) {
    match prior {
        Some(p) => {
            if let Err(e) = browser
                .command("Emulation.setDeviceMetricsOverride", p.clone(), true)
                .await
            {
                warn(&format!(
                    "deterministic capture: could not restore the prior viewport override: {e}"
                ));
            }
        }
        None => {
            if let Err(e) = browser
                .command("Emulation.clearDeviceMetricsOverride", json!({}), true)
                .await
            {
                warn(&format!(
                    "deterministic capture: could not clear the metrics override: {e}"
                ));
            }
        }
    }
}

/// Capture the active page. `deterministic: true` pins the conditions above
/// and restores every override before returning (best effort: a restore
/// failure is never reported as a capture failure - it is LOGGED, because a
/// silent swallow would hide a page left with a stale override, and the next
/// read-back shows the real state rather than a claim).
///
/// The whole capture runs under the connection's capture lock: the
/// deterministic path mutates page-visible state (metrics override, emulated
/// media, an injected stylesheet) and restoration must not interleave with
/// another capture's mutations.
pub async fn capture(browser: &Browser, opts: &Options) -> Result<Capture, String> {
    let _serial = browser
        .capture_lock()
        .await
        .map_err(|e| format!("screenshot: {e}"))?;
    if !opts.deterministic {
        let r = browser
            .command("Page.captureScreenshot", shot_params(opts), true)
            .await?;
        return decode(&r, false, opts.format);
    }

    let prior = browser.viewport_override().await;
    let (vw, vh, vdpr) = browser
        .viewport_readback()
        .await
        .unwrap_or((1280, 720, 1.0));
    let base_reduce = browser
        .eval("matchMedia('(prefers-reduced-motion: reduce)').matches")
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let width = opts.width.unwrap_or(vw).max(1);
    let height = opts.height.unwrap_or(vh).max(1);
    browser
        .command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": width, "height": height, "deviceScaleFactor": 1, "mobile": false}),
            true,
        )
        .await?;
    let mut media_emulated = media_emu_known().unwrap_or(true);
    if media_emulated {
        match browser
            .command(
                "Emulation.setEmulatedMedia",
                json!({"features": [{"name": "prefers-reduced-motion", "value": "reduce"}]}),
                true,
            )
            .await
        {
            Ok(_) => media_emu_remember(true),
            Err(e) if method_missing(&e) => {
                media_emu_remember(false);
                media_emulated = false;
                warn(
                    "deterministic capture: this engine has no Emulation.setEmulatedMedia; \
                     prefers-reduced-motion is not emulated - the freeze stylesheet still \
                     stops CSS animation",
                );
            }
            Err(e) => warn(&format!(
                "deterministic capture: setEmulatedMedia failed: {e} (preference left as-is)"
            )),
        }
    }
    let freeze = apply_freeze(browser).await;
    maybe_test_delay().await;
    let mut settle_timeouts: Vec<String> = Vec::new();
    let mut fonts_pending: i64 = 0;
    match tokio::time::timeout(SETTLE_CAP, browser.eval(SETTLE_JS)).await {
        Ok(Ok(v)) => {
            for t in v
                .get("timeouts")
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default()
            {
                if let Some(s) = t.as_str() {
                    settle_timeouts.push(s.to_string());
                }
            }
            fonts_pending = v.get("fontsPending").and_then(|n| n.as_i64()).unwrap_or(0);
        }
        Ok(Err(e)) => {
            warn(&format!(
                "deterministic capture: the settle script could not run: {e} (recording settleCap)"
            ));
            settle_timeouts.push("settleCap".to_string());
        }
        Err(_) => {
            warn("deterministic capture: settle exceeded its budget (proceeding with the shot)");
            settle_timeouts.push("settleCap".to_string());
        }
    }
    let live = live_metrics(browser).await;
    let shot = browser
        .command("Page.captureScreenshot", shot_params(opts), true)
        .await;

    restore_freeze(browser).await;
    if let Err(e) = browser.eval(&style_restore_js()).await {
        warn(&format!(
            "deterministic capture: could not remove the freeze stylesheet: {e}"
        ));
    }
    if media_emulated {
        if take_dropped_clear() {
            warn("test hook: dropped the media clear (GLADIUS_TEST_DROP_MEDIA_CLEAR)");
        } else if let Err(e) = browser
            .command("Emulation.setEmulatedMedia", clear_media_params(), true)
            .await
        {
            warn(&format!(
                "deterministic capture: could not clear emulated media: {e}"
            ));
        }
    }
    restore_metrics(browser, prior.as_ref()).await;

    let expectation = (vw, vh, vdpr, base_reduce);
    if !confirm_restored(browser, expectation).await {
        warn(
            "deterministic capture: page did not confirm the restored state within 2s; \
             re-applying the restore once",
        );
        if let Err(e) = browser.eval(&style_restore_js()).await {
            warn(&format!(
                "deterministic capture: retry could not remove the freeze stylesheet: {e}"
            ));
        }
        if media_emulated {
            if let Err(e) = browser
                .command("Emulation.setEmulatedMedia", clear_media_params(), true)
                .await
            {
                warn(&format!(
                    "deterministic capture: retry could not clear emulated media: {e}"
                ));
            }
        }
        restore_metrics(browser, prior.as_ref()).await;
        if !confirm_restored(browser, expectation).await {
            warn(
                "deterministic capture: page still reports the emulated state after a retry \
                 (the next read-back shows the truth)",
            );
        }
    }

    let mut cap = decode(&shot?, true, opts.format)?;
    cap.reduced_motion_emulated = media_emulated;
    cap.settled = settle_timeouts.is_empty();
    cap.settle_timeouts = settle_timeouts;
    cap.freeze = freeze;
    cap.fonts_pending = fonts_pending;
    let arr = live.as_array().cloned().unwrap_or_default();
    let num = |i: usize| arr.get(i).and_then(|v| v.as_f64());
    cap.width = num(0).map(|v| v as i64).unwrap_or(width);
    cap.height = num(1).map(|v| v as i64).unwrap_or(height);
    cap.dsf = num(2).unwrap_or(1.0);
    cap.url = arr
        .get(3)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(cap)
}

#[cfg(test)]
mod tests {
    use super::png_size;

    /// IHDR width/height are big-endian at bytes 16..24; the viewport fields
    /// alone cannot tell a full-page capture's real pixel size.
    #[test]
    fn png_size_reads_ihdr_big_endian() {
        let mut png = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, // signature
            0, 0, 0, 13, // IHDR length
            b'I', b'H', b'D', b'R', // chunk type
        ];
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((640, 480)));
        assert_eq!(png_size(b"not a png"), None);
    }
}
