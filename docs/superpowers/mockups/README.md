# UI mockups

HTML sketches from the design sessions, recovered from the session transcripts (2026-10-01); open them in a browser (they load Google Fonts and Tabler icons from CDNs).

- `toolbar_redesign_sketch.html` (2026-09-23): three toolbar concepts. **Option A, segmented chips, was approved**: raised chips with ghost icons, hairline dividers between chips, one solid-gold primary action, soft-filled fields. Implemented by `toolbar_chip`, `primary_button` and `toolbar_panel` in `crates/manifold/src/theme.rs`.
- `spacing-options.html` (2026-09-20): compact vs airy. Chosen: airy for dialogs and settings, compact for the graph toolbar; menus were later made compact too (`menu_item_style`).
- `heading-final-v2.html`, `editorial-options-visual.html` (2026-09-20): heading font and palette explorations behind the editorial retheme.

Compare with the running app using `scripts/chrome-shots.sh`.
- `toolbar_v2.html` (2026-10-01): self-contained (inline SVG icons, no CDN or host CSS variables, which is why the older sketches above render unstyled and iconless outside their original chat widget). Both bars built from the app's real controls: one button and field anatomy, 38 px bar, chips per cluster, icon-first, an inset field with hairline border and accent focus ring. A proposal, not yet implemented; the app currently differs in bar height (32 px), the text "Graph"/"Split"/"Tab actions" buttons and the borderless address field.
- `toolbar_fonts.html` (2026-10-01): the toolbar_v2 bars in 14 faces (Literata and Newsreader from `crates/manifold/assets/fonts`, the rest Windows system fonts), generated from `toolbar_v2.html`. Needs about 820 px of width; narrower panes scroll sideways.
- `toolbar_sans.html` (2026-10-01): the same bars in 16 open-licence sans faces (Inter, IBM Plex Sans, Manrope, DM Sans, Figtree, Plus Jakarta Sans, Instrument Sans, Schibsted Grotesk, Geist, Onest, Outfit, Sora, Atkinson Hyperlegible, Bricolage Grotesque, Familjen Grotesk, Space Grotesk) plus Literata for reference, ordered roughly neutral to idiosyncratic. Loads from Google Fonts, so it needs a network connection; any chosen face would have to be bundled (OFL) in `crates/manifold/assets/fonts`.
