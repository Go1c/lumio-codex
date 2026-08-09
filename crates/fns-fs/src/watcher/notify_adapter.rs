use std::path::{Path, PathBuf};
use std::time::Instant;

use notify::{Event, EventKind};

use super::{NativeWatchKind, NormalizedWatchEvent, WatchGap};

const MAX_EVENT_PATHS: usize = 4_096;
const MAX_EVENT_PATH_BYTES: usize = 4_096;
const MAX_EVENT_BATCH_BYTES: usize = 1_048_576;

pub(crate) fn normalize_notify_event(
    root: &Path,
    event: Event,
) -> Result<Option<NormalizedWatchEvent>, WatchGap> {
    if event.need_rescan() {
        return Err(WatchGap::Backend);
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return Ok(None);
    }
    if event.paths.len() > MAX_EVENT_PATHS {
        return Err(WatchGap::Overflow);
    }
    let mut event_bytes = 0usize;
    for path in &event.paths {
        let path_bytes = path.to_str().ok_or(WatchGap::Ambiguous)?.len();
        if path_bytes > MAX_EVENT_PATH_BYTES {
            return Err(WatchGap::Overflow);
        }
        event_bytes = event_bytes
            .checked_add(path_bytes)
            .ok_or(WatchGap::Overflow)?;
        if event_bytes > MAX_EVENT_BATCH_BYTES {
            return Err(WatchGap::Overflow);
        }
    }
    let kind = super::platform::normalize_event(&event)?;
    let expected_paths = match kind {
        NativeWatchKind::RenameBoth => 2,
        NativeWatchKind::RenameFrom | NativeWatchKind::RenameTo => 1,
        NativeWatchKind::Create | NativeWatchKind::Modify | NativeWatchKind::Remove => {
            if event.paths.is_empty() {
                return Err(WatchGap::Ambiguous);
            }
            event.paths.len()
        }
    };
    if event.paths.len() != expected_paths {
        return Err(WatchGap::Ambiguous);
    }
    let paths = event
        .paths
        .iter()
        .map(|path| relative_path(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(NormalizedWatchEvent {
        kind,
        paths,
        rename_cookie: event.tracker().map(|value| value as u64),
        observed_at: Instant::now(),
    }))
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, WatchGap> {
    let relative = path.strip_prefix(root).map_err(|_| WatchGap::OutsideRoot)?;
    if relative.as_os_str().is_empty() || relative.to_str().is_none() {
        return Err(WatchGap::Ambiguous);
    }
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(WatchGap::OutsideRoot);
    }
    Ok(relative.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use notify::{
        Event, EventKind,
        event::{AccessKind, AccessMode, ModifyKind, RenameMode},
    };

    use super::{
        MAX_EVENT_BATCH_BYTES, MAX_EVENT_PATH_BYTES, MAX_EVENT_PATHS, normalize_notify_event,
    };
    use crate::watcher::{NativeWatchKind, WatchGap};

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        Event {
            kind,
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn maps_create_modify_remove_and_all_rename_shapes() {
        let root = PathBuf::from("/workspace");
        let cases = [
            (
                EventKind::Create(notify::event::CreateKind::File),
                &["/workspace/new.txt"][..],
                NativeWatchKind::Create,
            ),
            (
                EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
                &["/workspace/new.txt"][..],
                NativeWatchKind::Modify,
            ),
            (
                EventKind::Remove(notify::event::RemoveKind::File),
                &["/workspace/new.txt"][..],
                NativeWatchKind::Remove,
            ),
            (
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &["/workspace/old.txt"][..],
                NativeWatchKind::RenameFrom,
            ),
            (
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &["/workspace/new.txt"][..],
                NativeWatchKind::RenameTo,
            ),
            (
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &["/workspace/old.txt", "/workspace/new.txt"][..],
                NativeWatchKind::RenameBoth,
            ),
        ];
        for (kind, paths, expected) in cases {
            let normalized = normalize_notify_event(&root, event(kind, paths))
                .unwrap()
                .unwrap();
            assert_eq!(normalized.kind, expected);
        }
    }

    #[test]
    fn preserves_tracker_and_turns_rescan_or_ambiguous_events_into_gaps() {
        let root = PathBuf::from("/workspace");
        let mut renamed = event(
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            &["/workspace/old.txt"],
        );
        renamed.attrs.set_tracker(7);
        assert_eq!(
            normalize_notify_event(&root, renamed)
                .unwrap()
                .unwrap()
                .rename_cookie,
            Some(7)
        );

        let rescan = event(
            EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
            &["/workspace/file"],
        )
        .set_flag(notify::event::Flag::Rescan);
        assert!(matches!(
            normalize_notify_event(&root, rescan),
            Err(WatchGap::Backend)
        ));
        assert!(matches!(
            normalize_notify_event(&root, event(EventKind::Any, &[])),
            Err(WatchGap::Ambiguous)
        ));
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                    &["/workspace/old.txt"],
                ),
            ),
            Err(WatchGap::Ambiguous)
        ));
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
                    &["/outside/file"],
                ),
            ),
            Err(WatchGap::OutsideRoot)
        ));
    }

    #[test]
    fn ignores_non_mutating_access_without_hiding_unknown_events() {
        let root = PathBuf::from("/workspace");
        for kind in [
            AccessKind::Read,
            AccessKind::Open(AccessMode::Read),
            AccessKind::Close(AccessMode::Read),
            AccessKind::Close(AccessMode::Write),
        ] {
            assert!(
                normalize_notify_event(
                    &root,
                    event(EventKind::Access(kind), &["/workspace/file.txt"]),
                )
                .unwrap()
                .is_none()
            );
        }

        let access_requiring_rescan = event(
            EventKind::Access(AccessKind::Read),
            &["/workspace/file.txt"],
        )
        .set_flag(notify::event::Flag::Rescan);
        assert!(matches!(
            normalize_notify_event(&root, access_requiring_rescan),
            Err(WatchGap::Backend)
        ));

        assert!(matches!(
            normalize_notify_event(&root, event(EventKind::Any, &["/workspace/file.txt"])),
            Err(WatchGap::Ambiguous)
        ));
    }

    #[test]
    fn rejects_parent_escape_and_unbounded_event_paths() {
        let root = PathBuf::from("/workspace");
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
                    &["/workspace/../outside/file"],
                ),
            ),
            Err(WatchGap::OutsideRoot)
        ));

        let oversized_path = format!("/workspace/{}", "x".repeat(MAX_EVENT_PATH_BYTES + 1));
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Create(notify::event::CreateKind::File),
                    &[oversized_path.as_str()],
                ),
            ),
            Err(WatchGap::Overflow)
        ));

        let paths = (0..MAX_EVENT_PATHS + 1)
            .map(|index| format!("/workspace/file-{index}"))
            .collect::<Vec<_>>();
        let path_refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Create(notify::event::CreateKind::File),
                    &path_refs,
                ),
            ),
            Err(WatchGap::Overflow)
        ));
    }

    #[test]
    fn rejects_oversized_event_batches_before_copying_paths() {
        let root = PathBuf::from("/workspace");
        let paths = (0..MAX_EVENT_BATCH_BYTES / 1_000 + 1)
            .map(|index| format!("/workspace/{index:0>1000}"))
            .collect::<Vec<_>>();
        let path_refs = paths.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(path_refs.len() < MAX_EVENT_PATHS);
        assert!(matches!(
            normalize_notify_event(
                &root,
                event(
                    EventKind::Create(notify::event::CreateKind::File),
                    &path_refs,
                ),
            ),
            Err(WatchGap::Overflow)
        ));
    }
}
