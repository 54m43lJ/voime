/*
 * voime.h — Voime 核心引擎 C ABI（M2 交付物，对照 PRD 5.1）
 *
 * 模型：FireRedVAD(流式) + FireRedASR2-AED(CTC int8) + FireRedPunc(BERT)
 * 约定：音频一律 16kHz s16 mono PCM；结果为一律 UTF-8 JSON 字符串：
 *   {"type":"partial","index":1,"start_ms":420,"end_ms":2300,"text":"你好世界","asr_ms":274,"rtf":0.146}
 *   {"type":"final","text":"你好世界。","sentences":[...],"session_secs":8.96,"punc_ms":12,"dropped_chunks":0,"wall_secs":9.0}
 *   {"type":"warning","reason":"max_duration_60s"} / {"type":"error","reason":"...","detail":"..."}
 *
 * 模型定位（voime_create_engine 时）：
 *   1) 环境变量 VOIME_MODEL_DIR  2) %APPDATA%/VoimeInput/models  3) 可执行文件目录/models
 * ONNX Runtime 定位：1) VOIME_ORT_DLL  2) 同目录 onnxruntime.dll
 * 模型未加载不导致 create 失败；start_session 返回 -4，前端下载后调 voime_load_models。
 */
#ifndef VOIME_H
#define VOIME_H

#include <stdint.h>

#ifdef _WIN32
#define VOIME_API extern "C" __declspec(dllimport)
#else
#define VOIME_API extern "C"
#endif

/* 1. 创建引擎实例（预加载模型）。失败返回 NULL（仅内存错误）。 */
VOIME_API void* voime_create_engine(void);

/* 2. 销毁引擎。 */
VOIME_API void voime_destroy_engine(void* engine_handle);

/* 3. 开始录音会话（PTT 按下）。
   返回: 0=成功, -1=已在录音, -3=正在 Finalizing 拒绝请求, -4=模型未加载 */
VOIME_API int32_t voime_start_session(void* engine_handle);

/* 4. 结束录音会话（PTT 松开，异步触发 Finalizing）。0=成功, -1=未在录音 */
VOIME_API int32_t voime_stop_session(void* engine_handle);

/* 5. 推送音频数据（单次浅拷贝入无锁队列，微秒级返回，绝不阻塞采集线程）。
   返回: 0=成功, -1=非录音态, -2=Buffer 溢出（策略：丢弃当前帧） */
VOIME_API int32_t voime_push_audio(void* engine_handle, const int16_t* data, uint32_t len);

/* 6. 注册异步事件通知（Rust 推理出结果时 SetEvent）。event_handle 为 Win32 Event。 */
VOIME_API void voime_register_event(void* engine_handle, void* event_handle);

/* 7. 拉取结果（原子性所有权转移）。
   返回: 成功返回字符串指针(需调用 voime_free_string 释放), 无结果返回 NULL */
VOIME_API const char* voime_poll_result(void* engine_handle, uint32_t* out_len);

/* 8. 释放结果内存（配套 poll_result 使用）。 */
VOIME_API void voime_free_string(const char* ptr);

/* ---------------- 扩展（非 PRD 接口） ---------------- */

/* 版本标识字符串（静态存储，无需释放）。 */
VOIME_API const char* voime_version(void);

/* 模型是否已就绪: 1=就绪, 0=未加载 */
VOIME_API int32_t voime_model_ready(void* engine_handle);

/* 加载/重载模型（前端完成模型下载后调用，建议 Idle 状态）。
   dir 为 UTF-8 路径。返回: 0=成功, -4=失败（失败详情经 error 结果回报） */
VOIME_API int32_t voime_load_models(void* engine_handle, const char* dir);

/* 语言提示: 0=auto, 1=zh, 2=en（影响英文文本规整；CTC 分支为 advisory） */
VOIME_API void voime_set_lang_hint(void* engine_handle, int32_t lang);

#endif /* VOIME_H */
