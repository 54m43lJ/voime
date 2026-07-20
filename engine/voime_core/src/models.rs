//! RUST-02 模型加载器：从指定目录加载 ONNX 模型至内存。
//!
//! 目录约定（见 docs/poc/POC_REPORT.md 与 stage-2 报告）：
//! ```text
//! <models>/
//! ├── FireRedVAD-ONNX/fireredvad_stream_vad_with_cache.onnx, cmvn.ark
//! ├── FireRedASR2-AED-CTC-ONNX/model.int8.onnx, tokens.txt, cmvn.ark
//! └── FireRedPunc-ONNX/punc_bert.onnx, tokenizer.json, out_dict
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::asr::AsrEngine;
use crate::error::{Result, VoimeError};
use crate::punc::PuncEngine;
use crate::vad::StreamVadEngine;

/// 模型文件相对路径。
pub const VAD_ONNX: &str = "FireRedVAD-ONNX/fireredvad_stream_vad_with_cache.onnx";
pub const VAD_CMVN: &str = "FireRedVAD-ONNX/cmvn.ark";
pub const ASR_ONNX: &str = "FireRedASR2-AED-CTC-ONNX/model.int8.onnx";
pub const ASR_TOKENS: &str = "FireRedASR2-AED-CTC-ONNX/tokens.txt";
pub const ASR_CMVN: &str = "FireRedASR2-AED-CTC-ONNX/cmvn.ark";
pub const PUNC_ONNX: &str = "FireRedPunc-ONNX/punc_bert.onnx";
pub const PUNC_TOKENIZER: &str = "FireRedPunc-ONNX/tokenizer.json";
pub const PUNC_OUTDICT: &str = "FireRedPunc-ONNX/out_dict";

/// 三引擎集合（RUST-02 交付：加载至内存）。
pub struct ModelManager {
    pub vad: std::sync::Arc<StreamVadEngine>,
    pub asr: std::sync::Arc<AsrEngine>,
    pub punc: std::sync::Arc<PuncEngine>,
    pub dir: PathBuf,
}

impl ModelManager {
    pub fn load(dir: &Path) -> Result<Self> {
        let check = |rel: &str| -> Result<PathBuf> {
            let p = dir.join(rel);
            if p.exists() {
                Ok(p)
            } else {
                Err(VoimeError::Model(format!("缺失模型文件: {}", p.display())))
            }
        };
        let vad = std::sync::Arc::new(StreamVadEngine::new(&check(VAD_ONNX)?, &check(VAD_CMVN)?)?);
        let asr = std::sync::Arc::new(AsrEngine::new(&check(ASR_ONNX)?, &check(ASR_TOKENS)?, &check(ASR_CMVN)?)?);
        let punc = std::sync::Arc::new(PuncEngine::new(&check(PUNC_ONNX)?, &check(PUNC_TOKENIZER)?, &check(PUNC_OUTDICT)?)?);
        Ok(Self { vad, asr, punc, dir: dir.to_path_buf() })
    }
}

/// 初始化全局 ONNX Runtime 环境（load-dynamic，进程一次）。
static ORT_INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();

pub fn init_ort(dll_path: &Path) -> Result<()> {
    ORT_INIT
        .get_or_init(|| {
            if !dll_path.exists() {
                return Err(format!("onnxruntime.dll 不存在: {}", dll_path.display()));
            }
            match ort::init_from(dll_path) {
                Ok(builder) => {
                    if builder.with_name("voime").commit() {
                        Ok(())
                    } else {
                        Err("ort commit 失败".to_string())
                    }
                }
                Err(e) => Err(format!("ort init_from: {e}")),
            }
        })
        .as_ref()
        .map_err(|e| VoimeError::Model(e.clone()))?;
    Ok(())
}

/// 按优先级定位 onnxruntime.dll：
/// 1) 环境变量 VOIME_ORT_DLL  2) 可执行文件/DLL 同目录  3) 仓库 .toolchain
pub fn find_ort_dll() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VOIME_ORT_DLL") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent()?;
        for name in ["onnxruntime.dll", "libonnxruntime.so"] {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        // 开发期: engine/target/{debug,release} -> repo/.toolchain/onnxruntime/lib
        for up in [2, 3, 4] {
            let mut base = dir.to_path_buf();
            for _ in 0..up {
                base = base.parent()?.to_path_buf();
            }
            let p = base.join(".toolchain/onnxruntime/lib/onnxruntime.dll");
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

/// 按优先级定位模型目录：
/// 1) 环境变量 VOIME_MODEL_DIR  2) %APPDATA%/VoimeInput/models  3) 可执行文件目录/models
pub fn find_models_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VOIME_MODEL_DIR") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(appdata).join("VoimeInput/models");
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.parent()?.join("models");
        if p.is_dir() {
            return Some(p);
        }
        // 开发期: 向上找仓库 models/
        let mut base = exe.parent()?.to_path_buf();
        for _ in 0..4 {
            let p = base.join("models");
            if p.is_dir() {
                return Some(p);
            }
            base = base.parent()?.to_path_buf();
        }
    }
    None
}
