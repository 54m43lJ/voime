// register.cpp — TSF 注册/注销（P5-01 COM + Profile + Category；P5-07 安全模式类别）
//
// 权限模型（实测 Windows 10 22H2）：
//   - COM 类（HKCU\Software\Classes\CLSID）：免管理员；UAC 提权进程同用户可见。
//   - ITfInputProcessorProfileMgr::RegisterProfile：写 HKLM\SOFTWARE\Microsoft\CTF\TIP，
//     **必须提权运行**（regsvr32 /s 经 frontend\tsf\register_tsf.ps1 自提权；
//     P6-01 安装程序默认提权）。PRD 5.1 纯 HKCU 注册对 TSF Profile 不可行。
//   - 类别注册含 SECUREMODE（P5-07：提权/安全场景声明）。
#include "tsf_defs.h"

#include <msctf.h>
#include <objbase.h>
#include <string>

static std::wstring module_path() {
    wchar_t b[MAX_PATH]{};
    GetModuleFileNameW(g_hmod, b, MAX_PATH);
    return b;
}

static std::wstring clsid_str() {
    wchar_t b[48]{};
    StringFromGUID2(CLSID_VoimeTextService, b, 48);
    return b;
}

static HRESULT write_com_class() {
    std::wstring base = L"Software\\Classes\\CLSID\\" + clsid_str();
    HKEY h = nullptr;
    if (RegCreateKeyExW(HKEY_CURRENT_USER, base.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr,
                        &h, nullptr) != ERROR_SUCCESS)
        return E_FAIL;
    RegSetValueExW(h, nullptr, 0, REG_SZ, (const BYTE*)VOIME_TIP_DESC_ZH,
                   (DWORD)((wcslen(VOIME_TIP_DESC_ZH) + 1) * sizeof(wchar_t)));
    RegCloseKey(h);

    if (RegCreateKeyExW(HKEY_CURRENT_USER, (base + L"\\InprocServer32").c_str(), 0, nullptr,
                        0, KEY_WRITE, nullptr, &h, nullptr) != ERROR_SUCCESS)
        return E_FAIL;
    std::wstring path = module_path();
    RegSetValueExW(h, nullptr, 0, REG_SZ, (const BYTE*)path.c_str(),
                   (DWORD)((path.size() + 1) * sizeof(wchar_t)));
    RegSetValueExW(h, L"ThreadingModel", 0, REG_SZ, (const BYTE*)L"Apartment",
                   sizeof(L"Apartment"));
    RegCloseKey(h);
    return S_OK;
}

static void delete_com_class() {
    std::wstring base = L"Software\\Classes\\CLSID\\" + clsid_str();
    RegDeleteKeyW(HKEY_CURRENT_USER, (base + L"\\InprocServer32").c_str());
    RegDeleteKeyW(HKEY_CURRENT_USER, base.c_str());
}

