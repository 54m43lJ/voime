use std::path::{Path, PathBuf};

use voime_core::audio::capture;
use voime_core::config;
use voime_core::model::{ModelManifest, MANIFEST_FILE};
use voime_ipc::{FrontendState, Role, ServerMessage, WorkerDiagnostics, WorkerState};
use voime_transport::Endpoint;

use crate::cli::BackendId;
use crate::client::Client;
use crate::config::FinalConfig;

pub fn run(endpoint: &Endpoint) -> bool {
    let mut ok = true;
    println!("endpoint: {endpoint}");
    check_endpoint(endpoint, &mut ok);
    let (snapshot, diagnostics) = check_daemon(endpoint, &mut ok);
    check_final_backend(&mut ok);
    check_workers(diagnostics.as_ref(), &mut ok);
    check_models(&mut ok);
    check_audio(&mut ok);
    check_log();
    #[cfg(unix)]
    check_fcitx(snapshot.as_ref(), &mut ok);
    #[cfg(windows)]
    check_windows(snapshot.as_ref(), &mut ok);
    ok
}

fn check_endpoint(endpoint: &Endpoint, ok: &mut bool) {
    #[cfg(unix)]
    if let Some(path) = endpoint.as_unix_path() {
        match std::fs::metadata(path) {
            Ok(metadata) => {
                use std::os::unix::fs::PermissionsExt;
                let mode = metadata.permissions().mode() & 0o777;
                if mode == 0o600 {
                    println!("  OK   权限 0600");
                } else {
                    println!("  WARN 权限 {mode:o}，期望 0600");
                }
                if let Some(parent) = path.parent() {
                    match std::fs::metadata(parent) {
                        Ok(metadata) => {
                            let parent_mode = metadata.permissions().mode() & 0o777;
                            if parent_mode == 0o700 {
                                println!("  OK   父目录权限 0700");
                            } else {
                                println!("  WARN 父目录权限 {parent_mode:o}，期望 0700");
                            }
                        }
                        Err(error) => {
                            println!("  FAIL 无法读取父目录：{error}");
                            *ok = false;
                        }
                    }
                }
            }
            Err(error) => {
                println!("  FAIL 端点不存在：{error}");
                *ok = false;
            }
        }
    }
    #[cfg(windows)]
    {
        let name = endpoint.to_string();
        if name.starts_with(r"\\.\pipe\") {
            println!("  OK   命名管道");
        } else {
            println!("  FAIL 端点不是命名管道：{name}");
            *ok = false;
        }
    }
}

fn check_daemon(
    endpoint: &Endpoint,
    ok: &mut bool,
) -> (Option<ServerMessage>, Option<WorkerDiagnosticsAnswer>) {
    match Client::connect_as(endpoint, Role::Control) {
        Ok(mut client) => match client.snapshot() {
            Ok(message) => {
                println!("  OK   daemon 在线");
                match client.diagnostics() {
                    Ok(diagnostics) => (Some(message), Some(diagnostics)),
                    Err(error) => {
                        println!("  WARN 无法读取 worker 诊断：{error:#}");
                        (Some(message), None)
                    }
                }
            }
            Err(error) => {
                println!("  FAIL daemon 握手失败：{error:#}");
                *ok = false;
                (None, None)
            }
        },
        Err(error) => {
            println!("  FAIL 无法连接 daemon：{error:#}");
            *ok = false;
            (None, None)
        }
    }
}

type WorkerDiagnosticsAnswer = (String, Option<WorkerDiagnostics>);

fn check_final_backend(ok: &mut bool) {
    println!("final:");
    let config = match FinalConfig::load() {
        Ok(config) => config,
        Err(error) => {
            println!("  FAIL 无法读取 Final 后端配置：{error:#}");
            *ok = false;
            return;
        }
    };
    println!("  OK   backend = {}", config.backend);
    if config.backend == BackendId::Cpu {
        println!("  OK   CPU Final 可用（SenseVoice）");
    } else {
        println!(
            "  OK   已选择 {}；Final 由对应 GPU worker 执行",
            config.backend
        );
    }
    let root = config::default_model_root();
    for backend in [BackendId::Qwen3] {
        let installed = model_dirs_present(&root, backend_model_dirs(backend));
        if backend == config.backend {
            if installed {
                println!("  OK   {} 已安装并选中", backend);
            } else {
                println!("  FAIL {} 已选中但模型目录缺失", backend);
                *ok = false;
            }
        } else if installed {
            println!("  WARN {} 已安装但未选择", backend);
        } else {
            println!("  WARN {} 未安装（未选择）", backend);
        }
    }
}

fn check_workers(diagnostics: Option<&WorkerDiagnosticsAnswer>, ok: &mut bool) {
    println!("worker:");
    let config = match FinalConfig::load() {
        Ok(config) => config,
        Err(_) => return,
    };
    for backend in [BackendId::Cpu, BackendId::Qwen3] {
        check_worker(backend, config.backend == backend, diagnostics, ok);
    }
}

