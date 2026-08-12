use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Field names that must never leave the redactor intact.
pub const SECRET_KEYS: &[&str] = &[
    "token",
    "password",
    "authorization",
    "auth",
    "secret",
    "api_key",
    "apikey",
    "api-key",
    "x-api-key",
    "access_token",
    "refresh_token",
    "sshprivatekey",
    "ssh_private_key",
    "private_key",
    "privatekey",
    "emailcode",
    "email_code",
    "verification_code",
    "smtp_password",
    "aws_secret_access_key",
    "filebody",
    "file_body",
    "content",
    "body",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RedactionSummary {
    pub secret_hits: u64,
    pub path_redactions: u64,
    pub fields_removed: u64,
}

impl RedactionSummary {
    pub fn merge(&mut self, other: &RedactionSummary) {
        self.secret_hits += other.secret_hits;
        self.path_redactions += other.path_redactions;
        self.fields_removed += other.fields_removed;
    }
}

/// Fingerprint a path for safe logging: hash + depth + extension.
pub fn path_fingerprint(path: &str) -> BTreeMap<String, Value> {
    let normalized = path.replace('\\', "/");
    let depth = normalized
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .count();
    let extension = std::path::Path::new(&normalized)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let hash = blake3::hash(normalized.as_bytes());
    let mut map = BTreeMap::new();
    map.insert(
        "pathHash".into(),
        Value::String(hex::encode(&hash.as_bytes()[..16])),
    );
    map.insert("relativeDepth".into(), Value::from(depth as u64));
    map.insert("extension".into(), Value::String(extension));
    map
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase().replace('-', "_");
    SECRET_KEYS.iter().any(|k| {
        let norm = k.replace('-', "_");
        lower == norm || lower.contains(&norm)
    })
}

fn looks_like_absolute_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("~/")
        || (value.len() > 2 && value.as_bytes()[1] == b':' && value.as_bytes()[2] == b'\\')
}

/// Redact a free-form string: mask bearer tokens, private key blocks, password assignments.
pub fn redact_string(input: &str) -> (String, RedactionSummary) {
    let mut summary = RedactionSummary::default();
    let mut out = input.to_string();

    // PEM private keys
    if out.contains("BEGIN") && out.contains("PRIVATE KEY") {
        out = "[REDACTED_PRIVATE_KEY]".to_string();
        summary.secret_hits += 1;
        return (out, summary);
    }

    // Bearer tokens
    let lower = out.to_ascii_lowercase();
    if let Some(bearer_at) = lower.find("bearer ") {
        let mut token_end = bearer_at + "bearer ".len();
        while token_end < out.len() {
            let b = out.as_bytes()[token_end];
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
                token_end += 1;
            } else {
                break;
            }
        }
        out.replace_range(bearer_at..token_end, "Bearer [REDACTED]");
        summary.secret_hits += 1;
    }

    // password=... patterns
    for needle in ["password=", "PASSWORD=", "smtp_password=", "api_token="] {
        if let Some(start) = out.find(needle) {
            let value_start = start + needle.len();
            let value_end = out[value_start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map(|i| value_start + i)
                .unwrap_or(out.len());
            out.replace_range(value_start..value_end, "[REDACTED]");
            summary.secret_hits += 1;
        }
    }

    (out, summary)
}

/// Deep-redact a JSON object tree. Removes secret keys, fingerprints paths.
pub fn redact_fields(input: &Value) -> (Value, RedactionSummary) {
    let mut summary = RedactionSummary::default();
    let redacted = redact_value(input, &mut summary, false);
    (redacted, summary)
}

fn redact_value(value: &Value, summary: &mut RedactionSummary, parent_is_path: bool) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if is_secret_key(k) {
                    summary.secret_hits += 1;
                    summary.fields_removed += 1;
                    out.insert(k.clone(), Value::String("[REDACTED]".into()));
                    continue;
                }
                let is_path_key = k.eq_ignore_ascii_case("path")
                    || k.eq_ignore_ascii_case("absolutepath")
                    || k.eq_ignore_ascii_case("absolute_path")
                    || k.eq_ignore_ascii_case("localroot")
                    || k.eq_ignore_ascii_case("local_root")
                    || k.eq_ignore_ascii_case("remoteroot")
                    || k.eq_ignore_ascii_case("remote_root");
                if is_path_key {
                    if let Some(path) = v.as_str() {
                        summary.path_redactions += 1;
                        let fp = path_fingerprint(path);
                        out.insert(k.clone(), Value::Object(fp.into_iter().collect()));
                        continue;
                    }
                }
                if k.eq_ignore_ascii_case("env") && v.is_object() {
                    // Drop full environment maps; keep only count.
                    summary.fields_removed += 1;
                    summary.secret_hits += 1;
                    let count = v.as_object().map(|m| m.len()).unwrap_or(0);
                    out.insert(
                        k.clone(),
                        serde_json::json!({ "redacted": true, "keyCount": count }),
                    );
                    continue;
                }
                out.insert(k.clone(), redact_value(v, summary, is_path_key));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| redact_value(v, summary, parent_is_path))
                .collect(),
        ),
        Value::String(s) => {
            if parent_is_path || looks_like_absolute_path(s) {
                summary.path_redactions += 1;
                return Value::Object(path_fingerprint(s).into_iter().collect());
            }
            let (text, partial) = redact_string(s);
            summary.merge(&partial);
            Value::String(text)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_secret_keys_and_paths() {
        let raw = json!({
            "token": "Bearer abc.def",
            "password": "secret",
            "absolutePath": "/Users/alice/projects/app/main.rs",
            "safe": "ok",
            "env": { "PATH": "/bin", "API_TOKEN": "sk-live-x" }
        });
        let (redacted, summary) = redact_fields(&raw);
        assert_eq!(redacted["token"], "[REDACTED]");
        assert_eq!(redacted["password"], "[REDACTED]");
        assert_eq!(redacted["safe"], "ok");
        assert!(redacted["absolutePath"]["pathHash"].is_string());
        assert!(redacted["env"]["redacted"].as_bool().unwrap());
        assert!(summary.secret_hits >= 3);
        assert!(summary.path_redactions >= 1);
    }

    #[test]
    fn secret_corpus_patterns_do_not_survive_redaction() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../../contracts/diagnostics/fixtures/secret-corpus.json"
        ))
        .unwrap();
        let sample = &corpus["sample_raw"];
        let (redacted, _) = redact_fields(sample);
        let text = serde_json::to_string(&redacted).unwrap();
        for pattern in corpus["patterns"].as_array().unwrap() {
            let p = pattern.as_str().unwrap();
            assert!(
                !text.contains(p),
                "redacted output still contains secret pattern: {p}\n{text}"
            );
        }
    }
}
