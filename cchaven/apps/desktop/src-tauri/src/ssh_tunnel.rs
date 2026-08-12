//! Project-scoped SSH LocalForward tunnel management.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use tokio_util::task::TaskTracker;

const MASTER_READY_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_CHECK_TIMEOUT: Duration = Duration::from_millis(500);
const CONTROL_OUTPUT_LIMIT: u64 = 16 * 1024;
const PROXY_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROXY_IO_TIMEOUT: Duration = Duration::from_millis(100);
const PROXY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_PROXY_CONNECTIONS: usize = 8;
const TUNNEL_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const TUNNEL_CLOSE_ALL_TIMEOUT: Duration = Duration::from_secs(30);

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TunnelErrorCode {
    PlatformUnsupported,
    InvalidHost,
    ListenerBindFailed,
    ListenerAddressFailed,
    ControlDirFailed,
    SpawnFailed,
    StartFailed,
    StartTimeout,
    LivenessFailed,
    MasterMismatch,
    ControlCaptureFailed,
    ControlSpawnFailed,
    ControlTimeout,
    ControlWaitFailed,
    ControlReadFailed,
    ForwardFailed,
    ForwardPathInvalid,
    ForwardSocketMissing,
    ForwardSocketInvalid,
    ForwardSocketOwnerMismatch,
    ForwardSocketReplaced,
    ForwardConnectFailed,
    ForwardPeerFailed,
    ForwardPeerMismatch,
    ForwardPeerUnsupported,
    ProxyListenerFailed,
    ProxySpawnFailed,
    ProxyJoinTimeout,
    ProxyJoinFailed,
    KillFailed,
    WaitTimeout,
    WaitFailed,
    OwnerMismatch,
}

impl TunnelErrorCode {
    pub(crate) const fn stable(self) -> &'static str {
        match self {
            Self::PlatformUnsupported => "tunnel_platform_unsupported",
            Self::InvalidHost => "tunnel_invalid_host",
            Self::ListenerBindFailed => "tunnel_listener_bind_failed",
            Self::ListenerAddressFailed => "tunnel_listener_address_failed",
            Self::ControlDirFailed => "tunnel_control_dir_failed",
            Self::SpawnFailed => "tunnel_spawn_failed",
            Self::StartFailed => "tunnel_start_failed",
            Self::StartTimeout => "tunnel_start_timeout",
            Self::LivenessFailed => "tunnel_liveness_failed",
            Self::MasterMismatch => "tunnel_master_mismatch",
            Self::ControlCaptureFailed => "tunnel_control_capture_failed",
            Self::ControlSpawnFailed => "tunnel_control_spawn_failed",
            Self::ControlTimeout => "tunnel_control_timeout",
            Self::ControlWaitFailed => "tunnel_control_wait_failed",
            Self::ControlReadFailed => "tunnel_control_read_failed",
            Self::ForwardFailed => "tunnel_forward_failed",
            Self::ForwardPathInvalid => "tunnel_forward_path_invalid",
            Self::ForwardSocketMissing => "tunnel_forward_socket_missing",
            Self::ForwardSocketInvalid => "tunnel_forward_socket_invalid",
            Self::ForwardSocketOwnerMismatch => "tunnel_forward_socket_owner_mismatch",
            Self::ForwardSocketReplaced => "tunnel_forward_socket_replaced",
            Self::ForwardConnectFailed => "tunnel_forward_connect_failed",
            Self::ForwardPeerFailed => "tunnel_forward_peer_failed",
            Self::ForwardPeerMismatch => "tunnel_forward_peer_mismatch",
            Self::ForwardPeerUnsupported => "tunnel_forward_peer_unsupported",
            Self::ProxyListenerFailed => "tunnel_proxy_listener_failed",
            Self::ProxySpawnFailed => "tunnel_proxy_spawn_failed",
            Self::ProxyJoinTimeout => "tunnel_proxy_join_timeout",
            Self::ProxyJoinFailed => "tunnel_proxy_join_failed",
            Self::KillFailed => "tunnel_kill_failed",
            Self::WaitTimeout => "tunnel_wait_timeout",
            Self::WaitFailed => "tunnel_wait_failed",
            Self::OwnerMismatch => "tunnel_owner_mismatch",
        }
    }

    pub(crate) const fn agent_code(self) -> fns_agent::AgentErrorCode {
        use fns_agent::AgentErrorCode;
        match self {
            Self::ControlTimeout
            | Self::StartTimeout
            | Self::ProxyJoinTimeout
            | Self::WaitTimeout => AgentErrorCode::ShutdownTimeout,
            Self::LivenessFailed
            | Self::MasterMismatch
            | Self::StartFailed
            | Self::ControlWaitFailed
            | Self::ProxyJoinFailed
            | Self::KillFailed
            | Self::WaitFailed => AgentErrorCode::AbnormalExit,
            Self::SpawnFailed | Self::ControlSpawnFailed | Self::ProxySpawnFailed => {
                AgentErrorCode::SpawnFailed
            }
            _ => AgentErrorCode::Network,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TunnelFailure {
    pub(crate) primary: TunnelErrorCode,
    pub(crate) cleanup: Vec<TunnelErrorCode>,
}

impl TunnelFailure {
    pub(crate) fn primary(primary: TunnelErrorCode) -> Self {
        Self {
            primary,
            cleanup: Vec::new(),
        }
    }

    fn with_cleanup(mut self, cleanup: TunnelFailure) -> Self {
        self.cleanup.push(cleanup.primary);
        self.cleanup.extend(cleanup.cleanup);
        self
    }

    pub(crate) fn stable(&self) -> String {
        self.cleanup
            .iter()
            .fold(self.primary.stable().to_owned(), |mut message, cleanup| {
                message.push_str(";cleanup=");
                message.push_str(cleanup.stable());
                message
            })
    }
}

impl From<TunnelErrorCode> for TunnelFailure {
    fn from(code: TunnelErrorCode) -> Self {
        Self::primary(code)
    }
}

impl std::fmt::Display for TunnelFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.stable())
    }
}

pub(crate) struct TunnelCreateFailure {
    pub(crate) failure: TunnelFailure,
    pub(crate) retained: Option<Box<dyn TunnelResource>>,
}

impl TunnelCreateFailure {
    fn unowned(failure: impl Into<TunnelFailure>) -> Self {
        Self {
            failure: failure.into(),
            retained: None,
        }
    }
}

impl From<TunnelFailure> for TunnelCreateFailure {
    fn from(failure: TunnelFailure) -> Self {
        Self::unowned(failure)
    }
}

impl From<TunnelErrorCode> for TunnelCreateFailure {
    fn from(code: TunnelErrorCode) -> Self {
        Self::unowned(code)
    }
}

impl std::fmt::Debug for TunnelCreateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TunnelCreateFailure")
            .field("failure", &self.failure)
            .field("retained", &self.retained.is_some())
            .finish()
    }
}

trait TunnelChild: Send {
    fn id(&self) -> u32;
    fn kill(&mut self) -> io::Result<()>;
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
}

impl TunnelChild for Child {
    fn id(&self) -> u32 {
        Child::id(self)
    }

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
}

trait TunnelProxyWorker: Send {
    fn is_finished(&self) -> bool;
    fn join(self: Box<Self>) -> Result<(), ()>;
}

struct ProxyThread(Option<JoinHandle<()>>);

impl TunnelProxyWorker for ProxyThread {
    fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn join(mut self: Box<Self>) -> Result<(), ()> {
        self.0
            .take()
            .map_or(Ok(()), |thread| thread.join().map_err(|_| ()))
    }
}

pub struct SshTunnel {
    local_port: u16,
    ssh_program: PathBuf,
    ssh_alias: String,
    control_dir: tempfile::TempDir,
    child: Option<Box<dyn TunnelChild>>,
    control_children: Vec<Box<dyn TunnelChild>>,
    proxy_stop: Arc<AtomicBool>,
    proxy_failed: Arc<AtomicBool>,
    proxy_thread: Option<Box<dyn TunnelProxyWorker>>,
}

