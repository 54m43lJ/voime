use std::path::PathBuf;

use voime_core::audio::capture;
use voime_core::config::default_model_dir;
use voime_core::model::{ModelManifest, MANIFEST_FILE};
use voime_ipc::{FrontendState, Role, ServerMessage};
use voime_transport::Endpoint;

use crate::client::Client;

pub fn run(endpoint: &Endpoint) -> bool {
    let mut ok = true;
    println!("endpoint: {endpoint}");
    #[cfg(unix)]
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
                ok = false;
            }
        }
    }

    let mut snapshot = None;
    match Client::connect_as(endpoint, Role::Control) {
        Ok(mut client) => match client.snapshot() {
            Ok(message) => {
                println!("  OK   daemon 在线");
                snapshot = Some(message);
            }
            Err(error) => {
                println!("  FAIL daemon 握手失败：{error:#}");
                ok = false;
            }
        },
        Err(error) => {
            println!("  FAIL 无法连接 daemon：{error:#}");
            ok = false;
        }
    }

    let model_dir = default_model_dir();
    let manifest_path = model_dir.join(MANIFEST_FILE);
    println!("model: {}", model_dir.display());
    match ModelManifest::load(&manifest_path) {
        Ok(manifest) => {
            if manifest.verify(&model_dir).is_ok() {
                println!("  OK   manifest 与 SHA-256 校验通过");
            } else {
                println!("  FAIL 模型文件缺失或 SHA-256 不匹配");
                ok = false;
            }
        }
        Err(error) => {
            println!("  FAIL 无法读取 manifest.json：{error:#}");
            ok = false;
        }
    }

    println!("audio:");
    match capture::list_input_devices() {
        Ok(devices) if devices.is_empty() => {
            println!("  FAIL 没有输入设备");
            ok = false;
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
            ok = false;
        }
    }

    println!("fcitx:");
    let process = fcitx_running();
    println!(
        "  {}  fcitx5 进程{}",
        if process { "OK  " } else { "FAIL" },
        if process { "运行中" } else { "未运行" }
    );
    if !process {
        ok = false;
    }
    let addon_dir = addon_path("addon/voime.conf");
    let library = library_path();
    let user_config = user_config_path();
    for (label, path) in [("addon 描述", addon_dir), ("voime.so", library)] {
        match path {
            Some(path) if path.is_file() => println!("  OK   {label} {}", path.display()),
            _ => {
                println!("  FAIL 未找到{label}");
                ok = false;
            }
        }
    }
    match addon_path("inputmethod/voime.conf") {
        Some(path) if path.is_file() => {
            println!(
                "  FAIL 旧输入法条目仍在 {}，请重新安装 addon 或删除该文件",
                path.display()
            );
            ok = false;
        }
        _ => println!("  OK   未注册输入法条目（Shift 不会切换到 Voime）"),
    }
    match &user_config {
        Some(path) if path.is_file() => println!("  OK   用户配置 {}", path.display()),
        _ => println!("  WARN 尚无用户配置（将使用默认 Mode=ptt Hotkey=Super+F2）"),
    }

    if let Some(ServerMessage::Snapshot {
        mode,
        hotkey,
        addon,
        ..
    }) = snapshot
    {
        println!(
            "  OK   Mode={} Hotkey={} addon={}",
            mode.map(|mode| mode.as_str()).unwrap_or("-"),
            hotkey.as_deref().unwrap_or("-"),
            match addon {
                FrontendState::Connected => "connected",
                FrontendState::Disconnected => "disconnected",
            }
        );
        if addon == FrontendState::Disconnected {
            println!("  WARN Fcitx addon 未连接 daemon");
        }
    }

    ok
}

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

fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("fcitx5").join("conf").join("voime.conf"))
}
