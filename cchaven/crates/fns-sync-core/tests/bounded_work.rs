mod support;

use std::io::Cursor;

use fns_sync_core::engine::InboundWorkLimits;
use fns_sync_core::{StreamItemStatus, SyncCommand, SyncError};

const STREAM_ITEM_COUNT: u32 = 72;
const LIVE_EVENT_PROBE_COUNT: u32 = 1_024;

#[test]
fn large_stream_is_incrementally_drained() {
    let mut fixture = support::EngineFixture::new();
    let bytes = b"shared payload";
    let content_hash = support::hash(bytes);

    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, STREAM_ITEM_COUNT, 0))
        .unwrap();
    for index in 0..STREAM_ITEM_COUNT {
        fixture
            .engine
            .snapshot_entry(fixture.snapshot_file_entry(
                index,
                1,
                &format!("bulk/{index:04}.bin"),
                bytes,
            ))
            .unwrap();
    }
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, STREAM_ITEM_COUNT, 0))
        .unwrap();

    fixture
        .engine
        .blob_available(
            content_hash,
            bytes.len() as u64,
            Cursor::new(bytes.as_slice()),
        )
        .unwrap();

    let applied_after_blob = fixture
        .engine
        .state()
        .stream_entries(fixture.stream_id())
        .unwrap()
        .into_iter()
        .filter(|record| record.status == StreamItemStatus::Applied)
        .count();
    assert!(applied_after_blob > 0);
    assert!(applied_after_blob < STREAM_ITEM_COUNT as usize);
    assert!(
        !fixture
            .path(&format!("bulk/{:04}.bin", STREAM_ITEM_COUNT - 1))
            .exists()
    );

    for _ in 0..STREAM_ITEM_COUNT {
        fixture.engine.pending_commands(8).unwrap();
        if fixture
            .path(&format!("bulk/{:04}.bin", STREAM_ITEM_COUNT - 1))
            .exists()
        {
            break;
        }
    }

    for index in 0..STREAM_ITEM_COUNT {
        assert_eq!(
            std::fs::read(fixture.path(&format!("bulk/{index:04}.bin"))).unwrap(),
            bytes
        );
    }
    assert_eq!(
        fixture
            .engine
            .cursor()
            .unwrap()
            .pending_ack_revision
            .unwrap()
            .get(),
        1
    );
}

#[test]
fn live_queue_is_bounded_with_a_stable_error() {
    let mut fixture = support::EngineFixture::new();
    let mut rejected = None;

    for index in 0..LIVE_EVENT_PROBE_COUNT {
        let event = fixture.remote_update_event(
            index,
            u64::from(index) + 1,
            &format!("live/{index:04}.bin"),
            b"missing",
        );
        match fixture.engine.event(event.clone()) {
            Ok(_) => {}
            Err(error) => {
                rejected = Some((event, error));
                break;
            }
        }
    }

    let (rejected_event, first_error) = rejected.expect("live queue must reject bounded growth");
    assert_eq!(
        first_error,
        SyncError::ResourceLimit {
            resource: "pending_live_events"
        }
    );
    assert_eq!(
        first_error.to_string(),
        "resource limit exceeded: pending_live_events"
    );
    let repeated_error = fixture.engine.event(rejected_event).unwrap_err();
    assert_eq!(repeated_error, first_error);
}

#[test]
fn live_queue_serialized_byte_limit_is_enforced() {
    let probe = support::EngineFixture::new();
    let probe_event = probe.remote_update_event(0, 1, "bytes/0000.bin", b"missing");
    let one_event_bytes = fns_sync_core::canonical_json(&probe_event).unwrap().len();
    drop(probe);

    let limits = InboundWorkLimits {
        max_pending_live_items: 8,
        max_pending_live_serialized_bytes: one_event_bytes,
        ..InboundWorkLimits::default()
    };
    let mut fixture = support::EngineFixture::new_with_inbound_work_limits(limits);
    fixture
        .engine
        .event(fixture.remote_update_event(0, 1, "bytes/0000.bin", b"missing"))
        .unwrap();

    assert_eq!(
        fixture
            .engine
            .event(fixture.remote_update_event(1, 2, "bytes/0001.bin", b"missing"))
            .unwrap_err(),
        SyncError::ResourceLimit {
            resource: "pending_live_events"
        }
    );
}

