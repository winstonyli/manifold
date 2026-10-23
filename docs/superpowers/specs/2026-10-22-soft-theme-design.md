---
status: draft
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

## Unknowns to settle in the first read of the build

1. **Tab render pass clear colour.** `gpu.rs:910` and `:963` clear to `Color::BLACK`. What shows through at card
   edges or while a tab loads, and whether a light canvas needs the clear to follow the theme.
2. **Web-engine surround.** Whether any engine host paints a default background that shows a seam in light.
3. **How pages learn the theme** (above).
4. **One accent, two jobs.** The accent is a fill (selected, primary button, with `BG`-toned ink on it) and also a text
   or ring colour. In light the blue that passes as a fill may not pass as thin text on white; the build may need
   `Palette.accent_text` beside `selection.bg_fill`, decided by the contrast table.
5. **Screen-constant drawing.** Card code paints in screen space with `inv_zoom`; a shadow's offset and blur must scale
   the same way the glow's `grow` does.

## Out of scope

- The 2+3 hybrid (space tint, per-kind header bands): parked, `PARKED.md`.
- A custom-drawn title bar (the existing note defers it).
- User-editable themes or more than two palettes.

## Open decisions for review

- **D1. Font.** Keep Familjen Grotesk as the one family (already bundled; drop Literata and Newsreader, 390 KB), or
  switch to Inter as in the mockups (adds a bundled font). Recommendation: Familjen Grotesk, since it is a close
  grotesque, already tuned (`y_offset_factor`), and adds nothing. Needs the user's call.
- **D2. Default theme.** System (recommended) or Dark.
