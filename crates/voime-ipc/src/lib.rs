//! Wire protocol shared by the Fcitx addon, the Windows frontend, `voimed`,
//! and `voime`.
//!
//! This crate is the single source of truth for message shapes, framing and
//! protocol version. See `voime-ipc` USER §4 for the contract.

use std::io::{BufRead, Write};

use anyhow::{bail, Context};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Incremented whenever an existing message changes shape.
pub const PROTOCOL_VERSION: u32 = 4;

/// One JSON line may not exceed this many bytes.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ptt,
    Toggle,
    Continuous,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ptt => "ptt",
            Self::Toggle => "toggle",
            Self::Continuous => "continuous",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Frontend,
    Tui,
    Control,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerSource {
    Hold,
    Toggle,
    Tui,
    Cli,
    Continuous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Recording,
    Finalizing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontendState {
    Connected,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidMessage,
    VersionMismatch,
    RoleConflict,
    Busy,
    NoInputContext,
    DaemonUnavailable,
    Unavailable,
    Inference,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrontendConfig {
    pub mode: Mode,
    pub hotkey: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub recording: bool,
    pub finalizing: bool,
    pub segment_id: Option<u64>,
    pub mode: Option<Mode>,
    pub hotkey: Option<String>,
    pub addon: FrontendState,
    pub transcript_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        role: Role,
        version: u32,
        #[serde(default)]
        config: Option<FrontendConfig>,
    },
    Configure {
        mode: Mode,
        hotkey: String,
    },
    Start {
        segment_id: u64,
        source: TriggerSource,
    },
    Stop {
        segment_id: u64,
        source: TriggerSource,
    },
    Toggle {
        segment_id: u64,
        source: TriggerSource,
    },
    ClearHistory {},
    Status {},
    Quit {},
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    Snapshot {
        mode: Option<Mode>,
        phase: Phase,
        partial: Option<String>,
        transcript: Vec<String>,
        addon: FrontendState,
        hotkey: Option<String>,
    },
    SegmentStarted {
        segment_id: u64,
        source: TriggerSource,
    },
    State {
        recording: bool,
        finalizing: bool,
        segment_id: Option<u64>,
    },
    Partial {
        segment_id: u64,
        text: String,
    },
    Final {
        segment_id: u64,
        text: String,
    },
    HistoryCleared {},
    Status {
        status: Status,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    Bye {},
}

/// Serializes one message plus the trailing newline used by line framing.
pub fn encode_message<T: Serialize>(message: &T) -> anyhow::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message).context("序列化 IPC 消息失败")?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        bail!("IPC 消息超过 {} 字节", MAX_MESSAGE_BYTES);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> anyhow::Result<()> {
    let bytes = encode_message(message)?;
    writer.write_all(&bytes).context("写入 IPC 消息失败")?;
    writer.flush().context("刷新 IPC 消息失败")?;
    Ok(())
}

/// Reads one newline-delimited JSON message. `Ok(None)` means clean EOF.
pub fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> anyhow::Result<Option<T>> {
    let mut line = Vec::new();
    let read = reader
        .read_until(b'\n', &mut line)
        .context("读取 IPC 消息失败")?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_MESSAGE_BYTES {
        bail!("IPC 消息超过 {} 字节", MAX_MESSAGE_BYTES);
    }
    if line.last() != Some(&b'\n') {
        bail!("IPC 消息缺少换行终止符");
    }
    line.pop();
    let message = serde_json::from_slice(&line).context("IPC 消息不是合法 JSON")?;
    Ok(Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_hello_with_config() {
        let message = ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: Some(FrontendConfig {
                mode: Mode::Continuous,
                hotkey: "Super+F2".to_string(),
            }),
        };
        let bytes = encode_message(&message).unwrap();
        let mut reader = Cursor::new(bytes);
        let decoded: ClientMessage = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn uses_external_tags_and_lowercase_values() {
        let bytes = encode_message(&ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: None,
        })
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with(r#"{"Hello":"#));
        assert!(text.contains(r#""role":"frontend""#));
        assert!(text.contains(r#""version":4"#));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn rejects_oversized_messages() {
        let payload = "x".repeat(MAX_MESSAGE_BYTES);
        let error = encode_message(&payload).unwrap_err();
        assert!(error.to_string().contains("超过"));
    }

    #[test]
    fn read_requires_newline_and_honours_eof() {
        let mut empty = Cursor::new(Vec::<u8>::new());
        let none: Option<ClientMessage> = read_message(&mut empty).unwrap();
        assert!(none.is_none());

        let mut missing = Cursor::new(br#"{"Status":{}}"#.to_vec());
        assert!(read_message::<_, ClientMessage>(&mut missing).is_err());
    }

    #[test]
    fn unit_messages_are_json_objects_with_an_external_tag() {
        let text =
            String::from_utf8(encode_message(&ClientMessage::ClearHistory {}).unwrap()).unwrap();
        assert_eq!(text, "{\"ClearHistory\":{}}\n");
        let text = String::from_utf8(encode_message(&ServerMessage::Bye {}).unwrap()).unwrap();
        assert_eq!(text, "{\"Bye\":{}}\n");
    }

    #[test]
    fn protocol_version_is_four() {
        assert_eq!(PROTOCOL_VERSION, 4);
    }
}
