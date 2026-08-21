//! CLI definition using clap.

use std::path::PathBuf;

/// The fns-agent CLI.
#[derive(clap::Parser)]
#[command(name = "fns-agent", version, disable_help_subcommand = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Run the agent daemon.
    Run {
        #[arg(long)]
        config: PathBuf,
        /// Read the token once from inherited stdin instead of a Linux file.
        #[arg(long)]
        token_stdin: bool,
    },
    /// Print agent status as JSON.
    Status {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, required = true)]
        json: bool,
    },
    /// Run diagnostic checks and print results as JSON.
    Diagnose {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, required = true)]
        json: bool,
    },
    #[command(name = "__worker", hide = true)]
    Worker,
}
