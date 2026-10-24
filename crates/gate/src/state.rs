//! EXPORTSTATE as the checks read it: a thin wrapper over the JSON with accessors for the cards, tabs, groups,
//! positions, labels and the bbox. The producer is `App::export_state` (debug_control.rs, doc comment on `ExportState`).

use serde_json::Value;

pub const TAB: &str = "application/vnd.manifold.tab";
pub const GROUP: &str = "application/vnd.manifold.group";
pub const JAR: &str = "application/vnd.manifold.jar";
pub const NOTE: &str = "text/markdown";
pub const IMAGE: &str = "image/png";
pub const PDF: &str = "application/pdf";
pub const JSON: &str = "application/json";
/// The Graph toolbar's "Fit" button (Reset View, fit all), as a `CLICK`: the zoom chip's last button in the 816 px test window.
pub const FIT_VIEW: &str = "CLICK 380 19";
pub const DOWNLOAD: &str = "application/octet-stream";

/// The headless window the smoke test sizes with RESIZE, and the graph toolbar's height rounded up:
/// `W, H, TOP`. `xy`/`lt` warn on stderr about a point outside it, as the python did.
pub const W: f64 = 816.0;
pub const H: f64 = 639.0;
pub const TOP: f64 = 40.0;

/// Python's `round(x)` for a float: ties go to the even integer (0.5 -> 0, 1.5 -> 2, 2.5 -> 2).
pub fn round_half_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// Reproduces python's `"%.0f" % x`, which is what the scripts put in command lines (`CLICK 407 332`): the value
/// rounded half to even, printed without a fraction. Python prints a negative value that rounds to zero as `-0`
/// (`"%.0f" % -0.4`); this prints `0`. The commands parse both the same, so command lines stay identical to the
/// scripts' except where the script would have printed `-0`.
pub fn fmt0(x: f64) -> String {
    let r = round_half_even(x);
    // `-0.0 as i64` is 0, so no "-0"; coordinates are far inside i64.
    format!("{}", r as i64)
}

/// `"%.0f %.0f"`.
pub fn fmt_pair(x: f64, y: f64) -> String {
    format!("{} {}", fmt0(x), fmt0(y))
}

/// One EXPORTSTATE file, parsed.
#[derive(Debug, Clone)]
pub struct State(pub Value);

impl State {
    pub fn parse(text: &str) -> Result<State, serde_json::Error> {
        serde_json::from_str(text).map(State)
    }

    /// The theme color `name` ("danger", "accent") as `[r, g, b]`, from EXPORTSTATE's `theme`.
    pub fn theme_color(&self, name: &str) -> Option<[u8; 3]> {
        let c = self.0.get("theme")?.get(name)?.as_array()?;
        let channel = |i: usize| c.get(i)?.as_u64().and_then(|v| u8::try_from(v).ok());
        Some([channel(0)?, channel(1)?, channel(2)?])
    }

    /// Every card of the current level in draw order, jar cards included (empty if the key is missing): what is on
    /// screen, for hit tests and layout.
    pub fn all_cards(&self) -> &[Value] {
        self.0.get("cards").and_then(Value::as_array).map_or(&[], Vec::as_slice)
    }

    /// Whether `card` is a virtual card (a source's, no node behind it): EXPORTSTATE gives it a `"virtual"` key.
    pub fn is_virtual(card: &Value) -> bool {
        card.get("virtual").is_some()
    }

    /// The cards a stage means by "the cards": `all_cards` without the jar cards (the Default jar card exists in every
    /// session and is no part of what the tab, sleep and spawn stages count) and without virtual cards (a stage that
    /// means them reads `all_cards`).
    pub fn cards(&self) -> Vec<&Value> {
        self.all_cards().iter().filter(|c| c["type"] != JAR && !Self::is_virtual(c)).collect()
    }

    /// Any card by id, jar cards included.
    pub fn card(&self, id: &str) -> Option<&Value> {
        self.all_cards().iter().find(|c| c["id"] == id)
    }

