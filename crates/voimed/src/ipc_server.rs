use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use voime_ipc::{ClientMessage, Role, ServerMessage, MAX_MESSAGE_BYTES};
use voime_log::vlog;
use voime_transport::{Endpoint, Listener, Stream};

#[derive(Debug)]
pub enum ServerEvent {
    Connected {
        client_id: u64,
    },
    Message {
        client_id: u64,
        message: ClientMessage,
    },
    BadMessage {
        client_id: u64,
        message: String,
    },
    Disconnected {
        client_id: u64,
        role: Option<Role>,
    },
}

struct ClientEntry {
    sender: Sender<ServerMessage>,
    role: Option<Role>,
}

pub struct IpcServer {
    events_rx: Receiver<ServerEvent>,
    clients: Arc<Mutex<HashMap<u64, ClientEntry>>>,
    shutdown: Arc<AtomicBool>,
    listener: Option<Arc<dyn Listener>>,
    accept_thread: Option<JoinHandle<()>>,
}

impl IpcServer {
    pub fn bind(endpoint: &Endpoint) -> Result<Self> {
        let listener: Arc<dyn Listener> = Arc::from(
            voime_transport::bind(endpoint).with_context(|| format!("绑定端点失败 {endpoint}"))?,
        );
        vlog!("IPC 监听 {endpoint}");
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let clients = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let next_id = Arc::new(AtomicU64::new(1));
        let accept_thread = {
            let clients = Arc::clone(&clients);
            let shutdown = Arc::clone(&shutdown);
            let events_tx = events_tx.clone();
            let listener = Arc::clone(&listener);
            std::thread::Builder::new()
                .name("voimed-ipc-accept".to_string())
                .spawn(move || {
                    while !shutdown.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok(stream) => {
                                let id = next_id.fetch_add(1, Ordering::Relaxed);
                                vlog!("客户端已接受 id={id}");
                                if let Err(error) = spawn_client(stream, id, &clients, &events_tx) {
                                    let _ = events_tx.send(ServerEvent::BadMessage {
                                        client_id: id,
                                        message: format!("{error:#}"),
                                    });
                                } else {
                                    let _ =
                                        events_tx.send(ServerEvent::Connected { client_id: id });
                                }
                            }
                            Err(error) => {
                                if shutdown.load(Ordering::Relaxed)
                                    || error.kind() == std::io::ErrorKind::Interrupted
                                {
                                    break;
                                }
                                vlog!("accept 失败：{error}");
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                    }
                })
                .context("创建 IPC accept 线程失败")?
        };

        Ok(Self {
            events_rx,
            clients,
            shutdown,
            listener: Some(listener),
            accept_thread: Some(accept_thread),
        })
    }

    pub fn events(&self) -> &Receiver<ServerEvent> {
        &self.events_rx
    }

    pub fn next_event(&self) -> Result<ServerEvent> {
        self.events_rx.recv().context("IPC 事件通道已关闭")
    }

