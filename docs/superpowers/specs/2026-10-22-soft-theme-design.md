---
status: done
---

# Soft theme: Heptabase-style light and dark — Design

## Question

The user is dissatisfied with the current look (warm near-black, gold accent, serif body, 2 px corners, glow depth)
and chose, from the mockups (`docs/superpowers/mockups/look_options.html`, `look_finalists.html`, README 2026-10-22),
a Heptabase-style look in **both dark and light**. Losing the current identity (gold, serif, warm black) is accepted.
The aim is one theme mechanism that serves both, with the look itself a set of values.

Decided with the user (2026-10-22): (1) colours come from egui `Visuals` first; (2) light and dark ship together;
(3) the 2+3 hybrid (space tint, per-kind header bands) is later, parked; (4) the identity loss is fine.

## The look (from the finalists mockup)

- **Soft elevation.** Cards are lighter than the canvas in dark (card `#3A3731` on canvas `#292723`), white on a tinted
  paper ground in light (`#FFF` on `#EBE8E1`). Depth is an offset ambient shadow, not a tinted glow.
- **Shape.** Corners about 12 px for cards, 8 px for controls, 10 px for chips (now 2, 5 and 6 px). One shape set for
  both themes.
- **One pastel accent**, blue (dark `#8FB0FF`, light `#3B6DF0`), replacing gold. Rust danger stays a warm red.
- **Calm canvas**: a faint dot grid and muted edges.

The shared colours (the 9 in `manifold_pages::palette`) keep one source: `palette.rs` gains `DARK` and `LIGHT` tables
and both `theme::Palette` and the page CSS are built from them, so the app and the pages cannot drift (the README's
2026-10-19 rule). `theme::Palette` adds only the app-only colours on top.

Hex values are the mockup's, to be tuned in the build against contrast tests, not fixed here.

## Design

### One mechanism: egui's two style slots

egui 0.36 already keeps a dark and a light `Style` (`Context::style_mut_of(Theme, ..)`, `set_theme(ThemePreference)`,
`Visuals::dark_mode`). `apply_style` fills **both** slots (today it resets both to `Visuals::dark()` and styles them identically via
`all_styles_mut`), the dark slot from `Visuals::dark()` plus the dark table, the light slot from `Visuals::light()`
plus the light table (so every field the table does not name starts coherent for its theme). Switching is `ctx.set_theme(Dark | Light | System)`.
Nothing else needs a "current theme" variable: code asks the `Visuals` it already holds.

### Where each colour comes from

| Today | Becomes | Via |
|---|---|---|
| `TEXT_PRIMARY` (21 uses, 10 files) | `Palette.text` | `Palette` (see below) |
| `TEXT_WEAK` (19, 7) | `Palette.text_weak` | `Palette` |
| `BG` (10, 6) as a fill | `visuals.panel_fill` / `extreme_bg_color` | egui |
| `TOOLBAR_FILL`, `SURFACE_RAISED`, `SURFACE_HOVER`, `OUTLINE` | the widget-state fills and strokes `apply_style` already sets | egui |
| `ACCENT` (55, 20) | `visuals.selection.bg_fill` (`apply_style` sets `hyperlink_color` and the active-widget fill to the same value) | egui |
| `DANGER_COLOR` (31, 14) | `visuals.error_fg_color` (set from the table in `apply_style`) | egui |
| `DANGER_FILL`, `SUCCESS_COLOR`, scrims, `NOTE_CARD_COLOR`, dot and edge colours, shadow strength | `theme::Palette` fields | `theme::palette(&Visuals) -> &'static Palette` |
| `TAG_COLOR_PALETTE` (15, 6) | unchanged (Okabe-Ito), checked on both surfaces | global |

**Text is the exception to visuals-first**, and the reason is checked: `Visuals::text_color()` is
`widgets.noninteractive.fg_stroke.color`, which `apply_style` sets to the *weak* tan (so plain labels are tan), and
`weak_text_color()` is derived from it by `weak_text_alpha` unless set. Mapping `TEXT_PRIMARY` to `text_color()`
would change colours and setting `weak_text_color` would change every existing `weak_text_color()` caller (card idle
borders, popup strokes). Phase 1 keeps both egui slots as they are and moves the two named text colours into
`Palette`. Everything with an exact egui slot (fills, strokes, selection, hyperlink, error) stays in `Visuals`.

