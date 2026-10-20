//! The document and everything that tracks its sync with the store, held apart from the UI so `App`'s other
//! `impl`s reach it through one name. Its id-holding fields are covered by `every_id_keyed_field_is_forgotten_or_excused`.

use std::collections::HashMap;

use crate::{local_state, mirror, node_store, session_sync, store};

pub(crate) struct Doc {
    /// The session store (spec "Journal"): `None` before `open_store`, when
    /// `%APPDATA%` is unset, or when the file couldn't be opened (toasted).
    pub(crate) journal: Option<store::journal::Journal>,
    /// This installation's replica id (`profile::replica_id`): what a close writes to `closed`, and what `Ctrl+Shift+T`
    /// matches. Persisted, unlike the journal's per-launch actor.
    pub(crate) replica: u64,
    /// Ids for nodes made while there is no journal (unit tests, a failed
    /// open): this launch's own actor, so they parse as store ids.
    pub(crate) fallback_ids: node_store::NodeIdFallback,
    /// The decoded copy of the document's mirrored fields (`mirror::Mirror`).
    pub(crate) mirror: mirror::Mirror,
    /// What the next save commits, and when saves happened or are owed (see `session_sync::PendingWrites`).
    pub(crate) writes: session_sync::PendingWrites,
    /// Personal state (`LocalNode`: settled pos, title, zoom, muted, live url) of nodes this replica keeps out of
    /// the graph because they are closed or under a closed node (spec decision 8). Filled by `hide_node`, read
    /// back by a reopen (`apply_remote`), written to `session.mnf.local` with the rest. `forget_id` drops it.
    pub(crate) hidden_local: HashMap<String, local_state::LocalNode>,
    /// The personal-state file's write bookkeeping (see `local_state::LocalPersist`).
    pub(crate) local_persist: local_state::LocalPersist,
    /// Set once the store reports a repair (`take_store_recovery`): what
    /// was loaded may be missing nodes, so `prune_backing_dirs` doesn't run.
    pub(crate) store_recovered: bool,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            journal: None,
            replica: crate::profile::replica_id(),
            fallback_ids: node_store::NodeIdFallback::default(),
            writes: session_sync::PendingWrites::default(),
            mirror: mirror::Mirror::default(),
            hidden_local: HashMap::new(),
            local_persist: local_state::LocalPersist::default(),
            store_recovered: false,
        }
    }
}
