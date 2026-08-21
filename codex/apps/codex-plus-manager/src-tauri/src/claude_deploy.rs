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
state=$HOME/.local/share/bestcodex/state
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
      sleep 1
    fi
  fi
  rm -f $state/watchdog.pid
fi
if [ $(id -u) -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemctl stop bestcodex-sync.service || true
  systemctl stop bestcodex-workspace.service || true
elif command -v systemctl >/dev/null 2>&1; then
  systemctl --user stop bestcodex-sync.service || true
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
    if scp_file(
        target,
        password,
        key_path,
        &artifacts.server,
        "~/.local/share/bestcodex/bin/fns-server",
    )
    .is_err()
    {
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能把同步组件传到服务器。".into()),
        };
    }
    on_progress(PrepareProgress::upload(2));
    if scp_file(
        target,
        password,
        key_path,
        &artifacts.agent,
        "~/.local/share/bestcodex/bin/fns-agent",
    )
    .is_err()
    {
        return PrepareOutcome {
            ok: false,
            error_code: Some("SSH_PREPARE_FAILED".into()),
            detail: Some("没能把同步组件传到服务器。".into()),
        };
    }

    on_progress(PrepareProgress::finish());
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
            PrepareOutcome {
                ok: false,
                error_code: Some("SSH_PREPARE_FAILED".into()),
                detail: Some("没能在服务器上装好同步组件。".into()),
            }
        }
    }
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
        "[Unit]\nDescription=BestCodex file sync\nAfter=network-online.target bestcodex-workspace.service\nWants=network-online.target\n\n[Service]\nType=simple\nRestart=always\nRestartSec=2\nWorkingDirectory=ROOT_PLACEHOLDER\nExecStart=HOME_PLACEHOLDER/.local/share/bestcodex/bin/fns-agent run --config HOME_PLACEHOLDER/.local/share/bestcodex/state/agent.json\nNoNewPrivileges=true\n\n[Install]\nWantedBy={wanted_by}\n"
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
        "{{\n  \"schemaVersion\": \"fns-agent-config/1\",\n  \"endpoint\": \"ws://127.0.0.1:9000/api/user/workspace-sync/v2\",\n  \"workspaceId\": \"{workspace_id}\",\n  \"clientId\": \"{client_id}\",\n  \"workspaceRoot\": \"ROOT_PLACEHOLDER\",\n  \"stateDir\": \"HOME_PLACEHOLDER/.local/share/bestcodex/state\",\n  \"tokenFile\": \"HOME_PLACEHOLDER/.local/share/bestcodex/state/token\",\n  \"sync\": {{\n    \"includes\": [\"**/*\"],\n    \"excludes\": [],\n    \"protectSecrets\": true\n  }},\n  \"transport\": {{ \"maxActiveTransfers\": 2 }}\n}}\n"
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
state=$home/.local/share/bestcodex/state
server_dir=$home/.local/share/bestcodex/server
root={root}
mkdir -p $root $state $server_dir $home/.config/systemd/user
chmod 0755 $bin/fns-server $bin/fns-agent
root=$(cd $root && pwd)
$bin/fns-server bootstrap-workspace --config $server_dir/config/config.yaml --token-file $state/token --workspace-id {workspace_id} --workspace-root $root
chmod 0600 $state/token
printf %s {config_b64} | base64 -d | sed s#ROOT_PLACEHOLDER#$root#g | sed s#HOME_PLACEHOLDER#$home#g > $state/agent.json
chmod 0600 $state/agent.json
printf %s {watchdog_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#ROOT_PLACEHOLDER#$root#g | sed s#PORT_PLACEHOLDER#9000#g > $state/watchdog.sh
chmod 0700 $state/watchdog.sh
if grep -R PLACEHOLDER $state/agent.json $state/watchdog.sh >/dev/null; then exit 1; fi
systemd_scope=none
watchdog_required=0
if [ $(id -u) -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  systemd_scope=system
  mkdir -p /etc/systemd/system
  printf %s {workspace_system_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g > /etc/systemd/system/bestcodex-workspace.service
  printf %s {sync_system_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#ROOT_PLACEHOLDER#$root#g > /etc/systemd/system/bestcodex-sync.service
  if grep -R PLACEHOLDER $state/agent.json /etc/systemd/system/bestcodex-workspace.service /etc/systemd/system/bestcodex-sync.service >/dev/null; then exit 1; fi
  systemctl daemon-reload || true
  systemctl enable --now bestcodex-workspace.service || true
  systemctl enable --now bestcodex-sync.service || true
  systemctl restart bestcodex-workspace.service bestcodex-sync.service || true
elif command -v systemctl >/dev/null 2>&1; then
  systemd_scope=user
  printf %s {workspace_user_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g > $home/.config/systemd/user/bestcodex-workspace.service
  printf %s {sync_user_b64} | base64 -d | sed s#HOME_PLACEHOLDER#$home#g | sed s#ROOT_PLACEHOLDER#$root#g > $home/.config/systemd/user/bestcodex-sync.service
  if grep -R PLACEHOLDER $state/agent.json $home/.config/systemd/user/bestcodex-workspace.service $home/.config/systemd/user/bestcodex-sync.service >/dev/null; then exit 1; fi
  systemctl --user daemon-reload || true
  systemctl --user enable --now bestcodex-workspace.service || true
  systemctl --user enable --now bestcodex-sync.service || true
  systemctl --user restart bestcodex-workspace.service bestcodex-sync.service || true
  if ! command -v loginctl >/dev/null 2>&1 || ! loginctl show-user $(id -u) -p Linger 2>/dev/null | grep -Fx Linger=yes >/dev/null; then
    systemctl --user disable --now bestcodex-sync.service bestcodex-workspace.service || true
    rm -f $home/.config/systemd/user/default.target.wants/bestcodex-sync.service
    rm -f $home/.config/systemd/user/default.target.wants/bestcodex-workspace.service
    systemd_scope=none
    watchdog_required=1
  fi
fi
process_pid() {{
  wanted=$1
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    case x$target in
      x$wanted)
        pid=${{proc_exe#/proc/}}
        echo ${{pid%/exe}}
        return 0
        ;;
    esac
  done
  return 1
}}
attempt=0
while [ $attempt -lt 5 ]; do
  if process_pid $bin/fns-server >/dev/null && process_pid $bin/fns-agent >/dev/null; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
if [ $watchdog_required -eq 1 ] || ! process_pid $bin/fns-server >/dev/null || ! process_pid $bin/fns-agent >/dev/null; then
  if [ $systemd_scope = system ]; then
    systemctl stop bestcodex-sync.service bestcodex-workspace.service || true
  elif [ $systemd_scope = user ]; then
    systemctl --user stop bestcodex-sync.service bestcodex-workspace.service || true
  fi
  watchdog_running=0
  if [ -f $state/watchdog.pid ]; then
    watchdog_pid=$(cat $state/watchdog.pid 2>/dev/null || true)
    if [ x$watchdog_pid != x ] && [ -r /proc/$watchdog_pid/cmdline ] && tr '\\000' '\\n' < /proc/$watchdog_pid/cmdline | grep -Fx $state/watchdog.sh >/dev/null 2>&1; then watchdog_running=1; fi
  fi
  if [ $watchdog_running -eq 0 ]; then
    nohup sh $state/watchdog.sh >/dev/null 2>>$state/watchdog.stderr.log &
    echo $! > $state/watchdog.pid
  fi
fi
attempt=0
while [ $attempt -lt 20 ]; do
  if process_pid $bin/fns-server >/dev/null && process_pid $bin/fns-agent >/dev/null; then exit 0; fi
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
  systemctl disable --now bestcodex-sync.service bestcodex-workspace.service || true
elif [ $systemd_scope = user ]; then
  systemctl --user disable --now bestcodex-sync.service bestcodex-workspace.service || true
fi
for wanted in $bin/fns-agent $bin/fns-server; do
  remaining_pid=$(process_pid $wanted || true)
  if [ x$remaining_pid != x ]; then kill $remaining_pid 2>/dev/null || true; fi
  process_stop_attempt=0
  while [ $process_stop_attempt -lt 10 ] && process_pid $wanted >/dev/null; do
    process_stop_attempt=$((process_stop_attempt + 1))
    sleep 1
  done
  remaining_pid=$(process_pid $wanted || true)
  if [ x$remaining_pid != x ]; then kill -KILL $remaining_pid 2>/dev/null || true; fi
done
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
        assert!(
            script.contains("systemctl restart bestcodex-workspace.service bestcodex-sync.service")
        );
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
            script.contains("enable --now bestcodex-sync.service || true"),
            "systemd enable must not abort under set -e: {script}"
        );
        assert!(
            !script.contains("if [ $started -eq 0 ]"),
            "nohup must still run when systemd exists but the process is missing: {script}"
        );
        assert!(script.contains("nohup sh $state/watchdog.sh"));
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
        assert!(failure_script.contains(
            "systemctl disable --now bestcodex-sync.service bestcodex-workspace.service"
        ));
        assert!(failure_script.contains(
            "systemctl --user disable --now bestcodex-sync.service bestcodex-workspace.service"
        ));
        assert!(failure_script.contains("for wanted in $bin/fns-agent $bin/fns-server"));
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
        assert!(script.contains(
            "systemctl --user disable --now bestcodex-sync.service bestcodex-workspace.service"
        ));
        assert!(script.contains(
            "rm -f $home/.config/systemd/user/default.target.wants/bestcodex-sync.service"
        ));
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
