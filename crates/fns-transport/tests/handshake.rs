use fns_transport::WorkspaceEndpoint;

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
