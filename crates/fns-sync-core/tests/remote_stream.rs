mod support;

use std::fs;

use fns_protocol::{WorkspaceMutationKind, WorkspaceRevision};
use fns_sync_core::{ConflictStatus, SyncCommand, SyncError};

#[test]
fn ack_is_emitted_only_after_every_remote_postimage_is_durable() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("a.txt", 10, b"old");
    fixture.seed_remote_file("b.txt", 10, b"gone");
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(10))
        .unwrap();
    fixture
        .engine
        .state_mut()
        .set_last_applied_revision(WorkspaceRevision::new(10))
        .unwrap();
    let begin = fixture.incremental_begin(10, 12, 2, 0);
    let update = fixture.remote_update_event(0, 11, "a.txt", b"server");
    let delete = fixture.remote_delete_event(1, 12, "b.txt");

    fixture.engine.snapshot_begin(begin).unwrap();
    assert!(
        fixture
            .engine
            .workspace_event(update)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    assert!(fixture.engine.workspace_event(delete).unwrap().is_empty());
    assert!(
        fixture
            .engine
            .snapshot_end(fixture.incremental_end(12, 2, 0))
            .unwrap()
            .is_empty()
    );
    let before_blobs = fixture.engine.pending_commands(16).unwrap();
    assert!(before_blobs.iter().any(support::is_download));
    assert!(support::ack_revisions(&before_blobs).is_empty());
    fixture.provide_requested_blobs();

    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(support::ack_revisions(&commands), vec![12]);
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 10);
    fixture.engine.ack_confirmed(fixture.ack(12)).unwrap();
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 12);
}

#[test]
fn live_remote_file_event_downloads_and_applies_without_active_stream() {
    let mut fixture = support::EngineFixture::new();
    let event = fixture.remote_update_event(0, 1, "remote.txt", b"server");

    let commands = fixture.engine.event(event).unwrap();
    assert!(commands.iter().any(support::is_download));
    assert!(!fixture.path("remote.txt").exists());

    fixture.provide_requested_blobs();

    assert_eq!(fs::read(fixture.path("remote.txt")).unwrap(), b"server");
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(support::ack_revisions(&commands), vec![1]);
}

#[test]
fn begin_identity_mode_and_count_are_validated() {
    let mut fixture = support::EngineFixture::new();
    let mut invalid = fixture.incremental_begin(0, 1, 1, 0);
    invalid.entry_count = 1;
    assert!(matches!(
        fixture.engine.snapshot_begin(invalid),
        Err(SyncError::StreamInvariant { .. }) | Err(SyncError::ProtocolInvariant { .. })
    ));
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert!(!fixture.path("anything").exists());

    let mut mismatched = fixture.incremental_begin(0, 1, 0, 0);
    mismatched.workspace_id =
        fns_protocol::WorkspaceId::parse("20000000-0000-4000-8000-000000000001").unwrap();
    assert!(matches!(
        fixture.engine.snapshot_begin(mismatched),
        Err(SyncError::ProtocolInvariant { .. })
    ));
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
}

#[test]
fn event_indices_are_contiguous_only_within_event_items() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 3, 3, 0))
        .unwrap();
    let first = fixture.remote_update_event(0, 1, "first.txt", b"first");
    let resolved = fixture.remote_conflict_resolved(2, "resolved.txt");
    let second = fixture.remote_delete_event(1, 3, "second.txt");

    assert!(fixture.engine.workspace_event(first).is_ok());
    assert!(fixture.engine.conflict_resolved(resolved).is_ok());
    assert!(fixture.engine.workspace_event(second).is_ok());
}

#[test]
fn mixed_event_resolved_event_revision_order_is_strict() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 3, 3, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "first.txt", b"first"))
        .unwrap();
    fixture
        .engine
        .conflict_resolved(fixture.remote_conflict_resolved(2, "resolved.txt"))
        .unwrap();
    let regressed = fixture.remote_delete_event(1, 1, "second.txt");
    assert!(matches!(
        fixture.engine.workspace_event(regressed),
        Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );
}

