use std::os::windows::process::CommandExt;
use std::process::Command;

use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::paths::cli_path;

pub fn doctor_text() -> String {
    let executable = cli_path();
    let mut command = Command::new(&executable);
    command.arg("doctor").creation_flags(CREATE_NO_WINDOW.0);
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => return format!("运行 doctor 失败：{error:#}"),
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if text.trim().is_empty() {
        text = format!("doctor 退出码：{:?}", output.status.code());
    }
    text
}
