use notify::Event;

use super::{NativeWatchKind, WatchGap, normalize_kind};

pub(crate) fn normalize_event(event: &Event) -> Result<NativeWatchKind, WatchGap> {
    normalize_kind(event)
}
