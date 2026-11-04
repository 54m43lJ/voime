use std::collections::HashMap;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::Context;
use crossbeam_channel::{Receiver, Sender};
use voime_ipc::{read_message, write_message, ClientMessage, Role, ServerMessage};
use voime_transport::{Endpoint, Listener, Stream};

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
    control: Arc<Mutex<Box<dyn Stream>>>,
}

pub struct IpcServer {
    events_rx: Receiver<ServerEvent>,
    clients: Arc<Mutex<HashMap<u64, ClientEntry>>>,
    shutdown: Arc<AtomicBool>,
    listener: Option<Arc<dyn Listener>>,
    accept_thread: Option<JoinHandle<()>>,
}

impl IpcServer {
    pub fn bind(endpoint: &Endpoint) -> anyhow::Result<Self> {
        let listener =
            voime_transport::bind(endpoint).with_context(|| format!("绑定端点失败 {endpoint}"))?;
        let listener: Arc<dyn Listener> = Arc::from(listener);
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let clients = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let accept_listener = Arc::clone(&listener);
        let accept_clients = Arc::clone(&clients);
        let accept_shutdown = Arc::clone(&shutdown);
        let accept_thread = std::thread::Builder::new()
            .name("voimed-accept".to_string())
            .spawn(move || {
                let next_id = AtomicU64::new(1);
                loop {
                    match accept_listener.accept() {
                        Ok(stream) => {
                            if accept_shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            let client_id = next_id.fetch_add(1, Ordering::Relaxed);
                            if let Err(error) =
                                spawn_client(stream, client_id, &accept_clients, &events_tx)
                            {
                                voime_log::vlog!("无法启动客户端 {}：{error:#}", client_id);
                            } else {
                                let _ = events_tx.send(ServerEvent::Connected { client_id });
                            }
                        }
                        Err(_) if accept_shutdown.load(Ordering::Relaxed) => break,
                        Err(error) => {
                            voime_log::vlog!("accept 失败：{error}");
                            break;
                        }
                    }
                }
            })
            .context("创建 IPC accept 线程失败")?;
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

    pub fn next_event(&self) -> anyhow::Result<ServerEvent> {
        self.events_rx.recv().context("IPC 事件通道已关闭")
    }

    pub fn broadcast(&self, message: &ServerMessage) {
        let ids: Vec<u64> = {
            let clients = self.clients.lock().expect("client map poisoned");
            clients.keys().copied().collect()
        };
        let mut slow = Vec::new();
        for id in ids {
            let clients = self.clients.lock().expect("client map poisoned");
            let Some(entry) = clients.get(&id) else {
                continue;
            };
            if entry.sender.try_send(message.clone()).is_err()
                && !matches!(message, ServerMessage::Partial { .. })
            {
                slow.push(id);
            }
        }
        for id in slow {
            self.disconnect(id);
        }
    }

    pub fn disconnect(&self, client_id: u64) {
        let entry = self
            .clients
            .lock()
            .expect("client map poisoned")
            .remove(&client_id);
        if let Some(entry) = entry {
            let _ = entry.control.lock().map(|mut stream| stream.shutdown());
        }
    }

    pub fn send_to(&self, client_id: u64, message: &ServerMessage) {
        let clients = self.clients.lock().expect("client map poisoned");
        if let Some(entry) = clients.get(&client_id) {
            if entry.sender.try_send(message.clone()).is_err()
                && !matches!(message, ServerMessage::Partial { .. })
            {
                drop(clients);
                self.disconnect(client_id);
            }
        }
    }

    pub fn set_role(&self, client_id: u64, role: Role) -> bool {
        let mut clients = self.clients.lock().expect("client map poisoned");
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
            .expect("client map poisoned")
            .get(&client_id)
            .and_then(|entry| entry.role)
    }

    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(listener) = self.listener.take() {
            let _ = listener.shutdown();
        }
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        let ids: Vec<u64> = {
            let clients = self.clients.lock().expect("client map poisoned");
            clients.keys().copied().collect()
        };
        for id in ids {
            self.disconnect(id);
        }
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
) -> anyhow::Result<()> {
    let writer = stream.try_clone().context("复制客户端写句柄失败")?;
    let control = stream.try_clone().context("复制客户端控制句柄失败")?;
    let (sender, receiver) = crossbeam_channel::bounded(64);
    clients.lock().expect("client map poisoned").insert(
        client_id,
        ClientEntry {
            sender,
            role: None,
            control: Arc::new(Mutex::new(control)),
        },
    );

    std::thread::Builder::new()
        .name(format!("voimed-client-{client_id}-write"))
        .spawn(move || {
            let mut writer = writer;
            while let Ok(message) = receiver.recv() {
                if write_message(&mut writer, &message).is_err() {
                    break;
                }
            }
        })
        .context("创建客户端 writer 线程失败")?;

    let reader_clients = Arc::clone(clients);
    let reader_events = events.clone();
    std::thread::Builder::new()
        .name(format!("voimed-client-{client_id}-read"))
        .spawn(move || {
            let mut reader = BufReader::new(stream);
            loop {
                match read_message::<_, ClientMessage>(&mut reader) {
                    Ok(Some(message)) => {
                        if reader_events
                            .send(ServerEvent::Message { client_id, message })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = reader_events.send(ServerEvent::BadMessage {
                            client_id,
                            message: format!("{error:#}"),
                        });
                        break;
                    }
                }
            }
            let role = reader_clients
                .lock()
                .expect("client map poisoned")
                .get(&client_id)
                .and_then(|entry| entry.role);
            let _ = reader_events.send(ServerEvent::Disconnected { client_id, role });
        })
        .context("创建客户端 reader 线程失败")?;
    Ok(())
}
