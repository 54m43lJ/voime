#include "globals.h"
#include "ime_service.h"

// Reference count for the DLL
static LONG g_dllRefCount = 0;

// Class factory for VoimeTextService
class VoimeClassFactory : public IClassFactory {
public:
    // IUnknown
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_IClassFactory)) {
            *ppv = static_cast<IClassFactory*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    STDMETHODIMP_(ULONG) AddRef() override {
        return InterlockedIncrement(&m_refCount);
    }

    STDMETHODIMP_(ULONG) Release() override {
        LONG c = InterlockedDecrement(&m_refCount);
        if (c == 0) delete this;
        return c;
    }

    // IClassFactory
    STDMETHODIMP CreateInstance(IUnknown* pUnkOuter, REFIID riid, void** ppv) override {
        if (pUnkOuter) return CLASS_E_NOAGGREGATION;

        auto* svc = new VoimeTextService();
        if (!svc) return E_OUTOFMEMORY;

        HRESULT hr = svc->QueryInterface(riid, ppv);
        svc->Release();
        return hr;
    }

    STDMETHODIMP LockServer(BOOL fLock) override {
        if (fLock)
            InterlockedIncrement(&g_dllRefCount);
        else
            InterlockedDecrement(&g_dllRefCount);
        return S_OK;
    }

private:
    LONG m_refCount = 1;
};


// DLL exports
STDAPI DllGetClassObject(REFCLSID rclsid, REFIID riid, void** ppv) {
    if (!IsEqualCLSID(rclsid, CLSID_VoimeTextService))
        return CLASS_E_CLASSNOTAVAILABLE;

    auto* factory = new VoimeClassFactory();
    if (!factory) return E_OUTOFMEMORY;

    HRESULT hr = factory->QueryInterface(riid, ppv);
    factory->Release();
    return hr;
}

STDAPI DllCanUnloadNow() {
    return g_dllRefCount == 0 ? S_OK : S_FALSE;
}

STDAPI DllRegisterServer() {
    return RegisterTextService();
}

STDAPI DllUnregisterServer() {
    return UnregisterTextService();
}


BOOL APIENTRY DllMain(HMODULE hModule, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(hModule);
    }
    return TRUE;
}
