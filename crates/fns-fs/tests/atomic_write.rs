use std::fs;
use std::io::Cursor;

use fns_fs::{
    ApplyCheckpoint, ApplyId, ApplyObservation, ApplyObserver, AtomicWorkspaceWriter, ContentCache,
    ExpectedEntry, FsOperation, MemoryHashCache, RootedWorkspace,
};
use fns_protocol::{
    WorkspaceContentHash, WorkspaceEntryKind, WorkspaceFileMetadata, WorkspacePath,
};

fn path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).unwrap()
}

fn hash(bytes: &[u8]) -> WorkspaceContentHash {
    WorkspaceContentHash::parse(&format!("blake3:{}", blake3::hash(bytes).to_hex())).unwrap()
}

fn metadata(size: u64) -> WorkspaceFileMetadata {
    WorkspaceFileMetadata {
        size,
        modified_at_ms: 0,
        executable: false,
    }
}

fn import(content: &ContentCache, bytes: &[u8]) -> WorkspaceContentHash {
    let content_hash = hash(bytes);
    content
        .import(&content_hash, bytes.len() as u64, Cursor::new(bytes))
        .unwrap();
    content_hash
}

struct MutateAfterPreimageValidation {
    path: std::path::PathBuf,
}

impl ApplyObserver for MutateAfterPreimageValidation {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::PreimageValidated {
            fs::write(&self.path, b"local").unwrap();
        }
    }
}

struct CreateTargetAfterPreimageValidation {
    path: std::path::PathBuf,
}

struct CreateDirectoryAfterPreimageValidation {
    path: std::path::PathBuf,
}

#[cfg(unix)]
struct MutateAfterFilesystemCommitted {
    path: std::path::PathBuf,
}

#[cfg(unix)]
struct SwapSymlinkAfterPreimageValidation {
    path: std::path::PathBuf,
    target: std::path::PathBuf,
}

struct CreateDeleteTombAfterCheck {
    root: std::path::PathBuf,
    apply_id: ApplyId,
}

impl ApplyObserver for CreateDeleteTombAfterCheck {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::DestinationBackedUp {
            fs::write(
                self.root.join(format!(".fns-delete-{}", self.apply_id.0)),
                b"sentinel",
            )
            .unwrap();
        }
    }
}

#[cfg(unix)]
struct ChangeBackupMetadata {
    root: std::path::PathBuf,
    apply_id: ApplyId,
}

#[cfg(unix)]
impl ApplyObserver for ChangeBackupMetadata {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::DestinationBackedUp {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(
                self.root.join(format!(".fns-delete-{}", self.apply_id.0)),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
    }
}

impl ApplyObserver for CreateTargetAfterPreimageValidation {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::DestinationBackedUp {
            fs::write(&self.path, b"late").unwrap();
        }
    }
}

impl ApplyObserver for CreateDirectoryAfterPreimageValidation {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::PreimageValidated {
            fs::create_dir(&self.path).unwrap();
        }
    }
}

#[cfg(unix)]
impl ApplyObserver for MutateAfterFilesystemCommitted {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::FilesystemCommitted {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[cfg(unix)]
impl ApplyObserver for SwapSymlinkAfterPreimageValidation {
    fn checkpoint(&self, checkpoint: ApplyCheckpoint) {
        if checkpoint == ApplyCheckpoint::PreimageValidated {
            use std::os::unix::fs::symlink;

            fs::remove_file(&self.path).unwrap();
            symlink(&self.target, &self.path).unwrap();
        }
    }
}

fn present(
    root: &RootedWorkspace,
    workspace_path: &WorkspacePath,
    content_hash: Option<WorkspaceContentHash>,
) -> ExpectedEntry {
    let observed = root.inspect(workspace_path).unwrap().unwrap();
    ExpectedEntry::Present {
        kind: observed.kind,
        content_hash,
        fingerprint: observed.fingerprint,
    }
}

#[test]
fn hash_mismatch_never_replaces_destination() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a.txt"), b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let expected = WorkspaceContentHash::parse(
        "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
    )
    .unwrap();

