use voime_core::audio::capture;
use voime_core::config::default_model_dir;
use voime_core::model::{ModelManifest, MANIFEST_FILE};
use voime_ipc::{FrontendState, Role, ServerMessage};
use voime_transport::Endpoint;

use crate::client::Client;

pub fn run(endpoint: &Endpoint) -> bool {
    let mut ok = true;
    println!("endpoint: {endpoint}");
    check_endpoint(endpoint, &mut ok);
    let snapshot = check_daemon(endpoint, &mut ok);
    check_model(&mut ok);
    check_audio(&mut ok);
    #[cfg(unix)]
    check_fcitx(snapshot.as_ref(), &mut ok);
    #[cfg(windows)]
    check_windows(snapshot.as_ref(), &mut ok);
    ok
}

#[cfg(unix)]
fn check_endpoint(endpoint: &Endpoint, ok: &mut bool) {
    if let Some(path) = endpoint.as_unix_path() {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() & 0o777;
                if mode == 0o600 {
                    println!("  OK   权限 0600");
                } else {
                    println!("  WARN 权限 {:o}，期望 0600", mode);
                }
            }
            Err(error) => {
                println!("  FAIL 端点不存在：{error}");
                *ok = false;
            }
        }
    }
}

#[cfg(windows)]
fn check_endpoint(endpoint: &Endpoint, ok: &mut bool) {
    let name = endpoint.to_string();
    if name.starts_with(r"\\.\pipe\") {
        println!("  OK   命名管道");
    } else {
        println!("  FAIL 端点不是命名管道：{name}");
        *ok = false;
    }
}

fn check_daemon(endpoint: &Endpoint, ok: &mut bool) -> Option<ServerMessage> {
    match Client::connect_as(endpoint, Role::Control) {
        Ok(mut client) => match client.snapshot() {
            Ok(message) => {
                println!("  OK   daemon 在线");
                Some(message)
            }
            Err(error) => {
                println!("  FAIL daemon 握手失败：{error:#}");
                *ok = false;
                None
            }
        },
        Err(error) => {
            println!("  FAIL 无法连接 daemon：{error:#}");
            *ok = false;
            None
        }
    }
}

fn check_model(ok: &mut bool) {
    let model_dir = default_model_dir();
    let manifest_path = model_dir.join(MANIFEST_FILE);
    println!("model: {}", model_dir.display());
    match ModelManifest::load(&manifest_path) {
        Ok(manifest) => {
            if manifest.verify(&model_dir).is_ok() {
                println!("  OK   manifest 与 SHA-256 校验通过");
            } else {
                println!("  FAIL 模型文件缺失或 SHA-256 不匹配");
                *ok = false;
            }
        }
        Err(error) => {
            println!("  FAIL 无法读取 manifest.json：{error:#}");
            *ok = false;
        }
    }
}

fn check_audio(ok: &mut bool) {
    println!("audio:");
    match capture::list_input_devices() {
        Ok(devices) if devices.is_empty() => {
            println!("  FAIL 没有输入设备");
            *ok = false;
        }
        Ok(devices) => {
            let default = capture::default_input_device_name()
                .ok()
                .flatten()
                .unwrap_or_else(|| "-".to_string());
            println!("  OK   {} 个输入设备，默认 {}", devices.len(), default);
        }
        Err(error) => {
            println!("  FAIL 枚举输入设备失败：{error:#}");
            *ok = false;
        }
    }
}