#[test]
fn authoritative_conflicts_are_counted_and_replaced() {
    let mut fixture = support::EngineFixture::new();
    let old =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000031", "1", "old.txt");
    fixture
        .engine
        .state_mut()
        .record_conflict(&old, ConflictStatus::Manual)
        .unwrap();
    let absent =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000033", "1", "absent.txt");
    let resolution_json = br#"{"choice":"current"}"#.to_vec();
    fixture
        .engine
        .state_mut()
        .put_conflict(&fns_sync_core::ConflictRecord {
            conflict_id: absent.conflict_id,
            workspace_id: absent.workspace_id,
            conflict_revision: absent.conflict_revision,
            created_json: fns_sync_core::canonical_json(&absent).unwrap(),
            status: ConflictStatus::Resolving,
            candidate_hash: None,
            resolution_digest: Some(fns_sync_core::body_digest(&resolution_json)),
            resolution_json: Some(resolution_json.clone()),
        })
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(4, 5, 0, 2))
        .unwrap();
    let retained =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000031", "2", "old.txt");
    let replacement =
        fixture.remote_conflict_created("10000000-0000-4000-8000-000000000032", "1", "new.txt");
    fixture.engine.conflict_created(retained.clone()).unwrap();
    fixture
        .engine
        .conflict_created(replacement.clone())
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 0, 2))
        .unwrap();

    let conflicts = fixture.engine.state().conflicts().unwrap();
    assert_eq!(conflicts.len(), 3);
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict.conflict_id == retained.conflict_id)
    );
    assert!(
        conflicts
            .iter()
            .any(|conflict| conflict.conflict_id == replacement.conflict_id)
    );
    let refreshed_absent = conflicts
        .iter()
        .find(|conflict| conflict.conflict_id == absent.conflict_id)
        .expect("absent pending conflict retained for dispatched resolution");
    assert_eq!(refreshed_absent.status, ConflictStatus::RefreshRequired);
    assert_eq!(
        refreshed_absent.resolution_json.as_deref(),
        Some(resolution_json.as_slice())
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .len(),
        2
    );
    assert!(
        !fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .iter()
            .any(|conflict| conflict.conflict_id == absent.conflict_id)
    );
}

#[test]
fn conflict_created_has_no_stream_index_or_tree_revision() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 0, 1))
        .unwrap();
    let conflict = fixture.remote_conflict_created(
        "10000000-0000-4000-8000-000000000033",
        "99",
        "conflict.txt",
    );
    fixture.engine.conflict_created(conflict).unwrap();
    assert!(
        fixture
            .engine
            .state()
            .stream_revision_items(fixture.stream_id())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_conflicts(fixture.stream_id())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn conflict_only_reconnect_updates_conflicts_without_same_revision_ack() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .state_mut()
        .set_last_ack_revision(WorkspaceRevision::new(5))
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(5, 5, 0, 1))
        .unwrap();
    fixture
        .engine
        .conflict_created(fixture.remote_conflict_created(
            "10000000-0000-4000-8000-000000000034",
            "1",
            "conflict.txt",
        ))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 0, 1))
        .unwrap();
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert!(support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()).is_empty());
}

#[test]
fn conflict_resolved_materializes_remote_postimage() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("resolved.txt", 1, b"base");
    fixture
        .engine
        .stage_bytes(&support::hash(b"current"), b"current")
        .unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(1, 2, 1, 0))
        .unwrap();
    fixture
        .engine
        .conflict_resolved(fixture.remote_conflict_resolved(2, "resolved.txt"))
        .unwrap();

    assert_eq!(fs::read(fixture.path("resolved.txt")).unwrap(), b"current");
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("resolved.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision,
        WorkspaceRevision::new(2)
    );
    assert!(fixture.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn end_must_match_begin() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 2, 1, 0))
        .unwrap();
    let mut end = fixture.incremental_end(3, 1, 0);
    end.final_revision = WorkspaceRevision::new(3);
    assert!(matches!(
        fixture.engine.snapshot_end(end),
        Err(SyncError::ProtocolInvariant { .. }) | Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert!(!fixture.path("anything").exists());
    assert!(
        !fixture
            .engine
            .state()
            .stream_state()
            .unwrap()
            .unwrap()
            .end_received
    );
}

#[test]
fn duplicate_exact_event_is_noop() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("same.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let event = fixture.remote_update_event(0, 1, "same.txt", b"new");
    assert!(fixture.engine.workspace_event(event.clone()).is_ok());
    fixture.provide_requested_blobs();
    let before = fixture.engine.state().row_counts().unwrap();
    assert!(fixture.engine.workspace_event(event).unwrap().is_empty());
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
}

#[test]
fn duplicate_operation_with_new_digest_is_invariant_error() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("same.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let first = fixture.remote_update_event(0, 1, "same.txt", b"new");
    fixture.engine.workspace_event(first).unwrap();
    let changed = fixture.remote_update_event(0, 1, "same.txt", b"changed");
    let before = fixture.engine.state().row_counts().unwrap();
    assert!(matches!(
        fixture.engine.workspace_event(changed),
        Err(SyncError::OperationChanged) | Err(SyncError::ProtocolInvariant { .. })
    ));
    assert_eq!(fixture.engine.state().row_counts().unwrap(), before);
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), b"old");
}

