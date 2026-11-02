use std::collections::HashMap;

use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, TensorElementType, ValueType};
use tokenizers::models::wordpiece::WordPiece;
use tokenizers::normalizers::bert::BertNormalizer;
use tokenizers::pre_tokenizers::bert::BertPreTokenizer;
use tokenizers::Tokenizer as HfTokenizer;

use crate::infer::session::SessionFactory;
use crate::model::PuncManifest;

pub const MAX_TOKENS: usize = 400;

#[derive(Clone, Copy, Debug)]
pub struct PuncConfig {
    pub max_tokens: usize,
}

impl Default for PuncConfig {
    fn default() -> Self {
        Self {
            max_tokens: MAX_TOKENS,
        }
    }
}

pub struct FireRedPunc {
    session: Session,
    tokenizer: HfTokenizer,
    labels: Vec<String>,
    config: PuncConfig,
}

impl FireRedPunc {
    pub fn new_with_factory(
        manifest: &PuncManifest,
        factory: &SessionFactory,
        config: PuncConfig,
    ) -> anyhow::Result<Self> {
        let session = factory.build(&manifest.model)?;
        Self::from_session(session, &manifest.vocab, &manifest.labels, config)
    }

    fn from_session(
        session: Session,
        vocab: &std::path::Path,
        labels: &std::path::Path,
        config: PuncConfig,
    ) -> anyhow::Result<Self> {
        let tokenizer = load_wordpiece(vocab)?;
        let labels_path = labels;
        let labels: Vec<String> = serde_json::from_slice(
            &std::fs::read(labels_path)
                .with_context(|| format!("无法读取标点标签 {}", labels_path.display()))?,
        )
        .with_context(|| format!("标点标签格式错误 {}", labels_path.display()))?;
        if labels.is_empty() {
            bail!("标点标签表为空：{}", labels_path.display());
        }
        Ok(Self {
            session,
            tokenizer,
            labels,
            config: PuncConfig {
                max_tokens: config.max_tokens.max(1),
            },
        })
    }

