//! Wire protocol shared by the Fcitx addon, `voimed`, and the optional TUI.

use std::io::{BufRead, Write};

use anyhow::{bail, Context};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Incremented whenever an existing message changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

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
pub enum Role {
    #[serde(rename = "frontend", alias = "fcitx")]
    Frontend,
    #[serde(rename = "tui")]
    Tui,
    #[serde(rename = "control")]
    Control,
}

impl Role {
    /// 旧 Rust 名称的兼容别名；线协议同时接受 `"fcitx"` 与 `"frontend"`。
    #[allow(non_upper_case_globals)]
    pub const Fcitx: Self = Self::Frontend;
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

/// 旧名称兼容别名。
pub type AddonState = FrontendState;

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

/// 旧名称兼容别名。
pub type FcitxConfig = FrontendConfig;

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
    ClearHistory,
    Status,
    Quit,
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
    HistoryCleared,
    Status {
        status: Status,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    Bye,
}

/// Serializes one message plus the trailing newline used by the line framing.
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
            role: Role::Fcitx,
            version: PROTOCOL_VERSION,
            config: Some(FcitxConfig {
                mode: Mode::Continuous,
                hotkey: "Super+F2".to_string(),
            }),
        };
        let bytes = encode_message(&message).unwrap();
        let mut cursor = Cursor::new(bytes);
        let decoded: ClientMessage = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn serializes_frontend_role_and_accepts_legacy_fcitx_role() {
        let message = ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: Some(FrontendConfig {
                mode: Mode::Ptt,
                hotkey: "Super+F2".to_string(),
            }),
        };
        let bytes = encode_message(&message).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"role\":\"frontend\""));

        let legacy = br#"{"Hello":{"role":"fcitx","version":1,"config":{"mode":"toggle","hotkey":"Super+F3"}}}"#;
        let decoded: ClientMessage = serde_json::from_slice(legacy).unwrap();
        assert_eq!(
            decoded,
            ClientMessage::Hello {
                role: Role::Frontend,
                version: PROTOCOL_VERSION,
                config: Some(FrontendConfig {
                    mode: Mode::Toggle,
                    hotkey: "Super+F3".to_string(),
                }),
            }
        );
    }

    #[test]
    fn uses_documented_external_tags_and_lowercase_values() {
        let bytes = encode_message(&ClientMessage::Configure {
            mode: Mode::Ptt,
            hotkey: "Super+F2".to_string(),
        })
        .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(
            text,
            "{\"Configure\":{\"mode\":\"ptt\",\"hotkey\":\"Super+F2\"}}\n"
        );
    }

    #[test]
    fn rejects_oversized_lines() {
        let mut cursor = Cursor::new(vec![b'x'; MAX_MESSAGE_BYTES + 1]);
        let error = read_message::<_, ClientMessage>(&mut cursor).unwrap_err();
        assert!(format!("{error:#}").contains("超过"));
    }

    #[test]
    fn rejects_missing_newline() {
        let mut cursor = Cursor::new(b"{\"Status\":null}".to_vec());
        assert!(read_message::<_, ClientMessage>(&mut cursor).is_err());
    }
}
