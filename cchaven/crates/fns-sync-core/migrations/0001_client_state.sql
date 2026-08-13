CREATE TABLE workspace_cursor (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    client_id TEXT NOT NULL,
    last_ack_revision TEXT NOT NULL,
    last_applied_revision TEXT NOT NULL,
    pending_ack_revision TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE TABLE path_states (
    workspace_id TEXT NOT NULL,
    path TEXT NOT NULL,
    state_json BLOB NOT NULL,
    state_digest BLOB NOT NULL CHECK(length(state_digest) = 32),
    PRIMARY KEY (workspace_id, path),
    FOREIGN KEY (workspace_id) REFERENCES workspace_cursor(workspace_id) ON DELETE CASCADE
);
CREATE TABLE outbox (
    client_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    body_json BLOB NOT NULL,
    body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
    stage TEXT NOT NULL CHECK(stage IN ('queued','dispatched','awaiting_blob','blocked_conflict')),
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (client_id, operation_id)
);
CREATE TABLE local_intents (
    workspace_id TEXT NOT NULL,
    path TEXT NOT NULL,
    intent_json BLOB NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, path)
);
CREATE TABLE stream_state (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    stream_id TEXT NOT NULL,
    mode TEXT NOT NULL CHECK(mode IN ('snapshot','incremental')),
    from_revision TEXT NOT NULL,
    final_revision TEXT NOT NULL,
    expected_entry_count INTEGER NOT NULL,
    expected_event_count INTEGER NOT NULL,
    expected_conflict_count INTEGER NOT NULL,
    next_event_index INTEGER NOT NULL,
    end_received INTEGER NOT NULL CHECK(end_received IN (0,1))
);
CREATE TABLE stream_entries (
    workspace_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    entry_index INTEGER NOT NULL,
    body_json BLOB NOT NULL,
    body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
    status TEXT NOT NULL CHECK(status IN ('received','waiting_blob','ready','applied','preserved')),
    PRIMARY KEY (workspace_id, stream_id, entry_index)
);
CREATE TABLE stream_revision_items (
    workspace_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    item_kind TEXT NOT NULL CHECK(item_kind IN ('event','conflict_resolved')),
    body_json BLOB NOT NULL,
    body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
    event_index INTEGER,
    status TEXT NOT NULL CHECK(status IN ('received','waiting_blob','ready','applied','preserved')),
    PRIMARY KEY (workspace_id, stream_id, revision),
    UNIQUE (workspace_id, stream_id, event_index)
);
CREATE TABLE stream_conflicts (
    workspace_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    conflict_id TEXT NOT NULL,
    conflict_revision TEXT NOT NULL,
    created_json BLOB NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('received','replaced','pruned')),
    PRIMARY KEY (workspace_id, stream_id, conflict_id)
);
CREATE TABLE apply_journal (
    apply_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    stream_id TEXT NOT NULL,
    item_kind TEXT NOT NULL CHECK(item_kind IN ('entry','event','conflict_resolved')),
    item_key TEXT NOT NULL,
    operation_json BLOB NOT NULL,
    preimage_json BLOB NOT NULL,
    postimage_json BLOB NOT NULL,
    stage TEXT NOT NULL CHECK(stage IN ('prepared','filesystem_started')),
    UNIQUE (workspace_id, stream_id, item_kind, item_key)
);
CREATE TABLE applied_operations (
    origin_client_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
    PRIMARY KEY (origin_client_id, operation_id)
);
CREATE TABLE conflicts (
    conflict_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    conflict_revision TEXT NOT NULL,
    created_json BLOB NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('waiting_blobs','manual','auto_ready','resolving','refresh_required')),
    candidate_hash TEXT,
    resolution_json BLOB,
    resolution_digest BLOB
);
CREATE TABLE hash_cache (
    workspace_id TEXT NOT NULL,
    path TEXT NOT NULL,
    fingerprint_json BLOB NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY (workspace_id, path)
);
PRAGMA user_version = 1;
