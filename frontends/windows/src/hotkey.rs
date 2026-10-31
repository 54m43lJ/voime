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

const MODIFIER_NAMES: [(u32, &str); 4] = [
    (MOD_CONTROL.0, "Ctrl"),
    (MOD_ALT.0, "Alt"),
    (MOD_SHIFT.0, "Shift"),
    (MOD_WIN.0, "Win"),
];

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
        for (flag, name) in MODIFIER_NAMES {
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
    MODIFIER_NAMES
        .iter()
        .filter(|(flag, _)| modifiers & flag != 0)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join("+")
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

/// Single source of truth for named keys, one row per virtual-key code:
/// `(code, display name, parse aliases)`. The first row under a display
/// name is canonical and supplies that name's code when parsing; extra
/// rows only add codes that display under the same name (left/right
/// modifier variants). Parsing compares case-insensitively, so `"PGUP"`
/// resolves through `PageUp`.
const NAMED_KEYS: &[(u32, &str, &[&str])] = &[
    (VK_SPACE.0 as u32, "Space", &[]),
    (VK_TAB.0 as u32, "Tab", &[]),
    (VK_RETURN.0 as u32, "Enter", &["Return", "NumpadEnter"]),
    (VK_ESCAPE.0 as u32, "Escape", &["Esc"]),
    (VK_BACK.0 as u32, "Backspace", &["Back"]),
    (VK_DELETE.0 as u32, "Delete", &["Del"]),
    (VK_INSERT.0 as u32, "Insert", &["Ins"]),
    (VK_HOME.0 as u32, "Home", &[]),
    (VK_END.0 as u32, "End", &[]),
    (VK_PRIOR.0 as u32, "PageUp", &["PgUp"]),
    (VK_NEXT.0 as u32, "PageDown", &["PgDn"]),
    (VK_UP.0 as u32, "Up", &[]),
    (VK_DOWN.0 as u32, "Down", &[]),
    (VK_LEFT.0 as u32, "Left", &[]),
    (VK_RIGHT.0 as u32, "Right", &[]),
    (VK_SNAPSHOT.0 as u32, "PrintScreen", &["PrtSc", "Snapshot"]),
    (VK_PAUSE.0 as u32, "Pause", &[]),
    (VK_CAPITAL.0 as u32, "CapsLock", &[]),
    (VK_NUMLOCK.0 as u32, "NumLock", &[]),
    (VK_SCROLL.0 as u32, "ScrollLock", &[]),
    (VK_APPS.0 as u32, "Apps", &["Menu"]),
    (VK_SLEEP.0 as u32, "Sleep", &[]),
    (VK_CLEAR.0 as u32, "Clear", &[]),
    (VK_SELECT.0 as u32, "Select", &[]),
    (VK_PRINT.0 as u32, "Print", &[]),
    (VK_EXECUTE.0 as u32, "Execute", &[]),
    (VK_HELP.0 as u32, "Help", &[]),
    (VK_SEPARATOR.0 as u32, "Separator", &[]),
    (VK_VOLUME_MUTE.0 as u32, "VolumeMute", &["Mute"]),
    (VK_VOLUME_DOWN.0 as u32, "VolumeDown", &[]),
    (VK_VOLUME_UP.0 as u32, "VolumeUp", &[]),
    (
        VK_MEDIA_NEXT_TRACK.0 as u32,
        "MediaNext",
        &["MediaNextTrack"],
    ),
    (
        VK_MEDIA_PREV_TRACK.0 as u32,
        "MediaPrev",
        &["MediaPrevTrack"],
    ),
    (VK_MEDIA_STOP.0 as u32, "MediaStop", &[]),
    (VK_MEDIA_PLAY_PAUSE.0 as u32, "MediaPlayPause", &[]),
    (VK_LAUNCH_MAIL.0 as u32, "LaunchMail", &[]),
    (VK_LAUNCH_MEDIA_SELECT.0 as u32, "LaunchMedia", &[]),
    (VK_LAUNCH_APP1.0 as u32, "LaunchApp1", &[]),
    (VK_LAUNCH_APP2.0 as u32, "LaunchApp2", &[]),
    (VK_BROWSER_BACK.0 as u32, "BrowserBack", &[]),
    (VK_BROWSER_FORWARD.0 as u32, "BrowserForward", &[]),
    (VK_BROWSER_REFRESH.0 as u32, "BrowserRefresh", &[]),
    (VK_BROWSER_STOP.0 as u32, "BrowserStop", &[]),
    (VK_BROWSER_SEARCH.0 as u32, "BrowserSearch", &[]),
    (VK_BROWSER_FAVORITES.0 as u32, "BrowserFavorites", &[]),
    (VK_BROWSER_HOME.0 as u32, "BrowserHome", &[]),
    (VK_KANA.0 as u32, "Kana", &[]),
    (VK_HANJA.0 as u32, "Hanja", &[]),
    (VK_IME_ON.0 as u32, "ImeOn", &[]),
    (VK_IME_OFF.0 as u32, "ImeOff", &[]),
    (VK_CONVERT.0 as u32, "Convert", &[]),
    (VK_NONCONVERT.0 as u32, "NonConvert", &[]),
    (VK_ACCEPT.0 as u32, "Accept", &[]),
    (VK_MODECHANGE.0 as u32, "ModeChange", &[]),
    (VK_PROCESSKEY.0 as u32, "ProcessKey", &[]),
    (VK_OEM_PLUS.0 as u32, "Equal", &["Plus"]),
    (VK_OEM_MINUS.0 as u32, "Minus", &[]),
    (VK_OEM_COMMA.0 as u32, "Comma", &[]),
    (VK_OEM_PERIOD.0 as u32, "Period", &["Dot"]),
    (VK_OEM_2.0 as u32, "Slash", &[]),
    (VK_OEM_1.0 as u32, "Semicolon", &[]),
    (VK_OEM_7.0 as u32, "Quote", &["Apostrophe"]),
    (VK_OEM_3.0 as u32, "Backquote", &["Grave", "Tilde"]),
    (VK_OEM_4.0 as u32, "LeftBracket", &["BracketLeft"]),
    (VK_OEM_5.0 as u32, "Backslash", &[]),
    (VK_OEM_6.0 as u32, "RightBracket", &["BracketRight"]),
    (VK_OEM_102.0 as u32, "Oem102", &[]),
    (VK_NUMPAD0.0 as u32, "Numpad0", &[]),
    (VK_NUMPAD0.0 as u32 + 1, "Numpad1", &[]),
    (VK_NUMPAD0.0 as u32 + 2, "Numpad2", &[]),
    (VK_NUMPAD0.0 as u32 + 3, "Numpad3", &[]),
    (VK_NUMPAD0.0 as u32 + 4, "Numpad4", &[]),
    (VK_NUMPAD0.0 as u32 + 5, "Numpad5", &[]),
    (VK_NUMPAD0.0 as u32 + 6, "Numpad6", &[]),
    (VK_NUMPAD0.0 as u32 + 7, "Numpad7", &[]),
    (VK_NUMPAD0.0 as u32 + 8, "Numpad8", &[]),
    (VK_NUMPAD0.0 as u32 + 9, "Numpad9", &[]),
    (VK_MULTIPLY.0 as u32, "NumpadMultiply", &["Multiply"]),
    (VK_ADD.0 as u32, "NumpadAdd", &["Add"]),
    (VK_SUBTRACT.0 as u32, "NumpadSubtract", &["Subtract"]),
    (VK_DIVIDE.0 as u32, "NumpadDivide", &["Divide"]),
    (VK_DECIMAL.0 as u32, "NumpadDecimal", &["Decimal"]),
    // 左右修饰键变体共用显示名，解析按名字取首行的规范码。
    (VK_SHIFT.0 as u32, "Shift", &[]),
    (VK_LSHIFT.0 as u32, "Shift", &[]),
    (VK_RSHIFT.0 as u32, "Shift", &[]),
    (VK_CONTROL.0 as u32, "Ctrl", &[]),
    (VK_LCONTROL.0 as u32, "Ctrl", &[]),
    (VK_RCONTROL.0 as u32, "Ctrl", &[]),
    (VK_MENU.0 as u32, "Alt", &[]),
    (VK_LMENU.0 as u32, "Alt", &[]),
    (VK_RMENU.0 as u32, "Alt", &[]),
    (VK_LWIN.0 as u32, "Win", &[]),
    (VK_RWIN.0 as u32, "Win", &[]),
];

fn named(name: &str) -> Option<VIRTUAL_KEY> {
    NAMED_KEYS
        .iter()
        .find(|(_, canonical, aliases)| {
            canonical.eq_ignore_ascii_case(name)
                || aliases.iter().any(|alias| alias.eq_ignore_ascii_case(name))
        })
        .map(|(code, _, _)| VIRTUAL_KEY(*code as u16))
}

pub fn key_name(key: u32) -> String {
    if (b'A' as u32..=b'Z' as u32).contains(&key) || (b'0' as u32..=b'9' as u32).contains(&key) {
        return char::from_u32(key).unwrap_or('?').to_string();
    }
    if (VK_F1.0 as u32..=VK_F1.0 as u32 + 23).contains(&key) {
        return format!("F{}", key - VK_F1.0 as u32 + 1);
    }
    let Some((_, name, _)) = NAMED_KEYS.iter().find(|(code, _, _)| *code == key) else {
        return format!("VK_0x{key:02X}");
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

    #[test]
    fn named_keys_have_unique_codes() {
        for (index, (code, name, _)) in NAMED_KEYS.iter().enumerate() {
            for (other, other_name, _) in &NAMED_KEYS[..index] {
                assert_ne!(code, other, "{name} 与 {other_name} 共用键码 0x{code:02X}");
            }
        }
    }

    #[test]
    fn named_keys_round_trip_through_display_and_parse() {
        for (code, name, _) in NAMED_KEYS {
            assert_eq!(key_name(*code), *name, "0x{code:02X}");
        }
        // 修饰键名在 parse() 里会被截为修饰符，不能作为主键出现。
        for (code, name, _) in NAMED_KEYS {
            if matches!(*name, "Shift" | "Ctrl" | "Alt" | "Win") {
                continue;
            }
            let spec = format!("Ctrl+{name}");
            let hotkey = parse(&spec).unwrap();
            assert_eq!(hotkey.key, *code, "{spec}");
            assert_eq!(hotkey.to_string(), spec);
        }
    }
}
