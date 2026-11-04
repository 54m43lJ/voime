pub mod labels;
pub mod tokenizer;

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;

use crate::model::{ModelManifest, TensorSpec};
use crate::punc::labels::{argmax, PuncLabels};
use crate::punc::tokenizer::{is_special, PuncTokenizer};
use crate::runtime::Runtime;

pub const MAX_TOKENS: usize = 510;

pub struct Punc {
    session: Session,
    tokenizer: PuncTokenizer,
    labels: PuncLabels,
}

impl Punc {
    pub fn new(manifest: &ModelManifest, runtime: &Runtime) -> anyhow::Result<Self> {
        let inputs = vec![
            TensorSpec {
                name: "input_ids".to_string(),
                dtype: "int64".to_string(),
                shape: vec![1, -1],
            },
            TensorSpec {
                name: "lengths".to_string(),
                dtype: "int64".to_string(),
                shape: vec![1],
            },
        ];
        let outputs = vec![TensorSpec {
            name: "logits".to_string(),
            dtype: "float32".to_string(),
            shape: vec![1, -1, 5],
        }];
        let session = runtime.create_with_spec(manifest.file("model.onnx")?, &inputs, &outputs)?;
        let tokenizer = PuncTokenizer::load(manifest.file("tokens.txt")?)?;
        let labels = PuncLabels::load(manifest.file("out_dict")?, manifest.file("punc.json")?)?;
        Ok(Self {
            session,
            tokenizer,
            labels,
        })
    }

    pub fn punctuate(&mut self, text: &str, cancel: &AtomicBool) -> anyhow::Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        let mut output = String::with_capacity(text.len() + 8);
        for chunk in split_chunks(text, &self.tokenizer, MAX_TOKENS) {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            output.push_str(&self.punctuate_chunk(&chunk)?);
        }
        Ok(output)
    }

    fn punctuate_chunk(&mut self, text: &str) -> anyhow::Result<String> {
        let (ids, pieces) = self.tokenizer.encode(text)?;
        if ids.is_empty() {
            return Ok(text.to_string());
        }
        let token_count = ids.len();
        let input_ids = Tensor::from_array(([1usize, token_count], ids.into_boxed_slice()))
            .context("构造标点 input_ids 失败")?;
        let lengths = Tensor::from_array(([1usize], vec![token_count as i64].into_boxed_slice()))
            .context("构造标点 lengths 失败")?;
        let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
            ("input_ids".to_string(), input_ids.into_dyn().into()),
            ("lengths".to_string(), lengths.into_dyn().into()),
        ];
        let outputs = self.session.run(inputs).context("标点模型推理失败")?;
        let output = outputs.get("logits").context("标点模型没有 logits 输出")?;
        let (shape, logits) = output
            .try_extract_tensor::<f32>()
            .context("标点模型输出类型错误")?;
        let classes = shape.last().copied().unwrap_or(0).max(0) as usize;
        if classes != self.labels.len() || logits.len() < token_count * classes {
            anyhow::bail!(
                "标点模型输出形状非法：{shape:?}（标签数 {}）",
                self.labels.len()
            );
        }
        let mut labels = Vec::with_capacity(token_count);
        for index in 0..token_count {
            if is_special(&pieces[index]) {
                labels.push(0);
                continue;
            }
            let start = index * classes;
            labels.push(argmax(&logits[start..start + classes]));
        }
        self.labels.insert(text, &pieces, &labels)
    }
}

fn split_chunks(text: &str, tokenizer: &PuncTokenizer, max_tokens: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let real_tokens = match tokenizer.encode(text) {
        Ok((_, pieces)) => pieces
            .into_iter()
            .skip(1)
            .take_while(|piece| piece.text != "[SEP]")
            .collect::<Vec<_>>(),
        Err(_) => {
            // Fall back to character boundaries if the tokenizer cannot
            // process the text; punctuation still gets a chance to run.
            return text
                .char_indices()
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
                .chunks(max_tokens.max(1))
                .map(|indices| {
                    let start = indices[0];
                    let end = indices
                        .last()
                        .and_then(|index| {
                            text[*index..]
                                .chars()
                                .next()
                                .map(|character| *index + character.len_utf8())
                        })
                        .unwrap_or(text.len());
                    text[start..end].to_string()
                })
                .collect();
        }
    };
    if real_tokens.len() <= max_tokens {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut taken = 0usize;
    for piece in &real_tokens {
        if taken == max_tokens {
            let end = piece.start.min(text.len());
            if end > start {
                chunks.push(text[start..end].to_string());
                start = end;
            }
            taken = 0;
        }
        taken += 1;
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    chunks
}
