use std::fs;
use std::io::Read;

use fns_fs::{ContentCache, MemoryHashCache, RootedWorkspace};
use fns_protocol::WorkspacePath;

#[test]
fn stages_hello_with_go_compatible_blake3_and_reuses_stable_cache() {
    let root_dir = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join("hello.txt"), b"hello").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let mut cache = MemoryHashCache::default();
    let path = WorkspacePath::parse("hello.txt").unwrap();

    let first = content
        .stage_workspace_entry(&root, &path, &mut cache)
        .unwrap();
    let second = content
        .stage_workspace_entry(&root, &path, &mut cache)
        .unwrap();

    assert_eq!(
        first.content_hash.as_str(),
        "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
    );
    assert_eq!(first, second);
    let mut cached = content.open_blob(&first.content_hash).unwrap();
    let mut bytes = Vec::new();
    cached.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"hello");
    assert_eq!(cache.hits(), 1);
}

#[cfg(unix)]
#[test]
fn staging_remains_confined_after_root_path_is_replaced() {
    let area = tempfile::tempdir().unwrap();
    let state_dir = tempfile::tempdir().unwrap();
    let root_path = area.path().join("root");
    let moved_path = area.path().join("moved");
    fs::create_dir(&root_path).unwrap();
    fs::write(root_path.join("entry"), b"original").unwrap();
    let root = RootedWorkspace::open(&root_path).unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let mut cache = MemoryHashCache::default();
    let path = WorkspacePath::parse("entry").unwrap();

    fs::rename(&root_path, &moved_path).unwrap();
    fs::create_dir(&root_path).unwrap();
    fs::write(root_path.join("entry"), b"replacement").unwrap();

    let descriptor = content
        .stage_workspace_entry(&root, &path, &mut cache)
        .unwrap();
    assert_eq!(descriptor.size, 8);
    assert_eq!(fs::read(moved_path.join("entry")).unwrap(), b"original");
}

#[test]
fn import_hash_mismatch_keeps_existing_blob() {
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let expected = WorkspacePath::parse("unused").unwrap();
    let root_dir = tempfile::tempdir().unwrap();
    fs::write(root_dir.path().join(expected.as_str()), b"good").unwrap();
    let root = RootedWorkspace::open(root_dir.path()).unwrap();
    let mut cache = MemoryHashCache::default();
    let descriptor = content
        .stage_workspace_entry(&root, &expected, &mut cache)
        .unwrap();

    let error = content
        .import(&descriptor.content_hash, 3, &b"bad"[..])
        .unwrap_err();
    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    let mut existing = content.open_blob(&descriptor.content_hash).unwrap();
    let mut bytes = Vec::new();
    existing.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"good");
}

#[test]
fn same_size_tampered_blob_is_rejected_without_replacement() {
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    let expected = fns_protocol::WorkspaceContentHash::parse(
        "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
    )
    .unwrap();

    content.import(&expected, 5, &b"hello"[..]).unwrap();
    let blob_path = state_dir
        .path()
        .join("blobs")
        .join(expected.as_str().trim_start_matches("blake3:"));
    fs::write(&blob_path, b"jello").unwrap();

    let error = content.import(&expected, 5, &b"hello"[..]).unwrap_err();

    assert!(matches!(error, fns_fs::FsError::ContentMismatch));
    assert_eq!(fs::read(blob_path).unwrap(), b"jello");
}

#[test]
fn opening_content_cache_cleans_only_stale_temp_files() {
    let state_dir = tempfile::tempdir().unwrap();
    let content = ContentCache::open(state_dir.path()).unwrap();
    drop(content);
    let temp_path = state_dir.path().join("tmp").join(".fns-tmp-stale");
    fs::write(&temp_path, b"stale").unwrap();
    fs::write(state_dir.path().join("tmp").join("keep.txt"), b"keep").unwrap();

    let _ = ContentCache::open(state_dir.path()).unwrap();

    assert!(!temp_path.exists());
    assert!(state_dir.path().join("tmp").join("keep.txt").exists());
}
