use clap::Parser;
use fns_agent::{
    AgentConfig,
    cli::{Cli, Command},
    run_diagnose, run_status,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { config } => {
            // Validate config, then print "not yet wired" for now.
            // Full daemon implementation is in the daemon module (Task 8 sub-task).
            match AgentConfig::load_linux(&config) {
                Ok(_) => {
                    eprintln!("fns-agent: configuration valid, daemon not yet wired");
                    ExitCode::from(3) // stopped
                }
                Err(e) => {
                    eprintln!("fns-agent: {e}");
                    ExitCode::from(e.exit_code() as u8)
                }
            }
        }
        Command::Status { config, json: _ } => {
            let path = config.unwrap_or_else(AgentConfig::default_config_path);
            match run_status(&path) {
                Ok(status) => {
                    // JSON is the only stdout output.
                    let json = serde_json::to_string(&status).unwrap_or_else(|_| "{}".into());
                    println!("{json}");
                    if status.running {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(3) // stopped
                    }
                }
                Err(e) => {
                    eprintln!("fns-agent: {e}");
                    ExitCode::from(e.exit_code() as u8)
                }
            }
        }
        Command::Diagnose { config, json: _ } => {
            let path = config.unwrap_or_else(AgentConfig::default_config_path);
            let report = run_diagnose(&path);
            let json = serde_json::to_string(&report).unwrap_or_else(|_| "{}".into());
            println!("{json}");
            if report.healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
    }
}
