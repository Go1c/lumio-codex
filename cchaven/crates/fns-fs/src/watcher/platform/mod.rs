use notify::{Event, EventKind, event::ModifyKind, event::RenameMode};

use super::{NativeWatchKind, WatchGap};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::normalize_event;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::normalize_event;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::normalize_event;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod generic;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) use generic::normalize_event;

fn normalize_kind(event: &Event) -> Result<NativeWatchKind, WatchGap> {
    match event.kind {
        EventKind::Create(_) if !event.paths.is_empty() => Ok(NativeWatchKind::Create),
        EventKind::Remove(_) if !event.paths.is_empty() => Ok(NativeWatchKind::Remove),
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Metadata(_))
        | EventKind::Modify(ModifyKind::Any)
            if !event.paths.is_empty() =>
        {
            Ok(NativeWatchKind::Modify)
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => Ok(NativeWatchKind::RenameFrom),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => Ok(NativeWatchKind::RenameTo),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => Ok(NativeWatchKind::RenameBoth),
        _ => Err(WatchGap::Ambiguous),
    }
}
