use std::path::PathBuf;
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
fn blocked_receiver_is_woken_by_a_later_overflow_gap() {
    let (ingress, receiver) = WatchIngress::bounded(1);
    let event = |name: &str| NormalizedWatchEvent {
        kind: NativeWatchKind::Modify,
        paths: vec![PathBuf::from(name)],
        rename_cookie: None,
        observed_at: Instant::now(),
    };
    ingress.try_send(event("first")).unwrap();

    let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let reader_barrier = std::sync::Arc::clone(&barrier);
    let reader = std::thread::spawn(move || {
        assert!(matches!(receiver.recv().unwrap(), WatchMessage::Event(_)));
        ready_sender.send(()).unwrap();
        assert!(matches!(receiver.recv().unwrap(), WatchMessage::Event(_)));
        ready_sender.send(()).unwrap();
        assert!(matches!(receiver.recv().unwrap(), WatchMessage::Event(_)));
        ready_sender.send(()).unwrap();
        reader_barrier.wait();
        match receiver.recv().unwrap() {
            WatchMessage::Gap(gap) => WatchMessage::Gap(gap),
            WatchMessage::Event(_) => receiver.recv().unwrap(),
        }
    });
    ready_receiver.recv().unwrap();
    ingress.try_send(event("second")).unwrap();
    ready_receiver.recv().unwrap();
    ingress.try_send(event("third")).unwrap();
    ready_receiver.recv().unwrap();
    barrier.wait();
    std::thread::sleep(Duration::from_millis(20));
    ingress.try_send(event("fourth")).unwrap();
    assert!(ingress.try_send(event("fifth")).is_err());

    assert!(matches!(reader.join().unwrap(), WatchMessage::Gap(_)));
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