HRESULT RegisterVoimeTip() {
    HRESULT hr = write_com_class();
    if (FAILED(hr)) return hr;

    HRESULT ci = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (FAILED(ci) && ci != RPC_E_CHANGED_MODE) return ci;

    ITfInputProcessorProfileMgr* ppm = nullptr;
    hr = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr, CLSCTX_INPROC_SERVER,
                          IID_ITfInputProcessorProfileMgr, (void**)&ppm);
    if (SUCCEEDED(hr)) {
        std::wstring icon = module_path();
        // zh-CN + en-US 双语注册（默认启用）
        HRESULT hr1 = ppm->RegisterProfile(
            CLSID_VoimeTextService, MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED),
            GUID_VoimeProfile, VOIME_TIP_DESC_ZH, (ULONG)wcslen(VOIME_TIP_DESC_ZH),
            icon.c_str(), (ULONG)icon.size(), 0, nullptr, 0, TRUE, 0);
        HRESULT hr2 = ppm->RegisterProfile(
            CLSID_VoimeTextService, MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US),
            GUID_VoimeProfile, VOIME_TIP_DESC_EN, (ULONG)wcslen(VOIME_TIP_DESC_EN),
            icon.c_str(), (ULONG)icon.size(), 0, nullptr, 0, TRUE, 0);
        ppm->Release();
        tsf_log("RegisterProfile zh=0x%08lx en=0x%08lx", hr1, hr2);
        if (FAILED(hr1) && FAILED(hr2)) hr = FAILED(hr1) ? hr1 : hr2;
    }

    // 自愈启用：profile 已存在时 RegisterProfile 不覆盖用户启用状态，
    // 曾卸载/禁用会导致 ActivateProfile E_INVALIDARG —— 显式重新启用。
    // 注意：EnableLanguageProfileByDefault 只写系统默认，不改用户禁用标记，
    // 必须 EnableLanguageProfile（fEnable=TRUE 同时写入 HKCU 启用项）。
    ITfInputProcessorProfiles* ipp = nullptr;
    HRESULT hr_ipp = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr,
                                      CLSCTX_INPROC_SERVER, IID_ITfInputProcessorProfiles,
                                      (void**)&ipp);
    if (SUCCEEDED(hr_ipp)) {
        HRESULT he1 = ipp->EnableLanguageProfile(
            CLSID_VoimeTextService, MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED),
            GUID_VoimeProfile, TRUE);
        HRESULT he2 = ipp->EnableLanguageProfile(
            CLSID_VoimeTextService, MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US),
            GUID_VoimeProfile, TRUE);
        tsf_log("EnableLanguageProfile zh=0x%08lx en=0x%08lx", he1, he2);
        ipp->Release();
    } else {
        tsf_log("ITfInputProcessorProfiles 创建失败 hr=0x%08lx", hr_ipp);
    }

    ITfCategoryMgr* cm = nullptr;
    HRESULT hr_cat = CoCreateInstance(CLSID_TF_CategoryMgr, nullptr, CLSCTX_INPROC_SERVER,
                                      IID_ITfCategoryMgr, (void**)&cm);
    if (SUCCEEDED(hr_cat)) {
        // 键盘类 TIP（语言栏可见）+ 安全模式（P5-07：UAC 提权/安全桌面进程）
        // + IMMERSIVESUPPORT（UWP/商店应用进程可加载）
        cm->RegisterCategory(CLSID_VoimeTextService, GUID_TFCAT_TIP_KEYBOARD,
                             CLSID_VoimeTextService);
        cm->RegisterCategory(CLSID_VoimeTextService, GUID_VoimeTipCapSecureMode,
                             CLSID_VoimeTextService);
        cm->RegisterCategory(CLSID_VoimeTextService, GUID_VoimeTipCapImmersive,
                             CLSID_VoimeTextService);
        cm->Release();
    }
    tsf_log("RegisterVoimeTip profile hr=0x%08lx category hr=0x%08lx", hr, hr_cat);

    if (ci == S_OK) CoUninitialize();
    return SUCCEEDED(hr) ? S_OK : hr;
}

HRESULT UnregisterVoimeTip() {
    HRESULT ci = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (FAILED(ci) && ci != RPC_E_CHANGED_MODE) return ci;

    ITfInputProcessorProfileMgr* ppm = nullptr;
    HRESULT hr = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr,
                                  CLSCTX_INPROC_SERVER, IID_ITfInputProcessorProfileMgr,
                                  (void**)&ppm);
    if (SUCCEEDED(hr)) {
        ppm->UnregisterProfile(CLSID_VoimeTextService,
                               MAKELANGID(LANG_CHINESE, SUBLANG_CHINESE_SIMPLIFIED),
                               GUID_VoimeProfile, 0);
        ppm->UnregisterProfile(CLSID_VoimeTextService,
                               MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US),
                               GUID_VoimeProfile, 0);
        ppm->Release();
    }

    ITfCategoryMgr* cm = nullptr;
    if (SUCCEEDED(CoCreateInstance(CLSID_TF_CategoryMgr, nullptr, CLSCTX_INPROC_SERVER,
                                   IID_ITfCategoryMgr, (void**)&cm))) {
        cm->UnregisterCategory(CLSID_VoimeTextService, GUID_TFCAT_TIP_KEYBOARD,
                               CLSID_VoimeTextService);
        cm->UnregisterCategory(CLSID_VoimeTextService, GUID_VoimeTipCapSecureMode,
                               CLSID_VoimeTextService);
        cm->UnregisterCategory(CLSID_VoimeTextService, GUID_VoimeTipCapImmersive,
                               CLSID_VoimeTextService);
        cm->Release();
    }

    delete_com_class();
    if (ci == S_OK) CoUninitialize();
    return S_OK;
}
