use std::path::PathBuf;

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() {
    init_tracing();

    let mut tunnel = std::process::Command::new("ssh")
        .arg("-N")
        .arg("-L")
        .arg("127.0.0.1:19050:127.0.0.1:9000")
        .arg("vps-108-80-81-15")
        .spawn()
        .expect("Failed to start SSH tunnel");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let workspace_id =
        fns_protocol::WorkspaceId::parse("10000000-0000-4000-8000-000000000002").unwrap();
    let client_id = fns_protocol::ClientId::parse(&uuid::Uuid::new_v4().to_string()).unwrap();

    let state_dir = PathBuf::from("/tmp/fns-test-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let workspace_root = PathBuf::from("/tmp/fns-test-sync");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis();
    let acceptance_path = workspace_root.join(format!("acceptance-{stamp}.txt"));
    let acceptance_content = format!("mutation roundtrip {stamp}\n");
    std::fs::write(&acceptance_path, acceptance_content.as_bytes())
        .expect("write mutation fixture");
    eprintln!("Prepared local mutation: {}", acceptance_path.display());

    let config = fns_agent::AgentConfig {
        schema_version: "fns-agent-config/1".into(),
        endpoint: "ws://127.0.0.1:19050/api/user/workspace-sync/v2".into(),
        workspace_id,
        client_id,
        workspace_root: workspace_root.clone(),
        state_dir: state_dir.clone(),
        token_file: PathBuf::from("/dev/null"),
        sync: fns_agent::config::AgentSyncConfig {
            includes: vec!["**".into()],
            excludes: vec![],
            protect_secrets: true,
        },
        transport: fns_agent::config::AgentTransportConfig {
            max_active_transfers: 2,
        },
    };

    let token = fns_platform::SecretToken::from_bytes_for_test(b"dummy");
    let shutdown = tokio_util::sync::CancellationToken::new();

    // Shut down only after the initial snapshot and the complete mutation →
    // BlobNeed(upload) → BlobEnd → mutation retry path have had time to finish.
    let mutation_shutdown = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(35)).await;
        mutation_shutdown.cancel();
    });

    eprintln!("Starting agent...");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        fns_agent::daemon::run_embedded(config, token, shutdown),
    )
    .await;

    match result {
        Ok(Ok(())) => eprintln!("Agent completed normally"),
        Ok(Err(e)) => eprintln!("Agent error: {:?}", e),
        Err(_) => eprintln!("Agent timed out (still running)"),
    }

    eprintln!("\nFiles synced:");
    if let Ok(entries) = std::fs::read_dir("/tmp/fns-test-sync") {
        for entry in entries.flatten() {
            eprintln!("  {}", entry.file_name().to_string_lossy());
        }
    }

    let _ = tunnel.kill();
    let _ = tunnel.wait();
}
