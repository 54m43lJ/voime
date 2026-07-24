// engine.h — voime_core.dll 动态加载封装（对照 engine/include/voime.h）
#pragma once
#include <cstdint>
#include <string>
#include <windows.h>

class Engine {
public:
    Engine() = default;
    ~Engine(); // 销毁引擎实例（不 FreeLibrary，进程退出时统一清理）

    // 加载 DLL 并解析全部导出符号
    bool load(const wchar_t* dll_path);
    bool loaded() const { return mod_ != nullptr; }
    const std::string& last_error() const { return err_; }

    // ---- 引擎实例生命周期 ----
    bool create();            // 预加载模型（遵循 voime.h 模型定位约定）
    void destroy();
    bool created() const { return handle_ != nullptr; }

    // ---- PRD 5.1 八接口 ----
    int start();                                  // 0=成功 -1=已录音 -3=Finalizing -4=模型未加载
    int stop();                                   // 0=成功 -1=未在录音
    int push(const int16_t* data, uint32_t len);  // 0=成功 -1=非录音 -2=溢出丢帧
    void register_event(HANDLE ev);
    bool poll(std::string& out);                  // true=取到一条结果（内部已 free_string）

    // ---- 扩展接口 ----
    std::string version();
    int model_ready();                            // 1=就绪 0=未加载
    int load_models(const std::string& dir_utf8); // 0=成功 -4=失败
    void set_lang_hint(int lang);                 // 0=auto 1=zh 2=en

private:
    HMODULE mod_ = nullptr;
    void* handle_ = nullptr;
    std::string err_;

    void* (*p_create)() = nullptr;
    void (*p_destroy)(void*) = nullptr;
    int32_t (*p_start)(void*) = nullptr;
    int32_t (*p_stop)(void*) = nullptr;
    int32_t (*p_push)(void*, const int16_t*, uint32_t) = nullptr;
    void (*p_register_event)(void*, void*) = nullptr;
    const char* (*p_poll)(void*, uint32_t*) = nullptr;
    void (*p_free_string)(const char*) = nullptr;
    const char* (*p_version)() = nullptr;
    int32_t (*p_model_ready)(void*) = nullptr;
    int32_t (*p_load_models)(void*, const char*) = nullptr;
    void (*p_set_lang_hint)(void*, int32_t) = nullptr;
};