pub(crate) trait TunnelResource: Send {
    fn local_port(&self) -> u16;
    fn is_alive(&mut self) -> Result<bool, TunnelFailure>;
    fn close(&mut self) -> Result<(), TunnelFailure>;
}

pub(crate) trait TunnelFactory: Send + Sync {
    fn create(
        &self,
        tunnel_key: &str,
        ssh_host: &str,
        remote_port: u16,
    ) -> Result<Box<dyn TunnelResource>, TunnelCreateFailure>;
}

struct SshTunnelFactory;

impl TunnelFactory for SshTunnelFactory {
    fn create(
        &self,
        _tunnel_key: &str,
        ssh_host: &str,
        remote_port: u16,
    ) -> Result<Box<dyn TunnelResource>, TunnelCreateFailure> {
        SshTunnel::create(ssh_host, remote_port)
            .map(|tunnel| Box::new(tunnel) as Box<dyn TunnelResource>)
    }
}

impl SshTunnel {
    pub fn create(ssh_alias: &str, remote_port: u16) -> Result<Self, TunnelCreateFailure> {
        Self::create_with_program(Path::new("ssh"), ssh_alias, remote_port)
    }

    fn create_with_program(
        ssh_program: &Path,
        ssh_alias: &str,
        remote_port: u16,
    ) -> Result<Self, TunnelCreateFailure> {
        #[cfg(not(unix))]
        {
            let _ = (ssh_program, ssh_alias, remote_port);
            return Err(TunnelErrorCode::PlatformUnsupported.into());
        }

        #[cfg(unix)]
        {
            validate_ssh_alias(ssh_alias).map_err(TunnelCreateFailure::from)?;
            let listener = TcpListener::bind("127.0.0.1:0")
                .map_err(|_| TunnelErrorCode::ListenerBindFailed)?;
            let local_port = listener
                .local_addr()
                .map_err(|_| TunnelErrorCode::ListenerAddressFailed)?
                .port();
            let control_dir = tempfile::Builder::new()
                .prefix("fns-ssh-")
                .tempdir()
                .map_err(|_| TunnelErrorCode::ControlDirFailed)?;
            let control_path = control_dir.path().join("master.sock");
            let forward_socket = control_dir.path().join("forward.sock");
            let child = spawn_master(ssh_program, ssh_alias, &control_path)
                .map_err(TunnelCreateFailure::from)?;
            let proxy_stop = Arc::new(AtomicBool::new(false));
            let proxy_failed = Arc::new(AtomicBool::new(false));
            let mut tunnel = Self {
                local_port,
                ssh_program: ssh_program.to_path_buf(),
                ssh_alias: ssh_alias.to_owned(),
                control_dir,
                child: Some(Box::new(child)),
                control_children: Vec::new(),
                proxy_stop: Arc::clone(&proxy_stop),
                proxy_failed: Arc::clone(&proxy_failed),
                proxy_thread: None,
            };

            let create_result = (|| {
                let child = tunnel.child.as_mut().expect("spawned SSH master missing");
                wait_for_master(
                    ssh_program,
                    ssh_alias,
                    &control_path,
                    tunnel.control_dir.path(),
                    child.as_mut(),
                    &mut tunnel.control_children,
                )?;
                let forward_socket_text = forward_socket
                    .to_str()
                    .ok_or(TunnelErrorCode::ForwardPathInvalid)?;
                let forward = format!("{forward_socket_text}:127.0.0.1:{remote_port}");
                let result = run_control_command(
                    ssh_program,
                    ssh_alias,
                    &control_path,
                    tunnel.control_dir.path(),
                    &["-O", "forward", "-L", &forward],
                    CONTROL_COMMAND_TIMEOUT,
                    &mut tunnel.control_children,
                )?;
                if !result.success {
                    return Err(TunnelErrorCode::ForwardFailed.into());
                }
                let forward_identity = forward_socket_identity(&forward_socket)?;
                let control_owner = std::fs::symlink_metadata(tunnel.control_dir.path())
                    .map_err(|_| TunnelErrorCode::ControlDirFailed)?
                    .uid();
                if forward_identity.owner != control_owner {
                    return Err(TunnelErrorCode::ForwardSocketOwnerMismatch.into());
                }
                let child = tunnel.child.as_mut().expect("spawned SSH master missing");
                ensure_master_matches(
                    ssh_program,
                    ssh_alias,
                    &control_path,
                    tunnel.control_dir.path(),
                    child.as_mut(),
                    CONTROL_CHECK_TIMEOUT,
                    &mut tunnel.control_children,
                )?;
                let expected_master_pid = tunnel
                    .child
                    .as_ref()
                    .expect("spawned SSH master missing")
                    .id();
                drop(connect_to_expected_master(
                    &forward_socket,
                    forward_identity,
                    expected_master_pid,
                )?);
                let proxy_thread = start_proxy(
                    listener,
                    forward_socket,
                    forward_identity,
                    expected_master_pid,
                    Arc::clone(&proxy_stop),
                    Arc::clone(&proxy_failed),
                )?;
                Ok(proxy_thread)
            })();
            match create_result {
                Ok(proxy_thread) => {
                    tunnel.proxy_thread = Some(Box::new(ProxyThread(Some(proxy_thread))));
                    Ok(tunnel)
                }
                Err(failure) => Err(finish_failed_construction(
                    tunnel,
                    failure,
                    TUNNEL_CLOSE_TIMEOUT,
                )),
            }
        }
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    fn is_alive(&mut self) -> Result<bool, TunnelFailure> {
        if self.proxy_failed.load(Ordering::Acquire)
            || self
                .proxy_thread
                .as_ref()
                .is_none_or(|proxy| proxy.is_finished())
        {
            return Ok(false);
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(false);
        };
        ensure_master_matches(
            &self.ssh_program,
            &self.ssh_alias,
            &self.control_dir.path().join("master.sock"),
            self.control_dir.path(),
            child.as_mut(),
            CONTROL_CHECK_TIMEOUT,
            &mut self.control_children,
        )
        .map(|()| true)
    }
}

fn spawn_master(
    ssh_program: &Path,
    ssh_alias: &str,
    control_path: &Path,
) -> Result<Child, TunnelFailure> {
    Command::new(ssh_program)
        .arg("-M")
        .arg("-S")
        .arg(control_path)
        .arg("-N")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ClearAllForwardings=yes")
        .arg("-o")
        .arg("ControlMaster=yes")
        .arg("-o")
        .arg("ControlPersist=no")
        .arg("-o")
        .arg("ServerAliveInterval=30")
        .arg("-o")
        .arg("ServerAliveCountMax=3")
        .arg("-o")
        .arg("ExitOnForwardFailure=yes")
        .arg("--")
        .arg(ssh_alias)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| TunnelErrorCode::SpawnFailed.into())
}

fn validate_ssh_alias(ssh_alias: &str) -> Result<(), TunnelFailure> {
    if ssh_alias.is_empty()
        || ssh_alias.starts_with('-')
        || ssh_alias
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(TunnelErrorCode::InvalidHost.into());
    }
    Ok(())
}

fn wait_for_master(
    ssh_program: &Path,
    ssh_alias: &str,
    control_path: &Path,
    capture_dir: &Path,
    child: &mut dyn TunnelChild,
    control_children: &mut Vec<Box<dyn TunnelChild>>,
) -> Result<(), TunnelFailure> {
    let deadline = Instant::now() + MASTER_READY_TIMEOUT;
    loop {
        match ensure_master_matches(
            ssh_program,
            ssh_alias,
            control_path,
            capture_dir,
            child,
            CONTROL_CHECK_TIMEOUT,
            control_children,
        ) {
            Ok(()) => return Ok(()),
            Err(error) if error.primary == TunnelErrorCode::StartFailed => return Err(error),
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Err(TunnelErrorCode::StartTimeout.into()),
        }
    }
}

