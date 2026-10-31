use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::{Context, Result};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::paths;

pub fn start_daemon() -> Result<()> {
    let executable = paths::daemon_path();
    Command::new(&executable)
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .with_context(|| format!("启动 daemon 失败 {}", executable.display()))?;
    Ok(())
}

pub fn stop_daemon() -> Result<()> {
    let executable = paths::cli_path();
    Command::new(&executable)
        .arg("ctl")
        .arg("quit")
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .with_context(|| format!("停止 daemon 失败 {}", executable.display()))?;
    Ok(())
}
