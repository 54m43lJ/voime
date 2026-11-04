use std::path::Path;

use anyhow::Context;
use tokenizers::models::wordpiece::WordPiece;
use tokenizers::normalizers::bert::BertNormalizer;
use tokenizers::pre_tokenizers::bert::BertPreTokenizer;
use tokenizers::Tokenizer;

#[derive(Clone, Debug)]
pub struct Piece {
    pub text: String,
    pub start: usize,
    pub end: usize,
    pub id: i64,
}

pub struct PuncTokenizer {
    tokenizer: Tokenizer,
}

impl PuncTokenizer {
    pub fn load(tokens: &Path) -> anyhow::Result<Self> {
        let path = tokens.to_str().context("标点词表路径不是 UTF-8")?;
        let vocab = WordPiece::read_file(path)
            .map_err(|error| anyhow::anyhow!("无法读取标点词表 {}：{error}", tokens.display()))?;
        let model = WordPiece::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .continuing_subword_prefix("##".to_string())
            .max_input_chars_per_word(100)
            .build()
            .map_err(|error| anyhow::anyhow!("构建 WordPiece 失败：{error}"))?;
        let mut tokenizer = Tokenizer::new(model);
        tokenizer
            .with_normalizer(Some(BertNormalizer::default()))
            .map_err(|error| anyhow::anyhow!("设置 BERT 归一化失败：{error}"))?;
        tokenizer.with_pre_tokenizer(Some(BertPreTokenizer));
        Ok(Self { tokenizer })
    }

    pub fn encode(&self, text: &str) -> anyhow::Result<(Vec<i64>, Vec<Piece>)> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|error| anyhow::anyhow!("标点分词失败：{error}"))?;
        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let mut pieces: Vec<Piece> = encoding
            .get_ids()
            .iter()
            .zip(encoding.get_offsets())
            .zip(encoding.get_tokens())
            .map(|((&id, &(start, end)), text)| Piece {
                text: text.clone(),
                start,
                end,
                id: id as i64,
            })
            .collect();
        // The exported FireRedPunc ONNX expects BERT [CLS] / [SEP] ids even
        // though the WordPiece vocab is loaded without a post-processor.
        ids.insert(0, 101);
        pieces.insert(
            0,
            Piece {
                text: "[CLS]".to_string(),
                start: 0,
                end: 0,
                id: 101,
            },
        );
        ids.push(102);
        pieces.push(Piece {
            text: "[SEP]".to_string(),
            start: text.len(),
            end: text.len(),
            id: 102,
        });
        Ok((ids, pieces))
    }

    pub fn max_tokens(&self) -> usize {
        510
    }
}

pub fn is_special(piece: &Piece) -> bool {
    matches!(piece.text.as_str(), "[CLS]" | "[SEP]" | "[PAD]" | "[UNK]")
}
