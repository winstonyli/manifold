//! A tab's trail (spec docs/superpowers/specs/2026-10-14-trail-design.md): its pages as a tree, so Back and Forward
//! survive an engine replacement (crash restart, wake from sleep, engine swap), a close and a restart, and pages left
//! by navigating from the middle stay reachable as branches.
//!
//! Each entry has a parent (Back) and `fwd`, the child the cursor last left it for (Forward). An engine report
//! (`observe`) moves the cursor, rewrites a redirect, or forks a child. A Back or Forward (`go`) is a pending move that
//! commits when the engine reports its arrival. `lin`/`lin_at` shadow the engine's own linear list: a move uses the
//! engine's `GoBack`/`GoForward` (scroll and form state kept) only when the engine's neighbour is the tree's, else it
//! is a plain `Navigate` to the entry's URL. Back at the root is the opened-from step (`trail_nav.rs`).
//!
//! The trees are saved to `tab-history.json`, keyed by node id: machine-local, not part of the shared session store
//! (a trail changes on every navigation; the store shares only user-started URL changes).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Most entries kept per tab, and most slots in the engine-list shadow.
const MAX_ENTRIES: usize = 50;
/// How long a move waits for the engine to report its arrival.
const MOVE_WINDOW: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dir {
    Back,
    Forward,
}

/// What the host sends the engine for a move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NavAction {
    /// The engine's own `GoBack` (its neighbour is the tree's).
    GoBack,
    GoForward,
    /// A plain load of the entry's URL.
    Navigate(String),
    None,
}

/// One page of the trail.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Entry {
    url: String,
    title: String,
    /// Back goes here. `None` only for the root, entry 0.
    parent: Option<usize>,
    /// Forward goes here: the child the cursor last left this entry for.
    fwd: Option<usize>,
}

/// What `tab-history.json` holds for one tab.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Saved {
    entries: Vec<Entry>,
    at: usize,
}

impl Saved {
    /// Decision 10's load check: `at` in range, entry 0 the only root, every parent before its child, every `fwd`
    /// one of its entry's children.
    fn valid(&self) -> bool {
        let n = self.entries.len();
        self.at < n
            && self.entries.iter().enumerate().all(|(i, e)| {
                let parent_ok = match e.parent {
                    None => i == 0,
                    Some(p) => p < i,
                };
                parent_ok && e.fwd.is_none_or(|f| f < n && self.entries[f].parent == Some(i))
            })
    }
}

/// A Back, Forward or branch pick on its way: it commits when a report arrives at `target`'s URL.
#[derive(Clone, Copy, Debug)]
struct Move {
    target: usize,
    from: usize,
    /// Sent as the engine's `GoBack`/`GoForward`, not a `Navigate`.
    native: bool,
    deadline: Instant,
}

/// What a commit does to the engine-list shadow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinStep {
    /// The engine moved one slot (`GoBack`/`GoForward`, or an inferred script move).
    Native,
    /// A new engine navigation: truncate after `lin_at`, push.
    Push,
    /// The engine's current slot now shows the target (a same-URL move, which sends nothing).
    Replace,
}

#[derive(Debug, Default)]
pub(crate) struct NavHistory {
    entries: Vec<Entry>,
    /// The cursor: the entry the tab shows.
    at: usize,
    /// The engine's linear list as entry indices, oldest first; `None` is a tombstone (an evicted entry's slot).
    lin: Vec<Option<usize>>,
    /// The engine's cursor in `lin`.
    lin_at: usize,
    pending: Option<Move>,
    /// The last folded report was still loading: a URL change next is a redirect.
    loading: bool,
    /// The cursor entry was chosen by a move or reloaded by a new engine, so a redirect rewrites it.
    fresh: bool,
    /// Changed since the last save.
    dirty: bool,
    /// The child tab Back last left this tab for, at the root of the child's trail (decision 7): Forward goes there
    /// when the trail has no Forward of its own. Not saved, not part of `dirty`.
    left_to: Option<String>,
}

impl NavHistory {
    /// Folds in one `NavigationStateChanged` (many repeat the same URL). `title` goes to the cursor entry when it is
    /// non-empty and the report is settled or repeats the cursor's URL (decision 11).
    pub(crate) fn observe(&mut self, url: &str, title: &str, loading: bool, can_go_forward: bool, now: Instant) {
        let before = self.saved_form();
        let same_page = self.entries.get(self.at).is_some_and(|e| e.url == url);
        if self.fold(url, loading, can_go_forward, now)
            && !title.is_empty()
            && (same_page || !loading)
            && self.entries[self.at].url == url
        {
            self.entries[self.at].title = title.to_owned();
        }
        self.dirty |= before != self.saved_form();
    }

    /// What a save writes; a change in it is a change worth saving.
    fn saved_form(&self) -> (Vec<Entry>, usize) {
        (self.entries.clone(), self.at)
    }

