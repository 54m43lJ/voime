use std::path::Path;

use anyhow::{bail, Context};
use ort::environment::Environment;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{TensorElementType, ValueType};

use crate::model::ModelManifest;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TensorSpec {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<i64>,
}

#[derive(Clone)]
pub struct SessionFactory {
    threads: usize,
    env: Environment,
}

impl SessionFactory {
    pub fn new(threads: usize) -> anyhow::Result<Self> {
        if threads == 0 {
            bail!("线程数必须大于 0");
        }
        let env = ort::init()
            .with_name("voime")
            .with_telemetry(false)
            .build()
            .context("创建 ONNX Runtime 环境失败")?;
        Ok(Self { threads, env })
    }

    pub fn create(&self, model: &Path) -> anyhow::Result<Session> {
        let mut builder = Session::builder(&self.env)
            .context("创建 ONNX 会话构建器失败")?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|error| anyhow::anyhow!("设置图优化级别失败：{error}"))?
            .with_intra_threads(self.threads)
            .map_err(|error| anyhow::anyhow!("设置 intra-op 线程数失败：{error}"))?;
        builder
            .commit_from_file(model)
            .with_context(|| format!("加载 ONNX 模型失败 {}", model.display()))
    }

    pub fn build(&self, manifest: &ModelManifest) -> anyhow::Result<Session> {
        let model = manifest
            .files
            .iter()
            .map(|file| file.path.as_path())
            .find(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("onnx"))
            })
            .with_context(|| format!("模型 {} 清单中没有 ONNX 文件", manifest.id))?;
        self.create(model)
    }
}

pub fn input_specs(session: &Session) -> anyhow::Result<Vec<TensorSpec>> {
    outlet_specs(session.inputs())
}

pub fn output_specs(session: &Session) -> anyhow::Result<Vec<TensorSpec>> {
    outlet_specs(session.outputs())
}

fn outlet_specs(outlets: &[ort::value::Outlet]) -> anyhow::Result<Vec<TensorSpec>> {
    outlets
        .iter()
        .map(|outlet| {
            let (dtype, shape) = match outlet.dtype() {
                ValueType::Tensor { ty, shape, .. } => (format!("{ty:?}"), shape),
                other => bail!("张量 {} 不是普通张量：{other:?}", outlet.name()),
            };
            Ok(TensorSpec {
                name: outlet.name().to_string(),
                dtype: normalize_dtype(&dtype),
                shape: shape.iter().copied().collect(),
            })
        })
        .collect()
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

pub fn normalize_dtype(dtype: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tensor_dtype_names() {
        assert!(dtype_is_f32("float"));
        assert!(dtype_is_f32("F32"));
        assert!(dtype_is_i64("long"));
        assert!(dtype_is_i32("int"));
        assert_eq!(normalize_dtype("float"), "float32");
    }
}
