use anyhow::Context;
use llama_cpp_4::prelude::*;

pub fn build_prompt(model: &LlamaModel, language: Option<&str>) -> anyhow::Result<String> {
    let language = language.unwrap_or("auto").trim();
    let language = if language.is_empty()
        || !language
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        "auto"
    } else {
        language
    };
    let marker = MtmdContext::default_marker();
    let user = if language.eq_ignore_ascii_case("auto") {
        marker.to_string()
    } else {
        format!("language {language}\n{marker}")
    };
    let messages = [LlamaChatMessage::new("user".to_string(), user)
        .map_err(|error| anyhow::anyhow!("构造 Qwen prompt 失败：{error}"))?];
    model
        .apply_chat_template(None, &messages, true)
        .context("Qwen GGUF 缺少 chat template")
}

pub fn tokenize_audio(
    mtmd: &MtmdContext,
    samples: &[f32],
    prompt: &str,
) -> anyhow::Result<MtmdInputChunks> {
    let bitmap = MtmdBitmap::from_audio(samples).context("创建 Qwen 音频 bitmap 失败")?;
    let text = MtmdInputText::new(prompt, true, true);
    let mut chunks = MtmdInputChunks::default();
    mtmd.tokenize(&text, &[&bitmap], &mut chunks)
        .context("Qwen mtmd tokenize 失败")?;
    Ok(chunks)
}

pub fn eval(
    mtmd: &mut MtmdContext,
    context: &mut LlamaContext<'_>,
    chunks: &MtmdInputChunks,
    n_past: &mut i32,
) -> anyhow::Result<()> {
    let n_batch = context.n_batch() as i32;
    mtmd.eval_chunks(context, chunks, 0, 0, n_batch, true, n_past)
        .context("Qwen mtmd eval 失败")?;
    Ok(())
}