    assert!(content.import(&expected, 5, Cursor::new(b"jello")).is_err());
    let _writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);
    assert_eq!(fs::read(root_dir.path().join("a.txt")).unwrap(), b"old");
}

#[test]
fn file_create_and_update_commit_exact_metadata() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let updated_hash = import(&content, b"world");
    let workspace_path = path("a.txt");
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let check = RootedWorkspace::open(root_dir.path()).unwrap();
    let writer = AtomicWorkspaceWriter::new(root, content);

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertFile {
                path: workspace_path.clone(),
                content_hash: content_hash.clone(),
                metadata: metadata(5),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(fs::read(root_dir.path().join("a.txt")).unwrap(), b"hello");
    let observed = check.inspect(&workspace_path).unwrap().unwrap();
    assert_eq!(observed.kind, WorkspaceEntryKind::File);
    assert_eq!(observed.metadata, metadata(5));

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertFile {
                path: workspace_path.clone(),
                content_hash: updated_hash,
                metadata: metadata(5),
                expected: present(&check, &workspace_path, Some(content_hash)),
            },
        )
        .unwrap();
    assert_eq!(fs::read(root_dir.path().join("a.txt")).unwrap(), b"world");
}

#[test]
fn concurrent_local_change_is_not_overwritten_after_preimage_validation() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("a.txt");
    fs::write(&target, b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let replacement_hash = import(&content, b"remote");
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("a.txt"), Some(hash(b"old")));
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(MutateAfterPreimageValidation {
            path: target.clone(),
        }),
    );

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertFile {
                path: path("a.txt"),
                content_hash: replacement_hash,
                metadata: metadata(6),
                expected,
            },
        )
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert_eq!(fs::read(target).unwrap(), b"local");
}

#[cfg(unix)]
#[test]
fn concurrent_metadata_change_is_not_deleted_after_destination_backup() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("a.txt");
    fs::write(&target, b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let replacement_hash = import(&content, b"remote");
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let target_path = path("a.txt");
    let expected = present(&root, &target_path, Some(hash(b"old")));
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(ChangeBackupMetadata {
            root: root_dir.path().to_path_buf(),
            apply_id,
        }),
    );

    let error = writer
        .apply(
            apply_id,
            &FsOperation::UpsertFile {
                path: target_path,
                content_hash: replacement_hash,
                metadata: metadata(6),
                expected,
            },
        )
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert_eq!(fs::read(target).unwrap(), b"old");
}

#[cfg(unix)]
#[test]
fn receipt_captures_postimage_before_filesystem_committed_observer_runs() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("a.txt");
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"remote");
    let writer = AtomicWorkspaceWriter::with_observer(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        content,
        Box::new(MutateAfterFilesystemCommitted {
            path: target.clone(),
        }),
    );

    let receipt = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertFile {
                path: path("a.txt"),
                content_hash,
                metadata: metadata(6),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert!(!receipt.postimages[0].as_ref().unwrap().metadata.executable);
    assert!(target.exists());
}

#[test]
fn concurrent_local_change_is_not_deleted_after_preimage_validation() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("a.txt");
    fs::write(&target, b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("a.txt"), Some(hash(b"old")));
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(MutateAfterPreimageValidation {
            path: target.clone(),
        }),
    );

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Delete {
                path: path("a.txt"),
                expected,
            },
        )
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert_eq!(fs::read(target).unwrap(), b"local");
}

#[test]
fn delete_does_not_replace_a_tomb_created_after_the_check() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a.txt"), b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let source = path("a.txt");
    let expected = present(&root, &source, Some(hash(b"old")));
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(CreateDeleteTombAfterCheck {
            root: root_dir.path().to_path_buf(),
            apply_id,
        }),
    );

    let error = writer
        .apply(
            apply_id,
            &FsOperation::Delete {
                path: source,
                expected,
            },
        )
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::Io { .. }));
    assert_eq!(fs::read(root_dir.path().join("a.txt")).unwrap(), b"old");
    assert_eq!(
        fs::read(root_dir.path().join(format!(".fns-delete-{}", apply_id.0))).unwrap(),
        b"sentinel"
    );
}

