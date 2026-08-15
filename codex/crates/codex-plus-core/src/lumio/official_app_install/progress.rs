use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{InstallPhase, InstallStatus};

static STATUS: OnceLock<Mutex<InstallStatus>> = OnceLock::new();
static CANCEL: AtomicBool = AtomicBool::new(false);
static JOB_RUNNING: AtomicBool = AtomicBool::new(false);

fn idle_status() -> InstallStatus {
    InstallStatus {
        phase: InstallPhase::Idle,
        stage: None,
        bytes_downloaded: None,
        bytes_total: None,
        error_code: None,
        installed_path: None,
    }
}

fn status_mutex() -> &'static Mutex<InstallStatus> {
    STATUS.get_or_init(|| Mutex::new(idle_status()))
}

pub fn current_status() -> InstallStatus {
    status_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn cancel_flag() -> &'static AtomicBool {
    &CANCEL
}

pub fn phase_kebab(phase: InstallPhase) -> &'static str {
    match phase {
        InstallPhase::Idle => "idle",
        InstallPhase::Planning => "planning",
        InstallPhase::Downloading => "downloading",
        InstallPhase::Verifying => "verifying",
        InstallPhase::Installing => "installing",
        InstallPhase::Detecting => "detecting",
        InstallPhase::Succeeded => "succeeded",
        InstallPhase::Failed => "failed",
        InstallPhase::Cancelled => "cancelled",
    }
}

pub(crate) fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

pub(crate) fn prepare_new_job() {
    CANCEL.store(false, Ordering::SeqCst);
}

pub(crate) fn try_begin_job() -> bool {
    JOB_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub(crate) fn end_job() {
    JOB_RUNNING.store(false, Ordering::SeqCst);
}

pub(crate) fn update_status(edit: impl FnOnce(&mut InstallStatus)) {
    let mut status = status_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    edit(&mut status);
}

pub(crate) fn set_planning() {
    update_status(|status| {
        status.phase = InstallPhase::Planning;
        status.stage = Some("plan");
        status.bytes_downloaded = None;
        status.bytes_total = None;
        status.error_code = None;
        status.installed_path = None;
    });
}

pub(crate) fn set_phase(phase: InstallPhase, stage: Option<&'static str>) {
    update_status(|status| {
        status.phase = phase;
        status.stage = stage;
    });
}

pub(crate) fn set_succeeded(path: std::path::PathBuf) {
    update_status(|status| {
        status.phase = InstallPhase::Succeeded;
        status.stage = None;
        status.error_code = None;
        status.installed_path = Some(path);
    });
}

pub(crate) fn set_failed(error_code: &str) {
    update_status(|status| {
        status.phase = InstallPhase::Failed;
        status.error_code = Some(error_code.to_string());
    });
}

pub(crate) fn set_cancelled() {
    update_status(|status| {
        status.phase = InstallPhase::Cancelled;
        status.stage = None;
        status.error_code = None;
    });
}

#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) struct StatusTestGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

#[cfg(test)]
pub(crate) fn reset_status_for_tests() -> StatusTestGuard {
    let guard = TEST_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    CANCEL.store(false, Ordering::SeqCst);
    JOB_RUNNING.store(false, Ordering::SeqCst);
    if let Ok(mut status) = status_mutex().lock() {
        *status = idle_status();
    }
    StatusTestGuard(guard)
}

#[cfg(test)]
mod tests {
    use super::super::InstallPhase;
    use super::current_status;

    #[test]
    fn current_status_starts_idle() {
        let _guard = super::reset_status_for_tests();
        let status = current_status();
        assert_eq!(status.phase, InstallPhase::Idle);
        assert!(status.stage.is_none());
        assert!(status.bytes_downloaded.is_none());
        assert!(status.bytes_total.is_none());
        assert!(status.error_code.is_none());
        assert!(status.installed_path.is_none());
    }

    #[test]
    fn request_cancel_sets_the_shared_flag() {
        use std::sync::atomic::Ordering;

        let _guard = super::reset_status_for_tests();
        assert!(!super::cancel_flag().load(Ordering::SeqCst));
        super::request_cancel();
        assert!(super::cancel_flag().load(Ordering::SeqCst));
    }
}
