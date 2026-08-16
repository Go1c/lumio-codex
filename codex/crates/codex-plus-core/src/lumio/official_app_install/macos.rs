use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use super::sources::OPENAI_MAC_TEAM_ID;

const INSTALL_FAILED: &str = "CODEX_APP_INSTALL_FAILED";
const VERIFY_FAILED: &str = "CODEX_APP_VERIFY_FAILED";
const APP_BUNDLE_NAME: &str = "Codex.app";
const SYSTEM_APPLICATIONS: &str = "/Applications";

#[cfg(target_os = "macos")]
const OFFICIAL_APP_NAMES: &[&str] = &[
    "Codex.app",
    "OpenAI Codex.app",
    "OpenAI.Codex.app",
    "ChatGPT.app",
];

pub fn interpret_codesign_output(
    verify_ok: bool,
    details_stderr: &str,
    expected_team_id: &str,
) -> Result<(), String> {
    if !verify_ok {
        return Err(VERIFY_FAILED.to_string());
    }
    match team_identifier_from_details(details_stderr) {
        Some(actual) if actual == expected_team_id => Ok(()),
        _ => Err(VERIFY_FAILED.to_string()),
    }
}

pub fn choose_macos_dest(existing: Option<&Path>, system_writable: bool) -> PathBuf {
    if let Some(existing) = existing {
        return existing.to_path_buf();
    }
    if system_writable {
        PathBuf::from(SYSTEM_APPLICATIONS).join(APP_BUNDLE_NAME)
    } else {
        user_applications().join(APP_BUNDLE_NAME)
    }
}

pub fn user_applications() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Applications");
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join("Applications"))
        .unwrap_or_else(|| PathBuf::from("Applications"))
}

pub fn verify_macos_team_id(app: &Path, team_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        verify_macos_bundle(app, team_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, team_id);
        Err(VERIFY_FAILED.to_string())
    }
}

pub fn install_macos_from_dmg(dmg: &Path, dest_root: Option<&Path>) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        install_macos_from_dmg_live(dmg, dest_root)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (dmg, dest_root);
        Err(INSTALL_FAILED.to_string())
    }
}

fn install_macos_app_with<V, C>(
    source_app: &Path,
    dest: &Path,
    verify: V,
    copy: C,
) -> Result<PathBuf, String>
where
    V: FnOnce(&Path) -> Result<(), String>,
    C: FnOnce(&Path, &Path) -> Result<(), String>,
{
    verify(source_app)?;
    copy(source_app, dest)?;
    Ok(dest.to_path_buf())
}

fn team_identifier_from_details(details_stderr: &str) -> Option<&str> {
    details_stderr.lines().find_map(|line| {
        line.trim()
            .strip_prefix("TeamIdentifier=")
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

#[cfg(target_os = "macos")]
fn verify_macos_bundle(app: &Path, team_id: &str) -> Result<(), String> {
    use std::process::Command;

    let verify = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app)
        .output()
        .map_err(|_| VERIFY_FAILED.to_string())?;
    let details = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(app)
        .output()
        .map_err(|_| VERIFY_FAILED.to_string())?;
    interpret_codesign_output(
        verify.status.success(),
        &String::from_utf8_lossy(&details.stderr),
        team_id,
    )?;

    let assess = Command::new("/usr/sbin/spctl")
        .args(["--assess", "--type", "execute"])
        .arg(app)
        .output()
        .map_err(|_| VERIFY_FAILED.to_string())?;
    if assess.status.success() {
        Ok(())
    } else {
        Err(VERIFY_FAILED.to_string())
    }
}

#[cfg(target_os = "macos")]
fn install_macos_from_dmg_live(dmg: &Path, dest_root: Option<&Path>) -> Result<PathBuf, String> {
    if !dmg.is_file() {
        return Err(INSTALL_FAILED.to_string());
    }

    let mount = unique_mount_dir();
    std::fs::create_dir_all(&mount).map_err(|_| INSTALL_FAILED.to_string())?;
    let mut guard = MountGuard {
        mount: mount.clone(),
        attached: false,
    };
    attach_dmg(dmg, &guard.mount)?;
    guard.attached = true;

    let source =
        find_official_app_in_mount(&guard.mount).ok_or_else(|| INSTALL_FAILED.to_string())?;
    let existing = crate::app_paths::find_macos_codex_app_default();
    // 用户选了目录：.app 落进该目录；否则沿用既有推断（已装位置 → /Applications → ~/Applications）。
    let dest = match dest_root {
        Some(root) => root.join(APP_BUNDLE_NAME),
        None => choose_macos_dest(existing.as_deref(), system_applications_writable()),
    };
    install_macos_app_with(
        &source,
        &dest,
        |app| verify_macos_team_id(app, OPENAI_MAC_TEAM_ID),
        copy_app_ditto,
    )
}

#[cfg(target_os = "macos")]
fn unique_mount_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("lumio-codex-dmg-{}-{stamp}", std::process::id()))
}

