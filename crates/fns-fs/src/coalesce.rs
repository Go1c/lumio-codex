use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use fns_protocol::{WorkspaceContentHash, WorkspaceEntryKind, WorkspacePath};

use crate::{FsError, NativeWatchKind, ObservedEntry, SyncRuleConfig, SyncRules};

pub const COALESCER_PATH_CAPACITY: usize = 8_192;
pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(200);
pub const RENAME_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FsChange {
    Create(WorkspacePath),
    Update(WorkspacePath),
    Delete(WorkspacePath),
    Rename {
        from: WorkspacePath,
        to: WorkspacePath,
    },
    RescanRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FsChangeKind {
    Create,
    Update,
    Delete,
    Rename,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoalescePush {
    Accepted,
    RescanRequired,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntrySignature {
    pub kind: WorkspaceEntryKind,
    pub content_hash: Option<WorkspaceContentHash>,
    pub size: u64,
}

pub trait PriorEntryLookup {
    fn signature(&self, path: &WorkspacePath) -> Option<EntrySignature>;

    fn observed(&self, _path: &WorkspacePath) -> Option<ObservedEntry> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ApplyId(pub uuid::Uuid);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyReceipt {
    pub apply_id: ApplyId,
    pub touched: Vec<WorkspacePath>,
    pub postimages: Vec<Option<ObservedEntry>>,
    pub postimage_hashes: Vec<Option<WorkspaceContentHash>>,
    pub cleanup_name: Option<String>,
}

#[derive(Clone, Copy)]
enum PendingKind {
    Create,
    Modify,
    Remove,
}

struct PendingPath {
    kind: PendingKind,
    last_seen: Instant,
}

struct RenameHalf {
    path: String,
    kind: NativeWatchKind,
    observed_at: Instant,
}

struct PendingRename {
    from: String,
    to: String,
    observed_at: Instant,
}

enum PathKey {
    Accepted(String),
    Ignored,
}

struct Suppression {
    missing: bool,
    kind: WorkspaceEntryKind,
    size: u64,
    executable: bool,
    fingerprint: crate::FileFingerprint,
    hash: Option<WorkspaceContentHash>,
    expires_at: Instant,
}

pub struct EventCoalescer {
    debounce: Duration,
    rename: Duration,
    capacity: usize,
    pending: BTreeMap<String, PendingPath>,
    occupied: BTreeSet<String>,
    cookie_halves: HashMap<u64, RenameHalf>,
    unmatched_renames: Vec<RenameHalf>,
    direct_renames: Vec<PendingRename>,
    suppressions: BTreeMap<String, Suppression>,
    rescan_required: bool,
    rules: SyncRules,
}

impl EventCoalescer {
    pub fn new(debounce: Duration, rename: Duration, capacity: usize) -> Self {
        let rules = SyncRules::compile(SyncRuleConfig::default())
            .expect("default synchronization rules are valid");
        Self::with_rules(debounce, rename, capacity, rules)
    }

    pub fn with_rules(
        debounce: Duration,
        rename: Duration,
        capacity: usize,
        rules: SyncRules,
    ) -> Self {
        Self {
            debounce,
            rename,
            capacity,
            pending: BTreeMap::new(),
            occupied: BTreeSet::new(),
            cookie_halves: HashMap::new(),
            unmatched_renames: Vec::new(),
            direct_renames: Vec::new(),
            suppressions: BTreeMap::new(),
            rescan_required: false,
            rules,
        }
    }

    pub fn push(&mut self, event: crate::NormalizedWatchEvent) -> CoalescePush {
        self.prune_suppressions(Instant::now());
        if self.rescan_required {
            return CoalescePush::RescanRequired;
        }
        match event.kind {
            crate::NativeWatchKind::RenameBoth => {
                if event.paths.len() != 2 {
                    return self.require_rescan();
                }
                let from = match self.path_key(&event.paths[0]) {
                    Ok(path) => path,
                    Err(()) => return self.require_rescan(),
                };
                let to = match self.path_key(&event.paths[1]) {
                    Ok(path) => path,
                    Err(()) => return self.require_rescan(),
                };
                match (from, to) {
                    (PathKey::Accepted(from), PathKey::Accepted(to)) => {
                        if !self.reserve_paths([from.clone(), to.clone()]) {
                            return self.require_rescan();
                        }
                        if !self.reserve_rename_slot() {
                            return self.require_rescan();
                        }
                        self.direct_renames.push(PendingRename {
                            from,
                            to,
                            observed_at: event.observed_at,
                        });
                    }
                    (PathKey::Accepted(path), PathKey::Ignored) => {
                        if !self.reserve_paths([path.clone()]) {
                            return self.require_rescan();
                        }
                        self.record_path(path, NativeWatchKind::Remove, event.observed_at);
                    }
                    (PathKey::Ignored, PathKey::Accepted(path)) => {
                        if !self.reserve_paths([path.clone()]) {
                            return self.require_rescan();
                        }
                        self.record_path(path, NativeWatchKind::Create, event.observed_at);
                    }
                    (PathKey::Ignored, PathKey::Ignored) => {}
                }
            }
            crate::NativeWatchKind::RenameFrom | crate::NativeWatchKind::RenameTo => {
                if event.paths.len() != 1 {
                    return self.require_rescan();
                }
                let path = match self.path_key(&event.paths[0]) {
                    Ok(PathKey::Accepted(path)) => path,
                    Ok(PathKey::Ignored) => return CoalescePush::Accepted,
                    Err(()) => return self.require_rescan(),
                };
                if !self.reserve_paths([path.clone()]) {
                    return self.require_rescan();
                }
                let half = RenameHalf {
                    path,
                    kind: event.kind,
                    observed_at: event.observed_at,
                };
                if let Some(cookie) = event.rename_cookie {
                    if let Some(previous) = self.cookie_halves.get(&cookie) {
                        if previous.kind == half.kind
                            || !within_window(previous.observed_at, half.observed_at, self.rename)
                        {
                            let previous = self.cookie_halves.remove(&cookie).unwrap();
                            if !self.push_unmatched(previous) || !self.reserve_rename_slot() {
                                return self.require_rescan();
                            }
                            self.cookie_halves.insert(cookie, half);
                        } else {
                            let previous = self.cookie_halves.remove(&cookie).unwrap();
                            let (from, to) = if previous.kind == crate::NativeWatchKind::RenameFrom
                            {
                                (previous.path, half.path)
                            } else {
                                (half.path, previous.path)
                            };
                            self.direct_renames.push(PendingRename {
                                from,
                                to,
                                observed_at: previous.observed_at.max(half.observed_at),
                            });
                        }
                    } else {
                        if !self.reserve_rename_slot() {
                            return self.require_rescan();
                        }
                        self.cookie_halves.insert(cookie, half);
                    }
                } else {
                    if !self.push_unmatched(half) {
                        return self.require_rescan();
                    }
                }
            }
            kind => {
                for path in event.paths {
                    let path = match self.path_key(&path) {
                        Ok(PathKey::Accepted(path)) => path,
                        Ok(PathKey::Ignored) => continue,
                        Err(()) => return self.require_rescan(),
                    };
                    if !self.reserve_paths([path.clone()]) {
                        return self.require_rescan();
                    }
                    self.record_path(path, kind, event.observed_at);
                }
            }
        }
        CoalescePush::Accepted
    }

    pub fn flush_ready(
        &mut self,
        now: Instant,
        prior: &dyn PriorEntryLookup,
    ) -> Result<Vec<FsChange>, FsError> {
        self.prune_suppressions(now);
        if self.rescan_required {
            self.clear_pending();
            self.rescan_required = false;
            return Ok(vec![FsChange::RescanRequired]);
        }
        let ready_paths = self
            .pending
            .iter()
            .filter(|(_, pending)| ready(now, pending.last_seen, self.debounce))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let mut changes = BTreeMap::new();
        for path in ready_paths {
            let pending = self.pending.remove(&path).expect("ready path exists");
            self.occupied.remove(&path);
            let workspace_path = parse_path(&path)?;
            if self.is_suppressed(&path, now, prior, &workspace_path) {
                continue;
            }
            let change = match pending.kind {
                PendingKind::Create => FsChange::Create(workspace_path),
                PendingKind::Modify => FsChange::Update(workspace_path),
                PendingKind::Remove => FsChange::Delete(workspace_path),
            };
            changes.insert(path, change);
        }

        let mut expired = Vec::new();
        self.cookie_halves.retain(|_, half| {
            if ready(now, half.observed_at, self.rename) {
                expired.push(RenameHalf {
                    path: half.path.clone(),
                    kind: half.kind,
                    observed_at: half.observed_at,
                });
                false
            } else {
                true
            }
        });
        expired.append(&mut self.unmatched_renames);
        let mut froms = Vec::new();
        let mut tos = Vec::new();
        for half in expired {
            if !ready(now, half.observed_at, self.rename) {
                self.unmatched_renames.push(half);
                continue;
            }
            match half.kind {
                crate::NativeWatchKind::RenameFrom => froms.push(half),
                crate::NativeWatchKind::RenameTo => tos.push(half),
                _ => {}
            }
        }
        let from_signatures = froms
            .iter()
            .map(|half| {
                parse_path(&half.path)
                    .ok()
                    .and_then(|path| prior.signature(&path))
            })
            .collect::<Vec<_>>();
        let to_signatures = tos
            .iter()
            .map(|half| {
                parse_path(&half.path)
                    .ok()
                    .and_then(|path| prior.signature(&path))
            })
            .collect::<Vec<_>>();
        let mut paired_from = BTreeSet::new();
        let mut paired_to = BTreeSet::new();
        for (from_index, from_signature) in from_signatures.iter().enumerate() {
            let Some(from_signature) = from_signature else {
                continue;
            };
            let candidates = to_signatures
                .iter()
                .enumerate()
                .filter(|(index, to_signature)| {
                    to_signature.as_ref() == Some(from_signature)
                        && within_window(
                            froms[from_index].observed_at,
                            tos[*index].observed_at,
                            self.rename,
                        )
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if candidates.len() != 1 {
                continue;
            }
            let to_index = candidates[0];
            let Some(to_signature) = to_signatures[to_index].as_ref() else {
                continue;
            };
            let reverse_candidates = from_signatures
                .iter()
                .filter(|candidate| candidate.as_ref() == Some(to_signature))
                .count();
            if reverse_candidates != 1 {
                continue;
            }
            paired_from.insert(from_index);
            paired_to.insert(to_index);
            let from = &froms[from_index].path;
            let to = &tos[to_index].path;
            self.occupied.remove(from);
            self.occupied.remove(to);
            changes.retain(|path, _| !is_descendant(path, from) && !is_descendant(path, to));
            let from_path = parse_path(from)?;
            let to_path = parse_path(to)?;
            if !(self.is_suppressed(from, now, prior, &from_path)
                && self.is_suppressed(to, now, prior, &to_path))
            {
                changes.insert(
                    from.clone(),
                    FsChange::Rename {
                        from: from_path,
                        to: to_path,
                    },
                );
            }
        }
        for (index, half) in froms.into_iter().enumerate() {
            if paired_from.contains(&index) {
                continue;
            }
            self.occupied.remove(&half.path);
            let path = parse_path(&half.path)?;
            if !self.is_suppressed(&half.path, now, prior, &path) {
                changes.insert(half.path.clone(), FsChange::Delete(path));
            }
        }
        for (index, half) in tos.into_iter().enumerate() {
            if paired_to.contains(&index) {
                continue;
            }
            self.occupied.remove(&half.path);
            let path = parse_path(&half.path)?;
            if !self.is_suppressed(&half.path, now, prior, &path) {
                changes.insert(half.path.clone(), FsChange::Create(path));
            }
        }

        let mut renames = Vec::new();
        self.direct_renames.retain(|rename| {
            if ready(now, rename.observed_at, self.debounce) {
                renames.push((rename.from.clone(), rename.to.clone()));
                false
            } else {
                true
            }
        });
        let raw_renames = renames.clone();
        for (from, to) in collapse_renames(renames) {
            self.occupied.remove(&from);
            self.occupied.remove(&to);
            for (raw_from, raw_to) in &raw_renames {
                self.occupied.remove(raw_from);
                self.occupied.remove(raw_to);
            }
            changes.retain(|path, _| !is_descendant(path, &from) && !is_descendant(path, &to));
            let from_path = parse_path(&from)?;
            let to_path = parse_path(&to)?;
            if !(self.is_suppressed(&from, now, prior, &from_path)
                && self.is_suppressed(&to, now, prior, &to_path))
            {
                changes.insert(
                    from.clone(),
                    FsChange::Rename {
                        from: from_path,
                        to: to_path,
                    },
                );
            }
        }

        Ok(changes.into_values().collect())
    }

    pub fn suppress(&mut self, receipt: &ApplyReceipt) {
        self.prune_suppressions(Instant::now());
        let expires_at = Instant::now() + self.rename;
        for (index, path) in receipt.touched.iter().enumerate() {
            let already_tracked = self.suppressions.contains_key(path.as_str());
            if !already_tracked && self.occupied.len() + self.suppressions.len() >= self.capacity {
                let Some(oldest) = self
                    .suppressions
                    .iter()
                    .min_by_key(|(_, suppression)| suppression.expires_at)
                    .map(|(path, _)| path.clone())
                else {
                    continue;
                };
                self.suppressions.remove(&oldest);
            }
            let observed = receipt.postimages.get(index).and_then(Option::as_ref);
            self.suppressions.insert(
                path.as_str().to_owned(),
                Suppression {
                    missing: observed.is_none(),
                    kind: observed.map_or(WorkspaceEntryKind::Tombstone, |observed| observed.kind),
                    size: observed.map_or(0, |observed| observed.metadata.size),
                    executable: observed.is_some_and(|observed| observed.metadata.executable),
                    fingerprint: observed
                        .map_or_else(synthetic_suppression_fingerprint, |observed| {
                            observed.fingerprint.clone()
                        }),
                    hash: receipt.postimage_hashes.get(index).cloned().flatten(),
                    expires_at,
                },
            );
        }
    }

    fn record_path(&mut self, path: String, kind: crate::NativeWatchKind, observed_at: Instant) {
        let next = match kind {
            crate::NativeWatchKind::Create => PendingKind::Create,
            crate::NativeWatchKind::Modify => PendingKind::Modify,
            crate::NativeWatchKind::Remove => PendingKind::Remove,
            _ => return,
        };
        let Some(previous) = self.pending.get_mut(&path) else {
            self.pending.insert(
                path,
                PendingPath {
                    kind: next,
                    last_seen: observed_at,
                },
            );
            return;
        };
        previous.kind = match (previous.kind, next) {
            (PendingKind::Create, PendingKind::Modify) => PendingKind::Create,
            (PendingKind::Create, PendingKind::Remove) => {
                self.occupied.remove(&path);
                self.pending.remove(&path);
                return;
            }
            (PendingKind::Remove, PendingKind::Create) => PendingKind::Modify,
            (PendingKind::Remove, PendingKind::Modify) => PendingKind::Modify,
            (PendingKind::Modify, PendingKind::Remove) => PendingKind::Remove,
            (previous, _) => previous,
        };
        previous.last_seen = observed_at;
    }

    fn reserve_paths<const N: usize>(&mut self, paths: [String; N]) -> bool {
        let additional = paths
            .iter()
            .filter(|path| !self.occupied.contains(*path))
            .count();
        if self.occupied.len() + additional + self.suppressions.len() > self.capacity {
            return false;
        }
        self.occupied.extend(paths);
        true
    }

    fn reserve_rename_slot(&self) -> bool {
        self.suppressions.len()
            + self.pending.len()
            + self.cookie_halves.len()
            + self.unmatched_renames.len()
            + self.direct_renames.len()
            < self.capacity
    }

    fn push_unmatched(&mut self, half: RenameHalf) -> bool {
        if let Some(previous) = self
            .unmatched_renames
            .iter_mut()
            .find(|previous| previous.path == half.path && previous.kind == half.kind)
        {
            previous.observed_at = previous.observed_at.max(half.observed_at);
            return true;
        }
        if !self.reserve_rename_slot() {
            return false;
        }
        self.unmatched_renames.push(half);
        true
    }

    fn path_key(&self, path: &std::path::Path) -> Result<PathKey, ()> {
        let path = path.to_str().ok_or(())?;
        #[cfg(windows)]
        let path = path.replace('\\', "/");
        #[cfg(windows)]
        let path = path.as_str();
        let workspace_path = WorkspacePath::parse(path).map_err(|_| ())?;
        let included_as_file = self.rules.decide(&workspace_path, false).included;
        let included_as_directory = self.rules.decide(&workspace_path, true).included;
        if included_as_file && included_as_directory {
            Ok(PathKey::Accepted(path.to_owned()))
        } else {
            Ok(PathKey::Ignored)
        }
    }

    fn require_rescan(&mut self) -> CoalescePush {
        self.clear_pending();
        self.rescan_required = true;
        CoalescePush::RescanRequired
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
        self.occupied.clear();
        self.cookie_halves.clear();
        self.unmatched_renames.clear();
        self.direct_renames.clear();
    }

    fn is_suppressed(
        &mut self,
        path: &str,
        now: Instant,
        prior: &dyn PriorEntryLookup,
        workspace_path: &WorkspacePath,
    ) -> bool {
        let Some(suppression) = self.suppressions.get(path) else {
            return false;
        };
        if suppression.expires_at <= now {
            self.suppressions.remove(path);
            return false;
        }
        if suppression.missing {
            return prior.signature(workspace_path).is_none();
        }
        let Some(current) = prior.signature(workspace_path) else {
            return false;
        };
        let basic_match = current.kind == suppression.kind
            && current.size == suppression.size
            && suppression
                .hash
                .as_ref()
                .is_none_or(|hash| current.content_hash.as_ref() == Some(hash));
        if !basic_match {
            return false;
        }
        let Some(observed) = prior.observed(workspace_path) else {
            return false;
        };
        observed.kind == suppression.kind
            && observed.metadata.size == suppression.size
            && observed.metadata.executable == suppression.executable
            && observed.fingerprint == suppression.fingerprint
    }

    fn prune_suppressions(&mut self, now: Instant) {
        self.suppressions
            .retain(|_, suppression| suppression.expires_at > now);
    }
}

fn synthetic_suppression_fingerprint() -> crate::FileFingerprint {
    #[cfg(unix)]
    let file_id = crate::NativeFileId::Unix {
        device: 0,
        inode: 0,
    };
    #[cfg(windows)]
    let file_id = crate::NativeFileId::Windows {
        volume_serial: 0,
        file_index: 0,
    };
    crate::FileFingerprint {
        file_id,
        size: 0,
        modified_at_ns: 0,
        changed_at_ns: 0,
    }
}

fn ready(now: Instant, at: Instant, window: Duration) -> bool {
    now.checked_duration_since(at)
        .is_some_and(|elapsed| elapsed >= window)
}

fn within_window(left: Instant, right: Instant, window: Duration) -> bool {
    left.checked_duration_since(right)
        .or_else(|| right.checked_duration_since(left))
        .is_some_and(|elapsed| elapsed <= window)
}

fn parse_path(path: &str) -> Result<WorkspacePath, FsError> {
    WorkspacePath::parse(path).map_err(|error| FsError::InvalidPath {
        reason: error.reason,
    })
}

fn is_descendant(path: &str, parent: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn collapse_renames(mut renames: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for left in 0..renames.len() {
            for right in 0..renames.len() {
                if left == right || renames[left].1 != renames[right].0 {
                    continue;
                }
                let from = renames[left].0.clone();
                let to = renames[right].1.clone();
                renames[left] = (from, to);
                renames.remove(right);
                changed = true;
                break 'outer;
            }
        }
    }
    renames.sort();
    renames.dedup();
    renames.retain(|(from, to)| from != to);
    renames
}
