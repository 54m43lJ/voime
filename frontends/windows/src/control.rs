use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use voime_ipc::{write_message, ClientMessage, Role, PROTOCOL_VERSION};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::paths;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const STOP_WAIT: Duration = Duration::from_secs(2);

pub struct Daemon {
    child: Child,
    job: Option<HANDLE>,
    stopped: bool,
}

impl Daemon {
    pub fn start() -> Result<Self> {
        let mut job = create_job().ok();
        let executable = paths::daemon_path();
        let child = Command::new(&executable)
            .creation_flags(CREATE_NO_WINDOW.0)
            .spawn()
            .with_context(|| format!("启动 daemon 失败 {}", executable.display()))?;
        if let Some(handle) = job {
            let process = HANDLE(child.as_raw_handle());
            let assigned = unsafe { AssignProcessToJobObject(handle, process) }.is_ok();
            if !assigned {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                job = None;
            }
        }
        Ok(Self {
            child,
            job,
            stopped: false,
        })
    }

    pub fn has_exited(&mut self) -> bool {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .unwrap_or(true)
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = quit_running();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
        if let Some(job) = self.job.take() {
            unsafe {
                let _ = CloseHandle(job);
            }
        }
    }
}

pub fn stop_existing() {
    let endpoint = voime_transport::default_endpoint();
    if quit_running_at(&endpoint).is_err() {
        return;
    }
    let deadline = Instant::now() + STOP_WAIT;
    while Instant::now() < deadline {
        if voime_transport::connect(&endpoint).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn quit_running() -> Result<()> {
    quit_running_at(&voime_transport::default_endpoint())
}

fn quit_running_at(endpoint: &voime_transport::Endpoint) -> Result<()> {
    let mut stream = voime_transport::connect(endpoint)
        .with_context(|| format!("连接 daemon 失败 {endpoint}"))?;
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            role: Role::Control,
            version: PROTOCOL_VERSION,
            config: None,
        },
    )?;
    write_message(&mut stream, &ClientMessage::Quit)?;
    Ok(())
}

fn create_job() -> windows::core::Result<HANDLE> {
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let result = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if let Err(error) = result {
            let _ = CloseHandle(job);
            return Err(error);
        }
        Ok(job)
    }
}
