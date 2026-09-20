# WebView2 Composition Hosting Migration — Design

## Motivation

`manifold-engine-webview2` currently hosts WebView2 windowed (`ICoreWebView2Controller`
against a real, off-screen `HWND`) and drives mouse input through a hand-rolled
JS/CDP hybrid in `input.rs`: `document.caretRangeFromPoint` + `Selection.collapse`/
`.extend` for drag-selection, `el.click()`/`.focus()` for clicks, `window.scrollBy`
(walking up to the first scrollable ancestor) for wheel/scroll. That approach is
real, working, and verified live — but it's a pile of one-off recipes, several of
which (touch, pinch, IME composition) would need experimental CDP methods this
project doesn't use yet, and none of which can reach a page's *own* JS-driven
custom drag/scroll widgets, since only real trusted OS-level events do that.

A round of investigation this session found and live-verified, piece by piece,
that Composition hosting (`ICoreWebView2CompositionController`) closes all of
that:

- Native `SendMouseInput` correctly drives click, drag-text-selection (the
  legacy API specifically — `SendPointerInput` does not extend a selection,
  unexplained, but is irrelevant since `SendMouseInput` works), and wheel/scroll,
  as real trusted OS-level input.
- `GraphicsCaptureItem::CreateFromVisual()` — a direct WinRT static method,
  distinct from the desktop-interop `CreateForVisual` this project's capture.rs
  already correctly found unavailable — captures a Composition-hosted WebView2
  visual's real rendered pixels correctly. This was the one blocker that had
  ruled out Composition hosting historically; it turned out to be solvable, just
  via a different API than originally tried.
