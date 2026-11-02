use std::path::{Path, PathBuf};

use voime_core::audio::capture;
use voime_core::config::{self, ACTIVE_FILE};
use voime_core::model::{BackendKind, ModelManifest, MANIFEST_FILE, PREVIEW_PACKAGE_ID};
use voime_ipc::{FrontendState, Role, ServerMessage};
use voime_transport::Endpoint;

use crate::client::Client;

pub fn run(endpoint: &Endpoint) -> bool {
    let mut ok = true;
    println!("endpoint: {endpoint}");
    check_endpoint(endpoint, &mut ok);
    let snapshot = check_daemon(endpoint, &mut ok);
    check_models(&mut ok);
    check_gpu(&mut ok);
    check_audio(&mut ok);
    check_log();
    #[cfg(unix)]
    check_fcitx(snapshot.as_ref(), &mut ok);
    #[cfg(windows)]
    check_windows(snapshot.as_ref(), &mut ok);
    ok
}

fn check_log() {
    println!("log:");
    if voime_log::enabled() {
        if let Some(path) = voime_log::log_path() {
            println!("  OK   已启用 {}", path.display());
        }
    } else if std::env::var_os("VOIME_LOG")
        .map(|value| !value.is_empty())
        .unwrap_or(false)
    {
        println!(
            "  OK   VOIME_LOG 已设置，daemon 与托盘将写入 {}",
            voime_log::default_directory().display()
        );
    } else {
        println!(
            "  WARN 未启用（设 VOIME_LOG=1 开启，默认 {}）",
            voime_log::default_directory().display()
        );
    }
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

fn check_models(ok: &mut bool) {
    let root = config::default_model_root();
    println!("model: {}", root.display());
    check_package(&root.join(PREVIEW_PACKAGE_ID), "SenseVoice 基础包", ok);

    let active = match config::read_active_id(&root) {
        Ok(Some(id)) => Some(id),
        Ok(None) => {
            println!(
                "  FAIL 缺少 {}；Final 选择未知",
                root.join(ACTIVE_FILE).display()
            );
            *ok = false;
            None
        }
        Err(error) => {
            println!("  FAIL 读取 active 失败：{error:#}");
            *ok = false;
            None
        }
    };
    let Some(active) = active else {
        return;
    };
    println!("  OK   active Final = {active}");
    let Ok(dir) = config::resolve_package(&root, &active) else {
        println!("  FAIL active 包名非法：{active}");
        *ok = false;
        return;
    };
    let Some(mut manifest) = check_package(&dir, "Final 包", ok) else {
        return;
    };
    if let Err(error) = manifest.rebase(&dir) {
        println!("  FAIL Final 路径重定位失败：{error:#}");
        *ok = false;
        return;
    }
    for required in &manifest.requires {
        let required_dir = root.join(required);
        if required_dir.join(MANIFEST_FILE).is_file() {
            println!("  OK   requires {required}");
        } else {
            println!("  FAIL Final 缺少依赖 {required}");
            *ok = false;
        }
    }
}

fn check_package(dir: &Path, label: &str, ok: &mut bool) -> Option<ModelManifest> {
    let path = dir.join(MANIFEST_FILE);
    match ModelManifest::load(&path) {
        Ok(manifest) => match manifest.verify(dir) {
            Ok(()) => {
                println!("  OK   {label} {}", manifest.display_name);
                Some(manifest)
            }
            Err(error) => {
                println!("  FAIL {label} 文件缺失或 SHA-256 不匹配：{error:#}");
                *ok = false;
                None
            }
        },
        Err(error) => {
            println!("  FAIL 无法读取 {label}：{error:#}");
            *ok = false;
            None
        }
    }
}

fn check_gpu(ok: &mut bool) {
    println!("gpu:");
    let root = config::default_model_root();
    let active = config::read_active_id(&root).ok().flatten();
    let mut worker_packages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path().join(MANIFEST_FILE);
            let Ok(manifest) = ModelManifest::load(&path) else {
                continue;
            };
            if manifest.backend.kind == BackendKind::Worker {
                let runtime = manifest.backend.runtime.clone().unwrap_or_default();
                worker_packages.push((entry.file_name().to_string_lossy().to_string(), runtime));
            }
        }
    }
    if worker_packages.is_empty() {
        println!("  WARN 未安装 GPU Final 包；Final 使用 CPU SenseVoice");
        return;
    }
    let mut runtime_ok = false;
    for (id, runtime) in &worker_packages {
        let available = runtime_available(runtime) && worker_binary().is_some();
        runtime_ok |= available;
        let selected = active.as_deref() == Some(id.as_str());
        if available {
            println!("  OK   {id} runtime={runtime} worker 可用");
        } else if selected {
            println!("  WARN {id} runtime={runtime} 不可用；Final 将回退 CPU SenseVoice");
        } else {
            println!("  WARN {id} runtime={runtime} 未安装");
        }
    }
    if !runtime_ok {
        println!("  WARN GPU runtime 不可用；CPU 路径仍可用");
    }
    // A missing GPU runtime is only a WARN: CPU Final remains usable.
    if worker_packages
        .iter()
        .any(|(id, _)| active.as_deref() == Some(id.as_str()))
        && !runtime_ok
    {
        let _ = ok;
    }
}

fn runtime_available(runtime: &str) -> bool {
    if let Some(dir) = std::env::var_os("VOIME_GPU_RUNTIME_DIR") {
        if !dir.is_empty() && Path::new(&dir).exists() {
            return true;
        }
    }
    if let Some(base) = std::env::var_os("VOIME_RUNTIME_DIR") {
        if Path::new(&base).join(runtime).exists() {
            return true;
        }
    }
    false
}

fn worker_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOIME_GPU_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "voime-gpu.exe"
    } else {
        "voime-gpu"
    };
    let sibling = exe.parent()?.join(name);
    sibling.is_file().then_some(sibling)
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
fn addon_path(relative: &str) -> Option<PathBuf> {
    for prefix in ["/usr", "/usr/local"] {
        let path = PathBuf::from(prefix)
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
fn library_path() -> Option<PathBuf> {
    for prefix in ["/usr", "/usr/local"] {
        let path = PathBuf::from(prefix)
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
fn user_config_path() -> Option<PathBuf> {
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        if !config.is_empty() {
            return Some(
                PathBuf::from(config)
                    .join("fcitx5")
                    .join("conf")
                    .join("voime.conf"),
            );
        }
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("fcitx5")
            .join("conf")
            .join("voime.conf")
    })
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
    if status != 0 || kind != REG_SZ {
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
