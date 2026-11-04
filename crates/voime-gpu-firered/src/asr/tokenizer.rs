use std::path::Path;

use anyhow::{bail, Context};

pub struct AedTokenizer {
    tokens: Vec<String>,
}

impl AedTokenizer {
    pub fn load(dict: &Path, _bpe: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(dict)
            .with_context(|| format!("无法读取 AED 词表 {}", dict.display()))?;
        let mut entries: Vec<Option<String>> = Vec::new();
        for (line_number, raw) in content.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let (token, id_text) = line.rsplit_once(' ').with_context(|| {
                format!(
                    "AED 词表行格式错误 {}:{}：缺少 id",
                    dict.display(),
                    line_number + 1
                )
            })?;
            let id: usize = id_text.trim().parse().with_context(|| {
                format!(
                    "AED 词表行格式错误 {}:{}：id 非法",
                    dict.display(),
                    line_number + 1
                )
            })?;
            if entries.len() <= id {
                entries.resize(id + 1, None);
            }
            if entries[id].replace(token.to_string()).is_some() {
                bail!("AED 词表 id {} 重复：{}", id, dict.display());
            }
        }
        if entries.is_empty() {
            bail!("AED 词表为空：{}", dict.display());
        }
        Ok(Self {
            tokens: entries
                .into_iter()
                .map(|token| token.unwrap_or_default())
                .collect(),
        })
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn decode(&self, ids: &[i64]) -> anyhow::Result<String> {
        let mut text = String::new();
        for &id in ids {
            if is_special(id) {
                continue;
            }
            let index = usize::try_from(id).context("AED token id 为负数")?;
            let Some(token) = self.tokens.get(index) else {
                continue;
            };
            if token.is_empty() || is_special_text(token) {
                continue;
            }
            text.push_str(&token.replace('\u{2581}', " "));
        }
        Ok(join_pieces(&text))
    }
}

fn is_special(id: i64) -> bool {
    (0..=4).contains(&id)
}

fn is_special_text(token: &str) -> bool {
    matches!(token, "<blank>" | "<unk>" | "<pad>" | "<sos>" | "<eos>")
}

fn join_pieces(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_space = false;
    for character in text.chars() {
        if character == ' ' {
            if !previous_space {
                result.push(' ');
            }
            previous_space = true;
        } else {
            result.push(character);
            previous_space = false;
        }
    }
    result.trim().to_string()
}
