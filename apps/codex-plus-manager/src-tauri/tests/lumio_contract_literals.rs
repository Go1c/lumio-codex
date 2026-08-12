//! 跨语言契约字面量。`health` 与凭据状态的取值目前由 Rust 单方面定义，前端靠它们分支：
//! 拼错一个字母不会报错，只会静默走进 else 并显示错误的修复引导。所以这里用**实际序列化**
//! （而不是比对源码里的常量字符串）把取值钉死，再顺着前端源码核对一遍对齐情况。

use std::fs;
use std::path::PathBuf;

use codex_plus_core::lumio::config_takeover::TakeoverHealth;
use codex_plus_core::lumio::credentials::CredentialStatus;
use codex_plus_manager_lib::lumio_commands::{LumioBootstrapPayload, takeover_health_payload};

const HEALTH_VALUES: [&str; 3] = ["not-applied", "healthy", "conflicted"];
const CREDENTIAL_STATUS_VALUES: [&str; 3] = ["present", "missing", "invalid"];

#[test]
fn the_takeover_health_payload_serializes_the_three_values_the_ui_branches_on() {
    // 变体穷尽匹配：新增一个 TakeoverHealth 变体时这里编译不过，
    // 逼迫作者同时更新前端的分支，而不是让 UI 静默走 else。
    for health in [
        TakeoverHealth::NotApplied,
        TakeoverHealth::Healthy,
        TakeoverHealth::Conflicted {
            error_code: "CODEX_CONFIG_CONFLICT".to_string(),
        },
    ] {
        let expected = match &health {
            TakeoverHealth::NotApplied => {
                r#"{"health":"not-applied","errorCode":null}"#.to_string()
            }
            TakeoverHealth::Healthy => r#"{"health":"healthy","errorCode":null}"#.to_string(),
            TakeoverHealth::Conflicted { error_code } => {
                format!(r#"{{"health":"conflicted","errorCode":"{error_code}"}}"#)
            }
        };

        let payload = takeover_health_payload(health);
        assert_eq!(serde_json::to_string(&payload).unwrap(), expected);
        assert!(HEALTH_VALUES.contains(&payload.health.as_str()));
    }
}

#[test]
fn the_bootstrap_payload_serializes_the_three_credential_states_the_ui_branches_on() {
    for status in [
        CredentialStatus::Present,
        CredentialStatus::Missing,
        CredentialStatus::Invalid,
    ] {
        let expected = match status {
            CredentialStatus::Present => "present",
            CredentialStatus::Missing => "missing",
            CredentialStatus::Invalid => "invalid",
        };

        let serialized = serde_json::to_string(&bootstrap_payload(status)).unwrap();
        assert!(
            serialized.contains(&format!(r#""credentialStatus":"{expected}""#)),
            "{serialized}"
        );
        assert!(CREDENTIAL_STATUS_VALUES.contains(&expected));
    }
}

/// 前端 `types.ts` 的联合类型与 Rust 枚举必须逐项相等。前端仍在并发开发中，
/// 联合类型还没落地时跳过而不判失败；一旦落地，取值集合必须完全一致。
#[test]
fn the_frontend_credential_status_union_matches_the_rust_enum() {
    let source = fs::read_to_string(frontend_dir().join("types.ts")).expect("lumio/types.ts");
    let Some(members) = union_members(&source, "LumioCredentialStatus") else {
        return;
    };

    assert_eq!(members, CREDENTIAL_STATUS_VALUES);
}

/// 前端每一处对 `health` 的字面量比较都必须是 Rust 真的会发出的取值。
/// 一处都没有时不判失败（前端可能还没接这段分支），但只要出现就必须合法。
#[test]
fn the_frontend_never_compares_health_against_a_value_rust_cannot_emit() {
    for file in frontend_sources() {
        let source = fs::read_to_string(&file).expect("frontend source");
        for literal in health_comparison_literals(&source) {
            assert!(
                HEALTH_VALUES.contains(&literal.as_str()),
                "{}: compares health against {literal:?}, which the command layer never emits",
                file.display()
            );
        }
    }
}

fn bootstrap_payload(credential_status: CredentialStatus) -> LumioBootstrapPayload {
    LumioBootstrapPayload {
        version: "0.0.0".to_string(),
        platform: "test".to_string(),
        arch: "test".to_string(),
        codex_app: None,
        account: None,
        telemetry_enabled: false,
        auto_update_enabled: true,
        credential_status,
    }
}

fn frontend_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/lumio")
}

fn frontend_sources() -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let mut pending = vec![frontend_dir()];
    while let Some(dir) = pending.pop() {
        let entries = fs::read_dir(&dir).unwrap_or_else(|_| panic!("read {}", dir.display()));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("ts" | "tsx")
            ) {
                sources.push(path);
            }
        }
    }
    assert!(!sources.is_empty(), "no frontend sources under lumio/");
    sources
}

/// 取出 `export type Name = "a" | "b";` 的成员。找不到声明时返回 `None`。
fn union_members(source: &str, name: &str) -> Option<Vec<String>> {
    let declaration = source.split_once(&format!("type {name}"))?.1;
    let body = declaration.split_once('=')?.1.split_once(';')?.0;
    Some(
        body.split('|')
            .map(|member| {
                member
                    .trim()
                    .trim_matches(['"', '\''].as_slice())
                    .to_string()
            })
            .filter(|member| !member.is_empty())
            .collect(),
    )
}

/// 取出所有 `health === "…"` / `health !== "…"` 比较里的字面量。
fn health_comparison_literals(source: &str) -> Vec<String> {
    let mut literals = Vec::new();
    for operator in ["health === ", "health !== "] {
        for tail in source.split(operator).skip(1) {
            let mut characters = tail.chars();
            let Some(quote @ ('"' | '\'')) = characters.next() else {
                continue;
            };
            literals.push(characters.take_while(|value| *value != quote).collect());
        }
    }
    literals
}
