use std::fmt;

use anyhow::{bail, Context};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VIRTUAL_KEY, VK_ACCEPT, VK_ADD, VK_APPS, VK_BACK,
    VK_BROWSER_BACK, VK_BROWSER_FAVORITES, VK_BROWSER_FORWARD, VK_BROWSER_HOME, VK_BROWSER_REFRESH,
    VK_BROWSER_SEARCH, VK_BROWSER_STOP, VK_CAPITAL, VK_CLEAR, VK_CONTROL, VK_CONVERT, VK_DECIMAL,
    VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_EXECUTE, VK_F1, VK_HANJA, VK_HELP,
    VK_HOME, VK_IME_OFF, VK_IME_ON, VK_INSERT, VK_KANA, VK_LAUNCH_APP1, VK_LAUNCH_APP2,
    VK_LAUNCH_MAIL, VK_LAUNCH_MEDIA_SELECT, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_MENU,
    VK_MODECHANGE, VK_MULTIPLY, VK_NEXT, VK_NONCONVERT, VK_NUMLOCK, VK_NUMPAD0, VK_OEM_1,
    VK_OEM_102, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PAUSE, VK_PRINT, VK_PRIOR, VK_PROCESSKEY,
    VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SCROLL, VK_SELECT,
    VK_SEPARATOR, VK_SHIFT, VK_SLEEP, VK_SNAPSHOT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP,
    VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HMENU, HWND_MESSAGE, WS_POPUP,
};

use crate::Frontend;

pub const HOTKEY_ID: i32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

