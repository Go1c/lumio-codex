use std::path::PathBuf;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use fns_fs::{
    NativeWatchKind, NormalizedWatchEvent, RootedWorkspace, WatchIngress, WatchMessage,
    start_platform_watcher,
};

#[test]
fn bounded_ingress_turns_overflow_into_one_rescan_gap() {
    let (ingress, receiver) = WatchIngress::bounded(2);
    let event = |name: &str| NormalizedWatchEvent {
        kind: NativeWatchKind::Modify,
        paths: vec![PathBuf::from(name)],
        rename_cookie: None,
        observed_at: Instant::now(),
    };

    assert!(ingress.try_send(event("a")).is_ok());
    assert!(ingress.try_send(event("b")).is_ok());
    assert!(ingress.try_send(event("c")).is_err());

    assert!(matches!(receiver.recv().unwrap(), WatchMessage::Gap(_)));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn zero_capacity_overflow_delivers_one_gap() {
    for attempt in 0..100 {
        let (ingress, receiver) = WatchIngress::bounded(0);
        let event = NormalizedWatchEvent {
            kind: NativeWatchKind::Modify,
            paths: vec![PathBuf::from(format!("overflow-{attempt}"))],
            rename_cookie: None,
            observed_at: Instant::now(),
        };

        assert!(ingress.try_send(event).is_err());
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            WatchMessage::Gap(fns_fs::WatchGap::Overflow)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(1)),
            Err(RecvTimeoutError::Timeout)
        ));
    }
}

#[test]
fn receive_timeout_distinguishes_messages_timeout_and_disconnection() {
    let event = || NormalizedWatchEvent {
        kind: NativeWatchKind::Modify,
        paths: vec![PathBuf::from("changed.txt")],
        rename_cookie: None,
        observed_at: Instant::now(),
    };

    let (event_ingress, event_receiver) = WatchIngress::bounded(1);
    event_ingress.try_send(event()).unwrap();
    assert!(matches!(
        event_receiver.recv_timeout(Duration::from_secs(1)),
        Ok(WatchMessage::Event(_))
    ));

    let (gap_ingress, gap_receiver) = WatchIngress::bounded(0);
    assert!(gap_ingress.try_send(event()).is_err());
    assert!(matches!(
        gap_receiver.recv_timeout(Duration::from_secs(1)),
        Ok(WatchMessage::Gap(fns_fs::WatchGap::Overflow))
    ));

    let (_idle_ingress, idle_receiver) = WatchIngress::bounded(1);
    assert!(matches!(
        idle_receiver.recv_timeout(Duration::from_millis(1)),
        Err(RecvTimeoutError::Timeout)
    ));

    let (disconnected_ingress, disconnected_receiver) = WatchIngress::bounded(1);
    drop(disconnected_ingress);
    assert!(matches!(
        disconnected_receiver.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn started_watcher_reports_a_real_file_change() {
    let area = tempfile::tempdir().unwrap();
    let root = RootedWorkspace::open(area.path()).unwrap();
    let (_watcher, receiver) = start_platform_watcher(&root, 32).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(area.path().join("changed.txt"), b"changed").unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found = false;
    while Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(WatchMessage::Event(event)) => {
                if event
                    .paths
                    .iter()
                    .any(|path| path == &PathBuf::from("changed.txt"))
                {
                    assert!(matches!(
                        event.kind,
                        NativeWatchKind::Create | NativeWatchKind::Modify
                    ));
                    found = true;
                    break;
                }
            }
            Ok(WatchMessage::Gap(_)) => break,
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    assert!(found, "watcher did not report the changed file");
}

#[cfg(target_os = "linux")]
#[test]
fn reading_a_watched_file_does_not_request_a_rescan() {
    let area = tempfile::tempdir().unwrap();
    let file = area.path().join("observed.txt");
    std::fs::write(&file, b"before").unwrap();
    let root = RootedWorkspace::open(area.path()).unwrap();
    let (_watcher, receiver) = start_platform_watcher(&root, 32).unwrap();
    std::thread::sleep(Duration::from_millis(20));

    assert_eq!(std::fs::read(&file).unwrap(), b"before");
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(250)),
        Err(RecvTimeoutError::Timeout)
    ));

    std::fs::write(&file, b"after").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found_mutation = false;
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(WatchMessage::Event(event))
                if event
                    .paths
                    .iter()
                    .any(|path| path == &PathBuf::from("observed.txt")) =>
            {
                found_mutation = true;
                break;
            }
            Ok(WatchMessage::Event(_)) => {}
            Ok(WatchMessage::Gap(gap)) => panic!("file mutation produced watcher gap: {gap:?}"),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("watcher disconnected"),
        }
    }
    assert!(found_mutation, "watcher did not report a real mutation");
}

#[cfg(unix)]
#[test]
fn watcher_reports_a_gap_when_the_bound_root_path_is_replaced() {
    use std::os::unix::fs::symlink;

    let area = tempfile::tempdir().unwrap();
    let root_path = area.path().join("root");
    let moved_path = area.path().join("moved");
    let outside_path = area.path().join("outside");
    std::fs::create_dir(&root_path).unwrap();
    std::fs::create_dir(&outside_path).unwrap();
    let root = RootedWorkspace::open(&root_path).unwrap();
    let (_watcher, receiver) = start_platform_watcher(&root, 32).unwrap();

    std::thread::sleep(Duration::from_millis(20));
    std::fs::rename(&root_path, &moved_path).unwrap();
    symlink(&outside_path, &root_path).unwrap();
    std::fs::write(moved_path.join("changed.txt"), b"changed").unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found_gap = false;
    while Instant::now() < deadline {
        match receiver.try_recv() {
            Ok(WatchMessage::Gap(fns_fs::WatchGap::OutsideRoot)) => {
                found_gap = true;
                break;
            }
            Ok(WatchMessage::Gap(_)) => {}
            Ok(WatchMessage::Event(_)) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }

    assert!(found_gap, "watcher did not report root replacement");
}