    pub fn punctuate(&mut self, text: &str) -> anyhow::Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        let mut output = String::with_capacity(text.len() + 8);
        for chunk in split_chunks(text, self.config.max_tokens.saturating_sub(2).max(16)) {
            output.push_str(&self.punctuate_chunk(&chunk)?);
        }
        let text = fix_ascii_punctuation(output.trim());
        Ok(fix_english_text(&text))
    }

    fn punctuate_chunk(&mut self, text: &str) -> anyhow::Result<String> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|error| anyhow::anyhow!("标点分词失败：{error}"))?;
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        if ids.is_empty() {
            return Ok(text.to_string());
        }
        if ids.len() > self.config.max_tokens {
            let mut chars: Vec<char> = text.chars().collect();
            if chars.len() <= 1 {
                return Ok(text.to_string());
            }
            let middle = chars.len() / 2;
            let right: String = chars.split_off(middle).into_iter().collect();
            let left: String = chars.into_iter().collect();
            let mut output = self.punctuate_chunk(&left)?;
            output.push_str(&self.punctuate_chunk(&right)?);
            return Ok(output);
        }

        let token_count = ids.len();
        let mut inputs: Vec<(String, SessionInputValue<'_>)> =
            Vec::with_capacity(self.session.inputs().len());
        for input in self.session.inputs() {
            let name = input.name().to_string();
            let lower = name.to_ascii_lowercase();
            match input.dtype() {
                ValueType::Tensor { ty, shape, .. } if *ty == TensorElementType::Int64 => {
                    if lower.contains("length") {
                        let tensor = Tensor::from_array((
                            [1usize],
                            vec![token_count as i64].into_boxed_slice(),
                        ))?;
                        inputs.push((name, tensor.into_dyn().into()));
                    } else if lower.contains("mask") {
                        let tensor = Tensor::from_array((
                            [1usize, token_count],
                            vec![1i64; token_count].into_boxed_slice(),
                        ))?;
                        inputs.push((name, tensor.into_dyn().into()));
                    } else if lower.contains("type") {
                        let tensor = Tensor::from_array((
                            [1usize, token_count],
                            vec![0i64; token_count].into_boxed_slice(),
                        ))?;
                        inputs.push((name, tensor.into_dyn().into()));
                    } else {
                        let _ = shape;
                        let tensor = Tensor::from_array((
                            [1usize, token_count],
                            ids.clone().into_boxed_slice(),
                        ))?;
                        inputs.push((name, tensor.into_dyn().into()));
                    }
                }
                other => bail!("标点模型不支持输入 {}：{other:?}", name),
            }
        }

        let outputs = self.session.run(inputs).context("标点模型推理失败")?;
        let output = &outputs[0];
        let (shape, logits) = output
            .try_extract_tensor::<f32>()
            .context("标点模型输出类型错误")?;
        let (tokens, classes, offset) = match shape.len() {
            3 => (
                *shape.get(1).unwrap_or(&0) as usize,
                *shape.get(2).unwrap_or(&0) as usize,
                0,
            ),
            2 => (
                *shape.first().unwrap_or(&0) as usize,
                *shape.get(1).unwrap_or(&0) as usize,
                0,
            ),
            _ => bail!("标点模型输出维数错误：{:?}", &**shape),
        };
        if classes < self.labels.len() {
            bail!(
                "标点模型类别数 {} 小于标签表 {}",
                classes,
                self.labels.len()
            );
        }
        let offset = if tokens == token_count + 1 { 1 } else { offset };
        if tokens < offset + token_count {
            bail!("标点模型输出长度不足：{} < {}", tokens, token_count);
        }

        let offsets = encoding.get_offsets();
        let mut insertions: HashMap<usize, String> = HashMap::new();
        for index in 0..token_count {
            let row = offset + index;
            let start = row * classes;
            let end = start + classes;
            let best = argmax(&logits[start..end]);
            let label = self
                .labels
                .get(best)
                .map(String::as_str)
                .unwrap_or_default();
            if label.trim().is_empty() {
                continue;
            }
            if let Some(&(_, token_end)) = offsets.get(index) {
                if token_end > 0 && token_end <= text.len() {
                    insertions.entry(token_end).or_default().push_str(label);
                }
            }
        }

        let mut output = String::with_capacity(text.len() + 8);
        for (index, character) in text.char_indices() {
            if let Some(label) = insertions.remove(&index) {
                output.push_str(&label);
            }
            output.push(character);
        }
        if let Some(label) = insertions.remove(&text.len()) {
            output.push_str(&label);
        }
        for (_, label) in insertions {
            output.push_str(&label);
        }
        Ok(output)
    }
}

fn load_wordpiece(path: &std::path::Path) -> anyhow::Result<HfTokenizer> {
    let path_str = path.to_str().context("词表路径不是合法 UTF-8")?;
    let vocab = WordPiece::read_file(path_str)
        .map_err(|error| anyhow::anyhow!("无法读取 BERT 词表 {}：{error}", path.display()))?;
    let model = WordPiece::builder()
        .vocab(vocab)
        .unk_token("[UNK]".to_string())
        .continuing_subword_prefix("##".to_string())
        .max_input_chars_per_word(100)
        .build()
        .map_err(|error| anyhow::anyhow!("构建 WordPiece 分词器失败：{error}"))?;
    let mut tokenizer = HfTokenizer::new(model);
    tokenizer
        .with_normalizer(Some(BertNormalizer::default()))
        .map_err(|error| anyhow::anyhow!("设置 BERT 归一化失败：{error}"))?;
    tokenizer.with_pre_tokenizer(Some(BertPreTokenizer));
    Ok(tokenizer)
}

fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, &value) in values.iter().enumerate() {
        if value > values[best] {
            best = index;
        }
    }
    best
}

fn split_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = (start + max_chars).min(chars.len());
        if end < chars.len() {
            let mut boundary = end;
            while boundary > start + 1 {
                boundary -= 1;
                if is_sentence_boundary(chars[boundary - 1].1) {
                    end = boundary;
                    break;
                }
            }
        }
        let end_byte = if end < chars.len() {
            chars[end].0
        } else {
            text.len()
        };
        let start_byte = chars[start].0;
        if end_byte > start_byte {
            chunks.push(text[start_byte..end_byte].to_string());
        }
        start = end.max(start + 1);
    }
    chunks
}

