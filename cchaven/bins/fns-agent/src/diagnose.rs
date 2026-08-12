//! Diagnostics: read-only checks that never load token bytes or open a socket.

use crate::config::AgentConfig;

use std::path::Path;

/// Diagnostic check status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Pass,
    Warning,
    Fail,
}

/// A single diagnostic check result.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub name: &'static str,
    pub status: DiagnosticStatus,
    pub code: &'static str,
}

/// A full diagnostic report.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub schema_version: &'static str,
    pub healthy: bool,
    pub checks: Vec<DiagnosticCheck>,
}

/// Run diagnostics on a config file.
pub fn run_diagnostics(config_path: &Path) -> DiagnosticReport {
    let mut checks = Vec::new();

    // Check 1: config file exists and is readable.
    let config = AgentConfig::load_linux(config_path);
    checks.push(match &config {
        Ok(_) => DiagnosticCheck {
            name: "config_file",
            status: DiagnosticStatus::Pass,
            code: "ok",
        },
        Err(_) => DiagnosticCheck {
            name: "config_file",
            status: DiagnosticStatus::Fail,
            code: "unreadable",
        },
    });

    let config = match config {
        Ok(c) => c,
        Err(_) => {
            // Can't run further checks without a valid config.
            return DiagnosticReport {
                schema_version: "fns-agent-diagnose/1",
                healthy: false,
                checks,
            };
        }
    };

    // Check 2: config schema valid (already validated by load).
    checks.push(DiagnosticCheck {
        name: "config_schema",
        status: DiagnosticStatus::Pass,
        code: "valid",
    });

    // Check 3: endpoint is loopback.
    let endpoint_ok = fns_transport::WorkspaceEndpoint::parse(&config.endpoint).is_ok();
    checks.push(DiagnosticCheck {
        name: "endpoint_loopback",
        status: if endpoint_ok {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Fail
        },
        code: if endpoint_ok {
            "loopback"
        } else {
            "non_loopback"
        },
    });

    // Check 4: token file exists and is private.
    #[cfg(target_os = "linux")]
    {
        let token_ok = fns_platform::verify_private_regular_linux(&config.token_file).is_ok();
        checks.push(DiagnosticCheck {
            name: "token_file",
            status: if token_ok {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            },
            code: if token_ok {
                "private"
            } else {
                "insecure_or_missing"
            },
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        checks.push(DiagnosticCheck {
            name: "token_file",
            status: DiagnosticStatus::Warning,
            code: "not_linux",
        });
    }

    // Check 5: workspace root exists.
    checks.push(DiagnosticCheck {
        name: "workspace_root",
        status: if config.workspace_root.exists() {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Warning
        },
        code: if config.workspace_root.exists() {
            "exists"
        } else {
            "missing"
        },
    });

    // Check 6: state dir exists.
    checks.push(DiagnosticCheck {
        name: "state_dir",
        status: if config.state_dir.exists() {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Warning
        },
        code: if config.state_dir.exists() {
            "exists"
        } else {
            "missing"
        },
    });

    // Check 7: singleton lease (probe only — don't signal or kill).
    match fns_platform::StateDirLease::probe(&config.state_dir) {
        Ok(false) => checks.push(DiagnosticCheck {
            name: "singleton_lock",
            status: DiagnosticStatus::Pass,
            code: "not_running",
        }),
        Ok(true) => checks.push(DiagnosticCheck {
            name: "singleton_lock",
            status: DiagnosticStatus::Warning,
            code: "already_running",
        }),
        Err(_) => checks.push(DiagnosticCheck {
            name: "singleton_lock",
            status: DiagnosticStatus::Fail,
            code: "corrupt",
        }),
    }

    // Check 8: runtime status file.
    let status_path = config.state_dir.join("runtime-status.json");
    checks.push(DiagnosticCheck {
        name: "runtime_status",
        status: if status_path.exists() {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Warning
        },
        code: if status_path.exists() {
            "present"
        } else {
            "absent"
        },
    });

    let healthy = !checks.iter().any(|c| c.status == DiagnosticStatus::Fail);

    DiagnosticReport {
        schema_version: "fns-agent-diagnose/1",
        healthy,
        checks,
    }
}
