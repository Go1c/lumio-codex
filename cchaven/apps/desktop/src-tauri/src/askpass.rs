//! Hands an SSH password to OpenSSH without it ever touching disk or argv.
//!
//! OpenSSH asks a helper program named by `SSH_ASKPASS` for the password. This
//! app points that at its own executable and passes the address of a private
//! unix socket; the re-executed binary connects, reads the password from the
//! parent process's memory and prints it. Nothing is written to disk, no
//! environment variable holds the secret (environments are readable by other
//! processes of the same user), and the socket lives in a 0700 directory.
//!
//! The app targets macOS; the non-unix build keeps the same API so the crate
//! still type-checks on the workspace's Windows CI leg.

/// Environment variable carrying the socket path to the re-executed helper.
pub const SOCKET_ENV: &str = "CCHAVEN_ASKPASS_SOCKET";

#[cfg(unix)]
pub use unix_impl::{AskpassServer, read_secret, run_askpass_helper};

#[cfg(not(unix))]
pub use fallback::{AskpassServer, run_askpass_helper};

#[cfg(unix)]
mod unix_impl {
    use super::SOCKET_ENV;

    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tokio::io::AsyncWriteExt;
    use tokio::net::{UnixListener, UnixStream};

    /// A one-project askpass endpoint, alive for the duration of one ssh call.
    pub struct AskpassServer {
        dir: PathBuf,
        socket_path: PathBuf,
        task: tokio::task::JoinHandle<()>,
    }

    impl AskpassServer {
        /// Bind the socket and start serving `password` to local callers.
        pub async fn start(password: &str) -> std::io::Result<Self> {
            // Kept short on purpose: unix socket paths cap out around 104 bytes.
            let dir = PathBuf::from("/tmp").join(format!(
                "cchaven-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            ));
            std::fs::create_dir_all(&dir)?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;

            let socket_path = dir.join("s");
            let listener = UnixListener::bind(&socket_path)?;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

            // The secret lives only in this task's stack, for as long as ssh
            // may ask for it.
            let secret = password.to_string();
            let task = tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let _ = stream.write_all(secret.as_bytes()).await;
                    let _ = stream.write_all(b"\n").await;
                    let _ = stream.shutdown().await;
                }
            });

            Ok(Self {
                dir,
                socket_path,
                task,
            })
        }

        pub fn socket_path(&self) -> &Path {
            &self.socket_path
        }

        /// Point an `ssh`/`scp` invocation at this endpoint.
        pub fn configure(&self, command: &mut Command) -> Result<(), String> {
            let exe = std::env::current_exe().map_err(|e| format!("无法定位程序路径：{e}"))?;
            command.env("SSH_ASKPASS", exe);
            // OpenSSH ≥ 8.4 only consults the helper when told to, or when there
            // is no terminal; force it so behaviour does not depend on how the
            // app was launched.
            command.env("SSH_ASKPASS_REQUIRE", "force");
            command.env("DISPLAY", ":0");
            command.env(SOCKET_ENV, &self.socket_path);
            Ok(())
        }

        /// Stop serving and remove the socket directory.
        pub async fn shutdown(self) {
            self.task.abort();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Askpass mode: print the password from the socket and exit.
    ///
    /// Called from `main` before any GUI initialisation, because OpenSSH
    /// re-executes this very binary as the helper.
    pub fn run_askpass_helper(socket_path: &str) -> std::process::ExitCode {
        use std::io::Read;
        use std::os::unix::net::UnixStream as StdUnixStream;

        match StdUnixStream::connect(socket_path) {
            Ok(mut stream) => {
                let mut secret = String::new();
                if stream.read_to_string(&mut secret).is_err() {
                    return std::process::ExitCode::FAILURE;
                }
                // OpenSSH takes the first line as the password.
                print!("{secret}");
                std::process::ExitCode::SUCCESS
            }
            Err(_) => std::process::ExitCode::FAILURE,
        }
    }

    /// Read the password the way the helper does; used by tests and by callers
    /// that want to verify the endpoint before spawning ssh.
    pub async fn read_secret(socket_path: &Path) -> std::io::Result<String> {
        use tokio::io::AsyncReadExt;

        let mut stream = UnixStream::connect(socket_path).await?;
        let mut secret = String::new();
        stream.read_to_string(&mut secret).await?;
        Ok(secret.trim_end_matches('\n').to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn serves_the_password_to_a_local_caller() {
            let server = AskpassServer::start("hunter2").await.expect("start");
            assert_eq!(
                read_secret(server.socket_path()).await.expect("read"),
                "hunter2"
            );
            // ssh may ask more than once (for example after a
            // keyboard-interactive round trip), so the endpoint stays available
            // until shutdown.
            assert_eq!(
                read_secret(server.socket_path()).await.expect("read again"),
                "hunter2"
            );
            server.shutdown().await;
        }

        #[tokio::test]
        async fn the_socket_directory_is_private_and_removed_on_shutdown() {
            let server = AskpassServer::start("hunter2").await.expect("start");
            let dir = server.dir.clone();
            let mode = std::fs::metadata(&dir).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o700);

            server.shutdown().await;
            assert!(!dir.exists());
        }

        #[tokio::test]
        async fn the_password_is_never_placed_in_the_child_environment() {
            let server = AskpassServer::start("hunter2").await.expect("start");
            let mut command = Command::new("ssh");
            server.configure(&mut command).expect("configure");

            let env: Vec<(String, String)> = command
                .get_envs()
                .filter_map(|(key, value)| {
                    Some((
                        key.to_string_lossy().into_owned(),
                        value?.to_string_lossy().into_owned(),
                    ))
                })
                .collect();
            assert!(env.iter().all(|(_, value)| value != "hunter2"));
            assert!(env.iter().any(|(key, _)| key == "SSH_ASKPASS"));
            assert!(
                env.iter()
                    .any(|(key, value)| key == "SSH_ASKPASS_REQUIRE" && value == "force")
            );
            assert!(env.iter().any(|(key, _)| key == SOCKET_ENV));
            server.shutdown().await;
        }

        #[tokio::test]
        async fn socket_paths_stay_within_the_unix_length_limit() {
            let server = AskpassServer::start("x").await.expect("start");
            assert!(server.socket_path().as_os_str().len() < 100);
            server.shutdown().await;
        }
    }
}

#[cfg(not(unix))]
mod fallback {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// Password authentication needs a unix socket; other platforms fall back to
    /// key-based authentication.
    pub struct AskpassServer {
        socket_path: PathBuf,
    }

    impl AskpassServer {
        pub async fn start(_password: &str) -> std::io::Result<Self> {
            Err(std::io::Error::other(
                "该平台不支持密码登录，请使用 SSH 密钥",
            ))
        }

        pub fn socket_path(&self) -> &Path {
            &self.socket_path
        }

        pub fn configure(&self, _command: &mut Command) -> Result<(), String> {
            Err("该平台不支持密码登录，请使用 SSH 密钥。".into())
        }

        pub async fn shutdown(self) {}
    }

    pub fn run_askpass_helper(_socket_path: &str) -> std::process::ExitCode {
        std::process::ExitCode::FAILURE
    }
}
