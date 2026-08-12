use crate::{io_error, HarnessError, Result};
use rustix::process::{Pid, Resource, Signal};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

const MAX_INTERPRETER_DEPTH: usize = 8;
const MAX_SHEBANG_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub struct ProcessSpec {
    pub label: String,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    io: ProcessIo,
}

#[derive(Clone, Copy, Debug)]
enum ProcessIo {
    Quiet,
    Control,
    Output,
}

pub struct PinnedExecutable {
    _pin_directory: tempfile::TempDir,
    components: Vec<PinnedComponent>,
    interpreter_sources: Vec<ValidatedSource>,
    source_device: u64,
    source_inode: u64,
}

struct PinnedComponent {
    file: File,
    state: FileState,
    kind: ExecutableKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ExecutableKind {
    Native,
    Script(Shebang),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Shebang {
    interpreter: PathBuf,
    argument: Option<OsString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileState {
    identity: ObjectIdentity,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    digest: [u8; 32],
}

#[derive(Debug)]
struct AncestorState {
    path: PathBuf,
    directory: File,
    identity: ObjectIdentity,
}

#[derive(Debug)]
struct ValidatedSource {
    path: PathBuf,
    ancestors: Vec<AncestorState>,
    state: FileState,
}

impl std::fmt::Debug for PinnedExecutable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedExecutable")
            .field("component_count", &self.components.len())
            .field("interpreter_count", &self.interpreter_sources.len())
            .field("source_device", &self.source_device)
            .field("source_inode", &self.source_inode)
            .finish_non_exhaustive()
    }
}

impl PinnedExecutable {
    pub fn pin(path: &Path) -> Result<Self> {
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| io_error(path, error))?;
        let source_metadata = source.metadata().map_err(|error| io_error(path, error))?;
        if !source_metadata.is_file() || source_metadata.permissions().mode() & 0o111 == 0 {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer must be an executable file",
            ));
        }
        let pin_directory = tempfile::tempdir().map_err(|error| io_error("observer pin", error))?;
        fs::set_permissions(pin_directory.path(), fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error(pin_directory.path(), error))?;
        let observer_path = pin_directory.path().join("observer");
        let observer = snapshot_file(&mut source, &observer_path)?;
        let observer_kind = parse_executable_kind(observer.as_raw_fd()).map_err(|_| {
            HarnessError::InvalidConfiguration(
                "effect observer has a malformed or unsupported shebang",
            )
        })?;
        // macOS /dev/fd opens share the source file offset. Python opens its script more than once,
        // so executing a script through /dev/fd can consume it without running any code. Retain the
        // private 0500 snapshot and validate its path against the pinned descriptor in the child.
        let keep_observer_path = cfg!(target_os = "macos");
        if !keep_observer_path {
            fs::remove_file(&observer_path).map_err(|error| io_error(&observer_path, error))?;
        }
        let observer_state =
            file_state(&observer).map_err(|error| io_error("observer pin", error))?;
        set_cloexec(observer.as_raw_fd(), true).map_err(|error| io_error("observer pin", error))?;

        let mut components = vec![PinnedComponent {
            file: observer,
            state: observer_state,
            kind: observer_kind,
        }];
        let mut interpreter_sources = Vec::new();
        let mut seen = vec![(source_metadata.dev(), source_metadata.ino())];

        while let ExecutableKind::Script(shebang) =
            &components.last().expect("observer component exists").kind
        {
            if interpreter_sources.len() == MAX_INTERPRETER_DEPTH {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer interpreter chain is too deep",
                ));
            }
            let (mut interpreter, validated) = open_validated_source(&shebang.interpreter)?;
            let identity = (
                validated.state.identity.device,
                validated.state.identity.inode,
            );
            if seen.contains(&identity) {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer interpreter chain contains a cycle",
                ));
            }
            seen.push(identity);
            let kind = parse_executable_kind(interpreter.as_raw_fd()).map_err(|_| {
                HarnessError::InvalidConfiguration(
                    "effect observer interpreter has a malformed shebang",
                )
            })?;
            #[cfg(target_os = "macos")]
            let interpreter_is_root_owned = validated.state.identity.uid == 0;
            interpreter_sources.push(validated);
            match kind {
                ExecutableKind::Native => {
                    #[cfg(target_os = "macos")]
                    let interpreter = if interpreter_is_root_owned {
                        interpreter
                    } else {
                        let pin_path = pin_directory
                            .path()
                            .join(format!("interpreter-{}", interpreter_sources.len()));
                        snapshot_file(&mut interpreter, &pin_path)?
                    };
                    let state = file_state(&interpreter)
                        .map_err(|error| io_error("observer interpreter", error))?;
                    set_cloexec(interpreter.as_raw_fd(), true)
                        .map_err(|error| io_error("observer interpreter", error))?;
                    components.push(PinnedComponent {
                        file: interpreter,
                        state,
                        kind: ExecutableKind::Native,
                    });
                }
                ExecutableKind::Script(script) => {
                    let pin_path = pin_directory
                        .path()
                        .join(format!("interpreter-{}", interpreter_sources.len()));
                    let pinned = snapshot_file(&mut interpreter, &pin_path)?;
                    if !cfg!(target_os = "macos") {
                        fs::remove_file(&pin_path).map_err(|error| io_error(&pin_path, error))?;
                    }
                    let state = file_state(&pinned)
                        .map_err(|error| io_error("observer interpreter pin", error))?;
                    set_cloexec(pinned.as_raw_fd(), true)
                        .map_err(|error| io_error("observer interpreter pin", error))?;
                    components.push(PinnedComponent {
                        file: pinned,
                        state,
                        kind: ExecutableKind::Script(script),
                    });
                }
            }
        }

        Ok(Self {
            _pin_directory: pin_directory,
            components,
            interpreter_sources,
            source_device: source_metadata.dev(),
            source_inode: source_metadata.ino(),
        })
    }

    pub fn source_is_same_file(&self, path: &Path) -> Result<bool> {
        let metadata = fs::metadata(path).map_err(|error| io_error(path, error))?;
        Ok(self.source_device == metadata.dev() && self.source_inode == metadata.ino())
    }

    pub fn raw_fd(&self) -> RawFd {
        self.components[0].file.as_raw_fd()
    }

    fn descriptors(&self) -> Vec<RawFd> {
        self.components
            .iter()
            .map(|component| component.file.as_raw_fd())
            .collect()
    }

    fn validate_execution_plan(&self) -> Result<()> {
        for source in &self.interpreter_sources {
            source.validate()?;
        }
        for component in &self.components {
            let current = file_state(&component.file)
                .map_err(|error| io_error("observer execution plan", error))?;
            if current != component.state {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer pinned execution object changed",
                ));
            }
            let current_kind = parse_executable_kind(component.file.as_raw_fd()).map_err(|_| {
                HarnessError::InvalidConfiguration(
                    "effect observer pinned execution object became malformed",
                )
            })?;
            if current_kind != component.kind {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer pinned execution plan changed",
                ));
            }
        }
        Ok(())
    }
}

