# POC-06: C++/Rust FFI 基础通讯与 WASAPI 采集验证

- `rust/` — Rust cdylib 核心 `voime_poc_core`：状态机（Idle/Recording/Finalizing）、
  crossbeam ArrayQueue 无锁队列（2048×480 样本 ≈ 61s 护栏）、条件变量 Worker 线程、
  CString 所有权转移、Win32 SetEvent 异步通知。FFI 接口对照 PRD 5.1（8 个导出函数
  + 3 个 POC 测试辅助函数）。
- `cpp/poc_host.cpp` — C++ 宿主：WASAPI 共享模式采集（48k F32）→ 线性重采样 16k s16
  → `voime_push_audio`；事件等待线程拉取结果；内存监控；溢出/拒绝确定性测试；合成正弦源。
- `build.ps1` — 一键构建（Rust + MinGW g++，均在工作区 `.toolchain/`）。

## 构建

```powershell
powershell -ExecutionPolicy Bypass -File poc\ffi\build.ps1
# 产物: poc\ffi\out\poc_host.exe + voime_poc_core.dll
```

## 运行

```powershell
cd poc\ffi\out
.\poc_host.exe --mode synthetic --seconds 15 --verbose     # 合成源 + Rust 逐 chunk 打印长度
.\poc_host.exe --mode wasapi --seconds 15                  # 真实 WASAPI 采集（无设备时自动回退合成源）
.\poc_host.exe --seconds 5 --overflow-test --finalizing-test  # -2 溢出 / -3 Finalizing 拒绝
.\poc_host.exe --mode synthetic --seconds 120              # 泄漏浸泡测试（内存应恒定）
```

## 验证结论

见 `docs/poc/POC_REPORT.md` §3。全部通过。
