use std::io::BufReader;

use anyhow::{bail, Context};
use crossbeam_channel::{Receiver, TryRecvError};
use voime_ipc::{
    read_message, write_message, ClientMessage, Role, ServerMessage, PROTOCOL_VERSION,
};
use voime_transport::{Endpoint, Stream};

pub struct Client {
    writer: Box<dyn Stream>,
    events: Receiver<ServerMessage>,
    closed: bool,
}

impl Client {
    pub fn connect(endpoint: &Endpoint) -> anyhow::Result<Self> {
        Self::connect_as(endpoint, Role::Tui)
    }

    pub fn connect_as(endpoint: &Endpoint, role: Role) -> anyhow::Result<Self> {
        let stream = voime_transport::connect(endpoint)
            .with_context(|| format!("连接 daemon 失败 {endpoint}"))?;
        let reader = stream.try_clone().context("复制 daemon 流失败")?;
        let (events_tx, events_rx) = crossbeam_channel::bounded(64);
        std::thread::Builder::new()
            .name("voime-ipc-read".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                while let Ok(Some(message)) = read_message::<_, ServerMessage>(&mut reader) {
                    if events_tx.send(message).is_err() {
                        break;
                    }
                }
            })
            .context("创建 IPC reader 线程失败")?;

        let mut client = Self {
            writer: stream,
            events: events_rx,
            closed: false,
        };
        client.send(&ClientMessage::Hello {
            role,
            version: PROTOCOL_VERSION,
            config: None,
        })?;
        Ok(client)
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

    pub fn send(&mut self, message: &ClientMessage) -> anyhow::Result<()> {
        write_message(&mut self.writer, message)
    }

    pub fn next_event(&mut self) -> anyhow::Result<ServerMessage> {
        self.events.recv().context("daemon 连接已断开")
    }

    pub fn next_event_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> anyhow::Result<ServerMessage> {
        self.events
            .recv_timeout(timeout)
            .context("等待 daemon 响应超时")
    }

    pub fn try_event(&mut self) -> Option<ServerMessage> {
        match self.events.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
        }
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn wait_for_state(
        &mut self,
        timeout: std::time::Duration,
    ) -> anyhow::Result<ServerMessage> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                bail!("等待 daemon 响应超时");
            }
            match self.events.recv_timeout(remaining) {
                Ok(message @ ServerMessage::State { .. }) => return Ok(message),
                Ok(message @ ServerMessage::Error { .. }) => return Ok(message),
                Ok(ServerMessage::Bye) => bail!("daemon 已退出"),
                Ok(_) => {}
                Err(_) => bail!("等待 daemon 响应超时"),
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.writer.shutdown();
    }
}
