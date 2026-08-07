use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fns_fs::{
    ApplyId, ApplyReceipt, EntrySignature, EventCoalescer, FileFingerprint, FsChange, NativeFileId,
    NativeWatchKind, NormalizedWatchEvent, PriorEntryLookup,
};
use fns_protocol::{
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspacePath,
};

struct Prior(BTreeMap<String, EntrySignature>);

impl PriorEntryLookup for Prior {
    fn signature(&self, path: &WorkspacePath) -> Option<EntrySignature> {
        self.0.get(path.as_str()).cloned()
    }
}

fn path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).unwrap()
}

fn event(
    kind: NativeWatchKind,
    paths: &[&str],
    rename_cookie: Option<u64>,
    observed_at: Instant,
) -> NormalizedWatchEvent {
    NormalizedWatchEvent {
        kind,
        paths: paths.iter().map(PathBuf::from).collect(),
        rename_cookie,
        observed_at,
    }
}

fn flush(coalescer: &mut EventCoalescer, start: Instant, prior: &Prior) -> Vec<FsChange> {
    coalescer
        .flush_ready(start + Duration::from_millis(501), prior)
        .unwrap()
}

fn file_signature(hash: Option<&str>, size: u64) -> EntrySignature {
    EntrySignature {
        kind: WorkspaceEntryKind::File,
        content_hash: hash.map(|value| WorkspaceContentHash::parse(value).unwrap()),
        size,
    }
}

fn receipt(path_value: &str, hash: &str, size: u64) -> ApplyReceipt {
    ApplyReceipt {
        apply_id: ApplyId(uuid::Uuid::nil()),
        touched: vec![path(path_value)],
        postimages: vec![Some(fns_fs::ObservedEntry {
            path: path(path_value),
            kind: WorkspaceEntryKind::File,
            metadata: WorkspaceFileMetadata {
                size,
                modified_at_ms: 0,
                executable: false,
            },
            fingerprint: FileFingerprint {
                file_id: NativeFileId::Unix {
                    device: 1,
                    inode: 2,
                },
                size,
                modified_at_ns: 3,
                changed_at_ns: 4,
            },
            symlink_target: None,
        })],
        postimage_hashes: vec![Some(WorkspaceContentHash::parse(hash).unwrap())],
        cleanup_name: None,
    }
}

#[test]
fn folds_bursts_and_pairs_only_unique_renames() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    let prior = Prior(BTreeMap::new());

    coalescer.push(event(NativeWatchKind::Modify, &["notes/a.md"], None, start));
    coalescer.push(event(
        NativeWatchKind::Modify,
        &["notes/a.md"],
        None,
        start + Duration::from_millis(10),
    ));
    coalescer.push(event(
        NativeWatchKind::RenameFrom,
        &["old.txt"],
        Some(7),
        start,
    ));
    coalescer.push(event(
        NativeWatchKind::RenameTo,
        &["new.txt"],
        Some(7),
        start + Duration::from_millis(20),
    ));

    assert_eq!(
        flush(&mut coalescer, start, &prior),
        vec![
            FsChange::Update(path("notes/a.md")),
            FsChange::Rename {
                from: path("old.txt"),
                to: path("new.txt"),
            },
        ]
    );
}

#[test]
fn create_modify_is_create() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::Create, &["a"], None, start));
    coalescer.push(event(NativeWatchKind::Modify, &["a"], None, start));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Create(path("a")),]
    );
}

#[test]
fn create_remove_is_noop() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::Create, &["a"], None, start));
    coalescer.push(event(NativeWatchKind::Remove, &["a"], None, start));
    assert!(flush(&mut coalescer, start, &Prior(BTreeMap::new())).is_empty());
}

#[test]
fn remove_create_is_update() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::Remove, &["a"], None, start));
    coalescer.push(event(NativeWatchKind::Create, &["a"], None, start));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Update(path("a")),]
    );
}

