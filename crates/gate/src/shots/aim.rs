//! Finding a card to aim at. A card off the window fails here rather than as a click on empty canvas, which would
//! leave a shot of nothing happening and no error.

use std::io;

use super::{rect, Shots};
use crate::state::State;

impl Shots {
    /// The first top-level (`place` "top"; else any) card of type `ty` (and engine state `loaded`): the centre of the
    /// part of its rect inside the window, since a card can hang off the edge. Place "chip" is the point where the
    /// jar chip hangs, a fixed number of screen points above the card's bottom-left corner.
    pub(super) fn center(&mut self, ty: &str, place: &str, loaded: Option<bool>) -> io::Result<(i64, i64)> {
        let s = self.state()?;
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let c = s
            .all_cards()
            .iter()
            .find(|c| State::card_is(c, ty, loaded) && (place != "top" || c["group"].is_null()))
            .ok_or_else(|| io::Error::other(format!("no {place} card of type {ty}")))?;
        let r = rect(c)?;
        if r[2] <= 0.0 || r[0] >= w || r[3] <= 36.0 || r[1] >= h {
            return Err(io::Error::other(format!("the {ty} card is off the window: {r:?}")));
        }
        Ok(if place == "chip" {
            ((r[0] + 18.0) as i64, (r[3] - 33.0) as i64)
        } else {
            (((r[0].max(0.0) + r[2].min(w)) / 2.0) as i64, ((r[1].max(36.0) + r[3].min(h)) / 2.0) as i64)
        })
    }

    /// The id of the first top-level card of type `ty` (import gives the seeded nodes fresh ids).
    pub(super) fn card_id(&mut self, ty: &str) -> io::Result<String> {
        let s = self.state()?;
        s.all_cards()
            .iter()
            .find(|c| State::card_is(c, ty, None) && c["group"].is_null())
            .and_then(|c| c["id"].as_str().map(str::to_string))
            .ok_or_else(|| io::Error::other(format!("no top-level card of type {ty}")))
    }
}
