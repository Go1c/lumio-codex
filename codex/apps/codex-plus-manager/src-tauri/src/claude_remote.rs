//! Remote host snapshots for 服务器状态 / 对话状态.
//!
//! User-visible fields never include the session tool or sidecar process names.

use crate::claude_ssh::{posix_single_quote, remote_shell_path};
use crate::claude_terminal::TerminalManager;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatusError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatusSnapshot {
    pub project_id: String,
    pub captured_at: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RemoteStatusError>,
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
    pub cores: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetrics {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServicesMetrics {
    pub items: Vec<ServiceItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationWindow {
    pub index: u32,
    pub id: String,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSnapshot {
    pub project_id: String,
    pub captured_at: String,
    pub ok: bool,
    pub session_exists: bool,
    pub windows: Vec<ConversationWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RemoteStatusError>,
}

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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteProcessProbe {
    #[serde(default)]
    comm: String,
    #[serde(default)]
    cmdline: String,
    #[serde(default)]
    rss_bytes: u64,
    #[serde(default)]
    cpu_percent: f64,
}

const SERVICE_KEYS: &[&str] = &["sync", "workspace", "claude"];

pub fn captured_at_now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

pub fn public_service_key(comm: &str, cmdline: &str) -> Option<&'static str> {
    let blob = format!("{comm} {cmdline}");
    if blob.contains("fns-agent") {
        return Some("sync");
    }
    if blob.contains("fns-server") {
        return Some("workspace");
    }
    if comm == "claude" || cmdline.split_whitespace().next() == Some("claude") {
        return Some("claude");
    }
    None
}

pub fn service_display_name(key: &str) -> String {
    match key {
        "sync" => "同步组件".into(),
        "workspace" => "远端服务".into(),
        "claude" => "Claude".into(),
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

pub fn server_status_error(
    project_id: &str,
    captured_at: &str,
    code: &str,
    message: &str,
) -> ServerStatusSnapshot {
    ServerStatusSnapshot {
        project_id: project_id.to_string(),
        captured_at: captured_at.to_string(),
        ok: false,
        error: Some(RemoteStatusError {
            code: code.to_string(),
            message: message.to_string(),
        }),
        host: None,
        services: None,
    }
}

pub fn parse_server_status_payload(
    stdout: &str,
    project_id: &str,
    captured_at: &str,
) -> ServerStatusSnapshot {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return server_status_error(
            project_id,
            captured_at,
            "parse_failed",
            "没能读取服务器状态。",
        );
    }
    let probe: RemoteHostProbe = match serde_json::from_str(trimmed) {
        Ok(probe) => probe,
        Err(_) => {
            return server_status_error(
                project_id,
                captured_at,
                "parse_failed",
                "没能读取服务器状态。",
            );
        }
    };

    let available = probe.memory.available_bytes;
    let total = probe.memory.total_bytes;
    let used = total.saturating_sub(available);
    let disks: Vec<DiskMetrics> = probe
        .disks
        .into_iter()
        .map(|disk| DiskMetrics {
            used_percent: used_percent(disk.used_bytes, disk.total_bytes),
            mount: disk.mount,
            total_bytes: disk.total_bytes,
            used_bytes: disk.used_bytes,
        })
        .collect();

    struct Agg {
        count: u32,
        cpu: f64,
        rss: u64,
    }
    let mut aggs = std::collections::HashMap::new();
    for key in SERVICE_KEYS {
        aggs.insert(
            *key,
            Agg {
                count: 0,
                cpu: 0.0,
                rss: 0,
            },
        );
    }
    for proc in probe.processes {
        let Some(key) = public_service_key(&proc.comm, &proc.cmdline) else {
            continue;
        };
        let agg = aggs.get_mut(key).expect("pre-inserted");
        agg.count += 1;
        agg.cpu += proc.cpu_percent;
        agg.rss = agg.rss.saturating_add(proc.rss_bytes);
    }
    let items = SERVICE_KEYS
        .iter()
        .map(|key| {
            let agg = aggs.get(key).expect("pre-inserted");
            ServiceItem {
                key: (*key).to_string(),
                display_name: service_display_name(key),
                running: agg.count > 0,
                process_count: agg.count,
                cpu_percent: agg.cpu,
                memory_rss_bytes: agg.rss,
            }
        })
        .collect();

    ServerStatusSnapshot {
        project_id: project_id.to_string(),
        captured_at: captured_at.to_string(),
        ok: true,
        error: None,
        host: Some(HostMetrics {
            hostname: probe.hostname,
            uptime_seconds: probe.uptime_seconds,
            cpu: CpuMetrics {
                usage_percent: probe.cpu.usage_percent,
                load1: probe.cpu.load1,
                cores: probe.cpu.cores,
            },
            memory: MemoryMetrics {
                total_bytes: total,
                used_bytes: used,
                used_percent: used_percent(used, total),
            },
            disks,
        }),
        services: Some(ServicesMetrics { items }),
    }
}

pub fn conversations_error(
    project_id: &str,
    captured_at: &str,
    session_exists: bool,
    code: &str,
    message: &str,
) -> ConversationSnapshot {
    ConversationSnapshot {
        project_id: project_id.to_string(),
        captured_at: captured_at.to_string(),
        ok: false,
        session_exists,
        windows: Vec::new(),
        error: Some(RemoteStatusError {
            code: code.to_string(),
            message: message.to_string(),
        }),
    }
}

pub fn parse_conversation_windows(
    stdout: &str,
    project_id: &str,
    captured_at: &str,
) -> ConversationSnapshot {
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed.contains("__NO_SESSION__") {
        return ConversationSnapshot {
            project_id: project_id.to_string(),
            captured_at: captured_at.to_string(),
            ok: true,
            session_exists: false,
            windows: Vec::new(),
            error: None,
        };
    }
    if trimmed.to_ascii_lowercase().contains("error connecting")
        || trimmed.to_ascii_lowercase().contains("no server running")
        || trimmed.to_ascii_lowercase().contains("session not found")
    {
        return ConversationSnapshot {
            project_id: project_id.to_string(),
            captured_at: captured_at.to_string(),
            ok: true,
            session_exists: false,
            windows: Vec::new(),
            error: None,
        };
    }

    let mut windows = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 3 {
            return conversations_error(
                project_id,
                captured_at,
                true,
                "parse_failed",
                "没能读取对话状态。",
            );
        }
        let Ok(index) = parts[0].parse::<u32>() else {
            return conversations_error(
                project_id,
                captured_at,
                true,
                "parse_failed",
                "没能读取对话状态。",
            );
        };
        let name = parts[1].to_string();
        let active = parts[2] == "1";
        let title = if name.is_empty() {
            format!("窗口 {index}")
        } else {
            name
        };
        windows.push(ConversationWindow {
            index,
            id: format!("{project_id}:{index}"),
            title,
            active,
        });
    }
    ConversationSnapshot {
        project_id: project_id.to_string(),
        captured_at: captured_at.to_string(),
        ok: true,
        session_exists: true,
        windows,
        error: None,
    }
}

pub fn build_host_probe_script(remote_root: &str) -> String {
    format!(
        "ROOT={root}; export ROOT; python3 - <<'PY'\n{body}\nPY",
        root = posix_single_quote(&strip_for_probe(remote_root)),
        body = HOST_PROBE_PYTHON,
    )
}

fn strip_for_probe(remote_root: &str) -> String {
    if remote_root.starts_with('~') || remote_root.starts_with('/') {
        remote_root.to_string()
    } else {
        remote_shell_path(remote_root)
            .trim_matches('\'')
            .to_string()
    }
}

pub fn conversation_list_command(project_id: &str) -> String {
    let session = TerminalManager::sanitize_session_name(project_id);
    let quoted = TerminalManager::posix_shell_single_quote(&session);
    format!(
        "if command -v tmux >/dev/null 2>&1; then tmux list-windows -t {quoted} -F '#{{window_index}}\t#{{window_name}}\t#{{window_active}}\t#{{window_panes}}\t#{{window_id}}' 2>/dev/null || echo __NO_SESSION__; else echo __NO_SESSION__; fi"
    )
}

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
        "cores": cores,
    },
    "memory": {"totalBytes": total, "availableBytes": avail},
    "disks": disks,
    "processes": procs,
}))
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_keys_are_user_facing() {
        assert_eq!(
            public_service_key("fns-agent", "/opt/fns-agent run"),
            Some("sync")
        );
        assert_eq!(service_display_name("sync"), "同步组件");
        assert!(
            !service_display_name("sync")
                .to_ascii_lowercase()
                .contains("agent")
        );
    }

    #[test]
    fn parse_server_status_maps_host_and_services() {
        let stdout = r#"{
            "hostname": "box",
            "uptimeSeconds": 3600,
            "cpu": {"usagePercent": 12.5, "load1": 0.4, "cores": 4},
            "memory": {"totalBytes": 8000, "availableBytes": 2000},
            "disks": [{"mount": "/", "totalBytes": 100, "usedBytes": 40}],
            "processes": [{"comm": "claude", "cmdline": "claude", "rssBytes": 10, "cpuPercent": 1.0}]
        }"#;
        let snap = parse_server_status_payload(stdout, "p1", "1");
        assert!(snap.ok);
        let host = snap.host.expect("host");
        assert_eq!(host.hostname.as_deref(), Some("box"));
        assert_eq!(host.memory.used_bytes, 6000);
        let services = snap.services.expect("services");
        let claude = services
            .items
            .iter()
            .find(|item| item.key == "claude")
            .expect("claude");
        assert!(claude.running);
        assert_eq!(claude.display_name, "Claude");
    }

    #[test]
    fn conversation_windows_parse_without_session_tool_name() {
        let snap = parse_conversation_windows("0\tmain\t1\t1\t@1\n1\tlogs\t0\t1\t@2\n", "p1", "1");
        assert!(snap.ok);
        assert!(snap.session_exists);
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].title, "main");
        assert!(snap.windows[0].active);
        let encoded = serde_json::to_string(&snap).unwrap();
        assert!(!encoded.to_ascii_lowercase().contains("tmux"));
        assert!(!encoded.to_ascii_lowercase().contains("agent"));
    }

    #[test]
    fn missing_conversation_session_is_empty_not_error() {
        let snap = parse_conversation_windows("__NO_SESSION__", "p1", "1");
        assert!(snap.ok);
        assert!(!snap.session_exists);
        assert!(snap.windows.is_empty());
    }
}
