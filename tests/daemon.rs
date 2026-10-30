use voime_ipc::{ClientMessage, Role, PROTOCOL_VERSION};
use voime_transport::{connect, Endpoint};
use voimed::ipc_server::{daemon_running, IpcServer, ServerEvent};

#[test]
fn daemon_running_detects_live_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::from_path(directory.path().join("voimed.sock"));
    assert!(!daemon_running(&endpoint));
    let _server = IpcServer::bind(&endpoint).unwrap();
    assert!(daemon_running(&endpoint));
}

#[test]
fn disconnect_event_carries_frontend_role_for_daemon_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::from_path(directory.path().join("voimed.sock"));
    let server = IpcServer::bind(&endpoint).unwrap();
    let mut client = connect(&endpoint).unwrap();
    let outer_id = loop {
        match server.next_event().unwrap() {
            ServerEvent::Connected { client_id } => break client_id,
            _ => continue,
        }
    };
    voime_ipc::write_message(
        &mut client,
        &ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: None,
        },
    )
    .unwrap();
    assert!(server.set_role(outer_id, Role::Frontend));
    drop(client);
    loop {
        match server.next_event().unwrap() {
            ServerEvent::Disconnected { client_id, role } => {
                assert_eq!(client_id, outer_id);
                assert_eq!(role, Some(Role::Frontend));
                break;
            }
            _ => continue,
        }
    }
}
