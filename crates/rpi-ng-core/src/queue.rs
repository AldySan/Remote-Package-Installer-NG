//! Persistent queue: items, state transitions, duplicate detection and
//! restart recovery.
//!
//! Recovery rule (`docs/ARCHITECTURE.md` §4.4): items whose outcome cannot be
//! known after a restart are marked [`ItemState::Unknown`] and are **never**
//! auto-resubmitted — the engine reconciles against the console when one is
//! connected, and resubmission is always a deliberate user act.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{
    model::{ItemId, ItemState, PartFile, PartId, QueueItem, unix_ms},
    pkg,
    settings::atomic_write,
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

/// Structural repairs applied while loading a queue document that was
/// hand-edited or corrupted. Empty for every file the app itself writes —
/// the app never emits duplicate ids, regressed counters or part-less items.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LoadRepairs {
    /// Items dropped because a later entry reused an existing id. Every
    /// id lookup (`get`/`remove`/`move_item`, engine state updates, UI row
    /// buttons) resolves to the FIRST match, so a duplicate id made the
    /// later item unreachable — its buttons and progress silently acted on
    /// the wrong row.
    pub dropped_duplicate_ids: Vec<ItemId>,
    /// `next_item` was at or below an existing item id: the next add would
    /// have handed out a colliding id. Raised to max id + 1.
    pub next_item_repaired: bool,
    /// `next_part` was at or below an existing part id. Raised to max id + 1.
    pub next_part_repaired: bool,
    /// Items with no files at all: kept visible but marked Invalid instead
    /// of letting the engine submit an install with zero URLs.
    pub invalidated_empty_items: Vec<ItemId>,
}

impl LoadRepairs {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// One human-readable log line per repair (shown in the activity panel).
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.dropped_duplicate_ids.is_empty() {
            out.push(format!(
                "queue.json contained duplicate item ids {} (kept the first entry of each; \
                 hand-edited entries can collide and make rows unmanageable)",
                self.dropped_duplicate_ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.next_item_repaired {
            out.push(
                "queue.json next_item was behind an existing id; raised past the highest id \
                 so new entries cannot collide"
                    .into(),
            );
        }
        if self.next_part_repaired {
            out.push(
                "queue.json next_part was behind an existing part id; raised past the highest id"
                    .into(),
            );
        }
        if !self.invalidated_empty_items.is_empty() {
            out.push(format!(
                "queue.json items {} had no files (marked invalid); remove them from the queue",
                self.invalidated_empty_items
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out
    }
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

    /// Load from `dir` (or the global data dir when `None`), discarding the
    /// repair report (see [`Queue::load_in_reported`]).
    pub fn load_in(dir: Option<&std::path::Path>) -> Self {
        Self::load_in_reported(dir).0
    }

    /// Load from `dir` (or the global data dir when `None`), reporting the
    /// structural repairs applied to a hand-edited or corrupted document.
    pub fn load_in_reported(dir: Option<&std::path::Path>) -> (Self, LoadRepairs) {
        let Some(dir) = dir else {
            return (Self::default(), LoadRepairs::default());
        };
        for candidate in [dir.join(QUEUE_FILE), dir.join(QUEUE_BAK)] {
            if let Ok(text) = std::fs::read_to_string(&candidate)
                && let Ok(doc) = serde_json::from_str::<QueueDoc>(&text)
            {
                return Self::from_doc(doc);
            }
        }
        (Self::default(), LoadRepairs::default())
    }

    /// Load from a JSON document (test/inspection entry point; applies the
    /// same recovery mapping as [`Queue::load`]).
    pub fn from_json(text: &str) -> Result<Self, QueueError> {
        Ok(Self::from_json_reported(text)?.0)
    }

    /// Load from a JSON document, reporting structural repairs (test entry
    /// point mirroring [`Queue::load_in_reported`]).
    pub fn from_json_reported(text: &str) -> Result<(Self, LoadRepairs), QueueError> {
        let doc = serde_json::from_str::<QueueDoc>(text)?;
        Ok(Self::from_doc(doc))
    }

    fn from_doc(doc: QueueDoc) -> (Self, LoadRepairs) {
        let mut repairs = LoadRepairs::default();
        let mut q = Queue {
            items: doc.items,
            next_item: doc.next_item,
            next_part: doc.next_part,
        };

        // Hand-edited documents may repeat an item id. Every lookup resolves
        // to the first match, so later duplicates are unreachable zombies —
        // drop them and keep the first entry (the one all live references
        // already point at).
        let mut seen: HashSet<ItemId> = HashSet::new();
        let mut kept = Vec::with_capacity(q.items.len());
        for item in q.items.drain(..) {
            if !seen.insert(item.id) {
                repairs.dropped_duplicate_ids.push(item.id);
                continue;
            }
            kept.push(item);
        }
        q.items = kept;

        // Counters must sit past every existing id, or the next add hands out
        // a colliding id. Saturating: a hand-entered u64::MAX must not panic.
        let max_item = q.items.iter().map(|i| i.id).max();
        let want_item = max_item.map_or(q.next_item, |m| q.next_item.max(m.saturating_add(1)));
        if want_item != q.next_item {
            q.next_item = want_item;
            repairs.next_item_repaired = true;
        }
        let max_part = q
            .items
            .iter()
            .flat_map(|i| i.parts.iter().map(|p| p.id))
            .max();
        let want_part = max_part.map_or(q.next_part, |m| q.next_part.max(m.saturating_add(1)));
        if want_part != q.next_part {
            q.next_part = want_part;
            repairs.next_part_repaired = true;
        }

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
            // A part-less item (hand edit / corruption) would otherwise be
            // submitted with zero files. Keep it visible and explain why.
            if item.parts.is_empty() && item.state != ItemState::Invalid {
                repairs.invalidated_empty_items.push(item.id);
                item.state = ItemState::Invalid;
                item.error = Some("no files in this entry (hand-edited queue.json?)".into());
                item.updated_at = unix_ms();
            }
        }
        (q, repairs)
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
                task_owner: None,
                job_id: None,
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
            let Some(cid) = item.content_id() else {
                continue;
            };
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
        // A fresh attempt must not inherit the previous attempt's leftovers:
        // `attempts` only ever grows during a submit cycle, so keeping it
        // meant a user-retried item got *zero* automatic submit retries once
        // one cycle had exhausted the budget (every transient blip then went
        // straight to Failed). The v2 job id belongs to the old console-side
        // job, not to the attempt about to be created.
        item.job_id = None;
        item.task_owner = None;
        item.attempts = 0;
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
        self.items
            .iter()
            .find(|i| i.state == ItemState::Queued)
            .map(|i| i.id)
    }

    pub fn any_active(&self) -> bool {
        self.items.iter().any(|i| i.state.is_active())
    }

    pub fn stats(&self) -> QueueStats {
        let mut stats = QueueStats::default();
        for item in &self.items {
            match item.state {
                ItemState::Queued | ItemState::Preparing | ItemState::Submitting => {
                    stats.pending += 1
                }
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
