use anyhow::{bail, Context};
use ort::session::{Session, SessionInputValue};
use ort::value::{Tensor, ValueType};

use super::firered_aed::AedConfig;

#[derive(Clone, Debug)]
pub struct Hypothesis {
    pub tokens: Vec<u32>,
    pub score: f32,
}

struct Beam {
    tokens: Vec<u32>,
    score: f32,
    finished: bool,
}

pub fn beam_search(
    decoder: &mut Session,
    encoder_out: &Tensor<f32>,
    config: &AedConfig,
) -> anyhow::Result<Vec<Hypothesis>> {
    let token_name = decoder
        .inputs()
        .iter()
        .find(|input| matches!(input.dtype(), ValueType::Tensor { ty, .. } if *ty == ort::value::TensorElementType::Int64))
        .map(|input| input.name().to_string())
        .context("ASR 解码器缺少 int64 token 输入")?;
    let encoder_name = decoder
        .inputs()
        .iter()
        .find(|input| matches!(input.dtype(), ValueType::Tensor { ty, .. } if *ty == ort::value::TensorElementType::Float32))
        .map(|input| input.name().to_string())
        .context("ASR 解码器缺少 float32 encoder_out 输入")?;
    let output_name = decoder
        .outputs()
        .first()
        .map(|output| output.name().to_string())
        .context("ASR 解码器缺少输出")?;

    let max_len = config.max_decode_len.max(1);
    let beam_size = config.beam_size.max(1);
    let mut beams = vec![Beam {
        tokens: vec![config.sos_id],
        score: 0.0,
        finished: false,
    }];

    for _ in 0..max_len {
        let mut candidates: Vec<Beam> = Vec::with_capacity(beams.len() * beam_size);

        for beam in &beams {
            if beam.finished {
                candidates.push(Beam {
                    tokens: beam.tokens.clone(),
                    score: beam.score,
                    finished: true,
                });
                continue;
            }

            let tokens: Vec<i64> = beam.tokens.iter().map(|&token| token as i64).collect();
            let token_tensor =
                Tensor::from_array(([1usize, tokens.len()], tokens.into_boxed_slice()))
                    .context("构造解码器 token 张量失败")?;
            let encoder_view = encoder_out.view();
            let inputs: Vec<(String, SessionInputValue<'_>)> = vec![
                (token_name.clone(), token_tensor.into_dyn().into()),
                (encoder_name.clone(), encoder_view.into()),
            ];
            let outputs = decoder.run(inputs).context("ASR 解码器推理失败")?;
            let logits = outputs
                .get(&output_name)
                .with_context(|| format!("ASR 解码器输出缺失：{output_name}"))?;
            let (shape, data) = logits
                .try_extract_tensor::<f32>()
                .context("ASR 解码器输出类型错误")?;

            let vocabulary = *shape.last().context("ASR 解码器输出维数错误")? as usize;
            let positions = if shape.len() >= 2 {
                *shape.get(shape.len() - 2).unwrap_or(&1) as usize
            } else {
                1
            };
            if vocabulary == 0 || positions == 0 {
                bail!("ASR 解码器输出为空");
            }
            let last = &data[(positions - 1) * vocabulary..positions * vocabulary];
            let log_probs = log_softmax(last);

            let order = top_k(&log_probs, beam_size);
            for &token in &order {
                let mut new_tokens = beam.tokens.clone();
                new_tokens.push(token as u32);
                candidates.push(Beam {
                    tokens: new_tokens,
                    score: beam.score + log_probs[token],
                    finished: token as u32 == config.eos_id,
                });
            }
        }

        candidates.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(beam_size);
        beams = candidates;
        if beams.iter().all(|beam| beam.finished) {
            break;
        }
    }

    let mut hypotheses: Vec<Hypothesis> = beams
        .into_iter()
        .map(|beam| Hypothesis {
            tokens: beam.tokens.into_iter().skip(1).collect(),
            score: beam.score,
        })
        .collect();
    hypotheses.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(hypotheses)
}

/// Returns the `k` highest-scoring token ids without sorting the whole
/// vocabulary. The decoder vocabulary is large (8667), and this runs for every
/// beam on every generated token.
fn top_k(log_probs: &[f32], k: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..log_probs.len()).collect();
    let compare = |left: &usize, right: &usize| {
        log_probs[*right]
            .partial_cmp(&log_probs[*left])
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    if k < order.len() {
        order.select_nth_unstable_by(k, compare);
        order.truncate(k);
    }
    order.sort_unstable_by(compare);
    order
}

fn log_softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum_exp: f32 = logits.iter().map(|value| (value - max).exp()).sum();
    let log_sum = max + sum_exp.ln();
    logits.iter().map(|value| value - log_sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_softmax_sums_to_one() {
        let probabilities: Vec<f32> = log_softmax(&[1.0, 2.0, 3.0])
            .into_iter()
            .map(f32::exp)
            .collect();
        let sum: f32 = probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn top_k_keeps_highest_scores_in_order() {
        assert_eq!(top_k(&[0.1, 0.9, 0.2, 0.8], 2), vec![1, 3]);
        assert_eq!(top_k(&[0.1, 0.9], 5), vec![1, 0]);
    }
}
