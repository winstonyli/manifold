# Hiding the view host's window from the taskbar (recorded 2026-11-04)

Code: `hide_from_taskbar` in `crates/manifold-view-webview2/src/window.rs`, which uses `ITaskbarList::DeleteTab`.

Three window-style approaches broke the old `Windows.Graphics.Capture` `CreateForWindow` capture. That capture no
longer exists (capture reads the Composition visual and ignores the window), so none is known to be a problem now, but
none has been tested against the current capture. Re-verify before simplifying `hide_from_taskbar` away (for example
with `WS_EX_TOOLWINDOW` at creation time):

- `WS_EX_TOOLWINDOW` or `WS_POPUP` at creation: `CreateForWindow` rejected both with `E_INVALIDARG` (why
  `create_hidden_window` uses plain `WS_OVERLAPPEDWINDOW`).
- An owner window, even an invisible purpose-built one: the same `E_INVALIDARG`.
- `WS_EX_TOOLWINDOW` added with `SetWindowLongPtrW` after capture started: the running capture came back blank white
  with a garbage-colored seam at the old caption boundary (with `SWP_FRAMECHANGED`), blank with no seam (bare style
  bit), or solid black with no frames (an explicit hide/show cycle).

`DeleteTab` touches no style bits, visibility or non-client area, so it shares none of these failures.
