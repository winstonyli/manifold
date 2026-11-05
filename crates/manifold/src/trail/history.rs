//! A tab's trail (spec docs/superpowers/specs/2026-10-14-trail-design.md): its pages as a tree, so Back and Forward
//! survive an engine replacement (crash restart, wake from sleep, engine swap), a close and a restart, and pages left
//! by navigating from the middle stay reachable as branches.
//!
//! Each entry has a parent (Back) and `fwd`, the child the cursor last left it for (Forward). An engine report
//! (`observe`) moves the cursor, rewrites a redirect, or forks a child. A Back or Forward (`go`) is a pending move that
//! commits when the engine reports its arrival. `lin`/`lin_at` shadow the engine's own linear list: a move uses the
//! engine's `GoBack`/`GoForward` (scroll and form state kept) only when the engine's neighbour is the tree's, else it
//! is a plain `Navigate` to the entry's URL. Back at the root is the opened-from step (`trail/nav.rs`).
//!
//! The trees are saved to `tab-history.json`, keyed by node id: machine-local, not part of the shared session store
//! (a trail changes on every navigation; the store shares only user-started URL changes).

use crate::json_file::ProfileFile;
use crate::store::model::NodeId;
use crate::url_parts::UrlParts;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// Most entries kept per tab, and most slots in the engine-list shadow.
const MAX_ENTRIES: usize = 50;
/// How long a move waits for the engine to report its arrival.
const MOVE_WINDOW: Duration = Duration::from_secs(3);
/// The same, once the engine has said it is loading: a busy machine can take longer than `MOVE_WINDOW` between two
/// reports of one navigation (measured 2026-10-15, `gate sleep` history on WebView2 under load).
const LOADING_WINDOW: Duration = Duration::from_secs(30);

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
    /// Names this page for as long as the process runs: unlike its index, it survives an eviction of an earlier page,
    /// and it is never reused. Not saved; `NavHistory::renumber` gives a loaded tree `0..n`.
    #[serde(skip)]
    serial: u32,
}

/// A tab's trail, numbered for the process: the high half of its pages' card ids (`trail/strip.rs`). Every
/// `Default` takes the next number, so a history made by `or_default()`, `insert` or a load needs no table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot(u32);

