//! Session-local undo and redo of drops and folds. Docs: `docs/superpowers/specs/2026-10-19-move-undo-design.md`.
//! In memory only: not saved, not synced. A revert is an ordinary edit through `edit_group`, `edit_parent` and
//! `edit_pos`, so other devices see it as one.

use super::*;

/// How many steps the history keeps, each way.
pub(crate) const UNDO_DEPTH: usize = 10;

pub(crate) enum UndoKind {
    /// Dissolve, Ungroup All, Merge: each group and where `fold_group` sent its members. Undo reopens the groups,
    /// redo folds them again.
    Fold(Vec<(String, Option<String>)>),
}

pub(crate) struct UndoEntry {
    pub(crate) serial: u64,
    pub(crate) message: String,
    pub(crate) kind: UndoKind,
}

#[derive(Default)]
pub(crate) struct UndoHistory {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    next: u64,
}

impl UndoHistory {
    /// A fresh step: onto `undo`, and nothing left to redo.
    pub(crate) fn record(&mut self, message: String, kind: UndoKind) -> u64 {
        self.next += 1;
        self.redo.clear();
        self.undo.push(UndoEntry { serial: self.next, message, kind });
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.next
    }

    /// Takes entry `serial` (the newest when `None`) off `redo` when `from_redo`, else off `undo`.
    pub(crate) fn take(&mut self, serial: Option<u64>, from_redo: bool) -> Option<UndoEntry> {
        let list = if from_redo { &mut self.redo } else { &mut self.undo };
        let at = match serial {
            Some(serial) => list.iter().position(|e| e.serial == serial)?,
            None => list.len().checked_sub(1)?,
        };
        Some(list.remove(at))
    }

    /// Puts an entry `take` gave from `undo` onto `redo` (`to_redo`), or one from `redo` back onto `undo`.
    pub(crate) fn put(&mut self, entry: UndoEntry, to_redo: bool) {
        let list = if to_redo { &mut self.redo } else { &mut self.undo };
        list.push(entry);
        if list.len() > UNDO_DEPTH {
            list.remove(0);
        }
    }
}

impl App {
    /// A new step: remembers it and offers its Undo in a toast.
    pub(crate) fn record_undo(&mut self, message: String, kind: UndoKind) {
        let serial = self.undo.record(message.clone(), kind);
        self.toasts.undo = Some((message.clone(), serial, false));
        self.queue_toast(message);
    }

    /// Undoes (or, with `redo`, redoes) entry `serial`, the newest when `None`, and offers the way back in a toast.
    /// An entry that restored nothing is dropped: its cards have all moved on.
    pub(crate) fn step_history(&mut self, serial: Option<u64>, redo: bool) {
        let Some(entry) = self.undo.take(serial, redo) else { return };
        let (done, total) = self.replay(&entry.kind, redo);
        let verb = if redo { "Redone" } else { "Undone" };
        let message = match (done, total) {
            (0, _) => format!("Nothing to {}: it has changed since", if redo { "redo" } else { "undo" }),
            (d, t) if d == t => format!("{verb}: {}", entry.message),
            (d, t) => format!("{verb} {d} of {t}: {}", entry.message),
        };
        if done > 0 {
            let serial = entry.serial;
            self.undo.put(entry, !redo);
            self.toasts.undo = Some((message.clone(), serial, !redo));
        }
        self.queue_toast(message);
        self.save_session();
    }

    /// Applies `kind` one way; returns how many of its parts took effect and how many there were.
    fn replay(&mut self, kind: &UndoKind, redo: bool) -> (usize, usize) {
        let UndoKind::Fold(groups) = kind;
        self.replay_fold(groups, redo)
    }

    /// Undo reopens each group (`reopen_node`, which puts back the members still where the fold sent them); redo
    /// folds each again as the group is now, if it and its destination are shown.
    fn replay_fold(&mut self, groups: &[(String, Option<String>)], redo: bool) -> (usize, usize) {
        let mut done = 0;
        for (group, dest) in groups {
            if redo {
                if self.nodes.contains_key(group) && dest.as_ref().is_none_or(|d| self.nodes.contains_key(d)) {
                    self.fold_group(group, dest.clone());
                    done += 1;
                }
            } else if let Ok(node) = group.parse() {
                self.reopen_node(node);
                done += 1;
            }
        }
        (done, groups.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(tag: &str) -> UndoKind {
        UndoKind::Fold(vec![(tag.to_owned(), None)])
    }

    /// A new record empties `redo`; undoing and redoing only move an entry between the lists.
    #[test]
    fn a_record_empties_redo_but_a_step_does_not() {
        let mut h = UndoHistory::default();
        let a = h.record("a".into(), fold("a"));
        let b = h.record("b".into(), fold("b"));
        let entry = h.take(None, false).expect("newest");
        assert_eq!(entry.serial, b);
        h.put(entry, true);
        assert_eq!(h.redo.len(), 1);
        let entry = h.take(Some(a), false).expect("by serial");
        h.put(entry, true);
        assert_eq!(h.redo.len(), 2, "a second undo keeps the first redo");
        h.record("c".into(), fold("c"));
        assert!(h.redo.is_empty(), "a fresh record clears redo");
    }

    /// The oldest entry goes past the cap, and serials are never reused.
    #[test]
    fn the_history_is_capped_and_serials_never_repeat() {
        let mut h = UndoHistory::default();
        let serials: Vec<u64> = (0..UNDO_DEPTH + 3).map(|i| h.record(i.to_string(), fold("x"))).collect();
        assert_eq!(h.undo.len(), UNDO_DEPTH);
        assert!(h.take(Some(serials[0]), false).is_none(), "the oldest is gone");
        assert!(serials.windows(2).all(|w| w[0] < w[1]));
    }
}
