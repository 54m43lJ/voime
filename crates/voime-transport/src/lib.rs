//! 本机字节流传输：统一 `Endpoint` / `Listener` / `Stream`，隔离 Unix socket
//! 与 Windows named pipe 的差异。协议与业务语义不属于本 crate。

use std::io::{self, Read, Write};

#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(unix)]
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Endpoint {
    Unix(PathBuf),
}

#[cfg(windows)]
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Endpoint {
    NamedPipe(String),
}

impl Endpoint {
    #[cfg(unix)]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::Unix(path.into())
    }

    #[cfg(windows)]
    pub fn from_name(name: impl Into<String>) -> Self {
        Self::NamedPipe(windows::normalize_name(&name.into()))
    }

    #[cfg(unix)]
    pub fn parse(value: &str) -> io::Result<Self> {
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "端点路径不能为空",
            ));
        }
        Ok(Self::Unix(PathBuf::from(value)))
    }

    #[cfg(windows)]
    pub fn parse(value: &str) -> io::Result<Self> {
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "端点名称不能为空",
            ));
        }
        Ok(Self::NamedPipe(windows::normalize_name(value)))
    }

    #[cfg(unix)]
    pub fn as_unix_path(&self) -> Option<&Path> {
        match self {
            Self::Unix(path) => Some(path),
        }
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => write!(formatter, "{}", path.display()),
            #[cfg(windows)]
            Self::NamedPipe(name) => write!(formatter, "{name}"),
        }
    }
}

pub trait Stream: Read + Write + Send {
    fn try_clone(&self) -> io::Result<Box<dyn Stream>>;
    fn shutdown(&mut self) -> io::Result<()>;
}

pub trait Listener: Send + Sync {
    fn accept(&self) -> io::Result<Box<dyn Stream>>;
    fn shutdown(&self) -> io::Result<()>;
}

pub fn bind(endpoint: &Endpoint) -> io::Result<Box<dyn Listener>> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => Ok(Box::new(unix::bind(path)?)),
        #[cfg(windows)]
        Endpoint::NamedPipe(name) => Ok(Box::new(windows::bind(name)?)),
    }
}

pub fn connect(endpoint: &Endpoint) -> io::Result<Box<dyn Stream>> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => Ok(Box::new(unix::connect(path)?)),
        #[cfg(windows)]
        Endpoint::NamedPipe(name) => Ok(Box::new(windows::connect(name)?)),
    }
}

pub fn default_endpoint() -> Endpoint {
    #[cfg(unix)]
    {
        unix::default_endpoint()
    }
    #[cfg(windows)]
    {
        windows::default_endpoint()
    }
}

