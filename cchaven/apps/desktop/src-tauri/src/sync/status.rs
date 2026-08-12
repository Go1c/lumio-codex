//! Aggregate sync state, derived from the engine rather than guessed at.
//!
//! 交互设计 6.3 defines exactly four states for the whole app. This module owns
//! the mapping from what the sync engine knows — queued mutations, inbound
//! stream items, open conflicts — plus what the session supervisor knows about
//! the connection, onto those four.

use serde::Serialize;
use tokio::time::Instant;

use fns_sync_core::{ConflictStatus, StreamItemStatus, SyncEngine, SyncError};

/// 6.3 全局唯一语义. Nothing in the app may invent a fifth state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    Synced,
    Syncing,
    Conflicts,
    Offline,
}

/// What the engine currently owes, counted the way the status bar words it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineProgress {
    /// Conflicts still waiting for the user (a resolution already on its way to
    /// the server counts as transfer, not as a question).
    pub conflicts: usize,
    /// Distinct files with work outstanding in either direction — the `{n}` of
    /// 「正在同步 {n} 个文件…」.
    pub pending: usize,
}

/// Everything the supervisor and the engine know, before it is reduced to one
/// of the four states.
#[derive(Clone, Debug)]
pub struct SyncSnapshot {
    pub connected: bool,
    pub progress: EngineProgress,
    /// When the next reconnect attempt is due, while offline.
    pub retry_at: Option<Instant>,
    /// Stable, non-sensitive reason the session is not up.
    pub detail: Option<&'static str>,
}

impl Default for SyncSnapshot {
    fn default() -> Self {
        Self {
            connected: false,
            progress: EngineProgress::default(),
            retry_at: None,
            detail: Some(super::DETAIL_STARTING),
        }
    }
}

/// The shape the frontend consumes (`SyncStatus` in `lib/types.ts`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncState,
    pub conflicts: usize,
    pub pending: usize,
    /// Seconds until the next reconnect attempt — the `{n}` of
    /// 「离线 — {n} 秒后重试」. `None` whenever the app is not counting down.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_in_seconds: Option<u32>,
    /// Why the session is down, for the activity panel and for support.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'static str>,
}

impl SyncSnapshot {
    /// Reduce to the four states, worst case first, matching the order the
    /// sidebar already uses for the global bar.
    pub fn status(&self, now: Instant) -> SyncStatus {
        let state = if self.progress.conflicts > 0 {
            SyncState::Conflicts
        } else if !self.connected {
            SyncState::Offline
        } else if self.progress.pending > 0 {
            SyncState::Syncing
        } else {
            SyncState::Synced
        };
        SyncStatus {
            state,
            conflicts: self.progress.conflicts,
            pending: self.progress.pending,
            retry_in_seconds: (!self.connected)
                .then(|| self.retry_in_seconds(now))
                .flatten(),
            detail: (!self.connected).then_some(self.detail).flatten(),
        }
    }

    fn retry_in_seconds(&self, now: Instant) -> Option<u32> {
        let retry_at = self.retry_at?;
        let remaining = retry_at.saturating_duration_since(now);
        // Round up: a countdown that shows 0 while still waiting reads as stuck.
        Some(u32::try_from(remaining.as_secs_f64().ceil() as u64).unwrap_or(u32::MAX))
    }
}

