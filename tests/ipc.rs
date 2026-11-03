use std::io::BufReader;

use voime_ipc::{
    read_message, write_message, ClientMessage, FrontendConfig, Mode, Role, ServerMessage,
    TriggerSource, PROTOCOL_VERSION,
};
use voime_transport::{connect, Endpoint};
use voimed::ipc_server::{IpcServer, ServerEvent};

#[test]
fn framing_round_trips_all_documented_client_messages() {
    let messages = vec![
        ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: Some(FrontendConfig {
                mode: Mode::Toggle,
                hotkey: "Super+F2".to_string(),
            }),
        },
        ClientMessage::Configure {
            mode: Mode::Continuous,
            hotkey: "Super+F3".to_string(),
        },
        ClientMessage::SetBackend {
            backend: voime_ipc::Backend::Gpu,
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
        ClientMessage::ClearHistory {},
        ClientMessage::Status {},
        ClientMessage::Quit {},
    ];
    for message in messages {
        let mut buffer = Vec::new();
        write_message(&mut buffer, &message).unwrap();
        let mut reader = BufReader::new(buffer.as_slice());
        let decoded: ClientMessage = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, message);
    }
}

#[cfg(windows)]
fn test_endpoint(label: &str) -> Endpoint {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    Endpoint::from_name(format!("voime-test-{label}-{}-{id}", std::process::id()))
}

#[test]
fn server_enforces_single_frontend_client_and_broadcasts() {
    #[cfg(unix)]
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let endpoint = Endpoint::from_path(directory.path().join("voime").join("voimed.sock"));
    #[cfg(windows)]
    let endpoint = test_endpoint("broadcast");
    let server = IpcServer::bind(&endpoint).unwrap();

    let mut frontend = connect(&endpoint).unwrap();
    let frontend_id = next_connected(&server);
    write_message(
        &mut frontend,
        &ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: None,
        },
    )
    .unwrap();
    assert!(server.set_role(frontend_id, Role::Frontend));

    let tui = connect(&endpoint).unwrap();
    let tui_id = next_connected(&server);
    assert_ne!(tui_id, frontend_id);
    assert!(!server.set_role(tui_id, Role::Frontend));
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