fn check_worker(
    backend: BackendId,
    selected: bool,
    diagnostics: Option<&WorkerDiagnosticsAnswer>,
    ok: &mut bool,
) {
    if backend == BackendId::Cpu {
        println!("  OK   cpu 内置 SenseVoice");
        return;
    }
    let root = config::default_model_root();
    let executable = worker_executable(backend);
    let models = model_dirs_present(&root, backend_model_dirs(backend));
    let label = backend.as_str();
    match (executable, models) {
        (Some(path), true) => {
            println!("  OK   {label} worker {}", path.display());
            println!("  OK   {label} 模型目录完整");
            if !selected {
                println!("  WARN {label} 未选择，daemon 不要求该 worker 运行");
                return;
            }
            let Some((reported, worker)) = diagnostics else {
                println!("  WARN {label} 已选择，但无法从 daemon 读取 worker 状态");
                return;
            };
            if reported != label || worker.is_none() {
                println!("  FAIL {label} 已选择，但 daemon 报告的后端状态不匹配");
                *ok = false;
                return;
            }
            let worker = worker.as_ref().expect("checked above");
            print_worker_state(label, worker, ok);
        }
        _ => {
            if selected {
                println!("  FAIL {label} 已选择但 worker 或模型目录缺失");
                *ok = false;
            } else {
                println!("  WARN {label} 未安装");
            }
        }
    }
}

fn print_worker_state(label: &str, worker: &WorkerDiagnostics, ok: &mut bool) {
    match worker.state {
        WorkerState::Starting => {
            println!("  WARN {label} worker 正在启动或未完成握手");
        }
        WorkerState::Ready | WorkerState::Running => {
            let state = match worker.state {
                WorkerState::Running => "running",
                _ => "ready",
            };
            println!(
                "  OK   {label} {state} runtime={} device={} vram_mb={} punc={}",
                worker.runtime.as_deref().unwrap_or("-"),
                worker.device.as_deref().unwrap_or("-"),
                worker.vram_mb,
                worker.punc
            );
        }
        WorkerState::Unavailable => {
            println!(
                "  FAIL {label} 已选择但 worker 不可用{}",
                worker
                    .error
                    .as_deref()
                    .map(|error| format!("：{error}"))
                    .unwrap_or_default()
            );
            *ok = false;
        }
    }
}

