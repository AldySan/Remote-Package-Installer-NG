//! Queue tests: grouping, duplicate detection, state recovery, persistence.

use std::path::PathBuf;

use rpi_ng_core::{model::ItemState, queue::Queue};

fn write_pkg(dir: &std::path::Path, name: &str, size: u64) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, vec![0u8; size as usize]).unwrap();
    path
}

#[test]
fn groups_multipart_pieces_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let p0 = write_pkg(tmp.path(), "Game_1.pkg", 10);
    let p2 = write_pkg(tmp.path(), "Game_0.pkg", 10);
    let p1 = write_pkg(tmp.path(), "Game_2.pkg", 10);

    let mut q = Queue::default();
    let (added, skipped) = q.add_paths(vec![p0, p1, p2]);
    assert_eq!(added.len(), 1);
    assert!(skipped.is_empty());
    let item = q.get(added[0]).unwrap();
    assert_eq!(item.parts.len(), 3);
    // Ordered by piece index 0,1,2 regardless of insertion order.
    assert_eq!(item.parts[0].file_name, "Game_0.pkg");
    assert_eq!(item.parts[1].file_name, "Game_1.pkg");
    assert_eq!(item.parts[2].file_name, "Game_2.pkg");
    assert!(item.is_multipart());
}

#[test]
fn separate_items_for_unrelated_files() {
    let tmp = tempfile::tempdir().unwrap();
    let a = write_pkg(tmp.path(), "Alpha.pkg", 5);
    let b = write_pkg(tmp.path(), "Beta.pkg", 5);
    let mut q = Queue::default();
    let (added, _) = q.add_paths(vec![a, b]);
    assert_eq!(added.len(), 2);
}

#[test]
fn detects_duplicates_by_path() {
    let tmp = tempfile::tempdir().unwrap();
    let a = write_pkg(tmp.path(), "Alpha.pkg", 5);
    let mut q = Queue::default();
    let (added, _) = q.add_paths(vec![a.clone()]);
    assert_eq!(added.len(), 1);
    let (added2, skipped) = q.add_paths(vec![a]);
    assert!(added2.is_empty());
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("duplicate"));
}

#[test]
fn skips_missing_files() {
    let mut q = Queue::default();
    let (added, skipped) = q.add_paths(vec![PathBuf::from("/nonexistent/pkg.pkg")]);
    assert!(added.is_empty());
    assert_eq!(skipped.len(), 1);
}

#[test]
fn requeue_and_clear_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let a = write_pkg(tmp.path(), "A.pkg", 5);
    let b = write_pkg(tmp.path(), "B.pkg", 5);
    let mut q = Queue::default();
    let (added, _) = q.add_paths(vec![a, b]);
    let (id_a, id_b) = (added[0], added[1]);

    q.get_mut(id_a).unwrap().state = ItemState::Completed;
    q.get_mut(id_b).unwrap().state = ItemState::Failed;

    assert_eq!(q.clear_finished(), 2);
    assert!(q.items().is_empty());

    // Requeue path.
    let (added, _) = {
        let tmp2 = tempfile::tempdir().unwrap();
        let c = write_pkg(tmp2.path(), "C.pkg", 5);
        q.add_paths(vec![c])
    };
    let id = added[0];
    q.get_mut(id).unwrap().state = ItemState::Failed;
    assert!(q.requeue(id));
    assert_eq!(q.get(id).unwrap().state, ItemState::Queued);
}

