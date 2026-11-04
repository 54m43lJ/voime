use std::sync::{Arc, Mutex};

use crate::asr::SenseVoice;

/// CPU Final: one SenseVoice pass over the complete segment.
pub struct Final {
    asr: Arc<Mutex<SenseVoice>>,
}

impl Final {
    pub fn new(asr: Arc<Mutex<SenseVoice>>) -> Self {
        Self { asr }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let mut asr = self
            .asr
            .lock()
            .map_err(|_| anyhow::anyhow!("SenseVoice 会话锁已损坏"))?;
        Ok(asr.transcribe(samples)?.text)
    }
}