#[test]
fn concurrent_local_change_is_not_moved_by_rename_after_preimage_validation() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("old");
    fs::write(&target, b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("old"), Some(hash(b"old")));
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(MutateAfterPreimageValidation {
            path: target.clone(),
        }),
    );

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Rename {
                path: path("old"),
                new_path: path("new"),
                content_hash: Some(hash(b"old")),
                metadata: metadata(3),
                source_expected: expected,
                target_expected: ExpectedEntry::Missing,
            },
        )
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert_eq!(fs::read(target).unwrap(), b"local");
    assert!(fs::read(root_dir.path().join("new")).is_err());
}

#[test]
fn target_created_after_backup_check_is_not_overwritten_by_rename() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("old"), Some(hash(b"old")));
    let target = root_dir.path().join("new");
    let writer = AtomicWorkspaceWriter::with_observer(
        root,
        content,
        Box::new(CreateTargetAfterPreimageValidation {
            path: target.clone(),
        }),
    );

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Rename {
                path: path("old"),
                new_path: path("new"),
                content_hash: Some(hash(b"old")),
                metadata: metadata(3),
                source_expected: expected,
                target_expected: ExpectedEntry::Missing,
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        fns_fs::FsError::ContentMismatch | fns_fs::FsError::Io { .. }
    ));
    assert_eq!(fs::read(root_dir.path().join("old")).unwrap(), b"old");
    assert_eq!(fs::read(target).unwrap(), b"late");
}

#[test]
fn mkdir_is_idempotent_for_matching_directory() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let check = RootedWorkspace::open(root_dir.path()).unwrap();
    let writer = AtomicWorkspaceWriter::new(root, content);
    let workspace_path = path("dir");
    let operation = FsOperation::Mkdir {
        path: workspace_path.clone(),
        metadata: metadata(0),
        expected: ExpectedEntry::Missing,
    };

    writer
        .apply(ApplyId(uuid::Uuid::new_v4()), &operation)
        .unwrap();
    let expected = present(&check, &workspace_path, None);
    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Mkdir {
                path: workspace_path.clone(),
                metadata: metadata(0),
                expected,
            },
        )
        .unwrap();

    assert!(check.inspect(&workspace_path).unwrap().is_some());
}

#[test]
fn mkdir_missing_does_not_claim_a_directory_created_after_preimage_check() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let target = root_dir.path().join("dir");
    let writer = AtomicWorkspaceWriter::with_observer(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        ContentCache::open(state_dir.path()).unwrap(),
        Box::new(CreateDirectoryAfterPreimageValidation {
            path: target.clone(),
        }),
    );

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Mkdir {
                path: path("dir"),
                metadata: metadata(0),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap_err();

    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert!(target.is_dir());
}

#[cfg(unix)]
#[test]
fn relative_symlink_is_created_without_following() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let target_hash = import(&content, b"target");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertSymlink {
                path: path("link"),
                content_hash: target_hash,
                metadata: metadata(6),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(
        fs::read_link(root_dir.path().join("link")).unwrap(),
        std::path::Path::new("target")
    );
}

#[cfg(unix)]
#[test]
fn absolute_or_escaping_symlink_is_rejected() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let target_hash = import(&content, b"/tmp");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);

    assert!(
        writer
            .apply(
                ApplyId(uuid::Uuid::new_v4()),
                &FsOperation::UpsertSymlink {
                    path: path("link"),
                    content_hash: target_hash,
                    metadata: metadata(4),
                    expected: ExpectedEntry::Missing,
                },
            )
            .is_err()
    );
    assert!(!root_dir.path().join("link").exists());
    assert!(fs::read_link(root_dir.path().join("link")).is_err());
}

