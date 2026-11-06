use crate::asr::SenseVoice;

/// CPU Final: one SenseVoice pass over the complete segment.
pub struct Final {
    asr: SenseVoice,
}

impl Final {
    pub fn new(asr: SenseVoice) -> Self {
        Self { asr }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        Ok(self.asr.transcribe(samples)?.text)
    }
}