`Palette` holds what `Visuals` has no exact slot for. `palette(visuals)` returns the `DARK` or `LIGHT` static by
`visuals.dark_mode`, the way `json_level_ui::palette(dark_mode)` and `card::card_style(kind, &Visuals)` already do.
Sites that hold a `Ui`, `Painter` or `Context` reach `Visuals` through it (`painter.ctx().style().visuals` where no
`Ui` exists).

Ink on a tag colour (`identity::ink_on`, which uses `BG`) becomes a fixed near-black: the ink sits on the tag colour,
not on the theme.

### Shape and depth

- `RADIUS`, `RADIUS_ITEM`, `RADIUS_CHIP` keep their names with new values (one set, both themes). The README records
  that some card-drawing literals at the same numbers are circle radii or margins: every `rect_filled`/`rect_stroke`
  call site is checked individually, not replaced wholesale.
- `card_glow_shapes` is the one site that paints card depth. It becomes a soft shadow (offset, low alpha, theme-aware
  strength from `Palette`), **with the same shape count (two rects)**: the parked "Fewer shapes in a `Detail::Full`
  card" and the glow measurements (PARKED.md, 2026-10-22) say card tessellation is already 14% of main-thread time
  at 400 cards, so the new depth must not add shapes. A ring (selected, tagged) stays a ring; the glow remains only
  where it signals state (hover, focus).

### Theme choice, title bar, pages

- **Setting**: `Settings.theme: Dark | Light | System`, default **System**, in the Settings page. `System` follows
  egui's `system_theme()` (winit's theme events).
- **Title bar**: `apply_dark_titlebar(window)` becomes `apply_titlebar(window, dark: bool)`. The frame loop compares
  `ctx.theme()` with the last applied value and re-applies on change (this covers both the setting and a system
  switch under `System`).
- **Pages**: `manifold_pages::palette` gains a light table; `palette_vars` emits both as `:root` and
  `:root[data-theme=light]` rules (or `prefers-color-scheme`), and the host tells a page which applies. How a
  `manifold://` page learns the app theme (a query value at navigation, or `prefers-color-scheme` set on the engine)
  is open: see Unknowns.

### Fonts

The Heptabase mockups use Inter everywhere. Dropping the serif is a separate decision (see Open decisions); this
design assumes the font set is fixed before the build, since the look values are tuned against it.

## Phasing (each lands green)

1. **Mechanism, no visual change.** Add `Palette` (dark values = today), fill both style slots from the table, convert
   the ~145 constant uses to `Visuals`/`Palette`, delete the constants. `gate chrome-shots` before and after must
   match pixel for pixel in dark.
2. **Heptabase dark values** and shape/shadow (`card_glow_shapes`, radii, canvas and grid colours).
3. **Light table, theme setting, title bar, pages**; contrast tests run per theme; `chrome-shots` for both.

## Testing

- Tests build a context with `apply_fonts` and sometimes `apply_style`; about 30 sites do only `apply_fonts`, so their
  `Visuals` is egui's stock dark and `visuals.selection.bg_fill` is not the accent. Phase 1 adds one
  `theme::testutil::ctx()` (fonts, style, a chosen theme) and moves those sites to it; tests that assert on colours
  take the theme explicitly, so they can run for both.
- The contrast table test (`text_colors_keep_aa_contrast_on_their_surfaces`) runs for both themes and pins accent,
  danger and text on their surfaces (AA 4.5:1 for text, 3:1 for hover outlines).
- A test that every `Palette` field differs where it must (a card fill is not the canvas fill in either theme).
- Tag colours on a white card: the swatch is a ring or dot, not text, so contrast is checked as a non-text 3:1 only.
- Phase 1's pixel-identical `chrome-shots` is the regression check for the refactor.

