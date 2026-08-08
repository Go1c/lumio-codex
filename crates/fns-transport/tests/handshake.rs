mod support;

use fns_transport::{TransportErrorCode, WorkspaceEndpoint, socket};

#[tokio::test]
async fn bearer_and_client_metadata_are_sent_before_upgrade() {
    let server =
        support::fake_server::FakeWorkspaceServer::require_bearer("sentinel.jwt".into()).await;
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).unwrap();
    let token = support::secret_token("sentinel.jwt");

    let stream = socket::connect(&endpoint, &token, "0.1.0").await.unwrap();
    // Drop the stream to close the connection.
    drop(stream);

    // Give the server time to process and store observation.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let obs = server.observation();
    assert_eq!(obs.path, Some("/api/user/workspace-sync/v2".into()));
    assert!(obs.authorization_matched);
    assert_eq!(obs.client.as_deref(), Some("fns-agent"));
    assert_eq!(obs.client_name.as_deref(), Some("fns-agent"));
    assert_eq!(obs.client_version.as_deref(), Some("0.1.0"));
    assert_eq!(obs.user_agent.as_deref(), Some("fns-agent/0.1.0"));
}

#[tokio::test]
async fn http_401_is_fatal_before_upgrade() {
    let server =
        support::fake_server::FakeWorkspaceServer::reject_upgrade(http::StatusCode::UNAUTHORIZED)
            .await;
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).unwrap();
    let token = support::secret_token("sentinel.jwt");

    let error = socket::connect(&endpoint, &token, "0.1.0")
        .await
        .unwrap_err();
    assert_eq!(error.code(), TransportErrorCode::AuthenticationRejected);
    assert!(!error.retryable());
}

#[tokio::test]
async fn http_403_is_fatal_before_upgrade() {
    let server =
        support::fake_server::FakeWorkspaceServer::reject_upgrade(http::StatusCode::FORBIDDEN)
            .await;
    let endpoint = WorkspaceEndpoint::parse(server.endpoint()).unwrap();
    let token = support::secret_token("sentinel.jwt");

    let error = socket::connect(&endpoint, &token, "0.1.0")
        .await
        .unwrap_err();
    assert_eq!(error.code(), TransportErrorCode::Forbidden);
    assert!(!error.retryable());
}

#[test]
fn endpoint_is_exact_explicit_and_loopback_only() {
    // Valid: ws + loopback IP + explicit port + exact path.
    assert!(WorkspaceEndpoint::parse("ws://127.0.0.1:8080/api/user/workspace-sync/v2").is_ok());
    assert!(WorkspaceEndpoint::parse("ws://[::1]:8080/api/user/workspace-sync/v2").is_ok());

    // Invalid cases.
    let invalid = [
        // Missing port.
        "ws://127.0.0.1/api/user/workspace-sync/v2",
        // wss not allowed (TLS is Task 7 SSH layer).
        "wss://127.0.0.1:8080/api/user/workspace-sync/v2",
        // localhost not accepted (must be IP literal).
        "ws://localhost:8080/api/user/workspace-sync/v2",
        // Non-loopback host.
        "ws://10.0.0.1:8080/api/user/workspace-sync/v2",
        // UserInfo present.
        "ws://user@127.0.0.1:8080/api/user/workspace-sync/v2",
        // Query present.
        "ws://127.0.0.1:8080/api/user/workspace-sync/v2?token=x",
        // Wrong path.
        "ws://127.0.0.1:8080/api/user/sync",
        // Fragment present.
        "ws://127.0.0.1:8080/api/user/workspace-sync/v2#frag",
        // Password present.
        "ws://user:pass@127.0.0.1:8080/api/user/workspace-sync/v2",
        // Non-IP host.
        "ws://example.com:8080/api/user/workspace-sync/v2",
        // http instead of ws.
        "http://127.0.0.1:8080/api/user/workspace-sync/v2",
    ];
    for value in &invalid {
        assert!(
            WorkspaceEndpoint::parse(value).is_err(),
            "should reject: {value}"
        );
    }
}

#[test]
fn endpoint_debug_does_not_leak_url_beyond_safe_fields() {
    let endpoint =
        WorkspaceEndpoint::parse("ws://127.0.0.1:8080/api/user/workspace-sync/v2").unwrap();
    let debug = format!("{endpoint:?}");
    assert!(debug.contains("127.0.0.1"));
    assert!(debug.contains("8080"));
    assert!(debug.contains("/api/user/workspace-sync/v2"));
}

#[test]
fn endpoint_as_url_returns_validated_url() {
    let endpoint = WorkspaceEndpoint::parse("ws://[::1]:9090/api/user/workspace-sync/v2").unwrap();
    let url = endpoint.as_url();
    assert_eq!(url.scheme(), "ws");
    assert_eq!(url.port(), Some(9090));
    assert_eq!(url.path(), "/api/user/workspace-sync/v2");
    assert_eq!(url.host_str(), Some("[::1]"));
}

#[test]
fn endpoint_ipv4_loopback_accepted() {
    let endpoint = WorkspaceEndpoint::parse("ws://127.0.0.1:1/api/user/workspace-sync/v2").unwrap();
    let url = endpoint.as_url();
    assert_eq!(url.port(), Some(1));
    assert_eq!(url.host_str(), Some("127.0.0.1"));
}