#[cfg(unix)]
#[test]
fn indirect_symlink_escape_is_rejected_before_commit() {
    use std::os::unix::fs::symlink;

    let area = tempfile::tempdir().unwrap();
    let root_dir = area.path().join("root");
    let outside = area.path().join("outside");
    fs::create_dir(&root_dir).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root_dir.join("escape")).unwrap();

    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let target_hash = import(&content, b"escape/secret");
    let writer = AtomicWorkspaceWriter::new(RootedWorkspace::open(&root_dir).unwrap(), content);

    assert!(
        writer
            .apply(
                ApplyId(uuid::Uuid::new_v4()),
                &FsOperation::UpsertSymlink {
                    path: path("link"),
                    content_hash: target_hash,
                    metadata: metadata(13),
                    expected: ExpectedEntry::Missing,
                },
            )
            .is_err()
    );
    assert!(fs::read_link(root_dir.join("link")).is_err());
    assert!(
        !fs::read_dir(&root_dir)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(".fns-tmp-"))
    );
}

#[cfg(unix)]
#[test]
fn symlink_swap_after_precheck_does_not_leave_an_escaped_link() {
    use std::os::unix::fs::symlink;

    let area = tempfile::tempdir().unwrap();
    let root_dir = area.path().join("root");
    let outside_dir = area.path().join("outside");
    fs::create_dir(&root_dir).unwrap();
    fs::create_dir(&outside_dir).unwrap();
    symlink("inside", root_dir.join("link")).unwrap();
    fs::write(outside_dir.join("secret"), b"secret").unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let target_hash = import(&content, b"inside");
    let writer = AtomicWorkspaceWriter::with_observer(
        RootedWorkspace::open(&root_dir).unwrap(),
        content,
        Box::new(SwapSymlinkAfterPreimageValidation {
            path: root_dir.join("link"),
            target: std::path::PathBuf::from("../outside/secret"),
        }),
    );
    let rooted = RootedWorkspace::open(&root_dir).unwrap();
    let expected = present(&rooted, &path("link"), Some(hash(b"inside")));

    let error = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertSymlink {
                path: path("link"),
                content_hash: target_hash,
                metadata: metadata(6),
                expected,
            },
        )
        .unwrap_err();

    assert!(matches!(error, fns_fs::FsError::PathEscape));
    assert!(fs::read_link(root_dir.join("link")).is_err());
}

#[test]
fn mkdir_postimage_matches_directory_metadata_contract() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let writer = AtomicWorkspaceWriter::new(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        ContentCache::open(state_dir.path()).unwrap(),
    );
    let operation = FsOperation::Mkdir {
        path: path("dir"),
        metadata: WorkspaceFileMetadata {
            size: 0,
            modified_at_ms: 1_234,
            executable: false,
        },
        expected: ExpectedEntry::Missing,
    };

    let apply_id = ApplyId(uuid::Uuid::new_v4());
    writer.apply(apply_id, &operation).unwrap();
    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Postimage
    );
}

#[test]
fn rename_rejects_a_postimage_that_does_not_match_its_contract() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("old"), Some(hash(b"old")));
    let writer = AtomicWorkspaceWriter::new(root, ContentCache::open(state_dir.path()).unwrap());

    assert!(
        writer
            .apply(
                ApplyId(uuid::Uuid::new_v4()),
                &FsOperation::Rename {
                    path: path("old"),
                    new_path: path("new"),
                    content_hash: Some(hash(b"old")),
                    metadata: metadata(99),
                    source_expected: expected,
                    target_expected: ExpectedEntry::Missing,
                },
            )
            .is_err()
    );
    assert_eq!(fs::read(root_dir.path().join("old")).unwrap(), b"old");
    assert!(fs::read(root_dir.path().join("new")).is_err());
}