impl Hotkey {
    pub const fn new(modifiers: u32, key: u32) -> Self {
        Self { modifiers, key }
    }

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

pub fn modifier_names(modifiers: u32) -> String {
    let mut names = Vec::new();
    for (flag, name) in [
        (MOD_CONTROL.0, "Ctrl"),
        (MOD_ALT.0, "Alt"),
        (MOD_SHIFT.0, "Shift"),
        (MOD_WIN.0, "Win"),
    ] {
        if modifiers & flag != 0 {
            names.push(name);
        }
    }
    names.join("+")
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
        return Ok(Hotkey::new(0, 0));
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
    let Some(key) = key else {
        bail!("热键缺少主键：{spec}");
    };
    if modifiers == 0 {
        bail!("热键至少需要一个修饰键：{spec}");
    }
    Ok(Hotkey::new(modifiers, key))
}

fn parse_key(part: &str) -> anyhow::Result<u32> {
    let part = part.trim();
    let upper = part.to_ascii_uppercase();
    if upper.len() == 1 {
        let character = upper.chars().next().unwrap_or('?');
        if character.is_ascii_alphanumeric() {
            return Ok(character as u32);
        }
        if let Some(key) = punctuation(&character) {
            return Ok(key.0 as u32);
        }
    }
    if let Some(number) = upper.strip_prefix('F') {
        if let Ok(number) = number.parse::<u16>() {
            if (1..=24).contains(&number) {
                return Ok(VK_F1.0 as u32 + u32::from(number - 1));
            }
        }
    }
    let hex = upper
        .strip_prefix("VK_")
        .or_else(|| upper.strip_prefix("0X"));
    if let Some(hex) = hex {
        let hex = hex.strip_prefix("0X").unwrap_or(hex);
        if let Ok(code) = u32::from_str_radix(hex, 16) {
            if (1..=0xFF).contains(&code) {
                return Ok(code);
            }
        }
    }
    if let Some(key) = named(&upper) {
        return Ok(key.0 as u32);
    }
    bail!("未知按键 {part}")
}

fn punctuation(character: &char) -> Option<VIRTUAL_KEY> {
    Some(match character {
        ';' => VK_OEM_1,
        '=' => VK_OEM_PLUS,
        ',' => VK_OEM_COMMA,
        '-' => VK_OEM_MINUS,
        '.' => VK_OEM_PERIOD,
        '/' => VK_OEM_2,
        '`' => VK_OEM_3,
        '[' => VK_OEM_4,
        '\\' => VK_OEM_5,
        ']' => VK_OEM_6,
        '\'' => VK_OEM_7,
        _ => return None,
    })
}

fn named(name: &str) -> Option<VIRTUAL_KEY> {
    let key = match name {
        "SPACE" => VK_SPACE,
        "TAB" => VK_TAB,
        "ENTER" | "RETURN" => VK_RETURN,
        "ESC" | "ESCAPE" => VK_ESCAPE,
        "BACKSPACE" | "BACK" => VK_BACK,
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
        "PRINTSCREEN" | "PRTSC" | "SNAPSHOT" => VK_SNAPSHOT,
        "PAUSE" => VK_PAUSE,
        "CAPSLOCK" => VK_CAPITAL,
        "NUMLOCK" => VK_NUMLOCK,
        "SCROLLLOCK" => VK_SCROLL,
        "APPS" | "MENU" => VK_APPS,
        "SLEEP" => VK_SLEEP,
        "CLEAR" => VK_CLEAR,
        "SELECT" => VK_SELECT,
        "PRINT" => VK_PRINT,
        "EXECUTE" => VK_EXECUTE,
        "HELP" => VK_HELP,
        "SEPARATOR" => VK_SEPARATOR,
        "MUTE" | "VOLUMEMUTE" => VK_VOLUME_MUTE,
        "VOLUMEDOWN" => VK_VOLUME_DOWN,
        "VOLUMEUP" => VK_VOLUME_UP,
        "MEDIANEXT" | "MEDIANEXTTRACK" => VK_MEDIA_NEXT_TRACK,
        "MEDIAPREV" | "MEDIAPREVTRACK" => VK_MEDIA_PREV_TRACK,
        "MEDIASTOP" => VK_MEDIA_STOP,
        "MEDIAPLAYPAUSE" => VK_MEDIA_PLAY_PAUSE,
        "LAUNCHMAIL" => VK_LAUNCH_MAIL,
        "LAUNCHMEDIA" => VK_LAUNCH_MEDIA_SELECT,
        "LAUNCHAPP1" => VK_LAUNCH_APP1,
        "LAUNCHAPP2" => VK_LAUNCH_APP2,
        "BROWSERBACK" => VK_BROWSER_BACK,
        "BROWSERFORWARD" => VK_BROWSER_FORWARD,
        "BROWSERREFRESH" => VK_BROWSER_REFRESH,
        "BROWSERSTOP" => VK_BROWSER_STOP,
        "BROWSERSEARCH" => VK_BROWSER_SEARCH,
        "BROWSERFAVORITES" => VK_BROWSER_FAVORITES,
        "BROWSERHOME" => VK_BROWSER_HOME,
        "KANA" => VK_KANA,
        "HANJA" => VK_HANJA,
        "IMEON" => VK_IME_ON,
        "IMEOFF" => VK_IME_OFF,
        "CONVERT" => VK_CONVERT,
        "NONCONVERT" => VK_NONCONVERT,
        "ACCEPT" => VK_ACCEPT,
        "MODECHANGE" => VK_MODECHANGE,
        "PROCESSKEY" => VK_PROCESSKEY,
        "PLUS" | "EQUAL" => VK_OEM_PLUS,
        "MINUS" => VK_OEM_MINUS,
        "COMMA" => VK_OEM_COMMA,
        "PERIOD" | "DOT" => VK_OEM_PERIOD,
        "SLASH" => VK_OEM_2,
        "SEMICOLON" => VK_OEM_1,
        "QUOTE" | "APOSTROPHE" => VK_OEM_7,
        "BACKQUOTE" | "GRAVE" | "TILDE" => VK_OEM_3,
        "LEFTBRACKET" | "BRACKETLEFT" => VK_OEM_4,
        "BACKSLASH" => VK_OEM_5,
        "RIGHTBRACKET" | "BRACKETRIGHT" => VK_OEM_6,
        "OEM102" => VK_OEM_102,
        "NUMPAD0" => VK_NUMPAD0,
        "NUMPAD1" => VIRTUAL_KEY(VK_NUMPAD0.0 + 1),
        "NUMPAD2" => VIRTUAL_KEY(VK_NUMPAD0.0 + 2),
        "NUMPAD3" => VIRTUAL_KEY(VK_NUMPAD0.0 + 3),
        "NUMPAD4" => VIRTUAL_KEY(VK_NUMPAD0.0 + 4),
        "NUMPAD5" => VIRTUAL_KEY(VK_NUMPAD0.0 + 5),
        "NUMPAD6" => VIRTUAL_KEY(VK_NUMPAD0.0 + 6),
        "NUMPAD7" => VIRTUAL_KEY(VK_NUMPAD0.0 + 7),
        "NUMPAD8" => VIRTUAL_KEY(VK_NUMPAD0.0 + 8),
        "NUMPAD9" => VIRTUAL_KEY(VK_NUMPAD0.0 + 9),
        "NUMPADMULTIPLY" | "MULTIPLY" => VK_MULTIPLY,
        "NUMPADADD" | "ADD" => VK_ADD,
        "NUMPADSUBTRACT" | "SUBTRACT" => VK_SUBTRACT,
        "NUMPADDIVIDE" | "DIVIDE" => VK_DIVIDE,
        "NUMPADDECIMAL" | "DECIMAL" => VK_DECIMAL,
        "NUMPADENTER" => VK_RETURN,
        _ => return None,
    };
    Some(key)
}

pub fn key_name(key: u32) -> String {
    if (b'A' as u32..=b'Z' as u32).contains(&key) || (b'0' as u32..=b'9' as u32).contains(&key) {
        return char::from_u32(key).unwrap_or('?').to_string();
    }
    if (VK_F1.0 as u32..=VK_F1.0 as u32 + 23).contains(&key) {
        return format!("F{}", key - VK_F1.0 as u32 + 1);
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
        VK_SNAPSHOT => "PrintScreen",
        VK_PAUSE => "Pause",
        VK_CAPITAL => "CapsLock",
        VK_NUMLOCK => "NumLock",
        VK_SCROLL => "ScrollLock",
        VK_APPS => "Apps",
        VK_SLEEP => "Sleep",
        VK_CLEAR => "Clear",
        VK_SELECT => "Select",
        VK_PRINT => "Print",
        VK_EXECUTE => "Execute",
        VK_HELP => "Help",
        VK_SEPARATOR => "Separator",
        VK_VOLUME_MUTE => "VolumeMute",
        VK_VOLUME_DOWN => "VolumeDown",
        VK_VOLUME_UP => "VolumeUp",
        VK_MEDIA_NEXT_TRACK => "MediaNext",
        VK_MEDIA_PREV_TRACK => "MediaPrev",
        VK_MEDIA_STOP => "MediaStop",
        VK_MEDIA_PLAY_PAUSE => "MediaPlayPause",
        VK_LAUNCH_MAIL => "LaunchMail",
        VK_LAUNCH_MEDIA_SELECT => "LaunchMedia",
        VK_LAUNCH_APP1 => "LaunchApp1",
        VK_LAUNCH_APP2 => "LaunchApp2",
        VK_BROWSER_BACK => "BrowserBack",
        VK_BROWSER_FORWARD => "BrowserForward",
        VK_BROWSER_REFRESH => "BrowserRefresh",
        VK_BROWSER_STOP => "BrowserStop",
        VK_BROWSER_SEARCH => "BrowserSearch",
        VK_BROWSER_FAVORITES => "BrowserFavorites",
        VK_BROWSER_HOME => "BrowserHome",
        VK_KANA => "Kana",
        VK_HANJA => "Hanja",
        VK_IME_ON => "ImeOn",
        VK_IME_OFF => "ImeOff",
        VK_CONVERT => "Convert",
        VK_NONCONVERT => "NonConvert",
        VK_ACCEPT => "Accept",
        VK_MODECHANGE => "ModeChange",
        VK_PROCESSKEY => "ProcessKey",
        VK_OEM_PLUS => "Equal",
        VK_OEM_MINUS => "Minus",
        VK_OEM_COMMA => "Comma",
        VK_OEM_PERIOD => "Period",
        VK_OEM_2 => "Slash",
        VK_OEM_1 => "Semicolon",
        VK_OEM_7 => "Quote",
        VK_OEM_3 => "Backquote",
        VK_OEM_4 => "LeftBracket",
        VK_OEM_5 => "Backslash",
        VK_OEM_6 => "RightBracket",
        VK_OEM_102 => "Oem102",
        VK_NUMPAD0 => "Numpad0",
        VK_ADD => "NumpadAdd",
        VK_SUBTRACT => "NumpadSubtract",
        VK_MULTIPLY => "NumpadMultiply",
        VK_DIVIDE => "NumpadDivide",
        VK_DECIMAL => "NumpadDecimal",
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT => "Shift",
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL => "Ctrl",
        VK_LMENU | VK_RMENU | VK_MENU => "Alt",
        VK_LWIN | VK_RWIN => "Win",
        _ if (VK_NUMPAD0.0 as u32 + 1..=VK_NUMPAD0.0 as u32 + 9).contains(&key) => {
            return format!("Numpad{}", key - VK_NUMPAD0.0 as u32);
        }
        _ => return format!("VK_0x{key:02X}"),
    };
    name.to_string()
}

pub fn modifier_state() -> u32 {
    let mut modifiers = 0;
    if down(&[VK_CONTROL, VK_LCONTROL, VK_RCONTROL]) {
        modifiers |= MOD_CONTROL.0;
    }
    if down(&[VK_MENU, VK_LMENU, VK_RMENU]) {
        modifiers |= MOD_ALT.0;
    }
    if down(&[VK_SHIFT, VK_LSHIFT, VK_RSHIFT]) {
        modifiers |= MOD_SHIFT.0;
    }
    if win_down() {
        modifiers |= MOD_WIN.0;
    }
    modifiers
}

pub fn win_down() -> bool {
    down(&[VK_LWIN, VK_RWIN])
}

fn down(keys: &[VIRTUAL_KEY]) -> bool {
    keys.iter()
        .any(|key| unsafe { GetAsyncKeyState(key.0 as i32) < 0 })
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
            HOT_KEY_MODIFIERS(hotkey.modifiers | MOD_NOREPEAT.0),
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

pub fn check_available(hotkey: &Hotkey) -> anyhow::Result<()> {
    if hotkey.is_empty() {
        return Ok(());
    }
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("STATIC"),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            HMENU::default(),
            HINSTANCE::default(),
            None,
        )
    }
    .context("创建热键校验窗口失败")?;
    let result = unsafe {
        RegisterHotKey(
            hwnd,
            HOTKEY_ID,
            HOT_KEY_MODIFIERS(hotkey.modifiers | MOD_NOREPEAT.0),
            hotkey.key,
        )
    }
    .with_context(|| format!("注册热键 {hotkey} 失败"));
    unsafe {
        let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
        let _ = DestroyWindow(hwnd);
    }
    result
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

