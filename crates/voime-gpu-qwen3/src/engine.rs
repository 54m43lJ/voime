use std::num::NonZeroU32;
use std::time::Instant;

use anyhow::{bail, Context};
use llama_cpp_4::prelude::*;

use crate::clean::clean_text;
use crate::decode;
use crate::manifest::Manifest;
use crate::mtmd;
use crate::worker::CancelFlag;

pub struct Engine {
    mtmd: llama_cpp_4::mtmd::MtmdContext,
    model: LlamaModel,
    backend: LlamaBackend,
}

impl Engine {
    pub fn load(
        manifest: &Manifest,
        model_dir: &std::path::Path,
        device: &str,
    ) -> anyhow::Result<Self> {
        let backend = LlamaBackend::init().context("初始化 llama.cpp backend 失败")?;
        let model_path = model_dir.join(&manifest.main_model()?.path);
        let model = LlamaModel::load_from_file(&backend, &model_path, &LlamaModelParams::default())
            .with_context(|| format!("加载 Qwen 主模型失败 {}", model_path.display()))?;
        if !matches!(model.ftype(), Some(LlamaFtype::MostlyBF16)) {
            bail!("Qwen 主模型不是 BF16：{:?}", model.ftype());
        }
        let mmproj_path = model_dir.join(&manifest.mmproj()?.path);
        if !mmproj_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("bf16") && !name.contains("Q8"))
        {
            bail!("Qwen mmproj 必须是 BF16：{}", mmproj_path.display());
        }
        let mtmd = MtmdContext::init_from_file(&mmproj_path, &model, MtmdContextParams::default())
            .with_context(|| format!("加载 Qwen mmproj 失败 {}", mmproj_path.display()))?;
        if !mtmd.capabilities(&model).audio {
            bail!("Qwen mmproj 不支持音频输入");
        }
        if mtmd.audio_sample_rate() != 16_000 {
            bail!("Qwen mmproj 采样率不是 16000");
        }
        if !device.is_empty() && !device.starts_with("vulkan") && !device.starts_with("cuda") {
            bail!("不支持的 Qwen 设备：{device}");
        }
        let mut engine = Self {
            mtmd,
            model,
            backend,
        };
        engine.warmup()?;
        Ok(engine)
    }

    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: Option<&str>,
        cancel: &CancelFlag,
        deadline: Instant,
    ) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let mut context = self
            .model
            .new_context(&self.backend, self.context_params())
            .context("创建 Qwen 上下文失败")?;
        let prompt = mtmd::build_prompt(&self.model, language)?;
        let chunks = mtmd::tokenize_audio(&self.mtmd, samples, &prompt)?;
        let mut n_past = 0;
        mtmd::eval(&mut self.mtmd, &mut context, &chunks, &mut n_past)?;
        let tokens = decode::greedy(&self.model, &mut context, &mut n_past, cancel, deadline)?;
        let raw = self
            .model
            .detokenize(&tokens, true, false)
            .context("Qwen detokenize 失败")?;
        Ok(clean_text(&raw))
    }

    fn warmup(&mut self) -> anyhow::Result<()> {
        let silence = vec![0.0f32; 1600];
        let cancel = CancelFlag::new();
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let _ = self.transcribe(&silence, Some("auto"), &cancel, deadline)?;
        Ok(())
    }

    fn context_params(&self) -> LlamaContextParams {
        // 30 s of audio plus the prompt and up to MAX_NEW_TOKENS output must
        // fit; 4096 also leaves room for mtmd audio embeddings.
        LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(4096))
            .with_n_batch(2048)
    }
}
