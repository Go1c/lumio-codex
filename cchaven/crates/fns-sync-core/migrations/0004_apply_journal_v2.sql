CREATE TABLE apply_journal_v2 (
    apply_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    item_kind TEXT NOT NULL CHECK(item_kind IN ('entry','event','conflict_resolved')),
    item_key TEXT NOT NULL,
    apply_namespace TEXT NOT NULL CHECK(apply_namespace IN (
        'snapshot_entry',
        'stream_event',
        'live_event',
        'stream_conflict_resolved',
        'live_conflict_resolved'
    )),
    operation_body_digest BLOB NOT NULL CHECK(length(operation_body_digest) = 32),
    operation_json BLOB NOT NULL,
    filesystem_operation_json BLOB NOT NULL,
    commit_json BLOB NOT NULL,
    preimage_json BLOB NOT NULL,
    postimage_json BLOB NOT NULL,
    filesystem_receipt_json BLOB,
    stage TEXT NOT NULL CHECK(stage IN (
        'prepared',
        'filesystem_started',
        'filesystem_applied',
        'database_committed',
        'finalized'
    )),
    UNIQUE (workspace_id, stream_id, item_kind, item_key)
);

INSERT INTO apply_journal_v2 (
    apply_id,
    workspace_id,
    stream_id,
    item_kind,
    item_key,
    apply_namespace,
    operation_body_digest,
    operation_json,
    filesystem_operation_json,
    commit_json,
    preimage_json,
    postimage_json,
    filesystem_receipt_json,
    stage
)
SELECT
    apply_id,
    workspace_id,
    stream_id,
    item_kind,
    item_key,
    CASE item_kind
        WHEN 'entry' THEN 'snapshot_entry'
        WHEN 'event' THEN 'stream_event'
        ELSE 'stream_conflict_resolved'
    END,
    zeroblob(32),
    operation_json,
    operation_json,
    x'',
    preimage_json,
    postimage_json,
    NULL,
    stage
FROM apply_journal;

DROP TABLE apply_journal;
ALTER TABLE apply_journal_v2 RENAME TO apply_journal;
PRAGMA user_version = 4;