#[test]
fn self_event_settles_without_file_rewrite() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("self.txt", b"local");
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "self.txt",
        )))
        .unwrap();
    let mutation = fixture.engine.outbox().unwrap()[0].mutation().unwrap();
    let event = support::self_event_from_mutation(&fixture, 0, 1, mutation);
    let before = fs::read(fixture.path("self.txt")).unwrap();
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture.engine.workspace_event(event).unwrap();
    assert_eq!(fs::read(fixture.path("self.txt")).unwrap(), before);
    assert!(fixture.engine.outbox().unwrap().is_empty());
}

#[test]
fn remote_file_directory_symlink_delete_and_rename_apply() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("delete.txt", 0, b"delete");
    fixture.write("old-dir/child.txt", b"child");
    fixture.seed_remote_directory("old-dir", 0);
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 5, 5, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 1, "file.txt", b"file"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_mkdir_event(1, 2, "dir"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_symlink_event(2, 3, "link", b"dir"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_delete_event(3, 4, "delete.txt"))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_rename_event(4, 5, "old-dir", "new-dir"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(5, 5, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert!(
        fixture
            .engine
            .pending_commands(16)
            .unwrap()
            .iter()
            .any(|command| {
                matches!(command, SyncCommand::SendAck(message) if message.revision.get() == 5)
            })
    );
    assert_eq!(fs::read(fixture.path("file.txt")).unwrap(), b"file");
    assert!(fixture.path("dir").is_dir());
    assert!(fixture.path("link").is_symlink());
    assert!(!fixture.path("delete.txt").exists());
    assert!(fixture.path("new-dir/child.txt").exists());
}

#[test]
fn snapshot_paths_are_utf8_byte_sorted() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 2, 0))
        .unwrap();
    let first = fixture.snapshot_file_entry(0, 1, "é.txt", b"one");
    fixture.engine.snapshot_entry(first).unwrap();
    let out_of_order = fixture.snapshot_file_entry(1, 1, "z.txt", b"two");
    assert!(matches!(
        fixture.engine.snapshot_entry(out_of_order),
        Err(SyncError::StreamInvariant { .. })
    ));
    assert_eq!(
        fixture.engine.state().row_counts().unwrap()["stream_entries"],
        1
    );
    assert_eq!(
        fixture
            .engine
            .state()
            .stream_entries(fixture.stream_id())
            .unwrap()[0]
            .entry_index,
        0
    );
}

#[test]
fn full_snapshot_path_revisions_need_not_be_sorted() {
    let mut fixture = support::EngineFixture::new();
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(5, 2, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 5, "a.txt", b"a"))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(1, 2, "b.txt", b"b"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(5, 2, 0))
        .unwrap();
    fixture.provide_requested_blobs();

    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        5
    );
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![5]
    );
}

#[test]
fn local_watcher_echo_during_snapshot_is_reconciled_as_remote_state() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("echo.txt", b"remote");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .record_local_change(fns_fs::FsChange::Create(support::workspace_path(
            "echo.txt",
        )))
        .unwrap();
    let modified_at_ms = std::fs::metadata(fixture.path("echo.txt"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut entry = fixture.snapshot_file_entry(0, 1, "echo.txt", b"remote");
    entry.entry.metadata.modified_at_ms = modified_at_ms as i64;
    entry.validate().unwrap();
    fixture.engine.snapshot_entry(entry).unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();

    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert!(fixture.engine.state().local_intents().unwrap().is_empty());
}

#[test]
fn initial_same_content_is_adopted() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("same.txt", b"same");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 1, "same.txt", b"same"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert_eq!(fs::read(fixture.path("same.txt")).unwrap(), b"same");
    assert!(fixture.engine.outbox().unwrap().is_empty());
    assert_eq!(
        fixture
            .engine
            .state()
            .path_state("same.txt")
            .unwrap()
            .unwrap()
            .state
            .path_revision
            .get(),
        1
    );
}

