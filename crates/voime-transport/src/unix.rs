use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::{Endpoint, Listener, Stream};

pub struct UnixListener {
    listener: StdUnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    cancelled: AtomicBool,
}

pub struct UnixStream {
    stream: StdUnixStream,
}

impl Listener for UnixListener {
    fn accept(&self) -> io::Result<Box<dyn Stream>> {
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error("listener"));
            }
            match self.listener.accept() {
                Ok((stream, _)) => return Ok(Box::new(UnixStream { stream })),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("接受连接失败 {}：{error}", self.path.display()),
                    ));
                }
            }
        }
    }

    fn shutdown(&self) -> io::Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
        if !self.owns_endpoint() {
            return Ok(());
        }
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io::Error::new(
                error.kind(),
                format!("删除 Unix socket 失败 {}：{error}", self.path.display()),
            )),
        }
    }
}

impl Drop for UnixListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl UnixListener {
    fn owns_endpoint(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata.dev() == self.device && metadata.ino() == self.inode,
            Err(_) => false,
        }
    }
}

impl Stream for UnixStream {
    fn try_clone(&self) -> io::Result<Box<dyn Stream>> {
        self.stream
            .try_clone()
            .map(|stream| Box::new(UnixStream { stream }) as Box<dyn Stream>)
    }

    fn shutdown(&mut self) -> io::Result<()> {
        match self.stream.shutdown(std::net::Shutdown::Both) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotConnected => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Read for UnixStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buffer)
    }
}

impl Write for UnixStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

pub fn bind(path: &Path) -> io::Result<UnixListener> {
    prepare_parent(path)?;
    if path.exists() {
        if stale_endpoint(path) {
            fs::remove_file(path).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("删除陈旧 Unix socket 失败 {}：{error}", path.display()),
                )
            })?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("daemon 已在运行：{}", path.display()),
            ));
        }
    }

    let listener = StdUnixListener::bind(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("绑定 Unix socket 失败 {}：{error}", path.display()),
        )
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("设置 Unix socket 权限失败 {}：{error}", path.display()),
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|error| io::Error::new(error.kind(), format!("设置非阻塞失败：{error}")))?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("读取 Unix socket 元数据失败 {}：{error}", path.display()),
        )
    })?;

    Ok(UnixListener {
        listener,
        path: path.to_path_buf(),
        device: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
        inode: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
        cancelled: AtomicBool::new(false),
    })
}

pub fn connect(path: &Path) -> io::Result<UnixStream> {
    StdUnixStream::connect(path)
        .map(|stream| UnixStream { stream })
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("连接 Unix socket 失败 {}：{error}", path.display()),
            )
        })
}

pub fn default_endpoint() -> Endpoint {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !runtime.is_empty() {
            return Endpoint::Unix(PathBuf::from(runtime).join("voime").join("voimed.sock"));
        }
    }
    Endpoint::Unix(
        PathBuf::from("/tmp")
            .join(format!("voime-{}", current_uid()))
            .join("voimed.sock"),
    )
}

fn prepare_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket 路径没有父目录：{}", path.display()),
        ));
    };
    if parent.exists() {
        let dedicated = parent
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == "voime" || name.starts_with("voime-"))
            .unwrap_or(false);
        if dedicated {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("设置 socket 目录权限失败 {}：{error}", parent.display()),
                )
            })?;
        }
        return Ok(());
    }
    fs::create_dir_all(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("创建 socket 目录失败 {}：{error}", parent.display()),
        )
    })?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("设置 socket 目录权限失败 {}：{error}", parent.display()),
        )
    })
}

fn stale_endpoint(path: &Path) -> bool {
    StdUnixStream::connect(path).is_err()
}

fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .unwrap_or(0)
}

fn cancelled_error(kind: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        format!("{kind} 已关闭（shutdown）"),
    )
}
