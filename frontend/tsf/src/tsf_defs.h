// tsf_defs.h — Voime TSF 文本服务公共定义（P5-01）
#pragma once
#include <windows.h>

// COM/Profile GUID（注册后即固定，变更需同步卸载/安装逻辑）
extern "C" {
extern const CLSID CLSID_VoimeTextService; // {C7E4B2A1-5F63-4D89-9B0E-3A2D8C1F6E54}
extern const GUID  GUID_VoimeProfile;      // {1F6E54C7-4B2A-4D89-A5F6-3A2D8C1F9B0E}
// MinGW msctf.h 缺失的安全模式类别（P5-07：UAC 提权应用加载 TIP 所需）
extern const GUID  GUID_VoimeTipCapSecureMode; // = GUID_TFCAT_TIPCAP_SECUREMODE
// UWP/immersive 进程加载 TIP 所需（MinGW 头文件缺失）
extern const GUID  GUID_VoimeTipCapImmersive; // = GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT
}

#define VOIME_TIP_DESC_ZH L"Voime 语音输入"
#define VOIME_TIP_DESC_EN L"Voime Voice Input"

// 隐藏消息窗口消息：IPC commit_text 从 reader 线程投递到 UI 线程执行注入
#define WM_VOIME_COMMIT (WM_APP + 41)

// dllmain.cpp 提供
extern HMODULE g_hmod;
extern LONG g_dll_locks;
void tsf_log(const char* fmt, ...); // %APPDATA%\VoimeInput\voime_tsf.log（带 pid 区分宿主）
