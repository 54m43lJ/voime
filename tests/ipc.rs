use std::io::BufReader;
use std::os::unix::net::UnixStream;

use voime_ipc::{
    read_message, write_message, ClientMessage, FcitxConfig, Mode, Role, ServerMessage,
    TriggerSource, PROTOCOL_VERSION,
};
use voimed::ipc_server::{IpcServer, ServerEvent};

#[test]
fn framing_round_trips_all_documented_client_messages() {
    let messages = vec![
        ClientMessage::Hello {
            role: Role::Fcitx,
            version: PROTOCOL_VERSION,
            config: Some(FcitxConfig {
                mode: Mode::Toggle,
                hotkey: "Super+F2".to_string(),
            }),
        },
        ClientMessage::Configure {
            mode: Mode::Continuous,
            hotkey: "Super+F3".to_string(),
        },
        ClientMessage::Start {
            segment_id: 7,
            source: TriggerSource::Hold,
        },
        ClientMessage::Stop {
            segment_id: 7,
            source: TriggerSource::Toggle,
        },
        ClientMessage::Toggle {
            segment_id: 8,
            source: TriggerSource::Tui,
        },
        ClientMessage::ClearHistory,
        ClientMessage::Status,
        ClientMessage::Quit,
    ];
    for message in messages {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).unwrap();
        let mut reader = BufReader::new(buffer.as_slice());
        let decoded: ClientMessage = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, message);
    }
}

#[test]
fn server_enforces_single_fcitx_client_and_broadcasts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("voime").join("voimed.sock");
    let server = IpcServer::bind(&path).unwrap();

    let mut fcitx = UnixStream::connect(&path).unwrap();
    let fcitx_id = next_connected(&server);
    write_message(
        &mut fcitx,
        &ClientMessage::Hello {
            role: Role::Fcitx,
            version: PROTOCOL_VERSION,
            config: None,
        },
    )
    .unwrap();
    assert!(server.set_role(fcitx_id, Role::Fcitx));

    let tui = UnixStream::connect(&path).unwrap();
    let tui_id = next_connected(&server);
    assert_ne!(tui_id, fcitx_id);
    assert!(!server.set_role(tui_id, Role::Fcitx));
    assert!(server.set_role(tui_id, Role::Tui));

    server.broadcast(&ServerMessage::Partial {
        segment_id: 1,
        text: "临时".to_string(),
    });
    let mut reader = BufReader::new(tui);
    match read_message::<_, ServerMessage>(&mut reader)
        .unwrap()
        .unwrap()
    {
        ServerMessage::Partial { text, .. } => assert_eq!(text, "临时"),
        other => panic!("unexpected message: {other:?}"),
    }
}

fn next_connected(server: &IpcServer) -> u64 {
    loop {
        match server.next_event().unwrap() {
            ServerEvent::Connected { client_id } => return client_id,
            _ => continue,
        }
    }
}
