#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("runtime_missing: {0}")]
    RuntimeMissing(String),
    #[error("device_unavailable: {0}")]
    DeviceUnavailable(String),
    #[error("load_failed: {0}")]
    LoadFailed(String),
    #[error("inference_failed: {0}")]
    InferenceFailed(String),
}

impl WorkerError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RuntimeMissing(_) => "runtime_missing",
            Self::DeviceUnavailable(_) => "device_unavailable",
            Self::LoadFailed(_) => "load_failed",
            Self::InferenceFailed(_) => "inference_failed",
        }
    }

    pub fn common_code(&self) -> voime_gpu_common::ErrorCode {
        voime_gpu_common::ErrorCode::from_wire(self.code())
            .unwrap_or(voime_gpu_common::ErrorCode::InferenceFailed)
    }
}