impl Default for Slot {
    fn default() -> Slot {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        Slot(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

impl Slot {
    pub(crate) fn get(self) -> u32 {
        self.0
    }
}

/// One page of the trail as the strip and the popup draw it.
pub(crate) struct TrailPage {
    pub(crate) parent: Option<usize>,
    /// `Entry::serial`: what names the page across an eviction.
    pub(crate) serial: u32,
    /// The title, or the URL's path (its host when there is none) when the title is empty; pages that share an address
    /// and differ in query or fragment lead with what differs (`page_labels`).
    pub(crate) label: String,
    /// The full URL (the tooltip).
    pub(crate) url: String,
    /// The page the tab shows.
    pub(crate) cursor: bool,
    /// On the chain from the root to the cursor.
    pub(crate) on_path: bool,
}

/// What `tab-history.json` holds: a tree per tab. A key that is not a node id names no tab of this build; it is
/// dropped, and so from the next save.
#[derive(Default, serde::Deserialize)]
struct SavedTrees(#[serde(deserialize_with = "crate::json_file::lenient_map")] HashMap<NodeId, Saved>);

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

/// A Back, Forward or branch pick on its way: it commits when a report arrives at `target`'s URL, and ends at its
/// deadline or at the first report of another URL.
#[derive(Clone, Copy, Debug)]
struct Move {
    target: usize,
    from: usize,
    /// Sent as the engine's `GoBack`/`GoForward`, not a `Navigate`.
    native: bool,
    deadline: Instant,
    /// The cursor is at `target`; the origin's old document may still report (in any order, some engines settling the
    /// target before the origin's last report), so its settled report is held until the deadline.
    committed: bool,
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
    /// Bumped by every eviction, which shifts the indices after it: an open Back/Forward popup recentres on it
    /// (`popups/dropdowns.rs`). Page ids do not need it: they carry the page's `serial`.
    epoch: u32,
    /// The serial the next pushed entry takes.
    next_serial: u32,
    slot: Slot,
    /// The last folded report was still loading: a URL change next is a redirect.
    loading: bool,
    /// The cursor entry was chosen by a move or reloaded by a new engine, so a redirect rewrites it.
    fresh: bool,
    /// Changed since the last save.
    dirty: bool,
    /// The child tab Back last left this tab for, at the root of the child's trail (decision 7): Forward goes there
    /// when the trail has no Forward of its own. Not saved, not part of `dirty`.
    left_to: Option<NodeId>,
}

impl NavHistory {
    /// Folds in one `NavigationStateChanged` (many repeat the same URL). `title` goes to the cursor entry when it is
    /// non-empty and the report is settled or repeats the cursor's URL (decision 11). Returns whether the report was
    /// the old document of a committed move finishing (step 3): the tab keeps the cursor's URL and title.
    pub(crate) fn observe(
        &mut self,
        url: &str,
        title: &str,
        loading: bool,
        can_go_forward: bool,
        now: Instant,
    ) -> bool {
        let before = self.saved_form();
        let same_page = self.entries.get(self.at).is_some_and(|e| e.url == url);
        let stale = self.is_stale_origin(url, loading, now);
        if self.fold(url, loading, can_go_forward, now)
            && !title.is_empty()
            && (same_page || !loading)
            && self.entries[self.at].url == url
        {
            self.entries[self.at].title = title.to_owned();
        }
        self.dirty |= before != self.saved_form();
        stale
    }

    /// Step 3's hold: a settled report of the origin while a committed move is still inside its window.
    fn is_stale_origin(&self, url: &str, loading: bool, now: Instant) -> bool {
        self.pending.is_some_and(|m| {
            m.committed
                && now <= m.deadline
                && !loading
                && self.entries[m.from].url == url
                && self.entries[m.target].url != url
        })
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
            let serial = self.take_serial();
            self.entries.push(Entry { url: url.to_owned(), title: String::new(), parent: None, fwd: None, serial });
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
        // 3: a pending move commits on its target's URL and waits through its origin's. A committed one holds the
        // origin's settled report until its deadline: the old document finishing, not a redirect (step 5).
        if let Some(m) = self.pending {
            if m.committed {
                if self.is_stale_origin(url, loading, now) {
                    return false;
                }
                if self.entries[m.target].url != url {
                    self.pending = None;
                }
            } else if self.entries[m.target].url == url {
                self.pending = Some(Move { committed: true, deadline: now + MOVE_WINDOW, ..m });
                self.commit(m.target, m.from, if m.native { LinStep::Native } else { LinStep::Push });
                self.loading = loading;
                return true;
            }
            if !m.committed && self.entries[m.from].url == url {
                // Servo reports only when a document commits: a slow target is quiet time, not total time.
                if let Some(p) = self.pending.as_mut() {
                    p.deadline = now + if loading { LOADING_WINDOW } else { MOVE_WINDOW };
                }
                self.settle(loading);
                return true;
            }
            if !m.committed {
                self.pending = None;
                self.fresh = false;
            }
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

    pub(crate) fn left_to(&self) -> Option<NodeId> {
        self.left_to
    }

    pub(crate) fn set_left_to(&mut self, child: Option<NodeId>) {
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
        self.pending.is_some_and(|m| !m.committed && now <= m.deadline)
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

    pub(crate) fn epoch(&self) -> u32 {
        self.epoch
    }

    pub(crate) fn slot(&self) -> Slot {
        self.slot
    }

    /// The serial of the page at arena index `entry`.
    #[cfg(test)]
    pub(crate) fn serial_of(&self, entry: usize) -> Option<u32> {
        self.entries.get(entry).map(|e| e.serial)
    }

    /// The arena index of the page with `serial`; `None` once that page has been evicted.
    pub(crate) fn index_of(&self, serial: u32) -> Option<usize> {
        self.entries.iter().position(|e| e.serial == serial)
    }

    fn take_serial(&mut self) -> u32 {
        self.next_serial += 1;
        self.next_serial - 1
    }

    /// Numbers the entries `0..n` in order (a loaded or hand-built tree).
    fn renumber(&mut self) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            e.serial = i as u32;
        }
        self.next_serial = self.entries.len() as u32;
    }

    /// Every page in entry order (a parent always before its children), for drawing.
    pub(crate) fn tree(&self) -> Vec<TrailPage> {
        if self.entries.is_empty() {
            return Vec::new();
        }
        let path: HashSet<usize> = std::iter::successors(Some(self.at), |&i| self.entries[i].parent).collect();
        let labels = page_labels(&self.entries);
        self.entries
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (e, label))| TrailPage {
                parent: e.parent,
                label,
                url: e.url.clone(),
                serial: e.serial,
                cursor: i == self.at,
                on_path: path.contains(&i),
            })
            .collect()
    }

    /// A tree built by hand: `(url, parent, fwd)` per entry, cursor `at`, no `lin`.
    #[cfg(test)]
    pub(crate) fn with_tree(spec: &[(&str, Option<usize>, Option<usize>)], at: usize) -> NavHistory {
        let entries = spec
            .iter()
            .map(|&(url, parent, fwd)| Entry { url: url.into(), title: String::new(), parent, fwd, serial: 0 })
            .collect();
        let mut h = NavHistory { entries, at, ..NavHistory::default() };
        h.renumber();
        h
    }

    /// The Back chip shows its branch marker: the cursor entry has siblings.
    pub(crate) fn back_fork(&self) -> bool {
        !self.branches(Dir::Back).is_empty()
    }

    /// The Forward chip shows its branch marker: the cursor entry has more than one child.
    pub(crate) fn forward_fork(&self) -> bool {
        self.branches(Dir::Forward).len() > 1
    }

    /// EXPORTSTATE's view of this trail (plan D10): entry count, the cursor's depth (0 at the root) and URL, the
    /// branch markers, the engine-list shadow's cursor and length, and `left_to`.
    pub(crate) fn export(&self) -> serde_json::Value {
        let depth =
            std::iter::successors(self.entries.get(self.at).and_then(|e| e.parent), |&p| self.entries[p].parent)
                .count();
        serde_json::json!({
            "entries": self.entries.len(),
            "depth": depth,
            "url": self.entries.get(self.at).map(|e| e.url.as_str()),
            "back_fork": self.back_fork(),
            "forward_fork": self.forward_fork(),
            "lin_at": self.lin_at,
            "lin_len": self.lin.len(),
            "left_to": self.left_to,
        })
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
        self.pending = Some(Move { target, from, native, deadline: now + MOVE_WINDOW, committed: false });
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
        let serial = self.take_serial();
        self.entries.push(Entry {
            url: url.to_owned(),
            title: String::new(),
            parent: Some(self.at),
            fwd: None,
            serial,
        });
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
        self.epoch += 1;
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

/// Each entry's label: its title, or its URL's path. Where other entries read the same, what differs from them
/// (`disambiguate::qualifiers`), first so a clipped label keeps it: `q=cats page=2 · Results`.
fn page_labels(entries: &[Entry]) -> Vec<String> {
    let parts: Vec<UrlParts> = entries.iter().map(|e| UrlParts::parse(&e.url)).collect();
    // An untitled page is read by its address alone: pages that differ in query then lead with the difference.
    let bases: Vec<&str> = entries
        .iter()
        .zip(&parts)
        .map(|(e, p)| if e.title.is_empty() { UrlParts::parse(p.address).short() } else { e.title.as_str() })
        .collect();
    let rows: Vec<(&str, &str)> = bases.iter().copied().zip(entries.iter().map(|e| e.url.as_str())).collect();
    // The same page twice (a loop in the tree) differs in nothing: no number to show.
    crate::disambiguate::qualifiers(&rows, false)
        .into_iter()
        .zip(entries.iter().zip(&parts).zip(bases))
        .map(|(q, ((e, p), base))| match q {
            Some(q) => format!("{q} · {base}"),
            None if e.title.is_empty() => p.short().to_owned(),
            None => e.title.clone(),
        })
        .collect()
}

const FILE: ProfileFile = ProfileFile { file: "tab-history.json", what: "tab history" };

/// The saved trees. A missing file gives none; one that does not parse (the old format included) is moved aside.
pub(crate) fn load() -> HashMap<NodeId, NavHistory> {
    FILE.path().map_or_else(HashMap::new, |path| load_from(&path))
}

pub(crate) fn load_from(path: &Path) -> HashMap<NodeId, NavHistory> {
    let SavedTrees(saved) = crate::json_file::load_or_move_aside(path, FILE.what).ok().flatten().unwrap_or_default();
    saved
        .into_iter()
        .filter(|(_, s)| s.valid())
        .map(|(id, s)| {
            let mut h = NavHistory { entries: s.entries, at: s.at, ..NavHistory::default() };
            h.renumber();
            h.evict_to(MAX_ENTRIES);
            (id, h)
        })
        .collect()
}

/// Drops the trees of tabs `keep` refuses, then writes the rest (one-page trees add nothing and are left out) when
/// any changed or any was dropped.
pub(crate) fn save(lists: &mut HashMap<NodeId, NavHistory>, keep: impl Fn(NodeId) -> bool) {
    save_to(lists, keep, FILE.path().as_deref());
}

pub(crate) fn save_to(lists: &mut HashMap<NodeId, NavHistory>, keep: impl Fn(NodeId) -> bool, path: Option<&Path>) {
    let before = lists.len();
    lists.retain(|&id, _| keep(id));
    let dropped = lists.len() != before;
    if !dropped && !lists.values().any(|h| h.dirty) {
        return;
    }
    let Some(path) = path else { return };
    let saved: HashMap<NodeId, Saved> = lists
        .iter()
        .filter(|(_, h)| h.entries.len() > 1)
        .map(|(&id, h)| (id, Saved { entries: h.entries.clone(), at: h.at }))
        .collect();
    crate::json_file::save(&saved, path, FILE.what);
    lists.values_mut().for_each(|h| h.dirty = false);
}

#[cfg(test)]
mod tests;
