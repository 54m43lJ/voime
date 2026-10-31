use anyhow::{bail, Context};

use crate::asr::firered_ctc::{AsrOutput, FireRedCtc};
use crate::asr::sense_voice::SenseVoice;
use crate::infer::session::SessionFactory;
use crate::model::{AsrArtifact, AsrKind};

/// All ASR implementations live in the same binary. The active model package
/// only selects which path the pipeline constructs.
pub enum AsrBackend {
    FireRedCtc(FireRedCtc),
    SenseVoice(SenseVoice),
}

impl AsrBackend {
    pub fn new_with_factory(
        artifact: &AsrArtifact,
        factory: &SessionFactory,
    ) -> anyhow::Result<Self> {
        match artifact.kind {
            AsrKind::FireRedCtc => Ok(Self::FireRedCtc(FireRedCtc::new_with_factory(
                artifact, factory,
            )?)),
            AsrKind::SenseVoice => Ok(Self::SenseVoice(SenseVoice::new_with_factory(
                artifact, factory,
            )?)),
            AsrKind::FireRedAed => {
                bail!("FireRed AED 后端尚未启用；请选择 fire-red-ctc 或 sense-voice 模型包")
            }
        }
    }

    pub fn transcribe(&mut self, features: &[Vec<f32>]) -> anyhow::Result<AsrOutput> {
        match self {
            Self::FireRedCtc(backend) => backend.transcribe(features),
            Self::SenseVoice(backend) => backend.transcribe(features),
        }
    }

    pub fn kind(&self) -> AsrKind {
        match self {
            Self::FireRedCtc(_) => AsrKind::FireRedCtc,
            Self::SenseVoice(_) => AsrKind::SenseVoice,
        }
    }

    pub fn supports_partial(&self) -> bool {
        self.kind().supports_partial()
    }
}

/// Creates the backend and rejects a kind that the binary does not implement
/// before the pipeline starts.
pub fn create(artifact: &AsrArtifact, factory: &SessionFactory) -> anyhow::Result<AsrBackend> {
    AsrBackend::new_with_factory(artifact, factory)
        .with_context(|| format!("加载 ASR 后端 {} 失败", artifact.kind.as_str()))
}
