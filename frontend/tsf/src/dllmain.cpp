// dllmain.cpp — voime_tsf.dll COM 入口（P5-01）
#include "tsf_defs.h"
#include "text_service.h"

#include <cstdarg>
#include <cstdio>
#include <new>
#include <string>

HMODULE g_hmod = nullptr;
LONG g_dll_locks = 0;

void tsf_log(const char* fmt, ...) {
    wchar_t dir[MAX_PATH];
    DWORD n = GetEnvironmentVariableW(L"APPDATA", dir, MAX_PATH);
    if (!n || n >= MAX_PATH) return;
    std::wstring path = std::wstring(dir) + L"\\VoimeInput";
    CreateDirectoryW(path.c_str(), nullptr);
    path += L"\\voime_tsf.log";
    FILE* fp = _wfopen(path.c_str(), L"ab");
    if (!fp) return;
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    SYSTEMTIME st{};
    GetLocalTime(&st);
    fprintf(fp, "[%02d:%02d:%02d.%03d pid=%lu tid=%lu] %s\n", st.wHour, st.wMinute,
            st.wSecond, st.wMilliseconds, GetCurrentProcessId(), GetCurrentThreadId(), buf);
    fclose(fp);
    OutputDebugStringA("[voime_tsf] ");
    OutputDebugStringA(buf);
    OutputDebugStringA("\n");
}

// register.cpp
HRESULT RegisterVoimeTip();
HRESULT UnregisterVoimeTip();

extern "C" BOOL WINAPI DllMain(HINSTANCE h, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_hmod = (HMODULE)h;
        DisableThreadLibraryCalls(h);
    }
    return TRUE;
}

class CVoimeClassFactory : public IClassFactory {
public:
    virtual ~CVoimeClassFactory() = default;
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) {
        if (!ppv) return E_POINTER;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_IClassFactory)) {
            *ppv = (IClassFactory*)this;
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() { return (ULONG)InterlockedIncrement(&ref_); }
    STDMETHODIMP_(ULONG) Release() {
        ULONG r = (ULONG)InterlockedDecrement(&ref_);
        if (!r) delete this;
        return r;
    }
    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** ppv) {
        if (!ppv) return E_POINTER;
        *ppv = nullptr;
        if (outer) return CLASS_E_NOAGGREGATION;
        CVoimeTextService* ts = new (std::nothrow) CVoimeTextService();
        if (!ts) return E_OUTOFMEMORY;
        HRESULT hr = ts->QueryInterface(riid, ppv);
        ts->Release();
        return hr;
    }
    STDMETHODIMP LockServer(BOOL lock) {
        if (lock) InterlockedIncrement(&g_dll_locks);
        else InterlockedDecrement(&g_dll_locks);
        return S_OK;
    }

private:
    LONG ref_ = 1;
};

extern "C" HRESULT STDAPICALLTYPE DllGetClassObject(REFCLSID rclsid, REFIID riid, LPVOID* ppv) {
    if (IsEqualCLSID(rclsid, CLSID_VoimeTextService)) {
        CVoimeClassFactory* f = new (std::nothrow) CVoimeClassFactory();
        if (!f) return E_OUTOFMEMORY;
        HRESULT hr = f->QueryInterface(riid, ppv);
        f->Release();
        return hr;
    }
    return CLASS_E_CLASSNOTAVAILABLE;
}

extern "C" HRESULT STDAPICALLTYPE DllCanUnloadNow() {
    return g_dll_locks == 0 ? S_OK : S_FALSE;
}

extern "C" HRESULT STDAPICALLTYPE DllRegisterServer() {
    HRESULT hr = RegisterVoimeTip();
    tsf_log("DllRegisterServer hr=0x%08lx", hr);
    return hr;
}

extern "C" HRESULT STDAPICALLTYPE DllUnregisterServer() {
    HRESULT hr = UnregisterVoimeTip();
    tsf_log("DllUnregisterServer hr=0x%08lx", hr);
    return hr;
}