#[test]
fn first_item_larger_than_byte_budget_still_respects_item_limit() {
    let limits = InboundWorkLimits {
        max_items_per_call: 1,
        max_serialized_bytes_per_call: 1,
        ..InboundWorkLimits::default()
    };
    let mut fixture = support::EngineFixture::new_with_inbound_work_limits(limits);
    let bytes = b"one shared blob";
    let content_hash = support::hash(bytes);

    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 2, 0))
        .unwrap();
    for (index, path) in ["large/first.bin", "large/second.bin"]
        .into_iter()
        .enumerate()
    {
        fixture
            .engine
            .snapshot_entry(fixture.snapshot_file_entry(index as u32, 1, path, bytes))
            .unwrap();
    }
    fixture
        .engine
        .snapshot_end(fixture.snapshot_end(1, 2, 0))
        .unwrap();

    fixture
        .engine
        .blob_available(
            content_hash,
            bytes.len() as u64,
            Cursor::new(bytes.as_slice()),
        )
        .unwrap();

    let applied = fixture
        .engine
        .state()
        .stream_entries(fixture.stream_id())
        .unwrap()
        .into_iter()
        .filter(|record| record.status == StreamItemStatus::Applied)
        .count();
    assert_eq!(applied, 1);
    assert!(fixture.path("large/first.bin").exists());
    assert!(!fixture.path("large/second.bin").exists());

    fixture.engine.pending_commands(8).unwrap();
    assert_eq!(
        std::fs::read(fixture.path("large/second.bin")).unwrap(),
        bytes
    );
}

#[test]
fn blocked_stream_and_live_downloads_get_fair_poll_turns() {
    let limits = InboundWorkLimits {
        max_items_per_call: 2,
        ..InboundWorkLimits::default()
    };
    let mut fixture = support::EngineFixture::new_with_inbound_work_limits(limits);
    let stream_bytes = b"stream blob";
    let live_bytes = b"live blob";
    let stream_hash = support::hash(stream_bytes);
    let live_hash = support::hash(live_bytes);

    fixture
        .engine
        .snapshot_begin(fixture.snapshot_begin(1, 1, 0))
        .unwrap();
    fixture
        .engine
        .snapshot_entry(fixture.snapshot_file_entry(0, 1, "stream.bin", stream_bytes))
        .unwrap();
    fixture
        .engine
        .event(fixture.remote_update_event(0, 2, "live.bin", live_bytes))
        .unwrap();

    let first = fixture.engine.pending_commands(1).unwrap();
    let second = fixture.engine.pending_commands(1).unwrap();
    assert!(matches!(
        first.as_slice(),
        [SyncCommand::DownloadBlob { content_hash, .. }] if *content_hash == stream_hash
    ));
    assert!(matches!(
        second.as_slice(),
        [SyncCommand::DownloadBlob { content_hash, .. }] if *content_hash == live_hash
    ));
}

#[cfg(unix)]
#[test]
fn failed_live_head_remains_queued_and_blocks_later_ack() {
    use std::os::unix::fs::symlink;

    let mut fixture = support::EngineFixture::new();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), fixture.path("escape")).unwrap();

    let first_bytes = b"first";
    let second_bytes = b"second";
    fixture
        .engine
        .stage_bytes(&support::hash(first_bytes), first_bytes)
        .unwrap();
    fixture
        .engine
        .stage_bytes(&support::hash(second_bytes), second_bytes)
        .unwrap();

    let first = fixture.remote_update_event(0, 1, "escape/first.txt", first_bytes);
    assert!(matches!(
        fixture.engine.event(first),
        Err(SyncError::Filesystem(fns_fs::FsError::PathEscape))
    ));

    let second = fixture.remote_update_event(1, 2, "second.txt", second_bytes);
    assert!(matches!(
        fixture.engine.event(second),
        Err(SyncError::Filesystem(fns_fs::FsError::PathEscape))
    ));
    assert!(!fixture.path("second.txt").exists());
    assert_eq!(fixture.engine.cursor().unwrap().pending_ack_revision, None);
    assert!(matches!(
        fixture.engine.pending_commands(16),
        Err(SyncError::Filesystem(fns_fs::FsError::PathEscape))
    ));

    std::fs::remove_file(fixture.path("escape")).unwrap();
    std::fs::create_dir(fixture.path("escape")).unwrap();
    let commands = fixture.engine.pending_commands(16).unwrap();
    assert_eq!(support::ack_revisions(&commands), vec![2]);
    assert_eq!(
        std::fs::read(fixture.path("escape/first.txt")).unwrap(),
        first_bytes
    );
    assert_eq!(
        std::fs::read(fixture.path("second.txt")).unwrap(),
        second_bytes
    );
}

#[test]
fn inbound_handlers_have_no_unbounded_drain_sentinel() {
    let engine_source = include_str!("../src/engine.rs");
    assert!(
        !engine_source.contains("usize::MAX"),
        "inbound work must use explicit item and byte budgets"
    );
    assert!(!engine_source.contains(".stream_entries("));
    assert!(!engine_source.contains(".stream_revision_items("));
    assert!(!engine_source.contains(".stream_conflicts("));
}
