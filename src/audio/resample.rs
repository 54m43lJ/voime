use rubato::{FastFixedIn, PolynomialDegree, Resampler};

use super::capture::AudioChunk;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
const RESAMPLE_CHUNK: usize = 1024;

pub struct StreamResampler {
    input_rate: u32,
    channels: u16,
    resampler: Option<FastFixedIn<f32>>,
    pending: Vec<f32>,
}

impl StreamResampler {
    pub fn new(input_rate: u32, channels: u16) -> Self {
        let input_rate = if input_rate == 0 {
            TARGET_SAMPLE_RATE
        } else {
            input_rate
        };
        let channels = channels.max(1);
        let resampler = if input_rate == TARGET_SAMPLE_RATE {
            None
        } else {
            let ratio = TARGET_SAMPLE_RATE as f64 / input_rate as f64;
            let resampler =
                FastFixedIn::<f32>::new(ratio, 1.0, PolynomialDegree::Septic, RESAMPLE_CHUNK, 1)
                    .expect("采样率转换器参数必须有效");
            Some(resampler)
        };
        Self {
            input_rate,
            channels,
            resampler,
            pending: Vec::new(),
        }
    }

    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn push(&mut self, chunk: &AudioChunk) -> Vec<f32> {
        if chunk.sample_rate != self.input_rate || chunk.channels != self.channels {
            return Vec::new();
        }
        let chunk_channels = chunk.channels.max(1) as usize;
        let frames = chunk.samples.len() / chunk_channels;
        let mut mono = Vec::with_capacity(frames);
        for frame in chunk.samples.chunks_exact(chunk_channels) {
            let sum: f32 = frame.iter().copied().sum();
            mono.push(sum / chunk_channels as f32);
        }

        let Some(resampler) = self.resampler.as_mut() else {
            return mono;
        };

        self.pending.extend_from_slice(&mono);
        let mut output = Vec::with_capacity(
            (self.pending.len() as f64 * TARGET_SAMPLE_RATE as f64 / self.input_rate as f64)
                as usize
                + 32,
        );
        while self.pending.len() >= RESAMPLE_CHUNK {
            let block: Vec<f32> = self.pending.drain(..RESAMPLE_CHUNK).collect();
            match resampler.process(&[block], None) {
                Ok(mut channels) => {
                    if let Some(channel) = channels.pop() {
                        output.extend_from_slice(&channel);
                    }
                }
                Err(_) => break,
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmixes_stereo() {
        let mut resampler = StreamResampler::new(16_000, 2);
        let chunk = AudioChunk {
            sample_rate: 16_000,
            channels: 2,
            samples: vec![1.0, -1.0, 0.5, 0.5],
        };
        let output = resampler.push(&chunk);
        assert_eq!(output, vec![0.0, 0.5]);
    }

    #[test]
    fn downsamples_48k_to_16k() {
        let mut resampler = StreamResampler::new(48_000, 1);
        let chunk = AudioChunk {
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.0; 48_000],
        };
        let output = resampler.push(&chunk);
        assert!(output.len() > 12_000);
        assert!(output.len() < 17_000);
    }
}