## Unknowns, checked 2026-10-22 (read-only)

1. **Clear colour.** `gpu.rs:910` (the tab render pass) and `:963` (the egui pass) clear the swapchain to `Color::BLACK`
   when first in the frame; the thumbnail downsample passes clear to transparent and are unaffected. egui paints
   panels over the whole window, so black shows only at an unpainted seam (for example the tab content viewport's
   edge). In light that seam would be a black hairline. Fix: both clears take the theme's canvas colour from
   `Palette` (one value, passed to `GpuState::render`).
2. **Engine backgrounds.** Tab content is captured into textures and drawn by wgpu, so no engine surface sits in the
   window; the only host background in the code is the WebView2 window class's null brush (`window.rs:75`). Nothing
   to change.
3. **Pages.** `manifold_pages::serve(uri)` takes no theme and builds each page once (`OnceLock`). Chosen: a process
   global `manifold_pages::set_dark(bool)` written by the app on theme change, and `serve` returns the cached dark or
   light page. It works the same in every engine, where `prefers-color-scheme` would need per-engine plumbing. An
   already open `manifold://` page keeps its old colours until reloaded; the app reloads open `manifold://` tabs on a
   theme change.
4. **One accent, two jobs.** The accent is a fill (selected, primary button, with `BG`-toned ink on it) and also a text
   or ring colour. In light the blue that passes as a fill may not pass as thin text on white; the build may need
   `Palette.accent_text` beside `selection.bg_fill`, decided by the contrast table.
5. **Screen-constant drawing.** Card code paints in screen space with `inv_zoom`; a shadow's offset and blur must scale
   the same way the glow's `grow` does.

## Out of scope

- The 2+3 hybrid (space tint, per-kind header bands): parked, `PARKED.md`.
- A custom-drawn title bar (the existing note defers it).
- User-editable themes or more than two palettes.

## Decisions

- **Font** (decided 2026-10-22): keep Familjen Grotesk as the one family; Literata and Newsreader go with the serif
  decision, nothing new is bundled.
- **Theme setting** (decided 2026-10-22): `System | Light | Dark`, default System.
- **Dark values as built** (2026-10-22): the mockup table passed the contrast test except three roles, nudged to
  `text_weak [186,181,170]`, `danger [240,132,112]`, `outline [152,147,136]`. The card fill is `faint_bg_color`
  (the raised surface), not `extreme_bg_color`, so a card is lighter than the canvas. The "refused" drag color is the
  yellow tag color, because sky blue collided with the blue accent.
- **Light values as built** (2026-10-22): light table in `manifold_pages::palette::LIGHT`; every role passes the
  per-theme contrast test. Unknown 4 resolved without `accent_text`: one deeper light accent `[40,90,215]` serves fill
  and text. Tag colors: stored as the canonical `TAG_COLOR_PALETTE` entry in both themes; `theme::tag_shown(stored,
  dark)` maps to `LIGHT_TAG_COLOR_PALETTE` (orange-yellow, yellow, sky blue deepened, same hues) at the card paint
  sites (border, swatch, node menu); equality logic (`next_tag_color`, jar free-color search) and the jar chip (on a
  dark scrim) stay canonical. The yellow "refused" color is unchanged in light.
- **Pages reach the engines** (2026-10-22, final review C1): the pages are served inside the engine host processes, so
  the app's `set_dark` alone never reached them. The theme travels as `HostToView::SetTheme { dark }` (protocol 5)
  and at spawn as `VIEW_DARK`; every host calls `manifold_pages::set_dark`. The app sends `SetTheme` to every tab
  (a later navigation uses the flag), then `Reload` to the open `manifold://` ones. The WebKit host (remote) takes the
  message only: its first page can load in the old palette.
- **Title bar** (final review I2): `window.set_theme(window_theme(choice))`, so winit owns the title bar; the DWM call
  is gone. egui's own `SetTheme` viewport command is never processed here (the app does not call
  `process_viewport_commands`), so `App::choose_theme` sets the window theme and refreshes egui's `system_theme`.
