//! The views driven from the keyboard: Tab to a card by its name, then what its keys do there.

use std::io;

use super::Shots;
use crate::check::check;

impl Shots {
    pub(super) fn keyboard_views(&mut self) -> io::Result<()> {
        // A click focuses the group card without entering it; Enter then does what a double-click does.
        self.view("graph-enter-group", |s| {
            s.tab_to("Group: ")?;
            s.shot("graph-enter-group-click")?;
            send!(s, "ENTER")?;
            s.shot("graph-enter-group")?;
            s.reset()
        })?;
        // Tab to the JSON card and Enter: the level opens with its root card focused.
        self.view("graph-enter-json", |s| {
            s.tab_to("JSON: ")?;
            send!(s, "ENTER")?;
            s.shot("graph-enter-json")?;
            s.reset()
        })?;
        // The menu opens on the focused card.
        self.view("graph-menu-key", |s| {
            s.tab_to("Tab: ")?;
            send!(s, "SHIFTF10", "WAITIDLE")?;
            check("Shift+F10 on a Tab-focused card opens its menu", s.menu_open()?);
            s.shot("graph-menu-key-open")?;
            send!(s, "TABKEY")?;
            s.shot("graph-menu-key-tab")?;
            // Still open: Down through the rows.
            // A shot after each Down: the frame it forces is what lets the next key see the new focus.
            for i in 1..=5 {
                send!(s, "ARROWDOWN")?;
                s.shot(&format!("graph-menu-key-down{i}"))?;
            }
            send!(s, "ESCAPE")?;
            s.shot("graph-menu-key-closed")?;
            s.reset()
        })?;
        // Space on a Tab-focused tab card selects it (and Space again clears it).
        self.view("graph-select-key", |s| {
            s.tab_to("Tab: ")?;
            send!(s, "SPACEKEY")?;
            s.shot("graph-select-key-on")?;
            send!(s, "SPACEKEY")?;
            s.shot("graph-select-key-off")?;
            // Two more cards selected: the selection bar sits on a box around both.
            send!(s, "TABKEY", "WAITIDLE", "SPACEKEY", "WAITIDLE", "TABKEY", "WAITIDLE", "SPACEKEY")?;
            s.shot("graph-select-key-two")?;
            // Pinned: the bar's pin goes accent, and each card keeps its passive pin mark.
            send!(s, "TOGGLEPIN")?;
            s.shot("graph-select-key-pinned")?;
            s.reset()
        })?;
        // Over `LOD_SIMPLIFY_COUNT` cards the graph draws simple: a pinned card shows its accent dot, not the glyph.
        self.view("graph-pinned-dense", |s| {
            let seeded: Vec<String> = s.state()?.cards().iter().map(|c| c["id"].to_string()).collect();
            for _ in 0..70 {
                send!(s, "NEWNOTE")?;
            }
            s.settle()?;
            send!(s, "TABKEY", "WAITIDLE")?;
            send!(s, "SPACEKEY", "WAITIDLE", "TOGGLEPIN", "WAITIDLE")?;
            for _ in 0..6 {
                s.inst.click_control(crate::state::ZOOM_OUT)?;
                s.settle()?;
            }
            s.shot("graph-pinned-dense")?;
            // The notes and the zoom would reach every view after this one.
            for id in s.state()?.cards().iter().map(|c| c["id"].to_string()).filter(|id| !seeded.contains(id)) {
                send!(s, format!("CLOSENODE {}", id.trim_matches('"')))?;
            }
            s.settle()?;
            s.inst.click_control(crate::state::FIT_VIEW)?;
            s.reset()
        })?;
        // Shift+Tab walks back out of the cards through the canvas, a stop with no name, to the bar's last control; Tab
        // comes back the same way.
        self.view("graph-shift-tab", |s| {
            let focus = |s: &mut Self| -> io::Result<Vec<String>> {
                Ok(s.state()?.selected_controls().into_iter().map(|c| c.name.clone()).collect())
            };
            send!(s, "DIGITKEY 1", "WAITIDLE")?;
            let card = focus(s)?;
            send!(s, "TABKEY shift", "WAITIDLE")?;
            let canvas = s.state()?.focus();
            send!(s, "TABKEY shift", "WAITIDLE")?;
            let bar = focus(s)?;
            println!("Shift+Tab twice from {card:?} went to {bar:?}");
            check("Digit 1 focuses the first card", !card.is_empty());
            check("Shift+Tab leaves the first card", canvas.is_some());
            check("A second Shift+Tab reaches a named control", !bar.is_empty() && bar != card);
            send!(s, "TABKEY", "WAITIDLE", "TABKEY", "WAITIDLE")?;
            check("Two Tabs come back to the first card", focus(s)? == card);
            s.reset()
        })?;
        // Tab to the Note card and Enter: the note starts editing; Escape ends it and Enter edits again (focus
        // returned to the card).
        self.view("note-key", |s| {
            s.tab_to("Note: ")?;
            send!(s, "ENTER", "WAITIDLE")?;
            check("Enter on the Note card starts editing", s.editing()?);
            s.shot("note-key-editing")?;
            send!(s, "ESCAPE", "WAITIDLE")?;
            check("Escape ends editing", !s.editing()?);
            send!(s, "ENTER", "WAITIDLE")?;
            check("Enter edits again, so focus returned to the card", s.editing()?);
            s.reset()
        })?;
        // Enter the JSON level by keyboard in a small window (so some cards are off screen) and Tab through every
        // card after the root: each Tab must select a new card, off-screen ones included (zoomed in so they are).
        self.view("json-tab-offscreen", |s| {
            let sel = |s: &mut Self| -> io::Result<(String, usize)> {
                let st = s.state()?;
                let j = &st.0["json_level"];
                Ok((j["selected"].to_string(), j["cards"].as_array().map_or(0, Vec::len)))
            };
            s.tab_to("JSON: ")?;
            send!(s, "ENTER", "WAITIDLE")?;
            // Zoomed in, the cards other than the root fall off screen.
            send!(
                s,
                "ZOOMIN",
                "ZOOMIN",
                "ZOOMIN",
                "MOUSEMOVE 700 650",
                "MOUSEDOWN 700 650",
                "MOUSEMOVE 400 650",
                "MOUSEMOVE 100 650",
                "MOUSEUP 100 650",
                "MOUSEMOVE 100 650",
                "MOUSEMOVE 5 790",
                "WAITIDLE"
            )?;
            s.settle()?;
            s.shot("json-tab-offscreen-before")?;
            let total = sel(s)?.1;
            let mut seen = Vec::new();
            for _ in 1..total {
                send!(s, "TABKEY", "WAITIDLE")?;
                s.settle()?;
                seen.push(sel(s)?.0);
            }
            println!("selected after each Tab: {} (cards: {total})", seen.join(" "));
            seen.sort();
            seen.dedup();
            check(
                &format!("Tab reached {} of {} JSON cards after the root", seen.len(), total.saturating_sub(1)),
                total > 1 && seen.len() >= total - 1,
            );
            s.shot("json-tab-offscreen")?;
            s.reset()
        })?;
        // Digits jump to the card with that Tab-order number (the Note is 3 of the 5 seeded cards): 3 focuses it, so
        // Enter starts editing.
        self.view("digit-jump", |s| {
            s.reset()?;
            send!(s, "DIGITKEY 3")?;
            s.shot("digit-jump")?;
            send!(s, "ENTER", "WAITIDLE")?;
            check("digit 3 focuses card 3 (the note), Enter edits it", s.editing()?);
            s.reset()
        })
    }
}
