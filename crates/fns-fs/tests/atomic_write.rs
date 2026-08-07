use std::fs;
use std::io::Cursor;

use fns_fs::{
    ApplyId, ApplyObservation, AtomicWorkspaceWriter, ContentCache, ExpectedEntry, FsOperation,
    MemoryHashCache, RootedWorkspace,
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
