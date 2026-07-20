//! RUST-04 StreamVadEngine：接收 PCM，返回句子边界（样本索引）。
//!
//! 模型：FireRedVAD 官方 fireredvad_stream_vad_with_cache.onnx
//!   输入  feat      [1, T, 80] f32（fbank+cmvn，逐帧时 T=1）
//!         caches_in [8, 1, 128, 19] f32
//!   输出  probs     [1, T, 1] f32
//!         caches_out[8, 1, 128, 19] f32（喂单帧时保持 19）
//! 后处理为 FireRedASR2S StreamVadPostprocessor 的忠实移植。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use ndarray::Array4;
use ort::session::Session;
use ort::value::Value;

use crate::error::{Result, VoimeError};
use crate::feat::{firered_fbank_opts, Cmvn, SingleFrameFbank};

pub const FRAME_SAMPLES: usize = 400; // 25ms @16k
const CACHE_ELEMS: usize = 8 * 128 * 19;

#[derive(Debug, Clone, Copy, Default)]
pub struct VadFrameResult {
    pub frame_idx: u64, // 1-based
    pub is_speech: bool,
    pub raw_prob: f32,
    pub smoothed_prob: f32,
    pub is_speech_start: bool,
    pub is_speech_end: bool,
    pub speech_start_frame: i64, // 1-based, -1 无
    pub speech_end_frame: i64,
}

/// 语音句段（样本坐标，相对 VadSession 起点）。
#[derive(Debug, Clone, Copy)]
pub struct VadSegment {
    pub start_sample: usize,
    pub end_sample: usize,
}

// ---------------- 后处理状态机（移植自 stream_vad_postprocessor.py） ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VadState {
    Silence,
    PossibleSpeech,
    Speech,
    PossibleSilence,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamVadConfig {
    pub smooth_window_size: usize,
    pub speech_threshold: f32,
    pub pad_start_frame: usize,
    pub min_speech_frame: usize,
    pub max_speech_frame: usize,
    pub min_silence_frame: usize,
}

impl Default for StreamVadConfig {
    fn default() -> Self {
        // 对齐阶段一 Python 配置
        Self {
            smooth_window_size: 5,
            speech_threshold: 0.4,
            pad_start_frame: 5,
            min_speech_frame: 8,
            max_speech_frame: 2000, // 单句上限（此处帧=25ms，2000 帧=50s）
            min_silence_frame: 20,
        }
    }
}

struct Postprocessor {
    cfg: StreamVadConfig,
    frame_cnt: u64,
    smooth_window: VecDeque<f32>,
    smooth_sum: f32,
    state: VadState,
    speech_cnt: usize,
    silence_cnt: usize,
    hit_max_speech: bool,
    last_speech_start_frame: i64,
    last_speech_end_frame: i64,
}

impl Postprocessor {
    fn new(cfg: StreamVadConfig) -> Self {
        Self {
            cfg,
            frame_cnt: 0,
            smooth_window: VecDeque::new(),
            smooth_sum: 0.0,
            state: VadState::Silence,
            speech_cnt: 0,
            silence_cnt: 0,
            hit_max_speech: false,
            last_speech_start_frame: -1,
            last_speech_end_frame: -1,
        }
    }

    #[allow(dead_code)]
    fn reset(&mut self) {
        let cfg = std::mem::take(&mut self.cfg);
        *self = Self::new(cfg);
    }

