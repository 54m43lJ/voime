use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{bail, Context};
use crossbeam_channel::{Receiver, Sender, TrySendError};
use voime_ipc::{ClientMessage, Role, ServerMessage, MAX_MESSAGE_BYTES};

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
    path: PathBuf,
    events_rx: Receiver<ServerEvent>,
    clients: Arc<Mutex<HashMap<u64, ClientEntry>>>,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl IpcServer {
    pub fn bind(path: &Path) -> anyhow::Result<Self> {
        prepare_parent(path)?;
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                bail!("daemon 已在运行：{}", path.display());
            }
            std::fs::remove_file(path)
                .with_context(|| format!("删除陈旧 socket 失败 {}", path.display()))?;
        }

        let listener = UnixListener::bind(path)
            .with_context(|| format!("绑定 socket 失败 {}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("设置 socket 权限失败 {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("设置 socket 非阻塞失败")?;

        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let clients = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let next_id = Arc::new(AtomicU64::new(1));
        let accept_thread = {
            let clients = Arc::clone(&clients);
            let shutdown = Arc::clone(&shutdown);
            let events_tx = events_tx.clone();
            std::thread::Builder::new()
                .name("voimed-ipc-accept".to_string())
                .spawn(move || {
                    while !shutdown.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let id = next_id.fetch_add(1, Ordering::Relaxed);
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
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(25));
                            }
                            Err(_) => std::thread::sleep(Duration::from_millis(100)),
                        }
                    }
                })
                .context("创建 IPC accept 线程失败")?
        };

        Ok(Self {
            path: path.to_path_buf(),
            events_rx,
            clients,
            shutdown,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn events(&self) -> &Receiver<ServerEvent> {
        &self.events_rx
    }

    pub fn next_event(&self) -> anyhow::Result<ServerEvent> {
        self.events_rx.recv().context("IPC 事件通道已关闭")
    }

    pub fn try_event(&self) -> Option<ServerEvent> {
        self.events_rx.try_recv().ok()
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

    /// Assigns a role. Only one Fcitx client is allowed.
    pub fn set_role(&self, client_id: u64, role: Role) -> bool {
        let mut clients = self.clients.lock().expect("clients mutex poisoned");
        if role == Role::Fcitx
            && clients
                .iter()
                .any(|(id, entry)| *id != client_id && entry.role == Some(Role::Fcitx))
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

    pub fn client_count(&self) -> usize {
        self.clients.lock().expect("clients mutex poisoned").len()
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        self.clients.lock().expect("clients mutex poisoned").clear();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn prepare_parent(path: &Path) -> anyhow::Result<()> {
    let Some(parent) = path.parent() else {
        bail!("socket 路径没有父目录：{}", path.display());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建 socket 目录失败 {}", parent.display()))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("设置 socket 目录权限失败 {}", parent.display()))?;
    Ok(())
}

pub fn daemon_running(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

fn spawn_client(
    stream: UnixStream,
    client_id: u64,
    clients: &Arc<Mutex<HashMap<u64, ClientEntry>>>,
    events: &Sender<ServerEvent>,
) -> anyhow::Result<()> {
    let read_stream = stream.try_clone().context("复制 IPC socket 失败")?;
    read_stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .context("设置 IPC 读超时失败")?;
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

    #[test]
    fn binds_private_socket_and_accepts_messages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voime").join("voimed.sock");
        let server = IpcServer::bind(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        let mut client = UnixStream::connect(&path).unwrap();
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
    fn rejects_second_fcitx_role() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("voimed.sock");
        let server = IpcServer::bind(&path).unwrap();
        let mut first = UnixStream::connect(&path).unwrap();
        let first_id = match server.next_event().unwrap() {
            ServerEvent::Connected { client_id } => client_id,
            other => panic!("unexpected event: {other:?}"),
        };
        let mut second = UnixStream::connect(&path).unwrap();
        let second_id = loop {
            match server.next_event().unwrap() {
                ServerEvent::Connected { client_id } if client_id != first_id => break client_id,
                _ => continue,
            }
        };
        assert!(server.set_role(first_id, Role::Fcitx));
        assert!(!server.set_role(second_id, Role::Fcitx));
        let _ = first.write_all(b"");
        let _ = second.write_all(b"");
    }
}
