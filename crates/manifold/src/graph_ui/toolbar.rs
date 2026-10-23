//! The Graph view's toolbar: breadcrumb, group chip, zoom chip, filter, Recently Closed and the selection actions.

use super::*;

/// What the Graph toolbar (`App::render_graph_toolbar`) hands back to
/// `build_graph_ui`: the actions its buttons pushed, the zoom-chip
/// requests, and the button rects that popups and drops anchor to.
pub(crate) struct GraphToolbar {
    pub(crate) actions: Vec<GraphAction>,
    pub(crate) reset_view: bool,
    pub(crate) reset_zoom: bool,
    pub(crate) zoom_step: Option<f32>,
    pub(crate) shortcuts_help_button_rect: Option<egui::Rect>,
    pub(crate) recently_closed_button_rect: Option<egui::Rect>,
    /// The breadcrumb's back button, also a drop target (see `drop_target`).
    pub(crate) leave_group_button_rect: Option<egui::Rect>,
}

/// The most units a breadcrumb crumb shows (`names::shorten`; a placeholder counts one).
const CRUMB_UNITS: usize = 20;

/// The Ungroup All chip's hover, saying how much one click does: it dissolves every group here, keeping the members.
fn ungroup_all_tip(groups: usize) -> String {
    format!("Ungroup All ({groups} group{}): keeps the members", if groups == 1 { "" } else { "s" })
}

impl App {
    /// The Graph view's toolbar row: the group breadcrumb, "+ New Tab",
    /// the group and zoom chips, the filter, Recently Closed, the
    /// selection's actions, a spawn error and the universal chip. What its
    /// buttons ask for comes back as a `GraphToolbar`, which
    /// `build_graph_ui` applies at the same points it always did.
    pub(crate) fn render_graph_toolbar(&mut self, ui: &mut egui::Ui, groups_at_level: &[NodeId]) -> GraphToolbar {
        // A JSON level is read-only: no creation, grouping or rename here.
        let on_json_level = self.json_level.is_some();
        let mut actions: Vec<GraphAction> = Vec::new();
        let mut reset_view = false;
        let mut reset_zoom = false;
        let mut graph_zoom_step: Option<f32> = None;
        let mut shortcuts_help_button_rect: Option<egui::Rect> = None;
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        let mut leave_group_button_rect: Option<egui::Rect> = None;
        // `toolbar_panel` already provides the compact spacing and the
        // horizontal-scroll overflow safety this row specifically needed
        // (a breadcrumb of arbitrary depth, a selection-actions cluster, a
        // spawn error, and "Recently Closed" all able to appear at once
        // had already hit real overflow at this fixed row height more than
        // once -- see the icon-only button migration and breadcrumb-name
        // truncation elsewhere in this file, both direct responses to
        // that). Every other toolbar in this app gets the same safety net
        // now too, not just this one.
        toolbar_panel("graph_toolbar", ui, |ui| {
            ui.horizontal_centered(|ui| {
                leave_group_button_rect = self.toolbar_breadcrumb(ui, on_json_level, &mut actions);
                // "+ New Tab" is the only toolbar spawn action kept --
                // it's also the sole recovery path when `self.nodes` is
                // completely empty (see `build_graph_ui`'s early return),
                // where the right-click empty-canvas card can't appear
                // yet. New Note/Add File/New Group used to sit here too,
                // but that card already offers all three identically
                // once any node exists, and while the graph is fully
                // empty those buttons' own flags were silently dropped
                // anyway (their sole consumption site is unreachable
                // from the early-return branch) -- removing them is a
                // pure simplification, not a capability loss.
                if !on_json_level && primary_button(ui, "+ New Tab").on_hover_text("New tab (Ctrl+T)").clicked() {
                    actions.push(GraphAction::NewTab(self.focused));
                }
                if !on_json_level && (!self.headless || !groups_at_level.is_empty()) {
                    self.toolbar_group_chip(ui, groups_at_level, &mut actions);
                }
                match self.toolbar_zoom(ui) {
                    Some(ZoomChipAction::Fit) => reset_view = true,
                    Some(ZoomChipAction::Reset100) => reset_zoom = true,
                    Some(ZoomChipAction::In) => graph_zoom_step = Some(ZOOM_STEP),
                    Some(ZoomChipAction::Out) => graph_zoom_step = Some(1.0 / ZOOM_STEP),
                    None => {}
                }
                self.toolbar_filter(ui, on_json_level);
                recently_closed_button_rect = self.toolbar_recently_closed(ui);
                if !on_json_level {
                    self.toolbar_selection(ui, &mut actions);
                }
                shortcuts_help_button_rect = Some(self.render_universal_chip(ui));
            });
        });
        GraphToolbar {
            actions,
            reset_view,
            reset_zoom,
            zoom_step: graph_zoom_step,
            shortcuts_help_button_rect,
            recently_closed_button_rect,
            leave_group_button_rect,
        }
    }