#[cfg(target_os = "macos")]
fn system_applications_writable() -> bool {
    dir_is_writable(Path::new(SYSTEM_APPLICATIONS))
}

#[cfg(target_os = "macos")]
fn dir_is_writable(path: &Path) -> bool {
    let probe = path.join(format!(".lumio-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn find_official_app_in_mount(mount: &Path) -> Option<PathBuf> {
    for name in OFFICIAL_APP_NAMES {
        let candidate = mount.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let entries = std::fs::read_dir(mount).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        for name in OFFICIAL_APP_NAMES {
            let nested = path.join(name);
            if nested.is_dir() {
                return Some(nested);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn attach_dmg(dmg: &Path, mount: &Path) -> Result<(), String> {
    use std::process::Command;

    let output = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(mount)
        .arg(dmg)
        .output()
        .map_err(|_| INSTALL_FAILED.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(INSTALL_FAILED.to_string())
    }
}

#[cfg(target_os = "macos")]
fn detach_dmg(mount: &Path) -> bool {
    use std::process::Command;

    let quiet = Command::new("/usr/bin/hdiutil")
        .args(["detach", "-quiet"])
        .arg(mount)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if quiet {
        return true;
    }
    Command::new("/usr/bin/hdiutil")
        .args(["detach", "-quiet", "-force"])
        .arg(mount)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn copy_app_ditto(src: &Path, dest: &Path) -> Result<(), String> {
    use std::process::Command;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|_| INSTALL_FAILED.to_string())?;
    }
    // ditto copies *into* dest when dest already exists as a directory, so stage first.
    let bundle_name = dest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new(APP_BUNDLE_NAME))
        .to_string_lossy();
    let staging = dest.with_file_name(format!(".{bundle_name}.lumio-new"));
    let backup = dest.with_file_name(format!(".{bundle_name}.lumio-old"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|_| INSTALL_FAILED.to_string())?;
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(|_| INSTALL_FAILED.to_string())?;
    }

    let status = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(&staging)
        .status()
        .map_err(|_| INSTALL_FAILED.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(INSTALL_FAILED.to_string());
    }

    if dest.exists() && std::fs::rename(dest, &backup).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(INSTALL_FAILED.to_string());
    }
    if std::fs::rename(&staging, dest).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        if backup.exists() {
            let _ = std::fs::rename(&backup, dest);
        }
        return Err(INSTALL_FAILED.to_string());
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

#[cfg(target_os = "macos")]
struct MountGuard {
    mount: PathBuf,
    attached: bool,
}

#[cfg(target_os = "macos")]
impl Drop for MountGuard {
    fn drop(&mut self) {
        if self.attached {
            if detach_dmg(&self.mount) {
                let _ = std::fs::remove_dir(&self.mount);
            }
            return;
        }
        let _ = std::fs::remove_dir_all(&self.mount);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        choose_macos_dest, install_macos_app_with, interpret_codesign_output, user_applications,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn rejects_bundle_when_team_id_is_not_openai() {
        let err = interpret_codesign_output(true, "TeamIdentifier=AAAAAAAAAA\n", "2DC432GLL2")
            .unwrap_err();
        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
    }

    #[test]
    fn accepts_openai_team_id() {
        interpret_codesign_output(true, "TeamIdentifier=2DC432GLL2\n", "2DC432GLL2").unwrap();
    }

    #[test]
    fn prefers_existing_official_location() {
        assert_eq!(
            choose_macos_dest(Some(Path::new("/Applications/Codex.app")), true),
            PathBuf::from("/Applications/Codex.app")
        );
    }

    #[test]
    fn falls_back_to_user_applications_when_system_not_writable() {
        assert_eq!(
            choose_macos_dest(None, false).file_name().unwrap(),
            "Codex.app"
        );
        assert!(choose_macos_dest(None, false).starts_with(user_applications()));
    }

    #[test]
    fn verify_failure_does_not_copy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("mnt").join("Codex.app");
        let dest = dir.path().join("Applications").join("Codex.app");
        std::fs::create_dir_all(&source).unwrap();

        let mut copied = false;
        let err = install_macos_app_with(
            &source,
            &dest,
            |_app| Err("CODEX_APP_VERIFY_FAILED".into()),
            |_src, dest| {
                copied = true;
                std::fs::create_dir_all(dest).unwrap();
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(err, "CODEX_APP_VERIFY_FAILED");
        assert!(!copied, "copier must not run after verify failure");
        assert!(!dest.exists(), "dest must not receive a copied .app");
    }
}
