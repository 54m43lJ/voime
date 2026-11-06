use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use voime_ipc::{FrontendConfig, Mode};
use windows::core::w;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

use crate::hotkey::Hotkey;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_mode")]
    pub mode: Mode,
    #[serde(default = "default_hotkey")]
    pub hotkey: Hotkey,
    #[serde(default = "default_true")]
    pub start_at_login: bool,
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            hotkey: default_hotkey(),
            start_at_login: default_true(),
            overlay_enabled: default_true(),
        }
    }
}

impl Config {
    pub fn to_ipc_config(&self) -> FrontendConfig {
        FrontendConfig {
            mode: self.mode,
            hotkey: self.hotkey.to_string(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败 {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("序列化配置失败")?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes)
            .with_context(|| format!("写入配置文件失败 {}", temporary.display()))?;
        // std::fs::rename 在 Windows 上使用 MOVEFILE_REPLACE_EXISTING，可直接覆盖旧文件。
        fs::rename(&temporary, &path)
            .with_context(|| format!("替换配置文件失败 {}", path.display()))?;
        Ok(())
    }
}

pub fn load() -> Result<Config> {
    let path = config_path();
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("解析配置文件失败 {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(error).with_context(|| format!("读取配置文件失败 {}", path.display())),
    }
}

pub fn config_path() -> PathBuf {
    appdata_dir().join("voime").join("config.json")
}

pub fn appdata_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn set_start_at_login(enabled: bool) -> Result<()> {
    let desired = if enabled {
        let executable = std::env::current_exe().context("读取 voime-tray 路径失败")?;
        Some(format!("\"{}\"", executable.display()))
    } else {
        None
    };
    let existing = login_value();
    if existing == desired {
        // Avoid rewriting the Run value on every launch; repeated
        // persistence-style registry writes trigger AV heuristics.
        return Ok(());
    }
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(anyhow::anyhow!("打开登录启动注册表项失败：{status:?}"));
    }
    let result = (|| -> Result<()> {
        if let Some(value) = desired {
            let mut bytes = Vec::with_capacity(value.len() * 2 + 2);
            for unit in value.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes.extend_from_slice(&[0, 0]);
            let status =
                unsafe { RegSetValueExW(key, w!("Voime"), 0, REG_SZ, Some(bytes.as_slice())) };
            if status != ERROR_SUCCESS {
                return Err(anyhow::anyhow!("写入登录启动项失败：{status:?}"));
            }
        } else {
            let status = unsafe { RegDeleteValueW(key, w!("Voime")) };
            if status != ERROR_SUCCESS && status != windows::Win32::Foundation::ERROR_FILE_NOT_FOUND
            {
                return Err(anyhow::anyhow!("删除登录启动项失败：{status:?}"));
            }
        }
        Ok(())
    })();
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn login_value() -> Option<String> {
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Voime"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut bytes = vec![0u8; size as usize];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Voime"),
            RRF_RT_REG_SZ,
            None,
            Some(bytes.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .filter(|pair| pair.len() == 2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    if units.is_empty() {
        None
    } else {
        Some(String::from_utf16_lossy(&units))
    }
}

fn default_mode() -> Mode {
    Mode::Ptt
}

fn default_hotkey() -> Hotkey {
    Hotkey::default()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_windows_contract() {
        let config = Config::default();
        assert_eq!(config.mode, Mode::Ptt);
        assert_eq!(config.hotkey.to_string(), "Ctrl+Alt+Space");
        assert!(config.start_at_login);
        assert!(config.overlay_enabled);
    }

    #[test]
    fn round_trips_config_json() {
        let config = Config::default();
        let bytes = serde_json::to_vec(&config).unwrap();
        let decoded: Config = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.hotkey, config.hotkey);
        assert_eq!(decoded.mode, config.mode);
    }
}
