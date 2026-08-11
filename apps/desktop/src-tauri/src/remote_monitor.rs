//! Remote host status + project-scoped tmux Claude session probes.

use crate::project::ProjectConfig;
use crate::terminal::TerminalManager;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const SSH_TIMEOUT_SECS: u64 = 10;
const STDOUT_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMonitorError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatusSnapshot {
    pub project_id: String,
    pub ssh_host_alias: String,
    pub captured_at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RemoteMonitorError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<ServicesMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disks: Vec<DiskMetrics>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetrics {
    pub usage_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load5: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load15: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetrics {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceItem {
    pub key: String,
    pub display_name: String,
    pub running: bool,
    pub process_count: u32,
    pub cpu_percent: f64,
    pub memory_rss_bytes: u64,
    pub pids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicesMetrics {
    pub items: Vec<ServiceItem>,
    pub our_services_memory_rss_bytes: u64,
    pub our_services_cpu_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionWindow {
    pub index: u32,
    pub id: String,
    pub name: String,
    pub title: String,
    pub active: bool,
    pub pane_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub looks_like_claude: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSessionsSnapshot {
    pub project_id: String,
    pub ssh_host_alias: String,
    pub tmux_session: String,
    pub captured_at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RemoteMonitorError>,
    pub session_exists: bool,
    pub windows: Vec<ClaudeSessionWindow>,
    pub active_index: Option<u32>,
}

/// Intermediate JSON shape emitted by the remote host probe script.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteHostProbe {
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    uptime_seconds: Option<u64>,
    cpu: RemoteCpuProbe,
    memory: RemoteMemoryProbe,
    #[serde(default)]
    disks: Vec<RemoteDiskProbe>,
    #[serde(default)]
    processes: Vec<RemoteProcessProbe>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteCpuProbe {
    usage_percent: f64,
    #[serde(default)]
    load1: Option<f64>,
    #[serde(default)]
    load5: Option<f64>,
    #[serde(default)]
    load15: Option<f64>,
    #[serde(default)]
    cores: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteMemoryProbe {
    total_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteDiskProbe {
    mount: String,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteProcessProbe {
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    comm: String,
    #[serde(default)]
    cmdline: String,
    #[serde(default)]
    rss_bytes: u64,
    #[serde(default)]
    cpu_percent: f64,
}

const SERVICE_KEYS: &[&str] = &["fns-agent", "fns-server", "claude"];
const PID_CAP: usize = 20;

/// Strict session name validation: charset only, equals sanitized form, max 64.
pub fn validate_tmux_session_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name.len() > 64 {
        return Err("invalid_session_name".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("invalid_session_name".into());
    }
    Ok(name.to_string())
}

/// Match a process to one of our tracked service keys.
pub fn match_service_key(comm: &str, cmdline: &str) -> Option<&'static str> {
    let blob = format!("{comm} {cmdline}");
    if blob.contains("fns-agent") {
        return Some("fns-agent");
    }
    if blob.contains("fns-server") {
        return Some("fns-server");
    }
    if comm == "claude" || cmdline.split_whitespace().next() == Some("claude") {
        return Some("claude");
    }
    None
}

fn display_name_for(key: &str) -> String {
    match key {
        "fns-agent" => "fns-agent".to_string(),
        "fns-server" => "fns-server".to_string(),
        "claude" => "claude".to_string(),
        other => other.to_string(),
    }
}

fn used_percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    }
}

/// Map remote probe JSON into a `ServerStatusSnapshot`.
pub fn parse_server_status_payload(
    stdout: &str,
    project_id: &str,
    ssh_host_alias: &str,
    captured_at: &str,
) -> ServerStatusSnapshot {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return server_status_error(
            project_id,
            ssh_host_alias,
            captured_at,
            "parse_failed",
            "Empty probe output",
        );
    }

    let probe: RemoteHostProbe = match serde_json::from_str(trimmed) {
        Ok(p) => p,
        Err(_) => {
            return server_status_error(
                project_id,
                ssh_host_alias,
                captured_at,
                "parse_failed",
                "Probe output is not valid JSON",
            );
        }
    };

    let available = probe.memory.available_bytes;
    let total = probe.memory.total_bytes;
    let used = total.saturating_sub(available);

    let disks: Vec<DiskMetrics> = probe
        .disks
        .into_iter()
        .map(|d| DiskMetrics {
            used_percent: used_percent(d.used_bytes, d.total_bytes),
            mount: d.mount,
            total_bytes: d.total_bytes,
            used_bytes: d.used_bytes,
            available_bytes: d.available_bytes,
        })
        .collect();

    // Aggregate processes by service key.
    struct Agg {
        count: u32,
        cpu: f64,
        rss: u64,
        pids: Vec<u32>,
    }
    let mut aggs: std::collections::HashMap<&'static str, Agg> = std::collections::HashMap::new();
    for key in SERVICE_KEYS {
        aggs.insert(
            *key,
            Agg {
                count: 0,
                cpu: 0.0,
                rss: 0,
                pids: Vec::new(),
            },
        );
    }

    for proc in probe.processes {
        let Some(key) = match_service_key(&proc.comm, &proc.cmdline) else {
            continue;
        };
        let agg = aggs.get_mut(key).expect("pre-inserted service key");
        agg.count += 1;
        agg.cpu += proc.cpu_percent;
        agg.rss = agg.rss.saturating_add(proc.rss_bytes);
        if let Some(pid) = proc.pid {
            if agg.pids.len() < PID_CAP {
                agg.pids.push(pid);
            }
        }
    }

    let mut items = Vec::with_capacity(SERVICE_KEYS.len());
    let mut our_rss = 0u64;
    let mut our_cpu = 0.0f64;
    for key in SERVICE_KEYS {
        let agg = aggs.get(key).expect("pre-inserted");
        if *key == "fns-agent" || *key == "fns-server" {
            our_rss = our_rss.saturating_add(agg.rss);
            our_cpu += agg.cpu;
        }
        items.push(ServiceItem {
            key: (*key).to_string(),
            display_name: display_name_for(key),
            running: agg.count > 0,
            process_count: agg.count,
            cpu_percent: agg.cpu,
            memory_rss_bytes: agg.rss,
            pids: agg.pids.clone(),
        });
    }

    ServerStatusSnapshot {
        project_id: project_id.to_string(),
        ssh_host_alias: ssh_host_alias.to_string(),
        captured_at: captured_at.to_string(),
        ok: true,
        error: None,
        host: Some(HostMetrics {
            hostname: probe.hostname,
            uptime_seconds: probe.uptime_seconds,
            cpu: CpuMetrics {
                usage_percent: probe.cpu.usage_percent,
                load1: probe.cpu.load1,
                load5: probe.cpu.load5,
                load15: probe.cpu.load15,
                cores: probe.cpu.cores,
            },
            memory: MemoryMetrics {
                total_bytes: total,
                used_bytes: used,
                available_bytes: available,
                used_percent: used_percent(used, total),
            },
            disks,
        }),
        services: Some(ServicesMetrics {
            items,
            our_services_memory_rss_bytes: our_rss,
            our_services_cpu_percent: our_cpu,
        }),
    }
}

/// Parse `tmux list-windows -F` TSV output.
pub fn parse_tmux_list_windows(
    stdout: &str,
    project_id: &str,
    ssh_host_alias: &str,
    tmux_session: &str,
    captured_at: &str,
) -> ClaudeSessionsSnapshot {
    let mut windows = Vec::new();
    let mut active_index: Option<u32> = None;

    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 4 {
            return sessions_error(
                project_id,
                ssh_host_alias,
                tmux_session,
                captured_at,
                true,
                "parse_failed",
                "Unexpected tmux list-windows format",
            );
        }
        let index: u32 = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => {
                return sessions_error(
                    project_id,
                    ssh_host_alias,
                    tmux_session,
                    captured_at,
                    true,
                    "parse_failed",
                    "Invalid window index",
                );
            }
        };
        let name = parts[1].to_string();
        let active = parts[2] == "1";
        let pane_count: u32 = parts[3].parse().unwrap_or(1);
        let window_id = parts
            .get(4)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("@{index}"));
        let title = if name.is_empty() {
            format!("Window {index}")
        } else {
            name.clone()
        };
        if active {
            active_index = Some(index);
        }
        windows.push(ClaudeSessionWindow {
            index,
            id: format!("{tmux_session}:{index}"),
            name,
            title,
            active,
            pane_count,
            looks_like_claude: None,
        });
        let _ = window_id; // reserved for future stable id use
    }

    ClaudeSessionsSnapshot {
        project_id: project_id.to_string(),
        ssh_host_alias: ssh_host_alias.to_string(),
        tmux_session: tmux_session.to_string(),
        captured_at: captured_at.to_string(),
        ok: true,
        error: None,
        session_exists: true,
        windows,
        active_index,
    }
}

pub fn server_status_error(
    project_id: &str,
    ssh_host_alias: &str,
    captured_at: &str,
    code: &str,
    message: &str,
) -> ServerStatusSnapshot {
    ServerStatusSnapshot {
        project_id: project_id.to_string(),
        ssh_host_alias: ssh_host_alias.to_string(),
        captured_at: captured_at.to_string(),
        ok: false,
        error: Some(RemoteMonitorError {
            code: code.to_string(),
            message: message.to_string(),
        }),
        host: None,
        services: None,
    }
}

pub fn sessions_error(
    project_id: &str,
    ssh_host_alias: &str,
    tmux_session: &str,
    captured_at: &str,
    session_exists: bool,
    code: &str,
    message: &str,
) -> ClaudeSessionsSnapshot {
    ClaudeSessionsSnapshot {
        project_id: project_id.to_string(),
        ssh_host_alias: ssh_host_alias.to_string(),
        tmux_session: tmux_session.to_string(),
        captured_at: captured_at.to_string(),
        ok: false,
        error: Some(RemoteMonitorError {
            code: code.to_string(),
            message: message.to_string(),
        }),
        session_exists,
        windows: Vec::new(),
        active_index: None,
    }
}

/// Reject probe stdout larger than the hard cap.
pub fn enforce_stdout_bound(stdout: &str) -> Result<(), RemoteMonitorError> {
    if stdout.len() > STDOUT_CAP {
        return Err(RemoteMonitorError {
            code: "output_truncated".into(),
            message: "Probe output exceeded size limit".into(),
        });
    }
    Ok(())
}

/// Build the remote host probe shell script. `remote_root` is single-quoted inside.
pub fn build_host_probe_script(remote_root: &str) -> String {
    let quoted_root = TerminalManager::posix_shell_single_quote(remote_root);
    // Remote Python probe: only dynamic input is ROOT assignment (single-quoted path).
    // Heredoc body is fixed and uses <<'PY' so no expansion of user content.
    format!(
        "ROOT={root}; export ROOT; python3 - <<'PY'\n{body}\nPY",
        root = quoted_root,
        body = HOST_PROBE_PYTHON,
    )
}

/// Fixed remote probe body (no user input).
const HOST_PROBE_PYTHON: &str = r#"
import json, os, time, glob
root = os.environ.get("ROOT", "/")

def read_stat():
    with open("/proc/stat") as f:
        parts = f.readline().split()
    nums = list(map(int, parts[1:8]))
    idle = nums[3] + (nums[4] if len(nums) > 4 else 0)
    return sum(nums), idle

t1, i1 = read_stat()
time.sleep(0.3)
t2, i2 = read_stat()
dt = max(t2 - t1, 1)
di = i2 - i1
cpu = max(0.0, min(100.0, (dt - di) * 100.0 / dt))
load = open("/proc/loadavg").read().split()
cores = os.cpu_count() or 1
mem = {}
for line in open("/proc/meminfo"):
    bits = line.split()
    if len(bits) >= 2:
        mem[bits[0].rstrip(":")] = int(bits[1]) * 1024
total = mem.get("MemTotal", 0)
avail = mem.get("MemAvailable", 0)
uptime = float(open("/proc/uptime").read().split()[0])
host = os.uname().nodename
disks = []
seen = set()
for line in open("/proc/mounts"):
    parts = line.split()
    if len(parts) < 2:
        continue
    mp = parts[1]
    if mp != "/" and not (root == mp or root.startswith(mp.rstrip("/") + "/")):
        continue
    if mp in seen:
        continue
    try:
        st = os.statvfs(mp)
        tot = st.f_frsize * st.f_blocks
        free = st.f_frsize * st.f_bavail
        disks.append({
            "mount": mp,
            "totalBytes": tot,
            "usedBytes": max(tot - free, 0),
            "availableBytes": free,
        })
        seen.add(mp)
    except Exception:
        pass
if not disks:
    st = os.statvfs("/")
    tot = st.f_frsize * st.f_blocks
    free = st.f_frsize * st.f_bavail
    disks = [{
        "mount": "/",
        "totalBytes": tot,
        "usedBytes": max(tot - free, 0),
        "availableBytes": free,
    }]
procs = []
for path in glob.glob("/proc/[0-9]*/status"):
    try:
        pid = int(path.split("/")[2])
        comm = ""
        rss = 0
        for line in open(path):
            if line.startswith("Name:"):
                comm = line.split()[1]
            elif line.startswith("VmRSS:"):
                rss = int(line.split()[1]) * 1024
        cmd = ""
        try:
            raw = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\x00", b" ")
            cmd = raw.decode("utf-8", "replace").strip()
        except Exception:
            pass
        blob = f"{comm} {cmd}"
        keep = False
        if "fns-agent" in blob or "fns-server" in blob:
            keep = True
        first = cmd.split()[:1]
        if comm == "claude" or first == ["claude"]:
            keep = True
        if not keep:
            continue
        procs.append({
            "pid": pid,
            "comm": comm,
            "cmdline": cmd[:400],
            "rssBytes": rss,
            "cpuPercent": 0.0,
        })
    except Exception:
        pass
print(json.dumps({
    "hostname": host,
    "uptimeSeconds": int(uptime),
    "cpu": {
        "usagePercent": round(cpu, 2),
        "load1": float(load[0]),
        "load5": float(load[1]),
        "load15": float(load[2]),
        "cores": cores,
    },
    "memory": {"totalBytes": total, "availableBytes": avail},
    "disks": disks,
    "processes": procs,
}))
"#;

struct SshCapture {
    stdout: String,
    exit_code: i32,
}

fn run_ssh_capture(alias: &str, remote_command: &str) -> Result<SshCapture, RemoteMonitorError> {
    if alias.trim().is_empty() {
        return Err(RemoteMonitorError {
            code: "ssh_failed".into(),
            message: "SSH host alias is empty".into(),
        });
    }

    let mut child = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=8")
        .arg(alias)
        .arg(remote_command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| RemoteMonitorError {
            code: "ssh_failed".into(),
            message: "Failed to start SSH".into(),
        })?;

    let stdout_pipe = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            let mut chunk = [0u8; 4096];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() < STDOUT_CAP {
                            let room = STDOUT_CAP - buf.len();
                            buf.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let status = child.wait().ok();
        let _ = tx.send((buf, status));
    });

    match rx.recv_timeout(Duration::from_secs(SSH_TIMEOUT_SECS)) {
        Ok((buf, status)) => {
            if buf.len() >= STDOUT_CAP {
                return Err(RemoteMonitorError {
                    code: "output_truncated".into(),
                    message: "Probe output exceeded size limit".into(),
                });
            }
            let stdout = String::from_utf8_lossy(&buf).into_owned();
            enforce_stdout_bound(&stdout)?;
            let exit_code = status.and_then(|s| s.code()).unwrap_or(1);
            Ok(SshCapture { stdout, exit_code })
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Err(RemoteMonitorError {
            code: "timeout".into(),
            message: "Probe timed out".into(),
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(RemoteMonitorError {
            code: "ssh_failed".into(),
            message: "SSH capture worker disconnected".into(),
        }),
    }
}

fn load_project(project_id: &str) -> Result<ProjectConfig, RemoteMonitorError> {
    ProjectConfig::find_by_id(project_id).map_err(|_| RemoteMonitorError {
        code: "project_not_found".into(),
        message: "Project not found".into(),
    })
}

fn tmux_target(session: &str, window_index: u32) -> Result<String, String> {
    if window_index > 10_000 {
        return Err("window index out of range".into());
    }
    let session = validate_tmux_session_name(session)?;
    // session is charset-safe; index is numeric — form session:index inside quotes as 'sess:0'
    let target = format!("{session}:{window_index}");
    Ok(TerminalManager::posix_shell_single_quote(&target))
}

/// Tauri command: list tmux windows for this project's session.
#[tauri::command]
pub fn list_claude_sessions(project_id: String) -> ClaudeSessionsSnapshot {
    let captured_at = chrono::Utc::now().to_rfc3339();
    let project = match load_project(&project_id) {
        Ok(p) => p,
        Err(e) => {
            return sessions_error(
                &project_id,
                "",
                "",
                &captured_at,
                false,
                &e.code,
                &e.message,
            );
        }
    };

    let session = match validate_tmux_session_name(&project.tmux_session) {
        Ok(s) => s,
        Err(_) => {
            return sessions_error(
                &project_id,
                &project.ssh_host_alias,
                &project.tmux_session,
                &captured_at,
                false,
                "invalid_session_name",
                "Invalid tmux session name",
            );
        }
    };

    let quoted = TerminalManager::posix_shell_single_quote(&session);
    let cmd = format!(
        "tmux list-windows -t {quoted} -F '#{{window_index}}\t#{{window_name}}\t#{{window_active}}\t#{{window_panes}}\t#{{window_id}}'"
    );

    match run_ssh_capture(&project.ssh_host_alias, &cmd) {
        Ok(capture) => {
            if capture.exit_code != 0 {
                // tmux returns non-zero when session is missing
                return sessions_error(
                    &project_id,
                    &project.ssh_host_alias,
                    &session,
                    &captured_at,
                    false,
                    "tmux_session_missing",
                    "tmux session not found on remote host",
                );
            }
            parse_tmux_list_windows(
                &capture.stdout,
                &project_id,
                &project.ssh_host_alias,
                &session,
                &captured_at,
            )
        }
        Err(e) => sessions_error(
            &project_id,
            &project.ssh_host_alias,
            &session,
            &captured_at,
            false,
            &e.code,
            &e.message,
        ),
    }
}

/// Select a tmux window for this project's session.
#[tauri::command]
pub fn switch_claude_session(project_id: String, window_index: u32) -> Result<(), String> {
    let project = load_project(&project_id).map_err(|e| e.message)?;
    let target = tmux_target(&project.tmux_session, window_index)?;
    let cmd = format!("tmux select-window -t {target}");
    let capture = run_ssh_capture(&project.ssh_host_alias, &cmd).map_err(|e| e.message)?;
    if capture.exit_code != 0 {
        return Err("window_not_found".into());
    }
    Ok(())
}

/// Kill a single tmux window for this project's session (not global pkill).
#[tauri::command]
pub fn kill_claude_session(project_id: String, window_index: u32) -> Result<(), String> {
    let project = load_project(&project_id).map_err(|e| e.message)?;
    let target = tmux_target(&project.tmux_session, window_index)?;
    let cmd = format!("tmux kill-window -t {target}");
    let capture = run_ssh_capture(&project.ssh_host_alias, &cmd).map_err(|e| e.message)?;
    if capture.exit_code != 0 {
        return Err("window_not_found".into());
    }
    Ok(())
}

/// Tauri command: remote host CPU/memory/disk + service process RSS.
#[tauri::command]
pub fn get_server_status(project_id: String) -> ServerStatusSnapshot {
    let captured_at = chrono::Utc::now().to_rfc3339();
    let project = match ProjectConfig::find_by_id(&project_id) {
        Ok(p) => p,
        Err(_) => {
            return server_status_error(
                &project_id,
                "",
                &captured_at,
                "project_not_found",
                "Project not found",
            );
        }
    };

    let script = build_host_probe_script(&project.remote_root);
    match run_ssh_capture(&project.ssh_host_alias, &script) {
        Ok(capture) => {
            if capture.exit_code != 0 && capture.stdout.trim().is_empty() {
                return server_status_error(
                    &project_id,
                    &project.ssh_host_alias,
                    &captured_at,
                    "remote_cmd_failed",
                    "Remote probe failed",
                );
            }
            parse_server_status_payload(
                &capture.stdout,
                &project_id,
                &project.ssh_host_alias,
                &captured_at,
            )
        }
        Err(e) => server_status_error(
            &project_id,
            &project.ssh_host_alias,
            &captured_at,
            &e.code,
            &e.message,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_service_key_agent_server_claude() {
        assert_eq!(
            match_service_key("fns-agent", "/opt/fns/fns-agent --config x"),
            Some("fns-agent")
        );
        assert_eq!(
            match_service_key("fns-server", "/usr/local/bin/fns-server"),
            Some("fns-server")
        );
        assert_eq!(match_service_key("claude", "claude"), Some("claude"));
        assert_eq!(match_service_key("bash", "-bash"), None);
    }

    #[test]
    fn parse_server_status_sums_our_services_excluding_claude() {
        let raw = r#"{
          "hostname":"box",
          "uptimeSeconds":100,
          "cpu":{"usagePercent":12.5,"load1":0.2,"load5":0.3,"load15":0.1,"cores":4},
          "memory":{"totalBytes":1000,"availableBytes":400},
          "disks":[{"mount":"/","totalBytes":100,"usedBytes":50,"availableBytes":50}],
          "processes":[
            {"comm":"fns-agent","cmdline":"/bin/fns-agent","rssBytes":100,"cpuPercent":1.0,"pid":1},
            {"comm":"fns-server","cmdline":"/bin/fns-server","rssBytes":200,"cpuPercent":2.0,"pid":2},
            {"comm":"claude","cmdline":"claude","rssBytes":500,"cpuPercent":10.0,"pid":3}
          ]
        }"#;
        let snap = parse_server_status_payload(raw, "proj-1", "myhost", "2026-08-11T00:00:00Z");
        assert!(snap.ok);
        let services = snap.services.expect("services");
        assert_eq!(services.our_services_memory_rss_bytes, 300);
        assert!((services.our_services_cpu_percent - 3.0).abs() < 0.01);
        let claude = services
            .items
            .iter()
            .find(|i| i.key == "claude")
            .expect("claude row");
        assert_eq!(claude.memory_rss_bytes, 500);
        let host = snap.host.expect("host");
        assert_eq!(host.memory.used_bytes, 600);
    }

    #[test]
    fn parse_tmux_list_windows_tsv() {
        let raw = "0\tmain\t1\t1\t@1\n1\tfix auth\t0\t1\t@2\n";
        let snap = parse_tmux_list_windows(
            raw,
            "proj-1",
            "myhost",
            "fns-demo",
            "2026-08-11T00:00:00Z",
        );
        assert!(snap.ok);
        assert!(snap.session_exists);
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[1].title, "fix auth");
        assert_eq!(snap.active_index, Some(0));
    }

    #[test]
    fn validate_tmux_session_name_rejects_injection() {
        assert!(validate_tmux_session_name("good_session-1").is_ok());
        assert!(validate_tmux_session_name("evil;rm").is_err());
        assert!(validate_tmux_session_name("a:b").is_err());
        assert!(validate_tmux_session_name("").is_err());
    }

    #[test]
    fn snapshot_error_helper_sets_ok_false() {
        let s = server_status_error("p", "h", "2026-08-11T00:00:00Z", "timeout", "probe timed out");
        assert!(!s.ok);
        assert_eq!(s.error.unwrap().code, "timeout");
    }

    #[test]
    fn reject_stdout_over_cap() {
        let big = "x".repeat(65 * 1024);
        assert!(enforce_stdout_bound(&big).is_err());
        assert!(enforce_stdout_bound("ok").is_ok());
    }

    #[test]
    fn host_probe_script_embeds_quoted_remote_root() {
        let script = build_host_probe_script("/home/u/project");
        assert!(script.contains("'/home/u/project'"));
        assert!(script.contains("usagePercent"));
        // Injection in root must remain inside single quotes after escaping.
        let evil = build_host_probe_script("/tmp/x'$(id)");
        assert!(evil.contains("'\\''"));
    }

    #[test]
    fn parse_tmux_empty_stdout_is_existing_session_zero_windows() {
        let snap = parse_tmux_list_windows(
            "",
            "proj-1",
            "host",
            "sess",
            "2026-08-11T00:00:00Z",
        );
        assert!(snap.ok);
        assert!(snap.session_exists);
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn tmux_target_quotes_session_index() {
        let t = tmux_target("good_sess", 2).unwrap();
        assert_eq!(t, "'good_sess:2'");
        assert!(tmux_target("bad;sess", 1).is_err());
    }
}