    /// A card the probe needs: `card`, or an error naming the id and the ids on screen, so a stale or hand-built id
    /// reads as that and not as a failed check.
    pub fn require_card(&self, id: &str) -> Result<&Value, String> {
        self.card(id).ok_or_else(|| {
            let present: Vec<&str> = self.all_cards().iter().filter_map(|c| c["id"].as_str()).collect();
            format!("no card {id}; cards present: {}", present.join(", "))
        })
    }

    /// `card_is`: a stored node of type `ty` (a JSON level's card and a virtual card are never one); `loaded` also
    /// requires that engine state (`Some(true)` a live tab, `Some(false)` a sleeping/waking/failed one).
    pub fn card_is(card: &Value, ty: &str, loaded: Option<bool>) -> bool {
        if card["json_card"].as_bool().unwrap_or(false) || Self::is_virtual(card) || card["type"] != ty {
            return false;
        }
        loaded.is_none_or(|want| card["loaded"].as_bool() == Some(want))
    }

    fn ids_of(&self, ty: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .cards()
            .iter()
            .filter(|c| Self::card_is(c, ty, None))
            .filter_map(|c| c["id"].as_str().map(str::to_string))
            .collect();
        ids.sort();
        ids
    }

    /// Sorted ids of the tab cards.
    pub fn tabs(&self) -> Vec<String> {
        self.ids_of(TAB)
    }

    /// Sorted ids of the group cards.
    pub fn groups(&self) -> Vec<String> {
        self.ids_of(GROUP)
    }

    /// `pt`: `"%.0f %.0f"`, after a stderr warning if the point is off the graph canvas.
    fn pt(&self, id: &str, x: f64, y: f64) -> String {
        if !((0.0..W).contains(&x) && (TOP..H).contains(&y)) {
            eprintln!("query: {x:.0},{y:.0} ({} {id}) is off the graph canvas", self.label(id));
        }
        fmt_pair(x, y)
    }

    /// `label`: the card's type, `;loaded` for a live tab, `json-card` for a JSON level's card.
    pub fn label(&self, id: &str) -> String {
        match self.card(id) {
            None => "missing".to_string(),
            Some(c) if c["json_card"].as_bool().unwrap_or(false) => "json-card".to_string(),
            Some(c) => {
                let live = if c["loaded"].as_bool() == Some(true) { ";loaded" } else { "" };
                format!("{}{live}", c["type"].as_str().unwrap_or("?"))
            }
        }
    }

    /// A card's `rect` (`[left, top, right, bottom]`), the one reader every stage uses.
    pub fn rect(&self, id: &str) -> Option<[f64; 4]> {
        Self::floats::<4>(self.card(id)?, "rect")
    }