- Two open, unresolved WebView2Feedback issues
  ([#5712](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5712),
  [#5713](https://github.com/MicrosoftEdge/WebView2Feedback/issues/5713)) describe
  Composition Mode input going unresponsive after repeated real-mouse
  drag-outside-the-window cycles. Stress-tested 190 purely-synthetic
  press-inside/drag-past-edge/release-outside cycles via `SendMouseInput` with
  zero degradation — that trigger condition (a real physical mouse leaving a
  real OS window) does not appear to apply to this project's fully-synthetic
  input model.
- `ICoreWebView2CompositionController` has **no native keyboard input API at
  all** — confirmed both by its absence from this project's pinned bindings and
  by a Microsoft team member's own explanation (WebView2Feedback#1054): real
  keyboard input is delivered by the OS as genuine `WM_KEYDOWN` messages to
  whichever window holds focus, with no injection API needed *for real hardware
  input*. Tested whether a `PostMessage`'d synthetic `WM_KEYDOWN` to the
  top-level Composition host window reaches the page anyway (with and without
  real Win32 `SetFocus`) — it does not, in either case. Keyboard input has no
  native path under either hosting mode.

## Scope decisions (confirmed during brainstorming)

- **Full replacement, not a fallback.** Once Composition hosting is verified
  equivalent, the windowed-hosting + JS/CDP-mouse code is deleted, not kept
  behind a flag. Matches this project's existing YAGNI-leaning style; a
  permanently-flagged second code path for something already fully verified
  is dead weight to maintain, not safety margin.
- **Mouse/pointer and capture migrate; keyboard does not.** `KeyDown`/`KeyUp`
  (CDP `Input.dispatchKeyEvent`) and `Char` (JS `execCommand('insertText', ...)`)
  stay exactly as they are today, on *either* hosting mode — there is no native
  alternative, confirmed above. This means CDP remains a permanent dependency
  for keyboard regardless of this migration; the migration's value is the mouse
  side (drag-selection, scroll, and unlocking touch/pinch as a new capability)
  and removing the experimental-CDP-method exposure that lived there, not
  eliminating CDP outright.
- **No minimum-WebView2-Runtime fallback path.** `CreateCoreWebView2CompositionController`
  requires `ICoreWebView2Environment3` (Runtime 89+, 2021) — old enough that every
  other capability this project already treats as a hard requirement (e.g. plain
  `CreateCoreWebView2Controller` itself) assumes a comparably current runtime.
  Failure to create the composition controller is a hard `.expect()`, matching
  `create_environment().expect(...)` and `create_controller(...).expect(...)`'s
  existing failure posture in `main.rs` — not a soft-degrade like `find::register(...).ok()`.

## Architecture

Four touch points, all inside `manifold-engine-webview2`:

### 1. Hosting (`main.rs` + `window.rs`)

`create_controller`'s single-step `CreateCoreWebView2Controller` is replaced by:

1. `Compositor::new()` + `ICompositorDesktopInterop::CreateDesktopWindowTarget(hwnd, false)`
   — same hidden, off-screen `HWND` `window.rs` already creates; Composition
   hosting still needs a real top-level window to attach its visual tree to,
   it just doesn't render *into* that window's own client area directly.
2. `compositor.CreateContainerVisual()`, sized via `SetSize` to match the
   tab's content size (same value `content_size_px()` already computes on the
   `manifold` side and sends via `HostToEngine::Resize`), set as the target's
   root.
3. `environment.cast::<ICoreWebView2Environment3>()?.CreateCoreWebView2CompositionController(hwnd, ...)`
   (async, same `wait_for_async_operation` pattern already used for environment/
   controller creation elsewhere in this file).
4. `comp_controller.SetRootVisualTarget(&root_visual)`.
5. The returned object is cast to `ICoreWebView2Controller` for everything that
   already works unchanged: `SetBounds`, `SetIsVisible`, `SetZoomFactor`,
   `MoveFocus`, `.CoreWebView2()`. Confirmed live this session: the object
   `CreateCoreWebView2CompositionController` returns implements both interfaces.

`resize_controller` gains one line: `root_visual.SetSize(...)` alongside the
existing `SetBounds` call, so the visual's own size tracks the controller's.

**`MoveFocus` must now be called on every relevant input dispatch path** (mouse
down, at minimum) — JS-driven input never needed real WebView2 focus tracking;
native `SendMouseInput` does, per Microsoft's own guidance already confirmed
this session.

### 2. Capture (`capture.rs`)

`Capture::new(hwnd)` → `Capture::new(&root_visual)`, swapping
`IGraphicsCaptureItemInterop::CreateForWindow(hwnd)` for
`GraphicsCaptureItem::CreateFromVisual(&root_visual)`. Everything downstream
(frame pool, `TryGetNextFrame`, D3D11 texture readback, tightly-packed BGRA8
copy) is unchanged — same `GraphicsCaptureItem`/`Direct3D11CaptureFramePool`
types either way.

**`client_rect_in_window`'s entire cropping step is deleted.** That function
exists solely to crop out a real window's non-client chrome (title bar,
borders) after `CreateForWindow` captures the *whole* window — `CreateFromVisual`
captures only the visual itself, which is exactly the content area with no
chrome to crop. This is a real simplification, not just a swap: a documented
source of fragility (recomputing the crop rect every frame, the `WS_POPUP`/
`WS_EX_TOOLWINDOW` rejection saga in `window.rs`'s own doc comments) goes away
entirely.

`Capture::resize` recreates the frame pool against the *visual's* new size
(`root_visual.SetSize(...)`'s value) instead of the window's.

### 3. Mouse/pointer input (`input.rs`)

`send_input`'s signature gains a `&ICoreWebView2CompositionController`
parameter alongside the existing `&ICoreWebView2` (still needed for keyboard).
Rewritten per event:

- `MouseDown`/`MouseMove`/`MouseUp` (drag-selection): `SendMouseInput` with
  `COREWEBVIEW2_MOUSE_EVENT_KIND_{MOVE,LEFT_BUTTON_DOWN,LEFT_BUTTON_UP}`,
  replacing the `caretRangeFromPoint`/`Selection` JS entirely. The existing
  `DragState` (active/moved) is still needed for the same reason as today:
  distinguishing a real drag from a stationary click, so a completed drag
  doesn't *also* synthesize a click. The `.focus()`-before-click fix stays
  conceptually (native mousedown+click already focuses form controls
  correctly per Chromium's own real input handling — this may make the
  explicit `.focus()` workaround unnecessary; verify during implementation
  rather than assume, same discipline as everything else this session).
- `Scroll`: `SendMouseInput` with `COREWEBVIEW2_MOUSE_EVENT_KIND_WHEEL`,
  `mousedata` carrying the delta — replaces the JS scrollable-ancestor walk.
  Real wheel events resolve their own target the same way a real browser
  does, so the manual DOM walk this session just added to `input.rs` becomes
  unnecessary.
- `KeyDown`/`KeyUp`/`Char`: **unchanged**, still call `CallDevToolsProtocolMethod`
  / `ExecuteScript` on `&ICoreWebView2` exactly as today.
- New capability, not previously possible: touch/pinch via `SendPointerInput`
  with `PT_TOUCH`-kind `ICoreWebView2PointerInfo` — not covered by anything
  `ipc_protocol::InputEvent` currently models, so this needs its own new
  variant if/when it's actually wired up. Out of scope for the initial
  migration; noted here so it isn't lost.

### 4. Everything else — unchanged

`nav_state.rs`, `find.rs`, `download.rs`, `popup.rs`, `scheme.rs`, `audio.rs`
all operate on `ICoreWebView2` (navigation, events, `ExecuteScript`) or
`ICoreWebView2Controller` (zoom), neither of which changes shape under
Composition hosting. No changes expected there; confirm during implementation
rather than assume.

## Migration order

Two independently-verifiable phases, not one big-bang change — this is a
correctness discipline (isolate what could break), not a time-cost one:

1. **Hosting + capture only.** Swap `create_controller`/`Capture::new` to
   Composition hosting + `CreateFromVisual`, leave `input.rs` entirely
   untouched (its `ExecuteScript`/CDP calls work identically against the same
   `ICoreWebView2` regardless of hosting mode). Verifies rendering/capture in
   isolation against the real multi-tab app — split view, resize, multiple
   simultaneous tabs — before touching input at all.
2. **Mouse input.** Swap `input.rs`'s `MouseDown`/`MouseMove`/`MouseUp`/`Scroll`
   handling to native `SendMouseInput`, once phase 1 is confirmed solid.

## Testing plan

- Re-run this session's existing verification surface (`drag_select_verify.rs`'s
  checks: drag-select, plain click, input-field focus, nested scroll) against
  the migrated real `input.rs`, not just a spike — confirms no regression
  versus what Option A already achieved.
- Multi-tab: two or more simultaneous Composition-hosted tabs (each engine-host
  is its own process with its own `DispatcherQueueController`, so no expected
  cross-tab interference, but confirm rather than assume).
- Split view (two tabs rendering side by side) still composes correctly.
- Resize behavior, reusing this session's DPI-fix verification approach
  (physical-pixel consistency across `content_size_px`/`SetBounds`/capture).
- Cross-origin iframe content: does native input reach it correctly via
  Chromium's own hit-testing (the theorized advantage over CDP, which would
  need explicit `Target.attachToTarget` session bookkeeping) — untested so far,
  worth confirming since it's one of the stated advantages of this approach.

## Open items deliberately deferred

- Touch/pinch (`SendPointerInput` + `PT_TOUCH`) — new capability, no existing
  `InputEvent` variant models it; separate follow-up once the core migration
  lands.
- ~~Whether native mousedown+click removes the need for `input.rs`'s explicit
  `.focus()`-before-`.click()` fix — verify during implementation.~~ **Settled
  (Task 5 live verification):** yes, it does. A native `SendMouseInput`
  `LEFT_BUTTON_DOWN`+`UP` on a plain `<input>` focuses it with no extra call
  needed — confirmed live by clicking into a plain `<input>` on a real test
  page and observing its `:focus` CSS outline appear, then sending `Char`
  events that typed real text into the field (screenshot evidence:
  `hello123` appeared in the input with a visible focus ring after nothing
  but a native click). Task 4 already omitted the old JS approach's explicit
  `.focus()` call when it rewrote mouse handling to `SendMouseInput`; this
  confirms that omission was correct and no code change is needed.
