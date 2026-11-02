use std::path::Path;

use anyhow::{bail, Context};
use ort::ep::ExecutionProviderDispatch;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

/// GPU execution provider registry for the worker process only.
pub struct GpuRuntime {
    device: String,
    provider: String,
    providers: Vec<ExecutionProviderDispatch>,
}

impl GpuRuntime {
    #[allow(unused_mut, unused_variables)]
    pub fn load(device: &str) -> anyhow::Result<Self> {
        let mut providers = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        let device_id = device
            .parse::<i32>()
            .ok()
            .filter(|value| *value >= 0)
            .unwrap_or(0);

        #[cfg(feature = "migraphx")]
        {
            providers.push(
                ort::ep::MIGraphX::default()
                    .with_device_id(device_id)
                    .with_fp16(true)
                    .build()
                    .error_on_failure(),
            );
            names.push("MIGraphXExecutionProvider");
        }
        #[cfg(feature = "directml")]
        {
            providers.push(
                ort::ep::DirectML::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
            names.push("DmlExecutionProvider");
        }
        #[cfg(feature = "cuda")]
        {
            providers.push(
                ort::ep::CUDA::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
            names.push("CUDAExecutionProvider");
        }

        if providers.is_empty() {
            bail!(
                "RuntimeMissing：voime-gpu 未启用 GPU EP（构建时选择 migraphx / directml / cuda）"
            );
        }
        let _ = ort::init()
            .with_name("voime-gpu")
            .with_telemetry(false)
            .with_execution_providers(providers.clone())
            .commit();
        Ok(Self {
            device: device.to_string(),
            provider: names.join(","),
            providers,
        })
    }

    pub fn create(&self, model: &Path) -> anyhow::Result<Session> {
        let mut builder = Session::builder()
            .map_err(|error| anyhow::anyhow!("创建 GPU ONNX 会话构建器失败：{error}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| anyhow::anyhow!("设置图优化级别失败：{error}"))?
            .with_execution_providers(self.providers.clone())
            .map_err(|error| anyhow::anyhow!("注册 GPU Execution Provider 失败：{error}"))?;
        builder
            .commit_from_file(model)
            .with_context(|| format!("加载 GPU ONNX 模型失败 {}", model.display()))
    }

    pub fn vram_mb(&self) -> u32 {
        std::env::var("VOIME_GPU_VRAM_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn device(&self) -> &str {
        &self.device
    }
}
