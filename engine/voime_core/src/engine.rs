//! RUST-07/08 引擎编排：无锁队列 + Worker 线程 + 异步事件 + 结果所有权转移。
//!
//! 流水线（对齐 PRD 3.3/4.1）：
//!   C++ push_audio --(单次浅拷贝入 ArrayQueue)--> Worker
//!   Worker: VAD 逐帧 -> 句段闭合 -> ASR -> partial 结果(SetEvent)
//!   stop_session -> Finalizing: 排空 -> 末段 ASR -> 全文 Punc -> final 结果 -> Idle
//! 护栏：60s 会话上限（强制收尾）；队列容量 2048 chunk ≈ 61s，溢出 -2 丢帧。

use std::collections::VecDeque;
use std::ffi::{c_void, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;

use crate::asr::LangHint;
use crate::models::ModelManager;
use crate::state::{SessionState, StateMachine};
use crate::vad::{VadSegment, VadSession};

type HANDLE = *mut c_void;
extern "system" {
    fn SetEvent(hEvent: HANDLE) -> i32;
}

pub const QUEUE_CAP_CHUNKS: usize = 2048;
pub const MAX_SESSION_SAMPLES: usize = 60 * 16000; // 60s 护栏

pub struct Engine {
    sm: StateMachine,
    queue: ArrayQueue<Box<[i16]>>,
    wake: Condvar,
    wake_lock: Mutex<()>,
    shutdown: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    results: Mutex<VecDeque<CString>>,
    event_handle: Mutex<Option<HANDLE>>,
    models: Mutex<Option<ModelManager>>,
    dropped: AtomicU64,
    lang_hint: Mutex<LangHint>,
}

impl Engine {
    pub fn new(models: Option<ModelManager>) -> Box<Self> {
        let engine = Box::new(Engine {
            sm: StateMachine::new(),
            queue: ArrayQueue::new(QUEUE_CAP_CHUNKS),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            shutdown: AtomicBool::new(false),
            worker: Mutex::new(None),
            results: Mutex::new(VecDeque::new()),
            event_handle: Mutex::new(None),
            models: Mutex::new(models),
            dropped: AtomicU64::new(0),
            lang_hint: Mutex::new(LangHint::Auto),
        });
        let ptr = &*engine as *const Engine as usize;
        let handle = std::thread::Builder::new()
            .name("voime-worker".into())
            .spawn(move || unsafe { (*(ptr as *const Engine)).worker_loop() })
            .expect("spawn voime-worker");
        *engine.worker.lock().unwrap() = Some(handle);
        engine
    }

    // -------- FFI 门面 --------
    pub fn start(&self) -> i32 {
        if self.models.lock().unwrap().is_none() {
            return -4; // MODELS_MISSING
        }
        self.sm.start()
    }

    pub fn stop(&self) -> i32 {
        let rc = self.sm.stop();
        if rc == 0 {
            self.wake.notify_all();
        }
        rc
    }

    pub fn push_audio(&self, data: &[i16]) -> i32 {
        if self.sm.state() != SessionState::Recording {
            return -1;
        }
        let chunk: Box<[i16]> = data.into();
        match self.queue.push(chunk) {
            Ok(()) => {
                self.wake.notify_one();
                0
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                -2
            }
        }
    }

    pub fn register_event(&self, h: HANDLE) {
        *self.event_handle.lock().unwrap() = Some(h);
    }

    pub fn poll_result(&self, out_len: *mut u32) -> *const std::ffi::c_char {
        let cs = self.results.lock().unwrap().pop_front();
        match cs {
            Some(cs) => {
                if !out_len.is_null() {
                    unsafe { *out_len = cs.as_bytes().len() as u32 };
                }
                cs.into_raw()
            }
            None => std::ptr::null(),
        }
    }

    pub fn model_ready(&self) -> bool {
        self.models.lock().unwrap().is_some()
    }

    pub fn set_lang_hint(&self, lang: LangHint) {
        *self.lang_hint.lock().unwrap() = lang;
    }

    /// 加载/重载模型（建议 Idle 时调用）。0=成功, -4=失败（附带 error 结果）。
    pub fn load_models(&self, dir: &Path) -> i32 {
        match ModelManager::load(dir) {
            Ok(m) => {
                *self.models.lock().unwrap() = Some(m);
                0
            }
            Err(e) => {
                self.push_result(serde_json::json!({
                    "type": "error", "reason": "model_load_failed", "detail": e.to_string()
                }));
                -4
            }
        }
    }

    fn push_result(&self, v: serde_json::Value) {
        let cs = CString::new(v.to_string()).expect("json no nul");
        self.results.lock().unwrap().push_back(cs);
        if let Some(h) = *self.event_handle.lock().unwrap() {
            unsafe { SetEvent(h) };
        }
    }

    // -------- Worker --------
    fn worker_loop(&self) {
        let mut vad: Option<VadSession> = None;
        let mut pcm_buf: Vec<i16> = Vec::with_capacity(16000 * 30);
        let mut sentences: Vec<String> = Vec::new();
        let mut seg_index = 0usize;
        let mut t_session = Instant::now();
        let mut warned_60s = false;
        loop {
            {
                let mut guard = self.wake_lock.lock().unwrap();
                while self.queue.is_empty() && !self.shutdown.load(Ordering::SeqCst) {
                    if self.sm.state() == SessionState::Finalizing {
                        break;
                    }
                    let (g, _t) = self
                        .wake
                        .wait_timeout(guard, Duration::from_millis(50))
                        .unwrap();
                    guard = g;
                }
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return;
            }

            // 注意：队列中的 chunk 全部来自 Recording 期间（push_audio 仅在录音态
            // 接受），Finalizing 的职责正是把它们消费完，因此此处不做状态过滤。
            while let Some(chunk) = self.queue.pop() {
                if pcm_buf.is_empty() {
                    t_session = Instant::now();
                }
                pcm_buf.extend_from_slice(&chunk);
                // 首次进入会话时建立 VAD 会话（全新 caches/状态）
                if vad.is_none() {
                    let models = self.models.lock().unwrap();
                    if let Some(m) = &*models {
                        vad = Some(m.vad.session());
                    }
                }
                if let Some(v) = vad.as_mut() {
                    match v.push(&chunk) {
                        Ok(segments) => {
                            for seg in segments {
                                self.run_asr(&pcm_buf, seg, &mut sentences, &mut seg_index);
                            }
                        }
                        Err(e) => self.push_result(serde_json::json!({
                            "type": "error", "reason": "vad_failed", "detail": e.to_string()
                        })),
                    }
                }
                // 60s 护栏（每会话只触发一次）
                if !warned_60s && pcm_buf.len() >= MAX_SESSION_SAMPLES {
                    warned_60s = true;
                    self.push_result(serde_json::json!({
                        "type": "warning", "reason": "max_duration_60s"
                    }));
                    self.stop();
                }
            }

            if self.sm.state() == SessionState::Finalizing && self.queue.is_empty() {
                self.finalize(&mut vad, &mut pcm_buf, &mut sentences, &mut seg_index, t_session);
                warned_60s = false;
                self.sm.finalize_done();
            }
        }
    }

    fn run_asr(
        &self,
        pcm_buf: &[i16],
        seg: VadSegment,
        sentences: &mut Vec<String>,
        seg_index: &mut usize,
    ) {
        let lang = *self.lang_hint.lock().unwrap();
        let models = self.models.lock().unwrap();
        let Some(m) = &*models else { return };
        let end = seg.end_sample.min(pcm_buf.len());
        let start = seg.start_sample.min(end);
        match m.asr.transcribe(&pcm_buf[start..end], lang) {
            Ok(r) => {
                *seg_index += 1;
                sentences.push(r.text.clone());
                self.push_result(serde_json::json!({
                    "type": "partial",
                    "index": *seg_index,
                    "start_ms": start * 1000 / 16000,
                    "end_ms": end * 1000 / 16000,
                    "text": r.text,
                    "asr_ms": r.infer_ms,
                    "rtf": (r.rtf * 1000.0).round() / 1000.0,
                }));
            }
            Err(e) => self.push_result(serde_json::json!({
                "type": "error", "reason": "asr_failed", "detail": e.to_string()
            })),
        }
    }

    fn finalize(
        &self,
        vad: &mut Option<VadSession>,
        pcm_buf: &mut Vec<i16>,
        sentences: &mut Vec<String>,
        seg_index: &mut usize,
        t_session: Instant,
    ) {
        let session_secs = pcm_buf.len() as f32 / 16000.0;
        if let Some(mut v) = vad.take() {
            if let Some(seg) = v.flush() {
                self.run_asr(pcm_buf, seg, sentences, seg_index);
            }
        }
        let joined = smart_join(sentences);
        let t0 = Instant::now();
        let (punctuated, punc_ms) = if joined.is_empty() {
            (String::new(), 0)
        } else {
            let models = self.models.lock().unwrap();
            match &*models {
                Some(m) => match m.punc.process(&joined) {
                    Ok(p) => (p, t0.elapsed().as_millis() as u64),
                    Err(e) => {
                        self.push_result(serde_json::json!({
                            "type": "error", "reason": "punc_failed", "detail": e.to_string()
                        }));
                        (joined.clone(), 0)
                    }
                },
                None => (joined.clone(), 0),
            }
        };
        self.push_result(serde_json::json!({
            "type": "final",
            "text": punctuated,
            "sentences": sentences,
            "session_secs": (session_secs * 1000.0).round() / 1000.0,
            "punc_ms": punc_ms,
            "dropped_chunks": self.dropped.swap(0, Ordering::Relaxed),
            "wall_secs": (t_session.elapsed().as_secs_f32() * 1000.0).round() / 1000.0,
        }));
        pcm_buf.clear();
        sentences.clear();
        *seg_index = 0;
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake.notify_all();
        let h = self.worker.lock().unwrap().take();
        if let Some(h) = h {
            let _ = h.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 句段文本拼接（移植 poc04 smart_join）：相邻英文/数字 token 之间补空格。
fn smart_join(texts: &[String]) -> String {
    let mut out = String::new();
    for t in texts {
        let t = t.trim();
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            let prev_ascii = out.chars().last().is_some_and(|c| c.is_ascii_alphanumeric());
            let next_ascii = t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
            if prev_ascii && next_ascii {
                out.push(' ');
            }
        }
        out.push_str(t);
    }
    out
}