fn ensure_master_matches(
    ssh_program: &Path,
    ssh_alias: &str,
    control_path: &Path,
    capture_dir: &Path,
    child: &mut dyn TunnelChild,
    timeout: Duration,
    control_children: &mut Vec<Box<dyn TunnelChild>>,
) -> Result<(), TunnelFailure> {
    let expected_pid = match child.try_wait() {
        Ok(Some(_)) => return Err(TunnelErrorCode::StartFailed.into()),
        Ok(None) => child.id(),
        Err(_) => return Err(TunnelErrorCode::LivenessFailed.into()),
    };
    let result = run_control_command(
        ssh_program,
        ssh_alias,
        control_path,
        capture_dir,
        &["-O", "check"],
        timeout,
        control_children,
    )?;
    if !result.success || parse_master_pid(&result.stdout, &result.stderr) != Some(expected_pid) {
        return Err(TunnelErrorCode::MasterMismatch.into());
    }
    match child.try_wait() {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(TunnelErrorCode::StartFailed.into()),
        Err(_) => Err(TunnelErrorCode::LivenessFailed.into()),
    }
}

struct ControlCommandResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_control_command(
    ssh_program: &Path,
    ssh_alias: &str,
    control_path: &Path,
    capture_dir: &Path,
    arguments: &[&str],
    timeout: Duration,
    control_children: &mut Vec<Box<dyn TunnelChild>>,
) -> Result<ControlCommandResult, TunnelFailure> {
    let command_id = uuid::Uuid::new_v4();
    let stdout_path = capture_dir.join(format!("command-{command_id}.stdout"));
    let stderr_path = capture_dir.join(format!("command-{command_id}.stderr"));
    let stdout =
        std::fs::File::create(&stdout_path).map_err(|_| TunnelErrorCode::ControlCaptureFailed)?;
    let stderr =
        std::fs::File::create(&stderr_path).map_err(|_| TunnelErrorCode::ControlCaptureFailed)?;
    let mut command = Command::new(ssh_program);
    command
        .arg("-S")
        .arg(control_path)
        .args(arguments)
        .arg("--")
        .arg(ssh_alias)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child: Box<dyn TunnelChild> = Box::new(
        command
            .spawn()
            .map_err(|_| TunnelErrorCode::ControlSpawnFailed)?,
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(control_command_failure(
                    child,
                    TunnelFailure::primary(TunnelErrorCode::ControlTimeout),
                    timeout,
                    control_children,
                ));
            }
            Err(_) => {
                let _ = std::fs::remove_file(&stdout_path);
                let _ = std::fs::remove_file(&stderr_path);
                return Err(control_command_failure(
                    child,
                    TunnelFailure::primary(TunnelErrorCode::ControlWaitFailed),
                    timeout,
                    control_children,
                ));
            }
        }
    };
    let stdout = read_capture(&stdout_path)?;
    let stderr = read_capture(&stderr_path)?;
    let _ = std::fs::remove_file(stdout_path);
    let _ = std::fs::remove_file(stderr_path);
    Ok(ControlCommandResult {
        success: status.success(),
        stdout,
        stderr,
    })
}

fn read_capture(path: &Path) -> Result<Vec<u8>, TunnelFailure> {
    let file = std::fs::File::open(path).map_err(|_| TunnelErrorCode::ControlReadFailed)?;
    let mut bytes = Vec::new();
    file.take(CONTROL_OUTPUT_LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|_| TunnelErrorCode::ControlReadFailed)?;
    Ok(bytes)
}

fn parse_master_pid(stdout: &[u8], stderr: &[u8]) -> Option<u32> {
    [stdout, stderr]
        .into_iter()
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .find_map(|text| {
            let (_, suffix) = text.split_once("(pid=")?;
            let digits = suffix.split_once(')')?.0;
            digits.parse().ok()
        })
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ForwardSocketIdentity {
    device: u64,
    inode: u64,
    owner: u32,
}

#[cfg(unix)]
fn forward_socket_identity(path: &Path) -> Result<ForwardSocketIdentity, TunnelFailure> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| TunnelErrorCode::ForwardSocketMissing)?;
    if !metadata.file_type().is_socket() {
        return Err(TunnelErrorCode::ForwardSocketInvalid.into());
    }
    Ok(ForwardSocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
    })
}

