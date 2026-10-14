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

    /// The current level's cards in draw order (empty if the key is missing).
    pub fn cards(&self) -> &[Value] {
        self.0.get("cards").and_then(Value::as_array).map_or(&[], Vec::as_slice)
    }

    pub fn card(&self, id: &str) -> Option<&Value> {
        self.cards().iter().find(|c| c["id"] == id)
    }

    /// `card_is`: a stored node of type `ty` (a JSON level's card is never one); `loaded` also
    /// requires that engine state (`Some(true)` a live tab, `Some(false)` a sleeping/waking/failed one).
    pub fn card_is(card: &Value, ty: &str, loaded: Option<bool>) -> bool {
        if card["json_card"].as_bool().unwrap_or(false) || card["type"] != ty {
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

    /// The card's center on screen, `"x y"` rounded as python's `%.0f` (`xy(id)`).
    pub fn xy(&self, id: &str) -> Option<String> {
        let [x, y] = Self::floats::<2>(self.card(id)?, "screen")?;
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
