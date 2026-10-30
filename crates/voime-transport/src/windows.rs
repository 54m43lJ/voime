use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, DUPLICATE_SAME_ACCESS, ERROR_ACCESS_DENIED,
    ERROR_BROKEN_PIPE, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    DuplicateHandle, GetCurrentProcess, GetCurrentProcessId,
};
use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
use windows_sys::Win32::System::IO::CancelIoEx;

use crate::{Endpoint, Listener, Stream};

const INVALID: isize = 0;

struct ListenerState {
    handle: isize,
    connecting: isize,
    cancelled: bool,
}

pub struct NamedPipeListener {
    name: String,
    state: Mutex<ListenerState>,
}

struct Shared {
    handles: Mutex<Vec<isize>>,
}

pub struct NamedPipeStream {
    handle: isize,
    shared: Arc<Shared>,
}

impl Listener for NamedPipeListener {
    fn accept(&self) -> io::Result<Box<dyn Stream>> {
        let handle = {
            let mut state = self.state.lock().expect("pipe listener poisoned");
            if state.cancelled || state.handle == INVALID {
                return Err(interrupted_error(&self.name));
            }
            let handle = state.handle;
            state.handle = INVALID;
            state.connecting = handle;
            handle
        };

        let connected = unsafe { ConnectNamedPipe(handle as HANDLE, null_mut()) };
        let error = if connected == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };

        let mut state = self.state.lock().expect("pipe listener poisoned");
        state.connecting = INVALID;
        if (connected == 0 && error != ERROR_PIPE_CONNECTED) || state.cancelled {
            drop(state);
            unsafe {
                CloseHandle(handle as HANDLE);
            }
            if error == ERROR_OPERATION_ABORTED {
                return Err(interrupted_error(&self.name));
            }
            return Err(pipe_error("ConnectNamedPipe", &self.name, error));
        }

        match create_pipe(&self.name, false) {
            Ok(next) => state.handle = next as isize,
            Err(error) => {
                drop(state);
                unsafe {
                    CloseHandle(handle as HANDLE);
                }
                return Err(error);
            }
        }
        drop(state);

        Ok(Box::new(NamedPipeStream::new(handle)))
    }

    fn shutdown(&self) -> io::Result<()> {
        let (current, connecting) = {
            let mut state = self.state.lock().expect("pipe listener poisoned");
            state.cancelled = true;
            let current = state.handle;
            state.handle = INVALID;
            let connecting = state.connecting;
            state.connecting = INVALID;
            (current, connecting)
        };
        unsafe {
            if connecting != INVALID {
                CancelIoEx(connecting as HANDLE, null());
            }
            if current != INVALID {
                CancelIoEx(current as HANDLE, null());
                CloseHandle(current as HANDLE);
            }
        }
        Ok(())
    }
}

impl Drop for NamedPipeListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl NamedPipeStream {
    fn new(handle: isize) -> Self {
        let shared = Arc::new(Shared {
            handles: Mutex::new(vec![handle]),
        });
        Self { handle, shared }
    }
}