#[cfg(unix)]
fn start_proxy(
    listener: TcpListener,
    forward_socket: PathBuf,
    forward_identity: ForwardSocketIdentity,
    expected_master_pid: u32,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, TunnelFailure> {
    listener
        .set_nonblocking(true)
        .map_err(|_| TunnelErrorCode::ProxyListenerFailed)?;
    std::thread::Builder::new()
        .name("fns-ssh-proxy".to_owned())
        .spawn(move || {
            proxy_loop(
                listener,
                &forward_socket,
                forward_identity,
                expected_master_pid,
                stop,
                failed,
            );
        })
        .map_err(|_| TunnelErrorCode::ProxySpawnFailed.into())
}

#[cfg(unix)]
fn proxy_loop(
    listener: TcpListener,
    forward_socket: &Path,
    forward_identity: ForwardSocketIdentity,
    expected_master_pid: u32,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) {
    let mut connections = Vec::new();
    while !stop.load(Ordering::Acquire) {
        if !reap_finished_threads(&mut connections) {
            failed.store(true, Ordering::Release);
            stop.store(true, Ordering::Release);
            break;
        }
        match listener.accept() {
            Ok((tcp, _)) => {
                if connections.len() >= MAX_PROXY_CONNECTIONS {
                    let _ = tcp.shutdown(Shutdown::Both);
                    continue;
                }
                if tcp.set_nonblocking(false).is_err() {
                    let _ = tcp.shutdown(Shutdown::Both);
                    failed.store(true, Ordering::Release);
                    stop.store(true, Ordering::Release);
                    break;
                }
                if forward_socket_identity(forward_socket) != Ok(forward_identity) {
                    let _ = tcp.shutdown(Shutdown::Both);
                    failed.store(true, Ordering::Release);
                    stop.store(true, Ordering::Release);
                    break;
                }
                let unix = match connect_to_expected_master(
                    forward_socket,
                    forward_identity,
                    expected_master_pid,
                ) {
                    Ok(unix) => unix,
                    Err(_) => {
                        let _ = tcp.shutdown(Shutdown::Both);
                        failed.store(true, Ordering::Release);
                        stop.store(true, Ordering::Release);
                        break;
                    }
                };
                let connection_stop = Arc::clone(&stop);
                let connection_failed = Arc::clone(&failed);
                match std::thread::Builder::new()
                    .name("fns-ssh-proxy-connection".to_owned())
                    .spawn(move || {
                        proxy_connection(tcp, unix, connection_stop, connection_failed);
                    }) {
                    Ok(connection) => connections.push(connection),
                    Err(_) => {
                        failed.store(true, Ordering::Release);
                        stop.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(PROXY_POLL_INTERVAL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                failed.store(true, Ordering::Release);
                stop.store(true, Ordering::Release);
                break;
            }
        }
    }
    drop(listener);
    for connection in connections {
        if connection.join().is_err() {
            failed.store(true, Ordering::Release);
        }
    }
}

#[cfg(unix)]
fn connect_to_expected_master(
    forward_socket: &Path,
    forward_identity: ForwardSocketIdentity,
    expected_master_pid: u32,
) -> Result<UnixStream, TunnelFailure> {
    if forward_socket_identity(forward_socket) != Ok(forward_identity) {
        return Err(TunnelErrorCode::ForwardSocketReplaced.into());
    }
    let unix =
        UnixStream::connect(forward_socket).map_err(|_| TunnelErrorCode::ForwardConnectFailed)?;
    let (unix, peer_pid) = kernel_peer_pid(unix)?;
    if peer_pid != expected_master_pid {
        return Err(TunnelErrorCode::ForwardPeerMismatch.into());
    }
    if forward_socket_identity(forward_socket) != Ok(forward_identity) {
        return Err(TunnelErrorCode::ForwardSocketReplaced.into());
    }
    Ok(unix)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn kernel_peer_pid(unix: UnixStream) -> Result<(UnixStream, u32), TunnelFailure> {
    unix.set_nonblocking(true)
        .map_err(|_| TunnelErrorCode::ForwardPeerFailed)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .map_err(|_| TunnelErrorCode::ForwardPeerFailed)?;
    let guard = runtime.enter();
    let unix =
        tokio::net::UnixStream::from_std(unix).map_err(|_| TunnelErrorCode::ForwardPeerFailed)?;
    // Tokio maps this to LOCAL_PEEREPID on Apple and SO_PEERCRED on Linux.
    let peer_pid = unix
        .peer_cred()
        .map_err(|_| TunnelErrorCode::ForwardPeerFailed)?
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or(TunnelErrorCode::ForwardPeerFailed)?;
    let unix = unix
        .into_std()
        .map_err(|_| TunnelErrorCode::ForwardPeerFailed)?;
    drop(guard);
    drop(runtime);
    unix.set_nonblocking(false)
        .map_err(|_| TunnelErrorCode::ForwardPeerFailed)?;
    Ok((unix, peer_pid))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn kernel_peer_pid(_unix: UnixStream) -> Result<(UnixStream, u32), TunnelFailure> {
    Err(TunnelErrorCode::ForwardPeerUnsupported.into())
}

#[cfg(unix)]
fn reap_finished_threads(connections: &mut Vec<JoinHandle<()>>) -> bool {
    let mut index = 0;
    while index < connections.len() {
        if connections[index].is_finished() {
            let connection = connections.swap_remove(index);
            if connection.join().is_err() {
                return false;
            }
        } else {
            index += 1;
        }
    }
    true
}

#[cfg(unix)]
fn proxy_connection(
    tcp: TcpStream,
    unix: UnixStream,
    tunnel_stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) {
    let streams = match (
        tcp.try_clone(),
        unix.try_clone(),
        tcp.try_clone(),
        unix.try_clone(),
    ) {
        (Ok(upload_tcp), Ok(upload_unix), Ok(shutdown_tcp), Ok(shutdown_unix)) => {
            (upload_tcp, upload_unix, shutdown_tcp, shutdown_unix)
        }
        _ => {
            failed.store(true, Ordering::Release);
            tunnel_stop.store(true, Ordering::Release);
            let _ = tcp.shutdown(Shutdown::Both);
            let _ = unix.shutdown(Shutdown::Both);
            return;
        }
    };
    for result in [
        tcp.set_read_timeout(Some(PROXY_IO_TIMEOUT)),
        tcp.set_write_timeout(Some(PROXY_IO_TIMEOUT)),
        unix.set_read_timeout(Some(PROXY_IO_TIMEOUT)),
        unix.set_write_timeout(Some(PROXY_IO_TIMEOUT)),
        streams.0.set_read_timeout(Some(PROXY_IO_TIMEOUT)),
        streams.1.set_write_timeout(Some(PROXY_IO_TIMEOUT)),
    ] {
        if result.is_err() {
            failed.store(true, Ordering::Release);
            tunnel_stop.store(true, Ordering::Release);
            let _ = tcp.shutdown(Shutdown::Both);
            let _ = unix.shutdown(Shutdown::Both);
            return;
        }
    }

    let connection_stop = Arc::new(AtomicBool::new(false));
    let upload_stop = Arc::clone(&connection_stop);
    let upload_tunnel_stop = Arc::clone(&tunnel_stop);
    let upload = std::thread::Builder::new()
        .name("fns-ssh-proxy-upload".to_owned())
        .spawn(move || copy_until_stopped(streams.0, streams.1, &upload_tunnel_stop, &upload_stop));
    let Ok(upload) = upload else {
        failed.store(true, Ordering::Release);
        tunnel_stop.store(true, Ordering::Release);
        let _ = tcp.shutdown(Shutdown::Both);
        let _ = unix.shutdown(Shutdown::Both);
        return;
    };

    let _ = copy_until_stopped(unix, tcp, &tunnel_stop, &connection_stop);
    connection_stop.store(true, Ordering::Release);
    let _ = streams.2.shutdown(Shutdown::Both);
    let _ = streams.3.shutdown(Shutdown::Both);
    if upload.join().is_err() {
        failed.store(true, Ordering::Release);
        tunnel_stop.store(true, Ordering::Release);
    }
}

fn copy_until_stopped<R, W>(
    mut reader: R,
    mut writer: W,
    tunnel_stop: &AtomicBool,
    connection_stop: &AtomicBool,
) -> io::Result<()>
where
    R: Read,
    W: Write,
{
    let mut buffer = vec![0_u8; PROXY_BUFFER_BYTES];
    while !tunnel_stop.load(Ordering::Acquire) && !connection_stop.load(Ordering::Acquire) {
        let length = match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                std::thread::sleep(PROXY_POLL_INTERVAL);
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut written = 0;
        while written < length
            && !tunnel_stop.load(Ordering::Acquire)
            && !connection_stop.load(Ordering::Acquire)
        {
            match writer.write(&buffer[written..length]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "proxy write zero")),
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    std::thread::sleep(PROXY_POLL_INTERVAL);
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn terminate_child(child: &mut dyn TunnelChild, timeout: Duration) -> Result<(), TunnelFailure> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(_) => return Err(TunnelErrorCode::WaitFailed.into()),
    }

    let kill_error = child
        .kill()
        .err()
        .map(|_| TunnelFailure::primary(TunnelErrorCode::KillFailed));
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return kill_error.map_or(Ok(()), Err),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(PROXY_POLL_INTERVAL);
            }
            Ok(None) => {
                let failure = TunnelFailure::primary(TunnelErrorCode::WaitTimeout);
                return Err(
                    kill_error.map_or(failure.clone(), |cleanup| failure.with_cleanup(cleanup))
                );
            }
            Err(_) => {
                let failure = TunnelFailure::primary(TunnelErrorCode::WaitFailed);
                return Err(
                    kill_error.map_or(failure.clone(), |cleanup| failure.with_cleanup(cleanup))
                );
            }
        }
    }
}

fn child_is_reaped(child: &mut dyn TunnelChild) -> bool {
    child.try_wait().is_ok_and(|status| status.is_some())
}

fn control_command_failure(
    mut child: Box<dyn TunnelChild>,
    primary: TunnelFailure,
    timeout: Duration,
    retained: &mut Vec<Box<dyn TunnelChild>>,
) -> TunnelFailure {
    match terminate_child(child.as_mut(), timeout) {
        Ok(()) => primary,
        Err(cleanup) => {
            if !child_is_reaped(child.as_mut()) {
                retained.push(child);
            }
            primary.with_cleanup(cleanup)
        }
    }
}

fn finish_failed_construction(
    mut tunnel: SshTunnel,
    primary: TunnelFailure,
    timeout: Duration,
) -> TunnelCreateFailure {
    match tunnel.close_with_timeout(timeout) {
        Ok(()) => TunnelCreateFailure::unowned(primary),
        Err(cleanup) => TunnelCreateFailure {
            failure: primary.with_cleanup(cleanup),
            retained: Some(Box::new(tunnel)),
        },
    }
}

impl SshTunnel {
    fn close_with_timeout(&mut self, timeout: Duration) -> Result<(), TunnelFailure> {
        self.proxy_stop.store(true, Ordering::Release);
        let child_result = if let Some(child) = self.child.as_mut() {
            let result = terminate_child(child.as_mut(), timeout);
            if child_is_reaped(child.as_mut()) {
                self.child.take();
            }
            result
        } else {
            Ok(())
        };

        let mut helper_failure: Option<TunnelFailure> = None;
        let mut retained_helpers = Vec::new();
        for mut helper in self.control_children.drain(..) {
            if let Err(failure) = terminate_child(helper.as_mut(), timeout) {
                helper_failure = Some(
                    helper_failure.map_or(failure.clone(), |primary| primary.with_cleanup(failure)),
                );
            }
            if !child_is_reaped(helper.as_mut()) {
                retained_helpers.push(helper);
            }
        }
        self.control_children = retained_helpers;

        let proxy_result: Result<(), TunnelFailure> = if self
            .proxy_thread
            .as_ref()
            .is_some_and(|proxy| !proxy.is_finished())
        {
            let deadline = Instant::now() + timeout;
            while self
                .proxy_thread
                .as_ref()
                .is_some_and(|proxy| !proxy.is_finished())
                && Instant::now() < deadline
            {
                std::thread::sleep(PROXY_POLL_INTERVAL);
            }
            if self
                .proxy_thread
                .as_ref()
                .is_some_and(|proxy| !proxy.is_finished())
            {
                Err(TunnelErrorCode::ProxyJoinTimeout.into())
            } else {
                self.proxy_thread.take().map_or(Ok(()), |proxy| {
                    proxy
                        .join()
                        .map_err(|_| TunnelErrorCode::ProxyJoinFailed.into())
                })
            }
        } else {
            self.proxy_thread.take().map_or(Ok(()), |proxy| {
                proxy
                    .join()
                    .map_err(|_| TunnelErrorCode::ProxyJoinFailed.into())
            })
        };

        let mut failure = child_result.err();
        if let Some(helper) = helper_failure {
            failure = Some(failure.map_or(helper.clone(), |primary| primary.with_cleanup(helper)));
        }
        if let Err(proxy) = proxy_result {
            failure = Some(failure.map_or(proxy.clone(), |primary| primary.with_cleanup(proxy)));
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        if let Err(error) = self.close_with_timeout(TUNNEL_CLOSE_TIMEOUT) {
            eprintln!("fns_ssh_tunnel_drop_cleanup_failed:{error}");
        }
    }
}

impl TunnelResource for SshTunnel {
    fn local_port(&self) -> u16 {
        SshTunnel::local_port(self)
    }

    fn is_alive(&mut self) -> Result<bool, TunnelFailure> {
        SshTunnel::is_alive(self)
    }

    fn close(&mut self) -> Result<(), TunnelFailure> {
        self.close_with_timeout(TUNNEL_CLOSE_TIMEOUT)
    }
}

struct TunnelEntry {
    ssh_host: String,
    remote_port: u16,
    tunnel: Option<Box<dyn TunnelResource>>,
    #[cfg(test)]
    fixture: Option<(u16, bool)>,
}

#[derive(Default)]
struct TunnelPool {
    entries: HashMap<String, TunnelEntry>,
}

impl TunnelPool {
    fn live_port_for(
        &mut self,
        project_id: &str,
        ssh_host: &str,
        remote_port: Option<u16>,
    ) -> Result<Option<u16>, TunnelFailure> {
        let Some(entry) = self.entries.get_mut(project_id) else {
            return Ok(None);
        };
        if entry.ssh_host != ssh_host
            || remote_port.is_some_and(|remote_port| entry.remote_port != remote_port)
        {
            return Err(TunnelErrorCode::OwnerMismatch.into());
        }
        #[cfg(test)]
        if let Some((port, alive)) = entry.fixture {
            if alive {
                return Ok(Some(port));
            }
            self.entries.remove(project_id);
            return Ok(None);
        }
        let liveness = entry
            .tunnel
            .as_mut()
            .map_or(Ok(false), |tunnel| tunnel.is_alive());
        if liveness == Ok(true) {
            return Ok(entry.tunnel.as_ref().map(|tunnel| tunnel.local_port()));
        }
        if let Some(tunnel) = entry.tunnel.as_mut()
            && let Err(cleanup) = tunnel.close()
        {
            return Err(match liveness {
                Err(primary) => primary.with_cleanup(cleanup),
                Ok(_) => cleanup,
            });
        }
        self.entries.remove(project_id);
        Ok(None)
    }

    fn get_or_create(
        &mut self,
        factory: &dyn TunnelFactory,
        project_id: &str,
        ssh_host: &str,
        remote_port: u16,
    ) -> Result<u16, TunnelFailure> {
        if let Some(port) = self.live_port_for(project_id, ssh_host, Some(remote_port))? {
            return Ok(port);
        }
        let tunnel = match factory.create(project_id, ssh_host, remote_port) {
            Ok(tunnel) => tunnel,
            Err(mut failure) => {
                if let Some(tunnel) = failure.retained.take() {
                    self.entries.insert(
                        project_id.to_owned(),
                        TunnelEntry {
                            ssh_host: ssh_host.to_owned(),
                            remote_port,
                            tunnel: Some(tunnel),
                            #[cfg(test)]
                            fixture: None,
                        },
                    );
                }
                return Err(failure.failure);
            }
        };
        let port = tunnel.local_port();
        self.entries.insert(
            project_id.to_owned(),
            TunnelEntry {
                ssh_host: ssh_host.to_owned(),
                remote_port,
                tunnel: Some(tunnel),
                #[cfg(test)]
                fixture: None,
            },
        );
        Ok(port)
    }

    fn close_all(&mut self) -> Result<(), TunnelFailure> {
        let keys = self.entries.keys().cloned().collect::<Vec<_>>();
        let mut first_failure: Option<TunnelFailure> = None;
        for key in keys {
            let result = self
                .entries
                .get_mut(&key)
                .and_then(|entry| entry.tunnel.as_mut())
                .map_or(Ok(()), |tunnel| tunnel.close());
            match result {
                Ok(()) => {
                    self.entries.remove(&key);
                }
                Err(failure) => {
                    first_failure = Some(
                        first_failure
                            .map_or(failure.clone(), |primary| primary.with_cleanup(failure)),
                    );
                }
            }
        }
        first_failure.map_or(Ok(()), Err)
    }

    #[cfg(test)]
    fn insert_for_test(&mut self, project_id: &str, ssh_host: &str, remote_port: u16, alive: bool) {
        self.entries.insert(
            project_id.to_owned(),
            TunnelEntry {
                ssh_host: ssh_host.to_owned(),
                remote_port,
                tunnel: None,
                fixture: Some((19050, alive)),
            },
        );
    }
}

#[derive(Clone)]
pub struct TunnelState {
    tunnels: Arc<Mutex<TunnelPool>>,
    factory: Arc<dyn TunnelFactory>,
    close_tasks: TaskTracker,
}

impl TunnelState {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_factory(factory: Arc<dyn TunnelFactory>) -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(TunnelPool::default())),
            factory,
            close_tasks: TaskTracker::new(),
        }
    }

    pub fn get_or_create(
        &self,
        project_id: &str,
        ssh_host: &str,
        remote_port: u16,
    ) -> Result<u16, TunnelFailure> {
        self.tunnels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_create(self.factory.as_ref(), project_id, ssh_host, remote_port)
    }

    pub fn close_project(&self, project_id: &str, ssh_host: &str) -> Result<(), TunnelFailure> {
        let mut tunnels = self
            .tunnels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = tunnels.entries.get_mut(project_id) {
            if entry.ssh_host != ssh_host {
                return Err(TunnelErrorCode::OwnerMismatch.into());
            }
            if let Some(tunnel) = entry.tunnel.as_mut() {
                tunnel.close()?;
            }
        }
        tunnels.entries.remove(project_id);
        Ok(())
    }

    pub async fn close_all(&self) -> Result<(), TunnelFailure> {
        let tunnels = Arc::clone(&self.tunnels);
        let (result_tx, result_rx) = oneshot::channel();
        self.close_tasks.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                tunnels
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .close_all()
            })
            .await
            .map_err(|_| TunnelFailure::primary(TunnelErrorCode::WaitFailed))
            .and_then(std::convert::identity);
            let _ = result_tx.send(result);
        });
        match tokio::time::timeout(TUNNEL_CLOSE_ALL_TIMEOUT, result_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TunnelErrorCode::WaitFailed.into()),
            Err(_) => Err(TunnelErrorCode::WaitTimeout.into()),
        }
    }
}