#[cfg(test)]
mod memory;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[cfg(unix)]
    #[test]
    fn unix_bind_connect_read_write_and_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::from_path(directory.path().join("voime").join("test.sock"));
        let listener = bind(&endpoint).expect("bind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = endpoint.as_unix_path().unwrap();
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let parent = path.parent().unwrap();
            let parent_mode = std::fs::metadata(parent).unwrap().permissions().mode() & 0o777;
            assert_eq!(parent_mode, 0o700);
        }
        let client = connect(&endpoint).expect("connect");
        let server = listener.accept().expect("accept");

        let mut writer = client;
        writer.write_all(b"hello\n").unwrap();
        let mut reader = BufReader::new(server);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "hello\n");

        listener.shutdown().unwrap();
        assert!(listener.accept().is_err());
        assert!(!endpoint.as_unix_path().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn unix_rejects_a_live_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::from_path(directory.path().join("live.sock"));
        let listener = bind(&endpoint).unwrap();
        let error = match bind(&endpoint) {
            Ok(_) => panic!("second bind should fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(listener);
    }

    #[test]
    fn memory_transport_satisfies_stream_contract() {
        let listener = memory::MemoryListener::new();
        let (client, server) = memory::MemoryStream::pair();
        listener.push(server);
        let mut server = listener.accept().expect("accept");
        let mut client = client;
        client.write_all(b"ping\n").unwrap();
        let mut line = String::new();
        BufReader::new(&mut server).read_line(&mut line).unwrap();
        assert_eq!(line, "ping\n");
        let mut clone = server.try_clone().unwrap();
        clone.shutdown().unwrap();
        assert_eq!(clone.read(&mut [0u8; 1]).unwrap(), 0);
        listener.shutdown().unwrap();
        assert!(listener.accept().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn default_endpoint_is_documented_socket() {
        let endpoint = default_endpoint();
        let path = endpoint.as_unix_path().unwrap();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("voimed.sock")
        );
    }

    #[cfg(windows)]
    fn test_pipe_name(label: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        format!("voime-test-{}-{}-{id}", label, std::process::id())
    }

    #[cfg(windows)]
    #[test]
    fn windows_bind_connect_read_write_and_shutdown() {
        let name = test_pipe_name("io");
        let endpoint = Endpoint::from_name(&name);
        let listener = bind(&endpoint).expect("bind");
        let client = connect(&endpoint).expect("connect");
        let mut server = listener.accept().expect("accept");

        let mut client = client;
        client.write_all(b"hello\n").unwrap();
        let mut line = String::new();
        BufReader::new(&mut server).read_line(&mut line).unwrap();
        assert_eq!(line, "hello\n");

        let mut clone = server.try_clone().unwrap();
        clone.shutdown().unwrap();
        assert_eq!(clone.read(&mut [0u8; 1]).unwrap(), 0);

        listener.shutdown().unwrap();
        assert!(listener.accept().is_err());
        assert!(connect(&endpoint).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_full_duplex_while_both_sides_read() {
        use std::sync::mpsc;
        use std::time::Duration;

        let name = test_pipe_name("duplex");
        let endpoint = Endpoint::from_name(&name);
        let listener = bind(&endpoint).unwrap();
        let client = connect(&endpoint).unwrap();
        let mut server = listener.accept().unwrap();
        let mut client = client;

        let server_reader = server.try_clone().unwrap();
        let client_reader = client.try_clone().unwrap();
        let (server_tx, server_rx) = mpsc::channel();
        let (client_tx, client_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(server_reader).read_line(&mut line).unwrap();
            let _ = server_tx.send(line);
        });
        std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(client_reader).read_line(&mut line).unwrap();
            let _ = client_tx.send(line);
        });

        std::thread::sleep(Duration::from_millis(50));
        let (write_tx, write_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = write_tx.send(server.write_all(b"pong\n"));
        });
        assert!(write_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_ok());
        client.write_all(b"ping\n").unwrap();
        assert_eq!(
            client_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "pong\n"
        );
        assert_eq!(
            server_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "ping\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_shutdown_wakes_blocked_accept() {
        use std::sync::Arc;
        use std::time::Duration;

        let name = test_pipe_name("wake");
        let endpoint = Endpoint::from_name(&name);
        let listener: Arc<dyn Listener> = Arc::from(bind(&endpoint).unwrap());
        let accepting = Arc::clone(&listener);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let error = accepting.accept().err();
            let _ = tx.send(error.is_some());
        });
        std::thread::sleep(Duration::from_millis(100));
        listener.shutdown().unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_a_live_endpoint() {
        let name = test_pipe_name("live");
        let endpoint = Endpoint::from_name(&name);
        let listener = bind(&endpoint).unwrap();
        let error = match bind(&endpoint) {
            Ok(_) => panic!("second bind should fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(listener);
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_endpoint_uses_pipe_namespace() {
        let endpoint = default_endpoint();
        let text = endpoint.to_string();
        assert!(text.starts_with(r"\\.\pipe\voime-"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_endpoint_accepts_short_and_full_names() {
        assert_eq!(
            Endpoint::from_name("voime-test").to_string(),
            r"\\.\pipe\voime-test"
        );
        assert_eq!(
            Endpoint::parse(r"\\.\pipe\voime-test").unwrap().to_string(),
            r"\\.\pipe\voime-test"
        );
    }
}
