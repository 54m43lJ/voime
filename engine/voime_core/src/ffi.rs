//! RUST-07 FFI 接口（C ABI，对照 PRD 5.1 + 扩展函数）。

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::asr::LangHint;
use crate::engine::Engine;
use crate::models::{find_models_dir, find_ort_dll, init_ort, ModelManager};

fn engine(handle: *mut c_void) -> &'static Engine {
    assert!(!handle.is_null(), "null engine handle");
    unsafe { &*(handle as *const Engine) }
}

/// 1. 创建引擎实例（预加载模型；模型缺失不失败，start_session 返回 -4）。
#[no_mangle]
pub extern "C" fn voime_create_engine() -> *mut c_void {
    let models = (|| -> Option<ModelManager> {
        let dll = find_ort_dll()?;
        init_ort(&dll).ok()?;
        let dir = find_models_dir()?;
        ModelManager::load(&dir).ok()
    })();
    Box::into_raw(Engine::new(models)) as *mut c_void
}

/// 2. 销毁引擎。
#[no_mangle]
pub extern "C" fn voime_destroy_engine(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(handle as *mut Engine) });
}

/// 3. 开始录音会话。0=成功, -1=已在录音, -3=Finalizing 拒绝, -4=模型未加载
#[no_mangle]
pub extern "C" fn voime_start_session(handle: *mut c_void) -> i32 {
    engine(handle).start()
}

/// 4. 结束录音会话（异步 Finalizing）。0=成功, -1=未在录音
#[no_mangle]
pub extern "C" fn voime_stop_session(handle: *mut c_void) -> i32 {
    engine(handle).stop()
}

/// 5. 推送音频（单次浅拷贝入无锁队列）。0=成功, -1=非录音态, -2=溢出丢帧
///
/// # Safety
/// `data` 必须指向 `len` 个 i16 的有效内存。
#[no_mangle]
pub unsafe extern "C" fn voime_push_audio(
    handle: *mut c_void,
    data: *const i16,
    len: u32,
) -> i32 {
    if data.is_null() || len == 0 {
        return -1;
    }
    let slice = std::slice::from_raw_parts(data, len as usize);
    engine(handle).push_audio(slice)
}

/// 6. 注册异步事件（产出结果时 SetEvent）。
#[no_mangle]
pub extern "C" fn voime_register_event(handle: *mut c_void, event_handle: *mut c_void) {
    engine(handle).register_event(event_handle);
}

/// 7. 拉取结果（所有权转移给调用方，需 voime_free_string 归还）。NULL=无结果。
#[no_mangle]
pub extern "C" fn voime_poll_result(handle: *mut c_void, out_len: *mut u32) -> *const c_char {
    engine(handle).poll_result(out_len)
}

/// 8. 释放结果内存（交还 Rust）。
///
/// # Safety
/// `ptr` 必须是 voime_poll_result 返回的指针。
#[no_mangle]
pub unsafe extern "C" fn voime_free_string(ptr: *const c_char) {
    if ptr.is_null() {
        return;
    }
    drop(CString::from_raw(ptr as *mut c_char));
}

// ---------------- 扩展（非 PRD 接口，见 voime.h） ----------------

/// 版本标识（FFI 连通性冒烟）。
#[no_mangle]
pub extern "C" fn voime_version() -> *const c_char {
    static V: OnceLock<CString> = OnceLock::new();
    V.get_or_init(|| CString::new(concat!("voime_core ", env!("CARGO_PKG_VERSION"))).unwrap())
        .as_ptr()
}

/// 模型是否已就绪。1=就绪, 0=未加载
#[no_mangle]
pub extern "C" fn voime_model_ready(handle: *mut c_void) -> i32 {
    engine(handle).model_ready() as i32
}

/// 加载/重载模型（前端下载完成后通知）。0=成功, -4=失败
///
/// # Safety
/// `dir` 必须为有效 UTF-8 路径的 C 字符串。
#[no_mangle]
pub unsafe extern "C" fn voime_load_models(handle: *mut c_void, dir: *const c_char) -> i32 {
    if dir.is_null() {
        return -4;
    }
    let dir = match CStr::from_ptr(dir).to_str() {
        Ok(s) => PathBuf::from(s),
        Err(_) => return -4,
    };
    engine(handle).load_models(&dir)
}

/// 设置语言提示：0=auto, 1=zh, 2=en
#[no_mangle]
pub extern "C" fn voime_set_lang_hint(handle: *mut c_void, lang: i32) {
    let lang = match lang {
        1 => LangHint::Zh,
        2 => LangHint::En,
        _ => LangHint::Auto,
    };
    engine(handle).set_lang_hint(lang);
}