impl Default for TunnelState {
    fn default() -> Self {
        Self::with_factory(Arc::new(SshTunnelFactory))
    }
}

#[tauri::command]
pub fn create_tunnel(
    ssh_alias: String,
    remote_port: Option<u16>,
    state: tauri::State<'_, TunnelState>,
) -> Result<u16, String> {
    let key = format!("onboarding:{ssh_alias}");
    state
        .get_or_create(&key, &ssh_alias, remote_port.unwrap_or(9000))
        .map_err(|failure| failure.stable())
}

#[tauri::command]
pub fn tunnel_endpoint(
    ssh_alias: String,
    state: tauri::State<'_, TunnelState>,
) -> Result<String, String> {
    let key = format!("onboarding:{ssh_alias}");
    let port = state
        .tunnels
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .live_port_for(&key, &ssh_alias, None)
        .map_err(|failure| failure.stable())?
        .ok_or_else(|| "tunnel_not_running".to_owned())?;
    Ok(format!("ws://127.0.0.1:{port}/api/user/workspace-sync/v2"))
}

#[tauri::command]
pub fn close_tunnel(ssh_alias: String, state: tauri::State<'_, TunnelState>) -> Result<(), String> {
    let key = format!("onboarding:{ssh_alias}");
    state
        .close_project(&key, &ssh_alias)
        .map_err(|failure| failure.stable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::atomic::AtomicUsize;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    struct RetryingReader {
        attempts: Arc<AtomicUsize>,
        error_kind: io::ErrorKind,
    }

    impl Read for RetryingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::from(self.error_kind))
        }
    }

    struct SingleByteReader(bool);

    impl Read for SingleByteReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                return Ok(0);
            }
            self.0 = true;
            buffer[0] = 1;
            Ok(1)
        }
    }

    struct RetryingWriter {
        attempts: Arc<AtomicUsize>,
        error_kind: io::ErrorKind,
    }

    impl Write for RetryingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::from(self.error_kind))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn wait_for_proxy_attempt(attempts: &AtomicUsize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while attempts.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_ne!(
            attempts.load(Ordering::SeqCst),
            0,
            "proxy never attempted I/O"
        );
    }

    #[test]
    fn idle_read_retries_are_bounded_and_cancelable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let tunnel_stop = Arc::new(AtomicBool::new(false));
        let connection_stop = Arc::new(AtomicBool::new(false));
        let worker_attempts = Arc::clone(&attempts);
        let worker_tunnel_stop = Arc::clone(&tunnel_stop);
        let worker_connection_stop = Arc::clone(&connection_stop);
        let worker = std::thread::spawn(move || {
            copy_until_stopped(
                RetryingReader {
                    attempts: worker_attempts,
                    error_kind: io::ErrorKind::WouldBlock,
                },
                io::sink(),
                &worker_tunnel_stop,
                &worker_connection_stop,
            )
        });

        wait_for_proxy_attempt(&attempts);
        let initial_attempts = attempts.load(Ordering::SeqCst);
        std::thread::sleep(PROXY_POLL_INTERVAL * 4);
        tunnel_stop.store(true, Ordering::SeqCst);
        worker.join().unwrap().unwrap();

        let retries = attempts.load(Ordering::SeqCst) - initial_attempts;
        assert!(retries <= 8, "idle reader retried {retries} times");
    }

    #[test]
    fn idle_write_retries_are_bounded_and_cancelable() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let tunnel_stop = Arc::new(AtomicBool::new(false));
        let connection_stop = Arc::new(AtomicBool::new(false));
        let worker_attempts = Arc::clone(&attempts);
        let worker_tunnel_stop = Arc::clone(&tunnel_stop);
        let worker_connection_stop = Arc::clone(&connection_stop);
        let worker = std::thread::spawn(move || {
            copy_until_stopped(
                SingleByteReader(false),
                RetryingWriter {
                    attempts: worker_attempts,
                    error_kind: io::ErrorKind::TimedOut,
                },
                &worker_tunnel_stop,
                &worker_connection_stop,
            )
        });

        wait_for_proxy_attempt(&attempts);
        let initial_attempts = attempts.load(Ordering::SeqCst);
        std::thread::sleep(PROXY_POLL_INTERVAL * 4);
        connection_stop.store(true, Ordering::SeqCst);
        worker.join().unwrap().unwrap();

        let retries = attempts.load(Ordering::SeqCst) - initial_attempts;
        assert!(retries <= 8, "idle writer retried {retries} times");
    }

    enum ChildWaitStep {
        Running,
        Reaped,
    }

    struct FixtureChild {
        kill_fails: bool,
        waits: Arc<Mutex<VecDeque<ChildWaitStep>>>,
        reaped: bool,
        dropped_unreaped: Option<Arc<std::sync::atomic::AtomicUsize>>,
    }

    impl Drop for FixtureChild {
        fn drop(&mut self) {
            if !self.reaped
                && let Some(dropped_unreaped) = self.dropped_unreaped.as_ref()
            {
                dropped_unreaped.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    impl TunnelChild for FixtureChild {
        fn id(&self) -> u32 {
            4242
        }

        fn kill(&mut self) -> io::Result<()> {
            if self.kill_fails {
                Err(io::Error::other("fixture kill failure"))
            } else {
                Ok(())
            }
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            #[cfg(unix)]
            if self.reaped {
                return Ok(Some(ExitStatus::from_raw(0)));
            }
            match self
                .waits
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ChildWaitStep::Running)
            {
                ChildWaitStep::Running => Ok(None),
                #[cfg(unix)]
                ChildWaitStep::Reaped => {
                    self.reaped = true;
                    Ok(Some(ExitStatus::from_raw(0)))
                }
                #[cfg(not(unix))]
                ChildWaitStep::Reaped => unreachable!(),
            }
        }
    }

    struct FixtureProxy {
        finished: bool,
        join_fails: bool,
    }

    impl TunnelProxyWorker for FixtureProxy {
        fn is_finished(&self) -> bool {
            self.finished
        }

        fn join(self: Box<Self>) -> Result<(), ()> {
            if self.join_fails { Err(()) } else { Ok(()) }
        }
    }

    fn owned_fixture_tunnel(child: FixtureChild, proxy: FixtureProxy) -> SshTunnel {
        SshTunnel {
            local_port: 19050,
            ssh_program: PathBuf::from("ssh"),
            ssh_alias: "fixture-host".into(),
            control_dir: tempfile::tempdir().unwrap(),
            child: Some(Box::new(child)),
            control_children: Vec::new(),
            proxy_stop: Arc::new(AtomicBool::new(false)),
            proxy_failed: Arc::new(AtomicBool::new(false)),
            proxy_thread: Some(Box::new(proxy)),
        }
    }

    #[test]
    fn ssh_tunnel_struct_exists() {
        assert!(std::mem::needs_drop::<SshTunnel>());
    }

    #[cfg(unix)]
    #[test]
    fn dropping_tunnel_terminates_and_reaps_real_child() {
        let child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let child_pid = child.id();
        let tunnel = SshTunnel {
            local_port: 19050,
            ssh_program: PathBuf::from("ssh"),
            ssh_alias: "fixture-host".into(),
            control_dir: tempfile::tempdir().unwrap(),
            child: Some(Box::new(child)),
            control_children: Vec::new(),
            proxy_stop: Arc::new(AtomicBool::new(false)),
            proxy_failed: Arc::new(AtomicBool::new(false)),
            proxy_thread: Some(Box::new(FixtureProxy {
                finished: true,
                join_fails: false,
            })),
        };

        assert!(
            Command::new("/bin/kill")
                .args(["-0", &child_pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "fixture child was not running before tunnel drop"
        );
        drop(tunnel);
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &child_pid.to_string()])
                .status()
                .unwrap()
                .success(),
            "tunnel child remained alive or unreaped after drop"
        );
    }

    #[test]
    fn explicit_close_is_bounded_fallible_and_retains_unreaped_ownership() {
        let kill_waits = Arc::new(Mutex::new(VecDeque::from([
            ChildWaitStep::Running,
            ChildWaitStep::Reaped,
        ])));
        let mut kill_failure = owned_fixture_tunnel(
            FixtureChild {
                kill_fails: true,
                waits: kill_waits,
                reaped: false,
                dropped_unreaped: None,
            },
            FixtureProxy {
                finished: true,
                join_fails: false,
            },
        );
        assert_eq!(
            kill_failure.close_with_timeout(Duration::from_millis(10)),
            Err(TunnelFailure::primary(TunnelErrorCode::KillFailed))
        );
        assert!(kill_failure.child.is_none(), "reaped child was retained");

        let timeout_waits = Arc::new(Mutex::new(VecDeque::new()));
        let mut wait_timeout = owned_fixture_tunnel(
            FixtureChild {
                kill_fails: false,
                waits: Arc::clone(&timeout_waits),
                reaped: false,
                dropped_unreaped: None,
            },
            FixtureProxy {
                finished: true,
                join_fails: false,
            },
        );
        assert_eq!(
            wait_timeout.close_with_timeout(Duration::from_millis(1)),
            Err(TunnelFailure::primary(TunnelErrorCode::WaitTimeout))
        );
        assert!(
            wait_timeout.child.is_some(),
            "unreaped child ownership was discarded"
        );
        timeout_waits
            .lock()
            .unwrap()
            .push_back(ChildWaitStep::Reaped);
        assert_eq!(
            wait_timeout.close_with_timeout(Duration::from_millis(10)),
            Ok(())
        );
        assert!(wait_timeout.child.is_none());

        let mut join_failure = owned_fixture_tunnel(
            FixtureChild {
                kill_fails: false,
                waits: Arc::new(Mutex::new(VecDeque::from([ChildWaitStep::Reaped]))),
                reaped: false,
                dropped_unreaped: None,
            },
            FixtureProxy {
                finished: true,
                join_fails: true,
            },
        );
        assert_eq!(
            join_failure.close_with_timeout(Duration::from_millis(10)),
            Err(TunnelFailure::primary(TunnelErrorCode::ProxyJoinFailed))
        );
        assert!(join_failure.child.is_none());
        assert!(join_failure.proxy_thread.is_none());
    }

    #[test]
    fn wrong_host_tunnel_reuse_is_rejected() {
        let mut pool = TunnelPool::default();
        pool.insert_for_test("project-a", "host-a", 9000, true);
        assert!(
            pool.live_port_for("project-a", "host-b", Some(9000))
                .is_err()
        );
    }

    #[test]
    fn wrong_remote_port_tunnel_reuse_is_rejected() {
        let mut pool = TunnelPool::default();
        pool.insert_for_test("project-a", "host-a", 9000, true);
        assert!(
            pool.live_port_for("project-a", "host-a", Some(9001))
                .is_err()
        );
    }

    #[test]
    fn dead_tunnel_is_not_reused() {
        let mut pool = TunnelPool::default();
        pool.insert_for_test("project-a", "host-a", 9000, false);
        assert_eq!(
            pool.live_port_for("project-a", "host-a", Some(9000))
                .unwrap(),
            None
        );
    }

    #[test]
    fn constructor_cleanup_failure_retains_the_exact_child_for_retry() {
        let waits = Arc::new(Mutex::new(VecDeque::new()));
        let dropped_unreaped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tunnel = owned_fixture_tunnel(
            FixtureChild {
                kill_fails: false,
                waits: Arc::clone(&waits),
                reaped: false,
                dropped_unreaped: Some(Arc::clone(&dropped_unreaped)),
            },
            FixtureProxy {
                finished: true,
                join_fails: false,
            },
        );

        let mut failure = finish_failed_construction(
            tunnel,
            TunnelFailure::primary(TunnelErrorCode::ForwardFailed),
            Duration::from_millis(1),
        );

        assert_eq!(failure.failure.primary, TunnelErrorCode::ForwardFailed);
        assert_eq!(failure.failure.cleanup, vec![TunnelErrorCode::WaitTimeout]);
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
        waits.lock().unwrap().push_back(ChildWaitStep::Reaped);
        failure.retained.as_mut().unwrap().close().unwrap();
        drop(failure);
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
    }

    struct FailedConstructionFactory {
        failure: Mutex<Option<TunnelCreateFailure>>,
    }

    impl TunnelFactory for FailedConstructionFactory {
        fn create(
            &self,
            _tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn TunnelResource>, TunnelCreateFailure> {
            Err(self
                .failure
                .lock()
                .unwrap()
                .take()
                .expect("constructor failure already consumed"))
        }
    }

    #[test]
    fn constructor_failure_moves_the_unreaped_child_into_tunnel_state() {
        let waits = Arc::new(Mutex::new(VecDeque::new()));
        let dropped_unreaped = Arc::new(AtomicUsize::new(0));
        let failure = finish_failed_construction(
            owned_fixture_tunnel(
                FixtureChild {
                    kill_fails: false,
                    waits: Arc::clone(&waits),
                    reaped: false,
                    dropped_unreaped: Some(Arc::clone(&dropped_unreaped)),
                },
                FixtureProxy {
                    finished: true,
                    join_fails: false,
                },
            ),
            TunnelFailure::primary(TunnelErrorCode::ForwardFailed),
            Duration::from_millis(1),
        );
        let state = TunnelState::with_factory(Arc::new(FailedConstructionFactory {
            failure: Mutex::new(Some(failure)),
        }));

        let failure = state
            .get_or_create("generation-key", "fixture-host", 9000)
            .unwrap_err();
        assert_eq!(failure.primary, TunnelErrorCode::ForwardFailed);
        assert_eq!(failure.cleanup, vec![TunnelErrorCode::WaitTimeout]);
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
        assert!(
            state
                .tunnels
                .lock()
                .unwrap()
                .entries
                .contains_key("generation-key"),
            "TunnelState did not take ownership before returning the failure"
        );

        waits.lock().unwrap().push_back(ChildWaitStep::Reaped);
        state
            .close_project("generation-key", "fixture-host")
            .unwrap();
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
        assert!(state.tunnels.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn control_helper_cleanup_failure_is_owned_until_a_retry_reaps_it() {
        let helper_waits = Arc::new(Mutex::new(VecDeque::new()));
        let dropped_unreaped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut tunnel = owned_fixture_tunnel(
            FixtureChild {
                kill_fails: false,
                waits: Arc::new(Mutex::new(VecDeque::from([ChildWaitStep::Reaped]))),
                reaped: false,
                dropped_unreaped: None,
            },
            FixtureProxy {
                finished: true,
                join_fails: false,
            },
        );
        let helper = Box::new(FixtureChild {
            kill_fails: false,
            waits: Arc::clone(&helper_waits),
            reaped: false,
            dropped_unreaped: Some(Arc::clone(&dropped_unreaped)),
        });

        let failure = control_command_failure(
            helper,
            TunnelFailure::primary(TunnelErrorCode::ControlTimeout),
            Duration::from_millis(1),
            &mut tunnel.control_children,
        );

        assert_eq!(failure.primary, TunnelErrorCode::ControlTimeout);
        assert_eq!(failure.cleanup, vec![TunnelErrorCode::WaitTimeout]);
        assert_eq!(tunnel.control_children.len(), 1);
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
        helper_waits
            .lock()
            .unwrap()
            .push_back(ChildWaitStep::Reaped);
        tunnel
            .close_with_timeout(Duration::from_millis(10))
            .unwrap();
        assert!(tunnel.control_children.is_empty());
        drop(tunnel);
        assert_eq!(dropped_unreaped.load(Ordering::SeqCst), 0);
    }

    struct PoolResourceState {
        close_failures: AtomicUsize,
        close_attempts: AtomicUsize,
        successful_closes: AtomicUsize,
        dropped_unclosed: AtomicUsize,
    }

    struct PoolResource {
        port: u16,
        alive: bool,
        closed: bool,
        state: Arc<PoolResourceState>,
    }

    impl TunnelResource for PoolResource {
        fn local_port(&self) -> u16 {
            self.port
        }

        fn is_alive(&mut self) -> Result<bool, TunnelFailure> {
            Ok(self.alive && !self.closed)
        }

        fn close(&mut self) -> Result<(), TunnelFailure> {
            self.state.close_attempts.fetch_add(1, Ordering::SeqCst);
            if self
                .state
                .close_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(TunnelFailure::primary(TunnelErrorCode::WaitTimeout));
            }
            if !self.closed {
                self.closed = true;
                self.state.successful_closes.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    impl Drop for PoolResource {
        fn drop(&mut self) {
            if !self.closed {
                self.state.dropped_unclosed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct SuccessorFactory {
        creates: AtomicUsize,
        successor: Arc<PoolResourceState>,
    }

    impl TunnelFactory for SuccessorFactory {
        fn create(
            &self,
            _tunnel_key: &str,
            _ssh_host: &str,
            _remote_port: u16,
        ) -> Result<Box<dyn TunnelResource>, TunnelCreateFailure> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(PoolResource {
                port: 19051,
                alive: true,
                closed: false,
                state: Arc::clone(&self.successor),
            }))
        }
    }

    #[test]
    fn liveness_eviction_retains_failed_close_and_rejects_a_successor() {
        let predecessor = Arc::new(PoolResourceState {
            close_failures: AtomicUsize::new(1),
            close_attempts: AtomicUsize::new(0),
            successful_closes: AtomicUsize::new(0),
            dropped_unclosed: AtomicUsize::new(0),
        });
        let successor = Arc::new(PoolResourceState {
            close_failures: AtomicUsize::new(0),
            close_attempts: AtomicUsize::new(0),
            successful_closes: AtomicUsize::new(0),
            dropped_unclosed: AtomicUsize::new(0),
        });
        let factory = SuccessorFactory {
            creates: AtomicUsize::new(0),
            successor: Arc::clone(&successor),
        };
        let mut pool = TunnelPool::default();
        pool.entries.insert(
            "project-a".into(),
            TunnelEntry {
                ssh_host: "host-a".into(),
                remote_port: 9000,
                tunnel: Some(Box::new(PoolResource {
                    port: 19050,
                    alive: false,
                    closed: false,
                    state: Arc::clone(&predecessor),
                })),
                fixture: None,
            },
        );

        let failure = pool
            .get_or_create(&factory, "project-a", "host-a", 9000)
            .unwrap_err();
        assert_eq!(failure.primary, TunnelErrorCode::WaitTimeout);
        assert!(pool.entries.contains_key("project-a"));
        assert_eq!(factory.creates.load(Ordering::SeqCst), 0);
        assert_eq!(predecessor.dropped_unclosed.load(Ordering::SeqCst), 0);

        assert_eq!(
            pool.get_or_create(&factory, "project-a", "host-a", 9000),
            Ok(19051)
        );
        assert_eq!(factory.creates.load(Ordering::SeqCst), 1);
        assert_eq!(predecessor.close_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(predecessor.successful_closes.load(Ordering::SeqCst), 1);
        assert_eq!(predecessor.dropped_unclosed.load(Ordering::SeqCst), 0);
        pool.entries
            .get_mut("project-a")
            .unwrap()
            .tunnel
            .as_mut()
            .unwrap()
            .close()
            .unwrap();
        pool.entries.remove("project-a");
        assert_eq!(successor.successful_closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn correlates_control_check_to_the_exact_master_pid() {
        assert_eq!(
            parse_master_pid(b"", b"Master running (pid=4312)\r\n"),
            Some(4312)
        );
        assert_eq!(parse_master_pid(b"Master running\n", b""), None);
    }

    #[cfg(unix)]
    #[test]
    fn control_check_owns_and_reaps_the_exact_master_child() {
        let fixture = tempfile::tempdir().unwrap();
        let ssh = fixture.path().join("ssh-fixture");
        std::fs::write(
            &ssh,
            r#"#!/bin/sh
control=""
operation=""
expect=""
for argument in "$@"; do
  case "$expect" in
    control) control="$argument" ;;
    operation) operation="$argument" ;;
  esac
  case "$argument" in
    -S) expect="control" ;;
    -O) expect="operation" ;;
    *) expect="" ;;
  esac
done
case "$operation" in
  check)
    test -r "${control}.pid" || exit 1
    pid="$(/bin/cat "${control}.pid")"
    echo "Master running (pid=${pid})" >&2
    ;;
  *)
    printf '%s\n' "$$" > "${control}.pid"
    exec /bin/sleep 86400
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&ssh).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&ssh, permissions).unwrap();

        let control_path = fixture.path().join("master.sock");
        let mut child = spawn_master(&ssh, "fixture-host", &control_path).unwrap();
        wait_for_master(
            &ssh,
            "fixture-host",
            &control_path,
            fixture.path(),
            &mut child,
            &mut Vec::new(),
        )
        .unwrap();
        let child_pid = child.id();
        terminate_child(&mut child, TUNNEL_CLOSE_TIMEOUT).unwrap();

        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &child_pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
            "SSH master was not reaped"
        );
    }

    #[cfg(unix)]
    #[test]
    fn owned_loopback_proxy_forwards_bytes_and_stops_without_threads() {
        let fixture = tempfile::tempdir().unwrap();
        let forward_socket = fixture.path().join("forward.sock");
        let unix_listener = UnixListener::bind(&forward_socket).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let proxy = start_proxy(
            listener,
            forward_socket.clone(),
            forward_socket_identity(&forward_socket).unwrap(),
            std::process::id(),
            Arc::clone(&stop),
            Arc::clone(&failed),
        )
        .unwrap();
        assert!(TcpListener::bind(address).is_err());

        let server = std::thread::spawn(move || {
            let (mut stream, _) = unix_listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.write_all(b"ping").unwrap();
        let mut response = [0_u8; 4];
        client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        drop(client);
        server.join().unwrap();

        stop.store(true, Ordering::Release);
        proxy.join().unwrap();
        assert!(!failed.load(Ordering::Acquire));
        drop(TcpListener::bind(address).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn proxy_rejects_replaced_forward_socket_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let forward_socket = fixture.path().join("forward.sock");
        let moved_socket = fixture.path().join("original-forward.sock");
        let original = UnixListener::bind(&forward_socket).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let proxy = start_proxy(
            listener,
            forward_socket.clone(),
            forward_socket_identity(&forward_socket).unwrap(),
            std::process::id(),
            Arc::clone(&stop),
            Arc::clone(&failed),
        )
        .unwrap();

        std::fs::rename(&forward_socket, &moved_socket).unwrap();
        let replacement = UnixListener::bind(&forward_socket).unwrap();
        let client = TcpStream::connect(address).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !failed.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(PROXY_POLL_INTERVAL);
        }
        assert!(failed.load(Ordering::Acquire));

        drop(client);
        drop(replacement);
        drop(original);
        stop.store(true, Ordering::Release);
        proxy.join().unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn forward_socket_connection_rejects_the_wrong_kernel_peer_pid() {
        let fixture = tempfile::tempdir().unwrap();
        let forward_socket = fixture.path().join("forward.sock");
        let _listener = UnixListener::bind(&forward_socket).unwrap();
        let identity = forward_socket_identity(&forward_socket).unwrap();
        let wrong_pid = std::process::id().checked_add(1).unwrap();

        let error = connect_to_expected_master(&forward_socket, identity, wrong_pid).unwrap_err();

        assert_eq!(
            error,
            TunnelFailure::primary(TunnelErrorCode::ForwardPeerMismatch)
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires FNS_TEST_SSH_ALIAS and a real remote service"]
    fn real_ssh_forward_reaches_remote_service_through_owned_listener() {
        let ssh_alias = std::env::var("FNS_TEST_SSH_ALIAS").expect("FNS_TEST_SSH_ALIAS");
        let remote_port = std::env::var("FNS_TEST_REMOTE_PORT")
            .unwrap_or_else(|_| "9000".to_owned())
            .parse::<u16>()
            .expect("FNS_TEST_REMOTE_PORT");
        let tunnel = SshTunnel::create(&ssh_alias, remote_port).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        client
            .write_all(
                b"GET /api/user/workspace-sync/v2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        let mut response = [0_u8; 1024];
        let length = client.read(&mut response).unwrap();
        assert!(response[..length].starts_with(b"HTTP/1.1 "));
        drop(client);
        drop(tunnel);
    }
}