    /// The first four numbers of a JSON array as a rect (a popup's or a card's `rect` value).
    pub fn rect_from(v: &Value) -> Option<[f64; 4]> {
        let a = v.as_array()?;
        Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?, a.get(3)?.as_f64()?])
    }

    fn floats<const N: usize>(card: &Value, key: &str) -> Option<[f64; N]> {
        let a = card.get(key)?.as_array()?;
        if a.len() < N {
            return None;
        }
        let mut out = [0.0; N];
        for (slot, v) in out.iter_mut().zip(a) {
            *slot = v.as_f64()?;
        }
        Some(out)
    }

    /// A point on the card, `"x y"` rounded as python's `%.0f` (`xy(id)`): its center, or when that is off the canvas
    /// (a card past the window's edge) or another card (a virtual trail page, say) is drawn over it, the first of a few
    /// other points that are on the canvas and that no card covers.
    pub fn xy(&self, id: &str) -> Option<String> {
        let card = self.card(id)?;
        let [x, y] = Self::floats::<2>(card, "screen")?;
        let covered = |x: f64, y: f64| {
            self.all_cards()
                .iter()
                .filter(|c| c["id"] != id)
                .filter_map(|c| Self::rect_from(&c["rect"]))
                .any(|r| (r[0]..=r[2]).contains(&x) && (r[1]..=r[3]).contains(&y))
        };
        let usable = |x: f64, y: f64| (0.0..W).contains(&x) && (TOP..H).contains(&y) && !covered(x, y);
        if !usable(x, y) {
            if let Some([l, t, r, b]) = Self::floats::<4>(card, "rect") {
                let grid = [0.5, 0.25, 0.75, 0.1, 0.9]
                    .into_iter()
                    .flat_map(|fy| [0.5, 0.25, 0.75, 0.1, 0.9].map(|fx| (l + (r - l) * fx, t + (b - t) * fy)));
                // The card's visible part first: a point of its rect clamped to the canvas.
                let visible = (l.max(0.0), t.max(TOP), r.min(W - 1.0), b.min(H - 1.0));
                let inside = [0.5, 0.25, 0.75, 0.1, 0.9].into_iter().flat_map(|fy| {
                    [0.5, 0.25, 0.75, 0.1, 0.9]
                        .map(|fx| (visible.0 + (visible.2 - visible.0) * fx, visible.1 + (visible.3 - visible.1) * fy))
                });
                if let Some((x, y)) = inside.chain(grid).find(|&(x, y)| usable(x, y)) {
                    return Some(self.pt(id, x, y));
                }
            }
        }
        Some(self.pt(id, x, y))
    }

    /// 3 points inside the card's top-left corner (`lt(id)`), clear of anything drawn at the center.
    pub fn lt(&self, id: &str) -> Option<String> {
        let [left, top, _, _] = Self::floats::<4>(self.card(id)?, "rect")?;
        Some(self.pt(id, left + 3.0, top + 3.0))
    }

    /// `"x0 y0 x1 y1"`: the tab cards' bounding box grown by 20, clamped to the canvas, or `""` with no tab
    /// (`bbox`).
    pub fn bbox(&self) -> String {
        let rects: Vec<[f64; 4]> =
            self.tabs().iter().filter_map(|id| Self::floats::<4>(self.card(id)?, "rect")).collect();
        if rects.is_empty() {
            return String::new();
        }
        let min = |i: usize| rects.iter().map(|r| r[i]).fold(f64::INFINITY, f64::min);
        let max = |i: usize| rects.iter().map(|r| r[i]).fold(f64::NEG_INFINITY, f64::max);
        format!(
            "{} {} {} {}",
            fmt0((min(0) - 20.0).max(0.0)),
            fmt0((min(1) - 20.0).max(TOP)),
            fmt0((max(2) + 20.0).min(W - 1.0)),
            fmt0((max(3) + 20.0).min(H - 1.0)),
        )
    }

    /// `focused_url`, `""` when null (the python's `s["focused_url"] or ""`).
    pub fn focused_url(&self) -> &str {
        self.0["focused_url"].as_str().unwrap_or("")
    }

    /// Whether a tab's trail has its cursor on the focused url: a Back or Forward has arrived, not only been sent (the
    /// tab's url changes when a plain move is sent).
    pub fn trail_at_focus(&self) -> bool {
        let url = self.focused_url();
        self.0["trail"].as_object().is_some_and(|t| t.values().any(|e| e["url"].as_str() == Some(url)))
    }

    /// The bottom-left toast's text, if one is showing.
    pub fn toast(&self) -> Option<&str> {
        self.0["toast"].as_str()
    }

    /// Whether the Tab view (one page full screen) is showing rather than the Graph.
    pub fn tab_view(&self) -> bool {
        self.0["tab_view"].as_bool().unwrap_or(false)
    }

    /// The Graph view level's group id, or `None` at the top.
    pub fn current_group(&self) -> Option<&str> {
        self.0["current_group"].as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A virtual trail-page card can sit over a group card's center: the smoke's double-click there entered nothing
    /// (SMOKE-GROUP-FLAKE), so `xy` aims at a point no other card covers.
    #[test]
    fn xy_avoids_a_card_drawn_over_the_center() {
        let s = State::parse(
            r#"{"cards":[
              {"id":"g","screen":[499.0,338.0],"rect":[442.0,299.0,556.0,377.0]},
              {"id":"v","virtual":true,"screen":[534.0,333.0],"rect":[477.0,318.0,591.0,349.0]}]}"#,
        )
        .unwrap();
        let at: Vec<f64> = s.xy("g").unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
        let over = |c: [f64; 4]| at[0] >= c[0] && at[0] <= c[2] && at[1] >= c[1] && at[1] <= c[3];
        assert!(over([442.0, 299.0, 556.0, 377.0]), "inside the card: {at:?}");
        assert!(!over([477.0, 318.0, 591.0, 349.0]), "clear of the trail page: {at:?}");
    }

    /// A card can reach past the window's bottom edge with its center outside it (the smoke's empty group at
    /// `reopen-nested`: center y 651 in a 639 px window): `xy` aims at the part that is on the canvas.
    #[test]
    fn xy_stays_on_the_canvas() {
        let s =
            State::parse(r#"{"cards":[{"id":"g","screen":[63.0,651.0],"rect":[-17.0,596.0,143.0,706.0]}]}"#).unwrap();
        let at: Vec<f64> = s.xy("g").unwrap().split(' ').map(|v| v.parse().unwrap()).collect();
        assert!((0.0..W).contains(&at[0]) && (TOP..H).contains(&at[1]), "on the canvas: {at:?}");
        assert!(at[1] >= 596.0 && at[0] >= -17.0 && at[0] <= 143.0, "inside the card: {at:?}");
    }

    /// A trimmed real shape: two tabs (one live), a group, a note and a JSON level card.
    fn fixture() -> State {
        State::parse(
            r#"{
              "tab_view": false,"transition": null, "focused_url": null, "address_bar_text": "",
              "current_group": null, "toast": "Saved", "selected": [],
              "cards": [
                {"id":"t2","type":"application/vnd.manifold.tab","loaded":true,"json_card":false,"group":null,
                 "screen":[407.4,332.5],"rect":[300.0,250.0,520.0,420.0]},
                {"id":"t1","type":"application/vnd.manifold.tab","loaded":false,"json_card":false,"group":"g",
                 "screen":[100.5,100.0],"rect":[10.0,45.0,190.0,160.0]},
                {"id":"g","type":"application/vnd.manifold.group","loaded":false,"json_card":false,
                 "screen":[600.0,500.0],"rect":[560.0,470.0,640.0,530.0]},
                {"id":"n","type":"text/markdown","loaded":false,"json_card":false,
                 "screen":[700.0,100.0],"rect":[680.0,80.0,760.0,120.0]},
                {"id":"j","type":"application/json","loaded":false,"json_card":true,
                 "screen":[50.0,300.0],"rect":[40.0,290.0,60.0,310.0]}
              ]
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn cards_tabs_groups() {
        let s = fixture();
        assert_eq!(s.cards().len(), 5);
        assert_eq!(s.tabs(), ["t1", "t2"]);
        assert_eq!(s.groups(), ["g"]);
        assert!(s.card("nope").is_none());
        let err = s.require_card("nope").unwrap_err();
        assert!(err.starts_with("no card nope; cards present: ") && err.contains('j'), "{err}");
    }

    #[test]
    fn card_is_checks_type_loaded_and_json_cards() {
        let s = fixture();
        assert!(State::card_is(s.card("t2").unwrap(), TAB, Some(true)));
        assert!(!State::card_is(s.card("t2").unwrap(), TAB, Some(false)));
        assert!(State::card_is(s.card("t1").unwrap(), TAB, Some(false)));
        assert!(State::card_is(s.card("t1").unwrap(), TAB, None));
        assert!(State::card_is(s.card("n").unwrap(), NOTE, None));
        assert!(!State::card_is(s.card("n").unwrap(), TAB, None));
        // A JSON level's card has the JSON type but is no stored node.
        assert!(!State::card_is(s.card("j").unwrap(), JSON, None));
    }

    /// A virtual card is on screen (`all_cards`, so layout and hit tests see it) but is no stored card to a stage.
    #[test]
    fn a_virtual_card_is_in_all_cards_only() {
        let s = State::parse(
            r#"{"cards":[
              {"id":"t1","type":"application/vnd.manifold.tab","json_card":false,"loaded":false},
              {"id":"virtual:x:a","type":"application/vnd.manifold.tab","json_card":false,"loaded":false,"virtual":"x"}]}"#,
        )
        .unwrap();
        assert_eq!((s.cards().len(), s.all_cards().len()), (1, 2));
        assert_eq!(s.tabs(), ["t1"]);
        assert!(!State::card_is(s.card("virtual:x:a").unwrap(), TAB, None));
    }

    #[test]
    fn jar_cards_and_their_edge_keys() {
        let s = State::parse(
            r#"{"cards":[
              {"id":"j1","type":"application/vnd.manifold.jar","json_card":false,"jar":null,"effective_jar":null,"is_default":true},
              {"id":"t1","type":"application/vnd.manifold.tab","json_card":false,"loaded":false,"jar":"j1","effective_jar":"j1","is_default":false}]}"#,
        )
        .unwrap();
        assert!(State::card_is(s.card("j1").unwrap(), JAR, None));
        assert!(!State::card_is(s.card("j1").unwrap(), TAB, None));
        assert_eq!(s.card("t1").unwrap()["jar"], "j1");
        assert_eq!(s.card("t1").unwrap()["effective_jar"], "j1");
        assert_eq!(s.card("j1").unwrap()["is_default"], true);
        // `cards` is what the tab stages count: no jar cards; `all_cards` keeps them.
        assert_eq!((s.cards().len(), s.all_cards().len()), (1, 2));
    }

    #[test]
    fn points_and_bbox() {
        let s = fixture();
        assert_eq!(s.xy("t2").as_deref(), Some("407 332")); // 332.5 ties to even
        assert_eq!(s.xy("t1").as_deref(), Some("100 100")); // 100.5 ties to even
        assert_eq!(s.lt("t2").as_deref(), Some("303 253"));
        assert_eq!(s.xy("nope"), None);
        // x0: 10-20 clamps to 0; y0: 45-20=25 clamps to TOP; x1: 520+20; y1: 420+20.
        assert_eq!(s.bbox(), "0 40 540 440");
        assert_eq!(State::parse(r#"{"cards":[]}"#).unwrap().bbox(), "");
    }

    #[test]
    fn labels_and_scalars() {
        let s = fixture();
        assert_eq!(s.label("t2"), "application/vnd.manifold.tab;loaded");
        assert_eq!(s.label("j"), "json-card");
        assert_eq!(s.focused_url(), "");
        assert_eq!(s.toast(), Some("Saved"));
        assert!(!s.tab_view());
        assert_eq!(s.current_group(), None);
    }

    #[test]
    fn rounding_matches_python() {
        // python: round(0.5)=0 round(1.5)=2 round(2.5)=2 round(-0.4)=0 round(-0.5)=0 (as -0.0)
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(-0.4), 0.0);
        assert_eq!(round_half_even(-0.5), 0.0);
        assert_eq!(round_half_even(12.345), 12.0);
    }

    #[test]
    fn fmt0_matches_python_percent_dot0f() {
        // python "%.0f": 0.5 -> '0', 1.5 -> '2', 2.5 -> '2', -0.4 -> '-0' (here '0'), -0.5 -> '-0' (here '0'),
        // 816.0 -> '816', 12.345 -> '12'. Only the two negative cases differ, by design.
        for (x, want) in [(0.5, "0"), (1.5, "2"), (2.5, "2"), (-0.4, "0"), (-0.5, "0"), (816.0, "816"), (12.345, "12")]
        {
            assert_eq!(fmt0(x), want, "fmt0({x})");
        }
        assert_eq!(fmt0(-1.5), "-2");
        assert_eq!(fmt_pair(407.5, 332.4), "408 332");
    }
}
