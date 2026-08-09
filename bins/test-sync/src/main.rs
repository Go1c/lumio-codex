use clap::Parser;
use std::process::ExitCode;
use test_sync::cli::{Cli, Command};
use tokio_util::sync::CancellationToken;

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    match arguments.next().as_deref() {
        Some(mode) if mode == std::ffi::OsStr::new("__exec-clean") => {
            if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--")) {
                eprintln!("test-sync exec-clean failed: missing separator");
                return ExitCode::FAILURE;
            }
            return match test_sync::process::exec_clean(arguments) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("test-sync exec-clean failed: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        Some(mode) if mode == std::ffi::OsStr::new("__exec-pinned") => {
            let descriptor_count = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|count| *count > 0);
            let mut descriptors = descriptor_count.map(Vec::with_capacity).unwrap_or_default();
            for _ in 0..descriptor_count.unwrap_or_default() {
                let Some(descriptor) = arguments
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<i32>().ok())
                    .filter(|descriptor| *descriptor >= 3)
                else {
                    eprintln!("test-sync exec-pinned failed: invalid descriptor arguments");
                    return ExitCode::FAILURE;
                };
                descriptors.push(descriptor);
            }
            if descriptor_count.is_none()
                || arguments.next().as_deref() != Some(std::ffi::OsStr::new("--"))
            {
                eprintln!("test-sync exec-pinned failed: invalid descriptor arguments");
                return ExitCode::FAILURE;
            }
            return match test_sync::process::exec_pinned(descriptors, arguments) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("test-sync exec-pinned failed: {error}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("test-sync runtime failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run())
}

async fn run() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let cancellation = CancellationToken::new();
    let signal = cancellation.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.cancel();
        }
    });
    let result = match cli.command {
        Command::Run(args) => test_sync::harness::run(args, cancellation).await,
    };
    match result {
        Ok(evidence) => {
            println!("e2e evidence: {}", evidence.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("test-sync failed: {error}");
            ExitCode::FAILURE
        }
    }
}
