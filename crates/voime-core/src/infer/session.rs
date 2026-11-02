use std::sync::Arc;

use anyhow::{bail, Context};
use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{TensorElementType, ValueType};

use crate::model::{Artifact, TensorSpec};

#[derive(Clone)]
pub struct SessionFactory {
    threads: usize,
    env: Arc<Environment>,
}

impl SessionFactory {
    pub fn new(threads: usize) -> anyhow::Result<Self> {
        if threads == 0 {
            bail!("线程数必须大于 0");
        }
        let _ = ort::init()
            .with_name("voime")
            .with_telemetry(false)
            .commit();
        let env = Environment::current().context("获取 ONNX Runtime 环境失败")?;
        Ok(Self { threads, env })
    }

    pub fn create(&self, model: &std::path::Path) -> anyhow::Result<Session> {
        let _environment = &self.env;
        let mut builder = Session::builder()
            .context("创建 ONNX 会话构建器失败")?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| anyhow::anyhow!("设置图优化级别失败：{error}"))?
            .with_intra_threads(self.threads)
            .map_err(|error| anyhow::anyhow!("设置 intra-op 线程数失败：{error}"))?;
        builder
            .commit_from_file(model)
            .with_context(|| format!("加载 ONNX 模型失败 {}", model.display()))
    }

    pub fn build(&self, artifact: &Artifact) -> anyhow::Result<Session> {
        let session = self.create(&artifact.model)?;
        validate(&session, artifact)
            .with_context(|| format!("模型张量契约不符 {}", artifact.model.display()))?;
        Ok(session)
    }
}

fn validate(session: &Session, artifact: &Artifact) -> anyhow::Result<()> {
    validate_side("输入", session.inputs(), &artifact.inputs)?;
    validate_side("输出", session.outputs(), &artifact.outputs)?;
    Ok(())
}

fn validate_side(
    label: &str,
    outlets: &[ort::value::Outlet],
    specs: &[TensorSpec],
) -> anyhow::Result<()> {
    if outlets.len() != specs.len() {
        bail!(
            "{label}张量数量不符：清单声明 {} 个，模型实际 {} 个",
            specs.len(),
            outlets.len()
        );
    }
    for spec in specs {
        let outlet = outlets
            .iter()
            .find(|outlet| outlet.name() == spec.name)
            .with_context(|| format!("{label}张量缺失：{}", spec.name))?;
        let (dtype, shape) = match outlet.dtype() {
            ValueType::Tensor { ty, shape, .. } => (format!("{ty:?}"), shape),
            other => bail!("{label}张量 {} 不是普通张量：{other:?}", spec.name),
        };
        if normalize_dtype(&dtype) != normalize_dtype(&spec.dtype) {
            bail!(
                "{label}张量 {} 类型不符：清单 {}，模型 {}",
                spec.name,
                spec.dtype,
                dtype
            );
        }
        if shape.len() != spec.shape.len() {
            bail!(
                "{label}张量 {} 维数不符：清单 {:?}，模型 {:?}",
                spec.name,
                spec.shape,
                &**shape
            );
        }
        for (index, (expected, actual)) in spec.shape.iter().zip(shape.iter()).enumerate() {
            if *expected >= 0 && *actual >= 0 && expected != actual {
                bail!(
                    "{label}张量 {} 第 {} 维不符：清单 {}，模型 {}",
                    spec.name,
                    index,
                    expected,
                    actual
                );
            }
        }
    }
    Ok(())
}

pub fn dtype_is_f32(dtype: &str) -> bool {
    normalize_dtype(dtype) == "float32"
}

pub fn dtype_is_i64(dtype: &str) -> bool {
    normalize_dtype(dtype) == "int64"
}

pub fn dtype_is_i32(dtype: &str) -> bool {
    normalize_dtype(dtype) == "int32"
}

fn normalize_dtype(dtype: &str) -> String {
    match dtype.to_ascii_lowercase().as_str() {
        "f32" | "float" | "float32" => "float32".to_string(),
        "f64" | "double" | "float64" => "float64".to_string(),
        "i64" | "long" | "int64" => "int64".to_string(),
        "i32" | "int" | "int32" => "int32".to_string(),
        "u8" | "uint8" => "uint8".to_string(),
        "i8" | "int8" => "int8".to_string(),
        other => other.to_string(),
    }
}

pub fn element_type_of(spec: &TensorSpec) -> anyhow::Result<TensorElementType> {
    match normalize_dtype(&spec.dtype).as_str() {
        "float32" => Ok(TensorElementType::Float32),
        "float64" => Ok(TensorElementType::Float64),
        "int64" => Ok(TensorElementType::Int64),
        "int32" => Ok(TensorElementType::Int32),
        "int8" => Ok(TensorElementType::Int8),
        "uint8" => Ok(TensorElementType::Uint8),
        other => bail!("不支持的张量类型：{other}"),
    }
}