    /// Decision 3's fold, steps 1 to 6. `false` when the report is held (step 4b, plan D1): nothing changed, not even
    /// `loading`.
    fn fold(&mut self, url: &str, loading: bool, can_go_forward: bool, now: Instant) -> bool {
        // 1: the first report makes the root.
        if self.entries.is_empty() {
            self.entries.push(Entry { url: url.to_owned(), title: String::new(), parent: None, fwd: None });
            self.at = 0;
            self.lin = vec![Some(0)];
            self.lin_at = 0;
            self.settle(loading);
            return true;
        }
        // 2: a move whose arrival never came.
        if self.pending.is_some_and(|m| now > m.deadline) {
            self.pending = None;
        }
        // 3: a pending move commits on its target's URL and waits through its origin's.
        if let Some(m) = self.pending {
            if self.entries[m.target].url == url {
                self.pending = None;
                self.commit(m.target, m.from, if m.native { LinStep::Native } else { LinStep::Push });
                self.loading = loading;
                return true;
            }
            if self.entries[m.from].url == url {
                self.settle(loading);
                return true;
            }
            self.pending = None;
            self.fresh = false;
        }
        // 4: the cursor's own URL.
        if self.entries[self.at].url == url {
            self.settle(loading);
            return true;
        }
        // 4b: page script's history.back()/forward(), inferred from the URL; a loading report's flags are stale.
        if let Some((dir, target)) = self.inferred(url) {
            if loading {
                return false;
            }
            if can_go_forward || (dir == Dir::Forward && self.lin_at + 2 == self.lin.len()) {
                let from = self.at;
                self.commit(target, from, LinStep::Native);
                self.loading = false;
                return true;
            }
        }
        // 5: a redirect of the load in progress: rewrite the entry, or (J5) branch from one that has children.
        let was_loading = std::mem::replace(&mut self.loading, loading);
        if was_loading {
            if self.fresh || !self.has_children(self.at) {
                let entry = &mut self.entries[self.at];
                entry.url = url.to_owned();
                entry.title.clear();
            } else {
                let child = self.child_or_push(url);
                self.entries[self.at].fwd = Some(child);
                self.at = child;
                self.fresh = false;
            }
            // The engine replaced its slot in place: no truncate, no push.
            if let Some(slot) = self.lin.get_mut(self.lin_at) {
                *slot = Some(self.at);
            }
            if !loading {
                self.fresh = false;
            }
            self.evict_to(MAX_ENTRIES);
            return true;
        }
        // 6: a new page: the equal child, else a new branch.
        let child = self.child_or_push(url);
        self.entries[self.at].fwd = Some(child);
        self.at = child;
        self.fresh = false;
        self.lin_push(child);
        self.evict_to(MAX_ENTRIES);
        true
    }

    /// A report that changes nothing but `loading`; a settled one ends the cursor entry's freshness.
    fn settle(&mut self, loading: bool) {
        self.loading = loading;
        if !loading {
            self.fresh = false;
        }
    }

    pub(crate) fn left_to(&self) -> Option<&str> {
        self.left_to.as_deref()
    }

    pub(crate) fn set_left_to(&mut self, child: Option<String>) {
        self.left_to = child;
    }

    /// The tree's neighbour whose URL is `url` and that the engine's list holds next to its cursor (4b's candidates).
    fn inferred(&self, url: &str) -> Option<(Dir, usize)> {
        [Dir::Back, Dir::Forward].into_iter().find_map(|dir| {
            let target = self.native_neighbour(dir)?;
            (self.entries[target].url == url).then_some((dir, target))
        })
    }

    /// The cursor's parent (Back) or `fwd` (Forward) when the engine's list holds it in the adjacent slot (decision
    /// 4's guard, without the engine's flag). A tombstone never matches.
    fn native_neighbour(&self, dir: Dir) -> Option<usize> {
        let cur = self.entries.get(self.at)?;
        let (target, slot) = match dir {
            Dir::Back => (cur.parent?, self.lin_at.checked_sub(1)?),
            Dir::Forward => (cur.fwd?, self.lin_at + 1),
        };
        (self.lin.get(slot) == Some(&Some(target))).then_some(target)
    }

    /// The cursor's parent (Back) or `fwd` (Forward).
    fn neighbour(&self, dir: Dir) -> Option<usize> {
        let cur = self.entries.get(self.at)?;
        match dir {
            Dir::Back => cur.parent,
            Dir::Forward => cur.fwd,
        }
    }

    fn has_children(&self, i: usize) -> bool {
        self.entries.iter().any(|e| e.parent == Some(i))
    }

    /// A Back or Forward press inside the trail: what to send, with nothing in the tree changed until the arrival
    /// (one exception: a target whose URL equals the cursor's commits now). `can_go_*` are the engine's last-reported
    /// flags. `NavAction::None` at the root's Back, at no `fwd`, and while a move is pending.
    pub(crate) fn go(&mut self, dir: Dir, can_go_back: bool, can_go_forward: bool, now: Instant) -> NavAction {
        if self.move_pending(now) {
            return NavAction::None;
        }
        let Some(target) = self.neighbour(dir) else { return NavAction::None };
        let engine = match dir {
            Dir::Back => can_go_back,
            Dir::Forward => can_go_forward,
        };
        let native = engine && self.native_neighbour(dir) == Some(target);
        self.start(target, native, now)
    }

    pub(crate) fn move_pending(&self, now: Instant) -> bool {
        self.pending.is_some_and(|m| now <= m.deadline)
    }

    /// A row of the branch list (decision 8): a plain move to `target`, never native (plan D3). Ignored for the
    /// cursor entry, an index out of range, and while a move is pending.
    pub(crate) fn go_to(&mut self, target: usize, now: Instant) -> NavAction {
        if self.move_pending(now) || target == self.at || target >= self.entries.len() {
            return NavAction::None;
        }
        self.start(target, false, now)
    }

    /// The branch list for `dir`: the cursor's siblings (Back; plan D7) or its children (Forward), oldest first,
    /// each with its title, or its URL when the title is empty.
    pub(crate) fn branches(&self, dir: Dir) -> Vec<(usize, String)> {
        let Some(cur) = self.entries.get(self.at) else { return Vec::new() };
        let parent = match dir {
            Dir::Back => {
                let Some(p) = cur.parent else { return Vec::new() };
                p
            }
            Dir::Forward => self.at,
        };
        self.entries
            .iter()
            .enumerate()
            .filter(|&(i, e)| e.parent == Some(parent) && i != self.at)
            .map(|(i, e)| (i, if e.title.is_empty() { e.url.clone() } else { e.title.clone() }))
            .collect()
    }

