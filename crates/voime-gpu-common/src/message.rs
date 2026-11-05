use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    Qwen3,
}

impl GpuBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qwen3 => "qwen3",
        }
    }
}

impl std::fmt::Display for GpuBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Hello {
        protocol: u32,
        backend: GpuBackend,
        model_dir: String,
        device: String,
    },
    Transcribe {
        id: u64,
        sample_rate: u32,
        language: String,
    },
    Cancel {
        id: u64,
    },
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Ready {
        protocol: u32,
        backend: GpuBackend,
        runtime: String,
        device: String,
        vram_mb: u32,
        punc: bool,
    },
    Result {
        id: u64,
        text: String,
        latency_ms: u64,
        rtf: f32,
    },
    Error {
        id: u64,
        code: ErrorCode,
        message: String,
    },
    Bye,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    RuntimeMissing,
    DeviceUnavailable,
    LoadFailed,
    InferenceFailed,
    Timeout,
    ProtocolMismatch,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeMissing => "runtime_missing",
            Self::DeviceUnavailable => "device_unavailable",
            Self::LoadFailed => "load_failed",
            Self::InferenceFailed => "inference_failed",
            Self::Timeout => "timeout",
            Self::ProtocolMismatch => "protocol_mismatch",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "runtime_missing" => Some(Self::RuntimeMissing),
            "device_unavailable" => Some(Self::DeviceUnavailable),
            "load_failed" => Some(Self::LoadFailed),
            "inference_failed" => Some(Self::InferenceFailed),
            "timeout" => Some(Self::Timeout),
            "protocol_mismatch" => Some(Self::ProtocolMismatch),
            _ => None,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
