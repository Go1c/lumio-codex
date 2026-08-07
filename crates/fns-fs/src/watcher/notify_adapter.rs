use std::path::{Path, PathBuf};
use std::time::Instant;

use notify::Event;

use super::{NativeWatchKind, NormalizedWatchEvent, WatchGap};

pub(crate) fn normalize_notify_event(
    root: &Path,
    event: Event,
) -> Result<NormalizedWatchEvent, WatchGap> {
    if event.need_rescan() {
        return Err(WatchGap::Backend);
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
    Ok(NormalizedWatchEvent {
        kind,
        paths,
        rename_cookie: event.tracker().map(|value| value as u64),
        observed_at: Instant::now(),
    })
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, WatchGap> {
    let relative = path.strip_prefix(root).map_err(|_| WatchGap::OutsideRoot)?;
    if relative.as_os_str().is_empty() || relative.to_str().is_none() {
        return Err(WatchGap::Ambiguous);
    }
    Ok(relative.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use notify::{Event, EventKind, event::ModifyKind, event::RenameMode};

    use super::normalize_notify_event;
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
            let normalized = normalize_notify_event(&root, event(kind, paths)).unwrap();
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
}