    /// The Back chip shows its branch marker: the cursor entry has siblings.
    pub(crate) fn back_fork(&self) -> bool {
        !self.branches(Dir::Back).is_empty()
    }

    /// The Forward chip shows its branch marker: the cursor entry has more than one child.
    pub(crate) fn forward_fork(&self) -> bool {
        self.branches(Dir::Forward).len() > 1
    }

    pub(crate) fn entry_url(&self, entry: usize) -> Option<&str> {
        self.entries.get(entry).map(|e| e.url.as_str())
    }

    /// Starts a move to `target` from the cursor.
    fn start(&mut self, target: usize, native: bool, now: Instant) -> NavAction {
        let from = self.at;
        let back = self.entries[from].parent == Some(target);
        let url = self.entries[target].url.clone();
        let action = match (native, back) {
            (true, true) => NavAction::GoBack,
            (true, false) => NavAction::GoForward,
            (false, _) => NavAction::Navigate(url.clone()),
        };
        if url == self.entries[from].url {
            // No report could tell the arrival from the origin. A plain move sends nothing: the tab shows that page.
            let before = self.saved_form();
            self.commit(target, from, if native { LinStep::Native } else { LinStep::Replace });
            self.dirty |= before != self.saved_form();
            return if native { action } else { NavAction::None };
        }
        self.pending = Some(Move { target, from, native, deadline: now + MOVE_WINDOW });
        action
    }

    /// The cursor arrives at `target` from `from` (decision 3's commit).
    fn commit(&mut self, target: usize, from: usize, step: LinStep) {
        let back = self.entries[from].parent == Some(target);
        if back {
            self.entries[target].fwd = Some(from);
        } else if let Some(p) = self.entries[target].parent {
            self.entries[p].fwd = Some(target);
        }
        self.at = target;
        self.fresh = true;
        self.left_to = None;
        match step {
            LinStep::Native if back => self.lin_at = self.lin_at.saturating_sub(1),
            LinStep::Native => self.lin_at = (self.lin_at + 1).min(self.lin.len().saturating_sub(1)),
            LinStep::Push => self.lin_push(target),
            LinStep::Replace => {
                if let Some(slot) = self.lin.get_mut(self.lin_at) {
                    *slot = Some(target);
                }
            }
        }
    }

    /// A new engine navigation to `entry`: the engine drops its forward slots and appends one.
    fn lin_push(&mut self, entry: usize) {
        self.lin.truncate(self.lin_at + 1);
        self.lin.push(Some(entry));
        if self.lin.len() > MAX_ENTRIES {
            self.lin.remove(0);
        }
        self.lin_at = self.lin.len() - 1;
    }

    /// The cursor's child at `url`, pushed as a new child if there is none (the fork rule).
    fn child_or_push(&mut self, url: &str) -> usize {
        self.left_to = None;
        if let Some(c) =
            (0..self.entries.len()).find(|&c| self.entries[c].parent == Some(self.at) && self.entries[c].url == url)
        {
            return c;
        }
        self.entries.push(Entry { url: url.to_owned(), title: String::new(), parent: Some(self.at), fwd: None });
        self.entries.len() - 1
    }

    /// The tab's engine was replaced and knows only the page at `url` (decision 4). A tree whose cursor is not at
    /// `url` (the tab moved while the app was closed, or the file is stale) is emptied, and the next report makes a
    /// new root.
    pub(crate) fn view_reset(&mut self, url: &str) {
        self.pending = None;
        self.loading = false;
        if self.entries.get(self.at).is_some_and(|e| e.url == url) {
            self.lin = vec![Some(self.at)];
            self.lin_at = 0;
            self.fresh = true;
            return;
        }
        self.dirty |= !self.entries.is_empty();
        self.entries.clear();
        self.at = 0;
        self.lin.clear();
        self.lin_at = 0;
        self.fresh = false;
    }

    /// The trail itself has a Back: the cursor is not the root.
    pub(crate) fn can_go_back(&self) -> bool {
        self.neighbour(Dir::Back).is_some()
    }

    pub(crate) fn can_go_forward(&self) -> bool {
        self.neighbour(Dir::Forward).is_some()
    }

    /// Decision 5: drops entries until at most `cap` remain, each time the oldest leaf off the protected set, else
    /// (the tree is a chain) the root.
    fn evict_to(&mut self, cap: usize) {
        while self.entries.len() > cap {
            let keep = self.protected();
            let leaf = (0..self.entries.len()).find(|&i| !keep.contains(&i) && !self.has_children(i));
            let victim = match leaf {
                Some(v) => v,
                None => {
                    // A chain sorted by index: the root's only child is entry 1.
                    let Some(child) = (1..self.entries.len()).find(|&c| self.entries[c].parent == Some(0)) else {
                        return;
                    };
                    if self.at == 0 {
                        self.at = child;
                        self.fresh = false;
                    }
                    self.entries[child].parent = None;
                    0
                }
            };
            self.remove(victim);
        }
    }

    /// The root-to-cursor path and the `fwd` chain from the cursor: what Back and Forward can reach.
    fn protected(&self) -> HashSet<usize> {
        let mut keep = HashSet::new();
        let mut cur = Some(self.at);
        while let Some(i) = cur.filter(|&i| keep.insert(i)) {
            cur = self.entries[i].parent;
        }
        let mut cur = self.entries[self.at].fwd;
        while let Some(i) = cur.filter(|&i| keep.insert(i)) {
            cur = self.entries[i].fwd;
        }
        keep
    }