#[test]
fn requeue_resets_the_previous_attempts_bookkeeping() {
    // A submit cycle that exhausted its transient-retry budget (attempts
    // grows to MAX_SUBMIT_ATTEMPTS - 1) used to leak into every later Retry:
    // the requeued item then had *zero* automatic submit retries left, so the
    // first transient blip failed it permanently. The stale v2 job id of the
    // old console-side job must go too.
    let tmp = tempfile::tempdir().unwrap();
    let a = write_pkg(tmp.path(), "A.pkg", 5);
    let mut q = Queue::default();
    let (added, _) = q.add_paths(vec![a]);
    let id = added[0];

    {
        let item = q.get_mut(id).unwrap();
        item.state = ItemState::Failed;
        item.error = Some("network error".into());
        item.task_id = Some(7);
        item.job_id = Some(3);
        item.attempts = 2; // what a fully-spent submit cycle leaves behind
    }
    assert!(q.requeue(id));
    let item = q.get(id).unwrap();
    assert_eq!(item.state, ItemState::Queued);
    assert_eq!(item.attempts, 0, "a fresh attempt gets a full retry budget");
    assert_eq!(
        item.job_id, None,
        "the old console job id must not carry over"
    );
    assert_eq!(item.task_id, None);
    assert!(item.error.is_none());
    assert!(item.progress.is_none());
}

#[test]
fn move_item_bounds() {
    let tmp = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["A.pkg", "B.pkg", "C.pkg"]
        .iter()
        .map(|n| write_pkg(tmp.path(), n, 5))
        .collect();
    let mut q = Queue::default();
    let (added, _) = q.add_paths(paths);
    let (a, b, c) = (added[0], added[1], added[2]);
    assert!(q.move_item(c, -1)); // C before B
    assert_eq!(
        q.items().iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![a, c, b]
    );
    assert!(!q.move_item(a, -1)); // already first
    assert!(!q.move_item(b, 1)); // already last (after moves: a,c,b)
}

#[test]
fn recovery_marks_active_states_unknown() {
    // Round-trip a queue document with an active item through from_json:
    // Transferring/Installing/Submitting/Preparing must become Unknown —
    // the never-auto-resubmit invariant.
    let doc = r#"{
      "next_item": 2,
      "next_part": 1,
      "items": [
        {"id":1,"parts":[{"id":0,"path":"/tmp/a.pkg","file_name":"a.pkg","size":10,"part_index":null,"pkg":null,"invalid_reason":null}],
         "state":"Transferring","error":null,"task_id":7,"console_title":null,"progress":null,"attempts":0,"added_at":0,"updated_at":0}
      ]
    }"#;
    let q = Queue::from_json(doc).unwrap();
    assert_eq!(q.items().len(), 1);
    assert_eq!(q.items()[0].state, ItemState::Unknown);
    assert_eq!(q.items()[0].task_id, Some(7)); // evidence retained for reconciliation

    // Finished states must survive recovery untouched.
    let doc2 = doc.replace("\"Transferring\"", "\"Completed\"");
    let q2 = Queue::from_json(&doc2).unwrap();
    assert_eq!(q2.items()[0].state, ItemState::Completed);

    // Corrupt documents fail cleanly.
    assert!(Queue::from_json("not json").is_err());
}

#[test]
fn stats_count_states() {
    let tmp = tempfile::tempdir().unwrap();
    let paths: Vec<_> = ["A.pkg", "B.pkg", "C.pkg", "D.pkg", "E.pkg"]
        .iter()
        .map(|n| write_pkg(tmp.path(), n, 5))
        .collect();
    let mut q = Queue::default();
    let (added, _) = q.add_paths(paths);
    q.get_mut(added[0]).unwrap().state = ItemState::Completed;
    q.get_mut(added[1]).unwrap().state = ItemState::Failed;
    q.get_mut(added[2]).unwrap().state = ItemState::Unknown;
    q.get_mut(added[3]).unwrap().state = ItemState::Cancelled;
    let stats = q.stats();
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.unknown, 1);
    assert_eq!(stats.cancelled, 1);
    assert_eq!(stats.pending, 1);
}

