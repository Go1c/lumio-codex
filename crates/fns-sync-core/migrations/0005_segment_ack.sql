-- Segment ack: allows a contiguous prefix of Applied incremental-stream
-- revision items to be acknowledged before the stream's SnapshotEnd arrives.
-- Without this, an interrupted stream (end_received=0) permanently stalls
-- last_ack at the pre-stream revision, forcing every reconnect to re-subscribe
-- and re-apply the same already-applied events.
ALTER TABLE workspace_cursor ADD COLUMN pending_segment_ack_revision TEXT;
PRAGMA user_version = 5;
