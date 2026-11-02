use std::path::Path;

use anyhow::{bail, Context};

/// Parser for sherpa-onnx `tokens.txt`: every line is `token id`.
pub struct AsrTokenizer {
    tokens: Vec<String>,
}

impl AsrTokenizer {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取词表 {}", path.display()))?;
        let mut entries: Vec<Option<String>> = Vec::new();
        for (line_number, raw) in content.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let (token, id_text) = line.rsplit_once(' ').with_context(|| {
                format!(
                    "词表行格式错误 {}:{}：缺少 id",
                    path.display(),
                    line_number + 1
                )
            })?;
            let id: usize = id_text.trim().parse().with_context(|| {
                format!(
                    "词表行格式错误 {}:{}：id 非法 {}",
                    path.display(),
                    line_number + 1,
                    id_text
                )
            })?;
            if entries.len() <= id {
                entries.resize(id + 1, None);
            }
            if entries[id].replace(token.to_string()).is_some() {
                bail!(
                    "词表行格式错误 {}:{}：id {} 重复",
                    path.display(),
                    line_number + 1,
                    id
                );
            }
        }
        if entries.is_empty() {
            bail!("词表为空：{}", path.display());
        }
        let tokens = entries
            .into_iter()
            .map(|token| token.unwrap_or_default())
            .collect();
        Ok(Self { tokens })
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn decode(&self, ids: &[usize]) -> String {
        let mut text = String::new();
        for &id in ids {
            let Some(token) = self.tokens.get(id) else {
                continue;
            };
            if token.is_empty() || is_special(token) {
                continue;
            }
            if token == "<space>" {
                text.push(' ');
                continue;
            }
            text.push_str(&token.replace('\u{2581}', " "));
        }
        collapse_spaces(text.trim())
    }
}

fn is_special(token: &str) -> bool {
    matches!(
        token,
        "<blank>" | "<unk>" | "<sos>" | "<eos>" | "<pad>" | "<s>" | "</s>"
    ) || (token.starts_with('<') && token.ends_with('>'))
        || (token.starts_with('[') && token.ends_with(']'))
}

fn collapse_spaces(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_space = false;
    for character in text.chars() {
        if character == ' ' {
            if !previous_space {
                result.push(character);
            }
            previous_space = true;
        } else {
            result.push(character);
            previous_space = false;
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenizer() -> AsrTokenizer {
        AsrTokenizer {
            tokens: vec![
                "<blank>".to_string(),
                "<unk>".to_string(),
                "\u{2581}hello".to_string(),
                "世界".to_string(),
                "\u{2581}".to_string(),
            ],
        }
    }

    #[test]
    fn decodes_sentencepiece_spaces() {
        assert_eq!(tokenizer().decode(&[2, 3]), "hello世界");
        assert_eq!(tokenizer().decode(&[2, 4, 3]), "hello 世界");
    }

    #[test]
    fn skips_blank_and_specials() {
        assert_eq!(tokenizer().decode(&[0, 2, 1, 3, 0]), "hello世界");
    }

    #[test]
    fn loads_sherpa_token_id_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tokens.txt");
        std::fs::write(&path, "<blank> 0\nhello 2\nworld 1\n").unwrap();
        let tokenizer = AsrTokenizer::load(&path).unwrap();
        assert_eq!(tokenizer.token_count(), 3);
        assert_eq!(tokenizer.decode(&[2, 1]), "helloworld");
    }

    #[test]
    fn rejects_duplicate_ids() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tokens.txt");
        std::fs::write(&path, "a 1\nb 1\n").unwrap();
        assert!(AsrTokenizer::load(&path).is_err());
    }
}
