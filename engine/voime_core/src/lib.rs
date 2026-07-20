//! voime_core — Voime Rust 核心引擎（阶段二 M2）。
//!
//! 模块对照开发任务：
//! - `models` RUST-02 模型加载器（ONNX -> 内存）
//! - `state`  RUST-03 SessionState 状态机（Finalizing 拒绝）
//! - `vad`    RUST-04 StreamVadEngine（PCM -> 句子边界）
//! - `asr`    RUST-05 AsrEngine（音频切片 -> 文本，语言提示参数）
//! - `punc`   RUST-06 PuncEngine（文本 -> 带标点文本）
//! - `engine` RUST-07/08 无锁队列 + Worker + 异步事件
//! - `ffi`    RUST-07 C ABI 导出（PRD 5.1）
//! - `feat`   fbank/CMVN 特征提取（与 Python kaldi_native_fbank 对齐）

pub mod asr;
pub mod engine;
pub mod error;
pub mod feat;
pub mod ffi;
pub mod models;
pub mod punc;
pub mod state;
pub mod vad;
