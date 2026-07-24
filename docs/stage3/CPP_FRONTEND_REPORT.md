# Voime 阶段三 C++ 前端报告（M3 交付物）

- 日期：2026-07-24
- 分支：voime-ng-stage3
- 交付：`frontend/`（`build/voime_input.exe` GUI 版 + `voime_input_console.exe` 自测版，
  随包 FFmpeg DLL：swresample-4.dll / avutil-58.dll）

## 0. 任务验收汇总

| 任务 | 验收标准 | 结果 | 结论 |
|---|---|---|---|
| CPP-01 | 搭建 C++ 项目，集成 Rust DLL，打印 Hello World | MinGW g++ 双产物构建（`frontend/build.ps1`）；FFI 动态加载打印 `voime_core 0.2.0` | ✅ |
| CPP-02 | WASAPI 共享模式采集 48k F32 | 事件驱动 + MMCSS Pro Audio 线程；实采 `48000Hz 2ch 32bit float`，3s 整 100 chunk / 48000 采样 | ✅ |
| CPP-03 | FFmpeg 重采样 48k F32 → 16k PCM | libswresample（延迟加载）；440Hz 正弦实测 439.5Hz、15984 采样/秒、峰值 16384（理论 16383） | ✅ |
| CPP-04 | 启动检测 DLL，缺失可下载修复 | 自检（存在性+加载+版本 swr 4.10.100↔FFmpeg 6.0）；移走 DLL → 检出缺失 → WinHTTP 下载 38MB → tar 解压覆盖 → 复检通过 | ✅ |
| CPP-05 | WH_KEYBOARD_LL 捕获 PTT，超时自动重注册 | 钩子捕获 Ctrl+Space press/release=1/1（抑制自动重复）；5s 守护节拍 + 60s 预防性重装 | ✅ |
| CPP-06 | Direct2D 无边框透明置顶悬浮窗 | 逐像素半透明（DIB+DCRenderTarget+UpdateLayeredWindow）、置顶/不抢焦点/点击穿透、底部居中、淡入淡出 | ✅ |
| CPP-07 | 主线程捕获 Event，poll_results 绘制文本 | MsgWaitForMultipleObjectsEx 单线程等待引擎事件+消息；wav E2E 收 3 partial + final（带标点） | ✅ |
| CPP-08 | SendInput 逐字符注入记事本，无乱码 | KEYEVENTF_UNICODE 批量注入；EDIT 控件实收 `你好世界Hello123。` 逐字一致 | ✅ |

**总体结论：阶段三全部任务通过（自测 15/15 PASS + 修复专项 3/3 PASS + 真机 PTT 流程验证）。**

真机 PTT 流程（模拟按键驱动真实应用）：`PTT 按下 → 开始录音(48000Hz 2ch 32bit float)
→ 松开 → Finalizing → final 结果回报`，3s 静默会话 final 正常收尾（text 空，未注入，符合预期）。

## 1. 架构与模块

```
frontend/
├── build.ps1              # MinGW g++ 双产物构建 + 拷贝运行时 DLL
└── src/
    ├── main.cpp           # 入口分发（GUI / --selftest / --repair）
    ├── app.{h,cpp}        # 编排：消息循环(MsgWaitForMultipleObjectsEx)、PTT 流程、
    │                      #   结果路由、托盘（最小：图标+退出）、依赖自检提示
    ├── engine.{h,cpp}     # voime_core.dll 延迟加载封装（voime.h 12 导出）
    ├── audio_capture.{h,cpp}  # WASAPI 共享模式事件采集线程（MMCSS），静默包补零
    ├── resampler.{h,cpp}  # FFmpeg swresample 延迟加载（F32/S16 任意率 → 16k s16 mono）
    ├── deps.{h,cpp}       # FFmpeg DLL 自检 + WinHTTP 下载 + tar.exe 解压修复
    ├── hotkey.{h,cpp}     # WH_KEYBOARD_LL + 修饰键精确匹配 + 守护重装
    ├── overlay.{h,cpp}    # D2D/DWrite 逐像素半透明悬浮窗（圆角、截头滚动、淡入淡出）
    ├── injector.{h,cpp}   # SendInput KEYEVENTF_UNICODE 分批注入
    ├── config.{h,cpp}     # TOML 子集解析，缺失写默认（PRD 5.2）
    ├── selftest.{h,cpp}   # 15 项自动化验收 + 修复专项
    └── util.h             # UTF-8/16 转换、exe 目录、JSON 字段提取
```

关键设计（对照 PRD）：

1. **PRD 3.3 并发模型**：采集线程内完成重采样（FFmpeg），480 采样 chunk 经
   `voime_push_audio` 微秒级入队；主线程单点 `MsgWaitForMultipleObjectsEx`
   同时等待引擎事件与窗口消息，事件触发即 poll 到底，无轮询线程。
