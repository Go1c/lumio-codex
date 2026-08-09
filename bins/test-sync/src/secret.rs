use crate::{io_error, HarnessError, Result};
use std::fs::File;
use std::io::{IsTerminal, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug)]
pub enum TokenSource {
    Stdin,
    Descriptor(u32),
}

pub struct SecretMaterial {
    bytes: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretMaterial([REDACTED])")
    }
}

impl SecretMaterial {
    pub fn read(source: TokenSource) -> Result<Self> {
        let mut bytes = Zeroizing::new(Vec::new());
        let descriptor = match source {
            TokenSource::Stdin => {
                if std::io::stdin().is_terminal() {
                    return Err(HarnessError::InvalidConfiguration(
                        "JWT stdin must be a private pipe, not a terminal",
                    ));
                }
                0
            }
            TokenSource::Descriptor(descriptor) => {
                if descriptor < 3 {
                    return Err(HarnessError::InvalidConfiguration(
                        "JWT descriptor must be inherited as fd 3 or greater",
                    ));
                }
                i32::try_from(descriptor).map_err(|_| {
                    HarnessError::InvalidConfiguration("JWT descriptor is out of range")
                })?
            }
        };
        let source = OwnedSourceDescriptor::acquire(descriptor)?;
        let file = source.duplicate()?;
        validate_private_descriptor(&file)?;
        file.take(fns_platform::MAX_TOKEN_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("private JWT descriptor", error))?;
        drop(source);
        strip_one_line_ending(&mut bytes);
        validate_jwt(&bytes)?;
        Ok(Self { bytes })
    }

    pub fn agent_secret(&self) -> fns_agent::protocol::SecretBytes {
        fns_agent::protocol::SecretBytes::new(self.bytes.to_vec())
    }

    pub(crate) fn redaction_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn strip_one_line_ending(bytes: &mut Vec<u8>) {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
}

fn validate_jwt(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() as u64 > fns_platform::MAX_TOKEN_BYTES {
        return Err(HarnessError::InvalidConfiguration("JWT size is invalid"));
    }
    let token = std::str::from_utf8(bytes)
        .map_err(|_| HarnessError::InvalidConfiguration("JWT must be ASCII"))?;
    if !is_valid_jwt(token) {
        return Err(HarnessError::InvalidConfiguration(
            "JWT must contain three canonical base64url segments",
        ));
    }
    Ok(())
}

pub(crate) fn is_valid_jwt(token: &str) -> bool {
    let mut segments = token.split('.');
    let valid = (0..3).all(|_| segments.next().is_some_and(is_valid_base64url_segment));
    valid && segments.next().is_none()
}

fn is_valid_base64url_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let core_len = bytes
        .iter()
        .position(|byte| *byte == b'=')
        .unwrap_or(bytes.len());
    if core_len == 0
        || !bytes[..core_len]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return false;
    }
    let padding = bytes.len() - core_len;
    if !bytes[core_len..].iter().all(|byte| *byte == b'=') || padding > 2 {
        return false;
    }
    if padding == 0 {
        core_len % 4 != 1
    } else {
        bytes.len().is_multiple_of(4) && core_len % 4 == 4 - padding
    }
}

struct OwnedSourceDescriptor {
    descriptor: OwnedFd,
}

impl OwnedSourceDescriptor {
    fn acquire(raw: RawFd) -> Result<Self> {
        // The caller explicitly transfers this inherited descriptor to the harness.
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let flags = rustix::io::fcntl_getfd(&descriptor).map_err(|_| {
            HarnessError::InvalidConfiguration("inherited JWT descriptor is not open")
        })?;
        rustix::io::fcntl_setfd(&descriptor, flags | rustix::io::FdFlags::CLOEXEC).map_err(
            |_| {
                HarnessError::InvalidConfiguration(
                    "JWT descriptor could not be made non-inheritable",
                )
            },
        )?;
        Ok(Self { descriptor })
    }

    fn duplicate(&self) -> Result<File> {
        let raw = self.descriptor.as_raw_fd();
        let candidates = [format!("/dev/fd/{raw}"), format!("/proc/self/fd/{raw}")];
        for candidate in candidates {
            match File::open(&candidate) {
                Ok(file) => return Ok(file),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(candidate, error)),
            }
        }
        Err(HarnessError::InvalidConfiguration(
            "inherited JWT descriptor could not be owned",
        ))
    }
}

#[cfg(unix)]
fn validate_private_descriptor(file: &File) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let metadata = file
        .metadata()
        .map_err(|error| io_error("inherited JWT descriptor", error))?;
    let kind = metadata.file_type();
    if kind.is_fifo() {
        if metadata.uid() == rustix::process::getuid().as_raw() && metadata.nlink() == 0 {
            return Ok(());
        }
        return Err(HarnessError::InvalidConfiguration(
            "JWT pipe must be an anonymous private endpoint",
        ));
    }
    if !kind.is_file() {
        return Err(HarnessError::InvalidConfiguration(
            "JWT descriptor must be a private regular file or anonymous pipe",
        ));
    }
    if metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(HarnessError::InvalidConfiguration(
            "JWT descriptor file must be owner-only",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_descriptor(_file: &File) -> Result<()> {
    Err(HarnessError::InvalidConfiguration(
        "inherited JWT descriptors require Unix",
    ))
}
