//! The `EXPORTSTATE` debug command's JSON: UI state a probe checks and the session store doesn't hold
//! (and shouldn't: it's the user's saved session). `gate` reads it for the probe scripts.
//!
//! Rects are `[left, top, right, bottom]` and points `[x, y]`, in points, which are pixels headless. Node ids are
//! written as strings, sorted where a list is a set (`selected`, `connecting.targets`). Suggestions arriving from
//! the network show up only once a frame has drained them, and every command draws one, so a test can poll
//! `suggestions` until it fills.
//!
//! - `cards`: the current level's cards in draw order, each with `type` (the mime type; `stored_type` is the raw
//!   stored string, `TYPE_UNKNOWN` in `type` for one this build doesn't know), `loaded` (a running engine only),
//!   `json_card` (a JSON level's card, which is no stored node and has only `id`, `type`, `loaded`, `unresponsive`,
//!   `json_card`, `screen` and `rect`), `screen` (its center) and `rect`.
//! - `mirror_*`: the replica's group, name, text and pos per node id, null where EXPORTSESSION leaves the field out.
//! - `edges`: `screen` is the point halfway along the curve (t = 0.5); `near_from` and `near_to` are t = 0.2 and
//!   0.8, where a probe grabs a link's end.
//! - `trail`: per tab, `NavHistory::export` plus the chips' enablement (`App::trail_state`).
//!
//! The file is replaced atomically (`store::atomic::replace`), so a test may read it as soon as it exists.

use crate::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// `[left, top, right, bottom]`.
fn rect_json(r: egui::Rect) -> Value {
    json!([r.min.x, r.min.y, r.max.x, r.max.y])
}

/// `[x, y]`.
fn point_json(p: egui::Pos2) -> Value {
    json!([p.x, p.y])
}

impl App {
    /// What `EXPORTSTATE` writes (see this module's doc). Built a section at a time, so no `json!` literal nears
    /// the macro's recursion limit.
    pub(crate) fn export_state(&self) -> Value {
        let sections = [
            self.focus_state(),
            self.omnibar_state(),
            json!({"cards": self.cards_state()}),
            self.mirror_state(),
            self.links_state(),
            self.window_state(),
            self.drawn_state(),
            json!({"labels": self.labels_json()}),
            json!({"right_panel": self.right_panel_state()}),
        ];
        let mut state = Map::new();
        for section in sections {
            let Value::Object(keys) = section else { unreachable!("a section is a JSON object") };
            state.extend(keys);
        }
        Value::Object(state)
    }

    /// `export_state`'s `tab_view`, `transition` and `focused_url`.
    fn focus_state(&self) -> Value {
        let focused_url =
            self.focused.as_ref().and_then(|id| self.nodes.get(id)).and_then(|n| n.as_tab()).map(|t| t.url.clone());
        json!({
            "tab_view": self.showing_tab_view(),
            "transition": self.transition.is_some(),
            "focused_url": focused_url,
        })
    }

