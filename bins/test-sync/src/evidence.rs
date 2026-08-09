use crate::{io_error, HarnessError, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, Default)]
pub struct Redactor {
    exact_secret: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Redactor([REDACTED])")
    }
}

impl Redactor {
    pub fn with_secret(secret: &[u8]) -> Self {
        let exact_secret = std::str::from_utf8(secret)
            .ok()
            .filter(|secret| !secret.is_empty())
            .map(|secret| Zeroizing::new(secret.to_owned()));
        Self { exact_secret }
    }

    pub fn redact_value(&self, mut value: Value) -> Value {
        redact_nested(&mut value, self);
        value
    }

    pub fn redact_text(&self, value: &str) -> String {
        let exact_redacted = match self.exact_secret.as_deref() {
            Some(secret) => value.replace(secret, "[REDACTED]"),
            None => value.to_owned(),
        };
        redact_jwts(&exact_redacted)
    }
}

fn redact_nested(value: &mut Value, redactor: &Redactor) {
    match value {
        Value::String(text) => *text = redactor.redact_text(text),
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| redact_nested(item, redactor)),
        Value::Object(fields) => fields
            .values_mut()
            .for_each(|item| redact_nested(item, redactor)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_jwts(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if !is_jwt_core_byte(bytes[index]) {
            index += 1;
            continue;
        }
        if index != 0 && (is_jwt_core_byte(bytes[index - 1]) || bytes[index - 1] == b'.') {
            index += 1;
            continue;
        }
        if let Some(end) = jwt_candidate_end(input, index) {
            output.push_str(&input[copied..index]);
            output.push_str("[REDACTED]");
            copied = end;
            index = end;
        } else {
            index += 1;
        }
    }
    output.push_str(&input[copied..]);
    output
}

fn is_jwt_core_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn jwt_candidate_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut end = start;
    for segment in 0..3 {
        while end < bytes.len() && is_jwt_core_byte(bytes[end]) {
            end += 1;
        }
        while end < bytes.len() && bytes[end] == b'=' {
            end += 1;
        }
        if segment < 2 {
            if bytes.get(end) != Some(&b'.') {
                return None;
            }
            end += 1;
        }
    }
    if end < bytes.len() && (is_jwt_core_byte(bytes[end]) || matches!(bytes[end], b'.' | b'=')) {
        return None;
    }
    crate::secret::is_valid_jwt(&input[start..end]).then_some(end)
}

pub struct EvidenceWriter {
    root: PathBuf,
    redactor: Redactor,
}

impl std::fmt::Debug for EvidenceWriter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvidenceWriter")
            .field("root", &self.root)
            .finish()
    }
}

impl EvidenceWriter {
    pub fn create(run_id: &str, exact_secret: &[u8]) -> Result<Self> {
        let client_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .ok_or(HarnessError::InvalidConfiguration(
                "cannot resolve client workspace root",
            ))?;
        let evidence_root = client_root.join("target/e2e-evidence");
        Self::create_in(&evidence_root, run_id, exact_secret)
    }

    pub fn create_in(evidence_root: &Path, run_id: &str, exact_secret: &[u8]) -> Result<Self> {
        validate_run_id(run_id)?;
        if !evidence_root.is_absolute() {
            return Err(HarnessError::InvalidConfiguration(
                "evidence root must be an absolute path",
            ));
        }
        create_private_directory_all(evidence_root)?;
        let root = evidence_root.join(run_id);
        create_private_directory(&root)?;
        create_private_directory(&root.join("checkpoints"))?;
        Ok(Self {
            root,
            redactor: Redactor::with_secret(exact_secret),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_json(&self, relative: &str, value: &impl serde::Serialize) -> Result<()> {
        let path = evidence_path(&self.root, relative)?;
        let value = serde_json::to_value(value)?;
        let redacted = self.redactor.redact_value(value);
        let mut encoded = serde_json::to_vec_pretty(&redacted)?;
        encoded.push(b'\n');
        write_private_new(&path, &encoded)
    }

    pub fn append_event(&self, stream: &str, value: &impl serde::Serialize) -> Result<()> {
        if !matches!(stream, "process" | "protocol") {
            return Err(HarnessError::InvalidConfiguration(
                "unknown evidence event stream",
            ));
        }
        let path = self.root.join(format!("{stream}.jsonl"));
        let value = self.redactor.redact_value(serde_json::to_value(value)?);
        let mut encoded = serde_json::to_vec(&value)?;
        encoded.push(b'\n');
        append_private(&path, &encoded)
    }

    pub fn finalize(&self) -> Result<PathBuf> {
        let sums_path = self.root.join("SHA256SUMS");
        if sums_path.exists() {
            return Err(HarnessError::InvalidConfiguration(
                "evidence checksums were already finalized",
            ));
        }
        let mut files = Vec::new();
        collect_files(&self.root, &self.root, &mut files)?;
        files.sort();
        let mut sums = Vec::new();
        for relative in files {
            let digest = sha256_file(&self.root.join(&relative))?;
            writeln!(sums, "{digest}  {}", relative.replace('\\', "/"))
                .map_err(|error| io_error(&sums_path, error))?;
        }
        write_private_new(&sums_path, &sums)?;
        Ok(sums_path)
    }
}

fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty()
        || run_id.len() > 80
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(HarnessError::InvalidConfiguration(
            "run ID must be an ASCII slug",
        ));
    }
    Ok(())
}

fn evidence_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.starts_with('/')
        || relative
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(HarnessError::InvalidConfiguration(
            "evidence path is not a safe relative path",
        ));
    }
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        create_private_directory_all(parent)?;
    }
    Ok(path)
}

fn create_private_directory_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| io_error(path, error))?;
    set_private_directory(path)
}

fn create_private_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => set_private_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(
            HarnessError::InvalidConfiguration("evidence run directory already exists"),
        ),
        Err(error) => Err(io_error(path, error)),
    }
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error(path, error))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_private_new(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    set_private_file_options(&mut options);
    let mut file = options.open(path).map_err(|error| io_error(path, error))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(path, error))
}

fn append_private(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.append(true).create(true);
    set_private_file_options(&mut options);
    let mut file = options.open(path).map_err(|error| io_error(path, error))?;
    file.write_all(contents)
        .and_then(|()| file.flush())
        .map_err(|error| io_error(path, error))
}

#[cfg(unix)]
fn set_private_file_options(options: &mut fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_options(_options: &mut fs::OpenOptions) {}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|error| io_error(directory, error))? {
        let entry = entry.map_err(|error| io_error(directory, error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(HarnessError::InvalidConfiguration(
                "evidence directory contains a symlink",
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| HarnessError::InvalidConfiguration("evidence path escaped root"))?
                .to_str()
                .ok_or(HarnessError::InvalidConfiguration(
                    "evidence path is not UTF-8",
                ))?;
            files.push(relative.to_owned());
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| io_error(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}