    pub fn broadcast(&self, message: &ServerMessage) {
        let mut clients = self.clients.lock().expect("clients mutex poisoned");
        let mut remove = Vec::new();
        for (id, entry) in clients.iter() {
            match entry.sender.try_send(message.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    if !matches!(message, ServerMessage::Partial { .. }) {
                        remove.push(*id);
                    }
                }
                Err(TrySendError::Disconnected(_)) => remove.push(*id),
            }
        }
        for id in remove {
            clients.remove(&id);
        }
    }

    /// Drops a client from the broadcast set. The writer thread flushes any
    /// already queued message (for example an Error) and then closes the
    /// socket.
    pub fn disconnect(&self, client_id: u64) {
        self.clients
            .lock()
            .expect("clients mutex poisoned")
            .remove(&client_id);
    }

    pub fn send_to(&self, client_id: u64, message: &ServerMessage) {
        let mut clients = self.clients.lock().expect("clients mutex poisoned");
        let Some(entry) = clients.get(&client_id) else {
            return;
        };
        match entry.sender.try_send(message.clone()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                if !matches!(message, ServerMessage::Partial { .. }) {
                    clients.remove(&client_id);
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                clients.remove(&client_id);
            }
        }
    }

    /// Assigns a role. Only one frontend client is allowed.
    pub fn set_role(&self, client_id: u64, role: Role) -> bool {
        let mut clients = self.clients.lock().expect("clients mutex poisoned");
        if role == Role::Frontend
            && clients
                .iter()
                .any(|(id, entry)| *id != client_id && entry.role == Some(Role::Frontend))
        {
            return false;
        }
        if let Some(entry) = clients.get_mut(&client_id) {
            entry.role = Some(role);
            true
        } else {
            false
        }
    }

    pub fn role(&self, client_id: u64) -> Option<Role> {
        self.clients
            .lock()
            .expect("clients mutex poisoned")
            .get(&client_id)
            .and_then(|entry| entry.role)
    }

    pub fn shutdown(&mut self) {
        vlog!("IPC 服务关闭");
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.as_ref() {
            let _ = listener.shutdown();
        }
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        self.clients.lock().expect("clients mutex poisoned").clear();
        self.listener.take();
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn daemon_running(endpoint: &Endpoint) -> bool {
    voime_transport::connect(endpoint).is_ok()
}

fn spawn_client(
    stream: Box<dyn Stream>,
    client_id: u64,
    clients: &Arc<Mutex<HashMap<u64, ClientEntry>>>,
    events: &Sender<ServerEvent>,
) -> Result<()> {
    let read_stream = stream.try_clone().context("复制 IPC 流失败")?;
    let write_stream = stream;
    let (message_tx, message_rx) = crossbeam_channel::bounded::<ServerMessage>(64);
    clients.lock().expect("clients mutex poisoned").insert(
        client_id,
        ClientEntry {
            sender: message_tx,
            role: None,
        },
    );

    let writer = std::thread::Builder::new()
        .name(format!("voimed-ipc-write-{client_id}"))
        .spawn(move || {
            let mut writer = std::io::BufWriter::new(write_stream);
            while let Ok(message) = message_rx.recv() {
                let bytes = match voime_ipc::encode_message(&message) {
                    Ok(bytes) => bytes,
                    Err(_) => break,
                };
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        })
        .context("创建 IPC writer 线程失败")?;
    let _ = writer;

    let events = events.clone();
    let clients = Arc::clone(clients);
    std::thread::Builder::new()
        .name(format!("voimed-ipc-read-{client_id}"))
        .spawn(move || {
            let mut reader = BufReader::new(read_stream);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.len() > MAX_MESSAGE_BYTES {
                            let _ = events.send(ServerEvent::BadMessage {
                                client_id,
                                message: format!("IPC 消息超过 {MAX_MESSAGE_BYTES} 字节"),
                            });
                            break;
                        }
                        if line.last() != Some(&b'\n') {
                            let _ = events.send(ServerEvent::BadMessage {
                                client_id,
                                message: "IPC 消息缺少换行终止符".to_string(),
                            });
                            break;
                        }
                        line.pop();
                        match serde_json::from_slice::<ClientMessage>(&line) {
                            Ok(message) => {
                                if events
                                    .send(ServerEvent::Message { client_id, message })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = events.send(ServerEvent::BadMessage {
                                    client_id,
                                    message: format!("IPC 消息不是合法 JSON：{error}"),
                                });
                                break;
                            }
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => break,
                }
            }
            let role = clients
                .lock()
                .expect("clients mutex poisoned")
                .remove(&client_id)
                .and_then(|entry| entry.role);
            let _ = events.send(ServerEvent::Disconnected { client_id, role });
        })
        .context("创建 IPC reader 线程失败")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use voime_ipc::{write_message, ClientMessage, PROTOCOL_VERSION};
    use voime_transport::connect;

    #[cfg(windows)]
    fn test_endpoint(label: &str) -> Endpoint {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Endpoint::from_name(format!("voime-test-{label}-{}-{id}", std::process::id()))
    }

    #[test]
    fn binds_private_socket_and_accepts_messages() {
        #[cfg(unix)]
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let endpoint = Endpoint::from_path(directory.path().join("voime").join("voimed.sock"));
        #[cfg(windows)]
        let endpoint = test_endpoint("accept");
        let server = IpcServer::bind(&endpoint).unwrap();

        let mut client = connect(&endpoint).unwrap();
        let event = server.next_event().unwrap();
        let client_id = match event {
            ServerEvent::Connected { client_id } => client_id,
            other => panic!("unexpected event: {other:?}"),
        };
        write_message(
            &mut client,
            &ClientMessage::Hello {
                role: Role::Tui,
                version: PROTOCOL_VERSION,
                config: None,
            },
        )
        .unwrap();
        let message = loop {
            match server.next_event().unwrap() {
                ServerEvent::Message { message, .. } => break message,
                _ => continue,
            }
        };
        assert!(matches!(
            message,
            ClientMessage::Hello {
                role: Role::Tui,
                ..
            }
        ));
        assert!(server.set_role(client_id, Role::Tui));
    }

    #[test]
    fn rejects_second_frontend_role() {
        #[cfg(unix)]
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let endpoint = Endpoint::from_path(directory.path().join("voimed.sock"));
        #[cfg(windows)]
        let endpoint = test_endpoint("role");
        let server = IpcServer::bind(&endpoint).unwrap();
        let mut first = connect(&endpoint).unwrap();
        let first_id = match server.next_event().unwrap() {
            ServerEvent::Connected { client_id } => client_id,
            other => panic!("unexpected event: {other:?}"),
        };
        let mut second = connect(&endpoint).unwrap();
        let second_id = loop {
            match server.next_event().unwrap() {
                ServerEvent::Connected { client_id } if client_id != first_id => break client_id,
                _ => continue,
            }
        };
        assert!(server.set_role(first_id, Role::Frontend));
        assert!(!server.set_role(second_id, Role::Frontend));
        let _ = first.write_all(b"");
        let _ = second.write_all(b"");
    }
}