    /// `export_state`'s omnibar keys, and the Graph level it filters.
    fn omnibar_state(&self) -> Value {
        // The inline omnibar's rows while its dropdown is open, each as what Enter on it would act on.
        let suggestions: Vec<String> = match self.omni.host.filter(|_| self.omni.open) {
            Some(host) => self
                .omni_rows(host)
                .into_iter()
                .map(|row| match row {
                    crate::omnibar::Row::Card { id, .. } => format!("tab:{id}"),
                    crate::omnibar::Row::Other { id, .. } => format!("card:{id}"),
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
        json!({
            "address_bar_text": self.omni.text,
            "suggestions": suggestions,
            "omni_matches": omni_matches,
            "group_suggestions": self.suggestions().iter().map(|s| json!({"label": s.label, "cards": s.cards})).collect::<Vec<_>>(),
            "highlighted": self.omni.highlighted,
            "current_group": self.current_group,
        })
    }

    /// `export_state`'s `cards`: the current level's cards, in draw order, with where each
    /// one's center is on screen: what a headless test needs to
    /// aim a click or drag at a card. A card is named by its type
    /// (`CardKind`), never a kind enum.
    fn cards_state(&self) -> Vec<Value> {
        let now = std::time::Instant::now();
        self.level_items()
            .into_iter()
            .filter_map(|item| {
                let screen = self.graph_world_rect_to_screen(item.rect);
                if item.origin == Origin::Json {
                    // A JSON level's card shows a piece of the Json node's value; it is no stored node.
                    return Some(json!({
                        "id": item.id,
                        "type": crate::store::model::TYPE_JSON,
                        "loaded": false,
                        "json_card": true,
                        "unresponsive": false,
                        "screen": point_json(screen.center()),
                        "rect": rect_json(screen),
                    }));
                }
                if let Origin::Virtual(source) = item.origin {
                    // A source's card: no node, so none of a stored card's fields; the abilities its kind grants.
                    let caps = item.caps();
                    return Some(json!({
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
                        "screen": point_json(screen.center()),
                        "rect": rect_json(screen),
                    }));
                }
                let kind = item.kind;
                let id = item.id.node()?;
                let node = self.nodes.get(&id)?;
                let world = self.layout_pos(id);
                Some(json!({
                    "id": item.id,
                    "type": kind.ty,
                    "stored_type": self.doc.mirror.ty(id), // the raw string; `type` is its row (`canonical_type`)
                    "loaded": kind.loaded(),
                    "json_card": false,
                    "group": self.doc.mirror.group(id),
                    "parent": self.opened_from(id),
                    "pinned": self.doc.mirror.pinned(id),
                    "labels": self.label_names(id), // name rule: reported as stored
                    // Whether the card draws a tag colour on its border (a jar's own colour, or its first label's).
                    "tag_color": self.card_view(&item, &HashMap::new()).is_some_and(|v| v.color.is_some()),
                    "name": self.doc.mirror.name(id), // name rule: reported as stored
                    // The tab's or group's own jar edge, the jar it resolves to, and whether a jar card is the Default.
                    "jar": self.doc.mirror.jar(id),
                    "effective_jar": self.effective_jar(id).map(|(j, _)| j),
                    "is_default": self.is_default_jar(id),
                    // A live tab whose engine stopped reading (`ViewTab::is_unresponsive`).
                    "unresponsive": node.as_tab().is_some_and(|t| t.tab.is_unresponsive(now)),
                    "editing": node.as_note().is_some_and(|n| n.editing),
                    // Why a tab's engine couldn't start, null if it could.
                    "failed": node.as_unloaded().and_then(|t| match &t.state {
                        UnloadedState::Failed(error) => Some(error),
                        _ => None,
                    }),
                    // A download card's progress and outcome, as its body draws them.
                    "download": node.as_download().map(|d| json!({
                        "url": self.doc.mirror.url(id),
                        "path": d.path,
                        "bytes": d.bytes_received,
                        "total": d.total_bytes,
                        "completed": d.is_completed(),
                        "failed": d.failure().is_some(),
                        // Why it failed: the kind's name, as the gate checks it.
                        "failure": d.failure().map(|f| format!("{:?}", f.kind)),
                    })),
                    // Where MOVENODE puts the card back after it was parked (gate layout).
                    "world": [world.x, world.y],
                    "screen": point_json(screen.center()),
                    "rect": rect_json(screen),
                }))
            })
            .collect()
    }

    /// `export_state`'s `mirror_*` keys. Every node's group as this replica shows it, at any level (a two-instance
    /// test compares them); likewise each group's name, each note's text and every node's document pos, null when
    /// absent (empty, or an unplaced pos), as EXPORTSESSION leaves them out, and every node's anchor (`{pos, group,
    /// explicit}`, null when none), which EXPORTSESSION never carries.
    fn mirror_state(&self) -> Value {
        let mirror_groups: BTreeMap<NodeId, Option<NodeId>> =
            self.linked_ids().map(|id| (id, self.doc.mirror.group(id))).collect();
        let non_empty = |s: &'_ str| (!s.is_empty()).then(|| s.to_owned());
        let mirror_names: BTreeMap<NodeId, Option<String>> = self
            .linked_ids()
            .filter(|id| self.is_container(*id))
            .map(|id| (id, non_empty(self.doc.mirror.name(id)))) // name rule: raw, compared with the document
            .collect();
        let mirror_texts: BTreeMap<NodeId, Option<String>> = self
            .linked_ids()
            .filter(|id| self.nodes.get(id).is_some_and(|n| n.as_note().is_some()))
            .map(|id| (id, non_empty(self.doc.mirror.text(id))))
            .collect();
        let mirror_pos: BTreeMap<NodeId, Option<[f32; 2]>> =
            self.linked_ids().map(|id| (id, self.doc.mirror.pos(id).map(|p| [p.x, p.y]))).collect();
        let mirror_anchor: BTreeMap<NodeId, Option<Value>> = self
            .linked_ids()
            .map(|id| {
                let anchor = self.doc.mirror.anchor(id);
                (id, anchor.map(|a| json!({"pos": [a.pos[0], a.pos[1]], "group": a.group, "explicit": a.explicit})))
            })
            .collect();
        json!({
            "mirror_groups": mirror_groups,
            "mirror_names": mirror_names,
            "mirror_texts": mirror_texts,
            "mirror_pos": mirror_pos,
            "mirror_anchor": mirror_anchor,
        })
    }

    /// `export_state`'s `links`: every link this replica knows (ends null where unset), whether or not it is shown.
    /// `shown` is true when both ends are in `nodes`, so a link whose end is hidden (closed) reads false and a
    /// deleted one is absent.
    fn links_json(&self) -> Vec<Value> {
        self.doc
            .mirror
            .link_ids()
            .map(|link| {
                let (from, to) = self.doc.mirror.link_ends(link);
                let shown = [from, to].iter().all(|e| e.is_some_and(|n| self.nodes.contains_key(&n)));
                json!({
                    "id": link, "from": from, "to": to, "shown": shown,
                })
            })
            .collect()
    }

    /// `export_state`'s `edges`: the lines drawn at this level, with the point halfway along each curve (t = 0.5) on
    /// screen: where a probe clicks a link line.
    fn edges_json(&self) -> Vec<Value> {
        self.level_edges()
            .iter()
            .map(|edge| {
                let on_screen = |t: f32| {
                    let world = match edge.curve {
                        crate::level::EdgeCurve::Bow => crate::links::gestures::bow_point(edge.from, edge.to, t),
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
                json!({
                    "kind": kind, "link": edge.link, "from": edge.from_id, "to": edge.to_id, "screen": point_json(mid),
                    "near_from": point_json(near_from), "near_to": point_json(near_to),
                })
            })
            .collect()
    }

    /// `export_state`'s `link_handles`: where each card has connect handles when it would show them (on hover, and
    /// on every card during a connect-drag), on screen, so a probe can press one:
    /// `{card id: {top|right|bottom|left: [left, top, right, bottom]}}`. Empty when nothing shows them.
    fn link_handles_json(&self) -> Map<String, Value> {
        let zoom_scale = self.camera.transform(self.graph_scene_outer_rect()).scaling;
        self.level_items()
            .into_iter()
            .filter(|item| item.origin == Origin::Stored && self.shows_link_handles())
            .map(|item| {
                let sides: Map<String, Value> =
                    crate::links::gestures::link_handle_rects(item.rect, zoom_scale, self.graph.detail)
                        .into_iter()
                        .map(|(side, rect)| (side.key().to_string(), rect_json(self.graph_world_rect_to_screen(rect))))
                        .collect();
                (item.id.to_string(), Value::Object(sides))
            })
            .collect()
    }

    /// `export_state`'s link, connect and drag keys.
    fn links_state(&self) -> Value {
        // Recently Closed as the document orders it, newest first (`Doc::closed_roots`).
        let closed: Vec<String> = self
            .doc
            .journal
            .as_ref()
            .map_or_else(Vec::new, |j| j.doc().closed_roots().into_iter().map(|(id, _)| id.to_string()).collect());
        json!({
            "links": self.links_json(),
            "closed": closed,
            "edges": self.edges_json(),
            // Pick-target mode: the source card, the edge and the cards a click would finish it on.
            "connecting": self.link.connecting.as_ref().map(|(from, edge)| {
                json!({
                    "from": from,
                    "edge": edge,
                    "targets": self.valid_targets(edge, *from).iter().map(NodeId::to_string).collect::<Vec<_>>(),
                })
            }),
            "link_handles": self.link_handles_json(),
            "connect_drag": self.link.connect_drag,
            // The Link node whose end a connect-drag from a curve is moving (`App::retarget`), or null.
            "retarget": self.link.retarget.as_ref().map(|r| r.link),
            // While it runs, what a release at the pointer would do (`App::retarget_preview`): "retarget", "delete",
            // "refused" or "cancel"; else null.
            "retarget_preview": self.link.retarget.as_ref().and(self.link.retarget_preview).map(crate::links::RetargetOutcome::name),
            // The card being dragged (`App::dragging`), or null.
            "dragging": self.interaction.dragging,
        })
    }

    /// `export_state`'s selection, toast, window, popup and theme keys.
    fn window_state(&self) -> Value {
        let mut selected: Vec<String> = self.interaction.selected.iter().map(|id| id.to_string()).collect();
        selected.sort();
        let window = self.egui_ctx.content_rect();
        let visuals = self.egui_ctx.global_style().visuals.clone();
        json!({
            "json_level": self.json_level_state(),
            // How many outdated surfaces were reconfigured and lost ones recreated (debug `SURFACE`).
            "surface_recoveries": self.gpu.as_ref().map(|g| g.recoveries),
            "selected": selected,
            // The toast showing, or else the next queued one.
            "toast": self.toasts.save_toast.as_ref().map(|(message, _)| message).or(self.toasts.save_toast_queue.front()),
            "view_kinds": self.view_kinds_state(),
            "window": [window.width(), window.height()],
            "find_result": self.find.result,
            "popups": self.popups_state(),
            // The theme colors the gate checks on screen, as [r, g, b]: the gate reads them here, not from its own copy.
            "theme": {"danger": crate::theme::rgb(crate::theme::danger(&visuals)), "accent": crate::theme::rgb(crate::theme::accent(&visuals))},
        })
    }

    /// `export_state`'s keys for what the last frame drew, and the trails.
    fn drawn_state(&self) -> Value {
        json!({
            // The named controls the last frame drew (`identity::controls_in`), list rows among them: the gate clicks
            // them by name, not by pixel, finds a list's highlighted or focused row by `selected`, and tells a Tab stop
            // from a disabled button by `focusable`.
            "controls": self.debug.controls.iter().map(|c| json!({"name": c.label, "selected": c.selected, "focusable": c.focusable, "rect": rect_json(c.rect)})).collect::<Vec<_>>(),
            "open_popup": self.menus.open_popup.map(|p| format!("{p:?}")),
            "identities": crate::identity::drawn_icons(&self.egui_ctx).iter().map(|i| json!({"kind": i.kind, "text": i.text, "rect": rect_json(i.rect)})).collect::<Vec<_>>(),
            "trail": self.trail_state(),
        })
    }

    /// `export_state`'s `labels`: every live label: its id, stored name and whether it has a colour.
    fn labels_json(&self) -> Vec<Value> {
        self.label_defs()
            .into_iter()
            .map(|(id, d)| json!({"id": id, "name": d.name, "color": d.color.is_some()})) // name rule: reported as stored
            .collect()
    }

    /// `export_state`'s `right_panel`: what it shows and the cards the pane describes (the gate's card-pane checks).
    fn right_panel_state(&self) -> Value {
        match self.right_panel() {
            None => Value::Null,
            Some(crate::graph_ui::RightPanel::Outline) => json!({"shows": "outline"}),
            Some(crate::graph_ui::RightPanel::Cards) => {
                // Ids written as `selected` writes them, so the gate compares them with its card ids.
                let mut cards: Vec<String> =
                    self.interaction.selected.iter().filter(|c| c.node().is_some()).map(|c| c.to_string()).collect();
                cards.sort();
                let card = self.pane_card().map(|id| id.to_string());
                json!({"shows": "cards", "cards": cards, "card": card})
            }
        }
    }

    /// `export_state`'s `trail`: each tab with a trail, by id, as `NavHistory::export` plus `back` and `forward`, the
    /// chips' enablement (`trail_enabled`).
    fn trail_state(&self) -> serde_json::Map<String, serde_json::Value> {
        self.scoped
            .nav_history
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
            (kind.desc().host.id.to_string(), serde_json::json!({"state": state, "disk": disk}))
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
mod tests;
