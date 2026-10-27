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