2. **PRD 2.2 依赖自愈**：swresample 延迟加载（LoadLibrary），缺 DLL 进程仍可启动；
   自检失败弹 PRD 原文提示“未检测到音频组件或组件版本冲突，是否尝试重新下载正确的依赖？”，
   选“是”下载 gyan.dev 官方 6.0 包修复并提示重启，选“否”禁用录入但可进主界面。
3. **PRD 4.2 热键守护**：回调仅 PostMessage（远离 LowLevelHooksTimeout）；
   5s 节拍检查 + 每 60s 预防性重装（重装后复位 pressed 防卡键）。
4. **PRD 2.3 悬浮窗**：半透明圆角（62% 黑）、状态行（绿）+ 内容行（白 20pt），
   超长截头“…”模拟滚动；完成后 900ms 淡出。

## 2. 自测覆盖（`voime_input_console.exe --selftest`）

| 用例 | 断言 | 实测 |
|---|---|---|
| FFI | 版本打印/实例创建 | voime_core 0.2.0 |
| 依赖自检 | DLL 存在+加载+版本 | libswresample 4.10.100 (FFmpeg 6.0) |
| 重采样 | 长度/频率/幅度 3 项 | 15984 采样、439.5Hz、16384 |
| 引擎 E2E | wav 回放收 partial+final | 3 partial + `你好世界，你好世界，你好世界。` |
| 热键 | SendInput 触发钩子 | press=1 release=1 |
| 悬浮窗 | 样式/可见/隐藏 | TOPMOST+LAYERED+NOACTIVATE+TOOLWINDOW |
| 注入 | EDIT 控件实收比对 | 逐字一致（含中文与句号） |
| WASAPI | 3s 实采 | 100 chunk / 48000 采样（±0） |

修复专项（`--repair`，真实下载）：检出缺失 → 下载 38MB → 解压覆盖 → 复检通过（3/3）。

## 3. 运行方式

```powershell
# 构建（产物 frontend/build/）
powershell -ExecutionPolicy Bypass -File frontend\build.ps1

# 自动化验收（需模型）
$env:VOIME_MODEL_DIR = "$PWD\models"
.\frontend\build\voime_input_console.exe --selftest --wav "$PWD\poc\python\out\session_zh_3sent.wav"
.\frontend\build\voime_input_console.exe --repair   # 依赖修复专项（下载 ~38MB）

# 正常运行（托盘 + Ctrl+Space PTT）
$env:VOIME_MODEL_DIR = "$PWD\models"
.\frontend\build\voime_input.exe
```

- 配置：`%APPDATA%\VoimeInput\config.toml`（首启自动写默认，PRD 5.2），日志 `voime.log` 同目录。
- 运行时 DLL（随 exe 同目录）：voime_core.dll、onnxruntime.dll、onnxruntime_providers_shared.dll、
  swresample-4.dll、avutil-58.dll（PRD 2.2：含 FFmpeg、不含模型）。

## 4. 缺陷修复记录（阶段三）

| # | 问题 | 修复 |
|---|---|---|
| 1 | `swr_get_version` 非导出符号（FFmpeg 版本函数名是 `swresample_version`） | 更正符号名，自检通过 |
| 2 | MinGW 链接 `wWinMain` 报 undefined WinMain | 构建加 `-municode` |
| 3 | console 子系统缺 GDI 导入（-mwindows 隐式带 -lgdi32，-mconsole 不带） | 显式 `-lgdi32 -lcomdlg32` |
| 4 | Python 测试驱动 INPUT 结构 40B 对齐错误致 SendInput 静默失败 | 测试驱动修正 union 尺寸（非产品代码） |
| 5 | gyan.dev 已清理 6.0 历史包（404） | 改用其 GitHub 镜像 `GyanD/codexffmpeg` release（同源官方构建） |

## 5. 已知限制与阶段四事项

| # | 事项 | 说明 |
|---|---|---|
| 1 | UAC 提权窗口无法注入 | PRD 2.4 既有 MVP 限制，TSF 演进（阶段五）解决 |
| 2 | 设置窗口未实现 | PRD 2.1 功能项，列入阶段四产品化（当前仅最小托盘：图标+退出） |
| 3 | 模型下载向导未实现 | QA-03/M4 范围；当前缺失时弹路径提示（支持 VOIME_MODEL_DIR） |
| 4 | 悬浮窗视觉仅冒烟验证 | 像素级外观/多显示器适配留 QA-01 人工验收 |
| 5 | FFmpeg 包为 GPL full 构建 | 仅用 swresample/avutil（LGPL 组件）；产品化可换 LGPL 构建瘦身 |
| 6 | >2 声道设备走 UNSPEC 布局 | 罕见麦克风阵列混音系数按 swr 默认；1/2 声道走标准掩码 |
