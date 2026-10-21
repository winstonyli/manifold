//! The Graph view's deferred overlay text and decorations, and the rule that a card's are hidden under cards drawn
//! over it.

/// One deferred `overlay_painter` paint call, queued during `Scene::show`'s
/// content closure and actually painted only after it returns -- see
/// `DeferredOverlays`'s own doc comment for why this exists at all. Takes
/// the *real* `overlay_painter`/`project` (recomputed fresh post-`Scene`)
/// as arguments at call time, not at queue time.
type DeferredOverlay = Box<dyn FnOnce(&egui::Painter, &dyn Fn(egui::Pos2) -> egui::Pos2)>;

/// A frame's worth of `overlay_painter` decorations (node labels, pin/audio/
/// close/eject icons, the empty-canvas menu's own row labels, ...), queued
/// during `Scene::show`'s content closure instead of painted immediately.
///
/// Root cause this works around: `overlay_painter` draws on a screen-space
/// layer deliberately untransformed by `Scene` (see that field's own doc
/// comment for why -- crisp text at any zoom, not blurry rescaled glyphs),
/// so anything painted on it has to convert its own world position to
/// screen space manually via `project`. But `project` is built from
/// `self.camera.rect`, which -- confirmed by reading `egui::Scene::
/// show`'s actual source (0.36.2) -- is only updated *after* the whole
/// content closure returns for the frame; a live pan/zoom drag is applied
/// to `Scene`'s own *internal* transform before the closure runs, which is
/// why `ui.painter()`-drawn card content (rendered on `Scene`'s own
/// transformed layer, where egui applies that internal transform
/// automatically) is always correctly positioned, while anything computed
/// from `project` during the same closure is exactly one frame stale --
/// invisible at rest, but a growing pixel gap between a card and its own
/// label/icons the faster the camera pans, since a fixed one-frame timing
/// lag becomes a bigger on-screen offset the more distance the camera
/// covers in that frame.
///
/// Fix: instead of painting through `overlay_painter` immediately (using
/// the stale `project` captured before `Scene::show`), every such call
/// queues a closure capturing its own world position and paint parameters.
/// Once `Scene::show` returns and `self.camera.rect` has been updated
/// to reflect this frame's actual pan/zoom, `project` is recomputed fresh
/// and every queued closure is invoked with it -- so overlay decorations
/// are painted using the *same* up-to-date transform the card content
/// itself already reflects, closing the gap entirely rather than just
/// making it smaller.
///
///
/// Every item is tagged with the card being drawn when it was queued (`begin_card`), and shown only where no card
/// drawn after that one covers it: the overlay layer sits above every card body, so untagged it would show a lower
/// card's text through a card drawn over it. An item queued outside a card, or by `push_top` (a badge or ghost that
/// belongs above everything), is played whole.
#[derive(Default)]
pub(crate) struct DeferredOverlays {
    items: Vec<(Option<usize>, DeferredOverlay)>,
    /// Every card begun, in draw order (later is on top).
    cards: Vec<egui::Rect>,
    current: Option<usize>,
}

impl DeferredOverlays {
    /// Items queued from here to `end_card` belong to the card at `rect` (world units).
    pub(crate) fn begin_card(&mut self, rect: egui::Rect) {
        self.current = Some(self.cards.len());
        self.cards.push(rect);
    }

    pub(crate) fn end_card(&mut self) {
        self.current = None;
    }

    /// Queues `overlay` for the card being drawn, hidden under the cards drawn after it; outside a card, played whole.
    pub(crate) fn push(&mut self, overlay: DeferredOverlay) {
        self.items.push((self.current, overlay));
    }

    /// Queues `overlay` to play whole, over every card.
    pub(crate) fn push_top(&mut self, overlay: DeferredOverlay) {
        self.items.push((None, overlay));
    }

    /// Plays every item with `project` (world to screen, a scale and a shift). A card's item runs once; the shapes it
    /// adds are then clipped to the pieces of the card's rect that no later card covers, one copy per piece.
    pub(crate) fn play(self, painter: &egui::Painter, project: &dyn Fn(egui::Pos2) -> egui::Pos2) {
        let Self { items, cards, .. } = self;
        let mut visible: Vec<Option<Vec<egui::Rect>>> = vec![None; cards.len()];
        let (ctx, layer) = (painter.ctx(), painter.layer_id());
        for (card, overlay) in items {
            let Some(card) = card else {
                overlay(painter, project);
                continue;
            };
            let pieces = visible[card].get_or_insert_with(|| {
                let covers: Vec<_> = cards[card + 1..].iter().filter(|c| c.intersects(cards[card])).collect();
                uncovered(cards[card], &covers)
            });
            let clips: Vec<egui::Rect> = pieces
                .iter()
                .map(|p| egui::Rect::from_min_max(project(p.min), project(p.max)).intersect(painter.clip_rect()))
                .collect();
            if clips.is_empty() {
                continue;
            }
            let start = ctx.graphics_mut(|g| g.entry(layer).next_idx()).0;
            overlay(painter, project);
            ctx.graphics_mut(|g| {
                let list = g.entry(layer);
                let added: Vec<_> = list.all_entries().skip(start).cloned().collect();
                for i in 0..added.len() {
                    list.mutate_shape(egui::layers::ShapeIdx(start + i), |s| {
                        s.clip_rect = s.clip_rect.intersect(clips[0])
                    });
                }
                for clip in &clips[1..] {
                    for s in &added {
                        list.add(s.clip_rect.intersect(*clip), s.shape.clone());
                    }
                }
            });
        }
    }
}