    /// Removes entry `v` and renumbers: a `parent`/`fwd` naming it becomes `None`, a `lin` slot naming it a tombstone.
    fn remove(&mut self, v: usize) {
        debug_assert!(self.pending.is_none(), "eviction follows a push, after step 3 dropped or committed the move");
        let shift = |i: usize| match i.cmp(&v) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(i - 1),
        };
        self.entries.remove(v);
        for e in &mut self.entries {
            e.parent = e.parent.and_then(shift);
            e.fwd = e.fwd.and_then(shift);
        }
        for slot in &mut self.lin {
            *slot = slot.and_then(shift);
        }
        if let Some(at) = shift(self.at) {
            self.at = at;
        }
    }

    /// A trail that visited `urls` in order, each reported loading then settled.
    #[cfg(test)]
    pub(crate) fn visited(urls: &[&str]) -> NavHistory {
        let now = Instant::now();
        let mut h = NavHistory::default();
        for url in urls {
            h.observe(url, "", true, false, now);
            h.observe(url, "", false, false, now);
        }
        h
    }
}

fn path() -> Option<PathBuf> {
    Some(crate::profile::dir()?.join("tab-history.json"))
}

/// The saved trees. A missing file gives none; one that does not parse (the old format included) is moved aside.
pub(crate) fn load() -> HashMap<String, NavHistory> {
    path().map_or_else(HashMap::new, |path| load_from(&path))
}

pub(crate) fn load_from(path: &Path) -> HashMap<String, NavHistory> {
    let saved: HashMap<String, Saved> =
        crate::json_file::load_or_move_aside(path, "tab history").ok().flatten().unwrap_or_default();
    saved
        .into_iter()
        .filter(|(_, s)| s.valid())
        .map(|(id, s)| {
            let mut h = NavHistory { entries: s.entries, at: s.at, ..NavHistory::default() };
            h.evict_to(MAX_ENTRIES);
            (id, h)
        })
        .collect()
}

/// Drops the trees of tabs `keep` refuses, then writes the rest (one-page trees add nothing and are left out) when
/// any changed or any was dropped.
pub(crate) fn save(lists: &mut HashMap<String, NavHistory>, keep: impl Fn(&str) -> bool) {
    save_to(lists, keep, path().as_deref());
}