#[cfg(unix)]
fn check_fcitx(snapshot: Option<&ServerMessage>, ok: &mut bool) {
    println!("fcitx:");
    let process = fcitx_running();
    println!(
        "  {}  fcitx5 进程{}",
        if process { "OK  " } else { "FAIL" },
        if process { "运行中" } else { "未运行" }
    );
    if !process {
        *ok = false;
    }
    let addon_dir = addon_path("addon/voime.conf");
    let library = library_path();
    let user_config = user_config_path();
    for (label, path) in [("addon 描述", addon_dir), ("voime.so", library)] {
        match path {
            Some(path) if path.is_file() => println!("  OK   {label} {}", path.display()),
            _ => {
                println!("  FAIL 未找到{label}");
                *ok = false;
            }
        }
    }
    match addon_path("inputmethod/voime.conf") {
        Some(path) if path.is_file() => {
            println!(
                "  FAIL 旧输入法条目仍在 {}，请重新安装 addon 或删除该文件",
                path.display()
            );
            *ok = false;
        }
        _ => println!("  OK   未注册输入法条目（Shift 不会切换到 Voime）"),
    }
    match &user_config {
        Some(path) if path.is_file() => println!("  OK   用户配置 {}", path.display()),
        _ => println!("  WARN 尚无用户配置（将使用默认 Mode=ptt Hotkey=Super+F2）"),
    }
    print_snapshot(snapshot, "Fcitx addon");
}

#[cfg(windows)]
fn check_windows(snapshot: Option<&ServerMessage>, ok: &mut bool) {
    println!("frontend:");
    let tray = process_running("voime-tray.exe");
    println!(
        "  {}  voime-tray.exe {}",
        if tray { "OK  " } else { "WARN" },
        if tray { "运行中" } else { "未运行" }
    );
    match microphone_privacy() {
        Some(value) if value.eq_ignore_ascii_case("Allow") => {
            println!("  OK   麦克风隐私 Allow");
        }
        Some(value) if value.eq_ignore_ascii_case("Deny") => {
            println!("  FAIL 麦克风隐私 Deny，请在 Windows 设置中允许桌面应用访问麦克风");
            *ok = false;
        }
        Some(value) => println!("  WARN 麦克风隐私值 {value}"),
        None => println!("  WARN 未读取到麦克风隐私设置"),
    }
    println!("  OK   未启用 TSF（可选）");
    print_snapshot(snapshot, "前端");
}

fn print_snapshot(snapshot: Option<&ServerMessage>, label: &str) {
    let Some(ServerMessage::Snapshot {
        mode,
        hotkey,
        addon,
        ..
    }) = snapshot
    else {
        return;
    };
    println!(
        "  OK   Mode={} Hotkey={} addon={}",
        mode.map(|mode| mode.as_str()).unwrap_or("-"),
        hotkey.as_deref().unwrap_or("-"),
        match addon {
            FrontendState::Connected => "connected",
            FrontendState::Disconnected => "disconnected",
        }
    );
    if *addon == FrontendState::Disconnected {
        println!("  WARN {label} 未连接 daemon");
    }
}

#[cfg(unix)]
fn fcitx_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            if comm.trim() == "fcitx5" {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn addon_path(relative: &str) -> Option<std::path::PathBuf> {
    for prefix in ["/usr", "/usr/local"] {
        let path = std::path::PathBuf::from(prefix)
            .join("share")
            .join("fcitx5")
            .join(relative);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(unix)]
fn library_path() -> Option<std::path::PathBuf> {
    for prefix in ["/usr", "/usr/local"] {
        let path = std::path::PathBuf::from(prefix)
            .join("lib")
            .join("fcitx5")
            .join("voime.so");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(unix)]
fn user_config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .map(|base| base.join("fcitx5").join("conf").join("voime.conf"))
}

#[cfg(windows)]
fn process_running(name: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = false;
    let mut current = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while current {
        let length = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let executable = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if executable.eq_ignore_ascii_case(name) {
            found = true;
            break;
        }
        current = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    found
}

#[cfg(windows)]
fn microphone_privacy() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ,
    };

    const PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessConsent\\DeviceAccess\\ConsentStore\\microphone";
    let path = wide(PATH);
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    let value_name = wide("Value");
    let mut kind = REG_SZ;
    let mut size = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 || size == 0 {
        unsafe {
            let _ = RegCloseKey(key);
        }
        return None;
    }
    let mut bytes = vec![0u8; size as usize];
    let status = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if status != 0 {
        return None;
    }
    if kind != REG_SZ {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
