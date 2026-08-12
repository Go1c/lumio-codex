mod atomic_write;
mod coalesce;
mod error;
mod hash;
mod root;
mod rules;
mod scan;
mod watcher;

pub use atomic_write::{
    ApplyCheckpoint, ApplyObservation, ApplyObserver, AtomicWorkspaceWriter, ExpectedEntry,
    FsOperation,
};
pub use coalesce::{
    ApplyId, ApplyReceipt, COALESCER_PATH_CAPACITY, CoalescePush, DEBOUNCE_WINDOW, EntrySignature,
    EventCoalescer, FsChange, FsChangeKind, PriorEntryLookup, RENAME_WINDOW,
};
pub use error::FsError;
pub use hash::{
    ContentCache, ContentDescriptor, HashCache, HashCacheError, MemoryHashCache,
    SealedContentImport, StagedContentImport,
};
pub use root::{
    CaseSensitivity, DirectorySnapshot, DirectorySnapshotEntry, FileFingerprint, NativeFileId,
    ObservedEntry, RootedWorkspace,
};
pub use rules::{
    DEFAULT_EXCLUDES, DEFAULT_INCLUDES, HARD_INTERNAL_EXCLUDES, HARD_SECRET_EXCLUDES, RuleDecision,
    RuleSource, SAFE_ENV_BASENAMES, SyncRuleConfig, SyncRules,
};
pub use scan::{ScanIssue, WorkspaceScan};
pub use watcher::{
    NativeWatchKind, NormalizedWatchEvent, PlatformWatcher, WATCH_QUEUE_CAPACITY, WatchGap,
    WatchIngress, WatchMessage, WatchReceiver, start_platform_watcher,
};
