use voime_transport::Endpoint;
use voimed::ipc_server::{daemon_running, IpcServer};

fn test_endpoint(label: &str) -> (tempfile::TempDir, Endpoint) {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let endpoint = Endpoint::from_path(directory.path().join("voime").join("voimed.sock"));
    #[cfg(windows)]
    let endpoint = {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Endpoint::from_name(format!(
            "voime-test-daemon-{label}-{}-{id}",
            std::process::id()
        ))
    };
    (directory, endpoint)
}

#[test]
fn bind_makes_endpoint_live_and_shutdown_removes_it() {
    let (_directory, endpoint) = test_endpoint("live");
    assert!(!daemon_running(&endpoint));
    let mut server = IpcServer::bind(&endpoint).unwrap();
    assert!(daemon_running(&endpoint));
    server.shutdown();
    assert!(!daemon_running(&endpoint));
}

#[test]
fn frontend_role_is_released_after_disconnect() {
    use std::time::Duration;
    use voime_ipc::Role;
    use voime_transport::connect;
    use voimed::ipc_server::ServerEvent;

    let (_directory, endpoint) = test_endpoint("roles");
    let mut server = IpcServer::bind(&endpoint).unwrap();
    let first = connect(&endpoint).unwrap();
    let first_id = match server
        .events()
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
    {
        ServerEvent::Connected { client_id } => client_id,
        _ => panic!("expected connected"),
    };
    assert!(server.set_role(first_id, Role::Frontend));
    assert_eq!(server.role(first_id), Some(Role::Frontend));

    let second = connect(&endpoint).unwrap();
    let second_id = match server
        .events()
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
    {
        ServerEvent::Connected { client_id } => client_id,
        _ => panic!("expected connected"),
    };
    assert!(!server.set_role(second_id, Role::Frontend));

    server.disconnect(first_id);
    assert!(server.set_role(second_id, Role::Frontend));
    drop((first, second));
    server.shutdown();
}