    fn process_one_frame(&mut self, raw_prob: f32) -> VadFrameResult {
        self.frame_cnt += 1;
        // 平滑
        self.smooth_window.push_back(raw_prob);
        self.smooth_sum += raw_prob;
        if self.smooth_window.len() > self.cfg.smooth_window_size.max(1) {
            self.smooth_sum -= self.smooth_window.pop_front().unwrap();
        }
        let smoothed = self.smooth_sum / self.smooth_window.len() as f32;
        let is_speech = smoothed >= self.cfg.speech_threshold;

        let mut r = VadFrameResult {
            frame_idx: self.frame_cnt,
            is_speech,
            raw_prob,
            smoothed_prob: smoothed,
            ..Default::default()
        };

        if self.hit_max_speech {
            r.is_speech_start = true;
            r.speech_start_frame = self.frame_cnt as i64;
            self.last_speech_start_frame = r.speech_start_frame;
            self.hit_max_speech = false;
        }

        match self.state {
            VadState::Silence => {
                if is_speech {
                    self.state = VadState::PossibleSpeech;
                    self.speech_cnt += 1;
                } else {
                    self.silence_cnt += 1;
                    self.speech_cnt = 0;
                }
            }
            VadState::PossibleSpeech => {
                if is_speech {
                    self.speech_cnt += 1;
                    if self.speech_cnt >= self.cfg.min_speech_frame {
                        self.state = VadState::Speech;
                        r.is_speech_start = true;
                        r.speech_start_frame = (self.frame_cnt as i64 - self.speech_cnt as i64 + 1
                            - self.cfg.pad_start_frame as i64)
                            .max(1)
                            .max(self.last_speech_end_frame + 1);
                        self.last_speech_start_frame = r.speech_start_frame;
                        self.silence_cnt = 0;
                    }
                } else {
                    self.state = VadState::Silence;
                    self.silence_cnt = 1;
                    self.speech_cnt = 0;
                }
            }
            VadState::Speech => {
                self.speech_cnt += 1;
                if is_speech {
                    self.silence_cnt = 0;
                    if self.speech_cnt >= self.cfg.max_speech_frame {
                        self.hit_max_speech = true;
                        self.speech_cnt = 0;
                        r.is_speech_end = true;
                        r.speech_end_frame = self.frame_cnt as i64;
                        r.speech_start_frame = self.last_speech_start_frame;
                        self.last_speech_start_frame = -1;
                        self.last_speech_end_frame = r.speech_end_frame;
                    }
                } else {
                    self.state = VadState::PossibleSilence;
                    self.silence_cnt += 1;
                }
            }
            VadState::PossibleSilence => {
                self.speech_cnt += 1;
                if is_speech {
                    self.state = VadState::Speech;
                    self.silence_cnt = 0;
                    if self.speech_cnt >= self.cfg.max_speech_frame {
                        self.hit_max_speech = true;
                        self.speech_cnt = 0;
                        r.is_speech_end = true;
                        r.speech_end_frame = self.frame_cnt as i64;
                        r.speech_start_frame = self.last_speech_start_frame;
                        self.last_speech_start_frame = -1;
                        self.last_speech_end_frame = r.speech_end_frame;
                    }
                } else {
                    self.silence_cnt += 1;
                    if self.silence_cnt >= self.cfg.min_silence_frame {
                        self.state = VadState::Silence;
                        r.is_speech_end = true;
                        r.speech_end_frame = self.frame_cnt as i64;
                        r.speech_start_frame = self.last_speech_start_frame;
                        self.last_speech_end_frame = r.speech_end_frame;
                        self.last_speech_start_frame = -1;
                        self.speech_cnt = 0;
                    }
                }
            }
        }
        r
    }

    fn in_speech(&self) -> bool {
        matches!(self.state, VadState::Speech | VadState::PossibleSilence)
    }
}

// ---------------- VAD 引擎 ----------------

pub struct StreamVadEngine {
    session: Mutex<Session>,
    cmvn: Cmvn,
    fbank: Mutex<SingleFrameFbank>,
    pub cfg: StreamVadConfig,
}

impl StreamVadEngine {
    pub fn new(onnx_path: &Path, cmvn_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(VoimeError::Ort)?
            .with_intra_threads(2)
            .map_err(|e| VoimeError::Model(e.to_string()))?
            .commit_from_file(onnx_path)
            .map_err(VoimeError::Ort)?;
        let cmvn = Cmvn::load(cmvn_path)?;
        let fbank = SingleFrameFbank::new(firered_fbank_opts())?;
        Ok(Self {
            session: Mutex::new(session),
            cmvn,
            fbank: Mutex::new(fbank),
            cfg: StreamVadConfig::default(),
        })
    }