impl ValidatedSource {
    fn validate(&self) -> Result<()> {
        validate_ancestors(&self.ancestors)?;
        let (file, current) = open_validated_source(&self.path)?;
        if current.state != self.state || current.ancestors != self.ancestors {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer interpreter path changed after configuration",
            ));
        }
        let descriptor_state = file_state(&file).map_err(|error| io_error(&self.path, error))?;
        if descriptor_state != self.state {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer interpreter changed after configuration",
            ));
        }
        Ok(())
    }
}

impl PartialEq for AncestorState {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.identity == other.identity
    }
}

impl Eq for AncestorState {}

fn snapshot_file(source: &mut File, destination: &Path) -> Result<File> {
    source
        .seek(SeekFrom::Start(0))
        .map_err(|error| io_error(destination, error))?;
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|error| io_error(destination, error))?;
    std::io::copy(source, &mut writer).map_err(|error| io_error(destination, error))?;
    writer
        .flush()
        .map_err(|error| io_error(destination, error))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o500))
        .map_err(|error| io_error(destination, error))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(destination)
        .map_err(|error| io_error(destination, error))?;
    Ok(file)
}

fn open_validated_source(path: &Path) -> Result<(File, ValidatedSource)> {
    let ancestors = capture_ancestors(path)?;
    let canonical = path.canonicalize().map_err(|error| io_error(path, error))?;
    if canonical != path {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter path must be canonical and symlink-free",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    let metadata = file.metadata().map_err(|error| io_error(path, error))?;
    validate_interpreter_metadata(&metadata)?;
    let path_metadata = fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    if object_identity(&metadata) != object_identity(&path_metadata) {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter path changed during validation",
        ));
    }
    let state = file_state(&file).map_err(|error| io_error(path, error))?;
    validate_ancestors(&ancestors)?;
    Ok((
        file,
        ValidatedSource {
            path: path.to_path_buf(),
            ancestors,
            state,
        },
    ))
}