/// Queue files written before the `task_owner` field existed (v0.0.6 and
/// earlier) must keep loading: the field defaults to `None`, which the
/// engine treats permissively so legacy queues re-attach exactly as before.
#[test]
fn legacy_queue_doc_without_task_owner_loads() {
    let doc = r#"{
        "next_item": 1,
        "next_part": 1,
        "items": [
            {
                "id": 0,
                "parts": [
                    {
                        "id": 0,
                        "path": "/tmp/legacy.pkg",
                        "file_name": "legacy.pkg",
                        "size": 10,
                        "part_index": null
                    }
                ],
                "state": "Unknown",
                "error": null,
                "task_id": 7,
                "job_id": null,
                "console_title": null,
                "progress": null,
                "attempts": 0,
                "added_at": 1,
                "updated_at": 1
            }
        ]
    }"#;
    let q = Queue::from_json(doc).expect("legacy queue doc loads");
    let item = q.get(0).unwrap();
    assert_eq!(item.task_id, Some(7));
    assert_eq!(
        item.task_owner, None,
        "missing owner defaults to permissive"
    );
}

// ---------------------------------------------------------------------------
// Hand-edited queue.json (structural sanitization on load)
// ---------------------------------------------------------------------------

fn hand_edited_item(id: u64, part_id: u64, path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    format!(
        r#"{{"id":{id},"parts":[{{"id":{part_id},"path":"{path}","file_name":"{file_name}","size":10,"part_index":null,"pkg":null,"invalid_reason":null}}],
            "state":"Queued","error":null,"task_id":null,"task_owner":null,"job_id":null,"console_title":null,"progress":null,"attempts":0,"added_at":0,"updated_at":0}}"#
    )
}

fn hand_edited_doc(next_item: u64, next_part: u64, items: &[String]) -> String {
    format!(
        r#"{{"next_item":{next_item},"next_part":{next_part},"items":[{}]}}"#,
        items.join(",")
    )
}

