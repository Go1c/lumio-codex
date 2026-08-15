use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

const VERIFY_FAILED: &str = "CODEX_APP_VERIFY_FAILED";

pub fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let expected = expected.trim();
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(VERIFY_FAILED.to_string());
    }

    let mut file = std::fs::File::open(path).map_err(|_| VERIFY_FAILED.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|_| VERIFY_FAILED.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(VERIFY_FAILED.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::verify_sha256;
    use sha2::{Digest, Sha256};

    const BODY: &[u8] = b"official-codex-app-fixture\n";

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn verify_sha256_accepts_a_matching_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg");
        std::fs::write(&path, BODY).unwrap();
        verify_sha256(&path, &sha256_hex(BODY)).unwrap();
    }

    #[test]
    fn verify_sha256_rejects_a_mismatch_without_deleting_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg");
        std::fs::write(&path, BODY).unwrap();
        let err = verify_sha256(&path, &"0".repeat(64)).unwrap_err();
        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
        assert!(path.exists());
    }

    #[test]
    fn verify_sha256_rejects_an_ill_formed_expected_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkg");
        std::fs::write(&path, BODY).unwrap();
        let err = verify_sha256(&path, "not-a-hash").unwrap_err();
        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
    }
}
