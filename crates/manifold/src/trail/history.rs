//! A tab's trail (spec docs/superpowers/specs/2026-10-14-trail-design.md): its pages as a tree, so Back and Forward
//! survive an engine replacement (crash restart, wake from sleep, engine swap), a close and a restart, and pages left
//! by navigating from the middle stay reachable as branches.
//!
//! Each entry has a parent (Back) and `fwd`, the child the cursor last left it for (Forward). An engine report
//! (`observe`) moves the cursor, rewrites a redirect, or forks a child. A Back or Forward (`go`), or a url the host
//! sends (`navigate`), is a pending move that commits when the engine reports its arrival and hides the page it left. `lin`/`lin_at` shadow the engine's own linear list: a move uses the
//! engine's `GoBack`/`GoForward` (scroll and form state kept) only when the engine's neighbour is the tree's, else it
//! is a plain `Navigate` to the entry's URL. Back at the root is the opened-from step (`trail/nav.rs`).
//!
//! The trees are saved to `tab-history.json`, keyed by node id: machine-local, not part of the shared session store
//! (a trail changes on every navigation; the store shares only user-started URL changes).

use crate::profile::file::ProfileFile;
use crate::store::model::NodeId;
use crate::url_parts::UrlParts;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// Most entries kept per tab, and most slots in the engine-list shadow.
const MAX_ENTRIES: usize = 50;
/// How long a move waits for the engine's next report after a settled one.
const MOVE_WINDOW: Duration = Duration::from_secs(3);
/// The same after a loading report, and before the move's first: a busy machine can take longer than `MOVE_WINDOW`
/// between two reports of one navigation (measured 2026-10-15, `gate sleep` history on WebView2 under load), a new
/// engine longer before its first (`2026-11-07-navlog-back`), and a slow server or Servo (which reports only when a
/// document commits) longer before a move's arrival.
const LOADING_WINDOW: Duration = Duration::from_secs(30);

