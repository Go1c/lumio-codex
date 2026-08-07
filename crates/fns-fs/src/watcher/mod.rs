mod notify_adapter;
mod platform;

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use notify::{RecursiveMode, Watcher};

use crate::{FsError, RootedWorkspace};

pub const WATCH_QUEUE_CAPACITY: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeWatchKind {
    Create,
    Modify,
    Remove,
    RenameFrom,
    RenameTo,
    RenameBoth,
}

#[derive(Clone, Debug)]
pub struct NormalizedWatchEvent {
    pub kind: NativeWatchKind,
    pub paths: Vec<PathBuf>,
    pub rename_cookie: Option<u64>,
    pub observed_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchGap {
    Backend,
    Overflow,
    Ambiguous,
    OutsideRoot,
}

#[derive(Clone, Debug)]
pub enum WatchMessage {
    Event(NormalizedWatchEvent),
    Gap(WatchGap),
}

struct GapState {
    code: AtomicU8,
    wake: Sender<()>,
    gate: Mutex<()>,
}

impl GapState {
    fn new(wake: Sender<()>) -> Self {
        Self {
            code: AtomicU8::new(0),
            wake,
            gate: Mutex::new(()),
        }
    }

    fn set(&self, gap: WatchGap) {
        let _guard = self.gate.lock().expect("watch gap gate is not poisoned");
        self.set_locked(gap);
    }

    fn set_locked(&self, gap: WatchGap) {
        let code = gap_code(gap);
        if self
            .code
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self.wake.try_send(());
        }
    }

    fn take(&self) -> Option<WatchGap> {
        WatchGap::from_code(self.code.swap(0, Ordering::AcqRel))
    }

    fn is_set(&self) -> bool {
        self.code.load(Ordering::Acquire) != 0
    }
}

#[derive(Clone)]
pub struct WatchIngress {
    sender: Sender<WatchMessage>,
    gap: Arc<GapState>,
}

pub struct WatchReceiver {
    receiver: Receiver<WatchMessage>,
    gap: Arc<GapState>,
    gap_wake: Receiver<()>,
}

impl WatchIngress {
    pub fn bounded(capacity: usize) -> (Self, WatchReceiver) {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        let (gap_wake, gap_wake_receiver) = crossbeam_channel::bounded(1);
        let gap = Arc::new(GapState::new(gap_wake));
        (
            Self {
                sender,
                gap: Arc::clone(&gap),
            },
            WatchReceiver {
                receiver,
                gap,
                gap_wake: gap_wake_receiver,
            },
        )
    }

    pub fn try_send(&self, event: NormalizedWatchEvent) -> Result<(), FsError> {
        let _guard = self
            .gap
            .gate
            .lock()
            .expect("watch gap gate is not poisoned");
        if self.gap.is_set() {
            return Err(FsError::QueueDisconnected);
        }
        match self.sender.try_send(WatchMessage::Event(event)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.gap.set_locked(WatchGap::Overflow);
                Err(FsError::QueueDisconnected)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.gap.set_locked(WatchGap::Backend);
                Err(FsError::QueueDisconnected)
            }
        }
    }

    fn mark_gap(&self, gap: WatchGap) {
        self.gap.set(gap);
    }
}

impl WatchReceiver {
    pub fn recv(&self) -> Result<WatchMessage, FsError> {
        loop {
            if let Some(gap) = self.take_gap() {
                return Ok(WatchMessage::Gap(gap));
            }
            crossbeam_channel::select! {
                recv(self.receiver) -> message => {
                    let message = message.map_err(|_| FsError::QueueDisconnected)?;
                    if let Some(gap) = self.take_gap() {
                        return Ok(WatchMessage::Gap(gap));
                    }
                    return Ok(message);
                }
                recv(self.gap_wake) -> _ => {}
            }
        }
    }

    pub fn try_recv(&self) -> Result<WatchMessage, FsError> {
        if let Some(gap) = self.take_gap() {
            return Ok(WatchMessage::Gap(gap));
        }
        let message = self.receiver.try_recv().map_err(|error| match error {
            TryRecvError::Empty => FsError::QueueDisconnected,
            TryRecvError::Disconnected => FsError::QueueDisconnected,
        })?;
        if let Some(gap) = self.take_gap() {
            return Ok(WatchMessage::Gap(gap));
        }
        Ok(message)
    }

    fn take_gap(&self) -> Option<WatchGap> {
        let _guard = self
            .gap
            .gate
            .lock()
            .expect("watch gap gate is not poisoned");
        if !self.gap.is_set() {
            return None;
        }
        while self.receiver.try_recv().is_ok() {}
        let gap = self.gap.take()?;
        let _ = self.gap_wake.try_recv();
        Some(gap)
    }
}

fn gap_code(gap: WatchGap) -> u8 {
    match gap {
        WatchGap::Backend => 1,
        WatchGap::Overflow => 2,
        WatchGap::Ambiguous => 3,
        WatchGap::OutsideRoot => 4,
    }
}

impl WatchGap {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Backend),
            2 => Some(Self::Overflow),
            3 => Some(Self::Ambiguous),
            4 => Some(Self::OutsideRoot),
            _ => None,
        }
    }
}

pub struct PlatformWatcher {
    _watcher: notify::RecommendedWatcher,
}

pub fn start_platform_watcher(
    root: &RootedWorkspace,
    capacity: usize,
) -> Result<(PlatformWatcher, WatchReceiver), FsError> {
    let (ingress, receiver) = WatchIngress::bounded(capacity);
    let root_path = root.canonical_root().to_path_buf();
    let callback_root = root.clone_for_watcher()?;
    let callback_ingress = ingress.clone();
    let watcher = notify::recommended_watcher(move |result| match result {
        Ok(_event) if !callback_root.bound_path_is_current() => {
            callback_ingress.mark_gap(WatchGap::OutsideRoot);
        }
        Ok(event) => match notify_adapter::normalize_notify_event(&root_path, event) {
            Ok(event) => {
                if callback_ingress.try_send(event).is_err() {
                    callback_ingress.mark_gap(WatchGap::Overflow);
                }
            }
            Err(gap) => callback_ingress.mark_gap(gap),
        },
        Err(_) => callback_ingress.mark_gap(WatchGap::Backend),
    })
    .map_err(|_| FsError::Io {
        operation: "create filesystem watcher",
    })?;
    let mut watcher = PlatformWatcher { _watcher: watcher };
    watcher
        ._watcher
        .watch(root.canonical_root(), RecursiveMode::Recursive)
        .map_err(|_| FsError::Io {
            operation: "start filesystem watcher",
        })?;
    Ok((watcher, receiver))
}
