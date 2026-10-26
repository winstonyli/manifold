//! Popups and floating windows. The anchored toolbar dropdowns are in `dropdowns`, their shared shell in
//! `popup_frame`, and the rest by name: `menus`, `confirm`, `find_bar`, `shortcuts_help`.

use super::*;
use crate::nav_history::Dir;
use crate::store::model::Kind;

mod confirm;
mod dropdowns;
mod find_bar;
mod menus;
mod popup_frame;
mod shortcuts_help;

pub(crate) use confirm::*;
#[cfg(test)] // the trail popup's tests (`trail_nav`) place it by `popup_start_rect`
pub(crate) use dropdowns::*;
pub(crate) use find_bar::*;
pub(crate) use menus::*;
pub(crate) use popup_frame::*;

/// A `TextEdit` that a shortcut or click wants focused. The widget may not
/// exist until a later frame (a rename field replaces its button, the find
/// bar opens), so the request waits in `App::pending_focus` until the
/// widget is built and takes it with `take_focus`. One at a time, since
/// only one widget can hold focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FocusTarget {
    /// The inline omnibar (`App::omni`), whichever view shows it.
    OmniBar,
    /// The Ctrl+K popup's omnibar (`App::omni_popup`).
    OmniPopup,
    FindBar,
    GroupRename,
    BookmarkFolder,
    /// The tab menu's "New Jar..." name field (`App::naming_jar`).
    JarName,
}