/// A pristine document — everything the app itself writes — reports zero
/// repairs. This pins that sanitization never touches normal files.
#[test]
fn well_formed_doc_reports_no_repairs() {
    let doc = hand_edited_doc(
        3,
        3,
        &[
            hand_edited_item(0, 0, "/tmp/a.pkg"),
            hand_edited_item(1, 1, "/tmp/b.pkg"),
            hand_edited_item(2, 2, "/tmp/c.pkg"),
        ],
    );
    let (_, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert!(repairs.is_empty(), "unexpected repairs: {repairs:?}");
    assert!(repairs.warnings().is_empty());
}

/// The user-iseng case: an entry copy-pasted so two items share one id. The
/// second entry is unreachable (every lookup resolves to the first match) —
/// its buttons would have acted on the wrong row. Keep the first, drop the
/// rest, and say so.
#[test]
fn duplicate_item_ids_are_dropped_first_wins() {
    let doc = hand_edited_doc(
        3,
        2,
        &[
            hand_edited_item(1, 0, "/tmp/real.pkg"),
            hand_edited_item(2, 1, "/tmp/other.pkg"),
            // Copy of item 1 with a different file — must disappear.
            hand_edited_item(1, 1, "/tmp/copy.pkg"),
        ],
    );
    let (q, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert_eq!(q.items().len(), 2);
    assert_eq!(repairs.dropped_duplicate_ids, vec![1]);
    // The survivor of id 1 is the FIRST entry.
    assert_eq!(q.get(1).unwrap().parts[0].file_name, "real.pkg");
    assert_eq!(q.get(2).unwrap().parts[0].file_name, "other.pkg");
    // The warning mentions the id, so the activity panel explains the loss.
    assert!(
        repairs
            .warnings()
            .iter()
            .any(|w| w.contains("duplicate item ids 1"))
    );
}

/// next_item behind an existing id (user renumbered entries): the next add
/// would have reused a live id and corrupted that item. Raised past the max.
#[test]
fn regressed_next_item_is_raised_past_the_highest_id() {
    let doc = hand_edited_doc(
        2, // behind: items already sit at 1..=3
        3,
        &[
            hand_edited_item(1, 0, "/tmp/a.pkg"),
            hand_edited_item(2, 1, "/tmp/b.pkg"),
            hand_edited_item(3, 2, "/tmp/c.pkg"),
        ],
    );
    let (q, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert!(repairs.next_item_repaired);

    // The proof: add a real file and its id must not collide with 1..=3.
    let tmp = tempfile::tempdir().unwrap();
    let new = write_pkg(tmp.path(), "New.pkg", 5);
    let mut q = q;
    let (added, _) = q.add_paths(vec![new]);
    assert_eq!(
        added,
        vec![4],
        "fresh ids continue past the highest existing"
    );
    // And the new item does not shadow any restored one.
    assert!(q.get(1).is_some());
    assert!(q.get(2).is_some());
    assert!(q.get(3).is_some());
    assert_eq!(q.items().len(), 4);
}

/// Same repair for the part counter (observable on the next add).
#[test]
fn regressed_next_part_is_raised_past_the_highest_part_id() {
    let doc = hand_edited_doc(
        2,
        0, // behind: parts already sit at 5..=6
        &[
            hand_edited_item(1, 5, "/tmp/a.pkg"),
            hand_edited_item(2, 6, "/tmp/b.pkg"),
        ],
    );
    let (q, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert!(repairs.next_part_repaired);
    let tmp = tempfile::tempdir().unwrap();
    let new = write_pkg(tmp.path(), "New.pkg", 5);
    let mut q = q;
    let (added, _) = q.add_paths(vec![new]);
    let part_id = q.get(added[0]).unwrap().parts[0].id;
    assert!(
        part_id > 6,
        "new part ids continue past the highest existing"
    );
}

/// An entry with no files would be submitted as an install with zero URLs;
/// it stays visible as Invalid (with a reason) instead of vanishing silently.
#[test]
fn partless_item_is_marked_invalid_not_dropped() {
    let doc = r#"{"next_item":2,"next_part":1,"items":[
        {"id":1,"parts":[],"state":"Queued","error":null,"task_id":null,"task_owner":null,"job_id":null,"console_title":null,"progress":null,"attempts":0,"added_at":0,"updated_at":0}
    ]}"#;
    let (q, repairs) = Queue::from_json_reported(doc).unwrap();
    assert_eq!(q.items().len(), 1, "the entry stays visible");
    let item = q.get(1).unwrap();
    assert_eq!(item.state, ItemState::Invalid);
    assert!(item.error.as_deref().unwrap().contains("no files"));
    assert_eq!(repairs.invalidated_empty_items, vec![1]);
}

/// A hand-entered u64::MAX id must not panic in debug builds (overflow) —
/// the counter saturates instead.
#[test]
fn saturating_counter_repair_on_max_id() {
    let doc = hand_edited_doc(0, 0, &[hand_edited_item(u64::MAX, 0, "/tmp/max.pkg")]);
    let (q, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert!(repairs.next_item_repaired);
    assert_eq!(q.items().len(), 1);
}

/// Repairs survive a save/load round trip: what the app writes back after
/// fixing a hand-edited file is pristine (second load reports nothing).
#[test]
fn sanitized_queue_round_trips_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = hand_edited_doc(
        1,
        0,
        &[
            hand_edited_item(1, 5, "/tmp/a.pkg"),
            hand_edited_item(1, 6, "/tmp/dup.pkg"),
        ],
    );
    let (q, repairs) = Queue::from_json_reported(&doc).unwrap();
    assert!(!repairs.is_empty());
    q.save_in(Some(tmp.path())).unwrap();

    let (q2, repairs2) = Queue::load_in_reported(Some(tmp.path()));
    assert!(
        repairs2.is_empty(),
        "second load must be pristine: {repairs2:?}"
    );
    assert_eq!(q2.items().len(), 1);
    assert_eq!(q2.items()[0].id, 1);
}
