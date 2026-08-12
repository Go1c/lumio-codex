use crate::secret::TokenSource;
use crate::{HarnessError, Result};
use clap::{ArgGroup, Args, Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "test-sync", about = "Bounded real-service FNS E2E harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Run(RunArgs),
    /// Isolation-only self-test orchestrator (requires testOnly=true profile).
    SelfTest(SelfTestArgs),
}

#[derive(Clone, Debug, Args)]
pub struct SelfTestArgs {
    /// Path to a JSON/TOML self-test profile (must set testOnly=true).
    #[arg(long)]
    pub profile: PathBuf,
    /// Parent directory for sandboxes (default: system temp).
    #[arg(long)]
    pub sandbox_parent: Option<PathBuf>,
    /// Wall-clock budget in seconds for the whole run (optional).
    #[arg(long)]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Args)]
#[command(group(
    ArgGroup::new("token_source")
        .required(true)
        .args(["token_stdin", "token_fd"])
))]
pub struct RunArgs {
    #[arg(long)]
    pub endpoint_a: String,
    #[arg(long)]
    pub endpoint_b: String,
    #[arg(long)]
    pub workspace_id: String,
    #[arg(long)]
    pub client_id_a: String,
    #[arg(long)]
    pub client_id_b: String,
    #[arg(long)]
    pub root_a: PathBuf,
    #[arg(long)]
    pub root_b: PathBuf,
    #[arg(long)]
    pub state_a: PathBuf,
    #[arg(long)]
    pub state_b: PathBuf,
    #[arg(long)]
    pub agent_binary: PathBuf,
    #[arg(long)]
    pub reconnect_hook: PathBuf,
    #[arg(long)]
    pub app_restart_hook: PathBuf,
    #[arg(long)]
    pub effect_observer: PathBuf,
    #[arg(long)]
    pub run_id: String,
    #[arg(long)]
    pub evidence_root: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub token_stdin: bool,
    #[arg(long)]
    pub token_fd: Option<u32>,
    #[arg(long, default_value_t = 30)]
    pub startup_timeout_seconds: u64,
    #[arg(long, default_value_t = 120)]
    pub checkpoint_timeout_seconds: u64,
    #[arg(long, default_value_t = 250)]
    pub sample_interval_millis: u64,
    #[arg(long, default_value_t = 30)]
    pub hook_timeout_seconds: u64,
    #[arg(long, default_value_t = 3)]
    pub term_grace_seconds: u64,
    #[arg(long, default_value_t = 3)]
    pub kill_timeout_seconds: u64,
    #[arg(long, default_value_t = 33_554_432)]
    pub large_file_bytes: u64,
    #[arg(long, default_value_t = 2)]
    pub max_active_transfers: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub startup: Duration,
    pub checkpoint: Duration,
    pub sample_interval: Duration,
    pub hook: Duration,
    pub term_grace: Duration,
    pub kill: Duration,
}

impl RunArgs {
    pub fn token_source(&self) -> Result<TokenSource> {
        match (self.token_stdin, self.token_fd) {
            (true, None) => Ok(TokenSource::Stdin),
            (false, Some(descriptor)) => Ok(TokenSource::Descriptor(descriptor)),
            _ => Err(HarnessError::InvalidConfiguration(
                "select exactly one JWT source",
            )),
        }
    }

    pub fn timeouts(&self) -> Result<Timeouts> {
        let values = [
            self.startup_timeout_seconds,
            self.checkpoint_timeout_seconds,
            self.sample_interval_millis,
            self.hook_timeout_seconds,
            self.term_grace_seconds,
            self.kill_timeout_seconds,
        ];
        if values.contains(&0) {
            return Err(HarnessError::InvalidConfiguration(
                "timeouts and sample interval must be positive",
            ));
        }
        Ok(Timeouts {
            startup: Duration::from_secs(self.startup_timeout_seconds),
            checkpoint: Duration::from_secs(self.checkpoint_timeout_seconds),
            sample_interval: Duration::from_millis(self.sample_interval_millis),
            hook: Duration::from_secs(self.hook_timeout_seconds),
            term_grace: Duration::from_secs(self.term_grace_seconds),
            kill: Duration::from_secs(self.kill_timeout_seconds),
        })
    }
}
