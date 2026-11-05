use std::io::Cursor;

use voime_ipc::{
    encode_message, read_message, ClientMessage, ErrorCode, Mode, Role, ServerMessage,
    TriggerSource, PROTOCOL_VERSION,
};

#[test]
fn protocol_version_is_two() {
    assert_eq!(PROTOCOL_VERSION, 2);
}

#[test]
fn round_trips_client_messages() {
    let messages = vec![
        ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: Some(voime_ipc::FrontendConfig {
                mode: Mode::Ptt,
                hotkey: "Super+F2".to_string(),
            }),
        },
        ClientMessage::Configure {
            mode: Mode::Continuous,
            hotkey: String::new(),
        },
        ClientMessage::Start {
            segment_id: 7,
            source: TriggerSource::Hold,
        },
        ClientMessage::Stop {
            segment_id: 7,
            source: TriggerSource::Hold,
        },
        ClientMessage::Toggle {
            segment_id: 0,
            source: TriggerSource::Toggle,
        },
        ClientMessage::ClearHistory {},
        ClientMessage::Status {},
        ClientMessage::Diagnostics {},
        ClientMessage::Quit {},
    ];
    for message in messages {
        let bytes = encode_message(&message).unwrap();
        let mut reader = Cursor::new(bytes);
        let decoded: ClientMessage = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, message);
    }
}

#[test]
fn error_codes_use_snake_case() {
    let text = String::from_utf8(
        encode_message(&ServerMessage::Error {
            code: ErrorCode::RoleConflict,
            segment_id: None,
            message: "已有前端连接".to_string(),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(text.contains("\"role_conflict\""));
    assert!(text.contains("\"segment_id\":null"));
}

#[test]
fn dropped_segment_error_carries_segment_id() {
    let text = String::from_utf8(
        encode_message(&ServerMessage::Error {
            code: ErrorCode::SegmentDropped,
            segment_id: Some(12),
            message: "空识别结果".to_string(),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(text.contains("\"segment_dropped\""));
    assert!(text.contains("\"segment_id\":12"));
}

#[test]
fn diagnostics_are_typed_and_do_not_change_status() {
    let text = String::from_utf8(
        encode_message(&ServerMessage::Diagnostics {
            backend: "firered".to_string(),
            worker: Some(voime_ipc::WorkerDiagnostics {
                state: voime_ipc::WorkerState::Ready,
                runtime: Some("cuda".to_string()),
                device: Some("cuda:0".to_string()),
                vram_mb: 4096,
                punc: true,
                error: None,
            }),
        })
        .unwrap(),
    )
    .unwrap();
    assert!(text.contains("\"Diagnostics\""));
    assert!(text.contains("\"state\":\"ready\""));
    assert!(text.contains("\"runtime\":\"cuda\""));
    assert!(!text.contains("\"Status\""));
}

#[test]
fn rejects_unterminated_and_oversized_lines() {
    let mut unterminated = Cursor::new(br#"{"Status":{}}"#.to_vec());
    assert!(read_message::<_, ClientMessage>(&mut unterminated).is_err());

    let oversized = vec![b'x'; voime_ipc::MAX_MESSAGE_BYTES + 1];
    assert!(encode_message(&String::from_utf8_lossy(&oversized)).is_err());
}
