use anyhow::Context;

use crate::asr::features::{Fbank, FbankConfig};
use crate::asr::sense_voice::SenseVoice;
use crate::infer::session::SessionFactory;
use crate::model::{AsrArtifact, PuncPolicy};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AsrOutput {
    pub ids: Vec<usize>,
    pub text: String,
}

/// The single Final entry point used by the pipeline. CPU SenseVoice is
/// implemented here; the daemon injects a worker implementation for GPU
/// FireRed AED. Core never loads a GPU runtime.
pub trait FinalBackend: Send {
    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<AsrOutput>;
}

pub struct SenseVoiceBackend {
    sense: SenseVoice,
    fbank: FbankConfig,
}

impl SenseVoiceBackend {
    pub fn new_with_factory(
        artifact: &AsrArtifact,
        factory: &SessionFactory,
        fbank: FbankConfig,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            sense: SenseVoice::new_with_factory(artifact, factory)?,
            fbank,
        })
    }
}

impl FinalBackend for SenseVoiceBackend {
    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<AsrOutput> {
        if samples.is_empty() {
            return Ok(AsrOutput::default());
        }
        let mut fbank = Fbank::new(self.fbank);
        let features = fbank
            .compute(samples)
            .context("计算 SenseVoice fbank 失败")?;
        if features.is_empty() {
            return Ok(AsrOutput::default());
        }
        self.sense.transcribe(&features)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalOutput {
    pub text: String,
    pub punc: PuncPolicy,
}
