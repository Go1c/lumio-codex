use std::fs;

use fns_fs::{RootedWorkspace, SyncRuleConfig, SyncRules};
use fns_protocol::{WorkspaceEntryKind, WorkspacePath};

fn path(value: &str) -> WorkspacePath {
    WorkspacePath::parse(value).expect("test path is valid")
}

#[cfg(unix)]
#[test]
fn rejects_symlink_root_and_outside_escape_without_following_scan_links() {
    use std::os::unix::fs::symlink;

    let area = tempfile::tempdir().unwrap();
    let root = area.path().join("root");
    let outside = area.path().join("outside");
    fs::create_dir_all(root.join("real")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret"), b"secret").unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    symlink("real", root.join("inside")).unwrap();
    symlink(&root, area.path().join("root-link")).unwrap();

    assert!(RootedWorkspace::open(&area.path().join("root-link")).is_err());
    let rooted = RootedWorkspace::open(&root).unwrap();
    assert!(rooted.inspect(&path("escape/secret")).is_err());
    assert!(rooted.inspect(&path("inside")).unwrap().is_some());

    let scan = rooted
        .scan(&SyncRules::compile(SyncRuleConfig::default()).unwrap())
        .unwrap();
    assert!(
        scan.entries
            .iter()
            .any(|entry| entry.path.as_str() == "inside")
    );
    assert!(
        !scan
            .entries
            .iter()
            .any(|entry| entry.path.as_str().starts_with("inside/"))
    );
    assert!(
        !scan
            .entries
            .iter()
            .any(|entry| entry.path.as_str().starts_with("escape/"))
    );
    assert!(
        !scan
            .entries
            .iter()
            .any(|entry| entry.path.as_str() == "escape")
    );
}

#[cfg(unix)]
#[test]
fn rejects_absolute_and_non_utf8_symlink_targets() {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    let area = tempfile::tempdir().unwrap();
    let root = area.path().join("root");
    let outside = area.path().join("outside");
    fs::create_dir_all(root.join("real")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    symlink(root.join("real"), root.join("absolute")).unwrap();
    let invalid_target = std::ffi::OsString::from_vec(vec![b'r', b'e', b'a', b'l', 0xff]);
    symlink(&invalid_target, root.join("invalid-utf8")).unwrap();

    let rooted = RootedWorkspace::open(&root).unwrap();

    assert!(matches!(
        rooted.inspect(&path("absolute")),
        Err(fns_fs::FsError::PathEscape)
    ));
    assert!(matches!(
        rooted.inspect(&path("invalid-utf8")),
        Err(fns_fs::FsError::PathEscape)
    ));
}

#[test]
fn scan_sorts_workspace_paths_by_utf8_bytes() {
    let area = tempfile::tempdir().unwrap();
    fs::create_dir_all(area.path().join("dir")).unwrap();
    fs::write(area.path().join("z"), b"z").unwrap();
    fs::write(area.path().join("a"), b"a").unwrap();
    fs::write(area.path().join("dir").join("café"), b"c").unwrap();

    let rooted = RootedWorkspace::open(area.path()).unwrap();
    let scan = rooted
        .scan(&SyncRules::compile(SyncRuleConfig::default()).unwrap())
        .unwrap();
    let paths = scan
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["a", "dir", "dir/café", "z"]);
}

#[test]
fn missing_leaf_is_valid_for_create_planning() {
    let area = tempfile::tempdir().unwrap();
    let rooted = RootedWorkspace::open(area.path()).unwrap();
    assert!(rooted.inspect(&path("new/file.txt")).unwrap().is_none());
}

#[test]
fn inspect_reports_entry_kind_and_metadata() {
    let area = tempfile::tempdir().unwrap();
    fs::create_dir(area.path().join("dir")).unwrap();
    fs::write(area.path().join("file"), b"file").unwrap();
    let rooted = RootedWorkspace::open(area.path()).unwrap();

    assert_eq!(
        rooted.inspect(&path("dir")).unwrap().unwrap().kind,
        WorkspaceEntryKind::Directory
    );
    assert_eq!(
        rooted.inspect(&path("file")).unwrap().unwrap().kind,
        WorkspaceEntryKind::File
    );
}

#[cfg(unix)]
#[test]
fn scan_reports_non_utf8_native_name() {
    use std::os::unix::ffi::OsStringExt;

    let area = tempfile::tempdir().unwrap();
    let invalid = std::ffi::OsString::from_vec(vec![b'b', b'a', b'd', 0xff]);
    if let Err(error) = fs::write(area.path().join(invalid), b"invalid") {
        assert!(error.raw_os_error().is_some());
        return;
    }
    let rooted = RootedWorkspace::open(area.path()).unwrap();
    let scan = rooted
        .scan(&SyncRules::compile(SyncRuleConfig::default()).unwrap())
        .unwrap();
    assert_eq!(scan.issues.len(), 1);
    assert!(scan.entries.is_empty());
}

#[cfg(unix)]
#[test]
fn normalization_alias_is_collision_never_existing() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let area = tempfile::tempdir().unwrap();
    let decomposed = OsString::from_vec("cafe\u{301}".as_bytes().to_vec());
    fs::write(area.path().join(decomposed), b"nfd").unwrap();
    let rooted = RootedWorkspace::open(area.path()).unwrap();
    let result = rooted.inspect(&path("café")).unwrap_err();
    assert!(matches!(result, fns_fs::FsError::PathCollision { .. }));
}
