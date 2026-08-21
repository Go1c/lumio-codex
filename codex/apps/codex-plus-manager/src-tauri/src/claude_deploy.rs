//! Deploy sync components to the remote host (not mkdir-only).

use crate::claude_files::expand_local_root;
use crate::claude_ssh::{
    ResolvedSshTarget, remote_prepare_mkdir, remote_shell_path, resolve_from_user_config,
    ssh_invocation_args,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const PREPARE_PROGRESS_EVENT: &str = "lumio://claude-prepare-progress";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareOutcome {
    pub ok: bool,
    pub error_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareProgress {
    pub phase: String,
    pub step: u32,
    pub total: u32,
    pub detail: String,
}

impl PrepareProgress {
    pub fn mkdir() -> Self {
        Self {
            phase: "mkdir".into(),
            step: 2,
            total: 4,
            detail: "正在服务器上创建项目目录…".into(),
        }
    }

    pub fn upload(index: u32) -> Self {
        Self {
            phase: "upload".into(),
            step: 3,
            total: 4,
            detail: format!("正在把同步组件传到服务器（{index} / 2）…"),
        }
    }

    pub fn finish() -> Self {
        Self {
            phase: "finish".into(),
            step: 4,
            total: 4,
            detail: "正在启动同步组件并保持运行…".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ArtifactPaths {
    pub server: PathBuf,
    pub agent: PathBuf,
}

pub fn find_artifacts(resource_dir: Option<&Path>) -> Option<ArtifactPaths> {
    if let Some(explicit) = std::env::var_os("BESTCODEX_CLAUDE_REMOTE_DIR") {
        let root = PathBuf::from(explicit);
        let paths = ArtifactPaths {
            server: root.join("fns-server"),
            agent: root.join("fns-agent"),
        };
        if is_real_artifact(&paths.server) && is_real_artifact(&paths.agent) {
            return Some(paths);
        }
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = resource_dir {
        candidates.push(dir.join("remote").join("linux-x86_64"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("remote").join("linux-x86_64"));
            candidates.push(parent.join("resources").join("remote").join("linux-x86_64"));
        }
    }
    for root in candidates {
        let paths = ArtifactPaths {
            server: root.join("fns-server"),
            agent: root.join("fns-agent"),
        };
        if is_real_artifact(&paths.server) && is_real_artifact(&paths.agent) {
            return Some(paths);
        }
    }
    None
}

fn is_real_artifact(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.len() > 1024)
        .unwrap_or(false)
}

pub fn human_prepare_detail(code: &str, host: &str, port: u16) -> String {
    match code {
        "DEPLOY_ARTIFACT_MISSING" => {
            "这个版本的 BestCodex 没有把同步组件打进来，不是服务器的问题。更新或重装 BestCodex 后再试。".into()
        }
        "SSH_AUTH_FAILED" => format!("无法登录 {host}。"),
        "SSH_UNREACHABLE" => format!("连不上 {host}:{port}。"),
        "SSH_ALIAS_UNKNOWN" => "本机 SSH 配置里没有这个 Host 别名。".into(),
        "SSH_HOST_REQUIRED" => "先填写公网 IP。".into(),
        _ => "没能在服务器上装好同步组件。".into(),
    }
}

pub fn inspect_remote_script(remote_root: &str) -> String {
    format!(
        "if [ -d {root} ]; then echo EXISTS:1; else echo EXISTS:0; fi; if [ -d {parent} ]; then ls -1 {parent}; fi; agent=$HOME/.local/share/bestcodex/bin/fns-agent; server=$HOME/.local/share/bestcodex/bin/fns-server; if [ -f $agent ] && [ -f $server ]; then echo COMPONENTS:1; else echo COMPONENTS:0; fi",
        root = crate::claude_ssh::remote_shell_path(remote_root),
        parent = crate::claude_ssh::remote_shell_path("~/bestcodex"),
    )
}

pub struct InspectReport {
    pub exists: bool,
    pub names: Vec<String>,
    pub components_installed: bool,
}

pub fn parse_inspect_output(stdout: &str) -> InspectReport {
    let mut exists = false;
    let mut components_installed = false;
    let mut names = Vec::new();
    for line in stdout.lines() {
        if let Some(flag) = line.strip_prefix("EXISTS:") {
            exists = flag.trim() == "1";
            continue;
        }
        if let Some(flag) = line.strip_prefix("COMPONENTS:") {
            components_installed = flag.trim() == "1";
            continue;
        }
        let name = line.trim();
        if name.is_empty() || name.starts_with('.') || name.contains('/') {
            continue;
        }
        names.push(name.to_string());
    }
    InspectReport {
        exists,
        names,
        components_installed,
    }
}

pub fn stop_sync_for_replace_script() -> String {
    "set -u
bin=$HOME/.local/share/bestcodex/bin
state_root=$HOME/.local/share/bestcodex/state
for watchdog_pid_file in $state_root/watchdog.pid $state_root/workspaces/*/watchdog.pid; do
  [ -f $watchdog_pid_file ] || continue
  watchdog_state=${watchdog_pid_file%/watchdog.pid}
  watchdog_pid=$(cat $watchdog_pid_file 2>/dev/null || true)
  if [ x$watchdog_pid != x ] && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $watchdog_state/watchdog.sh >/dev/null 2>&1; then
    kill $watchdog_pid 2>/dev/null || true
    watchdog_stop_attempt=0
    while [ $watchdog_stop_attempt -lt 10 ] && kill -0 $watchdog_pid 2>/dev/null && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $watchdog_state/watchdog.sh >/dev/null 2>&1; do
      watchdog_stop_attempt=$((watchdog_stop_attempt + 1))
      sleep 1
    done
    if kill -0 $watchdog_pid 2>/dev/null && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $watchdog_state/watchdog.sh >/dev/null 2>&1; then
      kill -KILL $watchdog_pid 2>/dev/null || true
      sleep 1
    fi
  fi
  rm -f $watchdog_pid_file
done
if [ $(id -u) -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemctl stop bestcodex-sync.service || true
  for unit_file in /etc/systemd/system/bestcodex-sync-*.service; do
    [ -f $unit_file ] || continue
    unit=${unit_file##*/}
    systemctl stop $unit || true
  done
  systemctl stop bestcodex-workspace.service || true
elif command -v systemctl >/dev/null 2>&1; then
  systemctl --user stop bestcodex-sync.service || true
  for unit_file in $HOME/.config/systemd/user/bestcodex-sync-*.service; do
    [ -f $unit_file ] || continue
    unit=${unit_file##*/}
    systemctl --user stop $unit || true
  done
  systemctl --user stop bestcodex-workspace.service || true
fi
for proc_exe in /proc/[0-9]*/exe; do
  [ -L $proc_exe ] || continue
  target=$(readlink $proc_exe 2>/dev/null || true)
  case x$target in
    x$bin/fns-agent|x$bin/fns-server)
      pid=${proc_exe#/proc/}
      kill ${pid%/exe} 2>/dev/null || true
      ;;
  esac
done
binary_stop_attempt=0
while [ $binary_stop_attempt -lt 10 ]; do
  binary_running=0
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    case x$target in
      x$bin/fns-agent|x$bin/fns-server)
        binary_running=1
        pid=${proc_exe#/proc/}
        kill ${pid%/exe} 2>/dev/null || true
        ;;
    esac
  done
  if [ $binary_running -eq 0 ]; then exit 0; fi
  binary_stop_attempt=$((binary_stop_attempt + 1))
  sleep 1
done
exit 1
"
    .into()
}

pub fn prepare_components(
    host: &str,
    user: &str,
    port: u16,
    alias: Option<&str>,
    local_root: &str,
    artifacts: Option<&ArtifactPaths>,
) -> PrepareOutcome {
    if let Err(code) = resolve_from_user_config(host, Some(user), port, alias) {
        return PrepareOutcome {
            ok: false,
            error_code: Some(code.into()),
            detail: Some(human_prepare_detail(code, host, port)),
        };
    }
    let local = expand_local_root(local_root);
    if std::fs::create_dir_all(&local).is_err() {
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能创建本机项目目录。".into()),
        };
    }
    if artifacts.is_none() {
        return PrepareOutcome {
            ok: false,
            error_code: Some("DEPLOY_ARTIFACT_MISSING".into()),
            detail: Some(human_prepare_detail("DEPLOY_ARTIFACT_MISSING", host, port)),
        };
    }
    PrepareOutcome {
        ok: true,
        error_code: None,
        detail: None,
    }
}

pub fn deploy_remote(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    remote_root: &str,
    artifacts: &ArtifactPaths,
    replace: bool,
    run_ssh: impl Fn(&str) -> Result<std::process::Output, &'static str>,
    on_progress: impl FnMut(PrepareProgress),
) -> PrepareOutcome {
    deploy_remote_with_copy(
        remote_root,
        artifacts,
        replace,
        run_ssh,
        |local, remote| scp_file(target, password, key_path, local, remote),
        on_progress,
    )
}

fn deploy_remote_with_copy(
    remote_root: &str,
    artifacts: &ArtifactPaths,
    replace: bool,
    run_ssh: impl Fn(&str) -> Result<std::process::Output, &'static str>,
    copy_file: impl Fn(&Path, &str) -> Result<(), &'static str>,
    mut on_progress: impl FnMut(PrepareProgress),
) -> PrepareOutcome {
    on_progress(PrepareProgress::mkdir());
    let mkdir = remote_prepare_mkdir(remote_root);
    let mkdir_result = run_ssh(&mkdir);
    match mkdir_result {
        Ok(output) if output.status.success() => {}
        failed => {
            log_ssh_failure("mkdir", &failed);
            return PrepareOutcome {
                ok: false,
                error_code: Some("SSH_PREPARE_FAILED".into()),
                detail: Some("没能在服务器上建好项目目录。".into()),
            };
        }
    }

    if replace {
        let stop_result = run_ssh(&stop_sync_for_replace_script());
        if !matches!(&stop_result, Ok(output) if output.status.success()) {
            log_ssh_failure("replace-stop", &stop_result);
            return PrepareOutcome {
                ok: false,
                error_code: Some("SSH_PREPARE_FAILED".into()),
                detail: Some("没能在服务器上装好同步组件。".into()),
            };
        }
    }

    on_progress(PrepareProgress::upload(1));
    if copy_file(
        &artifacts.server,
        "~/.local/share/bestcodex/bin/fns-server.new",
    )
    .is_err()
    {
        if replace {
            recover_after_failed_replace(&run_ssh);
        }
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能把同步组件传到服务器。".into()),
        };
    }
    on_progress(PrepareProgress::upload(2));
    if copy_file(
        &artifacts.agent,
        "~/.local/share/bestcodex/bin/fns-agent.new",
    )
    .is_err()
    {
        if replace {
            recover_after_failed_replace(&run_ssh);
        }
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能把同步组件传到服务器。".into()),
        };
    }

    on_progress(PrepareProgress::finish());
    let activate_result = run_ssh(&activate_staged_components_script());
    if !matches!(&activate_result, Ok(output) if output.status.success()) {
        log_ssh_failure("activate", &activate_result);
        let rolled_back = rollback_staged_components(&run_ssh);
        if replace && rolled_back {
            recover_after_failed_replace(&run_ssh);
        }
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能在服务器上装好同步组件。".into()),
        };
    }
    let start = keep_sync_running_script(remote_root);
    let start_result = run_ssh(&start);
    match start_result {
        Ok(output) if output.status.success() => PrepareOutcome {
            ok: true,
            error_code: None,
            detail: None,
        },
        failed => {
            log_ssh_failure("start", &failed);
            let stopped = stop_components_after_failed_start(&run_ssh);
            let rolled_back = stopped && rollback_staged_components(&run_ssh);
            if replace && rolled_back {
                recover_after_failed_replace(&run_ssh);
            }
            PrepareOutcome {
                ok: false,
                error_code: Some("SSH_PREPARE_FAILED".into()),
                detail: Some("没能在服务器上装好同步组件。".into()),
            }
        }
    }
}

fn stop_components_after_failed_start(
    run_ssh: &impl Fn(&str) -> Result<std::process::Output, &'static str>,
) -> bool {
    let stop = run_ssh(&stop_sync_for_replace_script());
    if matches!(&stop, Ok(output) if output.status.success()) {
        true
    } else {
        log_ssh_failure("replace-failed-start-stop", &stop);
        false
    }
}

fn rollback_staged_components(
    run_ssh: &impl Fn(&str) -> Result<std::process::Output, &'static str>,
) -> bool {
    let rollback = run_ssh(&rollback_staged_components_script());
    if matches!(&rollback, Ok(output) if output.status.success()) {
        true
    } else {
        log_ssh_failure("replace-rollback", &rollback);
        false
    }
}

fn recover_after_failed_replace(
    run_ssh: &impl Fn(&str) -> Result<std::process::Output, &'static str>,
) {
    let recovery = run_ssh(&restart_sync_after_failed_replace_script());
    if !matches!(&recovery, Ok(output) if output.status.success()) {
        log_ssh_failure("replace-recover", &recovery);
    }
}

pub fn activate_staged_components_script() -> String {
    "set -eu
bin=$HOME/.local/share/bestcodex/bin
marker=$bin/.bestcodex-replace-active
[ -f $bin/fns-server.new ]
[ -f $bin/fns-agent.new ]
chmod 0755 $bin/fns-server.new $bin/fns-agent.new
rollback_active() {
  rm -f $bin/fns-server $bin/fns-agent
  if [ -f $bin/fns-server.previous ]; then mv $bin/fns-server.previous $bin/fns-server; fi
  if [ -f $bin/fns-agent.previous ]; then mv $bin/fns-agent.previous $bin/fns-agent; fi
  rm -f $marker
}
if [ -f $marker ]; then
  rollback_active
else
  if [ ! -f $bin/fns-server ] && [ -f $bin/fns-server.previous ]; then mv $bin/fns-server.previous $bin/fns-server; fi
  if [ ! -f $bin/fns-agent ] && [ -f $bin/fns-agent.previous ]; then mv $bin/fns-agent.previous $bin/fns-agent; fi
fi
rm -f $bin/fns-server.previous $bin/fns-agent.previous
if [ -f $bin/fns-server ] && ! mv $bin/fns-server $bin/fns-server.previous; then
  exit 1
fi
if [ -f $bin/fns-agent ] && ! mv $bin/fns-agent $bin/fns-agent.previous; then
  if [ -f $bin/fns-server.previous ]; then mv $bin/fns-server.previous $bin/fns-server; fi
  exit 1
fi
: > $marker
if ! mv $bin/fns-server.new $bin/fns-server; then
  rollback_active
  exit 1
fi
if ! mv $bin/fns-agent.new $bin/fns-agent; then
  rollback_active
  exit 1
fi
"
    .into()
}

pub fn rollback_staged_components_script() -> String {
    "set -eu
bin=$HOME/.local/share/bestcodex/bin
marker=$bin/.bestcodex-replace-active
if [ -f $marker ]; then
  rm -f $bin/fns-server $bin/fns-agent
  if [ -f $bin/fns-server.previous ]; then mv $bin/fns-server.previous $bin/fns-server; fi
  if [ -f $bin/fns-agent.previous ]; then mv $bin/fns-agent.previous $bin/fns-agent; fi
  rm -f $marker
else
  if [ ! -f $bin/fns-server ] && [ -f $bin/fns-server.previous ]; then mv $bin/fns-server.previous $bin/fns-server; fi
  if [ ! -f $bin/fns-agent ] && [ -f $bin/fns-agent.previous ]; then mv $bin/fns-agent.previous $bin/fns-agent; fi
fi
rm -f $bin/fns-server.new $bin/fns-agent.new
"
    .into()
}

pub fn restart_sync_after_failed_replace_script() -> String {
    "set -u
home=$HOME
bin=$home/.local/share/bestcodex/bin
state_root=$home/.local/share/bestcodex/state
watchdog_required=0
if [ $(id -u) -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemctl disable --now bestcodex-sync.service || true
  systemctl start bestcodex-workspace.service || true
  for unit_file in /etc/systemd/system/bestcodex-sync-*.service; do
    [ -f $unit_file ] || continue
    unit=${unit_file##*/}
    systemctl start $unit || true
  done
elif command -v systemctl >/dev/null 2>&1; then
  systemctl --user disable --now bestcodex-sync.service || true
  if command -v loginctl >/dev/null 2>&1 && loginctl show-user $(id -u) -p Linger 2>/dev/null | grep -Fx Linger=yes >/dev/null; then
    systemctl --user start bestcodex-workspace.service || true
    for unit_file in $home/.config/systemd/user/bestcodex-sync-*.service; do
      [ -f $unit_file ] || continue
      unit=${unit_file##*/}
      systemctl --user start $unit || true
    done
  else
    systemctl --user stop bestcodex-sync.service bestcodex-workspace.service || true
    for unit_file in $home/.config/systemd/user/bestcodex-sync-*.service; do
      [ -f $unit_file ] || continue
      unit=${unit_file##*/}
      systemctl --user stop $unit || true
    done
    watchdog_required=1
  fi
fi
sleep 2
process_pid() {
  wanted=$1
  required_arg=${2-}
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    case x$target in
      x$wanted)
        pid=${proc_exe#/proc/}
        pid=${pid%/exe}
        if [ x$required_arg = x ] || { [ -r /proc/$pid/cmdline ] && tr '\000' '\n' < /proc/$pid/cmdline | grep -Fx $required_arg >/dev/null 2>&1; }; then
          echo $pid
          return 0
        fi
        ;;
    esac
  done
  return 1
}
for watchdog_script in $state_root/workspaces/*/watchdog.sh; do
  [ -f $watchdog_script ] || continue
  watchdog_state=${watchdog_script%/watchdog.sh}
  if [ $watchdog_required -eq 0 ] && process_pid $bin/fns-server >/dev/null && process_pid $bin/fns-agent $watchdog_state/agent.json >/dev/null; then continue; fi
  watchdog_running=0
  if [ -f $watchdog_state/watchdog.pid ]; then
    watchdog_pid=$(cat $watchdog_state/watchdog.pid 2>/dev/null || true)
    if [ x$watchdog_pid != x ] && [ -r /proc/$watchdog_pid/cmdline ] && tr '\000' '\n' < /proc/$watchdog_pid/cmdline | grep -Fx $watchdog_script >/dev/null 2>&1; then watchdog_running=1; fi
  fi
  if [ $watchdog_running -eq 0 ]; then
    nohup sh $watchdog_script >/dev/null 2>>$watchdog_state/watchdog.stderr.log &
    echo $! > $watchdog_state/watchdog.pid
  fi
done
exit 0
"
    .into()
}

fn format_ssh_failure_log(stage: &str, exit: Option<i32>, stderr: &[u8]) -> String {
    let tail_start = stderr.len().saturating_sub(4096);
    let mut bounded = String::from_utf8_lossy(&stderr[tail_start..]).into_owned();
    if bounded.len() > 4096 {
        let mut end = 4096;
        while !bounded.is_char_boundary(end) {
            end -= 1;
        }
        bounded.truncate(end);
    }
    format!(
        "[claude-deploy] stage={stage} exit={} stderr={bounded}",
        exit.map_or_else(|| "unknown".into(), |code| code.to_string())
    )
}

fn log_ssh_failure(stage: &str, result: &Result<std::process::Output, &'static str>) {
    match result {
        Ok(output) => eprintln!(
            "{}",
            format_ssh_failure_log(stage, output.status.code(), &output.stderr)
        ),
        Err(code) => eprintln!("[claude-deploy] stage={stage} exit=unavailable error={code}"),
    }
}

fn scp_invocation_args(
    target: &ResolvedSshTarget,
    key_path: Option<&str>,
    source: &Path,
    destination: &str,
) -> Result<Vec<String>, &'static str> {
    let key = crate::claude_ssh::effective_key_path(key_path, target);
    let mut args = ssh_invocation_args(target, key, None);
    // ssh_invocation_args ends with the destination host; scp wants host:path.
    let host = args.pop().ok_or("SSH_PREPARE_FAILED")?;
    // ssh uses -p for port; scp uses -P. scp -p means preserve times and
    // treats the port number as another source file.
    for arg in &mut args {
        if arg == "-p" {
            *arg = "-P".into();
        }
    }
    args.push(source.to_string_lossy().into_owned());
    args.push(format!("{host}:{destination}"));
    Ok(args)
}

fn scp_file(
    target: &ResolvedSshTarget,
    password: Option<&str>,
    key_path: Option<&str>,
    source: &Path,
    destination: &str,
) -> Result<(), &'static str> {
    let args = scp_invocation_args(target, key_path, source, destination)?;
    let key = crate::claude_ssh::effective_key_path(key_path, target);
    let mut command = Command::new("scp");
    command.args(&args);
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    let askpass =
        crate::claude_ssh::attach_askpass(&mut command, password, key, target.use_config)?;
    let output = command.output().map_err(|_| "SSH_CLIENT_MISSING")?;
    drop(askpass);
    if output.status.success() {
        Ok(())
    } else {
        log_ssh_failure("scp", &Ok(output));
        Err("SSH_PREPARE_FAILED")
    }
}

fn fnv1a64(data: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in data.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn uuid_from_tag(tag: &str, remote_root: &str) -> String {
    let hashed = fnv1a64(&format!("{tag}:{remote_root}")) & 0x0000_ffff_ffff_ffff;
    format!("6b657374-c0de-4000-8000-{hashed:012x}")
}

pub fn sync_workspace_id(remote_root: &str) -> String {
    uuid_from_tag("ws", remote_root)
}

pub fn sync_client_id(side: &str, remote_root: &str) -> String {
    uuid_from_tag(side, remote_root)
}

pub fn remote_sync_token_script(remote_root: &str) -> String {
    format!(
        "cat $HOME/.local/share/bestcodex/state/workspaces/{}/token",
        sync_workspace_id(remote_root)
    )
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut index = 0;
    while index < input.len() {
        let first = input[index];
        let second = input.get(index + 1).copied().unwrap_or(0);
        let third = input.get(index + 2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if index + 1 < input.len() {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if index + 2 < input.len() {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        index += 3;
    }
    out
}

pub fn remote_sync_unit(wanted_by: &str) -> String {
    format!(
        "[Unit]\nDescription=BestCodex file sync\nAfter=network-online.target bestcodex-workspace.service\nWants=network-online.target\n\n[Service]\nType=simple\nRestart=always\nRestartSec=2\nWorkingDirectory=ROOT_PLACEHOLDER\nExecStart=HOME_PLACEHOLDER/.local/share/bestcodex/bin/fns-agent run --config STATE_PLACEHOLDER/agent.json\nNoNewPrivileges=true\n\n[Install]\nWantedBy={wanted_by}\n"
    )
}

pub fn remote_workspace_unit(wanted_by: &str) -> String {
    format!(
        "[Unit]\nDescription=BestCodex remote workspace\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nRestart=always\nRestartSec=2\nWorkingDirectory=HOME_PLACEHOLDER/.local/share/bestcodex/server\nExecStart=HOME_PLACEHOLDER/.local/share/bestcodex/bin/fns-server run --config HOME_PLACEHOLDER/.local/share/bestcodex/server/config/config.yaml\nNoNewPrivileges=true\n\n[Install]\nWantedBy={wanted_by}\n"
    )
}

pub fn keep_sync_running_script(remote_root: &str) -> String {
    let root = remote_shell_path(remote_root);
    let workspace_id = sync_workspace_id(remote_root);
    let client_id = sync_client_id("remote", remote_root);
    let config = format!(
        "{{\n  \"schemaVersion\": \"fns-agent-config/1\",\n  \"endpoint\": \"ws://127.0.0.1:9000/api/user/workspace-sync/v2\",\n  \"workspaceId\": \"{workspace_id}\",\n  \"clientId\": \"{client_id}\",\n  \"workspaceRoot\": \"ROOT_PLACEHOLDER\",\n  \"stateDir\": \"STATE_PLACEHOLDER\",\n  \"tokenFile\": \"STATE_PLACEHOLDER/token\",\n  \"sync\": {{\n    \"includes\": [\"**/*\"],\n    \"excludes\": [],\n    \"protectSecrets\": true\n  }},\n  \"transport\": {{ \"maxActiveTransfers\": 2 }}\n}}\n"
    );
    let config_b64 = base64_encode(config.as_bytes());
    let sync_system_b64 = base64_encode(remote_sync_unit("multi-user.target").as_bytes());
    let sync_user_b64 = base64_encode(remote_sync_unit("default.target").as_bytes());
    let workspace_system_b64 = base64_encode(remote_workspace_unit("multi-user.target").as_bytes());
    let workspace_user_b64 = base64_encode(remote_workspace_unit("default.target").as_bytes());
    let watchdog_b64 =
        base64_encode(include_str!("../../../../scripts/sync-components/watchdog.sh").as_bytes());
    format!(
        "set -eu
home=$HOME
bin=$home/.local/share/bestcodex/bin
state_root=$home/.local/share/bestcodex/state
state=$state_root/workspaces/{workspace_id}
sync_unit=bestcodex-sync-{workspace_id}.service
server_dir=$home/.local/share/bestcodex/server
root={root}
require_all=0
if [ -f $bin/.bestcodex-replace-active ]; then require_all=1; fi
mkdir -p $root $state $server_dir $home/.config/systemd/user
chmod 0700 $state_root $state
chmod 0755 $bin/fns-server $bin/fns-agent
root=$(cd $root && pwd)
$bin/fns-server bootstrap-workspace --config $server_dir/config/config.yaml --token-file $state/token --workspace-id {workspace_id} --workspace-root $root
chmod 0600 $state/token
printf %s {config_b64} | base64 -d | sed s#ROOT_PLACEHOLDER#$root#g | sed s#STATE_PLACEHOLDER#$state#g > $state/agent.json
chmod 0600 $state/agent.json
printf %s {watchdog_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#STATE_PLACEHOLDER#$state#g | sed s#ROOT_PLACEHOLDER#$root#g | sed s#PORT_PLACEHOLDER#9000#g > $state/watchdog.sh
chmod 0700 $state/watchdog.sh
if grep -R PLACEHOLDER $state/agent.json $state/watchdog.sh >/dev/null; then exit 1; fi
systemd_scope=none
watchdog_required=0
if [ $(id -u) -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemd_scope=system
  mkdir -p /etc/systemd/system
  systemctl disable --now bestcodex-sync.service || true
  printf %s {workspace_system_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g > /etc/systemd/system/bestcodex-workspace.service
  printf %s {sync_system_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#STATE_PLACEHOLDER#$state#g | sed s#ROOT_PLACEHOLDER#$root#g > /etc/systemd/system/$sync_unit
  if grep -R PLACEHOLDER $state/agent.json /etc/systemd/system/bestcodex-workspace.service /etc/systemd/system/$sync_unit >/dev/null; then exit 1; fi
  systemctl daemon-reload || true
  systemctl enable --now bestcodex-workspace.service || true
  for unit_file in /etc/systemd/system/bestcodex-sync-*.service; do
    [ -f $unit_file ] || continue
    unit=${{unit_file##*/}}
    systemctl enable --now $unit || true
  done
  systemctl restart bestcodex-workspace.service $sync_unit || true
elif command -v systemctl >/dev/null 2>&1; then
  systemd_scope=user
  systemctl --user disable --now bestcodex-sync.service || true
  printf %s {workspace_user_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g > $home/.config/systemd/user/bestcodex-workspace.service
  printf %s {sync_user_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#STATE_PLACEHOLDER#$state#g | sed s#ROOT_PLACEHOLDER#$root#g > $home/.config/systemd/user/$sync_unit
  if grep -R PLACEHOLDER $state/agent.json $home/.config/systemd/user/bestcodex-workspace.service $home/.config/systemd/user/$sync_unit >/dev/null; then exit 1; fi
  systemctl --user daemon-reload || true
  systemctl --user enable --now bestcodex-workspace.service || true
  for unit_file in $home/.config/systemd/user/bestcodex-sync-*.service; do
    [ -f $unit_file ] || continue
    unit=${{unit_file##*/}}
    systemctl --user enable --now $unit || true
  done
  systemctl --user restart bestcodex-workspace.service $sync_unit || true
  if ! command -v loginctl >/dev/null 2>&1 || ! loginctl show-user $(id -u) -p Linger 2>/dev/null | grep -Fx Linger=yes >/dev/null; then
    for unit_file in $home/.config/systemd/user/bestcodex-sync-*.service; do
      [ -f $unit_file ] || continue
      unit=${{unit_file##*/}}
      systemctl --user disable --now $unit || true
      rm -f $home/.config/systemd/user/default.target.wants/$unit
    done
    systemctl --user disable --now bestcodex-workspace.service || true
    rm -f $home/.config/systemd/user/default.target.wants/bestcodex-workspace.service
    systemd_scope=none
    watchdog_required=1
  fi
fi
process_pid() {{
  wanted=$1
  required_arg=${{2-}}
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    case x$target in
      x$wanted)
        pid=${{proc_exe#/proc/}}
        pid=${{pid%/exe}}
        if [ x$required_arg = x ] || {{ [ -r /proc/$pid/cmdline ] && tr '\\000' '\\n' < /proc/$pid/cmdline | grep -Fx $required_arg >/dev/null 2>&1; }}; then
          echo $pid
          return 0
        fi
        ;;
    esac
  done
  return 1
}}
all_scoped_agents_running() {{
  found=0
  for agent_config in $state_root/workspaces/*/agent.json; do
    [ -f $agent_config ] || continue
    found=1
    if ! process_pid $bin/fns-agent $agent_config >/dev/null; then return 1; fi
  done
  [ $found -eq 1 ]
}}
required_agents_running() {{
  if [ $require_all -eq 1 ]; then
    all_scoped_agents_running
  else
    process_pid $bin/fns-agent $state/agent.json >/dev/null
  fi
}}
if [ -f $state_root/watchdog.pid ]; then
  legacy_watchdog_pid=$(cat $state_root/watchdog.pid 2>/dev/null || true)
  if [ x$legacy_watchdog_pid != x ] && [ -r /proc/$legacy_watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$legacy_watchdog_pid/cmdline | grep -Fx $state_root/watchdog.sh >/dev/null 2>&1; then
    kill $legacy_watchdog_pid 2>/dev/null || true
    legacy_stop_attempt=0
    while [ $legacy_stop_attempt -lt 10 ] && kill -0 $legacy_watchdog_pid 2>/dev/null && [ -r /proc/$legacy_watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$legacy_watchdog_pid/cmdline | grep -Fx $state_root/watchdog.sh >/dev/null 2>&1; do
      legacy_stop_attempt=$((legacy_stop_attempt + 1))
      sleep 1
    done
    if kill -0 $legacy_watchdog_pid 2>/dev/null && [ -r /proc/$legacy_watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$legacy_watchdog_pid/cmdline | grep -Fx $state_root/watchdog.sh >/dev/null 2>&1; then
      kill -KILL $legacy_watchdog_pid 2>/dev/null || true
      sleep 1
    fi
  fi
  rm -f $state_root/watchdog.pid
fi
legacy_agent_pid=$(process_pid $bin/fns-agent $state_root/agent.json || true)
if [ x$legacy_agent_pid != x ]; then kill $legacy_agent_pid 2>/dev/null || true; fi
attempt=0
while [ $attempt -lt 5 ]; do
  if process_pid $bin/fns-server >/dev/null && required_agents_running; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
if [ $watchdog_required -eq 1 ] || ! process_pid $bin/fns-server >/dev/null || ! required_agents_running; then
  if [ $systemd_scope = system ]; then
    systemctl stop $sync_unit || true
  elif [ $systemd_scope = user ]; then
    systemctl --user stop $sync_unit || true
  fi
  for watchdog_script in $state_root/workspaces/*/watchdog.sh; do
    [ -f $watchdog_script ] || continue
    watchdog_state=${{watchdog_script%/watchdog.sh}}
    watchdog_running=0
    if [ -f $watchdog_state/watchdog.pid ]; then
      watchdog_pid=$(cat $watchdog_state/watchdog.pid 2>/dev/null || true)
      if [ x$watchdog_pid != x ] && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $watchdog_script >/dev/null 2>&1; then watchdog_running=1; fi
    fi
    if [ $watchdog_running -eq 0 ]; then
      nohup sh $watchdog_script >/dev/null 2>>$watchdog_state/watchdog.stderr.log &
      echo $! > $watchdog_state/watchdog.pid
    fi
  done
fi
attempt=0
while [ $attempt -lt 20 ]; do
  if process_pid $bin/fns-server >/dev/null && required_agents_running; then
    if rm -f $bin/.bestcodex-replace-active; then
      rm -f $bin/fns-server.previous $bin/fns-agent.previous $bin/fns-server.new $bin/fns-agent.new || true
    fi
    exit 0
  fi
  attempt=$((attempt + 1))
  sleep 1
done
echo bestcodex-sync-start-failed >&2
if [ -f $state/watchdog.pid ]; then
  watchdog_pid=$(cat $state/watchdog.pid 2>/dev/null || true)
  if [ x$watchdog_pid != x ] && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $state/watchdog.sh >/dev/null 2>&1; then
    kill $watchdog_pid 2>/dev/null || true
    watchdog_stop_attempt=0
    while [ $watchdog_stop_attempt -lt 10 ] && kill -0 $watchdog_pid 2>/dev/null && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $state/watchdog.sh >/dev/null 2>&1; do
      watchdog_stop_attempt=$((watchdog_stop_attempt + 1))
      sleep 1
    done
    if kill -0 $watchdog_pid 2>/dev/null && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $state/watchdog.sh >/dev/null 2>&1; then
      kill -KILL $watchdog_pid 2>/dev/null || true
    fi
  fi
  rm -f $state/watchdog.pid
fi
if [ $systemd_scope = system ]; then
  systemctl disable --now $sync_unit || true
elif [ $systemd_scope = user ]; then
  systemctl --user disable --now $sync_unit || true
fi
remaining_pid=$(process_pid $bin/fns-agent $state/agent.json || true)
if [ x$remaining_pid != x ]; then kill $remaining_pid 2>/dev/null || true; fi
process_stop_attempt=0
while [ $process_stop_attempt -lt 10 ] && process_pid $bin/fns-agent $state/agent.json >/dev/null; do
  process_stop_attempt=$((process_stop_attempt + 1))
  sleep 1
done
remaining_pid=$(process_pid $bin/fns-agent $state/agent.json || true)
if [ x$remaining_pid != x ]; then kill -KILL $remaining_pid 2>/dev/null || true; fi
tail -n 40 $state/server.stderr.log >&2 2>/dev/null || true
tail -n 40 $state/agent.stderr.log >&2 2>/dev/null || true
exit 1
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_fails_when_sync_components_are_missing() {
        let local = tempfile::tempdir().unwrap();
        let outcome = prepare_components(
            "43.156.20.8",
            "root",
            22,
            None,
            &local.path().to_string_lossy(),
            None,
        );
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("DEPLOY_ARTIFACT_MISSING")
        );
        assert!(outcome.detail.as_deref().is_some_and(|d| {
            d.contains("这个版本") && !d.contains("这台电脑") && !d.contains("agent")
        }));
    }

    #[test]
    fn inspect_output_reports_existing_project_names() {
        let report = parse_inspect_output("EXISTS:1\nCOMPONENTS:1\nmy-project\nmy-project-2\n");
        assert!(report.exists);
        assert!(report.components_installed);
        assert_eq!(report.names, ["my-project", "my-project-2"]);
        let missing = parse_inspect_output("EXISTS:0\nCOMPONENTS:0\n");
        assert!(!missing.exists);
        assert!(!missing.components_installed);
        assert!(missing.names.is_empty());
        let script = inspect_remote_script("~/bestcodex/my-project");
        assert!(script.contains("COMPONENTS:"));
        assert!(script.contains("EXISTS:"));
        assert!(
            !script.contains('"'),
            "inspect must not emit double quotes that break sshd -c wrapping: {script}"
        );
    }

    #[test]
    fn replace_stops_running_binaries_before_copy() {
        let script = stop_sync_for_replace_script();
        let watchdog = script.find("watchdog.pid").expect("watchdog stop");
        let server = script.find("fns-server").expect("server stop");
        assert!(
            watchdog < server,
            "watchdog must stop before binaries: {script}"
        );
        assert!(script.contains("$state_root/workspaces/*/watchdog.pid"));
        assert!(script.contains("/etc/systemd/system/bestcodex-sync-*.service"));
        assert!(script.contains("$HOME/.config/systemd/user/bestcodex-sync-*.service"));
        assert!(script.contains("systemctl stop bestcodex-sync.service || true"));
        assert!(!script.contains("rm -rf"));
        assert!(!script.contains("sudo"));
        assert!(script.contains("watchdog_stop_attempt=0"));
        assert!(script.contains("kill -0 $watchdog_pid"));
        assert!(script.contains("/proc/[0-9]*/exe"));
        assert!(!script.contains("pkill"));
        assert!(script.contains("binary_stop_attempt=0"));
        assert!(script.contains("[ $binary_stop_attempt -lt 10 ]"));
        assert!(script.contains("exit 1"));
        assert!(
            !script.contains('"'),
            "double quotes break sshd's shell -c wrapper: {script}"
        );
        assert!(!script.contains("sudo"));
    }

    #[test]
    fn prepare_progress_copy_never_says_agent() {
        for progress in [
            PrepareProgress::mkdir(),
            PrepareProgress::upload(1),
            PrepareProgress::upload(2),
            PrepareProgress::finish(),
        ] {
            assert!(!progress.detail.contains("agent"));
            assert!(!progress.detail.contains("tmux"));
        }
    }

    #[test]
    fn deploy_emits_progress_before_remote_work() {
        let local = tempfile::tempdir().unwrap();
        let server = local.path().join("fns-server");
        let agent = local.path().join("fns-agent");
        std::fs::write(&server, vec![0u8; 2048]).unwrap();
        std::fs::write(&agent, vec![0u8; 2048]).unwrap();
        let artifacts = ArtifactPaths { server, agent };
        let target = ResolvedSshTarget {
            host: "127.0.0.1".into(),
            user: "root".into(),
            port: 22,
            alias: None,
            use_config: false,
            identity_file: None,
        };
        let mut seen = Vec::new();
        let outcome = deploy_remote(
            &target,
            None,
            None,
            "~/bestcodex/my-project",
            &artifacts,
            false,
            |_| Err("SSH_UNREACHABLE"),
            |progress| {
                seen.push((progress.phase.clone(), progress.detail.clone()));
            },
        );
        assert!(!outcome.ok);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "mkdir");
        assert!(seen[0].1.contains("项目目录"));
        assert!(seen.iter().all(|(_, detail)| !detail.contains("agent")));
    }

    #[test]
    fn scp_uses_capital_p_for_the_ssh_port() {
        let target = ResolvedSshTarget {
            host: "108.80.81.15".into(),
            user: "root".into(),
            port: 1080,
            alias: None,
            use_config: false,
            identity_file: None,
        };
        let args = scp_invocation_args(
            &target,
            None,
            Path::new("/tmp/fns-agent"),
            "~/.local/share/bestcodex/bin/fns-agent",
        )
        .expect("scp args");
        assert!(
            args.iter().any(|arg| arg == "-P"),
            "scp port flag is -P, not ssh's -p: {args:?}"
        );
        assert!(
            args.iter().any(|arg| arg == "1080"),
            "port must be kept: {args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg == "-p"),
            "scp -p means preserve times and would treat the port as a file: {args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg == "root@108.80.81.15:~/.local/share/bestcodex/bin/fns-agent"),
            "remote path must stay host:path: {args:?}"
        );
    }

    #[test]
    fn keep_sync_uses_a_server_issued_token() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        assert!(
            script.contains("bootstrap-workspace"),
            "server must issue the workspace token: {script}"
        );
        assert!(
            script.contains("--token-file $state/token"),
            "issued token must be written to the private token file: {script}"
        );
        assert!(
            !script.contains(&base64_encode(b"bestcodex-local-token")),
            "fixed tokens are rejected by current servers: {script}"
        );
        assert!(
            script.contains("daemon-reload || true"),
            "systemd reload must not abort the start script: {script}"
        );
    }

    #[test]
    fn remote_sync_state_and_service_are_scoped_to_the_workspace() {
        let first_root = "~/bestcodex/first";
        let second_root = "~/bestcodex/second";
        let first_id = sync_workspace_id(first_root);
        let second_id = sync_workspace_id(second_root);
        let first = keep_sync_running_script(first_root);
        let second = keep_sync_running_script(second_root);

        assert_ne!(first_id, second_id);
        assert!(first.contains(&format!("state=$state_root/workspaces/{first_id}")));
        assert!(second.contains(&format!("state=$state_root/workspaces/{second_id}")));
        assert!(first.contains(&format!("sync_unit=bestcodex-sync-{first_id}.service")));
        assert!(second.contains(&format!("sync_unit=bestcodex-sync-{second_id}.service")));
        assert!(first.contains("process_pid $bin/fns-agent $state/agent.json"));
        assert!(!first.contains("state=$home/.local/share/bestcodex/state\n"));
    }

    #[test]
    fn remote_sync_token_read_is_scoped_to_the_workspace() {
        let root = "~/bestcodex/docs";
        let workspace_id = sync_workspace_id(root);
        let script = remote_sync_token_script(root);
        assert_eq!(
            script,
            format!("cat $HOME/.local/share/bestcodex/state/workspaces/{workspace_id}/token")
        );
        assert!(!script.contains('"'));
    }

    #[test]
    fn legacy_global_watchdog_is_gone_before_scoped_agent_startup() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        let legacy_cleanup = script
            .find("if [ -f $state_root/watchdog.pid ]")
            .expect("legacy watchdog cleanup");
        let scoped_probe = script
            .find("while [ $attempt -lt 5 ]")
            .expect("scoped process probe");
        let cleanup = &script[legacy_cleanup..scoped_probe];

        assert!(cleanup.contains("kill -0 $legacy_watchdog_pid"));
        assert!(cleanup.contains("kill -KILL $legacy_watchdog_pid"));
        assert!(cleanup.contains("process_pid $bin/fns-agent $state_root/agent.json"));
    }

    #[test]
    fn keep_sync_running_script_enables_a_permanent_service() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        assert!(
            script.contains("systemctl") && script.contains("enable --now"),
            "install must enable a permanent service, not only chmod: {script}"
        );
        assert!(
            script.contains("Restart=always") || script.contains("base64"),
            "service must restart forever: {script}"
        );
        assert!(script.contains("fns-agent") || script.contains("bestcodex-sync"));
        assert!(
            !script.contains('"'),
            "double quotes break sshd's shell -c wrapper: {script}"
        );
        assert!(!script.contains("sudo"));
        assert!(!script.contains("tmux"));
        assert!(!script.contains("pgrep -f"));
        assert!(script.contains("/proc/[0-9]*/exe"));
        assert!(script.contains("watchdog.pid"));
        assert!(script.contains("systemctl restart bestcodex-workspace.service $sync_unit"));
        assert!(script.contains("systemctl --user restart bestcodex-workspace.service $sync_unit"));
        assert!(script.contains("systemctl disable --now bestcodex-sync.service || true"));
        assert!(script.contains("systemctl --user disable --now bestcodex-sync.service || true"));
        let unit = remote_sync_unit("multi-user.target");
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("fns-agent run --config"));
        assert!(!unit.contains("sudo"));
        let workspace_unit = remote_workspace_unit("multi-user.target");
        assert!(workspace_unit.contains("fns-server run --config"));
        assert!(!workspace_unit.contains(" run -p "));
        let workspace_id = sync_workspace_id("~/bestcodex/docs");
        assert_eq!(workspace_id.len(), 36);
        assert_eq!(workspace_id.chars().filter(|ch| *ch == '-').count(), 4);
        assert_ne!(
            sync_client_id("local", "~/bestcodex/docs"),
            sync_client_id("remote", "~/bestcodex/docs")
        );
    }

    #[test]
    fn keep_sync_running_starts_even_when_systemd_enable_fails() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        assert!(
            script.contains("systemctl enable --now $unit || true"),
            "systemd enable must not abort under set -e: {script}"
        );
        assert!(script.contains("systemctl --user enable --now $unit || true"));
        assert!(
            !script.contains("if [ $started -eq 0 ]"),
            "nohup must still run when systemd exists but the process is missing: {script}"
        );
        assert!(script.contains("nohup sh $watchdog_script"));
        assert!(script.contains("server.stderr.log"));
        assert!(script.contains("agent.stderr.log"));
        assert!(script.contains("tail -n 40"));
        assert!(script.contains("attempt=0"));
        assert!(script.contains("[ $attempt -lt 20 ]"));
        let failure = script.find("echo bestcodex-sync-start-failed").unwrap();
        let failure_script = &script[failure..];
        let cleanup = failure_script.find("kill $watchdog_pid").unwrap();
        assert!(
            cleanup > 0,
            "terminal failure must stop watchdog churn: {script}"
        );
        assert!(failure_script.contains("systemctl disable --now $sync_unit || true"));
        assert!(failure_script.contains("systemctl --user disable --now $sync_unit || true"));
        assert!(failure_script.contains("process_pid $bin/fns-agent $state/agent.json"));
        assert!(script.contains("all_scoped_agents_running()"));
        assert!(script.contains("for agent_config in $state_root/workspaces/*/agent.json"));
        assert!(
            script.contains("process_pid $bin/fns-server >/dev/null && required_agents_running")
        );
        assert!(script.contains("if [ $require_all -eq 1 ]"));
        assert!(
            !failure_script.contains("kill $server_pid"),
            "one workspace failure must not stop the shared server: {failure_script}"
        );
        assert!(
            !failure_script.contains("for wanted in $bin/fns-agent $bin/fns-server"),
            "one workspace failure must only clean up its own agent: {failure_script}"
        );
        assert!(failure_script.contains("kill -KILL $remaining_pid"));
        assert!(
            !script.contains('"'),
            "double quotes break sshd's shell -c wrapper: {script}"
        );
    }

    #[test]
    fn user_systemd_without_linger_falls_back_to_watchdog() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        assert!(script.contains("loginctl show-user $(id -u) -p Linger"));
        assert!(script.contains("grep -Fx Linger=yes"));
        assert!(script.contains("systemd_scope=none"));
        assert!(script.contains("watchdog_required=1"));
        assert!(script.contains("if [ $watchdog_required -eq 1 ]"));
        assert!(
            script.contains("for unit_file in $home/.config/systemd/user/bestcodex-sync-*.service")
        );
        assert!(script.contains("systemctl --user disable --now $unit || true"));
        assert!(script.contains("rm -f $home/.config/systemd/user/default.target.wants/$unit"));
        assert!(script.contains("systemctl --user disable --now bestcodex-workspace.service"));
    }

    #[test]
    fn generated_remote_files_replace_every_placeholder() {
        let script = keep_sync_running_script("~/bestcodex/docs");
        assert!(script.contains("sed s#HOME_PLACEHOLDER#$home#g"));
        assert!(script.contains("sed s#ROOT_PLACEHOLDER#$root#g"));
        assert!(script.contains("grep -R PLACEHOLDER"));
    }

    #[test]
    fn ssh_failure_diagnostic_is_bounded_and_internal() {
        let diagnostic = format_ssh_failure_log("start", Some(1), &vec![b'x'; 10_000]);
        assert!(
            diagnostic.len() <= 4_200,
            "diagnostic was {} bytes",
            diagnostic.len()
        );
        assert!(diagnostic.contains("stage=start"));
        assert!(diagnostic.contains("exit=1"));
        assert!(!human_prepare_detail("SSH_PREPARE_FAILED", "host", 22).contains(&diagnostic));
    }

    #[test]
    fn replace_stop_failure_aborts_before_upload() {
        let local = tempfile::tempdir().unwrap();
        let server = local.path().join("fns-server");
        let agent = local.path().join("fns-agent");
        std::fs::write(&server, vec![0u8; 2048]).unwrap();
        std::fs::write(&agent, vec![0u8; 2048]).unwrap();
        let artifacts = ArtifactPaths { server, agent };
        let target = ResolvedSshTarget {
            host: "127.0.0.1".into(),
            user: "root".into(),
            port: 22,
            alias: None,
            use_config: false,
            identity_file: None,
        };
        let calls = std::cell::Cell::new(0usize);
        let outcome = deploy_remote(
            &target,
            None,
            None,
            "~/bestcodex/my-project",
            &artifacts,
            true,
            |_| {
                let call = calls.get();
                calls.set(call + 1);
                if call == 0 {
                    Ok(Command::new("true").output().unwrap())
                } else {
                    Ok(Command::new("false").output().unwrap())
                }
            },
            |_| {},
        );
        assert!(!outcome.ok);
        assert_eq!(calls.get(), 2, "upload must not start after stop failure");
    }

    #[test]
    fn replace_copy_failure_restores_all_existing_services() {
        for failed_copy in [0usize, 1usize] {
            let local = tempfile::tempdir().unwrap();
            let server = local.path().join("fns-server");
            let agent = local.path().join("fns-agent");
            std::fs::write(&server, vec![0u8; 2048]).unwrap();
            std::fs::write(&agent, vec![0u8; 2048]).unwrap();
            let artifacts = ArtifactPaths { server, agent };
            let ssh_calls = std::cell::RefCell::new(Vec::new());
            let copy_calls = std::cell::Cell::new(0usize);

            let outcome = deploy_remote_with_copy(
                "~/bestcodex/my-project",
                &artifacts,
                true,
                |script| {
                    ssh_calls.borrow_mut().push(script.to_string());
                    Ok(Command::new("true").output().unwrap())
                },
                |_, remote| {
                    let call = copy_calls.get();
                    copy_calls.set(call + 1);
                    assert!(remote.ends_with(".new"));
                    if call == failed_copy {
                        Err("copy failed")
                    } else {
                        Ok(())
                    }
                },
                |_| {},
            );

            assert!(!outcome.ok);
            let calls = ssh_calls.borrow();
            assert_eq!(calls.len(), 3, "mkdir, stop, and recovery must run");
            let recovery = calls.last().unwrap();
            assert!(recovery.contains("bestcodex-sync-*.service"));
            assert!(recovery.contains("$state_root/workspaces/*/watchdog.sh"));
            assert!(recovery.contains("bestcodex-workspace.service"));
        }
    }

    #[test]
    fn replace_recovery_ignores_legacy_global_supervisors() {
        let recovery = restart_sync_after_failed_replace_script();

        assert!(!recovery.contains("systemctl start bestcodex-sync.service"));
        assert!(!recovery.contains("systemctl --user start bestcodex-sync.service"));
        assert!(!recovery.contains(
            "for watchdog_script in $state_root/watchdog.sh $state_root/workspaces/*/watchdog.sh"
        ));
        assert!(recovery.contains("bestcodex-sync-*.service"));
        assert!(recovery.contains("$state_root/workspaces/*/watchdog.sh"));
    }

    #[test]
    fn replace_start_failure_rolls_back_binaries_before_recovery() {
        let local = tempfile::tempdir().unwrap();
        let server = local.path().join("fns-server");
        let agent = local.path().join("fns-agent");
        std::fs::write(&server, vec![0u8; 2048]).unwrap();
        std::fs::write(&agent, vec![0u8; 2048]).unwrap();
        let artifacts = ArtifactPaths { server, agent };
        let ssh_calls = std::cell::RefCell::new(Vec::new());

        let outcome = deploy_remote_with_copy(
            "~/bestcodex/my-project",
            &artifacts,
            true,
            |script| {
                let mut calls = ssh_calls.borrow_mut();
                let call = calls.len();
                calls.push(script.to_string());
                if call == 3 {
                    Ok(Command::new("false").output().unwrap())
                } else {
                    Ok(Command::new("true").output().unwrap())
                }
            },
            |_, _| Ok(()),
            |_| {},
        );

        assert!(!outcome.ok);
        let calls = ssh_calls.borrow();
        assert_eq!(
            calls.len(),
            7,
            "mkdir, initial stop, activate, start, final stop, rollback, and recovery must run"
        );
        assert_eq!(calls[4], stop_sync_for_replace_script());
        assert!(calls[5].contains(".bestcodex-replace-active"));
        assert_eq!(calls[6], restart_sync_after_failed_replace_script());
    }

    #[test]
    fn staged_activation_and_replace_recovery_obey_remote_shell_constraints() {
        for script in [
            activate_staged_components_script(),
            rollback_staged_components_script(),
            restart_sync_after_failed_replace_script(),
        ] {
            assert!(!script.contains('"'));
            assert!(!script.contains("sudo"));
            assert!(!script.contains("rm -rf"));
        }
        let activate = activate_staged_components_script();
        assert!(activate.contains("[ -f $bin/fns-server.new ]"));
        assert!(activate.contains("[ -f $bin/fns-agent.new ]"));
        assert!(activate.contains(".bestcodex-replace-active"));
        assert!(activate.contains("$bin/fns-server.previous"));
        assert!(activate.contains("$bin/fns-agent.previous"));
        assert!(activate.contains("if ! mv $bin/fns-server.new $bin/fns-server"));
        assert!(activate.contains("if ! mv $bin/fns-agent.new $bin/fns-agent"));
        assert!(activate.contains("mv $bin/fns-server.new $bin/fns-server"));
        assert!(activate.contains("mv $bin/fns-agent.new $bin/fns-agent"));
        assert!(
            activate.find(": > $marker").unwrap()
                > activate
                    .find("mv $bin/fns-agent $bin/fns-agent.previous")
                    .unwrap(),
            "the active marker must only appear after both old binaries are backed up"
        );
        let recovery = restart_sync_after_failed_replace_script();
        assert!(recovery.contains("loginctl show-user $(id -u) -p Linger"));
        assert!(recovery.contains("watchdog_required=1"));
        assert!(recovery.contains("if [ $watchdog_required -eq 0 ]"));
    }

    #[cfg(unix)]
    #[test]
    fn staged_activation_restores_both_old_binaries_when_second_move_fails() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/share/bestcodex/bin");
        let fake_path = home.path().join("fake-path");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&fake_path).unwrap();
        std::fs::write(bin.join("fns-server"), b"old-server").unwrap();
        std::fs::write(bin.join("fns-agent"), b"old-agent").unwrap();
        std::fs::write(bin.join("fns-server.new"), b"new-server").unwrap();
        std::fs::write(bin.join("fns-agent.new"), b"new-agent").unwrap();
        let fake_mv = fake_path.join("mv");
        std::fs::write(
            &fake_mv,
            b"#!/bin/sh\ncase $1 in */fns-agent.new) exit 1;; esac\nexec /bin/mv \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_mv, std::fs::Permissions::from_mode(0o755)).unwrap();

        let output = Command::new("sh")
            .arg("-c")
            .arg(activate_staged_components_script())
            .env("HOME", home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", fake_path.display()))
            .output()
            .unwrap();

        assert!(!output.status.success());
        assert_eq!(
            std::fs::read(bin.join("fns-server")).unwrap(),
            b"old-server"
        );
        assert_eq!(std::fs::read(bin.join("fns-agent")).unwrap(), b"old-agent");
        assert!(!bin.join(".bestcodex-replace-active").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rollback_restores_a_partial_backup_created_before_the_marker() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/share/bestcodex/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("fns-server.previous"), b"old-server").unwrap();
        std::fs::write(bin.join("fns-agent"), b"old-agent").unwrap();

        let output = Command::new("sh")
            .arg("-c")
            .arg(rollback_staged_components_script())
            .env("HOME", home.path())
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(
            std::fs::read(bin.join("fns-server")).unwrap(),
            b"old-server"
        );
        assert_eq!(std::fs::read(bin.join("fns-agent")).unwrap(), b"old-agent");
        assert!(!bin.join("fns-server.previous").exists());
    }

    #[test]
    fn prepare_with_artifacts_creates_the_local_root() {
        let local = tempfile::tempdir().unwrap();
        let dest = local.path().join("project");
        let artifacts_dir = tempfile::tempdir().unwrap();
        let server = artifacts_dir.path().join("fns-server");
        let agent = artifacts_dir.path().join("fns-agent");
        std::fs::write(&server, b"server").unwrap();
        std::fs::write(&agent, b"agent").unwrap();
        let artifacts = ArtifactPaths { server, agent };
        let outcome = prepare_components(
            "43.156.20.8",
            "root",
            22,
            None,
            &dest.to_string_lossy(),
            Some(&artifacts),
        );
        assert!(outcome.ok);
        assert!(dest.exists());
    }
}
