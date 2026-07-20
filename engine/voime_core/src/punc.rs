//! RUST-06 PuncEngine：输入原始文本，返回带标点文本。
//!
//! 模型：自研导出的 punc_bert.onnx（chinese-lert-base + 5 类分类头，
//! 导出与校验见 tools/export_punc_onnx.py，与 PyTorch 参考输出逐字一致）。
//!   输入  input_ids [1,T] i64 / attention_mask [1,T] i64（图内自动加 [CLS]）
//!   输出  logits [1,T,5] f32（0=无标点, 1=，2=。3=？4=！）
//! 文本重建移植自 FireRedASR2S ModelIO.add_punc_to_txt + RuleBaedTxtFix。

use std::path::Path;
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Value;
use regex::Regex;
use tokenizers::Tokenizer;

use crate::error::{Result, VoimeError};

const SPACE_TAG: &str = " ";

pub struct PuncEngine {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    out_tags: Vec<String>, // id -> 标点
}

impl PuncEngine {
    pub fn new(onnx_path: &Path, tokenizer_path: &Path, out_dict_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(VoimeError::Ort)?
            .with_intra_threads(2)
            .map_err(|e| VoimeError::Model(e.to_string()))?
            .commit_from_file(onnx_path)
            .map_err(VoimeError::Ort)?;
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VoimeError::Tokenizer(e.to_string()))?;
        let out_tags = load_out_dict(out_dict_path)?;
        Ok(Self { session: Mutex::new(session), tokenizer, out_tags })
    }

    /// 文本加标点。空输入返回空串。
    pub fn process(&self, text: &str) -> Result<String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        let enc = self
            .tokenizer
            .encode(trimmed, false) // 不加 [CLS]/[SEP]，CLS 在图内添加
            .map_err(|e| VoimeError::Tokenizer(e.to_string()))?;
        let tokens = enc.get_tokens().to_vec();
        let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let t = ids.len();
        if t == 0 {
            return Ok(trimmed.to_string());
        }

        let ids_val = Value::from_array((vec![1usize, t], ids)).map_err(VoimeError::Ort)?;
        let mask_val =
            Value::from_array((vec![1usize, t], vec![1i64; t])).map_err(VoimeError::Ort)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs!["input_ids" => ids_val, "attention_mask" => mask_val])
            .map_err(VoimeError::Ort)?;
        let (shape, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(VoimeError::Ort)?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        let ntag = dims[2];

        let preds: Vec<usize> = (0..t)
            .map(|i| {
                let row = &logits[i * ntag..(i + 1) * ntag];
                row.iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            })
            .collect();

        let joined = self.add_punc_to_txt(&tokens, &preds);
        Ok(rule_based_fix(&joined, true))
    }

    /// 移植 ModelIO.add_punc_to_txt。
    fn add_punc_to_txt(&self, tokens: &[String], preds: &[usize]) -> String {
        let alnum = Regex::new(r"[a-zA-Z0-9#]+").unwrap();
        let mut txt = String::new();
        for (i, token) in tokens.iter().enumerate() {
            let tag = self.out_tags.get(preds[i]).map(|s| s.as_str()).unwrap_or(SPACE_TAG);
            let mut tok = token.clone();
            if tok.starts_with("##") {
                tok = tok.replace("##", "");
            } else if i > 0
                && alnum.is_match(&tok)
                && alnum.is_match(&tokens[i - 1])
                && self.out_tags.get(preds[i - 1]).map(|s| s.as_str()) == Some(SPACE_TAG)
            {
                tok = format!(" {tok}");
            }
            if tag == SPACE_TAG {
                txt.push_str(&tok);
            } else {
                txt.push_str(&tok);
                txt.push_str(tag);
            }
        }
        while txt.contains("  ") {
            txt = txt.replace("  ", " ");
        }
        txt
    }
}

/// 移植 RuleBaedTxtFix.fix。
fn rule_based_fix(txt: &str, capitalize_first: bool) -> String {
    let lower = txt.to_lowercase();
    let mut s = lower;
    let rules: [(Regex, &str); 12] = [
        (Regex::new(r"([a-z])，([a-z])").unwrap(), "$1, $2"),
        (Regex::new(r"([a-z])。([a-z])").unwrap(), "$1. $2"),
        (Regex::new(r"([a-z])？([a-z])").unwrap(), "$1? $2"),
        (Regex::new(r"([a-z])！([a-z])").unwrap(), "$1! $2"),
        (Regex::new(r"^([a-z]+)，").unwrap(), "$1,"),
        (Regex::new(r"^([a-z]+)。").unwrap(), "$1."),
        (Regex::new(r"^([a-z]+)？").unwrap(), "$1?"),
        (Regex::new(r"^([a-z]+)！").unwrap(), "$1!"),
        (Regex::new(r"( [a-zA-Z']+)，$").unwrap(), "$1,"),
        (Regex::new(r"( [a-zA-Z']+)。$").unwrap(), "$1."),
        (Regex::new(r"( [a-zA-Z']+)？$").unwrap(), "$1?"),
        (Regex::new(r"( [a-zA-Z']+)！$").unwrap(), "$1!"),
    ];
    for (re, rep) in &rules {
        s = re.replace_all(&s, *rep).to_string();
    }
    // I 大写
    for (re, rep) in [
        (Regex::new(r"^i ").unwrap(), "I "),
        (Regex::new(r"^i'm ").unwrap(), "I'm "),
        (Regex::new(r"^i'd ").unwrap(), "I'd "),
        (Regex::new(r"^i've ").unwrap(), "I've "),
        (Regex::new(r"^i'll ").unwrap(), "I'll "),
        (Regex::new(r" i ").unwrap(), " I "),
        (Regex::new(r" i'm ").unwrap(), " I'm "),
        (Regex::new(r" i'd ").unwrap(), " I'd "),
        (Regex::new(r" i've ").unwrap(), " I've "),
        (Regex::new(r" i'll ").unwrap(), " I'll "),
    ] {
        s = re.replace_all(&s, rep).to_string();
    }
    // 首字母大写
    if capitalize_first && s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        let mut chars = s.chars();
        let first = chars.next().unwrap().to_ascii_uppercase();
        s = first.to_string() + chars.as_str();
    }
    // 句末标点后英文大写
    let re = Regex::new(r"([.!?。？！])\s+([a-z])").unwrap();
    s = re
        .replace_all(&s, |caps: &regex::Captures| {
            format!("{} {}", &caps[1], caps[2].to_ascii_uppercase())
        })
        .to_string();
    s
}

/// out_dict 解析："tag id" 行格式，<space> 转为空格。
fn load_out_dict(path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let mut tags = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let mut parts = line.trim_end().split_whitespace();
        let word = parts.next().unwrap_or("");
        let idx = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(i);
        let word = if word == "<space>" { " " } else { word };
        if idx != tags.len() {
            return Err(VoimeError::Model(format!("out_dict 第 {} 行 id 不连续", i + 1)));
        }
        tags.push(word.to_string());
    }
    Ok(tags)
}
