use clap::Parser;
use fns_agent::{
    AgentCommand, AgentConfig, AgentProcess, AgentProcessOptions,
    cli::{Cli, Command},
    run_diagnose, run_status,
};
use std::io::Read;
use std::process::ExitCode;
use zeroize::Zeroizing;

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            config,
            token_stdin,
        } => match AgentConfig::load_linux(&config) {
            Ok(cfg) => {
                let token = match load_run_token(&cfg, token_stdin) {
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

                let executable = match std::env::current_exe() {
                    Ok(executable) => executable,
                    Err(_) => return ExitCode::from(6),
                };
                let supervised = async move {
                    let mut child = AgentProcess::spawn(
                        AgentCommand::new(executable).arg("__worker"),
                        cfg,
                        token,
                        AgentProcessOptions::default(),
                    )
                    .await?;
                    tokio::select! {
                        result = child.wait() => result.map(|_| ()),
                        signal = wait_for_parent_signal() => {
                            let shutdown = child.shutdown().await;
                            match signal {
                                Ok(()) => shutdown,
                                Err(error) => Err(error),
                            }
                        }
                    }
                };
                match runtime.block_on(supervised) {
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
        Command::Worker => {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return ExitCode::from(6),
            };
            match runtime.block_on(fns_agent::worker::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => ExitCode::from(error.exit_code() as u8),
            }
        }
    }
}

fn load_run_token(
    config: &AgentConfig,
    token_stdin: bool,
) -> Result<fns_platform::SecretToken, String> {
    if !token_stdin {
        return fns_platform::SecretToken::read_linux_file(&config.token_file)
            .map_err(|error| error.to_string());
    }

    let mut bytes = Zeroizing::new(Vec::new());
    std::io::stdin()
        .take(fns_platform::MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "private token input failed".to_string())?;
    fns_platform::SecretToken::from_private_ipc(std::mem::take(&mut *bytes))
        .map_err(|error| error.to_string())
}

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn wait_for_parent_signal() -> Result<(), fns_agent::AgentError> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| fns_agent::AgentError::new(fns_agent::AgentErrorCode::Core))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|_| fns_agent::AgentError::new(fns_agent::AgentErrorCode::Core))
            }
            signal = terminate.recv() => {
                signal.ok_or_else(|| fns_agent::AgentError::new(fns_agent::AgentErrorCode::Core))
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|_| fns_agent::AgentError::new(fns_agent::AgentErrorCode::Core))
    }
}
