//! Voime POC-06 Rust 核心：FFI/C ABI 验证。
//!
//! 验证点（对照 PRD 3.3/4.1/5.1）：
//! - 状态机 Idle/Recording/Finalizing，Finalizing 期间 start_session 返回 -3
//! - push_audio 单次浅拷贝入无锁队列（crossbeam ArrayQueue），溢出返回 -2 不阻塞
//! - Worker 线程条件变量休眠唤醒，消费 PCM 并产出结果
//! - 结果所有权转移（CString::into_raw / free_string 归还）
//! - Windows Event 异步通知（SetEvent）
//!
//! POC 阶段不加载真实模型：Worker 对 PCM 做样本计数/RMS 统计，
//! 每秒聚合一条 JSON 结果，Session 结束时产出汇总结果。

#![allow(non_snake_case)]

use crossbeam_queue::ArrayQueue;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// ---- Win32 极简声明（POC 不引 windows crate，控制依赖面） ----
type HANDLE = *mut c_void;
extern "system" {
    fn SetEvent(hEvent: HANDLE) -> i32;
}

/// 队列容量：2048 chunk × 480 样本(30ms) ≈ 61s，对齐 PRD 60s 护栏
const QUEUE_CAP_CHUNKS: usize = 2048;
/// 每秒聚合一条结果（16kHz）
const SAMPLES_PER_SEC: usize = 16000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
enum SessionState {
    Idle = 0,
    Recording = 1,
    Finalizing = 2,
}

struct Engine {
    state: Mutex<SessionState>,
    queue: ArrayQueue<Box<[i16]>>,
    /// (Mutex, Condvar) 配对：Worker 无任务时 0% CPU 休眠
    wake: Condvar,
    wake_lock: Mutex<()>,
    shutdown: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    results: Mutex<VecDeque<CString>>,
    event_handle: Mutex<Option<HANDLE>>,
    verbose: AtomicBool,
    /// 测试钩子：Worker 每 chunk 额外休眠毫秒数（用于确定性触发 -2 溢出与 -3 拒绝）
    test_worker_delay_ms: AtomicU32,
    // 统计
    pushed_chunks: AtomicU64,
    dropped_chunks: AtomicU64,
}

impl Engine {
    fn new() -> Box<Self> {
        let engine = Box::new(Engine {
            state: Mutex::new(SessionState::Idle),
            queue: ArrayQueue::new(QUEUE_CAP_CHUNKS),
            wake: Condvar::new(),
            wake_lock: Mutex::new(()),
            shutdown: AtomicBool::new(false),
            worker: Mutex::new(None),
            results: Mutex::new(VecDeque::new()),
            event_handle: Mutex::new(None),
            verbose: AtomicBool::new(false),
            test_worker_delay_ms: AtomicU32::new(0),
            pushed_chunks: AtomicU64::new(0),
            dropped_chunks: AtomicU64::new(0),
        });
        // Box 稳定后再起线程，用裸指针（usize 形式绕过 Send 约束）把引用交给 worker
        let ptr = &*engine as *const Engine as usize;
        let handle = std::thread::Builder::new()
            .name("voime-worker".into())
            .spawn(move || unsafe { (*(ptr as *const Engine)).worker_loop() })
            .expect("spawn worker");
        *engine.worker.lock().unwrap() = Some(handle);
        engine
    }

    fn push_result(&self, s: String) {
        let cs = CString::new(s).expect("no interior nul");
        self.results.lock().unwrap().push_back(cs);
        if let Some(h) = *self.event_handle.lock().unwrap() {
            unsafe { SetEvent(h) };
        }
    }

    fn worker_loop(&self) {
        let mut acc_samples: u64 = 0; // 当前秒聚合
        let mut acc_sq: u64 = 0; // 当前秒平方和（RMS）
        let mut total_samples: u64 = 0; // 会话累计
        let mut session_start: Option<Instant> = None;
        loop {
            // 休眠直到有数据 / shutdown / Finalizing 需要收尾
            {
                let mut guard = self.wake_lock.lock().unwrap();
                while self.queue.is_empty() && !self.shutdown.load(Ordering::SeqCst) {
                    // Finalizing 且队列已空 -> 先跳出收尾，别睡死
                    if *self.state.lock().unwrap() == SessionState::Finalizing {
                        break;
                    }
                    let (g, _t) = self
                        .wake
                        .wait_timeout(guard, Duration::from_millis(50))
                        .unwrap();
                    guard = g;
                }
                drop(guard);
            }
            if self.shutdown.load(Ordering::SeqCst) {
                return;
            }
            // 消费队列
            while let Some(chunk) = self.queue.pop() {
                let delay = self.test_worker_delay_ms.load(Ordering::Relaxed);
                if delay > 0 {
                    std::thread::sleep(Duration::from_millis(delay as u64));
                }
                if session_start.is_none() {
                    session_start = Some(Instant::now());
                }
                let n = chunk.len();
                let sq: u64 = chunk.iter().map(|&v| ((v as i64) * (v as i64)) as u64).sum();
                acc_samples += n as u64;
                acc_sq += sq;
                total_samples += n as u64;
                if self.verbose.load(Ordering::Relaxed) {
                    println!(
                        "[rust] chunk len={} total_samples={} queue_left={}",
                        n,
                        total_samples,
                        self.queue.len()
                    );
                }
                if acc_samples >= SAMPLES_PER_SEC as u64 {
                    let rms = ((acc_sq as f64 / acc_samples as f64).sqrt()) as i32;
                    self.push_result(format!(
                        "{{\"type\":\"progress\",\"samples\":{},\"rms\":{}}}",
                        total_samples, rms
                    ));
                    acc_samples = 0;
                    acc_sq = 0;
                }
            }
            // Finalizing 收尾：队列已空 -> 产出汇总 -> 回 Idle
            let mut state = self.state.lock().unwrap();
            if *state == SessionState::Finalizing && self.queue.is_empty() {
                let secs = total_samples as f64 / 16000.0;
                let wall = session_start.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);
                let msg = format!(
                    "{{\"type\":\"session_end\",\"total_samples\":{},\"secs\":{:.3},\"wall_secs\":{:.3},\"pushed_chunks\":{},\"dropped_chunks\":{}}}",
                    total_samples,
                    secs,
                    wall,
                    self.pushed_chunks.load(Ordering::Relaxed),
                    self.dropped_chunks.load(Ordering::Relaxed)
                );
                total_samples = 0;
                acc_samples = 0;
                acc_sq = 0;
                session_start = None;
                self.pushed_chunks.store(0, Ordering::Relaxed);
                self.dropped_chunks.store(0, Ordering::Relaxed);
                *state = SessionState::Idle;
                drop(state);
                self.push_result(msg);
            }
        }
    }
}