/// True, once, if `target` is the pending focus request: clears it so the
/// caller can `request_focus()` its widget.
pub(crate) fn take_focus(pending: &mut Option<FocusTarget>, target: FocusTarget) -> bool {
    if *pending == Some(target) {
        *pending = None;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focus_request_is_taken_once_by_its_own_widget() {
        let mut pending = Some(FocusTarget::FindBar);
        assert!(!take_focus(&mut pending, FocusTarget::OmniBar));
        assert!(take_focus(&mut pending, FocusTarget::FindBar));
        assert!(!take_focus(&mut pending, FocusTarget::FindBar));
    }
}

#[cfg(test)]
mod view_picker_tests {
    use super::*;

    fn escape() -> egui::Event {
        let key = egui::Key::Escape;
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.build_ui(ui));
        output.textures_delta.clear(); // nothing uploads the font atlas here
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The pane's half of the list navigation (card-pane spec R5): Up/Down step only while a row has focus, and
    /// with none focused the keys are left alone.
    #[test]
    fn step_focused_row_moves_only_a_focused_list() {
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |events: Vec<egui::Event>| -> (Vec<egui::Id>, bool) {
            let mut rows = Vec::new();
            let mut left = false;
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                for label in ["a", "b", "c"] {
                    rows.push(list_row(ui, label).id);
                }
                let list = take_list_rows(ui);
                step_focused_row(ui, &list.rows, list.partial);
                left = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
            });
            output.textures_delta.clear();
            (rows, left)
        };
        let (rows, _) = frame(vec![]);
        frame(vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), None, "no row takes focus on its own");
        let (_, left) = frame(vec![key(egui::Key::ArrowDown)]);
        assert!(left && ctx.memory(|m| m.focused()).is_none(), "Down with nothing focused is not the list's");
        ctx.memory_mut(|m| m.request_focus(rows[0]));
        frame(vec![]);
        frame(vec![key(egui::Key::ArrowDown)]);
        frame(vec![]);
        assert!(ctx.memory(|m| m.has_focus(rows[1])), "Down from a focused row steps");
        frame(vec![key(egui::Key::ArrowUp)]);
        frame(vec![key(egui::Key::ArrowUp)]);
        frame(vec![]);
        assert!(ctx.memory(|m| m.has_focus(rows[2])), "and wraps");
    }

    /// One frame of a three-row list popup; returns the row clicked.
    fn list_frame(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<usize> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut clicked = None;
        let mut output = ctx.run_ui(input, |ui| {
            let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
            build_list_popup_frame(ui.ctx(), "test_list", anchor, 100.0, 300.0, 1.0, true, |ui| {
                for row in 0..3 {
                    if list_row(ui, format!("row {row}")).clicked() {
                        clicked = Some(row);
                    }
                }
            });
        });
        output.textures_delta.clear(); // nothing uploads the font atlas here
        clicked
    }

    /// A popup of a row, a stop of three widgets (as the node menu's swatches) and a row: Up/Down walk the three stops,
    /// wrapping, and entering the stop lands on its first widget.
    #[test]
    fn a_stop_of_several_widgets_is_one_step_of_up_and_down() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut stops: Vec<Vec<egui::Id>> = Vec::new();
        let mut frame = |events: Vec<egui::Event>| -> Option<usize> {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
                build_list_popup_frame(ui.ctx(), "test_stop", anchor, 100.0, 300.0, 1.0, true, |ui| {
                    stops.clear();
                    stops.push(vec![list_row(ui, "first").id]);
                    let ids: Vec<egui::Id> = ui
                        .horizontal(|ui| {
                            (0..3)
                                .map(|_| ui.allocate_exact_size(egui::Vec2::splat(14.0), egui::Sense::click()).1.id)
                                .collect()
                        })
                        .inner;
                    nav_stop(ui, &ids);
                    stops.push(ids);
                    stops.push(vec![list_row(ui, "last").id]);
                });
            });
            output.textures_delta.clear();
            let focused = |id: &egui::Id| ctx.memory(|m| m.has_focus(*id));
            let at = stops.iter().position(|stop| stop.iter().any(focused))?;
            // Entering the stop focuses its first widget.
            assert!(at != 1 || focused(&stops[1][0]));
            Some(at)
        };
        frame(vec![]);
        assert_eq!(frame(vec![]), Some(0), "the popup opens on its first row");
        for want in [1, 2, 0] {
            frame(vec![key(egui::Key::ArrowDown)]);
            assert_eq!(frame(vec![]), Some(want), "Down");
        }
        frame(vec![key(egui::Key::ArrowUp)]);
        assert_eq!(frame(vec![]), Some(2), "Up wraps to the last stop");
        // The focused row going away (a page swap) leaves nothing focused: Down enters at the first row, Up at the last.
        for (press, want) in [(egui::Key::ArrowDown, 0), (egui::Key::ArrowUp, 2)] {
            ctx.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
            });
            assert_eq!(frame(vec![]), None, "focus dropped");
            frame(vec![key(press)]);
            assert_eq!(frame(vec![]), Some(want), "{press:?} with nothing focused");
        }
    }

    /// A page swap replaces the rows, so the focused one goes away: the popup focuses the new page's first row, as when it
    /// opens.
    #[test]
    fn a_page_swap_focuses_the_new_pages_first_row() {
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |page: u8, events: Vec<egui::Event>| -> Vec<egui::Id> {
            let mut first: Vec<egui::Id> = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
                build_list_popup_frame(ui.ctx(), "test_page", anchor, 100.0, 300.0, 1.0, true, |ui| {
                    nav_page(ui, egui::Id::new(page));
                    first.clear();
                    for label in ["a", "b", "c"] {
                        // Each page's rows have their own ids.
                        first.push(ui.push_id(page, |ui| list_row(ui, label).id).inner);
                    }
                });
            });
            output.textures_delta.clear();
            first
        };
        frame(0, vec![]);
        frame(0, vec![]);
        frame(0, vec![key(egui::Key::ArrowDown)]);
        let first = frame(0, vec![]);
        assert!(ctx.memory(|m| m.has_focus(first[1])), "Down moved to the second row");
        frame(1, vec![]);
        let first = frame(1, vec![]);
        assert!(ctx.memory(|m| m.has_focus(first[0])), "the new page opens on its first row");
    }

    /// The shared stepping rule: wrap or clamp by the net count, and every press in a frame counts.
    #[test]
    fn arrow_presses_step_a_list_by_their_net_count() {
        assert_eq!(step_index(1, 1, 4, true), 2);
        assert_eq!(step_index(3, 1, 4, true), 0, "wraps past the end");
        assert_eq!(step_index(0, -1, 4, true), 3, "wraps before the start");
        assert_eq!(step_index(1, 6, 4, true), 3, "several steps wrap together");
        assert_eq!(step_index(3, 1, 4, false), 3, "stops at the end");
        assert_eq!(step_index(1, -5, 4, false), 0, "stops at the start");
        let ctx = egui::Context::default();
        let mut seen = Vec::new();
        for events in [
            vec![key(egui::Key::ArrowDown), key(egui::Key::ArrowDown), key(egui::Key::ArrowUp)],
            vec![key(egui::Key::ArrowUp), key(egui::Key::ArrowUp)],
            vec![],
        ] {
            let input = egui::RawInput { events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| seen.push(ui.input(arrow_delta)));
            output.textures_delta.clear();
        }
        assert_eq!(seen, [1, -2, 0]);
    }

    /// Two Down presses that land in one frame (a slow frame, a key repeat) are two steps, not one.
    #[test]
    fn two_presses_in_one_frame_are_two_steps() {
        let ctx = crate::theme::testutil::ctx(true);
        let frame = |events: Vec<egui::Event>| -> Vec<egui::Id> {
            let mut rows: Vec<egui::Id> = Vec::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(60.0, 20.0));
                build_list_popup_frame(ui.ctx(), "test_two", anchor, 100.0, 300.0, 1.0, true, |ui| {
                    rows.clear();
                    for label in ["a", "b", "c", "d"] {
                        rows.push(list_row(ui, label).id);
                    }
                });
            });
            output.textures_delta.clear();
            rows
        };
        frame(vec![]);
        frame(vec![]);
        frame(vec![key(egui::Key::ArrowDown), key(egui::Key::ArrowDown)]);
        let rows = frame(vec![]);
        assert!(ctx.memory(|m| m.has_focus(rows[2])), "two Downs in one frame reach the third row");
    }

    /// A long History popup lays out only the rows in view. Down walks one entry at a time past the first view
    /// (the list scrolls), and Up walks back one at a time to the newest entry (the list scrolls back), without
    /// wrapping at either end of the laid-out window.
    #[test]
    fn a_long_history_popup_lays_out_only_the_rows_in_view() {
        let ctx = crate::theme::testutil::ctx(true);
        let history = (0..300)
            .map(|i| crate::history::HistoryEntry { title: format!("entry {i}"), url: format!("https://e/{i}") })
            .collect();
        let mut app = App { history, ..App::default() };
        app.menus.open_popup = Some(AnchoredPopup::History);
        let anchor = egui::Rect::from_min_size(egui::pos2(100.0, 40.0), egui::vec2(20.0, 20.0));
        // A second a frame, so `scroll_to_me`'s scroll animation finishes between frames.
        let mut time = 0.0;
        // One frame: the entry numbers painted, top to bottom, and the focused row's entry.
        let mut frame = |events: Vec<egui::Event>| -> (Vec<usize>, Option<usize>) {
            time += 1.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.build_history_popup(ui, anchor, 1.0));
            output.textures_delta.clear();
            let rows: Vec<(usize, egui::Pos2)> = output
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::Shape::Text(t) => {
                        t.galley.job.text.strip_prefix("entry ").and_then(|n| n.parse().ok()).map(|n| (n, t.pos))
                    }
                    _ => None,
                })
                .collect();
            let focused = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).map(|r| r.rect);
            let at = focused.and_then(|r| rows.iter().find(|(_, pos)| r.y_range().contains(pos.y + 1.0)).map(|p| p.0));
            (rows.into_iter().map(|(n, _)| n).collect(), at)
        };
        for _ in 0..3 {
            frame(vec![]);
        }
        let (rows, at) = frame(vec![]);
        // Painted rows alone prove nothing (a label out of view is laid out but not painted): count the widgets.
        let widgets = ctx.viewport(|v| v.this_pass.widgets.layers().map(|(_, w)| w.len()).sum::<usize>());
        assert!(widgets < 100, "{widgets} widgets laid out for 300 entries");
        assert_eq!((rows[0], at), (299, Some(299)), "newest first, and focused");
        frame(vec![key(egui::Key::ArrowUp)]);
        assert_eq!(frame(vec![]).1, Some(299), "Up on the first row does not wrap in a windowed list");
        let steps = rows.len() + 5;
        for k in 1..=steps {
            frame(vec![key(egui::Key::ArrowDown)]);
            frame(vec![]); // the focused row scrolls into view on the frame after it takes focus
            let (rows, at) = frame(vec![]);
            assert_eq!(at, Some(299 - k), "Down step {k}: {rows:?}");
        }
        let (rows, _) = frame(vec![]);
        assert!(!rows.contains(&299), "Down walked past the first view: {rows:?}");
        assert!(rows.windows(2).all(|w| w[0] == w[1] + 1), "a contiguous window, newest first: {rows:?}");
        for k in (0..steps).rev() {
            frame(vec![key(egui::Key::ArrowUp)]);
            frame(vec![]); // the focused row scrolls into view on the frame after it takes focus
            let (rows, at) = frame(vec![]);
            assert_eq!(at, Some(299 - k), "Up back to {}: {rows:?}", 299 - k);
        }
        frame(vec![]); // the scroll back to the top settles one frame later
        let (rows, at) = frame(vec![]);
        assert_eq!((rows[0], at), (299, Some(299)), "back at the top, scrolled back: {rows:?}");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::History));
    }

    /// A list popup opens on its first row; Down and Up move (wrapping) and Enter clicks the focused row.
    #[test]
    fn list_rows_take_arrows_and_enter() {
        let ctx = egui::Context::default();
        theme::apply_style(&ctx);
        for _ in 0..3 {
            assert_eq!(list_frame(&ctx, vec![]), None);
        }
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(0), "opens on the first row");
        list_frame(&ctx, vec![key(egui::Key::ArrowUp)]);
        list_frame(&ctx, vec![]);
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(2), "Up from the first row wraps to the last");
        list_frame(&ctx, vec![key(egui::Key::ArrowDown)]);
        list_frame(&ctx, vec![]);
        assert_eq!(list_frame(&ctx, vec![key(egui::Key::Enter)]), Some(0), "Down from the last row wraps to the first");
    }

    /// Escape closes an open engine picker and nothing else (the settings page stays), and focus goes back to
    /// the button that opened it; the next Escape leaves the page.
    #[test]
    fn escape_closes_the_picker_then_the_page_and_refocuses_the_button() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App { view_mode: ViewMode::Settings, ..App::default() };
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        app.menus.open_popup = Some(AnchoredPopup::DefaultViewKind);
        for _ in 0..3 {
            frame(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::DefaultViewKind), "the picker should stay open");
        frame(&mut app, &ctx, vec![escape()]);
        assert_eq!(app.menus.open_popup, None, "Escape should close the picker");
        assert_eq!(app.view_mode, ViewMode::Settings, "the same Escape must not leave the settings page");
        let opener = ctx.data(|d| d.get_temp::<egui::Id>(App::popup_opener_key(AnchoredPopup::DefaultViewKind)));
        frame(&mut app, &ctx, vec![]);
        assert!(opener.is_some_and(|id| ctx.memory(|m| m.has_focus(id))), "focus should return to the opening button");
        frame(&mut app, &ctx, vec![escape()]);
        assert_ne!(app.view_mode, ViewMode::Settings, "a second Escape should leave the page");
    }

    /// Clicking an unavailable engine in the open picker toasts its reason, switches nothing and leaves the
    /// picker open.
    #[test]
    fn clicking_an_unavailable_view_explains_and_keeps_the_picker_open() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App { view_mode: ViewMode::Settings, ..App::default() };
        let (first, reason) = (ViewKind::picker_order()[0], "needs Windows");
        // `build_ui` clears the per-frame state cache, so seed it inside each frame, past that clear.
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 1000.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let state = crate::view_install::ViewStatus::Unavailable(reason.to_string());
                app.view_ui.view_status_cache.borrow_mut().insert(first, state);
                app.build_ui_inner(ui);
            });
            output.textures_delta.clear();
        };
        let before = app.settings.default_starting_view_kind;
        app.menus.open_popup = Some(AnchoredPopup::DefaultViewKind);
        for _ in 0..4 {
            frame(&mut app, vec![]);
        }
        let area = ctx.memory(|m| m.area_rect(egui::Id::new("DefaultViewKind_popup"))).expect("the picker is drawn");
        let at = area.min + egui::vec2(120.0, 30.0); // inside the first card
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![button(true)]);
        frame(&mut app, vec![button(false)]);
        let toasted = |text: &str| {
            app.toasts
                .save_toast
                .iter()
                .map(|(t, _)| t)
                .chain(app.toasts.save_toast_queue.iter())
                .any(|t| t.contains(text))
        };
        assert!(toasted(reason), "the reason should be toasted");
        assert_eq!(app.menus.open_popup, Some(AnchoredPopup::DefaultViewKind), "the picker stays open");
        assert_eq!(app.settings.default_starting_view_kind, before, "nothing switched");
    }
}
