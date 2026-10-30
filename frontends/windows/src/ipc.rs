use anyhow::{bail, Context};
use crossbeam_channel::{Receiver, TryRecvError};
use voime_ipc::{read_message, write_message, ClientMessage, ServerMessage};
use voime_transport::{Endpoint, Stream};

use crate::Frontend;

pub struct IpcClient {
    writer: Box<dyn Stream>,
    events: Receiver<anyhow::Result<ServerMessage>>,
}

impl IpcClient {
    pub fn connect(endpoint: &Endpoint) -> anyhow::Result<Self> {
        let stream = voime_transport::connect(endpoint)
            .with_context(|| format!("连接 daemon 失败 {endpoint}"))?;
        let reader = stream.try_clone().context("复制 daemon 流失败")?;
        let (events_tx, events_rx) = crossbeam_channel::bounded(64);
        std::thread::Builder::new()
            .name("voime-tray-ipc-read".to_string())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(reader);
                loop {
                    match read_message::<_, ServerMessage>(&mut reader) {
                        Ok(Some(message)) => {
                            if events_tx.send(Ok(message)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            let _ = events_tx.send(Err(anyhow::anyhow!("daemon 已断开")));
                            break;
                        }
                        Err(error) => {
                            let _ = events_tx.send(Err(error));
                            break;
                        }
                    }
                }
            })
            .context("创建 IPC reader 线程失败")?;
        Ok(Self {
            writer: stream,
            events: events_rx,
        })
    }

    pub fn send(&mut self, message: &ClientMessage) -> anyhow::Result<()> {
        write_message(&mut self.writer, message)
    }

    pub fn next_event(&mut self) -> anyhow::Result<ServerMessage> {
        self.events.recv().context("daemon 连接已断开")?
    }

    pub fn try_event(&mut self) -> Option<anyhow::Result<ServerMessage>> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!("daemon 连接已断开"))),
        }
    }

    pub fn snapshot(&mut self) -> anyhow::Result<ServerMessage> {
        loop {
            match self.next_event()? {
                message @ ServerMessage::Snapshot { .. } => return Ok(message),
                ServerMessage::Error { code, message } => {
                    bail!("daemon 拒绝连接（{code:?}）：{message}")
                }
                ServerMessage::Bye => bail!("daemon 已退出"),
                _ => {}
            }
        }
    }
}

impl Drop for IpcClient {
    fn drop(&mut self) {
        let _ = self.writer.shutdown();
    }
}

pub fn dispatch(message: ServerMessage, frontend: &mut Frontend) {
    frontend.on_message(message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::sync::atomic::{AtomicU64, Ordering};
    use voime_ipc::{
        read_message, write_message, FrontendConfig, FrontendState, Mode, Phase, Role,
        PROTOCOL_VERSION,
    };
    use voime_transport::bind;

    fn test_endpoint() -> Endpoint {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Endpoint::from_name(format!("voime-test-client-{}-{id}", std::process::id()))
    }

    #[test]
    fn client_sends_hello_and_reads_snapshot() {
        let endpoint = test_endpoint();
        let listener = bind(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let reader = stream.try_clone().unwrap();
            let mut reader = BufReader::new(reader);
            let message: ClientMessage = read_message(&mut reader).unwrap().unwrap();
            match message {
                ClientMessage::Hello {
                    role,
                    version,
                    config,
                } => {
                    assert_eq!(role, Role::Frontend);
                    assert_eq!(version, PROTOCOL_VERSION);
                    assert_eq!(
                        config,
                        Some(FrontendConfig {
                            mode: Mode::Ptt,
                            hotkey: "Ctrl+Alt+Space".to_string(),
                        })
                    );
                }
                other => panic!("unexpected message: {other:?}"),
            }
            write_message(
                &mut stream,
                &ServerMessage::Snapshot {
                    mode: Some(Mode::Ptt),
                    phase: Phase::Idle,
                    partial: None,
                    transcript: Vec::new(),
                    addon: FrontendState::Connected,
                    hotkey: Some("Ctrl+Alt+Space".to_string()),
                },
            )
            .unwrap();
        });

        let mut client = IpcClient::connect(&endpoint).unwrap();
        client
            .send(&ClientMessage::Hello {
                role: Role::Frontend,
                version: PROTOCOL_VERSION,
                config: Some(FrontendConfig {
                    mode: Mode::Ptt,
                    hotkey: "Ctrl+Alt+Space".to_string(),
                }),
            })
            .unwrap();
        match client.snapshot().unwrap() {
            ServerMessage::Snapshot { addon, .. } => {
                assert_eq!(addon, FrontendState::Connected);
            }
            other => panic!("unexpected snapshot: {other:?}"),
        }
        server.join().unwrap();
    }

    #[test]
    fn client_reports_disconnect_after_eof() {
        let endpoint = test_endpoint();
        let listener = bind(&endpoint).unwrap();
        let server = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            drop(stream);
        });
        let mut client = IpcClient::connect(&endpoint).unwrap();
        let _ = client.send(&ClientMessage::Hello {
            role: Role::Frontend,
            version: PROTOCOL_VERSION,
            config: None,
        });
        let event = client.next_event();
        assert!(event.is_err());
        server.join().unwrap();
    }
}
