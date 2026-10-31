//! 轻量运行日志：由 `VOIME_LOG` 环境变量启用，按组件写追加式日志文件。
//!
//! - `VOIME_LOG` 未设置或为空：关闭；
//! - `VOIME_LOG=1|true|on`：写入默认日志目录；
//! - 其他非空值：视为日志目录路径。
//!
//! 默认目录：Windows 为 `%LOCALAPPDATA%\voime\logs`，Unix 为
//! `$XDG_STATE_HOME/voime/logs`（缺省 `~/.local/state/voime/logs`）。
//! 每个组件一个追加文件 `{component}.log`，行为隐私敏感的文本（识别结果）
//! 只记长度、不记内容；文件超过 [`ROTATE_BYTES`] 时在初始化时轮转为 `.log.1`。

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个日志文件超过该字节数后，在下次初始化时轮转。
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOGGER: OnceLock<Mutex<Option<Logger>>> = OnceLock::new();

struct Logger {
    file: File,
    path: PathBuf,
    component: String,
}

fn logger_slot() -> &'static Mutex<Option<Logger>> {
    LOGGER.get_or_init(|| Mutex::new(None))
}

fn lock() -> MutexGuard<'static, Option<Logger>> {
    logger_slot()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 按组件初始化日志。重复调用以最后一次为准。返回日志是否启用。
pub fn init(component: &str) -> bool {
    let dir = resolve_dir(std::env::var_os("VOIME_LOG"));
    let logger = dir.and_then(|dir| {
        open_log(&dir, component).ok().map(|file| Logger {
            file,
            path: dir.join(format!("{component}.log")),
            component: component.to_string(),
        })
    });
    let enabled = logger.is_some();
    *lock() = logger;
    ENABLED.store(enabled, Ordering::Relaxed);
    enabled
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 当前组件的日志文件路径；未启用时为 `None`。
pub fn log_path() -> Option<PathBuf> {
    lock().as_ref().map(|logger| logger.path.clone())
}

/// 默认日志目录（`VOIME_LOG=1` 时使用的位置）。
pub fn default_directory() -> PathBuf {
    base_state_dir().join("voime").join("logs")
}

/// 输出一行运行日志；未启用时零格式化开销。
#[macro_export]
macro_rules! vlog {
    ($($arg:tt)*) => {
        if $crate::enabled() {
            $crate::write(format_args!($($arg)*));
        }
    };
}

pub fn write(message: fmt::Arguments<'_>) {
    let mut guard = lock();
    let Some(logger) = guard.as_mut() else {
        return;
    };
    let line = render(&logger.component, message);
    let _ = logger.file.write_all(line.as_bytes());
    let _ = logger.file.flush();
}

fn render(component: &str, message: fmt::Arguments<'_>) -> String {
    format!("{} [{component}] {message}\n", timestamp())
}

fn resolve_dir(raw: Option<OsString>) -> Option<PathBuf> {
    let value = raw?;
    if value.is_empty() {
        return None;
    }
    let text = value.to_string_lossy().into_owned();
    match text.as_str() {
        "1" | "true" | "on" => Some(default_directory()),
        _ => Some(PathBuf::from(value)),
    }
}

fn base_state_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".local").join("state"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn open_log(dir: &Path, component: &str) -> io::Result<File> {
    open_log_at(dir, component, ROTATE_BYTES)
}

fn open_log_at(dir: &Path, component: &str, rotate_at: u64) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{component}.log"));
    if fs::metadata(&path)
        .map(|metadata| metadata.len() >= rotate_at)
        .unwrap_or(false)
    {
        let mut backup = path.clone().into_os_string();
        backup.push(".1");
        let _ = fs::rename(&path, PathBuf::from(backup));
    }
    OpenOptions::new().create(true).append(true).open(&path)
}

/// UTC 时间戳，格式 `2026-10-31T08:12:34.567Z`。
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// 自 1970-01-01 起的天数转日期（Howard Hinnant 的 civil_from_days 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_512), (2026, 2, 28));
        assert_eq!(civil_from_days(20_513), (2026, 3, 1));
    }

    #[test]
    fn resolves_env_value() {
        assert!(resolve_dir(None).is_none());
        assert!(resolve_dir(Some("".into())).is_none());
        assert_eq!(resolve_dir(Some("1".into())), Some(default_directory()));
        assert_eq!(
            resolve_dir(Some("1".into())),
            resolve_dir(Some("true".into()))
        );
        assert_eq!(
            resolve_dir(Some("C:\\voime-logs".into())),
            Some(PathBuf::from("C:\\voime-logs"))
        );
    }

    #[test]
    fn default_dir_is_under_voime_logs() {
        assert!(default_directory().ends_with(Path::new("voime").join("logs")));
    }

    #[test]
    fn rotates_when_over_limit() {
        let dir = std::env::temp_dir().join(format!("voime-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.log"), b"abcdef").unwrap();
        drop(open_log_at(&dir, "t", 4).unwrap());
        assert!(dir.join("t.log.1").exists());
        assert_eq!(fs::read(dir.join("t.log")).unwrap(), Vec::<u8>::new());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn line_format_is_timestamp_component_message() {
        let text = render("t", format_args!("hello {}", 1));
        assert!(text.contains("[t] hello 1\n"));
        // "YYYY-MM-DDTHH:MM:SS.mmmZ " 共 25 个字符。
        assert_eq!(text.len(), 25 + "[t] hello 1\n".len());
    }
}