#[test]
fn retry_replaces_only_its_own_leftover_temp_artifact() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    fs::write(
        root_dir.path().join(format!(".fns-tmp-{}", apply_id.0)),
        b"stale",
    )
    .unwrap();
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);

    writer
        .apply(
            apply_id,
            &FsOperation::UpsertFile {
                path: path("a"),
                content_hash,
                metadata: metadata(5),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();
    assert_eq!(fs::read(root_dir.path().join("a")).unwrap(), b"hello");
}

#[test]
fn delete_moves_then_finalizes_tomb() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a.txt"), b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = hash(b"old");
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let expected = present(&root, &path("a.txt"), Some(content_hash));
    let writer = AtomicWorkspaceWriter::new(root, content);
    let receipt = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Delete {
                path: path("a.txt"),
                expected,
            },
        )
        .unwrap();

    assert!(!root_dir.path().join("a.txt").exists());
    assert!(receipt.cleanup_name.is_some());
    writer.finalize(&receipt).unwrap();
    assert!(fs::read_dir(root_dir.path()).unwrap().next().is_none());
}

#[test]
fn file_rename_preserves_bytes() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old.txt"), b"old").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let writer = AtomicWorkspaceWriter::new(root, content);

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Rename {
                path: path("old.txt"),
                new_path: path("new.txt"),
                content_hash: Some(hash(b"old")),
                metadata: metadata(3),
                source_expected: present(
                    &RootedWorkspace::open(root_dir.path()).unwrap(),
                    &path("old.txt"),
                    Some(hash(b"old")),
                ),
                target_expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(fs::read(root_dir.path().join("new.txt")).unwrap(), b"old");
    assert!(!root_dir.path().join("old.txt").exists());
}

#[test]
fn rename_replaces_a_matching_target_without_overwriting_a_changed_target() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    fs::write(root_dir.path().join("new"), b"target").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let source = path("old");
    let target = path("new");
    let source_expected = present(&root, &source, Some(hash(b"old")));
    let target_expected = present(&root, &target, Some(hash(b"target")));
    let writer = AtomicWorkspaceWriter::new(root, ContentCache::open(state_dir.path()).unwrap());

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Rename {
                path: source,
                new_path: target,
                content_hash: Some(hash(b"old")),
                metadata: metadata(3),
                source_expected,
                target_expected,
            },
        )
        .unwrap();

    assert_eq!(fs::read(root_dir.path().join("new")).unwrap(), b"old");
    assert!(!root_dir.path().join("old").exists());
}

#[test]
fn directory_rename_moves_whole_tree() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(root_dir.path().join("old")).unwrap();
    fs::write(root_dir.path().join("old").join("child"), b"child").unwrap();
    let writer = AtomicWorkspaceWriter::new(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        ContentCache::open(state_dir.path()).unwrap(),
    );

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Rename {
                path: path("old"),
                new_path: path("new"),
                content_hash: None,
                metadata: metadata(0),
                source_expected: present(
                    &RootedWorkspace::open(root_dir.path()).unwrap(),
                    &path("old"),
                    None,
                ),
                target_expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(
        fs::read(root_dir.path().join("new").join("child")).unwrap(),
        b"child"
    );
}

#[test]
fn occupied_rename_target_is_not_overwritten() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    fs::write(root_dir.path().join("new"), b"new").unwrap();
    let writer = AtomicWorkspaceWriter::new(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        ContentCache::open(state_dir.path()).unwrap(),
    );

    assert!(
        writer
            .apply(
                ApplyId(uuid::Uuid::new_v4()),
                &FsOperation::Rename {
                    path: path("old"),
                    new_path: path("new"),
                    content_hash: Some(hash(b"old")),
                    metadata: metadata(3),
                    source_expected: present(
                        &RootedWorkspace::open(root_dir.path()).unwrap(),
                        &path("old"),
                        Some(hash(b"old")),
                    ),
                    target_expected: ExpectedEntry::Missing,
                },
            )
            .is_err()
    );
    assert_eq!(fs::read(root_dir.path().join("old")).unwrap(), b"old");
    assert_eq!(fs::read(root_dir.path().join("new")).unwrap(), b"new");
}

