---
status: draft
---

# Soft theme (Heptabase-style light and dark) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the warm-dark/gold look with a Heptabase-style soft theme in dark and light, switchable System | Light | Dark.

**Architecture:** `apply_style` fills egui's two style slots (dark from `Visuals::dark()` + the dark table, light from `Visuals::light()` + the light table). Colors with an exact egui slot are read from `Visuals`; the rest live in `theme::Palette`, reached by `theme::palette(&Visuals)`. The 9 shared colors have one source, `manifold_pages::palette::{DARK, LIGHT}`, feeding both the app and the page CSS. Phase 1 changes no pixels; phase 2 swaps dark values, shape and depth; phase 3 adds light, the setting, title bar and pages.

**Tech Stack:** Rust, egui 0.36.2 (`Context::style_mut_of`, `set_theme`, `Visuals::dark_mode`), wgpu, winit, the `gate` crate for pixel checks.

**Spec:** `docs/superpowers/specs/2026-10-22-soft-theme-design.md`

## Global Constraints

- Work on `main`; `cargo fmt --all` before each commit; `cargo clippy -j 8 -p manifold -p manifold-view-protocol -p manifold-pages -p manifold-view-webview2 -p manifold-view-pdfium -p gate --all-targets -- -D warnings` must pass; never `--no-verify`. Use `-j 8` on every cargo call.
- No test reads `%APPDATA%\manifold` (`profile::dir()` is `None` under `cfg(test)`); tests needing files use `store::testutil::temp_dir`.
- No migration or compatibility shims: when a mechanism replaces another, delete the old one and fix callers once. Old `settings.json` without `theme` takes the serde default (that is the format's existing field-level default, not a shim).
- Card depth keeps **two shapes** per card (`card_glow_shapes -> [Shape; 2]`): tessellation is 14% of main-thread time at 400 cards.
- One font family (Familjen Grotesk). Nothing new is bundled.
- Python, if any, runs as `python -P f.py < /dev/null`; no stdin python.
- Agents never tag, push or publish. The pre-merge gate (`target/debug/gate.exe all|light|ui`) runs only on the user's request; `gate chrome-shots` is used here for pixel parity.
- Shape values (spec): corners 12 / 8 / 10 px (cards / controls / chips). Accent dark `#8FB0FF`, light `#3B6DF0`. Dark card `#3A3731` on canvas `#292723`; light card `#FFFFFF` on `#EBE8E1`.

## Review Focus

1. A system theme switch while running under `System`: title bar, pages and canvas clear must follow without a restart (T9, T10).
2. An open `manifold://` tab when the theme changes: it must reload with the new colors, not keep stale ones (T10).
3. Light-mode seams: any unpainted gap showing `Color::BLACK` as a hairline (T7).
4. Text on a tag color in light (white card): ink and ring contrast (T8 test).
5. Any widget whose color was read from a constant at `const` time (`WAVEFORM_COLOR`, `CardStyle` consts) staying dark-only after the constants are deleted (T3-T5 grep check).

## File Structure

- `crates/manifold-pages/src/lib.rs`: `palette::Colors`, `DARK`, `LIGHT`; `palette_vars(&Colors)`; `set_dark`; per-theme page cache.
- `crates/manifold/src/theme.rs`: `Palette`, `palette()`, `accent()`, `danger()`, `style_for`, `apply_style`, `apply_titlebar`; constants deleted.
- `crates/manifold/src/theme/testutil.rs` (new, `cfg(test)`): `ctx(Theme)`.
- Use sites: see each task's list.

---

### Task 1: Palette tables in `manifold_pages`

**Files:**
- Modify: `crates/manifold-pages/src/lib.rs` (the `palette` module, `palette_vars`, `themed`, tests)

**Interfaces:**
- Produces: `manifold_pages::palette::Colors { bg, toolbar_fill, surface_raised, surface_hover, outline, text_primary, text_weak, accent, danger: [u8; 3] }`, `palette::DARK: Colors` (today's values), `palette::colors(dark: bool) -> &'static Colors`. The old flat constants are deleted.

- [ ] **Step 1: Write the failing test** in `lib.rs` tests:

```rust
#[test]
fn dark_table_keeps_todays_values_and_light_is_distinct() {
    assert_eq!(palette::DARK.bg, [23, 20, 15]);
    assert_eq!(palette::DARK.accent, [215, 162, 74]);
    assert_eq!(palette::colors(true), &palette::DARK);
}
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold-pages dark_table` — Expected: FAIL (no `Colors`).
- [ ] **Step 3: Implement.** Replace the module body with:

```rust
pub mod palette {
    #[derive(Debug, PartialEq, Eq)]
    pub struct Colors {
        pub bg: [u8; 3],
        pub toolbar_fill: [u8; 3],
        pub surface_raised: [u8; 3],
        pub surface_hover: [u8; 3],
        /// A resting-but-visible border: a hovered control's outline.
        pub outline: [u8; 3],
        pub text_primary: [u8; 3],
        pub text_weak: [u8; 3],
        pub accent: [u8; 3],
        pub danger: [u8; 3],
    }
    pub const DARK: Colors = Colors {
        bg: [23, 20, 15],
        toolbar_fill: [29, 26, 20],
        surface_raised: [41, 36, 27],
        surface_hover: [58, 50, 37],
        outline: [140, 127, 100],
        text_primary: [233, 226, 211],
        text_weak: [178, 164, 130],
        accent: [215, 162, 74],
        danger: [224, 112, 84],
    };
    /// Phase 1 has one table; `LIGHT` arrives in phase 3 (T9) and `colors` then picks by `dark`.
    pub fn colors(_dark: bool) -> &'static Colors {
        &DARK
    }
}
```

Change `palette_vars()` to iterate `[("bg", c.bg), ("panel", c.toolbar_fill), ("raised", c.surface_raised), ("hover", c.surface_hover), ("outline", c.outline), ("text", c.text_primary), ("weak", c.text_weak), ("accent", c.accent), ("danger", c.danger)]` with `let c = palette::colors(true);`.
- [ ] **Step 4:** `cargo test -j 8 -p manifold-pages` — Expected: PASS (existing `pages_take_their_colors_from_the_palette` still sees `--bg:#17140F;`).
- [ ] **Step 5:** Fix `theme.rs`'s `color(manifold_pages::palette::X)` lines to `color(manifold_pages::palette::DARK.x)` so the workspace compiles (`cargo check -j 8 -p manifold`), then commit: `git commit -am "Palette tables: Colors, DARK"`.

### Task 2: `theme::Palette`, `palette()`, two-slot `apply_style`, test helper

**Files:**
- Modify: `crates/manifold/src/theme.rs` (constants block ~308-400, `apply_style` 408-482, tests 851+)
- Create: `crates/manifold/src/theme/testutil.rs`

**Interfaces:**
- Produces: `theme::Palette { text, text_weak, danger_fill, success, scrim_text, scrim_text_weak, note_card, canvas: Color32 }`; `theme::palette(&egui::Visuals) -> &'static Palette`; `theme::accent(&Visuals) -> Color32` (= `selection.bg_fill`); `theme::danger(&Visuals) -> Color32` (= `error_fg_color`); `fn fill_style(style: &mut egui::Style, dark: bool)`; `pub(crate) const INK_DARK: Color32 = from_rgb(23, 20, 15)` and `INK_LIGHT: Color32 = from_rgb(236, 233, 226)` (ink on a tag color: theme-independent); `theme::testutil::ctx(dark: bool) -> egui::Context` (fonts + `apply_style` + `set_theme`).
- `canvas` is the swapchain clear (T7); in phase 1 equals `BG`.

- [ ] **Step 1: Failing tests** (in `theme.rs` tests):

```rust
#[test]
fn palette_follows_visuals_dark_mode() {
    let ctx = testutil::ctx(true);
    let v = ctx.global_style().visuals.clone();
    assert_eq!(palette(&v).text, TEXT_PRIMARY);
    assert_eq!(accent(&v), ACCENT);
    assert_eq!(danger(&v), DANGER_COLOR);
}

#[test]
fn error_fg_and_selection_are_set_by_apply_style() {
    let ctx = testutil::ctx(true);
    let v = ctx.global_style().visuals.clone();
    assert_eq!(v.selection.bg_fill, ACCENT);
    assert_eq!(v.error_fg_color, DANGER_COLOR);
}
```

- [ ] **Step 2:** `cargo test -j 8 -p manifold palette_follows` — Expected: FAIL (no `testutil`, `palette`).
- [ ] **Step 3: Implement.** Add `static DARK_PALETTE: Palette` holding today's `TEXT_PRIMARY`, `TEXT_WEAK`, `DANGER_FILL`, `SUCCESS_COLOR`, `SCRIM_TEXT`, `SCRIM_TEXT_WEAK`, `NOTE_CARD_COLOR`, `canvas: color(DARK.bg)`. `palette(v)` returns `&DARK_PALETTE` (phase 3 adds the light static and matches on `v.dark_mode`). Split `apply_style` into `fill_style(style, dark)` (the existing body, parameterized by a `&Colors` from `manifold_pages::palette::colors(dark)` and starting from `Visuals::dark()` or `Visuals::light()`), and
```rust
pub(crate) fn apply_style(ctx: &egui::Context) {
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        ctx.style_mut_of(theme, |style| fill_style(style, dark));
    }
}
```
`fill_style` also sets `visuals.error_fg_color` and `visuals.warn_fg_color` from the table's `danger`. Until T9 the app has no theme choice, so `apply_style` also calls `ctx.set_theme(egui::Theme::Dark)` (a light OS must not select the unfinished light slot; T9 replaces this line with the setting). Keep the existing constants in this task (deleted in T3-T6). `testutil.rs`:
```rust
pub(crate) fn ctx(dark: bool) -> egui::Context {
    let ctx = egui::Context::default();
    super::apply_fonts(&ctx);
    super::apply_style(&ctx);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx
}
```
Declare `#[cfg(test)] pub(crate) mod testutil;` in `theme.rs`.
- [ ] **Step 4:** `cargo test -j 8 -p manifold theme::` — Expected: PASS; the existing contrast tests still pass.
- [ ] **Step 5:** Commit `Theme: Palette, two-slot apply_style, test helper`.

### Task 3: Convert card code (`card.rs`, `card/*`)

**Files:** `card.rs`, `card/download_body.rs`, `card/image_body.rs`, `card/jar_chip.rs`, `card/note_body.rs`, `card/pdf_body.rs`, `card/tab_body.rs`, `card/tests.rs` (and any other `card/*` hit by the grep below).

**Interfaces:** Consumes T2's `palette()`, `accent()`, `danger()`.

Rule for every site: `ACCENT` -> `crate::theme::accent(ui.visuals())` (or `painter.ctx().global_style().visuals` clone held once per function where there is no `Ui`; bind `let v = ui.visuals();` once per function, not per use); `DANGER_COLOR` -> `crate::theme::danger(v)`; `TEXT_PRIMARY/TEXT_WEAK/BG-as-ink/DANGER_FILL/SUCCESS_COLOR/SCRIM_TEXT*/NOTE_CARD_COLOR` -> `crate::theme::palette(v).<field>`; `BG` used as a fill (`card.rs:1049`, the number cap) -> `v.panel_fill`. Sites known from the survey: `card.rs` 1047-1051, 1081, 1100, 1109, 1216, 276, 335, 337, 550, 609, 639, 691, 704; `download_body.rs` 54, 85; `image_body.rs` 28, 38; `jar_chip.rs` 181; `note_body.rs` 40; `pdf_body.rs` 88; `tab_body.rs` 83, 106, 110, 113, 180, 181, 370.

Two structural cases:
- `card.rs:114` `const WAVEFORM_COLOR = SUCCESS_COLOR` becomes `fn waveform_color(v: &Visuals) -> Color32 { palette(v).success }`; update its callers.
- `card_style(kind, &Visuals)` (`card.rs:261`) already takes `&Visuals`: `TYPE_GROUP => border_idle: accent(visuals)`; `card_border`'s `ACCENT`/`DANGER_COLOR` (335-337) take the `Visuals` it already receives or one added parameter, fixed at its callers.

- [ ] **Step 1:** Capture the baseline: `cargo build -j 8 -p manifold -p manifold-view-webview2 -p manifold-view-pdfium -p gate` then `target/debug/gate.exe chrome-shots` into `$TEMP/shots-before` (read `gate --help` for the output argument). Keep it for T3-T6.
- [ ] **Step 2:** Replace the sites per the rule. Add one test in `card/tests.rs`:
```rust
#[test]
fn border_colors_come_from_the_visuals() {
    let ctx = crate::theme::testutil::ctx(true);
    let v = ctx.global_style().visuals.clone();
    assert_eq!(crate::theme::accent(&v), crate::theme::ACCENT);
}
```
- [ ] **Step 3:** `cargo test -j 8 -p manifold card` — Expected: PASS. `grep -rnwE "ACCENT|DANGER_COLOR|TEXT_PRIMARY|TEXT_WEAK|BG|DANGER_FILL|SUCCESS_COLOR|SCRIM_TEXT|SCRIM_TEXT_WEAK|NOTE_CARD_COLOR" crates/manifold/src/card.rs crates/manifold/src/card` returns only comments/test lines.
- [ ] **Step 4:** Commit `Cards read colors from Visuals and Palette`.

### Task 4: Convert graph and canvas code

**Files:** `graph_ui/camera.rs` (293-313), `graph_ui/cards.rs` (303), `graph_ui/hints.rs` (11-20), `graph_ui/keys.rs` (266-267), `graph_ui/scene.rs` (37), `level.rs` (60), `link_gestures.rs` (191-206, 282-283, 510), `trail_strip.rs` (11, 96-125), `trail_nav.rs` (589), `zoom_chip.rs` (68), `image_view.rs` (267), `chrome_ui.rs` (94-114), `process_manager.rs` (comment only), `view_info.rs` (27-38, 161-211), `debug_export.rs` (265).

**Interfaces:** Consumes T2. Produces `link_gestures::retarget_look` signature gaining `v: &Visuals` if it lacks one (callers fixed once).

Mapping beyond T3's rule: `SURFACE_RAISED` -> `v.widgets.inactive.bg_fill`; `SURFACE_HOVER` -> `v.widgets.hovered.bg_fill`; `OUTLINE` -> `v.widgets.hovered.bg_stroke.color`; `TOOLBAR_FILL` -> `v.window_fill`; `refused_color()` -> `refused_color(v)` (`lerp_color32(TAG_SKY_BLUE, v.panel_fill, 0.2)`); `ViewTier::color`/`Tone::color` take `&Visuals`. `debug_export.rs:265` keeps exporting `danger` and `accent` using `ctx`'s visuals so `gate` smoke (`smoke.rs:655-688`) still reads the live theme.

- [ ] **Step 1:** Replace per the rule; where a function has no `Ui`/`Painter`/`Context`, add a `&Visuals` parameter and fix its callers (grep each).
- [ ] **Step 2:** Test in `view_info.rs` tests: `ViewTier::Recommended.color(&v) == palette(&v).success`.
- [ ] **Step 3:** `cargo test -j 8 -p manifold` — Expected: PASS. Grep as in T3 over these files returns nothing live.
- [ ] **Step 4:** Commit `Graph, links and info views read colors from Visuals`.

### Task 5: Convert popups, settings, tab view, identity

**Files:** `popups/confirm.rs` (28, 247-248 test), `popups/dropdowns.rs` (336, 591, 796, 974-991), `popups/menus.rs` (202-204), `popups/popup_frame.rs` (119), `popups/shortcuts_help.rs` (79), `settings_page.rs` (283-285, 348, 386), `tab_view.rs` (774, 814), `json_level_ui.rs` (32 comment, 324, 575, 720-722, 918, 1067 test), `identity.rs` (290-294 `ink_on`, tests 321-425), and `gate/src/smoke.rs` only if it references a deleted constant (it matches on names in comments/messages; check `cargo check -p gate`).

**Interfaces:** `identity::ink_on(color) -> Color32` keeps its signature and luminance rule but returns `theme::INK_DARK` where it returned `BG` and `theme::INK_LIGHT` where it returned `TEXT_PRIMARY` (consts from T2; theme-independent because the ink sits on the tag color).

- [ ] **Step 1:** Replace per the rule. In `identity.rs` tests, replace `BG`/`TEXT_PRIMARY` expectations with `INK_DARK`/`INK_LIGHT`.
- [ ] **Step 2:** Move the ~30 `apply_fonts`-only test contexts (`grep -rn "apply_fonts" crates/manifold/src | grep -v "fn apply_fonts"`) and the `apply_style` ones (list above: `graph_nav.rs:870`, `node_menu/tests.rs:472`, `popups.rs` x7, `settings_page.rs` x2, `view_info.rs:397`, `theme.rs` x2, `confirm.rs` x2, `dropdowns.rs:945`) to `crate::theme::testutil::ctx(true)`.
- [ ] **Step 3:** `cargo test -j 8 -p manifold` — Expected: PASS.
- [ ] **Step 4:** Commit `Popups, settings and identity read colors from Visuals; tests share testutil::ctx`.

### Task 6: Delete the constants; prove no pixel changed

**Files:** `theme.rs`.

- [ ] **Step 1:** Delete `TOOLBAR_FILL, ACCENT, DANGER_COLOR, DANGER_FILL, SUCCESS_COLOR, SCRIM_TEXT, SCRIM_TEXT_WEAK, NOTE_CARD_COLOR, BG, SURFACE_RAISED, SURFACE_HOVER, OUTLINE, TEXT_PRIMARY, TEXT_WEAK` and the `color()` helper; the dark `Palette` static and `fill_style` build from `manifold_pages::palette::DARK` directly. Update `theme.rs` tests to read the table: `const SURFACES` etc. take values from `palette::DARK`. Run `cargo test -j 8 -p manifold --no-run` and fix every test-only use the compiler reports (the survey excluded `tests.rs` files). Then `grep -rn "const .*Color32" crates/manifold/src` and confirm no remaining const bakes in a dark color (Review Focus 5).
- [ ] **Step 2:** `cargo check -j 8 --workspace --exclude manifold-view-cef --exclude manifold-view-servo --exclude manifold-view-webkit` — Expected: no unresolved names. `cargo clippy` command from Global Constraints — Expected: clean.
- [ ] **Step 3:** Rebuild, `gate chrome-shots` into `$TEMP/shots-after`; compare with `shots-before` byte for byte (`cmp` each file). Expected: identical in all 31 views. A difference means a mapping changed a color: fix the site, do not re-baseline.
- [ ] **Step 4:** Commit `Theme: delete the color constants (dark pixel-identical)`.

### Task 7: Heptabase dark values, shape and shadow, canvas clear

**Files:** `manifold-pages/src/lib.rs` (`DARK` values; page-test hex), `theme.rs` (radii, `DARK_PALETTE`, `fill_style` shadow), `card.rs` (`card_glow_shapes`, its callers' `intensity`), `gpu.rs` (clears at ~910 and ~963; `render` gains `clear: wgpu::Color`), the `render` caller in `event_loop.rs`/`main.rs` (grep `\.render(`).

**Interfaces:** `GpuState::render(&mut self, panes, content_on_top, egui_output, screenshot_path, canvas: egui::Color32)`; `Palette.shadow: u8` (peak alpha). `card_glow_shapes(rect, base_corner_radius, color, inv_zoom, intensity, offset_y: f32) -> [Shape; 2]` with rects `rect.translate(vec2(0, offset_y*inv_zoom)).expand(grow*inv_zoom)`.

- [ ] **Step 1: Failing tests.**
  - Contrast table: first change `DARK` to the new values, run `colour_roles_keep_their_contrast_on_their_surfaces` — it fails or passes: that run decides any value tweak (Expected: the table in the spec passes; if a role fails, nudge that channel by <=12 and rerun; record final values in the spec).
  - `card_glow_shapes_keeps_two_shapes_and_offsets_downward`: `shapes.len() == 2` and `shapes[0].visual_bounding_rect().center().y > rect.center().y`.
  - A `gpu.rs` unit test is not possible without a device; clear colour is checked in T10's smoke.
- [ ] **Step 2: Dark values** (`palette::DARK`): bg `[41,39,35]`, toolbar_fill `[47,45,40]`, surface_raised `[58,55,49]`, surface_hover `[70,67,60]`, outline `[140,135,124]`, text_primary `[236,233,226]`, text_weak `[176,171,160]`, accent `[143,176,255]`, danger `[236,120,100]`. Update `DARK_PALETTE.text/text_weak/canvas`, set `danger_fill` `[150,52,38]`, `shadow: 70`. Update the page test's expected `--bg:#292723;` and `--accent:#8FB0FF;`.
- [ ] **Step 3: Shape.** `RADIUS = 8`, `RADIUS_ITEM = 6`, `RADIUS_CHIP = 10`, card corner `12`. For each `rect_filled`/`rect_stroke` literal radius found by `grep -rn "rect_filled\|rect_stroke" crates/manifold/src | grep -E ", (2|3|4|5|6)\.0"` decide per site: card or control corners take the constant; circle radii and margins stay. Card corner source: `card_style` `rounding` fields (`card.rs:261-290`) set to 12 (`dim_rounding` follows).
- [ ] **Step 4: Shadow.** `card_glow_shapes` per the interface: alphas `(shadow*0.6, shadow*0.25)`, grows `(4.0, 12.0)`, `offset_y` 3.0 for rest shadow; hover/focus callers keep passing the state color and `intensity` as today. Note `chrome_ui.rs:114`'s call and every caller (`grep -rn card_glow_shapes`).
- [ ] **Step 5: Clear.** In `render`, convert `canvas` to `wgpu::Color` (linear if the surface is sRGB: copy the conversion `gpu.rs` already uses for egui colors; grep `srgb`); use it at both clears. Pass `palette(&ctx.global_style().visuals).canvas` from the caller.
- [ ] **Step 6:** Mute the canvas dot grid and edge colors: `grep -rn "dot\|grid" crates/manifold/src/graph_ui/scene.rs crates/manifold/src/level.rs` for the canvas dot grid; mute to `with_alpha(text_weak, 40)`.
- [ ] **Step 7:** `cargo test -j 8 -p manifold -p manifold-pages`, clippy, then `gate chrome-shots`; view the shots (screenshots or the generated sheet) and compare with `look_finalists.html` dark. Commit `Heptabase dark: values, radii, soft shadow, canvas clear`.

### Task 8: Light tables and per-theme contrast

**Files:** `manifold-pages/src/lib.rs`, `theme.rs`.

**Interfaces:** `palette::LIGHT: Colors`; `palette::colors(dark)` returns by flag; `LIGHT_PALETTE`; `palette(v)` matches on `v.dark_mode`; `fill_style(.., false)` starts from `Visuals::light()` and gets `shadow: 28`.

- [ ] **Step 1: Failing tests.** Parameterize the contrast test over `[true, false]` using `testutil::ctx(dark)` and `palette::colors(dark)`; add:
```rust
#[test]
fn card_and_canvas_differ_in_both_themes() {
    for dark in [true, false] {
        let c = manifold_pages::palette::colors(dark);
        assert_ne!(c.surface_raised, c.bg, "dark={dark}");
    }
}
#[test]
fn every_tag_color_has_3_to_1_on_its_themes_card() {
    for dark in [true, false] {
        let [r, g, b] = manifold_pages::palette::colors(dark).surface_raised;
        let card = egui::Color32::from_rgb(r, g, b);
        for c in TAG_COLOR_PALETTE {
            assert!(contrast(c, card) >= 3.0, "{c:?} on the {} card", if dark { "dark" } else { "light" });
        }
    }
}
```
Run: Expected FAIL (no `LIGHT`). If the tag-color test fails for specific colors on the light card (Okabe-Ito yellow is the likely one), do not patch a color: list the failures and ask the user whether tag swatches get a ring, or a per-theme tag table (a design decision).
- [ ] **Step 2: Light values:** bg `[235,232,225]`, toolbar_fill `[245,243,238]`, surface_raised `[255,255,255]`, surface_hover `[240,237,230]`, outline `[150,145,135]`, text_primary `[34,32,28]`, text_weak `[100,95,86]`, accent `[59,109,240]`, danger `[190,50,30]`. `LIGHT_PALETTE`: ink dark, `danger_fill [190,50,30]`, `success [46,125,50]`, `scrim_text` unchanged (scrims are dark in both), `note_card` unchanged, `canvas` = bg, `shadow 28`.
- [ ] **Step 3:** Run the contrast tests; any failure -> adjust that channel (<=12) and rerun; "accent as thin text on white" failing means adding `Palette.accent_text` (darker blue `[36,84,214]`) used by text sites (`popups/confirm.rs:28`, `shortcuts_help.rs:79`, `pin_color`); record the outcome in the spec's Unknown 4.
- [ ] **Step 4:** Commit `Light tables with per-theme contrast tests`.

### Task 9: Theme setting, title bar, pages

**Files:** `settings.rs` (`Settings.theme`), `settings_page.rs` (a row), `theme.rs` (`apply_titlebar`), `event_loop.rs:74` and the frame loop, `manifold-pages/src/lib.rs` (`set_dark`, cache by theme), page-reload hook in the app.

**Interfaces:** `Settings.theme: ThemeChoice { System, Light, Dark }` (serde snake_case, default `System`); `theme::apply_theme_choice(ctx, ThemeChoice)` -> `ctx.set_theme(egui::ThemePreference::{System,Light,Dark})`; `theme::apply_titlebar(window, dark: bool)` replacing `apply_dark_titlebar` (non-Windows: no-op); `manifold_pages::set_dark(bool)`; `serve` reads a process `AtomicBool`.

- [ ] **Step 1: Failing tests.** `settings.rs`: an old JSON without `theme` loads `System`; `Dark` round-trips. `manifold-pages`: after `set_dark(false)`, `serve("manifold://start").1` contains `--bg:#EBE8E1;` and after `set_dark(true)` contains `--bg:#292723;` (guard with a test mutex, the flag is process-global; serialize the two assertions in one test).
- [ ] **Step 2: Implement.** `themed(index)` caches two page sets (`[[String;4];2]`, one `OnceLock`), `serve` picks by the atomic. `apply_titlebar` sets `DWMWA_USE_IMMERSIVE_DARK_MODE` to `dark as BOOL`. Settings page row: a `ComboBox` like the search-engine one (`settings_page.rs:117`), save on change, call `apply_theme_choice`.
- [ ] **Step 3: Frame loop.** Add `fn theme_changed(last: &mut Option<egui::Theme>, now: egui::Theme) -> bool` in `theme.rs` (true on first call and on change) with a test `theme_changed_reports_first_and_each_switch` (calls: Dark->true, Dark->false, Light->true, Dark->true). Keep `last_theme: Option<egui::Theme>` on the app; each frame `if theme_changed(&mut self.last_theme, ctx.theme()) {` do: `apply_titlebar(&window, now == Dark)`, `manifold_pages::set_dark(..)`, reload open `manifold://` tabs (find the existing reload entry point: `grep -n "fn reload" crates/manifold/src/*.rs`). At startup call `apply_theme_choice` from the loaded `Settings` and `manifold_pages::set_dark` before the first frame, and delete T2's temporary `set_theme(Dark)` line in `apply_style`.
- [ ] **Step 4:** `cargo test -j 8 -p manifold -p manifold-pages`; clippy; commit `Theme setting System|Light|Dark: title bar and pages follow`.

### Task 10: Verify both themes end to end, docs, close-out

**Files:** `docs/superpowers/specs/2026-10-22-soft-theme-design.md` (`status: done`, record final values and Unknown 4/5 outcomes), `README.md` (theme notes), `CHANGELOG.md` is the user's at tag time (not touched), `PARKED.md` (leftovers).

- [ ] **Step 1:** `gate chrome-shots` in dark and then in light (setting `theme` to `light` in a scratch profile); read both sets; check no black hairline at the tab viewport edge, tag colors legible, notes readable. With a `manifold://start` tab open in a running build, switch the Settings choice Dark -> Light -> System and confirm the title bar, canvas and that page all follow without restart (Review Focus 1-3; no unit test reaches this).
- [ ] **Step 2:** Run `gate tier` and report which tier applies; run it only if the user asks (Global Constraints). Report CPU load and Defender state with any timing.
- [ ] **Step 3:** Docs: mark spec `done` and plan `done`; add leftover items (screen-constant shadow scaling if not done) to `PARKED.md` first.
- [ ] **Step 4:** Commit `Soft theme: docs`. Do not tag or push.