fn engine(handle: *mut c_void) -> &'static Engine {
    assert!(!handle.is_null(), "null engine handle");
    unsafe { &*(handle as *const Engine) }
}

// ============ FFI 导出（对照 PRD 5.1） ============

/// 1. 创建引擎实例
#[no_mangle]
pub extern "C" fn voime_create_engine() -> *mut c_void {
    Box::into_raw(Engine::new()) as *mut c_void
}

/// 2. 销毁引擎
#[no_mangle]
pub extern "C" fn voime_destroy_engine(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    let e = unsafe { Box::from_raw(handle as *mut Engine) };
    e.shutdown.store(true, Ordering::SeqCst);
    e.wake.notify_all();
    let handle = e.worker.lock().unwrap().take();
    if let Some(h) = handle {
        let _ = h.join();
    }
    // 析构：队列剩余 chunk 与结果随 Engine drop 自动释放
}

/// 3. 开始录音会话。0=成功, -1=已在录音, -3=Finalizing 拒绝
#[no_mangle]
pub extern "C" fn voime_start_session(handle: *mut c_void) -> i32 {
    let e = engine(handle);
    let mut state = e.state.lock().unwrap();
    match *state {
        SessionState::Idle => {
            *state = SessionState::Recording;
            0
        }
        SessionState::Recording => -1,
        SessionState::Finalizing => -3,
    }
}

/// 4. 结束录音会话（异步 Finalizing）。0=成功, -1=未在录音
#[no_mangle]
pub extern "C" fn voime_stop_session(handle: *mut c_void) -> i32 {
    let e = engine(handle);
    let mut state = e.state.lock().unwrap();
    if *state == SessionState::Recording {
        *state = SessionState::Finalizing;
        drop(state);
        e.wake.notify_all();
        0
    } else {
        -1
    }
}

/// 5. 推送音频（单次浅拷贝入无锁队列）。0=成功, -1=非录音态, -2=溢出丢帧
#[no_mangle]
pub extern "C" fn voime_push_audio(handle: *mut c_void, data: *const i16, len: u32) -> i32 {
    let e = engine(handle);
    if *e.state.lock().unwrap() != SessionState::Recording {
        return -1;
    }
    if data.is_null() || len == 0 {
        return -1;
    }
    let chunk: Box<[i16]> = unsafe { std::slice::from_raw_parts(data, len as usize) }.into();
    match e.queue.push(chunk) {
        Ok(()) => {
            e.pushed_chunks.fetch_add(1, Ordering::Relaxed);
            e.wake.notify_one();
            0
        }
        Err(_) => {
            e.dropped_chunks.fetch_add(1, Ordering::Relaxed);
            -2
        }
    }
}

/// 6. 注册异步事件（Rust 产出结果时 SetEvent）
#[no_mangle]
pub extern "C" fn voime_register_event(handle: *mut c_void, event: HANDLE) {
    let e = engine(handle);
    *e.event_handle.lock().unwrap() = Some(event);
}

/// 7. 拉取结果（所有权转移给调用方）。NULL=无结果
#[no_mangle]
pub extern "C" fn voime_poll_result(handle: *mut c_void, out_len: *mut u32) -> *const c_char {
    let e = engine(handle);
    let cs = e.results.lock().unwrap().pop_front();
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

/// 8. 释放结果内存（交还 Rust）
#[no_mangle]
pub extern "C" fn voime_free_string(ptr: *const c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe { drop(CString::from_raw(ptr as *mut c_char)) };
}

// ---- POC 测试辅助（非 PRD 接口） ----

/// 打印开关：打开后 Rust 逐 chunk 打印长度
#[no_mangle]
pub extern "C" fn voime_set_verbose(handle: *mut c_void, on: i32) {
    engine(handle).verbose.store(on != 0, Ordering::Relaxed);
}

/// 测试钩子：Worker 每 chunk 人工延时（确定性构造溢出/拒绝场景）
#[no_mangle]
pub extern "C" fn voime_test_set_worker_delay(handle: *mut c_void, ms: u32) {
    engine(handle).test_worker_delay_ms.store(ms, Ordering::Relaxed);
}

/// 全局版本标识（FFI 连通性冒烟）
#[no_mangle]
pub extern "C" fn voime_version() -> *const c_char {
    static V: OnceLock<CString> = OnceLock::new();
    V.get_or_init(|| CString::new("voime_poc_core 0.1.0").unwrap())
        .as_ptr()
}