fn validate_interpreter_metadata(metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_file() {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter must be a regular file",
        ));
    }
    let mode = metadata.mode();
    if mode & 0o111 == 0 {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter must be executable",
        ));
    }
    if mode & 0o022 != 0 {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter must not be group- or world-writable",
        ));
    }
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != 0 && metadata.uid() != effective_uid {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer interpreter must be owned by root or the current user",
        ));
    }
    Ok(())
}

fn capture_ancestors(path: &Path) -> Result<Vec<AncestorState>> {
    if !path.is_absolute() {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer shebang interpreter must be absolute",
        ));
    }
    let mut current = PathBuf::new();
    let mut ancestors = Vec::new();
    let components: Vec<_> = path.components().collect();
    if components.len() < 2 {
        return Err(HarnessError::InvalidConfiguration(
            "effect observer shebang interpreter path is invalid",
        ));
    }
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::RootDir => current.push(Path::new("/")),
            Component::Normal(value) => current.push(value),
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(HarnessError::InvalidConfiguration(
                    "effect observer shebang interpreter path must be normalized",
                ));
            }
        }
        if index + 1 == components.len() {
            break;
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| io_error(&current, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer interpreter ancestors must be real directories",
            ));
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&current)
            .map_err(|error| io_error(&current, error))?;
        let descriptor_metadata = directory
            .metadata()
            .map_err(|error| io_error(&current, error))?;
        if object_identity(&metadata) != object_identity(&descriptor_metadata) {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer interpreter ancestor changed during validation",
            ));
        }
        ancestors.push(AncestorState {
            path: current.clone(),
            directory,
            identity: object_identity(&metadata),
        });
    }
    Ok(ancestors)
}

fn validate_ancestors(ancestors: &[AncestorState]) -> Result<()> {
    for expected in ancestors {
        let metadata = fs::symlink_metadata(&expected.path)
            .map_err(|error| io_error(&expected.path, error))?;
        let descriptor_metadata = expected
            .directory
            .metadata()
            .map_err(|error| io_error(&expected.path, error))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || object_identity(&metadata) != expected.identity
            || object_identity(&descriptor_metadata) != expected.identity
        {
            return Err(HarnessError::InvalidConfiguration(
                "effect observer interpreter ancestor changed after configuration",
            ));
        }
    }
    Ok(())
}

fn object_identity(metadata: &fs::Metadata) -> ObjectIdentity {
    ObjectIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        mode: metadata.mode(),
    }
}

fn file_state(file: &File) -> std::io::Result<FileState> {
    let metadata = file.metadata()?;
    Ok(FileState {
        identity: object_identity(&metadata),
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
        digest: descriptor_digest(file)?,
    })
}

fn descriptor_digest(file: &File) -> std::io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut offset = 0_u64;
    loop {
        let length = file.read_at(&mut buffer, offset)?;
        if length == 0 {
            break;
        }
        hasher.update(&buffer[..length]);
        offset += u64::try_from(length).expect("buffer length fits in u64");
    }
    Ok(hasher.finalize().into())
}

fn set_cloexec(descriptor: RawFd, enabled: bool) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let updated = if enabled {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, updated) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl ProcessSpec {
    pub fn quiet<I, S>(label: impl Into<String>, program: impl AsRef<Path>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            label: label.into(),
            program: program.as_ref().to_path_buf(),
            args: args.into_iter().map(Into::into).collect(),
            io: ProcessIo::Quiet,
        }
    }

    pub fn control<I, S>(label: impl Into<String>, program: impl AsRef<Path>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            label: label.into(),
            program: program.as_ref().to_path_buf(),
            args: args.into_iter().map(Into::into).collect(),
            io: ProcessIo::Control,
        }
    }

    pub fn output<I, S>(label: impl Into<String>, program: impl AsRef<Path>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            label: label.into(),
            program: program.as_ref().to_path_buf(),
            args: args.into_iter().map(Into::into).collect(),
            io: ProcessIo::Output,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Termination {
    Exited,
    Terminated,
    Killed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupCleanup {
    pub termination: Termination,
    pub term_attempted: bool,
    pub kill_attempted: bool,
    pub descendants_present: bool,
    pub group_empty: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessOutcome {
    pub pid: Pid,
    pub pgid: Pid,
    pub status: ExitStatus,
    pub termination: Termination,
    pub group_cleanup: GroupCleanup,
}

#[derive(Clone, Debug)]
pub struct CleanupFailure {
    pub pid: Pid,
    pub pgid: Pid,
    pub leader_termination: Option<Termination>,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<i32>,
    pub term_attempted: bool,
    pub kill_attempted: bool,
    pub descendants_present: bool,
    pub leader_reaped: bool,
    pub group_empty: bool,
    pub timed_out: bool,
    pub detail: String,
}

impl std::fmt::Display for CleanupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "cleanup failed for PID {} PGID {}: {} (TERM={}, KILL={}, reaped={}, descendants={}, group_empty={}, timed_out={})",
            self.pid.as_raw_pid(),
            self.pgid.as_raw_pid(),
            self.detail,
            self.term_attempted,
            self.kill_attempted,
            self.leader_reaped,
            self.descendants_present,
            self.group_empty,
            self.timed_out,
        )
    }
}

