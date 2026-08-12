//! 敏感文件的原子写。`crate::settings::atomic_write` 以默认 umask（通常 0644）创建临时
//! 文件、rename 之后才由调用方 chmod 0600——在 rename 之前的那个窗口里，含明文令牌与
//! API Key 的临时文件对同机其他用户可读。那个函数被旧 Codex++ 代码大量复用、不在本期
//! 范围内，所以这里给 Lumio 的敏感文件单独实现一份「创建时就带 0600」的写入。
//!
//! 见 `.spec/decisions/0001-lumio-credentials-local-file.md`：本地文件 + 收紧权限是本期
//! 替代系统凭据库的**全部**保护，一个 0644 的窗口就把这个 ADR 的前提打掉了。

use std::path::{Path, PathBuf};

/// 原子写一个只有属主可读写的文件。
///
/// Unix 下临时文件在 `open(2)` 那一刻就是 0600，不存在「先按 umask 落盘、事后 chmod」的
/// 窗口；rename 保留的是临时文件那个 inode 的权限，所以最终文件同样是 0600。
/// 其他平台没有 umask 语义（权限由目录 ACL 继承），沿用既有的原子写。
pub(super) fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let temp = create_owner_only_temp(path, bytes)?;
        if let Err(error) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(error.into());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        crate::settings::atomic_write(path, bytes)
    }
}

/// 写好内容、权限已经收紧、但**还没** rename 到最终路径的临时文件。
#[cfg(unix)]
fn create_owner_only_temp(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path_for(path);
    // `mode` 只在创建新文件时生效：残留的临时文件会带着它自己的旧权限被复用，
    // 所以先清掉、再用 `create_new` 保证我们拿到的是一个刚创建的 inode。
    let _ = std::fs::remove_file(&temp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(bytes)?;
    // 令牌被截断等于强制重新登录，这一份内容值得等一次 fsync。
    file.sync_all()?;
    Ok(temp)
}

#[cfg(unix)]
fn temp_path_for(path: &Path) -> PathBuf {
    let mut temp = path.to_path_buf();
    let extension = path.extension().and_then(|value| value.to_str());
    temp.set_extension(match extension {
        Some(extension) => format!("{extension}.secret-tmp"),
        None => "secret-tmp".to_string(),
    });
    temp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// 权限窗口就在这里：临时文件在被 rename 之前就必须是 0600，
    /// 「先落盘再 chmod」把明文令牌暴露给同机其他用户的时间不为零。
    #[cfg(unix)]
    #[test]
    fn the_temp_file_is_owner_only_before_it_is_renamed_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("credentials.json");

        let temp = create_owner_only_temp(&target, b"rt_secret").unwrap();

        let mode = std::fs::metadata(&temp).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
        assert!(!target.exists(), "the rename must not have happened yet");
        assert_eq!(std::fs::read(&temp).unwrap(), b"rt_secret");
    }

    #[cfg(unix)]
    #[test]
    fn the_written_file_is_owner_only_even_when_it_used_to_be_world_readable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("credentials.json");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_secret(&target, b"rt_secret").unwrap();

        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "unexpected mode {:o}", mode & 0o777);
    }

    #[test]
    fn writing_replaces_the_target_and_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join("credentials.json");

        write_secret(&target, b"first").unwrap();
        write_secret(&target, b"second").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        let leftovers = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "credentials.json")
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
    }
}
