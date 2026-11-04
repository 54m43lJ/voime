use std::time::Instant;

use anyhow::{bail, Context};
use llama_cpp_4::prelude::*;

use crate::worker::CancelFlag;

pub const MAX_NEW_TOKENS: usize = 256;

pub fn greedy(
    model: &LlamaModel,
    context: &mut LlamaContext<'_>,
    n_past: &mut i32,
    cancel: &CancelFlag,
    deadline: Instant,
) -> anyhow::Result<Vec<LlamaToken>> {
    let sampler = LlamaSampler::greedy();
    let mut batch = LlamaBatch::new(context.n_batch() as usize, 1);
    let mut tokens = Vec::new();
    loop {
        let token = sampler.sample(context, -1);
        if model.is_eog_token(token) {
            break;
        }
        if tokens.len() >= MAX_NEW_TOKENS {
            break;
        }
        if cancel.is_cancelled() {
            bail!("cancelled");
        }
        if Instant::now() >= deadline {
            bail!("timeout");
        }
        tokens.push(token);
        batch.clear();
        batch
            .add(token, *n_past, &[0], true)
            .context("Qwen batch add 失败")?;
        context.decode(&mut batch).context("Qwen decode 失败")?;
        *n_past += 1;
    }
    Ok(tokens)
}
