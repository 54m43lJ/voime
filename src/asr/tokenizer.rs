use std::path::Path;

use anyhow::{bail, Context};

pub struct Tokenizer {
    vocab: Vec<String>,
    sos_id: u32,
    eos_id: u32,
    pad_id: u32,
}

impl Tokenizer {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取词表 {}", path.display()))?;
        let vocab: Vec<String> = content.lines().map(|line| line.to_string()).collect();
        if vocab.is_empty() {
            bail!("词表为空：{}", path.display());
        }
        let sos_id = find_id(&vocab, &["<s>", "<sos>", "<SOS>", "[SOS]"]).unwrap_or(1);
        let eos_id = find_id(&vocab, &["</s>", "<eos>", "<EOS>", "[EOS]"]).unwrap_or(2);
        let pad_id = find_id(&vocab, &["<pad>", "<PAD>", "[PAD]"]).unwrap_or(0);
        Ok(Self {
            vocab,
            sos_id,
            eos_id,
            pad_id,
        })
    }

    pub fn sos_id(&self) -> u32 {
        self.sos_id
    }

    pub fn eos_id(&self) -> u32 {
        self.eos_id
    }

    pub fn pad_id(&self) -> u32 {
        self.pad_id
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        let mut text = String::new();
        let mut pending_bytes: Vec<u8> = Vec::new();
        for &id in ids {
            let Some(token) = self.vocab.get(id as usize) else {
                continue;
            };
            if token.is_empty() {
                continue;
            }
            if token == "<space>" {
                text.push(' ');
                continue;
            }
            if let Some(byte) = parse_byte_token(token) {
                pending_bytes.push(byte);
                continue;
            }
            if is_special(token) {
                continue;
            }
            if !pending_bytes.is_empty() {
                text.push_str(&String::from_utf8_lossy(&pending_bytes));
                pending_bytes.clear();
            }
            text.push_str(token);
        }
        if !pending_bytes.is_empty() {
            text.push_str(&String::from_utf8_lossy(&pending_bytes));
        }
        let text = text
            .replace('\u{2581}', " ")
            .replace("<space>", " ")
            .replace('\u{00A0}', " ");
        collapse_spaces(text.trim())
    }
}

fn find_id(vocab: &[String], candidates: &[&str]) -> Option<u32> {
    for candidate in candidates {
        if let Some(index) = vocab.iter().position(|token| token == candidate) {
            return Some(index as u32);
        }
    }
    None
}

fn is_special(token: &str) -> bool {
    (token.starts_with('<') && token.ends_with('>'))
        || (token.starts_with('[') && token.ends_with(']'))
}

fn parse_byte_token(token: &str) -> Option<u8> {
    let inner = token.strip_prefix("<0x")?.strip_suffix('>')?;
    u8::from_str_radix(inner, 16).ok()
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

    #[test]
    fn decodes_sentencepiece_spaces_and_skips_specials() {
        let tokenizer = Tokenizer {
            vocab: vec![
                "<pad>".to_string(),
                "<s>".to_string(),
                "</s>".to_string(),
                "\u{2581}hello".to_string(),
                "世界".to_string(),
                "<unk>".to_string(),
            ],
            sos_id: 1,
            eos_id: 2,
            pad_id: 0,
        };
        assert_eq!(tokenizer.decode(&[3, 4, 2]), "hello世界");
    }

    #[test]
    fn decodes_byte_tokens() {
        let tokenizer = Tokenizer {
            vocab: vec![
                "<0xE4>".to_string(),
                "<0xBD>".to_string(),
                "<0xA0>".to_string(),
            ],
            sos_id: 0,
            eos_id: 0,
            pad_id: 0,
        };
        assert_eq!(tokenizer.decode(&[0, 1, 2]), "你");
    }
}