#[test]
fn modify_remove_is_delete() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::Modify, &["a"], None, start));
    coalescer.push(event(NativeWatchKind::Remove, &["a"], None, start));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Delete(path("a")),]
    );
}

#[test]
fn rename_chain_collapses_to_endpoints() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::RenameBoth, &["a", "b"], None, start));
    coalescer.push(event(
        NativeWatchKind::RenameBoth,
        &["b", "c"],
        None,
        start + Duration::from_millis(10),
    ));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Rename {
            from: path("a"),
            to: path("c"),
        },]
    );
}

#[test]
fn directory_rename_folds_child_burst() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(
        NativeWatchKind::RenameBoth,
        &["old", "new"],
        None,
        start,
    ));
    coalescer.push(event(
        NativeWatchKind::Modify,
        &["old/child.txt"],
        None,
        start + Duration::from_millis(10),
    ));
    coalescer.push(event(
        NativeWatchKind::Modify,
        &["new/child.txt"],
        None,
        start + Duration::from_millis(10),
    ));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Rename {
            from: path("old"),
            to: path("new"),
        },]
    );
}

#[test]
fn unmatched_rename_expires_to_delete_or_create() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::RenameFrom, &["old"], None, start));
    coalescer.push(event(NativeWatchKind::RenameTo, &["new"], None, start));
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::Create(path("new")), FsChange::Delete(path("old")),]
    );
}

#[test]
fn ambiguous_signature_does_not_guess_rename() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.push(event(NativeWatchKind::Remove, &["old"], None, start));
    coalescer.push(event(NativeWatchKind::Create, &["new"], None, start));
    let prior = Prior(BTreeMap::from([
        ("old".into(), file_signature(None, 3)),
        ("new".into(), file_signature(None, 3)),
    ]));
    assert_eq!(
        flush(&mut coalescer, start, &prior),
        vec![FsChange::Create(path("new")), FsChange::Delete(path("old")),]
    );
}

#[test]
fn exact_postimage_is_suppressed() {
    let start = Instant::now();
    let hash = "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f";
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.suppress(&receipt("a", hash, 5));
    coalescer.push(event(NativeWatchKind::Modify, &["a"], None, start));
    let prior = Prior(BTreeMap::from([(
        "a".into(),
        file_signature(Some(hash), 5),
    )]));
    assert!(
        coalescer
            .flush_ready(start + Duration::from_millis(201), &prior)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn newer_user_change_is_not_suppressed() {
    let start = Instant::now();
    let old_hash = "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f";
    let new_hash = "blake3:9b3b6b8d7f0d5d2e5c0b3e6b4a9f1c7b2d4e6f8a0c1e3d5f7b9a1c3e5f7a9b1c";
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    coalescer.suppress(&receipt("a", old_hash, 5));
    coalescer.push(event(NativeWatchKind::Modify, &["a"], None, start));
    let prior = Prior(BTreeMap::from([(
        "a".into(),
        file_signature(Some(new_hash), 5),
    )]));
    assert_eq!(
        coalescer
            .flush_ready(start + Duration::from_millis(201), &prior)
            .unwrap(),
        vec![FsChange::Update(path("a")),]
    );
}

#[test]
fn capacity_overflow_is_one_rescan() {
    let start = Instant::now();
    let mut coalescer = EventCoalescer::new(
        fns_fs::DEBOUNCE_WINDOW,
        fns_fs::RENAME_WINDOW,
        fns_fs::COALESCER_PATH_CAPACITY,
    );
    for index in 0..(fns_fs::COALESCER_PATH_CAPACITY + 1) {
        coalescer.push(event(
            NativeWatchKind::Modify,
            &[&format!("file-{index}")],
            None,
            start,
        ));
    }
    assert_eq!(
        flush(&mut coalescer, start, &Prior(BTreeMap::new())),
        vec![FsChange::RescanRequired]
    );
}