impl std::error::Error for CleanupFailure {}

pub struct OwnedChild {
    label: String,
    child: Child,
    pid: Pid,
    pgid: Pid,
    reaped: Option<ExitStatus>,
    reap_count: usize,
    group_clean: bool,
}

impl std::fmt::Debug for OwnedChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedChild")
            .field("label", &self.label)
            .field("pid", &self.pid)
            .field("pgid", &self.pgid)
            .field("reaped", &self.reaped)
            .field("group_clean", &self.group_clean)
            .finish()
    }
}

impl OwnedChild {
    pub fn spawn(spec: ProcessSpec) -> Result<Self> {
        Self::spawn_inner(spec, None)
    }

    pub fn spawn_pinned(spec: ProcessSpec, executable: &PinnedExecutable) -> Result<Self> {
        executable.validate_execution_plan()?;
        Self::spawn_inner(spec, Some(executable))
    }

    fn spawn_inner(spec: ProcessSpec, pinned: Option<&PinnedExecutable>) -> Result<Self> {
        let clean_exec = clean_exec_binary()?;
        let mut command = Command::new(&clean_exec);
        if let Some(pinned) = pinned {
            let descriptors = pinned.descriptors();
            command
                .arg("__exec-pinned")
                .arg(descriptors.len().to_string())
                .args(descriptors.iter().map(ToString::to_string))
                .arg("--")
                .args(&spec.args);
            unsafe {
                command.pre_exec(move || {
                    for descriptor in &descriptors {
                        if libc::lseek(*descriptor, 0, libc::SEEK_SET) == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        set_cloexec(*descriptor, false)?;
                    }
                    Ok(())
                });
            }
        } else {
            command
                .arg("__exec-clean")
                .arg("--")
                .arg(&spec.program)
                .args(&spec.args);
        }
        command
            .env_clear()
            .env("LANG", "C")
            .env("PATH", "/usr/bin:/bin")
            .kill_on_drop(true);
        match spec.io {
            ProcessIo::Quiet => {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
            }
            ProcessIo::Control => {
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit());
            }
            ProcessIo::Output => {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit());
            }
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command
            .spawn()
            .map_err(|error| io_error(&clean_exec, error))?;
        let raw_pid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(Pid::from_raw)
            .ok_or(HarnessError::Process("spawned child has no valid PID"))?;
        Ok(Self {
            label: spec.label,
            child,
            pid: raw_pid,
            pgid: raw_pid,
            reaped: None,
            reap_count: 0,
            group_clean: false,
        })
    }

    pub fn pid(&self) -> Pid {
        self.pid
    }

    pub fn pgid(&self) -> Pid {
        self.pgid
    }

    pub fn reap_count(&self) -> usize {
        self.reap_count
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub async fn wait(&mut self) -> Result<ExitStatus> {
        if let Some(status) = self.reaped {
            return Ok(status);
        }
        let status = self
            .child
            .wait()
            .await
            .map_err(|error| io_error(&self.label, error))?;
        self.reaped = Some(status);
        self.reap_count += 1;
        Ok(status)
    }

    fn try_reap(&mut self) -> Result<Option<ExitStatus>> {
        if let Some(status) = self.reaped {
            return Ok(Some(status));
        }
        let status = self
            .child
            .try_wait()
            .map_err(|error| io_error(&self.label, error))?;
        if let Some(status) = status {
            self.reaped = Some(status);
            self.reap_count += 1;
        }
        Ok(status)
    }

    pub async fn wait_or_cancel(
        &mut self,
        cancellation: &CancellationToken,
        term_grace: Duration,
        kill_timeout: Duration,
    ) -> Result<ProcessOutcome> {
        if cancellation.is_cancelled() {
            return self.terminate_and_reap(term_grace, kill_timeout).await;
        }
        tokio::select! {
            result = self.wait() => {
                let status = result?;
                self.complete_reaped(status, Termination::Exited, term_grace, kill_timeout).await
            },
            () = cancellation.cancelled() => {
                self.terminate_and_reap(term_grace, kill_timeout).await
            }
        }
    }

    pub async fn complete_reaped(
        &mut self,
        status: ExitStatus,
        termination: Termination,
        term_grace: Duration,
        kill_timeout: Duration,
    ) -> Result<ProcessOutcome> {
        let initial_term = termination != Termination::Exited;
        let initial_kill = termination == Termination::Killed;
        let group_cleanup = self
            .finish_group_cleanup(
                Some(status),
                Some(termination),
                initial_term,
                initial_kill,
                term_grace,
                kill_timeout,
            )
            .await?;
        Ok(ProcessOutcome {
            pid: self.pid,
            pgid: self.pgid,
            status,
            termination,
            group_cleanup,
        })
    }

    pub async fn terminate_and_reap(
        &mut self,
        term_grace: Duration,
        kill_timeout: Duration,
    ) -> Result<ProcessOutcome> {
        let (status, termination, term_attempted, kill_attempted) =
            if let Some(status) = self.try_reap()? {
                (status, Termination::Exited, false, false)
            } else {
                match self.signal_group(Signal::TERM) {
                    Ok(()) => match tokio::time::timeout(term_grace, self.wait()).await {
                        Ok(result) => (result?, Termination::Terminated, true, false),
                        Err(_) => match self.signal_group(Signal::KILL) {
                            Ok(()) => {
                                let status = tokio::time::timeout(kill_timeout, self.wait())
                                    .await
                                    .map_err(|_| {
                                        HarnessError::Cleanup(self.cleanup_failure(
                                            None,
                                            Some(Termination::Killed),
                                            true,
                                            true,
                                            true,
                                            true,
                                            "leader reap after SIGKILL timed out",
                                        ))
                                    })??;
                                (status, Termination::Killed, true, true)
                            }
                            Err(detail) => {
                                match tokio::time::timeout(kill_timeout, self.wait()).await {
                                    Ok(result) => (result?, Termination::Terminated, true, false),
                                    Err(_) => {
                                        return Err(HarnessError::Cleanup(self.cleanup_failure(
                                            None, None, true, true, true, false, detail,
                                        )));
                                    }
                                }
                            }
                        },
                    },
                    Err(detail) => match tokio::time::timeout(term_grace, self.wait()).await {
                        Ok(result) => (result?, Termination::Exited, false, false),
                        Err(_) => {
                            return Err(HarnessError::Cleanup(
                                self.cleanup_failure(None, None, true, false, false, false, detail),
                            ));
                        }
                    },
                }
            };
        let group_cleanup = self
            .finish_group_cleanup(
                Some(status),
                Some(termination),
                term_attempted,
                kill_attempted,
                term_grace,
                kill_timeout,
            )
            .await?;
        Ok(ProcessOutcome {
            pid: self.pid,
            pgid: self.pgid,
            status,
            termination,
            group_cleanup,
        })
    }

    pub async fn ensure_group_empty(
        &mut self,
        term_grace: Duration,
        kill_timeout: Duration,
    ) -> Result<GroupCleanup> {
        self.finish_group_cleanup(
            self.reaped,
            self.reaped.map(|_| Termination::Exited),
            false,
            false,
            term_grace,
            kill_timeout,
        )
        .await
    }

    async fn finish_group_cleanup(
        &mut self,
        status: Option<ExitStatus>,
        leader_termination: Option<Termination>,
        mut term_attempted: bool,
        mut kill_attempted: bool,
        term_grace: Duration,
        kill_timeout: Duration,
    ) -> Result<GroupCleanup> {
        let exists = self.group_exists().map_err(|detail| {
            HarnessError::Cleanup(self.cleanup_failure(
                status,
                leader_termination,
                term_attempted,
                kill_attempted,
                false,
                false,
                detail,
            ))
        })?;
        if !exists {
            self.group_clean = true;
            return Ok(GroupCleanup {
                termination: if kill_attempted {
                    Termination::Killed
                } else if term_attempted {
                    Termination::Terminated
                } else {
                    Termination::Exited
                },
                term_attempted,
                kill_attempted,
                descendants_present: false,
                group_empty: true,
            });
        }

        let descendants_present = status.is_some();
        if kill_attempted {
            if self
                .wait_for_group_exit(kill_timeout)
                .await
                .map_err(|detail| {
                    HarnessError::Cleanup(self.cleanup_failure(
                        status,
                        leader_termination,
                        term_attempted,
                        kill_attempted,
                        descendants_present,
                        false,
                        detail,
                    ))
                })?
            {
                self.group_clean = true;
                return Ok(GroupCleanup {
                    termination: Termination::Killed,
                    term_attempted,
                    kill_attempted,
                    descendants_present,
                    group_empty: true,
                });
            }
            return Err(HarnessError::Cleanup(self.cleanup_failure(
                status,
                leader_termination,
                term_attempted,
                kill_attempted,
                descendants_present,
                true,
                "owned process group exit after SIGKILL timed out",
            )));
        }

        if !term_attempted {
            self.signal_group(Signal::TERM).map_err(|detail| {
                HarnessError::Cleanup(self.cleanup_failure(
                    status,
                    leader_termination,
                    true,
                    kill_attempted,
                    descendants_present,
                    false,
                    detail,
                ))
            })?;
            term_attempted = true;
        }
        if self
            .wait_for_group_exit(term_grace)
            .await
            .map_err(|detail| {
                HarnessError::Cleanup(self.cleanup_failure(
                    status,
                    leader_termination,
                    term_attempted,
                    kill_attempted,
                    descendants_present,
                    false,
                    detail,
                ))
            })?
        {
            self.group_clean = true;
            return Ok(GroupCleanup {
                termination: Termination::Terminated,
                term_attempted,
                kill_attempted,
                descendants_present,
                group_empty: true,
            });
        }

        self.signal_group(Signal::KILL).map_err(|detail| {
            HarnessError::Cleanup(self.cleanup_failure(
                status,
                leader_termination,
                term_attempted,
                true,
                descendants_present,
                false,
                detail,
            ))
        })?;
        kill_attempted = true;
        if self
            .wait_for_group_exit(kill_timeout)
            .await
            .map_err(|detail| {
                HarnessError::Cleanup(self.cleanup_failure(
                    status,
                    leader_termination,
                    term_attempted,
                    kill_attempted,
                    descendants_present,
                    false,
                    detail,
                ))
            })?
        {
            self.group_clean = true;
            return Ok(GroupCleanup {
                termination: Termination::Killed,
                term_attempted,
                kill_attempted,
                descendants_present,
                group_empty: true,
            });
        }
        Err(HarnessError::Cleanup(self.cleanup_failure(
            status,
            leader_termination,
            term_attempted,
            kill_attempted,
            descendants_present,
            true,
            "owned process group exit timed out",
        )))
    }

    #[allow(clippy::too_many_arguments)]
    fn cleanup_failure(
        &self,
        status: Option<ExitStatus>,
        leader_termination: Option<Termination>,
        term_attempted: bool,
        kill_attempted: bool,
        descendants_present: bool,
        timed_out: bool,
        detail: impl Into<String>,
    ) -> CleanupFailure {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        CleanupFailure {
            pid: self.pid,
            pgid: self.pgid,
            leader_termination,
            exit_code: status.and_then(|status| status.code()),
            #[cfg(unix)]
            exit_signal: status.and_then(|status| status.signal()),
            #[cfg(not(unix))]
            exit_signal: None,
            term_attempted,
            kill_attempted,
            descendants_present,
            leader_reaped: status.is_some(),
            group_empty: false,
            timed_out,
            detail: detail.into(),
        }
    }

    #[cfg(unix)]
    fn group_exists(&self) -> std::result::Result<bool, String> {
        classify_group_probe(rustix::process::test_kill_process_group(self.pgid))
    }

    #[cfg(not(unix))]
    fn group_exists(&self) -> std::result::Result<bool, String> {
        Ok(false)
    }

    async fn wait_for_group_exit(&self, timeout: Duration) -> std::result::Result<bool, String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self.group_exists()? {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(10).min(timeout)).await;
        }
    }

    fn signal_group(&mut self, signal: Signal) -> std::result::Result<(), String> {
        #[cfg(unix)]
        {
            match rustix::process::kill_process_group(self.pgid, signal) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
                Err(error) => Err(format!("failed to signal owned process group: {error}")),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = signal;
            self.child
                .start_kill()
                .map_err(|error| format!("failed to kill owned child: {error}"))
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.reaped.is_some() && self.group_clean {
            return;
        }
        let _ = self.signal_group(Signal::KILL);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.reaped.is_none() {
                match self.child.try_wait() {
                    Ok(Some(status)) => {
                        self.reaped = Some(status);
                        self.reap_count += 1;
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
            if self.group_exists() == Ok(false) {
                self.group_clean = true;
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(unix)]
fn classify_group_probe(
    probe: std::result::Result<(), rustix::io::Errno>,
) -> std::result::Result<bool, String> {
    match probe {
        Ok(()) | Err(rustix::io::Errno::PERM) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(error) => Err(format!("failed to probe owned process group: {error}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::classify_group_probe;

    #[test]
    fn process_group_probe_classifies_permission_and_missing_without_weakening_errors() {
        assert_eq!(classify_group_probe(Ok(())), Ok(true));
        assert_eq!(classify_group_probe(Err(rustix::io::Errno::PERM)), Ok(true));
        assert_eq!(
            classify_group_probe(Err(rustix::io::Errno::SRCH)),
            Ok(false)
        );
        let error = classify_group_probe(Err(rustix::io::Errno::INVAL))
            .expect_err("unexpected probe errors remain observable");
        assert!(error.contains("failed to probe owned process group"));
    }
}

fn clean_exec_binary() -> Result<PathBuf> {
    let current = std::env::current_exe().map_err(|error| io_error("current executable", error))?;
    if current.file_name().is_some_and(|name| name == "test-sync") {
        return Ok(current);
    }
    let candidate = current
        .parent()
        .and_then(Path::parent)
        .map(|directory| directory.join("test-sync"))
        .ok_or(HarnessError::Process(
            "could not resolve descriptor-cleaning executable",
        ))?;
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(HarnessError::Process(
            "descriptor-cleaning executable is unavailable",
        ))
    }
}

#[cfg(unix)]
pub fn exec_clean(mut arguments: impl Iterator<Item = OsString>) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    let program = arguments.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing exec-clean program",
        )
    })?;
    close_unrelated_descriptors(&[]);
    let error = std::process::Command::new(program).args(arguments).exec();
    Err(error)
}

#[cfg(unix)]
pub fn exec_pinned(
    descriptors: Vec<RawFd>,
    arguments: impl Iterator<Item = OsString>,
) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;

    if descriptors.is_empty()
        || descriptors.len() > MAX_INTERPRETER_DEPTH + 1
        || descriptors.iter().any(|descriptor| *descriptor < 3)
        || descriptors
            .iter()
            .enumerate()
            .any(|(index, descriptor)| descriptors[..index].contains(descriptor))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid pinned execution descriptor plan",
        ));
    }
    close_unrelated_descriptors(&descriptors);
    let mut kinds = Vec::with_capacity(descriptors.len());
    for descriptor in &descriptors {
        if unsafe { libc::lseek(*descriptor, 0, libc::SEEK_SET) } == -1 {
            return Err(std::io::Error::last_os_error());
        }
        kinds.push(parse_executable_kind(*descriptor)?);
    }
    if !matches!(kinds.last(), Some(ExecutableKind::Native))
        || kinds[..kinds.len() - 1]
            .iter()
            .any(|kind| !matches!(kind, ExecutableKind::Script(_)))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pinned execution descriptor plan does not end in a native executable",
        ));
    }

    let native_descriptor = *descriptors.last().expect("descriptor plan is nonempty");
    let expected_native_path =
        kinds
            .get(kinds.len().saturating_sub(2))
            .and_then(|kind| match kind {
                ExecutableKind::Script(shebang) => Some(shebang.interpreter.as_path()),
                ExecutableKind::Native => None,
            });
    let program = native_descriptor_path(native_descriptor, expected_native_path)?;
    set_cloexec(native_descriptor, true)?;
    let mut command = std::process::Command::new(program);
    for (descriptor, kind) in descriptors[..descriptors.len() - 1]
        .iter()
        .zip(&kinds[..kinds.len() - 1])
        .rev()
    {
        let ExecutableKind::Script(shebang) = kind else {
            unreachable!("descriptor plan shape was validated")
        };
        if let Some(argument) = &shebang.argument {
            command.arg(argument);
        }
        command.arg(script_descriptor_path(*descriptor)?);
    }
    command.args(arguments);
    Err(command.exec())
}