fn check_models(ok: &mut bool) {
    let root = config::default_model_root();
    println!("model: {}", root.display());
    for (directory, name) in [
        ("firered-vad", "firered-vad"),
        ("sensevoice-small", "sensevoice-small"),
    ] {
        let dir = root.join(directory);
        let path = dir.join(MANIFEST_FILE);
        match ModelManifest::load(&path).and_then(|manifest| {
            manifest.verify(&dir)?;
            Ok(manifest)
        }) {
            Ok(manifest) => println!(
                "  OK   {name} manifest 与 SHA-256 校验通过（schema {}）",
                manifest.schema
            ),
            Err(error) => {
                println!("  FAIL {name} 校验失败：{error:#}");
                *ok = false;
            }
        }
    }
    for (directory, kind, backend, format, precision) in
        [("qwen3-asr-1.7b", "asr", BackendId::Qwen3, "gguf", "bf16")]
    {
        let dir = root.join(directory);
        let path = dir.join(MANIFEST_FILE);
        if !path.is_file() {
            println!("  WARN {directory} 未安装（{}）", backend);
            continue;
        }
        match verify_gpu_manifest(&dir, directory, kind, backend, format, precision) {
            Ok(()) => println!("  OK   {directory} manifest 校验通过（未加载权重）"),
            Err(error) => {
                println!("  FAIL {directory} manifest 校验失败：{error:#}");
                *ok = false;
            }
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

fn check_log() {
    println!("log:");
    match std::env::var_os("VOIME_LOG") {
        Some(value) if !value.is_empty() => {
            let directory = log_directory();
            let raw = value.to_string_lossy();
            if raw == "1" || raw.eq_ignore_ascii_case("true") || raw.eq_ignore_ascii_case("on") {
                println!(
                    "  OK   VOIME_LOG 已启用，daemon 与托盘将写入 {}",
                    directory.display()
                );
            } else {
                println!("  OK   VOIME_LOG 使用自定义目录 {}", directory.display());
            }
        }
        _ => {
            println!(
                "  WARN 未启用（设 VOIME_LOG=1 开启，默认 {}）",
                voime_log::default_directory().display()
            );
        }
    }
}

fn log_directory() -> PathBuf {
    match std::env::var_os("VOIME_LOG") {
        Some(value) if !value.is_empty() => {
            let raw = value.to_string_lossy();
            if raw == "1" || raw.eq_ignore_ascii_case("true") || raw.eq_ignore_ascii_case("on") {
                voime_log::default_directory()
            } else {
                PathBuf::from(value)
            }
        }
        _ => voime_log::default_directory(),
    }
}

fn verify_gpu_manifest(
    dir: &Path,
    directory: &str,
    kind: &str,
    backend: BackendId,
    format: &str,
    precision: &str,
) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    if value.get("schema").and_then(|value| value.as_u64()) != Some(5) {
        anyhow::bail!("schema 不是 5");
    }
    if value.get("id").and_then(|value| value.as_str()) != Some(directory) {
        anyhow::bail!("id 与目录名不一致");
    }
    if value.get("kind").and_then(|value| value.as_str()) != Some(kind) {
        anyhow::bail!("kind 不是 {kind}");
    }
    let runtime = value
        .get("runtime")
        .and_then(|value| value.as_object())
        .ok_or_else(|| anyhow::anyhow!("缺少 runtime"))?;
    if runtime.get("backend").and_then(|value| value.as_str()) != Some(backend.as_str()) {
        anyhow::bail!("runtime.backend 不是 {}", backend);
    }
    if runtime.get("format").and_then(|value| value.as_str()) != Some(format) {
        anyhow::bail!("runtime.format 不是 {format}");
    }
    if runtime.get("precision").and_then(|value| value.as_str()) != Some(precision) {
        anyhow::bail!("runtime.precision 不是 {precision}");
    }
    let max_segment_ms = runtime
        .get("max_segment_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if max_segment_ms == 0 || max_segment_ms > 30_000 {
        anyhow::bail!("runtime.max_segment_ms 超出 1..=30000");
    }
    let files = value
        .get("files")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("缺少 files"))?;
    if files.is_empty() {
        anyhow::bail!("files 为空");
    }
    for file in files {
        let relative = file
            .get("path")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("文件缺少 path"))?;
        let relative = Path::new(relative);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            anyhow::bail!("文件路径非法：{}", relative.display());
        }
        let path = dir.join(relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() == 0 {
            anyhow::bail!("文件缺失或不是普通文件：{}", path.display());
        }
        let sha = file
            .get("sha256")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow::anyhow!("文件缺少 sha256"))?;
        if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            anyhow::bail!("SHA-256 格式非法：{}", relative.display());
        }
    }
    Ok(())
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
    let addon = addon_path("addon/voime.conf");
    let library = library_path();
    for (label, path) in [("addon 描述", addon.clone()), ("voime.so", library.clone())] {
        match path {
            Some(path) if path.is_file() => println!("  OK   {label} {}", path.display()),
            _ => {
                println!("  FAIL 未找到{label}");
                *ok = false;
            }
        }
    }
    if let Some(library) = library {
        match system_addon_abi(&library) {
            Ok(()) => println!("  OK   系统 addon ABI 与 Fcitx5 匹配"),
            Err(error) => {
                println!("  FAIL 系统 addon ABI 检查失败：{error:#}");
                *ok = false;
            }
        }
    }
    match user_config_path() {
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

fn backend_model_dirs(backend: BackendId) -> &'static [&'static str] {
    match backend {
        BackendId::Cpu => &[],
        BackendId::Qwen3 => &["qwen3-asr-1.7b"],
    }
}

fn model_dirs_present(root: &Path, dirs: &[&str]) -> bool {
    !dirs.is_empty() && dirs.iter().all(|dir| root.join(dir).is_dir())
}

fn worker_executable(backend: BackendId) -> Option<PathBuf> {
    let name = match backend {
        BackendId::Cpu => return None,
        BackendId::Qwen3 => {
            if cfg!(windows) {
                "voime-gpu-qwen3.exe"
            } else {
                "voime-gpu-qwen3"
            }
        }
    };
    if let Some(path) = std::env::var_os(format!("VOIME_{}_BIN", backend.as_str().to_uppercase())) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join(name);
    sibling.is_file().then_some(sibling)
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
    let mut candidates = Vec::new();
    if let Ok(output) = std::process::Command::new("pkg-config")
        .args(["--variable=libdir", "Fcitx5Core"])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                candidates.push(PathBuf::from(path).join("fcitx5").join("voime.so"));
            }
        }
    }
    for prefix in ["/usr", "/usr/local"] {
        candidates.push(
            PathBuf::from(prefix)
                .join("lib")
                .join("fcitx5")
                .join("voime.so"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(unix)]
fn system_addon_abi(library: &Path) -> anyhow::Result<()> {
    if let Ok(status) = std::process::Command::new("pkg-config")
        .args(["--atleast-version=5.1.23", "Fcitx5Core"])
        .status()
    {
        if !status.success() {
            anyhow::bail!("Fcitx5 >= 5.1.23 未安装");
        }
    }
    let output = std::process::Command::new("ldd")
        .arg(library)
        .output()
        .map_err(|error| anyhow::anyhow!("运行 ldd 失败：{error}"))?;
    if !output.status.success() {
        anyhow::bail!("ldd 退出码非零");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if let Some(line) = text.lines().find(|line| line.contains("not found")) {
        anyhow::bail!("{}", line.trim());
    }
    Ok(())
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