#[test]
fn observe_classifies_preimage_postimage_and_divergence() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);
    let operation = FsOperation::UpsertFile {
        path: path("a"),
        content_hash,
        metadata: metadata(5),
        expected: ExpectedEntry::Missing,
    };
    let apply_id = ApplyId(uuid::Uuid::new_v4());

    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Preimage
    );
    writer.apply(apply_id, &operation).unwrap();
    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Postimage
    );
    fs::write(root_dir.path().join("a"), b"other").unwrap();
    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Diverged
    );
}

#[test]
fn successful_finalize_leaves_no_internal_residue() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a"), b"a").unwrap();
    let writer = AtomicWorkspaceWriter::new(
        RootedWorkspace::open(root_dir.path()).unwrap(),
        ContentCache::open(state_dir.path()).unwrap(),
    );
    let receipt = writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::Delete {
                path: path("a"),
                expected: present(
                    &RootedWorkspace::open(root_dir.path()).unwrap(),
                    &path("a"),
                    Some(hash(b"a")),
                ),
            },
        )
        .unwrap();
    writer.finalize(&receipt).unwrap();

    assert!(fs::read_dir(root_dir.path()).unwrap().next().is_none());
    assert!(fs::read_dir(state_dir.path()).unwrap().next().is_some());
    let _ = MemoryHashCache::default();
}

#[cfg(unix)]
#[test]
fn apply_remains_confined_after_root_path_is_replaced() {
    let area = tempfile::tempdir().unwrap();
    let root_dir = area.path().join("root");
    let moved = area.path().join("moved");
    fs::create_dir(&root_dir).unwrap();
    let rooted = RootedWorkspace::open(&root_dir).unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"original-root");
    let writer = AtomicWorkspaceWriter::new(rooted, content);

    fs::rename(&root_dir, &moved).unwrap();
    fs::create_dir(&root_dir).unwrap();

    writer
        .apply(
            ApplyId(uuid::Uuid::new_v4()),
            &FsOperation::UpsertFile {
                path: path("entry"),
                content_hash,
                metadata: metadata(13),
                expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(fs::read(moved.join("entry")).unwrap(), b"original-root");
    assert!(!root_dir.join("entry").exists());
}

#[test]
fn observe_requires_the_matching_apply_id_for_a_delete_tomb() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a"), b"a").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let operation = FsOperation::Delete {
        path: path("a"),
        expected: present(&root, &path("a"), Some(hash(b"a"))),
    };
    let writer = AtomicWorkspaceWriter::new(root, ContentCache::open(state_dir.path()).unwrap());
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let other_apply_id = ApplyId(uuid::Uuid::new_v4());

    writer.apply(apply_id, &operation).unwrap();

    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Postimage
    );
    assert_eq!(
        writer.observe(other_apply_id, &operation).unwrap(),
        ApplyObservation::Diverged
    );
}

#[cfg(unix)]
#[test]
fn observe_rejects_postimage_with_changed_metadata() {
    use std::os::unix::fs::PermissionsExt;

    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);
    let operation = FsOperation::UpsertFile {
        path: path("a"),
        content_hash,
        metadata: metadata(5),
        expected: ExpectedEntry::Missing,
    };
    let apply_id = ApplyId(uuid::Uuid::new_v4());

    writer.apply(apply_id, &operation).unwrap();
    fs::set_permissions(root_dir.path().join("a"), fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Diverged
    );
}

#[test]
fn observe_rejects_unrelated_temp_artifact() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);
    let operation = FsOperation::UpsertFile {
        path: path("a"),
        content_hash,
        metadata: metadata(5),
        expected: ExpectedEntry::Missing,
    };
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let other_apply_id = ApplyId(uuid::Uuid::new_v4());
    fs::write(
        root_dir
            .path()
            .join(format!(".fns-tmp-{}", other_apply_id.0)),
        b"stale",
    )
    .unwrap();

    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Diverged
    );
}