#[cfg(target_os = "linux")]
fn native_descriptor_path(
    descriptor: RawFd,
    _expected_path: Option<&Path>,
) -> std::io::Result<PathBuf> {
    let path = PathBuf::from(format!("/proc/self/fd/{descriptor}"));
    path.metadata()?;
    Ok(path)
}

#[cfg(target_os = "macos")]
fn native_descriptor_path(
    descriptor: RawFd,
    expected_path: Option<&Path>,
) -> std::io::Result<PathBuf> {
    match expected_path {
        Some(path) => validated_macos_descriptor_at_path(descriptor, path.to_path_buf()),
        None => validated_macos_descriptor_path(descriptor),
    }
}

#[cfg(target_os = "macos")]
fn validated_macos_descriptor_path(descriptor: RawFd) -> std::io::Result<PathBuf> {
    let mut buffer = vec![0_u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(descriptor, libc::F_GETPATH, buffer.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let length = buffer.iter().position(|byte| *byte == 0).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pinned native executable path is not terminated",
        )
    })?;
    buffer.truncate(length);
    validated_macos_descriptor_at_path(descriptor, PathBuf::from(OsString::from_vec(buffer)))
}

#[cfg(target_os = "macos")]
fn validated_macos_descriptor_at_path(
    descriptor: RawFd,
    path: PathBuf,
) -> std::io::Result<PathBuf> {
    let path_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)?;
    let descriptor_file = File::open(format!("/dev/fd/{descriptor}"))?;
    if file_state(&path_file)? != file_state(&descriptor_file)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "pinned executable path no longer matches its descriptor",
        ));
    }
    Ok(path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn native_descriptor_path(
    _descriptor: RawFd,
    _expected_path: Option<&Path>,
) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pinned observer execution is supported only on Linux and macOS",
    ))
}

