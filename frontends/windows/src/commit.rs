use anyhow::{bail, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY,
};

use crate::target::{self, Target};

const CHUNK_UNITS: usize = 64;

pub fn commit(target: &Target, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if !target::focused(target) {
        bail!("目标已失效，放弃提交");
    }
    let units = to_utf16(text);
    for chunk in chunks(&units) {
        send_unicode(chunk)?;
    }
    Ok(())
}

fn to_utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn chunks(units: &[u16]) -> impl Iterator<Item = &[u16]> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= units.len() {
            return None;
        }
        let mut end = (start + CHUNK_UNITS).min(units.len());
        if end < units.len() && (0xD800..=0xDBFF).contains(&units[end - 1]) {
            end += 1;
        }
        let chunk = &units[start..end];
        start = end;
        Some(chunk)
    })
}

fn send_unicode(chunk: &[u16]) -> Result<()> {
    let mut inputs = Vec::with_capacity(chunk.len() * 2);
    for unit in chunk {
        inputs.push(keyboard_input(*unit, false));
        inputs.push(keyboard_input(*unit, true));
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        bail!("SendInput 只发送了 {sent}/{} 个事件", inputs.len());
    }
    Ok(())
}

fn keyboard_input(unit: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: KEYBD_EVENT_FLAGS(flags.0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip_includes_surrogate_pairs() {
        let units = to_utf16("语音🎤test");
        assert!(units.len() > 4);
        assert!(units
            .windows(2)
            .any(|pair| (0xD800..=0xDBFF).contains(&pair[0])));
    }

    #[test]
    fn chunks_never_split_surrogate_pairs() {
        let text = "a".repeat(CHUNK_UNITS - 1) + "🎤" + &"b".repeat(CHUNK_UNITS);
        let units = to_utf16(&text);
        for chunk in chunks(&units) {
            assert!(!chunk.is_empty());
            assert!(!(0xD800..=0xDBFF).contains(chunk.last().unwrap()));
        }
        assert_eq!(chunks(&units).flatten().count(), units.len());
    }
}