/// How long the engine may stay quiet after a report (or after a move is sent) whose `loading` flag this is.
fn window(loading: bool) -> Duration {
    if loading {
        LOADING_WINDOW
    } else {
        MOVE_WINDOW
    }
}

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
struct SavedTrees(#[serde(deserialize_with = "crate::profile::file::lenient_map")] HashMap<NodeId, Saved>);

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

/// A navigation on its way, from the cursor: it commits at its arrival, and ends at its deadline, at the first report
/// of another URL, or when a load it started ends at `origin` (the 204 rule). Until it commits, every report of
/// `origin` is the old page's, hidden; after, its settled ones until the deadline.
#[derive(Clone, Debug)]
struct Move {
    to: To,
    /// The cursor's URL when the move started; the cursor stays there until the commit.
    origin: String,
    deadline: Instant,
    /// The cursor is at the target; the origin's old document may still report (in any order, some engines settling
    /// the target before the origin's last report), so its settled report is held until the deadline.
    committed: bool,
    /// A load began after the move did: a loading report of `origin` while the last report was settled. A settled one
    /// next means the navigation ended without leaving (an attachment, a 204).
    started: bool,
}

/// A URL an old document may still report: what a move the host replaced was still waiting on.
#[derive(Clone, Debug)]
struct Held {
    url: String,
    until: Instant,
    /// A committed move's origin: only its settled reports are an old document's; a loading one is a real return to
    /// it, and ends the hold.
    settled: bool,
}

impl Held {
    /// Whether this hold covers a report of `url` with this loading flag.
    fn covers(&self, url: &str, loading: bool) -> bool {
        self.url == url && !(loading && self.settled)
    }
}

/// Where a move goes.
#[derive(Clone, Debug)]
enum To {
    /// A trail entry, by the engine's `GoBack`/`GoForward`.
    Native(usize),
    /// A trail entry, by a `Navigate` to its URL.
    Plain(usize),
    /// A page the trail has not seen, by a `Navigate` to this URL (`navigate`): the first report of another URL is it,
    /// whatever the engine made of the URL sent. It becomes `Plain` at the commit.
    New(String),
}

impl To {
    fn target(&self) -> Option<usize> {
        match self {
            To::Native(t) | To::Plain(t) => Some(*t),
            To::New(_) => None,
        }
    }
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
    /// What moves the host replaced or ended were still waiting on (`retire`): every report of these is an old
    /// document's, held until `until`.
    held: Vec<Held>,
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
    /// the old page of a pending move (step 3): the tab keeps the URL and title it shows.
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
        let stale = self.is_stale_origin(url, loading, now) || self.holds(url, loading, now);
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

    /// Step 3's hold: a report of a pending move's origin inside its window. Before the commit, any but the one that
    /// ends a started load; after, a settled one.
    fn is_stale_origin(&self, url: &str, loading: bool, now: Instant) -> bool {
        self.pending.as_ref().is_some_and(|m| {
            now <= m.deadline
                && m.origin == url
                && match m.to.target() {
                    Some(t) if m.committed => !loading && self.entries[t].url != url,
                    _ => loading || !m.started,
                }
        })
    }

    /// Step 3's hold on what an ended move was waiting on (`held`).
    fn holds(&self, url: &str, loading: bool, now: Instant) -> bool {
        self.held.iter().any(|h| h.covers(url, loading) && now <= h.until)
    }

    /// What a save writes; a change in it is a change worth saving.
    fn saved_form(&self) -> (Vec<Entry>, usize) {
        (self.entries.clone(), self.at)
    }

    /// Decision 3's fold, steps 1 to 6. `false` when the report is set aside without touching the trail or `loading`: a
    /// held url's (step 3) or a loading report a settled one will decide (step 4b, plan D1).
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
        self.expire(now);
        // 3: a pending move waits through its origin's reports and commits at its arrival. A committed one holds the
        // origin's settled report until its deadline: the old document finishing, not a redirect (step 5). A report
        // of what an ended move was waiting on is held, and restarts its hold; a committed one's origin, settled only.
        self.held.retain(|h| now <= h.until && (h.url != url || h.covers(url, loading)));
        if let Some(h) = self.held.iter_mut().find(|h| h.covers(url, loading)) {
            h.until = now + window(loading);
            return false;
        }
        if let Some(m) = self.pending.clone() {
            let stale = self.is_stale_origin(url, loading, now);
            if m.committed {
                if stale {
                    return false;
                }
                if m.to.target().is_none_or(|t| self.entries[t].url != url) {
                    // Another page, or a load of the origin (a real return, which keeps no hold on it).
                    self.retire(now, false);
                    self.release(url);
                }
            } else if stale {
                let started = loading && !self.loading;
                if let Some(p) = self.pending.as_mut() {
                    p.started |= started;
                    // Servo reports only when a document commits: a slow target is quiet time, not total time.
                    p.deadline = now + window(loading);
                }
                self.settle(loading);
                return true;
            } else if m.origin == url {
                // The 204 rule: the load it started ended where it began. The cursor's own URL (step 4).
                self.pending = None;
            } else if let Some(t) = m.to.target().filter(|&t| self.entries[t].url == url) {
                let step = if matches!(m.to, To::Native(_)) { LinStep::Native } else { LinStep::Push };
                self.pending = Some(Move { committed: true, deadline: now + MOVE_WINDOW, ..m });
                self.commit(t, self.at, step);
                self.loading = loading;
                return true;
            } else if matches!(m.to, To::New(_)) {
                // A new engine navigation, never a redirect of the origin's own load (step 5). The push may evict,
                // which renumbers: the move names its target after it.
                self.pending = None;
                self.push_page(url);
                self.pending = Some(Move { to: To::Plain(self.at), committed: true, deadline: now + MOVE_WINDOW, ..m });
                self.loading = loading;
                return true;
            } else {
                // The page navigated on its own: the target won't come, but the origin's document may still report.
                self.retire(now, false);
                self.fresh = false;
            }
        }
        // 4: the cursor's own URL.
        if self.entries[self.at].url == url {
            self.settle(loading);
            return true;
        }
        // 4b: page script's history.back()/forward(), inferred from the URL; a loading report's flags are stale.
        let fits = self.inferred(url);
        if fits.iter().any(Option::is_some) {
            if loading {
                return false;
            }
            let last = self.lin_at + 2 == self.lin.len();
            let commits = |&(dir, _): &(Dir, usize)| can_go_forward || (dir == Dir::Forward && last);
            if let Some((_, target)) = fits.into_iter().flatten().find(commits) {
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
        // 6: a new page.
        self.push_page(url);
        true
    }

    /// A new engine navigation from the cursor to `url`: the equal child, else a new branch.
    fn push_page(&mut self, url: &str) {
        let child = self.child_or_push(url);
        self.entries[self.at].fwd = Some(child);
        self.at = child;
        self.fresh = false;
        self.lin_push(child);
        self.evict_to(MAX_ENTRIES);
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
    fn inferred(&self, url: &str) -> [Option<(Dir, usize)>; 2] {
        [Dir::Back, Dir::Forward].map(|dir| {
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

    /// A trail move awaits its arrival. A typed navigation (`To::New`) doesn't count: a move replaces it, as Back
    /// during a load leaves the committed page in a browser.
    pub(crate) fn move_pending(&self, now: Instant) -> bool {
        self.pending.as_ref().is_some_and(|m| !m.committed && m.to.target().is_some() && now <= m.deadline)
    }

    /// Whether the pending move, of any kind, has arrived; `None` with no move. A report that turns `Some(false)` into
    /// `Some(true)` is the arrival of the page the host sent.
    pub(crate) fn move_arrived(&self) -> Option<bool> {
        self.pending.as_ref().map(|m| m.committed)
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
            self.retire(now, true);
            self.release(&url);
            let before = self.saved_form();
            self.commit(target, from, if native { LinStep::Native } else { LinStep::Replace });
            self.dirty |= before != self.saved_form();
            return if native { action } else { NavAction::None };
        }
        let to = if native { To::Native(target) } else { To::Plain(target) };
        self.begin(to, now);
        action
    }

    /// The host sent the engine `url` (typed, Home, a history row): a `New` move, in place of any pending one, so the
    /// page it leaves can't report over it. None for the cursor's own URL, as in `start` (no report could tell the
    /// arrival from the origin), or on an empty trail (no origin; the engine's first report makes the root).
    pub(crate) fn navigate(&mut self, url: &str, now: Instant) {
        if self.entries.get(self.at).is_some_and(|e| e.url != url) {
            self.begin(To::New(url.to_owned()), now);
        } else {
            self.retire(now, true);
            self.release(url);
        }
    }

    /// Drops a move past its deadline (fold step 2), holding nothing. When it never arrived, returns the cursor's URL:
    /// the page the tab is still on, which the host shows again in place of the one it sent (`expire_trail_moves`).
    pub(crate) fn expire(&mut self, now: Instant) -> Option<String> {
        let m = self.pending.take_if(|m| now > m.deadline)?;
        (!m.committed).then(|| self.entries[self.at].url.clone())
    }

    /// Starts a move to `to` in place of any pending one.
    fn begin(&mut self, to: To, now: Instant) {
        self.retire(now, true);
        let origin = self.entries[self.at].url.clone();
        let sent = self.sent(&to).to_owned();
        self.release(&origin);
        self.release(&sent);
        // The engine has said nothing of this navigation yet: as long as a load may take (`LOADING_WINDOW`), until a
        // report restarts it (step 3).
        let deadline = now + LOADING_WINDOW;
        self.pending = Some(Move { to, origin, deadline, committed: false, started: false });
    }

    /// The URL the host sends the engine for a move to `to`.
    fn sent<'a>(&'a self, to: &'a To) -> &'a str {
        match to {
            To::Native(t) | To::Plain(t) => &self.entries[*t].url,
            To::New(url) => url,
        }
    }

    /// Ends the pending move: what it was still waiting on, its origin and (when the host replaced it, `by_host`) its
    /// unreached target, is held (`held`) for as long as a new move would wait. When the page moved on by itself, a
    /// committed move's origin is held as the committed rule holds it: settled reports only.
    fn retire(&mut self, now: Instant, by_host: bool) {
        let Some(m) = self.pending.take().filter(|m| now <= m.deadline) else { return };
        let until = now + window(self.loading);
        let arrival = (by_host && !m.committed).then(|| self.sent(&m.to).to_owned());
        let origin = Held { url: m.origin, until, settled: !by_host && m.committed };
        for held in [Some(origin), arrival.map(|url| Held { url, until, settled: false })].into_iter().flatten() {
            self.release(&held.url);
            self.held.push(held);
        }
    }

    /// `url` is where the tab is or is going: its reports are no old document's.
    fn release(&mut self, url: &str) {
        self.held.retain(|h| h.url != url);
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
        // The old engine's documents are gone with it.
        self.pending = None;
        self.held.clear();
        // The new engine is loading `url`, though it can take seconds to say so.
        self.loading = true;
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
    let SavedTrees(saved) =
        crate::profile::file::load_or_move_aside(path, FILE.what).ok().flatten().unwrap_or_default();
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
    crate::profile::file::save(&saved, path, FILE.what);
    lists.values_mut().for_each(|h| h.dirty = false);
}

#[cfg(test)]
mod tests;