#[test]
fn initial_difference_queues_base_zero_without_overwrite() {
    let mut fixture = support::EngineFixture::new();
    fixture.write("different.txt", b"local");
    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 1, "different.txt", b"server"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 1, 0))
        .unwrap();
    fixture.provide_requested_blobs();
    assert_eq!(fs::read(fixture.path("different.txt")).unwrap(), b"local");
    let commands = fixture.engine.pending_commands(16).unwrap();
    let mutations = commands
        .iter()
        .filter(|command| matches!(command, SyncCommand::Mutation(_)))
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 1);
    assert!(matches!(mutations[0], SyncCommand::Mutation(mutation)
        if mutation.kind == WorkspaceMutationKind::UpsertFile
            && mutation.base_path_revision == WorkspaceRevision::ZERO));
}

#[test]
fn local_edit_during_remote_update_is_preserved() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("edit.txt", 7, b"base");
    fixture.write("edit.txt", b"local");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(7, 8, 1, 0))
        .unwrap();
    let result = fixture
        .engine
        .workspace_event(fixture.remote_update_event(0, 8, "edit.txt", b"server"))
        .unwrap();
    assert!(result.is_empty());
    assert_eq!(fs::read(fixture.path("edit.txt")).unwrap(), b"local");
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, SyncCommand::Mutation(mutation)
        if mutation.base_path_revision.get() == 7))
    );
    assert_eq!(
        fixture.engine.cursor().unwrap().last_applied_revision.get(),
        0
    );
}

#[test]
fn missing_blob_is_requested_again_after_reopen() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("reopen.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    let event = fixture.remote_update_event(0, 1, "reopen.txt", b"new");
    assert!(
        fixture
            .engine
            .workspace_event(event)
            .unwrap()
            .iter()
            .any(support::is_download)
    );
    fixture.engine.close().unwrap();
    let mut reopened = fixture.reopen();
    let commands = reopened.engine.pending_commands(16).unwrap();
    assert_eq!(
        commands
            .iter()
            .filter(|command| support::is_download(command))
            .count(),
        1
    );
}

#[test]
fn prepared_apply_journal_is_removed_on_reopen() {
    let mut fixture = support::EngineFixture::new();
    let state = support::path_state(
        "journal.txt",
        1,
        fns_protocol::RequiredNullable::Null,
        support::file_metadata(0),
        fns_protocol::WorkspaceEntryKind::Directory,
    );
    let operation = fns_sync_core::model::RemoteApplyOperation::Upsert {
        state: state.clone(),
    };
    let apply_id = fns_sync_core::ApplyId(
        uuid::Uuid::parse_str("10000000-0000-4000-8000-000000000041").unwrap(),
    );
    let workspace_id = fixture.engine.state().workspace_id();
    let stream_id = fixture.stream_id();
    fixture
        .engine
        .state_mut()
        .put_apply_journal(&fns_sync_core::ApplyJournalRecord {
            apply_id,
            workspace_id,
            stream_id,
            item_kind: fns_sync_core::ApplyItemKind::Entry,
            item_key: "journal.txt".to_owned(),
            operation_json: fns_sync_core::canonical_json(&operation).unwrap(),
            preimage_json: b"null".to_vec(),
            postimage_json: fns_sync_core::canonical_json(&vec![state]).unwrap(),
            stage: fns_sync_core::ApplyStage::Prepared,
        })
        .unwrap();
    assert_eq!(fixture.engine.state().apply_journals().unwrap().len(), 1);
    fixture.engine.close().unwrap();

    let reopened = fixture.reopen();
    assert!(reopened.engine.state().apply_journals().unwrap().is_empty());
}

#[test]
fn ack_repeats_until_correlated_success() {
    let mut fixture = support::EngineFixture::new();
    fixture.seed_remote_file("ack.txt", 0, b"old");
    fixture
        .engine
        .snapshot_begin(fixture.incremental_begin(0, 1, 1, 0))
        .unwrap();
    fixture
        .engine
        .workspace_event(fixture.remote_delete_event(0, 1, "ack.txt"))
        .unwrap();
    fixture
        .engine
        .snapshot_end(fixture.incremental_end(1, 1, 0))
        .unwrap();
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![1]
    );
    assert_eq!(
        support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()),
        vec![1]
    );
    assert!(fixture.engine.ack_confirmed(fixture.ack(0)).is_err());
    assert_eq!(fixture.engine.cursor().unwrap().last_ack_revision.get(), 0);
    fixture.engine.ack_confirmed(fixture.ack(1)).unwrap();
    assert!(fixture.engine.state().stream_state().unwrap().is_none());
    assert!(support::ack_revisions(&fixture.engine.pending_commands(16).unwrap()).is_empty());
}
