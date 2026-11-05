use std::path::Path;

use anyhow::{bail, Context};
use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use crate::config::OrtEp;
use crate::error::WorkerError;
use crate::model::TensorSpec;

pub struct Runtime {
    env: Environment,
    ep: OrtEp,
    device: String,
}

impl Runtime {
    pub fn load(ep: OrtEp, device: &str) -> anyhow::Result<Self> {
        let current = OrtEp::current();
        if ep != current {
            return Err(WorkerError::RuntimeMissing(format!(
                "worker 构建只包含 {} EP，不能使用 {}",
                current.as_str(),
                ep.as_str()
            ))
            .into());
        }
        if !device.is_empty() && !device.starts_with(current.device_prefix()) {
            return Err(WorkerError::DeviceUnavailable(format!(
                "请求设备 {device} 与编译时 EP {} 不匹配",
                current.as_str()
            ))
            .into());
        }
        let device_id = device
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| *value >= 0)
            .unwrap_or(0);
        #[allow(unused_mut)]
        let mut providers: Vec<ort::ep::ExecutionProviderDispatch> = Vec::new();
        #[cfg(feature = "cuda")]
        {
            if ep != OrtEp::Cuda {
                return Err(WorkerError::RuntimeMissing(format!(
                    "构建只包含 CUDA EP，不能使用 {}",
                    ep.as_str()
                ))
                .into());
            }
            providers.push(
                ort::ep::CUDA::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
        }
        #[cfg(feature = "migraphx")]
        {
            if ep != OrtEp::Migraphx {
                return Err(WorkerError::RuntimeMissing(format!(
                    "构建只包含 MIGraphX EP，不能使用 {}",
                    ep.as_str()
                ))
                .into());
            }
            providers.push(
                ort::ep::MIGraphX::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
        }
        #[cfg(feature = "rocm")]
        {
            if ep != OrtEp::Rocm {
                return Err(WorkerError::RuntimeMissing(format!(
                    "构建只包含 ROCm EP，不能使用 {}",
                    ep.as_str()
                ))
                .into());
            }
            providers.push(
                ort::ep::ROCm::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
        }
        #[cfg(feature = "directml")]
        {
            if ep != OrtEp::Directml {
                return Err(WorkerError::RuntimeMissing(format!(
                    "构建只包含 DirectML EP，不能使用 {}",
                    ep.as_str()
                ))
                .into());
            }
            providers.push(
                ort::ep::DirectML::default()
                    .with_device_id(device_id)
                    .build()
                    .error_on_failure(),
            );
        }
        if providers.is_empty() {
            return Err(WorkerError::RuntimeMissing(
                "worker 未启用任何 GPU EP（构建时选择 cuda / migraphx / directml）".to_string(),
            )
            .into());
        }
        let env = ort::init()
            .with_name("voime-gpu-firered")
            .with_telemetry(false)
            .with_execution_providers(providers)
            .build()
            .map_err(|error| WorkerError::RuntimeMissing(error.to_string()))?;
        Ok(Self {
            env,
            ep,
            device: if device.is_empty() {
                format!("{}:{device_id}", ep.device_prefix())
            } else {
                device.to_string()
            },
        })
    }

    pub fn ep_name(&self) -> &'static str {
        self.ep.as_str()
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn create(&self, model: &Path) -> anyhow::Result<Session> {
        let mut builder = Session::builder(&self.env)
            .context("创建 GPU ONNX 会话构建器失败")?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|error| anyhow::anyhow!("设置图优化级别失败：{error}"))?;
        // ROCm EP does not claim shape/control nodes; ORT must leave those
        // nodes on the CPU EP. CUDA, MIGraphX and DirectML remain GPU-only.
        if self.ep != OrtEp::Rocm {
            builder = builder
                .with_disable_cpu_fallback()
                .map_err(|error| anyhow::anyhow!("禁用 CPU EP 回退失败：{error}"))?;
        }
        builder
            .commit_from_file(model)
            .with_context(|| format!("加载 GPU ONNX 模型失败 {}", model.display()))
    }

    pub fn create_with_spec(
        &self,
        model: &Path,
        inputs: &[TensorSpec],
        outputs: &[TensorSpec],
    ) -> anyhow::Result<Session> {
        let session = self.create(model)?;
        validate_io(&session, inputs, outputs)?;
        Ok(session)
    }

    /// Creates a GPU session that may still place unsupported graph nodes on
    /// the CPU EP. FireRedPunc contains shape/control nodes that MIGraphX does
    /// not implement; the ASR sessions stay GPU-only.
    pub fn create_with_cpu_fallback(
        &self,
        model: &Path,
        inputs: &[TensorSpec],
        outputs: &[TensorSpec],
    ) -> anyhow::Result<Session> {
        let mut builder = Session::builder(&self.env)
            .context("创建 GPU ONNX 会话构建器失败")?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|error| anyhow::anyhow!("设置图优化级别失败：{error}"))?;
        let session = builder
            .commit_from_file(model)
            .with_context(|| format!("加载 GPU ONNX 模型失败 {}", model.display()))?;
        validate_io(&session, inputs, outputs)?;
        Ok(session)
    }
}

fn validate_io(
    session: &Session,
    inputs: &[TensorSpec],
    outputs: &[TensorSpec],
) -> anyhow::Result<()> {
    validate_side("输入", session.inputs(), inputs)?;
    validate_side("输出", session.outputs(), outputs)?;
    Ok(())
}

fn validate_side(
    label: &str,
    outlets: &[ort::value::Outlet],
    specs: &[TensorSpec],
) -> anyhow::Result<()> {
    for spec in specs {
        let outlet = outlets
            .iter()
            .find(|outlet| outlet.name() == spec.name)
            .with_context(|| format!("{label}张量缺失：{}", spec.name))?;
        let (shape, actual_dtype) = match outlet.dtype() {
            ort::value::ValueType::Tensor { shape, ty, .. } => (shape, *ty),
            other => bail!("{label}张量 {} 不是普通张量：{other:?}", spec.name),
        };
        let expected_dtype = match spec.dtype.to_ascii_lowercase().as_str() {
            "float32" | "f32" => ort::value::TensorElementType::Float32,
            "int64" | "i64" => ort::value::TensorElementType::Int64,
            "int32" | "i32" => ort::value::TensorElementType::Int32,
            other => bail!("{label}张量 {} dtype 非法：{other}", spec.name),
        };
        if expected_dtype != actual_dtype {
            bail!(
                "{label}张量 {} dtype 不符：{:?} vs {actual_dtype:?}",
                spec.name,
                expected_dtype
            );
        }
        if spec.shape.len() != shape.len() {
            bail!(
                "{label}张量 {} 维数不符：{} vs {:?}",
                spec.name,
                spec.shape.len(),
                &**shape
            );
        }
        for (index, (expected, actual)) in spec.shape.iter().zip(shape.iter()).enumerate() {
            if *expected >= 0 && *actual >= 0 && expected != actual {
                bail!(
                    "{label}张量 {} 第 {} 维不符：{expected} vs {actual}",
                    spec.name,
                    index
                );
            }
        }
    }
    Ok(())
}
