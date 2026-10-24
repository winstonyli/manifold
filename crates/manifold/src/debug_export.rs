//! The `EXPORTSTATE` debug command's JSON: UI state a probe checks and the session store doesn't hold.
//!
//! `EXPORTSTATE path` -- writes UI state that the session store doesn't
//! hold (and shouldn't: it's the user's saved session) as JSON, for a
//! test to check: `tab_view`, `transition`, `focused_url`,
//! `address_bar_text`, `omni_matches` (the ids of the cards the Graph omnibar's text matches, page text included), `suggestions` (the omnibar dropdown's rows: `tab:<id>` for an open
//! tab, `setting:<id>`, a history url, or the text a search or the typed row acts on), `highlighted` (a row
//! index, or null) and `current_group` (the Graph view level's group
//! id, or null at the top). `cards` lists the current level's cards in
//! draw order, each as `id`, `type` (the card's mime type, e.g.
//! `application/vnd.manifold.tab` or `text/markdown`; `store::model`'s
//! `TYPE_*`), `stored_type` (the type string as stored: the same but for an `image/*` other than `image/png`, a
//! different case, or a type this build doesn't know, whose `type` is `TYPE_UNKNOWN`), `loaded` (true for a tab with a running engine only),
//! `json_card` (true for a JSON level's card, which is no stored node:
//! its keys are only `id`, `type` (`application/json`), `loaded` (false), `unresponsive` (false),
//! `json_card`, `screen` and `rect`), `group`, `parent`, `color` (whether it has a tag color),
//! `pinned`, `editing` (a note being edited), `screen` (its center) and
//! `rect` (`[left, top, right, bottom]`), in points, which are pixels
//! headless. `gate` reads these for the probe
//! scripts. `selected` is the rectangle
//! selection's ids, sorted. `mirror_groups` maps every node id (any
//! level) to its group as this replica shows it (the mirror), or null.
//! `mirror_names` (groups), `mirror_texts` (notes) and `mirror_pos` (every node, `[x, y]`) map ids to the
//! mirror's name, text and document pos the same way, null where EXPORTSESSION leaves the field out (empty, or
//! pos (0, 0)).
//! `links` lists every Link node as `{id, from, to}`. `edges` lists the lines drawn at the current level as
//! `{kind: link|opened, link (the Link node id, or null), from, to, screen: [x, y], near_from, near_to}`, where
//! `screen` is the point halfway along the curve (t = 0.5) between the two cards' facing sides and `near_from`
//! and `near_to` are the points at t = 0.2 and 0.8, where a probe grabs a link's `from` or `to` end.
//! `retarget` is the Link node whose end a connect-drag from its curve is moving, or null. `connecting` is pick-target mode as
//! `{from, edge, targets}` (`targets` sorted), or null. `link_handles` maps each card that would show connect-drag
//! handles (every card when nothing else owns the pointer; every card during a connect-drag) to
//! `{top, right, bottom, left}` (those shown at this zoom), each `[left, top, right, bottom]` on screen.
//! `connect_drag` is the source card of a running connect-drag, or null.
//! `view_kinds` maps each engine name to `{state, disk}`: the state label the engine lists show and what the disk
//! holds of its downloadable parts ("host ready", for CEF "host ready, runtime missing").
//! `toast` is the bottom-left toast's text
//! (the one showing, else the next queued), or null. Suggestions arriving from the network show up
//! only once a frame has drained them, and every command draws one, so
//! a test can poll this until `suggestions` fills.
//! `window` is the window's `[width, height]` in points; `popups` lists the visible menus, dialogs, dropdowns and
//! toasts as `{id, order, rect}` (see `App::popups_state`).
//! `trail` maps each tab with a trail to `{entries, depth, url, back_fork, forward_fork, lin_at, lin_len,
//! left_to, back, forward}` (`App::trail_state`): `depth` is 0 at the trail's first page, `back`/`forward` the
//! chips' enablement.
//! The file is replaced atomically (`store::atomic::replace`), so a test may read it as soon as it exists.