/// The parts of `rect` that no rect in `covers` overlaps, as disjoint rects.
fn uncovered(rect: egui::Rect, covers: &[&egui::Rect]) -> Vec<egui::Rect> {
    let mut pieces = vec![rect];
    for cover in covers {
        pieces = pieces
            .into_iter()
            .flat_map(|p| {
                if !p.intersects(**cover) {
                    return vec![p];
                }
                let hole = p.intersect(**cover);
                let strips = [
                    egui::Rect::from_min_max(p.min, egui::pos2(p.max.x, hole.min.y)),
                    egui::Rect::from_min_max(egui::pos2(p.min.x, hole.max.y), p.max),
                    egui::Rect::from_min_max(egui::pos2(p.min.x, hole.min.y), egui::pos2(hole.min.x, hole.max.y)),
                    egui::Rect::from_min_max(egui::pos2(hole.max.x, hole.min.y), egui::pos2(p.max.x, hole.max.y)),
                ];
                strips.into_iter().filter(|s| s.width() > 0.0 && s.height() > 0.0).collect()
            })
            .collect();
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    }

    /// The area two rects share (`Rect::area` of a disjoint intersection is positive).
    fn overlap(a: egui::Rect, b: egui::Rect) -> f32 {
        let i = a.intersect(b);
        i.width().max(0.0) * i.height().max(0.0)
    }

    fn area(pieces: &[egui::Rect]) -> f32 {
        pieces.iter().map(|p| p.width() * p.height()).sum()
    }

    #[test]
    fn uncovered_is_the_rect_minus_the_covers_as_disjoint_pieces() {
        let own = r(0.0, 0.0, 10.0, 10.0);
        assert_eq!(uncovered(own, &[]), [own], "nothing over it");
        assert_eq!(uncovered(own, &[&r(20.0, 0.0, 30.0, 10.0)]), [own], "a cover elsewhere");
        assert!(uncovered(own, &[&r(-1.0, -1.0, 11.0, 11.0)]).is_empty(), "fully covered");
        let hole = r(2.0, 3.0, 5.0, 6.0);
        let pieces = uncovered(own, &[&hole]);
        assert_eq!(area(&pieces), 100.0 - 9.0);
        assert!(pieces.iter().all(|p| overlap(*p, hole) == 0.0), "none enters the hole");
        let second = r(4.0, 0.0, 8.0, 5.0);
        let two = uncovered(own, &[&hole, &second]);
        let union = 9.0 + 20.0 - overlap(hole, second);
        assert_eq!(area(&two), 100.0 - union, "overlapping covers are not subtracted twice");
        for (i, a) in two.iter().enumerate() {
            assert!(two[i + 1..].iter().all(|b| overlap(*a, *b) == 0.0), "pieces are disjoint");
        }
    }

    #[test]
    fn a_lower_cards_shapes_are_clipped_to_what_no_later_card_covers() {
        let ctx = egui::Context::default();
        let mark = |name: u8| -> DeferredOverlay {
            Box::new(move |painter, _| {
                painter.rect_filled(r(0.0, 0.0, 1.0, 1.0), f32::from(name), egui::Color32::WHITE);
            })
        };
        let mut overlays = DeferredOverlays::default();
        overlays.begin_card(r(0.0, 0.0, 10.0, 10.0));
        overlays.push(mark(1));
        overlays.end_card();
        overlays.begin_card(r(5.0, 0.0, 15.0, 10.0));
        overlays.push(mark(2));
        overlays.push_top(mark(3));
        overlays.end_card();
        overlays.push(mark(4));
        let mut overlays = Some(overlays);
        let (mut clips, mut full) = (Vec::new(), egui::Rect::NOTHING);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter().with_clip_rect(r(-100.0, -100.0, 100.0, 100.0));
            full = painter.clip_rect();
            overlays.take().unwrap().play(&painter, &|p| p * 2.0);
            let layer = painter.layer_id();
            clips = ctx.graphics_mut(|g| {
                g.entry(layer)
                    .all_entries()
                    .filter_map(|c| match &c.shape {
                        egui::Shape::Rect(rect) => Some((rect.corner_radius.nw, c.clip_rect)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
        })
        .textures_delta
        .clear();
        let of = |name: u8| clips.iter().filter(|(n, _)| *n == name).map(|(_, c)| *c).collect::<Vec<_>>();
        assert_eq!(of(1), [r(0.0, 0.0, 10.0, 20.0)], "the left half only, in screen units");
        assert_eq!(of(2), [r(10.0, 0.0, 30.0, 20.0)], "the topmost card is whole");
        assert_eq!(of(3), [full], "a top item is not narrowed");
        assert_eq!(of(4), [full], "an item outside any card is not narrowed");
    }
}