    /// 单帧（400 样本）推理，返回原始概率。
    fn infer_prob(&self, pcm400: &[i16], caches: &mut Vec<f32>) -> Result<f32> {
        let mut feat = self.fbank.lock().unwrap().compute(pcm400)?.to_vec();
        self.cmvn.apply(&mut feat);

        let feat_val = Value::from_array((vec![1usize, 1, 80], feat)).map_err(VoimeError::Ort)?;
        let caches_val = Value::from_array((
            vec![8usize, 1, 128, 19],
            std::mem::take(caches),
        ))
        .map_err(VoimeError::Ort)?;

        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs!["feat" => feat_val, "caches_in" => caches_val])
            .map_err(VoimeError::Ort)?;
        let (_ps, probs) = outputs["probs"].try_extract_tensor::<f32>().map_err(VoimeError::Ort)?;
        let prob = probs[0];
        let (cs, cout) = outputs["caches_out"]
            .try_extract_tensor::<f32>()
            .map_err(VoimeError::Ort)?;
        let dims: Vec<usize> = cs.iter().map(|&d| d as usize).collect();
        if dims != vec![8, 1, 128, 19] {
            return Err(VoimeError::Model(format!("caches_out 形状异常: {dims:?}")));
        }
        *caches = cout.to_vec();
        Ok(prob)
    }

    /// 新建一次流式会话（调用方持有，逐帧喂入）。
    pub fn session(self: &std::sync::Arc<Self>) -> VadSession {
        VadSession::new(self.clone())
    }
}

/// 流式 VAD 会话：逐帧喂 PCM，产出句段边界（样本索引）。
pub struct VadSession {
    engine: std::sync::Arc<StreamVadEngine>,
    pp: Postprocessor,
    caches: Vec<f32>,
    remainder: Vec<i16>, // 不足一帧的尾巴
    frames_fed: u64,
}

impl VadSession {
    fn new(engine: std::sync::Arc<StreamVadEngine>) -> Self {
        let pp = Postprocessor::new(engine.cfg);
        Self {
            engine,
            pp,
            caches: vec![0.0; CACHE_ELEMS],
            remainder: Vec::new(),
            frames_fed: 0,
        }
    }

    /// 喂入任意长度 PCM，返回本批产生的句段事件。
    pub fn push(&mut self, pcm: &[i16]) -> Result<Vec<VadSegment>> {
        self.remainder.extend_from_slice(pcm);
        let mut segments = Vec::new();
        let mut taken = 0;
        while self.remainder.len() - taken >= FRAME_SAMPLES {
            let frame = &self.remainder[taken..taken + FRAME_SAMPLES];
            let prob = self.engine.infer_prob(frame, &mut self.caches)?;
            self.frames_fed += 1;
            let r = self.pp.process_one_frame(prob);
            if r.is_speech_end {
                segments.push(VadSegment {
                    start_sample: ((r.speech_start_frame.max(1) - 1) as usize) * FRAME_SAMPLES,
                    end_sample: (r.speech_end_frame as usize) * FRAME_SAMPLES,
                });
            }
            taken += FRAME_SAMPLES;
        }
        self.remainder.drain(..taken);
        Ok(segments)
    }

    /// 流结束：若仍在语音中，强制闭合当前句段。
    pub fn flush(&mut self) -> Option<VadSegment> {
        if self.pp.in_speech() && self.pp.last_speech_start_frame > 0 {
            Some(VadSegment {
                start_sample: ((self.pp.last_speech_start_frame - 1) as usize) * FRAME_SAMPLES,
                end_sample: (self.frames_fed as usize) * FRAME_SAMPLES,
            })
        } else {
            None
        }
    }

    pub fn fed_samples(&self) -> usize {
        (self.frames_fed as usize) * FRAME_SAMPLES
    }

    /// 内部缓冲区中尚未凑满一帧的样本数。
    pub fn remainder_len(&self) -> usize {
        self.remainder.len()
    }
}

// 保持 ndarray 引用（caches 构造用）
#[allow(dead_code)]
fn _type_check(_: Array4<f32>) {}