pub(crate) fn save_to(lists: &mut HashMap<String, NavHistory>, keep: impl Fn(&str) -> bool, path: Option<&Path>) {
    let before = lists.len();
    lists.retain(|id, _| keep(id));
    let dropped = lists.len() != before;
    if !dropped && !lists.values().any(|h| h.dirty) {
        return;
    }
    let Some(path) = path else { return };
    let saved: HashMap<&String, Saved> = lists
        .iter()
        .filter(|(_, h)| h.entries.len() > 1)
        .map(|(id, h)| (id, Saved { entries: h.entries.clone(), at: h.at }))
        .collect();
    crate::json_file::save(&saved, path, "tab history");
    lists.values_mut().for_each(|h| h.dirty = false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::temp_dir;

    fn visit(h: &mut NavHistory, urls: &[&str], now: Instant) {
        for url in urls {
            h.observe(url, "", true, false, now);
            h.observe(url, "", false, false, now);
        }
    }

    fn urls(h: &NavHistory) -> Vec<&str> {
        h.entries.iter().map(|e| e.url.as_str()).collect()
    }

    fn parents(h: &NavHistory) -> Vec<Option<usize>> {
        h.entries.iter().map(|e| e.parent).collect()
    }

    /// A native Back the engine completes: one settled report at the parent's URL, Forward now possible.
    fn back(h: &mut NavHistory, now: Instant) {
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        let parent = h.entries[h.at].parent.unwrap();
        let url = h.entries[parent].url.clone();
        h.observe(&url, "", false, true, now);
    }

    /// A tree built by hand: `(url, parent, fwd)` per entry, cursor `at`, no `lin`.
    fn tree(spec: &[(&str, Option<usize>, Option<usize>)], at: usize) -> NavHistory {
        let entries = spec
            .iter()
            .map(|&(url, parent, fwd)| Entry { url: url.into(), title: String::new(), parent, fwd })
            .collect();
        NavHistory { entries, at, ..NavHistory::default() }
    }

    #[test]
    fn the_fork_rule_reuses_an_equal_child_and_otherwise_adds_a_sibling() {
        let now = Instant::now();
        let mut h = NavHistory::default();
        visit(&mut h, &["a", "b"], now);
        back(&mut h, now);
        visit(&mut h, &["c"], now);
        assert_eq!(urls(&h), ["a", "b", "c"]);
        assert_eq!(parents(&h), [None, Some(0), Some(0)]);
        assert_eq!((h.at, h.lin.clone(), h.lin_at), (2, vec![Some(0), Some(2)], 1));
        back(&mut h, now);
        visit(&mut h, &["b"], now);
        assert_eq!(h.entries.len(), 3, "an equal child is reused");
        assert_eq!((h.at, h.entries[0].fwd), (1, Some(1)));
    }

    #[test]
    fn a_redirect_on_an_entry_with_children_adds_a_child_in_the_same_lin_slot() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        assert!(h.fresh, "a commit marks the target fresh");
        h.observe("a", "", false, true, now);
        assert!(!h.fresh, "a later settled report clears it");
        h.observe("a", "", true, true, now);
        h.observe("a2", "", true, true, now);
        assert_eq!(urls(&h), ["a", "b", "a2"]);
        assert_eq!((h.at, h.entries[2].parent, h.entries[0].fwd), (2, Some(0), Some(2)));
        assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(2), Some(1)], 0), "the engine replaced its slot");
        assert_eq!(h.go(Dir::Back, true, true, now), NavAction::Navigate("a".into()), "no engine slot before a2");
    }

    #[test]
    fn a_plain_back_is_a_new_engine_navigation_and_forward_then_navigates() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()), "the engine cannot go back");
        h.observe("a", "", true, false, now);
        h.observe("a", "", false, false, now);
        assert_eq!((h.at, h.entries[0].fwd), (0, Some(1)));
        assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(1), Some(0)], 2));
        assert_eq!(h.go(Dir::Forward, true, true, now), NavAction::Navigate("b".into()), "no engine slot after a");
    }

    #[test]
    fn view_reset_rebuilds_lin_and_makes_the_cursor_entry_fresh() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        h.observe("a", "", false, true, now);
        h.view_reset("a");
        assert_eq!((h.lin.clone(), h.lin_at, h.fresh), (vec![Some(0)], 0, true));
        h.observe("a", "", true, false, now);
        h.observe("a2", "", true, false, now);
        assert_eq!(urls(&h), ["a2", "b"], "a fresh entry's redirect rewrites it, children or not");
        assert_eq!(h.go(Dir::Forward, false, true, now), NavAction::Navigate("b".into()));
    }

    #[test]
    fn view_reset_with_another_url_empties_the_tree_and_the_next_report_makes_a_root() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        h.dirty = false;
        h.view_reset("x");
        assert!(h.entries.is_empty() && h.lin.is_empty() && h.dirty);
        h.observe("x", "", false, false, now);
        assert_eq!((urls(&h), h.at, h.lin.clone()), (vec!["x"], 0, vec![Some(0)]));
    }

    #[test]
    fn view_reset_drops_a_pending_move() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        h.view_reset("b");
        assert!(h.pending.is_none());
        assert_eq!(
            h.go(Dir::Back, true, false, now),
            NavAction::Navigate("a".into()),
            "a new engine has no slot before b"
        );
    }

    #[test]
    fn eviction_drops_the_oldest_unprotected_leaf_and_remaps() {
        // a -> {old, b}; b -> c. The cursor is b and Forward from it is c: the path and the fwd chain are a, b, c.
        let mut h =
            tree(&[("a", None, Some(2)), ("old", Some(0), None), ("b", Some(0), Some(3)), ("c", Some(2), None)], 2);
        h.lin = vec![Some(0), Some(2), Some(1)];
        h.evict_to(3);
        assert_eq!(urls(&h), ["a", "b", "c"]);
        assert_eq!(parents(&h), [None, Some(0), Some(1)]);
        assert_eq!((h.at, h.entries[0].fwd, h.entries[1].fwd), (1, Some(1), Some(2)));
        assert_eq!(h.lin, [Some(0), Some(1), None], "the dropped entry's slot is a tombstone");
    }

    #[test]
    fn eviction_picks_the_lower_of_two_unprotected_leaves() {
        // a -> {x, y, b}: x and y are both unprotected leaves, x is older.
        let mut h = tree(&[("a", None, Some(3)), ("x", Some(0), None), ("y", Some(0), None), ("b", Some(0), None)], 3);
        h.evict_to(3);
        assert_eq!(urls(&h), ["a", "y", "b"]);
        assert_eq!(h.at, 2);
    }

    #[test]
    fn eviction_of_a_chain_drops_the_root_and_moves_a_root_cursor() {
        let mut h = tree(&[("a", None, Some(1)), ("b", Some(0), Some(2)), ("c", Some(1), None)], 0);
        h.evict_to(2);
        assert_eq!(urls(&h), ["b", "c"]);
        assert_eq!(parents(&h), [None, Some(0)]);
        assert_eq!((h.at, h.entries[0].fwd), (0, Some(1)));
        assert!(!h.can_go_back(), "the new root's Back is the opened-from step's");
    }

    #[test]
    fn an_evicted_lin_slot_is_a_tombstone_and_native_back_still_lines_up() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        h.view_reset("b");
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
        h.observe("a", "", false, false, now);
        visit(&mut h, &["c"], now);
        assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(1), Some(0), Some(2)], 2));
        h.evict_to(2);
        assert_eq!(urls(&h), ["a", "c"]);
        assert_eq!((h.lin.clone(), h.lin_at, h.at), (vec![None, Some(0), Some(1)], 2, 1));
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack, "the engine's slot before c is still a");
    }

    #[test]
    fn an_evicted_fwd_becomes_none() {
        // a -> {b, c}; b -> x, left for c. The cursor is c: x is the one unprotected leaf.
        let mut h =
            tree(&[("a", None, Some(3)), ("b", Some(0), Some(2)), ("x", Some(1), None), ("c", Some(0), None)], 3);
        h.evict_to(3);
        assert_eq!(urls(&h), ["a", "b", "c"]);
        assert_eq!((h.entries[1].fwd, h.entries[0].fwd, h.at), (None, Some(2), 2));
    }

    #[test]
    fn the_tree_and_lin_are_capped() {
        let now = Instant::now();
        let pages: Vec<String> = (0..55).map(|n| format!("p{n}")).collect();
        let pages: Vec<&str> = pages.iter().map(String::as_str).collect();
        let mut h = NavHistory::default();
        visit(&mut h, &pages, now);
        assert_eq!((h.entries.len(), h.entries[0].url.as_str(), h.entries[0].parent), (MAX_ENTRIES, "p5", None));
        assert_eq!((h.lin.len(), h.lin_at, h.at), (MAX_ENTRIES, MAX_ENTRIES - 1, MAX_ENTRIES - 1));
        assert!(h.lin.iter().all(Option::is_some), "the front slots left lin with their entries");
    }

    #[test]
    fn a_saved_tree_round_trips_and_a_restored_tab_goes_back_by_navigate() {
        let now = Instant::now();
        let path = temp_dir("nav-history-round-trip").join("tab-history.json");
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        visit(&mut h, &["c"], now);
        h.observe("c", "C", false, false, now);
        let mut lists = HashMap::from([("t".to_string(), h)]);
        save_to(&mut lists, |_| true, Some(&path));
        let mut loaded = load_from(&path);
        let r = loaded.get_mut("t").unwrap();
        assert_eq!((&r.entries, r.at), (&lists["t"].entries, lists["t"].at));
        assert!(r.lin.is_empty(), "lin is not saved");
        r.view_reset("c");
        assert_eq!(r.go(Dir::Back, true, false, now), NavAction::Navigate("a".into()));
    }

    #[test]
    fn a_saved_tree_that_breaks_an_invariant_is_dropped() {
        let path = temp_dir("nav-history-invalid").join("tab-history.json");
        let e = |url: &str, parent: Option<usize>, fwd: Option<usize>| serde_json::json!({"url": url, "title": "", "parent": parent, "fwd": fwd});
        let file = serde_json::json!({
            "good": {"entries": [e("a", None, Some(1)), e("b", Some(0), None)], "at": 1},
            "bad_at": {"entries": [e("a", None, None), e("b", Some(0), None)], "at": 2},
            "two_roots": {"entries": [e("a", None, None), e("b", None, None)], "at": 1},
            "parent_after": {"entries": [e("a", None, None), e("b", Some(2), None), e("c", Some(0), None)], "at": 2},
            "fwd_not_a_child": {"entries": [e("a", None, None), e("b", Some(0), Some(2)), e("c", Some(0), None)], "at": 2},
            "empty": {"entries": [], "at": 0},
        });
        std::fs::write(&path, file.to_string()).unwrap();
        assert_eq!(load_from(&path).keys().collect::<Vec<_>>(), ["good"]);
    }

    #[test]
    fn a_saved_tree_over_the_cap_is_evicted_on_load() {
        let path = temp_dir("nav-history-over-cap").join("tab-history.json");
        let entries: Vec<Entry> = (0usize..60)
            .map(|n| Entry { url: format!("p{n}"), title: String::new(), parent: n.checked_sub(1), fwd: None })
            .collect();
        let file = HashMap::from([("t", Saved { entries, at: 59 })]);
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        let loaded = load_from(&path);
        let h = &loaded["t"];
        assert_eq!((h.entries.len(), h.entries[0].url.as_str(), h.at), (MAX_ENTRIES, "p10", MAX_ENTRIES - 1));
    }

    #[test]
    fn a_file_in_the_old_format_is_moved_aside() {
        let dir = temp_dir("nav-history-old-format");
        let path = dir.join("tab-history.json");
        std::fs::write(&path, r#"{"t": {"entries": ["a", "b"], "index": 1}}"#).unwrap();
        assert!(load_from(&path).is_empty());
        assert!(!path.exists());
        let aside = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("tab-history.unreadable-"));
        assert!(aside, "kept for a look, not overwritten");
    }

    #[test]
    fn a_tab_that_is_gone_leaves_the_file_even_when_nothing_changed() {
        let path = temp_dir("nav-history-prune").join("tab-history.json");
        let mut lists = HashMap::from([
            ("t1".to_string(), NavHistory::visited(&["a", "b"])),
            ("t2".to_string(), NavHistory::visited(&["c", "d"])),
        ]);
        save_to(&mut lists, |_| true, Some(&path));
        assert!(lists.values().all(|h| !h.dirty));
        save_to(&mut lists, |id| id == "t1", Some(&path));
        assert_eq!(load_from(&path).keys().collect::<Vec<_>>(), ["t1"]);
    }

    #[test]
    fn only_a_tree_change_marks_it_dirty() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a"]);
        assert!(h.dirty);
        h.dirty = false;
        h.observe("a", "", false, false, now);
        assert!(!h.dirty, "a repeated report");
        h.observe("a", "A", false, false, now);
        assert!(h.dirty, "a title");
        let left = tree(&[("a", None, Some(1)), ("b", Some(0), None), ("c", Some(0), None)], 0);
        let right = tree(&[("a", None, Some(2)), ("b", Some(0), None), ("c", Some(0), None)], 0);
        assert_ne!(left.saved_form(), right.saved_form(), "a fwd");
    }

    #[test]
    fn left_to_is_cleared_by_a_move_or_a_fork_but_not_by_a_report() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        h.set_left_to(Some("child".into()));
        h.observe("b", "B", false, false, now);
        assert_eq!(h.left_to(), Some("child"));
        visit(&mut h, &["c"], now);
        assert_eq!(h.left_to(), None, "a fork");
        h.set_left_to(Some("child".into()));
        back(&mut h, now);
        assert_eq!(h.left_to(), None, "a commit");
    }

    #[test]
    fn a_history_with_only_left_to_is_not_saved() {
        let path = temp_dir("nav-history-left-to-only").join("tab-history.json");
        let mut parent = NavHistory::default();
        parent.set_left_to(Some("child".into()));
        assert!(!parent.dirty);
        let mut lists = HashMap::from([("p".to_string(), parent), ("t".to_string(), NavHistory::visited(&["a", "b"]))]);
        save_to(&mut lists, |_| true, Some(&path));
        assert_eq!(load_from(&path).keys().collect::<Vec<_>>(), ["t"]);
    }

    #[test]
    fn a_repeated_old_url_keeps_the_pending_move() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        h.observe("b", "", false, false, now);
        assert!(h.pending.is_some());
        h.observe("a", "", false, true, now);
        assert_eq!((h.at, h.lin_at, h.pending.is_none()), (0, 0, true));
    }

    #[test]
    fn a_mismatching_arrival_forks_instead_of_moving() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        h.observe("x", "", true, false, now);
        assert_eq!(urls(&h), ["a", "b", "x"]);
        assert_eq!((h.at, h.entries[2].parent, h.pending.is_none()), (2, Some(1), true));
    }

    #[test]
    fn a_stale_move_changes_nothing_and_go_waits_for_it() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        let before = (h.entries.clone(), h.at, h.lin.clone(), h.lin_at);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        assert_eq!(h.go(Dir::Back, true, false, now + Duration::from_secs(1)), NavAction::None, "a second press");
        let later = now + Duration::from_secs(4);
        h.observe("b", "", false, false, later);
        assert_eq!((h.entries.clone(), h.at, h.lin.clone(), h.lin_at), before);
        assert!(h.pending.is_none());
        assert_eq!(h.go(Dir::Back, true, false, later), NavAction::GoBack);
    }

    #[test]
    fn a_plain_move_whose_arrival_redirects_rewrites_the_target() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
        h.observe("a", "", true, false, now);
        h.observe("a2", "", true, false, now);
        h.observe("a2", "", false, false, now);
        assert_eq!(urls(&h), ["a2", "b"], "the entry Back chose stays the cursor, children and all");
        assert_eq!((h.at, h.lin.clone(), h.lin_at), (0, vec![Some(0), Some(1), Some(0)], 2), "no extra slot");
    }

    #[test]
    fn a_fresh_navigation_during_a_plain_move_leaves_the_target_alone() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::Navigate("a".into()));
        h.observe("x", "", true, false, now);
        assert_eq!(urls(&h), ["a", "b", "x"]);
        assert_eq!((h.at, h.entries[2].parent), (2, Some(1)));
    }

    /// `a`, then `b` loading, then a held loading `a` and a settled `a` that is no Back (no Forward flag): a redirect
    /// of b's load, so b's entry is rewritten to `a` and the cursor shares its parent's URL.
    fn equal_parent(now: Instant) -> NavHistory {
        let mut h = NavHistory::visited(&["a"]);
        h.observe("b", "", true, false, now);
        h.observe("a", "", true, false, now);
        assert_eq!(urls(&h), ["a", "b"], "held");
        h.observe("a", "", false, false, now);
        assert_eq!((urls(&h), h.at), (vec!["a", "a"], 1));
        h
    }

    #[test]
    fn a_move_between_equal_urls_commits_at_go() {
        let now = Instant::now();
        let mut h = equal_parent(now);
        assert_eq!(h.go(Dir::Back, false, false, now), NavAction::None, "the tab already shows that page");
        assert_eq!(
            (h.at, h.entries[0].fwd, h.lin.clone(), h.pending.is_none()),
            (0, Some(1), vec![Some(0), Some(0)], true)
        );
        let mut h = equal_parent(now);
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        assert_eq!((h.at, h.lin_at, h.pending.is_none()), (0, 0, true));
    }

    /// Step 4 (the cursor's own URL) runs before 4b: a repeated settled report with Forward set is no inferred Back.
    #[test]
    fn the_cursors_own_url_wins_over_an_inferred_back() {
        let now = Instant::now();
        let mut h = equal_parent(now);
        h.observe("a", "", false, true, now);
        assert_eq!((h.at, h.lin_at, urls(&h)), (1, 1, vec!["a", "a"]), "no spurious Back commit");
    }

    #[test]
    fn a_script_back_is_inferred_and_a_link_to_the_parents_url_forks() {
        let now = Instant::now();
        // history.back(): the loading report (old flags) is held, the settled one has Forward set.
        let mut h = NavHistory::visited(&["a", "b"]);
        h.observe("a", "", true, false, now);
        assert_eq!((h.entries.len(), h.at, h.loading), (2, 1, false), "held: nothing changes");
        h.observe("a", "", false, true, now);
        assert_eq!((h.entries.len(), h.at, h.lin_at, h.entries[0].fwd), (2, 0, 0, Some(1)));
        // A link to the parent's URL: the settled report has no Forward flag.
        let mut h = NavHistory::visited(&["a", "b"]);
        h.observe("a", "", true, false, now);
        h.observe("a", "", false, false, now);
        assert_eq!((urls(&h), h.at, h.entries[2].parent), (vec!["a", "b", "a"], 2, Some(1)));
        // The same from a fresh cursor entry: the held report must not make the settled one b's redirect.
        let mut h = NavHistory::visited(&["a", "b", "c"]);
        back(&mut h, now);
        assert!(h.fresh);
        h.observe("a", "", true, true, now);
        h.observe("a", "", false, false, now);
        assert_eq!(urls(&h), ["a", "b", "c", "a"], "b is not rewritten");
        assert_eq!((h.at, h.entries[3].parent), (3, Some(1)));
    }

    #[test]
    fn a_held_report_then_another_url_forks_or_rewrites_by_the_earlier_loading() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        h.observe("a", "", true, false, now);
        h.observe("x", "", true, false, now);
        assert_eq!((urls(&h), h.at), (vec!["a", "b", "x"], 2), "a fork from b; the held a never got an entry");
        // b was still loading when the held report came: x is b's redirect.
        let mut h = NavHistory::visited(&["a"]);
        h.observe("b", "", true, false, now);
        h.observe("a", "", true, false, now);
        h.observe("x", "", true, false, now);
        assert_eq!((urls(&h), h.at), (vec!["a", "x"], 1));
    }

    #[test]
    fn a_held_report_then_a_repeat_or_a_late_settled_report() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        h.observe("a", "", true, false, now);
        h.observe("a", "", true, false, now);
        h.observe("a", "Page A", false, true, now);
        assert_eq!((h.at, h.entries.len(), h.entries[0].title.as_str()), (0, 2, "Page A"));
        // The settled report decides alone: how long ago the held one came does not matter (plan D1).
        let mut h = NavHistory::visited(&["a", "b"]);
        h.observe("a", "", true, false, now);
        h.observe("a", "", false, true, now + Duration::from_secs(10));
        assert_eq!((h.at, h.entries.len()), (0, 2));
    }

    #[test]
    fn an_inferred_forward_needs_the_flag_except_at_the_last_slot() {
        let now = Instant::now();
        // At the last slot a click and history.forward() leave the same tree and lin: no flag needed.
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        h.observe("a", "", false, true, now);
        h.observe("b", "", true, true, now);
        h.observe("b", "", false, false, now);
        assert_eq!((h.at, h.lin_at, h.lin.len(), h.fresh), (1, 1, 2, true));
        // A slot before the last, a settled report only (CEF, Servo): the flag decides.
        let mut h = NavHistory::visited(&["a", "b", "c"]);
        back(&mut h, now);
        back(&mut h, now);
        h.observe("b", "", false, true, now);
        assert_eq!((h.at, h.lin_at, h.lin.len()), (1, 1, 3), "history.forward()");
        let mut h = NavHistory::visited(&["a", "b", "c"]);
        back(&mut h, now);
        back(&mut h, now);
        h.observe("b", "", false, false, now);
        assert_eq!((h.at, h.lin_at, h.lin.len()), (1, 1, 2), "a click on a link to b: the engine dropped c");
        assert_eq!(h.entries.len(), 3, "the equal child is reused");
    }

    #[test]
    fn go_while_a_report_is_held_moves_as_usual() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b", "c"]);
        back(&mut h, now);
        h.observe("a", "", true, true, now);
        assert_eq!(h.go(Dir::Forward, true, true, now), NavAction::GoForward);
        h.observe("c", "", false, false, now);
        assert_eq!((h.at, h.lin_at), (2, 2));
    }

    #[test]
    fn the_real_engine_order_infers_both_ways() {
        let now = Instant::now();
        // WebView2: the loading report has the old position's flags, the settled one the new position's.
        let mut h = NavHistory::visited(&["a", "b", "c"]);
        h.observe("b", "", true, false, now);
        h.observe("b", "", false, true, now);
        assert_eq!((h.at, h.lin_at), (1, 1), "history.back() from the last slot");
        h.observe("c", "", true, true, now);
        h.observe("c", "", false, false, now);
        assert_eq!((h.at, h.lin_at, h.lin.len()), (2, 2, 3), "history.forward() to the last slot");
        assert_eq!(h.go(Dir::Back, true, false, now), NavAction::GoBack);
        h.observe("b", "", false, true, now);
        h.observe("a", "", true, true, now);
        h.observe("a", "", false, false, now);
        assert_eq!((urls(&h), h.at), (vec!["a", "b", "c", "a"], 3), "a link to the parent's URL at a non-last slot");
        assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(1), Some(3)], 2));
    }

    #[test]
    fn titles_follow_the_cursor_entry() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a"]);
        h.observe("a", "A", false, false, now);
        h.observe("b", "A", true, false, now);
        assert_eq!(h.entries[1].title, "", "a loading report may carry the previous page's title");
        h.observe("b", "B", false, false, now);
        h.observe("c", "C", false, false, now);
        let titles: Vec<&str> = h.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["A", "B", "C"]);
    }

    #[test]
    fn a_branch_pick_navigates_once_and_points_fwd_at_it() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        visit(&mut h, &["c"], now);
        assert_eq!(h.branches(Dir::Back), [(1, "b".to_string())]);
        assert!(h.back_fork() && !h.forward_fork());
        assert_eq!(h.go_to(1, now), NavAction::Navigate("b".into()), "never native");
        h.observe("b", "", true, false, now);
        h.observe("b", "", false, false, now);
        assert_eq!((h.entries.len(), h.at, h.entries[0].fwd), (3, 1, Some(1)), "no second entry for b");
        assert_eq!((h.lin.clone(), h.lin_at), (vec![Some(0), Some(2), Some(1)], 2));
        assert_eq!(
            h.go(Dir::Back, true, false, now),
            NavAction::Navigate("a".into()),
            "the engine's slot before b is c"
        );
    }

    #[test]
    fn a_forward_pick_points_fwd_at_it() {
        let now = Instant::now();
        let mut h = NavHistory::visited(&["a", "b"]);
        back(&mut h, now);
        visit(&mut h, &["c"], now);
        back(&mut h, now);
        assert!(h.forward_fork());
        assert_eq!(h.entries[0].fwd, Some(2), "Back from c");
        assert_eq!(h.branches(Dir::Forward), [(1, "b".to_string()), (2, "c".to_string())]);
        assert_eq!(h.go_to(1, now), NavAction::Navigate("b".into()));
        h.observe("b", "", false, false, now);
        assert_eq!((h.at, h.entries[0].fwd), (1, Some(1)));
        assert_eq!(h.go_to(1, now), NavAction::None, "the cursor entry itself");
    }
}
