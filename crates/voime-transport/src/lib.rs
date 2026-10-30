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
}