    #[test]
    fn parses_extended_keys_round_trip() {
        for spec in [
            "Ctrl+NumpadAdd",
            "Ctrl+Shift+PrintScreen",
            "Alt+MediaPlayPause",
            "Win+VolumeUp",
            "Ctrl+Shift+LeftBracket",
            "Ctrl+Minus",
            "Alt+F24",
        ] {
            let hotkey = parse(spec).unwrap();
            assert_eq!(hotkey.to_string(), spec);
            assert_eq!(parse(&hotkey.to_string()).unwrap(), hotkey);
        }
    }

    #[test]
    fn parses_hex_fallback_round_trip() {
        let hotkey = parse("Ctrl+VK_0xE2").unwrap();
        assert_eq!(hotkey.to_string(), "Ctrl+Oem102");
    }

    #[test]
    fn captured_virtual_keys_round_trip_through_display() {
        for key in [
            VK_OEM_1.0 as u32,
            VK_OEM_PLUS.0 as u32,
            VK_OEM_MINUS.0 as u32,
            VK_NUMPAD0.0 as u32,
            VK_ADD.0 as u32,
            VK_MEDIA_PLAY_PAUSE.0 as u32,
            VK_OEM_102.0 as u32,
            0xE5,
        ] {
            let spec = format!("Ctrl+{}", key_name(key));
            assert_eq!(parse(&spec).unwrap().key, key, "{spec}");
        }
    }
}