impl Stream for NamedPipeStream {
    fn try_clone(&self) -> io::Result<Box<dyn Stream>> {
        let mut duplicate: HANDLE = null_mut();
        let process = unsafe { GetCurrentProcess() };
        let result = unsafe {
            DuplicateHandle(
                process,
                self.handle as HANDLE,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if result == 0 {
            return Err(last_error("DuplicateHandle", &pipe_name(self.handle)));
        }
        let handle = duplicate as isize;
        self.shared
            .handles
            .lock()
            .expect("pipe handles poisoned")
            .push(handle);
        Ok(Box::new(Self {
            handle,
            shared: Arc::clone(&self.shared),
        }))
    }

    fn shutdown(&mut self) -> io::Result<()> {
        let handles = self
            .shared
            .handles
            .lock()
            .expect("pipe handles poisoned")
            .clone();
        for handle in handles {
            unsafe {
                CancelIoEx(handle as HANDLE, null());
            }
        }
        unsafe {
            DisconnectNamedPipe(self.handle as HANDLE);
        }
        Ok(())
    }
}

impl Read for NamedPipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut read = 0u32;
        let result = unsafe {
            ReadFile(
                self.handle as HANDLE,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        };
        if result == 0 {
            let error = unsafe { GetLastError() };
            if is_eof(error) {
                return Ok(0);
            }
            return Err(pipe_error("ReadFile", &pipe_name(self.handle), error));
        }
        Ok(read as usize)
    }
}

impl Write for NamedPipeStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut written = 0u32;
        let result = unsafe {
            WriteFile(
                self.handle as HANDLE,
                buffer.as_ptr(),
                buffer.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if result == 0 {
            let error = unsafe { GetLastError() };
            return Err(pipe_error("WriteFile", &pipe_name(self.handle), error));
        }
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for NamedPipeStream {
    fn drop(&mut self) {
        self.shared
            .handles
            .lock()
            .expect("pipe handles poisoned")
            .retain(|handle| *handle != self.handle);
        unsafe {
            CloseHandle(self.handle as HANDLE);
        }
    }
}

pub fn bind(name: &str) -> io::Result<NamedPipeListener> {
    let handle = create_pipe(name, true)?;
    Ok(NamedPipeListener {
        name: name.to_string(),
        state: Mutex::new(ListenerState {
            handle: handle as isize,
            connecting: INVALID,
            cancelled: false,
        }),
    })
}

pub fn connect(name: &str) -> io::Result<NamedPipeStream> {
    let wide = wide(name);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateFileW", name));
    }
    Ok(NamedPipeStream::new(handle as isize))
}

pub fn default_endpoint() -> Endpoint {
    let session = current_session_id();
    let user = current_user_name().unwrap_or_else(|| "user".to_string());
    Endpoint::NamedPipe(endpoint_name(session, &user))
}

pub(crate) fn normalize_name(name: &str) -> String {
    if name.starts_with(r"\\.\pipe\") {
        name.to_string()
    } else {
        format!(r"\\.\pipe\{name}")
    }
}

fn endpoint_name(session_id: u32, user: &str) -> String {
    normalize_name(&format!("voime-{session_id}-{user}"))
}

fn current_session_id() -> u32 {
    let mut session = 0u32;
    let result = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    if result == 0 {
        0
    } else {
        session
    }
}

fn current_user_name() -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    let result = unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut size) };
    if result == 0 {
        return std::env::var("USERNAME").ok();
    }
    let length = buffer.iter().position(|value| *value == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

fn current_user_sddl() -> String {
    "D:P(A;;GA;;;OW)".to_string()
}

fn create_pipe(name: &str, first_instance: bool) -> io::Result<HANDLE> {
    let sddl = wide(&current_user_sddl());
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    };
    if converted == 0 {
        return Err(last_error("ConvertStringSecurityDescriptor", name));
    }

    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide_name = wide(name);
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first_instance {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let handle = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &attributes,
        )
    };
    let error = if handle == INVALID_HANDLE_VALUE {
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    unsafe {
        LocalFree(descriptor as *mut c_void);
    }

    if let Some(error) = error {
        if error == ERROR_ACCESS_DENIED {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("daemon 已在运行：{name}"),
            ));
        }
        return Err(pipe_error("CreateNamedPipeW", name, error));
    }
    Ok(handle)
}

fn is_eof(error: u32) -> bool {
    matches!(
        error,
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_OPERATION_ABORTED
    )
}

fn interrupted_error(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Interrupted,
        format!("命名管道已关闭（{name}）"),
    )
}

fn last_error(operation: &str, name: &str) -> io::Error {
    pipe_error(operation, name, unsafe { GetLastError() })
}

fn pipe_error(operation: &str, name: &str, code: u32) -> io::Error {
    let kind = if code == ERROR_ACCESS_DENIED {
        io::ErrorKind::PermissionDenied
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, format!("{operation} 失败（{name}，Win32 {code}）"))
}

fn pipe_name(handle: isize) -> String {
    format!("handle {handle}")
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
