//! Named cookie jars as synced definitions (`App::jar_defs`, spec "Data model"). A jar is a `Kind::Jar` store node
//! but never a graph node: `session_sync` bridges these to the document in `shared_nodes`, `materialize` and
//! `remove_remotely_deleted`.

use crate::jar::{Jar, JarChoice, JarKey};
use crate::session_sync::{color_of, Fields, Shared};
use crate::store::model::{field, FieldValue, NodeId};
use crate::theme::TAG_COLOR_PALETTE;
use crate::{App, JarDef};

impl App {
    /// A new named jar with the next palette colour. Saved by the caller's `save_session`.
    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn create_jar(&mut self, name: &str) -> NodeId {
        let id: NodeId = self.alloc_node_id().parse().expect("alloc_node_id returns a store id");
        let color = Some(TAG_COLOR_PALETTE[self.jar_defs.len() % TAG_COLOR_PALETTE.len()]);
        self.jar_defs.insert(id, JarDef { name: name.trim().to_string(), color, ephemeral: false });
        id
    }

    pub(crate) fn jar_named(&self, name: &str) -> Option<NodeId> {
        self.jar_defs.iter().find(|(_, d)| d.name == name).map(|(id, _)| *id)
    }

    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn rename_jar(&mut self, id: NodeId, name: &str) {
        let name = name.trim();
        if let (false, Some(def)) = (name.is_empty(), self.jar_defs.get_mut(&id)) {
            def.name = name.to_string();
        }
    }

    /// Turns jar `id`'s ephemeral flag on or off (deviation 10): on deletes its file, keeping its cookies in memory
    /// until quit; off saves what it holds now, if it is loaded.
    pub(crate) fn set_jar_ephemeral(&mut self, id: NodeId, on: bool) {
        let Some(def) = self.jar_defs.get_mut(&id) else { return };
        if def.ephemeral == on {
            return;
        }
        def.ephemeral = on;
        let key = JarKey::Named(id);
        if on {
            if let Some(dir) = self.jars_dir() {
                if let Err(e) = Jar::delete_file(&dir, key) {
                    eprintln!("jar: could not delete {}: {e}", key.file_stem());
                }
            }
        } else if self.jars.contains_key(&key) {
            self.save_jar(key);
        }
    }

    /// The user deleted jar `id`: tabs and groups that named it fall back to the default jar (deviation 9), then it is
    /// forgotten. Its document node is deleted by the next save (`diff`: live no longer has it).
    #[allow(dead_code)] // The jars page (a later task) calls it.
    pub(crate) fn delete_jar(&mut self, id: NodeId) {
        for node in self.nodes.values_mut() {
            match &mut node.payload {
                crate::node::NodePayload::Tab(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Unloaded(t) if t.jar.jar == Some(id) => t.jar = Default::default(),
                crate::node::NodePayload::Group(g) if g.default_jar == Some(id) => g.default_jar = None,
                _ => {}
            }
        }
        self.forget_jar(id);
    }

    /// Drops jar `id` (a local or remote delete): its definition, its cookies and its file. A tab still running on it
    /// has its pulls dropped (`apply_pulled_cookies`).
    pub(crate) fn forget_jar(&mut self, id: NodeId) {
        self.jar_defs.remove(&id);
        let key = JarKey::Named(id);
        self.jars.remove(&key);
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.remove(&key);
        }
        if let Some(dir) = self.jars_dir() {
            if let Err(e) = Jar::delete_file(&dir, key) {
                eprintln!("jar: could not delete {}: {e}", key.file_stem());
            }
        }
    }

    /// `profile::jars_dir()`, or a test's temp dir. Never the real profile under test (`profile::dir()` is None).
    pub(crate) fn jars_dir(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        if let Some(dir) = &self.jars_dir_override {
            return Some(dir.clone());
        }
        crate::profile::jars_dir()
    }

    /// The only place a `jar` field becomes a key (spec "Resolution"). `true` when it names a jar that no longer
    /// exists (deleted, or not synced yet): the tab uses the default jar and its chip warns.
    pub(crate) fn resolved_jar(&self, choice: &JarChoice) -> (JarKey, bool) {
        match choice.jar {
            None => (JarKey::Default, false),
            Some(id) if self.jar_defs.contains_key(&id) => (JarKey::Named(id), false),
            Some(_) => (JarKey::Default, true),
        }
    }

    /// Loads `key`'s jar the first time a tab needs it (spec "Manifold state": named jars load lazily). An ephemeral
    /// jar starts empty and is never read from disk.
    pub(crate) fn ensure_jar_loaded(&mut self, key: JarKey) {
        if self.jars.contains_key(&key) {
            return;
        }
        let jar = match self.jars_dir() {
            Some(dir) if !self.is_ephemeral(key) => Jar::load(&dir, key),
            _ => Jar::default(),
        };
        self.publish_jar(key, jar);
    }

    /// Stores `jar` under `key` and copies it to the spawn threads' snapshot map.
    fn publish_jar(&mut self, key: JarKey, jar: Jar) {
        if let Ok(mut snapshots) = self.jar_snapshots.lock() {
            snapshots.insert(key, jar.clone());
        }
        self.jars.insert(key, jar);
    }

    fn is_ephemeral(&self, key: JarKey) -> bool {
        matches!(key, JarKey::Named(id) if self.jar_defs.get(&id).is_some_and(|d| d.ephemeral))
    }

    /// Writes `key`'s file unless it is ephemeral. A failure is logged (never cookie values) and leaves other jars
    /// alone.
    pub(crate) fn save_jar(&self, key: JarKey) {
        let (Some(dir), Some(jar)) = (self.jars_dir(), self.jars.get(&key)) else { return };
        if self.is_ephemeral(key) {
            return;
        }
        if let Err(e) = jar.save(&dir, key) {
            eprintln!("jar: could not save {}: {e}", key.file_stem());
        }
    }

    /// Empties `key`'s jar and saves it. Task 6 also switches the live tabs using it (their engines still hold the
    /// old cookies). A named jar that no longer exists is left alone, so no file is written for it.
    #[allow(dead_code)] // The jars page's "Clear cookies" (a later task) calls it.
    pub(crate) fn clear_jar_cookies(&mut self, key: JarKey) {
        if matches!(key, JarKey::Named(id) if !self.jar_defs.contains_key(&id)) {
            return;
        }
        self.ensure_jar_loaded(key);
        let Some(jar) = self.jars.get(&key) else { return };
        // Cleared in place, so a locked jar stays locked and its unreadable file is still never overwritten.
        let mut jar = jar.clone();
        jar.cookies.clear();
        self.publish_jar(key, jar);
        self.save_jar(key);
    }

    /// A `Kind::Jar` change from the document: applies the fields that differ from what this replica last saw.
    pub(crate) fn materialize_jar(&mut self, id: NodeId, fields: &Fields, seen: &Option<Shared>, now: &Shared) {
        let changed = |no: u32| seen.as_ref().is_none_or(|s| s.fields.get(&no) != now.fields.get(&no));
        let def =
            self.jar_defs.entry(id).or_insert_with(|| JarDef { name: String::new(), color: None, ephemeral: false });
        if changed(field::NAME) {
            def.name = match fields.get(&field::NAME) {
                Some(FieldValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
        }
        if changed(field::COLOR) {
            def.color = fields.get(&field::COLOR).and_then(color_of);
        }
        if changed(field::EPHEMERAL) {
            let on = matches!(fields.get(&field::EPHEMERAL), Some(FieldValue::Bool(true)));
            self.set_jar_ephemeral(id, on);
        }
    }
}