#[test]
fn observe_rejects_unrelated_rename_backup_artifact() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let content_hash = import(&content, b"hello");
    let writer =
        AtomicWorkspaceWriter::new(RootedWorkspace::open(root_dir.path()).unwrap(), content);
    let operation = FsOperation::UpsertFile {
        path: path("a"),
        content_hash,
        metadata: metadata(5),
        expected: ExpectedEntry::Missing,
    };
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    fs::write(
        root_dir
            .path()
            .join(format!(".fns-rename-{}", uuid::Uuid::new_v4())),
        b"stale",
    )
    .unwrap();

    assert_eq!(
        writer.observe(apply_id, &operation).unwrap(),
        ApplyObservation::Diverged
    );
}

#[test]
fn rename_retry_resumes_after_source_backup_was_staged() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let source = path("old");
    let expected = present(&root, &source, Some(hash(b"old")));
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    fs::rename(
        root_dir.path().join("old"),
        root_dir.path().join(format!(".fns-rename-{}", apply_id.0)),
    )
    .unwrap();
    let writer = AtomicWorkspaceWriter::new(root, ContentCache::open(state_dir.path()).unwrap());

    writer
        .apply(
            apply_id,
            &FsOperation::Rename {
                path: source,
                new_path: path("new"),
                content_hash: Some(hash(b"old")),
                metadata: metadata(3),
                source_expected: expected,
                target_expected: ExpectedEntry::Missing,
            },
        )
        .unwrap();

    assert_eq!(fs::read(root_dir.path().join("new")).unwrap(), b"old");
    assert!(
        !root_dir
            .path()
            .join(format!(".fns-rename-{}", apply_id.0))
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn rename_retry_recovers_after_source_was_committed_before_metadata() {
    use std::os::unix::fs::PermissionsExt;

    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("old"), b"old").unwrap();
    fs::write(root_dir.path().join("new"), b"target").unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let source_hash = import(&content, b"old");
    let rooted = RootedWorkspace::open(root_dir.path()).unwrap();
    let source = path("old");
    let target = path("new");
    let source_expected = present(&rooted, &source, Some(source_hash.clone()));
    let target_expected = present(&rooted, &target, Some(hash(b"target")));
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let source_backup = root_dir.path().join(format!(".fns-rename-{}", apply_id.0));
    let target_backup = root_dir.path().join(format!(".fns-delete-{}", apply_id.0));
    fs::rename(root_dir.path().join("old"), &source_backup).unwrap();
    fs::rename(root_dir.path().join("new"), &target_backup).unwrap();
    fs::rename(&source_backup, root_dir.path().join("new")).unwrap();
    fs::set_permissions(
        root_dir.path().join("new"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let writer = AtomicWorkspaceWriter::new(rooted, content);
    let receipt = writer
        .apply(
            apply_id,
            &FsOperation::Rename {
                path: source,
                new_path: target,
                content_hash: Some(source_hash),
                metadata: metadata(3),
                source_expected,
                target_expected,
            },
        )
        .unwrap();

    assert_eq!(fs::read(root_dir.path().join("new")).unwrap(), b"old");
    assert!(
        !root_dir
            .path()
            .join(format!(".fns-delete-{}", apply_id.0))
            .exists()
    );
    assert!(receipt.cleanup_name.is_none());
}

#[test]
fn finalize_rejects_a_receipt_with_a_different_apply_id() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("a"), b"a").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let operation = FsOperation::Delete {
        path: path("a"),
        expected: present(&root, &path("a"), Some(hash(b"a"))),
    };
    let writer = AtomicWorkspaceWriter::new(root, ContentCache::open(state_dir.path()).unwrap());
    let apply_id = ApplyId(uuid::Uuid::new_v4());
    let mut receipt = writer.apply(apply_id, &operation).unwrap();
    receipt.apply_id = ApplyId(uuid::Uuid::new_v4());

    assert!(writer.finalize(&receipt).is_err());
    assert!(
        root_dir
            .path()
            .join(format!(".fns-delete-{}", apply_id.0))
            .exists()
    );
}