/// Count what the engine still owes the server and what it still owes the user.
///
/// Runs on the engine thread through `EngineHandle::with_engine`, so it sees a
/// consistent view between two protocol calls.
pub fn engine_progress(engine: &SyncEngine) -> Result<EngineProgress, SyncError> {
    let state = engine.state();

    let mut pending_paths = Vec::new();
    for record in engine.outbox()? {
        let mutation = record.mutation().map_err(|_| SyncError::CorruptState {
            table: "outbox",
            field: "body_json",
        })?;
        push_unique(&mut pending_paths, mutation.path.to_string());
        if let Some(new_path) = mutation.new_path {
            push_unique(&mut pending_paths, new_path.to_string());
        }
    }

    // Inbound work: stream items the engine has received but not yet landed on
    // disk are files on their way in, and the user is waiting for them too.
    if let Some(stream) = state.stream_state()? {
        let unsettled = |status: StreamItemStatus| {
            !matches!(
                status,
                StreamItemStatus::Applied | StreamItemStatus::Preserved
            )
        };
        let inbound = state
            .stream_entries(stream.stream_id)?
            .iter()
            .filter(|entry| unsettled(entry.status))
            .count()
            + state
                .stream_revision_items(stream.stream_id)?
                .iter()
                .filter(|item| unsettled(item.status))
                .count();
        // Stream items are keyed by revision rather than path, so they are
        // added as a count instead of being de-duplicated against the outbox.
        return Ok(EngineProgress {
            conflicts: open_conflicts(engine)?,
            pending: pending_paths.len() + inbound,
        });
    }

    Ok(EngineProgress {
        conflicts: open_conflicts(engine)?,
        pending: pending_paths.len(),
    })
}

/// Conflicts the user still has to answer.
fn open_conflicts(engine: &SyncEngine) -> Result<usize, SyncError> {
    Ok(engine
        .state()
        .conflicts()?
        .iter()
        .filter(|conflict| conflict.status != ConflictStatus::Resolving)
        .count())
}

fn push_unique(paths: &mut Vec<String>, path: String) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn snapshot(connected: bool, conflicts: usize, pending: usize) -> SyncSnapshot {
        SyncSnapshot {
            connected,
            progress: EngineProgress { conflicts, pending },
            retry_at: None,
            detail: None,
        }
    }

    #[test]
    fn a_connected_idle_engine_is_fully_synced() {
        let now = Instant::now();
        let status = snapshot(true, 0, 0).status(now);
        assert_eq!(status.state, SyncState::Synced);
        assert_eq!(status.retry_in_seconds, None);
    }

    #[test]
    fn outstanding_work_counts_the_files_the_engine_still_owes() {
        let now = Instant::now();
        let status = snapshot(true, 0, 3).status(now);
        assert_eq!(status.state, SyncState::Syncing);
        assert_eq!(status.pending, 3);
    }

    #[test]
    fn conflicts_outrank_everything_including_being_offline() {
        let now = Instant::now();
        let status = snapshot(false, 2, 5).status(now);
        assert_eq!(status.state, SyncState::Conflicts);
        assert_eq!(status.conflicts, 2);
    }

    #[test]
    fn being_offline_outranks_having_transfers_queued() {
        let now = Instant::now();
        let status = snapshot(false, 0, 4).status(now);
        assert_eq!(status.state, SyncState::Offline);
        // The queue is still reported: it is what will move once we reconnect.
        assert_eq!(status.pending, 4);
    }

    #[test]
    fn the_offline_countdown_is_the_supervisors_real_deadline() {
        let now = Instant::now();
        let mut state = snapshot(false, 0, 0);
        state.retry_at = Some(now + Duration::from_secs(10));

        assert_eq!(state.status(now).retry_in_seconds, Some(10));
        assert_eq!(
            state
                .status(now + Duration::from_millis(7_500))
                .retry_in_seconds,
            Some(3),
            "a partially elapsed second still reads as a second remaining"
        );
        assert_eq!(
            state.status(now + Duration::from_secs(30)).retry_in_seconds,
            Some(0),
            "an overdue attempt never counts backwards"
        );
    }

    #[test]
    fn a_connected_session_stops_advertising_a_retry() {
        let now = Instant::now();
        let mut state = snapshot(true, 0, 0);
        state.retry_at = Some(now + Duration::from_secs(10));
        state.detail = Some("stale");

        let status = state.status(now);
        assert_eq!(status.state, SyncState::Synced);
        assert_eq!(status.detail, None);
    }
}
