use anyhow::Context;

pub struct ClipboardWriter {
    clipboard: arboard::Clipboard,
}

impl ClipboardWriter {
    pub fn new() -> anyhow::Result<Self> {
        let clipboard = arboard::Clipboard::new().context("建立剪贴板句柄失败")?;
        Ok(Self { clipboard })
    }

    pub fn set_transcript(&mut self, lines: &[String]) -> anyhow::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let text = join_transcript(lines);
        self.clipboard.set_text(text).context("写入剪贴板失败")?;
        Ok(())
    }
}

pub fn join_transcript(lines: &[String]) -> String {
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_without_trailing_newline() {
        let lines = vec!["第一行".to_string(), "second".to_string()];
        assert_eq!(join_transcript(&lines), "第一行\nsecond");
    }

    #[test]
    fn empty_transcript_is_empty() {
        assert_eq!(join_transcript(&[]), "");
    }
}