use super::*;

impl App {
    /// What `EXPORTSTATE` writes (see this module's doc).
    pub(crate) fn export_state(&self) -> serde_json::Value {
        let focused_url =
            self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.url.clone());
        // The inline omnibar's rows while its dropdown is open, each as what Enter on it would act on.
        let suggestions: Vec<String> = match self.omni.host.filter(|_| self.omni.open) {
            Some(host) => self
                .omni_rows(host)
                .into_iter()
                .map(|row| match row {
                    crate::omnibar::Row::Card { id, .. } => format!("tab:{id}"),
                    crate::omnibar::Row::Setting { id, .. } => format!("setting:{id}"),
                    crate::omnibar::Row::History { url, .. } => url,
                    crate::omnibar::Row::Command(command) => format!("command:{}", command.title),
                    crate::omnibar::Row::Typed { text } | crate::omnibar::Row::Suggestion { text } => text,
                })
                .collect(),
            None => Vec::new(),
        };
        // The cards the Graph omnibar's text matches (title, url, page text...), by id; none while it has no text.
        let omni_matches: Vec<String> = match self.omni_filter().trim() {
            "" => Vec::new(),
            filter => {
                let mut ids: Vec<NodeId> =
                    self.nodes.keys().copied().filter(|&id| self.node_matches(filter, id)).collect();
                ids.sort();
                ids.into_iter().map(|id| id.to_string()).collect()
            }
        };
        // The current level's cards, in draw order, with where each
        // one's center is on screen: what a headless test needs to
        // aim a click or drag at a card. A card is named by its type
        // (`CardKind`), never a kind enum (plan 2b-6, Ruling 9).
        let now = std::time::Instant::now();
        let window = self.egui_ctx.content_rect();
        let visuals = self.egui_ctx.global_style().visuals.clone();
        let cards: Vec<serde_json::Value> = self
            .level_items()
            .into_iter()
            .filter_map(|item| {
                let screen = self.graph_world_rect_to_screen(item.rect);
                if item.origin == Origin::Json {
                    // A JSON level's card shows a piece of the Json node's value; it is no stored node.
                    return Some(serde_json::json!({
                        "id": item.id,
                        "type": crate::store::model::TYPE_JSON,
                        "loaded": false,
                        "json_card": true,
                        "unresponsive": false,
                        "screen": [screen.center().x, screen.center().y],
                        "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                    }));
                }
                if let Origin::Virtual(source) = item.origin {
                    // A source's card: no node, so none of a stored card's fields; the abilities its kind grants.
                    let caps = item.caps();
                    return Some(serde_json::json!({
                        "id": item.id,
                        "type": item.kind.ty,
                        "loaded": false,
                        "json_card": false,
                        "virtual": source,
                        "abilities": {
                            "restorable": caps.restorable,
                            "draw_layer": caps.draw_layer,
                            "selectable": caps.selectable,
                            "drag_source": caps.drag_source,
                            "drop_target": caps.drop_target,
                            "menu": caps.menu,
                            "stored_controls": caps.stored_controls,
                        },
                        "unresponsive": false,
                        "screen": [screen.center().x, screen.center().y],
                        "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                    }));
                }
                let kind = item.kind;
                let id = item.id.node()?;
                let node = self.nodes.get(&id)?;
                Some(serde_json::json!({
                    "id": item.id,
                    "type": kind.ty,
                    "stored_type": self.doc.mirror.ty(id), // the raw string; `type` is its row (`canonical_type`)
                    "loaded": kind.loaded(),
                    "json_card": false,
                    "group": self.doc.mirror.group(id),
                    "parent": self.opened_from(id),
                    "color": self.doc.mirror.color(id).is_some(),
                    "pinned": self.doc.mirror.pinned(id),
                    "name": self.doc.mirror.name(id), // name rule: reported as stored
                    // The tab's or group's own jar edge, the jar it resolves to, and whether a jar card is the Default.
                    "jar": self.doc.mirror.jar(id),
                    "effective_jar": self.effective_jar(id).map(|(j, _)| j),
                    "is_default": self.is_default_jar(id),
                    // A live tab whose engine stopped reading (`ViewTab::is_unresponsive`).
                    "unresponsive": node.as_tab().is_some_and(|t| t.tab.is_unresponsive(now)),
                    "editing": node.as_note().is_some_and(|n| n.editing),
                    // Where MOVENODE puts the card back after it was parked (gate layout).
                    "world": [self.layout_pos(id).x, self.layout_pos(id).y],
                    "screen": [screen.center().x, screen.center().y],
                    "rect": [screen.min.x, screen.min.y, screen.max.x, screen.max.y],
                }))
            })
            .collect();
        // Every node's group as this replica shows it, at any level (a two-instance test compares them).
        let mirror_groups: std::collections::BTreeMap<NodeId, Option<NodeId>> =
            self.linked_ids().map(|id| (id, self.doc.mirror.group(id))).collect();
        // Likewise each group's name, each note's text and every node's document pos, null when absent
        // (empty, or Pos (0,0)), as EXPORTSESSION leaves them out.
        let non_empty = |s: &'_ str| (!s.is_empty()).then(|| s.to_owned());
        let mirror_names: std::collections::BTreeMap<NodeId, Option<String>> = self
            .linked_ids()
            .filter(|id| self.is_container(*id))
            .map(|id| (id, non_empty(self.doc.mirror.name(id)))) // name rule: raw, compared with the document
            .collect();
        let mirror_texts: std::collections::BTreeMap<NodeId, Option<String>> = self
            .linked_ids()
            .filter(|id| self.nodes.get(id).is_some_and(|n| n.as_note().is_some()))
            .map(|id| (id, non_empty(self.doc.mirror.text(id))))
            .collect();
        let mirror_pos: std::collections::BTreeMap<NodeId, Option<[f32; 2]>> =
            self.linked_ids().map(|id| (id, self.doc.mirror.pos(id).map(|p| [p.x, p.y]))).collect();
        // Sorted by text, as when ids were text, so the export's order is the one the gate has always read.
        let mut selected: Vec<String> = self.interaction.selected.iter().map(|id| id.to_string()).collect();
        selected.sort();
        // Every link this replica knows (ends null where unset), whether or not it is shown. `shown` is true when
        // both ends are in `nodes`, so a link whose end is hidden (closed) reads false and a deleted one is absent.
        let links: Vec<serde_json::Value> = self
            .doc
            .mirror
            .link_ids()
            .map(|link| {
                let (from, to) = self.doc.mirror.link_ends(link);
                let shown = [from, to].iter().all(|e| e.is_some_and(|n| self.nodes.contains_key(&n)));
                serde_json::json!({
                    "id": link, "from": from, "to": to, "shown": shown,
                })
            })
            .collect();
        // Recently Closed as the document orders it, newest first (`Doc::closed_roots`).
        let closed: Vec<String> = self
            .doc
            .journal
            .as_ref()
            .map_or_else(Vec::new, |j| j.doc().closed_roots().into_iter().map(|(id, _)| id.to_string()).collect());
        // The lines drawn at this level, with the point halfway along each curve (t = 0.5) on screen: where a probe
        // clicks a link line.
        let edges: Vec<serde_json::Value> = self
            .level_edges()
            .iter()
            .map(|edge| {
                let on_screen = |t: f32| {
                    let world = match edge.curve {
                        crate::level::EdgeCurve::Bow => crate::link_gestures::bow_point(edge.from, edge.to, t),
                        crate::level::EdgeCurve::Horizontal => edge.from.lerp(edge.to, t),
                    };
                    self.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min
                };
                let (mid, near_from, near_to) = (on_screen(0.5), on_screen(0.2), on_screen(0.8));
                let kind = match edge.kind {
                    crate::level::EdgeKind::Link => "link",
                    crate::level::EdgeKind::Jar => "jar",
                    crate::level::EdgeKind::Opened => "opened",
                };
                serde_json::json!({
                    "kind": kind, "link": edge.link, "from": edge.from_id, "to": edge.to_id, "screen": [mid.x, mid.y],
                    "near_from": [near_from.x, near_from.y], "near_to": [near_to.x, near_to.y],
                })
            })
            .collect();
        // Where each card has connect handles when it would show them (on hover, and on every card during a
        // connect-drag), on screen, so a probe can press one:
        // `{card id: {top|right|bottom|left: [left, top, right, bottom]}}`. Empty when nothing shows them.
        let (zoom_scale, _) = self.camera.transform(self.graph_scene_outer_rect());
        let link_handles: serde_json::Map<String, serde_json::Value> = self
            .level_items()
            .into_iter()
            .filter(|item| item.origin == Origin::Stored && self.shows_link_handles())
            .map(|item| {
                let sides: serde_json::Map<String, serde_json::Value> =
                    crate::link_gestures::link_handle_rects(item.rect, zoom_scale, self.graph_detail)
                        .into_iter()
                        .map(|(side, rect)| {
                            let r = self.graph_world_rect_to_screen(rect);
                            (side.key().to_string(), serde_json::json!([r.left(), r.top(), r.right(), r.bottom()]))
                        })
                        .collect();
                (item.id.to_string(), serde_json::Value::Object(sides))
            })
            .collect();
        serde_json::json!({
            "tab_view": self.showing_tab_view(),
            "transition": self.transition.is_some(),
            "focused_url": focused_url,
            "address_bar_text": self.omni.text,
            "suggestions": suggestions,
            "omni_matches": omni_matches,
            "group_suggestions": self.suggestions().iter().map(|s| serde_json::json!({"label": s.label, "cards": s.cards})).collect::<Vec<_>>(),
            "highlighted": self.omni.highlighted,
            "current_group": self.current_group,
            "cards": cards,
            "mirror_groups": mirror_groups,
            "mirror_names": mirror_names,
            "mirror_texts": mirror_texts,
            "mirror_pos": mirror_pos,
            "links": links,
            "closed": closed,
            "edges": edges,
            // Pick-target mode: the source card, the edge and the cards a click would finish it on.
            "connecting": self.link.connecting.as_ref().map(|(from, edge)| {
                serde_json::json!({
                    "from": from,
                    "edge": edge,
                    "targets": self.valid_targets(edge, *from).iter().map(NodeId::to_string).collect::<Vec<_>>(),
                })
            }),
            "link_handles": link_handles,
            "connect_drag": self.link.connect_drag,
            // The Link node whose end a connect-drag from a curve is moving (`App::retarget`), or null.
            "retarget": self.link.retarget.as_ref().map(|r| r.link),
            // While it runs, what a release at the pointer would do (`App::retarget_preview`): "retarget", "delete",
            // "refused" or "cancel"; else null.
            "retarget_preview": self.link.retarget.as_ref().and(self.link.retarget_preview).map(crate::links::RetargetOutcome::name),
            // The card being dragged (`App::dragging`), or null.
            "dragging": self.interaction.dragging,
            "json_level": self.json_level_state(),
            "selected": selected,
            // The toast showing, or else the next queued one.
            "toast": self.toasts.save_toast.as_ref().map(|(message, _)| message).or(self.toasts.save_toast_queue.front()),
            "view_kinds": self.view_kinds_state(),
            "window": [window.width(), window.height()],
            "popups": self.popups_state(),
            // The theme colors the gate checks on screen, as [r, g, b]: the gate reads them here, not from its own copy.
            "theme": {"danger": crate::theme::rgb(crate::theme::danger(&visuals)), "accent": crate::theme::rgb(crate::theme::accent(&visuals))},
            "rows": crate::identity::drawn_rows(&self.egui_ctx).iter().map(|r| serde_json::json!({"text": r.text, "selected": r.selected, "rect": [r.rect.min.x, r.rect.min.y, r.rect.max.x, r.rect.max.y]})).collect::<Vec<_>>(),
            "open_popup": self.menus.open_popup.map(|p| format!("{p:?}")),
            "identities": crate::identity::drawn_icons(&self.egui_ctx).iter().map(|i| serde_json::json!({"kind": i.kind, "text": i.text, "rect": [i.rect.min.x, i.rect.min.y, i.rect.max.x, i.rect.max.y]})).collect::<Vec<_>>(),
            "trail": self.trail_state(),
        })
    }

    /// `export_state`'s `trail`: each tab with a trail, by id, as `NavHistory::export` plus `back` and `forward`, the
    /// chips' enablement (`trail_enabled`).
    fn trail_state(&self) -> serde_json::Map<String, serde_json::Value> {
        self.nav_history
            .iter()
            .map(|(id, h)| {
                let mut v = h.export();
                let (back, forward) = self.trail_enabled(*id);
                v["back"] = back.into();
                v["forward"] = forward.into();
                (id.to_string(), v)
            })
            .collect()
    }

    /// `export_state`'s `popups`: every visible egui `Area` above the panels (menus, dialogs, dropdowns, toasts,
    /// tooltips), as `{id, order, rect: [left, top, right, bottom]}` in points, so a layout check can see one run past
    /// the window.
    fn popups_state(&self) -> Vec<serde_json::Value> {
        self.egui_ctx.memory(|m| {
            let mut layers: Vec<_> = m
                .areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| matches!(layer.order, egui::Order::Foreground | egui::Order::Tooltip))
                .filter_map(|layer| m.area_rect(layer.id).map(|rect| (layer, rect)))
                .map(|(layer, rect)| {
                    serde_json::json!({
                        "id": format!("{:?}", layer.id),
                        "order": format!("{:?}", layer.order),
                        "rect": [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
                    })
                })
                .collect();
            layers.sort_by_key(|v| v["id"].to_string());
            layers
        })
    }

    /// `export_state`'s `view_kinds`: each engine's name mapped to `{state, disk}`, the label the lists show
    /// (`view_status_label`) and what the disk holds of its downloadable parts (`on_disk_text`).
    fn view_kinds_state(&self) -> serde_json::Value {
        let view_kinds = ViewKind::picker_order().into_iter().map(|kind| {
            let state = crate::view_info::view_status_label(&self.view_status(kind)).0;
            let disk = crate::view_info::on_disk_text(kind, crate::process_manager::view_on_disk(kind));
            (kind.desc().name.to_string(), serde_json::json!({"state": state, "disk": disk}))
        });
        serde_json::Value::Object(view_kinds.collect())
    }

    /// `EXPORTSTATE`: writes `export_state` to `path` through `store::atomic::replace`, so a probe never reads
    /// a half-written file. A failure is logged.
    pub(crate) fn write_export_state(&self, path: &std::path::Path) {
        if let Err(err) = crate::store::atomic::replace(path, self.export_state().to_string().as_bytes()) {
            eprintln!("EXPORTSTATE: couldn't write {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_id::testutil::node;

    /// `EXPORTSTATE` names a card by its type (plan 2b-6, Ruling 9): `type` (the mime), `loaded` (true for a live tab
    /// only) and `json_card` (a JSON level's card, not a stored node); the old card enum's `kind` is gone.
    /// `view_kinds` has every engine, each with its state label and what the disk holds (no data dir under test, so
    /// the downloaded engines read as not installed with nothing pinned).
    #[test]
    fn export_state_lists_each_view_kinds_state_and_disk() {
        let state = App::default().export_state();
        let view_kinds = state["view_kinds"].as_object().unwrap();
        assert_eq!(view_kinds.len(), 4);
        assert_eq!(view_kinds["webview2"]["disk"], "host ready");
        assert_eq!(view_kinds["cef"]["disk"], "host missing, runtime missing");
        assert!(view_kinds["servo"]["state"].as_str().unwrap().starts_with("Unavailable"), "{view_kinds:?}");
    }

    /// `window` is the window's size and `popups` the visible Foreground layers' rects (`gate layout` reads both).
    #[test]
    fn export_state_gives_the_theme_colors() {
        let state = App::default().export_state();
        assert_eq!(state["theme"]["danger"], serde_json::json!(manifold_pages::palette::DARK.danger));
        assert_eq!(state["theme"]["accent"], serde_json::json!(manifold_pages::palette::DARK.accent));
    }

    #[test]
    fn export_state_gives_the_window_and_each_foreground_popup() {
        let app = App::default();
        let mut output = app.egui_ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Area::new(egui::Id::new("probe_popup")).order(egui::Order::Foreground).fixed_pos([10.0, 20.0]).show(
                ui.ctx(),
                |ui| {
                    ui.label("x");
                },
            );
        });
        output.textures_delta.clear();
        let state = app.export_state();
        assert_eq!(state["window"].as_array().map(Vec::len), Some(2));
        let popups = state["popups"].as_array().unwrap();
        let probe = popups.iter().find(|p| p["id"].as_str().is_some_and(|id| id.contains("probe_popup"))).unwrap();
        assert_eq!(probe["rect"][0], 10.0);
        assert_eq!(probe["rect"][1], 20.0);
    }

    /// `trail` gives each tab's trail and the chips' enablement (`gate trail` reads it).
    #[test]
    fn export_state_gives_each_trail() {
        let mut app = App::default();
        app.nav_history.insert(node(1), crate::nav_history::NavHistory::visited(&["a", "b"]));
        let state = app.export_state();
        let trail = &state["trail"][node(1).to_string()];
        assert_eq!(
            (trail["entries"].as_u64(), trail["depth"].as_u64(), trail["url"].as_str()),
            (Some(2), Some(1), Some("b"))
        );
        assert_eq!(
            (trail["back"].as_bool(), trail["forward"].as_bool(), trail["back_fork"].as_bool()),
            (Some(true), Some(false), Some(false))
        );
        assert_eq!(
            (trail["lin_at"].as_u64(), trail["lin_len"].as_u64(), trail["left_to"].is_null()),
            (Some(1), Some(2), true)
        );
    }

    #[test]
    fn export_state_names_cards_by_type_loaded_and_json_card() {
        use crate::node::{NotePayload, TabPayload, UnloadedState, UnloadedTabPayload};
        use crate::process_manager::{ViewKind, ViewTab};
        use crate::store::model::{Kind, TYPE_GROUP, TYPE_JSON, TYPE_NOTE, TYPE_TAB};
        let (live_id, asleep_id, note_id, group_id, json_id) = (node(1), node(2), node(3), node(4), node(5));
        let dir = crate::store::testutil::temp_dir("export_state");
        let path = dir.join("a.json");
        std::fs::write(&path, r#"{"a": {"b": 1}}"#).unwrap();
        let mut app = App::default();
        let (tab, _peer) = ViewTab::fake(ViewKind::Servo);
        let live = Runtime::Tab(TabPayload {
            tab,
            url: "https://a/".into(),
            title: String::new(),
            can_go_back: false,
            can_go_forward: false,
            loading: false,
            audio_playing: false,
            muted: false,
            zoom_factor: 1.0,
            swap_target: None,
        });
        let asleep = Runtime::Unloaded(UnloadedTabPayload {
            url: "https://s/".into(),
            title: String::new(),
            muted: false,
            zoom_factor: 1.0,
            state: UnloadedState::Sleeping,
        });
        app.test_node(live_id, Kind::Tab, live);
        app.test_node(asleep_id, Kind::Tab, asleep);
        app.test_node(note_id, Kind::Note, Runtime::Note(NotePayload { text: "t".into(), editing: false }));
        app.test_node(group_id, Kind::Group, Runtime::None);
        app.insert_json_node_with_id(json_id, path.to_string_lossy().into_owned(), egui::Vec2::ZERO, None).unwrap();
        let cards = |app: &App| -> std::collections::BTreeMap<String, serde_json::Value> {
            let state = app.export_state();
            let cards = state["cards"].as_array().expect("cards is a list");
            cards.iter().map(|c| (c["id"].as_str().unwrap().to_owned(), c.clone())).collect()
        };

        let top = cards(&app);
        assert_eq!(top.len(), 5, "{top:?}");
        for (id, ty, loaded) in [
            (live_id, TYPE_TAB, true),
            (asleep_id, TYPE_TAB, false),
            (note_id, TYPE_NOTE, false),
            (group_id, TYPE_GROUP, false),
            (json_id, TYPE_JSON, false),
        ] {
            let card = &top[&id.to_string()];
            assert_eq!(card["type"], ty, "{id}: {card}");
            assert_eq!(card["loaded"], loaded, "{id}: {card}");
            assert_eq!(card["json_card"], false, "{id}: {card}");
            assert!(card.get("kind").is_none(), "{id}: {card}");
            for key in [
                "group",
                "parent",
                "color",
                "pinned",
                "editing",
                "world",
                "screen",
                "rect",
                "jar",
                "effective_jar",
                "is_default",
                "stored_type",
            ] {
                assert!(card.get(key).is_some(), "{id} keeps {key}: {card}");
            }
        }

        // A JSON level's cards are not stored nodes: `json_card`, typed as the JSON they show.
        app.switch_group_level(Some(json_id));
        let level = cards(&app);
        let json_card = |pointer: &str| {
            use crate::card_id::{fnv64, CardId, JSON_SOURCE};
            CardId::virt(JSON_SOURCE, fnv64(&[&json_id.to_string(), "#", pointer])).to_string()
        };
        let mut want = [json_card(""), json_card("/a")];
        want.sort();
        assert_eq!(level.keys().cloned().collect::<Vec<_>>(), want);
        for card in level.values() {
            assert_eq!(card["json_card"], true, "{card}");
            assert_eq!(card["type"], TYPE_JSON, "{card}");
            assert_eq!(card["loaded"], false, "{card}");
            assert!(card.get("kind").is_none(), "{card}");
            for key in ["screen", "rect"] {
                assert!(card.get(key).is_some(), "{key}: {card}");
            }
        }
        app.switch_group_level(None);
        assert_eq!(cards(&app)[&live_id.to_string()]["unresponsive"], serde_json::json!(false));
        app.nodes.get(&live_id).and_then(|n| n.as_tab()).unwrap().tab.wedge();
        assert_eq!(cards(&app)[&live_id.to_string()]["unresponsive"], serde_json::json!(true), "its queue filled");
    }

    /// EXPORTSTATE's `edges`: each line at this level with its kind, Link node, ends and on-screen middle.
    #[test]
    fn export_state_lists_the_level_edges_with_their_link() {
        use crate::node::NotePayload;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(a, 0.0), (b, 400.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::vec2(x, 0.0), Kind::Note, note);
        }
        let link = app.create_link(a, b);
        let state = app.export_state();
        let edges = state["edges"].as_array().expect("an array");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["kind"], "link");
        assert_eq!(edges[0]["link"], link.to_string());
        assert_eq!(
            (edges[0]["from"].as_str(), edges[0]["to"].as_str()),
            (Some(a.to_string().as_str()), Some(b.to_string().as_str()))
        );
        // a's right side to b's left side, bowed: the curve's t = 0.5 point, not the straight segment's middle.
        let (ra, rb) = (app.card_rect(a).unwrap(), app.card_rect(b).unwrap());
        let (from, to) = (ra.right_center(), rb.left_center());
        let world = crate::link_gestures::bow_point(from, to, 0.5);
        assert!((world - from.lerp(to, 0.5)).length() > 1.0, "the bow moves the middle off the chord");
        let mid = app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min;
        let screen: Vec<f64> = edges[0]["screen"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((screen[0] as f32 - mid.x).abs() < 0.01 && (screen[1] as f32 - mid.y).abs() < 0.01, "{screen:?}");
        for (key, t) in [("near_from", 0.2), ("near_to", 0.8)] {
            let world = crate::link_gestures::bow_point(from, to, t);
            let want = app.graph_world_rect_to_screen(egui::Rect::from_min_max(world, world)).min;
            let got: Vec<f64> = edges[0][key].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            assert!((got[0] as f32 - want.x).abs() < 0.01 && (got[1] as f32 - want.y).abs() < 0.01, "{key}: {got:?}");
        }
        assert!(state["retarget"].is_null());
        app.start_connect_drag(a);
        app.link.retarget = Some(crate::link_gestures::LinkRetarget { link, fixed_is_from: true });
        assert_eq!(app.export_state()["retarget"], link.to_string());
    }

    /// EXPORTSTATE's `link_handles`: each card that would show connect handles, each side's on-screen rect.
    #[test]
    fn export_state_gives_each_linkable_card_its_handle_rects() {
        use crate::node::NotePayload;
        use crate::store::model::Kind;
        let mut app = App::default();
        let (a, b) = (app.alloc_node_id(), app.alloc_node_id());
        for (id, x) in [(a, 0.0), (b, 400.0)] {
            let note = Runtime::Note(NotePayload { text: "n".into(), editing: false });
            app.insert_placeholder_node(id, egui::vec2(x, 0.0), Kind::Note, note);
        }
        let state = app.export_state();
        let handles = state["link_handles"][a.to_string()].as_object().expect("a has handles");
        assert_eq!(handles.keys().map(String::as_str).collect::<Vec<_>>(), ["top", "right", "bottom", "left"]);
        let r: Vec<f32> = handles["right"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let card = app.graph_world_rect_to_screen(app.card_rect(a).unwrap());
        assert!((egui::pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0) - card.right_center()).length() < 0.01);
        assert!((r[2] - r[0] - crate::link_gestures::HANDLE_SHORT_PX).abs() < 0.01, "16 points across the edge");
        assert!((r[3] - r[1] - crate::link_gestures::HANDLE_LONG_PX).abs() < 0.01, "28 points along it");
        assert!(state["connect_drag"].is_null() && state["dragging"].is_null());
        app.start_connect_drag(a);
        let state = app.export_state();
        assert_eq!(state["connect_drag"], a.to_string());
        assert!(state["link_handles"].get(b.to_string()).is_some(), "during a connect-drag every card has them");
        app.cancel_connect();
        app.interaction.dragging = Some(crate::card_id::CardId::from(b));
        assert_eq!(app.export_state()["dragging"], b.to_string());
    }

    /// EXPORTSTATE writes through `store::atomic::replace`, so a probe can read the file as soon as it exists:
    /// a blocked target stays unwritten (a plain write would ignore the block).
    #[test]
    fn export_state_is_written_through_the_atomic_replace() {
        let path = crate::store::testutil::temp_dir("export-state-atomic").join("state.json");
        let app = App::default();
        crate::store::atomic::block_for_test(&path, true);
        app.write_export_state(&path);
        assert!(!path.exists(), "the write went around store::atomic::replace");
        crate::store::atomic::block_for_test(&path, false);
        app.write_export_state(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), app.export_state().to_string());
    }
}