#[cfg(target_os = "linux")]
fn script_descriptor_path(descriptor: RawFd) -> std::io::Result<PathBuf> {
    Ok(PathBuf::from(format!("/proc/self/fd/{descriptor}")))
}

#[cfg(target_os = "macos")]
fn script_descriptor_path(descriptor: RawFd) -> std::io::Result<PathBuf> {
    validated_macos_descriptor_path(descriptor)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn script_descriptor_path(descriptor: RawFd) -> std::io::Result<PathBuf> {
    Ok(PathBuf::from(format!("/dev/fd/{descriptor}")))
}

fn parse_executable_kind(descriptor: RawFd) -> std::io::Result<ExecutableKind> {
    let mut buffer = [0_u8; MAX_SHEBANG_BYTES + 1];
    let length = unsafe { libc::pread(descriptor, buffer.as_mut_ptr().cast(), buffer.len(), 0) };
    if length == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let bytes = &buffer[..usize::try_from(length).expect("pread length is nonnegative")];
    if !bytes.starts_with(b"#!") {
        return Ok(ExecutableKind::Native);
    }
    let line_end = bytes.iter().position(|byte| *byte == b'\n');
    if line_end.is_none() && bytes.len() > MAX_SHEBANG_BYTES
        || line_end.is_some_and(|end| end >= MAX_SHEBANG_BYTES)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "effect observer shebang is too long",
        ));
    }
    let mut line = &bytes[..line_end.unwrap_or(bytes.len())];
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    if line.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "effect observer shebang contains NUL",
        ));
    }
    let shebang = line
        .strip_prefix(b"#!")
        .expect("shebang prefix checked above");
    let mut parts = shebang
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|part| !part.is_empty());
    let interpreter = parts
        .next()
        .filter(|part| part.starts_with(b"/"))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "effect observer shebang interpreter is not absolute",
            )
        })?;
    let interpreter_argument = parts.next();
    if parts.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "effect observer shebang has too many arguments",
        ));
    }
    Ok(ExecutableKind::Script(Shebang {
        interpreter: PathBuf::from(OsString::from_vec(interpreter.to_vec())),
        argument: interpreter_argument.map(|argument| OsString::from_vec(argument.to_vec())),
    }))
}

#[cfg(unix)]
fn close_unrelated_descriptors(preserve: &[RawFd]) {
    let descriptor_directory = if Path::new("/proc/self/fd").is_dir() {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    let descriptors = std::fs::read_dir(descriptor_directory).ok().map(|entries| {
        entries
            .filter_map(std::result::Result::ok)
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
            .filter(|descriptor| *descriptor >= 3 && !preserve.contains(descriptor))
            .collect::<Vec<_>>()
    });
    if let Some(descriptors) = descriptors {
        for descriptor in descriptors {
            // The trampoline is already a successfully exec'd, single-threaded process. Its only
            // intentional IPC is on stdio, so every other descriptor is unrelated and must close.
            unsafe {
                libc::close(descriptor);
            }
        }
        return;
    }

    let maximum = rustix::process::getrlimit(Resource::Nofile)
        .current
        .unwrap_or(1_048_576)
        .min(i32::MAX as u64);
    for descriptor in 3..maximum {
        if preserve.contains(&(descriptor as RawFd)) {
            continue;
        }
        unsafe {
            libc::close(descriptor as i32);
        }
    }
}