    /// The breadcrumb trail with its back button, the rename box on the current group and, on a JSON level, the
    /// JSON bar. Returns the back button's rect.
    pub(super) fn toolbar_breadcrumb(
        &mut self,
        ui: &mut egui::Ui,
        on_json_level: bool,
        actions: &mut Vec<GraphAction>,
    ) -> Option<egui::Rect> {
        let mut leave_group_button_rect: Option<egui::Rect> = None;
        // Empty at the true top level; one entry per ancestor
        // group otherwise (root-most first, current group
        // last) -- groups nest now, so this is a real
        // breadcrumb trail, not just "are we inside the one
        // possible group or not." See `App::current_group`'s own doc
        // comment on nesting and `group_breadcrumb`'s own.
        let breadcrumb = self.group_breadcrumb();
        if !breadcrumb.is_empty() {
            // Goes up exactly one level (to the current group's
            // own parent, possibly another group, not straight
            // to the top) -- the breadcrumb trail below takes
            // the place "+ New Group" would otherwise fill in
            // the empty space "+ New Tab"/"Reset View" already
            // occupy at the top level. `CARET_LEFT`, not a raw
            // "◀" glyph -- the Tab toolbar's identical back
            // chevron already migrated to `egui_phosphor` for
            // this exact font's tofu-box risk with symbol
            // glyphs; this one had been missed.
            let leave_group_response = ui.button(egui_phosphor::light::CARET_LEFT).on_hover_text("Back one level");
            if leave_group_response.clicked() {
                actions.push(GraphAction::LeaveGroup);
            }
            leave_group_button_rect = Some(leave_group_response.rect);
            let crumb_count = breadcrumb.len();
            for (i, (id, name)) in breadcrumb.iter().enumerate() {
                // Each crumb's own name comes straight from a
                // Group's name -- unbounded, and now often an
                // object key or `[index]` from "Expand as
                // Graph" (see that feature's own doc comment),
                // which tend to run longer than a hand-typed
                // group name. A plain `ui.button(name)` has no
                // width limit of its own in this horizontal,
                // non-wrapping row, so a deeply-nested chain of
                // long names could push clean off the toolbar's
                // own visible edge -- the exact overflow this
                // toolbar's other buttons were already
                // confirmed live to hit once before (see the
                // icon-only button conversion elsewhere in this
                // file). So it is drawn by the display rule and
                // cut to `CRUMB_UNITS` by `names::shorten` (small
                // fixed chrome, not a place for arbitrarily long
                // text), where a placeholder counts as one unit
                // and cannot dodge the bound; the hover gives the
                // whole name.
                let segs = crate::names::display_name(name, crate::names::UNTITLED);
                let shown = crate::names::shorten(&segs, CRUMB_UNITS);
                let text = crate::names::name_job(ui, "", shown.as_deref().unwrap_or(&segs), "");
                if i + 1 == crumb_count && on_json_level {
                    // The Json node itself: its name is its file's,
                    // so there is nothing to rename.
                    let label = ui.label(text);
                    let hover = crate::names::name_hover(name, CRUMB_UNITS, "", str::to_string);
                    if !hover.is_empty() {
                        label.on_hover_text(hover);
                    }
                } else if i + 1 == crumb_count {
                    let is_renaming = self.renaming_group.as_ref().is_some_and(|(rid, _)| rid == id);
                    if is_renaming {
                        if let Some((_, draft)) = &mut self.renaming_group {
                            let response = toolbar_field(ui, |ui| ui.add(field_edit(draft).desired_width(140.0)));
                            if take_focus(&mut self.pending_focus, FocusTarget::GroupRename) {
                                response.request_focus();
                            }
                            if response.lost_focus() {
                                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                    actions.push(GraphAction::Node(*id, NodeAction::CancelRename));
                                } else {
                                    actions.push(GraphAction::Node(*id, NodeAction::CommitRename(draft.clone())));
                                }
                            }
                        }
                    } else if ui
                        .button(text)
                        .on_hover_text(crate::names::name_hover(name, CRUMB_UNITS, "Click to rename (F2)", |f| {
                            format!("Click to rename, F2 ({f})")
                        }))
                        .clicked()
                    {
                        actions.push(GraphAction::Node(*id, NodeAction::Rename));
                    }
                } else {
                    // An ancestor, not the current group itself
                    // -- click to jump straight there (reuses
                    // `GraphAction::EnterGroup`, the exact same "switch
                    // current_group and ease the camera" effect
                    // double-clicking a group card triggers;
                    // it doesn't care which direction that is).
                    if ui
                        .button(text)
                        .on_hover_text(crate::names::name_hover(name, CRUMB_UNITS, "Jump to this group", |f| {
                            format!("Jump to this group ({f})")
                        }))
                        .clicked()
                    {
                        actions.push(GraphAction::EnterGroup(*id));
                    }
                    // `CARET_RIGHT`, not a raw "›" glyph -- same
                    // tofu-box reasoning as the back-one-level
                    // button above.
                    ui.label(egui_phosphor::light::CARET_RIGHT);
                }
            }
            if on_json_level {
                self.render_json_bar(ui);
            }
        }
        leave_group_button_rect
    }

    /// Import / Ungroup All / Collapse chip for the groups at this level.
    fn toolbar_group_chip(&mut self, ui: &mut egui::Ui, groups_at_level: &[NodeId], actions: &mut Vec<GraphAction>) {
        // These three buttons used to carry full text labels
        // ("Import Group...", "Ungroup All", "Collapse Groups"/
        // "Expand Groups") and, together with "+ New Group"/
        // "+ New Tab"/"Reset View", made this toolbar cramped
        // enough to confirm live (text touching the window
        // edge at 1300pt wide). Switched to `egui_phosphor`
        // icons instead of folding into a dropdown -- each
        // stays a one-click action, just narrower, with the
        // old label preserved as a hover tooltip so the action
        // is still discoverable. All three now work at any
        // level, not just the top -- see `App::current_group`'s own
        // doc comment on nesting; each already acts on
        // `self.current_group`/the level's groups (`level_scan`,
        // its own level), never hardcoding the top level.
        //
        // The rule this toolbar actually follows, for any
        // future addition: the identity/entry-point action that
        // defines what this toolbar *is* ("+ New Tab") stays
        // full text; everything else added later to relieve
        // crowding goes icon-only with a hover tooltip carrying
        // the old label, per this paragraph's own history.
        // "Reset View" was the other text holdout until it
        // became the zoom chip's Fit button (hover text keeps
        // the name).
        //
        // Same "real file dialog blocks on OS/human input
        // neither exists in headless mode" reasoning as every
        // other Export/Import button -- `IMPORTGROUP <path>`
        // calls the same underlying import directly instead.
        toolbar_chip(ui, |ui| {
            let mut bounds: Option<egui::Rect> = None;
            let mut grow = |r: egui::Rect| {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            };
            if !self.headless {
                let response =
                    chip_icon_button(ui, egui::Id::new("icon_btn_import_group"), egui_phosphor::light::UPLOAD_SIMPLE)
                        .named_tip("Import Group...");
                grow(response.rect);
                if response.clicked() {
                    actions.push(GraphAction::ImportGroup);
                }
            }
            if !groups_at_level.is_empty() {
                let response =
                    chip_icon_button(ui, egui::Id::new("icon_btn_ungroup_all"), egui_phosphor::light::FOLDER_MINUS)
                        .named_tip(ungroup_all_tip(groups_at_level.len()));
                grow(response.rect);
                if response.clicked() {
                    actions.push(GraphAction::UngroupAll);
                }
            }
            if !groups_at_level.is_empty() {
                let (icon, hover) = if self.groups_collapsed {
                    (egui_phosphor::light::CORNERS_OUT, "Expand Groups")
                } else {
                    (egui_phosphor::light::CORNERS_IN, "Collapse Groups")
                };
                let response = chip_icon_button(ui, egui::Id::new("icon_btn_collapse_groups"), icon).named_tip(hover);
                grow(response.rect);
                if response.clicked() {
                    self.groups_collapsed = !self.groups_collapsed;
                }
            }
            bounds.unwrap_or(ui.cursor())
        });
    }

    /// The zoom chip; what it asks for, if anything.
    fn toolbar_zoom(&self, ui: &mut egui::Ui) -> Option<ZoomChipAction> {
        // −/+/100%/Fit in one chip (`render_zoom_chip`, shared
        // with Image view). Fit is the old "Reset View"
        // (re-fit every node at this level); the `%` readout
        // keeps the current pan and only resets the scale.
        let zoom = self.camera.zoom_pct / 100.0;
        render_zoom_chip(
            ui,
            self.camera.zoom_pct,
            zoom > GRAPH_ZOOM_MIN + 1e-3,
            zoom < GRAPH_ZOOM_MAX - 1e-3,
            "Reset View (fit all)",
        )
    }

    /// The filter box and its clear button (not on a JSON level).
    fn toolbar_filter(&mut self, ui: &mut egui::Ui, on_json_level: bool) {
        // Not on a JSON level: it filters no cards there, and the
        // outline has the level's own search.
        if !on_json_level {
            toolbar_field(ui, |ui| {
                ui.add(field_edit(&mut self.graph_filter).desired_width(150.0).hint_text("Filter..."))
            });
        }
        // Only shown once there's actually something to clear --
        // same `egui_phosphor::light::X` glyph every other close/
        // dismiss affordance in this app already uses, so this
        // reads as "clear" at a glance rather than a new symbol.
        if !on_json_level
            && !self.graph_filter.is_empty()
            && ui.button(egui_phosphor::light::X).on_hover_text("Clear filter").clicked()
        {
            self.graph_filter.clear();
        }
    }

    /// "Recently Closed (n)"; returns its rect for the popup anchor.
    fn toolbar_recently_closed(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        let mut recently_closed_button_rect: Option<egui::Rect> = None;
        let count = self.doc.journal.as_ref().map_or(0, |j| j.doc().closed_roots().len());
        if count > 0 {
            let label = format!("Recently Closed ({count})");
            let response = ui.button(label).on_hover_text("Reopen the last closed tab (Ctrl+Shift+T)");
            self.popup_opener_clicked(ui, AnchoredPopup::RecentlyClosed, &response);
            recently_closed_button_rect = Some(response.rect);
        }
        recently_closed_button_rect
    }

    /// How many cards inside the selected groups (at any depth) are not themselves selected: they close with the
    /// group (`GraphAction::CloseSelected`).
    fn unselected_members_of_selection(&self) -> usize {
        let mut inside = std::collections::HashSet::new();
        let mut frontier: Vec<NodeId> = self.interaction.selected.iter().filter_map(|c| c.node()).collect();
        while let Some(group) = frontier.pop() {
            for member in self.members_of(group) {
                if inside.insert(member) {
                    frontier.push(member);
                }
            }
        }
        inside.iter().filter(|id| !self.interaction.selected.contains(&CardId::from(**id))).count()
    }

    /// Group/Close Selected, shown once a rectangle select has picked something.
    fn toolbar_selection(&self, ui: &mut egui::Ui, actions: &mut Vec<GraphAction>) {
        // Rectangle-select's own action bar: only shown once a
        // right-drag has actually selected something (see
        // `handle_rect_select`).
        // Counts the stored cards: both actions leave a virtual card alone.
        let stored = self.selected_stored_count();
        if stored > 0 {
            // Matches "+ New Group" above: groups the selection
            // into a new subgroup of whichever level is
            // currently being viewed, not just the top.
            if ui.button(format!("Group Selected ({stored})")).clicked() {
                actions.push(GraphAction::GroupSelected);
            }
            let inside = self.unselected_members_of_selection();
            let label = match inside {
                0 => format!("Close Selected ({stored})"),
                n => format!("Close Selected ({stored} + {n} inside)"),
            };
            if ui.button(label).on_hover_text("A group closes with everything in it").clicked() {
                actions.push(GraphAction::CloseSelected);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name rules: crumbs draw hidden characters as placeholders and shorten to 20 units, a placeholder counting one.
    #[test]
    fn breadcrumb_crumbs_follow_the_display_rule() {
        let ctx = crate::theme::testutil::ctx(true);
        let mut app = App::default();
        let (outer, inner) = (app.alloc_node_id(), app.alloc_node_id());
        app.insert_group_at(outer, None);
        app.insert_group_at(inner, None);
        app.edit_group(inner, Some(outer));
        app.doc.mirror.set(
            outer,
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("x".repeat(30)),
        );
        app.doc.mirror.set(
            inner,
            crate::store::model::field::NAME,
            crate::store::model::FieldValue::Str("Re\u{200B}ading".into()),
        );
        app.current_group = Some(inner);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| app.toolbar_breadcrumb(ui, false, &mut Vec::new()));
        });
        output.textures_delta.clear();
        let galleys: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.clone()),
                _ => None,
            })
            .collect();
        let texts: Vec<String> = galleys.iter().map(|g| g.text().to_string()).collect();
        assert!(texts.contains(&format!("{}…", "x".repeat(19))), "{texts:?}");
        let current = galleys.iter().find(|g| g.text() == "Re⟦200B⟧ading").unwrap_or_else(|| panic!("{texts:?}"));
        assert!(current.job.sections.iter().any(|s| s.format.font_id.family == egui::FontFamily::Monospace));
    }

    /// Close Selected names the cards that close with a selected group and are not selected themselves, at any depth.
    #[test]
    fn close_selected_counts_what_a_selected_group_takes_with_it() {
        use crate::fetched::Placement;
        let mut app = App::default();
        let top = Placement { group: None, pos: egui::Vec2::ZERO };
        use crate::card_id::testutil::node;
        let [outer, inner, a, b, c] = [1, 2, 3, 4, 5].map(node);
        app.place_group(Some(outer), "Outer", top);
        app.place_group(Some(inner), "Inner", Placement { group: Some(outer), pos: egui::Vec2::ZERO });
        for (id, group) in [(a, outer), (b, inner), (c, inner)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::Vec2::ZERO, crate::store::model::Kind::Note, note);
            app.edit_group(id, Some(group));
        }
        app.interaction.selected.insert(CardId::from(a));
        assert_eq!(app.unselected_members_of_selection(), 0, "a note holds nothing");
        app.interaction.selected.insert(CardId::from(outer));
        assert_eq!(app.unselected_members_of_selection(), 3, "inner, b and c; a is selected itself");
        app.interaction.selected.insert(CardId::from(c));
        assert_eq!(app.unselected_members_of_selection(), 2, "c is selected too");
    }

    /// The hint is per level: an empty group shows it; a level with a node (a group card counts) does not.
    #[test]
    fn drops_and_ungroup_all_say_what_they_will_do() {
        assert_eq!(DropEffect::Merge.hint("Research"), "Merge into Research");
        assert_eq!(DropEffect::Join.hint("A"), "Move into A");
        assert_eq!(DropEffect::GroupWith.hint("B"), "Group with B");
        assert_eq!(DropEffect::Reparent.hint("C"), "Make child of C");
        assert_eq!(DropEffect::Eject.hint(""), "Move out of group");
        assert_eq!(ungroup_all_tip(1), "Ungroup All (1 group): keeps the members");
        assert_eq!(ungroup_all_tip(3), "Ungroup All (3 groups): keeps the members");
    }
}
