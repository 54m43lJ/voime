// engine.cpp — voime_core.dll 动态加载封装实现
#include "engine.h"

Engine::~Engine() { destroy(); }

#define RESOLVE(field, name)                                     \
    do {                                                         \
        field = (decltype(field))GetProcAddress(mod_, name);     \
        if (!field) { err_ = "缺少导出符号: " name; return false; } \
    } while (0)

bool Engine::load(const wchar_t* dll_path) {
    mod_ = LoadLibraryExW(dll_path, nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!mod_) {
        err_ = "LoadLibrary 失败, GetLastError=" + std::to_string(GetLastError());
        return false;
    }
    RESOLVE(p_create, "voime_create_engine");
    RESOLVE(p_destroy, "voime_destroy_engine");
    RESOLVE(p_start, "voime_start_session");
    RESOLVE(p_stop, "voime_stop_session");
    RESOLVE(p_push, "voime_push_audio");
    RESOLVE(p_register_event, "voime_register_event");
    RESOLVE(p_poll, "voime_poll_result");
    RESOLVE(p_free_string, "voime_free_string");
    RESOLVE(p_version, "voime_version");
    RESOLVE(p_model_ready, "voime_model_ready");
    RESOLVE(p_load_models, "voime_load_models");
    RESOLVE(p_set_lang_hint, "voime_set_lang_hint");
    return true;
}
#undef RESOLVE

bool Engine::create() {
    if (!loaded() || handle_) return handle_ != nullptr;
    handle_ = p_create();
    if (!handle_) err_ = "voime_create_engine 返回 NULL";
    return handle_ != nullptr;
}

void Engine::destroy() {
    if (handle_ && p_destroy) p_destroy(handle_);
    handle_ = nullptr;
}

int Engine::start() { return handle_ ? p_start(handle_) : -100; }
int Engine::stop() { return handle_ ? p_stop(handle_) : -100; }
int Engine::push(const int16_t* data, uint32_t len) {
    return handle_ ? p_push(handle_, data, len) : -100;
}
void Engine::register_event(HANDLE ev) {
    if (handle_) p_register_event(handle_, ev);
}

bool Engine::poll(std::string& out) {
    if (!handle_) return false;
    uint32_t len = 0;
    const char* s = p_poll(handle_, &len);
    if (!s) return false;
    out.assign(s, len);
    p_free_string(s);
    return true;
}

std::string Engine::version() {
    if (!loaded()) return "(dll 未加载)";
    const char* v = p_version();
    return v ? v : "(null)";
}

int Engine::model_ready() { return handle_ ? p_model_ready(handle_) : 0; }
int Engine::load_models(const std::string& dir_utf8) {
    return handle_ ? p_load_models(handle_, dir_utf8.c_str()) : -4;
}
void Engine::set_lang_hint(int lang) {
    if (handle_) p_set_lang_hint(handle_, lang);
}
