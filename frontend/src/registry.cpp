#include "registry.h"
#include "globals.h"

#include <stdio.h>

// Helper: get the full path of this DLL
static bool GetDllPath(wchar_t* buf, DWORD size) {
    HMODULE hm = nullptr;
    if (!GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            (LPCWSTR)&GetDllPath, &hm))
        return false;
    GetModuleFileNameW(hm, buf, size);
    return true;
}

// Helper: set registry string value
static LONG SetRegStr(HKEY hKey, const wchar_t* name, const wchar_t* value) {
    return RegSetValueExW(hKey, name, 0, REG_SZ,
        (const BYTE*)value, (DWORD)((wcslen(value) + 1) * sizeof(wchar_t)));
}

// Register CLSID for the text service
static HRESULT RegisterCLSID() {
    wchar_t dllPath[MAX_PATH];
    if (!GetDllPath(dllPath, MAX_PATH)) return E_FAIL;

    const wchar_t* clsidStr = L"{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}";
    wchar_t keyPath[256];
    swprintf(keyPath, 256, L"CLSID\\%s", clsidStr);

    HKEY hKey;
    LONG lr = RegCreateKeyExW(HKEY_LOCAL_MACHINE, keyPath,
        0, nullptr, 0, KEY_WRITE, nullptr, &hKey, nullptr);
    if (lr != ERROR_SUCCESS) return E_FAIL;

    RegSetValueExW(hKey, nullptr, 0, REG_SZ,
        (const BYTE*)L"Voime Text Service", 38);

    HKEY hInProc;
    lr = RegCreateKeyExW(hKey, L"InProcServer32",
        0, nullptr, 0, KEY_WRITE, nullptr, &hInProc, nullptr);
    if (lr == ERROR_SUCCESS) {
        SetRegStr(hInProc, nullptr, dllPath);
        SetRegStr(hInProc, L"ThreadingModel", L"Apartment");
        RegCloseKey(hInProc);
    }
    RegCloseKey(hKey);
    return S_OK;
}

// Unregister CLSID
static HRESULT UnregisterCLSID() {
    const wchar_t* clsidStr = L"{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}";
    wchar_t keyPath[256];
    swprintf(keyPath, 256, L"CLSID\\%s", clsidStr);

    RegDeleteTreeW(HKEY_LOCAL_MACHINE, keyPath);
    return S_OK;
}

// Register TSF text service profile
static HRESULT RegisterProfile() {
    ITfInputProcessorProfileMgr* pProfileMgr = nullptr;
    HRESULT hr = CoCreateInstance(CLSID_TF_InputProcessorProfiles,
        nullptr, CLSCTX_INPROC_SERVER, IID_ITfInputProcessorProfileMgr,
        (void**)&pProfileMgr);
    if (FAILED(hr)) return hr;

    hr = pProfileMgr->RegisterProfile(
        CLSID_VoimeTextService,
        0x0804,                             // LangID: zh-CN
        GUID_VoimeProfile,
        L"Voime",                           // shown in language bar
        (ULONG)wcslen(L"Voime"),
        L"语音输入法",                        // description
        (ULONG)wcslen(L"语音输入法"),
        nullptr, 0,                         // icon
        nullptr, 0,                         // hkl
        0,                                  // flags
        TRUE                                 // enabled
    );

    pProfileMgr->Release();
    return hr;
}

static HRESULT UnregisterProfile() {
    ITfInputProcessorProfileMgr* pProfileMgr = nullptr;
    HRESULT hr = CoCreateInstance(CLSID_TF_InputProcessorProfiles,
        nullptr, CLSCTX_INPROC_SERVER, IID_ITfInputProcessorProfileMgr,
        (void**)&pProfileMgr);
    if (FAILED(hr)) return hr;

    hr = pProfileMgr->UnregisterProfile(
        CLSID_VoimeTextService, 0x0804, GUID_VoimeProfile, 0);
    pProfileMgr->Release();
    return hr;
}

HRESULT RegisterTextService() {
    HRESULT hr = RegisterCLSID();
    if (FAILED(hr)) return hr;
    hr = CoInitialize(nullptr);
    if (SUCCEEDED(hr)) {
        hr = RegisterProfile();
        CoUninitialize();
    }
    return hr;
}

HRESULT UnregisterTextService() {
    HRESULT hr = CoInitialize(nullptr);
    if (SUCCEEDED(hr)) {
        UnregisterProfile();
        CoUninitialize();
    }
    UnregisterCLSID();
    return S_OK;
}