fn is_sentence_boundary(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '!' | '?' | ';' | '；' | '，' | ','
    )
}

/// Inserts a space after ASCII sentence punctuation when it is immediately
/// followed by a Latin letter or digit, matching the official Punc text fixer.
fn fix_ascii_punctuation(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 4);
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        result.push(character);
        if matches!(character, ',' | '.' | '?' | '!') {
            if let Some(&next) = characters.peek() {
                if next.is_ascii_alphabetic() {
                    result.push(' ');
                }
            }
        }
    }
    result
}

/// The official English text fixer: lowercase, use ASCII punctuation when it
/// touches English words, and capitalize the first character.
fn fix_english_text(text: &str) -> String {
    let lowered = text.to_lowercase();
    let characters: Vec<char> = lowered.chars().collect();
    let mut result = String::with_capacity(lowered.len());
    for (index, &character) in characters.iter().enumerate() {
        let ascii_punctuation = match character {
            '，' => Some(','),
            '。' => Some('.'),
            '？' => Some('?'),
            '！' => Some('!'),
            _ => None,
        };
        if let Some(ascii) = ascii_punctuation {
            let previous = index
                .checked_sub(1)
                .and_then(|index| characters.get(index))
                .is_some_and(|character| character.is_ascii_lowercase());
            let next = characters
                .get(index + 1)
                .is_some_and(|character| character.is_ascii_lowercase());
            let starts_english = index > 0
                && characters[..index]
                    .iter()
                    .all(|character| character.is_ascii_lowercase());
            let ends_english = index + 1 == characters.len()
                && previous
                && trailing_word_has_space(&characters, index);
            if previous && next {
                result.push(ascii);
                result.push(' ');
                continue;
            }
            if starts_english || ends_english {
                result.push(ascii);
                continue;
            }
        }
        result.push(character);
    }
    if let Some(first) = result.chars().next() {
        if first.is_ascii_lowercase() {
            let mut characters = result.chars();
            let mut capitalized = String::new();
            if let Some(first) = characters.next() {
                capitalized.extend(first.to_uppercase());
                capitalized.extend(characters);
                return uppercase_pronoun_i(&capitalized);
            }
        }
    }
    uppercase_pronoun_i(&result)
}

fn uppercase_pronoun_i(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    for (index, &character) in characters.iter().enumerate() {
        if character == 'i' {
            let previous = index
                .checked_sub(1)
                .and_then(|index| characters.get(index))
                .is_none_or(|character| !character.is_alphanumeric() && *character != '\'');
            let next = characters
                .get(index + 1)
                .is_none_or(|character| !character.is_alphanumeric());
            if previous && next {
                result.push('I');
                continue;
            }
        }
        result.push(character);
    }
    result
}

fn trailing_word_has_space(characters: &[char], punctuation: usize) -> bool {
    let mut start = punctuation;
    while start > 0
        && (characters[start - 1].is_ascii_alphabetic() || characters[start - 1] == '\'')
    {
        start -= 1;
    }
    start > 0 && characters[start - 1] == ' '
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_all_characters() {
        let text = "第一句。第二句。第三句。";
        let chunks = split_chunks(text, 6);
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn argmax_returns_highest_index() {
        assert_eq!(argmax(&[0.1, 0.9, 0.2]), 1);
    }

    #[test]
    fn ascii_punctuation_gets_following_space() {
        assert_eq!(fix_ascii_punctuation("hello,world"), "hello, world");
        assert_eq!(fix_ascii_punctuation("你好，世界"), "你好，世界");
    }

    #[test]
    fn english_punctuation_is_fixed() {
        assert_eq!(fix_english_text("HELLO SPEECH。"), "Hello speech.");
        assert_eq!(fix_english_text("你好世界。"), "你好世界。");
        assert_eq!(fix_english_text("我用iPhone。很好"), "我用iphone。很好");
        assert_eq!(fix_english_text("YOU AND I"), "You and I");
    }
}
