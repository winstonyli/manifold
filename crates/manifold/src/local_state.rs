//! Personal state (spec "Personal state"): what this profile shows and
//! how, never merged. JSON inside `session.mnf.local`'s state bytes
//! (plan ruling 4).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

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
    #[serde(default, deserialize_with = "lenient_camera")]
    pub camera: Option<([f32; 4], Option<String>)>,
    /// Per node, by live id.
    #[serde(default)]
    pub nodes: BTreeMap<String, LocalNode>,
    /// Copy path on a JSON level gives the accessor form
    /// (`users[0].name`) rather than a JSON Pointer.
    #[serde(default)]
    pub json_accessor_paths: bool,
    /// The JSON outline panel is hidden.
    #[serde(default)]
    pub json_outline_hidden: bool,
    /// The JSON outline panel's width, once resized.
    #[serde(default)]
    pub json_outline_width: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LocalNode {
    /// The settled position, so the graph reopens as it was left.
    #[serde(deserialize_with = "lenient_pos")]
    pub pos: [f32; 2],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoom: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub editing: bool,
    /// A tab's current url, when its page moved on from the shared `url`
    /// without sharing (a redirect, a script's navigation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_url: Option<LiveUrl>,
}

/// See `LocalNode::live_url`. A restart reopens `url` while the shared
/// `url` is still `from`; once another replica has shared a new one, that
/// wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LiveUrl {
    pub url: String,
    pub from: String,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// A `null` coordinate (an older build wrote NaN that way) reads as 0.
fn lenient_pos<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f32; 2], D::Error> {
    let [x, y] = <[Option<f32>; 2]>::deserialize(d)?;
    Ok([x.unwrap_or(0.0), y.unwrap_or(0.0)])
}

/// A camera rect with a `null` in it reads as no saved camera.
#[allow(clippy::type_complexity, reason = "mirrors the field's own type")]
fn lenient_camera<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<([f32; 4], Option<String>)>, D::Error> {
    let raw = Option::<([Option<f32>; 4], Option<String>)>::deserialize(d)?;
    Ok(raw.and_then(|(r, level)| Some(([r[0]?, r[1]?, r[2]?, r[3]?], level))))
}

/// Whether `a` and `b` differ enough to write: settled positions within
/// 1 px count as equal, so physics jitter never causes a write.
pub(crate) fn differs(a: &LocalState, b: &LocalState) -> bool {
    let strip = |s: &LocalState| {
        let mut s = s.clone();
        s.nodes.values_mut().for_each(|n| n.pos = [0.0, 0.0]);
        // A non-finite camera is never written (`to_bytes`), so it must not read as a change either.
        if s.camera.as_ref().is_some_and(|(r, _)| r.iter().any(|v| !v.is_finite())) {
            s.camera = None;
        }
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
/// The personal-state file's write bookkeeping (`App::local_persist`).
pub(crate) struct LocalPersist {
    /// Personal state (spec "Personal state") last written to
    /// `session.mnf.local`, or read at startup: `maybe_write_local` skips
    /// writing when the current state compares equal, so an idle session
    /// never rewrites the file.
    pub(crate) local_written: Option<LocalState>,
    /// Set while startup couldn't read `session.mnf.local` (another
    /// window held the lock, or the read failed): nothing writes it, so
    /// this profile's saved state isn't replaced by defaults, and
    /// `poll_store` retries the read.
    pub(crate) local_unread: Option<crate::session_sync::LocalUnread>,
    /// When `local_written` was last written (or, at startup, `Instant::now`
    /// so the first `EVERY` window starts from launch, not from `UNIX_EPOCH`).
    pub(crate) local_written_at: std::time::Instant,
    /// Personal-state entries for nodes `apply_local` found still spawning
    /// (no live node yet): applied once each one's node is inserted
    /// (`handle_tab_spawn_result`), then removed. Only `nodes` is ever
    /// populated here -- the view fields are resolved once, in `apply_local`
    /// itself, since spawning is enough for those to take effect.
    pub(crate) pending_local: Option<LocalState>,
}

impl Default for LocalPersist {
    fn default() -> Self {
        Self {
            local_written: None,
            local_unread: None,
            local_written_at: std::time::Instant::now(),
            pending_local: None,
        }
    }
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

    fn all_finite(&self) -> bool {
        self.nodes.values().all(|n| n.pos.iter().all(|v| v.is_finite()))
            && self.camera.as_ref().is_none_or(|(r, _)| r.iter().all(|v| v.is_finite()))
            && self.json_outline_width.is_none_or(f32::is_finite)
    }

    /// serde_json writes NaN and infinity as `null`, which a plain `f32`
    /// can't read back: one bad value used to make the whole file unreadable,
    /// Recently Closed included. A non-finite position is zeroed, any other non-finite value dropped.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        if self.all_finite() {
            return serde_json::to_vec(self).expect("personal state serializes");
        }
        let mut clean = self.clone();
        clean.nodes.values_mut().flat_map(|n| n.pos.iter_mut()).filter(|v| !v.is_finite()).for_each(|v| *v = 0.0);
        if clean.camera.as_ref().is_some_and(|(r, _)| r.iter().any(|v| !v.is_finite())) {
            clean.camera = None;
        }
        clean.json_outline_width = clean.json_outline_width.filter(|w| w.is_finite());
        serde_json::to_vec(&clean).expect("personal state serializes")
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

    #[test]
    fn a_non_finite_value_costs_only_itself() {
        let mut state = with_pos(f32::NAN);
        state.nodes.insert("b".into(), LocalNode { pos: [5.0, 6.0], title: "kept".into(), ..LocalNode::default() });
        state.camera = Some(([0.0, f32::INFINITY, 1.0, 1.0], None));
        state.json_outline_width = Some(f32::NAN);
        let back = LocalState::from_bytes(&state.to_bytes()).expect("still readable");
        assert_eq!(back.nodes["a"].pos, [0.0, 0.0], "the bad position is zeroed, the node kept");
        assert_eq!(back.nodes["b"].title, "kept");
        assert_eq!((back.camera, back.json_outline_width), (None, None));
    }

    #[test]
    fn a_non_finite_camera_is_no_change() {
        let mut state = with_pos(1.0);
        state.camera = Some(([0.0, f32::NAN, 1.0, 1.0], None));
        assert!(!differs(&state, &state.clone()), "it is never written, so it never differs");
    }

    /// Files written before the fix hold `null` where the value was NaN.
    #[test]
    fn nulls_written_by_older_builds_read_back() {
        let bytes = br#"{"camera":[[0.0,null,1.0,1.0],null],"nodes":{"a":{"pos":[null,2.0]}},"closed":[{"nodes":[]}]}"#;
        let state = LocalState::from_bytes(bytes).expect("lenient");
        assert_eq!(state.nodes["a"].pos, [0.0, 2.0]);
        assert_eq!(state.camera, None);
        assert_eq!(state.nodes.len(), 1, "an old file's Recently Closed list is ignored, not an error");
    }

    #[test]
    fn json_level_preferences_round_trip_and_default_off() {
        let state = LocalState {
            json_accessor_paths: true,
            json_outline_hidden: true,
            json_outline_width: Some(310.0),
            ..LocalState::default()
        };
        let back = LocalState::from_bytes(&state.to_bytes()).unwrap();
        assert_eq!(
            (back.json_accessor_paths, back.json_outline_hidden, back.json_outline_width),
            (true, true, Some(310.0))
        );
        let old = LocalState::from_bytes(br#"{}"#).unwrap();
        assert_eq!((old.json_accessor_paths, old.json_outline_hidden, old.json_outline_width), (false, false, None));
    }
}
