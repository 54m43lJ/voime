//! RUST-05 AsrEngine：输入音频切片，返回文本，支持语言提示参数。
//!
//! 模型：VidraAI/FireRedASR2-onnx（官方权重的 encoder+CTC 分支 int8 导出）
//!   输入  x        [N, T, 80] f32（fbank+cmvn）
//!         x_lens   [N] i64
//!   输出  log_probs     [N, T', 8667] f32
//!         log_probs_len [N] i64
//! 解码：CTC greedy（argmax -> 去重 -> 去 blank=0），对齐 dict.txt 8667 词表。
//!
//! 说明：语言提示参数对 CTC 分支为 advisory（图内无 LID prompt 入口），
//! 当前影响英文文本规整（空格规范）；接入 attention decoder 后消费该参数。

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use ort::session::Session;
use ort::value::Value;

use crate::error::{Result, VoimeError};
use crate::feat::{fbank_whole, firered_fbank_opts, Cmvn};

const BLANK_ID: usize = 0;
const VOCAB: usize = 8667;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangHint {
    Auto,
    Zh,
    En,
}

#[derive(Debug, Clone)]
pub struct AsrResult {
    pub text: String,
    pub audio_ms: u64,
    pub infer_ms: u64,
    pub rtf: f32,
}

pub struct AsrEngine {
    session: Mutex<Session>,
    cmvn: Cmvn,
    tokens: Vec<String>,
}

impl AsrEngine {
    pub fn new(onnx_path: &Path, tokens_path: &Path, cmvn_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(VoimeError::Ort)?
            .with_intra_threads(0) // 默认全核
            .map_err(|e| VoimeError::Model(e.to_string()))?
            .commit_from_file(onnx_path)
            .map_err(VoimeError::Ort)?;
        let cmvn = Cmvn::load(cmvn_path)?;
        let tokens = load_tokens(tokens_path)?;
        if tokens.len() != VOCAB {
            return Err(VoimeError::Model(format!(
                "tokens.txt 词表 {} != {VOCAB}",
                tokens.len()
            )));
        }
        Ok(Self { session: Mutex::new(session), cmvn, tokens })
    }

    /// 识别一段 16kHz i16 PCM（建议 ≤20s，VAD 单句上限内）。
    pub fn transcribe(&self, pcm: &[i16], lang: LangHint) -> Result<AsrResult> {
        if pcm.len() < 400 {
            return Ok(AsrResult { text: String::new(), audio_ms: 0, infer_ms: 0, rtf: 0.0 });
        }
        let t0 = Instant::now();
        // fbank + cmvn
        let feats = fbank_whole(&firered_fbank_opts(), pcm)?;
        let t = feats.len();
        let mut flat: Vec<f32> = Vec::with_capacity(t * 80);
        for f in &feats {
            flat.extend_from_slice(f);
        }
        self.cmvn.apply(&mut flat);

        let x = Value::from_array((vec![1usize, t, 80], flat)).map_err(VoimeError::Ort)?;
        let x_lens = Value::from_array((vec![1usize], vec![t as i64])).map_err(VoimeError::Ort)?;

        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs!["x" => x, "x_lens" => x_lens])
            .map_err(VoimeError::Ort)?;
        let (lps, log_probs) = outputs["log_probs"]
            .try_extract_tensor::<f32>()
            .map_err(VoimeError::Ort)?;
        let dims: Vec<usize> = lps.iter().map(|&d| d as usize).collect();
        let (_ls, lp_lens) = outputs["log_probs_len"]
            .try_extract_tensor::<i64>()
            .map_err(VoimeError::Ort)?;
        let out_t = dims[1];
        let valid = (lp_lens[0] as usize).min(out_t);

        // CTC greedy
        let ids = ctc_greedy(log_probs, valid, VOCAB);
        let text = self.detokenize(&ids, lang);

        let cost = t0.elapsed();
        let audio_ms = (pcm.len() as u64) * 1000 / 16000;
        Ok(AsrResult {
            text,
            audio_ms,
            infer_ms: cost.as_millis() as u64,
            rtf: cost.as_secs_f32() / (pcm.len() as f32 / 16000.0),
        })
    }

    /// id 序列 -> 文本：拼接 token、▁->空格、剔除 <...> 特殊标记、小写化。
    fn detokenize(&self, ids: &[usize], lang: LangHint) -> String {
        let mut s = String::new();
        for &id in ids {
            if id >= self.tokens.len() {
                continue;
            }
            let tok = &self.tokens[id];
            if tok.starts_with('<') && tok.ends_with('>') {
                continue; // <blank> <sil> <sos> <eos> <pad> <unk> 等
            }
            s.push_str(tok);
        }
        let s = s.replace('▁', " ");
        let s = s.trim().to_lowercase();
        match lang {
            LangHint::En => s.split_whitespace().collect::<Vec<_>>().join(" "),
            _ => s,
        }
    }
}

/// CTC greedy：逐帧 argmax，去连续重复，去 blank。
fn ctc_greedy(log_probs: &[f32], valid_t: usize, vocab: usize) -> Vec<usize> {
    let mut ids = Vec::with_capacity(valid_t / 2);
    let mut prev = usize::MAX;
    for t in 0..valid_t {
        let row = &log_probs[t * vocab..(t + 1) * vocab];
        let mut best = 0usize;
        let mut best_v = f32::NEG_INFINITY;
        for (i, &v) in row.iter().enumerate() {
            if v > best_v {
                best_v = v;
                best = i;
            }
        }
        if best != BLANK_ID && best != prev {
            ids.push(best);
        }
        prev = best;
    }
    ids
}

/// 读取 tokens.txt（dict.txt 同格式："token id"，兼容无 id 列与 <space> 行）。
fn load_tokens(path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let mut tokens = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let mut parts = line.trim_end().split_whitespace();
        let word = parts.next().unwrap_or("");
        let (word, idx) = if let Some(idstr) = parts.next() {
            match idstr.parse::<usize>() {
                Ok(id) => (word, id),
                Err(_) => (word, i),
            }
        } else if word.is_empty() {
            (" ", i) // 空行/空格行
        } else {
            (word, i)
        };
        let word = if word == "<space>" { " " } else { word };
        if idx != tokens.len() {
            return Err(VoimeError::Model(format!(
                "tokens.txt 第 {} 行 id 不连续: {idx} != {}",
                i + 1,
                tokens.len()
            )));
        }
        tokens.push(word.to_string());
    }
    Ok(tokens)
}
