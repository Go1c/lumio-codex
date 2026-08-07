mod error;
mod hash;
mod root;
mod rules;
mod scan;
mod watcher;

pub use error::FsError;
pub use hash::{ContentCache, ContentDescriptor, HashCache, HashCacheError, MemoryHashCache};
pub use root::{CaseSensitivity, FileFingerprint, NativeFileId, ObservedEntry, RootedWorkspace};
pub use rules::{
    DEFAULT_EXCLUDES, DEFAULT_INCLUDES, HARD_INTERNAL_EXCLUDES, HARD_SECRET_EXCLUDES, RuleDecision,
    RuleSource, SAFE_ENV_BASENAMES, SyncRuleConfig, SyncRules,
};
pub use scan::{ScanIssue, WorkspaceScan};
pub use watcher::{
    NativeWatchKind, NormalizedWatchEvent, PlatformWatcher, WATCH_QUEUE_CAPACITY, WatchGap,
    WatchIngress, WatchMessage, WatchReceiver, start_platform_watcher,
};
