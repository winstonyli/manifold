//! Personal state (spec "Personal state"): what this profile shows and
//! how, never merged. JSON inside `session.mnf.local`'s state bytes
//! (plan ruling 4).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::store::json::JsonDoc;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalState {
    #[serde(default)]
    pub focused: Option<String>,
    /// Item view (a tab or image full-size) rather than Graph view.
    #[serde(default)]
    pub view_item: bool,
    #[serde(default)]
    pub current_group: Option<String>,
    #[serde(default)]
    pub viewed_image: Option<String>,
    #[serde(default)]
    pub split_tab: Option<String>,
    #[serde(default)]
    pub groups_collapsed: bool,
    /// The camera rect `[min x, min y, max x, max y]` and the level it was
    /// saved at (plan ruling 5).
    #[serde(default)]
    pub camera: Option<([f32; 4], Option<String>)>,
    /// Per node, by live id.
    #[serde(default)]
    pub nodes: BTreeMap<String, LocalNode>,
    /// Recently Closed, oldest first (plan ruling 7).
    #[serde(default)]
    pub closed: Vec<JsonDoc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalNode {
    /// The settled position, so the graph reopens as it was left.
    pub pos: [f32; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub editing: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Whether `a` and `b` differ enough to write: settled positions within
/// 1 px count as equal, so physics jitter never causes a write.
pub(crate) fn differs(a: &LocalState, b: &LocalState) -> bool {
    let strip = |s: &LocalState| {
        let mut s = s.clone();
        s.nodes.values_mut().for_each(|n| n.pos = [0.0, 0.0]);
        s
    };
    if strip(a) != strip(b) {
        return true;
    }
    a.nodes.iter().any(|(id, n)| {
        let m = &b.nodes[id]; // same keys: `strip` compared them
        (n.pos[0] - m.pos[0]).abs() > 1.0 || (n.pos[1] - m.pos[1]).abs() > 1.0
    })
}

impl LocalState {
    /// This state with ids renamed by `map` (an import gave nodes fresh
    /// ids); ids `map` lacks are dropped.
    pub(crate) fn remap(&self, map: &HashMap<String, String>) -> LocalState {
        let id = |old: &Option<String>| old.as_ref().and_then(|o| map.get(o)).cloned();
        LocalState {
            focused: id(&self.focused),
            current_group: id(&self.current_group),
            viewed_image: id(&self.viewed_image),
            split_tab: id(&self.split_tab),
            camera: self
                .camera
                .clone()
                .filter(|(_, level)| level.is_none() || id(level).is_some())
                .map(|(r, level)| (r, id(&level))),
            nodes: self.nodes.iter().filter_map(|(k, v)| Some((map.get(k)?.clone(), v.clone()))).collect(),
            ..self.clone()
        }
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("personal state serializes")
    }

    /// `None` for bytes that aren't this shape: treated as a fresh start.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<LocalState> {
        serde_json::from_slice(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_pos(x: f32) -> LocalState {
        let node = LocalNode { pos: [x, 0.0], ..LocalNode::default() };
        LocalState { nodes: BTreeMap::from([("a".to_owned(), node)]), ..LocalState::default() }
    }

    #[test]
    fn jitter_under_a_pixel_is_not_a_change() {
        assert!(!differs(&with_pos(10.0), &with_pos(10.9)));
        assert!(differs(&with_pos(10.0), &with_pos(11.5)));
        assert!(differs(&with_pos(10.0), &LocalState::default()), "a node appearing is a change");
    }

    #[test]
    fn remap_renames_ids_and_drops_unknown_ones() {
        let mut state = with_pos(1.0);
        state.focused = Some("a".into());
        state.current_group = Some("gone".into());
        let map = HashMap::from([("a".to_owned(), "b".to_owned())]);
        let out = state.remap(&map);
        assert_eq!(out.focused.as_deref(), Some("b"));
        assert_eq!(out.current_group, None);
        assert!(out.nodes.contains_key("b"));
    }

    #[test]
    fn bytes_round_trip_and_garbage_is_a_fresh_start() {
        let state = with_pos(3.0);
        assert_eq!(LocalState::from_bytes(&state.to_bytes()), Some(state));
        assert_eq!(LocalState::from_bytes(b"\x00not json"), None);
    }
}
