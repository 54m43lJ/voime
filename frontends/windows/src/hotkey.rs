use std::fmt;

use anyhow::{bail, Context};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
    MOD_SHIFT, MOD_WIN, VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1,
    VK_HOME, VK_INSERT, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};

use crate::Frontend;

pub const HOTKEY_ID: i32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

impl Hotkey {
    pub fn is_empty(self) -> bool {
        self.key == 0
    }

    pub fn is_down(self) -> bool {
        if self.is_empty() {
            return false;
        }
        unsafe { GetAsyncKeyState(self.key as i32) < 0 }
    }
}

impl Default for Hotkey {
    fn default() -> Self {
        parse("Ctrl+Alt+Space").expect("默认热键必须有效")
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }
        let mut first = true;
        for (flag, name) in [
            (MOD_CONTROL.0, "Ctrl"),
            (MOD_ALT.0, "Alt"),
            (MOD_SHIFT.0, "Shift"),
            (MOD_WIN.0, "Win"),
        ] {
            if self.modifiers & flag != 0 {
                if !first {
                    formatter.write_str("+")?;
                }
                formatter.write_str(name)?;
                first = false;
            }
        }
        if !first {
            formatter.write_str("+")?;
        }
        formatter.write_str(&key_name(self.key))
    }
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        parse(&value).map_err(D::Error::custom)
    }
}

pub struct HotkeyHandle {
    hwnd: HWND,
    id: i32,
}

pub fn parse(spec: &str) -> anyhow::Result<Hotkey> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(Hotkey {
            modifiers: 0,
            key: 0,
        });
    }
    let mut modifiers = 0u32;
    let mut key = None;
    for part in spec
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL.0,
            "alt" | "option" => modifiers |= MOD_ALT.0,
            "shift" => modifiers |= MOD_SHIFT.0,
            "win" | "super" | "meta" => modifiers |= MOD_WIN.0,
            _ => {
                if key.is_some() {
                    bail!("热键包含多个主键：{spec}");
                }
                key = Some(parse_key(part).with_context(|| format!("无法解析热键 {spec}"))?);
            }
        }
    }
    let key = key.with_context(|| format!("热键缺少主键：{spec}"))?;
    if modifiers == 0 {
        bail!("热键至少需要一个修饰键：{spec}");
    }
    Ok(Hotkey { modifiers, key })
}

fn parse_key(part: &str) -> anyhow::Result<u32> {
    let upper = part.to_ascii_uppercase();
    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            return Ok(byte as u32);
        }
    }
    if let Some(number) = upper.strip_prefix('F') {
        if let Ok(number) = number.parse::<u16>() {
            if (1..=24).contains(&number) {
                return Ok(VK_F1.0 as u32 + u32::from(number - 1));
            }
        }
    }
    let key = match upper.as_str() {
        "SPACE" => VK_SPACE,
        "TAB" => VK_TAB,
        "ENTER" | "RETURN" => VK_RETURN,
        "ESC" | "ESCAPE" => VK_ESCAPE,
        "BACKSPACE" => VK_BACK,
        "DELETE" | "DEL" => VK_DELETE,
        "INSERT" | "INS" => VK_INSERT,
        "HOME" => VK_HOME,
        "END" => VK_END,
        "PAGEUP" | "PGUP" => VK_PRIOR,
        "PAGEDOWN" | "PGDN" => VK_NEXT,
        "UP" => VK_UP,
        "DOWN" => VK_DOWN,
        "LEFT" => VK_LEFT,
        "RIGHT" => VK_RIGHT,
        other => bail!("未知按键 {other}"),
    };
    Ok(key.0 as u32)
}

fn key_name(key: u32) -> String {
    if (b'A' as u32..=b'Z' as u32).contains(&key) || (b'0' as u32..=b'9' as u32).contains(&key) {
        return char::from_u32(key).unwrap_or('?').to_string();
    }
    let name = match VIRTUAL_KEY(key as u16) {
        VK_SPACE => "Space",
        VK_TAB => "Tab",
        VK_RETURN => "Enter",
        VK_ESCAPE => "Escape",
        VK_BACK => "Backspace",
        VK_DELETE => "Delete",
        VK_INSERT => "Insert",
        VK_HOME => "Home",
        VK_END => "End",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_UP => "Up",
        VK_DOWN => "Down",
        VK_LEFT => "Left",
        VK_RIGHT => "Right",
        _ if (VK_F1.0 as u32..=VK_F1.0 as u32 + 23).contains(&key) => {
            return format!("F{}", key - VK_F1.0 as u32 + 1);
        }
        _ => return format!("0x{key:02X}"),
    };
    name.to_string()
}

pub fn register(hotkey: &Hotkey, frontend: &mut Frontend) -> anyhow::Result<HotkeyHandle> {
    let hwnd = frontend.hwnd().context("托盘窗口尚未创建，无法注册热键")?;
    if hotkey.is_empty() {
        return Ok(HotkeyHandle { hwnd, id: 0 });
    }
    unsafe {
        RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            HOT_KEY_MODIFIERS(hotkey.modifiers),
            hotkey.key,
        )
        .with_context(|| format!("注册热键 {hotkey} 失败"))?;
    }
    Ok(HotkeyHandle {
        hwnd,
        id: HOTKEY_ID,
    })
}

pub fn unregister(handle: HotkeyHandle) {
    if handle.id != 0 {
        unsafe {
            let _ = UnregisterHotKey(handle.hwnd, handle.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_hotkey_round_trip() {
        let hotkey = parse("Ctrl+Alt+Space").unwrap();
        assert_eq!(hotkey.to_string(), "Ctrl+Alt+Space");
        assert_eq!(parse(&hotkey.to_string()).unwrap(), hotkey);
    }

    #[test]
    fn parses_empty_as_disabled() {
        let hotkey = parse("").unwrap();
        assert!(hotkey.is_empty());
        assert_eq!(hotkey.to_string(), "");
    }

    #[test]
    fn rejects_hotkey_without_modifier() {
        assert!(parse("Space").is_err());
    }

    #[test]
    fn parses_function_keys() {
        assert_eq!(parse("Shift+F12").unwrap().key, VK_F1.0 as u32 + 11);
    }
}
