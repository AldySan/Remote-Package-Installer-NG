//! Persistent queue: items, state transitions, duplicate detection and
//! restart recovery.
//!
//! Recovery rule (`docs/ARCHITECTURE.md` §4.4): items whose outcome cannot be
//! known after a restart are marked [`ItemState::Unknown`] and are **never**
//! auto-resubmitted — the engine reconciles against the console when one is
//! connected, and resubmission is always a deliberate user act.

use std::{collections::HashSet, path::{Path, PathBuf}};

use crate::{
    model::{unix_ms, ItemId, ItemState, PartFile, PartId, QueueItem},
    pkg,
    settings::{atomic_write, data_dir},
};

const QUEUE_FILE: &str = "queue.json";
const QUEUE_BAK: &str = "queue.json.bak";

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// The queue, persisted as JSON after every transition (atomic writes).
#[derive(Debug, Default)]
pub struct Queue {
    items: Vec<QueueItem>,
    next_item: ItemId,
    next_part: PartId,
}

impl Queue {
    pub fn load() -> Self {
        Self::load_in(crate::settings::data_dir().as_deref())
    }

    /// Load from `dir` (or the global data dir when `None`).
    pub fn load_in(dir: Option<&std::path::Path>) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        for candidate in [dir.join(QUEUE_FILE), dir.join(QUEUE_BAK)] {
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                if let Ok(doc) = serde_json::from_str::<QueueDoc>(&text) {
                    return Self::from_doc(doc);
                }
            }
        }
        Self::default()
    }

    /// Load from a JSON document (test/inspection entry point; applies the
    /// same recovery mapping as [`Queue::load`]).
    pub fn from_json(text: &str) -> Result<Self, QueueError> {
        let doc = serde_json::from_str::<QueueDoc>(text)?;
        Ok(Self::from_doc(doc))
    }

    fn from_doc(doc: QueueDoc) -> Self {
        let mut q = Queue {
            items: doc.items,
            next_item: doc.next_item,
            next_part: doc.next_part,
        };
        // Recovery: unknown-outcome states become Unknown — never auto-resumed.
        for item in &mut q.items {
            match item.state {
                ItemState::Transferring
                | ItemState::Installing
                | ItemState::Submitting
                | ItemState::Preparing => {
                    item.state = ItemState::Unknown;
                    item.updated_at = unix_ms();
                }
                _ => {}
            }
        }
        q
    }

    fn to_doc(&self) -> QueueDoc {
        QueueDoc {
            next_item: self.next_item,
            next_part: self.next_part,
            items: self.items.clone(),
        }
    }

    pub fn save(&self) -> Result<(), QueueError> {
        self.save_in(crate::settings::data_dir().as_deref())
    }

    pub fn save_in(&self, dir: Option<&std::path::Path>) -> Result<(), QueueError> {
        let Some(dir) = dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let path = dir.join(QUEUE_FILE);
        if path.exists() {
            let _ = std::fs::copy(&path, dir.join(QUEUE_BAK));
        }
        let text = serde_json::to_string_pretty(&self.to_doc())?;
        let tmp = dir.join("queue.json.tmp");
        atomic_write(&tmp, &path, text.as_bytes())?;
        Ok(())
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [QueueItem] {
        &mut self.items
    }

    pub fn get_mut(&mut self, id: ItemId) -> Option<&mut QueueItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    pub fn get(&self, id: ItemId) -> Option<&QueueItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Add file paths (already scanned, sizes known via metadata). Files are
    /// parsed lazily by the engine; here they become parts/items with
    /// multipart grouping and duplicate detection.
    ///
    /// Returns the ids of newly added items (duplicates and unreadable files
    /// are skipped with reasons returned for logging).
    pub fn add_paths(&mut self, paths: Vec<PathBuf>) -> (Vec<ItemId>, Vec<String>) {
        let mut added = Vec::new();
        let mut skipped = Vec::new();

        // Existence + size pass.
        let mut files: Vec<(PathBuf, u64)> = Vec::new();
        for path in paths {
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => files.push((path, m.len())),
                Ok(_) => skipped.push(format!("not a file: {}", path.display())),
                Err(e) => skipped.push(format!("unreadable: {} ({e})", path.display())),
            }
        }

        // Multipart grouping by (base name, directory).
        #[derive(Debug, PartialEq, Eq, Hash, Clone, Ord, PartialOrd)]
        struct GroupKey {
            base: String,
            dir: PathBuf,
        }
        let mut groups: std::collections::BTreeMap<GroupKey, Vec<(PathBuf, u64, Option<u32>)>> =
            std::collections::BTreeMap::new();
        for (path, size) in files {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (base, part_index) = pkg::split_part_name(&file_name);
            let dir = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
            groups
                .entry(GroupKey { base, dir })
                .or_default()
                .push((path, size, part_index));
        }

        // Existing keys for dedup.
        let existing: HashSet<String> = self.items.iter().map(|i| i.dedup_key()).collect();
        let mut keys_of_new: Vec<String> = Vec::new();

        for (_key, mut group) in groups {
            // Sort pieces by index (None first) then by name for stable order.
            group.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

            let parts: Vec<PartFile> = group
                .into_iter()
                .map(|(path, size, part_index)| {
                    let file_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "package.pkg".into());
                    PartFile {
                        id: self.next_part,
                        path,
                        file_name,
                        size,
                        part_index,
                        pkg: None,
                        invalid_reason: None,
                    }
                })
                .collect();
            self.next_part += parts.len() as PartId;

            let now = unix_ms();
            let item = QueueItem {
                id: self.next_item,
                parts,
                state: ItemState::Queued,
                error: None,
                task_id: None,
                console_title: None,
                progress: None,
                attempts: 0,
                added_at: now,
                updated_at: now,
            };
            self.next_item += 1;

            // Dedup: skip if an equivalent key already exists (in queue or in
            // this batch). Content ids are not parsed yet at add time, so the
            // pre-parse key is the first part's canonical path; the engine
            // re-checks with content ids after parsing.
            let key = item.dedup_key();
            if existing.contains(&key) || keys_of_new.contains(&key) {
                skipped.push(format!("duplicate: {}", key));
                // Parts were registered with ids but never exposed to the
                // server yet — nothing to unregister.
                continue;
            }
            keys_of_new.push(key);
            added.push(item.id);
            self.items.push(item);
        }

        (added, skipped)
    }

    /// Replace a placeholder dedup check after metadata parsing: returns ids
    /// of items whose parsed content id duplicates another item's.
    pub fn duplicates_by_content_id(&self) -> Vec<(ItemId, ItemId)> {
        let mut first_by_cid: std::collections::HashMap<String, ItemId> =
            std::collections::HashMap::new();
        let mut dupes = Vec::new();
        for item in &self.items {
            if item.state.is_finished() {
                continue;
            }
            let Some(cid) = item.content_id() else { continue };
            let key = format!("cid:{cid}");
            match first_by_cid.get(&key) {
                Some(first) if *first != item.id => dupes.push((*first, item.id)),
                _ => {
                    first_by_cid.insert(key, item.id);
                }
            }
        }
        dupes
    }

    pub fn remove(&mut self, id: ItemId) -> Option<QueueItem> {
        let pos = self.items.iter().position(|i| i.id == id)?;
        Some(self.items.remove(pos))
    }

    pub fn move_item(&mut self, id: ItemId, delta: i32) -> bool {
        let Some(pos) = self.items.iter().position(|i| i.id == id) else {
            return false;
        };
        let new_pos = pos as i64 + delta as i64;
        if new_pos < 0 || new_pos >= self.items.len() as i64 {
            return false;
        }
        let new_pos = new_pos as usize;
        let item = self.items.remove(pos);
        self.items.insert(new_pos, item);
        true
    }

    /// Requeue a finished/failed/cancelled/unknown item for a fresh attempt.
    pub fn requeue(&mut self, id: ItemId) -> bool {
        let Some(item) = self.get_mut(id) else {
            return false;
        };
        if item.state.is_active() {
            return false;
        }
        item.state = ItemState::Queued;
        item.error = None;
        item.task_id = None;
        item.progress = None;
        item.updated_at = unix_ms();
        true
    }

    pub fn clear_finished(&mut self) -> usize {
        let before = self.items.len();
        self.items.retain(|i| !i.state.is_finished());
        before - self.items.len()
    }

    /// Next item to process (first queued, in order).
    pub fn next_queued(&self) -> Option<ItemId> {
        self.items.iter().find(|i| i.state == ItemState::Queued).map(|i| i.id)
    }

    pub fn any_active(&self) -> bool {
        self.items.iter().any(|i| i.state.is_active())
    }

    pub fn stats(&self) -> QueueStats {
        let mut stats = QueueStats::default();
        for item in &self.items {
            match item.state {
                ItemState::Queued | ItemState::Preparing | ItemState::Submitting => stats.pending += 1,
                ItemState::Transferring | ItemState::Installing => stats.active += 1,
                ItemState::Paused => stats.paused += 1,
                ItemState::Completed => stats.completed += 1,
                ItemState::Failed | ItemState::Invalid => stats.failed += 1,
                ItemState::Cancelled => stats.cancelled += 1,
                ItemState::Unknown => stats.unknown += 1,
            }
        }
        stats
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    pub pending: usize,
    pub active: usize,
    pub paused: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub unknown: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct QueueDoc {
    next_item: ItemId,
    next_part: PartId,
    items: Vec<QueueItem>,
}
