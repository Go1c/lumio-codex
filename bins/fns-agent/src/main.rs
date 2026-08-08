use clap::Parser;
use fns_agent::{
    AgentConfig,
    cli::{Cli, Command},
    daemon, run_diagnose, run_status,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { config } => match AgentConfig::load_linux(&config) {
            Ok(cfg) => {
                // Load the token from the separate token file.
                let token = match fns_platform::SecretToken::read_linux_file(&cfg.token_file) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("fns-agent: cannot read token: {e}");
                        return ExitCode::from(2);
                    }
                };

                // Run the daemon on a Tokio runtime.
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("fns-agent: runtime error: {e}");
                        return ExitCode::from(6);
                    }
                };

                match runtime.block_on(daemon::run(cfg, token)) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("fns-agent: {e}");
                        ExitCode::from(e.exit_code() as u8)
                    }
                }
            }
            Err(e) => {
                eprintln!("fns-agent: {e}");
                ExitCode::from(e.exit_code() as u8)
            }
        },
        Command::Status { config, json: _ } => {
            let path = config.unwrap_or_else(AgentConfig::default_config_path);
            match run_status(&path) {
                Ok(status) => {
                    let json = serde_json::to_string(&status).unwrap_or_else(|_| "{}".into());
                    println!("{json}");
                    if status.running {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(3)
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
