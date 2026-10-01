# UI mockups

HTML sketches from the design sessions, recovered from the session transcripts (2026-10-01); open them in a browser (they load Google Fonts and Tabler icons from CDNs).

- `toolbar_redesign_sketch.html` (2026-09-23): three toolbar concepts. **Option A, segmented chips, was approved**: raised chips with ghost icons, hairline dividers between chips, one solid-gold primary action, soft-filled fields. Implemented by `toolbar_chip`, `primary_button` and `toolbar_panel` in `crates/manifold/src/theme.rs`.
- `spacing-options.html` (2026-09-20): compact vs airy. Chosen: airy for dialogs and settings, compact for the graph toolbar; menus were later made compact too (`menu_item_style`).
- `heading-final-v2.html`, `editorial-options-visual.html` (2026-09-20): heading font and palette explorations behind the editorial retheme.

Compare with the running app using `scripts/chrome-shots.sh`.
