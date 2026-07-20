//! voime_cli — 阶段二引擎验证 CLI（不经 FFI，直接用 rlib）。
//!
//! 子命令：
//!   feat-dump  导出 Rust fbank+cmvn 特征（供与 Python knf 数值对比）
//!   vad        流式 VAD 句段切分（对齐 POC-02）
//!   asr        VAD+ASR 流水线（对齐 POC-03）
//!   punc       文本加标点（对齐 POC-04 的 Punc 环节）
//!   e2e        完整 PTT 会话：partial + final（对齐 POC-03/04 合并）
//!   bench      逐句 ASR 延迟 + Punc 延迟（对齐 POC-05）

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use voime_core::asr::LangHint;
use voime_core::feat::{fbank_whole, firered_fbank_opts, Cmvn};
use voime_core::models::{find_models_dir, find_ort_dll, init_ort, ModelManager};

#[derive(Parser)]
#[command(name = "voime_cli", about = "Voime engine validation CLI")]
struct Cli {
    /// 模型目录（默认: VOIME_MODEL_DIR 或自动探测）
    #[arg(long, global = true)]
    models: Option<PathBuf>,
    /// onnxruntime.dll 路径（默认: VOIME_ORT_DLL 或自动探测）
    #[arg(long, global = true)]
    ort: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    FeatDump { wav: PathBuf, out: PathBuf },
    Vad { wav: PathBuf },
    Asr { wav: PathBuf, #[arg(long, default_value = "auto")] lang: String },
    Punc { text: String },
    E2e { wav: PathBuf, #[arg(long, default_value = "auto")] lang: String },
    Bench { wav: PathBuf, #[arg(long, default_value_t = 3)] reps: u32 },
}

fn read_wav_16k(path: &PathBuf) -> Result<Vec<i16>> {
    let mut r = hound::WavReader::open(path).context("open wav")?;
    let spec = r.spec();
    anyhow::ensure!(
        spec.sample_rate == 16000 && spec.channels == 1 && spec.bits_per_sample == 16,
        "需要 16kHz 16bit 单声道 wav，实际: {}Hz {}ch {}bit",
        spec.sample_rate,
        spec.channels,
        spec.bits_per_sample
    );
    Ok(r.samples::<i16>().collect::<hound::Result<Vec<_>>>()?)
}

fn load(cli: &Cli) -> Result<ModelManager> {
    let dll = cli.ort.clone().or_else(find_ort_dll).context("onnxruntime.dll 未找到")?;
    init_ort(&dll)?;
    let dir = cli.models.clone().or_else(find_models_dir).context("models 目录未找到")?;
    Ok(ModelManager::load(&dir)?)
}

fn parse_lang(s: &str) -> LangHint {
    match s {
        "zh" => LangHint::Zh,
        "en" => LangHint::En,
        _ => LangHint::Auto,
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::FeatDump { wav, out } => {
            let pcm = read_wav_16k(wav)?;
            let models_dir = cli.models.clone().or_else(find_models_dir).unwrap();
            let cmvn = Cmvn::load(&models_dir.join("FireRedASR2-AED-CTC-ONNX/cmvn.ark"))?;
            let feats = fbank_whole(&firered_fbank_opts(), &pcm)?;
            let t = feats.len();
            let mut flat: Vec<f32> = Vec::with_capacity(t * 80);
            for f in &feats {
                flat.extend_from_slice(f);
            }
            cmvn.apply(&mut flat);
            let mut buf = Vec::with_capacity(16 + t * 80 * 4);
            buf.extend_from_slice(&(t as u64).to_le_bytes());
            buf.extend_from_slice(&(80u64).to_le_bytes());
            for v in &flat {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            std::fs::write(out, &buf)?;
            println!("feat: {} frames x 80 -> {}", t, out.display());
        }
        Cmd::Vad { wav } => {
            let pcm = read_wav_16k(wav)?;
            let m = load(&cli)?;
            let mut session = m.vad.session();
            let t0 = Instant::now();
            let mut segs = session.push(&pcm)?;
            if let Some(s) = session.flush() {
                segs.push(s);
            }
            let cost = t0.elapsed();
            println!("音频 {:.2}s, VAD 耗时 {:.1}ms, 句段 {} 个:",
                     pcm.len() as f32 / 16000.0, cost.as_secs_f32() * 1000.0, segs.len());
            for (i, s) in segs.iter().enumerate() {
                println!("  #{}: {:.2}s ~ {:.2}s ({:.2}s)", i + 1,
                         s.start_sample as f32 / 16000.0, s.end_sample as f32 / 16000.0,
                         (s.end_sample - s.start_sample) as f32 / 16000.0);
            }
        }
        Cmd::Asr { wav, lang } => {
            let pcm = read_wav_16k(wav)?;
            let m = load(&cli)?;
            let mut session = m.vad.session();
            let mut segs = session.push(&pcm)?;
            if let Some(s) = session.flush() {
                segs.push(s);
            }
            for (i, s) in segs.iter().enumerate() {
                let seg = &pcm[s.start_sample..s.end_sample.min(pcm.len())];
                let r = m.asr.transcribe(seg, parse_lang(lang))?;
                println!("#{} [{:.2}s~{:.2}s] ({}ms, rtf={:.3}) {}",
                         i + 1, s.start_sample as f32 / 16000.0,
                         s.end_sample as f32 / 16000.0, r.infer_ms, r.rtf, r.text);
            }
        }
        Cmd::Punc { text } => {
            let m = load(&cli)?;
            let t0 = Instant::now();
            let out = m.punc.process(text)?;
            println!("输入: {text}");
            println!("输出: {out}");
            println!("耗时: {:.1}ms", t0.elapsed().as_secs_f32() * 1000.0);
        }
        Cmd::E2e { wav, lang } => {
            let pcm = read_wav_16k(wav)?;
            let m = load(&cli)?;
            let mut session = m.vad.session();
            let mut segs = session.push(&pcm)?;
            if let Some(s) = session.flush() {
                segs.push(s);
            }
            let mut texts = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let seg = &pcm[s.start_sample..s.end_sample.min(pcm.len())];
                let r = m.asr.transcribe(seg, parse_lang(lang))?;
                println!("[partial #{}] {} (asr {}ms)", i + 1, r.text, r.infer_ms);
                texts.push(r.text);
            }
            let joined = smart_join(&texts);
            let t0 = Instant::now();
            let punctuated = m.punc.process(&joined)?;
            println!("[final] {punctuated}  (punc {:.1}ms)", t0.elapsed().as_secs_f32() * 1000.0);
        }
        Cmd::Bench { wav, reps } => {
            let pcm = read_wav_16k(wav)?;
            let m = load(&cli)?;
            let mut session = m.vad.session();
            let mut segs = session.push(&pcm)?;
            if let Some(s) = session.flush() {
                segs.push(s);
            }
            // 预热
            if let Some(s) = segs.first() {
                let _ = m.asr.transcribe(&pcm[s.start_sample..s.end_sample.min(pcm.len())], LangHint::Auto)?;
            }
            let mut texts = Vec::new();
            for (i, s) in segs.iter().enumerate() {
                let seg = &pcm[s.start_sample..s.end_sample.min(pcm.len())];
                let dur = seg.len() as f32 / 16000.0;
                let mut best = f32::MAX;
                let mut text = String::new();
                for _ in 0..*reps {
                    let t0 = Instant::now();
                    let r = m.asr.transcribe(seg, LangHint::Auto)?;
                    best = best.min(t0.elapsed().as_secs_f32());
                    text = r.text;
                }
                println!("#{} ({:.2}s): asr {:.0}ms rtf={:.3}  {}",
                         i + 1, dur, best * 1000.0, best / dur, text);
                texts.push(text);
            }
            let joined = smart_join(&texts);
            let t0 = Instant::now();
            let punctuated = m.punc.process(&joined)?;
            let punc_ms = t0.elapsed().as_secs_f32() * 1000.0;
            println!("[punc] {punctuated}  ({punc_ms:.1}ms)");
        }
    }
    Ok(())
}

fn smart_join(texts: &[String]) -> String {
    let mut out = String::new();
    for t in texts {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            let prev_ascii = out.chars().last().is_some_and(|c| c.is_ascii_alphanumeric());
            let next_ascii = t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
            if prev_ascii && next_ascii {
                out.push(' ');
            }
        }
        out.push_str(t);
    }
    out
}
