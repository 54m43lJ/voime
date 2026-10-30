use std::os::unix::net::UnixStream;

use voime_ipc::{ClientMessage, Role, PROTOCOL_VERSION};
use voimed::ipc_server::{daemon_running, IpcServer, ServerEvent};

#[test]
fn daemon_running_detects_live_socket() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("voimed.sock");
    assert!(!daemon_running(&path));
    let _server = IpcServer::bind(&path).unwrap();
    assert!(daemon_running(&path));
}

#[test]
fn disconnect_event_carries_fcitx_role_for_daemon_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("voimed.sock");
    let server = IpcServer::bind(&path).unwrap();
    let mut client = UnixStream::connect(&path).unwrap();
    let outer_id = loop {
        match server.next_event().unwrap() {
            ServerEvent::Connected { client_id } => break client_id,
            _ => continue,
        }
    };
    voime_ipc::write_message(
        &mut client,
        &ClientMessage::Hello {
            role: Role::Fcitx,
            version: PROTOCOL_VERSION,
            config: None,
        },
    )
    .unwrap();
    assert!(server.set_role(outer_id, Role::Fcitx));
    drop(client);
    loop {
        match server.next_event().unwrap() {
            ServerEvent::Disconnected { client_id, role } => {
                assert_eq!(client_id, outer_id);
                assert_eq!(role, Some(Role::Fcitx));
                break;
            }
            _ => continue,
        }
    }
}
